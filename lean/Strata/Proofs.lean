/-
Machine-checked properties of the abstract model.

Assumptions, all of them, stated once:

A1. The model in `Strata/Model.lean` is the thing being reasoned about. Nothing here refers to
    the Rust code.
A2. A world is a total function from tenant ids to optional tenants; a step is addressed to
    exactly one tenant through its context.
A3. Authorization is the model's: the owner, editors, and writers on their own records.
A4. `step_preserves_wf` excludes the non-strict schema changes (`Intent.strict`), which are
    allowed to leave non-conforming records, and records that agree on a unique field, by design.
    Referential integrity, the third part of the invariant, needs no such exclusion
    (`step_preserves_refs` in `Strata/References.lean`). The name and uniqueness parts are in
    `Strata/Uniqueness.lean`.

No `sorry`, no added axioms. `Audit.lean` prints the axioms each theorem depends on.
-/

import Strata.Model
import Strata.References
import Strata.Uniqueness

namespace Strata

/-! ## Property 4: determinism -/

/-- The same state, intent and context give the same result. In the model this holds by
    construction, because `step` is a function; the theorem states it so that it is on record
    and so that the audit covers it. -/
theorem step_deterministic (w : World) (i : Intent) (c : Ctx) (r₁ r₂ : Except Err World)
    (h₁ : step w i c = r₁) (h₂ : step w i c = r₂) : r₁ = r₂ := by
  rw [← h₁, ← h₂]

/-! ## Property 3: tenant isolation -/

/-- An accepted step addressed to one tenant leaves every other tenant exactly as it was.

    Verifies: KRN-013 -/
theorem step_isolated (w w' : World) (i : Intent) (c : Ctx)
    (h : step w i c = .ok w') : ∀ x, x ≠ c.tenant → w' x = w x := by
  intro x hx
  unfold step at h
  split at h
  · cases h
  · split at h
    · cases h
    · cases h
      simp [hx]

/-- The result for the addressed tenant depends on that tenant's state only: two worlds that
    agree on it give the same outcome for it, whatever else they contain. -/
theorem step_local (w₁ w₂ : World) (i : Intent) (c : Ctx)
    (h : w₁ c.tenant = w₂ c.tenant) :
    (step w₁ i c).map (fun w => w c.tenant) = (step w₂ i c).map (fun w => w c.tenant) := by
  unfold step
  rw [h]
  cases hw : w₂ c.tenant with
  | none => rfl
  | some t =>
    cases hs : stepTenant t c.principal i with
    | error e => simp [hs]
    | ok t' => simp [hs, Except.map]

/-! ## Property 2: authorization -/

/-- Every accepted step was made by a principal authorized to make it. -/
theorem step_ok_authorized (w w' : World) (i : Intent) (c : Ctx) (t : Tenant)
    (ht : w c.tenant = some t) (h : step w i c = .ok w') :
    authorized t c.principal i = true := by
  unfold step at h
  rw [ht] at h
  simp only at h
  cases hs : stepTenant t c.principal i with
  | error e => rw [hs] at h; cases h
  | ok t' =>
    cases i with
    | create fs =>
      simp only [stepTenant] at hs
      by_cases hc : canCreate t c.principal = true
      · simpa [authorized] using hc
      · simp [hc] at hs
    | update id fs ev =>
      simp only [stepTenant] at hs
      cases hl : t.records.lookup id with
      | none => rw [hl] at hs; cases hs
      | some old =>
        rw [hl] at hs
        by_cases he : canEdit t c.principal old = true
        · simp [authorized, hl, he]
        · simp [he] at hs
    | delete id ev =>
      simp only [stepTenant] at hs
      cases hl : t.records.lookup id with
      | none => rw [hl] at hs; cases hs
      | some old =>
        rw [hl] at hs
        by_cases he : canEdit t c.principal old = true
        · simp [authorized, hl, he]
        · simp [he] at hs
    | setRequired f strict =>
      simp only [stepTenant] at hs
      by_cases ho : c.principal = t.owner
      · simp [authorized, ho]
      · simp [ho] at hs
    | setUnique f strict =>
      simp only [stepTenant] at hs
      by_cases ho : c.principal = t.owner
      · simp [authorized, ho]
      · simp [ho] at hs
    | defineUnique fs =>
      simp only [stepTenant] at hs
      by_cases ho : c.principal = t.owner
      · simp [authorized, ho]
      · simp [ho] at hs

/-- A principal who may not edit a record cannot change it: the step is refused, so no new
    world exists in which the record differs. -/
theorem unauthorized_cannot_update (w : World) (c : Ctx) (t : Tenant) (id : RecordId)
    (old : Record) (fs : Fields) (ev : Option Nat)
    (ht : w c.tenant = some t) (hl : t.records.lookup id = some old)
    (hno : canEdit t c.principal old = false) :
    step w (.update id fs ev) c = .error .unauthorized := by
  simp [step, ht, stepTenant, hl, hno]

theorem unauthorized_cannot_delete (w : World) (c : Ctx) (t : Tenant) (id : RecordId)
    (old : Record) (ev : Option Nat)
    (ht : w c.tenant = some t) (hl : t.records.lookup id = some old)
    (hno : canEdit t c.principal old = false) :
    step w (.delete id ev) c = .error .unauthorized := by
  simp [step, ht, stepTenant, hl, hno]

/-- The refusal does not reveal whether a record exists: a principal who could act on no record
    it did not create (neither the owner nor an editor) is refused an update of a record that does
    not exist with `unauthorized`, the refusal it gets for an existing record it may not edit, so
    it cannot probe which ids exist (the kernel's KRN-1080). -/
theorem missing_record_hidden_on_update (w : World) (c : Ctx) (t : Tenant) (id : RecordId)
    (fs : Fields) (ev : Option Nat)
    (ht : w c.tenant = some t) (hl : t.records.lookup id = none)
    (hno : canEditAny t c.principal = false) :
    step w (.update id fs ev) c = .error .unauthorized := by
  simp [step, ht, stepTenant, hl, missingRecord, hno]

theorem missing_record_hidden_on_delete (w : World) (c : Ctx) (t : Tenant) (id : RecordId)
    (ev : Option Nat)
    (ht : w c.tenant = some t) (hl : t.records.lookup id = none)
    (hno : canEditAny t c.principal = false) :
    step w (.delete id ev) c = .error .unauthorized := by
  simp [step, ht, stepTenant, hl, missingRecord, hno]

/-- Stated in terms of the outcome: an unauthorized principal's intent is never accepted.

    Verifies: KRN-015 -/
theorem unauthorized_cannot_mutate (w : World) (i : Intent) (c : Ctx) (t : Tenant)
    (ht : w c.tenant = some t) (hno : authorized t c.principal i = false) :
    ∀ w', step w i c ≠ .ok w' := by
  intro w' h
  have := step_ok_authorized w w' i c t ht h
  rw [hno] at this
  cases this

/-! ## Property 1: accepted transitions preserve the invariant -/

/-- A strict, accepted step on one well-formed tenant yields a well-formed tenant. -/
theorem stepTenant_conform_ids (t t' : Tenant) (p : PrincipalId) (i : Intent)
    (hconf : ∀ q ∈ t.records, conforms t.schema q.2 = true)
    (hids : ∀ q ∈ t.records, q.1 < t.nextId) (hstrict : i.strict = true)
    (h : stepTenant t p i = .ok t') :
    (∀ q ∈ t'.records, conforms t'.schema q.2 = true) ∧ (∀ q ∈ t'.records, q.1 < t'.nextId) := by
  cases i with
  | create fs =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · rename_i hp
        cases h
        refine ⟨?_, ?_⟩
        · intro q hq
          cases hq with
          | head => exact (problem_none hp).1
          | tail _ hq' => exact hconf q hq'
        · intro q hq
          cases hq with
          | head => exact Nat.lt_succ_self _
          | tail _ hq' => exact Nat.lt_succ_of_lt (hids q hq')
  | update id fs ev =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · rename_i hp
            cases h
            refine ⟨?_, ?_⟩
            · intro q hq
              obtain ⟨q₀, hq₀, rfl⟩ := List.mem_map.mp hq
              by_cases hid : (q₀.1 == id) = true
              · simpa [hid] using (problem_none hp).1
              · simpa [hid] using hconf q₀ hq₀
            · intro q hq
              obtain ⟨q₀, hq₀, rfl⟩ := List.mem_map.mp hq
              by_cases hid : (q₀.1 == id) = true
              · simpa [hid] using hids q₀ hq₀
              · simpa [hid] using hids q₀ hq₀
  | delete id ev =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          rename_i hnb
          have hnb : t.records.any (blocks t.schema id) = false := by simpa using hnb
          cases h
          refine ⟨?_, ?_⟩
          · intro q hq
            obtain ⟨q₀, hq₀, rfl⟩ := List.mem_map.mp hq
            obtain ⟨hmem, hne⟩ := List.mem_filter.mp hq₀
            exact conforms_afterDelete t id q₀ hmem (by simpa using hne) hnb (hconf q₀ hmem)
          · intro q hq
            obtain ⟨q₀, hq₀, rfl⟩ := List.mem_map.mp hq
            exact hids q₀ (List.mem_filter.mp hq₀).1
  | setRequired f strict =>
    obtain ⟨hc, -⟩ := strict_schema_change_conforms t t' p (.setRequired f strict) rfl hstrict h
    obtain ⟨hn, hr⟩ := setRequired_spec t t' p f strict h
    refine ⟨hc, fun q hq => ?_⟩
    rw [hn]
    exact hids q (by rw [hr] at hq; exact hq)
  | setUnique f s =>
    obtain ⟨hn, hr, hs, -⟩ := setUnique_spec t t' p f s h
    rw [hn, hr, hs]
    exact ⟨fun q hq => conforms_makeUnique (hconf q hq), hids⟩
  | defineUnique fs =>
    obtain ⟨hn, hr, hs, -⟩ := defineUnique_spec t t' p fs h
    rw [hn, hr, hs]
    exact ⟨hconf, hids⟩

/-- The state invariant is kept by every accepted strict step on a tenant. -/
theorem stepTenant_preserves_wf (t t' : Tenant) (p : PrincipalId) (i : Intent)
    (hwf : t.WF) (hstrict : i.strict = true) (h : stepTenant t p i = .ok t') : t'.WF := by
  have hu := stepTenant_preserves_unique t t' p i hwf hstrict h
  obtain ⟨hconf, hids, hrefs, hd, -⟩ := hwf
  obtain ⟨hc, hi⟩ := stepTenant_conform_ids t t' p i hconf hids hstrict h
  exact ⟨hc, hi, stepTenant_preserves_refs t t' p i hrefs h,
    stepTenant_preserves_distinct t t' p i hd h, hu⟩

/-- Property 1 for the world: a strict, accepted step from a well-formed world leads to a
    well-formed world.

    Verifies: KRN-014 -/
theorem step_preserves_wf (w w' : World) (i : Intent) (c : Ctx)
    (hwf : w.WF) (hstrict : i.strict = true) (h : step w i c = .ok w') : w'.WF := by
  intro x t' hx
  unfold step at h
  cases ht : w c.tenant with
  | none => rw [ht] at h; cases h
  | some t =>
    rw [ht] at h
    simp only at h
    cases hs : stepTenant t c.principal i with
    | error e => rw [hs] at h; cases h
    | ok t₁ =>
      rw [hs] at h
      cases h
      by_cases hxc : x = c.tenant
      · simp [hxc] at hx
        subst hx
        exact stepTenant_preserves_wf t _ c.principal i (hwf c.tenant t ht) hstrict hs
      · simp [hxc] at hx
        exact hwf x t' hx

/-- Property 1 for uniqueness, for the world: a strict, accepted step from a well-formed world
    leaves every tenant's unique fields and composite constraints unique. -/
theorem step_preserves_unique (w w' : World) (i : Intent) (c : Ctx)
    (hwf : w.WF) (hstrict : i.strict = true) (h : step w i c = .ok w') :
    ∀ x t', w' x = some t' → t'.UniqueRecords :=
  fun x t' hx => ((step_preserves_wf w w' i c hwf hstrict h) x t' hx).2.2.2.2

/-- The exclusion in A4 is necessary, not cautious: a non-strict schema change can leave a
    well-formed tenant with a record that no longer conforms. -/
theorem lax_change_can_break_wf :
    ∃ (t t' : Tenant) (p : PrincipalId) (f : FieldId),
      t.WF ∧ stepTenant t p (.setRequired f false) = .ok t' ∧ ¬ t'.WF := by
  refine ⟨{ schema := [(1, { ty := .int, required := false })], owner := 0, editors := [],
            writers := [], records := [(0, { owner := 0, version := 1, fields := [] })],
            nextId := 1 },
          { schema := [(1, { ty := .int, required := true })], owner := 0, editors := [],
            writers := [], records := [(0, { owner := 0, version := 1, fields := [] })],
            nextId := 1 },
          0, 1, ?_, ?_, ?_⟩
  · refine ⟨?_, ?_, ?_, ?_, ?_⟩
    · intro q hq
      simp at hq
      subst hq
      decide
    · intro q hq
      simp at hq
      subst hq
      decide
    · intro q hq
      simp at hq
      subst hq
      decide
    · unfold Tenant.Distinct
      decide
    · unfold Tenant.UniqueRecords
      decide
  · simp [stepTenant, makeRequired, schemaChangeRefused]
  · intro hwf
    have := hwf.1 (0, { owner := 0, version := 1, fields := [] }) (by simp)
    revert this
    decide

end Strata
