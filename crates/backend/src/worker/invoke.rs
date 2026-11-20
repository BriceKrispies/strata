//! The step kind invoke-operation (WFL-2685, D-7682): a step that invokes a tenant operation
//! (D-6546 to D-6549) as the automation principal.
//!
//! The kernel decides the invocation like any other (`strata_kernel::operations::invoke`): the
//! principal is `automation:<workflow-id>` holding its run-as roles, so the operation's invokers
//! must name one of them or every signed-in person; the inputs are the ones the operation
//! declares, bound from the step's expressions and checked against their types; and the
//! operation's steps run with the operation's own powers and no more. The step key is the
//! invocation's idempotency key, so an attempt made again after a crash between invoking and
//! reporting finds the first result recorded under the key and changes nothing (SCH-1524).

use super::steps::{Done, Next, StepError};
use std::collections::BTreeMap;
use strata_kernel::{Expr, Intent, Output, TenantOperationId, Value};
use strata_model::types::AutomationStepResult;

/// Run the step: evaluate its inputs with `eval`, decide the invocation with `decide` under the
/// step key, and report the records the operation created, changed and deleted.
pub(super) fn run(
    operation: &TenantOperationId,
    inputs: &BTreeMap<String, Expr>,
    key: &str,
    eval: &dyn Fn(&str, &Expr) -> Result<Value, StepError>,
    decide: &dyn Fn(Intent) -> Result<Output, StepError>,
    then: &dyn Fn() -> Next,
) -> Result<Done, StepError> {
    let mut bound: BTreeMap<String, Value> = BTreeMap::new();
    for (name, expr) in inputs {
        bound.insert(name.clone(), eval(name, expr)?);
    }
    let intent = Intent::InvokeOperation {
        operation: operation.clone(),
        inputs: bound,
        idempotency_key: key.to_string(),
    };
    match decide(intent)? {
        Output::Operation {
            created,
            changed,
            deleted,
            ..
        } => {
            let refs = |ids: &[strata_kernel::RecordId]| {
                Value::List(ids.iter().map(|id| Value::Ref(*id)).collect())
            };
            Ok(Done {
                outputs: vec![
                    ("created", refs(&created)),
                    ("changed", refs(&changed)),
                    ("deleted", refs(&deleted)),
                ],
                result: AutomationStepResult::InvokeOperation {
                    created,
                    changed,
                    deleted,
                },
                next: then(),
            })
        }
        other => Err(StepError::fatal(format!("unexpected output {other:?}"))),
    }
}
