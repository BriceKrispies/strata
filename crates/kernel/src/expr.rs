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

use crate::rules::Lookup;
use crate::schema::{FieldDef, ObjectTypeDef};
use crate::state::{Need, Principal, Record, RecordExt};
pub use crate::types::{CmpOp, Expr, Scope};
use crate::types::{
    FieldId, FieldType, FieldTypeExt, ObjectTypeId, RecordId, RoleId, Timestamp, Value,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use strata_model_core::tenant_rule_language as language;

pub use language::{check, parse, EvalError, Shape};

thread_local! {
    /// The first state a reference lookup found missing since the last [`clear_missing`]. The
    /// generated evaluator reports a failed lookup as a bare message, so the need is parked here
    /// for the caller to turn into `InsufficientState` and have the shell load it. Per thread,
    /// cleared before each evaluation that cares, and never read by a decision.
    static MISSING: RefCell<Option<Need>> = const { RefCell::new(None) };
}

/// Forget any missing need noted so far on this thread.
pub fn clear_missing() {
    MISSING.with(|slot| *slot.borrow_mut() = None);
}

/// Take the missing need noted by the evaluations since [`clear_missing`], if any.
pub fn take_missing() -> Option<Need> {
    MISSING.with(|slot| slot.borrow_mut().take())
}

fn note_missing(need: Need) {
    MISSING.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(need);
        }
    });
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
        let found = lookup.record(object, *target).map_err(|e| {
            if let Some(need) = e.missing.clone() {
                note_missing(need);
            }
            EvalError(e.message)
        })?;
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
