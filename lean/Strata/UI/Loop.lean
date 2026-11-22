/-
The agent build loop's guards (docs/design-system.md, "Drafts as unpublished product versions" and
"The agent build loop").

A draft is an unpublished version of a product, held in a per-tenant draft store outside the tenant's
state; the tenant's state changes only when the owner runs the upgrade of an approved revision.

- **Draft steps frame the tenant** (`draft_steps_frame`): draft writes and discards leave the tenant's
  state, and so every member's served view, unchanged. This holds **by construction** of the model (draft
  events touch only the draft store); the real guarantee is the implementation test that the tenant's
  schema, records and schema version are byte-identical across draft writes (WG-5464).
- **The preview is built from a viewer-scoped snapshot** (`snapshot_readable`,
  `snapshot_ignores_unreadable`, `preview_depends_on_readable_only`): only records the viewer may read
  under the live schema's policies, each redacted by the field read rules. Draft filters, draft policy
  conditions and conversions run on the snapshot alone, so values the viewer may not read cannot change
  any preview, however often it is repeated. Draft policies only narrow (`preview_reads_within_viewer`),
  and references are followed in the snapshot, over chains of any length (`followed_within_viewer`).
- **No stale draft write** (`no_stale_draft_write`, `revisions_increase`).
- **The approved revision** (`approve_applies_held`, `approve_class_matches`,
  `approve_refused_when_moved`, `approve_refused_on_hash_mismatch`): the upgrade reads the revision's
  immutable snapshot by id, refuses a content hash other than the approved one, re-plans, and applies only
  an unchanged diff. The model has one live state: that the kernel commits against the schema version
  the plan used (`expected_version`) is not modelled, nor is the classification's computation (a
  hypothesis of `approve_class_matches`).
- **Staged upgrades** (`staged_invisible_until_last`): members are served nothing new until the last
  stage, which holds every grant and interface element. By construction of the model; the real guarantee
  is the planner's test that no earlier stage holds a policy, permission, invoker set or interface
  element (WG-5472).

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

/-- A live record: its id, the ids it references, and its fields (field id, value). -/
structure Rec where
  id : Nat
  refs : List Nat
  fields : List (Nat × Nat)

/-- What a viewer may read of a record: the field read rules trim the fields. -/
def redact (readField : Nat → Nat → Bool) (r : Rec) : Rec :=
  { r with fields := r.fields.filter (fun p => readField r.id p.1) }

/-- The viewer-scoped snapshot the preview is built from: the records the viewer may read, live
    schema policies deciding, each redacted by the field read rules. Draft filters, draft policy
    conditions and conversions run on this and nothing else. -/
def snapshot (canRead : Nat → Bool) (readField : Nat → Nat → Bool) (store : List Rec) : List Rec :=
  (store.filter (fun r => canRead r.id)).map (redact readField)

/-- Everything in the snapshot comes from a record the viewer may read, with only fields the viewer
    may read.
    Verifies: DS-018 -/
theorem snapshot_readable (canRead : Nat → Bool) (readField : Nat → Nat → Bool) (store : List Rec)
    (x : Rec) (hx : x ∈ snapshot canRead readField store) :
    ∃ r ∈ store, canRead r.id = true ∧ x.id = r.id ∧
      ∀ p ∈ x.fields, p ∈ r.fields ∧ readField r.id p.1 = true := by
  simp only [snapshot, List.mem_map, List.mem_filter] at hx
  obtain ⟨r, ⟨hr, hc⟩, rfl⟩ := hx
  refine ⟨r, hr, hc, rfl, ?_⟩
  intro p hp
  simp only [redact, List.mem_filter] at hp
  exact hp

/-- Two live records that agree on everything the viewer may read. -/
def Agree (readField : Nat → Nat → Bool) (r s : Rec) : Prop :=
  r.id = s.id ∧ r.refs = s.refs ∧
    r.fields.filter (fun p => readField r.id p.1) = s.fields.filter (fun p => readField s.id p.1)

/-- Two stores that agree, record by record, on everything the viewer may read. -/
def AgreeAll (readField : Nat → Nat → Bool) : List Rec → List Rec → Prop
  | [], [] => True
  | r :: rs, s :: ss => Agree readField r s ∧ AgreeAll readField rs ss
  | _, _ => False

theorem snapshot_cons (canRead : Nat → Bool) (readField : Nat → Nat → Bool) (r : Rec) (rs : List Rec) :
    snapshot canRead readField (r :: rs) =
      if canRead r.id then redact readField r :: snapshot canRead readField rs
      else snapshot canRead readField rs := by
  unfold snapshot
  by_cases h : canRead r.id = true
  · simp [List.filter_cons, h]
  · simp [List.filter_cons, h]

/-- Values the viewer may not read do not change the snapshot, so no draft filter, policy condition
    or conversion can depend on them, however often a builder previews: no field value leaks.
    Verifies: DS-018 -/
theorem snapshot_ignores_unreadable (canRead : Nat → Bool) (readField : Nat → Nat → Bool) :
    ∀ (s1 s2 : List Rec), AgreeAll readField s1 s2 →
      snapshot canRead readField s1 = snapshot canRead readField s2
  | [], [], _ => rfl
  | r :: rs, s :: ss, ⟨⟨hid, href, hf⟩, hrest⟩ => by
    have ih := snapshot_ignores_unreadable canRead readField rs ss hrest
    have hred : redact readField r = redact readField s := by
      cases r; cases s
      simp only [redact] at hf ⊢
      simp only at hid href
      subst hid; subst href
      rw [hf]
    rw [snapshot_cons, snapshot_cons, hid, hred, ih]
  | [], _ :: _, h => absurd h (by simp [AgreeAll])
  | _ :: _, [], h => absurd h (by simp [AgreeAll])

/-- A preview computation: any function of the snapshot, for any draft policy, filter or conversion.
    Previews over stores that agree on what the viewer may read are equal. -/
theorem preview_depends_on_readable_only {α : Type} (canRead : Nat → Bool) (readField : Nat → Nat → Bool)
    (f : List Rec → α) (s1 s2 : List Rec) (h : AgreeAll readField s1 s2) :
    f (snapshot canRead readField s1) = f (snapshot canRead readField s2) := by
  rw [snapshot_ignores_unreadable canRead readField s1 s2 h]

/-- Draft policies only narrow: the preview's rows are snapshot rows a draft policy keeps.
    Verifies: DS-018 -/
theorem preview_reads_within_viewer (draftPolicy : Rec → Bool) (snap : List Rec) (r : Rec)
    (h : r ∈ snap.filter draftPolicy) : r ∈ snap := (List.mem_filter.1 h).1

/-- A reference followed in a preview is looked up in the snapshot, never in the store. -/
def lookupIn (snap : List Rec) (id : Nat) : Option Rec := snap.find? (fun r => r.id == id)

theorem lookupIn_mem (snap : List Rec) (id : Nat) (r : Rec) (h : lookupIn snap id = some r) :
    r ∈ snap := List.mem_of_find?_eq_some h

/-- The records reached by following references `k` hops from some rows. -/
def hops (snap : List Rec) : Nat → List Rec → List Rec
  | 0, rows => rows
  | k + 1, rows => (hops snap k rows).flatMap (fun r => r.refs.filterMap (lookupIn snap))

/-- Every record reached by any chain of references from snapshot rows is in the snapshot, so it is
    readable and redacted (`snapshot_readable`).
    Verifies: DS-018 -/
theorem followed_within_viewer (snap : List Rec) (rows : List Rec) (hrows : ∀ r ∈ rows, r ∈ snap) :
    ∀ (k : Nat) (x : Rec), x ∈ hops snap k rows → x ∈ snap
  | 0, x, h => hrows x h
  | k + 1, x, h => by
    simp only [hops, List.mem_flatMap, List.mem_filterMap] at h
    obtain ⟨_, _, id, _, hx⟩ := h
    exact lookupIn_mem snap id x hx


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

/-- A held approval: the submitted revision's id and content hash, and the diff and classification
    shown. -/
structure Held (R H D C : Type) where
  rev : R
  hash : H
  diff : D
  cls : C

/-- The owner's upgrade of an approved revision: read the revision's immutable snapshot from the
    draft store by id, refuse unless its content hash is the approved one, re-plan its bundle against
    the live tenant, and apply only when the diff equals the approved one. The upgrade takes the
    revision id and hash, never a bundle from the request. -/
def approve {S R H Bd D C : Type} [DecidableEq H] [DecidableEq D] (store : R → Option (Bd × H))
    (plan : S → Bd → D) (apply : S → D → S) (s : S) (h : Held R H D C) : Option S :=
  match store h.rev with
  | some (b, hh) => if hh = h.hash ∧ plan s b = h.diff then some (apply s h.diff) else none
  | none => none

/-- What the upgrade applies is exactly the approved diff, from the snapshot with the approved hash,
    re-planned equal on the live tenant.
    Verifies: DS-016 -/
theorem approve_applies_held {S R H Bd D C : Type} [DecidableEq H] [DecidableEq D]
    (store : R → Option (Bd × H)) (plan : S → Bd → D) (apply : S → D → S) (s s' : S)
    (h : Held R H D C) (ha : approve store plan apply s h = some s') :
    s' = apply s h.diff ∧ ∃ b, store h.rev = some (b, h.hash) ∧ plan s b = h.diff := by
  unfold approve at ha
  split at ha
  · rename_i b hh hs
    split at ha
    · rename_i hc
      cases ha
      exact ⟨rfl, b, by rw [hs, hc.1], hc.2⟩
    · cases ha
  · cases ha

/-- The classification of what is applied is the one approved, given it was computed from the held
    diff.
    Verifies: DS-016 -/
theorem approve_class_matches {S R H Bd D C : Type} [DecidableEq H] [DecidableEq D]
    (store : R → Option (Bd × H)) (plan : S → Bd → D) (apply : S → D → S) (classify : D → C)
    (s s' : S) (h : Held R H D C) (hc : h.cls = classify h.diff)
    (ha : approve store plan apply s h = some s') :
    ∃ b, store h.rev = some (b, h.hash) ∧ classify (plan s b) = h.cls := by
  obtain ⟨_, b, hb, hp⟩ := approve_applies_held store plan apply s s' h ha
  exact ⟨b, hb, by rw [hp, hc]⟩

/-- A live tenant that moved so that the diff differs refuses the upgrade (`product.base-moved`).
    Verifies: DS-016 -/
theorem approve_refused_when_moved {S R H Bd D C : Type} [DecidableEq H] [DecidableEq D]
    (store : R → Option (Bd × H)) (plan : S → Bd → D) (apply : S → D → S) (s : S)
    (h : Held R H D C) (b : Bd) (hb : store h.rev = some (b, h.hash)) (hm : plan s b ≠ h.diff) :
    approve store plan apply s h = none := by
  unfold approve
  rw [hb]
  simp [hm]

/-- A snapshot whose content hash is not the approved one refuses the upgrade.
    Verifies: DS-016 -/
theorem approve_refused_on_hash_mismatch {S R H Bd D C : Type} [DecidableEq H] [DecidableEq D]
    (store : R → Option (Bd × H)) (plan : S → Bd → D) (apply : S → D → S) (s : S)
    (h : Held R H D C) (b : Bd) (hh : H) (hb : store h.rev = some (b, hh)) (hne : hh ≠ h.hash) :
    approve store plan apply s h = none := by
  unfold approve
  rw [hb]
  simp [hne]

/-- A staged upgrade: earlier stages change only what members cannot reach (new entities with no
    grants, no invokers and no interface); the last stage carries every policy, permission, invoker
    set and interface element. -/
structure Staged (Dt I : Type) where
  data : Dt
  iface : I

/-- Applying earlier stages, each acting on the unreachable part only. -/
def applyEarly {Dt I : Type} (s : Staged Dt I) (stages : List (Dt → Dt)) : Staged Dt I :=
  { s with data := stages.foldl (fun d f => f d) s.data }

/-- Members are served a function of what they can reach (the interface, grants and invokers), so
    nothing new is served to them until the last stage commits, whichever earlier stages have run.
    Verifies: DS-021 -/
theorem staged_invisible_until_last {Dt I V : Type} (serve : I → V) (s : Staged Dt I)
    (stages : List (Dt → Dt)) : serve (applyEarly s stages).iface = serve s.iface := rfl

end Strata.UI.Loop
