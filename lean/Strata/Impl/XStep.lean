/-
The executable kernel, stage 2: one intent against an indexed tenant.

`stepTenantX` is `stepTenantI` (stage 1) with the scans gone:

* a write's clashes are index lookups (`problemX`);
* a delete asks the referrer index which records block it (`blockedX`), and clears the references it
  must clear by visiting the cells the index lists, not every slot (`XTenant.deleteX`);
* every accepted step updates the indexes for the cells it changes (`XTenant.reindex`), and a schema
  change that makes a field or a constraint unique indexes the existing records for it.

A schema change still checks the records the change touches, as stage 1 does: a strict change must
look at them all, so this cost is inherent. `stepTenantX_core` says the indexed step's tenant is the
stage 1 step's tenant, and `stepTenantX_ok` that an accepted indexed step keeps the indexes
consistent.
-/

import Strata.Impl.XProblem
import Strata.Impl.Step

namespace Strata
namespace Impl

/-! ## Updating one object type in place -/

/-- The object types with `f` applied to the entry of `ty`. The entry is rewritten where it stands,
    so a unique list lets the compiled code update the entry's slots without copying them. -/
def modifyTypeI : List (TypeId × IObj) → TypeId → (IObj → IObj) → List (TypeId × IObj)
  | [], _, _ => []
  | (k, p) :: es, ty, f => if k == ty then (k, f p) :: es else (k, p) :: modifyTypeI es ty f

/-- The tenant with `f` applied to object type `ty`. -/
def ITenant.modifyType (t : ITenant) (ty : TypeId) (f : IObj → IObj) : ITenant :=
  { t with types := modifyTypeI t.types ty f }

/-! ## The blocked check and the clearing of a delete -/

/-- Some record names record `id` of type `ty` through a field that forbids its deletion: a look at
    the cells the referrer index lists for it. -/
def blockedX (core : ITenant) (rx : MM Cell) (ty : TypeId) (id : RecordId) : Bool :=
  (rx.get (ty, id)).any (fun c => match core.cellRec c with
    | some (o, r) => blocks c.1 o.schema ty id (c.2, r)
    | none => false)

/-- Clear the references to `(ty, id)` from the record in cell `c`, if it holds one, and update the
    indexes for it. -/
def XTenant.clearOne (ty : TypeId) (id : RecordId) (y : XTenant) (c : Cell) : XTenant :=
  match y with
  | ⟨core, ux, cx, rx⟩ =>
    match core.typ? c.1 with
    | none => ⟨core, ux, cx, rx⟩
    | some o =>
      match getRec o.slots c.2 with
      | none => ⟨core, ux, cx, rx⟩
      | some r =>
        let sch := o.schema
        let us := o.uniques
        let r' := clearRef sch ty id r
        XTenant.reindex
          (core.modifyType c.1 (fun o => { o with slots := setSlot o.slots c.2 (some r') }))
          ux cx rx c.1 c.2 sch us (some r) (some r')

/-- The tenant after record `id` of type `ty` (holding `old`, in a type with schema `sch` and
    constraints `us`) is deleted: the record leaves its slot and the indexes, and then each cell the
    referrer index lists for it has its references to it cleared. -/
def XTenant.deleteX (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell) (ty : TypeId) (id : RecordId)
    (sch : Schema) (us : List (List FieldId)) (old : Record) : XTenant :=
  let y := XTenant.reindex
    (core.modifyType ty (fun o => { o with slots := setSlot o.slots id none }))
    ux cx rx ty id sch us (some old) none
  (y.ridx.get (ty, id)).foldl (XTenant.clearOne ty id) y

/-- The cells of a type's slot array. -/
def cellsOf (ty : TypeId) (n : Nat) : List Cell := (List.range n).map (fun i => (ty, i))

/-- The unique-index keys the records of type `ty` hold under schema `sch`, by cell. -/
def ukeysUnder (ty : TypeId) (o : IObj) (sch : Schema) (c : Cell) :
    List (TypeId × FieldId × Val) :=
  match getRec o.slots c.2 with
  | some r => ukeysOf ty sch r
  | none => []

/-- The composite-index keys of the constraint `fs` the records of type `ty` hold, by cell. -/
def ckeysUnder (ty : TypeId) (o : IObj) (fs : List FieldId) (c : Cell) :
    List (TypeId × List FieldId × List Val) :=
  match getRec o.slots c.2 with
  | some r => ckeysOf ty [fs] r
  | none => []

/-! ## The step -/

/-- One intent against one indexed tenant, in pieces. A refusal hands the tenant back untouched,
    built from the pieces the step was given, so a caller that keeps no other reference to the
    tenant lets the compiled code update it in place, accepted or not. The permission tests are the
    stage-1 ones (`canCreateI`, `canUpdateI`, `canDeleteI`, `canManage` on the header). -/
def stepParts (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell) (p : Principal) :
    Intent → Except (Err × XTenant) XTenant
  | .create ty fs =>
    match core.typ? ty with
    | none => .error (unknownTypeI core p ty .create, ⟨core, ux, cx, rx⟩)
    | some o =>
      if !canAttemptCreateI core p ty then .error (.unauthorized, ⟨core, ux, cx, rx⟩)
      else
      match newRecord o.schema p fs with
      | .error _ => .error (.evaluation, ⟨core, ux, cx, rx⟩)
      | .ok r =>
        if !canCreateI core p ty r then .error (.unauthorized, ⟨core, ux, cx, rx⟩)
        else
          match problemX core.limits o.schema o.uniques true core.slotsOf ux cx ty core.nextId r with
          | some e => .error (e, ⟨core, ux, cx, rx⟩)
          | none =>
            match constraintProblem (writeEnv p r none) o.constraints with
            | some e => .error (e, ⟨core, ux, cx, rx⟩)
            | none =>
              let id := core.nextId
              let sch := o.schema
              let us := o.uniques
              .ok (XTenant.reindex
                { core.modifyType ty
                    (fun o => { o with slots := (padTo o.slots id).push (some r) }) with
                  nextId := id + 1 }
                ux cx rx ty id sch us none (some r))
  | .update ty id fs ev =>
    match core.typ? ty with
    | none => .error (unknownTypeI core p ty .update, ⟨core, ux, cx, rx⟩)
    | some o =>
      match getRec o.slots id with
      | none => .error (missingRecordI core p ty .update, ⟨core, ux, cx, rx⟩)
      | some old =>
        let r := patched old fs
        if !canUpdateI core p ty old r then .error (.unauthorized, ⟨core, ux, cx, rx⟩)
        else if versionClash ev old then .error (.versionConflict, ⟨core, ux, cx, rx⟩)
        else
          match problemX core.limits o.schema o.uniques true core.slotsOf ux cx ty id r with
          | some e => .error (e, ⟨core, ux, cx, rx⟩)
          | none =>
            match constraintProblem (writeEnv p r (some old)) o.constraints with
            | some e => .error (e, ⟨core, ux, cx, rx⟩)
            | none =>
              let sch := o.schema
              let us := o.uniques
              .ok (XTenant.reindex
                (core.modifyType ty (fun o => { o with slots := setSlot o.slots id (some r) }))
                ux cx rx ty id sch us (some old) (some r))
  | .delete ty id ev =>
    match core.typ? ty with
    | none => .error (unknownTypeI core p ty .delete, ⟨core, ux, cx, rx⟩)
    | some o =>
      match getRec o.slots id with
      | none => .error (missingRecordI core p ty .delete, ⟨core, ux, cx, rx⟩)
      | some old =>
        if !canDeleteI core p ty old then .error (.unauthorized, ⟨core, ux, cx, rx⟩)
        else if versionClash ev old then .error (.versionConflict, ⟨core, ux, cx, rx⟩)
        else if blockedX core rx ty id then .error (.stillReferenced, ⟨core, ux, cx, rx⟩)
        else .ok (XTenant.deleteX core ux cx rx ty id o.schema o.uniques old)
  | .setRequired ty f strict =>
    if !canManage core.header p then .error (.unauthorized, ⟨core, ux, cx, rx⟩)
    else
      match core.typ? ty with
      | none => .error (.invalidSchema, ⟨core, ux, cx, rx⟩)
      | some o =>
        let t' := core.putType ty { o with schema := makeRequired o.schema f }
        if schemaChangeRefused core.limits (scopedTypes t'.abs.types (ty :: fieldTargets o.schema f))
            strict then .error (.migrationRejected, ⟨core, ux, cx, rx⟩)
        else .ok ⟨t', ux, cx, rx⟩
  | .setUnique ty f strict =>
    if !canManage core.header p then .error (.unauthorized, ⟨core, ux, cx, rx⟩)
    else
      match core.typ? ty with
      | none => .error (.invalidSchema, ⟨core, ux, cx, rx⟩)
      | some o =>
        match o.schema.lookup f with
        | none => .error (.invalidSchema, ⟨core, ux, cx, rx⟩)
        | some d =>
          if d.ty.isList then .error (.invalidSchema, ⟨core, ux, cx, rx⟩)
          else
            let t' := core.putType ty { o with schema := makeUnique o.schema f }
            if schemaChangeRefused core.limits
                (scopedTypes t'.abs.types (ty :: fieldTargets o.schema f)) strict then
              .error (.migrationRejected, ⟨core, ux, cx, rx⟩)
            else .ok ⟨t', ux.addCells (cellsOf ty o.slots.size)
              (ukeysUnder ty o (makeUnique o.schema f)), cx, rx⟩
  | .defineUnique ty fs =>
    if !canManage core.header p then .error (.unauthorized, ⟨core, ux, cx, rx⟩)
    else
      match core.typ? ty with
      | none => .error (.invalidSchema, ⟨core, ux, cx, rx⟩)
      | some o =>
        if !uniqueFieldsOk o.schema fs then .error (.invalidSchema, ⟨core, ux, cx, rx⟩)
        else if dupFields (absSlots o.slots) fs then .error (.uniqueViolated, ⟨core, ux, cx, rx⟩)
        else
          let t' := core.putType ty { o with uniques := o.uniques ++ [fs] }
          if schemaChangeRefused core.limits (scopedTypes t'.abs.types [ty]) true then
            .error (.migrationRejected, ⟨core, ux, cx, rx⟩)
          else .ok ⟨t', ux, cx.addCells (cellsOf ty o.slots.size) (ckeysUnder ty o fs), rx⟩

/-- One intent against one indexed tenant; a refusal hands the tenant back. -/
def stepTenantXL (x : XTenant) (p : Principal) (i : Intent) : Except (Err × XTenant) XTenant :=
  match x with
  | ⟨core, ux, cx, rx⟩ => stepParts core ux cx rx p i

/-- One intent against one indexed tenant. -/
def stepTenantX (x : XTenant) (p : Principal) (i : Intent) : Except Err XTenant :=
  (stepTenantXL x p i).mapError Prod.fst

end Impl
end Strata
