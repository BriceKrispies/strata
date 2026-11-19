//! What the construct backends share: how a typed value is read as a number, a text or a bool
//! (the types the expression language has), the scope an expression is translated in, and
//! durations.
//!
//! The expression language knows booleans, numbers and texts. A generated value is one of those
//! wrapped in a declared newtype (`types::TenantId(String)`), a prelude newtype
//! (`prelude::Timestamp(i64)`), an enum or a `uuid`. A [`Leaf`] says how to unwrap such a value to
//! the type the language sees: newtypes by `.0`, a `uuid` by `to_string()`, an enum by its
//! specification text. Timestamps are read as `i64` milliseconds and durations as `u64`
//! milliseconds. An optional value (or `option<T>`) is only readable as an `Option`, which an
//! expression can compare with `null`; lists and structs cannot be read at all, and a reference
//! to one is refused with a diagnostic that says so.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{Literal, TokenStream};
use quote::quote;
use strata_spec_expr::{Binding, Expr, ExprKind, Scope, Ty, TypeErrorKind};

use crate::ir::{Spec, TypeKind};
use crate::naming;
use crate::render::Uses;
use crate::resolve::{Builtin, TypeRef};

/// What an expression can do with a leaf value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A boolean.
    Bool,
    /// A number: an integer, an `f64`, a timestamp or a duration.
    Number,
    /// A text: a string, a `uuid` or an enum.
    Text,
    /// Bytes.
    Bytes,
}

/// One wrapper between a field's Rust type and its built-in value.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Layer {
    /// A declared id or scalar: `types::Name(inner)`.
    Newtype(String),
    /// `prelude::Timestamp(i64)`.
    Timestamp,
    /// `prelude::DurationMs(u64)`.
    Duration,
    /// A `uuid::Uuid`, read as its text.
    Uuid,
    /// A declared enum, read as its specification text.
    Enum,
}

/// A value of a built-in representation, possibly wrapped, and how to read it.
#[derive(Debug, Clone, PartialEq)]
pub struct Leaf {
    /// The type the expression language sees.
    pub ty: Ty,
    /// What it can do.
    pub class: Class,
    /// The built-in representation at the bottom of the wrappers.
    pub base: Builtin,
    /// The wrappers, outermost first.
    layers: Vec<Layer>,
}

impl Leaf {
    /// The value of `place` as the expression language sees it. `place` must be a place or
    /// value expression that is an atom for precedence purposes.
    pub fn read(&self, place: TokenStream) -> TokenStream {
        let mut out = place;
        for layer in &self.layers {
            out = match layer {
                Layer::Newtype(_) | Layer::Timestamp | Layer::Duration => quote! { #out.0 },
                Layer::Uuid => quote! { #out.to_string() },
                Layer::Enum => quote! { #out.as_str().to_string() },
            };
        }
        out
    }

    /// A text leaf's value as a `&str` expression.
    pub fn as_str(&self, place: TokenStream) -> TokenStream {
        let mut out = place;
        for layer in &self.layers {
            out = match layer {
                Layer::Newtype(_) | Layer::Timestamp | Layer::Duration => quote! { #out.0 },
                Layer::Uuid => quote! { #out.to_string().as_str() },
                Layer::Enum => quote! { #out.as_str() },
            };
        }
        if matches!(self.layers.last(), Some(Layer::Uuid | Layer::Enum)) {
            out
        } else {
            quote! { #out.as_str() }
        }
    }

    /// An expression whose `.as_bytes()` gives this text leaf's UTF-8 bytes: the `String` itself,
    /// the owned text of a uuid, or an enum's `&str`. Unlike [`Self::as_str`] it adds no
    /// `.as_str()` that `.as_bytes()` would make redundant.
    pub fn text_for_bytes(&self, place: TokenStream) -> TokenStream {
        let mut out = place;
        let last = self.layers.len().saturating_sub(1);
        for (i, layer) in self.layers.iter().enumerate() {
            out = match layer {
                Layer::Newtype(_) | Layer::Timestamp | Layer::Duration => quote! { #out.0 },
                Layer::Uuid if i == last => quote! { #out.to_string() },
                Layer::Uuid => quote! { #out.to_string().as_str() },
                Layer::Enum => quote! { #out.as_str() },
            };
        }
        out
    }

    /// The Rust value of this leaf's type made from a value of the language's type, or `None`
    /// when the language's value cannot give one (a `uuid` or an enum).
    pub fn build(&self, value: TokenStream, uses: &mut Uses) -> Option<TokenStream> {
        let mut out = value;
        for layer in self.layers.iter().rev() {
            out = match layer {
                Layer::Newtype(id) => {
                    uses.types = true;
                    let name = naming::ident(&naming::pascal(id).ok()?);
                    quote! { types::#name(#out) }
                }
                Layer::Timestamp => {
                    uses.prelude = true;
                    quote! { prelude::Timestamp(#out) }
                }
                Layer::Duration => {
                    uses.prelude = true;
                    quote! { prelude::DurationMs(#out) }
                }
                Layer::Uuid | Layer::Enum => return None,
            };
        }
        Some(out)
    }

    /// True when the value is held bare, with no wrapper to unwrap.
    pub fn layers_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// True when [`Leaf::build`] can make a value.
    pub fn buildable(&self) -> bool {
        !self
            .layers
            .iter()
            .any(|l| matches!(l, Layer::Uuid | Layer::Enum))
    }
}

fn builtin_leaf(b: Builtin) -> (Ty, Class, Vec<Layer>) {
    match b {
        Builtin::Bool => (Ty::Bool, Class::Bool, vec![]),
        Builtin::U8 => (Ty::U8, Class::Number, vec![]),
        Builtin::U16 => (Ty::U16, Class::Number, vec![]),
        Builtin::U32 => (Ty::U32, Class::Number, vec![]),
        Builtin::U64 => (Ty::U64, Class::Number, vec![]),
        Builtin::I32 => (Ty::I32, Class::Number, vec![]),
        Builtin::I64 => (Ty::I64, Class::Number, vec![]),
        Builtin::F64 => (Ty::F64, Class::Number, vec![]),
        Builtin::String => (Ty::String, Class::Text, vec![]),
        Builtin::Bytes => (Ty::Bytes, Class::Bytes, vec![]),
        Builtin::Timestamp => (Ty::I64, Class::Number, vec![Layer::Timestamp]),
        Builtin::Duration => (Ty::U64, Class::Number, vec![Layer::Duration]),
        Builtin::Uuid => (Ty::String, Class::Text, vec![Layer::Uuid]),
    }
}

/// The leaf of a type that is a built-in, a declared id or scalar, or a declared enum; `None`
/// for options, lists, structs and names that do not resolve.
pub fn leaf(spec: &Spec, t: &TypeRef) -> Option<Leaf> {
    match t {
        TypeRef::Builtin(b) => {
            let (ty, class, layers) = builtin_leaf(*b);
            Some(Leaf {
                ty,
                class,
                base: *b,
                layers,
            })
        }
        TypeRef::Declared(id) => {
            let decl = spec.type_decl(id)?;
            match &decl.kind {
                TypeKind::Id { repr } | TypeKind::Scalar { repr } => {
                    let base = Builtin::parse(repr.as_deref()?)?;
                    let (ty, class, inner) = builtin_leaf(base);
                    let mut layers = vec![Layer::Newtype(id.clone())];
                    layers.extend(inner);
                    Some(Leaf {
                        ty,
                        class,
                        base,
                        layers,
                    })
                }
                TypeKind::Enum { .. } => Some(Leaf {
                    ty: Ty::String,
                    class: Class::Text,
                    base: Builtin::String,
                    layers: vec![Layer::Enum],
                }),
                TypeKind::Struct { .. } => None,
            }
        }
        TypeRef::List(_) | TypeRef::Option(_) => None,
    }
}

/// The type with every outer `option<...>` removed, and how many were removed.
pub fn peel_options(t: &TypeRef) -> (&TypeRef, usize) {
    let mut t = t;
    let mut n = 0;
    while let TypeRef::Option(inner) = t {
        t = inner;
        n += 1;
    }
    (t, n)
}

/// The type as the specification writes it.
pub fn type_text(t: &TypeRef) -> String {
    match t {
        TypeRef::Builtin(b) => b.as_str().to_string(),
        TypeRef::Declared(id) => id.clone(),
        TypeRef::List(inner) => format!("list<{}>", type_text(inner)),
        TypeRef::Option(inner) => format!("option<{}>", type_text(inner)),
    }
}

/// The value `t` has when cleared: nothing for an option, empty for text, bytes and lists, zero
/// for numbers, `false`, the nil `uuid`. `None` for enums and structs, which have no such value.
pub fn zero_value(spec: &Spec, t: &TypeRef, uses: &mut Uses) -> Option<TokenStream> {
    match t {
        TypeRef::Option(_) => Some(quote! { None }),
        TypeRef::List(_) => Some(quote! { Vec::new() }),
        _ => {
            let l = leaf(spec, t)?;
            let base = match l.base {
                Builtin::Bool => quote! { false },
                Builtin::F64 => quote! { 0.0 },
                Builtin::String => quote! { String::new() },
                Builtin::Bytes => quote! { Vec::new() },
                Builtin::Uuid => {
                    // The nil uuid is a `uuid::Uuid`, which has no wrapper to build.
                    return if l
                        .layers
                        .iter()
                        .all(|x| matches!(x, Layer::Newtype(_) | Layer::Uuid))
                    {
                        let mut out = quote! { uuid::Uuid::nil() };
                        for layer in l.layers.iter().rev() {
                            if let Layer::Newtype(id) = layer {
                                uses.types = true;
                                let name = naming::ident(&naming::pascal(id).ok()?);
                                out = quote! { types::#name(#out) };
                            }
                        }
                        Some(out)
                    } else {
                        None
                    };
                }
                _ => quote! { 0 },
            };
            if !l.buildable() {
                return None;
            }
            l.build(base, uses)
        }
    }
}

/// What an expression's reference stands for.
#[derive(Debug, Clone)]
pub struct Entry {
    /// How the expression language reads it, or `None` when it cannot.
    pub binding: Option<Binding>,
    /// Why it cannot be read, when [`Entry::binding`] is `None`.
    pub why: Option<String>,
    /// The Rust place that holds it.
    pub place: TokenStream,
    /// Its full type, with `option<...>` for an optional field.
    pub eff: TypeRef,
}

/// The entry for a value of type `type_name` (`optional` wraps it in an option) held in `place`.
pub fn entry(spec: &Spec, type_name: &str, optional: bool, place: TokenStream) -> Entry {
    let mut eff = TypeRef::parse(type_name);
    if optional {
        eff = TypeRef::Option(Box::new(eff));
    }
    let (inner, options) = peel_options(&eff);
    let shown = type_text(&eff);
    let readable = leaf(spec, inner);
    let (binding, why) = match (&readable, options) {
        (None, _) => (
            None,
            Some(format!(
                "of type {shown}, which an expression cannot read (only built-in values, ids, scalars and enums can be read)"
            )),
        ),
        (Some(l), 0) => (
            Some(Binding {
                ty: l.ty.clone(),
                tokens: l.read(place.clone()),
            }),
            None,
        ),
        (Some(_), _) => (
            Some(Binding {
                ty: Ty::Option(Box::new(Ty::Named(type_text(inner)))),
                tokens: place.clone(),
            }),
            None,
        ),
    };
    Entry {
        binding,
        why,
        place,
        eff,
    }
}

/// The references an expression may make, and how it is translated.
#[derive(Debug, Default)]
pub struct ExprScope {
    heads: BTreeMap<String, BTreeMap<String, Entry>>,
    singles: BTreeMap<String, Entry>,
}

impl ExprScope {
    /// An empty scope.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the references `head.<name>`.
    pub fn head(&mut self, head: &str, entries: Vec<(String, Entry)>) {
        self.heads
            .entry(head.to_string())
            .or_default()
            .extend(entries);
    }

    /// Adds the one-word reference `name`.
    pub fn single(&mut self, name: &str, entry: Entry) {
        self.singles.insert(name.to_string(), entry);
    }

    /// The entry a reference names.
    pub fn lookup(&self, path: &[String]) -> Option<&Entry> {
        match path {
            [one] => self.singles.get(one),
            [head, name] => self.heads.get(head)?.get(name),
            _ => None,
        }
    }

    /// Adds `principal.id`, `principal.tenant` and `principal.role`, read from the value of the
    /// `generated` `support::Caller` held in `caller`.
    pub fn principal(&mut self, caller: &TokenStream) {
        let text = |tokens: TokenStream| Entry {
            binding: Some(Binding {
                ty: Ty::String,
                tokens,
            }),
            why: None,
            place: TokenStream::new(),
            eff: TypeRef::Builtin(Builtin::String),
        };
        self.head(
            "principal",
            vec![
                ("id".to_string(), text(quote! { #caller.id.clone() })),
                (
                    "tenant".to_string(),
                    text(quote! { #caller.tenant.clone().unwrap_or_default() }),
                ),
                (
                    "role".to_string(),
                    text(quote! { #caller.role.clone().unwrap_or_default() }),
                ),
            ],
        );
    }
}

impl Scope for ExprScope {
    fn resolve(&self, path: &[&str]) -> Option<Binding> {
        let owned: Vec<String> = path.iter().map(|p| (*p).to_string()).collect();
        self.lookup(&owned)?.binding.clone()
    }

    fn overflow_error(&self) -> TokenStream {
        quote! { crate::support::Overflow }
    }
}

/// The heads (`request`, `principal`, ...) and one-word names an expression reads.
pub fn heads_used(expr: &Expr) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect(expr, &mut out);
    out
}

fn collect(expr: &Expr, out: &mut BTreeSet<String>) {
    match &expr.kind {
        ExprKind::Lit(_) => {}
        ExprKind::Ref(path) => {
            if let Some(first) = path.first() {
                out.insert(first.clone());
            }
        }
        ExprKind::Un(_, a) => collect(a, out),
        ExprKind::Bin(_, a, b) => {
            collect(a, out);
            collect(b, out);
        }
    }
}

/// A translated expression.
#[derive(Debug)]
pub struct Translated {
    /// The Rust expression, which uses `?` and so belongs in a function returning
    /// `Result<_, crate::support::Overflow>`.
    pub tokens: TokenStream,
    /// The heads and one-word names it reads.
    pub heads: BTreeSet<String>,
}

/// Parses `src`, or says why not in one line.
pub fn parse(src: &str) -> Result<Expr, String> {
    strata_spec_expr::parse(src).map_err(|e| format!("\"{src}\" does not parse: {}", e.message))
}

fn type_message(scope: &ExprScope, src: &str, e: &strata_spec_expr::TypeError) -> String {
    if e.kind == TypeErrorKind::UnknownReference {
        // "Unknown reference \"a.b\"": a reference that exists but cannot be read says why.
        if let Some(name) = e.message.split('"').nth(1) {
            let path: Vec<String> = name.split('.').map(str::to_string).collect();
            if let Some(why) = scope.lookup(&path).and_then(|entry| entry.why.as_ref()) {
                return format!("\"{src}\": \"{name}\" is {why}");
            }
        }
    }
    format!("\"{src}\": {}", e.message)
}

/// Translates a boolean expression.
pub fn boolean(scope: &ExprScope, src: &str) -> Result<Translated, String> {
    let expr = parse(src)?;
    let ty = strata_spec_expr::type_of(&expr, scope).map_err(|e| type_message(scope, src, &e))?;
    if ty != Ty::Bool {
        return Err(format!(
            "\"{src}\" is {ty}, and a condition must be a boolean"
        ));
    }
    let tokens =
        strata_spec_expr::to_rust(&expr, scope).map_err(|e| type_message(scope, src, &e))?;
    Ok(Translated {
        tokens,
        heads: heads_used(&expr),
    })
}

/// Translates an expression that must have type `expected`.
pub fn typed(scope: &ExprScope, src: &str, expected: &Ty) -> Result<Translated, String> {
    let expr = parse(src)?;
    let tokens = strata_spec_expr::to_rust_as(&expr, scope, expected)
        .map_err(|e| type_message(scope, src, &e))?;
    Ok(Translated {
        tokens,
        heads: heads_used(&expr),
    })
}

/// The reference an expression consists of, when it is nothing else.
pub fn bare_reference(src: &str) -> Option<Vec<String>> {
    match strata_spec_expr::parse(src).ok()?.kind {
        ExprKind::Ref(path) => Some(path),
        _ => None,
    }
}

/// True when the expression is the literal `null`.
pub fn is_null(src: &str) -> bool {
    strata_spec_expr::parse(src)
        .is_ok_and(|e| matches!(e.kind, ExprKind::Lit(strata_spec_expr::Lit::Null)))
}

/// The Rust name of a field, function or variable made from a specification name.
pub fn snake_ident(name: &str) -> proc_macro2::Ident {
    naming::ident(&naming::snake(name).unwrap_or_default())
}

/// The Rust name of a type made from a specification id.
pub fn pascal_ident(name: &str) -> proc_macro2::Ident {
    naming::ident(&naming::pascal(name).unwrap_or_default())
}

/// The Rust name of a constant made from a specification id.
pub fn upper_ident(name: &str) -> proc_macro2::Ident {
    naming::ident(&naming::upper_snake(name).unwrap_or_default())
}

/// The entries of a contract's inputs or outputs, read from `base.<field>`. Fields whose type
/// is not stated are left out; the contract backend refuses them.
pub fn contract_entries(
    spec: &Spec,
    fields: &[crate::ir::ContractField],
    base: &TokenStream,
) -> Vec<(String, Entry)> {
    fields
        .iter()
        .filter_map(|f| {
            let ty = f.type_name.as_deref()?;
            let name = snake_ident(&f.name);
            Some((
                f.name.clone(),
                entry(spec, ty, f.optional, quote! { #base.#name }),
            ))
        })
        .collect()
}

/// The entries of a record's fields, read from `base.<field>`.
pub fn record_entries(
    spec: &Spec,
    record: &crate::ir::Record,
    base: &TokenStream,
) -> Vec<(String, Entry)> {
    record
        .fields
        .iter()
        .map(|f| {
            let name = snake_ident(&f.name);
            (
                f.name.clone(),
                entry(spec, &f.type_name, f.optional, quote! { #base.#name }),
            )
        })
        .collect()
}

/// The path of a contract's request type, from the crate root.
pub fn request_path(contract_id: &str) -> TokenStream {
    let name = naming::ident(&format!(
        "{}Request",
        naming::pascal(contract_id).unwrap_or_default()
    ));
    quote! { crate::contracts::#name }
}

/// The path of a contract's response type, from the crate root.
pub fn response_path(contract_id: &str) -> TokenStream {
    let name = naming::ident(&format!(
        "{}Response",
        naming::pascal(contract_id).unwrap_or_default()
    ));
    quote! { crate::contracts::#name }
}

/// The path of a record's type, from the crate root.
pub fn record_path(record_id: &str) -> TokenStream {
    let name = pascal_ident(record_id);
    quote! { crate::records::#name }
}

/// A duration of the form `"<decimal> <unit>"` (units `ns`, `us`, `ms`, `s`, `min`, `h`, `d`)
/// in nanoseconds. Refuses a duration that is not positive, not a whole number of nanoseconds,
/// or longer than a `u64` of nanoseconds (about 584 years).
pub fn duration_ns(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .ok_or_else(|| format!("\"{text}\" has no unit"))?;
    let (amount, unit) = text.split_at(split);
    let unit_ns: u128 = match unit.trim() {
        "ns" => 1,
        "us" => 1_000,
        "ms" => 1_000_000,
        "s" => 1_000_000_000,
        "min" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        "d" => 86_400_000_000_000,
        other => {
            return Err(format!(
                "\"{text}\" has the unit \"{other}\", which is not ns, us, ms, s, min, h or d"
            ))
        }
    };
    let (whole, fraction) = amount.split_once('.').unwrap_or((amount, ""));
    if whole.is_empty() || fraction.len() > 18 || amount.matches('.').count() > 1 {
        return Err(format!("\"{text}\" is not a decimal number with a unit"));
    }
    let digits: u128 = format!("{whole}{fraction}")
        .parse()
        .map_err(|_| format!("\"{text}\" is not a decimal number with a unit"))?;
    let scale = 10u128.pow(u32::try_from(fraction.len()).unwrap_or(0));
    let total = digits
        .checked_mul(unit_ns)
        .ok_or_else(|| format!("\"{text}\" is too long"))?;
    if total % scale != 0 {
        return Err(format!("\"{text}\" is not a whole number of nanoseconds"));
    }
    let ns = total / scale;
    if ns == 0 {
        return Err(format!("\"{text}\" is not positive"));
    }
    u64::try_from(ns).map_err(|_| format!("\"{text}\" is longer than 584 years"))
}

/// A `std::time::Duration` constant for `ns` nanoseconds, in the largest whole unit.
pub fn duration_tokens(ns: u64) -> TokenStream {
    if ns % 1_000_000_000 == 0 {
        let lit = Literal::u64_unsuffixed(ns / 1_000_000_000);
        quote! { std::time::Duration::from_secs(#lit) }
    } else if ns % 1_000_000 == 0 {
        let lit = Literal::u64_unsuffixed(ns / 1_000_000);
        quote! { std::time::Duration::from_millis(#lit) }
    } else {
        let lit = Literal::u64_unsuffixed(ns);
        quote! { std::time::Duration::from_nanos(#lit) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{System, TypeDecl};

    fn spec() -> Spec {
        Spec {
            path: "s".into(),
            system: System {
                id: "s".into(),
                name: "S".into(),
                generation_target: None,
            },
            components: vec![],
            types: vec![
                TypeDecl {
                    id: "tenant-id".into(),
                    name: "T".into(),
                    description: None,
                    kind: TypeKind::Id {
                        repr: Some("string".into()),
                    },
                },
                TypeDecl {
                    id: "since".into(),
                    name: "S".into(),
                    description: None,
                    kind: TypeKind::Scalar {
                        repr: Some("timestamp".into()),
                    },
                },
                TypeDecl {
                    id: "state".into(),
                    name: "S".into(),
                    description: None,
                    kind: TypeKind::Enum {
                        values: vec!["a".into()],
                    },
                },
            ],
            records: vec![],
            contracts: vec![],
            events: vec![],
            machines: vec![],
            constructs: Default::default(),
        }
    }

    fn read(spec: &Spec, ty: &str) -> String {
        let l = leaf(spec, &TypeRef::parse(ty)).unwrap();
        l.read(quote! { x }).to_string().replace(' ', "")
    }

    #[test]
    fn leaves_are_unwrapped_to_what_the_language_sees() {
        let s = spec();
        assert_eq!(read(&s, "u32"), "x");
        assert_eq!(read(&s, "tenant-id"), "x.0");
        assert_eq!(read(&s, "timestamp"), "x.0");
        assert_eq!(read(&s, "since"), "x.0.0");
        assert_eq!(read(&s, "uuid"), "x.to_string()");
        assert_eq!(read(&s, "state"), "x.as_str().to_string()");
        assert!(leaf(&s, &TypeRef::parse("list<u8>")).is_none());
    }

    #[test]
    fn leaves_are_rebuilt_where_they_can_be() {
        let s = spec();
        let mut uses = Uses::default();
        let l = leaf(&s, &TypeRef::parse("since")).unwrap();
        let built = l.build(quote! { v }, &mut uses).unwrap().to_string();
        assert_eq!(
            built.replace(' ', ""),
            "types::Since(prelude::Timestamp(v))"
        );
        assert!(uses.types && uses.prelude);
        assert!(leaf(&s, &TypeRef::parse("state"))
            .unwrap()
            .build(quote! { v }, &mut uses)
            .is_none());
    }

    #[test]
    fn durations_parse_exactly() {
        assert_eq!(duration_ns("100 ms"), Ok(100_000_000));
        assert_eq!(duration_ns("1.5 s"), Ok(1_500_000_000));
        assert_eq!(duration_ns("400 d"), Ok(400 * 86_400_000_000_000));
        assert!(duration_ns("0 s").is_err());
        assert!(duration_ns("1.5 ns").is_err());
        assert!(duration_ns("5").is_err());
        assert!(duration_ns("5 weeks").is_err());
        assert!(duration_ns("1e3 s").is_err());
        assert!(duration_ns("99999999 d").is_err());
    }

    #[test]
    fn duration_constants_use_the_largest_whole_unit() {
        assert_eq!(
            duration_tokens(2_000_000_000).to_string().replace(' ', ""),
            "std::time::Duration::from_secs(2)"
        );
        assert_eq!(
            duration_tokens(2_500_000).to_string().replace(' ', ""),
            "std::time::Duration::from_nanos(2500000)"
        );
        assert_eq!(
            duration_tokens(3_000_000).to_string().replace(' ', ""),
            "std::time::Duration::from_millis(3)"
        );
    }
}
