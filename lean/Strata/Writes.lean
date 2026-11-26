/-
Slice 2 of the reference model: record writes in full.

What the slice claims, about the model in `Strata/Model.lean`:

- An update is a patch. The record an accepted update writes is the old one patched and one
  version on (`update_writes_patched`); a field the patch does not name keeps its values
  (`update_keeps_unnamed`, from `patch_keeps_unnamed`); the value named last for a field is its
  value afterwards, and a named `null` removes it (`patch_last_wins`).
- Versions. An accepted update raises the record's version by one (`update_raises_version`); an
  accepted delete raises the version of every record whose reference it clears
  (`delete_raises_referrer_version`); a created record is at version 1 (`create_fills_default`).
- `expected_version`. One who may act on the record and names another version is refused with
  `versionConflict`, whatever the patch, so no state follows (`version_conflict_on_update`,
  `version_conflict_on_delete`); the version is asked after the permission, so one who may not act
  is refused `unauthorized` whatever version it names (`unauthorized_cannot_update` and
  `unauthorized_cannot_delete` in `Strata/Proofs.lean`, which take any expected version).
- Defaults. An accepted create gives every field of the schema it does not name the field's
  constant default (`create_fills_default`).
- Lists. A required list never forbids a delete: only a restricting definition does
  (`required_list_never_blocks`). Typing elementwise, dangling elements refused, elements removed
  on delete and referential integrity over lists are the slice 1 theorems, whose definitions now
  read list elements (`problem_none`, `step_preserves_refs`, `delete_leaves_no_reference`).
- Size limits. A record over the record bound is refused with `limitExceeded` before anything
  else about it is checked (`create_refused_over_record_bound`); every record an accepted create or
  update writes is within the record bound and holds no value over the string or list bound
  (`create_within_limits`, `update_within_limits`).

No `sorry`, no added axioms. `Audit.lean` prints the axioms each theorem depends on.
-/

import Strata.Model
import Strata.References

namespace Strata

/-! ## Lemmas: patches -/

private theorem valuesOf_filter_ne (fs : Fields) (f g : FieldId) (h : g ≠ f) :
    Fields.valuesOf (fs.filter (fun e => e.1 != g)) f = fs.valuesOf f := by
  unfold Fields.valuesOf
  rw [List.filter_filter]
  congr 1
  apply List.filter_congr
  intro e _
  by_cases he : e.1 = f
  · subst he
    have : e.1 ≠ g := fun h' => h h'.symm
    simp [this]
  · simp [he]

theorem valuesOf_set_ne (fs : Fields) (fv : FieldId × Val) (f : FieldId) (h : fv.1 ≠ f) :
    (fs.set fv).valuesOf f = fs.valuesOf f := by
  unfold Fields.set
  split
  · exact valuesOf_filter_ne fs f fv.1 h
  · rw [← valuesOf_filter_ne fs f fv.1 h]
    simp [Fields.valuesOf, h]

/-- A patch leaves every field it does not name exactly as it was. -/
theorem patch_keeps_unnamed (f : FieldId) :
    ∀ (ps fs : Fields), f ∉ ps.map Prod.fst → (fs.patch ps).valuesOf f = fs.valuesOf f
  | [], fs, _ => rfl
  | p :: ps, fs, h => by
    simp only [List.map_cons, List.mem_cons, not_or] at h
    unfold Fields.patch
    rw [List.foldl_cons]
    have ih := patch_keeps_unnamed f ps (fs.set p) h.2
    unfold Fields.patch at ih
    rw [ih]
    exact valuesOf_set_ne fs p f (fun he => h.1 he.symm)

theorem get_set_same (fs : Fields) (f : FieldId) (v : Val) : (fs.set (f, v)).get f = v := by
  have hnone : (fs.filter (fun g => g.1 != f)).lookup f = none := by
    induction fs with
    | nil => rfl
    | cons q fs ih =>
      obtain ⟨k, w⟩ := q
      rw [List.filter_cons]
      by_cases hk : k = f
      · subst hk; simpa using ih
      · simp only [bne_iff_ne, ne_eq, hk, not_false_eq_true, ite_true]
        rw [lookup_cons_nat]
        simp [Ne.symm hk, ih]
  unfold Fields.set
  split
  · rename_i hv
    have : v = .null := by simpa using hv
    subst this
    simp [Fields.get, hnone]
  · simp [Fields.get]

/-- The value a patch names last for a field is the field's value afterwards; a `null` there
    leaves the field absent. -/
theorem patch_last_wins (fs ps : Fields) (f : FieldId) (v : Val) :
    (fs.patch (ps ++ [(f, v)])).get f = v := by
  unfold Fields.patch
  rw [List.foldl_append, List.foldl_cons, List.foldl_nil]
  exact get_set_same _ f v

/-! ## Lemmas: the records after a write -/

theorem lookup_map_replace (rs : Records) (id : RecordId) (r : Record) :
    (rs.map (fun q => if q.1 == id then (q.1, r) else q)).lookup id = (rs.lookup id).map (fun _ => r) := by
  induction rs with
  | nil => rfl
  | cons q rs ih =>
    obtain ⟨k, w⟩ := q
    by_cases hk : k = id
    · subst hk; simp [List.lookup]
    · have hk' : (k == id) = false := by simpa using hk
      simp only [List.map_cons, hk', Bool.false_eq_true, ite_false]
      rw [lookup_cons_nat, lookup_cons_nat]
      simp only [Ne.symm hk, ite_false]
      exact ih

/-! ## The records after a write -/

/-- The record an accepted update writes is the stored one patched, one version on, when the type has no
    workflow assignment (`Tenant.Plain`; slice 8 writes the assignments too, see `Strata/Workflows.lean`). -/
theorem update_writes_patched (t t' : Tenant) (p : Principal) (ty : TypeId) (id : RecordId)
    (ps : Fields) (ev : Option Nat) (old : Record) (hpl : t.Plain) (hl : t.recordAt ty id = some old)
    (h : stepTenant t p (.update ty id ps ev) = .ok t') :
    t'.recordAt ty id = some (patched old ps) := by
  unfold Tenant.recordAt at hl ⊢
  cases hT : t.typ? ty with
  | none => rw [hT] at hl; cases hl
  | some o =>
    rw [hT] at hl
    have hL : o.records.lookup id = some old := by simpa using hl
    obtain ⟨-, -, ht⟩ := update_ok_parts hpl hT hL h
    subst ht
    rw [typ?_putType_self t ty o _ hT]
    show List.lookup id (o.records.map (fun q => if q.1 == id then (q.1, patched old ps) else q)) = some (patched old ps)
    rw [lookup_map_replace, hL]
    rfl

/-- A field an accepted update does not name keeps its values (with no workflow assignment). -/
theorem update_keeps_unnamed (t t' : Tenant) (p : Principal) (ty : TypeId) (id : RecordId)
    (ps : Fields) (ev : Option Nat) (old : Record) (f : FieldId) (hpl : t.Plain)
    (hl : t.recordAt ty id = some old)
    (hf : f ∉ ps.map Prod.fst) (h : stepTenant t p (.update ty id ps ev) = .ok t') :
    ∃ r, t'.recordAt ty id = some r ∧ r.fields.valuesOf f = old.fields.valuesOf f :=
  ⟨_, update_writes_patched t t' p ty id ps ev old hpl hl h, patch_keeps_unnamed f ps old.fields hf⟩

/-- An accepted update raises the record's version by exactly one: versions strictly increase. The
    workflows change fields, never the version, so this holds for every tenant. -/
theorem update_raises_version (t t' : Tenant) (p : Principal) (ty : TypeId) (id : RecordId)
    (ps : Fields) (ev : Option Nat) (old : Record) (hl : t.recordAt ty id = some old)
    (h : stepTenant t p (.update ty id ps ev) = .ok t') :
    ∃ r, t'.recordAt ty id = some r ∧ r.version = old.version + 1 ∧ old.version < r.version := by
  unfold Tenant.recordAt at hl ⊢
  cases hT : t.typ? ty with
  | none => rw [hT] at hl; cases hl
  | some o =>
    rw [hT] at hl
    have hL : o.records.lookup id = some old := by simpa using hl
    obtain ⟨r, hF, -, -, -, -, ht⟩ := update_ok_parts_gen hT hL h
    subst ht
    have hv : r.version = old.version + 1 := by
      have := fireAll_version hF
      simpa [patched] using this
    refine ⟨r, ?_, hv, by omega⟩
    rw [typ?_putType_self t ty o _ hT]
    show List.lookup id (o.records.map (fun q => if q.1 == id then (q.1, r) else q)) = some r
    rw [lookup_map_replace, hL]
    rfl

/-- An accepted delete rewrites every other record that refers to the deleted one, of any type: the
    references cleared and the version one on. -/
theorem delete_raises_referrer_version (t t' : Tenant) (p : Principal) (ty : TypeId)
    (id : RecordId) (ev : Option Nat) (S : TypeId) (o : ObjType) (x : RecordId) (q : Record)
    (hS : t.typ? S = some o) (hq : (x, q) ∈ o.records) (hne : S = ty → x ≠ id)
    (hr : recRefers o.schema ty id q = true) (h : stepTenant t p (.delete ty id ev) = .ok t') :
    ∃ o', t'.typ? S = some o' ∧
      (x, { q with fields := clearFields o.schema ty id q.fields, version := q.version + 1 }) ∈
        o'.records := by
  have ht := stepTenant_delete_eq h
  subst ht
  refine ⟨delObj ty id S o, by rw [typ?_afterDelete, hS]; rfl, ?_⟩
  have hmem : (x, q) ∈ (if S == ty then o.records.filter (fun q => q.1 != id) else o.records) := by
    by_cases hST : (S == ty) = true
    · rw [if_pos hST]
      have hSt : S = ty := by simpa using hST
      exact List.mem_filter.mpr ⟨hq, by simpa [bne_iff_ne] using hne hSt⟩
    · rw [if_neg hST]; exact hq
  unfold delObj
  refine List.mem_map.mpr ⟨(x, q), hmem, ?_⟩
  have hq' : clearRef o.schema ty id q = { q with fields := clearFields o.schema ty id q.fields, version := q.version + 1 } := by
    unfold clearRef
    rw [if_pos hr]
  simp [delClear, hq']

/-! ## Expected versions -/

/-- One who may change a record and names a version it is not at is refused with
    `versionConflict`, whatever the patch: no state follows, so nothing changes. -/
theorem version_conflict_on_update (w : World) (c : Ctx) (t : Tenant) (ty : TypeId) (id : RecordId)
    (old : Record) (ps : Fields) (v : Nat)
    (ht : w c.tenant = some t) (hl : t.recordAt ty id = some old)
    (hcan : canUpdate t c.principal ty old (patched old ps) = true) (hv : v ≠ old.version)
    (hr : callerReserved c.principal = false) :
    step w (.update ty id ps (some v)) c = .error .versionConflict ∧
      ∀ w', step w (.update ty id ps (some v)) c ≠ .ok w' := by
  have hc : versionClash (some v) old = true := by simpa [versionClash] using hv
  have hs : stepTenant t c.principal (.update ty id ps (some v)) = .error .versionConflict := by
    unfold Tenant.recordAt at hl
    cases hT : t.typ? ty with
    | none => rw [hT] at hl; cases hl
    | some o =>
      rw [hT] at hl
      have hL : o.records.lookup id = some old := by simpa using hl
      simp [stepTenant, hT, hL, hcan, hc]
  have : step w (.update ty id ps (some v)) c = .error .versionConflict := by
    simp [step, reservedRefused, Intent.named, hr, ht, hs]
  exact ⟨this, fun w' h => by rw [this] at h; cases h⟩

theorem version_conflict_on_delete (w : World) (c : Ctx) (t : Tenant) (ty : TypeId) (id : RecordId)
    (old : Record) (v : Nat)
    (ht : w c.tenant = some t) (hl : t.recordAt ty id = some old)
    (hcan : canDelete t c.principal ty old = true) (hv : v ≠ old.version)
    (hr : callerReserved c.principal = false) :
    step w (.delete ty id (some v)) c = .error .versionConflict ∧
      ∀ w', step w (.delete ty id (some v)) c ≠ .ok w' := by
  have hc : versionClash (some v) old = true := by simpa [versionClash] using hv
  have hs : stepTenant t c.principal (.delete ty id (some v)) = .error .versionConflict := by
    unfold Tenant.recordAt at hl
    cases hT : t.typ? ty with
    | none => rw [hT] at hl; cases hl
    | some o =>
      rw [hT] at hl
      have hL : o.records.lookup id = some old := by simpa using hl
      simp [stepTenant, hT, hL, hcan, hc]
  have : step w (.delete ty id (some v)) c = .error .versionConflict := by
    simp [step, reservedRefused, Intent.named, hr, ht, hs]
  exact ⟨this, fun w' h => by rw [this] at h; cases h⟩

/-! ## Defaults -/

/-- An accepted create writes, under the next id and at version 1, the default of every field of
    the schema of its type the intent does not name, when the default is the constant `v`. -/
theorem create_fills_default (t t' : Tenant) (p : Principal) (ty : TypeId) (fs : Fields)
    (o : ObjType) (f : FieldId) (d : FieldDef) (v : Val)
    (ho : t.typ? ty = some o) (hd : (f, d) ∈ o.schema) (hdef : d.default = .lit v) (hv : v ≠ .null)
    (hn : fs.any (fun fv => fv.1 == f) = false) (h : stepTenant t p (.create ty fs) = .ok t') :
    ∃ o', t'.typ? ty = some o' ∧ ∃ r, o'.records.lookup t.nextId = some r ∧ r.version = 1 ∧
      (f, v) ∈ r.fields := by
  obtain ⟨r, hN, -, -, ht⟩ := create_ok_parts ho h
  obtain ⟨ds, hD, hr⟩ := newRecord_parts hN
  subst ht
  refine ⟨_, typ?_putType_self t ty o _ ho, r, by simp, ?_, ?_⟩
  · rw [hr]
  · rw [hr]
    apply List.mem_append_right
    exact defaultsOf_mem _ fs o.schema ds f d v hD hd hn (by rw [hdef]; rfl) hv

/-! ## Lists -/

/-- A required list never forbids a delete: a reference held in a list forbids it only through a
    definition of the field that restricts deletion. -/
theorem required_list_never_blocks (schema : Schema) (f : FieldId) (vs : List Elem) :
    forbidsDelete schema f (.list vs) = schema.any (fun fd => fd.1 == f && fd.2.ty.restricts) := by
  simp [forbidsDelete, Val.isList]

/-! ## Size limits -/

theorem problem_none_within {L : Limits} {schema : Schema} {us : List (List FieldId)}
    {uniq : Bool} {env : Env} {rs : Records} {self : RecordId} {r : Record}
    (h : problem L schema us uniq env rs self r = none) :
    r.size ≤ L.maxRecord ∧ ∀ fv ∈ r.fields, fv.2.overLimit L = false := by
  obtain ⟨hs, hval, -⟩ := problem_none_parts h
  refine ⟨hs, fun fv hfv => ?_⟩
  obtain ⟨d, -, hp⟩ := hval fv hfv
  exact (valueProblem_none hp).1

/-- A create whose record is over the record bound is refused with `limitExceeded`, before its
    fields are checked. -/
theorem create_refused_over_record_bound (t : Tenant) (p : Principal) (ty : TypeId) (fs : Fields)
    (o : ObjType) (ho : t.typ? ty = some o) (r : Record)
    (hN : newRecord o.schema p fs = .ok r) (hc : canCreate t p ty r = true)
    (hw : initialsOk o.workflows r = true) (hs : t.limits.maxRecord < r.size) :
    stepTenant t p (.create ty fs) = .error .limitExceeded := by
  simp [stepTenant, ho, hN, hc, hw, canCreate_attempt _ _ _ _ hc, problem, hs]

/-- Every record an accepted create writes is within the record bound and holds no value over the
    string or list bound. -/
theorem create_within_limits (t t' : Tenant) (p : Principal) (ty : TypeId) (fs : Fields)
    (o : ObjType) (ho : t.typ? ty = some o) (h : stepTenant t p (.create ty fs) = .ok t') :
    ∃ r, newRecord o.schema p fs = .ok r ∧ r.size ≤ t.limits.maxRecord ∧
      ∀ fv ∈ r.fields, fv.2.overLimit t.limits = false := by
  obtain ⟨r, hN, hP, -, -⟩ := create_ok_parts ho h
  exact ⟨r, hN, problem_none_within hP⟩

theorem update_within_limits (t t' : Tenant) (p : Principal) (ty : TypeId) (id : RecordId)
    (ps : Fields) (ev : Option Nat) (o : ObjType) (old : Record) (hpl : t.Plain)
    (ho : t.typ? ty = some o)
    (hl : o.records.lookup id = some old)
    (h : stepTenant t p (.update ty id ps ev) = .ok t') :
    (patched old ps).size ≤ t.limits.maxRecord ∧
      ∀ fv ∈ (patched old ps).fields, fv.2.overLimit t.limits = false := by
  obtain ⟨hP, -, -⟩ := update_ok_parts hpl ho hl h
  exact problem_none_within hP

end Strata
