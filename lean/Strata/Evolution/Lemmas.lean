/-
Supporting lemmas for the schema evolution proofs: how `apply` and `migrate` act on lookups.
-/

import Strata.Evolution.Model

namespace Strata.Evolution

/-- Whether the change makes cell `(o, f)` stop existing. -/
def lost : Change → Nat → Nat → Prop
  | .removeField o' f', o, f => o' = o ∧ f' = f
  | .removeObject o', o, _ => o' = o
  | .moveField o' f' _ _ _, o, f => o' = o ∧ f' = f
  | .splitField o' f' .., o, f => o' = o ∧ f' = f
  | _, _, _ => False

/-- The changes that never remove or lose an existing cell, dependent or object and that
    rewrite no dependent: adding a field, widening, renaming, re-encoding. -/
def isMono : Change → Bool
  | .addField .. => true
  | .widen .. => true
  | .renameField .. => true
  | .retype .. => true
  | _ => false

theorem mono_of_compat (c : Change) (h : classify c = .compat) : isMono c = true := by
  cases c <;> simp_all [classify, isMono]

theorem removedDeps_of_mono (c : Change) (h : isMono c = true) : removedDeps c = [] := by
  cases c <;> simp_all [isMono, removedDeps]

theorem objGone_of_mono (c : Change) (h : isMono c = true) (b : Nat) : objGone c b = false := by
  cases c <;> simp_all [isMono, objGone]

theorem subst_of_mono (c : Change) (h : isMono c = true) (e : Read) : e.subst c = e := by
  induction e with
  | cell o f => cases c <;> simp_all [isMono, Read.subst, cellSub]
  | join jn a b iha ihb => simp [Read.subst, iha, ihb]

theorem rewriteReader_of_reads (c : Change) (r : Reader)
    (h : ∀ e ∈ r.reads, e.subst c = e) : rewriteReader c r = r := by
  have : r.reads.map (Read.subst c) = r.reads :=
    (List.map_congr_left (g := id) h).trans (List.map_id _)
  simp only [rewriteReader, this]

theorem rewrite_of_mono (c : Change) (h : isMono c = true) (d : Dep) : rewrite c d = d := by
  cases d with
  | reader r => exact congrArg Dep.reader (rewriteReader_of_reads c r fun e _ => subst_of_mono c h e)
  | view v => rfl

theorem subst_of_avoid (c : Change) (s : Schema) (e : Read)
    (h : ∀ p ∈ e.cells, p ∉ footprint c s) : e.subst c = e := by
  induction e with
  | cell o f =>
    have hp := h (o, f) (by simp [Read.cells])
    cases c <;> simp_all [Read.subst, cellSub, footprint]
  | join jn a b iha ihb =>
    simp only [Read.subst, Read.cells, List.mem_append] at *
    rw [iha (fun p hp => h p (Or.inl hp)), ihb (fun p hp => h p (Or.inr hp))]

theorem eval_congr (e : Read) (d1 d2 : Data) (h : ∀ p ∈ e.cells, d1 p.1 p.2 = d2 p.1 p.2) :
    e.eval d1 = e.eval d2 := by
  induction e with
  | cell o f => exact h (o, f) (by simp [Read.cells])
  | join jn a b iha ihb =>
    simp only [Read.eval]
    rw [iha (fun p hp => h p (by simp [Read.cells, hp])),
      ihb (fun p hp => h p (by simp [Read.cells, hp]))]

theorem mem_cells {r : Reader} {e : Read} (he : e ∈ r.reads) {p : Nat × Nat} (hp : p ∈ e.cells) :
    p ∈ r.cells := List.mem_flatMap.2 ⟨e, he, hp⟩

theorem updObj_map_id (o : Nat) (g : Obj → Obj) (hg : ∀ ob, (g ob).id = ob.id) (objs : List Obj) :
    (updObj o g objs).map Obj.id = objs.map Obj.id := by
  simp only [updObj, List.map_map]
  apply List.map_congr_left
  intro ob _
  simp only [Function.comp]
  split <;> simp [hg]

theorem updField_map_id (f : Nat) (g : Field → Field) (hg : ∀ fl, (g fl).id = fl.id)
    (fields : List Field) :
    (fields.map fun fl => if fl.id = f then g fl else fl).map Field.id = fields.map Field.id := by
  simp only [List.map_map]
  apply List.map_congr_left
  intro fl _
  simp only [Function.comp]
  split <;> simp [hg]

theorem mem_updObj {o : Nat} {g : Obj → Obj} {objs : List Obj} {ob' : Obj}
    (h : ob' ∈ updObj o g objs) :
    ∃ ob ∈ objs, (ob.id = o ∧ ob' = g ob) ∨ (ob.id ≠ o ∧ ob' = ob) := by
  simp only [updObj, List.mem_map] at h
  obtain ⟨ob, hob, rfl⟩ := h
  by_cases hid : ob.id = o
  · exact ⟨ob, hob, Or.inl ⟨hid, by simp [hid]⟩⟩
  · exact ⟨ob, hob, Or.inr ⟨hid, by simp [hid]⟩⟩

theorem updObj_mem_self {o : Nat} {g : Obj → Obj} {objs : List Obj} {ob : Obj} (h : ob ∈ objs) :
    (if ob.id = o then g ob else ob) ∈ updObj o g objs := by
  simp only [updObj, List.mem_map]
  exact ⟨ob, h, rfl⟩

/-! ## What `apply` does to dependents and to existence -/

@[simp] theorem Dep.id_reader (r : Reader) : (Dep.reader r).id = r.id := rfl
@[simp] theorem Dep.id_view (v : View) : (Dep.view v).id = v.id := rfl

@[simp] theorem rewrite_id (c : Change) (d : Dep) : (rewrite c d).id = d.id := by
  cases d <;> rfl

theorem deps_apply_of_mono (c : Change) (s : Schema) (h : isMono c = true) :
    (apply c s).deps = s.deps := by
  have hk : s.deps.filter (keepDep c) = s.deps := by
    apply List.filter_eq_self.2
    intro d _
    simp [keepDep, removedDeps_of_mono c h]
  simp only [apply, hk]
  exact (List.map_congr_left (g := id) (fun d _ => rewrite_of_mono c h d)).trans (List.map_id _)

theorem findR_filter_keep (p : Dep → Bool) (q : Nat) (h : ∀ d : Dep, d.id = q → p d = true) :
    ∀ ds : List Dep, findR (ds.filter p) q = findR ds q := by
  intro ds
  induction ds with
  | nil => rfl
  | cons d ds ih =>
    cases d with
    | reader r =>
      by_cases hr : r.id = q
      · have := h (.reader r) hr
        simp [List.filter_cons, this, findR, hr]
      · by_cases hp : p (.reader r) = true
        · simp [List.filter_cons, hp, findR, hr, ih]
        · simp [List.filter_cons, hp, findR, hr, ih]
    | view v =>
      by_cases hp : p (.view v) = true
      · simp [List.filter_cons, hp, findR, ih]
      · simp [List.filter_cons, hp, findR, ih]

theorem findR_map_rewrite (c : Change) (q : Nat) :
    ∀ ds : List Dep, findR (ds.map (rewrite c)) q = (findR ds q).map (rewriteReader c) := by
  intro ds
  induction ds with
  | nil => rfl
  | cons d ds ih =>
    cases d with
    | reader r =>
      by_cases hr : r.id = q
      · simp [findR, rewrite, rewriteReader, hr]
      · simp [findR, rewrite, rewriteReader, hr, ih]
    | view v => simp [findR, rewrite, ih]

theorem findR_apply (c : Change) (s : Schema) (q : Nat) (h : q ∉ removedDeps c) :
    findR (apply c s).deps q = (findR s.deps q).map (rewriteReader c) := by
  unfold apply
  simp only
  rw [findR_map_rewrite, findR_filter_keep (keepDep c) q]
  intro d hd
  simp [keepDep, hd, h]

theorem findR_mem {ds : List Dep} {q : Nat} {r : Reader} (h : findR ds q = some r) :
    Dep.reader r ∈ ds := by
  induction ds with
  | nil => simp [findR] at h
  | cons d ds ih =>
    cases d with
    | reader r' =>
      by_cases hr : r'.id = q
      · simp only [findR, hr, ite_true, Option.some.injEq] at h
        subst h; simp
      · simp only [findR, hr, ite_false] at h
        exact List.mem_cons_of_mem _ (ih h)
    | view v => exact List.mem_cons_of_mem _ (ih (by simpa [findR] using h))

theorem mem_deps_apply (c : Change) (s : Schema) (d : Dep) (hd : d ∈ s.deps)
    (hr : d.id ∉ removedDeps c) : rewrite c d ∈ (apply c s).deps := by
  simp only [apply, List.mem_map, List.mem_filter]
  exact ⟨d, ⟨hd, by simpa [keepDep] using hr⟩, rfl⟩

theorem mem_deps_apply_imp (c : Change) (s : Schema) (d' : Dep) (hd : d' ∈ (apply c s).deps) :
    ∃ d ∈ s.deps, d.id ∉ removedDeps c ∧ d' = rewrite c d := by
  simp only [apply, List.mem_map, List.mem_filter] at hd
  obtain ⟨d, ⟨h1, h2⟩, rfl⟩ := hd
  exact ⟨d, h1, by simpa [keepDep] using h2, rfl⟩

theorem applyObjs_map_id (c : Change) (hc : ∀ o, c ≠ .removeObject o) (objs : List Obj) :
    (applyObjs c objs).map Obj.id = objs.map Obj.id := by
  cases c with
  | addField o fld v => exact updObj_map_id _ _ (by intro _; rfl) _
  | widen o f ty => exact updObj_map_id _ _ (by intro _; rfl) _
  | renameField o f n => exact updObj_map_id _ _ (by intro _; rfl) _
  | removeField o f => exact updObj_map_id _ _ (by intro _; rfl) _
  | narrow o f ty => exact updObj_map_id _ _ (by intro _; rfl) _
  | retargetRef o f t => exact updObj_map_id _ _ (by intro _; rfl) _
  | removeObject o => exact absurd rfl (hc o)
  | removeDep q => rfl
  | moveField o f r o' fld =>
    show (updObj o' _ (updObj o _ objs)).map Obj.id = _
    rw [updObj_map_id _ _ (by intro _; rfl), updObj_map_id _ _ (by intro _; rfl)]
  | splitField o f f1 f2 sp jn => exact updObj_map_id _ _ (by intro _; rfl) _
  | retype o f ty enc dec => exact updObj_map_id _ _ (by intro _; rfl) _

/-- Every change except deleting an object keeps the set of object ids. -/
theorem hasObj_apply (c : Change) (s : Schema) (t : Nat) (hc : ∀ o, c ≠ .removeObject o) :
    hasObj (apply c s) t ↔ hasObj s t := by
  unfold hasObj
  show t ∈ (applyObjs c s.objs).map Obj.id ↔ _
  rw [applyObjs_map_id c hc]

/-- An object that is not deleted still exists. -/
theorem hasObj_apply' (c : Change) (s : Schema) (t : Nat) (h : hasObj s t)
    (hg : objGone c t = false) : hasObj (apply c s) t := by
  cases c with
  | removeObject o =>
    simp only [objGone, decide_eq_false_iff_not] at hg
    unfold hasObj at *
    simp only [apply, applyObjs, List.mem_map, List.mem_filter] at *
    obtain ⟨ob, hob, rfl⟩ := h
    exact ⟨ob, ⟨hob, by simpa using hg⟩, rfl⟩
  | _ => exact (hasObj_apply _ s t (by simp)).2 h

theorem hasField_upd (s s' : Schema) (o' o f : Nat) (g : Obj → Obj)
    (hg : ∀ ob, (g ob).id = ob.id) (hs : s'.objs = updObj o' g s.objs) (h : hasField s o f)
    (hk : o' = o → ∀ ob, ob.id = o → f ∈ ob.fields.map Field.id →
      f ∈ (g ob).fields.map Field.id) :
    hasField s' o f := by
  obtain ⟨ob, hob, hid, hf⟩ := h
  refine ⟨if ob.id = o' then g ob else ob, ?_, ?_⟩
  · rw [hs]; exact updObj_mem_self hob
  · by_cases e : ob.id = o'
    · simp only [e, ite_true]
      exact ⟨by rw [hg]; exact hid, hk (e.symm.trans hid) ob hid hf⟩
    · simp only [e, ite_false]
      exact ⟨hid, hf⟩

theorem updField_fields_ids (f : Nat) (g : Field → Field) (hg : ∀ fl, (g fl).id = fl.id)
    (ob : Obj) : (updField f g ob).fields.map Field.id = ob.fields.map Field.id :=
  updField_map_id f g hg ob.fields

/-- A cell that is not lost still exists. -/
theorem hasField_apply (c : Change) (s : Schema) (o f : Nat) (h : hasField s o f)
    (hl : ¬ lost c o f) : hasField (apply c s) o f := by
  cases c with
  | addField o' fld v =>
    exact hasField_upd s _ o' o f _ (by intro _; rfl) rfl h (by intro _ ob _ hf; simp [hf])
  | widen o' f' ty =>
    exact hasField_upd s _ o' o f _ (by intro _; rfl) rfl h (by
      intro _ ob _ hf; rw [updField_fields_ids _ _ (by intro _; rfl)]; exact hf)
  | renameField o' f' n =>
    exact hasField_upd s _ o' o f _ (by intro _; rfl) rfl h (by
      intro _ ob _ hf; rw [updField_fields_ids _ _ (by intro _; rfl)]; exact hf)
  | narrow o' f' ty =>
    exact hasField_upd s _ o' o f _ (by intro _; rfl) rfl h (by
      intro _ ob _ hf; rw [updField_fields_ids _ _ (by intro _; rfl)]; exact hf)
  | retargetRef o' f' t =>
    exact hasField_upd s _ o' o f _ (by intro _; rfl) rfl h (by
      intro _ ob _ hf; rw [updField_fields_ids _ _ (by intro _; rfl)]; exact hf)
  | retype o' f' ty enc dec =>
    exact hasField_upd s _ o' o f _ (by intro _; rfl) rfl h (by
      intro _ ob _ hf; rw [updField_fields_ids _ _ (by intro _; rfl)]; exact hf)
  | removeField o' f' =>
    refine hasField_upd s _ o' o f _ (by intro _; rfl) rfl h ?_
    intro e ob _ hf
    subst e
    have hne : f' ≠ f := fun e' => hl ⟨rfl, e'⟩
    simp only [List.mem_map, List.mem_filter] at hf ⊢
    obtain ⟨fl, hfl, rfl⟩ := hf
    exact ⟨fl, ⟨hfl, by simp only [bne_iff_ne, ne_eq]; exact fun e => hne e.symm⟩, rfl⟩
  | removeObject o' =>
    obtain ⟨ob, hob, hid, hf⟩ := h
    have hne : o' ≠ o := fun e => hl e
    refine ⟨ob, ?_, hid, hf⟩
    simp only [apply, applyObjs, List.mem_filter]
    exact ⟨hob, by simpa [hid] using hne.symm⟩
  | removeDep q => exact h
  | moveField o' f' r o'' fld =>
    have h1 : hasField { s with objs := updObj o' (fun ob => { ob with
        fields := ob.fields.filter (fun fl => fl.id != f') }) s.objs } o f := by
      refine hasField_upd s _ o' o f _ (by intro _; rfl) rfl h ?_
      intro e ob _ hf
      subst e
      have hne : f' ≠ f := fun e' => hl ⟨rfl, e'⟩
      simp only [List.mem_map, List.mem_filter] at hf ⊢
      obtain ⟨fl, hfl, rfl⟩ := hf
      exact ⟨fl, ⟨hfl, by simp only [bne_iff_ne, ne_eq]; exact fun e => hne e.symm⟩, rfl⟩
    exact hasField_upd _ _ o'' o f _ (by intro _; rfl) rfl h1 (by intro _ ob _ hf; simp [hf])
  | splitField o' f' f1 f2 sp jn =>
    refine hasField_upd s _ o' o f _ (by intro _; rfl) rfl h ?_
    intro e ob _ hf
    subst e
    have hne : f' ≠ f := fun e' => hl ⟨rfl, e'⟩
    simp only [List.map_append, List.mem_append, List.mem_map, List.mem_filter] at hf ⊢
    obtain ⟨fl, hfl, rfl⟩ := hf
    exact Or.inl ⟨fl, ⟨hfl, by simp only [bne_iff_ne, ne_eq]; exact fun e => hne e.symm⟩, rfl⟩

/-- Every existing cell of a deleted object, and the cell removed by a field removal or moved
    or split away, is in the footprint. -/
theorem lost_mem_footprint (c : Change) (s : Schema) (o f : Nat) (h : hasField s o f)
    (hl : lost c o f) : (o, f) ∈ footprint c s := by
  cases c with
  | removeField o' f' => obtain ⟨rfl, rfl⟩ := hl; simp [footprint]
  | moveField o' f' r o'' fld => obtain ⟨rfl, rfl⟩ := hl; simp [footprint]
  | splitField o' f' f1 f2 sp jn => obtain ⟨rfl, rfl⟩ := hl; simp [footprint]
  | removeObject o' =>
    subst hl
    obtain ⟨ob, hob, hid, hf⟩ := h
    simp only [footprint, List.mem_flatMap]
    refine ⟨ob, hob, ?_⟩
    simp only [hid, ite_true, List.mem_map]
    obtain ⟨fl, hfl, rfl⟩ := List.mem_map.1 hf
    exact ⟨fl, hfl, rfl⟩
  | _ => exact absurd hl (by simp [lost])

/-! ## Migration leaves untouched cells alone -/

/-- A cell that exists and is not in the footprint has the same value after migration. -/
theorem migrate_stable (c : Change) (s : Schema) (data : Data) (hpre : pre c s) (o f : Nat)
    (h : hasField s o f) (hn : (o, f) ∉ footprint c s) :
    migrate c data o f = data o f := by
  cases c with
  | addField o' fld v =>
    obtain ⟨_, hfresh, _⟩ := hpre
    obtain ⟨ob, hob, hid, hf⟩ := h
    have : ¬ (o = o' ∧ f = fld.id) := by
      rintro ⟨rfl, rfl⟩
      exact hfresh ob hob hid hf
    simp [migrate, this]
  | widen _ _ _ => rfl
  | renameField _ _ _ => rfl
  | narrow o' f' ty =>
    have : ¬ (o = o' ∧ f = f') := by
      rintro ⟨rfl, rfl⟩; exact hn (by simp [footprint])
    simp [migrate, this]
  | removeField o' f' =>
    have : ¬ (o = o' ∧ f = f') := by
      rintro ⟨rfl, rfl⟩; exact hn (by simp [footprint])
    simp [migrate, this]
  | retargetRef o' f' t =>
    have : ¬ (o = o' ∧ f = f') := by
      rintro ⟨rfl, rfl⟩; exact hn (by simp [footprint])
    simp [migrate, this]
  | retype o' f' ty enc dec =>
    have : ¬ (o = o' ∧ f = f') := by
      rintro ⟨rfl, rfl⟩; exact hn (by simp [footprint])
    simp [migrate, this]
  | removeObject o' =>
    have : o ≠ o' := fun e => hn (lost_mem_footprint _ s o f h e.symm)
    simp [migrate, this]
  | removeDep q => rfl
  | moveField o' f' r o'' fld =>
    obtain ⟨_, _, _, _, _, hfresh, _⟩ := hpre
    have h1 : ¬ (o = o'' ∧ f = fld.id) := by
      rintro ⟨rfl, rfl⟩
      obtain ⟨ob, hob, hid, hf⟩ := h
      exact hfresh ob hob hid hf
    have h2 : ¬ (o = o' ∧ f = f') := by
      rintro ⟨rfl, rfl⟩; exact hn (by simp [footprint])
    simp [migrate, h1, h2]
  | splitField o' f' f1 f2 sp jn =>
    obtain ⟨_, _, _, _, hfresh, _⟩ := hpre
    obtain ⟨ob, hob, hid, hf⟩ := h
    have h1 : ¬ (o = o' ∧ f = f1.id) := by
      rintro ⟨rfl, rfl⟩
      exact (hfresh ob hob hid).1 hf
    have h2 : ¬ (o = o' ∧ f = f2.id) := by
      rintro ⟨rfl, rfl⟩
      exact (hfresh ob hob hid).2 hf
    have h3 : ¬ (o = o' ∧ f = f') := by
      rintro ⟨rfl, rfl⟩; exact hn (by simp [footprint])
    simp [migrate, h1, h2, h3]

/-- A reader that no touched cell reaches and whose object survives is valid in the new schema
    and takes the same value on migrated data. -/
theorem reader_stable (c : Change) (s : Schema) (data : Data) (hpre : pre c s) (r : Reader)
    (hv : ValidDep s (.reader r)) (hh : readerHit c s r = false) :
    ValidDep (apply c s) (.reader (rewriteReader c r)) ∧
      evalReader (rewriteReader c r) (migrate c data) = evalReader r data := by
  simp only [readerHit, Bool.or_eq_false_iff, List.any_eq_false, decide_eq_true_eq] at hh
  obtain ⟨hreads, hbase⟩ := hh
  obtain ⟨hb, hr⟩ := hv
  have hid : rewriteReader c r = r :=
    rewriteReader_of_reads c r fun e he =>
      subst_of_avoid c s e (fun p hp => hreads p (mem_cells he hp))
  rw [hid]
  refine ⟨⟨hasObj_apply' c s _ hb hbase, ?_⟩, ?_⟩
  · intro p hp
    exact hasField_apply c s _ _ (hr p hp) (fun hl => hreads p hp (lost_mem_footprint c s _ _ (hr p hp) hl))
  · unfold evalReader
    congr 1
    apply List.map_congr_left
    intro e he
    apply eval_congr
    intro p hp
    exact migrate_stable c s data hpre _ _ (hr p (mem_cells he hp)) (hreads p (mem_cells he hp))

end Strata.Evolution
