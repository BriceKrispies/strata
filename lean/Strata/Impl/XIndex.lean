/-
The executable kernel, stage 2: the index structure.

An index here is a hash map from a key to the cells (object type and record id) that hold it, with
the operations the kernel's steps need: add a cell under a key, remove it, and the same for a list of
keys. `MM.Inv m ck` says the map is exactly the relation `ck`: a cell is listed under a key if and
only if the key is among the cell's keys. The three indexes of the kernel (unique values, composite
constraints, referrers) are three instances of it, each with its own key function.

The lemmas here are about the structure only: how a change of one cell's keys (`inv_change`) and a
rebuild over a list of cells (`inv_addCells`) keep the invariant. Nothing here mentions records.
-/

import Std.Data.HashMap
import Strata.Model

namespace Strata
namespace Impl

open Std

/-- A cell: an object type and a record id. -/
abbrev Cell := TypeId × RecordId

deriving instance Hashable for Elem
deriving instance Hashable for Val

instance : LawfulHashable Val :=
  ⟨fun a b h => by have := eq_of_beq h; subst this; rfl⟩

/-- An index: each key lists the cells that hold it. -/
abbrev MM (K : Type) [BEq K] [Hashable K] := HashMap K (List Cell)

section
variable {K : Type} [BEq K] [Hashable K] [LawfulBEq K] [LawfulHashable K]

/-- The cells listed under a key. -/
def MM.get (m : MM K) (k : K) : List Cell := m.getD k []

/-- List a cell under a key. -/
def MM.add (m : MM K) (k : K) (c : Cell) : MM K := m.insert k (c :: m.get k)

/-- Take a cell off a key; a key left with no cell is dropped. -/
def MM.remove (m : MM K) (k : K) (c : Cell) : MM K :=
  match (m.get k).filter (fun d => d != c) with
  | [] => m.erase k
  | l => m.insert k l

/-- List a cell under each of the keys. -/
def MM.addAll (m : MM K) (ks : List K) (c : Cell) : MM K := ks.foldl (fun m k => m.add k c) m

/-- Take a cell off each of the keys. -/
def MM.removeAll (m : MM K) (ks : List K) (c : Cell) : MM K :=
  ks.foldl (fun m k => m.remove k c) m

/-- Add, for each cell of the list, the cell's keys. -/
def MM.addCells (m : MM K) (cs : List Cell) (keys : Cell → List K) : MM K :=
  cs.foldl (fun m c => m.addAll (keys c) c) m

/-- The index is exactly the relation `ck`: a cell is listed under a key iff the key is one of the
    cell's keys. -/
def MM.Inv (m : MM K) (ck : Cell → List K) : Prop := ∀ k c, c ∈ m.get k ↔ k ∈ ck c

theorem MM.get_add (m : MM K) (k k' : K) (c : Cell) :
    (m.add k c).get k' = if k == k' then c :: m.get k else m.get k' := by
  unfold MM.add MM.get
  rw [HashMap.getD_insert]

theorem MM.get_remove (m : MM K) (k k' : K) (c : Cell) :
    (m.remove k c).get k' = if k == k' then (m.get k).filter (fun d => d != c) else m.get k' := by
  unfold MM.remove
  by_cases h : (k == k') = true
  · have hk : k = k' := beq_iff_eq.mp h
    subst hk
    cases hf : (m.get k).filter (fun d => d != c) with
    | nil => simp [MM.get, HashMap.getD_erase]
    | cons x xs => simp [MM.get, HashMap.getD_insert]
  · cases hf : (m.get k).filter (fun d => d != c) with
    | nil => simp [MM.get, HashMap.getD_erase, h]
    | cons x xs => simp [MM.get, HashMap.getD_insert, h]

theorem MM.mem_addAll (ks : List K) : ∀ (m : MM K) (c c' : Cell) (k : K),
    c' ∈ (m.addAll ks c).get k ↔ c' ∈ m.get k ∨ (c' = c ∧ k ∈ ks) := by
  induction ks with
  | nil => intro m c c' k; simp [MM.addAll]
  | cons k0 ks ih =>
    intro m c c' k
    unfold MM.addAll at ih ⊢
    simp only [List.foldl_cons]
    rw [ih, MM.get_add]
    by_cases h : k0 = k
    · subst h; simp; grind
    · have hb : (k0 == k) = false := by simp [h]
      simp [hb, h]; grind

theorem MM.mem_removeAll (ks : List K) : ∀ (m : MM K) (c c' : Cell) (k : K),
    c' ∈ (m.removeAll ks c).get k ↔ c' ∈ m.get k ∧ ¬ (c' = c ∧ k ∈ ks) := by
  induction ks with
  | nil => intro m c c' k; simp [MM.removeAll]
  | cons k0 ks ih =>
    intro m c c' k
    unfold MM.removeAll at ih ⊢
    simp only [List.foldl_cons]
    rw [ih, MM.get_remove]
    by_cases h : k0 = k
    · subst h; simp; grind
    · have hb : (k0 == k) = false := by simp [h]
      simp [hb, h]; grind

theorem MM.mem_addCells (cs : List Cell) (keys : Cell → List K) : ∀ (m : MM K) (c' : Cell) (k : K),
    c' ∈ (m.addCells cs keys).get k ↔ c' ∈ m.get k ∨ (c' ∈ cs ∧ k ∈ keys c') := by
  induction cs with
  | nil => intro m c' k; simp [MM.addCells]
  | cons c cs ih =>
    intro m c' k
    unfold MM.addCells at ih ⊢
    simp only [List.foldl_cons]
    rw [ih, MM.mem_addAll]
    by_cases h : c' = c
    · subst h; simp; grind
    · simp [h]

/-- One cell's keys change from `ck c₀` to `new`, every other cell's stay: take the cell off its old
    keys and list it under the new ones. -/
theorem MM.inv_change (m : MM K) (ck ck' : Cell → List K) (c₀ : Cell) (new : List K)
    (hinv : m.Inv ck) (hck : ∀ c, ck' c = if c = c₀ then new else ck c) :
    ((m.removeAll (ck c₀) c₀).addAll new c₀).Inv ck' := by
  intro k c'
  rw [MM.mem_addAll, MM.mem_removeAll, hinv, hck]
  by_cases h : c' = c₀
  · subst h; simp
  · simp [h]

/-- Rebuild over a list of cells: the new keys of a cell are its old keys, and, when the cell is in
    the list, those of `keys`. -/
theorem MM.inv_addCells (m : MM K) (cs : List Cell) (keys : Cell → List K) (ck ck' : Cell → List K)
    (hinv : m.Inv ck) (hck : ∀ c k, k ∈ ck' c ↔ k ∈ ck c ∨ (c ∈ cs ∧ k ∈ keys c)) :
    (m.addCells cs keys).Inv ck' := by
  intro k c
  rw [MM.mem_addCells, hinv, hck]

end

end Impl
end Strata
