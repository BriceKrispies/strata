//! The expression language used by constraints, policy conditions, workflow guards, defaults
//! and query filters.
//!
//! The language is the specification's `tenant-rule-language`. specc generates its parser
//! ([`parse`]), its printer (`Display` of [`Expr`], the authoring syntax evidence strings
//! embed), its static check ([`check`]) and its evaluator into
//! `strata_model_core::tenant_rule_language`. This module supplies what those read from the
//! kernel's data: a record's fields and system attributes, following a reference through a
//! [`Lookup`], parameters, the clock, the principal, the roles, and the field definitions of a
//! definition site.
//!
//! Evaluation is total and deterministic. Logic is three-valued in the SQL manner: a comparison
//! with `null` yields `null`, `and`/`or` follow Kleene logic. Each use site decides what `null`
//! means: a constraint is violated only by `false`; a policy, guard or filter passes only on
//! `true`. A type error is an evaluation error, never a silent `false`.
//!
//! Arithmetic (`+`, `-`, `*`, `ms_between`, `days`, `hours`, `minutes`) is checked: an operand or
//! result outside ±(2^53-1), the range Rust and JavaScript both hold exactly, is the evaluation
//! error `integer-overflow`, never a wrapped value (D-6540, D-6541); `concat` past 4,096 bytes is
//! `string-too-long` (D-6542). Like any evaluation error, either fails the decision it belongs to.

use crate::rules::Lookup;
use crate::schema::{FieldDef, ObjectTypeDef};
use crate::state::{Need, Principal, Record, RecordExt};
pub use crate::types::{ArithOp, CmpOp, DurationUnit, Expr, Scope};
use crate::types::{
    FieldId, FieldType, FieldTypeExt, ObjectTypeId, RecordId, RoleId, Timestamp, Value,
};
use std::collections::{BTreeMap, BTreeSet};
use strata_model_core::tenant_rule_language as language;

pub use language::{check, parse, EvalError, Shape};

/// The first record a reference in `expr` points at that the snapshot does not cover, in the
/// order the expression is written, or `None` when every reference it can follow is covered.
///
/// The generated evaluator reports a failed lookup as a bare message, so a caller whose
/// evaluation failed asks this, a pure function of the expression and its environment, whether
/// the failure was a record that was merely not loaded: it then answers `InsufficientState`
/// naming the record, and the shell loads it and asks again. A reference is followed exactly as
/// [`eval`] follows it: from a single reference field of the scope's record, through the
/// environment's [`Lookup`]. Where [`eval`] could not follow it (no lookup, no record in the
/// scope, a null or non-reference value, a field that is not a single reference) there is no
/// need to name.
pub fn missing(expr: &Expr, env: &Env<'_>) -> Option<Need> {
    let lookup = env.lookup?;
    first_missing(expr, env, lookup)
}

fn first_missing(expr: &Expr, env: &Env<'_>, lookup: Lookup<'_>) -> Option<Need> {
    match expr {
        Expr::Through(scope, field, _) => {
            let record = match scope {
                Scope::Record => env.record,
                Scope::Old => env.old,
                Scope::New => env.new,
            }?;
            let id = FieldId::new(field).ok()?;
            let Value::Ref(target) = record.get(&id) else {
                return None;
            };
            let reference = lookup
                .tenant
                .schema
                .objects
                .get(&record.object)
                .and_then(|object| object.fields.get(&id))
                .and_then(|def| def.ty.ref_target());
            let Some((object, _, false)) = reference else {
                return None;
            };
            lookup.record(object, *target).err().and_then(|e| e.missing)
        }
        Expr::Var(..)
        | Expr::Lit(_)
        | Expr::Param(_)
        | Expr::Now
        | Expr::Principal
        | Expr::HasRole(_) => None,
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Graphemes(a)
        | Expr::IsEmoji(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => first_missing(a, env, lookup),
        Expr::And(items) | Expr::Or(items) => items
            .iter()
            .find_map(|item| first_missing(item, env, lookup)),
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => {
            first_missing(a, env, lookup).or_else(|| first_missing(b, env, lookup))
        }
        Expr::If(a, b, c) => first_missing(a, env, lookup)
            .or_else(|| first_missing(b, env, lookup))
            .or_else(|| first_missing(c, env, lookup)),
    }
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

/// The system attribute of a record, `None` for any other name.
fn system_attribute(record: &Record, name: &str) -> Option<Value> {
    Some(match name {
        "$id" => Value::Ref(record.id),
        "$version" => Value::Int(record.version as i64),
        "$created_at" => Value::Time(record.created_at.0),
        "$updated_at" => Value::Time(record.updated_at.0),
        "$created_by" => match &record.created_by {
            Some(principal) => Value::Str(principal.as_str().to_string()),
            None => Value::Null,
        },
        _ => return None,
    })
}

/// A field or system attribute of a record. `null` when there is no record.
pub fn attribute(record: Option<&Record>, name: &str) -> Result<Value, EvalError> {
    let Some(record) = record else {
        return Ok(Value::Null);
    };
    if let Some(value) = system_attribute(record, name) {
        return Ok(value);
    }
    match FieldId::new(name) {
        Ok(id) => Ok(record.get(&id).clone()),
        Err(_) => Err(EvalError(format!("unknown attribute {name}"))),
    }
}

/// A record as the generated evaluator reads it.
struct Row<'r>(&'r Record);

impl language::Record for Row<'_> {
    fn system_attribute(&self, name: &str) -> Value {
        system_attribute(self.0, name).unwrap_or(Value::Null)
    }

    fn field(&self, name: &str) -> Result<Value, String> {
        let id = FieldId::new(name).map_err(|e| e.to_string())?;
        Ok(self.0.get(&id).clone())
    }
}

/// An [`Env`] as the generated evaluator reads it.
struct Scopes<'e, 'a> {
    env: &'e Env<'a>,
    record: Option<Row<'a>>,
    old: Option<Row<'a>>,
    new: Option<Row<'a>>,
}

impl<'e, 'a> Scopes<'e, 'a> {
    fn new(env: &'e Env<'a>) -> Self {
        Scopes {
            env,
            record: env.record.map(Row),
            old: env.old.map(Row),
            new: env.new.map(Row),
        }
    }

    fn row(&self, scope: &Scope) -> Option<&Row<'a>> {
        match scope {
            Scope::Record => self.record.as_ref(),
            Scope::Old => self.old.as_ref(),
            Scope::New => self.new.as_ref(),
        }
    }
}

impl language::Env for Scopes<'_, '_> {
    fn record(&self, scope: &Scope) -> Option<&dyn language::Record> {
        self.row(scope).map(|row| row as &dyn language::Record)
    }

    fn follow(
        &self,
        scope: &Scope,
        field: &str,
        target: &RecordId,
    ) -> Result<Option<Box<dyn language::Record + '_>>, EvalError> {
        let lookup = self
            .env
            .lookup
            .ok_or_else(|| EvalError(format!("{field} cannot be followed here")))?;
        let Some(Row(record)) = self.row(scope) else {
            return Ok(None);
        };
        let id = FieldId::new(field).map_err(|e| EvalError(e.to_string()))?;
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
        // A record the snapshot does not cover is an evaluation error here; the caller learns
        // which record it was from [`missing`].
        let found = lookup
            .record(object, *target)
            .map_err(|e| EvalError(e.message))?;
        Ok(found.map(|found| Box::new(Row(found.as_ref())) as Box<dyn language::Record + '_>))
    }

    fn param(&self, name: &str) -> Option<Value> {
        self.env.params.get(name).cloned()
    }

    fn now(&self) -> i64 {
        self.env.now.0
    }

    fn principal(&self) -> Option<String> {
        match self.env.principal {
            Principal::User(id) => Some(id.as_str().to_string()),
            _ => None,
        }
    }

    fn has_role(&self, role: &str) -> bool {
        self.env.roles.iter().any(|held| held.as_str() == role)
    }
}

/// Evaluate an expression.
pub fn eval(expr: &Expr, env: &Env<'_>) -> Result<Value, EvalError> {
    language::eval(expr, &Scopes::new(env))
}

/// Evaluate to a definite truth value: only `true` passes.
pub fn holds(expr: &Expr, env: &Env<'_>) -> Result<bool, EvalError> {
    language::holds(expr, &Scopes::new(env))
}

// ---------------------------------------------------------------------------------------------
// Static check
// ---------------------------------------------------------------------------------------------

fn shape_of_type(ty: &FieldType) -> Shape {
    match ty {
        FieldType::Str => Shape::Str,
        FieldType::Int => Shape::Int,
        FieldType::Bool => Shape::Bool,
        FieldType::Time => Shape::Time,
        FieldType::Ref { .. } => Shape::Ref,
        FieldType::List(_) => Shape::List,
        // A file is held as its file identifier and a language as its tag, both text.
        FieldType::File { .. } | FieldType::Language => Shape::Str,
    }
}

/// What an expression may refer to at its definition site. [`check`] takes it as the generated
/// language's `TypeEnv`.
pub struct TypeEnv<'a> {
    pub fields: &'a BTreeMap<FieldId, FieldDef>,
    pub params: &'a BTreeSet<String>,
    pub roles: &'a BTreeSet<RoleId>,
    /// Every object type, for following references.
    pub objects: &'a BTreeMap<ObjectTypeId, ObjectTypeDef>,
}

impl TypeEnv<'_> {
    /// The object type a single reference field of the definition site points at.
    fn target(&self, field: &str) -> Result<&ObjectTypeDef, String> {
        let id = FieldId::new(field).map_err(|e| e.to_string())?;
        let def = self
            .fields
            .get(&id)
            .ok_or_else(|| format!("unknown field {field}"))?;
        let Some((object, _, false)) = def.ty.ref_target() else {
            return Err(format!("{field} is not a single reference"));
        };
        self.objects
            .get(object)
            .ok_or_else(|| format!("unknown object type {object}"))
    }
}

impl language::TypeEnv for TypeEnv<'_> {
    fn field_shape(&self, through: Option<&str>, name: &str) -> Result<Shape, String> {
        let fields = match through {
            None => self.fields,
            Some(reference) => &self.target(reference)?.fields,
        };
        let id = FieldId::new(name).map_err(|e| e.to_string())?;
        let def = fields
            .get(&id)
            .ok_or_else(|| format!("unknown field {name}"))?;
        Ok(shape_of_type(&def.ty))
    }

    fn follow(&self, field: &str) -> Result<(), String> {
        self.target(field).map(|_| ())
    }

    fn has_param(&self, name: &str) -> bool {
        self.params.contains(name)
    }

    fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|held| held.as_str() == role)
    }
}
