//! Whether an automation may start runs (contract activate-workflow, WFL-002, WFL-1521,
//! WFL-2684, D-7681).
//!
//! An automation has a state (spec type `automation-state`): a draft until activate-workflow
//! activates it, active while it may start runs, and suspended when its definer can no longer
//! confer a role of its run-as (WFL-1529, which the field `suspended-for` records). Only an
//! active automation starts runs: a trigger that names a draft declines with the reason
//! automation-inactive, and a person who starts a draft directly is refused.
//!
//! An automation held without a state is active: the definitions that existed before the state
//! did, and the ones a product installs, run as they always have. Activation is a definition made
//! again with the state active by the principal who activates it, so that the checks of defining
//! apply to it unchanged: the run-as roles are ones the activating principal may confer
//! (WFL-1528) and the catalogue check passes (D-6538).

use crate::types::{AutomationDef, AutomationState};

/// The state of an automation: suspended when it has lost its definer's authority, else the state
/// it was defined with, active when it names none.
pub fn state_of(def: &AutomationDef) -> AutomationState {
    if def.suspended_for.is_some() {
        return AutomationState::Suspended;
    }
    def.state.unwrap_or(AutomationState::Active)
}

/// Whether the automation may start runs.
pub fn is_active(def: &AutomationDef) -> bool {
    state_of(def) == AutomationState::Active
}

/// Why a draft or suspended automation starts no run, for a declined record or a refusal.
pub fn inactive_detail(def: &AutomationDef) -> String {
    format!(
        "automation {} is {}, not active",
        def.id,
        state_of(def).as_str()
    )
}

/// The automation as activation defines it: the same definition with the state active.
pub fn activated(def: &AutomationDef) -> AutomationDef {
    AutomationDef {
        state: Some(AutomationState::Active),
        ..def.clone()
    }
}
