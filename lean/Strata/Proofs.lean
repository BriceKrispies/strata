/-
Machine-checked properties of the abstract model.

Assumptions, all of them, stated once:

A1. The model in `Strata/Model.lean` is the thing being reasoned about. Nothing here refers to
    the Rust code.
A2. A world is a total function from tenant ids to optional tenants; a step is addressed to
    exactly one tenant through its context.
A3. Authorization is the model's: the owner, editors, and writers on their own records, for every
    object type of the tenant (the kernel's policies name the types they were defined for; the
    fixtures define the same policies for each type). An object type the tenant does not have may
    be acted on by the owner alone (`unknownType`).
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

/-- A step that a tenant accepts was made by a principal authorized to make it. -/
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
    | create ty fs =>
      cases hT : t.typ? ty with
      | none => simp [stepTenant, hT] at hs
      | some o =>
        simp only [stepTenant, hT] at hs
        split at hs
        · cases hs
        · split at hs
          · cases hs
          · rename_i hc
            have hc' : canCreate t c.principal = true := by simpa using hc
            simp [authorized, hT, hc']
    | update ty id fs ev =>
      cases hT : t.typ? ty with
      | none => simp [stepTenant, hT] at hs
      | some o =>
        cases hL : o.records.lookup id with
        | none => simp [stepTenant, hT, hL, missingRecord] at hs
        | some old =>
          have hr : t.recordAt ty id = some old := by
            unfold Tenant.recordAt; rw [hT]; simpa using hL
          simp only [stepTenant, hT, hL] at hs
          split at hs
          · cases hs
          · rename_i hc
            have hc' : canEdit t c.principal old = true := by simpa using hc
            simp [authorized, hr, hc']
    | delete ty id ev =>
      cases hT : t.typ? ty with
      | none => simp [stepTenant, hT] at hs
      | some o =>
        cases hL : o.records.lookup id with
        | none => simp [stepTenant, hT, hL, missingRecord] at hs
        | some old =>
          have hr : t.recordAt ty id = some old := by
            unfold Tenant.recordAt; rw [hT]; simpa using hL
          simp only [stepTenant, hT, hL] at hs
          split at hs
          · cases hs
          · rename_i hc
            have hc' : canEdit t c.principal old = true := by simpa using hc
            simp [authorized, hr, hc']
    | setRequired ty f strict =>
      simp only [stepTenant] at hs
      split at hs
      · cases hs
      · rename_i hp
        have hp' : c.principal = t.owner := by simpa using hp
        cases hT : t.typ? ty with
        | none => simp [hT] at hs
        | some o => simp [authorized, hT, hp']
    | setUnique ty f strict =>
      simp only [stepTenant] at hs
      split at hs
      · cases hs
      · rename_i hp
        have hp' : c.principal = t.owner := by simpa using hp
        cases hT : t.typ? ty with
        | none => simp [hT] at hs
        | some o => simp [authorized, hT, hp']
    | defineUnique ty fs =>
      simp only [stepTenant] at hs
      split at hs
      · cases hs
      · rename_i hp
        have hp' : c.principal = t.owner := by simpa using hp
        cases hT : t.typ? ty with
        | none => simp [hT] at hs
        | some o => simp [authorized, hT, hp']

/-- A principal who may not edit a record cannot change it: the step is refused, so no new
    world exists in which the record differs. -/
theorem unauthorized_cannot_update (w : World) (c : Ctx) (t : Tenant) (ty : TypeId) (id : RecordId)
    (o : ObjType) (old : Record) (fs : Fields) (ev : Option Nat)
    (ht : w c.tenant = some t) (hty : t.typ? ty = some o) (hl : o.records.lookup id = some old)
    (hno : canEdit t c.principal old = false) :
    step w (.update ty id fs ev) c = .error .unauthorized := by
  simp [step, ht, stepTenant, hty, hl, hno]

theorem unauthorized_cannot_delete (w : World) (c : Ctx) (t : Tenant) (ty : TypeId) (id : RecordId)
    (o : ObjType) (old : Record) (ev : Option Nat)
    (ht : w c.tenant = some t) (hty : t.typ? ty = some o) (hl : o.records.lookup id = some old)
    (hno : canEdit t c.principal old = false) :
    step w (.delete ty id ev) c = .error .unauthorized := by
  simp [step, ht, stepTenant, hty, hl, hno]

/-- The refusal does not reveal whether a record exists: a principal who could act on no record
    it did not create (neither the owner nor an editor) is refused an update of a record that does
    not exist with `unauthorized`, the refusal it gets for an existing record it may not edit, so
    it cannot probe which ids exist (the kernel's KRN-1080). -/
theorem missing_record_hidden_on_update (w : World) (c : Ctx) (t : Tenant) (ty : TypeId)
    (id : RecordId) (o : ObjType) (fs : Fields) (ev : Option Nat)
    (ht : w c.tenant = some t) (hty : t.typ? ty = some o) (hl : o.records.lookup id = none)
    (hno : canEditAny t c.principal = false) :
    step w (.update ty id fs ev) c = .error .unauthorized := by
  simp [step, ht, stepTenant, hty, hl, missingRecord, hno]

theorem missing_record_hidden_on_delete (w : World) (c : Ctx) (t : Tenant) (ty : TypeId)
    (id : RecordId) (o : ObjType) (ev : Option Nat)
    (ht : w c.tenant = some t) (hty : t.typ? ty = some o) (hl : o.records.lookup id = none)
    (hno : canEditAny t c.principal = false) :
    step w (.delete ty id ev) c = .error .unauthorized := by
  simp [step, ht, stepTenant, hty, hl, missingRecord, hno]

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

/-- Replacing the schema of one object type, with its records as they were, keeps the identifiers
    below the counter. -/
theorem idsBelow_of_putType_schema {t : Tenant} {ty : TypeId} {o o' : ObjType}
    (ho : t.typ? ty = some o) (hr : o'.records = o.records) (hids : t.IdsBelow) :
    (t.putType ty o').IdsBelow := by
  intro T o'' hT q hq
  by_cases hTy : T = ty
  · subst hTy
    rw [typ?_putType_self t T o o' ho] at hT
    cases hT
    have hq' : q ∈ o.records := by rw [← hr]; exact hq
    exact hids T o ho q hq'
  · rw [typ?_putType_other t ty T o' hTy] at hT
    exact hids T o'' hT q hq

/-- A strict, accepted step on one well-formed tenant keeps every record conforming, and every
    record id below the counter. -/
theorem stepTenant_conform_ids (t t' : Tenant) (p : PrincipalId) (i : Intent)
    (hconf : t.Conform) (hids : t.IdsBelow) (hstrict : i.strict = true)
    (h : stepTenant t p i = .ok t') :
    t'.Conform ∧ t'.IdsBelow := by
  cases i with
  | create ty fs =>
    cases hT : t.typ? ty with
    | none => simp [stepTenant, hT] at h
    | some o =>
      obtain ⟨r, -, hP, -, ht⟩ := create_ok_parts hT h
      subst ht
      constructor
      · intro T o' hT' q hq
        change (t.putType ty { o with records := (t.nextId, r) :: o.records }).typ? T = some o' at hT'
        by_cases hTy : T = ty
        · subst hTy
          rw [typ?_putType_self t T o _ hT] at hT'
          cases hT'
          rcases List.mem_cons.mp hq with rfl | hq'
          · exact (problem_none hP).1
          · exact hconf T o hT q hq'
        · rw [typ?_putType_other t ty T _ hTy] at hT'
          exact hconf T o' hT' q hq
      · intro T o' hT' q hq
        change (t.putType ty { o with records := (t.nextId, r) :: o.records }).typ? T = some o' at hT'
        show q.1 < t.nextId + 1
        by_cases hTy : T = ty
        · subst hTy
          rw [typ?_putType_self t T o _ hT] at hT'
          cases hT'
          rcases List.mem_cons.mp hq with rfl | hq'
          · exact Nat.lt_succ_self _
          · exact Nat.lt_succ_of_lt (hids T o hT q hq')
        · rw [typ?_putType_other t ty T _ hTy] at hT'
          exact Nat.lt_succ_of_lt (hids T o' hT' q hq)
  | update ty id fs ev =>
    cases hT : t.typ? ty with
    | none => simp [stepTenant, hT] at h
    | some o =>
      cases hL : o.records.lookup id with
      | none => simp [stepTenant, hT, hL, missingRecord] at h
      | some old =>
        obtain ⟨hP, -, ht⟩ := update_ok_parts hT hL h
        subst ht
        constructor
        · intro T o' hT' q hq
          by_cases hTy : T = ty
          · subst hTy
            rw [typ?_putType_self t T o _ hT] at hT'
            cases hT'
            obtain ⟨q₀, hq₀, hmap⟩ := List.mem_map.mp hq
            by_cases hid : (q₀.1 == id) = true
            · simp only [hid, if_true] at hmap
              rw [← hmap]
              exact (problem_none hP).1
            · simp only [hid, if_false] at hmap
              rw [← hmap]
              exact hconf T o hT q₀ hq₀
          · rw [typ?_putType_other t ty T _ hTy] at hT'
            exact hconf T o' hT' q hq
        · intro T o' hT' q hq
          by_cases hTy : T = ty
          · subst hTy
            rw [typ?_putType_self t T o _ hT] at hT'
            cases hT'
            obtain ⟨q₀, hq₀, hmap⟩ := List.mem_map.mp hq
            by_cases hid : (q₀.1 == id) = true
            · simp only [hid, if_true] at hmap
              rw [← hmap]
              exact hids T o hT q₀ hq₀
            · simp only [hid, if_false] at hmap
              rw [← hmap]
              exact hids T o hT q₀ hq₀
          · rw [typ?_putType_other t ty T _ hTy] at hT'
            exact hids T o' hT' q hq
  | delete ty id ev =>
    have ht := stepTenant_delete_eq h
    subst ht
    have hnb : t.blockedDelete ty id = false := by
      simp only [stepTenant] at h
      cases hT : t.typ? ty with
      | none => simp [hT] at h
      | some o =>
        cases hL : o.records.lookup id with
        | none => simp [hT, hL, missingRecord] at h
        | some old =>
          simp only [hT, hL] at h
          split at h
          · cases h
          · split at h
            · cases h
            · split at h
              · cases h
              · rename_i hc
                simpa using hc
    constructor
    · intro T o' hT' q hq
      rw [typ?_afterDelete] at hT'
      cases ho : t.typ? T with
      | none => rw [ho] at hT'; cases hT'
      | some o =>
        rw [ho] at hT'
        simp only [Option.map_some, Option.some.injEq] at hT'
        subst hT'
        obtain ⟨q₀, hq₀, hmap⟩ := List.mem_map.mp (by unfold delObj at hq; exact hq)
        subst hmap
        have hq₀' : q₀ ∈ o.records := by
          by_cases hTy : (T == ty) = true
          · rw [if_pos hTy] at hq₀
            exact (List.mem_filter.mp hq₀).1
          · rw [if_neg hTy] at hq₀
            exact hq₀
        have hne : ¬ (T == ty && q₀.1 == id) = true := by
          intro hb
          by_cases hTy : (T == ty) = true
          · rw [if_pos hTy] at hq₀
            have h1 := (List.mem_filter.mp hq₀).2
            have h2 : (q₀.1 == id) = true := ((Bool.and_eq_true _ _).mp hb).2
            simp [bne, h2] at h1
          · have h3 : (T == ty) = true := ((Bool.and_eq_true _ _).mp hb).1
            exact hTy h3
        exact conforms_afterDelete t ty id T o q₀ ho hq₀' hne hnb (hconf T o ho q₀ hq₀')
    · intro T o' hT' q hq
      rw [typ?_afterDelete] at hT'
      cases ho : t.typ? T with
      | none => rw [ho] at hT'; cases hT'
      | some o =>
        rw [ho] at hT'
        simp only [Option.map_some, Option.some.injEq] at hT'
        subst hT'
        obtain ⟨q₀, hq₀, hmap⟩ := List.mem_map.mp (by unfold delObj at hq; exact hq)
        subst hmap
        have hq₀' : q₀ ∈ o.records := by
          by_cases hTy : (T == ty) = true
          · rw [if_pos hTy] at hq₀
            exact (List.mem_filter.mp hq₀).1
          · rw [if_neg hTy] at hq₀
            exact hq₀
        exact hids T o ho q₀ hq₀'
  | setRequired ty f strict =>
    have hcu := strict_schema_change_conforms t t' p (.setRequired ty f strict) rfl hstrict h hconf
    refine ⟨hcu.2.2, ?_⟩
    cases hT : t.typ? ty with
    | none => simp only [stepTenant, hT] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hT] at h
      repeat' split at h
      all_goals try cases h
      exact idsBelow_of_putType_schema hT rfl hids
  | setUnique ty f strict =>
    have hcu := strict_schema_change_conforms t t' p (.setUnique ty f strict) rfl hstrict h hconf
    refine ⟨hcu.2.2, ?_⟩
    cases hT : t.typ? ty with
    | none => simp only [stepTenant, hT] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hT] at h
      repeat' split at h
      all_goals try cases h
      exact idsBelow_of_putType_schema hT rfl hids
  | defineUnique ty fs =>
    have hcu := strict_schema_change_conforms t t' p (.defineUnique ty fs) rfl hstrict h hconf
    refine ⟨hcu.2.2, ?_⟩
    cases hT : t.typ? ty with
    | none => simp only [stepTenant, hT] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hT] at h
      repeat' split at h
      all_goals try cases h
      exact idsBelow_of_putType_schema hT rfl hids

/-- A strict, accepted step on one well-formed tenant yields a well-formed tenant. -/
theorem stepTenant_preserves_wf (t t' : Tenant) (p : PrincipalId) (i : Intent)
    (hwf : t.WF) (hstrict : i.strict = true) (h : stepTenant t p i = .ok t') : t'.WF := by
  obtain ⟨hconf, hids, hrefs, hd, hu⟩ := hwf
  obtain ⟨hc, hi⟩ := stepTenant_conform_ids t t' p i hconf hids hstrict h
  exact ⟨hc, hi, stepTenant_preserves_refs t t' p i hrefs h,
    stepTenant_preserves_distinct t t' p i hd h,
    stepTenant_preserves_unique t t' p i ⟨hconf, hids, hrefs, hd, hu⟩ hstrict h⟩

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
    leaves every object type's unique fields and composite constraints unique. -/
theorem step_preserves_unique (w w' : World) (i : Intent) (c : Ctx)
    (hwf : w.WF) (hstrict : i.strict = true) (h : step w i c = .ok w') :
    ∀ x t', w' x = some t' → t'.UniqueRecords :=
  fun x t' hx => ((step_preserves_wf w w' i c hwf hstrict h) x t' hx).2.2.2.2

/-- A tenant with one object type of one optional integer field, and one record, with no value in
    it. -/
def laxReqObj : ObjType :=
  { schema := [(1, { ty := .int, required := false })],
    records := [(0, { owner := 0, version := 1, fields := [] })] }

def laxReqTenant : Tenant :=
  { types := [(1, laxReqObj)], owner := 0, editors := [], writers := [], nextId := 1 }

theorem laxReqTenant_typ (T : TypeId) (o : ObjType) (h : laxReqTenant.typ? T = some o) :
    T = 1 ∧ o = laxReqObj := by
  unfold laxReqTenant Tenant.typ? at h
  by_cases hT : T = 1
  · subst hT
    simp [List.lookup, laxReqObj] at h
    exact ⟨rfl, h.symm⟩
  · have hb : (T == 1) = false := by simpa using hT
    simp [List.lookup, hb] at h

theorem laxReqTenant_wf : laxReqTenant.WF := by
  refine ⟨?_, ?_, ?_, ?_, ?_⟩
  · intro T o ho q hq
    obtain ⟨rfl, rfl⟩ := laxReqTenant_typ T o ho
    have hall : ∀ q ∈ laxReqObj.records, conforms laxReqObj.schema q.2 = true := by decide
    exact hall q hq
  · intro T o ho q hq
    obtain ⟨rfl, rfl⟩ := laxReqTenant_typ T o ho
    have hall : ∀ q ∈ laxReqObj.records, q.1 < laxReqTenant.nextId := by decide
    exact hall q hq
  · intro T o ho q hq
    obtain ⟨rfl, rfl⟩ := laxReqTenant_typ T o ho
    have hall : ∀ q ∈ laxReqObj.records, refsOk laxReqTenant.env laxReqObj.schema q.2 = true := by
      decide
    exact hall q hq
  · refine ⟨by decide, fun T o ho => ?_⟩
    obtain ⟨rfl, rfl⟩ := laxReqTenant_typ T o ho
    exact ⟨by decide, by decide⟩
  · intro T o ho q₁ hq₁ q₂ hq₂ hne
    obtain ⟨rfl, rfl⟩ := laxReqTenant_typ T o ho
    have hU : ∀ q₁ ∈ laxReqObj.records, ∀ q₂ ∈ laxReqObj.records, q₁.1 ≠ q₂.1 →
        (∀ fd ∈ laxReqObj.schema, fd.2.unique = true →
          agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
        (∀ u ∈ laxReqObj.uniques, agreeOn u q₁.2.fields q₂.2.fields = false) := by decide
    exact hU q₁ hq₁ q₂ hq₂ hne

/-- The exclusion in A4 is necessary, not cautious: a non-strict schema change can leave a
    well-formed tenant with a record that no longer conforms. -/
theorem lax_change_can_break_wf :
    ∃ (t t' : Tenant) (p : PrincipalId),
      t.WF ∧ stepTenant t p (.setRequired 1 1 false) = .ok t' ∧ ¬ t'.WF := by
  refine ⟨laxReqTenant, laxReqTenant.putType 1 { laxReqObj with
    schema := makeRequired laxReqObj.schema 1 }, 0, laxReqTenant_wf, rfl, ?_⟩
  intro hw
  have hT1 := typ?_putType_self laxReqTenant 1 laxReqObj
    { laxReqObj with schema := makeRequired laxReqObj.schema 1 } rfl
  have h1 := hw.1 1 _ hT1 (0, { owner := 0, version := 1, fields := [] }) (by decide)
  revert h1
  decide

end Strata
