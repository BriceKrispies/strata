//! Languages (`languages`, kind `language`; Exegra WG-4929): an expression language the users of
//! the specified system write, such as Strata's tenant rule language, and its generation into the
//! pure crate (WG-4931).
//!
//! A language is data: an AST union, a value union, the static shapes, the scopes names are read
//! through, and one operator per AST variant (or per case of a split variant), each naming a
//! primitive of the catalogue below. specc implements every primitive once, pinned to the kernel
//! function Exegra's documentation cites (`crates/kernel/src/expr.rs`, `crates/substrate/src/
//! syntax.rs`), and generates from the language, into one module named after it:
//!
//! - a tokenizer and a precedence-climbing parser for the declared forms and levels (`parse`);
//! - a printer, `Display` of the AST type, byte-identical to the kernel's `Display` for `Expr`,
//!   because evidence strings embed it;
//! - a static checker (`check`), equivalent to the kernel's `check`, over a [`TypeEnv`]-like
//!   trait the definition site implements;
//! - an evaluator (`eval` and `holds`), equivalent to the kernel's `eval`: Kleene logic, null
//!   propagation and evaluation errors exactly as the primitives say, over an environment trait.
//!
//! What the kernel reads from its own data structures (a record's fields and system attributes,
//! following a reference, parameters, the clock, the principal, the roles, field definitions)
//! is a trait method the caller implements; everything else is generated.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};
use serde_json::Value as Json;

use crate::diagnostic::Diagnostic;
use crate::ir::{Spec, TypeDecl, TypeKind, Variant};
use crate::load::{Loader, Obj};
use crate::naming;
use crate::render::{self, Uses};
use crate::resolve::{Builtin, TypeRef};

/// Code of a language whose references do not resolve or contradict each other.
pub const LANGUAGE_INVALID: &str = "specc.language-invalid";
/// Code of a language that is well formed but uses something specc cannot generate.
pub const LANGUAGE_UNSUPPORTED: &str = "specc.language-unsupported";

// ---------------------------------------------------------------------------------------------
// The typed form of the section
// ---------------------------------------------------------------------------------------------

/// A `language` entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    /// The language id.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The description, if any.
    pub description: Option<String>,
    /// The union type of the AST.
    pub ast: Option<String>,
    /// The union type of the values.
    pub values: Option<String>,
    /// The static shapes the checker gives expressions, in order.
    pub shapes: Vec<String>,
    /// The shape of null and of what the checker cannot pin down.
    pub any_shape: Option<String>,
    /// The scopes names are read through.
    pub scopes: Vec<LanguageScope>,
    /// The operators, in declaration order.
    pub operators: Vec<Operator>,
}

/// A scope of a language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageScope {
    /// The word the scope is written as.
    pub name: String,
    /// `record-scope`, `param`, `constant` or `role-test`.
    pub kind: String,
    /// For a record scope: the value of the AST's scope field it stands for.
    pub value: Option<String>,
    /// For a record scope: its system attributes, `$`-names with their shapes.
    pub attributes: Vec<(String, String)>,
}

/// An operator of a language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operator {
    /// The operator name.
    pub name: String,
    /// The AST variant it builds.
    pub variant: Option<String>,
    /// `case: {field, value}` of a split variant.
    pub case: Option<(String, String)>,
    /// `syntax.form`.
    pub form: Option<String>,
    /// `syntax.token`.
    pub token: Option<String>,
    /// `syntax.precedence`.
    pub precedence: Option<u64>,
    /// `syntax.assoc`.
    pub assoc: Option<String>,
    /// One list of accepted shapes per expression operand.
    pub operands: Vec<Vec<String>>,
    /// `result`: a shape or `operand:<n>`.
    pub result: Option<String>,
    /// `nulls`.
    pub nulls: Option<String>,
    /// `primitive`.
    pub primitive: Option<String>,
}

impl Loader {
    /// The `languages` section.
    pub(crate) fn languages(&mut self, top: &Obj) -> Vec<Language> {
        self.section(top, "languages")
            .into_iter()
            .filter_map(|(id, obj)| self.language(&id, obj))
            .collect()
    }

    fn language(&mut self, id: &str, obj: &Obj) -> Option<Language> {
        let name = self.req_str(obj, "name", id);
        let description = self.opt_str(obj, "description", id);
        let ast = self.opt_str(obj, "ast", id);
        let values = self.opt_str(obj, "values", id);
        let shapes = self.opt_str_list(obj, "shapes", id).unwrap_or_default();
        let any_shape = self.opt_str(obj, "anyShape", id);
        let mut scopes = Vec::new();
        for s in self.obj_list(obj, "scopes", id) {
            let (Some(name), Some(kind)) =
                (self.req_str(s, "name", id), self.req_str(s, "kind", id))
            else {
                continue;
            };
            let value = self.opt_str(s, "value", id);
            let mut attributes = Vec::new();
            for a in self.obj_list(s, "attributes", id) {
                if let (Some(n), Some(shape)) =
                    (self.req_str(a, "name", id), self.req_str(a, "shape", id))
                {
                    attributes.push((n, shape));
                }
            }
            scopes.push(LanguageScope {
                name,
                kind,
                value,
                attributes,
            });
        }
        let mut operators = Vec::new();
        for o in self.obj_list(obj, "operators", id) {
            if let Some(op) = self.operator(id, o) {
                operators.push(op);
            }
        }
        Some(Language {
            id: id.to_string(),
            name: name?,
            description,
            ast,
            values,
            shapes,
            any_shape,
            scopes,
            operators,
        })
    }

    fn operator(&mut self, id: &str, o: &Obj) -> Option<Operator> {
        let name = self.req_str(o, "name", id)?;
        let variant = self.opt_str(o, "variant", id);
        let case = match o.get("case") {
            None | Some(Json::Null) => None,
            Some(Json::Object(c)) => {
                match (self.req_str(c, "field", id), self.req_str(c, "value", id)) {
                    (Some(f), Some(v)) => Some((f, v)),
                    _ => None,
                }
            }
            Some(_) => {
                self.shape(id, format!("\"case\" of operator {name} must be an object"));
                None
            }
        };
        let (mut form, mut token, mut precedence, mut assoc) = (None, None, None, None);
        match o.get("syntax") {
            None | Some(Json::Null) => {}
            Some(Json::Object(s)) => {
                form = self.opt_str(s, "form", id);
                token = self.opt_str(s, "token", id);
                precedence = self
                    .opt_number(s, "precedence", id)
                    .and_then(|n| n.as_u64());
                assoc = self.opt_str(s, "assoc", id);
            }
            Some(_) => self.shape(
                id,
                format!("\"syntax\" of operator {name} must be an object"),
            ),
        }
        let mut operands = Vec::new();
        match o.get("operands") {
            None | Some(Json::Null) => {}
            Some(Json::Array(items)) => {
                for item in items {
                    match item {
                        Json::String(s) => operands.push(vec![s.clone()]),
                        Json::Array(alts) if alts.iter().all(Json::is_string) => operands.push(
                            alts.iter()
                                .filter_map(Json::as_str)
                                .map(str::to_string)
                                .collect(),
                        ),
                        _ => self.shape(
                            id,
                            format!(
                                "an operand of operator {name} must be a shape or a list of shapes"
                            ),
                        ),
                    }
                }
            }
            Some(_) => self.shape(
                id,
                format!("\"operands\" of operator {name} must be a list"),
            ),
        }
        Some(Operator {
            name,
            variant,
            case,
            form,
            token,
            precedence,
            assoc,
            operands,
            result: self.opt_str(o, "result", id),
            nulls: self.opt_str(o, "nulls", id),
            primitive: self.opt_str(o, "primitive", id),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// The primitive catalogue
// ---------------------------------------------------------------------------------------------

/// A primitive of the catalogue: what an operator can mean. specc implements each once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prim {
    /// `eq`
    Eq,
    /// `ne`
    Ne,
    /// `lt`
    Lt,
    /// `le`
    Le,
    /// `gt`
    Gt,
    /// `ge`
    Ge,
    /// `and3`
    And3,
    /// `or3`
    Or3,
    /// `not3`
    Not3,
    /// `in-list`
    InList,
    /// `is-null`
    IsNull,
    /// `len`
    Len,
    /// `lower`
    Lower,
    /// `contains`
    Contains,
    /// `starts-with`
    StartsWith,
    /// `is-slug`
    IsSlug,
    /// `if`
    If,
    /// `has-role`
    HasRole,
    /// `now`
    Now,
    /// `principal`
    Principal,
    /// `param`
    Param,
    /// `var`
    Var,
    /// `through`
    Through,
    /// `literal`
    Literal,
    /// `list-literal`
    ListLiteral,
    /// `add-checked`: int + int, or timestamp + int milliseconds (D-6540, D-6541).
    AddChecked,
    /// `sub-checked`: int - int, or timestamp - int milliseconds.
    SubChecked,
    /// `mul-checked`: int * int.
    MulChecked,
    /// `time-difference`: `ms_between(a, b)`, b minus a in milliseconds.
    TimeDifference,
    /// `days-ms`: n times 86,400,000.
    DaysMs,
    /// `hours-ms`: n times 3,600,000.
    HoursMs,
    /// `minutes-ms`: n times 60,000.
    MinutesMs,
    /// `concat-checked`: two strings joined, at most 4,096 bytes (D-6542).
    ConcatChecked,
    /// `to-text`: an int, timestamp, reference, bool or string as text.
    ToText,
}

impl Prim {
    /// The checked integer primitives: every operand and result in ±(2^53-1), else
    /// `integer-overflow` (D-6540).
    fn checked_int(self) -> bool {
        matches!(
            self,
            Prim::AddChecked
                | Prim::SubChecked
                | Prim::MulChecked
                | Prim::TimeDifference
                | Prim::DaysMs
                | Prim::HoursMs
                | Prim::MinutesMs
        )
    }

    /// The primitives whose operands the checker checks one after another, which may give the
    /// shape of an operand as their result (`operand:<n>`).
    fn checked_in_turn(self) -> bool {
        matches!(
            self,
            Prim::Not3
                | Prim::IsNull
                | Prim::Len
                | Prim::Lower
                | Prim::StartsWith
                | Prim::IsSlug
                | Prim::AddChecked
                | Prim::SubChecked
                | Prim::MulChecked
                | Prim::TimeDifference
                | Prim::DaysMs
                | Prim::HoursMs
                | Prim::MinutesMs
                | Prim::ConcatChecked
                | Prim::ToText
        )
    }
}

/// The largest magnitude of an integer of checked arithmetic, 2^53-1 (D-6540, D-5004), which
/// Rust's `i64` and JavaScript's numbers both hold exactly.
pub const SAFE_INT: i64 = 9_007_199_254_740_991;
/// The longest string `concat` may build, in bytes of UTF-8 (D-6542).
pub const MAX_TEXT_BYTES: usize = 4096;
/// The evaluation error of checked arithmetic outside ±[`SAFE_INT`].
pub const INTEGER_OVERFLOW: &str = "integer-overflow";
/// The evaluation error of a `concat` longer than [`MAX_TEXT_BYTES`].
pub const STRING_TOO_LONG: &str = "string-too-long";

/// How many expression operands a primitive takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    /// That many.
    Fixed(usize),
    /// One list of operands (an n-ary operator).
    List,
}

/// An entry of the catalogue, as Exegra's `LANGUAGE_PRIMITIVES` states it.
#[derive(Debug, Clone, Copy)]
pub struct PrimitiveInfo {
    /// The catalogue name.
    pub name: &'static str,
    /// The primitive.
    pub prim: Prim,
    /// Its expression operands.
    pub arity: Arity,
    /// The syntax forms it can be written in.
    pub forms: &'static [&'static str],
    /// Its null handling.
    pub nulls: &'static str,
    /// For the scope form, the kind of scope it reads.
    pub scope: Option<&'static str>,
    /// The kernel code it is pinned to.
    pub kernel: &'static str,
}

const fn prim(
    name: &'static str,
    prim: Prim,
    arity: Arity,
    forms: &'static [&'static str],
    nulls: &'static str,
    scope: Option<&'static str>,
    kernel: &'static str,
) -> PrimitiveInfo {
    PrimitiveInfo {
        name,
        prim,
        arity,
        forms,
        nulls,
        scope,
        kernel,
    }
}

/// The catalogue, pinned to the kernel as Exegra's documentation cites it.
pub const PRIMITIVES: &[PrimitiveInfo] = &[
    prim("eq", Prim::Eq, Arity::Fixed(2), &["infix"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Cmp(CmpOp::Eq) -> compare"),
    prim("ne", Prim::Ne, Arity::Fixed(2), &["infix"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Cmp(CmpOp::Ne) -> compare"),
    prim("lt", Prim::Lt, Arity::Fixed(2), &["infix"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Cmp(CmpOp::Lt) -> compare"),
    prim("le", Prim::Le, Arity::Fixed(2), &["infix"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Cmp(CmpOp::Le) -> compare"),
    prim("gt", Prim::Gt, Arity::Fixed(2), &["infix"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Cmp(CmpOp::Gt) -> compare"),
    prim("ge", Prim::Ge, Arity::Fixed(2), &["infix"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Cmp(CmpOp::Ge) -> compare"),
    prim("and3", Prim::And3, Arity::List, &["keyword-infix"], "kleene-and", None, "crates/kernel/src/expr.rs: eval, Expr::And -> tri_and"),
    prim("or3", Prim::Or3, Arity::List, &["keyword-infix"], "kleene-or", None, "crates/kernel/src/expr.rs: eval, Expr::Or -> tri_or"),
    prim("not3", Prim::Not3, Arity::Fixed(1), &["prefix-word"], "kleene-not", None, "crates/kernel/src/expr.rs: eval, Expr::Not"),
    prim("in-list", Prim::InList, Arity::Fixed(2), &["keyword-infix", "infix"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::In"),
    prim("is-null", Prim::IsNull, Arity::Fixed(1), &["call"], "total", None, "crates/kernel/src/expr.rs: eval, Expr::IsNull"),
    prim("len", Prim::Len, Arity::Fixed(1), &["call"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Len"),
    prim("lower", Prim::Lower, Arity::Fixed(1), &["call"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Lower"),
    prim("contains", Prim::Contains, Arity::Fixed(2), &["call"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Contains"),
    prim("starts-with", Prim::StartsWith, Arity::Fixed(2), &["call"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::StartsWith"),
    prim("is-slug", Prim::IsSlug, Arity::Fixed(1), &["call"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::IsSlug -> is_slug"),
    prim("if", Prim::If, Arity::Fixed(3), &["call"], "null-is-false", None, "crates/kernel/src/expr.rs: eval, Expr::If"),
    prim("has-role", Prim::HasRole, Arity::Fixed(0), &["scope"], "total", Some("role-test"), "crates/kernel/src/expr.rs: eval, Expr::HasRole; crates/substrate/src/syntax.rs: primary, \"has_role\""),
    prim("now", Prim::Now, Arity::Fixed(0), &["scope"], "total", Some("constant"), "crates/kernel/src/expr.rs: eval, Expr::Now"),
    prim("principal", Prim::Principal, Arity::Fixed(0), &["scope"], "total", Some("constant"), "crates/kernel/src/expr.rs: eval, Expr::Principal"),
    prim("param", Prim::Param, Arity::Fixed(0), &["scope"], "total", Some("param"), "crates/kernel/src/expr.rs: eval, Expr::Param"),
    prim("var", Prim::Var, Arity::Fixed(0), &["scope"], "total", Some("record-scope"), "crates/kernel/src/expr.rs: eval, Expr::Var -> attribute"),
    prim("through", Prim::Through, Arity::Fixed(0), &["scope"], "total", Some("record-scope"), "crates/kernel/src/expr.rs: eval, Expr::Through"),
    prim("literal", Prim::Literal, Arity::Fixed(0), &["literal"], "total", None, "crates/kernel/src/expr.rs: eval, Expr::Lit; write_value; crates/substrate/src/syntax.rs: tokenize, primary"),
    prim("list-literal", Prim::ListLiteral, Arity::Fixed(0), &["literal"], "total", None, "crates/kernel/src/expr.rs: eval, Expr::Lit(Value::List); crates/substrate/src/syntax.rs: primary, \"[\""),
    prim("add-checked", Prim::AddChecked, Arity::Fixed(2), &["infix"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Arith(ArithOp::Add) -> checked"),
    prim("sub-checked", Prim::SubChecked, Arity::Fixed(2), &["infix"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Arith(ArithOp::Sub) -> checked"),
    prim("mul-checked", Prim::MulChecked, Arity::Fixed(2), &["infix"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Arith(ArithOp::Mul) -> checked"),
    prim("time-difference", Prim::TimeDifference, Arity::Fixed(2), &["call"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::MsBetween -> checked"),
    prim("days-ms", Prim::DaysMs, Arity::Fixed(1), &["call"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Duration(DurationUnit::Days) -> checked"),
    prim("hours-ms", Prim::HoursMs, Arity::Fixed(1), &["call"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Duration(DurationUnit::Hours) -> checked"),
    prim("minutes-ms", Prim::MinutesMs, Arity::Fixed(1), &["call"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Duration(DurationUnit::Minutes) -> checked"),
    prim("concat-checked", Prim::ConcatChecked, Arity::Fixed(2), &["call"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Concat"),
    prim("to-text", Prim::ToText, Arity::Fixed(1), &["call"], "propagate", None, "crates/kernel/src/expr.rs: eval, Expr::Text"),
];

/// The catalogue entry of a primitive name.
pub fn primitive(name: &str) -> Option<&'static PrimitiveInfo> {
    PRIMITIVES.iter().find(|p| p.name == name)
}

/// The literal forms: the kernel's `Value::kind` name of each (which evaluation errors print),
/// and the payload representations specc accepts for the value variant it takes.
const LITERAL_FORMS: &[(&str, &str, &[Builtin])] = &[
    ("null", "null", &[]),
    ("bool", "bool", &[Builtin::Bool]),
    ("int", "int", &[Builtin::I64]),
    ("string", "string", &[Builtin::String]),
    ("time", "timestamp", &[Builtin::I64, Builtin::U64]),
    ("ref", "reference", &[Builtin::U64, Builtin::I64]),
    ("list", "list", &[]),
];

// ---------------------------------------------------------------------------------------------
// Resolution: the language checked against the types, as specc will generate it
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Assoc {
    Left,
    Right,
    None,
    NAry,
}

#[derive(Debug, Clone)]
enum Form {
    /// `infix` (`symbol`) or `keyword-infix`.
    Binary {
        symbol: bool,
        token: String,
        precedence: u64,
        assoc: Assoc,
    },
    /// `prefix-word`.
    Prefix { token: String, precedence: u64 },
    /// `call`.
    Call { token: String },
    /// `literal`, of the literal form.
    Literal { form: String },
    /// `scope`: the token names the scope (none for record-scope reads).
    Scope { token: Option<String> },
}

/// An operator resolved against its AST variant.
#[derive(Debug, Clone)]
struct Op<'a> {
    op: &'a Operator,
    info: &'static PrimitiveInfo,
    variant: &'a Variant,
    /// The case field's index and the case value.
    case: Option<(usize, String)>,
    /// The indices of the expression fields, in order (one list field for an n-ary operator).
    exprs: Vec<usize>,
    form: Form,
}

impl Op<'_> {
    fn token(&self) -> &str {
        match &self.form {
            Form::Binary { token, .. } | Form::Prefix { token, .. } | Form::Call { token } => token,
            Form::Literal { form } => form,
            Form::Scope { token } => token.as_deref().unwrap_or(""),
        }
    }
}

/// A record scope with its value of the AST's scope enum.
#[derive(Debug, Clone)]
struct RecordScope<'a> {
    name: &'a str,
    value: &'a str,
    attributes: &'a [(String, String)],
}

/// A language ready to emit.
struct Plan<'a> {
    spec: &'a Spec,
    lang: &'a Language,
    ast: &'a TypeDecl,
    values: &'a TypeDecl,
    ops: Vec<Op<'a>>,
    record_scopes: Vec<RecordScope<'a>>,
    /// The scope enum the record scopes' values belong to.
    scope_enum: Option<&'a TypeDecl>,
    /// The literal form of each value variant, by variant name.
    value_forms: Vec<(String, String)>,
    /// The shape of each value variant's literal, by variant name.
    value_shapes: Vec<(String, String)>,
}

/// The Rust module name of a language.
pub fn module_name(lang: &Language) -> String {
    naming::snake(&lang.id).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// The bridge to functions (WG-4934)
// ---------------------------------------------------------------------------------------------

/// How a function's `eval` and `check` of a language ([`crate::function`]) reach the generated
/// module: its environment traits implemented over the environment structs the function passes,
/// read as Exegra reads them (`src/core/language-eval.js`). For `eval`, a record scope is an
/// `option<map<string, V>>` field, a param scope a `map<string, V>`, a constant a `V` and a role
/// test a `list<string>`, each named as its scope; no lookup is available, so following a
/// reference is an evaluation error. For `check`, the struct has `fields: map<string, string>`
/// (a shape name per field), `params: list<string>` and `roles: list<string>`, and a field read
/// through a reference has the any shape.
pub(crate) struct Bridge<'a> {
    plan: Plan<'a>,
}

/// The bridge of a language specc generates; `None` when [`check`] refuses the language.
pub(crate) fn bridge<'a>(spec: &'a Spec, lang: &'a Language) -> Option<Bridge<'a>> {
    resolve(spec, lang).ok().map(|plan| Bridge { plan })
}

/// The scope kinds of the language a bridge reads, with the struct field each scope is.
pub(crate) struct ScopeField {
    /// The scope's name, which is also the field's.
    pub name: String,
    /// `record-scope`, `param`, `constant` or `role-test`.
    pub kind: String,
}

impl Bridge<'_> {
    /// The id of the language's AST union.
    pub(crate) fn ast_id(&self) -> &str {
        &self.plan.ast.id
    }

    /// The id of the language's value union.
    pub(crate) fn values_id(&self) -> &str {
        &self.plan.values.id
    }

    /// The scopes, in declaration order.
    pub(crate) fn scopes(&self) -> Vec<ScopeField> {
        self.plan
            .lang
            .scopes
            .iter()
            .map(|s| ScopeField {
                name: s.name.clone(),
                kind: s.kind.clone(),
            })
            .collect()
    }

    fn reads_records(&self) -> bool {
        !self.plan.record_scopes.is_empty()
            && (self.plan.uses(Prim::Var) || self.plan.uses(Prim::Through))
    }

    fn module(&self) -> TokenStream {
        let m = naming::ident(&module_name(self.plan.lang));
        quote! { crate::#m }
    }

    /// The scope token of the operator of a primitive (its scope's name), if the language has one.
    fn token_of(&self, p: Prim) -> Option<&str> {
        self.plan
            .ops
            .iter()
            .find(|o| o.info.prim == p)
            .and_then(|o| match &o.form {
                Form::Scope { token: Some(t) } => Some(t.as_str()),
                _ => None,
            })
    }

    fn field(name: &str) -> Ident {
        naming::ident(&naming::snake(name).unwrap_or_default())
    }

    /// `impl Record for BTreeMap<String, V>`: the record of a record scope, a field or system
    /// attribute read from the map (null when absent), as Exegra reads it. Nothing when the
    /// language reads no records.
    pub(crate) fn record_impl(&self) -> TokenStream {
        if !self.reads_records() {
            return TokenStream::new();
        }
        let module = self.module();
        let value = self.plan.value_ty();
        let null = self.plan.value("null");
        quote! {
            impl #module::Record for std::collections::BTreeMap<String, #value> {
                fn system_attribute(&self, name: &str) -> #value {
                    self.get(name).cloned().unwrap_or(#null)
                }

                fn field(&self, name: &str) -> Result<#value, String> {
                    Ok(self.get(name).cloned().unwrap_or(#null))
                }
            }
        }
    }

    /// `fn <name>(expr: &Ast, env: &<env>) -> Result<V, String>`: `eval` of the language over an
    /// environment struct whose fields are named as the scopes. A constant scope the evaluator
    /// reads as a time (`now`) or a principal must hold a value of that kind (a time; a string or
    /// null): anything else is an `Err`, where Exegra would hand the value back unchanged.
    pub(crate) fn eval_fn(&self, name: &Ident, env: &TokenStream) -> TokenStream {
        let module = self.module();
        let ast = self.plan.ast_ty();
        let value = self.plan.value_ty();
        let null = self.plan.value("null");
        let mut fields: Vec<TokenStream> = Vec::new();
        let mut inits: Vec<TokenStream> = Vec::new();
        let mut extracts: Vec<TokenStream> = Vec::new();
        let mut methods: Vec<TokenStream> = Vec::new();
        let needs_env =
            self.reads_records() || self.plan.uses(Prim::Param) || self.plan.uses(Prim::HasRole);
        if needs_env {
            fields.push(quote! { env: &'a #env });
            inits.push(quote! { env });
        }
        if self.reads_records() {
            let scope = self.plan.scope_enum_path();
            let arms: Vec<TokenStream> = self
                .plan
                .record_scopes
                .iter()
                .map(|s| {
                    let v = self.plan.scope_value(s.value);
                    let f = Self::field(s.name);
                    quote! { #v => &self.env.#f, }
                })
                .collect();
            methods.push(quote! {
                fn record(&self, scope: &#scope) -> Option<&dyn #module::Record> {
                    let record = match scope {
                        #(#arms)*
                    };
                    record.as_ref().map(|r| r as &dyn #module::Record)
                }
            });
        }
        if self.plan.uses(Prim::Through) {
            let scope = self.plan.scope_enum_path();
            let target = self.plan.payload_ty("ref");
            methods.push(quote! {
                fn follow(
                    &self,
                    _scope: &#scope,
                    _field: &str,
                    _target: &#target,
                ) -> Result<Option<Box<dyn #module::Record + '_>>, #module::EvalError> {
                    Err(#module::EvalError(
                        "following a reference needs a lookup, and none is available".to_string(),
                    ))
                }
            });
        }
        if let (true, Some(t)) = (self.plan.uses(Prim::Param), self.token_of(Prim::Param)) {
            let f = Self::field(t);
            methods.push(quote! {
                fn param(&self, name: &str) -> Option<#value> {
                    self.env.#f.get(name).cloned()
                }
            });
        }
        if let (true, Some(t)) = (self.plan.uses(Prim::Now), self.token_of(Prim::Now)) {
            let f = Self::field(t);
            let time = self.plan.payload_ty("time");
            let tv = self.plan.value("time");
            let message = lit_str(&format!("the scope {t} holds {{other:?}}, not a time"));
            fields.push(quote! { now: #time });
            inits.push(quote! { now });
            extracts.push(quote! {
                let now = match &env.#f {
                    #tv(t) => *t,
                    other => return Err(format!(#message)),
                };
            });
            methods.push(quote! {
                fn now(&self) -> #time {
                    self.now
                }
            });
        }
        if let (true, Some(t)) = (
            self.plan.uses(Prim::Principal),
            self.token_of(Prim::Principal),
        ) {
            let f = Self::field(t);
            let text = self.plan.payload_ty("string");
            let sv = self.plan.value("string");
            let message = lit_str(&format!(
                "the scope {t} holds {{other:?}}, not a principal's id or null"
            ));
            fields.push(quote! { principal: Option<#text> });
            inits.push(quote! { principal });
            extracts.push(quote! {
                let principal = match &env.#f {
                    #sv(id) => Some(id.clone()),
                    #null => None,
                    other => return Err(format!(#message)),
                };
            });
            methods.push(quote! {
                fn principal(&self) -> Option<#text> {
                    self.principal.clone()
                }
            });
        }
        if let (true, Some(t)) = (self.plan.uses(Prim::HasRole), self.token_of(Prim::HasRole)) {
            let f = Self::field(t);
            methods.push(quote! {
                fn has_role(&self, role: &str) -> bool {
                    self.env.#f.iter().any(|held| held == role)
                }
            });
        }
        let (decl, header, build) = match (fields.is_empty(), needs_env) {
            (true, _) => (
                quote! { struct Scopes; },
                quote! { Scopes },
                quote! { Scopes },
            ),
            (false, true) => (
                quote! { struct Scopes<'a> { #(#fields,)* } },
                quote! { Scopes<'_> },
                quote! { Scopes { #(#inits,)* } },
            ),
            (false, false) => (
                quote! { struct Scopes { #(#fields,)* } },
                quote! { Scopes },
                quote! { Scopes { #(#inits,)* } },
            ),
        };
        let unused = if needs_env || !extracts.is_empty() {
            TokenStream::new()
        } else {
            quote! { let _ = env; }
        };
        quote! {
            pub(super) fn #name(expr: &#ast, env: &#env) -> Result<#value, String> {
                #unused
                #(#extracts)*
                #decl

                impl #module::Env for #header {
                    #(#methods)*
                }

                #module::eval(expr, &#build).map_err(|e| e.0)
            }
        }
    }

    /// `fn <name>(expr: &Ast, env: &<env>) -> Result<String, String>`: `check` of the language
    /// against a definition site stated as a struct `{fields, params, roles}`; `Ok` of the shape's
    /// name.
    pub(crate) fn check_fn(&self, name: &Ident, env: &TokenStream) -> TokenStream {
        let module = self.module();
        let ast = self.plan.ast_ty();
        let shape_path = |s: &str| {
            let ident = naming::ident(&naming::pascal(s).unwrap_or_default());
            quote! { #module::Shape::#ident }
        };
        let any = shape_path(self.plan.lang.any_shape.as_deref().unwrap_or_default());
        let names: Vec<Literal> = self.plan.lang.shapes.iter().map(|s| lit_str(s)).collect();
        let paths: Vec<TokenStream> = self
            .plan
            .lang
            .shapes
            .iter()
            .map(|s| shape_path(s))
            .collect();
        let mut methods = Vec::new();
        if self.reads_records() {
            methods.push(quote! {
                fn field_shape(&self, through: Option<&str>, name: &str) -> Result<#module::Shape, String> {
                    if through.is_some() {
                        return Ok(#any);
                    }
                    match self.0.fields.get(name).map(String::as_str) {
                        #( Some(#names) => Ok(#paths), )*
                        Some(other) => Err(format!("the field {name} has the unknown shape {other}")),
                        None => Err(format!("unknown field {name}")),
                    }
                }
            });
        }
        if self.plan.uses(Prim::Through) {
            methods.push(quote! {
                fn follow(&self, field: &str) -> Result<(), String> {
                    if self.0.fields.contains_key(field) {
                        Ok(())
                    } else {
                        Err(format!("unknown field {field}"))
                    }
                }
            });
        }
        if self.plan.uses(Prim::Param) {
            methods.push(quote! {
                fn has_param(&self, name: &str) -> bool {
                    self.0.params.iter().any(|p| p == name)
                }
            });
        }
        if self.plan.uses(Prim::HasRole) {
            methods.push(quote! {
                fn has_role(&self, role: &str) -> bool {
                    self.0.roles.iter().any(|r| r == role)
                }
            });
        }
        let (decl, header, build) = if methods.is_empty() {
            (
                quote! { let _ = env; struct Site; },
                quote! { Site },
                quote! { Site },
            )
        } else {
            (
                quote! { struct Site<'a>(&'a #env); },
                quote! { Site<'_> },
                quote! { Site(env) },
            )
        };
        quote! {
            pub(super) fn #name(expr: &#ast, env: &#env) -> Result<String, String> {
                #decl

                impl #module::TypeEnv for #header {
                    #(#methods)*
                }

                #module::check(expr, &#build).map(|shape| {
                    let name = match shape {
                        #( #paths => #names, )*
                    };
                    name.to_string()
                })
            }
        }
    }
}

/// The readiness checks of every language.
pub fn check(spec: &Spec) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut modules: BTreeMap<String, &str> = BTreeMap::new();
    let mut asts: BTreeMap<&str, &str> = BTreeMap::new();
    for lang in &spec.languages {
        match naming::snake(&lang.id) {
            Ok(module) => {
                let reserved = ["prelude", "lib", crate::function::MODULE]
                    .contains(&module.as_str())
                    || crate::backend::BACKENDS
                        .iter()
                        .any(|b| b.module() == module.as_str());
                if reserved {
                    out.push(Diagnostic::new(
                        LANGUAGE_UNSUPPORTED,
                        &lang.id,
                        format!("the language becomes the module `{module}`, which the generated crate already has."),
                    ));
                } else if let Some(other) = modules.insert(module.clone(), &lang.id) {
                    out.push(Diagnostic::new(
                        LANGUAGE_UNSUPPORTED,
                        &lang.id,
                        format!(
                            "the languages {other} and {} both become the module `{module}`.",
                            lang.id
                        ),
                    ));
                }
            }
            Err(reason) => out.push(Diagnostic::new(
                render::NAME_INVALID,
                &lang.id,
                format!("the language id cannot become a Rust module name: {reason}."),
            )),
        }
        if let Some(ast) = &lang.ast {
            if let Some(other) = asts.insert(ast, &lang.id) {
                out.push(Diagnostic::new(
                    LANGUAGE_UNSUPPORTED,
                    &lang.id,
                    format!("the languages {other} and {} share the AST {ast}; each AST type has one printer.", lang.id),
                ));
            }
        }
        if let Err(mut d) = resolve(spec, lang) {
            out.append(&mut d);
        }
    }
    out
}

fn resolve<'a>(spec: &'a Spec, lang: &'a Language) -> Result<Plan<'a>, Vec<Diagnostic>> {
    let mut out = Vec::new();
    let id = lang.id.as_str();
    let mut err = |code: &str, message: String| out.push(Diagnostic::new(code, id, message));
    let union = |name: &Option<String>, what: &str, err: &mut dyn FnMut(&str, String)| {
        let Some(name) = name else {
            err(
                LANGUAGE_INVALID,
                format!("the language states no {what} type."),
            );
            return None;
        };
        match spec.type_decl(name) {
            Some(
                t @ TypeDecl {
                    kind: TypeKind::Union { .. },
                    ..
                },
            ) => {
                if !crate::backend::types::is_core_type(spec, name) {
                    err(
                        LANGUAGE_UNSUPPORTED,
                        format!("the {what} type {name} reaches a uuid, so it is not in the pure crate the language is generated into."),
                    );
                }
                Some(t)
            }
            _ => {
                err(
                    LANGUAGE_INVALID,
                    format!("the {what} type {name} is not a declared union."),
                );
                None
            }
        }
    };
    let ast = union(&lang.ast, "ast", &mut err);
    let values = union(&lang.values, "values", &mut err);
    let shapes: BTreeSet<&str> = lang.shapes.iter().map(String::as_str).collect();
    if shapes.is_empty() || shapes.len() != lang.shapes.len() {
        err(
            LANGUAGE_INVALID,
            "the shapes are empty or name one shape twice.".to_string(),
        );
    }
    for s in &lang.shapes {
        if naming::pascal(s).is_err() {
            err(
                render::NAME_INVALID,
                format!("the shape {s} cannot become a Rust variant name."),
            );
        }
    }
    match &lang.any_shape {
        Some(a) if shapes.contains(a.as_str()) => {}
        _ => err(
            LANGUAGE_INVALID,
            "the anyShape is not one of the shapes.".to_string(),
        ),
    }
    let (Some(ast), Some(values)) = (ast, values) else {
        return Err(out);
    };
    let TypeKind::Union {
        variants: ast_variants,
        ..
    } = &ast.kind
    else {
        return Err(out);
    };
    let TypeKind::Union {
        variants: value_variants,
        ..
    } = &values.kind
    else {
        return Err(out);
    };
    let shape_ok = |s: &str| shapes.contains(s);

    // Record scopes and the enum they select.
    let mut record_scopes = Vec::new();
    for s in &lang.scopes {
        if !["record-scope", "param", "constant", "role-test"].contains(&s.kind.as_str()) {
            err(
                LANGUAGE_INVALID,
                format!("the scope {} has an unknown kind {}.", s.name, s.kind),
            );
        }
        if s.kind == "record-scope" {
            for (_, shape) in &s.attributes {
                if !shape_ok(shape) {
                    err(
                        LANGUAGE_INVALID,
                        format!(
                            "an attribute of scope {} has the undeclared shape {shape}.",
                            s.name
                        ),
                    );
                }
            }
            match &s.value {
                Some(v) => record_scopes.push(RecordScope {
                    name: &s.name,
                    value: v,
                    attributes: &s.attributes,
                }),
                None => err(
                    LANGUAGE_UNSUPPORTED,
                    format!("the record scope {} states no value of the AST's scope field; specc reads record scopes through that field.", s.name),
                ),
            }
        }
    }

    let mut ops = Vec::new();
    let mut scope_enum: Option<&TypeDecl> = None;
    let mut value_forms = Vec::new();
    let mut value_shapes = Vec::new();
    for op in &lang.operators {
        let name = &op.name;
        let Some(info) = op.primitive.as_deref().and_then(primitive) else {
            err(
                LANGUAGE_INVALID,
                format!(
                    "operator {name} names no primitive of the catalogue ({:?}).",
                    op.primitive
                ),
            );
            continue;
        };
        if op.nulls.as_deref() != Some(info.nulls) {
            err(
                LANGUAGE_INVALID,
                format!(
                    "operator {name} states nulls {:?}; its primitive {} is {}.",
                    op.nulls, info.name, info.nulls
                ),
            );
        }
        let Some(variant) = op
            .variant
            .as_deref()
            .and_then(|v| ast_variants.iter().find(|x| x.name == v))
        else {
            err(
                LANGUAGE_INVALID,
                format!("operator {name} names no variant of {}.", ast.id),
            );
            continue;
        };
        let Some(form_name) = op.form.as_deref().filter(|f| info.forms.contains(f)) else {
            err(
                LANGUAGE_INVALID,
                format!(
                    "operator {name} is written in form {:?}; its primitive takes {:?}.",
                    op.form, info.forms
                ),
            );
            continue;
        };
        let token = op.token.clone();
        let need_token = |err: &mut dyn FnMut(&str, String)| -> Option<String> {
            if token.is_none() {
                err(
                    LANGUAGE_INVALID,
                    format!("operator {name} states no token."),
                );
            }
            token.clone()
        };
        let form = match form_name {
            "infix" | "keyword-infix" => {
                let Some(token) = need_token(&mut err) else {
                    continue;
                };
                let assoc = match op.assoc.as_deref() {
                    Some("left") => Assoc::Left,
                    Some("right") => Assoc::Right,
                    Some("none") => Assoc::None,
                    Some("n-ary") => Assoc::NAry,
                    other => {
                        err(
                            LANGUAGE_INVALID,
                            format!("binary operator {name} has the associativity {other:?}."),
                        );
                        continue;
                    }
                };
                if (assoc == Assoc::NAry) != (info.arity == Arity::List) {
                    err(LANGUAGE_INVALID, format!("operator {name}: n-ary associativity goes with a list primitive, and only with one."));
                    continue;
                }
                let Some(precedence) = op.precedence.filter(|p| *p > 0) else {
                    err(
                        LANGUAGE_INVALID,
                        format!("binary operator {name} states no positive precedence."),
                    );
                    continue;
                };
                let symbol = form_name == "infix";
                let lexical_ok = if symbol {
                    !token.is_empty() && token.chars().all(|c| "=!<>+-*/%&|^~?:".contains(c))
                } else {
                    is_word(&token)
                };
                if !lexical_ok {
                    err(LANGUAGE_INVALID, format!("operator {name} has the token {token:?}, which is not in its lexical class."));
                    continue;
                }
                Form::Binary {
                    symbol,
                    token,
                    precedence,
                    assoc,
                }
            }
            "prefix-word" => {
                let Some(token) = need_token(&mut err) else {
                    continue;
                };
                let Some(precedence) = op.precedence.filter(|p| *p > 0) else {
                    err(
                        LANGUAGE_INVALID,
                        format!("prefix operator {name} states no positive precedence."),
                    );
                    continue;
                };
                if !is_word(&token) {
                    err(
                        LANGUAGE_INVALID,
                        format!("operator {name} has the token {token:?}, which is not a word."),
                    );
                    continue;
                }
                Form::Prefix { token, precedence }
            }
            "call" => {
                let Some(token) = need_token(&mut err) else {
                    continue;
                };
                if !is_word(&token) {
                    err(
                        LANGUAGE_INVALID,
                        format!("operator {name} has the token {token:?}, which is not a word."),
                    );
                    continue;
                }
                Form::Call { token }
            }
            "literal" => {
                let Some(token) = need_token(&mut err) else {
                    continue;
                };
                let allowed: &[&str] = if info.prim == Prim::ListLiteral {
                    &["list"]
                } else {
                    &["null", "bool", "int", "string", "time", "ref"]
                };
                if !allowed.contains(&token.as_str()) {
                    err(LANGUAGE_INVALID, format!("literal operator {name} has the form {token:?}; its primitive takes {allowed:?}."));
                    continue;
                }
                Form::Literal { form: token }
            }
            _ => {
                let kind = info.scope.unwrap_or_default();
                if kind == "record-scope" {
                    if token.is_some() {
                        err(
                            LANGUAGE_INVALID,
                            format!("operator {name} reads the record scopes and takes no token."),
                        );
                    }
                    if record_scopes.is_empty() {
                        err(
                            LANGUAGE_INVALID,
                            format!(
                                "operator {name} reads record scopes; the language declares none."
                            ),
                        );
                    }
                } else {
                    let Some(t) = need_token(&mut err) else {
                        continue;
                    };
                    if !lang.scopes.iter().any(|s| s.name == t && s.kind == kind) {
                        err(LANGUAGE_INVALID, format!("operator {name} reads the scope {t}, which is not a declared {kind} scope."));
                    }
                }
                Form::Scope { token }
            }
        };

        // The variant's fields, by role.
        let ast_name = ast.id.as_str();
        let is_expr = |t: &str| TypeRef::parse(t) == TypeRef::Declared(ast_name.to_string());
        let is_expr_list = |t: &str| {
            TypeRef::parse(t) == TypeRef::List(Box::new(TypeRef::Declared(ast_name.to_string())))
        };
        let mut case = None;
        if let Some((field, value)) = &op.case {
            match variant.fields.iter().position(|f| &f.name == field) {
                Some(i) => case = Some((i, value.clone())),
                None => {
                    err(
                        LANGUAGE_INVALID,
                        format!(
                            "operator {name} splits {} on {field}, which it does not have.",
                            variant.name
                        ),
                    );
                    continue;
                }
            }
        }
        let mut exprs = Vec::new();
        let mut others = Vec::new();
        for (i, f) in variant.fields.iter().enumerate() {
            if f.optional {
                err(LANGUAGE_UNSUPPORTED, format!("variant {} has the optional field {}; specc generates languages over required fields.", variant.name, f.name));
            }
            if case.as_ref().is_some_and(|(c, _)| *c == i) {
                continue;
            }
            if is_expr(&f.type_name) || is_expr_list(&f.type_name) {
                exprs.push(i);
            } else if matches!(TypeRef::parse(&f.type_name), TypeRef::Option(_)) {
                err(LANGUAGE_UNSUPPORTED, format!("variant {} has the field {} of an option type; specc generates expression operands that are always present.", variant.name, f.name));
            } else {
                others.push(i);
            }
        }
        let field_type = |i: usize| TypeRef::parse(&variant.fields[i].type_name);
        let operand_fields_ok = match info.arity {
            Arity::List => exprs.len() == 1 && is_expr_list(&variant.fields[exprs[0]].type_name),
            Arity::Fixed(n) => {
                exprs.len() == n && exprs.iter().all(|i| is_expr(&variant.fields[*i].type_name))
            }
        };
        let others_ok = match info.prim {
            Prim::Var | Prim::Through => {
                let strings = if info.prim == Prim::Var { 1 } else { 2 };
                others.len() == 1 + strings
                    && others[1..]
                        .iter()
                        .all(|i| field_type(*i) == TypeRef::Builtin(Builtin::String))
                    && match field_type(others[0]) {
                        TypeRef::Declared(e) => match spec.type_decl(&e) {
                            Some(t) if unit_values(t).is_some() => {
                                if scope_enum.is_some_and(|s| s.id != t.id) {
                                    false
                                } else {
                                    scope_enum = Some(t);
                                    true
                                }
                            }
                            _ => false,
                        },
                        _ => false,
                    }
            }
            Prim::Param | Prim::HasRole => {
                others.len() == 1 && is_text(spec, &field_type(others[0]))
            }
            _ => others.is_empty(),
        };
        if !operand_fields_ok || !others_ok {
            err(
                LANGUAGE_INVALID,
                format!(
                    "operator {name}: the fields of variant {} do not fit the primitive {}.",
                    variant.name, info.name
                ),
            );
            continue;
        }
        // Operand and result shapes.
        let expected_operands = match info.arity {
            Arity::List => 1,
            Arity::Fixed(n) => n,
        };
        if op.operands.len() != expected_operands {
            err(
                LANGUAGE_INVALID,
                format!(
                    "operator {name} states {} operands; its primitive takes {expected_operands}.",
                    op.operands.len()
                ),
            );
            continue;
        }
        for s in op.operands.iter().flatten() {
            if !shape_ok(s) {
                err(
                    LANGUAGE_INVALID,
                    format!("operator {name} has an operand of the undeclared shape {s}."),
                );
            }
        }
        match info.prim {
            Prim::Var | Prim::Through => {}
            Prim::If => {
                if op.result.as_deref() != Some("operand:2") {
                    err(LANGUAGE_UNSUPPORTED, format!("operator {name}: the if primitive's result is its then branch (operand:2), with the else branch's shape when that is the any shape."));
                }
            }
            _ => match op.result.as_deref() {
                Some(r) if shape_ok(r) => {}
                // The shape of an operand (`operand:1` of + and -), for a primitive whose
                // operands the checker checks one after another.
                Some(r) if operand_ref(r).is_some() => {
                    let n = operand_ref(r).unwrap_or_default();
                    if !info.prim.checked_in_turn() || n == 0 || n > expected_operands {
                        err(LANGUAGE_UNSUPPORTED, format!("operator {name} has the result {r}; specc gives the shape of an operand only for a primitive whose operands are checked one after another, and only of an operand it has."));
                    }
                }
                other => err(
                    LANGUAGE_INVALID,
                    format!(
                        "operator {name} has the result {other:?}, which is not a declared shape."
                    ),
                ),
            },
        }
        // Literal operators split the variant holding a value, by value variant.
        if matches!(form, Form::Literal { .. }) {
            let holds_value = variant.fields.len() == 1
                && TypeRef::parse(&variant.fields[0].type_name)
                    == TypeRef::Declared(values.id.clone());
            let Some((_, value_name)) = case.as_ref().filter(|_| holds_value) else {
                err(LANGUAGE_UNSUPPORTED, format!("literal operator {name} must split a variant whose one field is the value type {}, on that field.", values.id));
                continue;
            };
            let Some(vv) = value_variants.iter().find(|v| &v.name == value_name) else {
                err(
                    LANGUAGE_INVALID,
                    format!(
                        "operator {name}: {value_name} is not a variant of {}.",
                        values.id
                    ),
                );
                continue;
            };
            let form_name = op.token.clone().unwrap_or_default();
            let payload_ok = match form_name.as_str() {
                "null" => vv.fields.is_empty(),
                "list" => {
                    vv.newtype
                        && vv.fields.len() == 1
                        && TypeRef::parse(&vv.fields[0].type_name)
                            == TypeRef::List(Box::new(TypeRef::Declared(values.id.clone())))
                }
                other => {
                    let allowed = LITERAL_FORMS
                        .iter()
                        .find(|(f, _, _)| *f == other)
                        .map(|(_, _, b)| *b)
                        .unwrap_or(&[]);
                    // A time or a reference may also be an unconstrained id over one of the
                    // allowed built-ins (`Ref(RecordId)`): printed through its `Display`, read
                    // back by wrapping the number.
                    let wrapped_ok = |id: &str| {
                        ["time", "ref"].contains(&other)
                            && wrapped_builtin(spec, id).is_some_and(|b| allowed.contains(&b))
                    };
                    vv.newtype
                        && vv.fields.len() == 1
                        && match TypeRef::parse(&vv.fields[0].type_name) {
                            TypeRef::Builtin(b) => allowed.contains(&b),
                            TypeRef::Declared(id) => wrapped_ok(&id),
                            _ => false,
                        }
                }
            };
            if !payload_ok {
                err(LANGUAGE_UNSUPPORTED, format!("the value variant {value_name} does not hold what the {form_name} literal form needs."));
                continue;
            }
            if value_forms.iter().any(|(_, f)| f == &form_name) {
                err(
                    LANGUAGE_INVALID,
                    format!("two value variants take the literal form {form_name}."),
                );
            }
            value_forms.push((value_name.clone(), form_name));
            value_shapes.push((value_name.clone(), op.result.clone().unwrap_or_default()));
        } else if let Some((ci, value)) = &case {
            let ok = match TypeRef::parse(&variant.fields[*ci].type_name) {
                TypeRef::Declared(e) => spec
                    .type_decl(&e)
                    .and_then(unit_values)
                    .is_some_and(|values| values.contains(value)),
                _ => false,
            };
            if !ok {
                err(LANGUAGE_UNSUPPORTED, format!("operator {name}: a non-literal operator splits its variant on a value of an enum field; {value} is not one."));
                continue;
            }
        }
        ops.push(Op {
            op,
            info,
            variant,
            case,
            exprs,
            form,
        });
    }

    // Every variant (and every case) has exactly one operator, so every match is exhaustive.
    for v in ast_variants {
        let on: Vec<&Op> = ops.iter().filter(|o| o.variant.name == v.name).collect();
        if on.is_empty() {
            err(
                LANGUAGE_INVALID,
                format!("the AST variant {} has no operator.", v.name),
            );
            continue;
        }
        let cased = on.iter().filter(|o| o.case.is_some()).count();
        if (cased != 0 && cased != on.len()) || (cased == 0 && on.len() > 1) {
            err(LANGUAGE_INVALID, format!("the AST variant {} has more than one operator, or a mix of split and plain ones.", v.name));
            continue;
        }
        if cased > 0 {
            let fields: BTreeSet<usize> = on
                .iter()
                .filter_map(|o| o.case.as_ref().map(|c| c.0))
                .collect();
            let cases: Vec<&str> = on
                .iter()
                .filter_map(|o| o.case.as_ref().map(|c| c.1.as_str()))
                .collect();
            let all: Vec<String> = match fields
                .iter()
                .next()
                .map(|i| TypeRef::parse(&v.fields[*i].type_name))
            {
                Some(TypeRef::Declared(t)) => match spec.type_decl(&t).map(|d| &d.kind) {
                    Some(TypeKind::Enum { values }) => values.clone(),
                    Some(TypeKind::Union { variants, .. }) => {
                        variants.iter().map(|x| x.name.clone()).collect()
                    }
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
            let distinct: BTreeSet<&str> = cases.iter().copied().collect();
            if fields.len() != 1
                || distinct.len() != cases.len()
                || all.iter().any(|a| !distinct.contains(a.as_str()))
            {
                err(
                    LANGUAGE_INVALID,
                    format!(
                        "the cases of the AST variant {} are not each value of one field, once.",
                        v.name
                    ),
                );
            }
        }
    }
    if ops
        .iter()
        .any(|o| matches!(o.info.prim, Prim::Var | Prim::Through))
    {
        if let Some((evs, eid)) = scope_enum.and_then(|e| Some((unit_values(e)?, &e.id))) {
            let scoped: BTreeSet<&str> = record_scopes.iter().map(|s| s.value).collect();
            let all: BTreeSet<&str> = evs.iter().map(String::as_str).collect();
            if scoped != all || scoped.len() != record_scopes.len() {
                err(
                    LANGUAGE_INVALID,
                    format!("the record scopes' values are not each value of {eid}, once."),
                );
            }
        }
    }
    // Binary levels: one associativity per level, and an n-ary level holds one operator.
    let mut levels: BTreeMap<u64, Vec<(Assoc, &str)>> = BTreeMap::new();
    for o in &ops {
        if let Form::Binary {
            precedence,
            assoc,
            token,
            ..
        } = &o.form
        {
            levels
                .entry(*precedence)
                .or_default()
                .push((*assoc, token.as_str()));
        }
    }
    for (p, at) in &levels {
        if at.iter().any(|(a, _)| *a != at[0].0) {
            err(
                LANGUAGE_INVALID,
                format!("the binary operators of precedence {p} differ in associativity."),
            );
        }
        if at[0].0 == Assoc::NAry && at.len() > 1 {
            err(LANGUAGE_UNSUPPORTED, format!("precedence {p} has several n-ary operators; specc collects a run of one operator into its list."));
        }
    }
    // The value forms each primitive builds or inspects.
    let has_form = |f: &str| value_forms.iter().any(|(_, x)| x == f);
    for o in &ops {
        let needs: &[&str] = match o.info.prim {
            Prim::And3
            | Prim::Or3
            | Prim::Not3
            | Prim::IsNull
            | Prim::IsSlug
            | Prim::If
            | Prim::HasRole
            | Prim::Eq
            | Prim::Ne => &["null", "bool"],
            Prim::Lt | Prim::Le | Prim::Gt | Prim::Ge => &["null", "bool"],
            Prim::InList => &["null", "bool", "list"],
            Prim::Len => &["null", "int", "string", "list"],
            Prim::Lower => &["null", "string"],
            Prim::Contains => &["null", "bool", "string", "list"],
            Prim::StartsWith => &["null", "bool", "string"],
            Prim::Now => &["time"],
            Prim::Principal => &["null", "string"],
            Prim::Param | Prim::Var => &["null"],
            Prim::Through => &["null", "ref"],
            Prim::Literal | Prim::ListLiteral => &[],
            Prim::AddChecked
            | Prim::SubChecked
            | Prim::MulChecked
            | Prim::DaysMs
            | Prim::HoursMs
            | Prim::MinutesMs => &["null", "int"],
            Prim::TimeDifference => &["null", "int", "time"],
            Prim::ConcatChecked | Prim::ToText => &["null", "string"],
        };
        for f in needs {
            if !has_form(*f) {
                err(LANGUAGE_UNSUPPORTED, format!("operator {} needs values of the {f} literal form, which no literal operator gives.", o.op.name));
            }
        }
    }
    // Time arithmetic computes on the time payload as an i64 of milliseconds.
    let time_payload = value_forms
        .iter()
        .find(|(_, f)| f == "time")
        .and_then(|(name, _)| value_variants.iter().find(|v| &v.name == name))
        .and_then(|vv| vv.fields.first())
        .map(|f| TypeRef::parse(&f.type_name));
    let time_arithmetic = ops.iter().any(|o| {
        matches!(
            o.info.prim,
            Prim::AddChecked | Prim::SubChecked | Prim::TimeDifference
        )
    });
    if time_arithmetic && time_payload.is_some_and(|t| t != TypeRef::Builtin(Builtin::I64)) {
        err(LANGUAGE_UNSUPPORTED, "time arithmetic needs the time literal's value variant to hold an i64 of milliseconds.".to_string());
    }
    if ops.iter().any(|o| o.info.prim == Prim::Contains)
        && !value_shapes
            .iter()
            .any(|(v, _)| value_forms.iter().any(|(w, f)| w == v && f == "string"))
    {
        err(
            LANGUAGE_UNSUPPORTED,
            "contains needs the shape of a string literal.".to_string(),
        );
    }
    if !out.is_empty() {
        return Err(out);
    }
    Ok(Plan {
        spec,
        lang,
        ast,
        values,
        ops,
        record_scopes,
        scope_enum,
        value_forms,
        value_shapes,
    })
}

/// The values of an enumeration, or the variant names of a union whose variants all have no
/// fields: either is a Rust enum of unit variants named in PascalCase, which is all a language
/// needs of a scope or a case field.
fn unit_values(decl: &TypeDecl) -> Option<Vec<String>> {
    match &decl.kind {
        TypeKind::Enum { values } => Some(values.clone()),
        TypeKind::Union { variants, .. } if variants.iter().all(|v| v.fields.is_empty()) => {
            Some(variants.iter().map(|v| v.name.clone()).collect())
        }
        _ => None,
    }
}

/// The built-in an unconstrained declared `id` wraps (`record-id` over `u64`), as a newtype
/// struct `Name(pub T)`.
fn wrapped_builtin(spec: &Spec, id: &str) -> Option<Builtin> {
    let decl = spec.type_decl(id)?;
    match &decl.kind {
        TypeKind::Id { repr } if decl.constraints.is_empty() => {
            repr.as_deref().and_then(Builtin::parse)
        }
        _ => None,
    }
}

/// True when the type is text: `string`, or a declared `id` over `string` (`role-id`), which
/// the generated type gives `as_str` and `Display`.
fn is_text(spec: &Spec, t: &TypeRef) -> bool {
    match t {
        TypeRef::Builtin(b) => *b == Builtin::String,
        TypeRef::Declared(id) => spec.type_decl(id).is_some_and(|d| {
            matches!(&d.kind, TypeKind::Id { repr } if repr.as_deref().and_then(Builtin::parse) == Some(Builtin::String))
        }),
        _ => false,
    }
}

/// `n` of a result written `operand:<n>`.
fn operand_ref(result: &str) -> Option<usize> {
    result.strip_prefix("operand:")?.parse().ok()
}

fn is_word(token: &str) -> bool {
    let mut chars = token.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------------------------

/// The module of a language. Assumes [`check`] passed.
pub fn emit(spec: &Spec, lang: &Language) -> TokenStream {
    match resolve(spec, lang) {
        Ok(plan) => plan.emit(),
        Err(_) => TokenStream::new(),
    }
}

/// The field index of the `k`-th field of an operator's variant that is neither an expression
/// operand nor its case field.
fn other_index(o: &Op, k: usize) -> usize {
    (0..o.variant.fields.len())
        .filter(|i| !o.exprs.contains(i) && o.case.as_ref().is_none_or(|c| c.0 != *i))
        .nth(k)
        .unwrap_or_default()
}

fn lit_str(s: &str) -> Literal {
    Literal::string(s)
}

fn v(i: usize) -> Ident {
    format_ident!("v{}", i)
}

impl<'a> Plan<'a> {
    fn type_path(&self, decl: &TypeDecl) -> TokenStream {
        let name = naming::ident(&naming::pascal(&decl.id).unwrap_or_default());
        quote! { types::#name }
    }

    fn ast_ty(&self) -> TokenStream {
        self.type_path(self.ast)
    }

    fn value_ty(&self) -> TokenStream {
        self.type_path(self.values)
    }

    fn shape(&self, name: &str) -> TokenStream {
        let ident = naming::ident(&naming::pascal(name).unwrap_or_default());
        quote! { Shape::#ident }
    }

    fn any(&self) -> TokenStream {
        self.shape(self.lang.any_shape.as_deref().unwrap_or_default())
    }

    /// The value variant of a literal form, as a path (`types::Value::Bool`).
    fn value(&self, form: &str) -> TokenStream {
        let name = self
            .value_forms
            .iter()
            .find(|(_, f)| f == form)
            .map(|(v, _)| v.as_str())
            .unwrap_or_default();
        let ident = naming::ident(&naming::pascal(name).unwrap_or_default());
        let ty = self.value_ty();
        quote! { #ty::#ident }
    }

    fn has_form(&self, form: &str) -> bool {
        self.value_forms.iter().any(|(_, f)| f == form)
    }

    /// The payload type of a literal form's value variant, as written.
    fn payload_ref(&self, form: &str) -> Option<TypeRef> {
        let (name, _) = self.value_forms.iter().find(|(_, f)| f == form)?;
        let TypeKind::Union { variants, .. } = &self.values.kind else {
            return None;
        };
        let vv = variants.iter().find(|v| &v.name == name)?;
        Some(TypeRef::parse(&vv.fields.first()?.type_name))
    }

    /// The payload builtin of a literal form's value variant: the built-in itself, or the one
    /// a declared id wraps ([`wrapped_builtin`]).
    fn payload(&self, form: &str) -> Option<Builtin> {
        match self.payload_ref(form)? {
            TypeRef::Builtin(b) => Some(b),
            TypeRef::Declared(id) => wrapped_builtin(self.spec, &id),
            _ => None,
        }
    }

    /// The declared id a literal form's payload is, when it is one.
    fn payload_decl(&self, form: &str) -> Option<&'a TypeDecl> {
        match self.payload_ref(form)? {
            TypeRef::Declared(id) => self.spec.type_decl(&id),
            _ => None,
        }
    }

    fn payload_ty(&self, form: &str) -> TokenStream {
        let mut uses = Uses::default();
        if let Some(decl) = self.payload_decl(form) {
            return self.type_path(decl);
        }
        match self.payload(form) {
            Some(b) => render::builtin_type(b, &mut uses),
            None => quote! { () },
        }
    }

    /// `value` (of the payload's built-in) as the payload: wrapped when it is a declared id.
    fn wrap_payload(&self, form: &str, value: TokenStream) -> TokenStream {
        match self.payload_decl(form) {
            Some(decl) => {
                let path = self.type_path(decl);
                quote! { #path(#value) }
            }
            None => value,
        }
    }

    /// The declared text id of a field (`role-id`), when it is one rather than a `string`.
    fn text_decl(&self, variant: &Variant, i: usize) -> Option<&'a TypeDecl> {
        match TypeRef::parse(&variant.fields[i].type_name) {
            TypeRef::Declared(id) => self.spec.type_decl(&id),
            _ => None,
        }
    }

    /// The field `v<i>` as a `&str`: itself for a `String`, `.as_str()` of a text id.
    fn text_ref(&self, variant: &Variant, i: usize) -> TokenStream {
        let b = v(i);
        if self.text_decl(variant, i).is_some() {
            quote! { #b.as_str() }
        } else {
            quote! { #b }
        }
    }

    /// The statements that turn the `String` bound to `name` into the field's type in place:
    /// nothing for a `string`, the id's constructor (its constraints checked, a violation
    /// refused as the kernel refuses it) for a text id.
    fn text_conversion(&self, variant: &Variant, i: usize, name: &Ident) -> TokenStream {
        match self.text_decl(variant, i) {
            None => TokenStream::new(),
            Some(decl) => {
                let path = self.type_path(decl);
                if decl.constraints.is_empty() {
                    quote! { let #name = #path(#name); }
                } else {
                    quote! { let #name = #path::new(#name).map_err(|e| e.to_string())?; }
                }
            }
        }
    }

    fn variant_ident(v: &Variant) -> Ident {
        naming::ident(&naming::pascal(&v.name).unwrap_or_default())
    }

    fn uses(&self, p: Prim) -> bool {
        self.ops.iter().any(|o| o.info.prim == p)
    }

    fn scope_enum_path(&self) -> TokenStream {
        match self.scope_enum {
            Some(e) => self.type_path(e),
            None => quote! { () },
        }
    }

    fn scope_value(&self, value: &str) -> TokenStream {
        let ty = self.scope_enum_path();
        let ident = naming::ident(&naming::pascal(value).unwrap_or_default());
        quote! { #ty::#ident }
    }

    fn case_value(&self, op: &Op) -> TokenStream {
        let (i, value) = op.case.as_ref().expect("a split operator has a case");
        let field = &op.variant.fields[*i];
        let decl = match TypeRef::parse(&field.type_name) {
            TypeRef::Declared(t) => self.spec.type_decl(&t),
            _ => None,
        };
        let ty = decl
            .map(|d| self.type_path(d))
            .unwrap_or_else(|| quote! { () });
        let ident = naming::ident(&naming::pascal(value).unwrap_or_default());
        quote! { #ty::#ident }
    }

    fn boxed(&self, variant: &Variant, i: usize) -> bool {
        crate::backend::types::field_is_boxed(self.spec, self.ast, &variant.fields[i])
    }

    /// A pattern of the variant binding the fields in `used` to `v<i>` and ignoring the rest.
    fn pattern(&self, variant: &Variant, used: &[usize]) -> TokenStream {
        let ty = self.ast_ty();
        let ident = Self::variant_ident(variant);
        if variant.fields.is_empty() {
            return quote! { #ty::#ident };
        }
        if variant.newtype || variant.tuple {
            // Positional fields (a newtype's one, a tuple variant's two or more), in order.
            let binds: Vec<TokenStream> = (0..variant.fields.len())
                .map(|i| {
                    if used.contains(&i) {
                        let b = v(i);
                        quote! { #b }
                    } else {
                        quote! { _ }
                    }
                })
                .collect();
            return quote! { #ty::#ident(#(#binds),*) };
        }
        let fields: Vec<TokenStream> = variant
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let name = naming::ident(&naming::snake(&f.name).unwrap_or_default());
                if used.contains(&i) {
                    let b = v(i);
                    quote! { #name: #b }
                } else {
                    quote! { #name: _ }
                }
            })
            .collect();
        quote! { #ty::#ident { #(#fields),* } }
    }

    /// The variant built from one value per field (expression fields boxed as the type holds them).
    fn construct(&self, variant: &Variant, values: Vec<TokenStream>) -> TokenStream {
        let ty = self.ast_ty();
        let ident = Self::variant_ident(variant);
        let values: Vec<TokenStream> = values
            .into_iter()
            .enumerate()
            .map(|(i, value)| {
                if self.boxed(variant, i) {
                    quote! { Box::new(#value) }
                } else {
                    value
                }
            })
            .collect();
        if variant.fields.is_empty() {
            return quote! { #ty::#ident };
        }
        if variant.newtype || variant.tuple {
            return quote! { #ty::#ident(#(#values),*) };
        }
        let fields: Vec<TokenStream> = variant
            .fields
            .iter()
            .zip(values)
            .map(|(f, value)| {
                let name = naming::ident(&naming::snake(&f.name).unwrap_or_default());
                quote! { #name: #value }
            })
            .collect();
        quote! { #ty::#ident { #(#fields),* } }
    }

    /// An operator's variant built from its expression operands (in order) and its case.
    fn construct_op(&self, op: &Op, operands: Vec<TokenStream>) -> TokenStream {
        let mut operands = operands.into_iter();
        let values: Vec<TokenStream> = (0..op.variant.fields.len())
            .map(|i| {
                if op.case.as_ref().is_some_and(|(c, _)| *c == i) {
                    self.case_value(op)
                } else {
                    operands.next().unwrap_or_default()
                }
            })
            .collect();
        self.construct(op.variant, values)
    }

    /// The literal-carrying variant (`Lit`), if the language has literal operators.
    fn literal_variant(&self) -> Option<&'a Variant> {
        self.ops
            .iter()
            .find(|o| matches!(o.form, Form::Literal { .. }))
            .map(|o| o.variant)
    }

    /// `Lit(<value>)`.
    fn literal(&self, value: TokenStream) -> TokenStream {
        match self.literal_variant() {
            Some(variant) => self.construct(variant, vec![value]),
            None => quote! { unreachable!() },
        }
    }

    /// The operators of each AST variant, in variant order.
    fn by_variant(&self) -> Vec<(&'a Variant, Vec<&Op<'a>>)> {
        let ast: &'a TypeDecl = self.ast;
        let TypeKind::Union { variants, .. } = &ast.kind else {
            return Vec::new();
        };
        variants
            .iter()
            .map(|v| {
                (
                    v,
                    self.ops
                        .iter()
                        .filter(|o| o.variant.name == v.name)
                        .collect(),
                )
            })
            .collect()
    }

    fn emit(&self) -> TokenStream {
        let docs = self.module_docs();
        let shapes = self.shape_enum();
        let errors = quote! {
            /// An evaluation error: a type error, or a read the environment refused. Never a silent `false`.
            #[derive(Clone, Debug, PartialEq, Eq)]
            pub struct EvalError(pub String);

            impl fmt::Display for EvalError {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str(&self.0)
                }
            }

            impl std::error::Error for EvalError {}
        };
        let traits = self.traits();
        let parser = self.parser();
        let printer = self.printer();
        let evaluator = self.evaluator();
        let checker = self.checker();
        quote! {
            #docs
            use std::fmt;

            use crate::types;

            #shapes
            #errors
            #traits
            #parser
            #printer
            #evaluator
            #checker
        }
    }

    fn module_docs(&self) -> TokenStream {
        let mut lines = vec![format!(
            " {} (`{}`): its parser, printer, static checker and evaluator, generated from the language entity and specc's primitive catalogue.",
            self.lang.name, self.lang.id
        )];
        if let Some(d) = &self.lang.description {
            lines.push(String::new());
            lines.push(format!(
                " {}",
                d.split_whitespace().collect::<Vec<_>>().join(" ")
            ));
        }
        lines.push(String::new());
        lines.push(format!(
            " Expressions are `types::{}` and evaluate to `types::{}`. The printer (`Display`) prints the authoring syntax `parse` reads back.",
            naming::pascal(&self.ast.id).unwrap_or_default(),
            naming::pascal(&self.values.id).unwrap_or_default()
        ));
        let items: Vec<String> = self
            .ops
            .iter()
            .map(|o| {
                let form = match &o.form {
                    Form::Binary {
                        symbol,
                        token,
                        precedence,
                        assoc,
                    } => format!(
                        "{} `{token}`, precedence {precedence}, {}",
                        if *symbol { "infix" } else { "keyword-infix" },
                        match assoc {
                            Assoc::Left => "left",
                            Assoc::Right => "right",
                            Assoc::None => "non-associative",
                            Assoc::NAry => "n-ary",
                        }
                    ),
                    Form::Prefix { token, precedence } => {
                        format!("prefix `{token}`, precedence {precedence}")
                    }
                    Form::Call { token } => format!("call `{token}(...)`"),
                    Form::Literal { form } => format!("{form} literal"),
                    Form::Scope { token: Some(t) } => format!("scope `{t}`"),
                    Form::Scope { token: None } => "record scope read".to_string(),
                };
                format!(
                    "`{}`: {form}; primitive `{}`, nulls `{}`; pinned to `{}`.",
                    o.op.name, o.info.name, o.info.nulls, o.info.kernel
                )
            })
            .collect();
        lines.push(String::new());
        lines.push(" Operators:".to_string());
        lines.push(String::new());
        lines.extend(items.iter().map(|i| format!(" - {i}")));
        quote! { #( #![doc = #lines] )* }
    }

    fn shape_enum(&self) -> TokenStream {
        let idents: Vec<Ident> = self
            .lang
            .shapes
            .iter()
            .map(|s| naming::ident(&naming::pascal(s).unwrap_or_default()))
            .collect();
        let docs: Vec<String> = self
            .lang
            .shapes
            .iter()
            .map(|s| {
                if Some(s) == self.lang.any_shape.as_ref() {
                    format!(" The shape `{s}`: null, and what the check cannot pin down.")
                } else {
                    format!(" The shape `{s}`.")
                }
            })
            .collect();
        quote! {
            /// The statically known shape of an expression.
            #[derive(Clone, Copy, Debug, PartialEq, Eq)]
            pub enum Shape {
                #( #[doc = #docs] #idents, )*
            }
        }
    }

    fn traits(&self) -> TokenStream {
        let value = self.value_ty();
        let mut env = Vec::new();
        let mut type_env = Vec::new();
        let records =
            !self.record_scopes.is_empty() && (self.uses(Prim::Var) || self.uses(Prim::Through));
        let scope = self.scope_enum_path();
        let record = if records {
            env.push(quote! {
                /// The record a record scope reads, or `None` when the scope has none (such as the old record on a create).
                fn record(&self, scope: &#scope) -> Option<&dyn Record>;
            });
            type_env.push(quote! {
                /// The shape of a field that is not a system attribute: of the definition site's record, or, with `through`, of the record that reference field points at. An error when the name is not a field id or names no field.
                fn field_shape(&self, through: Option<&str>, name: &str) -> Result<Shape, String>;
            });
            quote! {
                /// A record an expression reads.
                pub trait Record {
                    /// A system attribute the scope declares (only those are asked for).
                    fn system_attribute(&self, name: &str) -> #value;
                    /// A field's value, null when unset; an error, with the reason, when the name is not a valid field id.
                    fn field(&self, name: &str) -> Result<#value, String>;
                }
            }
        } else {
            TokenStream::new()
        };
        if self.uses(Prim::Through) {
            let target = self.payload_ty("ref");
            env.push(quote! {
                /// The record a reference field of the scope's record points at: `None` when it does not exist; an error when the field is not a single reference, or the reference cannot be followed here.
                fn follow(&self, scope: &#scope, field: &str, target: &#target) -> Result<Option<Box<dyn Record + '_>>, EvalError>;
            });
            type_env.push(quote! {
                /// Checks that a field of the definition site's record is a single reference to an existing object type.
                fn follow(&self, field: &str) -> Result<(), String>;
            });
        }
        if self.uses(Prim::Param) {
            env.push(quote! {
                /// A named parameter, `None` when it is not supplied.
                fn param(&self, name: &str) -> Option<#value>;
            });
            type_env.push(quote! {
                /// Whether the definition site declares the parameter.
                fn has_param(&self, name: &str) -> bool;
            });
        }
        if self.uses(Prim::Now) {
            let time = self.payload_ty("time");
            env.push(quote! {
                /// The context timestamp.
                fn now(&self) -> #time;
            });
        }
        if self.uses(Prim::Principal) {
            let text = self.payload_ty("string");
            env.push(quote! {
                /// The acting principal's id for a user, `None` for an anonymous or system principal.
                fn principal(&self) -> Option<#text>;
            });
        }
        if self.uses(Prim::HasRole) {
            env.push(quote! {
                /// Whether the acting principal holds the role.
                fn has_role(&self, role: &str) -> bool;
            });
            type_env.push(quote! {
                /// Whether the role is declared at the definition site.
                fn has_role(&self, role: &str) -> bool;
            });
        }
        quote! {
            #record

            /// Everything an expression may observe.
            pub trait Env {
                #(#env)*
            }

            /// What an expression may refer to at its definition site.
            pub trait TypeEnv {
                #(#type_env)*
            }
        }
    }

    // --- parser ------------------------------------------------------------------------------

    fn parser(&self) -> TokenStream {
        let ast = self.ast_ty();
        // Symbols: the infix tokens and the punctuation the parser keeps, longest first.
        let mut symbols: Vec<String> = Vec::new();
        for o in &self.ops {
            if let Form::Binary {
                symbol: true,
                token,
                ..
            } = &o.form
            {
                if !symbols.contains(token) {
                    symbols.push(token.clone());
                }
            }
        }
        for p in ["(", ")", "[", "]", ",", "."] {
            symbols.push(p.to_string());
        }
        symbols.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
        let longest = symbols.iter().map(|s| s.chars().count()).max().unwrap_or(1);
        let count = symbols.len();
        let longest = Literal::usize_unsuffixed(longest);
        let count = Literal::usize_unsuffixed(count);

        let words = self.ops.iter().any(|o| {
            matches!(
                o.form,
                Form::Binary { symbol: false, .. } | Form::Prefix { .. }
            )
        });
        let idents = self.ops.iter().any(|o| {
            matches!(
                o.info.prim,
                Prim::Var | Prim::Through | Prim::Param | Prim::HasRole
            )
        });
        let int_args = self
            .ops
            .iter()
            .any(|o| matches!(&o.form, Form::Literal { form } if form == "time" || form == "ref"));
        let calls = self.ops.iter().any(|o| matches!(o.form, Form::Call { .. }));
        let word_fn = if words {
            quote! {
                fn word(&mut self, word: &str) -> bool {
                    if matches!(self.peek(), Some(Token::Ident(w)) if w == word) {
                        self.at += 1;
                        true
                    } else {
                        false
                    }
                }
            }
        } else {
            TokenStream::new()
        };
        let ident_fn = if idents {
            quote! {
                fn ident(&mut self) -> Result<String, String> {
                    match self.next() {
                        Some(Token::Ident(name)) => Ok(name),
                        other => Err(format!("expected a name, found {other:?}")),
                    }
                }
            }
        } else {
            TokenStream::new()
        };
        let int_fn = if int_args {
            quote! {
                fn int_arg(&mut self) -> Result<i64, String> {
                    self.expect("(")?;
                    let value = match self.next() {
                        Some(Token::Int(i)) => i,
                        other => return Err(format!("expected a number, found {other:?}")),
                    };
                    self.expect(")")?;
                    Ok(value)
                }
            }
        } else {
            TokenStream::new()
        };
        let top = self.top_level();
        let args_fn = if calls {
            quote! {
                fn args<const N: usize>(&mut self) -> Result<[#ast; N], String> {
                    self.expect("(")?;
                    let mut args = Vec::new();
                    for i in 0..N {
                        if i > 0 {
                            self.expect(",")?;
                        }
                        args.push(self.#top()?);
                    }
                    self.expect(")")?;
                    args.try_into()
                        .map_err(|_| "an argument list of the wrong length".to_string())
                }
            }
        } else {
            TokenStream::new()
        };
        let levels = self.level_fns();
        let primary = self.primary_fn();
        // With a binary `-`, a `-` before a digit is the sign of a literal only where no operand
        // ends: `a - 1` and `a -1` subtract, `-1` and `(x - -1)` hold a negative literal (D-6543).
        let minus = self.ops.iter().any(|o| {
            matches!(&o.form, Form::Binary { symbol: true, token, .. } if token.starts_with('-'))
        });
        let (sign_test, ends_operand) = if minus {
            let words: Vec<Literal> = self
                .ops
                .iter()
                .filter_map(|o| match &o.form {
                    Form::Binary {
                        symbol: false,
                        token,
                        ..
                    }
                    | Form::Prefix { token, .. } => Some(lit_str(token)),
                    _ => None,
                })
                .collect();
            let n = Literal::usize_unsuffixed(words.len());
            (
                quote! { && !ends_operand(tokens.last()) },
                quote! {
                    /// Whether a token ends an operand, so that a `-` after it is the binary operator rather than the sign of a literal.
                    fn ends_operand(token: Option<&Token>) -> bool {
                        const WORD_OPERATORS: [&str; #n] = [#(#words),*];
                        match token {
                            Some(Token::Int(_) | Token::Str(_)) => true,
                            Some(Token::Sym(s)) => *s == ")" || *s == "]",
                            Some(Token::Ident(w)) => !WORD_OPERATORS.contains(&w.as_str()),
                            None => false,
                        }
                    }
                },
            )
        } else {
            (TokenStream::new(), TokenStream::new())
        };
        let symbols: Vec<Literal> = symbols.iter().map(|s| lit_str(s)).collect();
        quote! {
            #[derive(Clone, Debug, PartialEq)]
            enum Token {
                Ident(String),
                Int(i64),
                Str(String),
                Sym(&'static str),
            }

            #ends_operand

            fn tokenize(source: &str) -> Result<Vec<Token>, String> {
                const SYMBOLS: [&str; #count] = [#(#symbols),*];
                let chars: Vec<char> = source.chars().collect();
                let mut tokens = Vec::new();
                let mut i = 0;
                while i < chars.len() {
                    let c = chars[i];
                    if c.is_whitespace() {
                        i += 1;
                    } else if c.is_ascii_alphabetic() || c == '_' || c == '$' {
                        let start = i;
                        while i < chars.len()
                            && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '$')
                        {
                            i += 1;
                        }
                        tokens.push(Token::Ident(chars[start..i].iter().collect()));
                    } else if c.is_ascii_digit()
                        || (c == '-' && chars.get(i + 1).is_some_and(char::is_ascii_digit) #sign_test)
                    {
                        let start = i;
                        i += 1;
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                        let text: String = chars[start..i].iter().collect();
                        tokens.push(Token::Int(
                            text.parse().map_err(|_| format!("number out of range: {text}"))?,
                        ));
                    } else if c == '\'' {
                        let mut text = String::new();
                        i += 1;
                        loop {
                            match chars.get(i) {
                                None => return Err("unterminated string".into()),
                                Some('\'') => break,
                                Some('\\') => {
                                    i += 1;
                                    match chars.get(i) {
                                        Some('n') => text.push('\n'),
                                        Some(c @ ('\'' | '\\')) => text.push(*c),
                                        other => return Err(format!("bad escape: {other:?}")),
                                    }
                                }
                                Some(c) => text.push(*c),
                            }
                            i += 1;
                        }
                        i += 1;
                        tokens.push(Token::Str(text));
                    } else {
                        let rest: String = chars[i..chars.len().min(i + #longest)].iter().collect();
                        let symbol = SYMBOLS
                            .iter()
                            .find(|s| rest.starts_with(**s))
                            .ok_or_else(|| format!("unexpected character {c:?}"))?;
                        tokens.push(Token::Sym(symbol));
                        i += symbol.chars().count();
                    }
                }
                Ok(tokens)
            }

            struct Parser {
                tokens: Vec<Token>,
                at: usize,
            }

            impl Parser {
                fn peek(&self) -> Option<&Token> {
                    self.tokens.get(self.at)
                }

                fn next(&mut self) -> Option<Token> {
                    let token = self.tokens.get(self.at).cloned();
                    self.at += 1;
                    token
                }

                #word_fn

                fn sym(&mut self, symbol: &str) -> bool {
                    if matches!(self.peek(), Some(Token::Sym(s)) if *s == symbol) {
                        self.at += 1;
                        true
                    } else {
                        false
                    }
                }

                fn expect(&mut self, symbol: &str) -> Result<(), String> {
                    if self.sym(symbol) {
                        Ok(())
                    } else {
                        Err(format!("expected {symbol:?}, found {:?}", self.peek()))
                    }
                }

                #ident_fn
                #int_fn
                #args_fn
                #levels
                #primary
            }

            /// Parses an expression from its authoring syntax.
            pub fn parse(source: &str) -> Result<#ast, String> {
                let mut parser = Parser {
                    tokens: tokenize(source)?,
                    at: 0,
                };
                let expr = parser.#top().map_err(|e| format!("{e} in {source:?}"))?;
                match parser.peek() {
                    None => Ok(expr),
                    Some(extra) => Err(format!("unexpected {extra:?} after the expression in {source:?}")),
                }
            }
        }
    }

    /// The precedence levels, lowest first, with their operators.
    fn levels(&self) -> Vec<(u64, Vec<&Op<'a>>)> {
        let mut levels: BTreeMap<u64, Vec<&Op>> = BTreeMap::new();
        for o in &self.ops {
            match &o.form {
                Form::Binary { precedence, .. } | Form::Prefix { precedence, .. } => {
                    levels.entry(*precedence).or_default().push(o)
                }
                _ => {}
            }
        }
        levels.into_iter().collect()
    }

    fn top_level(&self) -> Ident {
        if self.levels().is_empty() {
            format_ident!("primary")
        } else {
            format_ident!("level_0")
        }
    }

    fn level_fns(&self) -> TokenStream {
        let ast = self.ast_ty();
        let levels = self.levels();
        let mut out = Vec::new();
        for (k, (precedence, ops)) in levels.iter().enumerate() {
            let this = format_ident!("level_{}", k);
            let next = if k + 1 < levels.len() {
                format_ident!("level_{}", k + 1)
            } else {
                format_ident!("primary")
            };
            let doc = format!(" Precedence {precedence}.");
            let matcher = |symbol: bool, token: &str| {
                let token = lit_str(token);
                if symbol {
                    quote! { self.sym(#token) }
                } else {
                    quote! { self.word(#token) }
                }
            };
            let prefixes: Vec<TokenStream> = ops
                .iter()
                .filter_map(|o| match &o.form {
                    Form::Prefix { token, .. } => {
                        let m = matcher(false, token);
                        let built = self.construct_op(o, vec![quote! { self.#this()? }]);
                        Some(quote! {
                            if #m {
                                return Ok(#built);
                            }
                        })
                    }
                    _ => None,
                })
                .collect();
            let binaries: Vec<&&Op> = ops
                .iter()
                .filter(|o| matches!(o.form, Form::Binary { .. }))
                .collect();
            let assoc = binaries.first().and_then(|o| match o.form {
                Form::Binary { assoc, .. } => Some(assoc),
                _ => None,
            });
            let body = match assoc {
                None => quote! { self.#next() },
                Some(Assoc::NAry) => {
                    let o = binaries[0];
                    let Form::Binary { symbol, token, .. } = &o.form else {
                        unreachable!()
                    };
                    let m = matcher(*symbol, token);
                    let built = self.construct_op(o, vec![quote! { items }]);
                    quote! {
                        let mut items = vec![self.#next()?];
                        while #m {
                            items.push(self.#next()?);
                        }
                        Ok(if items.len() == 1 { items.remove(0) } else { #built })
                    }
                }
                Some(Assoc::None) => {
                    let arms: Vec<TokenStream> = binaries
                        .iter()
                        .map(|o| {
                            let Form::Binary { symbol, token, .. } = &o.form else {
                                unreachable!()
                            };
                            let m = matcher(*symbol, token);
                            let built = self
                                .construct_op(o, vec![quote! { left }, quote! { self.#next()? }]);
                            quote! {
                                if #m {
                                    return Ok(#built);
                                }
                            }
                        })
                        .collect();
                    quote! {
                        let left = self.#next()?;
                        #(#arms)*
                        Ok(left)
                    }
                }
                Some(Assoc::Left) => {
                    let arms: Vec<TokenStream> = binaries
                        .iter()
                        .map(|o| {
                            let Form::Binary { symbol, token, .. } = &o.form else {
                                unreachable!()
                            };
                            let m = matcher(*symbol, token);
                            let built = self
                                .construct_op(o, vec![quote! { left }, quote! { self.#next()? }]);
                            quote! {
                                if #m {
                                    left = #built;
                                    continue;
                                }
                            }
                        })
                        .collect();
                    quote! {
                        let mut left = self.#next()?;
                        loop {
                            #(#arms)*
                            return Ok(left);
                        }
                    }
                }
                Some(Assoc::Right) => {
                    let arms: Vec<TokenStream> = binaries
                        .iter()
                        .map(|o| {
                            let Form::Binary { symbol, token, .. } = &o.form else {
                                unreachable!()
                            };
                            let m = matcher(*symbol, token);
                            let built = self
                                .construct_op(o, vec![quote! { left }, quote! { self.#this()? }]);
                            quote! {
                                if #m {
                                    return Ok(#built);
                                }
                            }
                        })
                        .collect();
                    quote! {
                        let left = self.#next()?;
                        #(#arms)*
                        Ok(left)
                    }
                }
            };
            out.push(quote! {
                #[doc = #doc]
                fn #this(&mut self) -> Result<#ast, String> {
                    #(#prefixes)*
                    #body
                }
            });
        }
        quote! { #(#out)* }
    }

    fn primary_fn(&self) -> TokenStream {
        let ast = self.ast_ty();
        let top = self.top_level();
        let mut token_arms = Vec::new();
        if self.has_form("int") {
            let lit = self.literal({
                let int = self.value("int");
                quote! { #int(i) }
            });
            token_arms.push(quote! { Some(Token::Int(i)) => Ok(#lit), });
        }
        if self.has_form("string") {
            let lit = self.literal({
                let s = self.value("string");
                quote! { #s(s) }
            });
            token_arms.push(quote! { Some(Token::Str(s)) => Ok(#lit), });
        }
        token_arms.push(quote! {
            Some(Token::Sym("(")) => {
                let inner = self.#top()?;
                self.expect(")")?;
                Ok(inner)
            }
        });
        if self.has_form("list") {
            let list = self.value("list");
            let lit = self.literal(quote! { #list(items) });
            let pattern = self
                .literal_variant()
                .map(|variant| self.pattern(variant, &[0]))
                .unwrap_or_default();
            token_arms.push(quote! {
                Some(Token::Sym("[")) => {
                    let mut items = Vec::new();
                    while !self.sym("]") {
                        if !items.is_empty() {
                            self.expect(",")?;
                        }
                        match self.primary()? {
                            #pattern => items.push(v0),
                            other => return Err(format!("a list holds literals only, found {other}")),
                        }
                    }
                    Ok(#lit)
                }
            });
        }
        // Words.
        let mut word_arms = Vec::new();
        if self.has_form("bool") {
            let b = self.value("bool");
            let t = self.literal(quote! { #b(true) });
            let f = self.literal(quote! { #b(false) });
            word_arms.push(quote! { "true" => Ok(#t), });
            word_arms.push(quote! { "false" => Ok(#f), });
        }
        if self.has_form("null") {
            let n = self.value("null");
            let lit = self.literal(quote! { #n });
            word_arms.push(quote! { "null" => Ok(#lit), });
        }
        for o in &self.ops {
            let Form::Scope { token } = &o.form else {
                continue;
            };
            match (o.info.prim, token) {
                (Prim::Now | Prim::Principal, Some(t)) => {
                    let t = lit_str(t);
                    let built = self.construct_op(o, vec![]);
                    word_arms.push(quote! { #t => Ok(#built), });
                }
                (Prim::Param, Some(t)) => {
                    let t = lit_str(t);
                    if self.text_decl(o.variant, 0).is_some() {
                        let name = format_ident!("name");
                        let convert = self.text_conversion(o.variant, 0, &name);
                        let built = self.construct(o.variant, vec![quote! { name }]);
                        word_arms.push(quote! {
                            #t => {
                                self.expect(".")?;
                                let name = self.ident()?;
                                #convert
                                Ok(#built)
                            }
                        });
                    } else {
                        let built = self.construct(o.variant, vec![quote! { self.ident()? }]);
                        word_arms.push(quote! {
                            #t => {
                                self.expect(".")?;
                                Ok(#built)
                            }
                        });
                    }
                }
                (Prim::HasRole, Some(t)) => {
                    let t = lit_str(t);
                    let name = format_ident!("name");
                    // A text id is checked before the closing parenthesis, as the kernel's
                    // parser checks a role id.
                    let convert = self.text_conversion(o.variant, 0, &name);
                    let built = self.construct(o.variant, vec![quote! { name }]);
                    word_arms.push(quote! {
                        #t => {
                            self.expect("(")?;
                            let name = self.ident()?;
                            #convert
                            self.expect(")")?;
                            Ok(#built)
                        }
                    });
                }
                _ => {}
            }
        }
        let var = self.ops.iter().find(|o| o.info.prim == Prim::Var);
        let through = self.ops.iter().find(|o| o.info.prim == Prim::Through);
        if var.is_some() || through.is_some() {
            let names: Vec<Literal> = self.record_scopes.iter().map(|s| lit_str(s.name)).collect();
            let scope_arms: Vec<TokenStream> = self
                .record_scopes
                .iter()
                .map(|s| {
                    let n = lit_str(s.name);
                    let value = self.scope_value(s.value);
                    quote! { #n => #value, }
                })
                .collect();
            let through_built = through.map(|o| {
                self.construct(
                    o.variant,
                    vec![
                        quote! { which },
                        quote! { attribute },
                        quote! { self.ident()? },
                    ],
                )
            });
            let var_built = var
                .map(|o| self.construct(o.variant, vec![quote! { which }, quote! { attribute }]));
            let rest = match (var_built, through_built) {
                (Some(var), Some(through)) => quote! {
                    if self.sym(".") {
                        Ok(#through)
                    } else {
                        Ok(#var)
                    }
                },
                (Some(var), None) => quote! { Ok(#var) },
                (None, Some(through)) => quote! {
                    self.expect(".")?;
                    Ok(#through)
                },
                (None, None) => quote! { unreachable!() },
            };
            word_arms.push(quote! {
                #(#names)|* => {
                    self.expect(".")?;
                    let which = match name.as_str() {
                        #(#scope_arms)*
                        _ => unreachable!("the arm matched a record scope"),
                    };
                    let attribute = self.ident()?;
                    #rest
                }
            });
        }
        if self.has_form("time") {
            let t = self.value("time");
            let value = match self.payload("time") {
                Some(Builtin::U64) => quote! {
                    u64::try_from(self.int_arg()?).map_err(|_| "a time is not negative")?
                },
                _ => quote! { self.int_arg()? },
            };
            let value = self.wrap_payload("time", value);
            let lit = self.literal(quote! { #t(#value) });
            word_arms.push(quote! { "time" => Ok(#lit), });
        }
        if self.has_form("ref") {
            let r = self.value("ref");
            let value = match self.payload("ref") {
                Some(Builtin::I64) => quote! { self.int_arg()? },
                _ => quote! {
                    u64::try_from(self.int_arg()?).map_err(|_| "a record id is not negative")?
                },
            };
            let value = self.wrap_payload("ref", value);
            let lit = self.literal(quote! { #r(#value) });
            word_arms.push(quote! { "ref" => Ok(#lit), });
        }
        for o in &self.ops {
            let Form::Call { token } = &o.form else {
                continue;
            };
            let t = lit_str(token);
            let n = o.exprs.len();
            let names: Vec<Ident> = (0..n).map(|i| format_ident!("a{}", i)).collect();
            let count = Literal::usize_unsuffixed(n);
            let built = self.construct_op(o, names.iter().map(|a| quote! { #a }).collect());
            word_arms.push(quote! {
                #t => {
                    let [#(#names),*] = self.args::<#count>()?;
                    Ok(#built)
                }
            });
        }
        quote! {
            fn primary(&mut self) -> Result<#ast, String> {
                match self.next() {
                    #(#token_arms)*
                    Some(Token::Ident(name)) => match name.as_str() {
                        #(#word_arms)*
                        other => Err(format!("unknown name {other:?}")),
                    },
                    other => Err(format!("unexpected {other:?}")),
                }
            }
        }
    }

    // --- printer -----------------------------------------------------------------------------

    fn printer(&self) -> TokenStream {
        let ast = self.ast_ty();
        let value = self.value_ty();
        let mut arms = Vec::new();
        for (variant, ops) in self.by_variant() {
            let first = ops[0];
            if matches!(first.form, Form::Literal { .. }) {
                let pattern = self.pattern(variant, &[0]);
                arms.push(quote! { #pattern => write_value(v0, f), });
                continue;
            }
            if first.case.is_some() {
                let ci = first.case.as_ref().map(|c| c.0).unwrap_or_default();
                let mut used = first.exprs.clone();
                used.push(ci);
                let pattern = self.pattern(variant, &used);
                let case_ident = v(ci);
                let inner: Vec<TokenStream> = ops
                    .iter()
                    .map(|o| {
                        let case = self.case_value(o);
                        let print = self.print_op(o);
                        quote! { #case => #print, }
                    })
                    .collect();
                arms.push(quote! { #pattern => match #case_ident { #(#inner)* }, });
                continue;
            }
            let used: Vec<usize> = (0..variant.fields.len()).collect();
            let pattern = self.pattern(variant, &used);
            let print = self.print_op(first);
            arms.push(quote! { #pattern => #print, });
        }
        let value_arms: Vec<TokenStream> = self
            .value_forms
            .iter()
            .map(|(name, form)| {
                let ident = naming::ident(&naming::pascal(name).unwrap_or_default());
                match form.as_str() {
                    "null" => quote! { #value::#ident => f.write_str("null"), },
                    "bool" | "int" => quote! { #value::#ident(x) => write!(f, "{x}"), },
                    "time" => quote! { #value::#ident(x) => write!(f, "time({x})"), },
                    "ref" => quote! { #value::#ident(x) => write!(f, "ref({x})"), },
                    "string" => quote! {
                        #value::#ident(s) => {
                            f.write_str("'")?;
                            for c in s.chars() {
                                match c {
                                    '\'' => f.write_str("\\'")?,
                                    '\\' => f.write_str("\\\\")?,
                                    '\n' => f.write_str("\\n")?,
                                    c => write!(f, "{c}")?,
                                }
                            }
                            f.write_str("'")
                        }
                    },
                    _ => quote! {
                        #value::#ident(items) => {
                            f.write_str("[")?;
                            for (i, item) in items.iter().enumerate() {
                                if i > 0 {
                                    f.write_str(", ")?;
                                }
                                write_value(item, f)?;
                            }
                            f.write_str("]")
                        }
                    },
                }
            })
            .collect();
        let write_value = if self.literal_variant().is_some() {
            quote! {
                fn write_value(value: &#value, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    match value {
                        #(#value_arms)*
                    }
                }
            }
        } else {
            TokenStream::new()
        };
        let joined = if self.ops.iter().any(|o| {
            matches!(
                o.form,
                Form::Binary {
                    assoc: Assoc::NAry,
                    ..
                }
            )
        }) {
            quote! {
                fn write_joined(items: &[#ast], word: &str, identity: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    if items.is_empty() {
                        return f.write_str(identity);
                    }
                    f.write_str("(")?;
                    for (i, item) in items.iter().enumerate() {
                        if i > 0 {
                            write!(f, " {word} ")?;
                        }
                        write!(f, "{item}")?;
                    }
                    f.write_str(")")
                }
            }
        } else {
            TokenStream::new()
        };
        let scope_word = if self.uses(Prim::Var) || self.uses(Prim::Through) {
            let scope = self.scope_enum_path();
            let arms: Vec<TokenStream> = self
                .record_scopes
                .iter()
                .map(|s| {
                    let value = self.scope_value(s.value);
                    let n = lit_str(s.name);
                    quote! { #value => #n, }
                })
                .collect();
            quote! {
                fn scope_word(scope: &#scope) -> &'static str {
                    match scope {
                        #(#arms)*
                    }
                }
            }
        } else {
            TokenStream::new()
        };
        quote! {
            #write_value
            #joined
            #scope_word

            /// The authoring syntax: what `parse` reads back.
            impl fmt::Display for #ast {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    match self {
                        #(#arms)*
                    }
                }
            }
        }
    }

    /// The printing of one operator, its expression operands bound to `v<i>`.
    fn print_op(&self, o: &Op) -> TokenStream {
        let ex: Vec<String> = o.exprs.iter().map(|i| format!("{{v{i}}}")).collect();
        let others: Vec<usize> = (0..o.variant.fields.len())
            .filter(|i| !o.exprs.contains(i) && o.case.as_ref().is_none_or(|c| c.0 != *i))
            .collect();
        match &o.form {
            Form::Binary {
                assoc: Assoc::NAry,
                token,
                ..
            } => {
                let items = v(o.exprs[0]);
                let token = lit_str(token);
                let identity = lit_str(if o.info.prim == Prim::And3 {
                    "true"
                } else {
                    "false"
                });
                quote! { write_joined(#items, #token, #identity, f) }
            }
            Form::Binary { token, .. } => {
                let fmt = lit_str(&format!("({} {token} {})", ex[0], ex[1]));
                quote! { write!(f, #fmt) }
            }
            Form::Prefix { token, .. } => {
                let fmt = lit_str(&format!("{token}({})", ex[0]));
                quote! { write!(f, #fmt) }
            }
            Form::Call { token } => {
                let fmt = lit_str(&format!("{token}({})", ex.join(", ")));
                quote! { write!(f, #fmt) }
            }
            Form::Literal { .. } => quote! { write_value(v0, f) },
            Form::Scope { token } => match o.info.prim {
                Prim::Var => {
                    let scope = v(others[0]);
                    let fmt = lit_str(&format!("{{}}.{{v{}}}", others[1]));
                    quote! { write!(f, #fmt, scope_word(#scope)) }
                }
                Prim::Through => {
                    let scope = v(others[0]);
                    let fmt = lit_str(&format!("{{}}.{{v{}}}.{{v{}}}", others[1], others[2]));
                    quote! { write!(f, #fmt, scope_word(#scope)) }
                }
                Prim::Param => {
                    let fmt = lit_str(&format!(
                        "{}.{{v{}}}",
                        token.as_deref().unwrap_or_default(),
                        others[0]
                    ));
                    quote! { write!(f, #fmt) }
                }
                Prim::HasRole => {
                    let fmt = lit_str(&format!(
                        "{}({{v{}}})",
                        token.as_deref().unwrap_or_default(),
                        others[0]
                    ));
                    quote! { write!(f, #fmt) }
                }
                _ => {
                    let word = lit_str(token.as_deref().unwrap_or_default());
                    quote! { f.write_str(#word) }
                }
            },
        }
    }

    // --- evaluator ---------------------------------------------------------------------------

    fn evaluator(&self) -> TokenStream {
        let ast = self.ast_ty();
        let value = self.value_ty();
        let null = self.value("null");
        let mut arms = Vec::new();
        for (variant, ops) in self.by_variant() {
            let first = ops[0];
            if matches!(first.form, Form::Literal { .. }) {
                let pattern = self.pattern(variant, &[0]);
                arms.push(quote! { #pattern => v0.clone(), });
                continue;
            }
            let used = self.used_fields(first);
            let pattern = self.pattern(variant, &used);
            if let Some((ci, _)) = &first.case {
                let case_ident = v(*ci);
                let inner: Vec<TokenStream> = ops
                    .iter()
                    .map(|o| {
                        let case = self.case_value(o);
                        let body = self.eval_op(o);
                        quote! { #case => #body, }
                    })
                    .collect();
                arms.push(quote! { #pattern => match #case_ident { #(#inner)* }, });
            } else {
                let body = self.eval_op(first);
                arms.push(quote! { #pattern => #body, });
            }
        }
        // Helpers, each only when an operator needs it.
        let mut helpers = Vec::new();
        let kinds: Vec<TokenStream> = self
            .value_forms
            .iter()
            .map(|(name, form)| {
                let ident = naming::ident(&naming::pascal(name).unwrap_or_default());
                let kind = LITERAL_FORMS
                    .iter()
                    .find(|(f, _, _)| f == form)
                    .map(|(_, k, _)| *k)
                    .unwrap_or_default();
                if form == "null" {
                    quote! { #value::#ident => #kind, }
                } else {
                    quote! { #value::#ident(_) => #kind, }
                }
            })
            .collect();
        helpers.push(quote! {
            /// The kind of a value, as evaluation errors name it.
            fn kind(value: &#value) -> &'static str {
                match value {
                    #(#kinds)*
                }
            }

            fn type_error(op: &str, value: &#value) -> EvalError {
                EvalError(format!("{op} does not apply to a {}", kind(value)))
            }
        });
        if self.uses(Prim::Var) || self.uses(Prim::Through) {
            let scope = self.scope_enum_path();
            helpers.push(quote! {
                /// A field or system attribute of a record; null when there is no record.
                fn attribute(record: Option<&dyn Record>, scope: &#scope, name: &str) -> Result<#value, EvalError> {
                    let Some(record) = record else {
                        return Ok(#null);
                    };
                    if system_attribute_shape(scope, name).is_some() {
                        return Ok(record.system_attribute(name));
                    }
                    record
                        .field(name)
                        .map_err(|_| EvalError(format!("unknown attribute {name}")))
                }
            });
        }
        if self.uses(Prim::And3) || self.uses(Prim::Or3) {
            let b = self.value("bool");
            helpers.push(quote! {
                /// Kleene logic over operands evaluated left to right: `decisive` (false for and, true for or) ends the run.
                fn kleene(
                    values: impl Iterator<Item = Result<#value, EvalError>>,
                    decisive: bool,
                    op: &str,
                ) -> Result<#value, EvalError> {
                    let mut saw_null = false;
                    for value in values {
                        match value? {
                            #b(b) if b == decisive => return Ok(#b(decisive)),
                            #b(_) => {}
                            #null => saw_null = true,
                            other => return Err(type_error(op, &other)),
                        }
                    }
                    Ok(if saw_null { #null } else { #b(!decisive) })
                }
            });
        }
        let compares = [Prim::Eq, Prim::Ne, Prim::Lt, Prim::Le, Prim::Gt, Prim::Ge];
        if compares.iter().any(|p| self.uses(*p)) {
            let b = self.value("bool");
            helpers.push(quote! {
                /// Null when either side is null; an error when the two are of different kinds.
                fn comparable(left: &#value, right: &#value) -> Result<bool, EvalError> {
                    if matches!(left, #null) || matches!(right, #null) {
                        return Ok(false);
                    }
                    if kind(left) != kind(right) {
                        return Err(EvalError(format!(
                            "cannot compare a {} with a {}",
                            kind(left),
                            kind(right)
                        )));
                    }
                    Ok(true)
                }

                fn equality(left: #value, right: #value, equal: bool) -> Result<#value, EvalError> {
                    if !comparable(&left, &right)? {
                        return Ok(#null);
                    }
                    Ok(#b((left == right) == equal))
                }
            });
            if [Prim::Lt, Prim::Le, Prim::Gt, Prim::Ge]
                .iter()
                .any(|p| self.uses(*p))
            {
                let ordered: Vec<TokenStream> = ["int", "time", "string"]
                    .iter()
                    .filter(|f| self.has_form(f))
                    .map(|f| {
                        let path = self.value(f);
                        quote! { (#path(a), #path(b)) => a.cmp(b), }
                    })
                    .collect();
                helpers.push(quote! {
                    fn order(
                        left: #value,
                        right: #value,
                        test: fn(std::cmp::Ordering) -> bool,
                    ) -> Result<#value, EvalError> {
                        if !comparable(&left, &right)? {
                            return Ok(#null);
                        }
                        let ordering = match (&left, &right) {
                            #(#ordered)*
                            _ => return Err(type_error("ordering", &left)),
                        };
                        Ok(#b(test(ordering)))
                    }
                });
            }
        }
        if self.uses(Prim::IsSlug) {
            helpers.push(quote! {
                /// Whether a string is a slug: `[a-z0-9]+(-[a-z0-9]+)*`.
                pub fn is_slug(s: &str) -> bool {
                    !s.is_empty()
                        && s.split('-').all(|part| {
                            !part.is_empty()
                                && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                        })
                }
            });
        }
        if self.ops.iter().any(|o| o.info.prim.checked_int()) {
            let safe_int = Literal::i64_unsuffixed(SAFE_INT);
            let overflow = lit_str(INTEGER_OVERFLOW);
            helpers.push(quote! {
                /// The largest magnitude of an integer of checked arithmetic, 2^53-1: what Rust and JavaScript both hold exactly.
                const SAFE_INT: i64 = #safe_int;

                /// An operand of checked arithmetic: within ±(2^53-1), or the error `integer-overflow`.
                fn safe(value: i64) -> Result<i64, EvalError> {
                    if (-SAFE_INT..=SAFE_INT).contains(&value) {
                        Ok(value)
                    } else {
                        Err(EvalError(#overflow.to_string()))
                    }
                }

                /// The result of checked arithmetic: an `i64` operation that did not overflow and lies within ±(2^53-1), or the error `integer-overflow`. Never wrapped or saturated.
                fn checked(value: Option<i64>) -> Result<i64, EvalError> {
                    match value {
                        Some(value) => safe(value),
                        None => Err(EvalError(#overflow.to_string())),
                    }
                }
            });
        }
        if self.uses(Prim::ConcatChecked) {
            let max = Literal::usize_unsuffixed(MAX_TEXT_BYTES);
            helpers.push(quote! {
                /// The longest string `concat` builds, in bytes of UTF-8.
                const MAX_TEXT_BYTES: usize = #max;
            });
        }
        let b = self.value("bool");
        quote! {
            #(#helpers)*

            /// Evaluates an expression. Total and deterministic; a type error is an error, never a silent `false`.
            pub fn eval(expr: &#ast, env: &dyn Env) -> Result<#value, EvalError> {
                Ok(match expr {
                    #(#arms)*
                })
            }

            /// Evaluates to a definite truth value: only `true` passes.
            pub fn holds(expr: &#ast, env: &dyn Env) -> Result<bool, EvalError> {
                Ok(eval(expr, env)? == #b(true))
            }
        }
    }

    /// The fields an operator's evaluation and check read.
    fn used_fields(&self, o: &Op) -> Vec<usize> {
        (0..o.variant.fields.len()).collect()
    }

    fn eval_op(&self, o: &Op) -> TokenStream {
        let null = self.value("null");
        let e = |k: usize| v(o.exprs[k]);
        let others: Vec<Ident> = (0..o.variant.fields.len())
            .filter(|i| !o.exprs.contains(i) && o.case.as_ref().is_none_or(|c| c.0 != *i))
            .map(v)
            .collect();
        let label = lit_str(o.token());
        let b = || self.value("bool");
        match o.info.prim {
            Prim::Literal | Prim::ListLiteral => quote! { v0.clone() },
            Prim::Var => {
                let (scope, name) = (&others[0], &others[1]);
                quote! { attribute(env.record(#scope), #scope, #name)? }
            }
            Prim::Through => {
                let (scope, field, target) = (&others[0], &others[1], &others[2]);
                let r = self.value("ref");
                quote! {
                    match env.record(#scope) {
                        None => #null,
                        Some(record) => match record.field(#field).map_err(EvalError)? {
                            #null => #null,
                            #r(id) => match env.follow(#scope, #field, &id)? {
                                Some(found) => attribute(Some(&*found), #scope, #target)?,
                                None => #null,
                            },
                            other => return Err(type_error("following a reference", &other)),
                        },
                    }
                }
            }
            Prim::Param => {
                let name = self.text_ref(o.variant, other_index(o, 0));
                quote! { env.param(#name).unwrap_or(#null) }
            }
            Prim::Now => {
                let t = self.value("time");
                quote! { #t(env.now()) }
            }
            Prim::Principal => {
                let s = self.value("string");
                quote! {
                    match env.principal() {
                        Some(id) => #s(id),
                        None => #null,
                    }
                }
            }
            Prim::HasRole => {
                let role = self.text_ref(o.variant, other_index(o, 0));
                let b = b();
                quote! { #b(env.has_role(#role)) }
            }
            Prim::Not3 => {
                let x = e(0);
                let b = b();
                quote! {
                    match eval(#x, env)? {
                        #b(x) => #b(!x),
                        #null => #null,
                        other => return Err(type_error(#label, &other)),
                    }
                }
            }
            Prim::And3 | Prim::Or3 => {
                let items = e(0);
                let decisive = o.info.prim == Prim::Or3;
                quote! { kleene(#items.iter().map(|item| eval(item, env)), #decisive, #label)? }
            }
            Prim::Eq | Prim::Ne => {
                let (l, r) = (e(0), e(1));
                let equal = o.info.prim == Prim::Eq;
                quote! { equality(eval(#l, env)?, eval(#r, env)?, #equal)? }
            }
            Prim::Lt | Prim::Le | Prim::Gt | Prim::Ge => {
                let (l, r) = (e(0), e(1));
                let test = match o.info.prim {
                    Prim::Lt => quote! { std::cmp::Ordering::is_lt },
                    Prim::Le => quote! { std::cmp::Ordering::is_le },
                    Prim::Gt => quote! { std::cmp::Ordering::is_gt },
                    _ => quote! { std::cmp::Ordering::is_ge },
                };
                quote! { order(eval(#l, env)?, eval(#r, env)?, #test)? }
            }
            Prim::InList => {
                let (item, list) = (e(0), e(1));
                let l = self.value("list");
                let b = b();
                quote! {
                    match (eval(#item, env)?, eval(#list, env)?) {
                        (#null, _) | (_, #null) => #null,
                        (item, #l(items)) => #b(items.contains(&item)),
                        (_, other) => return Err(type_error(#label, &other)),
                    }
                }
            }
            Prim::IsNull => {
                let x = e(0);
                let b = b();
                quote! { #b(matches!(eval(#x, env)?, #null)) }
            }
            Prim::Len => {
                let x = e(0);
                let (s, l, i) = (self.value("string"), self.value("list"), self.value("int"));
                quote! {
                    match eval(#x, env)? {
                        #null => #null,
                        #s(s) => #i(s.chars().count() as i64),
                        #l(items) => #i(items.len() as i64),
                        other => return Err(type_error(#label, &other)),
                    }
                }
            }
            Prim::Lower => {
                let x = e(0);
                let s = self.value("string");
                quote! {
                    match eval(#x, env)? {
                        #null => #null,
                        #s(s) => #s(s.to_lowercase()),
                        other => return Err(type_error(#label, &other)),
                    }
                }
            }
            Prim::Contains => {
                let (h, n) = (e(0), e(1));
                let (s, l) = (self.value("string"), self.value("list"));
                let b = b();
                quote! {
                    match (eval(#h, env)?, eval(#n, env)?) {
                        (#null, _) | (_, #null) => #null,
                        (#s(h), #s(n)) => #b(h.contains(n.as_str())),
                        (#l(items), needle) => #b(items.contains(&needle)),
                        (other, _) => return Err(type_error(#label, &other)),
                    }
                }
            }
            Prim::StartsWith => {
                let (t, p) = (e(0), e(1));
                let s = self.value("string");
                let b = b();
                quote! {
                    match (eval(#t, env)?, eval(#p, env)?) {
                        (#null, _) | (_, #null) => #null,
                        (#s(t), #s(p)) => #b(t.starts_with(p.as_str())),
                        (other, _) => return Err(type_error(#label, &other)),
                    }
                }
            }
            Prim::IsSlug => {
                let x = e(0);
                let s = self.value("string");
                let b = b();
                quote! {
                    match eval(#x, env)? {
                        #null => #null,
                        #s(s) => #b(is_slug(&s)),
                        other => return Err(type_error(#label, &other)),
                    }
                }
            }
            Prim::If => {
                let (c, t, otherwise) = (e(0), e(1), e(2));
                let b = b();
                quote! {
                    match eval(#c, env)? {
                        #b(true) => eval(#t, env)?,
                        #b(false) | #null => eval(#otherwise, env)?,
                        other => return Err(type_error(#label, &other)),
                    }
                }
            }
            // Both operands first, left to right; then nulls; then the left's kind, then the
            // right's; then the range of each operand and of the result.
            Prim::AddChecked | Prim::SubChecked | Prim::MulChecked => {
                let (l, r) = (e(0), e(1));
                let i = self.value("int");
                let op = match o.info.prim {
                    Prim::AddChecked => quote! { checked_add },
                    Prim::SubChecked => quote! { checked_sub },
                    _ => quote! { checked_mul },
                };
                // A timestamp moves by milliseconds under + and -; * takes ints only.
                let (time_arm, left_ok) =
                    if o.info.prim != Prim::MulChecked && self.has_form("time") {
                        let t = self.value("time");
                        (
                            quote! { (#t(a), #i(b)) => #t(checked(safe(a)?.#op(safe(b)?))?), },
                            quote! { #i(_) | #t(_) },
                        )
                    } else {
                        (TokenStream::new(), quote! { #i(_) })
                    };
                quote! {
                    match (eval(#l, env)?, eval(#r, env)?) {
                        (#null, _) | (_, #null) => #null,
                        (#i(a), #i(b)) => #i(checked(safe(a)?.#op(safe(b)?))?),
                        #time_arm
                        (#left_ok, other) => return Err(type_error(#label, &other)),
                        (other, _) => return Err(type_error(#label, &other)),
                    }
                }
            }
            Prim::TimeDifference => {
                let (from, to) = (e(0), e(1));
                let (i, t) = (self.value("int"), self.value("time"));
                quote! {
                    match (eval(#from, env)?, eval(#to, env)?) {
                        (#null, _) | (_, #null) => #null,
                        (#t(a), #t(b)) => #i(checked(safe(b)?.checked_sub(safe(a)?))?),
                        (#t(_), other) => return Err(type_error(#label, &other)),
                        (other, _) => return Err(type_error(#label, &other)),
                    }
                }
            }
            Prim::DaysMs | Prim::HoursMs | Prim::MinutesMs => {
                let x = e(0);
                let i = self.value("int");
                let unit = Literal::i64_unsuffixed(match o.info.prim {
                    Prim::DaysMs => 86_400_000,
                    Prim::HoursMs => 3_600_000,
                    _ => 60_000,
                });
                quote! {
                    match eval(#x, env)? {
                        #null => #null,
                        #i(n) => #i(checked(safe(n)?.checked_mul(#unit))?),
                        other => return Err(type_error(#label, &other)),
                    }
                }
            }
            Prim::ConcatChecked => {
                let (l, r) = (e(0), e(1));
                let s = self.value("string");
                let too_long = lit_str(STRING_TOO_LONG);
                quote! {
                    match (eval(#l, env)?, eval(#r, env)?) {
                        (#null, _) | (_, #null) => #null,
                        (#s(a), #s(b)) => {
                            let joined = a + b.as_str();
                            if joined.len() > MAX_TEXT_BYTES {
                                return Err(EvalError(#too_long.to_string()));
                            }
                            #s(joined)
                        }
                        (#s(_), other) => return Err(type_error(#label, &other)),
                        (other, _) => return Err(type_error(#label, &other)),
                    }
                }
            }
            Prim::ToText => {
                let x = e(0);
                let s = self.value("string");
                let mut arms = vec![quote! { #null => #null, }, quote! { #s(s) => #s(s), }];
                for form in ["bool", "int", "time", "ref"] {
                    if self.has_form(form) {
                        let path = self.value(form);
                        arms.push(quote! { #path(x) => #s(x.to_string()), });
                    }
                }
                // Every other value (a list) is an error; the arm is left out when no value is left.
                let covered = ["null", "string", "bool", "int", "time", "ref"];
                if self
                    .value_forms
                    .iter()
                    .any(|(_, f)| !covered.contains(&f.as_str()))
                {
                    arms.push(quote! { other => return Err(type_error(#label, &other)), });
                }
                quote! {
                    match eval(#x, env)? {
                        #(#arms)*
                    }
                }
            }
        }
    }

    // --- checker -----------------------------------------------------------------------------

    fn checker(&self) -> TokenStream {
        let ast = self.ast_ty();
        let value = self.value_ty();
        let any = self.any();
        let mut arms = Vec::new();
        for (variant, ops) in self.by_variant() {
            let first = ops[0];
            if matches!(first.form, Form::Literal { .. }) {
                let pattern = self.pattern(variant, &[0]);
                arms.push(quote! { #pattern => shape_of_value(v0), });
                continue;
            }
            let used = self.used_fields(first);
            let pattern = self.pattern(variant, &used);
            if let Some((ci, _)) = &first.case {
                let case_ident = v(*ci);
                let inner: Vec<TokenStream> = ops
                    .iter()
                    .map(|o| {
                        let case = self.case_value(o);
                        let body = self.check_op(o);
                        quote! { #case => { #body } }
                    })
                    .collect();
                arms.push(quote! { #pattern => match #case_ident { #(#inner)* }, });
            } else {
                let body = self.check_op(first);
                arms.push(quote! { #pattern => { #body } });
            }
        }
        let mut helpers = Vec::new();
        helpers.push(quote! {
            fn expect(shape: Shape, allowed: &[Shape], what: &str) -> Result<(), String> {
                if shape == #any || allowed.contains(&shape) {
                    Ok(())
                } else {
                    Err(format!("{what} does not accept a {shape:?} operand"))
                }
            }
        });
        if self.literal_variant().is_some() {
            let arms: Vec<TokenStream> = self
                .value_shapes
                .iter()
                .map(|(name, shape)| {
                    let ident = naming::ident(&naming::pascal(name).unwrap_or_default());
                    let form = self
                        .value_forms
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, f)| f.as_str())
                        .unwrap_or_default();
                    let shape = self.shape(shape);
                    if form == "null" {
                        quote! { #value::#ident => #shape, }
                    } else {
                        quote! { #value::#ident(_) => #shape, }
                    }
                })
                .collect();
            helpers.push(quote! {
                fn shape_of_value(value: &#value) -> Shape {
                    match value {
                        #(#arms)*
                    }
                }
            });
        }
        if self.uses(Prim::Var) || self.uses(Prim::Through) {
            let scope = self.scope_enum_path();
            // Scopes with the same attributes share an arm.
            let mut groups: Vec<(&[(String, String)], Vec<&str>)> = Vec::new();
            for s in &self.record_scopes {
                match groups.iter_mut().find(|(a, _)| *a == s.attributes) {
                    Some((_, values)) => values.push(s.value),
                    None => groups.push((s.attributes, vec![s.value])),
                }
            }
            let arms: Vec<TokenStream> = groups
                .iter()
                .map(|(attributes, values)| {
                    let values: Vec<TokenStream> =
                        values.iter().map(|v| self.scope_value(v)).collect();
                    let names: Vec<Literal> = attributes.iter().map(|(n, _)| lit_str(n)).collect();
                    let shapes: Vec<TokenStream> =
                        attributes.iter().map(|(_, s)| self.shape(s)).collect();
                    quote! {
                        #(#values)|* => match name {
                            #( #names => Some(#shapes), )*
                            _ => None,
                        },
                    }
                })
                .collect();
            helpers.push(quote! {
                /// The shape of a system attribute the scope declares, `None` for any other name.
                fn system_attribute_shape(scope: &#scope, name: &str) -> Option<Shape> {
                    match scope {
                        #(#arms)*
                    }
                }

                fn attribute_shape(env: &dyn TypeEnv, through: Option<&str>, scope: &#scope, name: &str) -> Result<Shape, String> {
                    match system_attribute_shape(scope, name) {
                        Some(shape) => Ok(shape),
                        None => env.field_shape(through, name),
                    }
                }
            });
        }
        quote! {
            #(#helpers)*

            /// Checks an expression against its definition site and returns its shape.
            pub fn check(expr: &#ast, env: &dyn TypeEnv) -> Result<Shape, String> {
                Ok(match expr {
                    #(#arms)*
                })
            }
        }
    }

    /// `expect(<shape>, &[allowed], what)?;` for a declared operand, or nothing when it accepts the any shape.
    fn expect(&self, shape: TokenStream, allowed: &[String], what: &str) -> TokenStream {
        let any = self.lang.any_shape.as_deref().unwrap_or_default();
        if allowed.iter().any(|a| a == any) {
            return quote! { let _ = #shape; };
        }
        let allowed: Vec<TokenStream> = allowed.iter().map(|a| self.shape(a)).collect();
        let what = lit_str(what);
        quote! { expect(#shape, &[#(#allowed),*], #what)?; }
    }

    fn result_shape(&self, o: &Op) -> TokenStream {
        self.shape(o.op.result.as_deref().unwrap_or_default())
    }

    fn check_op(&self, o: &Op) -> TokenStream {
        let e = |k: usize| v(o.exprs[k]);
        let others: Vec<Ident> = (0..o.variant.fields.len())
            .filter(|i| !o.exprs.contains(i) && o.case.as_ref().is_none_or(|c| c.0 != *i))
            .map(v)
            .collect();
        let any = self.any();
        let token = o.token().to_string();
        let operands = &o.op.operands;
        let result = || self.result_shape(o);
        match o.info.prim {
            Prim::Literal | Prim::ListLiteral => quote! { shape_of_value(v0) },
            Prim::Var => {
                let (scope, name) = (&others[0], &others[1]);
                quote! { attribute_shape(env, None, #scope, #name)? }
            }
            Prim::Through => {
                let (scope, field, target) = (&others[0], &others[1], &others[2]);
                quote! {
                    env.follow(#field)?;
                    attribute_shape(env, Some(#field.as_str()), #scope, #target)?
                }
            }
            Prim::Param => {
                let name = &others[0];
                let text = self.text_ref(o.variant, other_index(o, 0));
                let r = result();
                let message = lit_str(&format!("undeclared parameter {{{name}}}"));
                quote! {
                    if !env.has_param(#text) {
                        return Err(format!(#message));
                    }
                    #r
                }
            }
            Prim::HasRole => {
                let role = &others[0];
                let text = self.text_ref(o.variant, other_index(o, 0));
                let r = result();
                let message = lit_str(&format!("unknown role {{{role}}}"));
                quote! {
                    if !env.has_role(#text) {
                        return Err(format!(#message));
                    }
                    #r
                }
            }
            Prim::Now | Prim::Principal => result(),
            Prim::And3 | Prim::Or3 => {
                let items = e(0);
                let ex = self.expect(quote! { check(item, env)? }, &operands[0], "and/or");
                let r = result();
                quote! {
                    for item in #items {
                        #ex
                    }
                    #r
                }
            }
            Prim::Eq | Prim::Ne | Prim::Lt | Prim::Le | Prim::Gt | Prim::Ge => {
                let (l, r) = (e(0), e(1));
                let el = self.expect(quote! { left }, &operands[0], "ordering");
                let er = self.expect(quote! { right }, &operands[1], "ordering");
                let res = result();
                quote! {
                    let (left, right) = (check(#l, env)?, check(#r, env)?);
                    if left != #any && right != #any && left != right {
                        return Err(format!("cannot compare {left:?} with {right:?}"));
                    }
                    #el
                    #er
                    #res
                }
            }
            Prim::InList | Prim::Contains => {
                let (a, b) = (e(0), e(1));
                let ea = self.expect(quote! { first }, &operands[0], &token);
                let eb = self.expect(quote! { second }, &operands[1], &token);
                let extra = if o.info.prim == Prim::Contains {
                    let text = self
                        .value_shapes
                        .iter()
                        .find(|(n, _)| {
                            self.value_forms
                                .iter()
                                .any(|(w, f)| w == n && f == "string")
                        })
                        .map(|(_, s)| s.clone())
                        .unwrap_or_default();
                    let ex = self.expect(quote! { second }, &[text.clone()], &token);
                    let text = self.shape(&text);
                    quote! {
                        if first == #text {
                            #ex
                        }
                    }
                } else {
                    TokenStream::new()
                };
                let r = result();
                quote! {
                    let (first, second) = (check(#a, env)?, check(#b, env)?);
                    #ea
                    #eb
                    #extra
                    #r
                }
            }
            Prim::If => {
                let (c, t, otherwise) = (e(0), e(1), e(2));
                let ec = self.expect(quote! { check(#c, env)? }, &operands[0], &token);
                let message = lit_str(&format!(
                    "{token} branches differ: {{then:?}} and {{otherwise:?}}"
                ));
                quote! {
                    #ec
                    let (then, otherwise) = (check(#t, env)?, check(#otherwise, env)?);
                    if then != #any && otherwise != #any && then != otherwise {
                        return Err(format!(#message));
                    }
                    if then == #any { otherwise } else { then }
                }
            }
            // One operand after another, giving the shape of an operand (+ and - give the
            // shape of the left: int, time, or the any shape).
            _ if o.op.result.as_deref().and_then(operand_ref).is_some() => {
                let n = o.op.result.as_deref().and_then(operand_ref).unwrap_or(1);
                let shapes: Vec<Ident> = (0..o.exprs.len())
                    .map(|k| format_ident!("s{}", k))
                    .collect();
                let checks: Vec<TokenStream> = (0..o.exprs.len())
                    .map(|k| {
                        let x = e(k);
                        let s = &shapes[k];
                        let ex = self.expect(quote! { #s }, &operands[k], &token);
                        quote! {
                            let #s = check(#x, env)?;
                            #ex
                        }
                    })
                    .collect();
                let result = &shapes[n - 1];
                quote! {
                    #(#checks)*
                    #result
                }
            }
            // One operand after another: not, is_null, len, lower, is_slug, starts_with, the
            // checked arithmetic, ms_between, the durations, concat and text.
            _ => {
                let checks: Vec<TokenStream> = (0..o.exprs.len())
                    .map(|k| {
                        let x = e(k);
                        self.expect(quote! { check(#x, env)? }, &operands[k], &token)
                    })
                    .collect();
                let r = result();
                quote! {
                    #(#checks)*
                    #r
                }
            }
        }
    }
}
