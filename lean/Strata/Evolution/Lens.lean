/-
Projections and the lens laws (theorem 5).

A dependent pinned to an earlier version of the schema reads the new data through `project c`,
the read-only view of the new data in the old shape (the lens `get`). `migrate c` is its `put`:
it takes data in the old shape to the new shape, so `project c ∘ migrate c` is the law that
matters, "reading the new shape back returns what the old shape held" (D-8214).

The change forms that have a left inverse are the ones `hasLeftInverse` names. This file proves
that for them `project c (migrate c data)` agrees with `data` on every cell of the schema, that
a pinned dependent therefore evaluates as it did, and that the changes that remove or clamp a
cell have no left inverse at all.
-/

import Strata.Evolution.Proofs

namespace Strata.Evolution

/-- **The left inverse law (the `put`-`get` law of the lens).** For a change that has a left
    inverse, reading the migrated data back through the projection returns the original data on
    every cell the schema has: the projection rebuilds the old shape from the new data. This is
    why a projection is defined only for these changes (a removal has none).

    Verifies: MIG-597. -/
theorem project_migrate (c : Change) (s : Schema) (data : Data) (hpre : pre c s)
    (hinv : hasLeftInverse c) (o f : Nat) (h : hasField s o f) :
    project c (migrate c data) o f = data o f := by
  cases c with
  | addField o' fld v =>
    simp only [project]
    exact migrate_stable _ s data hpre o f h (by simp [footprint])
  | widen _ _ _ => rfl
  | renameField _ _ _ => rfl
  | moveField o' f' r o'' fld =>
    by_cases hc : o = o' ∧ f = f'
    · obtain ⟨rfl, rfl⟩ := hc
      simp [project, migrate]
    · have hs := migrate_stable (.moveField o' f' r o'' fld) s data hpre o f h
        (by simp [footprint]; exact fun e1 e2 => hc ⟨e1, e2⟩)
      simp [project, hc, hs]
  | splitField o' f' f1 f2 sp jn =>
    obtain ⟨_, hne, _, _, _, hjn⟩ := hpre
    have hne' : f2.id ≠ f1.id := fun h => hne h.symm
    by_cases hc : o = o' ∧ f = f'
    · obtain ⟨rfl, rfl⟩ := hc
      simp [project, migrate, hne', hjn]
    · have hs := migrate_stable (.splitField o' f' f1 f2 sp jn) s data ⟨‹_›, hne, ‹_›, ‹_›, ‹_›, hjn⟩
        o f h (by simp [footprint]; exact fun e1 e2 => hc ⟨e1, e2⟩)
      simp [project, hc, hs]
  | retype o' f' ty enc dec =>
    by_cases hc : o = o' ∧ f = f'
    · obtain ⟨rfl, rfl⟩ := hc
      simp only [project, migrate, and_self, ite_true]
      exact hinv _
    · have hs := migrate_stable (.retype o' f' ty enc dec) s data hpre o f h
        (by simp [footprint]; exact fun e1 e2 => hc ⟨e1, e2⟩)
      simp [project, hc, hs]
  | removeField _ _ => exact absurd hinv (by simp [hasLeftInverse])
  | narrow _ _ _ => exact absurd hinv (by simp [hasLeftInverse])
  | retargetRef _ _ _ => exact absurd hinv (by simp [hasLeftInverse])
  | removeObject _ => exact absurd hinv (by simp [hasLeftInverse])
  | removeDep _ => exact absurd hinv (by simp [hasLeftInverse])

/-- The cells a change with a left inverse touches exist in the schema. -/
theorem footprint_hasField (c : Change) (s : Schema) (hpre : pre c s) (hinv : hasLeftInverse c)
    (p : Nat × Nat) (hp : p ∈ footprint c s) : hasField s p.1 p.2 := by
  cases c <;> simp_all [footprint, hasLeftInverse, pre]

/-- **Left inverse on the cells a change touches.** For a reversible change, the inverse
    (`project`, which undoes `migrate`) restores exactly the cells the change touched. -/
theorem left_inverse_on_footprint (c : Change) (s : Schema) (data : Data) (hpre : pre c s)
    (hinv : hasLeftInverse c) (p : Nat × Nat) (hp : p ∈ footprint c s) :
    project c (migrate c data) p.1 p.2 = data p.1 p.2 :=
  project_migrate c s data hpre hinv p.1 p.2 (footprint_hasField c s hpre hinv p hp)

theorem evalDep_deps (s s' : Schema) (h : s.deps = s'.deps) (data : Data) (d : Dep) :
    evalDep s' data d = evalDep s data d := by
  cases d <;> simp [evalDep, h]

/-- **A pinned dependent evaluates the same (theorem 5, evaluation form).** Let a change have a
    left inverse. A dependent of the schema reading the new data through the projection gets what
    it got from the data before: the pinned dependent is not broken by the change. It is evaluated
    in the schema it was pinned to, on the projected data. For a change that does not rewrite
    dependents (a re-encoding, `isMono`) the dependent is also still in the live schema, and
    reading the projected data there gives the same value. -/
theorem pinned_eval (c : Change) (s : Schema) (data : Data) (hall : ∀ d ∈ s.deps, ValidDep s d)
    (hpre : pre c s) (hinv : hasLeftInverse c) (d : Dep) (hd : ValidDep s d) :
    evalDep s (project c (migrate c data)) d = evalDep s data d := by
  have hr : ∀ r : Reader, ValidDep s (.reader r) →
      evalReader r (project c (migrate c data)) = evalReader r data := by
    intro r hv
    unfold evalReader
    congr 1
    apply List.map_congr_left
    intro e he
    apply eval_congr
    intro p hp
    exact project_migrate c s data hpre hinv p.1 p.2 (hv.2 p (mem_cells he hp))
  cases d with
  | reader r => exact hr r hd
  | view v =>
    simp only [evalDep]
    congr 1
    apply List.map_congr_left
    intro q hq
    cases hf : findR s.deps q with
    | none => rfl
    | some r => exact hr r (hall _ (findR_mem hf))

/-- A pinned dependent still in the live schema (the change rewrites nothing) reads the
    projected data there and gets what it got before. -/
theorem pinned_eval_live (c : Change) (s : Schema) (data : Data)
    (hall : ∀ d ∈ s.deps, ValidDep s d) (hpre : pre c s) (hinv : hasLeftInverse c)
    (hm : isMono c = true) (d : Dep) (hd : ValidDep s d) :
    evalDep (apply c s) (project c (migrate c data)) d = evalDep s data d := by
  rw [evalDep_deps s (apply c s) (deps_apply_of_mono c s hm).symm]
  exact pinned_eval c s data hall hpre hinv d hd

/-! ## Irreversible changes have no left inverse -/

/-- The data with one cell set. -/
def setCell (data : Data) (o f : Nat) (v : Val) : Data :=
  fun o' f' => if o' = o ∧ f' = f then v else data o' f'

/-- If two data that differ only on cell `(o, f)` migrate to the same data, no function of the
    migrated data can recover that cell: the change has no left inverse there. -/
theorem no_left_inverse (c : Change) (o f : Nat) (v1 v2 : Val) (hne : v1 ≠ v2)
    (hc : migrate c (setCell (fun _ _ => .null) o f v1) =
      migrate c (setCell (fun _ _ => .null) o f v2)) :
    ¬ ∃ g : Data → Data, ∀ data, g (migrate c data) o f = data o f := by
  rintro ⟨g, hg⟩
  have a := hg (setCell (fun _ _ => .null) o f v1)
  have b := hg (setCell (fun _ _ => .null) o f v2)
  rw [hc] at a
  simp only [setCell, and_self, ite_true] at a b
  exact hne (a.symm.trans b)

/-- **A removed field cannot be recovered.** Removing a field has no left inverse on its cell:
    no function of the migrated data returns what the cell held. Removing a field is therefore
    irreversible (as MIG-557 says), and a dependent of it cannot be pinned to a projection. -/
theorem removeField_irreversible (o f : Nat) :
    ¬ ∃ g : Data → Data, ∀ data, g (migrate (.removeField o f) data) o f = data o f :=
  no_left_inverse _ o f (.num 0) (.num 1) (by simp) (by
    funext o' f'
    by_cases h : o' = o ∧ f' = f <;> simp [migrate, setCell, h])

theorem retargetRef_irreversible (o f t : Nat) :
    ¬ ∃ g : Data → Data, ∀ data, g (migrate (.retargetRef o f t) data) o f = data o f :=
  no_left_inverse _ o f (.num 0) (.num 1) (by simp) (by
    funext o' f'
    by_cases h : o' = o ∧ f' = f <;> simp [migrate, setCell, h])

theorem removeObject_irreversible (o f : Nat) :
    ¬ ∃ g : Data → Data, ∀ data, g (migrate (.removeObject o) data) o f = data o f :=
  no_left_inverse _ o f (.num 0) (.num 1) (by simp) (by
    funext o' f'
    by_cases h : o' = o <;> simp [migrate, setCell, h])

/-- Narrowing to `nat` clamps a negative to zero, so `-1` and `0` migrate alike. -/
theorem narrow_nat_irreversible (o f : Nat) :
    ¬ ∃ g : Data → Data, ∀ data, g (migrate (.narrow o f .nat) data) o f = data o f :=
  no_left_inverse _ o f (.num (-1)) (.num 0) (by simp) (by
    funext o' f'
    by_cases h : o' = o ∧ f' = f
    · obtain ⟨rfl, rfl⟩ := h
      simp [migrate, setCell, narrowVal]
    · simp [migrate, setCell, h])

/-! ## A pinned breaking change exists -/

/-- The pinned dependent that makes a breaking change acceptable is not vacuous: in `exSchema` with
    its query pinned, re-encoding the field (with the identity as encoding and as inverse) is a
    breaking change that reaches the query, and the acceptance rule admits it. -/
theorem pinned_breaking_change_accepted :
    ∃ (s : Schema) (c : Change), classify c = .breaking ∧ 10 ∈ impact c s ∧ accepted c s := by
  refine ⟨{ exSchema with pins := [10] }, .retype 1 1 .nat id id, rfl, by decide, ?_⟩
  have himp : impact (.retype 1 1 .nat id id) { exSchema with pins := [10] } = [10] := by decide
  refine ⟨⟨by decide, trivial⟩, fun _ q hq => ?_⟩
  rw [himp] at hq
  have : q = 10 := by simpa using hq
  subst this
  exact ⟨by simp, fun _ => rfl⟩

end Strata.Evolution
