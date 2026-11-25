/-
The executable kernel, stage 2: lemmas about the object-type list and about changing one cell.
-/

import Strata.Impl.XStep

namespace Strata
namespace Impl

theorem modifyTypeI_eq (l : List (TypeId × IObj)) (ty : TypeId) (f : IObj → IObj) (o : IObj)
    (h : l.lookup ty = some o) : modifyTypeI l ty f = putTypeI l ty (f o) := by
  induction l with
  | nil => simp [List.lookup] at h
  | cons e es ih =>
    obtain ⟨k, p⟩ := e
    by_cases hk : k = ty
    · subst hk
      simp only [modifyTypeI, putTypeI, beq_self_eq_true, if_true]
      have : ((k, p) :: es).lookup k = some p := lookup_cons_eq k p es
      rw [this] at h
      cases h
      rfl
    · have hkb : (k == ty) = false := by simp [hk]
      have htk : (ty == k) = false := by simp [Ne.symm hk]
      simp only [modifyTypeI, putTypeI, hkb, Bool.false_eq_true, if_false]
      rw [lookup_cons_ne _ _ _ _ htk] at h
      rw [ih h]

theorem ITenant.modifyType_eq (t : ITenant) (ty : TypeId) (f : IObj → IObj) (o : IObj)
    (h : t.typ? ty = some o) : t.modifyType ty f = t.putType ty (f o) := by
  unfold ITenant.modifyType ITenant.putType
  rw [modifyTypeI_eq _ _ _ _ h]

theorem putTypeI_keys (l : List (TypeId × IObj)) (ty : TypeId) (o : IObj) :
    (putTypeI l ty o).map Prod.fst = l.map Prod.fst := by
  induction l with
  | nil => rfl
  | cons e es ih =>
    obtain ⟨k, p⟩ := e
    by_cases hk : k = ty
    · subst hk; simp [putTypeI]
    · have hkb : (k == ty) = false := by simp [hk]
      simp [putTypeI, hkb, ih]

theorem typ?_putType (t : ITenant) (ty : TypeId) (o : IObj) (T : TypeId) :
    (t.putType ty o).typ? T = if T == ty then (if (t.typ? ty).isSome then some o else none)
      else t.typ? T := by
  unfold ITenant.putType ITenant.typ?
  exact lookup_putTypeI _ _ _ _

theorem mem_of_lookup {α : Type} (l : List (TypeId × α)) (k : TypeId) (v : α)
    (h : l.lookup k = some v) : (k, v) ∈ l := by
  induction l with
  | nil => simp [List.lookup] at h
  | cons e es ih =>
    obtain ⟨k', p⟩ := e
    by_cases hk : k = k'
    · subst hk
      rw [lookup_cons_eq] at h
      cases h
      exact List.mem_cons_self
    · have : (k == k') = false := by simp [hk]
      rw [lookup_cons_ne _ _ _ _ this] at h
      exact List.mem_cons_of_mem _ (ih h)

theorem lookup_of_mem_nodup {α : Type} (l : List (TypeId × α)) (e : TypeId × α) (he : e ∈ l)
    (hnd : (l.map Prod.fst).Nodup) : l.lookup e.1 = some e.2 := by
  induction l with
  | nil => simp at he
  | cons e' es ih =>
    obtain ⟨k', p⟩ := e'
    obtain ⟨k, v⟩ := e
    simp only [List.map_cons, List.nodup_cons] at hnd
    rcases List.mem_cons.mp he with h | h
    · cases h; exact lookup_cons_eq _ _ _
    · have hne : k ≠ k' := by
        intro hk; subst hk
        exact hnd.1 (List.mem_map.mpr ⟨(k, v), h, rfl⟩)
      have : (k == k') = false := by simp [hne]
      rw [lookup_cons_ne _ _ _ _ this]
      exact ih h hnd.2

theorem getRec_of_ge (a : Array (Option Record)) (i : Nat) (h : a.size ≤ i) : getRec a i = none := by
  unfold getRec
  simp [Array.getElem?_eq_none h]

theorem getRec_of_lt (a : Array (Option Record)) (i : Nat) (h : i < a.size) : getRec a i = a[i] := by
  unfold getRec
  simp [h]

/-- Replacing the slots of one object type, changing at most the slot `id`, is a change of that
    cell. -/
theorem cc_putType (t : ITenant) (ty : TypeId) (id : RecordId) (new : Option Record) (o o' : IObj)
    (ho : t.typ? ty = some o) (hs : o'.schema = o.schema) (hu : o'.uniques = o.uniques)
    (hother : ∀ i, i ≠ id → getRec o'.slots i = getRec o.slots i) (hnew : getRec o'.slots id = new) :
    CellChange t (t.putType ty o') ty id new := by
  refine ⟨?_, ?_, o, o', ho, ?_, hs, hu, hother, hnew⟩
  · exact putTypeI_keys _ _ _
  · intro T hT
    rw [typ?_putType]
    have : (T == ty) = false := by simp [hT]
    simp [this]
  · rw [typ?_putType]
    simp [ho]

theorem CellChange.withNextId {t t' : ITenant} {ty : TypeId} {id : RecordId} {new : Option Record}
    (h : CellChange t t' ty id new) (n : Nat) : CellChange t { t' with nextId := n } ty id new :=
  ⟨h.keys, h.other, h.here⟩

theorem lt_of_getRec (a : Array (Option Record)) (i : Nat) (r : Record) (h : getRec a i = some r) :
    i < a.size := (Array.getElem?_eq_some_iff.mp (getRec_some a i r h)).1

end Impl
end Strata
