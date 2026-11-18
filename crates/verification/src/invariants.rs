//! State invariants, evaluated on a complete tenant state.
//!
//! These are the properties an accepted transition is supposed to preserve. They are checked
//! after every step of the kernel's state-machine tests and at the end of every simulation run.
//! Each one is evaluated by the kernel's own validation code, not by a second implementation.

use crate::evidence::Class;
use serde::{Deserialize, Serialize};
use strata_kernel::rules::{validate_record, Lookup};
use strata_kernel::{owner_role, Coverage, ErrorKind, Principal, TenantState, World};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvariantResult {
    pub id: String,
    pub statement: String,
    pub holds: bool,
    /// Up to twenty counterexamples, in a deterministic order.
    pub violations: Vec<String>,
    /// How many counterexamples there are in all.
    pub violation_count: usize,
    pub class: Class,
}

fn result(id: &str, statement: &str, mut violations: Vec<String>) -> InvariantResult {
    let violation_count = violations.len();
    violations.truncate(20);
    InvariantResult {
        id: id.to_string(),
        statement: statement.to_string(),
        holds: violation_count == 0,
        violations,
        violation_count,
        class: Class::Reproduced,
    }
}

/// Every invariant of one tenant's state.
pub fn check_tenant(tenant: &TenantState) -> Vec<InvariantResult> {
    let lookup = Lookup {
        tenant,
        coverage: &Coverage::Complete,
    };
    let schema = &tenant.schema;
    let (mut shape, mut unique, mut refs, mut workflow, mut placement) =
        (vec![], vec![], vec![], vec![], vec![]);

    for (object, records) in &tenant.records {
        let Some(def) = schema.objects.get(object) else {
            placement.push(format!(
                "{} record(s) of undefined object type {object}",
                records.len()
            ));
            continue;
        };
        for (id, record) in records {
            let at = format!("{object}/{id}");
            if record.id != *id || record.object != *object {
                placement.push(format!("{at} is stored under the wrong key"));
            }
            if id.0 >= tenant.next_record {
                placement.push(format!(
                    "{at} has an id the counter has not reached ({})",
                    tenant.next_record
                ));
            }
            // Re-validate as the kernel would on a write, at the time the record was last written.
            let outcome = validate_record(
                lookup,
                schema,
                def,
                record,
                None,
                &Principal::System,
                record.updated_at,
            );
            let failures = match outcome {
                Ok(outcome) => outcome.failed,
                Err(error) => {
                    shape.push(format!("{at}: {error}"));
                    continue;
                }
            };
            for failure in failures {
                let line = format!("{at}: {} ({})", failure.check.rule, failure.check.detail);
                match failure.kind {
                    ErrorKind::UniqueViolated => unique.push(line),
                    ErrorKind::DanglingReference => refs.push(line),
                    ErrorKind::WorkflowViolation => workflow.push(line),
                    _ => shape.push(line),
                }
            }
        }
    }

    let mut membership = Vec::new();
    if !tenant
        .members
        .values()
        .any(|roles| roles.contains(&owner_role()))
    {
        membership.push("the tenant has no owner".to_string());
    }
    for (principal, roles) in &tenant.members {
        if roles.is_empty() {
            membership.push(format!("{principal} is a member with no role"));
        }
        for role in roles
            .iter()
            .filter(|role| !schema.roles.contains_key(*role))
        {
            membership.push(format!("{principal} holds undefined role {role}"));
        }
    }

    vec![
        result("schema_consistent", "The schema refers only to things it defines", schema.problems()),
        result(
            "records_conform",
            "Every record has only known fields, of the right type, with required fields present and every constraint satisfied",
            shape,
        ),
        result("unique_fields", "No two records share a value in a unique field", unique),
        result("references_resolve", "Every reference points at a record that exists", refs),
        result("workflow_states", "Every workflow field holds one of its workflow's states", workflow),
        result("records_placed", "Every record is stored under its own object type and id, below the id counter", placement),
        result("membership", "The tenant has an owner and every member holds only defined roles", membership),
    ]
}

/// Every invariant of every tenant, plus the one that spans tenants.
pub fn check_world(world: &World) -> Vec<InvariantResult> {
    let mut merged: Vec<InvariantResult> = Vec::new();
    let mut keys = Vec::new();
    for (id, tenant) in &world.tenants {
        if tenant.id != *id {
            keys.push(format!("tenant {} is stored under {id}", tenant.id));
        }
        for found in check_tenant(tenant) {
            let prefixed = found.violations.iter().map(|v| format!("{id}: {v}"));
            match merged.iter_mut().find(|m| m.id == found.id) {
                Some(existing) => {
                    existing.holds &= found.holds;
                    existing.violation_count += found.violation_count;
                    existing.violations.extend(prefixed);
                    existing.violations.truncate(20);
                }
                None => merged.push(InvariantResult {
                    violations: prefixed.collect(),
                    ..found
                }),
            }
        }
    }
    merged.push(result(
        "tenant_ownership",
        "Every tenant's state is stored under that tenant's id",
        keys,
    ));
    merged
}
