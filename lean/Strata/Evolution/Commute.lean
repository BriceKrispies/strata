/-
Commutation of changes with disjoint footprints (theorem 6).

Two changes made from one base revision commute when what they touch (`ents`: the cells, field
lists and dependents they touch, together with their impact sets) is disjoint. The proof works
on a normal form. The object-type effect of a change is a short sequence of primitive edits
(`Prim`); two primitives commute when the entities they edit differ (`Indep`). The data effect is
one write per cell (`wr`): the new value of a cell is a function of one source cell; two changes
commute when neither writes a cell the other writes or reads. The dependents are rewritten by
substitution of reads, and substitutions at different cells commute.

Equality is exact: the schemas are equal as lists, field order included, because a change that
appends to an object's field list touches that object (`Ent.obj`) and so is never disjoint from
another that does.
-/

import Strata.Evolution.Rewrite

namespace Strata.Evolution

theorem Disjoint.symm {c₁ c₂ : Change} {s : Schema} (h : Disjoint c₁ c₂ s) : Disjoint c₂ c₁ s :=
  fun e h2 h1 => h e h1 h2

theorem mem_ents_of_ents0 {c : Change} {s : Schema} {e : Ent} (h : e ∈ ents0 c s) :
    e ∈ ents c s := List.mem_append_left _ h

/-- Disjoint changes touch no entity in common, impact sets aside. -/
theorem Disjoint.ents0 {c₁ c₂ : Change} {s : Schema} (h : Disjoint c₁ c₂ s) :
    ∀ e ∈ ents0 c₁ s, e ∉ ents0 c₂ s :=
  fun e h1 h2 => h e (mem_ents_of_ents0 h1) (mem_ents_of_ents0 h2)

/-! ## Object types: primitive edits -/

/-- A primitive edit of the list of object types. `set` edits one field in place and keeps
    its id. -/
inductive Prim where
  | add (o : Nat) (a : Field)
  | del (o f : Nat)
  | set (o f : Nat) (g : Field → Field) (hg : ∀ fl, (g fl).id = fl.id)
  | rmObj (o : Nat)

def runP : Prim → List Obj → List Obj
  | .add o a, l => updObj o (fun ob => { ob with fields := ob.fields ++ [a] }) l
  | .del o f, l =>
    updObj o (fun ob => { ob with fields := ob.fields.filter (fun fl => fl.id != f) }) l
  | .set o f g _, l => updObj o (updField f g) l
  | .rmObj o, l => l.filter fun ob => ob.id != o

def runPrims (ps : List Prim) (l : List Obj) : List Obj := ps.foldl (fun l p => runP p l) l

/-- The object-type effect of a change, as primitive edits in order. -/
def prims : Change → List Prim
  | .addField o fld _ => [.add o fld]
  | .widen o f ty => [.set o f (fun fl => { fl with ty := ty }) (fun _ => rfl)]
  | .renameField o f n => [.set o f (fun fl => { fl with name := n }) (fun _ => rfl)]
  | .removeField o f => [.del o f]
  | .narrow o f ty => [.set o f (fun fl => { fl with ty := ty }) (fun _ => rfl)]
  | .retargetRef o f t => [.set o f (fun fl => { fl with ty := .ref t }) (fun _ => rfl)]
  | .removeObject o => [.rmObj o]
  | .removeDep _ => []
  | .moveField o f _ o' fld => [.del o f, .add o' fld]
  | .splitField o f f1 f2 _ _ => [.del o f, .add o f1, .add o f2]
  | .retype o f ty _ _ => [.set o f (fun fl => { fl with ty := ty }) (fun _ => rfl)]

/-- The entities a primitive edits. -/
def Prim.keys : Prim → List Ent
  | .add o a => [.obj o, .cell o a.id]
  | .del o f => [.cell o f]
  | .set o f _ _ => [.cell o f]
  | .rmObj _ => []

/-- When two primitives commute: they edit different field lists or different fields. -/
def Indep : Prim → Prim → Prop
  | .add o _, .add o' _ => o ≠ o'
  | .add o a, .del o' f => o ≠ o' ∨ a.id ≠ f
  | .add o a, .set o' f _ _ => o ≠ o' ∨ a.id ≠ f
  | .del o f, .add o' a => o ≠ o' ∨ a.id ≠ f
  | .set o f _ _, .add o' a => o ≠ o' ∨ a.id ≠ f
  | .set o f _ _, .set o' f' _ _ => o ≠ o' ∨ f ≠ f'
  | _, _ => True

theorem updObj_comp (o : Nat) (g1 g2 : Obj → Obj) (hg2 : ∀ ob, (g2 ob).id = ob.id)
    (l : List Obj) : updObj o g1 (updObj o g2 l) = updObj o (fun ob => g1 (g2 ob)) l := by
  simp only [updObj, List.map_map]
  apply List.map_congr_left
  intro ob _
  simp only [Function.comp]
  by_cases h : ob.id = o <;> simp [h, hg2]

theorem applyObjs_eq_run (c : Change) (l : List Obj) : applyObjs c l = runPrims (prims c) l := by
  cases c with
  | splitField o f f1 f2 sp jn =>
    simp only [applyObjs, runPrims, prims, runP, List.foldl, updObj, List.map_map]
    apply List.map_congr_left
    intro ob _
    simp only [Function.comp]
    by_cases h : ob.id = o <;> simp [h, List.append_assoc]
  | _ => rfl

theorem updObj_comm (o1 o2 : Nat) (g1 g2 : Obj → Obj) (hg1 : ∀ ob, (g1 ob).id = ob.id)
    (hg2 : ∀ ob, (g2 ob).id = ob.id)
    (h : ∀ ob : Obj, ob.id = o1 → ob.id = o2 → g1 (g2 ob) = g2 (g1 ob)) (l : List Obj) :
    updObj o1 g1 (updObj o2 g2 l) = updObj o2 g2 (updObj o1 g1 l) := by
  simp only [updObj, List.map_map]
  apply List.map_congr_left
  intro ob _
  simp only [Function.comp]
  by_cases h1 : ob.id = o1 <;> by_cases h2 : ob.id = o2
  · obtain rfl : o1 = o2 := h1.symm.trans h2
    have e := h ob h1 h2
    simp [h1, hg1, hg2, e]
  · simp only [if_pos h1, if_neg h2, hg1, hg2]
  · simp only [if_neg h1, if_pos h2, hg1, hg2]
  · simp [h1, h2, hg1, hg2]

theorem updObj_filter (o o' : Nat) (g : Obj → Obj) (hg : ∀ ob, (g ob).id = ob.id) (l : List Obj) :
    (updObj o g l).filter (fun ob => ob.id != o') = updObj o g (l.filter fun ob => ob.id != o') := by
  induction l with
  | nil => rfl
  | cons ob l ih =>
    by_cases h1 : ob.id = o <;> by_cases h2 : ob.id = o' <;>
      simp_all [updObj, List.filter_cons]

theorem runP_comm (p q : Prim) (h : Indep p q) (l : List Obj) :
    runP p (runP q l) = runP q (runP p l) := by
  cases p with
  | add o a =>
    cases q with
    | add o' a' =>
      simp only [runP]
      exact updObj_comm _ _ _ _ (by intro _; rfl) (by intro _; rfl)
        (fun ob h1 h2 => absurd (h1.symm.trans h2) h) l
    | del o' f =>
      simp only [runP]
      refine updObj_comm _ _ _ _ (by intro _; rfl) (by intro _; rfl) ?_ l
      intro ob h1 h2
      rcases h with hne | hne
      · exact absurd (h1.symm.trans h2) hne
      · have : (a.id != f) = true := by simpa using hne
        simp [List.filter_append, this]
    | set o' f g hg =>
      simp only [runP]
      refine updObj_comm _ _ _ _ (by intro _; rfl) (by intro _; rfl) ?_ l
      intro ob h1 h2
      rcases h with hne | hne
      · exact absurd (h1.symm.trans h2) hne
      · simp [updField, hne]
    | rmObj o' =>
      simp only [runP]
      exact (updObj_filter _ _ _ (by intro _; rfl) l).symm
  | del o f =>
    cases q with
    | add o' a =>
      simp only [runP]
      refine (updObj_comm _ _ _ _ (by intro _; rfl) (by intro _; rfl) ?_ l).symm
      intro ob h1 h2
      rcases h with hne | hne
      · exact absurd (h2.symm.trans h1) hne
      · have : (a.id != f) = true := by simpa using hne
        simp [List.filter_append, this]
    | del o' f' =>
      simp only [runP]
      refine updObj_comm _ _ _ _ (by intro _; rfl) (by intro _; rfl) ?_ l
      intro ob _ _
      simp [List.filter_filter, Bool.and_comm]
    | set o' f' g hg =>
      simp only [runP]
      refine updObj_comm _ _ _ _ (by intro _; rfl) (by intro _; rfl) ?_ l
      intro ob _ _
      simp only [updField]
      congr 1
      rw [List.filter_map]
      congr 1
      apply List.filter_congr
      intro fl _
      by_cases hf : fl.id = f' <;> simp [Function.comp, hf, hg]
    | rmObj o' =>
      simp only [runP]
      exact (updObj_filter _ _ _ (by intro _; rfl) l).symm
  | set o f g hg =>
    cases q with
    | add o' a =>
      simp only [runP]
      refine (updObj_comm _ _ _ _ (by intro _; rfl) (by intro _; rfl) ?_ l).symm
      intro ob h1 h2
      rcases h with hne | hne
      · exact absurd (h2.symm.trans h1) hne
      · simp [updField, hne]
    | del o' f' =>
      simp only [runP]
      refine updObj_comm _ _ _ _ (by intro _; rfl) (by intro _; rfl) ?_ l
      intro ob _ _
      simp only [updField]
      congr 1
      rw [List.filter_map]
      congr 1
      apply List.filter_congr
      intro fl _
      by_cases hf : fl.id = f <;> simp [Function.comp, hf, hg]
    | set o' f' g' hg' =>
      simp only [runP]
      refine updObj_comm _ _ _ _ (by intro _; rfl) (by intro _; rfl) ?_ l
      intro ob h1 h2
      rcases h with hne | hne
      · exact absurd (h1.symm.trans h2) hne
      · simp only [updField, List.map_map]
        congr 1
        apply List.map_congr_left
        intro fl _
        simp only [Function.comp]
        by_cases hf : fl.id = f <;> by_cases hf' : fl.id = f'
        · exact absurd (hf.symm.trans hf') hne
        · simp [hf, hf', hg, hg', hne]
        · simp [hf, hf', hg, hg', hne.symm]
        · simp [hf, hf']
    | rmObj o' =>
      simp only [runP]
      exact (updObj_filter _ _ _ (by intro _; rfl) l).symm
  | rmObj o =>
    cases q with
    | add o' a =>
      simp only [runP]
      exact updObj_filter _ _ _ (by intro _; rfl) l
    | del o' f =>
      simp only [runP]
      exact updObj_filter _ _ _ (by intro _; rfl) l
    | set o' f g hg =>
      simp only [runP]
      exact updObj_filter _ _ _ (by intro _; rfl) l
    | rmObj o' =>
      simp only [runP]
      simp only [runP, List.filter_filter]
      congr 1
      funext ob
      simp [Bool.and_comm]

theorem runPrims_step (q : Prim) (ps : List Prim) (h : ∀ p ∈ ps, Indep p q) :
    ∀ l, runPrims ps (runP q l) = runP q (runPrims ps l) := by
  induction ps with
  | nil => intro l; rfl
  | cons p ps ih =>
    intro l
    simp only [runPrims, List.foldl_cons]
    rw [runP_comm p q (h p (by simp)) l]
    exact ih (fun p' hp' => h p' (List.mem_cons_of_mem _ hp')) _

theorem runPrims_comm (ps qs : List Prim) (h : ∀ p ∈ ps, ∀ q ∈ qs, Indep p q) :
    ∀ l, runPrims ps (runPrims qs l) = runPrims qs (runPrims ps l) := by
  induction qs with
  | nil => intro l; rfl
  | cons q qs ih =>
    intro l
    simp only [runPrims, List.foldl_cons]
    have := ih (fun p hp q' hq' => h p hp q' (List.mem_cons_of_mem _ hq')) (runP q l)
    simp only [runPrims] at this
    rw [this]
    have hs := runPrims_step q ps (fun p hp => h p hp q (by simp)) l
    simp only [runPrims] at hs
    rw [hs]

theorem indep_of_keys (p q : Prim) (h : ∀ e ∈ p.keys, e ∉ q.keys) : Indep p q := by
  cases p <;> cases q <;> simp_all [Prim.keys, Indep] <;> omega

theorem keys_sub (c : Change) (s : Schema) : ∀ p ∈ prims c, ∀ e ∈ p.keys, e ∈ ents0 c s := by
  cases c <;> simp [prims, Prim.keys, ents0]

/-- The object types after two disjoint changes do not depend on the order. -/
theorem applyObjs_comm (c₁ c₂ : Change) (s : Schema) (h : Disjoint c₁ c₂ s) (l : List Obj) :
    applyObjs c₂ (applyObjs c₁ l) = applyObjs c₁ (applyObjs c₂ l) := by
  rw [applyObjs_eq_run, applyObjs_eq_run, applyObjs_eq_run, applyObjs_eq_run]
  apply runPrims_comm
  intro q hq p hp
  exact indep_of_keys q p fun e he he' =>
    h.symm.ents0 e (keys_sub c₂ s q hq e he) (keys_sub c₁ s p hp e he')

/-! ## Dependents: rewrites commute -/

theorem subst_nonrw (c : Change) (h : isRewriting c = false) (e : Read) : e.subst c = e := by
  induction e with
  | cell o f => cases c <;> simp_all [isRewriting, Read.subst, cellSub]
  | join jn a b iha ihb => simp [Read.subst, iha, ihb]

theorem subst_ite (c : Change) (p : Prop) [Decidable p] (a b : Read) :
    Read.subst c (if p then a else b) = if p then Read.subst c a else Read.subst c b := by
  split <;> rfl

/-- Substituting the reads of two changes that touch different cells commutes: the cell one
    relocates is not the cell the other relocates, nor one the other creates. -/
theorem subst_comm (c₁ c₂ : Change) (s : Schema) (h : ∀ e ∈ ents0 c₁ s, e ∉ ents0 c₂ s)
    (e : Read) : (e.subst c₁).subst c₂ = (e.subst c₂).subst c₁ := by
  induction e with
  | join jn a b iha ihb => simp [Read.subst, iha, ihb]
  | cell o f =>
    by_cases hr2 : isRewriting c₂ = true
    · by_cases hr1 : isRewriting c₁ = true
      · cases c₁ with
        | moveField o1 f1 r1 p1 fld1 =>
          cases c₂ with
          | moveField o2 f2 r2 p2 fld2 =>
            simp [ents0] at h
            by_cases a1 : o = o1 ∧ f = f1 <;> by_cases a2 : o = o2 ∧ f = f2 <;>
              simp [Read.subst, cellSub, subst_ite, a1, a2] <;> grind
          | splitField o2 f2 g1 g2 sp2 jn2 =>
            simp [ents0] at h
            by_cases a1 : o = o1 ∧ f = f1 <;> by_cases a2 : o = o2 ∧ f = f2 <;>
              simp [Read.subst, cellSub, subst_ite, a1, a2] <;> grind
          | _ => simp [isRewriting] at hr2
        | splitField o1 f1 g1 g2 sp1 jn1 =>
          cases c₂ with
          | moveField o2 f2 r2 p2 fld2 =>
            simp [ents0] at h
            by_cases a1 : o = o1 ∧ f = f1 <;> by_cases a2 : o = o2 ∧ f = f2 <;>
              simp [Read.subst, cellSub, subst_ite, a1, a2] <;> grind
          | splitField o2 f2 h1 h2 sp2 jn2 =>
            simp [ents0] at h
            by_cases a1 : o = o1 ∧ f = f1 <;> by_cases a2 : o = o2 ∧ f = f2 <;>
              simp [Read.subst, cellSub, subst_ite, a1, a2] <;> grind
          | _ => simp [isRewriting] at hr2
        | _ => simp [isRewriting] at hr1
      · have h1 := subst_nonrw c₁ (by simpa using hr1)
        simp only [h1]
    · have h2 := subst_nonrw c₂ (by simpa using hr2)
      simp only [h2]

theorem rewrite_comm (c₁ c₂ : Change) (s : Schema) (h : ∀ e ∈ ents0 c₁ s, e ∉ ents0 c₂ s)
    (d : Dep) : rewrite c₂ (rewrite c₁ d) = rewrite c₁ (rewrite c₂ d) := by
  cases d with
  | reader r =>
    show Dep.reader (rewriteReader c₂ (rewriteReader c₁ r)) =
      Dep.reader (rewriteReader c₁ (rewriteReader c₂ r))
    congr 1
    simp only [rewriteReader, List.map_map]
    congr 1
    apply List.map_congr_left
    intro e _
    simp only [Function.comp]
    exact subst_comm c₁ c₂ s h e
  | view v => rfl

theorem deps_comm (c₁ c₂ : Change) (s : Schema) (h : ∀ e ∈ ents0 c₁ s, e ∉ ents0 c₂ s) :
    (((s.deps.filter (keepDep c₁)).map (rewrite c₁)).filter (keepDep c₂)).map (rewrite c₂) =
      (((s.deps.filter (keepDep c₂)).map (rewrite c₂)).filter (keepDep c₁)).map (rewrite c₁) := by
  have k1 : ∀ c c' : Change, (keepDep c ∘ rewrite c') = keepDep c := by
    intro c c'; funext d; simp [keepDep]
  rw [List.filter_map, List.filter_map, k1, k1, List.map_map, List.map_map,
    List.filter_filter, List.filter_filter]
  have hf : (fun d => keepDep c₁ d && keepDep c₂ d) = (fun d => keepDep c₂ d && keepDep c₁ d) := by
    funext d; simp [Bool.and_comm]
  rw [hf]
  apply List.map_congr_left
  intro d _
  simp only [Function.comp]
  exact rewrite_comm c₁ c₂ s h d

/-! ## Data: one write per cell -/

/-- What a change writes to cell `(o, f)`: the cell it reads and the function of its value. A
    cell no change writes keeps its value. -/
def wr : Change → Nat → Nat → Option ((Nat × Nat) × (Val → Val))
  | .addField o fld v, o1, f1 =>
    if o1 = o ∧ f1 = fld.id then some ((o1, f1), fun _ => v) else none
  | .narrow o f ty, o1, f1 => if o1 = o ∧ f1 = f then some ((o1, f1), narrowVal ty) else none
  | .removeField o f, o1, f1 => if o1 = o ∧ f1 = f then some ((o1, f1), fun _ => .null) else none
  | .retargetRef o f _, o1, f1 =>
    if o1 = o ∧ f1 = f then some ((o1, f1), fun _ => .null) else none
  | .removeObject o, o1, f1 => if o1 = o then some ((o1, f1), fun _ => .null) else none
  | .moveField o f _ o' fld, o1, f1 =>
    if o1 = o' ∧ f1 = fld.id then some ((o, f), fun v => v)
    else if o1 = o ∧ f1 = f then some ((o1, f1), fun _ => .null) else none
  | .splitField o f f1' f2' sp _, o1, f1 =>
    if o1 = o ∧ f1 = f1'.id then some ((o, f), fun v => (sp v).1)
    else if o1 = o ∧ f1 = f2'.id then some ((o, f), fun v => (sp v).2)
    else if o1 = o ∧ f1 = f then some ((o1, f1), fun _ => .null) else none
  | .retype o f _ enc _, o1, f1 => if o1 = o ∧ f1 = f then some ((o1, f1), enc) else none
  | _, _, _ => none

/-- Migration is the one-step form: each cell is rewritten from its source cell, or left. -/
theorem migrate_wr (c : Change) (data : Data) (o f : Nat) :
    migrate c data o f = match wr c o f with
      | none => data o f
      | some (src, e) => e (data src.1 src.2) := by
  cases c <;> simp only [migrate, wr] <;> split <;> simp_all <;> split <;> simp_all <;>
    split <;> simp_all

theorem migrate_comm_of (c₁ c₂ : Change) (data : Data)
    (hw : ∀ o f : Nat, (wr c₁ o f).isSome → wr c₂ o f = none)
    (hr₁ : ∀ (o f : Nat) (src : Nat × Nat) (e : Val → Val),
      wr c₁ o f = some (src, e) → wr c₂ src.1 src.2 = none)
    (hr₂ : ∀ (o f : Nat) (src : Nat × Nat) (e : Val → Val),
      wr c₂ o f = some (src, e) → wr c₁ src.1 src.2 = none) :
    migrate c₂ (migrate c₁ data) = migrate c₁ (migrate c₂ data) := by
  funext o f
  rw [migrate_wr c₂ (migrate c₁ data) o f, migrate_wr c₁ (migrate c₂ data) o f]
  cases h1 : wr c₁ o f with
  | none =>
    cases h2 : wr c₂ o f with
    | none =>
      simp only [h1, h2]
      rw [migrate_wr c₁ data o f, migrate_wr c₂ data o f, h1, h2]
    | some v =>
      obtain ⟨src, e⟩ := v
      have h3 := hr₂ o f src e h2
      simp only [h1, h2]
      simp only [migrate_wr c₁ data src.1 src.2, h3, migrate_wr c₂ data o f, h2]
  | some v =>
    obtain ⟨src, e⟩ := v
    have h2 : wr c₂ o f = none := hw o f (by simp [h1])
    have h3 := hr₁ o f src e h1
    simp only [h1, h2]
    simp only [migrate_wr c₂ data src.1 src.2, h3, migrate_wr c₁ data o f, h1]

/-! ## Which cells a change owns -/

/-- A cell is owned by a change when the change lists it, or when the change deletes its
    object type. -/
def owns (c : Change) (s : Schema) (p : Nat × Nat) : Prop :=
  Ent.cell p.1 p.2 ∈ ents0 c s ∨ ∃ o, c = .removeObject o ∧ p.1 = o

theorem wr_owns (c : Change) (s : Schema) (o f : Nat) (h : (wr c o f).isSome) :
    owns c s (o, f) := by
  cases c <;> simp only [wr] at h <;> (repeat' split at h) <;> simp_all [owns, ents0]

theorem wr_src_owns (c : Change) (s : Schema) (o f : Nat) (src : Nat × Nat) (e : Val → Val)
    (h : wr c o f = some (src, e)) : owns c s src := by
  cases c <;> simp only [wr] at h <;> (repeat' split at h) <;> (try simp at h) <;>
    (try obtain ⟨rfl, -⟩ := h) <;> simp_all [owns, ents0]

theorem cell_mem_rmObj (s : Schema) (o f : Nat) (h : hasField s o f) :
    Ent.cell o f ∈ ents0 (.removeObject o) s := by
  obtain ⟨ob, hob, hid, hf⟩ := h
  simp only [ents0, footprint, List.mem_cons, List.mem_map, List.mem_flatMap]
  right
  refine ⟨(o, f), ⟨ob, hob, ?_⟩, rfl⟩
  rw [if_pos hid]
  obtain ⟨fl, hfl, rfl⟩ := List.mem_map.1 hf
  exact List.mem_map.2 ⟨fl, hfl, rfl⟩

/-- A cell a change lists either exists in the schema or belongs to an object type the change
    lists. -/
theorem cell_ent_ex_or_obj (c : Change) (s : Schema) (hpre : pre c s) (o f : Nat)
    (h : Ent.cell o f ∈ ents0 c s) : hasField s o f ∨ Ent.obj o ∈ ents0 c s := by
  cases c with
  | addField o0 fld v =>
    simp only [ents0, List.mem_cons, List.not_mem_nil, or_false, reduceCtorEq, Ent.cell.injEq,
      false_or] at h
    obtain ⟨rfl, rfl⟩ := h
    simp [ents0]
  | widen o0 f0 ty =>
    simp only [ents0, List.mem_cons, List.not_mem_nil, or_false, Ent.cell.injEq] at h
    obtain ⟨rfl, rfl⟩ := h
    exact Or.inl hpre.1
  | renameField o0 f0 n =>
    simp only [ents0, List.mem_cons, List.not_mem_nil, or_false, Ent.cell.injEq] at h
    obtain ⟨rfl, rfl⟩ := h
    exact Or.inl hpre
  | removeField o0 f0 =>
    simp only [ents0, List.mem_cons, List.not_mem_nil, or_false, reduceCtorEq, Ent.cell.injEq,
      false_or] at h
    obtain ⟨rfl, rfl⟩ := h
    exact Or.inl hpre
  | narrow o0 f0 ty =>
    simp only [ents0, List.mem_cons, List.not_mem_nil, or_false, Ent.cell.injEq] at h
    obtain ⟨rfl, rfl⟩ := h
    exact Or.inl hpre.1
  | retargetRef o0 f0 t =>
    simp only [ents0, List.mem_cons, List.not_mem_nil, or_false, Ent.cell.injEq] at h
    obtain ⟨rfl, rfl⟩ := h
    exact Or.inl hpre.1
  | retype o0 f0 ty enc dec =>
    simp only [ents0, List.mem_cons, List.not_mem_nil, or_false, Ent.cell.injEq] at h
    obtain ⟨rfl, rfl⟩ := h
    exact Or.inl hpre.1
  | removeDep q => simp [ents0] at h
  | removeObject o0 =>
    left
    simp only [ents0, footprint, List.mem_cons, reduceCtorEq, List.mem_map, List.mem_flatMap,
      false_or] at h
    obtain ⟨p, ⟨ob, hob, hp⟩, hpe⟩ := h
    by_cases hid : ob.id = o0
    · rw [if_pos hid] at hp
      obtain ⟨fl, hfl, rfl⟩ := List.mem_map.1 hp
      simp only [Ent.cell.injEq] at hpe
      obtain ⟨rfl, rfl⟩ := hpe
      exact ⟨ob, hob, hid, List.mem_map.2 ⟨fl, hfl, rfl⟩⟩
    · rw [if_neg hid] at hp
      simp at hp
  | moveField o0 f0 r0 p0 fld =>
    simp only [ents0, List.mem_cons, List.not_mem_nil, or_false, reduceCtorEq, Ent.cell.injEq,
      false_or] at h
    rcases h with ⟨rfl, rfl⟩ | ⟨rfl, rfl⟩ | ⟨rfl, rfl⟩
    · exact Or.inl hpre.1
    · exact Or.inl hpre.2.1
    · right; simp [ents0]
  | splitField o0 f0 f1 f2 sp jn =>
    simp only [ents0, List.mem_cons, List.not_mem_nil, or_false, reduceCtorEq, Ent.cell.injEq,
      false_or] at h
    rcases h with ⟨rfl, rfl⟩ | ⟨rfl, rfl⟩ | ⟨rfl, rfl⟩
    · exact Or.inl hpre.1
    · right; simp [ents0]
    · right; simp [ents0]

/-- A cell a change lists and the object type it deletes share an entity. -/
theorem rm_common (c : Change) (s : Schema) (hpre : pre c s) (p : Nat × Nat)
    (h : Ent.cell p.1 p.2 ∈ ents0 c s) :
    ∃ e, e ∈ ents0 c s ∧ e ∈ ents0 (.removeObject p.1) s := by
  rcases cell_ent_ex_or_obj c s hpre p.1 p.2 h with hf | ho
  · exact ⟨_, h, cell_mem_rmObj s p.1 p.2 hf⟩
  · exact ⟨_, ho, by simp [ents0]⟩

theorem owns_disjoint (c₁ c₂ : Change) (s : Schema) (hd : Disjoint c₁ c₂ s) (h₁ : pre c₁ s)
    (h₂ : pre c₂ s) (p : Nat × Nat) (o₁ : owns c₁ s p) (o₂ : owns c₂ s p) : False := by
  rcases o₁ with a | ⟨o, rfl, hp⟩
  · rcases o₂ with b | ⟨o', rfl, hp'⟩
    · exact hd.ents0 _ a b
    · subst hp'
      obtain ⟨e, he1, he2⟩ := rm_common c₁ s h₁ p a
      exact hd.ents0 e he1 he2
  · rcases o₂ with b | ⟨o', rfl, hp'⟩
    · subst hp
      obtain ⟨e, he1, he2⟩ := rm_common c₂ s h₂ p b
      exact hd.ents0 e he2 he1
    · have : o = o' := hp.symm.trans hp'
      subst this
      exact hd.ents0 (Ent.obj o) (by simp [ents0]) (by simp [ents0])

/-- The data after two disjoint changes does not depend on the order. -/
theorem migrate_comm (c₁ c₂ : Change) (s : Schema) (data : Data) (hd : Disjoint c₁ c₂ s)
    (h₁ : pre c₁ s) (h₂ : pre c₂ s) :
    migrate c₂ (migrate c₁ data) = migrate c₁ (migrate c₂ data) := by
  refine migrate_comm_of c₁ c₂ data ?_ ?_ ?_
  · intro o f hp
    cases h₀ : wr c₂ o f with
    | none => rfl
    | some _ =>
      exact (owns_disjoint c₁ c₂ s hd h₁ h₂ (o, f) (wr_owns c₁ s _ _ hp)
        (wr_owns c₂ s _ _ (by simp [h₀]))).elim
  · intro o f src e h
    cases h₀ : wr c₂ src.1 src.2 with
    | none => rfl
    | some _ =>
      exact (owns_disjoint c₁ c₂ s hd h₁ h₂ src (wr_src_owns c₁ s _ _ _ _ h)
        (wr_owns c₂ s _ _ (by simp [h₀]))).elim
  · intro o f src e h
    cases h₀ : wr c₁ src.1 src.2 with
    | none => rfl
    | some _ =>
      exact (owns_disjoint c₁ c₂ s hd h₁ h₂ src (wr_owns c₁ s _ _ (by simp [h₀]))
        (wr_src_owns c₂ s _ _ _ _ h)).elim

/-! ## Theorem 6 -/

/-- **Commutation (theorem 6).** Two accepted changes made from one base revision whose
    footprints are disjoint (what they touch, with their impact sets, share nothing) apply in
    either order to the same schema and migrate the data to the same data: `apply c₂ (apply c₁ s)
    = apply c₁ (apply c₂ s)` and `migrate c₂ (migrate c₁ data) = migrate c₁ (migrate c₂ data)`.
    The equality is exact, as lists, field order and dependent order included. Each change needs
    only its own acceptance (`pre`) for this; that each is still accepted on the schema the
    other produced is what the revision guard checks, and is not needed for the equality.

    Verifies: MIG-602. -/
theorem commutation (c₁ c₂ : Change) (s : Schema) (data : Data) (hacc₁ : accepted c₁ s)
    (hacc₂ : accepted c₂ s) (hd : Disjoint c₁ c₂ s) :
    apply c₂ (apply c₁ s) = apply c₁ (apply c₂ s) ∧
      migrate c₂ (migrate c₁ data) = migrate c₁ (migrate c₂ data) := by
  refine ⟨?_, migrate_comm c₁ c₂ s data hd hacc₁.1 hacc₂.1⟩
  cases s with
  | mk objs deps pins =>
    have ho := applyObjs_comm c₁ c₂ ⟨objs, deps, pins⟩ hd objs
    have hdp := deps_comm c₁ c₂ ⟨objs, deps, pins⟩ hd.ents0
    simp only [apply]
    rw [ho, hdp]

end Strata.Evolution
