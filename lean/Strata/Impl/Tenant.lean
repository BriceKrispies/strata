/-
The executable kernel, stage 1: the implementation tenant.

An implementation tenant keeps each object type's records in an array of slots (`IObj.slots`): slot
`i` holds the record with id `i`, or nothing. Record ids come from one counter for the whole tenant,
as the spec's do, so a type's array may be shorter than the counter (the ids that went to other
types are not in it); `Strata.Impl.padTo` fills the gap when a record is created. The invariant
`ITenant.Inv` says no array is longer than the counter.

`ITenant.abs` is the spec's tenant that an implementation tenant stands for: each type's records
read back as the spec's list, most recent first (`absSlots`).
-/

import Strata.Impl.Store

namespace Strata
namespace Impl

/-- One object type of an implementation tenant: its schema, its composite constraints and its
    records in slots. -/
structure IObj where
  schema : Schema
  uniques : List (List FieldId) := []
  slots : Array (Option Record) := #[]

/-- One tenant, each type's records held in slots. -/
structure ITenant where
  types : List (TypeId × IObj)
  owner : PrincipalId
  editors : List PrincipalId
  writers : List PrincipalId
  nextId : Nat
  limits : Limits := Limits.kernel

/-- The whole implementation state: each tenant id has a tenant or does not. -/
abbrev IWorld := TenantId → Option ITenant

/-- The spec's object type for an implementation one. -/
def IObj.abs (o : IObj) : ObjType :=
  { schema := o.schema, uniques := o.uniques, records := absSlots o.slots }

theorem IObj.abs_schema (o : IObj) (s : Schema) : { o with schema := s }.abs = { o.abs with schema := s } :=
  rfl

theorem IObj.abs_uniques (o : IObj) (u : List (List FieldId)) :
    { o with uniques := u }.abs = { o.abs with uniques := u } := rfl

/-- The spec's entry for an implementation entry. -/
def absEntry (e : TypeId × IObj) : TypeId × ObjType := (e.1, e.2.abs)

/-- The spec's tenant that an implementation tenant stands for. -/
def ITenant.abs (t : ITenant) : Tenant :=
  { types := t.types.map absEntry, owner := t.owner, editors := t.editors, writers := t.writers,
    nextId := t.nextId, limits := t.limits }

/-- The spec's world for an implementation world. -/
def absW (w : IWorld) : World := fun x => (w x).map ITenant.abs

/-- The object type `ty` of the tenant, if it has one. -/
def ITenant.typ? (t : ITenant) (ty : TypeId) : Option IObj := t.types.lookup ty

/-- The slots of object type `ty`; none when the tenant has no such type. -/
def ITenant.slotsOf (t : ITenant) (ty : TypeId) : Array (Option Record) :=
  match t.typ? ty with
  | some o => o.slots
  | none => #[]

/-- The object types with the entry of `ty` replaced by `o` (the spec's `putTypeList`). -/
def putTypeI : List (TypeId × IObj) → TypeId → IObj → List (TypeId × IObj)
  | [], _, _ => []
  | (k, p) :: es, ty, o => if k == ty then (ty, o) :: es else (k, p) :: putTypeI es ty o

/-- The tenant with object type `ty` replaced by `o`. -/
def ITenant.putType (t : ITenant) (ty : TypeId) (o : IObj) : ITenant :=
  { t with types := putTypeI t.types ty o }

/-- The invariant of an implementation tenant: no type's slot array is longer than the counter, so
    a created record's id is its slot. -/
def ITenant.Inv (t : ITenant) : Prop := ∀ ty o, t.typ? ty = some o → o.slots.size ≤ t.nextId

/-- The invariant of an implementation world. -/
def IWorld.Inv (w : IWorld) : Prop := ∀ x t, w x = some t → t.Inv

theorem lookup_map_abs (l : List (TypeId × IObj)) (ty : TypeId) :
    (l.map absEntry).lookup ty = (l.lookup ty).map IObj.abs := by
  induction l with
  | nil => rfl
  | cons e es ih =>
    obtain ⟨k, o⟩ := e
    simp only [List.map_cons, List.lookup, absEntry]
    cases h : (ty == k) <;> simp [ih]

theorem typ?_abs (t : ITenant) (ty : TypeId) : t.abs.typ? ty = (t.typ? ty).map IObj.abs := by
  unfold Tenant.typ? ITenant.typ? ITenant.abs
  exact lookup_map_abs _ _

theorem putTypeI_abs (l : List (TypeId × IObj)) (ty : TypeId) (o : IObj) :
    (putTypeI l ty o).map absEntry = putTypeList (l.map absEntry) ty o.abs := by
  induction l with
  | nil => rfl
  | cons e es ih =>
    obtain ⟨k, p⟩ := e
    simp only [putTypeI, putTypeList, List.map_cons, absEntry]
    cases h : (k == ty) <;> simp [ih, absEntry]

theorem putType_abs (t : ITenant) (ty : TypeId) (o : IObj) :
    (t.putType ty o).abs = t.abs.putType ty o.abs := by
  unfold ITenant.putType ITenant.abs Tenant.putType
  simp only [putTypeI_abs]

theorem env_abs (t : ITenant) : t.abs.env = fun T => absSlots (t.slotsOf T) := by
  funext T
  unfold Tenant.env ITenant.slotsOf
  rw [typ?_abs]
  cases t.typ? T <;> simp [IObj.abs, absSlots, ascend]

end Impl
end Strata
