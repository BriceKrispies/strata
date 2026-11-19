//! The expression language used by constraints, policy conditions, workflow guards, defaults
//! and query filters, with its static check and its evaluation semantics.
//!
//! Evaluation is total and deterministic. Logic is three-valued in the SQL manner: a comparison
//! with `null` yields `null`, `and`/`or` follow Kleene logic. Each use site decides what `null`
//! means: a constraint is violated only by `false`; a policy, guard or filter passes only on
//! `true`. A type error is an evaluation error, never a silent `false`.

use crate::ids::{FieldId, ObjectTypeId, RoleId, Timestamp};
use crate::rules::Lookup;
use crate::schema::{FieldDef, ObjectTypeDef};
use crate::state::{Principal, Record};
use crate::value::{FieldType, Value};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Which record a variable reads from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Scope {
    /// The record being accessed: the stored record, or the candidate when creating.
    Record,
    /// The stored record before an update. All fields are `null` on create.
    Old,
    /// The candidate record after a create or update.
    New,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Expr {
    Lit(Value),
    /// A field, or a system attribute: `$id`, `$version`, `$created_by`, `$created_at`,
    /// `$updated_at`.
    Var(Scope, String),
    /// A field, or system attribute, of the record that a reference field points at:
    /// `new.author.principal` is the `principal` of the author record `new.author` refers to.
    /// One hop only. `null` when the reference is empty or the record is gone.
    Through(Scope, String, String),
    /// A named query parameter.
    Param(String),
    /// The context timestamp.
    Now,
    /// The acting principal's id as a string, `null` for anonymous and system.
    Principal,
    HasRole(RoleId),
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Cmp(CmpOp, Box<Expr>, Box<Expr>),
    /// Membership of the left value in the right list.
    In(Box<Expr>, Box<Expr>),
    IsNull(Box<Expr>),
    /// Character count of a string, element count of a list.
    Len(Box<Expr>),
    Lower(Box<Expr>),
    /// Substring test for strings, element test for lists.
    Contains(Box<Expr>, Box<Expr>),
    StartsWith(Box<Expr>, Box<Expr>),
    /// Lower-case letters and digits separated by single hyphens.
    IsSlug(Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
}

/// Everything an expression may observe.
pub struct Env<'a> {
    pub record: Option<&'a Record>,
    pub old: Option<&'a Record>,
    pub new: Option<&'a Record>,
    pub params: &'a BTreeMap<String, Value>,
    pub now: Timestamp,
    pub principal: &'a Principal,
    pub roles: &'a BTreeSet<RoleId>,
    /// The records a reference may be followed into (`Expr::Through`). Following a reference
    /// is an evaluation error where there is none, or where the snapshot does not cover the
    /// target, so a decision never rests on a record that was merely not loaded.
    pub lookup: Option<Lookup<'a>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvalError(pub String);

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn type_error(op: &str, value: &Value) -> EvalError {
    EvalError(format!("{op} does not apply to a {}", value.kind()))
}

/// A field or system attribute of a record. `null` when there is no record.
pub fn attribute(record: Option<&Record>, name: &str) -> Result<Value, EvalError> {
    let Some(record) = record else {
        return Ok(Value::Null);
    };
    Ok(match name {
        "$id" => Value::Ref(record.id),
        "$version" => Value::Int(record.version as i64),
        "$created_at" => Value::Time(record.created_at.0),
        "$updated_at" => Value::Time(record.updated_at.0),
        "$created_by" => match &record.created_by {
            Some(principal) => Value::Str(principal.as_str().to_string()),
            None => Value::Null,
        },
        field => match FieldId::new(field) {
            Ok(id) => record.get(&id).clone(),
            Err(_) => return Err(EvalError(format!("unknown attribute {field}"))),
        },
    })
}

fn tri_and(values: impl Iterator<Item = Result<Value, EvalError>>) -> Result<Value, EvalError> {
    let mut saw_null = false;
    for value in values {
        match value? {
            Value::Bool(false) => return Ok(Value::Bool(false)),
            Value::Bool(true) => {}
            Value::Null => saw_null = true,
            other => return Err(type_error("and", &other)),
        }
    }
    Ok(if saw_null {
        Value::Null
    } else {
        Value::Bool(true)
    })
}

fn tri_or(values: impl Iterator<Item = Result<Value, EvalError>>) -> Result<Value, EvalError> {
    let mut saw_null = false;
    for value in values {
        match value? {
            Value::Bool(true) => return Ok(Value::Bool(true)),
            Value::Bool(false) => {}
            Value::Null => saw_null = true,
            other => return Err(type_error("or", &other)),
        }
    }
    Ok(if saw_null {
        Value::Null
    } else {
        Value::Bool(false)
    })
}

fn compare(op: CmpOp, left: Value, right: Value) -> Result<Value, EvalError> {
    use std::cmp::Ordering;
    if left.is_null() || right.is_null() {
        return Ok(Value::Null);
    }
    if left.kind() != right.kind() {
        return Err(EvalError(format!(
            "cannot compare a {} with a {}",
            left.kind(),
            right.kind()
        )));
    }
    let result = match op {
        CmpOp::Eq => left == right,
        CmpOp::Ne => left != right,
        ordered => {
            let ordering: Ordering = match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => a.cmp(b),
                (Value::Time(a), Value::Time(b)) => a.cmp(b),
                (Value::Str(a), Value::Str(b)) => a.cmp(b),
                _ => return Err(type_error("ordering", &left)),
            };
            match ordered {
                CmpOp::Lt => ordering == Ordering::Less,
                CmpOp::Le => ordering != Ordering::Greater,
                CmpOp::Gt => ordering == Ordering::Greater,
                _ => ordering != Ordering::Less,
            }
        }
    };
    Ok(Value::Bool(result))
}

/// Whether a string is a slug: `[a-z0-9]+(-[a-z0-9]+)*`.
pub fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Evaluate an expression.
pub fn eval(expr: &Expr, env: &Env<'_>) -> Result<Value, EvalError> {
    Ok(match expr {
        Expr::Lit(value) => value.clone(),
        Expr::Var(scope, name) => {
            let record = match scope {
                Scope::Record => env.record,
                Scope::Old => env.old,
                Scope::New => env.new,
            };
            attribute(record, name)?
        }
        Expr::Through(scope, field, target) => {
            let record = match scope {
                Scope::Record => env.record,
                Scope::Old => env.old,
                Scope::New => env.new,
            };
            let Some(record) = record else {
                return Ok(Value::Null);
            };
            let id = FieldId::new(field).map_err(|e| EvalError(e.to_string()))?;
            match record.get(&id) {
                Value::Null => Value::Null,
                Value::Ref(target_id) => {
                    let lookup = env
                        .lookup
                        .ok_or_else(|| EvalError(format!("{field} cannot be followed here")))?;
                    let reference = lookup
                        .tenant
                        .schema
                        .objects
                        .get(&record.object)
                        .and_then(|object| object.fields.get(&id))
                        .and_then(|def| def.ty.ref_target());
                    let Some((object, _, false)) = reference else {
                        return Err(EvalError(format!("{field} is not a reference")));
                    };
                    match lookup
                        .record(object, *target_id)
                        .map_err(|e| EvalError(e.message))?
                    {
                        Some(found) => attribute(Some(found.as_ref()), target)?,
                        None => Value::Null,
                    }
                }
                other => return Err(type_error("following a reference", other)),
            }
        }
        Expr::Param(name) => env.params.get(name).cloned().unwrap_or(Value::Null),
        Expr::Now => Value::Time(env.now.0),
        Expr::Principal => match env.principal {
            Principal::User(id) => Value::Str(id.as_str().to_string()),
            _ => Value::Null,
        },
        Expr::HasRole(role) => Value::Bool(env.roles.contains(role)),
        Expr::Not(inner) => match eval(inner, env)? {
            Value::Bool(b) => Value::Bool(!b),
            Value::Null => Value::Null,
            other => return Err(type_error("not", &other)),
        },
        Expr::And(items) => tri_and(items.iter().map(|item| eval(item, env)))?,
        Expr::Or(items) => tri_or(items.iter().map(|item| eval(item, env)))?,
        Expr::Cmp(op, left, right) => compare(*op, eval(left, env)?, eval(right, env)?)?,
        Expr::In(item, list) => match (eval(item, env)?, eval(list, env)?) {
            (Value::Null, _) | (_, Value::Null) => Value::Null,
            (item, Value::List(items)) => Value::Bool(items.contains(&item)),
            (_, other) => return Err(type_error("in", &other)),
        },
        Expr::IsNull(inner) => Value::Bool(eval(inner, env)?.is_null()),
        Expr::Len(inner) => match eval(inner, env)? {
            Value::Null => Value::Null,
            Value::Str(s) => Value::Int(s.chars().count() as i64),
            Value::List(items) => Value::Int(items.len() as i64),
            other => return Err(type_error("len", &other)),
        },
        Expr::Lower(inner) => match eval(inner, env)? {
            Value::Null => Value::Null,
            Value::Str(s) => Value::Str(s.to_lowercase()),
            other => return Err(type_error("lower", &other)),
        },
        Expr::Contains(haystack, needle) => match (eval(haystack, env)?, eval(needle, env)?) {
            (Value::Null, _) | (_, Value::Null) => Value::Null,
            (Value::Str(h), Value::Str(n)) => Value::Bool(h.contains(n.as_str())),
            (Value::List(items), needle) => Value::Bool(items.contains(&needle)),
            (other, _) => return Err(type_error("contains", &other)),
        },
        Expr::StartsWith(text, prefix) => match (eval(text, env)?, eval(prefix, env)?) {
            (Value::Null, _) | (_, Value::Null) => Value::Null,
            (Value::Str(t), Value::Str(p)) => Value::Bool(t.starts_with(p.as_str())),
            (other, _) => return Err(type_error("starts_with", &other)),
        },
        Expr::IsSlug(inner) => match eval(inner, env)? {
            Value::Null => Value::Null,
            Value::Str(s) => Value::Bool(is_slug(&s)),
            other => return Err(type_error("is_slug", &other)),
        },
        Expr::If(cond, then, otherwise) => match eval(cond, env)? {
            Value::Bool(true) => eval(then, env)?,
            Value::Bool(false) | Value::Null => eval(otherwise, env)?,
            other => return Err(type_error("if", &other)),
        },
    })
}

/// Evaluate to a definite truth value: only `true` passes.
pub fn holds(expr: &Expr, env: &Env<'_>) -> Result<bool, EvalError> {
    Ok(eval(expr, env)? == Value::Bool(true))
}

// ---------------------------------------------------------------------------------------------
// Static check
// ---------------------------------------------------------------------------------------------

/// The statically known shape of an expression. `Any` covers `null`, parameters and anything
/// the check cannot pin down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Bool,
    Int,
    Str,
    Time,
    Ref,
    List,
    Any,
}

fn shape_of_type(ty: &FieldType) -> Shape {
    match ty {
        FieldType::Str => Shape::Str,
        FieldType::Int => Shape::Int,
        FieldType::Bool => Shape::Bool,
        FieldType::Time => Shape::Time,
        FieldType::Ref { .. } => Shape::Ref,
        FieldType::List(_) => Shape::List,
    }
}

fn shape_of_value(value: &Value) -> Shape {
    match value {
        Value::Null => Shape::Any,
        Value::Bool(_) => Shape::Bool,
        Value::Int(_) => Shape::Int,
        Value::Str(_) => Shape::Str,
        Value::Time(_) => Shape::Time,
        Value::Ref(_) => Shape::Ref,
        Value::List(_) => Shape::List,
    }
}

/// What an expression may refer to at its definition site.
pub struct TypeEnv<'a> {
    pub fields: &'a BTreeMap<FieldId, FieldDef>,
    pub params: &'a BTreeSet<String>,
    pub roles: &'a BTreeSet<RoleId>,
    /// Every object type, for following references.
    pub objects: &'a BTreeMap<ObjectTypeId, ObjectTypeDef>,
}

/// The shape of a field or system attribute of a record with these fields.
fn attribute_shape(fields: &BTreeMap<FieldId, FieldDef>, name: &str) -> Result<Shape, String> {
    Ok(match name {
        "$id" => Shape::Ref,
        "$version" => Shape::Int,
        "$created_at" | "$updated_at" => Shape::Time,
        "$created_by" => Shape::Str,
        field => {
            let id = FieldId::new(field).map_err(|e| e.to_string())?;
            let def = fields
                .get(&id)
                .ok_or_else(|| format!("unknown field {field}"))?;
            shape_of_type(&def.ty)
        }
    })
}

fn expect(shape: Shape, allowed: &[Shape], what: &str) -> Result<(), String> {
    if shape == Shape::Any || allowed.contains(&shape) {
        Ok(())
    } else {
        Err(format!("{what} does not accept a {shape:?} operand"))
    }
}

/// Check an expression against its definition site and return its shape.
pub fn check(expr: &Expr, env: &TypeEnv<'_>) -> Result<Shape, String> {
    Ok(match expr {
        Expr::Lit(value) => shape_of_value(value),
        Expr::Var(_, name) => attribute_shape(env.fields, name)?,
        Expr::Through(_, field, target) => {
            let id = FieldId::new(field).map_err(|e| e.to_string())?;
            let def = env
                .fields
                .get(&id)
                .ok_or_else(|| format!("unknown field {field}"))?;
            let Some((object, _, false)) = def.ty.ref_target() else {
                return Err(format!("{field} is not a single reference"));
            };
            let object = env
                .objects
                .get(object)
                .ok_or_else(|| format!("unknown object type {object}"))?;
            attribute_shape(&object.fields, target)?
        }
        Expr::Param(name) => {
            if !env.params.contains(name) {
                return Err(format!("undeclared parameter {name}"));
            }
            Shape::Any
        }
        Expr::Now => Shape::Time,
        Expr::Principal => Shape::Str,
        Expr::HasRole(role) => {
            if !env.roles.contains(role) {
                return Err(format!("unknown role {role}"));
            }
            Shape::Bool
        }
        Expr::Not(inner) => {
            expect(check(inner, env)?, &[Shape::Bool], "not")?;
            Shape::Bool
        }
        Expr::And(items) | Expr::Or(items) => {
            for item in items {
                expect(check(item, env)?, &[Shape::Bool], "and/or")?;
            }
            Shape::Bool
        }
        Expr::Cmp(op, left, right) => {
            let (l, r) = (check(left, env)?, check(right, env)?);
            if l != Shape::Any && r != Shape::Any && l != r {
                return Err(format!("cannot compare {l:?} with {r:?}"));
            }
            if !matches!(op, CmpOp::Eq | CmpOp::Ne) {
                let ordered = [Shape::Int, Shape::Time, Shape::Str];
                expect(l, &ordered, "ordering")?;
                expect(r, &ordered, "ordering")?;
            }
            Shape::Bool
        }
        Expr::In(item, list) => {
            check(item, env)?;
            expect(check(list, env)?, &[Shape::List], "in")?;
            Shape::Bool
        }
        Expr::IsNull(inner) => {
            check(inner, env)?;
            Shape::Bool
        }
        Expr::Len(inner) => {
            expect(check(inner, env)?, &[Shape::Str, Shape::List], "len")?;
            Shape::Int
        }
        Expr::Lower(inner) => {
            expect(check(inner, env)?, &[Shape::Str], "lower")?;
            Shape::Str
        }
        Expr::Contains(haystack, needle) => {
            let h = check(haystack, env)?;
            let n = check(needle, env)?;
            expect(h, &[Shape::Str, Shape::List], "contains")?;
            if h == Shape::Str {
                expect(n, &[Shape::Str], "contains")?;
            }
            Shape::Bool
        }
        Expr::StartsWith(text, prefix) => {
            expect(check(text, env)?, &[Shape::Str], "starts_with")?;
            expect(check(prefix, env)?, &[Shape::Str], "starts_with")?;
            Shape::Bool
        }
        Expr::IsSlug(inner) => {
            expect(check(inner, env)?, &[Shape::Str], "is_slug")?;
            Shape::Bool
        }
        Expr::If(cond, then, otherwise) => {
            expect(check(cond, env)?, &[Shape::Bool], "if")?;
            let (t, o) = (check(then, env)?, check(otherwise, env)?);
            if t != Shape::Any && o != Shape::Any && t != o {
                return Err(format!("if branches differ: {t:?} and {o:?}"));
            }
            if t == Shape::Any {
                o
            } else {
                t
            }
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Printing. The output is the authoring syntax that `strata_substrate::syntax` parses.
// ---------------------------------------------------------------------------------------------

fn write_value(value: &Value, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match value {
        Value::Null => f.write_str("null"),
        Value::Bool(b) => write!(f, "{b}"),
        Value::Int(i) => write!(f, "{i}"),
        Value::Str(s) => {
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
        Value::Time(t) => write!(f, "time({t})"),
        Value::Ref(id) => write!(f, "ref({id})"),
        Value::List(items) => {
            f.write_str("[")?;
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write_value(item, f)?;
            }
            f.write_str("]")
        }
    }
}

fn write_joined(items: &[Expr], word: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if items.is_empty() {
        return f.write_str(if word == "and" { "true" } else { "false" });
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

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Lit(value) => write_value(value, f),
            Expr::Var(scope, name) => {
                let scope = match scope {
                    Scope::Record => "record",
                    Scope::Old => "old",
                    Scope::New => "new",
                };
                write!(f, "{scope}.{name}")
            }
            Expr::Through(scope, field, target) => {
                let scope = match scope {
                    Scope::Record => "record",
                    Scope::Old => "old",
                    Scope::New => "new",
                };
                write!(f, "{scope}.{field}.{target}")
            }
            Expr::Param(name) => write!(f, "param.{name}"),
            Expr::Now => f.write_str("now"),
            Expr::Principal => f.write_str("principal"),
            Expr::HasRole(role) => write!(f, "has_role({role})"),
            Expr::Not(inner) => write!(f, "not({inner})"),
            Expr::And(items) => write_joined(items, "and", f),
            Expr::Or(items) => write_joined(items, "or", f),
            Expr::Cmp(op, left, right) => {
                let op = match op {
                    CmpOp::Eq => "==",
                    CmpOp::Ne => "!=",
                    CmpOp::Lt => "<",
                    CmpOp::Le => "<=",
                    CmpOp::Gt => ">",
                    CmpOp::Ge => ">=",
                };
                write!(f, "({left} {op} {right})")
            }
            Expr::In(item, list) => write!(f, "({item} in {list})"),
            Expr::IsNull(inner) => write!(f, "is_null({inner})"),
            Expr::Len(inner) => write!(f, "len({inner})"),
            Expr::Lower(inner) => write!(f, "lower({inner})"),
            Expr::Contains(a, b) => write!(f, "contains({a}, {b})"),
            Expr::StartsWith(a, b) => write!(f, "starts_with({a}, {b})"),
            Expr::IsSlug(inner) => write!(f, "is_slug({inner})"),
            Expr::If(c, t, o) => write!(f, "if({c}, {t}, {o})"),
        }
    }
}
