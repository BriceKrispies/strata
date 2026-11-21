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
#print axioms Strata.missing_record_hidden_on_update
#print axioms Strata.missing_record_hidden_on_delete
#print axioms Strata.step_ok_authorized
#print axioms Strata.step_isolated
#print axioms Strata.step_local
#print axioms Strata.step_deterministic
#print axioms Strata.lax_change_can_break_wf

#print axioms Strata.Evolution.closure
#print axioms Strata.Evolution.locality
#print axioms Strata.Evolution.compatibility
#print axioms Strata.Evolution.breaking_change_can_change_value
#print axioms Strata.Evolution.breaking_change_can_invalidate
#print axioms Strata.Evolution.rewrite_correct
#print axioms Strata.Evolution.rewrite_correct_nb
#print axioms Strata.Evolution.project_migrate
#print axioms Strata.Evolution.left_inverse_on_footprint
#print axioms Strata.Evolution.pinned_eval
#print axioms Strata.Evolution.pinned_eval_live
#print axioms Strata.Evolution.removeField_irreversible
#print axioms Strata.Evolution.retargetRef_irreversible
#print axioms Strata.Evolution.removeObject_irreversible
#print axioms Strata.Evolution.narrow_nat_irreversible
#print axioms Strata.Evolution.commutation
#print axioms Strata.Evolution.rewrite_compose
#print axioms Strata.Evolution.rewrite_chain
#print axioms Strata.Evolution.pinned_breaking_change_accepted

#print axioms Strata.WorkingSet.decideTenant_faithful
#print axioms Strata.WorkingSet.decideOn_fetch
#print axioms Strata.WorkingSet.untouched_unchanged
#print axioms Strata.WorkingSet.hit_current
#print axioms Strata.WorkingSet.serve_eq_fetch
#print axioms Strata.WorkingSet.sound_of_advance
#print axioms Strata.WorkingSet.read_after_commit
#print axioms Strata.WorkingSet.commit_sound
#print axioms Strata.WorkingSet.stepWS_sound
#print axioms Strata.WorkingSet.stepWS_within
#print axioms Strata.WorkingSet.stepWS_transparent
#print axioms Strata.WorkingSet.runWS_transparent
#print axioms Strata.WorkingSet.runWS_within
#print axioms Strata.WorkingSet.runWS_sound
#print axioms Strata.WorkingSet.admit_keeps_pinned
#print axioms Strata.WorkingSet.evict_keeps_pinned
#print axioms Strata.WorkingSet.resize_keeps_pinned
#print axioms Strata.WorkingSet.pin_resident
#print axioms Strata.WorkingSet.commit_keeps_other_tenants
#print axioms Strata.WorkingSet.commit_keeps_untouched
#print axioms Strata.WorkingSet.resize_budget_exact
#print axioms Strata.WorkingSet.machine_within
