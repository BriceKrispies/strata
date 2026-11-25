/-
The executable kernel, stage 1: records in slots.

An implementation tenant keeps its records in an array. Slot `i` holds the record with id `i`, or
nothing when that record was never created or has been deleted. Looking a record up by id is then
an array index, and creating one is an append, instead of the spec's list scans.

The spec's records are a list, most recently created first. `absSlots` gives the list the spec
would hold for a slot array, in the same order, so the abstraction is an ordinary function and the
refinement can be stated as an equality of outcomes and states.

Nothing here is trusted: every lemma is proved, and `Strata/Impl/Step.lean` proves the step
refines the spec. The record store operations are the ones the spec performs on its list:
lookup by id (`lookup_absSlots`), an existence test (`has_absSlots`), a scan over the records
(`anyFrom`, `any_absSlots`), a write (`absSlots_setIfInBounds`), a create (`absSlots_push`) and
the delete's two steps (`absSlots_clear`).
-/

import Strata.Model

namespace Strata
namespace Impl

/-- One tenant, its records held in slots. `slots[i]` holds the record with id `i`, or nothing.
    The invariant `ITenant.Inv` says the slot array has one slot per id below the counter. -/
structure ITenant where
  schema : Schema
  owner : PrincipalId
  editors : List PrincipalId
  writers : List PrincipalId
  slots : Array (Option Record)
  nextId : Nat
  limits : Limits := Limits.kernel
  uniques : List (List FieldId) := []

/-- The whole implementation state: each tenant id has a tenant or does not. -/
abbrev IWorld := TenantId → Option ITenant

/-! ## The abstraction -/

/-- A cell: the record a slot holds, under its id `b`, as a one-element list, or none. -/
def cellOf (b : Nat) : Option Record → Records
  | none => []
  | some r => [(b, r)]

/-- The records of a list of slots, the slot at position `i` having id `b + i`, most recent
    first. The last slot in the list is the first record in the result. -/
def ascend : Nat → List (Option Record) → Records
  | _, [] => []
  | b, o :: os => ascend (b + 1) os ++ cellOf b o

/-- The spec's records for a slot array: slot `i` is the record with id `i`. -/
def absSlots (a : Array (Option Record)) : Records := ascend 0 a.toList

/-- The spec's tenant that an implementation tenant stands for. -/
def ITenant.abs (t : ITenant) : Tenant :=
  { schema := t.schema, owner := t.owner, editors := t.editors, writers := t.writers,
    records := absSlots t.slots, nextId := t.nextId, limits := t.limits, uniques := t.uniques }

/-- The spec's world for an implementation world. -/
def absW (w : IWorld) : World := fun x => (w x).map ITenant.abs

/-- The invariant of an implementation tenant: one slot per id below the counter. -/
def ITenant.Inv (t : ITenant) : Prop := t.slots.size = t.nextId

/-- The invariant of an implementation world. -/
def IWorld.Inv (w : IWorld) : Prop := ∀ x t, w x = some t → t.Inv

/-! ## Lookup, existence and scans, on the slots -/

/-- The record with id `id`, if the slot holds one. -/
def getRec (a : Array (Option Record)) (id : RecordId) : Option Record := (a[id]?).join

/-- Whether a record with this id exists. -/
def hasI (a : Array (Option Record)) (id : RecordId) : Bool := (getRec a id).isSome

/-- Whether some slot below `n` holds a record that satisfies `P` (the record with its id). A
    loop over the slots, with no list built. -/
def anyFrom (a : Array (Option Record)) (P : RecordId × Record → Bool) : Nat → Bool
  | 0 => false
  | n + 1 => (match getRec a n with
      | some r => P (n, r)
      | none => false) || anyFrom a P n

/-- Replace slot `i` when it is in range; otherwise leave the array as it is. -/
def setSlot (a : Array (Option Record)) (i : Nat) (o : Option Record) : Array (Option Record) :=
  a.setIfInBounds i o

/-! ## Proofs about the abstraction -/

theorem ascend_append (xs ys : List (Option Record)) (b : Nat) :
    ascend b (xs ++ ys) = ascend (b + xs.length) ys ++ ascend b xs := by
  induction xs generalizing b with
  | nil => simp [ascend]
  | cons o os ih =>
    rw [List.cons_append, ascend, ih (b + 1), ascend, List.length_cons]
    have h : b + 1 + os.length = b + (os.length + 1) := by omega
    rw [h, List.append_assoc]

theorem lookup_append_nat (k : Nat) (xs ys : Records) :
    (xs ++ ys).lookup k = (xs.lookup k).or (ys.lookup k) := by
  induction xs with
  | nil => simp [List.lookup]
  | cons x xs ih =>
    obtain ⟨q, r⟩ := x
    simp only [List.cons_append, List.lookup, ih]
    cases h : (k == q) <;> simp [Option.or]

theorem lookup_cell (k b : Nat) (o : Option Record) :
    (cellOf b o).lookup k = if k = b then o else none := by
  cases o with
  | none => simp [cellOf]
  | some r =>
    by_cases h : k = b
    · subst h; simp [cellOf, List.lookup]
    · have hk : (k == b) = false := by simp [h]
      simp [cellOf, List.lookup, hk, h]

theorem lookup_ascend (k : Nat) (L : List (Option Record)) (b : Nat) :
    (ascend b L).lookup k = if b ≤ k then (L[k - b]?).join else none := by
  induction L generalizing b with
  | nil => simp [ascend]
  | cons o os ih =>
    rw [ascend, lookup_append_nat, ih (b + 1), lookup_cell]
    by_cases hkb : k = b
    · subst hkb
      have h1 : ¬ (k + 1 ≤ k) := by omega
      rw [if_neg h1, if_pos rfl, if_pos (Nat.le_refl k), Nat.sub_self]
      cases o <;> rfl
    · by_cases hb : b ≤ k
      · have h1 : b + 1 ≤ k := by omega
        have h2 : k - b = (k - (b + 1)) + 1 := by omega
        simp [hkb, hb, h1, h2]
      · have h1 : ¬ (b + 1 ≤ k) := by omega
        simp [hkb, hb, h1]

theorem lookup_absSlots (a : Array (Option Record)) (k : Nat) :
    (absSlots a).lookup k = getRec a k := by
  unfold absSlots getRec
  rw [lookup_ascend]
  simp [Array.getElem?_toList]

theorem has_absSlots (a : Array (Option Record)) (k : Nat) :
    Records.has (absSlots a) k = hasI a k := by
  unfold Records.has hasI
  rw [lookup_absSlots]

theorem mem_ascend (q : RecordId × Record) (L : List (Option Record)) (b : Nat) :
    q ∈ ascend b L ↔ ∃ i r, q = (b + i, r) ∧ L[i]? = some (some r) := by
  induction L generalizing b with
  | nil => simp [ascend]
  | cons o os ih =>
    rw [ascend, List.mem_append, ih (b + 1)]
    constructor
    · rintro (⟨i, r, hq, hi⟩ | hc)
      · refine ⟨i + 1, r, ?_, ?_⟩
        · rw [hq]; congr 1; omega
        · simpa using hi
      · cases o with
        | none => simp [cellOf] at hc
        | some x =>
          simp [cellOf] at hc
          obtain ⟨rfl, rfl⟩ := hc
          exact ⟨0, x, by simp, by simp⟩
    · rintro ⟨i, r, hq, hi⟩
      cases i with
      | zero =>
        simp at hi
        subst hi
        right
        simp [cellOf, hq]
      | succ j =>
        left
        refine ⟨j, r, ?_, ?_⟩
        · rw [hq]; congr 1; omega
        · simpa using hi

theorem mem_absSlots (q : RecordId × Record) (a : Array (Option Record)) :
    q ∈ absSlots a ↔ ∃ i r, q = (i, r) ∧ a[i]? = some (some r) := by
  unfold absSlots
  rw [mem_ascend]
  simp only [Nat.zero_add, Array.getElem?_toList]

theorem anyFrom_iff (a : Array (Option Record)) (P : RecordId × Record → Bool) (n : Nat) :
    anyFrom a P n = true ↔ ∃ i r, i < n ∧ a[i]? = some (some r) ∧ P (i, r) = true := by
  induction n with
  | zero => simp [anyFrom]
  | succ n ih =>
    simp only [anyFrom, Bool.or_eq_true, ih]
    constructor
    · rintro (h | ⟨i, r, hi, hr, hp⟩)
      · unfold getRec at h
        cases hc : a[n]? with
        | none => simp [hc] at h
        | some o =>
          cases o with
          | none => simp [hc] at h
          | some r =>
            simp [hc] at h
            exact ⟨n, r, by omega, by simp [hc], h⟩
      · exact ⟨i, r, by omega, hr, hp⟩
    · rintro ⟨i, r, hi, hr, hp⟩
      by_cases hin : i = n
      · subst hin
        left
        unfold getRec
        simp [hr, hp]
      · right
        exact ⟨i, r, by omega, hr, hp⟩

theorem any_absSlots (a : Array (Option Record)) (P : RecordId × Record → Bool) :
    (absSlots a).any P = anyFrom a P a.size := by
  rw [Bool.eq_iff_iff, List.any_eq_true, anyFrom_iff]
  constructor
  · rintro ⟨q, hq, hp⟩
    obtain ⟨i, r, rfl, hi⟩ := (mem_absSlots q a).mp hq
    have hlt : i < a.size := by
      obtain ⟨h, _⟩ := Array.getElem?_eq_some_iff.mp hi
      exact h
    exact ⟨i, r, hlt, hi, hp⟩
  · rintro ⟨i, r, _, hi, hp⟩
    exact ⟨(i, r), (mem_absSlots (i, r) a).mpr ⟨i, r, rfl, hi⟩, hp⟩

theorem ids_ge (b : Nat) (L : List (Option Record)) (q : RecordId × Record)
    (hq : q ∈ ascend b L) : b ≤ q.1 := by
  obtain ⟨i, r, rfl, _⟩ := (mem_ascend q L b).mp hq
  simp

theorem ascend_replicate (n : Nat) : ∀ b, ascend b (List.replicate n none) = [] := by
  induction n with
  | zero => intro b; simp [ascend]
  | succ n ih => intro b; simp [List.replicate_succ, ascend, cellOf, ih]

theorem absSlots_replicate (n : Nat) : absSlots (Array.replicate n none) = [] := by
  unfold absSlots
  rw [Array.toList_replicate, ascend_replicate]

theorem absSlots_push (a : Array (Option Record)) (r : Record) :
    absSlots (a.push (some r)) = (a.size, r) :: absSlots a := by
  unfold absSlots
  rw [Array.toList_push, ascend_append, Array.length_toList]
  simp [ascend, cellOf]

theorem beq_false_of_ne' {a b : Nat} (h : a ≠ b) : (a == b) = false := by
  simp [h]

/-- Writing a slot that holds a record: the spec's update of that record in place. -/
theorem ascend_set (k : Nat) (r : Record) (L : List (Option Record)) (b : Nat) (hb : b ≤ k)
    (hc : ∃ x, L[k - b]? = some (some x)) :
    ascend b (L.set (k - b) (some r)) =
      (ascend b L).map (fun q => if q.1 == k then (q.1, r) else q) := by
  induction L generalizing b with
  | nil => simp at hc
  | cons o os ih =>
    by_cases hkb : k = b
    · subst hkb
      rw [Nat.sub_self] at hc ⊢
      obtain ⟨x, hx⟩ := hc
      simp only [List.getElem?_cons_zero, Option.some.injEq] at hx
      subst hx
      rw [List.set_cons_zero, ascend, ascend, List.map_append]
      congr 1
      · have hmap : (ascend (k + 1) os).map (fun q => if q.1 == k then (q.1, r) else q) =
            (ascend (k + 1) os).map id := by
          apply List.map_congr_left
          intro q hq
          have h1 := ids_ge (k + 1) os q hq
          have h2 : (q.1 == k) = false := beq_false_of_ne' (by omega)
          simp [h2]
        rw [hmap, List.map_id]
      · simp [cellOf]
    · have hk1 : b + 1 ≤ k := by omega
      have hsub : k - b = (k - (b + 1)) + 1 := by omega
      rw [hsub] at hc ⊢
      rw [List.set_cons_succ, ascend, ascend, List.map_append]
      congr 1
      · apply ih (b + 1) hk1
        obtain ⟨x, hx⟩ := hc
        exact ⟨x, by simpa using hx⟩
      · have hne : (b == k) = false := beq_false_of_ne' (fun h => hkb h.symm)
        cases o with
        | none => simp [cellOf]
        | some x =>
          have hne' : b ≠ k := fun h => hkb h.symm
          simp [cellOf, hne']

theorem absSlots_setIfInBounds (a : Array (Option Record)) (k : Nat) (r : Record) (x : Record)
    (hc : a[k]? = some (some x)) :
    absSlots (a.setIfInBounds k (some r)) =
      (absSlots a).map (fun q => if q.1 == k then (q.1, r) else q) := by
  unfold absSlots
  rw [Array.toList_setIfInBounds]
  have hc' : ∃ y, a.toList[k - 0]? = some (some y) := by
    refine ⟨x, ?_⟩
    rw [Nat.sub_zero, Array.getElem?_toList, hc]
  have key := ascend_set k r a.toList 0 (Nat.zero_le k) hc'
  rw [Nat.sub_zero] at key
  exact key

theorem ascend_map (g : Record → Record) (L : List (Option Record)) (b : Nat) :
    ascend b (L.map (Option.map g)) = (ascend b L).map (fun q => (q.1, g q.2)) := by
  induction L generalizing b with
  | nil => simp [ascend]
  | cons o os ih =>
    rw [List.map_cons, ascend, ascend, List.map_append, ih]
    congr 1
    cases o with
    | none => simp [cellOf]
    | some x => simp [cellOf]

/-- Clearing a slot: the records below it keep their ids, and the one with id `k` is gone. -/
theorem ascend_clear (k : Nat) (L : List (Option Record)) (b : Nat) (hb : b ≤ k) :
    ascend b (L.set (k - b) none) = (ascend b L).filter (fun q => q.1 != k) := by
  induction L generalizing b with
  | nil => simp [ascend]
  | cons o os ih =>
    by_cases hkb : k = b
    · subst hkb
      rw [Nat.sub_self, List.set_cons_zero, ascend, ascend, List.filter_append]
      have hkeep : List.filter (fun q => q.1 != k) (ascend (k + 1) os) = ascend (k + 1) os := by
        rw [List.filter_eq_self]
        intro q hq
        have h1 := ids_ge (k + 1) os q hq
        simp only [bne_iff_ne, ne_eq]
        omega
      rw [hkeep]
      cases o with
      | none => simp [cellOf]
      | some x => simp [cellOf]
    · have hk1 : b + 1 ≤ k := by omega
      have hsub : k - b = (k - (b + 1)) + 1 := by omega
      rw [hsub, List.set_cons_succ, ascend, ascend, List.filter_append]
      congr 1
      · exact ih (b + 1) hk1
      · have hne : (b != k) = true := by
          simp only [bne_iff_ne, ne_eq]
          exact fun h => hkb h.symm
        cases o with
        | none => simp [cellOf]
        | some x => simp [cellOf, hne]

/-- The delete's slot update: every record loses its references to `k` (`g`), and slot `k` is
    cleared. The spec's delete filters out the record `k` and maps the others. -/
theorem absSlots_clear (a : Array (Option Record)) (k : Nat) (g : Record → Record) :
    absSlots ((a.map (Option.map g)).setIfInBounds k none) =
      ((absSlots a).filter (fun q => q.1 != k)).map (fun q => (q.1, g q.2)) := by
  unfold absSlots
  have key := ascend_clear k (a.toList.map (Option.map g)) 0 (Nat.zero_le k)
  rw [Nat.sub_zero] at key
  rw [Array.toList_setIfInBounds, Array.toList_map, key, ascend_map, List.filter_map]
  rfl

/-! ## The abstraction of a fresh tenant -/

/-- A tenant with no records, as the implementation starts it: one empty slot per id below the
    counter. -/
def ITenant.ofEmpty (t : Tenant) : ITenant :=
  { schema := t.schema, owner := t.owner, editors := t.editors, writers := t.writers,
    slots := Array.replicate t.nextId none, nextId := t.nextId, limits := t.limits,
    uniques := t.uniques }

theorem ofEmpty_abs (t : Tenant) (h : t.records = []) : (ITenant.ofEmpty t).abs = t := by
  cases t
  simp only at h
  subst h
  unfold ITenant.abs ITenant.ofEmpty absSlots
  rw [Array.toList_replicate, ascend_replicate]

theorem ofEmpty_inv (t : Tenant) : (ITenant.ofEmpty t).Inv := by
  unfold ITenant.Inv ITenant.ofEmpty
  simp

end Impl
end Strata
