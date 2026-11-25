/-
The executable kernel, stage 1: one intent against one tenant, and against the world.

`stepTenantI` is the spec's `stepTenant` (`Strata/Model.lean`) on slots. The permission tests and
the versions are the spec's functions on the same fields, restated on `ITenant` only where the
spec's takes a `Tenant` (they read no records, so each restatement is `rfl` on the abstraction).
Record lookups, the write checks and the scans go through `Strata/Impl/Store.lean` and
`Strata/Impl/Problem.lean`. The schema changes check the types they touch; they call the spec's
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

/-- The refusal for an intent that names an object type the tenant does not have (the spec's
    `unknownType`). -/
def unknownTypeI (t : ITenant) (p : PrincipalId) : Err :=
  if p == t.owner then .unknownObject else .unauthorized

/-- Some record of any type refers to record `id` of type `ty` through a field that forbids its
    deletion (the spec's `blockedDelete`): a scan of every slot. -/
def blockedI (t : ITenant) (ty : TypeId) (id : RecordId) : Bool :=
  t.types.any (fun e => anyFrom e.2.slots (blocks e.1 e.2.schema ty id) e.2.slots.size)

/-- The object type `j` after record `id` of type `ty` is deleted (the spec's `delObj`): in its own
    table the slot is cleared, and in every table the references to it are cleared. -/
def delObjI (ty : TypeId) (id : RecordId) (j : TypeId) (o : IObj) : IObj :=
  let cleared := o.slots.map (Option.map (clearRef o.schema ty id))
  { o with slots := if j == ty then setSlot cleared id none else cleared }

/-- One intent against one tenant, on slots. -/
def stepTenantI (t : ITenant) (p : PrincipalId) : Intent → Except Err ITenant
  | .create ty fs =>
    match t.typ? ty with
    | none => .error (unknownTypeI t p)
    | some o =>
      if !canCreateI t p then .error .unauthorized
      else
        let r := newRecord o.schema p fs
        match problemI t.limits o.schema o.uniques true t.slotsOf o.slots t.nextId r with
        | some e => .error e
        | none =>
          let o' := { o with slots := (padTo o.slots t.nextId).push (some r) }
          .ok { t.putType ty o' with nextId := t.nextId + 1 }
  | .update ty id fs ev =>
    match t.typ? ty with
    | none => .error (unknownTypeI t p)
    | some o =>
      match getRec o.slots id with
      | none => .error (missingRecordI t p)
      | some old =>
        if !canEditI t p old then .error .unauthorized
        else if versionClash ev old then .error .versionConflict
        else
          let r := patched old fs
          match problemI t.limits o.schema o.uniques true t.slotsOf o.slots id r with
          | some e => .error e
          | none => .ok (t.putType ty { o with slots := setSlot o.slots id (some r) })
  | .delete ty id ev =>
    match t.typ? ty with
    | none => .error (unknownTypeI t p)
    | some o =>
      match getRec o.slots id with
      | none => .error (missingRecordI t p)
      | some old =>
        if !canEditI t p old then .error .unauthorized
        else if versionClash ev old then .error .versionConflict
        else if blockedI t ty id then .error .stillReferenced
        else .ok { t with types := t.types.map (fun e => (e.1, delObjI ty id e.1 e.2)) }
  | .setRequired ty f strict =>
    if p != t.owner then .error .unauthorized
    else
      match t.typ? ty with
      | none => .error .invalidSchema
      | some o =>
        let t' := t.putType ty { o with schema := makeRequired o.schema f }
        if schemaChangeRefused t.limits (scopedTypes t'.abs.types (ty :: fieldTargets o.schema f))
            strict then .error .migrationRejected
        else .ok t'
  | .setUnique ty f strict =>
    if p != t.owner then .error .unauthorized
    else
      match t.typ? ty with
      | none => .error .invalidSchema
      | some o =>
        match o.schema.lookup f with
        | none => .error .invalidSchema
        | some d =>
          if d.ty.isList then .error .invalidSchema
          else
            let t' := t.putType ty { o with schema := makeUnique o.schema f }
            if schemaChangeRefused t.limits
                (scopedTypes t'.abs.types (ty :: fieldTargets o.schema f)) strict then
              .error .migrationRejected
            else .ok t'
  | .defineUnique ty fs =>
    if p != t.owner then .error .unauthorized
    else
      match t.typ? ty with
      | none => .error .invalidSchema
      | some o =>
        if !uniqueFieldsOk o.schema fs then .error .invalidSchema
        else if dupFields (absSlots o.slots) fs then .error .uniqueViolated
        else
          let t' := t.putType ty { o with uniques := o.uniques ++ [fs] }
          if schemaChangeRefused t.limits (scopedTypes t'.abs.types [ty]) true then
            .error .migrationRejected
          else .ok t'

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
    missingRecordI t p = missingRecord t.abs p := rfl

theorem unknownTypeI_eq (t : ITenant) (p : PrincipalId) :
    unknownTypeI t p = unknownType t.abs p := rfl

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

theorem problemI_abs (t : ITenant) (o : IObj) (self : RecordId) (r : Record) :
    problem t.limits o.schema o.uniques true t.abs.env (absSlots o.slots) self r =
      problemI t.limits o.schema o.uniques true t.slotsOf o.slots self r := by
  rw [env_abs]
  exact problem_eq _ _ _ _ t.slotsOf o.slots self r

theorem blocked_eq (t : ITenant) (ty : TypeId) (id : RecordId) :
    blockedI t ty id = t.abs.blockedDelete ty id := by
  unfold blockedI Tenant.blockedDelete ITenant.abs
  simp only [List.any_map]
  congr 1
  funext e
  simp only [Function.comp, absEntry, IObj.abs, any_absSlots]

@[simp] theorem abs_owner (t : ITenant) : t.abs.owner = t.owner := rfl
@[simp] theorem abs_limits (t : ITenant) : t.abs.limits = t.limits := rfl
@[simp] theorem abs_nextId (t : ITenant) : t.abs.nextId = t.nextId := rfl
@[simp] theorem abs_types (t : ITenant) : t.abs.types = t.types.map absEntry := rfl

theorem afterDelete_abs (t : ITenant) (ty : TypeId) (id : RecordId) :
    (ITenant.mk (t.types.map (fun e => (e.1, delObjI ty id e.1 e.2))) t.owner t.editors t.writers
      t.nextId t.limits).abs = t.abs.afterDelete ty id := by
  unfold ITenant.abs Tenant.afterDelete
  simp only [List.map_map]
  congr 1
  apply List.map_congr_left
  intro e _
  simp only [Function.comp, absEntry, delObjI, IObj.abs, delObj]
  by_cases h : e.1 = ty
  · have hb : (e.1 == ty) = true := by simp [h]
    simp only [hb, if_true, setSlot, delClear, absSlots_clear]
  · have hb : (e.1 == ty) = false := by simp [h]
    simp only [hb, Bool.false_eq_true, if_false, delClear, absSlots_map]

/-- `Except.map` pushed into an `if`. -/
theorem except_map_ite (f : ITenant → Tenant) {c : Prop} [Decidable c] (a b : Except Err ITenant) :
    Except.map f (if c then a else b) = if c then Except.map f a else Except.map f b := by
  split <;> rfl

/-! ## Refinement of one tenant's step -/

/-- The implementation's result, read through the abstraction, is the spec's result on the
    abstracted state: the same refusal, or the abstraction of the accepted tenant. No type's slot
    array is longer than the counter (`hinv`), so a created record gets the spec's id. -/
theorem stepTenantI_refines (t : ITenant) (p : PrincipalId) (i : Intent) (hinv : t.Inv) :
    (stepTenantI t p i).map ITenant.abs = stepTenant t.abs p i := by
  cases i with
  | create ty fs =>
    cases hty : t.typ? ty with
    | none =>
      have hty' : t.abs.typ? ty = none := by rw [typ?_abs, hty]; rfl
      simp [stepTenantI, stepTenant, hty, hty', unknownTypeI_eq, Except.map]
    | some o =>
      have hty' : t.abs.typ? ty = some o.abs := by rw [typ?_abs, hty]; rfl
      have hsz := hinv ty o hty
      have hp : problem t.abs.limits o.abs.schema o.abs.uniques true t.abs.env o.abs.records
          t.abs.nextId (newRecord o.abs.schema p fs) =
          problemI t.limits o.schema o.uniques true t.slotsOf o.slots t.nextId
            (newRecord o.schema p fs) := problemI_abs t o t.nextId (newRecord o.schema p fs)
      simp only [stepTenantI, stepTenant, hty, hty', canCreateI_eq]
      by_cases hc : (!canCreate t.abs p) = true
      · simp [hc, Except.map]
      · simp only [hc, if_false, Bool.false_eq_true, hp]
        cases hq : problemI t.limits o.schema o.uniques true t.slotsOf o.slots t.nextId
            (newRecord o.schema p fs) with
        | some e => simp [Except.map]
        | none =>
          simp only [Except.map]
          congr 1
          simp only [ITenant.putType, ITenant.abs, putTypeI_abs, Tenant.putType]
          congr 2
          simp only [IObj.abs, absSlots_push, size_padTo _ _ hsz, absSlots_padTo]
  | update ty id fs ev =>
    cases hty : t.typ? ty with
    | none =>
      have hty' : t.abs.typ? ty = none := by rw [typ?_abs, hty]; rfl
      simp [stepTenantI, stepTenant, hty, hty', unknownTypeI_eq, Except.map]
    | some o =>
      have hty' : t.abs.typ? ty = some o.abs := by rw [typ?_abs, hty]; rfl
      have hl : o.abs.records.lookup id = getRec o.slots id := lookup_absSlots o.slots id
      cases hg : getRec o.slots id with
      | none =>
        simp only [stepTenantI, stepTenant, hty, hty', hg, hl, missingRecordI_eq]
        rfl
      | some old =>
        have hslot := getRec_some o.slots id old hg
        have hp : problem t.abs.limits o.abs.schema o.abs.uniques true t.abs.env o.abs.records
            id (patched old fs) =
            problemI t.limits o.schema o.uniques true t.slotsOf o.slots id (patched old fs) :=
          problemI_abs t o id (patched old fs)
        simp only [stepTenantI, stepTenant, hty, hty', hg, hl, canEditI_eq]
        by_cases hc : (!canEdit t.abs p old) = true
        · simp [hc, Except.map]
        · by_cases hv : versionClash ev old = true
          · simp [hc, hv, Except.map]
          · simp only [hc, hv, if_false, Bool.false_eq_true, hp]
            cases hq : problemI t.limits o.schema o.uniques true t.slotsOf o.slots id
                (patched old fs) with
            | some e => simp [Except.map]
            | none =>
              simp only [Except.map]
              congr 1
              simp only [ITenant.putType, ITenant.abs, putTypeI_abs, Tenant.putType]
              congr 2
              simp only [IObj.abs, setSlot, absSlots_setIfInBounds _ _ _ _ hslot]
  | delete ty id ev =>
    cases hty : t.typ? ty with
    | none =>
      have hty' : t.abs.typ? ty = none := by rw [typ?_abs, hty]; rfl
      simp [stepTenantI, stepTenant, hty, hty', unknownTypeI_eq, Except.map]
    | some o =>
      have hty' : t.abs.typ? ty = some o.abs := by rw [typ?_abs, hty]; rfl
      have hl : o.abs.records.lookup id = getRec o.slots id := lookup_absSlots o.slots id
      cases hg : getRec o.slots id with
      | none =>
        simp only [stepTenantI, stepTenant, hty, hty', hg, hl, missingRecordI_eq]
        rfl
      | some old =>
        simp only [stepTenantI, stepTenant, hty, hty', hg, hl, canEditI_eq, blocked_eq]
        by_cases hc : (!canEdit t.abs p old) = true
        · simp [hc, Except.map]
        · by_cases hv : versionClash ev old = true
          · simp [hc, hv, Except.map]
          · by_cases hb : t.abs.blockedDelete ty id = true
            · simp [hc, hv, hb, Except.map]
            · simp only [hc, hv, hb, if_false, Bool.false_eq_true, Except.map]
              congr 1
              exact afterDelete_abs t ty id
  | setRequired ty f strict =>
    by_cases ho : (p != t.owner) = true
    · simp [stepTenantI, stepTenant, ho, Except.map]
    · have ho' : (p != t.abs.owner) = false := by simpa using ho
      cases hty : t.typ? ty with
      | none =>
        have hty' : t.abs.typ? ty = none := by rw [typ?_abs, hty]; rfl
        simp [stepTenantI, stepTenant, hty, hty', ho, ho', Except.map]
      | some o =>
        have hty' : t.abs.typ? ty = some o.abs := by rw [typ?_abs, hty]; rfl
        have hpt := putType_abs t ty { o with schema := makeRequired o.schema f }
        simp only [stepTenantI, stepTenant, hty, hty', ho, ho', if_false, Bool.false_eq_true]
        simp only [hpt, IObj.abs_schema, IObj.abs, abs_owner, abs_limits, ho, ho', if_false,
          Bool.false_eq_true]
        split <;> simp_all [Except.map] <;> rfl
  | setUnique ty f strict =>
    by_cases ho : (p != t.owner) = true
    · simp [stepTenantI, stepTenant, ho, Except.map]
    · have ho' : (p != t.abs.owner) = false := by simpa using ho
      cases hty : t.typ? ty with
      | none =>
        have hty' : t.abs.typ? ty = none := by rw [typ?_abs, hty]; rfl
        simp [stepTenantI, stepTenant, hty, hty', ho, ho', Except.map]
      | some o =>
        have hty' : t.abs.typ? ty = some o.abs := by rw [typ?_abs, hty]; rfl
        have hpt := putType_abs t ty { o with schema := makeUnique o.schema f }
        have hl : o.abs.schema.lookup f = o.schema.lookup f := rfl
        simp only [stepTenantI, stepTenant, hty, hty', ho, ho', if_false, Bool.false_eq_true, hl]
        cases o.schema.lookup f with
        | none => simp [Except.map]
        | some d =>
          simp only []
          by_cases hd : d.ty.isList = true
          · simp [hd, Except.map]
          · simp only [hd, if_false, Bool.false_eq_true]
            simp only [hpt, IObj.abs_schema, IObj.abs, abs_owner, abs_limits]
            split <;> simp_all [Except.map] <;> rfl
  | defineUnique ty fs =>
    by_cases ho : (p != t.owner) = true
    · simp [stepTenantI, stepTenant, ho, Except.map]
    · have ho' : (p != t.abs.owner) = false := by simpa using ho
      cases hty : t.typ? ty with
      | none =>
        have hty' : t.abs.typ? ty = none := by rw [typ?_abs, hty]; rfl
        simp [stepTenantI, stepTenant, hty, hty', ho, ho', Except.map]
      | some o =>
        have hty' : t.abs.typ? ty = some o.abs := by rw [typ?_abs, hty]; rfl
        have hpt := putType_abs t ty { o with uniques := o.uniques ++ [fs] }
        simp only [stepTenantI, stepTenant, hty, hty', ho, ho', if_false, Bool.false_eq_true]
        by_cases hu : (!uniqueFieldsOk o.schema fs) = true
        · simp [hu, Except.map, IObj.abs]
        · by_cases hd : dupFields (absSlots o.slots) fs = true
          · simp [hu, hd, Except.map, IObj.abs]
          · simp only [hu, hd, if_false, Bool.false_eq_true, IObj.abs]
            simp only [hpt, IObj.abs_uniques, IObj.abs, abs_limits]
            split <;> simp_all [Except.map] <;> rfl

end Impl
end Strata
