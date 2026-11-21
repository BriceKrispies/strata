/-
Rewriting dependents for adaptable changes: a move and a split substitute the reads of the
cell they relocate. This file proves that a rewritten dependent is valid and evaluates as it did
(theorem 4, `rewrite_correct`), and the facts about the changes that touch nothing (`isMono`)
that closure, compatibility and composition share.
-/

import Strata.Evolution.Lemmas

namespace Strata.Evolution

/-- The changes that substitute reads: a move and a split. -/
def isRewriting : Change → Bool
  | .moveField .. => true
  | .splitField .. => true
  | _ => false

/-- The cells a change creates and that a rewrite points readers at. -/
def newCells : Change → List (Nat × Nat)
  | .moveField _ _ _ o' fld => [(o', fld.id)]
  | .splitField o _ f1 f2 _ _ => [(o, f1.id), (o, f2.id)]
  | _ => []

theorem lost_of_mono (c : Change) (h : isMono c = true) (o f : Nat) : ¬ lost c o f := by
  cases c <;> simp_all [isMono, lost]

theorem removedDeps_of_rewriting (c : Change) (h : isRewriting c = true) : removedDeps c = [] := by
  cases c <;> simp_all [isRewriting, removedDeps]

theorem not_removeObject_of_rewriting (c : Change) (h : isRewriting c = true) :
    ∀ o, c ≠ .removeObject o := by
  intro o e; subst e; simp [isRewriting] at h

/-- Under a change that loses nothing and rewrites nothing (add a field, widen, rename,
    re-encode), every valid dependent stays valid. -/
theorem valid_of_mono (c : Change) (s : Schema) (hm : isMono c = true) (d : Dep)
    (hv : ValidDep s d) : ValidDep (apply c s) d := by
  cases d with
  | reader r =>
    obtain ⟨hb, hr⟩ := hv
    exact ⟨hasObj_apply' c s _ hb (objGone_of_mono c hm _),
      fun p hp => hasField_apply c s _ _ (hr p hp) (lost_of_mono c hm _ _)⟩
  | view v =>
    intro q hq
    rw [findR_apply c s q (by simp [removedDeps_of_mono c hm]), Option.isSome_map]
    exact hv q hq

theorem mem_cells_subst (c : Change) (hr : isRewriting c = true) (e : Read) (p : Nat × Nat)
    (hp : p ∈ (e.subst c).cells) : (p ∈ e.cells ∧ ¬ lost c p.1 p.2) ∨ p ∈ newCells c := by
  induction e with
  | cell o f =>
    cases c with
    | moveField o0 f0 r o' fld =>
      by_cases h : o = o0 ∧ f = f0
      · right
        simp only [Read.subst, cellSub, h, and_self, ite_true, Read.cells, List.mem_singleton] at hp
        simp [newCells, hp]
      · left
        simp only [Read.subst, cellSub, h, ite_false, Read.cells, List.mem_singleton] at hp
        subst hp
        exact ⟨by simp [Read.cells], fun hl => h ⟨hl.1.symm, hl.2.symm⟩⟩
    | splitField o0 f0 f1 f2 sp jn =>
      by_cases h : o = o0 ∧ f = f0
      · right
        simp only [Read.subst, cellSub, h, and_self, ite_true, Read.cells, List.mem_append,
          List.mem_singleton] at hp
        simp only [newCells, List.mem_cons, List.not_mem_nil, or_false]
        rcases hp with hp | hp <;> simp [hp]
      · left
        simp only [Read.subst, cellSub, h, ite_false, Read.cells, List.mem_singleton] at hp
        subst hp
        exact ⟨by simp [Read.cells], fun hl => h ⟨hl.1.symm, hl.2.symm⟩⟩
    | _ => simp [isRewriting] at hr
  | join jn a b iha ihb =>
    simp only [Read.subst, Read.cells, List.mem_append] at hp ⊢
    rcases hp with hp | hp
    · rcases iha hp with ⟨h1, h2⟩ | h
      · exact Or.inl ⟨Or.inl h1, h2⟩
      · exact Or.inr h
    · rcases ihb hp with ⟨h1, h2⟩ | h
      · exact Or.inl ⟨Or.inr h1, h2⟩
      · exact Or.inr h

/-- The cells a move or a split creates exist in the new schema. -/
theorem hasField_new (c : Change) (s : Schema) (hr : isRewriting c = true) (hpre : pre c s)
    (p : Nat × Nat) (hp : p ∈ newCells c) : hasField (apply c s) p.1 p.2 := by
  cases c with
  | moveField o f r o' fld =>
    obtain ⟨_, _, _, ho', _, _, _⟩ := hpre
    simp only [newCells, List.mem_singleton] at hp
    subst hp
    obtain ⟨ob, hob, hid⟩ := List.mem_map.1 ho'
    have h1 := updObj_mem_self (o := o) (g := fun ob : Obj => { ob with
      fields := ob.fields.filter (fun fl => fl.id != f) }) hob
    have hid1 : (if ob.id = o then { ob with
        fields := ob.fields.filter (fun fl => fl.id != f) } else ob).id = o' := by
      by_cases e : ob.id = o
      · rw [if_pos e]; exact hid
      · rw [if_neg e]; exact hid
    have h2 := updObj_mem_self (o := o') (g := fun ob : Obj => { ob with
      fields := ob.fields ++ [fld] }) h1
    refine ⟨_, h2, ?_, ?_⟩
    · simp [hid1]
    · simp [hid1]
  | splitField o f f1 f2 sp jn =>
    obtain ⟨⟨ob, hob, hid, _⟩, _⟩ := hpre
    have h1 := updObj_mem_self (o := o) (g := fun ob : Obj => { ob with
      fields := ob.fields.filter (fun fl => fl.id != f) ++ [f1, f2] }) hob
    simp only [newCells, List.mem_cons, List.not_mem_nil, or_false] at hp
    rcases hp with rfl | rfl
    · exact ⟨_, h1, by simp [hid], by simp [hid]⟩
    · exact ⟨_, h1, by simp [hid], by simp [hid]⟩
  | _ => simp [isRewriting] at hr

/-- A move or a split leaves every valid dependent valid once it is rewritten. -/
theorem valid_of_rewrite (c : Change) (s : Schema) (hr : isRewriting c = true) (hpre : pre c s)
    (d : Dep) (hv : ValidDep s d) : ValidDep (apply c s) (rewrite c d) := by
  cases d with
  | reader r =>
    obtain ⟨hb, hcells⟩ := hv
    show ValidDep (apply c s) (.reader (rewriteReader c r))
    refine ⟨(hasObj_apply c s _ (not_removeObject_of_rewriting c hr)).2 hb, ?_⟩
    intro p hp
    simp only [Reader.cells, rewriteReader, List.mem_flatMap, List.mem_map] at hp
    obtain ⟨e', ⟨e, he, rfl⟩, hp⟩ := hp
    rcases mem_cells_subst c hr e p hp with ⟨h1, h2⟩ | h
    · exact hasField_apply c s _ _ (hcells p (mem_cells he h1)) h2
    · exact hasField_new c s hr hpre p h
  | view v =>
    intro q hq
    rw [findR_apply c s q (by simp [removedDeps_of_rewriting c hr]), Option.isSome_map]
    exact hv q hq

/-! ## Evaluation -/

/-- A substituted read has the value the read had. -/
theorem eval_subst (c : Change) (s : Schema) (data : Data) (hpre : pre c s)
    (hr : isRewriting c = true) (e : Read) (hv : ∀ p ∈ e.cells, hasField s p.1 p.2) :
    (e.subst c).eval (migrate c data) = e.eval data := by
  induction e with
  | join jn a b iha ihb =>
    simp only [Read.subst, Read.eval]
    rw [iha (fun p hp => hv p (by simp [Read.cells, hp])),
      ihb (fun p hp => hv p (by simp [Read.cells, hp]))]
  | cell o f =>
    have hex := hv (o, f) (by simp [Read.cells])
    cases c with
    | moveField o0 f0 r o' fld =>
      by_cases h : o = o0 ∧ f = f0
      · obtain ⟨rfl, rfl⟩ := h
        simp [Read.subst, cellSub, Read.eval, migrate]
      · have := migrate_stable (.moveField o0 f0 r o' fld) s data hpre o f hex
          (by simp [footprint]; exact fun e1 e2 => h ⟨e1, e2⟩)
        simpa [Read.subst, cellSub, Read.eval, h] using this
    | splitField o0 f0 f1 f2 sp jn =>
      by_cases h : o = o0 ∧ f = f0
      · obtain ⟨rfl, rfl⟩ := h
        obtain ⟨_, hne, _, _, _, hjn⟩ := hpre
        have hne' : f2.id ≠ f1.id := fun h => hne h.symm
        simp [Read.subst, cellSub, Read.eval, migrate, hne', hjn]
      · have := migrate_stable (.splitField o0 f0 f1 f2 sp jn) s data hpre o f hex
          (by simp [footprint]; exact fun e1 e2 => h ⟨e1, e2⟩)
        simpa [Read.subst, cellSub, Read.eval, h] using this
    | _ => simp [isRewriting] at hr

/-- A change that rewrites nothing and is not breaking (add a field, widen, rename) leaves a valid
    reader's value alone on the migrated data. -/
theorem eval_reader_mono (c : Change) (s : Schema) (data : Data) (hpre : pre c s)
    (hm : isMono c = true) (hnb : classify c ≠ .breaking) (r : Reader)
    (hv : ValidDep s (.reader r)) : evalReader r (migrate c data) = evalReader r data := by
  cases c with
  | widen _ _ _ => rfl
  | renameField _ _ _ => rfl
  | addField o fld v =>
    unfold evalReader
    congr 1
    apply List.map_congr_left
    intro e he
    apply eval_congr
    intro p hp
    exact migrate_stable _ s data hpre _ _ (hv.2 p (mem_cells he hp)) (by simp [footprint])
  | _ => simp_all [isMono, classify]

/-- A rewritten reader has the value the reader had, for any change that is not breaking. -/
theorem eval_reader_nb (c : Change) (s : Schema) (data : Data) (hpre : pre c s)
    (hnb : classify c ≠ .breaking) (r : Reader) (hv : ValidDep s (.reader r)) :
    evalReader (rewriteReader c r) (migrate c data) = evalReader r data := by
  by_cases hrw : isRewriting c = true
  · unfold evalReader
    simp only [rewriteReader, List.map_map]
    congr 1
    apply List.map_congr_left
    intro e he
    exact eval_subst c s data hpre hrw e (fun p hp => hv.2 p (mem_cells he hp))
  · have hm : isMono c = true := by
      cases c <;> simp_all [isRewriting, isMono, classify]
    rw [rewriteReader_of_reads c r (fun e _ => subst_of_mono c hm e)]
    exact eval_reader_mono c s data hpre hm hnb r hv

theorem removedDeps_of_nb (c : Change) (h : classify c ≠ .breaking) : removedDeps c = [] := by
  cases c <;> simp_all [removedDeps, classify]

theorem valid_of_nb (c : Change) (s : Schema) (hpre : pre c s) (hnb : classify c ≠ .breaking)
    (d : Dep) (hv : ValidDep s d) : ValidDep (apply c s) (rewrite c d) := by
  by_cases hrw : isRewriting c = true
  · exact valid_of_rewrite c s hrw hpre d hv
  · have hm : isMono c = true := by
      cases c <;> simp_all [isRewriting, isMono, classify]
    rw [rewrite_of_mono c hm]
    exact valid_of_mono c s hm d hv

/-- **Rewrite correctness (theorem 4, for every change that is not breaking).** Applying a change
    that is not breaking together with the rewrite of a dependent leaves the dependent present
    and valid in the new schema, and it evaluates on the migrated data to what it evaluated to
    on the data before. -/
theorem rewrite_correct_nb (c : Change) (s : Schema) (data : Data)
    (hall : ∀ d ∈ s.deps, ValidDep s d) (hpre : pre c s) (hnb : classify c ≠ .breaking)
    (d : Dep) (hd : d ∈ s.deps) :
    rewrite c d ∈ (apply c s).deps ∧ ValidDep (apply c s) (rewrite c d) ∧
      evalDep (apply c s) (migrate c data) (rewrite c d) = evalDep s data d := by
  have hrem := removedDeps_of_nb c hnb
  refine ⟨mem_deps_apply c s d hd (by simp [hrem]), valid_of_nb c s hpre hnb d (hall d hd), ?_⟩
  cases d with
  | reader r => exact eval_reader_nb c s data hpre hnb r (hall _ hd)
  | view v =>
    simp only [rewrite, evalDep]
    congr 1
    apply List.map_congr_left
    intro q hq
    rw [findR_apply c s q (by simp [hrem])]
    cases hf : findR s.deps q with
    | none => rfl
    | some r =>
      exact eval_reader_nb c s data hpre hnb r (hall _ (findR_mem hf))

/-- **Rewrite correctness (theorem 4).** For an accepted adaptable change (a rename, a move, a
    total split) on a well-formed schema and a dependent of the schema, the rewritten dependent
    is in the new schema, is valid there, and evaluates on the migrated data to exactly what the
    original dependent evaluated to on the data before. A dependent that follows the change by
    rewriting loses nothing: it gives the answers it gave.

    Verifies: MIG-591. -/
theorem rewrite_correct (c : Change) (s : Schema) (data : Data) (hwf : WF s)
    (hacc : accepted c s) (hc : classify c = .adaptable) (d : Dep) (hd : d ∈ s.deps) :
    rewrite c d ∈ (apply c s).deps ∧ ValidDep (apply c s) (rewrite c d) ∧
      evalDep (apply c s) (migrate c data) (rewrite c d) = evalDep s data d :=
  rewrite_correct_nb c s data hwf.deps hacc.1 (by rw [hc]; simp) d hd

end Strata.Evolution
