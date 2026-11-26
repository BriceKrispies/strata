/-
Lemmas about the workflow decisions of slice 8 (`fireOne`, `fireAll`, `findTransition`, `applySets`).

They are kept apart from `Strata/Workflows.lean`, which states the theorems, so that the files that
decompose an accepted update (`StepParts.lean`, and the theorems built on it) can use them without
importing the theorems.
-/
import Strata.Model

namespace Strata

/-- A tenant none of whose transitions assigns a field: a workflow then changes the record only by its
    checks. Every transition of every workflow of every object type has no assignments. -/
def Tenant.Plain (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ w ∈ o.workflows, ∀ tr ∈ w.transitions, tr.set = []

/-- The transition `findTransition` gives is one of the transitions searched. -/
theorem findTransition_mem (env : ExprEnv) (was becomes : Val) :
    ∀ (trs : List WfTransition) (tr : WfTransition),
      findTransition env was becomes trs = some tr → tr ∈ trs
  | [], tr, h => by simp [findTransition] at h
  | t :: ts, tr, h => by
    unfold findTransition at h
    split at h
    · cases h; exact List.mem_cons_self
    · exact List.mem_cons_of_mem _ (findTransition_mem env was becomes ts tr h)

/-- Without assignments a workflow leaves the candidate as it is: a successful step returns it. -/
theorem fireOne_plain {p : Principal} {old cand : Record} {w : Workflow} {r : Record}
    (hw : ∀ tr ∈ w.transitions, tr.set = []) (h : fireOne p old cand w = .ok r) : r = cand := by
  unfold fireOne at h
  dsimp only at h
  split at h
  · cases h; rfl
  · split at h
    · cases h
    · rename_i tr hT
      have hs := hw tr (findTransition_mem _ _ _ _ _ hT)
      rw [hs] at h
      simp only [evalSets, applySets, Except.ok.injEq] at h
      subst h
      rfl

/-- Without assignments, every workflow of the type leaves the candidate as it is. -/
theorem fireAll_plain {p : Principal} {old cand : Record} :
    ∀ (ws : List Workflow) (r : Record),
      (∀ w ∈ ws, ∀ tr ∈ w.transitions, tr.set = []) → fireAll p old cand ws = .ok r → r = cand
  | [], r, _, h => by
    simp only [fireAll, Except.ok.injEq] at h
    exact h.symm
  | w :: ws, r, hw, h => by
    unfold fireAll at h
    split at h
    · cases h
    · rename_i c hc
      have hc' : c = cand := fireOne_plain (hw w List.mem_cons_self) hc
      subst hc'
      exact fireAll_plain ws r (fun w' hw' => hw w' (List.mem_cons_of_mem _ hw')) h

/-- A new record whose workflow field is an initial state holds a state of the workflow. -/
theorem wfStateOk_of_initial {w : Workflow} {r : Record} (h : wfInitialOk w r = true) :
    wfStateOk w r = true := by
  unfold wfInitialOk at h
  unfold wfStateOk
  cases hv : r.fields.get w.field with
  | null => simp [hv] at h
  | int i => simp [hv] at h
  | bool b => simp [hv] at h
  | ref x => simp [hv] at h
  | list vs => simp [hv] at h
  | str s =>
    simp only [hv] at h ⊢
    have hs : s ∈ w.initial := by simpa using h
    simp [Workflow.states, hs]

/-- The assignments change fields, never the version. -/
theorem applySets_version : ∀ (as : List (FieldId × Val)) (r : Record),
    (applySets r as).version = r.version
  | [], _ => rfl
  | fv :: as, r => by
    show (applySets { r with fields := Fields.set r.fields fv } as).version = r.version
    rw [applySets_version as]

/-- A workflow changes fields, never the version. -/
theorem fireOne_version {p : Principal} {old cand : Record} {w : Workflow} {r : Record}
    (h : fireOne p old cand w = .ok r) : r.version = cand.version := by
  unfold fireOne at h
  dsimp only at h
  split at h
  · cases h; rfl
  · split at h
    · cases h
    · split at h
      · cases h
      · have h2 := Except.ok.inj h
        rw [← h2, applySets_version]

/-- Every workflow of the type changes fields, never the version. -/
theorem fireAll_version {p : Principal} {old : Record} {ws : List Workflow} {cand r : Record}
    (h : fireAll p old cand ws = .ok r) : r.version = cand.version := by
  induction ws generalizing cand with
  | nil =>
    simp only [fireAll, Except.ok.injEq] at h
    rw [← h]
  | cons w ws ih =>
    unfold fireAll at h
    split at h
    · cases h
    · rename_i c hc
      rw [ih h, fireOne_version hc]

/-- Setting a field keeps the field names distinct. -/
theorem fields_set_nodup (fs : Fields) (fv : FieldId × Val)
    (h : (fs.map Prod.fst).Nodup) : ((Fields.set fs fv).map Prod.fst).Nodup := by
  unfold Fields.set
  split
  · exact h.sublist (List.Sublist.map _ List.filter_sublist)
  · simp only [List.map_cons, List.nodup_cons]
    constructor
    · intro hmem
      obtain ⟨g, hg, hgf⟩ := List.mem_map.mp hmem
      have hne := (List.mem_filter.mp hg).2
      have : g.1 = fv.1 := by simpa using hgf
      simp [this] at hne
    · exact h.sublist (List.Sublist.map _ List.filter_sublist)

/-- Assignments keep the field names distinct. -/
theorem applySets_nodup : ∀ (as : List (FieldId × Val)) (r : Record),
    (r.fields.map Prod.fst).Nodup → ((applySets r as).fields.map Prod.fst).Nodup
  | [], _, h => h
  | fv :: as, r, h =>
    applySets_nodup as { r with fields := Fields.set r.fields fv } (fields_set_nodup _ _ h)

/-- A workflow keeps the field names distinct. -/
theorem fireOne_nodup {p : Principal} {old cand : Record} {w : Workflow} {r : Record}
    (hc : (cand.fields.map Prod.fst).Nodup) (h : fireOne p old cand w = .ok r) :
    (r.fields.map Prod.fst).Nodup := by
  unfold fireOne at h
  dsimp only at h
  split at h
  · cases h; exact hc
  · split at h
    · cases h
    · split at h
      · cases h
      · have h2 := Except.ok.inj h
        rw [← h2]
        exact applySets_nodup _ _ hc

/-- Every workflow of the type keeps the field names distinct. -/
theorem fireAll_nodup {p : Principal} {old : Record} {ws : List Workflow} {cand r : Record}
    (hc : (cand.fields.map Prod.fst).Nodup) (h : fireAll p old cand ws = .ok r) :
    (r.fields.map Prod.fst).Nodup := by
  induction ws generalizing cand with
  | nil =>
    simp only [fireAll, Except.ok.injEq] at h
    rw [← h]
    exact hc
  | cons w ws ih =>
    unfold fireAll at h
    split at h
    · cases h
    · rename_i c hc'
      exact ih (fireOne_nodup hc hc') h

/-- A record that starts in initial states of every workflow holds states of every workflow. -/
theorem statesOk_of_initials {ws : List Workflow} {r : Record} (h : initialsOk ws r = true) :
    statesOk ws r = true := by
  unfold initialsOk at h
  unfold statesOk
  rw [List.all_eq_true] at h ⊢
  intro w hw
  exact wfStateOk_of_initial (by simpa using h w hw)

end Strata
