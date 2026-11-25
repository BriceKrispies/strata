/-
Fairness and the spec form `Init ∧ □[Next]_vars ∧ Fairness` for the temporal embedding.

Fairness is stated over steps, not over changes of state: an action that is taken in a step counts
as taken even if the step leaves the state unchanged. This is IronFleet's convention, and it is what
makes an always-enabled action meaningful: "if you hold the lock, pass it on; otherwise do nothing"
is a step that is always possible, and taking it is a step whether or not the lock moved.

`enabled A s` says some step of `A` leaves `s`. `weakFair A` says: if `A` is eventually enabled
forever, then `A` is taken infinitely often. `strongFair A` says: if `A` is enabled infinitely
often, then `A` is taken infinitely often.
-/

import Strata.Temporal.Formula

namespace Strata.Temporal

section Fairness

variable {σ : Type} {α : Type}

/-- An action is enabled in `s` when some step of it leaves `s`. -/
abbrev enabled (A : σ → σ → Prop) : σ → Prop := fun s => ∃ s', A s s'

/-- The stuttering box `□[N]_v`: every step is an `N` step or leaves the variables `v` unchanged. -/
abbrev stepBox (N : σ → σ → Prop) (v : σ → α) : Temporal σ :=
  always (act (fun s s' => N s s' ∨ v s' = v s))

/-- Weak fairness of `A`: `◇□Enabled A ⇒ □◇A`. -/
abbrev weakFair (A : σ → σ → Prop) : Temporal σ :=
  imp (eventually (always (lift (enabled A)))) (always (eventually (act A)))

/-- Strong fairness of `A`: `□◇Enabled A ⇒ □◇A`. -/
abbrev strongFair (A : σ → σ → Prop) : Temporal σ :=
  imp (always (eventually (lift (enabled A)))) (always (eventually (act A)))

/-- The spec form `Init ∧ □[N]_v ∧ F`, where `F` is the fairness condition. -/
abbrev spec (Init : σ → Prop) (N : σ → σ → Prop) (v : σ → α) (F : Temporal σ) : Temporal σ :=
  conj (lift Init) (conj (stepBox N v) F)

/-- Strong fairness implies weak fairness. -/
theorem strong_implies_weak (A : σ → σ → Prop) (b : Behaviour σ) (h : strongFair A b) :
    weakFair A b := by
  intro hen
  refine h ?_
  obtain ⟨j, hj⟩ := hen
  intro m
  refine ⟨j, ?_⟩
  have hm : lift (enabled A) (suffix (j + m) b) := by
    have := hj m
    rw [suffix_suffix] at this
    exact this
  rw [suffix_suffix, Nat.add_comm]
  exact hm

end Fairness

end Strata.Temporal
