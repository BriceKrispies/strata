/-
Supporting lemmas for the schema evolution proofs: how `apply` and `migrate` act on lookups.
-/

import Strata.Evolution.Model

namespace Strata.Evolution

/-- Whether the change makes cell `(o, f)` stop existing. -/
def lost : Change → Nat → Nat → Prop
  | .removeField o' f', o, f => o' = o ∧ f' = f
  | .removeObject o', o, _ => o' = o
  | _, _, _ => False

/-- The changes that never remove or reinterpret an existing cell or dependent. -/
def isMono : Change → Bool
  | .addField .. => true
  | .widen .. => true
  | .renameField .. => true
  | _ => false

theorem mono_of_not_breaking (c : Change) (h : classify c ≠ .breaking) : isMono c = true := by
  cases c <;> simp_all [classify, isMono]

theorem removedDeps_of_mono (c : Change) (h : isMono c = true) : removedDeps c = [] := by
  cases c <;> simp_all [isMono, removedDeps]

theorem objGone_of_mono (c : Change) (h : isMono c = true) (b : Nat) : objGone c b = false := by
  cases c <;> simp_all [isMono, objGone]

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

theorem deps_apply_of_ne (c : Change) (s : Schema) (h : removedDeps c = []) :
    (apply c s).deps = s.deps := by
  cases c <;> simp_all [apply, removedDeps]

theorem findR_filter (ds : List Dep) (q x : Nat) (h : q ≠ x) :
    findR (ds.filter fun d => d.id != x) q = findR ds q := by
  induction ds with
  | nil => rfl
  | cons d ds ih =>
    cases d with
    | reader r =>
      by_cases hr : r.id = q
      · have : r.id ≠ x := by omega
        have hq : ¬ q = x := h
        simp [List.filter_cons, this, findR, hr, hq]
      · by_cases hx : r.id = x
        · simp [List.filter_cons, hx, findR, hr, ih]
          omega
        · simp [List.filter_cons, hx, findR, hr, ih]
    | view v =>
      by_cases hx : v.id = x
      · simp [List.filter_cons, hx, findR, ih]
      · simp [List.filter_cons, hx, findR, ih]

theorem findR_apply (c : Change) (s : Schema) (q : Nat) (h : q ∉ removedDeps c) :
    findR (apply c s).deps q = findR s.deps q := by
  cases c with
  | removeDep x =>
    simp only [removedDeps, List.mem_singleton] at h
    exact findR_filter _ _ _ h
  | _ => rfl

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
    (hr : d.id ∉ removedDeps c) : d ∈ (apply c s).deps := by
  cases c with
  | removeDep x =>
    simp only [removedDeps, List.mem_singleton] at hr
    simp [apply, hd, hr]
  | _ => exact hd

theorem mem_deps_apply_imp (c : Change) (s : Schema) (d : Dep) (hd : d ∈ (apply c s).deps) :
    d ∈ s.deps := by
  cases c with
  | removeDep x => exact (List.mem_filter.1 hd).1
  | _ => exact hd

/-- Every change except deleting an object keeps the set of object ids. -/
theorem hasObj_apply (c : Change) (s : Schema) (t : Nat) (hc : ∀ o, c ≠ .removeObject o) :
    hasObj (apply c s) t ↔ hasObj s t := by
  unfold hasObj
  cases c with
  | addField o fld v => simp only [apply]; exact (congrArg (fun l => t ∈ l) (updObj_map_id _ _ (by intro _; rfl) _)).to_iff
  | widen o f ty => simp only [apply]; exact (congrArg (fun l => t ∈ l) (updObj_map_id _ _ (by intro _; rfl) _)).to_iff
  | renameField o f n => simp only [apply]; exact (congrArg (fun l => t ∈ l) (updObj_map_id _ _ (by intro _; rfl) _)).to_iff
  | removeField o f => simp only [apply]; exact (congrArg (fun l => t ∈ l) (updObj_map_id _ _ (by intro _; rfl) _)).to_iff
  | narrow o f ty => simp only [apply]; exact (congrArg (fun l => t ∈ l) (updObj_map_id _ _ (by intro _; rfl) _)).to_iff
  | retargetRef o f t' => simp only [apply]; exact (congrArg (fun l => t ∈ l) (updObj_map_id _ _ (by intro _; rfl) _)).to_iff
  | removeObject o => exact absurd rfl (hc o)
  | removeDep q => rfl

/-- An object that is not deleted still exists. -/
theorem hasObj_apply' (c : Change) (s : Schema) (t : Nat) (h : hasObj s t)
    (hg : objGone c t = false) : hasObj (apply c s) t := by
  cases c with
  | removeObject o =>
    simp only [objGone, decide_eq_false_iff_not] at hg
    unfold hasObj at *
    simp only [apply, List.mem_map, List.mem_filter] at *
    obtain ⟨ob, hob, rfl⟩ := h
    exact ⟨ob, ⟨hob, by simpa using hg⟩, rfl⟩
  | _ => exact (hasObj_apply _ s t (by simp)).2 h

theorem hasField_upd (s : Schema) (o' o f : Nat) (g : Obj → Obj) (hg : ∀ ob, (g ob).id = ob.id)
    (h : hasField s o f)
    (hk : o' = o → ∀ ob, ob.id = o → f ∈ ob.fields.map Field.id →
      f ∈ (g ob).fields.map Field.id) :
    hasField { s with objs := updObj o' g s.objs } o f := by
  obtain ⟨ob, hob, hid, hf⟩ := h
  refine ⟨if ob.id = o' then g ob else ob, updObj_mem_self hob, ?_⟩
  by_cases e : ob.id = o'
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
    exact hasField_upd s o' o f _ (by intro _; rfl) h (by intro _ ob _ hf; simp [hf])
  | widen o' f' ty =>
    exact hasField_upd s o' o f _ (by intro _; rfl) h (by
      intro _ ob _ hf; rw [updField_fields_ids _ _ (by intro _; rfl)]; exact hf)
  | renameField o' f' n =>
    exact hasField_upd s o' o f _ (by intro _; rfl) h (by
      intro _ ob _ hf; rw [updField_fields_ids _ _ (by intro _; rfl)]; exact hf)
  | narrow o' f' ty =>
    exact hasField_upd s o' o f _ (by intro _; rfl) h (by
      intro _ ob _ hf; rw [updField_fields_ids _ _ (by intro _; rfl)]; exact hf)
  | retargetRef o' f' t =>
    exact hasField_upd s o' o f _ (by intro _; rfl) h (by
      intro _ ob _ hf; rw [updField_fields_ids _ _ (by intro _; rfl)]; exact hf)
  | removeField o' f' =>
    refine hasField_upd s o' o f _ (by intro _; rfl) h ?_
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
    simp only [apply, List.mem_filter]
    exact ⟨hob, by simpa [hid] using hne.symm⟩
  | removeDep q => exact h

/-- Every existing cell of a deleted object, and the cell removed by a field removal, is in the
    footprint. -/
theorem lost_mem_footprint (c : Change) (s : Schema) (o f : Nat) (h : hasField s o f)
    (hl : lost c o f) : (o, f) ∈ footprint c s := by
  cases c with
  | removeField o' f' => obtain ⟨rfl, rfl⟩ := hl; simp [footprint]
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
  | removeObject o' =>
    have : o ≠ o' := fun e => hn (lost_mem_footprint _ s o f h e.symm)
    simp [migrate, this]
  | removeDep q => rfl

/-- A reader that no touched cell reaches and whose object survives is valid in the new schema
    and takes the same value on migrated data. -/
theorem reader_stable (c : Change) (s : Schema) (data : Data) (hpre : pre c s) (r : Reader)
    (hv : ValidDep s (.reader r)) (hh : readerHit c s r = false) :
    ValidDep (apply c s) (.reader r) ∧ evalReader r (migrate c data) = evalReader r data := by
  simp only [readerHit, Bool.or_eq_false_iff, List.any_eq_false, decide_eq_true_eq] at hh
  obtain ⟨hreads, hbase⟩ := hh
  obtain ⟨hb, hr⟩ := hv
  refine ⟨⟨hasObj_apply' c s _ hb hbase, ?_⟩, ?_⟩
  · intro p hp
    exact hasField_apply c s _ _ (hr p hp) (fun hl => hreads p hp (lost_mem_footprint c s _ _ (hr p hp) hl))
  · unfold evalReader
    congr 1
    apply List.map_congr_left
    intro p hp
    exact migrate_stable c s data hpre _ _ (hr p hp) (hreads p hp)

end Strata.Evolution
