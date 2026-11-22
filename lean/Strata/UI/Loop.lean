/-
The agent build loop's guards (docs/design-system.md, "The agent build loop" and "Drafts, published,
and dormant entities").

- **The effective schema.** The tenant schema holds its active entities and, apart, a draft overlay of
  dormant ones. Every evaluation for a non-builder is given the active schema alone; there is no list
  of call sites to route through a gate, because the dormant entities are not in what those call sites
  are given. The invariant the classifier enforces is that no active entity references a dormant one.
  Proved: everything reachable by references from what a non-builder is given is active
  (`nonbuilder_reach_active`), so no dormant entity is reachable (`dormant_unreachable`); the closure
  invariant is kept by an accepted step (`step_keeps_closed`) and by publishing, which moves a draft into
  the active schema (`publish_keeps_closed`); without the invariant a dormant entity is reachable
  (`unclosed_reaches_dormant`).
- **The build lease.** The lease (token and expiry) is kernel state of the tenant; a step is applied only
  when it carries the current token and the clock is before the expiry. Proved: after an acquisition no
  step with an earlier token is applied (`no_stale_step_after_acquire`), and no step is applied at or
  after the expiry in force (`no_step_after_expiry`).
- **The held upgrade.** Approval recomputes the diff and applies it only when it equals the held one
  (`approve_applies_held`, `approve_class_matches`, `approve_refused_when_moved`). The classification
  is a hypothesis of `approve_class_matches`: that the classifier runs inside the applying transaction
  is not modelled.

No `sorry`, no added axioms.
-/

namespace Strata.UI.Loop

/-- A tenant schema: its active entities and its draft overlay. -/
structure Schema (E : Type) where
  active : List E
  draft : List E

/-- What an evaluation is given: the active schema for a non-builder, the active schema with the draft
    overlay for a builder. -/
def view {E : Type} (s : Schema E) (builder : Bool) : List E :=
  if builder then s.active ++ s.draft else s.active

/-- No active entity references a dormant one: every reference from an active entity is active. -/
def Closed {E : Type} (refs : E → List E) (s : Schema E) : Prop :=
  ∀ e ∈ s.active, ∀ r ∈ refs e, r ∈ s.active

/-- Reachable by following references from a starting list. -/
inductive Reach {E : Type} (refs : E → List E) (start : List E) : E → Prop where
  | base (e : E) : e ∈ start → Reach refs start e
  | step (e r : E) : Reach refs start e → r ∈ refs e → Reach refs start r

/-- Everything reachable from what a non-builder is given is active.
    Verifies: DS-017 -/
theorem nonbuilder_reach_active {E : Type} (refs : E → List E) (s : Schema E) (hc : Closed refs s)
    (x : E) (h : Reach refs (view s false) x) : x ∈ s.active := by
  induction h with
  | base e he => simpa [view] using he
  | step e r _ hr ih => exact hc e ih r hr

/-- No dormant entity, absent from the active schema, is reachable by a non-builder.
    Verifies: DS-017 -/
theorem dormant_unreachable {E : Type} (refs : E → List E) (s : Schema E) (hc : Closed refs s)
    (x : E) (hx : x ∉ s.active) : ¬ Reach refs (view s false) x :=
  fun h => hx (nonbuilder_reach_active refs s hc x h)

/-- Without the closure invariant, an active entity referencing a dormant one exposes it. -/
theorem unclosed_reaches_dormant :
    ∃ (refs : Nat → List Nat) (s : Schema Nat) (x : Nat),
      x ∉ s.active ∧ Reach refs (view s false) x := by
  refine ⟨fun e => if e = 0 then [1] else [], ⟨[0], [1]⟩, 1, by simp, ?_⟩
  exact Reach.step 0 1 (Reach.base 0 (by simp [view])) (by simp)

/-- A step changes the active schema and the draft; the classifier accepts it only when the result is
    closed. -/
def accept {E : Type} [DecidableEq E] (refs : E → List E) (s' : Schema E) : Bool :=
  s'.active.all (fun e => (refs e).all (fun r => s'.active.contains r))

/-- An accepted step leaves the schema closed.
    Verifies: DS-018 -/
theorem step_keeps_closed {E : Type} [DecidableEq E] (refs : E → List E) (s' : Schema E)
    (h : accept refs s' = true) : Closed refs s' := by
  intro e he r hr
  simp only [accept, List.all_eq_true, List.contains_iff_mem] at h
  exact h e he r hr

/-- Publishing moves the draft overlay into the active schema in one change. -/
def publish {E : Type} (s : Schema E) : Schema E := ⟨s.active ++ s.draft, []⟩

/-- Publishing keeps the schema closed when every reference of a draft entity is active or in the
    draft.
    Verifies: DS-018 -/
theorem publish_keeps_closed {E : Type} (refs : E → List E) (s : Schema E) (hc : Closed refs s)
    (hd : ∀ e ∈ s.draft, ∀ r ∈ refs e, r ∈ s.active ∨ r ∈ s.draft) : Closed refs (publish s) := by
  intro e he r hr
  simp only [publish, List.mem_append] at he ⊢
  rcases he with he | he
  · exact Or.inl (hc e he r hr)
  · exact hd e he r hr

/-- After publishing, what a non-builder is given is what a builder was given before. -/
theorem publish_view {E : Type} (s : Schema E) : view (publish s) false = view s true := by
  simp [publish, view]

/-- Lease events, each with the clock: an acquisition with a lifetime, a renewal by the holder, or a
    step carrying a token. -/
inductive Ev where
  | acquire (now ttl : Nat)
  | renew (token now ttl : Nat)
  | step (token now : Nat)

/-- The lease, as tenant kernel state: the current token and expiry, and the applied steps with the
    token, the time and the expiry in force when each was applied. -/
structure Lease where
  cur : Nat
  expiry : Nat
  applied : List (Nat × Nat × Nat)

/-- One event. -/
def stepL (s : Lease) : Ev → Lease
  | .acquire now ttl => { s with cur := s.cur + 1, expiry := now + ttl }
  | .renew t now ttl => if t = s.cur ∧ now < s.expiry then { s with expiry := now + ttl } else s
  | .step t now =>
    if t = s.cur ∧ now < s.expiry then { s with applied := (t, now, s.expiry) :: s.applied } else s

/-- A run of events. -/
def runL (s : Lease) : List Ev → Lease
  | [] => s
  | e :: es => runL (stepL s e) es

theorem stepL_cur_ge (s : Lease) (e : Ev) : s.cur ≤ (stepL s e).cur := by
  cases e with
  | acquire now ttl => simp [stepL]
  | renew t now ttl => simp only [stepL]; split <;> simp
  | step t now => simp only [stepL]; split <;> simp

theorem stepL_new (s : Lease) (e : Ev) (x : Nat × Nat × Nat) (hx : x ∈ (stepL s e).applied)
    (hn : x ∉ s.applied) : x.1 = s.cur ∧ x.2.1 < x.2.2 ∧ x.2.2 = s.expiry := by
  cases e with
  | acquire now ttl => simp [stepL] at hx; exact absurd hx hn
  | renew t now ttl =>
    simp only [stepL] at hx
    split at hx <;> exact absurd hx hn
  | step t now =>
    simp only [stepL] at hx
    split at hx
    · rename_i hc
      simp only [List.mem_cons] at hx
      rcases hx with rfl | hx
      · exact ⟨hc.1, hc.2, rfl⟩
      · exact absurd hx hn
    · exact absurd hx hn

theorem runL_new_tokens_ge (s : Lease) : ∀ es x, x ∈ (runL s es).applied → x ∉ s.applied →
    s.cur ≤ x.1 ∧ x.2.1 < x.2.2
  | [], x, h, hn => absurd h hn
  | e :: es, x, h, hn => by
    simp only [runL] at h
    by_cases hm : x ∈ (stepL s e).applied
    · obtain ⟨h1, h2, _⟩ := stepL_new s e x hm hn
      exact ⟨by omega, h2⟩
    · obtain ⟨h1, h2⟩ := runL_new_tokens_ge (stepL s e) es x h hm
      exact ⟨Nat.le_trans (stepL_cur_ge s e) h1, h2⟩

/-- After an acquisition, no step carrying an earlier token is ever applied.
    Verifies: DS-015 -/
theorem no_stale_step_after_acquire (s : Lease) (now ttl : Nat) (es : List Ev) (x : Nat × Nat × Nat)
    (h : x ∈ (runL s (.acquire now ttl :: es)).applied) (hn : x ∉ s.applied) : s.cur < x.1 := by
  simp only [runL] at h
  have := (runL_new_tokens_ge (stepL s (.acquire now ttl)) es x h (by simpa [stepL] using hn)).1
  simp [stepL] at this
  omega

/-- Every step applied was applied before the expiry in force when it was applied.
    Verifies: DS-015 -/
theorem no_step_after_expiry (s : Lease) (es : List Ev) (x : Nat × Nat × Nat)
    (h : x ∈ (runL s es).applied) (hn : x ∉ s.applied) : x.2.1 < x.2.2 :=
  (runL_new_tokens_ge s es x h hn).2

/-- A held upgrade: the bundle, and the diff and classification shown when it was held. -/
structure Held (Bd D C : Type) where
  bundle : Bd
  diff : D
  cls : C

/-- Approval: recompute the diff against the schema as it is now; apply it only when it equals the
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

/-- The classification of what is applied is the one shown, given that it was computed from the held
    diff.
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

end Strata.UI.Loop
