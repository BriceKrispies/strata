/-
The agent build loop's guards (docs/design-system.md, "The agent build loop").

Three small models, each of one guard:

- **The build lease and its fencing token.** Each acquisition (or theft) of a product's lease raises the
  token; a step carries the token it was issued with and is applied only when that is the current one.
  Proved: after an acquisition, no step carrying an earlier token is ever applied
  (`no_stale_step_after_acquire`).
- **The held upgrade.** An upgrade that needs approval is held with the diff and the classification
  shown to the owner. Approval recomputes the diff against the schema as it is then and applies it only
  when it equals the held one. Proved: what is applied is exactly the diff that was approved, and its
  classification is the one shown (`approve_applies_held`, `approve_class_matches`); a moved schema
  refuses the approval (`approve_refused_when_moved`).
- **Dormant entities.** Every entry point asks one gate, which admits a dormant entity for builders only.
  Proved: through any list of entry points routed through the gate, a non-builder reaches no dormant
  entity (`dormant_unreachable`).

These model the rules, not the Rust: that every entry point of the kernel and the gateway routes
through the gate is the implementation's obligation, held by a test that enumerates the contracts.
No `sorry`, no added axioms.
-/

namespace Strata.UI.Loop

/-- Lease events: an acquisition, or a step carrying a token. -/
inductive Ev where
  | acquire
  | step (token : Nat)

/-- The lease state: the current token and the tokens of the steps applied, newest first. -/
structure Lease where
  cur : Nat
  applied : List Nat

/-- One event. -/
def stepL (s : Lease) : Ev → Lease
  | .acquire => { s with cur := s.cur + 1 }
  | .step t => if t = s.cur then { s with applied := t :: s.applied } else s

/-- A run of events. -/
def runL (s : Lease) : List Ev → Lease
  | [] => s
  | e :: es => runL (stepL s e) es

theorem runL_cur_ge (s : Lease) : ∀ es, s.cur ≤ (runL s es).cur
  | [] => Nat.le_refl _
  | e :: es => by
    have h : s.cur ≤ (stepL s e).cur := by
      cases e with
      | acquire => simp [stepL]
      | step t => simp only [stepL]; split <;> simp
    exact Nat.le_trans h (runL_cur_ge (stepL s e) es)

theorem runL_new_tokens_ge (s : Lease) : ∀ es t, t ∈ (runL s es).applied → t ∉ s.applied →
    s.cur ≤ t
  | [], t, h, hn => absurd h hn
  | e :: es, t, h, hn => by
    simp only [runL] at h
    by_cases hm : t ∈ (stepL s e).applied
    · cases e with
      | acquire => simp [stepL] at hm; exact absurd hm hn
      | step u =>
        simp only [stepL] at hm
        split at hm
        · rename_i hu
          simp at hm
          rcases hm with rfl | hm
          · omega
          · exact absurd hm hn
        · exact absurd hm hn
    · have := runL_new_tokens_ge (stepL s e) es t h hm
      have hc : s.cur ≤ (stepL s e).cur := by
        cases e with
        | acquire => simp [stepL]
        | step u => simp only [stepL]; split <;> simp
      omega

/-- After an acquisition, no step carrying an earlier token is ever applied.
    Verifies: DS-015 -/
theorem no_stale_step_after_acquire (s : Lease) (es : List Ev) (t : Nat)
    (h : t ∈ (runL s (.acquire :: es)).applied) (hn : t ∉ s.applied) : s.cur < t := by
  simp only [runL] at h
  have := runL_new_tokens_ge (stepL s .acquire) es t h (by simpa [stepL] using hn)
  simp [stepL] at this
  omega

/-- A held upgrade: the bundle, and the diff and classification shown when it was held. -/
structure Held (Bd D C : Type) where
  bundle : Bd
  diff : D
  cls : C

/-- Approval: recompute the diff against the schema as it is now, and apply it only when it equals the
    held one. -/
def approve {S Bd D C : Type} [DecidableEq D] (plan : S → Bd → D) (apply : S → D → S)
    (s : S) (h : Held Bd D C) : Option S :=
  if plan s h.bundle = h.diff then some (apply s h.diff) else none

/-- What approval applies is exactly the held diff, recomputed equal on the schema at approval.
    Verifies: DS-016 -/
theorem approve_applies_held {S Bd D C : Type} [DecidableEq D] (plan : S → Bd → D)
    (apply : S → D → S) (s s' : S) (h : Held Bd D C) (ha : approve plan apply s h = some s') :
    s' = apply s h.diff ∧ plan s h.bundle = h.diff := by
  unfold approve at ha
  split at ha
  · rename_i heq; cases ha; exact ⟨rfl, heq⟩
  · cases ha

/-- The classification of what is applied is the one the owner was shown.
    Verifies: DS-016 -/
theorem approve_class_matches {S Bd D C : Type} [DecidableEq D] (plan : S → Bd → D)
    (apply : S → D → S) (classify : D → C) (s s' : S) (h : Held Bd D C)
    (hc : h.cls = classify h.diff) (ha : approve plan apply s h = some s') :
    classify (plan s h.bundle) = h.cls := by
  rw [(approve_applies_held plan apply s s' h ha).2, hc]

/-- A schema that moved so that the diff differs refuses the approval.
    Verifies: DS-016 -/
theorem approve_refused_when_moved {S Bd D C : Type} [DecidableEq D] (plan : S → Bd → D)
    (apply : S → D → S) (s : S) (h : Held Bd D C) (hm : plan s h.bundle ≠ h.diff) :
    approve plan apply s h = none := by
  unfold approve
  rw [if_neg hm]

/-- The one gate every entry point asks: a dormant entity admits builders only. -/
def gate (dormant builder : Bool) (inner : Bool) : Bool := (!dormant || builder) && inner

/-- An entry point: the entity it reaches, and its own decision. -/
structure Entry (E : Type) where
  entity : E
  decide : Bool

/-- What a caller reaches through a list of entry points routed through the gate. -/
def reached {E : Type} (dormant : E → Bool) (builder : Bool) (es : List (Entry E)) : List E :=
  (es.filter (fun e => gate (dormant e.entity) builder e.decide)).map (·.entity)

/-- A non-builder reaches no dormant entity, through any entry point routed through the gate.
    Verifies: DS-017 -/
theorem dormant_unreachable {E : Type} (dormant : E → Bool) (es : List (Entry E)) (x : E)
    (hx : x ∈ reached dormant false es) : dormant x = false := by
  simp only [reached, List.mem_map, List.mem_filter, gate, Bool.or_false, Bool.and_eq_true,
    Bool.not_eq_true'] at hx
  obtain ⟨e, ⟨_, hd, _⟩, rfl⟩ := hx
  exact hd

end Strata.UI.Loop
