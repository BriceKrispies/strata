//! The definitions a tenant declares beyond its records' shape: triggers and automations
//! (D-6520 to D-6539), tenant operations (D-6546 to D-6549) and composite unique constraints
//! (D-6558). The kernel holds and validates them; it does not run them. The automation worker
//! runs automations (WG-4992) and the tenant-operation decision runs operations.
//!
//! Every check here is static and made when the schema that holds the definition is checked
//! ([`crate::schema::TenantSchemaExt::problems`]), so a definition that names something that does
//! not exist is refused when it is defined, and a change that removes something a definition
//! names is refused while the definition names it. What the kernel does not hold is not checked
//! here: segments live in the segment store beside the tenant state, and schedules and webhook
//! subscriptions are not built. Settings, capabilities, the ledger declarations and the mail and
//! notice templates are held in the schema ([`crate::settings`], [`crate::capabilities`],
//! [`crate::ledger`], [`crate::templates`]), so a definition that names one is checked here. Each
//! kind of reference has one function called wherever a definition names the kind
//! ([`segment_reference`], [`ledger_unit_reference`], [`template_reference`],
//! [`schedule_reference`]); the two kinds the schema does not hold answer
//! [`ReferenceCheck::NotChecked`].
//!
//! The authority checks that depend on the defining principal (the run-as roles of an automation
//! under ADM-152, the powers the steps of an operation use under D-6547) are made by
//! `transition::change_schema`, which knows who is defining.

use crate::expr::{self, Expr, Shape, TypeEnv};
use crate::limits;
use crate::schema::{Action, FieldDef, FieldType, ObjectTypeDef, Resource, TenantSchema};
use crate::settings;
use crate::types::{
    public_role, AutomationDef, AutomationStep, AutomationStepDef, BranchTest, FieldId,
    FieldTypeExt, LedgerUnit, ObjectTypeId, OperationSettingLevel, RoleId, SettingId, SettingLevel,
    StepName, TemplateId, TenantOperationDef, TenantOperationPostingOwner, TenantOperationStep,
    TriggerDef, TriggerEvent, UniqueDef,
};
use std::collections::{BTreeMap, BTreeSet};

/// The capability that defining or removing a trigger or an automation needs (WFL-1520).
pub const AUTOMATIONS_DEFINE: &str = "automations-define";

/// The capability that a posting step of a tenant operation needs of its definer (D-6547).
pub const LEDGER_POST: &str = "ledger-post";

/// The answer of the check of one reference a definition makes to something the kernel may not
/// hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceCheck {
    /// The kernel holds the kind and checked: the problem, or `None` when the reference is good.
    Checked(Option<String>),
    /// The kernel does not hold the kind yet, so the reference is not checked.
    NotChecked,
}

/// Add the problem of a checked reference to `problems`, at `at`. A reference that is not checked
/// adds nothing.
fn report(check: ReferenceCheck, at: &str, problems: &mut Vec<String>) {
    if let ReferenceCheck::Checked(Some(problem)) = check {
        problems.push(format!("{at}: {problem}"));
    }
}

/// The check of a segment a definition names (a trigger on entering or leaving a segment, a
/// fan-out step). Not checked: segments live in the segment store beside the tenant state
/// (`crate::segments`, D-5030), not in the schema, so a check of the schema alone cannot see
/// them; the segment engine refuses a reference to a segment it does not hold when it runs.
pub fn segment_reference<T: std::fmt::Debug>(
    _schema: &TenantSchema,
    _segment: &T,
) -> ReferenceCheck {
    ReferenceCheck::NotChecked
}

/// The check of a ledger unit a definition names (a trigger on a posting, a posting step of an
/// automation or an operation): the unit is declared in the ledger declarations of the schema
/// (`crate::ledger`, D-5000).
pub fn ledger_unit_reference(schema: &TenantSchema, unit: &LedgerUnit) -> ReferenceCheck {
    let declared = schema
        .ledger
        .as_ref()
        .is_some_and(|ledger| ledger.units.contains_key(unit));
    ReferenceCheck::Checked(
        (!declared).then(|| format!("names ledger unit {unit}, which the tenant does not declare")),
    )
}

/// The check of a mail or notice template a definition names (a send-mail or create-notice
/// step): the schema holds a template of that id (`crate::templates`, D-7341). Which table it is
/// in is checked by `templates::problems`, which names a template of the wrong kind.
pub fn template_reference(schema: &TenantSchema, template: &TemplateId) -> ReferenceCheck {
    let held = schema
        .mail_templates
        .as_ref()
        .is_some_and(|all| all.contains_key(template))
        || schema
            .notice_templates
            .as_ref()
            .is_some_and(|all| all.contains_key(template));
    ReferenceCheck::Checked(
        (!held).then(|| format!("names template {template}, which the tenant does not define")),
    )
}

/// The check of a schedule a definition names (a trigger on a schedule falling due). Not checked:
/// schedules are not held by the kernel yet.
pub fn schedule_reference<T: std::fmt::Debug>(
    _schema: &TenantSchema,
    _schedule: &T,
) -> ReferenceCheck {
    ReferenceCheck::NotChecked
}

/// The problem with a setting a definition names, or `None` when the schema defines it or the
/// platform does (a trigger on a setting changing).
pub fn setting_reference_problem(schema: &TenantSchema, setting: &SettingId) -> Option<String> {
    let defined = crate::settings::definition(schema, setting).is_some()
        || crate::settings::PLATFORM_SETTINGS.contains(&setting.as_str());
    (!defined).then(|| format!("unknown setting {setting}"))
}

/// The problem with a capability a definition needs, or `None` when it is a platform capability or
/// one the schema declares (the capability a posting step needs of the definer of an operation).
pub fn capability_reference_problem(schema: &TenantSchema, capability: &str) -> Option<String> {
    let known = !crate::capabilities::is_product_form(capability)
        || crate::capabilities::is_declared(schema, capability);
    (!known).then(|| format!("unknown capability {capability}"))
}

/// The references a trigger's event makes to the settings of the schema and to what the kernel
/// does not hold yet.
fn event_reference_problems(
    schema: &TenantSchema,
    event: &TriggerEvent,
    at: &str,
    problems: &mut Vec<String>,
) {
    match event {
        TriggerEvent::SettingChanged { setting } => {
            if let Some(problem) = setting_reference_problem(schema, setting) {
                problems.push(format!("{at}: {problem}"));
            }
        }
        TriggerEvent::SegmentEntered { segment } | TriggerEvent::SegmentExited { segment } => {
            report(segment_reference(schema, segment), at, problems)
        }
        TriggerEvent::LedgerPosted { unit } => {
            report(ledger_unit_reference(schema, unit), at, problems)
        }
        TriggerEvent::ScheduleDue { schedule } => {
            report(schedule_reference(schema, schedule), at, problems)
        }
        TriggerEvent::RecordCreated { .. }
        | TriggerEvent::RecordUpdated { .. }
        | TriggerEvent::RecordDeleted { .. }
        | TriggerEvent::InboundWebhook { .. } => {}
        // A declared kind is held by the tenant's declarations, which the schema does not carry
        // yet, so the kind is not checked here (SCH-1576).
        TriggerEvent::DeclaredEvent { .. } => {}
    }
}

/// The parameters every event binds (D-6523).
const COMMON_PARAMS: [&str; 3] = ["seq", "at", "hop"];

/// The parameters an event of this kind binds for a trigger condition (D-6523), beyond the
/// record scopes a record event binds.
pub fn event_params(event: &TriggerEvent) -> BTreeSet<String> {
    let own: &[&str] = match event {
        TriggerEvent::RecordCreated { .. } | TriggerEvent::RecordDeleted { .. } => &[],
        TriggerEvent::RecordUpdated { .. } => &["transition"],
        TriggerEvent::SegmentEntered { .. } | TriggerEvent::SegmentExited { .. } => {
            &["person", "segment", "reason"]
        }
        TriggerEvent::LedgerPosted { .. } => &[
            "owner",
            "unit",
            "amount",
            "balance",
            "balance_before",
            "entry_sequence",
            "reason",
        ],
        TriggerEvent::SettingChanged { .. } => &["setting", "level", "subject"],
        TriggerEvent::ScheduleDue { .. } => &["schedule", "due_at"],
        TriggerEvent::InboundWebhook { .. } => &["endpoint", "event_id", "body"],
        TriggerEvent::DeclaredEvent { .. } => &["kind"],
    };
    COMMON_PARAMS
        .iter()
        .chain(own)
        .map(|name| name.to_string())
        .collect()
}

/// Every parameter any event binds (D-6523): what an automation, which any trigger may start,
/// can read of its starting event. A person who starts a run directly binds the same names.
pub fn any_event_params() -> BTreeSet<String> {
    [
        "seq",
        "at",
        "hop",
        "transition",
        "person",
        "segment",
        "reason",
        "owner",
        "unit",
        "amount",
        "balance",
        "balance_before",
        "entry_sequence",
        "setting",
        "level",
        "subject",
        "schedule",
        "due_at",
        "endpoint",
        "event_id",
        "body",
        "kind",
    ]
    .iter()
    .map(|name| name.to_string())
    .collect()
}

/// The output fields of a completed step of this kind (automation-step-result, WFL-1523).
pub fn step_outputs(action: &AutomationStep) -> &'static [&'static str] {
    match action {
        AutomationStep::CreateRecord { .. } => &["record", "version"],
        AutomationStep::UpdateRecord { .. } => &["version"],
        AutomationStep::DeleteRecord { .. } => &[],
        AutomationStep::PostLedger { .. } => &["entry_sequence", "balance"],
        AutomationStep::SendMail { .. } => &["message"],
        AutomationStep::CreateNotice { .. } => &["item"],
        AutomationStep::SendDigest { .. } => &["messages"],
        AutomationStep::CallWebhook { .. } => &["delivery", "status"],
        AutomationStep::WaitFor { .. } | AutomationStep::WaitUntil { .. } => &[],
        AutomationStep::Branch { .. } => &["taken"],
        AutomationStep::FanOut { .. } => &["members", "batches"],
        AutomationStep::RequestApproval { .. } => &["approved", "by"],
        AutomationStep::InvokeOperation { .. } => &["created", "changed", "deleted"],
        AutomationStep::End => &[],
    }
}

/// The parameters the expressions of an automation may read: every parameter an event binds,
/// `member` inside a fan-out, and `<step>_<output>` for each output of each of its steps
/// (D-6523, D-6535), and the three an inbound webhook delivery hands a run it starts directly
/// (receive-inbound-webhook, CT-172).
pub fn automation_params(def: &AutomationDef) -> BTreeSet<String> {
    let mut params = any_event_params();
    params.insert("member".to_string());
    for name in ["webhook_event", "webhook_endpoint", "webhook_body"] {
        params.insert(name.to_string());
    }
    for (name, step) in &def.steps {
        for output in step_outputs(&step.action) {
            params.insert(format!("{name}_{output}"));
        }
    }
    params
}

/// The steps one step names, in a fixed order: those its action names (then and else of a
/// branch, each of a fan-out, approved and refused of an approval), then its next step, then its
/// compensation.
pub fn successors(step: &AutomationStepDef) -> Vec<&StepName> {
    let mut out: Vec<&StepName> = match &step.action {
        AutomationStep::Branch { then, r#else, .. } => vec![then, r#else],
        AutomationStep::FanOut { each, .. } => vec![each],
        AutomationStep::RequestApproval {
            approved, refused, ..
        } => vec![approved, refused],
        _ => Vec::new(),
    };
    out.extend(step.next.iter());
    out.extend(step.compensate.iter());
    out
}

/// The steps reachable from the start step of an automation, following every step a step names.
pub fn reachable(def: &AutomationDef) -> BTreeSet<&StepName> {
    let mut seen: BTreeSet<&StepName> = BTreeSet::new();
    let mut frontier = vec![&def.start];
    while let Some(name) = frontier.pop() {
        let Some(step) = def.steps.get(name) else {
            continue;
        };
        if seen.insert(name) {
            frontier.extend(successors(step));
        }
    }
    seen
}

/// The number of nodes of an expression, each operator and each operand one node.
pub fn expr_nodes(expr: &Expr) -> usize {
    1 + match expr {
        Expr::Lit(_)
        | Expr::Var(..)
        | Expr::Through(..)
        | Expr::Param(_)
        | Expr::Now
        | Expr::Principal
        | Expr::HasRole(_) => 0,
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Graphemes(a)
        | Expr::IsEmoji(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => expr_nodes(a),
        Expr::And(items) | Expr::Or(items) => items.iter().map(expr_nodes).sum(),
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => expr_nodes(a) + expr_nodes(b),
        Expr::If(a, b, c) => expr_nodes(a) + expr_nodes(b) + expr_nodes(c),
    }
}

/// The composite unique constraints of an object type, in id order.
pub fn uniques(object: &ObjectTypeDef) -> impl Iterator<Item = &UniqueDef> {
    object.unique.iter().flat_map(BTreeMap::values)
}

/// What the definer of a tenant operation must hold (D-6547, OPR-1547): an action on an object
/// type for each record step, and a capability for each posting step, each once, in step order.
pub fn operation_powers(op: &TenantOperationDef) -> (Vec<(Action, Resource)>, BTreeSet<String>) {
    let mut actions: Vec<(Action, Resource)> = Vec::new();
    let mut capabilities = BTreeSet::new();
    for step in all_steps(&op.steps) {
        let wanted = match step {
            TenantOperationStep::CreateRecord { object, .. } => {
                Some((Action::Create, Resource::Object(object.clone())))
            }
            TenantOperationStep::UpdateRecord { object, .. } => {
                Some((Action::Update, Resource::Object(object.clone())))
            }
            TenantOperationStep::DeleteRecord { object, .. } => {
                Some((Action::Delete, Resource::Object(object.clone())))
            }
            TenantOperationStep::PostLedger { .. } | TenantOperationStep::ReverseLedger { .. } => {
                capabilities.insert(LEDGER_POST.to_string());
                None
            }
            // A find reads under the invoker's own rights, and a setting is written under the rights
            // the settings route asks of the invoker (WG-5606), not a power of the operation.
            TenantOperationStep::FindRecord { .. }
            | TenantOperationStep::Branch { .. }
            | TenantOperationStep::WriteSetting { .. } => None,
        };
        if let Some(wanted) = wanted {
            if !actions.contains(&wanted) {
                actions.push(wanted);
            }
        }
    }
    (actions, capabilities)
}

/// Every step of a list in order, each branch's steps after the branch that holds them, to any
/// depth: what a check or a count of the steps of an operation walks.
pub fn all_steps(steps: &[TenantOperationStep]) -> Vec<&TenantOperationStep> {
    let mut out = Vec::new();
    for step in steps {
        out.push(step);
        if let TenantOperationStep::Branch {
            matched, unmatched, ..
        } = step
        {
            out.extend(all_steps(matched));
            out.extend(all_steps(unmatched));
        }
    }
    out
}

/// Whether a name is one an input or a step id may take: `[a-z][a-z0-9_]*`, so that it can be
/// read as `param.<name>`.
fn is_param_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Check one expression of a definition at `at`, naming it `what`; with `truth`, it must also be
/// a truth value.
fn check_expr(
    problems: &mut Vec<String>,
    at: &str,
    what: &str,
    expr: &Expr,
    env: &TypeEnv<'_>,
    truth: bool,
) {
    match expr::check(expr, env) {
        Ok(Shape::Bool | Shape::Any) => {}
        Ok(shape) if truth => {
            problems.push(format!("{at}: {what} is {shape:?}, not a truth value"))
        }
        Ok(_) => {}
        Err(e) => problems.push(format!("{at}: {what}: {e}")),
    }
}

/// The record fields of an object type a step or condition reads through `record`, `old` and
/// `new`, and whether the object type exists.
fn object_fields<'a>(
    schema: &'a TenantSchema,
    object: &ObjectTypeId,
    none: &'a BTreeMap<FieldId, FieldDef>,
) -> (&'a BTreeMap<FieldId, FieldDef>, bool) {
    match schema.objects.get(object) {
        Some(def) => (&def.fields, true),
        None => (none, false),
    }
}

/// The problems of the composite unique constraints of one object type (D-6558, CMS-1556): two to
/// four fields, each declared, not a list and named once.
pub fn unique_problems(object: &ObjectTypeDef, problems: &mut Vec<String>) {
    for unique in uniques(object) {
        let at = format!("{}.{}", object.id, unique.id);
        let count = unique.fields.len();
        if !(limits::MIN_UNIQUE_FIELDS..=limits::MAX_UNIQUE_FIELDS).contains(&count) {
            problems.push(format!(
                "{at}: a unique constraint names {count} fields, not two to four"
            ));
        }
        for field in &unique.fields {
            match object.fields.get(field) {
                None => problems.push(format!("{at}: unknown field {field}")),
                Some(def) if matches!(def.ty, FieldType::List(_)) => {
                    problems.push(format!("{at}: {field} is a list field"))
                }
                Some(_) => {}
            }
        }
        let distinct: BTreeSet<&FieldId> = unique.fields.iter().collect();
        if distinct.len() < count {
            problems.push(format!("{at}: names a field twice"));
        }
        if unique.message.trim().is_empty() {
            problems.push(format!("{at}: a unique constraint needs a message"));
        }
    }
}

/// The problems of the triggers of a schema (WFL-1520, INT-1522, D-6523, CT-790): at most one
/// hundred; each names an automation of the schema; a record event names an object type of the
/// schema and, with a transition, a transition of a workflow of that type; the condition checks
/// against the parameters its event binds and the fields of its record, and is a truth value.
pub fn trigger_problems(
    schema: &TenantSchema,
    roles: &BTreeSet<RoleId>,
    problems: &mut Vec<String>,
) {
    let triggers: Vec<&TriggerDef> = schema.triggers.iter().flat_map(BTreeMap::values).collect();
    if triggers.len() > limits::MAX_TRIGGERS {
        problems.push(format!(
            "limit: {} triggers is more than the maximum of {}",
            triggers.len(),
            limits::MAX_TRIGGERS
        ));
    }
    let none = BTreeMap::new();
    for trigger in triggers {
        let at = format!("trigger {}", trigger.id);
        let named = schema
            .automations
            .as_ref()
            .is_some_and(|automations| automations.contains_key(&trigger.automation));
        if !named {
            problems.push(format!("{at}: unknown automation {}", trigger.automation));
        }
        event_reference_problems(schema, &trigger.on, &at, problems);
        let fields = match &trigger.on {
            TriggerEvent::RecordCreated { object }
            | TriggerEvent::RecordUpdated { object, .. }
            | TriggerEvent::RecordDeleted { object } => {
                let (fields, exists) = object_fields(schema, object, &none);
                if !exists {
                    problems.push(format!("{at}: unknown object type {object}"));
                }
                if let (
                    true,
                    TriggerEvent::RecordUpdated {
                        transition: Some(name),
                        ..
                    },
                ) = (exists, &trigger.on)
                {
                    let declared = schema
                        .workflows
                        .values()
                        .filter(|w| &w.object == object)
                        .any(|w| w.transitions.iter().any(|t| &t.name == name));
                    if !declared {
                        problems.push(format!(
                            "{at}: no workflow of {object} has a transition {name}"
                        ));
                    }
                }
                fields
            }
            _ => &none,
        };
        if let Some(condition) = &trigger.condition {
            let params = event_params(&trigger.on);
            let env = TypeEnv {
                fields,
                params: &params,
                roles,
                objects: &schema.objects,
            };
            check_expr(problems, &at, "condition", condition, &env, true);
        }
    }
}

/// The problems of one step's action at `at`, its expressions checked against `env` (no record
/// fields) and, for a branch that names an object type, that type's fields.
fn action_problems(
    schema: &TenantSchema,
    at: &str,
    action: &AutomationStep,
    env: &TypeEnv<'_>,
    problems: &mut Vec<String>,
) {
    match action {
        AutomationStep::CreateRecord { object, fields } => {
            written_problems(schema, at, object, fields, env, problems)
        }
        AutomationStep::UpdateRecord {
            object,
            record,
            fields,
        } => {
            written_problems(schema, at, object, fields, env, problems);
            check_expr(problems, at, "record", record, env, false);
        }
        AutomationStep::DeleteRecord { object, record } => {
            written_problems(schema, at, object, &BTreeMap::new(), env, problems);
            check_expr(problems, at, "record", record, env, false);
        }
        AutomationStep::PostLedger {
            owner,
            amount,
            unit,
            ..
        } => {
            check_expr(problems, at, "owner", owner, env, false);
            check_expr(problems, at, "amount", amount, env, false);
            report(ledger_unit_reference(schema, unit), at, problems);
        }
        AutomationStep::SendMail {
            to,
            address,
            segment,
            template,
            ..
        } => {
            // Exactly one target: a principal (`to`), an address or a segment (NTF-3200).
            let named = [to.is_some(), address.is_some(), segment.is_some()];
            if named.iter().filter(|set| **set).count() != 1 {
                problems.push(format!(
                    "{at}: a mail names exactly one of to, address and segment"
                ));
            }
            for (what, expr) in [("to", to), ("address", address)] {
                if let Some(expr) = expr {
                    check_expr(problems, at, what, expr, env, false);
                }
            }
            if let Some(segment) = segment {
                report(segment_reference(schema, segment), at, problems);
            }
            report(template_reference(schema, template), at, problems);
        }
        AutomationStep::SendDigest { template, .. } => {
            report(template_reference(schema, template), at, problems);
        }
        AutomationStep::CreateNotice {
            to,
            segment,
            record,
            template,
        } => {
            // An address has no inbox, so a notice names a principal or a segment (NTF-3200).
            if to.is_some() == segment.is_some() {
                problems.push(format!(
                    "{at}: a notice names exactly one of to and segment"
                ));
            }
            if let Some(to) = to {
                check_expr(problems, at, "to", to, env, false);
            }
            if let Some(segment) = segment {
                report(segment_reference(schema, segment), at, problems);
            }
            report(template_reference(schema, template), at, problems);
            if let Some(record) = record {
                check_expr(problems, at, "record", record, env, false);
            }
        }
        AutomationStep::CallWebhook { body, .. } => {
            for (key, value) in body {
                check_expr(problems, at, &format!("body {key}"), value, env, false);
            }
        }
        AutomationStep::WaitFor { duration } => {
            if duration.0 == 0 || duration.0 > limits::MAX_WAIT_MS {
                problems.push(format!("{at}: a wait is from 1 ms to 366 days"));
            }
        }
        AutomationStep::WaitUntil { at: when } => check_expr(problems, at, "at", when, env, false),
        AutomationStep::Branch {
            condition,
            object,
            record,
            ..
        } => {
            let none = BTreeMap::new();
            let fields = match (object, record) {
                (Some(object), Some(record)) => {
                    check_expr(problems, at, "record", record, env, false);
                    let (fields, exists) = object_fields(schema, object, &none);
                    if !exists {
                        problems.push(format!("{at}: unknown object type {object}"));
                    }
                    fields
                }
                (None, None) => &none,
                _ => {
                    problems.push(format!(
                        "{at}: a branch names both an object type and a record, or neither"
                    ));
                    &none
                }
            };
            let branch_env = TypeEnv {
                fields,
                params: env.params,
                roles: env.roles,
                objects: env.objects,
            };
            check_expr(problems, at, "condition", condition, &branch_env, true);
        }
        AutomationStep::FanOut { batch, segment, .. } => {
            report(segment_reference(schema, segment), at, problems);
            if *batch == 0 || *batch > limits::MAX_FAN_OUT_BATCH {
                problems.push(format!("{at}: a fan-out batch is from 1 to 100"));
            }
        }
        AutomationStep::RequestApproval { policy, .. } => {
            if !schema.policies.contains_key(policy) {
                problems.push(format!("{at}: unknown policy {policy}"));
            }
        }
        // Checked by [`invoke_operation_problems`], which needs the automation's run-as.
        AutomationStep::InvokeOperation { .. } => {}
        AutomationStep::End => {}
    }
}

/// The problems of an invoke-operation step at `at` (WFL-2685, D-7682): the operation exists, the
/// run-as roles of the automation may invoke it (its invokers name a role of the run-as or every
/// signed-in person), every input it gives is one the operation declares and its expression checks
/// and has the shape of the input's type, and every required input is given. The operation's own
/// definition bounds what the step can do: the kernel decides the invocation as the automation
/// principal, whose powers are its run-as roles and never more than the operation's steps use.
fn invoke_operation_problems(
    schema: &TenantSchema,
    automation: &AutomationDef,
    at: &str,
    action: &AutomationStep,
    env: &TypeEnv<'_>,
    problems: &mut Vec<String>,
) {
    let AutomationStep::InvokeOperation { operation, inputs } = action else {
        return;
    };
    let Some(op) = schema
        .operations
        .as_ref()
        .and_then(|all| all.get(operation))
    else {
        problems.push(format!("{at}: unknown operation {operation}"));
        return;
    };
    if !op.invokers.signed_in && op.invokers.roles.is_disjoint(&automation.run_as.roles) {
        problems.push(format!(
            "{at}: no run-as role of automation {} may invoke operation {operation}",
            automation.id
        ));
    }
    for (name, value) in inputs {
        let Some(input) = op.inputs.iter().find(|input| &input.name == name) else {
            problems.push(format!("{at}: operation {operation} has no input {name}"));
            continue;
        };
        match expr::check(value, env) {
            Ok(shape) if !shape_fits(shape, &input.ty) => problems.push(format!(
                "{at}: input {name} is {shape:?}, but operation {operation} expects {}",
                input.ty.name()
            )),
            Ok(_) => {}
            Err(e) => problems.push(format!("{at}: input {name}: {e}")),
        }
    }
    for input in &op.inputs {
        if input.required && !inputs.contains_key(&input.name) {
            problems.push(format!(
                "{at}: operation {operation} needs the input {}",
                input.name
            ));
        }
    }
}

/// Whether an expression of this shape can give a value of `ty`: a shape the check cannot pin
/// down fits any type, a whole number fits a time and a reference (as a record id does), and a
/// file or a language is text.
fn shape_fits(shape: Shape, ty: &FieldType) -> bool {
    matches!(
        (shape, ty),
        (Shape::Any, _)
            | (
                Shape::Str,
                FieldType::Str | FieldType::File { .. } | FieldType::Language
            )
            | (
                Shape::Int,
                FieldType::Int | FieldType::Time | FieldType::Ref { .. }
            )
            | (Shape::Bool, FieldType::Bool)
            | (Shape::Time, FieldType::Time)
            | (Shape::Ref, FieldType::Ref { .. })
            | (Shape::List, FieldType::List(_))
    )
}

/// The problems of the automations of a schema (D-6520 to D-6535, INT-1522, CT-792): the id is not
/// a field workflow's; at most fifty steps; the start step and every step a step names exist;
/// the run-as names roles of the schema other than public; the run key and every expression of a
/// step check against the automation's parameters; a retry, a timeout, a wait and a fan-out batch
/// are within their bounds; every step can be reached from the start step.
pub fn automation_problems(
    schema: &TenantSchema,
    roles: &BTreeSet<RoleId>,
    problems: &mut Vec<String>,
) {
    let none = BTreeMap::new();
    for automation in schema.automations.iter().flat_map(BTreeMap::values) {
        let at = format!("automation {}", automation.id);
        if schema.workflows.contains_key(&automation.id) {
            problems.push(format!("{at}: the id is taken by a field workflow"));
        }
        if automation.steps.len() > limits::MAX_AUTOMATION_STEPS {
            problems.push(format!(
                "{at}: {} steps is more than the maximum of {}",
                automation.steps.len(),
                limits::MAX_AUTOMATION_STEPS
            ));
        }
        let has_start = automation.steps.contains_key(&automation.start);
        if !has_start {
            problems.push(format!("{at}: unknown start step {}", automation.start));
        }
        for role in &automation.run_as.roles {
            if !roles.contains(role) {
                problems.push(format!("{at}: unknown role {role}"));
            }
            if *role == public_role() {
                problems.push(format!("{at}: everyone already holds public"));
            }
        }
        let params = automation_params(automation);
        let env = TypeEnv {
            fields: &none,
            params: &params,
            roles,
            objects: &schema.objects,
        };
        if let Some(key) = &automation.run_key {
            check_expr(problems, &at, "run key", key, &env, false);
        }
        for (name, step) in &automation.steps {
            let sat = format!("{at}.{name}");
            for next in successors(step) {
                if !automation.steps.contains_key(next) {
                    problems.push(format!("{sat}: unknown step {next}"));
                }
            }
            if let Some(retry) = &step.retry {
                if retry.attempts == 0 || retry.attempts > limits::MAX_STEP_ATTEMPTS {
                    problems.push(format!("{sat}: retry attempts outside 1 to 10"));
                }
                let delays = [retry.first_delay, retry.max_delay];
                if delays
                    .iter()
                    .flatten()
                    .any(|d| d.0 > limits::MAX_STEP_DELAY_MS)
                {
                    problems.push(format!("{sat}: a retry delay is longer than one day"));
                }
                if let (Some(first), Some(max)) = (retry.first_delay, retry.max_delay) {
                    if first > max {
                        problems.push(format!(
                            "{sat}: the first retry delay is longer than the maximum delay"
                        ));
                    }
                }
            }
            if let Some(timeout) = step.timeout {
                let waits = matches!(
                    step.action,
                    AutomationStep::WaitFor { .. }
                        | AutomationStep::WaitUntil { .. }
                        | AutomationStep::RequestApproval { .. }
                );
                if waits {
                    problems.push(format!(
                        "{sat}: a wait or an approval has no attempt timeout"
                    ));
                } else if timeout.0 == 0 || timeout.0 > limits::MAX_STEP_TIMEOUT_MS {
                    problems.push(format!("{sat}: a timeout is from 1 ms to 15 minutes"));
                }
            }
            action_problems(schema, &sat, &step.action, &env, problems);
            invoke_operation_problems(schema, automation, &sat, &step.action, &env, problems);
        }
        if has_start {
            let reached = reachable(automation);
            for name in automation.steps.keys() {
                if !reached.contains(name) {
                    problems.push(format!(
                        "{at}.{name}: cannot be reached from the start step"
                    ));
                }
            }
        }
    }
}

/// The problems of the tenant operations of a schema (D-6546 to D-6549, OPR-1549): at most sixteen
/// steps and eight postings; someone may invoke it, through roles of the schema; inputs have
/// distinct names that `param` can read and refer to object types of the schema; the subject is
/// an object type of the schema named by a reference input to it; conditions have distinct ids,
/// an error code, at most two hundred nodes, and check as truth values against the inputs and the
/// subject's fields; steps have distinct ids that are not input names, name object types and
/// fields of the schema, and their expressions check against the inputs, the ids of the create
/// steps before them and the subject's fields.
pub fn operation_problems(
    schema: &TenantSchema,
    roles: &BTreeSet<RoleId>,
    problems: &mut Vec<String>,
) {
    let none = BTreeMap::new();
    for op in schema.operations.iter().flat_map(BTreeMap::values) {
        let at = format!("operation {}", op.id);
        let all = all_steps(&op.steps);
        if all.len() > limits::MAX_OPERATION_STEPS {
            problems.push(format!(
                "{at}: {} steps is more than the maximum of {}",
                all.len(),
                limits::MAX_OPERATION_STEPS
            ));
        }
        let postings = all
            .iter()
            .filter(|s| matches!(s, TenantOperationStep::PostLedger { .. }))
            .count();
        for step in &all {
            if let TenantOperationStep::ReverseLedger { id, posting, .. } = step {
                let names_posting = all.iter().any(|other| {
                    matches!(other, TenantOperationStep::PostLedger { id: named, .. } if named == posting)
                });
                if !names_posting {
                    problems.push(format!(
                        "{at}.{id}: reverses {posting}, which is no posting step of the operation"
                    ));
                }
            }
        }
        if postings > limits::MAX_OPERATION_POSTINGS {
            problems.push(format!(
                "{at}: {postings} posting steps is more than the maximum of {}",
                limits::MAX_OPERATION_POSTINGS
            ));
        }
        if !op.invokers.signed_in && op.invokers.roles.is_empty() {
            problems.push(format!("{at}: names no invoker"));
        }
        for role in &op.invokers.roles {
            if !roles.contains(role) {
                problems.push(format!("{at}: unknown role {role}"));
            }
        }
        for input in &op.inputs {
            if !is_param_name(&input.name) {
                problems.push(format!("{at}: input {:?} is not a name", input.name));
            }
            if let Some((target, _, _)) = input.ty.ref_target() {
                if !schema.objects.contains_key(target) {
                    problems.push(format!(
                        "{at}: input {} refers to unknown object type {target}",
                        input.name
                    ));
                }
            }
        }
        for name in repeated(op.inputs.iter().map(|input| input.name.as_str())) {
            problems.push(format!("{at}: input {name} is declared twice"));
        }
        let inputs: BTreeSet<String> = op
            .inputs
            .iter()
            .filter(|input| is_param_name(&input.name))
            .map(|input| input.name.clone())
            .collect();
        let fields = match &op.subject {
            None => &none,
            Some(subject) => {
                let (fields, exists) = object_fields(schema, &subject.object, &none);
                if !exists {
                    problems.push(format!(
                        "{at}: unknown subject object type {}",
                        subject.object
                    ));
                }
                let refers = op.inputs.iter().any(|input| {
                    input.name == subject.input
                        && matches!(&input.ty, FieldType::Ref { target, .. } if *target == subject.object)
                });
                if !refers {
                    problems.push(format!(
                        "{at}: subject input {} is not a reference to {}",
                        subject.input, subject.object
                    ));
                }
                fields
            }
        };
        for condition in &op.conditions {
            let what = format!("condition {}", condition.id);
            if condition.error.trim().is_empty() {
                problems.push(format!("{at}: {what} names no error code"));
            }
            let nodes = expr_nodes(&condition.expr);
            if nodes > limits::MAX_CONDITION_NODES {
                problems.push(format!(
                    "{at}: {what} has {nodes} nodes, more than the maximum of {}",
                    limits::MAX_CONDITION_NODES
                ));
            }
            let env = TypeEnv {
                fields,
                params: &inputs,
                roles,
                objects: &schema.objects,
            };
            check_expr(problems, &at, &what, &condition.expr, &env, true);
        }
        for id in repeated(op.conditions.iter().map(|c| c.id.as_str())) {
            problems.push(format!("{at}: condition {id} is declared twice"));
        }
        let mut params = inputs.clone();
        let mut finds = BTreeSet::new();
        let check = StepCheck {
            schema,
            roles,
            fields,
            at: &at,
            inputs: &inputs,
        };
        steps_problems(check, &op.steps, &mut params, &mut finds, problems);
        for id in repeated(all_steps(&op.steps).into_iter().map(step_id)) {
            problems.push(format!("{at}: step {id} is declared twice"));
        }
        for capability in operation_powers(op).1 {
            if let Some(problem) = capability_reference_problem(schema, &capability) {
                problems.push(format!("{at}: {problem}"));
            }
        }
    }
}

/// What the steps of one tenant operation are checked against: the schema, the roles, the fields
/// of the subject, the name of the operation for the messages, and its inputs.
#[derive(Clone, Copy)]
struct StepCheck<'a> {
    schema: &'a TenantSchema,
    roles: &'a BTreeSet<RoleId>,
    fields: &'a BTreeMap<FieldId, FieldDef>,
    at: &'a str,
    inputs: &'a BTreeSet<String>,
}

/// The problems of the steps of a list, in order. Each step reads the inputs and the steps before
/// it. The steps a branch holds read what the list read before the branch; a binding made inside a
/// branch is not read after it. `finds` holds the find-record steps read so far.
fn steps_problems(
    check: StepCheck<'_>,
    steps: &[TenantOperationStep],
    params: &mut BTreeSet<String>,
    finds: &mut BTreeSet<String>,
    problems: &mut Vec<String>,
) {
    let StepCheck {
        schema,
        roles,
        fields,
        at,
        inputs,
    } = check;
    for step in steps {
        let id = step_id(step);
        let sat = format!("{at}.{id}");
        if !is_param_name(id) {
            problems.push(format!("{at}: step {id:?} is not a name"));
        }
        if inputs.contains(id) {
            problems.push(format!("{at}: step {id} has the name of an input"));
        }
        let env = TypeEnv {
            fields,
            params,
            roles,
            objects: &schema.objects,
        };
        match step {
            TenantOperationStep::CreateRecord {
                object,
                fields: written,
                ..
            }
            | TenantOperationStep::UpdateRecord {
                object,
                fields: written,
                ..
            } => written_problems(schema, &sat, object, written, &env, problems),
            TenantOperationStep::DeleteRecord { object, .. } => {
                written_problems(schema, &sat, object, &BTreeMap::new(), &env, problems)
            }
            TenantOperationStep::PostLedger { .. }
            | TenantOperationStep::ReverseLedger { .. }
            | TenantOperationStep::FindRecord { .. }
            | TenantOperationStep::Branch { .. }
            | TenantOperationStep::WriteSetting { .. } => {}
        }
        match step {
            TenantOperationStep::UpdateRecord { record, .. }
            | TenantOperationStep::DeleteRecord { record, .. } => {
                check_expr(problems, &sat, "record", record, &env, false)
            }
            TenantOperationStep::PostLedger {
                owner,
                scope,
                amount,
                unit,
                reference,
                ..
            } => {
                report(ledger_unit_reference(schema, unit), &sat, problems);
                if let TenantOperationPostingOwner::Principal { principal } = owner {
                    check_expr(problems, &sat, "owner", principal, &env, false);
                }
                if let Some(scope) = scope {
                    check_expr(problems, &sat, "scope", scope, &env, false);
                }
                check_expr(problems, &sat, "amount", amount, &env, false);
                if let Some(reference) = reference {
                    check_expr(problems, &sat, "reference", reference, &env, false);
                }
            }
            TenantOperationStep::ReverseLedger {
                reference, reason, ..
            } => {
                check_expr(problems, &sat, "reference", reference, &env, false);
                if reason.len() > limits::MAX_LEDGER_REASON_BYTES {
                    problems.push(format!(
                        "{sat}: a reason is at most {} bytes",
                        limits::MAX_LEDGER_REASON_BYTES
                    ));
                }
            }
            TenantOperationStep::FindRecord {
                object,
                predicate,
                error,
                ..
            } => find_problems(
                schema, &sat, object, predicate, error, params, roles, problems,
            ),
            TenantOperationStep::Branch {
                test,
                matched,
                unmatched,
                ..
            } => {
                match test {
                    BranchTest::Found { binding } => {
                        if !finds.contains(binding) {
                            problems.push(format!(
                                "{sat}: tests {binding}, which is not a find-record step before it"
                            ));
                        }
                    }
                    BranchTest::Condition { expr } => {
                        check_expr(problems, &sat, "condition", expr, &env, true)
                    }
                }
                for nested in matched.iter().chain(unmatched.iter()) {
                    if matches!(
                        nested,
                        TenantOperationStep::FindRecord { .. } | TenantOperationStep::Branch { .. }
                    ) {
                        problems.push(format!(
                            "{sat}: step {} is a find or a branch, which a branch does not hold",
                            step_id(nested)
                        ));
                    }
                }
                for side in [matched, unmatched] {
                    let mut inner = params.clone();
                    let mut inner_finds = finds.clone();
                    steps_problems(check, side, &mut inner, &mut inner_finds, problems);
                }
            }
            TenantOperationStep::WriteSetting {
                setting,
                level,
                value,
                ..
            } => {
                check_expr(problems, &sat, "value", value, &env, false);
                setting_write_problems(schema, &sat, setting, *level, problems);
            }
            TenantOperationStep::CreateRecord { .. } => {}
        }
        match step {
            TenantOperationStep::CreateRecord { .. } => {
                params.insert(id.to_string());
            }
            TenantOperationStep::FindRecord { .. } => {
                params.insert(id.to_string());
                finds.insert(id.to_string());
            }
            _ => {}
        }
    }
}

/// The problems of a write-setting step (WG-5606): the setting is one the tenant defines, and the level
/// the step writes at is one its definition allows, as the settings route reads the definition.
fn setting_write_problems(
    schema: &TenantSchema,
    sat: &str,
    setting: &SettingId,
    level: OperationSettingLevel,
    problems: &mut Vec<String>,
) {
    let level = match level {
        OperationSettingLevel::Person => SettingLevel::Person,
        OperationSettingLevel::Tenant => SettingLevel::Tenant,
    };
    match settings::definition(schema, setting) {
        None => problems.push(format!(
            "{sat}: writes {setting}, which the tenant does not define"
        )),
        Some(def) if !settings::is_open(&def, level) => problems.push(format!(
            "{sat}: writes {setting} at the {} level, which its definition does not allow",
            level.as_str()
        )),
        Some(_) => {}
    }
}

/// The problems of a find-record step: an object type that exists, an error code for more than one
/// match, and a predicate that checks as a truth value over the candidate's fields (`record`).
#[allow(clippy::too_many_arguments)]
fn find_problems(
    schema: &TenantSchema,
    at: &str,
    object: &ObjectTypeId,
    predicate: &Expr,
    error: &str,
    params: &BTreeSet<String>,
    roles: &BTreeSet<RoleId>,
    problems: &mut Vec<String>,
) {
    let Some(def) = schema.objects.get(object) else {
        problems.push(format!("{at}: unknown object type {object}"));
        return;
    };
    if error.trim().is_empty() {
        problems.push(format!("{at}: names no error code for more than one match"));
    }
    let nodes = expr_nodes(predicate);
    if nodes > limits::MAX_CONDITION_NODES {
        problems.push(format!(
            "{at}: predicate has {nodes} nodes, more than the maximum of {}",
            limits::MAX_CONDITION_NODES
        ));
    }
    let env = TypeEnv {
        fields: &def.fields,
        params,
        roles,
        objects: &schema.objects,
    };
    check_expr(problems, at, "predicate", predicate, &env, true);
}

/// The id of a step of a tenant operation.
pub fn step_id(step: &TenantOperationStep) -> &str {
    match step {
        TenantOperationStep::CreateRecord { id, .. }
        | TenantOperationStep::UpdateRecord { id, .. }
        | TenantOperationStep::DeleteRecord { id, .. }
        | TenantOperationStep::PostLedger { id, .. }
        | TenantOperationStep::ReverseLedger { id, .. }
        | TenantOperationStep::FindRecord { id, .. }
        | TenantOperationStep::Branch { id, .. }
        | TenantOperationStep::WriteSetting { id, .. } => id,
    }
}

/// The names that occur more than once, each once, in the order of their first occurrence.
fn repeated<'a>(names: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let all: Vec<&str> = names.collect();
    let mut out: Vec<&str> = Vec::new();
    for name in &all {
        if !out.contains(name) && all.iter().filter(|other| *other == name).count() > 1 {
            out.push(name);
        }
    }
    out
}

/// The problems of the fields a record step writes (of an automation or a tenant operation): the
/// object type exists, and each field is one of its fields whose expression checks.
fn written_problems(
    schema: &TenantSchema,
    at: &str,
    object: &ObjectTypeId,
    fields: &BTreeMap<FieldId, Expr>,
    env: &TypeEnv<'_>,
    problems: &mut Vec<String>,
) {
    match schema.objects.get(object) {
        None => problems.push(format!("{at}: unknown object type {object}")),
        Some(def) => {
            for (field, value) in fields {
                if !def.fields.contains_key(field) {
                    problems.push(format!("{at}: unknown field {field}"));
                }
                check_expr(problems, at, field.as_str(), value, env, false);
            }
        }
    }
}
