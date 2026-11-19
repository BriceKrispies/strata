//! What must hold before anything is emitted: the specification's generation target, and every
//! backend's own check. specc refuses with all the diagnostics together.

use crate::backend::BACKENDS;
use crate::diagnostic::{self, Diagnostic};
use crate::ir::Spec;

/// Code of a `system.generation.target` other than `rust`.
pub const TARGET: &str = "specc.generation-target";

/// Every reason to refuse the specification, in a stable order: the system first, then each
/// backend's diagnostics in backend order and declaration order.
pub fn check(spec: &Spec) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if let Some(target) = &spec.system.generation_target {
        if target != "rust" {
            out.push(Diagnostic::new(
                TARGET,
                &spec.system.id,
                format!(
                    "The specification's generation target is \"{target}\"; specc generates rust."
                ),
            ));
        }
    }
    for backend in BACKENDS {
        out.extend(backend.check(spec));
    }
    out.extend(crate::language::check(spec));
    diagnostic::dedup(out)
}
