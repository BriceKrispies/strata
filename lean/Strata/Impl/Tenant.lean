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
  constraints : List Expr := []

/-- One tenant, each type's records held in slots. -/
structure ITenant where
  types : List (TypeId × IObj)
  members : List (PrincipalId × List RoleId)
  grants : List (RoleId × Grant)
  policies : List Policy
  nextId : Nat
  limits : Limits := Limits.kernel

/-- The whole implementation state: each tenant id has a tenant or does not. -/
abbrev IWorld := TenantId → Option ITenant

/-- The spec's object type for an implementation one. -/
def IObj.abs (o : IObj) : ObjType :=
  { schema := o.schema, uniques := o.uniques, records := absSlots o.slots,
    constraints := o.constraints }

theorem IObj.abs_schema (o : IObj) (s : Schema) : { o with schema := s }.abs = { o.abs with schema := s } :=
  rfl

theorem IObj.abs_uniques (o : IObj) (u : List (List FieldId)) :
    { o with uniques := u }.abs = { o.abs with uniques := u } := rfl

/-- The spec's entry for an implementation entry. -/
def absEntry (e : TypeId × IObj) : TypeId × ObjType := (e.1, e.2.abs)

/-- The spec's tenant that an implementation tenant stands for. -/
def ITenant.abs (t : ITenant) : Tenant :=
  { types := t.types.map absEntry, members := t.members, grants := t.grants, policies := t.policies,
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

theorem lookup_cons_ne {α : Type} (T k : TypeId) (p : α) (es : List (TypeId × α))
    (h : (T == k) = false) : ((k, p) :: es).lookup T = es.lookup T := by
  simp [List.lookup, h]

theorem lookup_cons_eq {α : Type} (k : TypeId) (p : α) (es : List (TypeId × α)) :
    ((k, p) :: es).lookup k = some p := by
  simp [List.lookup]

theorem lookup_putTypeI (l : List (TypeId × IObj)) (ty : TypeId) (o : IObj) (T : TypeId) :
    (putTypeI l ty o).lookup T = if T == ty then (if (l.lookup ty).isSome then some o else none)
      else l.lookup T := by
  induction l with
  | nil => simp [putTypeI]
  | cons e es ih =>
    obtain ⟨k, p⟩ := e
    by_cases hk : k = ty
    · subst hk
      by_cases hT : T = k
      · subst hT; simp [putTypeI, lookup_cons_eq]
      · have hTk : (T == k) = false := by simp [hT]
        simp [putTypeI, hTk, lookup_cons_ne _ _ _ _ hTk, hT]
    · have hkb : (k == ty) = false := by simp [hk]
      simp only [putTypeI, hkb, Bool.false_eq_true, if_false]
      by_cases hT : T = k
      · subst hT
        have hTy : (T == ty) = false := by simpa using hk
        simp [hTy, lookup_cons_eq]
      · have hTk : (T == k) = false := by simp [hT]
        rw [lookup_cons_ne _ _ _ _ hTk, ih]
        by_cases hTy : T = ty
        · subst hTy
          rw [lookup_cons_ne _ _ _ _ hTk]
        · simp [hTy, lookup_cons_ne _ _ _ _ hTk]

theorem inv_putType (t : ITenant) (ty : TypeId) (o : IObj) (n : Nat) (hn : t.nextId ≤ n)
    (ho : o.slots.size ≤ n) (hinv : t.Inv) :
    ∀ T q, (t.putType ty o).typ? T = some q → q.slots.size ≤ n := by
  intro T q hq
  unfold ITenant.putType ITenant.typ? at hq
  simp only [lookup_putTypeI] at hq
  by_cases hT : (T == ty) = true
  · simp only [hT, if_true] at hq
    split at hq
    · simp at hq; subst hq; exact ho
    · cases hq
  · simp only [hT, Bool.false_eq_true, if_false] at hq
    have := hinv T q hq
    omega

theorem lookup_map_entry {α β : Type} (l : List (TypeId × α)) (g : TypeId → α → β) (T : TypeId) :
    (l.map (fun e => (e.1, g e.1 e.2))).lookup T = (l.lookup T).map (g T) := by
  induction l with
  | nil => rfl
  | cons e es ih =>
    obtain ⟨k, p⟩ := e
    simp only [List.map_cons, List.lookup]
    by_cases hT : T = k
    · subst hT; simp
    · have : (T == k) = false := by simp [hT]
      simp [this, ih]

theorem getRec_setSlot (a : Array (Option Record)) (id : RecordId) (v : Option Record)
    (hb : id < a.size) (i : RecordId) :
    getRec (setSlot a id v) i = if i = id then v else getRec a i := by
  unfold getRec setSlot
  rw [Array.getElem?_setIfInBounds]
  by_cases h : id = i
  · subst h; simp [hb]
  · have h' : ¬ i = id := fun e => h e.symm
    simp [h, h']

theorem getRec_push_padTo (a : Array (Option Record)) (n : Nat) (r : Record) (h : a.size ≤ n)
    (i : RecordId) :
    getRec ((padTo a n).push (some r)) i = if i = n then some r else getRec a i := by
  unfold getRec padTo
  rw [Array.getElem?_push]
  have hs : (a ++ Array.replicate (n - a.size) none).size = n := by simp; omega
  rw [hs]
  by_cases hi : i = n
  · subst hi; simp
  · simp only [hi, if_false]
    rw [Array.getElem?_append]
    by_cases hl : i < a.size
    · simp [hl]
    · have : a[i]? = none := by simp [Array.getElem?_eq_none, Nat.not_lt.mp hl]
      simp [hl, this]
      by_cases hr : i - a.size < n - a.size
      · simp [Array.getElem?_replicate, hr]
      · simp [hr]

theorem size_setSlot (a : Array (Option Record)) (i : Nat) (v : Option Record) :
    (setSlot a i v).size = a.size := by
  simp [setSlot]

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

end Impl
end Strata
