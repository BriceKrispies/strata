/-
Proof rules for leads-to, invariance and weak fairness: TLA's WF1 and its always-enabled form.

Every rule is about one behaviour `b`, so a premise may use the behaviour's own fairness condition.
Premises about states are stated for the step relation that the behaviour's steps satisfy.
-/

import Strata.Temporal.Fairness

namespace Strata.Temporal

section Rules

variable {σ : Type}

/-! ### Leads-to -/

/-- Leads-to is reflexive. -/
theorem leadsTo_refl (F : Temporal σ) (b : Behaviour σ) : leadsTo F F b := by
  intro n hF
  exact eventually_of hF

/-- Leads-to is transitive. -/
theorem leadsTo_trans {F G H : Temporal σ} (b : Behaviour σ)
    (h1 : leadsTo F G b) (h2 : leadsTo G H b) : leadsTo F H b := by
  intro n hF
  obtain ⟨j, hG⟩ := h1 n hF
  rw [suffix_suffix] at hG
  obtain ⟨k, hH⟩ := h2 (n + j) hG
  refine ⟨j + k, ?_⟩
  rw [suffix_suffix] at hH ⊢
  rw [Nat.add_assoc] at hH
  exact hH

/-- Leads-to distributes over disjunction in its source: `P ~> R` and `Q ~> R` give `P ∨ Q ~> R`. -/
theorem leadsTo_disj {F G R : Temporal σ} (b : Behaviour σ)
    (h1 : leadsTo F R b) (h2 : leadsTo G R b) : leadsTo (disj F G) R b := by
  intro n h
  rcases h with hF | hG
  · exact h1 n hF
  · exact h2 n hG

/-- Implication under box gives leads-to: `□(P → Q)` gives `P ~> Q`. -/
theorem leadsTo_of_always_imp {F G : Temporal σ} (b : Behaviour σ)
    (h : always (imp F G) b) : leadsTo F G b := by
  intro n hF
  exact eventually_of (h n hF)

/-- A leads-to property applied to a behaviour that starts in its source. -/
theorem leadsTo_apply {F G : Temporal σ} (b : Behaviour σ) (h : leadsTo F G b) (hF : F b) :
    eventually G b := by
  have h0 : F (suffix 0 b) → eventually G (suffix 0 b) := h 0
  have h1 := h0 (by rw [suffix_zero]; exact hF)
  rw [suffix_zero] at h1
  exact h1

/-- Well-founded induction for leads-to. If from `P` with measure `n` the behaviour reaches `Q`, or
reaches `P` with a smaller measure, for every `n`, then `P ~> Q`. -/
theorem leadsTo_wf {P Q : σ → Prop} (m : σ → Nat) (b : Behaviour σ)
    (h : ∀ n, leadsTo (conj (lift P) (lift (fun s => m s = n)))
        (disj (lift Q) (conj (lift P) (lift (fun s => m s < n)))) b) :
    leadsTo (lift P) (lift Q) b := by
  have key : ∀ N k, P (b k) → m (b k) < N → ∃ j, Q (b (k + j)) := by
    intro N
    induction N with
    | zero =>
      intro k _ hlt
      exact absurd hlt (Nat.not_lt_zero _)
    | succ N ih =>
      intro k hP hlt
      have hk : conj (lift P) (lift (fun s => m s = m (b k))) (suffix k b) := ⟨hP, rfl⟩
      obtain ⟨j, hj⟩ := h (m (b k)) k hk
      rcases hj with hQ | ⟨hP2, hlt2⟩
      · rw [suffix_suffix, lift_suffix] at hQ
        exact ⟨j, hQ⟩
      · rw [suffix_suffix, lift_suffix] at hP2 hlt2
        obtain ⟨j', hj'⟩ := ih (k + j) hP2 (by omega)
        exact ⟨j + j', by rw [← Nat.add_assoc]; exact hj'⟩
  intro k hP
  obtain ⟨j, hQ⟩ := key (m (b k) + 1) k ((lift_suffix _ _ _).1 hP) (by omega)
  refine ⟨j, ?_⟩
  rw [suffix_suffix, lift_suffix]
  exact hQ

/-! ### Invariance -/

/-- Invariance: if `I` holds initially, and every step from an `I` state to a state satisfying `Step`
preserves `I`, then `□I`. -/
theorem invariant_of (I Init : σ → Prop) (Step : σ → σ → Prop) (b : Behaviour σ)
    (hinit : Init (b 0)) (hI : ∀ s, Init s → I s)
    (hpres : ∀ s s', I s → Step s s' → I s')
    (hsteps : always (act Step) b) : always (lift I) b := by
  have hs : ∀ n, Step (b n) (b (n + 1)) := hsteps
  have hall : ∀ n, I (b n) := by
    intro n
    induction n with
    | zero => exact hI _ hinit
    | succ n ih => exact hpres _ _ ih (hs n)
  intro n
  exact (lift_suffix I n b).2 (hall n)

/-- Invariance for a spec `Init ∧ □[N]_v ∧ F`: `I` holds initially, and every `N` step and every
stuttering step preserves it. -/
theorem invariant_of_spec {α : Type} {Init : σ → Prop} {N : σ → σ → Prop} {v : σ → α}
    {F : Temporal σ} (I : σ → Prop) (b : Behaviour σ)
    (hI : ∀ s, Init s → I s) (hpres : ∀ s s', I s → N s s' → I s')
    (hstut : ∀ s s', I s → v s' = v s → I s')
    (h : spec Init N v F b) : always (lift I) b := by
  have hinit : Init (b 0) := h.1
  have hsteps : always (act (fun s s' => N s s' ∨ v s' = v s)) b := h.2.1
  exact invariant_of I Init (fun s s' => N s s' ∨ v s' = v s) b hinit hI
    (fun s s' hs hstep => Or.elim hstep (hpres s s' hs) (hstut s s' hs)) hsteps

/-! ### WF1 and the always-enabled form -/

/-- WF1 (Lamport). Let `P` and `Q` be state predicates. Every step from `P` that is a `Step` step
leads to `P` or `Q`; every `A` step from `P` leads to `Q`; `A` is enabled wherever `P` holds; and
`A` is weakly fair. Then `P ~> Q`. -/
theorem wf1 {P Q : σ → Prop} {A Step : σ → σ → Prop} (b : Behaviour σ)
    (hsteps : always (act Step) b)
    (h1 : ∀ s s', P s → Step s s' → P s' ∨ Q s')
    (h2 : ∀ s s', P s → Step s s' → A s s' → Q s')
    (h3 : ∀ s, P s → enabled A s)
    (hwf : weakFair A b) : leadsTo (lift P) (lift Q) b := by
  have hs : ∀ n, Step (b n) (b (n + 1)) := hsteps
  intro k hPk
  have hPk' : P (b k) := (lift_suffix _ _ _).1 hPk
  have key : ∃ j, Q (b (k + j)) := by
    refine Classical.byContradiction fun hne => ?_
    have hnoQ : ∀ j, ¬ Q (b (k + j)) := fun j hq => hne ⟨j, hq⟩
    have hPall : ∀ j, P (b (k + j)) := by
      intro j
      induction j with
      | zero => exact hPk'
      | succ j ih =>
        rcases h1 _ _ ih (hs (k + j)) with h | h
        · exact h
        · exact (hnoQ (j + 1) h).elim
    have hen : ∀ j, enabled A (b (k + j)) := fun j => h3 _ (hPall j)
    have hev : eventually (always (lift (enabled A))) b := by
      refine ⟨k, ?_⟩
      intro n
      rw [suffix_suffix]
      exact (lift_suffix _ _ _).2 (hen n)
    obtain ⟨j, hA⟩ := hwf hev k
    rw [suffix_suffix] at hA
    have hA' : A (b (k + j)) (b (k + j + 1)) := hA
    exact hnoQ (j + 1) (h2 _ _ (hPall j) (hs (k + j)) hA')
  obtain ⟨j, hQ⟩ := key
  refine ⟨j, ?_⟩
  rw [suffix_suffix]
  exact (lift_suffix _ _ _).2 hQ

/-- The always-enabled form of WF1. If `A` is enabled in every state and weakly fair, the WF1 premise
on enabledness holds everywhere, so a progress step of `A` gives a leads-to. -/
theorem always_enabled_leadsTo {P Q : σ → Prop} {A Step : σ → σ → Prop} (b : Behaviour σ)
    (hsteps : always (act Step) b)
    (h1 : ∀ s s', P s → Step s s' → P s' ∨ Q s')
    (h2 : ∀ s s', P s → Step s s' → A s s' → Q s')
    (henab : ∀ s, enabled A s) (hwf : weakFair A b) : leadsTo (lift P) (lift Q) b :=
  wf1 b hsteps h1 h2 (fun s _ => henab s) hwf

/-- An always-enabled, weakly fair action is taken infinitely often. -/
theorem always_enabled_infinitely_often {A : σ → σ → Prop} (b : Behaviour σ)
    (henab : ∀ s, enabled A s) (hwf : weakFair A b) : always (eventually (act A)) b :=
  hwf ⟨0, fun n => henab (suffix n (suffix 0 b) 0)⟩

end Rules

end Strata.Temporal
