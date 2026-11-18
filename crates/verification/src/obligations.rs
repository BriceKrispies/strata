//! The proof-obligation table: each property Strata claims, the Lean theorems that prove it for
//! the abstract model, the Rust tests that exercise it on the real kernel, and what is assumed.

use crate::evidence::Class;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Obligation {
    pub id: String,
    /// The property in plain words.
    pub property: String,
    /// What exactly the Lean theorems say, in one sentence.
    pub statement: String,
    pub theorems: Vec<String>,
    /// Rust tests that check the corresponding behaviour of the production kernel.
    pub rust_tests: Vec<String>,
    /// What the proof does not cover.
    pub limits: String,
    pub class: Class,
}

fn obligation(id: &str, property: &str, statement: &str, theorems: &[&str], rust_tests: &[&str], limits: &str) -> Obligation {
    Obligation {
        id: id.to_string(),
        property: property.to_string(),
        statement: statement.to_string(),
        theorems: theorems.iter().map(|t| format!("Strata.{t}")).collect(),
        rust_tests: rust_tests.iter().map(|t| t.to_string()).collect(),
        limits: limits.to_string(),
        class: Class::Proven,
    }
}

/// Every obligation, in the order of the four required properties.
pub fn all() -> Vec<Obligation> {
    vec![
        obligation(
            "invariant_preservation",
            "Accepted transitions preserve the state invariant",
            "If every record of every tenant conforms to its schema and has an id below the counter, then after any accepted strict step the same is true.",
            &["step_preserves_wf", "stepTenant_preserves_wf", "lax_change_can_break_wf"],
            &[
                "strata-verification::state_machine::every_accepted_state_satisfies_every_invariant",
                "strata-kernel::behaviour::a_strict_migration_is_rejected_when_existing_records_would_not_conform",
            ],
            "The model's invariant is conformance to field types and required fields. Constraints, uniqueness, references and workflow states are part of the Rust invariant and are covered only by the Rust property test. A grandfathered schema change is excluded on purpose, and `lax_change_can_break_wf` shows the exclusion is necessary.",
        ),
        obligation(
            "authorization",
            "An unauthorized principal cannot mutate a protected record",
            "A step is accepted only if the acting principal is authorized for it; an update or delete by a principal who may not edit the record is refused with `unauthorized`.",
            &["unauthorized_cannot_mutate", "unauthorized_cannot_update", "unauthorized_cannot_delete", "step_ok_authorized"],
            &[
                "strata-kernel::properties::outsiders_cannot_mutate",
                "strata-kernel::behaviour::writers_edit_only_their_own_notes",
                "strata-substrate::publication_template::an_author_cannot_touch_another_authors_post",
            ],
            "The model's authorization is owner, editors, and writers on their own records. The kernel's policies are data with arbitrary conditions; the theorem covers the shape, not every policy a tenant can write.",
        ),
        obligation(
            "tenant_isolation",
            "A tenant-scoped transition cannot modify another tenant's data",
            "An accepted step addressed to one tenant leaves every other tenant unchanged, and its result for the addressed tenant does not depend on any other tenant's state.",
            &["step_isolated", "step_local"],
            &[
                "strata-kernel::properties::a_step_never_touches_another_tenant",
                "strata-storage-api::conformance::tenants_are_isolated",
                "strata-backend::service::a_session_of_one_tenant_is_nobody_on_another",
            ],
            "Proved for the kernel-level world. Isolation in storage, sessions and the WebSocket path is established by tests, not by proof.",
        ),
        obligation(
            "determinism",
            "The same state, intent and context produce the same result",
            "`step` is a function: two evaluations on equal arguments are equal.",
            &["step_deterministic"],
            &[
                "strata-kernel::properties::transitions_are_deterministic",
                "strata-simulation::simulation::the_same_scenario_gives_the_same_trace",
            ],
            "In Lean this is true of any function and carries no weight of its own. The substance is on the Rust side: the kernel has no clock, randomness, I/O or unordered iteration, which the purity scan and the trace-hash tests check.",
        ),
    ]
}

/// The assumptions every proof here rests on.
pub fn assumptions() -> Vec<&'static str> {
    vec![
        "The object of every theorem is the abstract model in lean/Strata/Model.lean, not the Rust source.",
        "The model has one object type per tenant, three scalar value kinds, no constraints, workflows, references, uniqueness, queries or events.",
        "The model's update replaces a record's fields; the kernel's update is a patch. They coincide when the patch names every field, which is how the fixtures use it.",
        "Correspondence between model and kernel is supported by running both on the same generated scenarios, and by review. It is evidence, not proof.",
        "Lean's own kernel and its three standard axioms (propext, Classical.choice, Quot.sound) are trusted.",
    ]
}
