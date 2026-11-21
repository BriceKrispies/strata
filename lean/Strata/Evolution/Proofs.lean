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

import Strata.Evolution.Rewrite

namespace Strata.Evolution

theorem not_hit (c : Change) (s : Schema) (d : Dep) (hd : d ∈ s.deps)
    (hi : d.id ∉ impact c s) : depHit c s d = false := by
  refine Decidable.byContradiction fun h => hi ?_
  simp only [impact, List.mem_map, List.mem_filter]
  exact ⟨d, ⟨hd, by simpa using h⟩, rfl⟩

theorem rewrite_of_unhit (c : Change) (s : Schema) (d : Dep) (hh : depHit c s d = false) :
    rewrite c d = d := by
  cases d with
  | reader r =>
    simp only [depHit, readerHit, Bool.or_eq_false_iff, List.any_eq_false, decide_eq_true_eq] at hh
    exact congrArg Dep.reader (rewriteReader_of_reads c r fun e he =>
      subst_of_avoid c s e (fun p hp => hh.1 p (mem_cells he hp)))
  | view v => rfl

/-- A dependent that nothing the change touches reaches, and that the change does not delete,
    is still valid after it and takes the same value on migrated data. (It is stated for the
    rewritten dependent, which for such a dependent is the dependent itself.) -/
theorem dep_stable (c : Change) (s : Schema) (data : Data) (hpre : pre c s)
    (hall : ∀ d ∈ s.deps, ValidDep s d) (d : Dep) (hd : d ∈ s.deps)
    (hh : depHit c s d = false) (hr : d.id ∉ removedDeps c) :
    ValidDep (apply c s) (rewrite c d) ∧
      evalDep (apply c s) (migrate c data) (rewrite c d) = evalDep s data d := by
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
      rw [findR_apply c s q (hh' q hq).2, Option.isSome_map]
      exact hv q hq
    · simp only [rewrite, evalDep]
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
    deletes a dependent, by `removedDeps c`.

    Verifies: MIG-605. -/
theorem locality (c : Change) (s : Schema) (data : Data) (hwf : WF s) (hacc : accepted c s)
    (d : Dep) (hd : d ∈ s.deps) (hi : d.id ∉ impact c s) (hr : d.id ∉ removedDeps c) :
    d ∈ (apply c s).deps ∧ ValidDep (apply c s) d ∧
      evalDep (apply c s) (migrate c data) d = evalDep s data d := by
  have hh := not_hit c s d hd hi
  have hrw := rewrite_of_unhit c s d hh
  obtain ⟨hv, he⟩ := dep_stable c s data hacc.1 hwf.deps d hd hh hr
  rw [hrw] at hv he
  exact ⟨hrw ▸ mem_deps_apply c s d hd hr, hv, he⟩

/-! ## Closure -/

theorem validTy_apply (c : Change) (s : Schema) (hc : ∀ o, c ≠ .removeObject o) (ty : Ty) :
    validTy (apply c s) ty ↔ validTy s ty := by
  cases ty <;> simp [validTy, hasObj_apply c s _ hc]

/-- The object-level components of well-formedness. -/
structure ObjsWF (s : Schema) : Prop where
  objIds : (s.objs.map Obj.id).Nodup
  fieldIds : ∀ ob ∈ s.objs, (ob.fields.map Field.id).Nodup
  tys : ∀ ob ∈ s.objs, ∀ fl ∈ ob.fields, validTy s fl.ty

theorem validTy_upd (s : Schema) (o : Nat) (g : Obj → Obj) (hg : ∀ ob, (g ob).id = ob.id)
    (ty : Ty) : validTy { s with objs := updObj o g s.objs } ty ↔ validTy s ty := by
  cases ty <;> simp [validTy, hasObj, updObj_map_id o g hg s.objs]

/-- Updating one object by an id-preserving function keeps the first three components of
    well-formedness, given that the updated object's fields stay unique and valid. -/
theorem upd_wf {s : Schema} (hwf : ObjsWF s) (o : Nat) (g : Obj → Obj) (hg : ∀ ob, (g ob).id = ob.id)
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
  have ho : ObjsWF s := ⟨hwf.objIds, hwf.fieldIds, hwf.tys⟩
  cases c with
  | addField o fld v =>
    obtain ⟨_, hfresh, hvt⟩ := hpre
    refine upd_wf ho o _ (by intro _; rfl) ?_ ?_
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
    refine upd_wf ho o _ (by intro _; rfl) ?_ ?_
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
    refine upd_wf ho o _ (by intro _; rfl) ?_ ?_
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
    refine upd_wf ho o _ (by intro _; rfl) ?_ ?_
    · intro ob hob hid
      exact (List.Sublist.map _ (List.filter_sublist)).nodup (hwf.fieldIds ob hob)
    · intro ob hob hid fl hfl
      exact hwf.tys ob hob fl (List.mem_filter.1 hfl).1
  | narrow o f ty =>
    obtain ⟨_, hvt, _⟩ := hpre
    refine upd_wf ho o _ (by intro _; rfl) ?_ ?_
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
    refine upd_wf ho o _ (by intro _; rfl) ?_ ?_
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
  | moveField o f r o' fld =>
    obtain ⟨_, _, _, _, hvt, hfresh, _⟩ := hpre
    let g1 : Obj → Obj := fun ob => { ob with fields := ob.fields.filter (fun fl => fl.id != f) }
    have step1 := upd_wf ho o g1 (fun _ => rfl)
      (fun ob hob _ => (List.Sublist.map _ List.filter_sublist).nodup (hwf.fieldIds ob hob))
      (fun ob hob _ fl hfl => hwf.tys ob hob fl (List.mem_filter.1 hfl).1)
    obtain ⟨a1, b1, c1⟩ := step1
    refine upd_wf (s := { s with objs := updObj o g1 s.objs }) ⟨a1, b1, c1⟩ o'
      (fun ob => { ob with fields := ob.fields ++ [fld] }) (fun _ => rfl) ?_ ?_
    · intro ob' hob' hid'
      obtain ⟨ob, hob, h⟩ := mem_updObj hob'
      simp only [List.map_append, List.map_cons, List.map_nil]
      rw [List.nodup_append]
      refine ⟨b1 ob' hob', by simp, ?_⟩
      intro a ha b hb
      simp only [List.mem_singleton] at hb
      subst hb
      intro hab
      subst hab
      rcases h with ⟨hid, h⟩ | ⟨hne, h⟩
      · rw [h] at ha hid'
        exact hfresh ob hob hid' ((List.Sublist.map _ List.filter_sublist).subset ha)
      · rw [h] at ha hid'
        exact hfresh ob hob hid' ha
    · intro ob' hob' hid' fl hfl
      simp only [List.mem_append, List.mem_singleton] at hfl
      rcases hfl with h | h
      · exact c1 ob' hob' fl h
      · rw [h]
        exact (validTy_upd s o g1 (fun _ => rfl) _).2 hvt
  | splitField o f f1 f2 sp jn =>
    obtain ⟨_, hne, hv1, hv2, hfresh, _⟩ := hpre
    refine upd_wf ho o
      (fun ob => { ob with fields := ob.fields.filter (fun fl => fl.id != f) ++ [f1, f2] })
      (fun _ => rfl) ?_ ?_
    · intro ob hob hid
      simp only [List.map_append, List.map_cons, List.map_nil]
      rw [List.nodup_append]
      refine ⟨(List.Sublist.map _ List.filter_sublist).nodup (hwf.fieldIds ob hob),
        by simp [hne], ?_⟩
      intro a ha b hb
      have hf := hfresh ob hob hid
      have ha' : a ∈ ob.fields.map Field.id := (List.Sublist.map _ List.filter_sublist).subset ha
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hb
      rcases hb with rfl | rfl
      · intro e; exact hf.1 (e ▸ ha')
      · intro e; exact hf.2 (e ▸ ha')
    · intro ob hob hid fl hfl
      simp only [List.mem_append, List.mem_filter, List.mem_cons, List.not_mem_nil,
        or_false] at hfl
      rcases hfl with ⟨h, _⟩ | rfl | rfl
      · exact hwf.tys ob hob fl h
      · exact hv1
      · exact hv2
  | retype o f ty enc dec =>
    obtain ⟨_, hvt⟩ := hpre
    refine upd_wf ho o _ (by intro _; rfl) ?_ ?_
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
        simp only [validTy, hasObj, apply, applyObjs, List.mem_map, List.mem_filter] at h1 ⊢
        obtain ⟨ob', hob', rfl⟩ := h1
        refine ⟨ob', ⟨hob', ?_⟩, rfl⟩
        have : ob'.id ≠ o := fun e => h2 (by rw [e])
        simpa using this
      | _ => simp [validTy]
  | removeDep q =>
    exact ⟨hwf.objIds, hwf.fieldIds, hwf.tys⟩

/-- A change that is not a re-encoding, a move or a split and that is breaking reaches nothing
    once accepted: the dependents it would reach cannot be pinned to a projection, because it
    has none. -/
theorem impact_nil_of_acc (c : Change) (s : Schema) (hacc : accepted c s) (hm : isMono c = false)
    (hr : isRewriting c = false) : impact c s = [] := by
  have hb : classify c = .breaking := by
    cases c <;> simp_all [isMono, isRewriting, classify]
  have hl : ¬ hasLeftInverse c := by
    cases c <;> simp_all [isMono, isRewriting, hasLeftInverse]
  exact List.eq_nil_iff_forall_not_mem.2 fun q hq => hl (hacc.2 hb q hq).2

/-- **Closure.** A well-formed schema stays well-formed under every accepted change. The
    acceptance rule is only local checks, and for a breaking change that every dependent it
    reaches is pinned and the change has a left inverse; it does not itself mention
    well-formedness of the result. A pinned dependent that a breaking change reaches stays valid
    because the changes that have a left inverse lose no cell.

    Verifies: MIG-605. -/
theorem closure (c : Change) (s : Schema) (hwf : WF s) (hacc : accepted c s) :
    WF (apply c s) := by
  have hpre := hacc.1
  obtain ⟨h1, h2, h3⟩ := objs_wf c s hwf hpre
  refine ⟨h1, h2, h3, ?_, ?_⟩
  · have : (apply c s).deps.map Dep.id = (s.deps.filter (keepDep c)).map Dep.id := by
      simp only [apply, List.map_map]
      apply List.map_congr_left
      intro d _
      simp
    rw [this]
    exact (List.Sublist.map _ (List.filter_sublist)).nodup hwf.depIds
  · intro d' hd'
    obtain ⟨d, hd1, hd2, rfl⟩ := mem_deps_apply_imp c s d' hd'
    by_cases hm : isMono c = true
    · rw [rewrite_of_mono c hm]; exact valid_of_mono c s hm d (hwf.deps d hd1)
    · by_cases hrw : isRewriting c = true
      · exact valid_of_rewrite c s hrw hpre d (hwf.deps d hd1)
      · have himp := impact_nil_of_acc c s hacc (by simpa using hm) (by simpa using hrw)
        have hi : d.id ∉ impact c s := by rw [himp]; simp
        exact (dep_stable c s (fun _ _ => .null) hpre hwf.deps d hd1 (not_hit c s d hd1 hi) hd2).1

/-! ## Compatibility -/

/-- **Compatibility.** A compatible change (adding a field or widening a type) needs no rewrite
    and breaks nothing: every dependent valid before is valid after, and evaluates to the same
    value on the migrated data. It needs no impact check, because the change reaches no
    dependent's meaning: adding a field changes no existing cell, a widening changes a type, not
    a value. Adding a field fills a cell that was not there, so the change must be well formed
    (`pre`: the new id is fresh) and the dependents of the schema valid, so that nothing read the
    new cell before.

    Verifies: MIG-590. -/
theorem compatibility (c : Change) (s : Schema) (data : Data) (hc : classify c = .compat)
    (hpre : pre c s) (hall : ∀ d ∈ s.deps, ValidDep s d) (d : Dep) (hd : ValidDep s d) :
    ValidDep (apply c s) d ∧ evalDep (apply c s) (migrate c data) d = evalDep s data d := by
  have hm : isMono c = true := mono_of_compat c hc
  have hnb : classify c ≠ .breaking := by rw [hc]; simp
  refine ⟨valid_of_mono c s hm d hd, ?_⟩
  cases d with
  | reader r => exact eval_reader_mono c s data hpre hm hnb r hd
  | view v =>
    simp only [evalDep, deps_apply_of_mono c s hm]
    congr 1
    apply List.map_congr_left
    intro q hq
    cases hf : findR s.deps q with
    | none => rfl
    | some r => exact eval_reader_mono c s data hpre hm hnb r (hall _ (findR_mem hf))

/-! ## The classification is necessary -/

/-- A concrete schema: one object (id 1) with an `int` field (id 1), and one query (id 10) that
    reads that field and returns it. -/
def exSchema : Schema :=
  { objs := [{ id := 1, name := "item", fields := [{ id := 1, name := "qty", ty := .int }] }]
    deps := [.reader { id := 10, isPolicy := false, base := 1, reads := [.cell 1 1],
                       fn := fun vs => vs.headD .null }] }

def exReader : Dep :=
  .reader { id := 10, isPolicy := false, base := 1, reads := [.cell 1 1],
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
    `-1` to `0`. The acceptance rule refuses the change because the dependent is in its impact and is not
    pinned. -/
theorem breaking_change_can_change_value :
    ∃ (s : Schema) (c : Change) (d : Dep) (data : Data),
      WF s ∧ d ∈ s.deps ∧ classify c = .breaking ∧ d.id ∈ impact c s ∧ ¬ accepted c s ∧
      ¬ (ValidDep (apply c s) d ∧ evalDep (apply c s) (migrate c data) d = evalDep s data d) := by
  refine ⟨exSchema, .narrow 1 1 .nat, exReader, fun _ _ => .num (-1), exSchema_wf, ?_, ?_, ?_, ?_, ?_⟩
  · simp [exSchema, exReader]
  · rfl
  · decide
  · intro h
    have h10 : (10 : Nat) ∈ impact (.narrow 1 1 .nat) exSchema := by decide
    exact absurd ((h.2 rfl) 10 h10).1 (by decide)
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
    have h10 : (10 : Nat) ∈ impact (.removeField 1 1) exSchema := by decide
    exact absurd ((h.2 rfl) 10 h10).1 (by decide)
  · decide

end Strata.Evolution
