//! Invoking a tenant operation (D-6546 to D-6549, OPR-1540 to OPR-1554, contract
//! decide-tenant-operation CT-806).
//!
//! A tenant operation is declared data ([`crate::definitions`] checks it when it is defined). The
//! intent `InvokeOperation` names one, with its inputs and the caller's idempotency key, and is
//! decided here, purely:
//!
//! 1. The operation exists, and the caller is a signed-in person its invokers name. That person
//!    holds tenant-operation-invoke, which every signed-in person holds, and needs nothing else
//!    (D-6547, OPR-1546).
//! 2. The inputs are bound: each one declared, each required one present, each of its field type.
//!    They are read as `param.<name>`.
//! 3. The subject, when the operation has one, is loaded; one that does not exist or that the
//!    invoker may not read is refused as a record that does not exist.
//! 4. Every condition is evaluated on the state before any step; false or null refuses the
//!    invocation with the condition's code (OPR-1542, OPR-1543).
//! 5. The steps run in order on a working copy of the state, so a later step sees what an earlier
//!    one did, and reads the record an earlier create step made as `param.<step id>` (D-6548).
//!    Each record change is validated as a direct change is (field types, required fields,
//!    limits, unique and composite unique constraints, references, constraints, workflow states
//!    and transitions), but it is authorised by the operation's own powers, which its definer
//!    had to hold when it was defined (`transition::require_operation_powers`), never by the
//!    invoker's policies: no policy and no workflow guard is asked of the invoker (D-6547).
//!    A record a step creates names the invoker as its creator (OPR-1548).
//! 6. Any refusal refuses the whole invocation, names the step, and leaves no delta (OPR-1542);
//!    an accepted invocation is one delta with every change and the events of every change, in
//!    step order (OPR-1541).
//!
//! What the runtime does around it: the store's idempotency (`strata_storage_api::execute_by`
//! with `Context::idempotency_key`) answers a repeat of the same key by the same principal with
//! the first result, and refuses the key with another intent; a dry run is decided and not
//! committed (OPR-1552).
//!
//! Posting steps call the ledger engine through [`Run::post_to_ledger`], the one place this
//! module touches it: each posting is decided by `crate::ledger::post_after` on the state the
//! steps before it leave.

use crate::definitions::operation_powers;
use crate::expr::{self, Env, Scope};
use crate::intent::{
    Check, CheckExt, Context, DeltaOp, DomainError, DomainErrorExt, DomainEvent, Effect, ErrorKind,
    EventBody, Output, StateDelta, Transition,
};
use crate::limits;
use crate::read::visible_in;
use crate::rules::{missing_error, roles_of, validate_record, Lookup};
use crate::schema::{Action, Resource, TenantSchema, TenantSchemaExt};
use crate::state::{
    Coverage, Need, Principal, Record, RecordExt, State, TenantState, TenantStateExt,
};
use crate::transition::field_needs;
use crate::types::{
    BranchTest, Expr, FieldId, FieldTypeExt, ObjectTypeId, OnDelete, PrincipalId, RecordId, RoleId,
    StateRevision, TenantOperationDef, TenantOperationId, TenantOperationPostingOwner,
    TenantOperationStep, Value, ValueExt,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The ledger and result types an invocation produces, as the specification states them.
pub use strata_model_core::types::{
    LedgerAmount, LedgerEntry, LedgerOwner, LedgerPostingRequest, LedgerScope, LedgerUnit,
    TenantOperationResult,
};

/// The longest idempotency key of an invocation, in bytes (D-6595; `rec-tenant-operation-run`
/// keeps it in at most 128).
pub const MAX_KEY_BYTES: usize = 128;

/// The operation of a schema with this id.
pub fn operation_of<'a>(
    schema: &'a TenantSchema,
    id: &TenantOperationId,
) -> Option<&'a TenantOperationDef> {
    schema.operations.as_ref()?.get(id)
}

/// Whether `key` is acceptable as the idempotency key of an invocation: 1 to [`MAX_KEY_BYTES`]
/// visible ASCII characters.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_KEY_BYTES && key.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// The idempotency key of the posting a step makes: `op:<operation>:<invoker>:<key>:<step>`, so
/// a repeat cannot post twice even after the run record is gone (D-6548, OPR-1545, LDG-006).
pub fn posting_key(
    operation: &TenantOperationId,
    invoker: &PrincipalId,
    key: &str,
    step: &str,
) -> String {
    format!("op:{operation}:{invoker}:{key}:{step}")
}

/// The idempotency key of a posting that names the record it is for (WG-5598): the operation, the
/// reference and the posting step, so that a reversal in a later invocation finds the posting again.
pub fn reference_key(operation: &TenantOperationId, reference: &str, step: &str) -> String {
    format!("ref:{operation}:{reference}:{step}")
}

/// The idempotency key of the reversal of a posting made under a reference (WG-5598). It names the
/// posting step, not the reversal step, so that a second reversal of the posting is found whichever
/// step makes it.
pub fn reversal_key(operation: &TenantOperationId, reference: &str, posting: &str) -> String {
    format!("rev:{operation}:{reference}:{posting}")
}

/// The result of an accepted invocation as contract invoke-tenant-operation answers it, from the
/// transition's output, the revision it was committed at and whether it is a repeat's recorded
/// result. `None` for the output of any other intent.
pub fn result_of(
    output: &Output,
    revision: StateRevision,
    replayed: bool,
) -> Option<TenantOperationResult> {
    match output {
        Output::Operation {
            operation,
            created,
            changed,
            deleted,
            entries,
        } => Some(TenantOperationResult {
            operation: operation.clone(),
            replayed,
            created: created.clone(),
            changed: changed.clone(),
            deleted: deleted.clone(),
            entries: entries.clone(),
            revision,
        }),
        _ => None,
    }
}

/// A refusal of `kind` whose failed check is `rule`, after the checks made so far.
fn refusal(evidence: &[Check], kind: ErrorKind, rule: String, detail: String) -> DomainError {
    let mut checks = evidence.to_vec();
    checks.push(Check::fail(rule, detail.clone()));
    DomainError::new(kind, detail).with(checks)
}

/// The id of a step.
fn step_id(step: &TenantOperationStep) -> &str {
    crate::definitions::step_id(step)
}

/// Decide an invocation of `operation` against a snapshot of `tenant` that covers `coverage`.
/// The tenant and the context have already been matched by `transition`.
///
/// The four stages below are shared with [`preflight`], which runs the same functions on the
/// inputs a row supplies: a stage's answer is the same whichever of the two asks it.
pub fn invoke(
    tenant: &TenantState,
    coverage: &Coverage,
    ctx: &Context,
    operation: &TenantOperationId,
    inputs: &BTreeMap<String, Value>,
    idempotency_key: &str,
) -> Result<Transition, DomainError> {
    let mut evidence: Vec<Check> = Vec::new();
    let Some(op) = operation_of(&tenant.schema, operation) else {
        return Err(refusal(
            &evidence,
            ErrorKind::InvalidIntent,
            "operation:exists".into(),
            format!("no operation {operation}"),
        ));
    };
    let lookup = Lookup { tenant, coverage };
    let invoker = stage_invoker(tenant, ctx, operation, op, idempotency_key, &mut evidence)?;
    let params = stage_inputs(operation, op, inputs, &mut evidence)?;
    let subject = stage_subject(lookup, ctx, operation, op, &params, &mut evidence)?;
    stage_conditions(
        lookup,
        ctx,
        operation,
        op,
        &invoker.roles,
        &params,
        subject.as_deref(),
        &mut evidence,
    )?;

    // The steps, each on the state the steps before it leave (OPR-1543).
    let mut run = Run {
        ctx,
        op,
        operation,
        invoker: invoker.user,
        key: idempotency_key,
        roles: invoker.roles,
        subject,
        params,
        work: TenantState::clone(tenant),
        coverage: coverage.clone(),
        ops: Vec::new(),
        events: Vec::new(),
        evidence,
        created: Vec::new(),
        changed: Vec::new(),
        deleted: Vec::new(),
        entries: Vec::new(),
    };
    for step in &op.steps {
        if let Err(error) = run.step(step) {
            return Err(run.refused(step_id(step), error));
        }
    }
    Ok(run.finish(tenant))
}

/// The answer of [`preflight`] for one invocation a row would make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preflight {
    /// Stages 1 to 4 pass for these inputs, so the invocation is not refused by them. It may
    /// still be refused by a step (a check of a record or the ledger); the invocation decides.
    Allowed,
    /// Stage 1, 2, 3 or 4 refuses: `code` is the condition's code for a condition, else the rule
    /// of the failed check.
    Refused { code: String, detail: String },
    /// The answer depends on something preflight may not or cannot know.
    Unknown(UnknownReason),
}

/// Why [`preflight`] answers `unknown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownReason {
    /// The row does not supply this input, which the operation needs.
    InputNotSupplied(String),
    /// A condition reads a record or a field the viewer may not read.
    WithheldRead,
    /// The snapshot does not hold what a stage reads.
    NotLoaded,
    /// The session's budget of preflight answers is spent.
    Budget,
}

/// The number of preflight answers one session may still give. The session owns it: an answer
/// past the budget is `unknown` with [`UnknownReason::Budget`], never a guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightBudget {
    remaining: u32,
}

impl PreflightBudget {
    pub fn new(remaining: u32) -> Self {
        Self { remaining }
    }

    pub fn remaining(&self) -> u32 {
        self.remaining
    }

    fn take(&mut self) -> bool {
        match self.remaining.checked_sub(1) {
            Some(rest) => {
                self.remaining = rest;
                true
            }
            None => false,
        }
    }
}

/// Whether the operation's conditions read the clock or the principal. A row affordance computed
/// from such a condition is individual to the viewer and is never stored as a shared copy.
pub fn reads_individual(op: &TenantOperationDef) -> bool {
    op.conditions
        .iter()
        .any(|condition| reads_individual_expr(&condition.expr))
}

fn reads_individual_expr(e: &Expr) -> bool {
    matches!(e, Expr::Now | Expr::Principal | Expr::HasRole(_))
        || children(e).into_iter().any(reads_individual_expr)
}

/// Whether an invocation that `row` supplies inputs for would be allowed, by stages 1 to 4 run
/// on the snapshot. Pure, and advisory: the invocation itself decides, and a refusal at the
/// click refreshes the answer. `allowed` is never given from a check the invocation does not run,
/// and any stage that would refuse is `refused`.
///
/// `unknown` when the row does not supply an input the operation needs (its required inputs and
/// its subject input), when a condition reads a record or field the viewer may not read (under
/// the viewer's read rights), when the snapshot does not hold what a stage reads, and past the
/// session's budget.
pub fn preflight(
    tenant: &TenantState,
    coverage: &Coverage,
    ctx: &Context,
    operation: &TenantOperationId,
    row: &BTreeMap<String, Value>,
    idempotency_key: &str,
    budget: &mut PreflightBudget,
) -> Preflight {
    if !budget.take() {
        return Preflight::Unknown(UnknownReason::Budget);
    }
    let Some(op) = operation_of(&tenant.schema, operation) else {
        return Preflight::Refused {
            code: "operation:exists".into(),
            detail: format!("no operation {operation}"),
        };
    };
    let lookup = Lookup { tenant, coverage };
    let mut evidence: Vec<Check> = Vec::new();
    let invoker = match stage_invoker(tenant, ctx, operation, op, idempotency_key, &mut evidence) {
        Ok(invoker) => invoker,
        Err(error) => return answer_refusal(op, &error),
    };

    // An input the operation needs that the row does not supply is not known to be refused.
    let supplied = |name: &str| matches!(row.get(name), Some(value) if !value.is_null());
    for input in &op.inputs {
        if input.required && !supplied(&input.name) {
            return Preflight::Unknown(UnknownReason::InputNotSupplied(input.name.clone()));
        }
    }
    if let Some(def) = &op.subject {
        if !supplied(&def.input) {
            return Preflight::Unknown(UnknownReason::InputNotSupplied(def.input.clone()));
        }
    }

    let params = match stage_inputs(operation, op, row, &mut evidence) {
        Ok(params) => params,
        Err(error) => return answer_refusal(op, &error),
    };
    let subject = match stage_subject(lookup, ctx, operation, op, &params, &mut evidence) {
        Ok(subject) => subject,
        Err(error) => return answer_refusal(op, &error),
    };
    let env = Env {
        record: subject.as_deref(),
        old: subject.as_deref(),
        new: subject.as_deref(),
        params: &params,
        now: ctx.now,
        principal: &ctx.principal,
        roles: &invoker.roles,
        lookup: Some(lookup),
    };
    if op
        .conditions
        .iter()
        .any(|condition| reads_withheld(&condition.expr, &env, lookup, ctx))
    {
        return Preflight::Unknown(UnknownReason::WithheldRead);
    }
    match stage_conditions(
        lookup,
        ctx,
        operation,
        op,
        &invoker.roles,
        &params,
        subject.as_deref(),
        &mut evidence,
    ) {
        Ok(()) => Preflight::Allowed,
        Err(error) => answer_refusal(op, &error),
    }
}

/// The preflight answer of a refusal by one of the stages: a snapshot that falls short is
/// `unknown`, every other refusal is `refused` with its code.
fn answer_refusal(op: &TenantOperationDef, error: &DomainError) -> Preflight {
    if error.kind == ErrorKind::InsufficientState {
        return Preflight::Unknown(UnknownReason::NotLoaded);
    }
    let rule = error
        .evidence
        .last()
        .map(|check| check.rule.clone())
        .unwrap_or_default();
    let code = match rule.rsplit_once(":condition:") {
        Some((_, id)) => op
            .conditions
            .iter()
            .find(|condition| condition.id == id)
            .map(|condition| condition.error.clone())
            .unwrap_or(rule),
        None => rule,
    };
    Preflight::Refused {
        code,
        detail: error.message.clone(),
    }
}

/// The principal who invokes, and the roles it holds, once stage 1 has passed.
struct Invoker {
    user: PrincipalId,
    roles: BTreeSet<RoleId>,
}

/// Stage 1: the caller is a signed-in person whom the invokers of the operation name, and the
/// idempotency key is acceptable (D-6547, OPR-1546, D-6595). The operation's existence is checked
/// by the caller.
fn stage_invoker(
    tenant: &TenantState,
    ctx: &Context,
    operation: &TenantOperationId,
    op: &TenantOperationDef,
    idempotency_key: &str,
    evidence: &mut Vec<Check>,
) -> Result<Invoker, DomainError> {
    let at = format!("operation:{operation}");
    let Principal::User(invoker) = &ctx.principal else {
        return Err(refusal(
            evidence,
            ErrorKind::Unauthorized,
            format!("{at}:invoker"),
            "only a signed-in person invokes a tenant operation".into(),
        ));
    };
    let roles = roles_of(tenant, &ctx.principal);
    if !op.invokers.signed_in && op.invokers.roles.is_disjoint(&roles) {
        return Err(refusal(
            evidence,
            ErrorKind::Unauthorized,
            format!("{at}:invoker"),
            format!("the invokers of operation {operation} do not name {invoker}"),
        ));
    }
    evidence.push(Check::pass(
        format!("{at}:invoker"),
        "a signed-in person the invokers name, holding tenant-operation-invoke as every signed-in person does",
    ));

    // The key the run is known by; the store records the result under the context's key, so the
    // two must be the same key.
    if !valid_key(idempotency_key) {
        return Err(refusal(
            evidence,
            ErrorKind::InvalidIntent,
            format!("{at}:idempotency_key"),
            format!("an idempotency key is 1 to {MAX_KEY_BYTES} visible ASCII characters"),
        ));
    }
    if ctx
        .idempotency_key
        .as_deref()
        .is_some_and(|given| given != idempotency_key)
    {
        return Err(refusal(
            evidence,
            ErrorKind::InvalidIntent,
            format!("{at}:idempotency_key"),
            "the context carries another idempotency key than the invocation".into(),
        ));
    }
    Ok(Invoker {
        user: invoker.clone(),
        roles,
    })
}

/// Stage 2: the inputs are declared, present when required, and of their field type (OPR-1540).
/// They are read as `param.<name>`.
fn stage_inputs(
    operation: &TenantOperationId,
    op: &TenantOperationDef,
    inputs: &BTreeMap<String, Value>,
    evidence: &mut Vec<Check>,
) -> Result<BTreeMap<String, Value>, DomainError> {
    for name in inputs.keys() {
        if !op.inputs.iter().any(|input| &input.name == name) {
            return Err(refusal(
                evidence,
                ErrorKind::UnknownField,
                format!("input:{operation}.{name}"),
                format!("operation {operation} has no input {name}"),
            ));
        }
    }
    let mut params: BTreeMap<String, Value> = BTreeMap::new();
    for input in &op.inputs {
        let rule = format!("input:{operation}.{}", input.name);
        let value = inputs.get(&input.name).cloned().unwrap_or(Value::Null);
        if value.is_null() {
            if input.required {
                return Err(refusal(
                    evidence,
                    ErrorKind::MissingRequired,
                    rule,
                    format!("the input {} is required", input.name),
                ));
            }
            params.insert(input.name.clone(), Value::Null);
            continue;
        }
        if let Some(problem) = limits::value_problem(&value) {
            return Err(refusal(evidence, ErrorKind::LimitExceeded, rule, problem));
        }
        if !input.ty.admits(&value) {
            return Err(refusal(
                evidence,
                ErrorKind::TypeMismatch,
                rule,
                format!(
                    "the input {} expects {}, found {}",
                    input.name,
                    input.ty.name(),
                    value.kind()
                ),
            ));
        }
        evidence.push(Check::pass(rule, input.ty.name()));
        params.insert(input.name.clone(), value);
    }
    Ok(params)
}

/// Stage 3: the subject, when the operation has one, is a record the invoker may read. One that
/// does not exist, or that the invoker may not read, is refused as a record that does not exist,
/// so the invoker cannot tell the two apart.
fn stage_subject(
    lookup: Lookup<'_>,
    ctx: &Context,
    operation: &TenantOperationId,
    op: &TenantOperationDef,
    params: &BTreeMap<String, Value>,
    evidence: &mut Vec<Check>,
) -> Result<Option<Arc<Record>>, DomainError> {
    let at = format!("operation:{operation}");
    let Some(def) = &op.subject else {
        return Ok(None);
    };
    let rule = format!("{at}:subject");
    let Some(Value::Ref(id)) = params.get(&def.input) else {
        return Err(refusal(
            evidence,
            ErrorKind::UnknownRecord,
            rule,
            format!("no {} named by the input {}", def.object, def.input),
        ));
    };
    let id = *id;
    let found = lookup.record(&def.object, id)?;
    let readable = match found {
        Some(record) => visible_in(lookup, &ctx.principal, record, ctx.now)?,
        None => false,
    };
    if !readable {
        return Err(refusal(
            evidence,
            ErrorKind::UnknownRecord,
            rule,
            format!("no {} {id}", def.object),
        ));
    }
    evidence.push(Check::pass(
        rule,
        format!("{} {id} is readable by the invoker", def.object),
    ));
    Ok(found.cloned())
}

/// Stage 4: every condition holds on the state before any step; false or null refuses the
/// invocation with the condition's code (OPR-1542, OPR-1543).
#[allow(clippy::too_many_arguments)]
fn stage_conditions(
    lookup: Lookup<'_>,
    ctx: &Context,
    operation: &TenantOperationId,
    op: &TenantOperationDef,
    roles: &BTreeSet<RoleId>,
    params: &BTreeMap<String, Value>,
    subject: Option<&Record>,
    evidence: &mut Vec<Check>,
) -> Result<(), DomainError> {
    let at = format!("operation:{operation}");
    let env = Env {
        record: subject,
        old: subject,
        new: subject,
        params,
        now: ctx.now,
        principal: &ctx.principal,
        roles,
        lookup: Some(lookup),
    };
    for condition in &op.conditions {
        let rule = format!("{at}:condition:{}", condition.id);
        match expr::holds(&condition.expr, &env) {
            Ok(true) => evidence.push(Check::pass(rule, condition.expr.to_string())),
            Ok(false) => {
                return Err(refusal(
                    evidence,
                    ErrorKind::ConstraintViolated,
                    rule,
                    format!("{}: {}", condition.error, condition.message),
                ))
            }
            Err(e) => {
                if let Some(missing) = missing_error(&condition.expr, &env) {
                    return Err(missing);
                }
                return Err(refusal(
                    evidence,
                    ErrorKind::Evaluation,
                    rule,
                    format!("{}: {e}", condition.expr),
                ));
            }
        }
    }
    Ok(())
}

/// The direct sub-expressions of an expression.
pub(crate) fn children(e: &Expr) -> Vec<&Expr> {
    match e {
        Expr::Lit(_)
        | Expr::Var(..)
        | Expr::Through(..)
        | Expr::Param(_)
        | Expr::Now
        | Expr::Principal
        | Expr::HasRole(_) => Vec::new(),
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Graphemes(a)
        | Expr::IsEmoji(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => vec![a.as_ref()],
        Expr::And(items) | Expr::Or(items) => items.iter().collect(),
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => vec![a.as_ref(), b.as_ref()],
        Expr::If(a, b, c) => vec![a.as_ref(), b.as_ref(), c.as_ref()],
    }
}

/// The record a scope names in an environment.
fn scope_record<'a>(env: &Env<'a>, scope: &Scope) -> Option<&'a Record> {
    match scope {
        Scope::Record => env.record,
        Scope::Old => env.old,
        Scope::New => env.new,
    }
}

/// Whether a field of `record` has a read rule the principal of `env` does not satisfy (or that
/// cannot be evaluated for them).
fn field_withheld(lookup: Lookup<'_>, env: &Env<'_>, record: &Record, name: &str) -> bool {
    let Ok(id) = FieldId::new(name) else {
        return false;
    };
    let Some(rule) = lookup
        .tenant
        .schema
        .objects
        .get(&record.object)
        .and_then(|object| object.fields.get(&id))
        .and_then(|def| def.read.as_ref())
    else {
        return false;
    };
    let scoped = Env {
        record: Some(record),
        old: Some(record),
        new: Some(record),
        params: env.params,
        now: env.now,
        principal: env.principal,
        roles: env.roles,
        lookup: env.lookup,
    };
    !matches!(expr::holds(rule, &scoped), Ok(true))
}

/// The record a reference field of the scope's record points at, when the snapshot holds it.
fn through_target(
    lookup: Lookup<'_>,
    env: &Env<'_>,
    scope: &Scope,
    field: &str,
) -> Option<Arc<Record>> {
    let record = scope_record(env, scope)?;
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
    lookup.record(object, *target).ok().flatten().cloned()
}

/// Whether an expression reads a record or a field the viewer may not read: a field of the
/// subject with a read rule the viewer fails, or a followed reference whose record is not
/// visible to the viewer, or whose attribute has a read rule the viewer fails.
fn reads_withheld(e: &Expr, env: &Env<'_>, lookup: Lookup<'_>, ctx: &Context) -> bool {
    let here = match e {
        Expr::Var(scope, name) => {
            scope_record(env, scope).is_some_and(|record| field_withheld(lookup, env, record, name))
        }
        Expr::Through(scope, field, attribute) => match through_target(lookup, env, scope, field) {
            Some(target) => {
                !matches!(
                    visible_in(lookup, &ctx.principal, &target, ctx.now),
                    Ok(true)
                ) || field_withheld(lookup, env, &target, attribute)
            }
            None => false,
        },
        _ => false,
    };
    here || children(e)
        .into_iter()
        .any(|child| reads_withheld(child, env, lookup, ctx))
}

/// One invocation in progress: the working copy of the state its steps change, and what they
/// changed so far.
struct Run<'a> {
    ctx: &'a Context,
    op: &'a TenantOperationDef,
    operation: &'a TenantOperationId,
    invoker: PrincipalId,
    key: &'a str,
    roles: BTreeSet<RoleId>,
    subject: Option<Arc<Record>>,
    /// The inputs, and the record of each create step that ran, by step id.
    params: BTreeMap<String, Value>,
    work: TenantState,
    /// The snapshot's coverage, with each record a step created.
    coverage: Coverage,
    ops: Vec<DeltaOp>,
    events: Vec<EventBody>,
    evidence: Vec<Check>,
    created: Vec<RecordId>,
    changed: Vec<RecordId>,
    deleted: Vec<RecordId>,
    entries: Vec<LedgerEntry>,
}

impl Run<'_> {
    fn lookup(&self) -> Lookup<'_> {
        Lookup {
            tenant: &self.work,
            coverage: &self.coverage,
        }
    }

    fn fail(&self, kind: ErrorKind, rule: String, detail: String) -> DomainError {
        refusal(&self.evidence, kind, rule, detail)
    }

    /// The refusal of the whole invocation for a refused step: the step's own kind, its message
    /// prefixed with the step, and a failed check naming the step. A snapshot that fell short is
    /// passed on as it is, so the shell loads what is missing and decides again.
    fn refused(&self, step: &str, error: DomainError) -> DomainError {
        if error.kind == ErrorKind::InsufficientState {
            return error;
        }
        let DomainError {
            kind,
            message,
            evidence: carried,
            missing,
        } = error;
        let mut evidence = if carried.is_empty() {
            self.evidence.clone()
        } else {
            carried
        };
        evidence.push(Check::fail(
            format!("operation:{}:step:{step}", self.operation),
            message.clone(),
        ));
        DomainError {
            kind,
            message: format!("step {step}: {message}"),
            evidence,
            missing,
        }
    }

    /// Evaluate an expression of a step: the inputs and earlier create steps as parameters, the
    /// subject as the record, the invoker as the principal.
    fn eval(&self, what: &str, e: &Expr) -> Result<Value, DomainError> {
        let record = self.subject.as_deref();
        let env = Env {
            record,
            old: record,
            new: record,
            params: &self.params,
            now: self.ctx.now,
            principal: &self.ctx.principal,
            roles: &self.roles,
            lookup: Some(self.lookup()),
        };
        expr::eval(e, &env).map_err(|error| {
            missing_error(e, &env).unwrap_or_else(|| {
                self.fail(
                    ErrorKind::Evaluation,
                    format!("operation:{}:{what}", self.operation),
                    format!("{what}: {e}: {error}"),
                )
            })
        })
    }

    /// D-6547: a step acts with a power of the operation, one its definer had to hold, without
    /// a condition, when it defined the operation; never with the invoker's own.
    fn power(&mut self, action: Action, object: &ObjectTypeId) -> Result<(), DomainError> {
        let rule = format!("operation:{}:power", self.operation);
        let (actions, _) = operation_powers(self.op);
        if actions.contains(&(action, Resource::Object(object.clone()))) {
            self.evidence.push(Check::pass(
                rule,
                format!("{action:?} on {object}, a power of the operation that its definer held"),
            ));
            Ok(())
        } else {
            Err(self.fail(
                ErrorKind::Unauthorized,
                rule,
                format!("the operation holds no power to {action:?} {object}"),
            ))
        }
    }

    /// The record id the `record` expression of an update or delete step names.
    fn target(&self, step: &str, record: &Expr) -> Result<RecordId, DomainError> {
        match self.eval(&format!("{step}.record"), record)? {
            Value::Ref(id) => Ok(id),
            other => Err(self.fail(
                ErrorKind::InvalidIntent,
                format!("operation:{}:{step}.record", self.operation),
                format!(
                    "the record of step {step} is {}, not a reference",
                    other.kind()
                ),
            )),
        }
    }

    /// Put a record into the working copy, where the steps after this one see it.
    fn put(&mut self, record: Arc<Record>) {
        if let Coverage::Partial(loaded) = &mut self.coverage {
            loaded.insert(Need::Record {
                object: record.object.clone(),
                id: record.id,
            });
        }
        self.work
            .records
            .entry(record.object.clone())
            .or_default()
            .insert(record.id, record);
    }

    fn mark_changed(&mut self, id: RecordId) {
        if !self.created.contains(&id) && !self.changed.contains(&id) {
            self.changed.push(id);
        }
    }

    fn step(&mut self, step: &TenantOperationStep) -> Result<(), DomainError> {
        match step {
            TenantOperationStep::CreateRecord { id, object, fields } => {
                self.create(id, object, fields)
            }
            TenantOperationStep::UpdateRecord {
                id,
                object,
                record,
                fields,
            } => self.update(id, object, record, fields),
            TenantOperationStep::DeleteRecord { id, object, record } => {
                self.delete(id, object, record)
            }
            TenantOperationStep::PostLedger {
                id,
                owner,
                unit,
                scope,
                amount,
                reason,
                reference,
            } => self.post(
                id,
                owner,
                unit,
                scope.as_ref(),
                amount,
                reason,
                reference.as_ref(),
            ),
            TenantOperationStep::ReverseLedger {
                id,
                posting,
                reference,
                reason,
            } => self.reverse(id, posting, reference, reason),
            TenantOperationStep::FindRecord {
                id,
                object,
                predicate,
                error,
            } => self.find(id, object, predicate, error),
            TenantOperationStep::Branch {
                id,
                test,
                matched,
                unmatched,
            } => self.branch(id, test, matched, unmatched),
        }
    }

    /// Whether an expression of a step holds, evaluated as [`Run::eval`] evaluates one.
    fn holds(&self, what: &str, e: &Expr) -> Result<bool, DomainError> {
        let record = self.subject.as_deref();
        let env = Env {
            record,
            old: record,
            new: record,
            params: &self.params,
            now: self.ctx.now,
            principal: &self.ctx.principal,
            roles: &self.roles,
            lookup: Some(self.lookup()),
        };
        expr::holds(e, &env).map_err(|error| {
            missing_error(e, &env).unwrap_or_else(|| {
                self.fail(
                    ErrorKind::Evaluation,
                    format!("operation:{}:{what}", self.operation),
                    format!("{what}: {e}: {error}"),
                )
            })
        })
    }

    /// A find-record step: the records of the object the invoker may read whose predicate holds,
    /// the predicate's `record` being each candidate in turn. None binds `param.<step>` to null,
    /// one to its id, and more than one refuses with the step's error code. A record the invoker
    /// may not read is not a candidate, so the outcome does not reveal that it exists.
    fn find(
        &mut self,
        step: &str,
        object: &ObjectTypeId,
        predicate: &Expr,
        error: &str,
    ) -> Result<(), DomainError> {
        let schema = self.work.schema.clone();
        if !schema.objects.contains_key(object) {
            return Err(self.fail(
                ErrorKind::UnknownObject,
                format!("operation:{}:{step}", self.operation),
                format!("no object type {object}"),
            ));
        }
        let now = self.ctx.now;
        let candidates: Vec<Arc<Record>> =
            self.lookup().all_of(object)?.into_iter().cloned().collect();
        let mut found: Vec<RecordId> = Vec::new();
        for candidate in candidates {
            if !visible_in(self.lookup(), &self.ctx.principal, &candidate, now)? {
                continue;
            }
            let env = Env {
                record: Some(&candidate),
                old: Some(&candidate),
                new: Some(&candidate),
                params: &self.params,
                now,
                principal: &self.ctx.principal,
                roles: &self.roles,
                lookup: Some(self.lookup()),
            };
            let matches = expr::holds(predicate, &env).map_err(|e| {
                missing_error(predicate, &env).unwrap_or_else(|| {
                    self.fail(
                        ErrorKind::Evaluation,
                        format!("operation:{}:{step}.predicate", self.operation),
                        format!("{step}.predicate: {predicate}: {e}"),
                    )
                })
            })?;
            if matches {
                found.push(candidate.id);
            }
        }
        let rule = format!("operation:{}:{step}", self.operation);
        match found.as_slice() {
            [] => {
                self.params.insert(step.to_string(), Value::Null);
                self.evidence
                    .push(Check::pass(rule, format!("no {object} matches {step}")));
                Ok(())
            }
            [one] => {
                self.params.insert(step.to_string(), Value::Ref(*one));
                self.evidence
                    .push(Check::pass(rule, format!("{object} {one} matches {step}")));
                Ok(())
            }
            _ => Err(self.fail(
                ErrorKind::ConstraintViolated,
                rule,
                format!(
                    "{error}: {} {object} records match step {step}, where at most one may",
                    found.len()
                ),
            )),
        }
    }

    /// A branch step: its test is read on the state the steps before it leave, and the steps of
    /// the side it takes run in order. A binding that side makes is not read after the branch.
    fn branch(
        &mut self,
        step: &str,
        test: &BranchTest,
        matched: &[TenantOperationStep],
        unmatched: &[TenantOperationStep],
    ) -> Result<(), DomainError> {
        let rule = format!("operation:{}:{step}.test", self.operation);
        let taken = match test {
            BranchTest::Found { binding } => match self.params.get(binding) {
                Some(Value::Ref(_)) => true,
                Some(Value::Null) | None => false,
                Some(other) => {
                    return Err(self.fail(
                        ErrorKind::InvalidIntent,
                        rule,
                        format!(
                            "step {step} tests {binding}, which is {}, not a found record",
                            other.kind()
                        ),
                    ))
                }
            },
            BranchTest::Condition { expr } => self.holds(&format!("{step}.test"), expr)?,
        };
        self.evidence.push(Check::pass(
            rule,
            format!("{step}: {}", if taken { "matched" } else { "unmatched" }),
        ));
        let side = if taken { matched } else { unmatched };
        let bound: BTreeSet<String> = self.params.keys().cloned().collect();
        for inner in side {
            if let Err(error) = self.step(inner) {
                return Err(self.refused(step_id(inner), error));
            }
        }
        self.params.retain(|name, _| bound.contains(name));
        Ok(())
    }

    fn create(
        &mut self,
        step: &str,
        object: &ObjectTypeId,
        fields: &BTreeMap<FieldId, Expr>,
    ) -> Result<(), DomainError> {
        self.power(Action::Create, object)?;
        let schema = self.work.schema.clone();
        let Some(def) = schema.objects.get(object) else {
            return Err(self.fail(
                ErrorKind::UnknownObject,
                format!("operation:{}:{step}", self.operation),
                format!("no object type {object}"),
            ));
        };
        let now = self.ctx.now;
        let mut values: BTreeMap<FieldId, Value> = BTreeMap::new();
        for (field, e) in fields {
            let value = self.eval(&format!("{step}.{field}"), e)?;
            if !value.is_null() {
                values.insert(field.clone(), value);
            }
        }
        // Defaults, as for a direct create, for the fields the step does not write.
        let mut defaulted = Vec::new();
        {
            let no_params = BTreeMap::new();
            let env = Env {
                record: None,
                old: None,
                new: None,
                params: &no_params,
                now,
                principal: &self.ctx.principal,
                roles: &self.roles,
                lookup: Some(self.lookup()),
            };
            for field in def.fields.values() {
                if fields.contains_key(&field.id) {
                    continue;
                }
                let Some(default) = &field.default else {
                    continue;
                };
                let value = expr::eval(default, &env).map_err(|e| {
                    self.fail(
                        ErrorKind::Evaluation,
                        format!("default:{object}.{}", field.id),
                        format!("default of {}: {e}", field.id),
                    )
                })?;
                if !value.is_null() {
                    defaulted.push((field.id.clone(), value, default.to_string()));
                }
            }
        }
        for (field, value, text) in defaulted {
            self.evidence
                .push(Check::pass(format!("default:{object}.{field}"), text));
            values.insert(field, value);
        }
        let id = RecordId(self.work.next_record);
        crate::normalise::normalise_fields(def, &mut values);
        let candidate = Record {
            id,
            object: object.clone(),
            version: 1,
            // OPR-1548: the invoker made it, so the journal says who acted.
            created_by: Some(self.invoker.clone()),
            created_at: now,
            updated_at: now,
            fields: values,
        };
        for workflow in schema.workflows_of(object) {
            let rule = format!("workflow:{}:initial", workflow.id);
            match candidate.get(&workflow.field) {
                Value::Str(state) if workflow.initial.contains(state) => self
                    .evidence
                    .push(Check::pass(rule, format!("starts in {state}"))),
                other => {
                    return Err(self.fail(
                        ErrorKind::WorkflowViolation,
                        rule,
                        format!("{other:?} is not an initial state of {}", workflow.id),
                    ))
                }
            }
        }
        let validation = validate_record(
            self.lookup(),
            &schema,
            def,
            &candidate,
            None,
            &self.ctx.principal,
            now,
        )?;
        validation.into_result(&mut self.evidence)?;
        let record = Arc::new(candidate);
        self.work.next_record += 1;
        self.put(record.clone());
        self.ops.push(DeltaOp::PutRecord(record.clone()));
        self.events.push(EventBody::RecordCreated { record });
        self.created.push(id);
        self.params.insert(step.to_string(), Value::Ref(id));
        Ok(())
    }

    fn update(
        &mut self,
        step: &str,
        object: &ObjectTypeId,
        record: &Expr,
        fields: &BTreeMap<FieldId, Expr>,
    ) -> Result<(), DomainError> {
        self.power(Action::Update, object)?;
        let schema = self.work.schema.clone();
        let Some(def) = schema.objects.get(object) else {
            return Err(self.fail(
                ErrorKind::UnknownObject,
                format!("operation:{}:{step}", self.operation),
                format!("no object type {object}"),
            ));
        };
        let now = self.ctx.now;
        let id = self.target(step, record)?;
        let Some(old) = self.lookup().record(object, id)?.cloned() else {
            return Err(self.fail(
                ErrorKind::UnknownRecord,
                format!("operation:{}:{step}.record", self.operation),
                format!("no {object} {id}"),
            ));
        };
        let before: &Record = &old;
        let mut candidate = Record::clone(before);
        for (field, e) in fields {
            let value = self.eval(&format!("{step}.{field}"), e)?;
            if value.is_null() {
                candidate.fields.remove(field);
            } else {
                candidate.fields.insert(field.clone(), value);
            }
        }
        candidate.version = before.version + 1;
        candidate.updated_at = now;

        // A change of a workflow field follows a declared transition, and its assignments are
        // made; the guard is not asked of the invoker, because the step acts as the operation
        // (D-6547).
        let no_params = BTreeMap::new();
        for workflow in schema.workflows_of(object) {
            let from = before.get(&workflow.field).clone();
            let to = candidate.get(&workflow.field).clone();
            if from == to {
                continue;
            }
            let rule = format!("workflow:{}", workflow.id);
            let Some(transition) = workflow.transitions.iter().find(|t| {
                Some(t.from.as_str()) == from.as_str() && Some(t.to.as_str()) == to.as_str()
            }) else {
                return Err(self.fail(
                    ErrorKind::WorkflowViolation,
                    rule,
                    format!("no transition from {from:?} to {to:?}"),
                ));
            };
            let mut assigned = Vec::new();
            {
                let env = Env {
                    record: Some(before),
                    old: Some(before),
                    new: Some(&candidate),
                    params: &no_params,
                    now,
                    principal: &self.ctx.principal,
                    roles: &self.roles,
                    lookup: Some(self.lookup()),
                };
                for (field, value) in &transition.set {
                    let value = expr::eval(value, &env).map_err(|e| {
                        missing_error(value, &env).unwrap_or_else(|| {
                            self.fail(
                                ErrorKind::Evaluation,
                                format!("{rule}:{}", transition.name),
                                format!("{}: set {field}: {e}", transition.name),
                            )
                        })
                    })?;
                    assigned.push((field.clone(), value));
                }
            }
            for (field, value) in assigned {
                if value.is_null() {
                    candidate.fields.remove(&field);
                } else {
                    candidate.fields.insert(field, value);
                }
            }
            self.evidence.push(Check::pass(
                format!("{rule}:{}", transition.name),
                format!(
                    "{} -> {}, as the operation; the guard is not asked of the invoker",
                    transition.from, transition.to
                ),
            ));
        }

        crate::normalise::normalise_fields(def, &mut candidate.fields);
        let validation = validate_record(
            self.lookup(),
            &schema,
            def,
            &candidate,
            Some(before),
            &self.ctx.principal,
            now,
        )?;
        validation.into_result(&mut self.evidence)?;
        let after = Arc::new(candidate);
        self.put(after.clone());
        self.ops.push(DeltaOp::PutRecord(after.clone()));
        self.events.push(EventBody::RecordUpdated {
            before: old.clone(),
            after,
            transitions: None,
        });
        self.mark_changed(id);
        Ok(())
    }

    fn delete(
        &mut self,
        step: &str,
        object: &ObjectTypeId,
        record: &Expr,
    ) -> Result<(), DomainError> {
        self.power(Action::Delete, object)?;
        let schema = self.work.schema.clone();
        if !schema.objects.contains_key(object) {
            return Err(self.fail(
                ErrorKind::UnknownObject,
                format!("operation:{}:{step}", self.operation),
                format!("no object type {object}"),
            ));
        }
        let now = self.ctx.now;
        let id = self.target(step, record)?;
        let Some(old) = self.lookup().record(object, id)?.cloned() else {
            return Err(self.fail(
                ErrorKind::UnknownRecord,
                format!("operation:{}:{step}.record", self.operation),
                format!("no {object} {id}"),
            ));
        };
        // The references to it, as a direct delete treats them: a restricting or required one
        // refuses; the others are cleared, every field of one referrer in one rewrite.
        let target = Value::Ref(id);
        let mut cleared: BTreeMap<(ObjectTypeId, RecordId), (Arc<Record>, Record)> =
            BTreeMap::new();
        for (from, field, is_list) in schema.referrers(object) {
            let Some((_, on_delete, _)) = field.ty.ref_target() else {
                continue;
            };
            let referrers: Vec<Arc<Record>> = self
                .lookup()
                .by_field(&from.id, &field.id, &target)?
                .into_iter()
                .cloned()
                .collect();
            for referrer in referrers {
                if referrer.id == id && from.id == *object {
                    continue;
                }
                let rule = format!("reference:{}.{}", from.id, field.id);
                if on_delete == OnDelete::Restrict || (field.required && !is_list) {
                    return Err(self.fail(
                        ErrorKind::StillReferenced,
                        rule,
                        format!("{} {} refers to {object} {id}", from.id, referrer.id),
                    ));
                }
                let (_, rewrite) = cleared
                    .entry((from.id.clone(), referrer.id))
                    .or_insert_with(|| (referrer.clone(), Record::clone(&referrer)));
                match rewrite.fields.get(&field.id) {
                    Some(Value::List(items)) => {
                        let kept: Vec<Value> =
                            items.iter().filter(|v| **v != target).cloned().collect();
                        rewrite.fields.insert(field.id.clone(), Value::List(kept));
                    }
                    _ => {
                        rewrite.fields.remove(&field.id);
                    }
                }
                self.evidence.push(Check::pass(
                    rule,
                    format!("cleared on {} {}", from.id, referrer.id),
                ));
            }
        }
        for (before, mut rewrite) in cleared.into_values() {
            rewrite.version += 1;
            rewrite.updated_at = now;
            let after = Arc::new(rewrite);
            let changed = after.id;
            self.put(after.clone());
            self.ops.push(DeltaOp::PutRecord(after.clone()));
            self.events.push(EventBody::RecordUpdated {
                before,
                after,
                transitions: None,
            });
            self.mark_changed(changed);
        }
        if let Some(records) = self.work.records.get_mut(object) {
            records.remove(&id);
        }
        self.ops.push(DeltaOp::DeleteRecord {
            object: object.clone(),
            id,
        });
        self.events.push(EventBody::RecordDeleted { before: old });
        if !self.deleted.contains(&id) {
            self.deleted.push(id);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn post(
        &mut self,
        step: &str,
        owner: &TenantOperationPostingOwner,
        unit: &LedgerUnit,
        scope: Option<&Expr>,
        amount: &Expr,
        reason: &str,
        reference: Option<&Expr>,
    ) -> Result<(), DomainError> {
        let rule = |what: &str| format!("operation:{}:{step}.{what}", self.operation);
        let owner = self.posting_owner(step, owner)?;
        let scope = self.posting_scope(step, scope)?;
        let amount = match self.eval(&format!("{step}.amount"), amount)? {
            Value::Int(n) => LedgerAmount(n),
            other => {
                return Err(self.fail(
                    ErrorKind::InvalidIntent,
                    rule("amount"),
                    format!("the amount of step {step} is {}, not an int", other.kind()),
                ))
            }
        };
        let idempotency_key = match reference {
            None => posting_key(self.operation, &self.invoker, self.key, step),
            Some(e) => {
                let text = self.reference_text(step, e)?;
                reference_key(self.operation, &text, step)
            }
        };
        let request = LedgerPostingRequest {
            owner,
            unit: unit.clone(),
            amount,
            idempotency_key,
            reason: reason.to_string(),
            corrects: None,
            scope,
        };
        self.post_to_ledger(request)
    }

    /// The owner of a posting step, as the posting names it.
    fn posting_owner(
        &self,
        step: &str,
        owner: &TenantOperationPostingOwner,
    ) -> Result<LedgerOwner, DomainError> {
        let rule = |what: &str| format!("operation:{}:{step}.{what}", self.operation);
        Ok(match owner {
            TenantOperationPostingOwner::Invoker => LedgerOwner::Principal {
                principal: self.invoker.clone(),
            },
            TenantOperationPostingOwner::Pool { pool } => LedgerOwner::Pool { pool: pool.clone() },
            TenantOperationPostingOwner::Principal { principal } => {
                match self.eval(&format!("{step}.owner"), principal)? {
                    Value::Str(text) => match PrincipalId::new(text.clone()) {
                        Ok(principal) => LedgerOwner::Principal { principal },
                        Err(_) => {
                            return Err(self.fail(
                                ErrorKind::InvalidIntent,
                                rule("owner"),
                                format!(
                                    "the owner of step {step}, {text:?}, is not a principal id"
                                ),
                            ))
                        }
                    },
                    other => {
                        return Err(self.fail(
                            ErrorKind::InvalidIntent,
                            rule("owner"),
                            format!(
                                "the owner of step {step} is {}, not a principal id",
                                other.kind()
                            ),
                        ))
                    }
                }
            }
        })
    }

    /// The scope of a posting step, as the posting names it.
    fn posting_scope(
        &self,
        step: &str,
        scope: Option<&Expr>,
    ) -> Result<Option<LedgerScope>, DomainError> {
        let rule = |what: &str| format!("operation:{}:{step}.{what}", self.operation);
        Ok(match scope {
            None => None,
            Some(e) => match self.eval(&format!("{step}.scope"), e)? {
                Value::Null => None,
                Value::Str(text) => match LedgerScope::new(text) {
                    Ok(scope) => Some(scope),
                    Err(_) => {
                        return Err(self.fail(
                            ErrorKind::LimitExceeded,
                            rule("scope"),
                            format!("the scope of step {step} is longer than 128 characters"),
                        ))
                    }
                },
                other => {
                    return Err(self.fail(
                        ErrorKind::InvalidIntent,
                        rule("scope"),
                        format!("the scope of step {step} is {}, not text", other.kind()),
                    ))
                }
            },
        })
    }

    /// The text of a reference of a posting step: the id of the record it is for, or text.
    fn reference_text(&self, step: &str, reference: &Expr) -> Result<String, DomainError> {
        match self.eval(&format!("{step}.reference"), reference)? {
            Value::Ref(id) => Ok(id.to_string()),
            Value::Str(text) if !text.is_empty() => Ok(text),
            other => Err(self.fail(
                ErrorKind::InvalidIntent,
                format!("operation:{}:{step}.reference", self.operation),
                format!(
                    "the reference of step {step} is {}, not a record or text",
                    other.kind()
                ),
            )),
        }
    }

    /// A reverse-ledger step (WG-5598): the reversal of the posting step `posting` of this operation,
    /// made under the reference. The posting's owner, unit and scope are the ones it names; the
    /// reversal is refused when that posting was never made or was already reversed.
    fn reverse(
        &mut self,
        step: &str,
        posting: &str,
        reference: &Expr,
        reason: &str,
    ) -> Result<(), DomainError> {
        let op = self.op;
        let found = crate::definitions::all_steps(&op.steps)
            .into_iter()
            .find_map(|s| match s {
                TenantOperationStep::PostLedger {
                    id,
                    owner,
                    unit,
                    scope,
                    ..
                } if id == posting => Some((owner, unit, scope.as_ref())),
                _ => None,
            });
        let Some((owner, unit, scope)) = found else {
            return Err(self.fail(
                ErrorKind::InvalidIntent,
                format!("operation:{}:{step}.posting", self.operation),
                format!(
                    "step {step} reverses {posting}, which is no posting step of the operation"
                ),
            ));
        };
        let owner = self.posting_owner(step, owner)?;
        let scope = self.posting_scope(step, scope)?;
        let text = self.reference_text(step, reference)?;
        let request = LedgerPostingRequest {
            owner,
            unit: unit.clone(),
            // The amount is the negated amount of the posting, which the ledger reads from the entry.
            amount: LedgerAmount(0),
            idempotency_key: reversal_key(self.operation, &text, posting),
            reason: reason.to_string(),
            corrects: None,
            scope,
        };
        let posting_key = reference_key(self.operation, &text, posting);
        let state = self.ledger_state();
        let posted =
            crate::ledger::reverse_after(&state, &self.ops, &request, &posting_key, self.ctx)?;
        self.absorb(posted);
        Ok(())
    }

    /// The one call into the ledger engine (contract decide-ledger-posting CT-280): the posting
    /// is decided against the working copy after the operations of the steps before it
    /// ([`crate::ledger::post_after`]), so it sees the headers and entries an earlier posting step
    /// of this invocation wrote (D-6548). On acceptance its delta operations and events join the
    /// invocation's and its entry the result; a refusal (an undeclared unit or owner, a floor, a
    /// cap, a key used with another request) refuses the invocation, and a snapshot that lacks the
    /// account's header or the entry of the key is `InsufficientState` naming it.
    fn post_to_ledger(&mut self, request: LedgerPostingRequest) -> Result<(), DomainError> {
        let state = self.ledger_state();
        let posted = crate::ledger::post_after(&state, &self.ops, &request, self.ctx)?;
        self.absorb(posted);
        Ok(())
    }

    /// The snapshot the ledger decides on: the working copy of the tenant and the coverage.
    fn ledger_state(&self) -> State {
        State {
            tenant_id: self.work.id.clone(),
            tenant: Some(Arc::new(self.work.clone())),
            coverage: self.coverage.clone(),
        }
    }

    /// What an accepted posting adds to the invocation: its operations, events, checks and entry.
    fn absorb(&mut self, posted: crate::ledger::Posted) {
        self.evidence.extend(posted.evidence);
        self.ops.extend(posted.ops);
        self.events.extend(posted.events);
        self.entries.push(posted.result.entry);
    }

    /// The accepted invocation: one delta with every change, the events of every change in step
    /// order, a publication of each, and the result.
    fn finish(self, base: &TenantState) -> Transition {
        let Run {
            ctx,
            operation,
            work,
            ops,
            events,
            evidence,
            created,
            changed,
            deleted,
            entries,
            ..
        } = self;
        let revision = StateRevision(base.revision.0 + 1);
        let first_seq = base.event_seq + 1;
        let events: Vec<DomainEvent> = events
            .into_iter()
            .enumerate()
            .map(|(i, body)| DomainEvent {
                tenant: base.id.clone(),
                seq: first_seq + i as u64,
                revision,
                at: ctx.now,
                principal: ctx.principal.clone(),
                correlation: ctx.correlation.clone(),
                body,
            })
            .collect();
        let effects: Vec<Effect> = events
            .iter()
            .map(|event| Effect::Publish { seq: event.seq })
            .collect();
        Transition {
            delta: StateDelta {
                tenant: base.id.clone(),
                base_revision: Some(base.revision),
                new_revision: revision,
                next_record: work.next_record,
                event_seq: base.event_seq + events.len() as u64,
                ops,
            },
            events,
            effects,
            evidence,
            output: Output::Operation {
                operation: operation.clone(),
                created,
                changed,
                deleted,
                entries,
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The plan of what an invocation reads.
// ---------------------------------------------------------------------------------------------

/// What an invocation is expected to read, before anything is loaded: the subject, the records
/// the update and delete steps name through an input or a literal, the referrers of a deleted
/// record, and the unique values and references of the fields a step writes from an input or a
/// literal. Added to `out`.
pub fn needs(
    schema: &TenantSchema,
    operation: &TenantOperationId,
    inputs: &BTreeMap<String, Value>,
    out: &mut BTreeSet<Need>,
) {
    plan(schema, None, operation, inputs, out);
}

/// [`needs`] once the snapshot holds the subject and the records the steps name: the fields
/// read from the subject are known, and an update is validated over the stored record's fields
/// as well as the written ones. A decision then asks for what remains (a reference a condition
/// follows, a value a step computes) one need at a time, which the operation's limit of sixteen
/// steps bounds.
pub fn refine_needs(
    tenant: &TenantState,
    operation: &TenantOperationId,
    inputs: &BTreeMap<String, Value>,
) -> Vec<Need> {
    let mut out = BTreeSet::new();
    plan(&tenant.schema, Some(tenant), operation, inputs, &mut out);
    out.into_iter().collect()
}

/// The value of a step expression when it can be known before the invocation runs: a literal,
/// an input, or a field of the subject when the snapshot holds it.
fn known(e: &Expr, subject: Option<&Record>, inputs: &BTreeMap<String, Value>) -> Option<Value> {
    match e {
        Expr::Lit(value) => Some(value.clone()),
        Expr::Param(name) => inputs.get(name).cloned(),
        Expr::Var(_, name) => subject.and_then(|record| expr::attribute(Some(record), name).ok()),
        _ => None,
    }
}

fn known_fields(
    fields: &BTreeMap<FieldId, Expr>,
    subject: Option<&Record>,
    inputs: &BTreeMap<String, Value>,
) -> BTreeMap<FieldId, Value> {
    fields
        .iter()
        .filter_map(|(field, e)| known(e, subject, inputs).map(|value| (field.clone(), value)))
        .collect()
}

/// The needs of a posting step: the account's header and the limit of the account, when its owner
/// and scope are known before the run (WG-5598: a reversal asks for the same).
fn posting_needs_of(
    loaded: Option<&TenantState>,
    owner: &TenantOperationPostingOwner,
    unit: &LedgerUnit,
    scope: Option<&Expr>,
    subject: Option<&Record>,
    inputs: &BTreeMap<String, Value>,
    out: &mut BTreeSet<Need>,
) {
    // The account's header, when its owner and scope are known before the run; the
    // entry of the posting's key (which names the invoker and the invocation's key)
    // and an owner the invoker is are asked for by the decision, one need at a time.
    let Some(tenant) = loaded else {
        return;
    };
    let owner = match owner {
        TenantOperationPostingOwner::Pool { pool } => {
            Some(LedgerOwner::Pool { pool: pool.clone() })
        }
        TenantOperationPostingOwner::Principal { principal } => {
            match known(principal, subject, inputs) {
                Some(Value::Str(text)) => PrincipalId::new(text)
                    .ok()
                    .map(|principal| LedgerOwner::Principal { principal }),
                _ => None,
            }
        }
        TenantOperationPostingOwner::Invoker => None,
    };
    let scope = match scope {
        None => Some(String::new()),
        Some(e) => match known(e, subject, inputs) {
            Some(Value::Str(text)) => Some(text),
            Some(Value::Null) => Some(String::new()),
            _ => None,
        },
    };
    if let (Some(owner), Some(scope)) = (owner, scope) {
        out.insert(Need::Ledger(crate::types::LedgerNeed::Account {
            account: crate::ledger::account_id(&tenant.id, &owner, unit, &scope),
        }));
        // The limit of this account, and no other (D-6554).
        out.insert(Need::Ledger(crate::types::LedgerNeed::Limit {
            owner,
            unit: unit.clone(),
            scope: crate::types::LedgerScope(scope),
        }));
    }
}

fn plan(
    schema: &TenantSchema,
    loaded: Option<&TenantState>,
    operation: &TenantOperationId,
    inputs: &BTreeMap<String, Value>,
    out: &mut BTreeSet<Need>,
) {
    let Some(op) = operation_of(schema, operation) else {
        return;
    };
    let subject: Option<Arc<Record>> = op.subject.as_ref().and_then(|def| {
        let Some(Value::Ref(id)) = inputs.get(&def.input) else {
            return None;
        };
        out.insert(Need::Record {
            object: def.object.clone(),
            id: *id,
        });
        loaded.and_then(|tenant| tenant.record(&def.object, *id).cloned())
    });
    let subject = subject.as_deref();
    for step in crate::definitions::all_steps(&op.steps) {
        match step {
            TenantOperationStep::CreateRecord { object, fields, .. } => {
                field_needs(schema, out, object, &known_fields(fields, subject, inputs));
            }
            TenantOperationStep::FindRecord { object, .. } => {
                out.insert(Need::AllOf(object.clone()));
            }
            TenantOperationStep::Branch { .. } => {}
            TenantOperationStep::UpdateRecord {
                object,
                record,
                fields,
                ..
            } => {
                let mut written = known_fields(fields, subject, inputs);
                if let Some(Value::Ref(id)) = known(record, subject, inputs) {
                    out.insert(Need::Record {
                        object: object.clone(),
                        id,
                    });
                    if let Some(stored) = loaded.and_then(|tenant| tenant.record(object, id)) {
                        let mut merged = stored.fields.clone();
                        for (field, value) in written {
                            if value.is_null() {
                                merged.remove(&field);
                            } else {
                                merged.insert(field, value);
                            }
                        }
                        written = merged;
                    }
                }
                field_needs(schema, out, object, &written);
            }
            TenantOperationStep::DeleteRecord { object, record, .. } => {
                if let Some(Value::Ref(id)) = known(record, subject, inputs) {
                    out.insert(Need::Record {
                        object: object.clone(),
                        id,
                    });
                    for (from, field, _) in schema.referrers(object) {
                        out.insert(Need::ByField {
                            object: from.id.clone(),
                            field: field.id.clone(),
                            value: Value::Ref(id),
                        });
                    }
                }
            }
            TenantOperationStep::PostLedger {
                owner, unit, scope, ..
            } => posting_needs_of(loaded, owner, unit, scope.as_ref(), subject, inputs, out),
            TenantOperationStep::ReverseLedger { posting, .. } => {
                let found = crate::definitions::all_steps(&op.steps)
                    .into_iter()
                    .find_map(|s| match s {
                        TenantOperationStep::PostLedger {
                            id,
                            owner,
                            unit,
                            scope,
                            ..
                        } if id == posting => Some((owner, unit, scope.as_ref())),
                        _ => None,
                    });
                if let Some((owner, unit, scope)) = found {
                    posting_needs_of(loaded, owner, unit, scope, subject, inputs, out);
                }
            }
        }
    }
}
