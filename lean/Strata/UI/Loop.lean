/-
The agent build loop's guards (docs/design-system.md, "Drafts as unpublished product versions" and
"The agent build loop").

A draft is an unpublished version of a product, held in a per-tenant draft store outside the tenant's
state; the tenant's state changes only when the owner runs the upgrade of an approved revision.

- **Draft steps frame the tenant** (`draft_steps_frame`): any sequence of draft writes and discards
  leaves the tenant's state, and therefore everything any member is served (a function of that state),
  unchanged.
- **Preview reads within the viewer** (`preview_reads_within_viewer`, `followed_within_viewer`): a
  preview reads live rows as the viewer and lets draft policies only narrow; references are followed
  through a read-checked lookup, which returns nothing for a record the viewer may not read
  (`checked_lookup_unreadable`). So every live row and every followed value in a preview is one the
  viewer may read, whatever the draft's policies say.
- **No stale draft write** (`no_stale_draft_write`, `revisions_increase`): a write applies only at the
  draft's current revision (compare-and-swap), and revisions strictly increase.
- **The approved revision** (`approve_applies_held`, `approve_class_matches`, `approve_refused_when_moved`):
  the owner's upgrade re-plans the approved revision against the live tenant and applies only when the
  diff equals the approved one. That the classification is computed from the held diff is a hypothesis
  of `approve_class_matches`.

No `sorry`, no added axioms.
-/

namespace Strata.UI.Loop

/-- The platform's state for one tenant: the tenant's own state and, apart, its draft store. -/
structure Sys (T D : Type) where
  tenant : T
  drafts : D

/-- Draft events: they act on the draft store only. -/
inductive DraftEv (D : Type) where
  | write (f : D → D)
  | discard (f : D → D)

/-- One draft event. -/
def draftStep {T D : Type} (s : Sys T D) : DraftEv D → Sys T D
  | .write f => { s with drafts := f s.drafts }
  | .discard f => { s with drafts := f s.drafts }

/-- A run of draft events. -/
def draftRun {T D : Type} (s : Sys T D) : List (DraftEv D) → Sys T D
  | [] => s
  | e :: es => draftRun (draftStep s e) es

/-- Draft events leave the tenant's state, and so every member's served view, unchanged.
    Verifies: DS-017 -/
theorem draft_steps_frame {T D V : Type} (serve : T → V) :
    ∀ (s : Sys T D) (es : List (DraftEv D)),
      (draftRun s es).tenant = s.tenant ∧ serve (draftRun s es).tenant = serve s.tenant
  | s, [] => ⟨rfl, rfl⟩
  | s, e :: es => by
    have h : (draftStep s e).tenant = s.tenant := by cases e <;> rfl
    obtain ⟨h1, _⟩ := draft_steps_frame serve (draftStep s e) es
    simp only [draftRun]
    exact ⟨h1.trans h, by rw [h1.trans h]⟩

/-- A live record: its id and the ids it references. -/
structure Rec where
  id : Nat
  refs : List Nat

/-- The read-checked lookup: a record by id, only when the viewer may read it. -/
def lookupChecked (canRead : Rec → Bool) (store : List Rec) (id : Nat) : Option Rec :=
  match store.find? (fun r => r.id == id) with
  | some r => if canRead r then some r else none
  | none => none

/-- The read-checked lookup returns nothing for a record the viewer may not read. -/
theorem checked_lookup_unreadable (canRead : Rec → Bool) (store : List Rec) (id : Nat) (r : Rec)
    (h : lookupChecked canRead store id = some r) : canRead r = true := by
  unfold lookupChecked at h
  split at h
  · split at h
    · rename_i hc; cases h; exact hc
    · cases h
  · cases h

/-- A preview's live rows: the rows the viewer may read, narrowed by any draft policy. -/
def previewRows (canRead : Rec → Bool) (draftPolicy : Rec → Bool) (rows : List Rec) : List Rec :=
  (rows.filter canRead).filter draftPolicy

/-- Every live row in a preview is one the viewer may read, whatever the draft policy.
    Verifies: DS-018 -/
theorem preview_reads_within_viewer (canRead draftPolicy : Rec → Bool) (rows : List Rec) (r : Rec)
    (h : r ∈ previewRows canRead draftPolicy rows) : canRead r = true := by
  simp only [previewRows, List.mem_filter] at h
  exact h.1.2

/-- The values a preview reaches by following references from its rows, through the checked lookup. -/
def followed (canRead : Rec → Bool) (store rows : List Rec) : List Rec :=
  rows.flatMap (fun r => r.refs.filterMap (lookupChecked canRead store))

/-- Every value reached by following a reference in a preview is one the viewer may read.
    Verifies: DS-018 -/
theorem followed_within_viewer (canRead : Rec → Bool) (store rows : List Rec) (x : Rec)
    (h : x ∈ followed canRead store rows) : canRead x = true := by
  simp only [followed, List.mem_flatMap, List.mem_filterMap] at h
  obtain ⟨_, _, id, _, hx⟩ := h
  exact checked_lookup_unreadable canRead store id x hx

/-- A draft in the store: its revision and the revisions at which writes were applied, newest first. -/
structure Draft where
  rev : Nat
  applied : List Nat

/-- A write names the revision it was prepared against; it applies only at the current one. -/
def writeAt (d : Draft) (expected : Nat) : Draft :=
  if expected = d.rev then { rev := d.rev + 1, applied := d.rev :: d.applied } else d

/-- A run of writes. -/
def writes (d : Draft) : List Nat → Draft
  | [] => d
  | e :: es => writes (writeAt d e) es

/-- A write applies only at the current revision: a stale one changes nothing.
    Verifies: DS-015 -/
theorem no_stale_draft_write (d : Draft) (expected : Nat) (h : expected ≠ d.rev) :
    writeAt d expected = d := by
  simp [writeAt, h]

/-- Every applied write's revision is below the draft's revision, and the applied revisions strictly
    decrease from newest to oldest, so revisions strictly increase. -/
def Ordered : List Nat → Nat → Prop
  | [], _ => True
  | x :: xs, bound => x < bound ∧ Ordered xs x

/-- Revisions strictly increase over any run of writes.
    Verifies: DS-015 -/
theorem revisions_increase : ∀ (d : Draft) (es : List Nat), Ordered d.applied d.rev →
    Ordered (writes d es).applied (writes d es).rev ∧ d.rev ≤ (writes d es).rev
  | d, [], h => ⟨h, Nat.le_refl _⟩
  | d, e :: es, h => by
    have hstep : Ordered (writeAt d e).applied (writeAt d e).rev ∧ d.rev ≤ (writeAt d e).rev := by
      unfold writeAt
      split
      · exact ⟨⟨Nat.lt_succ_self _, h⟩, Nat.le_succ _⟩
      · exact ⟨h, Nat.le_refl _⟩
    obtain ⟨h1, h2⟩ := revisions_increase (writeAt d e) es hstep.1
    exact ⟨h1, Nat.le_trans hstep.2 h2⟩

/-- A held approval: the approved revision of the draft, and the diff and classification shown. -/
structure Held (Bd D C : Type) where
  bundle : Bd
  diff : D
  cls : C

/-- The owner's upgrade of an approved revision: re-plan against the live tenant, apply only when the
    diff equals the approved one. -/
def approve {S Bd D C : Type} [DecidableEq D] (plan : S → Bd → D) (apply : S → D → S)
    (s : S) (h : Held Bd D C) : Option S :=
  if plan s h.bundle = h.diff then some (apply s h.diff) else none

/-- What the upgrade applies is exactly the approved diff, re-planned equal on the live tenant.
    Verifies: DS-016 -/
theorem approve_applies_held {S Bd D C : Type} [DecidableEq D] (plan : S → Bd → D)
    (apply : S → D → S) (s s' : S) (h : Held Bd D C) (ha : approve plan apply s h = some s') :
    s' = apply s h.diff ∧ plan s h.bundle = h.diff := by
  unfold approve at ha
  split at ha
  · rename_i heq; cases ha; exact ⟨rfl, heq⟩
  · cases ha

/-- The classification of what is applied is the one the owner approved.
    Verifies: DS-016 -/
theorem approve_class_matches {S Bd D C : Type} [DecidableEq D] (plan : S → Bd → D)
    (apply : S → D → S) (classify : D → C) (s s' : S) (h : Held Bd D C)
    (hc : h.cls = classify h.diff) (ha : approve plan apply s h = some s') :
    classify (plan s h.bundle) = h.cls := by
  rw [(approve_applies_held plan apply s s' h ha).2, hc]

/-- A live tenant that moved so that the diff differs refuses the upgrade (`product.base-moved`).
    Verifies: DS-016 -/
theorem approve_refused_when_moved {S Bd D C : Type} [DecidableEq D] (plan : S → Bd → D)
    (apply : S → D → S) (s : S) (h : Held Bd D C) (hm : plan s h.bundle ≠ h.diff) :
    approve plan apply s h = none := by
  unfold approve
  rw [if_neg hm]

end Strata.UI.Loop
