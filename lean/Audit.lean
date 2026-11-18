/-
Prints, for each theorem the project claims, the axioms its proof depends on.
Run with `lake env lean Audit.lean`. A theorem proved with `sorry` would list `sorryAx`.
-/
import Strata

#print axioms Strata.step_preserves_wf
#print axioms Strata.stepTenant_preserves_wf
#print axioms Strata.unauthorized_cannot_mutate
#print axioms Strata.unauthorized_cannot_update
#print axioms Strata.unauthorized_cannot_delete
#print axioms Strata.step_ok_authorized
#print axioms Strata.step_isolated
#print axioms Strata.step_local
#print axioms Strata.step_deterministic
#print axioms Strata.lax_change_can_break_wf
