/-
The executable kernel, stage 1: one intent against one tenant, and against the world.

`stepTenantI` is the spec's `stepTenant` (`Strata/Model.lean`) on slots. The permission tests and
the versions are the spec's functions on the same fields, restated on `ITenant` only where the
spec's takes a `Tenant` (they read no records, so each restatement is `rfl` on the abstraction).
Record lookups, the write checks and the scans go through `Strata/Impl/Store.lean` and
`Strata/Impl/Problem.lean`. The schema changes check the whole table; they call the spec's
`schemaChangeRefused` and `dupFields` on the abstraction, once per change.

The refinement theorems say that the implementation's result, read through `ITenant.abs` and
`absW`, is the spec's result for the abstracted state, outcome for outcome, and that an accepted
step keeps the slot invariant (`ITenant.Inv`).
-/

import Strata.Impl.Problem

namespace Strata
namespace Impl

/-- May `p` create a record in the tenant (the spec's `canCreate`). -/
def canCreateI (t : ITenant) (p : PrincipalId) : Bool :=
  p == t.owner || t.editors.contains p || t.writers.contains p

/-- May `p` change or delete record `r` (the spec's `canEdit`). -/
def canEditI (t : ITenant) (p : PrincipalId) (r : Record) : Bool :=
  p == t.owner || t.editors.contains p || (t.writers.contains p && r.owner == p)

/-- May `p` act on a record when there is no record to test (the spec's `canEditAny`). -/
def canEditAnyI (t : ITenant) (p : PrincipalId) : Bool :=
  p == t.owner || t.editors.contains p

/-- The refusal for an update or delete of a record that does not exist (the spec's
    `missingRecord`). -/
def missingRecordI (t : ITenant) (p : PrincipalId) : Err :=
  if canEditAnyI t p then .notFound else .unauthorized

/-- One intent against one tenant, on slots. -/
def stepTenantI (t : ITenant) (p : PrincipalId) : Intent → Except Err ITenant
  | .create fs =>
    if !canCreateI t p then .error .unauthorized
    else
      let r := newRecord t.schema p fs
      match problemI t.limits t.schema t.uniques true t.slots t.nextId r with
      | some e => .error e
      | none => .ok { t with slots := t.slots.push (some r), nextId := t.nextId + 1 }
  | .update id fs ev =>
    match getRec t.slots id with
    | none => .error (missingRecordI t p)
    | some old =>
      if !canEditI t p old then .error .unauthorized
      else if versionClash ev old then .error .versionConflict
      else
        let r := patched old fs
        match problemI t.limits t.schema t.uniques true t.slots id r with
        | some e => .error e
        | none => .ok { t with slots := setSlot t.slots id (some r) }
  | .delete id ev =>
    match getRec t.slots id with
    | none => .error (missingRecordI t p)
    | some old =>
      if !canEditI t p old then .error .unauthorized
      else if versionClash ev old then .error .versionConflict
      else if anyFrom t.slots (blocks t.schema id) t.slots.size then .error .stillReferenced
      else .ok { t with slots := setSlot (t.slots.map (Option.map (clearRef id))) id none }
  | .setRequired f strict =>
    if p != t.owner then .error .unauthorized
    else
      let schema' := makeRequired t.schema f
      if schemaChangeRefused t.limits schema' t.uniques (absSlots t.slots) strict then
        .error .migrationRejected
      else .ok { t with schema := schema' }
  | .setUnique f strict =>
    if p != t.owner then .error .unauthorized
    else
      match t.schema.lookup f with
      | none => .error .invalidSchema
      | some d =>
        if d.ty.isList then .error .invalidSchema
        else if schemaChangeRefused t.limits (makeUnique t.schema f) t.uniques (absSlots t.slots)
            strict then
          .error .migrationRejected
        else .ok { t with schema := makeUnique t.schema f }
  | .defineUnique fs =>
    if p != t.owner then .error .unauthorized
    else if !uniqueFieldsOk t.schema fs then .error .invalidSchema
    else if dupFields (absSlots t.slots) fs then .error .uniqueViolated
    else if schemaChangeRefused t.limits t.schema (t.uniques ++ [fs]) (absSlots t.slots) true then
      .error .migrationRejected
    else .ok { t with uniques := t.uniques ++ [fs] }

/-- The implementation's transition function on the world: one intent, in one context. -/
def stepI (w : IWorld) (i : Intent) (c : Ctx) : Except Err IWorld :=
  match w c.tenant with
  | none => .error .noTenant
  | some t =>
    match stepTenantI t c.principal i with
    | .error e => .error e
    | .ok t' => .ok (fun x => if x = c.tenant then some t' else w x)

/-! ## Agreement of the restated functions -/

theorem canCreateI_eq (t : ITenant) (p : PrincipalId) : canCreateI t p = canCreate t.abs p := rfl

theorem canEditI_eq (t : ITenant) (p : PrincipalId) (r : Record) :
    canEditI t p r = canEdit t.abs p r := rfl

theorem missingRecordI_eq (t : ITenant) (p : PrincipalId) :
    missingRecordI t p = missingRecord t.abs p := by
  unfold missingRecordI missingRecord canEditAnyI canEditAny ITenant.abs
  rfl

theorem problemI_eq_abs (t : ITenant) (us : List (List FieldId)) (uniq : Bool) (self : RecordId)
    (r : Record) (L : Limits) (schema : Schema) :
    problemI L schema us uniq t.slots self r = problem L schema us uniq (absSlots t.slots) self r :=
  (problem_eq L schema us uniq t.slots self r).symm

theorem getRec_some (a : Array (Option Record)) (id : RecordId) (r : Record)
    (h : getRec a id = some r) : a[id]? = some (some r) := by
  unfold getRec at h
  cases hc : a[id]? with
  | none => rw [hc] at h; simp at h
  | some o =>
    rw [hc] at h
    cases o with
    | none => simp at h
    | some x =>
      simp at h
      subst h
      rfl

/-- `Except.map` pushed into an `if`. -/
theorem except_map_ite (f : ITenant → Tenant) {c : Prop} [Decidable c] (a b : Except Err ITenant) :
    Except.map f (if c then a else b) = if c then Except.map f a else Except.map f b := by
  split <;> rfl

/-! ## Refinement of one tenant's step -/

/-- The implementation's result, read through the abstraction, is the spec's result on the
    abstracted state: the same refusal, or the abstraction of the accepted tenant. The slot
    count matches the counter (`hinv`), so a created record gets the spec's id. -/
theorem stepTenantI_refines (t : ITenant) (p : PrincipalId) (i : Intent)
    (hinv : t.slots.size = t.nextId) :
    (stepTenantI t p i).map ITenant.abs = stepTenant t.abs p i := by
  cases i with
  | create fs =>
    simp only [stepTenantI, stepTenant, canCreateI_eq, problemI_eq_abs, ITenant.abs]
    all_goals (repeat' split)
    all_goals simp_all [Except.map, ITenant.abs, absSlots_push]
  | update id fs ev =>
    cases hg : getRec t.slots id with
    | none =>
      simp only [stepTenantI, stepTenant, ITenant.abs, lookup_absSlots, hg, missingRecordI_eq]
      rfl
    | some old =>
      have hslot := getRec_some t.slots id old hg
      simp only [stepTenantI, stepTenant, ITenant.abs, lookup_absSlots, hg, canEditI_eq,
        problemI_eq_abs]
      all_goals (repeat' split)
      all_goals simp_all [Except.map, ITenant.abs, setSlot, absSlots_setIfInBounds _ _ _ _ hslot]
  | delete id ev =>
    cases hg : getRec t.slots id with
    | none =>
      simp only [stepTenantI, stepTenant, ITenant.abs, lookup_absSlots, hg, missingRecordI_eq]
      rfl
    | some old =>
      simp only [stepTenantI, stepTenant, ITenant.abs, lookup_absSlots, hg, any_absSlots]
      cases h1 : canEditI t p old <;> cases h2 : versionClash ev old <;>
        cases h3 : anyFrom t.slots (blocks t.schema id) t.slots.size <;>
        simp_all [canEditI_eq, Except.map, ITenant.abs, setSlot, absSlots_clear]
  | setRequired f strict =>
    simp only [stepTenantI, stepTenant, ITenant.abs]
    all_goals (repeat' split)
    all_goals simp_all [Except.map, ITenant.abs]
  | setUnique f strict =>
    simp only [stepTenantI, stepTenant, ITenant.abs]
    cases hl : t.schema.lookup f <;> simp only [hl] <;> (try simp only [except_map_ite])
      <;> (repeat' split) <;> simp_all [Except.map, ITenant.abs]
  | defineUnique fs =>
    simp only [stepTenantI, stepTenant, ITenant.abs]
    all_goals (repeat' split)
    all_goals simp_all [Except.map, ITenant.abs]

end Impl
end Strata
