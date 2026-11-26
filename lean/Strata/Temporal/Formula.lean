/-
Temporal formulas over behaviours, embedded in Lean 4 without Mathlib.

A behaviour is an infinite sequence of states, `Nat → σ`. A temporal formula is a predicate on
behaviours, `Temporal σ := Behaviour σ → Prop`. A state predicate is lifted to the first state of
the behaviour, an action predicate relates the first two states. `always`, `eventually` and `next`
look at the behaviour from a later position, given by `suffix`.

Every connective is an `abbrev`, so a formula unfolds to its meaning wherever Lean needs it, and
the rules in the other files are stated and proved directly on behaviours.
-/

namespace Strata.Temporal

/-- An infinite sequence of states. -/
abbrev Behaviour (σ : Type) : Type := Nat → σ

/-- A temporal formula: a predicate on behaviours. -/
abbrev Temporal (σ : Type) : Type := Behaviour σ → Prop

/-- The behaviour from position `n` on: `suffix n b` is `b n, b (n+1), ...`. -/
def suffix {σ : Type} (n : Nat) (b : Behaviour σ) : Behaviour σ := fun k => b (n + k)

/-- `F` entails `G`: every behaviour satisfying `F` satisfies `G`. -/
abbrev Entails {σ : Type} (F G : Temporal σ) : Prop := ∀ b, F b → G b

/-- `F` holds on every behaviour. -/
abbrev Valid {σ : Type} (F : Temporal σ) : Prop := ∀ b, F b

section Connectives

variable {σ : Type}

/-- A state predicate, evaluated in the first state. -/
abbrev lift (P : σ → Prop) : Temporal σ := fun b => P (b 0)

/-- An action, evaluated on the first two states. -/
abbrev act (A : σ → σ → Prop) : Temporal σ := fun b => A (b 0) (b 1)

abbrev truth : Temporal σ := fun _ => True

abbrev falsity : Temporal σ := fun _ => False

abbrev neg (F : Temporal σ) : Temporal σ := fun b => ¬ F b

abbrev conj (F G : Temporal σ) : Temporal σ := fun b => F b ∧ G b

abbrev disj (F G : Temporal σ) : Temporal σ := fun b => F b ∨ G b

abbrev imp (F G : Temporal σ) : Temporal σ := fun b => F b → G b

/-- `□F`: `F` holds at every position. -/
abbrev always (F : Temporal σ) : Temporal σ := fun b => ∀ n, F (suffix n b)

/-- `◇F`: `F` holds at some position. -/
abbrev eventually (F : Temporal σ) : Temporal σ := fun b => ∃ n, F (suffix n b)

/-- `○F`: `F` holds at the next position. -/
abbrev next (F : Temporal σ) : Temporal σ := fun b => F (suffix 1 b)

/-- `F ~> G`: whenever `F` holds, `G` holds now or later. -/
abbrev leadsTo (F G : Temporal σ) : Temporal σ := always (imp F (eventually G))

end Connectives

section Lemmas

variable {σ : Type}

theorem suffix_zero (b : Behaviour σ) : suffix 0 b = b := by
  funext k
  simp [suffix]

theorem suffix_suffix (m n : Nat) (b : Behaviour σ) :
    suffix m (suffix n b) = suffix (n + m) b := by
  funext k
  simp only [suffix, Nat.add_assoc]

theorem lift_suffix (P : σ → Prop) (n : Nat) (b : Behaviour σ) :
    lift P (suffix n b) ↔ P (b n) := by
  simp [lift, suffix]

theorem act_suffix (A : σ → σ → Prop) (n : Nat) (b : Behaviour σ) :
    act A (suffix n b) ↔ A (b n) (b (n + 1)) := by
  simp [act, suffix]

theorem eventually_of {F : Temporal σ} {b : Behaviour σ} (h : F b) : eventually F b := by
  refine ⟨0, ?_⟩
  rw [suffix_zero]
  exact h

/-- Box implies the formula now: `□P → P`. -/
theorem always_elim (F : Temporal σ) : Entails (always F) F := by
  intro b h
  have h0 : F (suffix 0 b) := h 0
  rw [suffix_zero] at h0
  exact h0

/-- Box implies box of box: `□P → □□P`. -/
theorem always_always (F : Temporal σ) : Entails (always F) (always (always F)) := by
  intro b h n m
  rw [suffix_suffix]
  exact h (n + m)

/-- Box is monotone: `(P → Q) → (□P → □Q)`, for entailment. -/
theorem always_mono {F G : Temporal σ} (h : Entails F G) : Entails (always F) (always G) := by
  intro b ha n
  exact h _ (ha n)

/-- `□(P → Q) → (□P → □Q)`. -/
theorem always_imp (F G : Temporal σ) :
    Entails (always (imp F G)) (imp (always F) (always G)) := by
  intro b h ha n
  exact h n (ha n)

/-- Box distributes over conjunction. -/
theorem always_conj (F G : Temporal σ) (b : Behaviour σ) :
    always (conj F G) b ↔ always F b ∧ always G b :=
  ⟨fun h => ⟨fun n => (h n).1, fun n => (h n).2⟩, fun h n => ⟨h.1 n, h.2 n⟩⟩

/-- Eventually distributes over disjunction. -/
theorem eventually_disj (F G : Temporal σ) (b : Behaviour σ) :
    eventually (disj F G) b ↔ eventually F b ∨ eventually G b := by
  constructor
  · rintro ⟨n, h | h⟩
    · exact Or.inl ⟨n, h⟩
    · exact Or.inr ⟨n, h⟩
  · rintro (⟨n, h⟩ | ⟨n, h⟩)
    · exact ⟨n, Or.inl h⟩
    · exact ⟨n, Or.inr h⟩

/-- Eventually of eventually implies eventually: `◇◇P → ◇P`. -/
theorem eventually_eventually (F : Temporal σ) (b : Behaviour σ) :
    eventually (eventually F) b → eventually F b := by
  rintro ⟨n, m, h⟩
  rw [suffix_suffix] at h
  exact ⟨n + m, h⟩

/-- Duality: `¬□P ↔ ◇¬P`. -/
theorem not_always_iff (F : Temporal σ) (b : Behaviour σ) :
    ¬ always F b ↔ eventually (neg F) b := by
  constructor
  · intro h
    refine Classical.byContradiction fun hn => h fun n => ?_
    exact Classical.byContradiction fun hF => hn ⟨n, hF⟩
  · rintro ⟨n, hF⟩ h
    exact hF (h n)

/-- Duality: `¬◇P ↔ □¬P`. -/
theorem not_eventually_iff (F : Temporal σ) (b : Behaviour σ) :
    ¬ eventually F b ↔ always (neg F) b :=
  ⟨fun h n hF => h ⟨n, hF⟩, fun h ⟨n, hF⟩ => h n hF⟩

end Lemmas

end Strata.Temporal
