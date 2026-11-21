/-
Machine-checked properties of the schema evolution model in `Strata/Evolution/Model.lean`.

Assumptions, all of them, stated once:

E1. The model is the thing reasoned about. Nothing here refers to the Rust code, and it is
    independent of the kernel model in `Strata/Model.lean`.
E2. A dependent is a read set plus a function of the values read. What the function does with
    them is arbitrary; nothing is assumed about it, so the theorems hold for every expression
    language that reads only what it declares.
E3. Data is a context of one record per object. Whether records conform to their field types
    is not part of the model; the migration says what a changed cell becomes.
E4. `accepted` is the local checks (`pre`) plus an empty impact for a breaking change. It is
    deliberately not "the result is well-formed": closure is a theorem about it, not a
    restatement of it. Pins and projections (WG-5327) and concurrency (WG-5330) are absent.
E5. Locality needs `WF s`: in a schema whose dependents already read missing cells there is
    nothing to be local about.

`Audit.lean` prints the axioms each theorem depends on: only Lean's standard three.
-/

import Strata.Evolution.Lemmas

namespace Strata.Evolution

theorem lost_of_mono (c : Change) (h : isMono c = true) (o f : Nat) : ¬ lost c o f := by
  cases c <;> simp_all [isMono, lost]

theorem mem_deps_apply_imp' (c : Change) (s : Schema) (d : Dep) (hd : d ∈ (apply c s).deps) :
    d ∈ s.deps ∧ d.id ∉ removedDeps c := by
  cases c with
  | removeDep x =>
    obtain ⟨h1, h2⟩ := List.mem_filter.1 hd
    refine ⟨h1, ?_⟩
    simpa [removedDeps] using h2
  | _ => exact ⟨hd, by simp [removedDeps]⟩

theorem not_hit (c : Change) (s : Schema) (d : Dep) (hd : d ∈ s.deps)
    (hi : d.id ∉ impact c s) : depHit c s d = false := by
  refine Decidable.byContradiction fun h => hi ?_
  simp only [impact, List.mem_map, List.mem_filter]
  exact ⟨d, ⟨hd, by simpa using h⟩, rfl⟩

/-- A dependent that nothing the change touches reaches, and that the change does not delete,
    is still valid after it and takes the same value on migrated data. -/
theorem dep_stable (c : Change) (s : Schema) (data : Data) (hpre : pre c s)
    (hall : ∀ d ∈ s.deps, ValidDep s d) (d : Dep) (hd : d ∈ s.deps)
    (hh : depHit c s d = false) (hr : d.id ∉ removedDeps c) :
    ValidDep (apply c s) d ∧ evalDep (apply c s) (migrate c data) d = evalDep s data d := by
  cases d with
  | reader r => exact reader_stable c s data hpre r (hall _ hd) hh
  | view v =>
    have hh' : ∀ q ∈ v.uses, idHit c s q = false ∧ q ∉ removedDeps c := by
      intro q hq
      have h := List.any_eq_false.1 hh q hq
      rw [Bool.or_eq_true, not_or] at h
      exact ⟨by simpa using h.1, by simpa using h.2⟩
    have hv := hall _ hd
    refine ⟨?_, ?_⟩
    · intro q hq
      rw [findR_apply c s q (hh' q hq).2]
      exact hv q hq
    · simp only [evalDep]
      congr 1
      apply List.map_congr_left
      intro q hq
      rw [findR_apply c s q (hh' q hq).2]
      cases hf : findR s.deps q with
      | none => rfl
      | some r =>
        have hrh := (hh' q hq).1
        simp only [idHit, hf] at hrh
        exact (reader_stable c s data hpre r (hall _ (findR_mem hf)) hrh).2

/-- **Locality.** An accepted change leaves every dependent it does not reach as it was: the
    dependent is still in the schema (unless the change deletes it by name), is still valid,
    and evaluates to the same value on the migrated data as on the original. A query nobody
    changed under does not change its answer. Needs `WF s`.

    The dependents the change reaches are listed by `impact c s`; the one it deletes, if it
    deletes a dependent, by `removedDeps c`. -/
theorem locality (c : Change) (s : Schema) (data : Data) (hwf : WF s) (hacc : accepted c s)
    (d : Dep) (hd : d ∈ s.deps) (hi : d.id ∉ impact c s) (hr : d.id ∉ removedDeps c) :
    d ∈ (apply c s).deps ∧ ValidDep (apply c s) d ∧
      evalDep (apply c s) (migrate c data) d = evalDep s data d := by
  obtain ⟨hv, he⟩ := dep_stable c s data hacc.1 hwf.deps d hd (not_hit c s d hd hi) hr
  exact ⟨mem_deps_apply c s d hd hr, hv, he⟩

/-! ## Closure -/

theorem validTy_apply (c : Change) (s : Schema) (hc : ∀ o, c ≠ .removeObject o) (ty : Ty) :
    validTy (apply c s) ty ↔ validTy s ty := by
  cases ty <;> simp [validTy, hasObj_apply c s _ hc]

/-- Updating one object by an id-preserving function keeps the first three components of
    well-formedness, given that the updated object's fields stay unique and valid. -/
theorem upd_wf {s : Schema} (hwf : WF s) (o : Nat) (g : Obj → Obj) (hg : ∀ ob, (g ob).id = ob.id)
    (hfn : ∀ ob ∈ s.objs, ob.id = o → ((g ob).fields.map Field.id).Nodup)
    (hty : ∀ ob ∈ s.objs, ob.id = o → ∀ fl ∈ (g ob).fields, validTy s fl.ty) :
    let s' : Schema := { s with objs := updObj o g s.objs }
    (s'.objs.map Obj.id).Nodup ∧ (∀ ob ∈ s'.objs, (ob.fields.map Field.id).Nodup) ∧
      ∀ ob ∈ s'.objs, ∀ fl ∈ ob.fields, validTy s' fl.ty := by
  intro s'
  have hids : (s'.objs.map Obj.id) = s.objs.map Obj.id := updObj_map_id o g hg s.objs
  have hvt : ∀ ty, validTy s' ty ↔ validTy s ty := by
    intro ty; cases ty <;> simp [validTy, hasObj, hids]
  refine ⟨by rw [hids]; exact hwf.objIds, ?_, ?_⟩
  · intro ob' hob'
    obtain ⟨ob, hob, h | h⟩ := mem_updObj hob'
    · rw [h.2]; exact hfn ob hob h.1
    · rw [h.2]; exact hwf.fieldIds ob hob
  · intro ob' hob' fl hfl
    rw [hvt]
    obtain ⟨ob, hob, h | h⟩ := mem_updObj hob'
    · rw [h.2] at hfl; exact hty ob hob h.1 fl hfl
    · rw [h.2] at hfl; exact hwf.tys ob hob fl hfl

theorem sublist_nodup {α : Type} {l₁ l₂ : List α} (h : l₁.Sublist l₂) (hn : l₂.Nodup) :
    l₁.Nodup := hn.sublist h

/-- The first three components of well-formedness are preserved by every change that passes
    its local checks. -/
theorem objs_wf (c : Change) (s : Schema) (hwf : WF s) (hpre : pre c s) :
    ((apply c s).objs.map Obj.id).Nodup ∧
      (∀ ob ∈ (apply c s).objs, (ob.fields.map Field.id).Nodup) ∧
      ∀ ob ∈ (apply c s).objs, ∀ fl ∈ ob.fields, validTy (apply c s) fl.ty := by
  cases c with
  | addField o fld v =>
    obtain ⟨_, hfresh, hvt⟩ := hpre
    refine upd_wf hwf o _ (by intro _; rfl) ?_ ?_
    · intro ob hob hid
      simp only [List.map_append, List.map_cons, List.map_nil]
      rw [List.nodup_append]
      refine ⟨hwf.fieldIds ob hob, by simp, ?_⟩
      intro a ha b hb
      simp only [List.mem_singleton] at hb
      subst hb
      intro hab
      subst hab
      exact hfresh ob hob hid ha
    · intro ob hob hid fl hfl
      simp only [List.mem_append, List.mem_singleton] at hfl
      rcases hfl with h | h
      · exact hwf.tys ob hob fl h
      · rw [h]; exact hvt
  | widen o f ty =>
    obtain ⟨_, hvt, _⟩ := hpre
    refine upd_wf hwf o _ (by intro _; rfl) ?_ ?_
    · intro ob hob hid
      simp only [updField]
      rw [updField_map_id _ _ (by intro _; rfl)]
      exact hwf.fieldIds ob hob
    · intro ob hob hid fl hfl
      simp only [updField, List.mem_map] at hfl
      obtain ⟨fl0, hfl0, rfl⟩ := hfl
      by_cases hf : fl0.id = f
      · simpa [hf] using hvt
      · simpa [hf] using hwf.tys ob hob fl0 hfl0
  | renameField o f n =>
    refine upd_wf hwf o _ (by intro _; rfl) ?_ ?_
    · intro ob hob hid
      simp only [updField]
      rw [updField_map_id _ _ (by intro _; rfl)]
      exact hwf.fieldIds ob hob
    · intro ob hob hid fl hfl
      simp only [updField, List.mem_map] at hfl
      obtain ⟨fl0, hfl0, rfl⟩ := hfl
      by_cases hf : fl0.id = f
      · simpa [hf] using hwf.tys ob hob fl0 hfl0
      · simpa [hf] using hwf.tys ob hob fl0 hfl0
  | removeField o f =>
    refine upd_wf hwf o _ (by intro _; rfl) ?_ ?_
    · intro ob hob hid
      exact (List.Sublist.map _ (List.filter_sublist)).nodup (hwf.fieldIds ob hob)
    · intro ob hob hid fl hfl
      exact hwf.tys ob hob fl (List.mem_filter.1 hfl).1
  | narrow o f ty =>
    obtain ⟨_, hvt, _⟩ := hpre
    refine upd_wf hwf o _ (by intro _; rfl) ?_ ?_
    · intro ob hob hid
      simp only [updField]
      rw [updField_map_id _ _ (by intro _; rfl)]
      exact hwf.fieldIds ob hob
    · intro ob hob hid fl hfl
      simp only [updField, List.mem_map] at hfl
      obtain ⟨fl0, hfl0, rfl⟩ := hfl
      by_cases hf : fl0.id = f
      · simpa [hf] using hvt
      · simpa [hf] using hwf.tys ob hob fl0 hfl0
  | retargetRef o f t =>
    obtain ⟨_, hvt, _⟩ := hpre
    refine upd_wf hwf o _ (by intro _; rfl) ?_ ?_
    · intro ob hob hid
      simp only [updField]
      rw [updField_map_id _ _ (by intro _; rfl)]
      exact hwf.fieldIds ob hob
    · intro ob hob hid fl hfl
      simp only [updField, List.mem_map] at hfl
      obtain ⟨fl0, hfl0, rfl⟩ := hfl
      by_cases hf : fl0.id = f
      · simpa [hf, validTy] using hvt
      · simpa [hf] using hwf.tys ob hob fl0 hfl0
  | removeObject o =>
    obtain ⟨_, hnoref⟩ := hpre
    refine ⟨?_, ?_, ?_⟩
    · exact (List.Sublist.map _ (List.filter_sublist)).nodup hwf.objIds
    · intro ob hob
      exact hwf.fieldIds ob (List.mem_filter.1 hob).1
    · intro ob hob fl hfl
      obtain ⟨hob1, hob2⟩ := List.mem_filter.1 hob
      have hne : ob.id ≠ o := by simpa using hob2
      have h1 := hwf.tys ob hob1 fl hfl
      have h2 := hnoref ob hob1 hne fl hfl
      cases hty : fl.ty with
      | ref t =>
        rw [hty] at h1 h2
        simp only [validTy, hasObj, apply, List.mem_map, List.mem_filter] at h1 ⊢
        obtain ⟨ob', hob', rfl⟩ := h1
        refine ⟨ob', ⟨hob', ?_⟩, rfl⟩
        have : ob'.id ≠ o := fun e => h2 (by rw [e])
        simpa using this
      | _ => simp [validTy]
  | removeDep q =>
    exact ⟨hwf.objIds, hwf.fieldIds, hwf.tys⟩

/-- Under a change that is neither a removal nor a reinterpretation (rename, widen, add a
    field), every valid dependent stays valid. -/
theorem valid_of_mono (c : Change) (s : Schema) (hm : isMono c = true) (d : Dep)
    (hv : ValidDep s d) : ValidDep (apply c s) d := by
  cases d with
  | reader r =>
    obtain ⟨hb, hr⟩ := hv
    exact ⟨hasObj_apply' c s _ hb (objGone_of_mono c hm _),
      fun p hp => hasField_apply c s _ _ (hr p hp) (lost_of_mono c hm _ _)⟩
  | view v =>
    intro q hq
    rw [findR_apply c s q (by simp [removedDeps_of_mono c hm])]
    exact hv q hq

/-- **Closure.** A well-formed schema stays well-formed under every accepted change. The
    acceptance rule is only local checks and an empty impact for a breaking change; it does not
    itself mention well-formedness of the result. -/
theorem closure (c : Change) (s : Schema) (hwf : WF s) (hacc : accepted c s) :
    WF (apply c s) := by
  obtain ⟨hpre, hbreak⟩ := hacc
  obtain ⟨h1, h2, h3⟩ := objs_wf c s hwf hpre
  refine ⟨h1, h2, h3, ?_, ?_⟩
  · by_cases hm : isMono c = true
    · rw [deps_apply_of_ne c s (removedDeps_of_mono c hm)]; exact hwf.depIds
    · cases c with
      | removeDep q =>
        exact (List.Sublist.map _ (List.filter_sublist)).nodup hwf.depIds
      | _ => first | exact hwf.depIds | simp [isMono] at hm
  · intro d hd
    obtain ⟨hd1, hd2⟩ := mem_deps_apply_imp' c s d hd
    by_cases hm : isMono c = true
    · exact valid_of_mono c s hm d (hwf.deps d hd1)
    · have hb : classify c = .breaking :=
        Decidable.byContradiction fun hnb => hm (mono_of_not_breaking c hnb)
      have himp := hbreak hb
      have hi : d.id ∉ impact c s := by rw [himp]; simp
      exact (dep_stable c s (fun _ _ => .null) hpre hwf.deps d hd1 (not_hit c s d hd1 hi) hd2).1

/-! ## Compatibility -/

/-- **Compatibility.** A compatible change (a rename or a widening) needs no migration and
    breaks nothing: every dependent valid before is valid after, and evaluates to the same
    value on the migrated data (which is the data). It needs no acceptance check beyond being
    compatible, and no impact check, because the change reaches no dependent's meaning: a rename
    changes a label, not an id, and a widening changes a type, not a value. -/
theorem compatibility (c : Change) (s : Schema) (data : Data) (hc : classify c = .compat)
    (d : Dep) (hd : ValidDep s d) :
    ValidDep (apply c s) d ∧ evalDep (apply c s) (migrate c data) d = evalDep s data d := by
  have hm : isMono c = true := mono_of_not_breaking c (by rw [hc]; simp)
  refine ⟨valid_of_mono c s hm d hd, ?_⟩
  cases c with
  | widen _ _ _ => cases d <;> rfl
  | renameField _ _ _ => cases d <;> rfl
  | _ => simp [classify] at hc

/-! ## The classification is necessary -/

/-- A concrete schema: one object (id 1) with an `int` field (id 1), and one query (id 10) that
    reads that field and returns it. -/
def exSchema : Schema :=
  { objs := [{ id := 1, name := "item", fields := [{ id := 1, name := "qty", ty := .int }] }]
    deps := [.reader { id := 10, isPolicy := false, base := 1, reads := [(1, 1)],
                       fn := fun vs => vs.headD .null }] }

def exReader : Dep :=
  .reader { id := 10, isPolicy := false, base := 1, reads := [(1, 1)],
            fn := fun vs => vs.headD .null }

theorem exSchema_wf : WF exSchema := by
  refine ⟨by decide, by decide, by decide, by decide, ?_⟩
  intro d hd
  simp only [exSchema, List.mem_singleton] at hd
  subst hd
  decide

/-- **The classification is necessary.** There is a well-formed schema, a breaking change, and
    a dependent that reads what it breaks, such that applying the change anyway (ignoring the
    classification) changes the dependent's value: narrowing `int` to `nat` clamps a stored
    `-1` to `0`. The acceptance rule refuses the change because the dependent is in its impact. -/
theorem breaking_change_can_change_value :
    ∃ (s : Schema) (c : Change) (d : Dep) (data : Data),
      WF s ∧ d ∈ s.deps ∧ classify c = .breaking ∧ d.id ∈ impact c s ∧ ¬ accepted c s ∧
      ¬ (ValidDep (apply c s) d ∧ evalDep (apply c s) (migrate c data) d = evalDep s data d) := by
  refine ⟨exSchema, .narrow 1 1 .nat, exReader, fun _ _ => .num (-1), exSchema_wf, ?_, ?_, ?_, ?_, ?_⟩
  · simp [exSchema, exReader]
  · rfl
  · decide
  · intro h
    have := h.2 rfl
    revert this
    decide
  · decide

/-- The same, for a removal: applying the removal anyway leaves the dependent reading a field
    that no longer exists. -/
theorem breaking_change_can_invalidate :
    ∃ (s : Schema) (c : Change) (d : Dep) (data : Data),
      WF s ∧ d ∈ s.deps ∧ classify c = .breaking ∧ d.id ∈ impact c s ∧ ¬ accepted c s ∧
      ¬ (ValidDep (apply c s) d ∧ evalDep (apply c s) (migrate c data) d = evalDep s data d) := by
  refine ⟨exSchema, .removeField 1 1, exReader, fun _ _ => .num 7, exSchema_wf, ?_, ?_, ?_, ?_, ?_⟩
  · simp [exSchema, exReader]
  · rfl
  · decide
  · intro h
    have := h.2 rfl
    revert this
    decide
  · decide

end Strata.Evolution
