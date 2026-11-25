/-
The executable kernel, stage 2: the indexed tenant and what its indexes mean.

An indexed tenant (`XTenant`) is a stage 1 tenant (`core`, the records in slots) together with three
indexes, each kept consistent by every accepted step:

* `uidx`, the unique-value index. Key `(type, field, value)`; it lists the cells of that type whose
  record holds `value`, keyed, in `field`, for each field the schema declares unique. A clash check
  is a lookup.
* `cidx`, the composite index. Key `(type, constraint, values)`; it lists the cells of that type whose
  record holds all the constraint's fields, keyed, with those values, for each composite constraint
  of the type.
* `ridx`, the referrer index. Key `(target type, target id)`; it lists the cells whose record names
  that target through a field of a reference type (alone or as a list element). A delete asks it which
  records block the delete and which hold a reference to clear.

What an index must hold is given by a key function per cell (`ITenant.ukeys`, `ckeys`, `rkeys`): the
keys of the record in the cell, under the schema of its type. `XTenant.Ok` says each index is exactly
that relation (`MM.Inv`), together with the slot invariant of stage 1 and that the object types have
distinct ids (the stage 1 lookup finds the first entry of a type; the spec's scans visit every entry).
-/

import Strata.Impl.Problem
import Strata.Impl.XIndex
import Strata.References

namespace Strata
namespace Impl

open Std

/-! ## The keys of a record -/

/-- The record ids a value names: a reference, or the references in a list. -/
def refIds : Val → List RecordId
  | .ref x => [x]
  | .list vs => vs.filterMap (fun e => match e with
      | .ref x => some x
      | _ => none)
  | _ => []

theorem mem_refIds (id : RecordId) (v : Val) : id ∈ refIds v ↔ v.refersTo id = true := by
  cases v with
  | ref x =>
    simp only [refIds, Val.refersTo, List.mem_singleton, beq_iff_eq]
    exact ⟨fun h => h.symm, fun h => h.symm⟩
  | list vs =>
    simp only [refIds, Val.refersTo, List.mem_filterMap, List.contains_eq_any_beq, List.any_eq_true,
      beq_iff_eq]
    constructor
    · rintro ⟨e, he, h⟩
      cases e with
      | ref x => simp at h; subst h; exact ⟨_, he, rfl⟩
      | _ => simp at h
    · rintro ⟨e, he, h⟩
      subst h
      exact ⟨_, he, rfl⟩
  | _ => simp [refIds, Val.refersTo]

/-- The unique-index keys of a record of type `S`: for each field the schema declares unique, in
    which the record holds a keyed value, the type, the field and the value. -/
def ukeysOf (S : TypeId) (schema : Schema) (r : Record) : List (TypeId × FieldId × Val) :=
  schema.filterMap (fun fd =>
    if fd.2.unique && (r.fields.get fd.1).keyed then some (S, fd.1, r.fields.get fd.1) else none)

/-- The composite-index keys of a record of type `S`: for each constraint all of whose fields the
    record holds keyed, the type, the constraint and the values. -/
def ckeysOf (S : TypeId) (us : List (List FieldId)) (r : Record) :
    List (TypeId × List FieldId × List Val) :=
  us.filterMap (fun u =>
    if u.all (fun f => (r.fields.get f).keyed) then some (S, u, u.map r.fields.get) else none)

/-- The referrer-index keys of a record of a type with schema `schema`: the targets (type and id) it
    names through its reference fields. -/
def rkeysOf (schema : Schema) (r : Record) : List Cell :=
  r.fields.flatMap (fun fv => match schema.lookup fv.1 with
    | some d => (match d.ty.target? with
        | some t => (refIds fv.2).map (fun x => (t, x))
        | none => [])
    | none => [])

theorem mem_ukeysOf (S : TypeId) (schema : Schema) (r : Record) (k : TypeId × FieldId × Val) :
    k ∈ ukeysOf S schema r ↔ k.1 = S ∧ ∃ fd ∈ schema, fd.1 = k.2.1 ∧ fd.2.unique = true ∧
      (r.fields.get k.2.1).keyed = true ∧ r.fields.get k.2.1 = k.2.2 := by
  obtain ⟨S', f, v⟩ := k
  unfold ukeysOf
  simp only [List.mem_filterMap]
  constructor
  · rintro ⟨fd, hfd, h⟩
    split at h
    · rename_i hc
      simp only [Option.some.injEq, Prod.mk.injEq] at h
      obtain ⟨h1, h2, h3⟩ := h
      simp only [Bool.and_eq_true] at hc
      subst h1; subst h2; subst h3
      exact ⟨rfl, fd, hfd, rfl, hc.1, hc.2, rfl⟩
    · cases h
  · rintro ⟨h1, fd, hfd, h2, h3, h4, h5⟩
    refine ⟨fd, hfd, ?_⟩
    try dsimp only at h1 h2 h3 h4 h5
    subst h1; subst h2; subst h5
    simp [h3, h4]

theorem mem_ckeysOf (S : TypeId) (us : List (List FieldId)) (r : Record)
    (k : TypeId × List FieldId × List Val) :
    k ∈ ckeysOf S us r ↔ k.1 = S ∧ k.2.1 ∈ us ∧ k.2.1.all (fun f => (r.fields.get f).keyed) = true ∧
      k.2.2 = k.2.1.map r.fields.get := by
  obtain ⟨S', u, vs⟩ := k
  unfold ckeysOf
  simp only [List.mem_filterMap]
  constructor
  · rintro ⟨u', hu', h⟩
    split at h
    · rename_i hc
      simp only [Option.some.injEq, Prod.mk.injEq] at h
      obtain ⟨h1, h2, h3⟩ := h
      subst h1; subst h2; subst h3
      exact ⟨rfl, hu', hc, rfl⟩
    · cases h
  · rintro ⟨h1, hu, h3, h4⟩
    try dsimp only at h1 hu h3 h4
    subst h1; subst h4
    refine ⟨u, hu, ?_⟩
    simp [h3]

theorem mem_rkeysOf (schema : Schema) (r : Record) (k : Cell) :
    k ∈ rkeysOf schema r ↔ recRefers schema k.1 k.2 r = true := by
  obtain ⟨t, id⟩ := k
  unfold rkeysOf recRefers
  simp only [List.mem_flatMap, List.any_eq_true]
  constructor
  · rintro ⟨fv, hfv, hm⟩
    refine ⟨fv, hfv, ?_⟩
    unfold fieldRefers
    cases hl : schema.lookup fv.1 with
    | none => simp [hl] at hm
    | some d =>
      simp only [hl] at hm ⊢
      cases ht : d.ty.target? with
      | none => simp [ht] at hm
      | some t' =>
        simp only [ht, List.mem_map, Prod.mk.injEq] at hm ⊢
        obtain ⟨x, hx, h1, h2⟩ := hm
        subst h1; subst h2
        simp [(mem_refIds _ _).mp hx]
  · rintro ⟨fv, hfv, h⟩
    refine ⟨fv, hfv, ?_⟩
    unfold fieldRefers at h
    cases hl : schema.lookup fv.1 with
    | none => simp [hl] at h
    | some d =>
      simp only [hl, Bool.and_eq_true, beq_iff_eq] at h
      simp only [hl, h.1, List.mem_map]
      exact ⟨id, (mem_refIds _ _).mpr h.2, rfl⟩

/-! ## The keys of a cell -/

/-- The object type and the record of a cell, if the tenant holds one there. -/
def ITenant.cellRec (t : ITenant) (c : Cell) : Option (IObj × Record) :=
  match t.typ? c.1 with
  | some o => (getRec o.slots c.2).map (fun r => (o, r))
  | none => none

/-- The unique-index keys of a cell. -/
def ITenant.ukeys (t : ITenant) (c : Cell) : List (TypeId × FieldId × Val) :=
  match t.cellRec c with
  | some (o, r) => ukeysOf c.1 o.schema r
  | none => []

/-- The composite-index keys of a cell. -/
def ITenant.ckeys (t : ITenant) (c : Cell) : List (TypeId × List FieldId × List Val) :=
  match t.cellRec c with
  | some (o, r) => ckeysOf c.1 o.uniques r
  | none => []

/-- The referrer-index keys of a cell. -/
def ITenant.rkeys (t : ITenant) (c : Cell) : List Cell :=
  match t.cellRec c with
  | some (o, r) => rkeysOf o.schema r
  | none => []

/-- The unique-index keys of a possibly absent record. -/
def okeysU (S : TypeId) (schema : Schema) : Option Record → List (TypeId × FieldId × Val)
  | some r => ukeysOf S schema r
  | none => []

/-- The composite-index keys of a possibly absent record. -/
def okeysC (S : TypeId) (us : List (List FieldId)) :
    Option Record → List (TypeId × List FieldId × List Val)
  | some r => ckeysOf S us r
  | none => []

/-- The referrer-index keys of a possibly absent record. -/
def okeysR (schema : Schema) : Option Record → List Cell
  | some r => rkeysOf schema r
  | none => []

/-- The tenant with the indexes. -/
structure XTenant where
  core : ITenant
  uidx : MM (TypeId × FieldId × Val)
  cidx : MM (TypeId × List FieldId × List Val)
  ridx : MM Cell

/-- The consistency invariant: the slot invariant, one entry per object type id, and each index
    exactly the relation its key function gives. -/
def XTenant.Ok (x : XTenant) : Prop :=
  x.core.Inv ∧ (x.core.types.map Prod.fst).Nodup ∧ x.uidx.Inv x.core.ukeys ∧
    x.cidx.Inv x.core.ckeys ∧ x.ridx.Inv x.core.rkeys

/-! ## A change of one cell -/

/-- Tenant `t'` is `t` with the record of cell `(ty, id)` replaced by `new`: the same object types,
    the same schemas and constraints, the same records in every other cell. -/
structure CellChange (t t' : ITenant) (ty : TypeId) (id : RecordId) (new : Option Record) : Prop where
  keys : t'.types.map Prod.fst = t.types.map Prod.fst
  other : ∀ T, T ≠ ty → t'.typ? T = t.typ? T
  here : ∃ o o', t.typ? ty = some o ∧ t'.typ? ty = some o' ∧ o'.schema = o.schema ∧
    o'.uniques = o.uniques ∧ (∀ i, i ≠ id → getRec o'.slots i = getRec o.slots i) ∧
    getRec o'.slots id = new

/-- The keys a key function gives a cell. -/
def ITenant.keysBy {K : Type} (kf : TypeId → IObj → Record → List K) (t : ITenant) (c : Cell) : List K :=
  match t.cellRec c with
  | some (o, r) => kf c.1 o r
  | none => []

/-- Replacing one cell's record changes only that cell's keys, to those of the new record under the
    type's schema. -/
theorem CellChange.keysBy {K : Type} (kf : TypeId → IObj → Record → List K)
    (hkf : ∀ S o o' r, o'.schema = o.schema → o'.uniques = o.uniques → kf S o' r = kf S o r)
    {t t' : ITenant} {ty : TypeId} {id : RecordId} {new : Option Record}
    (h : CellChange t t' ty id new) (o : IObj) (ho : t.typ? ty = some o) (c : Cell) :
    t'.keysBy kf c = if c = (ty, id) then (match new with | some r => kf ty o r | none => [])
      else t.keysBy kf c := by
  obtain ⟨o0, o', h0, h', hs, hu, hother, hnew⟩ := h.here
  rw [ho] at h0
  cases h0
  obtain ⟨T, i⟩ := c
  unfold ITenant.keysBy ITenant.cellRec
  by_cases hT : T = ty
  · subst hT
    rw [h', ho]
    by_cases hi : i = id
    · subst hi
      simp only [if_true, hnew]
      cases new <;> simp [hkf _ _ _ _ hs hu]
    · have : ¬ ((T, i) = (T, id)) := by simp [hi]
      simp only [this, if_false, hother i hi]
      cases getRec o.slots i <;> simp [hkf _ _ _ _ hs hu]
  · have : ¬ ((T, i) = (ty, id)) := by simp [hT]
    simp only [this, if_false, h.other T hT]

theorem ITenant.keysBy_at {K : Type} (kf : TypeId → IObj → Record → List K) (t : ITenant)
    (ty : TypeId) (id : RecordId) (o : IObj) (old : Option Record) (ho : t.typ? ty = some o)
    (hold : getRec o.slots id = old) :
    t.keysBy kf (ty, id) = (match old with | some r => kf ty o r | none => []) := by
  unfold ITenant.keysBy ITenant.cellRec
  simp only [ho, hold]
  cases old <;> rfl

theorem ukf_congr (S : TypeId) (o o' : IObj) (r : Record) (hs : o'.schema = o.schema)
    (_hu : o'.uniques = o.uniques) : ukeysOf S o'.schema r = ukeysOf S o.schema r := by rw [hs]

theorem ckf_congr (S : TypeId) (o o' : IObj) (r : Record) (_hs : o'.schema = o.schema)
    (hu : o'.uniques = o.uniques) : ckeysOf S o'.uniques r = ckeysOf S o.uniques r := by rw [hu]

theorem rkf_congr (_S : TypeId) (o o' : IObj) (r : Record) (hs : o'.schema = o.schema)
    (_hu : o'.uniques = o.uniques) : rkeysOf o'.schema r = rkeysOf o.schema r := by rw [hs]

/-- The indexed tenant after the record of cell `(ty, id)`, in a type with schema `sch` and
    constraints `us`, goes from `old` to `new`: its old keys are taken off the indexes and its new
    ones listed. The tenant is `core'`. -/
def XTenant.reindex (core' : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell) (ty : TypeId) (id : RecordId)
    (sch : Schema) (us : List (List FieldId)) (old new : Option Record) : XTenant :=
  { core := core',
    uidx := (ux.removeAll (okeysU ty sch old) (ty, id)).addAll (okeysU ty sch new) (ty, id),
    cidx := (cx.removeAll (okeysC ty us old) (ty, id)).addAll (okeysC ty us new) (ty, id),
    ridx := (rx.removeAll (okeysR sch old) (ty, id)).addAll (okeysR sch new) (ty, id) }

/-- Replacing one cell's record, with the indexes updated by `reindex`, keeps the invariant. -/
theorem XTenant.ok_reindex (x : XTenant) (core' : ITenant) (ty : TypeId) (id : RecordId) (o : IObj)
    (old new : Option Record) (hok : x.Ok) (hch : CellChange x.core core' ty id new)
    (ho : x.core.typ? ty = some o) (hold : getRec o.slots id = old) (hinv : core'.Inv) :
    (XTenant.reindex core' x.uidx x.cidx x.ridx ty id o.schema o.uniques old new).Ok := by
  obtain ⟨_, hnd, hu, hc, hr⟩ := hok
  refine ⟨hinv, ?_, ?_, ?_, ?_⟩
  · show (core'.types.map Prod.fst).Nodup
    rw [hch.keys]; exact hnd
  · have h1 := ITenant.keysBy_at (fun S o r => ukeysOf S o.schema r) x.core ty id o old ho hold
    have h2 := hch.keysBy (fun S o r => ukeysOf S o.schema r) (ukf_congr) o ho
    have e : x.core.ukeys (ty, id) = okeysU ty o.schema old := by
      cases old <;> exact h1
    have := MM.inv_change x.uidx x.core.ukeys core'.ukeys (ty, id) (okeysU ty o.schema new) hu
      (fun c => by
        show core'.keysBy (fun S o r => ukeysOf S o.schema r) c = _
        rw [h2 c]; cases new <;> rfl)
    rw [e] at this
    exact this
  · have h1 := ITenant.keysBy_at (fun S o r => ckeysOf S o.uniques r) x.core ty id o old ho hold
    have h2 := hch.keysBy (fun S o r => ckeysOf S o.uniques r) (ckf_congr) o ho
    have e : x.core.ckeys (ty, id) = okeysC ty o.uniques old := by
      cases old <;> exact h1
    have := MM.inv_change x.cidx x.core.ckeys core'.ckeys (ty, id) (okeysC ty o.uniques new) hc
      (fun c => by
        show core'.keysBy (fun S o r => ckeysOf S o.uniques r) c = _
        rw [h2 c]; cases new <;> rfl)
    rw [e] at this
    exact this
  · have h1 := ITenant.keysBy_at (fun S o r => rkeysOf o.schema r) x.core ty id o old ho hold
    have h2 := hch.keysBy (fun S o r => rkeysOf o.schema r) (rkf_congr) o ho
    have e : x.core.rkeys (ty, id) = okeysR o.schema old := by
      cases old <;> exact h1
    have := MM.inv_change x.ridx x.core.rkeys core'.rkeys (ty, id) (okeysR o.schema new) hr
      (fun c => by
        show core'.keysBy (fun S o r => rkeysOf o.schema r) c = _
        rw [h2 c]; cases new <;> rfl)
    rw [e] at this
    exact this

end Impl
end Strata
