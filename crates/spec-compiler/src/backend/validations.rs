//! Validations (`validations` section) to the `validations` module.
//!
//! A validation on a contract input becomes part of `validate(&self)` on the contract's request
//! type; one on a record field becomes part of `validate_rules(&self)` on the record (the record's
//! own `validate` checks its field constraints). Both return the first rule that fails, as a
//! `ValidationError` that carries the validation's id and message.
//!
//! An absent optional value passes every rule except `required`. `required` means present, and
//! for text, bytes and lists not empty. `pattern` must match the whole value. A `custom` rule
//! whose expression overflows fails. Lengths count what `unit` says.

use proc_macro2::{Literal, TokenStream};
use quote::quote;
use serde_json::{Number, Value};
use strata_spec_expr::Binding;

use super::view::{self, Class, Entry, ExprScope, Leaf};
use super::Backend;
use crate::constructs::{Rule, Unit, Validation};
use crate::diagnostic::Diagnostic;
use crate::ir::Spec;
use crate::naming;
use crate::render::{self, Namespace, Scope, Uses};
use crate::resolve::{self, Builtin, TypeRef};

/// Code of a validation that does not name exactly one target.
pub const TARGET: &str = "specc.validation-target";
/// Code of a validation whose contract, input, record or field is not declared.
pub const TARGET_UNKNOWN: &str = "specc.validation-target-unknown";
/// Code of a rule that does not fit its target's type.
pub const TARGET_TYPE: &str = "specc.validation-target-type";
/// Code of a rule parameter that is missing or invalid.
pub const PARAM: &str = "specc.validation-param";
/// Code of a pattern that is not a regular expression.
pub const PATTERN: &str = "specc.validation-pattern";
/// Code of a `oneOf` value the target's type cannot hold.
pub const VALUES: &str = "specc.validation-values";
/// Code of a `custom` expression that does not translate.
pub const EXPRESSION: &str = "specc.validation-expression";
/// Code of a `maxLength` without a unit.
pub const UNIT_MISSING: &str = "ready.validation-unit-missing";

/// The validations backend.
pub struct ValidationsBackend;

/// What a validation is about.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Input { contract: String, input: String },
    Field { record: String, field: String },
}

fn target_of(v: &Validation) -> Option<Target> {
    match (&v.contract, &v.input, &v.record, &v.field) {
        (Some(contract), Some(input), None, None) => Some(Target::Input {
            contract: contract.clone(),
            input: input.clone(),
        }),
        (None, None, Some(record), Some(field)) => Some(Target::Field {
            record: record.clone(),
            field: field.clone(),
        }),
        _ => None,
    }
}

/// One validation, ready to emit.
struct Compiled {
    /// The helper functions: the check and, for a custom rule, its expression.
    functions: TokenStream,
    /// The statement that runs the check from `validate`, or nothing when no check is needed.
    call: Option<TokenStream>,
    /// The type the check is a method of.
    owner: TokenStream,
    /// The key that groups validations of one owner.
    owner_key: String,
    /// True for a record, whose method is `validate_rules`.
    record: bool,
}

type Failure = (&'static str, String);

fn fail(code: &'static str, message: String) -> Failure {
    (code, message)
}

fn number_as_integer(n: &Number) -> Option<i128> {
    if let Some(i) = n.as_i64() {
        return Some(i128::from(i));
    }
    if let Some(u) = n.as_u64() {
        return Some(i128::from(u));
    }
    let f = n.as_f64()?;
    (f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0).then_some(f as i128)
}

fn integer_tokens(v: i128) -> TokenStream {
    let lit = Literal::u128_unsuffixed(v.unsigned_abs());
    if v < 0 {
        quote! { -#lit }
    } else {
        quote! { #lit }
    }
}

fn float_tokens(f: f64) -> TokenStream {
    let lit = Literal::f64_unsuffixed(f.abs());
    if f < 0.0 {
        quote! { -#lit }
    } else {
        quote! { #lit }
    }
}

/// Wraps `inner` in the `if let Some(value)` block that reaches a value inside an optional
/// `place`. `inner` receives the place of the value (a copy of it when `copy`, else a reference)
/// and a reference to it. Without an option the place is the field itself.
fn peeled(
    place: &TokenStream,
    options: usize,
    copy: bool,
    inner: &dyn Fn(TokenStream, TokenStream) -> TokenStream,
) -> TokenStream {
    if options == 0 {
        return inner(place.clone(), quote! { &#place });
    }
    let (pattern, reference) = if copy {
        (quote! { Some(&value) }, quote! { &value })
    } else {
        (quote! { Some(value) }, quote! { value })
    };
    let body = inner(quote! { value }, reference);
    if body.is_empty() {
        return body;
    }
    quote! {
        if let #pattern = &#place {
            #body
        }
    }
}

fn compile(
    spec: &Spec,
    v: &Validation,
    uses: &mut Uses,
    statics: &mut Vec<TokenStream>,
) -> Result<Compiled, Failure> {
    let target = target_of(v).ok_or_else(|| {
        fail(
            TARGET,
            format!(
                "Validation \"{}\" must name a contract with an input, or a record with a field, and nothing else.",
                v.id
            ),
        )
    })?;
    let (base, type_name, optional, owner, owner_key, record) = match &target {
        Target::Input { contract, input } => {
            let c = spec.contracts.iter().find(|c| &c.id == contract).ok_or_else(|| {
                fail(
                    TARGET_UNKNOWN,
                    format!("Validation \"{}\" is about contract \"{contract}\", which is not declared.", v.id),
                )
            })?;
            let f = c.inputs.iter().find(|f| &f.name == input).ok_or_else(|| {
                fail(
                    TARGET_UNKNOWN,
                    format!(
                        "Contract \"{contract}\" has no input \"{input}\" (validation \"{}\").",
                        v.id
                    ),
                )
            })?;
            let ty = f.type_name.clone().unwrap_or_default();
            (
                (quote! { request }, view::snake_ident(input)),
                ty,
                f.optional,
                view::request_path(contract),
                format!("contract {contract}"),
                false,
            )
        }
        Target::Field { record, field } => {
            let r = spec.record(record).ok_or_else(|| {
                fail(
                    TARGET_UNKNOWN,
                    format!(
                        "Validation \"{}\" is about record \"{record}\", which is not declared.",
                        v.id
                    ),
                )
            })?;
            let f = r.field(field).ok_or_else(|| {
                fail(
                    TARGET_UNKNOWN,
                    format!(
                        "Record \"{record}\" has no field \"{field}\" (validation \"{}\").",
                        v.id
                    ),
                )
            })?;
            (
                (quote! { record }, view::snake_ident(field)),
                f.type_name.clone(),
                f.optional,
                view::record_path(record),
                format!("record {record}"),
                true,
            )
        }
    };
    if !resolve::resolves(spec, &type_name) {
        return Err(fail(
            TARGET_TYPE,
            format!(
                "The target of validation \"{}\" has {}.",
                v.id,
                resolve::unresolved_text(Some(&type_name))
            ),
        ));
    }
    let full = {
        let t = TypeRef::parse(&type_name);
        if optional {
            TypeRef::Option(Box::new(t))
        } else {
            t
        }
    };
    let (inner_type, options) = view::peel_options(&full);
    if options > 1 {
        return Err(fail(
            TARGET_TYPE,
            format!(
                "The target of validation \"{}\" is an option of an option.",
                v.id
            ),
        ));
    }
    let leaf: Option<Leaf> = view::leaf(spec, inner_type);
    let is_list = matches!(inner_type, TypeRef::List(_));
    let (head, field_ident) = base;
    let place = quote! { #head.#field_ident };
    let copy = leaf.as_ref().is_some_and(|l| {
        l.layers_empty()
            && matches!(
                l.base,
                Builtin::Bool
                    | Builtin::U8
                    | Builtin::U16
                    | Builtin::U32
                    | Builtin::U64
                    | Builtin::I32
                    | Builtin::I64
                    | Builtin::F64
            )
    });

    let error = view::upper_ident(&v.id);
    let refuse = quote! { return Err(#error); };
    let check_name = naming::ident(&format!(
        "check_{}",
        naming::snake(&v.id).unwrap_or_default()
    ));
    let mut extra = TokenStream::new();

    let kind =
        |what: &str| -> String { format!("The rule {what} of validation \"{}\" needs", v.id) };

    let body: TokenStream = match v.rule {
        Rule::Required => {
            let collection = |p: TokenStream| -> TokenStream {
                if is_list {
                    quote! { if #p.is_empty() { #refuse } }
                } else if let Some(l) = leaf
                    .as_ref()
                    .filter(|l| l.buildable() && matches!(l.base, Builtin::String | Builtin::Bytes))
                {
                    let read = l.read(p);
                    quote! { if #read.is_empty() { #refuse } }
                } else {
                    TokenStream::new()
                }
            };
            if options == 0 {
                collection(place.clone())
            } else {
                let rest = peeled(&place, options, copy, &|p, _| collection(p));
                quote! {
                    if #place.is_none() { #refuse }
                    #rest
                }
            }
        }
        Rule::MaxLength => {
            let n = v
                .max_length
                .as_ref()
                .and_then(Number::as_u64)
                .filter(|n| *n >= 1)
                .ok_or_else(|| {
                    fail(
                        PARAM,
                        format!("{} \"maxLength\", a positive integer.", kind("maxLength")),
                    )
                })?;
            let unit = v.unit.ok_or_else(|| {
                fail(
                    UNIT_MISSING,
                    format!(
                        "Validation \"{}\" limits a length but does not say whether it counts characters or bytes (\"unit\").",
                        v.id
                    ),
                )
            })?;
            let lit = Literal::u64_unsuffixed(n);
            let lengthed = is_list
                || leaf
                    .as_ref()
                    .is_some_and(|l| matches!(l.class, Class::Text | Class::Bytes));
            if !lengthed {
                return Err(fail(
                    TARGET_TYPE,
                    format!(
                        "{} a string, bytes or a list; the target is {type_name}.",
                        kind("maxLength")
                    ),
                ));
            }
            peeled(&place, options, copy, &|p, _| {
                if is_list {
                    return quote! { if #p.len() > #lit { #refuse } };
                }
                let Some(l) = &leaf else {
                    return TokenStream::new();
                };
                match (l.class, unit) {
                    (Class::Text, Unit::Characters) => {
                        let text = l.as_str(p);
                        quote! { if #text.chars().count() > #lit { #refuse } }
                    }
                    (Class::Text, Unit::Bytes) => {
                        let text = l.as_str(p);
                        quote! { if #text.len() > #lit { #refuse } }
                    }
                    _ => {
                        let read = l.read(p);
                        quote! { if #read.len() > #lit { #refuse } }
                    }
                }
            })
        }
        Rule::Pattern => {
            let pattern = v.pattern.as_deref().ok_or_else(|| {
                fail(
                    PARAM,
                    format!("{} \"pattern\", a regular expression.", kind("pattern")),
                )
            })?;
            let Some(l) = leaf.as_ref().filter(|l| l.class == Class::Text) else {
                return Err(fail(
                    TARGET_TYPE,
                    format!("{} a string; the target is {type_name}.", kind("pattern")),
                ));
            };
            let anchored = format!("\\A(?:{pattern})\\z");
            if let Err(e) = regex::Regex::new(&anchored) {
                let reason = e
                    .to_string()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                return Err(fail(
                    PATTERN,
                    format!("The pattern of validation \"{}\" is not a valid regular expression: {reason}", v.id),
                ));
            }
            let name = naming::ident(&format!(
                "{}_PATTERN",
                naming::upper_snake(&v.id).unwrap_or_default()
            ));
            let doc = format!(
                " The pattern of validation `{}`, matched against the whole value.",
                v.id
            );
            statics.push(quote! {
                #[doc = #doc]
                static #name: LazyLock<Regex> =
                    LazyLock::new(|| Regex::new(#anchored).expect("specc checked that the pattern compiles"));
            });
            peeled(&place, options, copy, &|p, _| {
                let text = l.as_str(p);
                quote! { if !#name.is_match(#text) { #refuse } }
            })
        }
        Rule::Range => {
            if v.min.is_none() && v.max.is_none() {
                return Err(fail(
                    PARAM,
                    format!("{} \"min\", \"max\" or both.", kind("range")),
                ));
            }
            let Some(l) = leaf.as_ref().filter(|l| l.class == Class::Number) else {
                return Err(fail(
                    TARGET_TYPE,
                    format!("{} a number; the target is {type_name}.", kind("range")),
                ));
            };
            if let (Some(min), Some(max)) = (&v.min, &v.max) {
                if min.as_f64() > max.as_f64() {
                    return Err(fail(
                        PARAM,
                        format!("The range of validation \"{}\" is empty: min {min} is greater than max {max}.", v.id),
                    ));
                }
            }
            let float = l.base == Builtin::F64;
            let bound = |n: &Number,
                         p: &TokenStream,
                         op: TokenStream|
             -> Result<TokenStream, Failure> {
                let read = l.read(p.clone());
                if float {
                    let f = n.as_f64().filter(|f| f.is_finite()).ok_or_else(|| {
                        fail(
                            PARAM,
                            format!("A bound of validation \"{}\" is not a finite number.", v.id),
                        )
                    })?;
                    let lit = float_tokens(f);
                    return Ok(quote! { if #read #op #lit { #refuse } });
                }
                if let Some(i) = number_as_integer(n) {
                    let lit = integer_tokens(i);
                    Ok(quote! { if i128::from(#read) #op #lit { #refuse } })
                } else {
                    let f = n.as_f64().filter(|f| f.is_finite()).ok_or_else(|| {
                        fail(
                            PARAM,
                            format!("A bound of validation \"{}\" is not a finite number.", v.id),
                        )
                    })?;
                    let lit = float_tokens(f);
                    Ok(quote! { if (#read as f64) #op #lit { #refuse } })
                }
            };
            // Resolve the bounds once, so a bad bound is refused rather than emitted.
            let probe = quote! { x };
            let mut checks = Vec::new();
            if let Some(n) = &v.min {
                bound(n, &probe, quote! { < })?;
                checks.push((n.clone(), true));
            }
            if let Some(n) = &v.max {
                bound(n, &probe, quote! { > })?;
                checks.push((n.clone(), false));
            }
            peeled(&place, options, copy, &|p, _| {
                let mut out = TokenStream::new();
                for (n, is_min) in &checks {
                    let op = if *is_min {
                        quote! { < }
                    } else {
                        quote! { > }
                    };
                    if let Ok(t) = bound(n, &p, op) {
                        out.extend(t);
                    }
                }
                out
            })
        }
        Rule::OneOf => {
            if v.values.is_empty() {
                return Err(fail(
                    PARAM,
                    format!("{} a non-empty \"values\".", kind("oneOf")),
                ));
            }
            let Some(l) = leaf.as_ref().filter(|l| l.class != Class::Bytes) else {
                return Err(fail(
                    TARGET_TYPE,
                    format!(
                        "{} a string, number or bool; the target is {type_name}.",
                        kind("oneOf")
                    ),
                ));
            };
            let bad = |value: &Value, what: &str| {
                fail(
                    VALUES,
                    format!("The value {value} of validation \"{}\" is not {what}, the type of its target.", v.id),
                )
            };
            let mut values = Vec::new();
            for value in &v.values {
                values.push(match (l.class, value) {
                    (Class::Text, Value::String(s)) => quote! { #s },
                    (Class::Bool, Value::Bool(b)) => quote! { #b },
                    (Class::Number, Value::Number(n)) if l.base == Builtin::F64 => {
                        float_tokens(n.as_f64().unwrap_or(0.0))
                    }
                    (Class::Number, Value::Number(n)) => match number_as_integer(n) {
                        Some(i) => integer_tokens(i),
                        None => return Err(bad(value, "a whole number")),
                    },
                    (Class::Text, _) => return Err(bad(value, "a string")),
                    (Class::Bool, _) => return Err(bad(value, "a boolean")),
                    _ => return Err(bad(value, "a number")),
                });
            }
            peeled(&place, options, copy, &|p, _| {
                let read = l.read(p.clone());
                match l.class {
                    Class::Text => {
                        let text = l.as_str(p);
                        quote! { if ![#(#values),*].contains(&#text) { #refuse } }
                    }
                    Class::Bool => quote! { if ![#(#values),*].contains(&#read) { #refuse } },
                    _ if l.base == Builtin::F64 => {
                        quote! { if ![#(#values),*].contains(&#read) { #refuse } }
                    }
                    _ => quote! { if ![#(#values),*].contains(&i128::from(#read)) { #refuse } },
                }
            })
        }
        Rule::Custom => {
            let src = v.expr.as_deref().ok_or_else(|| {
                fail(
                    PARAM,
                    format!("{} \"expr\", a boolean expression.", kind("custom")),
                )
            })?;
            let mut scope = ExprScope::new();
            let (head_name, entries) = match &target {
                Target::Input { contract, .. } => {
                    let c = spec.contracts.iter().find(|c| &c.id == contract);
                    (
                        "request",
                        c.map(|c| view::contract_entries(spec, &c.inputs, &quote! { request }))
                            .unwrap_or_default(),
                    )
                }
                Target::Field { record, .. } => {
                    let r = spec.record(record);
                    (
                        "record",
                        r.map(|r| view::record_entries(spec, r, &quote! { record }))
                            .unwrap_or_default(),
                    )
                }
            };
            scope.head(head_name, entries);
            // The helper takes a copy of a plain number or bool and a reference to anything else.
            let value_place = match &leaf {
                Some(l) if !l.layers_empty() || copy => quote! { value },
                _ => quote! { (*value) },
            };
            let value_entry = match &leaf {
                Some(l) => Entry {
                    binding: Some(Binding {
                        ty: l.ty.clone(),
                        tokens: l.read(value_place.clone()),
                    }),
                    why: None,
                    place: value_place,
                    eff: inner_type.clone(),
                },
                None => Entry {
                    binding: None,
                    why: Some(format!(
                        "of type {type_name}, which an expression cannot read"
                    )),
                    place: value_place,
                    eff: inner_type.clone(),
                },
            };
            scope.single("value", value_entry);
            let translated = view::boolean(&scope, src).map_err(|why| {
                fail(
                    EXPRESSION,
                    format!(
                        "The expression of validation \"{}\" cannot be generated: {why}.",
                        v.id
                    ),
                )
            })?;
            let expr_name = naming::ident(&format!(
                "expr_{}",
                naming::snake(&v.id).unwrap_or_default()
            ));
            let value_ty = render::rust_type(spec, inner_type, Scope::Other, uses);
            let tokens = &translated.tokens;
            // Parameters the expression does not read would be unused.
            let head_param = if translated.heads.contains(head_name) {
                naming::ident(head_name)
            } else {
                naming::ident(&format!("_{head_name}"))
            };
            let value_param = if translated.heads.contains("value") {
                naming::ident("value")
            } else {
                naming::ident("_value")
            };
            let value_decl = if copy {
                quote! { #value_param: #value_ty }
            } else {
                quote! { #value_param: &#value_ty }
            };
            extra = quote! {
                fn #expr_name(#head_param: &#owner, #value_decl) -> Result<bool, crate::support::Overflow> {
                    Ok(#tokens)
                }
            };
            peeled(&place, options, copy, &|p, reference| {
                let argument = if copy { p } else { reference };
                quote! {
                    if !matches!(#expr_name(#head, #argument), Ok(true)) { #refuse }
                }
            })
        }
    };

    let (functions, call) = if body.is_empty() {
        (TokenStream::new(), None)
    } else {
        let param = quote! { #head: &#owner };
        (
            quote! {
                #extra
                fn #check_name(#param) -> Result<(), ValidationError> {
                    #body
                    Ok(())
                }
            },
            Some(quote! { #check_name(self)?; }),
        )
    };
    Ok(Compiled {
        functions,
        call,
        owner,
        owner_key,
        record,
    })
}

impl Backend for ValidationsBackend {
    fn module(&self) -> &'static str {
        "validations"
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut names = Namespace::new("module validations");
        names.claim(&mut out, "", "the list of every validation", "ALL");
        let mut statics = Vec::new();
        for v in &spec.constructs.validations {
            let what = format!("validation \"{}\"", v.id);
            match naming::upper_snake(&v.id) {
                Ok(upper) => {
                    names.claim(&mut out, &v.id, &what, &upper);
                    if v.rule == Rule::Pattern {
                        names.claim(
                            &mut out,
                            &v.id,
                            &format!("the pattern of {what}"),
                            &format!("{upper}_PATTERN"),
                        );
                    }
                }
                Err(reason) => out.push(Diagnostic::new(
                    render::NAME_INVALID,
                    &v.id,
                    format!("{what} cannot become a Rust name: {reason}."),
                )),
            }
            if let Ok(snake) = naming::snake(&v.id) {
                names.claim(
                    &mut out,
                    &v.id,
                    &format!("the check of {what}"),
                    &format!("check_{snake}"),
                );
                if v.rule == Rule::Custom {
                    names.claim(
                        &mut out,
                        &v.id,
                        &format!("the expression of {what}"),
                        &format!("expr_{snake}"),
                    );
                }
            }
            if v.message.as_deref().is_none_or(str::is_empty) {
                out.push(Diagnostic::new(
                    PARAM,
                    &v.id,
                    format!("Validation \"{}\" has no message.", v.id),
                ));
            }
            if let Err((code, message)) = compile(spec, v, &mut Uses::default(), &mut statics) {
                out.push(Diagnostic::new(code, &v.id, message));
            }
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let mut uses = Uses::default();
        let mut statics = Vec::new();
        let mut consts = Vec::new();
        let mut names = Vec::new();
        let mut functions = Vec::new();
        let mut owners: Vec<(String, TokenStream, bool, Vec<TokenStream>)> = Vec::new();
        for v in &spec.constructs.validations {
            let name = view::upper_ident(&v.id);
            let id = &v.id;
            let message = v.message.clone().unwrap_or_default();
            let doc = format!(" Validation `{}`: {}", v.id, message);
            consts.push(quote! {
                #[doc = #doc]
                pub const #name: ValidationError = ValidationError { id: #id, message: #message };
            });
            names.push(name);
            if let Ok(c) = compile(spec, v, &mut uses, &mut statics) {
                functions.push(c.functions);
                if let Some(call) = c.call {
                    match owners.iter_mut().find(|o| o.0 == c.owner_key) {
                        Some(o) => o.3.push(call),
                        None => owners.push((c.owner_key, c.owner, c.record, vec![call])),
                    }
                }
            }
        }
        let impls: Vec<TokenStream> = owners
            .iter()
            .map(|(_, owner, record, calls)| {
                let (method, doc) = if *record {
                    (
                        quote! { validate_rules },
                        " Checks the record's validation rules, returning the first one it breaks. Field constraints are checked by `validate`.",
                    )
                } else {
                    (
                        quote! { validate },
                        " Checks the request against its validation rules, returning the first one it breaks.",
                    )
                };
                quote! {
                    impl #owner {
                        #[doc = #doc]
                        pub fn #method(&self) -> Result<(), ValidationError> {
                            #(#calls)*
                            Ok(())
                        }
                    }
                }
            })
            .collect();
        let pattern_uses = if statics.is_empty() {
            TokenStream::new()
        } else {
            quote! {
                use std::sync::LazyLock;

                use regex::Regex;
            }
        };
        let crate_uses = uses.import();
        quote! {
            #![doc = " The validation rules of the specification: what an input or a record field must meet, and the error each rule gives."]
            use std::fmt;

            #pattern_uses
            #crate_uses

            /// A value that breaks a validation rule.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub struct ValidationError {
                /// The id of the validation in the specification.
                pub id: &'static str,
                /// What the specification says to the caller.
                pub message: &'static str,
            }

            impl fmt::Display for ValidationError {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    write!(f, "{} (validation {})", self.message, self.id)
                }
            }

            impl std::error::Error for ValidationError {}

            #(#consts)*

            /// Every validation, in declaration order.
            pub const ALL: &[ValidationError] = &[#(#names),*];

            #(#statics)*
            #(#functions)*
            #(#impls)*
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::load;

    fn spec(extra: &str) -> Spec {
        load(
            "s",
            &format!(
                r#"{{ "system": {{ "id": "s", "name": "S" }},
                "records": [ {{ "id": "rec", "name": "R", "owner": "svc", "key": ["id"], "schemaVersion": 1,
                    "fields": [ {{ "name": "id", "type": "u32" }}, {{ "name": "note", "type": "string", "optional": true }} ] }} ],
                "contracts": [ {{ "id": "do-it", "name": "Do it", "realisation": "generated",
                    "inputs": [ {{ "name": "slug", "type": "string" }}, {{ "name": "count", "type": "u32" }},
                                {{ "name": "mode", "type": "option<string>" }} ] }} ],
                {extra} }}"#
            ),
        )
        .unwrap()
    }

    fn v(body: &str) -> String {
        format!(r#""validations": [ {body} ]"#)
    }

    fn codes(s: &Spec) -> Vec<String> {
        ValidationsBackend
            .check(s)
            .into_iter()
            .map(|d| d.code)
            .collect()
    }

    #[test]
    fn every_rule_is_emitted() {
        let s = spec(&v(
            r#"{ "id": "slug-req", "contract": "do-it", "input": "slug", "rule": "required", "message": "m" },
               { "id": "slug-len", "contract": "do-it", "input": "slug", "rule": "maxLength", "maxLength": 8, "unit": "characters", "message": "m" },
               { "id": "slug-pat", "contract": "do-it", "input": "slug", "rule": "pattern", "pattern": "[a-z]+", "message": "m" },
               { "id": "count-range", "contract": "do-it", "input": "count", "rule": "range", "min": 1, "max": 9, "message": "m" },
               { "id": "mode-one", "contract": "do-it", "input": "mode", "rule": "oneOf", "values": ["a", "b"], "message": "m" },
               { "id": "count-even", "contract": "do-it", "input": "count", "rule": "custom", "expr": "value % 2 == 0 && request.count > 0", "message": "m" },
               { "id": "note-len", "record": "rec", "field": "note", "rule": "maxLength", "maxLength": 4, "unit": "bytes", "message": "m" }"#,
        ));
        assert_eq!(codes(&s), Vec::<String>::new());
        let text = ValidationsBackend.emit(&s).to_string();
        assert!(text.contains("pub fn validate (& self)"));
        assert!(text.contains("pub fn validate_rules (& self)"));
        assert!(text.contains("SLUG_PAT_PATTERN"));
        assert!(text.contains("pub const SLUG_LEN : ValidationError"));
    }

    #[test]
    fn rules_that_do_not_fit_are_refused() {
        let s = spec(&v(
            r#"{ "id": "a", "contract": "do-it", "input": "count", "rule": "maxLength", "maxLength": 3, "unit": "bytes", "message": "m" },
               { "id": "b", "contract": "do-it", "input": "slug", "rule": "maxLength", "maxLength": 3, "message": "m" },
               { "id": "c", "contract": "do-it", "input": "slug", "rule": "pattern", "pattern": "(", "message": "m" },
               { "id": "d", "contract": "do-it", "input": "slug", "rule": "range", "min": 1, "message": "m" },
               { "id": "e", "contract": "do-it", "input": "count", "rule": "oneOf", "values": [1.5], "message": "m" },
               { "id": "f", "contract": "do-it", "input": "count", "rule": "custom", "expr": "value == 'x'", "message": "m" },
               { "id": "g", "contract": "do-it", "input": "nope", "rule": "required", "message": "m" },
               { "id": "h", "contract": "do-it", "rule": "required", "message": "m" }"#,
        ));
        let got = codes(&s);
        for code in [
            TARGET_TYPE,
            UNIT_MISSING,
            PATTERN,
            VALUES,
            EXPRESSION,
            TARGET_UNKNOWN,
            TARGET,
        ] {
            assert!(got.iter().any(|c| c == code), "{code} in {got:?}");
        }
    }
}
