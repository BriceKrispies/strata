/-
Slice 3 of the reference model: uniqueness.

What the slice claims, about the model in `Strata/Model.lean`:

- A unique field, and a composite unique constraint, is kept unique by every accepted strict step
  (`stepTenant_preserves_unique`, `step_preserves_unique`): no two distinct records agree on a
  unique field, or on every field of a constraint, where agreeing needs a keyed (present, not a
  list) value in each.
- A write that would duplicate is refused with `uniqueViolated` and no state follows, when the
  write is otherwise valid (`create_duplicate_refused`, `update_duplicate_refused`).
- A composite constraint that existing records already violate is refused with `uniqueViolated`
  (`define_unique_refused_over_duplicates`); a strict change that makes a field unique over records
  that already agree on it is refused with `migrationRejected`
  (`set_unique_refused_over_duplicates`).
- The exclusion of the non-strict field change is necessary: there is a well-formed tenant that a
  lax `setUnique` breaks (`lax_unique_change_can_break_wf`).
- The name invariant `Tenant.Distinct` (distinct schema names, and distinct field names within a
  record) is kept by every accepted step (`stepTenant_preserves_distinct`). It is what the delete
  case of the uniqueness invariant needs: a delete clears references, and that must not make two
  values equal.

The checks follow the kernel's order (`Strata/Model.lean`, `problem`): per field in schema order, a
unique clash after the type check and before the reference check; composite constraints after all
fields.

No `sorry`, no added axioms. `Audit.lean` prints the axioms each theorem depends on.
-/

import Strata.Model
import Strata.References

namespace Strata

/-! ## Lemmas: the unique checks of one write -/

/-- The checks before the unique clash pass for a value that passes every check, and the
    unique-size test is not met. -/
theorem valueProblem_conds {L : Limits} {uniq : Bool} {rs : Records} {self : RecordId}
    {f : FieldId} {d : FieldDef} {v : Val} (h : valueProblem L uniq rs self f d v = none) :
    v.overLimit L = false ∧ (d.unique && decide (L.maxUnique < v.size)) = false ∧
      v.hasType d.ty = true := by
  have hov : v.overLimit L = false := (valueProblem_none h).1
  refine ⟨hov, ?_, (valueProblem_none h).2.1⟩
  cases hb : (d.unique && decide (L.maxUnique < v.size))
  · rfl
  · exfalso
    unfold valueProblem at h
    simp [hov, hb] at h

/-- Without the unique checks, a value that passes is passed with them, or refused with
    `uniqueViolated`. -/
theorem valueProblem_uniq_dichotomy {L : Limits} {rs : Records} {self : RecordId} {f : FieldId}
    {d : FieldDef} {v : Val} (h : valueProblem L false rs self f d v = none) :
    valueProblem L true rs self f d v = none ∨
      valueProblem L true rs self f d v = some .uniqueViolated := by
  obtain ⟨hov, hb, hty⟩ := valueProblem_conds h
  have hres : v.resolves rs = true := (valueProblem_none h).2.2.1
  by_cases hc : (d.unique && clashAt rs self f v) = true
  · right
    have hu : d.unique = true := (Bool.and_eq_true _ _).mp hc |>.1
    have hsz : ¬ (L.maxUnique < v.size) := by
      have hb' := hb
      rw [hu] at hb'
      simpa using hb'
    have hc2 : clashAt rs self f v = true := ((Bool.and_eq_true _ _).mp hc).2
    unfold valueProblem
    simp [hov, hty, hc2, hu, hsz]
  · left
    unfold valueProblem
    simp [hov, hb, hty, hc, hres]

/-- A value that passes every check before the unique clash, and clashes, is refused with
    `uniqueViolated`. -/
theorem valueProblem_clash {L : Limits} {rs : Records} {self : RecordId} {f : FieldId}
    {d : FieldDef} {v : Val} (h : valueProblem L false rs self f d v = none) (hu : d.unique = true)
    (hc : clashAt rs self f v = true) :
    valueProblem L true rs self f d v = some .uniqueViolated := by
  obtain ⟨hov, hb, hty⟩ := valueProblem_conds h
  have hsz : ¬ (L.maxUnique < v.size) := by
    have hb' := hb
    rw [hu] at hb'
    simpa using hb'
  unfold valueProblem
  simp [hov, hty, hc, hu, hsz]

/-- Over a list, `findSome?` is `none` or `some e` when every element's result is `none` or
    `some e`. -/
theorem findSome_dichotomy {α β : Type} (f : α → Option β) (e : β) (l : List α)
    (h : ∀ x ∈ l, f x = none ∨ f x = some e) :
    l.findSome? f = none ∨ l.findSome? f = some e := by
  induction l with
  | nil => left; rfl
  | cons a l ih =>
    simp only [List.findSome?]
    rcases h a List.mem_cons_self with ha | ha
    · rw [ha]
      exact ih (fun x hx => h x (List.mem_cons_of_mem _ hx))
    · rw [ha]
      right; rfl

/-- Over a list whose results are `none` or `some e`, `findSome?` is `some e` when one element
    gives `some e`. -/
theorem findSome_hit {α β : Type} (f : α → Option β) (e : β) (l : List α)
    (h : ∀ x ∈ l, f x = none ∨ f x = some e) (x : α) (hx : x ∈ l) (hfx : f x = some e) :
    l.findSome? f = some e := by
  induction l with
  | nil => simp at hx
  | cons a l ih =>
    simp only [List.findSome?]
    rcases h a List.mem_cons_self with ha | ha
    · rw [ha]
      rcases List.mem_cons.mp hx with rfl | hx'
      · rw [ha] at hfx; cases hfx
      · exact ih (fun y hy => h y (List.mem_cons_of_mem _ hy)) hx'
    · rw [ha]

/-- The field-level dichotomy: a field that passes its checks without the unique checks passes
    them, or is refused with `uniqueViolated`. -/
theorem fieldProblem_uniq_dichotomy {L : Limits} {rs : Records} {self : RecordId}
    {fs : Fields} {fd : FieldId × FieldDef} (h : fieldProblem L false rs self fs fd = none) :
    fieldProblem L true rs self fs fd = none ∨
      fieldProblem L true rs self fs fd = some .uniqueViolated := by
  obtain ⟨-, hall⟩ := fieldProblem_none h
  unfold fieldProblem at h ⊢
  by_cases hc : (fd.2.required && (fs.valuesOf fd.1).isEmpty) = true
  · simp only [hc, if_true] at h; cases h
  · simp only [hc, Bool.false_eq_true, if_false]
    exact findSome_dichotomy _ _ _ (fun v hv => valueProblem_uniq_dichotomy (hall v hv))

/-- A field that passes its checks without the unique checks, and holds a value another record
    clashes with, is refused with `uniqueViolated`. -/
theorem fieldProblem_clash {L : Limits} {rs : Records} {self : RecordId} {fs : Fields}
    {fd : FieldId × FieldDef} (h : fieldProblem L false rs self fs fd = none)
    (hu : fd.2.unique = true) (v : Val) (hv : v ∈ fs.valuesOf fd.1)
    (hc : clashAt rs self fd.1 v = true) :
    fieldProblem L true rs self fs fd = some .uniqueViolated := by
  obtain ⟨-, hall⟩ := fieldProblem_none h
  have hval := valueProblem_clash (hall v hv) hu hc
  have hne : (fs.valuesOf fd.1).isEmpty = false := by
    cases hl : fs.valuesOf fd.1 with
    | nil => rw [hl] at hv; cases hv
    | cons _ _ => rfl
  have hfd : (fd.2.required && (fs.valuesOf fd.1).isEmpty) = false := by simp [hne]
  unfold fieldProblem
  rw [hfd]
  simp only [Bool.false_eq_true, if_false]
  exact findSome_hit _ _ _
    (fun x hx => valueProblem_uniq_dichotomy (hall x hx)) v hv hval

/-! ## Lemmas: the record and the schema without a clash -/

/-- A write the unique checks pass names no other record's unique value, and no other record on
    every field of a composite constraint. -/
theorem problem_unique_none {L : Limits} {schema : Schema} {us : List (List FieldId)}
    {rs : Records} {self : RecordId} {r : Record}
    (h : problem L schema us true rs self r = none) :
    (∀ fd ∈ schema, fd.2.unique = true → ∀ q ∈ rs, q.1 ≠ self →
        agreeOn [fd.1] r.fields q.2.fields = false) ∧
    (∀ u ∈ us, ∀ q ∈ rs, q.1 ≠ self → agreeOn u r.fields q.2.fields = false) := by
  unfold problem at h
  split at h
  · cases h
  · split at h
    · split at h
      · cases h
      · rename_i hf
        have hall := List.findSome?_eq_none_iff.mp hf
        refine ⟨fun fd hfd hu q hq hne => ?_, fun u hu q hq hne => ?_⟩
        · obtain ⟨-, hvals⟩ := fieldProblem_none (hall fd hfd)
          cases hag : agreeOn [fd.1] r.fields q.2.fields with
          | false => rfl
          | true =>
            exfalso
            have hk : (r.fields.get fd.1).keyed = true ∧
                r.fields.get fd.1 = q.2.fields.get fd.1 := by
              simpa [agreeOn] using hag
            have hnn : r.fields.get fd.1 ≠ .null := by
              intro h0; rw [h0] at hk; simp [Val.keyed] at hk
            have hv : r.fields.get fd.1 ∈ r.fields.valuesOf fd.1 :=
              mem_valuesOf (get_mem rfl hnn)
            have hp := hvals _ hv
            have hcl := (valueProblem_none hp).2.2.2 rfl hu
            have hcl' : clashAt rs self fd.1 (r.fields.get fd.1) = true := by
              unfold clashAt
              rw [hk.1, Bool.true_and, List.any_eq_true]
              exact ⟨q, hq, by simp [hne, hk.2.symm]⟩
            rw [hcl] at hcl'
            cases hcl'
        · unfold compositeProblem at h
          have hfalse : (us.any (fun u => rs.any (fun q => q.1 != self && agreeOn u r.fields q.2.fields)))
              = false := by
            simpa using h
          have h1 : (rs.any (fun q => q.1 != self && agreeOn u r.fields q.2.fields)) = false := by
            simpa using (List.any_eq_false.mp hfalse u hu)
          have h2 := List.any_eq_false.mp h1 q hq
          have hb : (q.1 != self) = true := by simpa using hne
          simpa [hb] using h2
    · cases h

/-- The refusal of a write that passes the checks without the unique ones, and then clashes: the
    unique checks refuse it, with `uniqueViolated`. Either way the write is not accepted. -/
theorem problem_uniq_dichotomy {L : Limits} {schema : Schema} {us : List (List FieldId)}
    {rs : Records} {self : RecordId} {r : Record}
    (h : problem L schema us false rs self r = none) :
    problem L schema us true rs self r = none ∨
      problem L schema us true rs self r = some .uniqueViolated := by
  unfold problem at h ⊢
  by_cases hs : decide (L.maxRecord < r.size) = true
  · simp only [hs, if_true] at h; cases h
  · simp only [hs, Bool.false_eq_true, if_false] at h ⊢
    by_cases hk : (r.fields.all (fun fv => (schema.lookup fv.1).isSome)) = true
    · simp only [hk, if_true] at h ⊢
      cases hf : schema.findSome? (fieldProblem L false rs self r.fields) with
      | some e => simp [hf] at h
      | none =>
        rw [hf] at h
        have hall := List.findSome?_eq_none_iff.mp hf
        rcases findSome_dichotomy (fieldProblem L true rs self r.fields) .uniqueViolated schema
            (fun x hx => fieldProblem_uniq_dichotomy (hall x hx)) with h1 | h1
        · rw [h1]
          unfold compositeProblem at h ⊢
          by_cases hc : us.any (fun u => rs.any (fun q => q.1 != self && agreeOn u r.fields q.2.fields))
              = true
          · right; simp [hc]
          · left; simp [hc]
        · rw [h1]; right; rfl
    · simp only [hk, Bool.false_eq_true, if_false] at h ⊢
      cases h

/-- The refusal of a write that is valid except that it clashes is `uniqueViolated`: a valid
    write, that is, one the checks without the unique ones pass, which another record clashes with
    on a unique field or a composite constraint, is refused with `uniqueViolated`. -/
theorem problem_uniq_refusal {L : Limits} {schema : Schema} {us : List (List FieldId)}
    {rs : Records} {self : RecordId} {r : Record}
    (hfree : problem L schema us false rs self r = none)
    (hclash : (∃ fd ∈ schema, fd.2.unique = true ∧
        ∃ q ∈ rs, q.1 ≠ self ∧ agreeOn [fd.1] r.fields q.2.fields = true) ∨
      (∃ u ∈ us, ∃ q ∈ rs, q.1 ≠ self ∧ agreeOn u r.fields q.2.fields = true)) :
    problem L schema us true rs self r = some .uniqueViolated := by
  rcases problem_uniq_dichotomy hfree with h | h
  · exfalso
    obtain ⟨h1, h2⟩ := problem_unique_none h
    rcases hclash with ⟨fd, hfd, hu, q, hq, hne, hag⟩ | ⟨u, hu, q, hq, hne, hag⟩
    · have := h1 fd hfd hu q hq hne
      rw [this] at hag; cases hag
    · have := h2 u hu q hq hne
      rw [this] at hag; cases hag
  · exact h

/-! ## Lemmas: agreement -/

theorem agreeOn_comm (us : List FieldId) (a b : Fields) : agreeOn us a b = agreeOn us b a := by
  unfold agreeOn
  congr 1
  funext f
  generalize a.get f = x
  generalize b.get f = y
  by_cases h : x = y
  · subst h; rfl
  · have h' : y ≠ x := fun e => h e.symm
    have e1 : (x == y) = false := beq_eq_false_iff_ne.mpr h
    have e2 : (y == x) = false := beq_eq_false_iff_ne.mpr h'
    rw [e1, e2]
    simp

/-- No two distinct records of a stored state agree on the fields, when the stored state says so. -/
theorem dupFields_false {rs : Records} {us : List FieldId} (h : dupFields rs us = false)
    {q₁ q₂ : RecordId × Record} (h₁ : q₁ ∈ rs) (h₂ : q₂ ∈ rs) (hne : q₁.1 ≠ q₂.1) :
    agreeOn us q₁.2.fields q₂.2.fields = false := by
  unfold dupFields at h
  have h1 : (rs.any (fun q₂ => q₁.1 != q₂.1 && agreeOn us q₁.2.fields q₂.2.fields)) = false := by
    simpa using (List.any_eq_false.mp h q₁ h₁)
  have h2 := List.any_eq_false.mp h1 q₂ h₂
  have hb : (q₁.1 != q₂.1) = true := by simpa using hne
  simpa [hb] using h2

/-- Taking references out of a value that is keyed leaves it as it was. -/
theorem dropRef_getD_keyed (id : RecordId) (v : Val)
    (h : ((v.dropRef id).getD .null).keyed = true) :
    (v.dropRef id).getD .null = v ∧ v.keyed = true := by
  cases v with
  | null => simp [Val.dropRef, Val.keyed] at h
  | int i => simp [Val.dropRef, Val.keyed]
  | str s => simp [Val.dropRef, Val.keyed]
  | bool b => simp [Val.dropRef, Val.keyed]
  | ref x =>
    by_cases hx : x = id
    · simp [Val.dropRef, Val.keyed, hx] at h
    · simp [Val.dropRef, Val.keyed, hx]
  | list vs => simp [Val.dropRef, Val.keyed] at h

theorem get_cons_self (k : FieldId) (v : Val) (fs : Fields) :
    Fields.get ((k, v) :: fs) k = v := by
  simp [Fields.get, List.lookup]

theorem get_cons_ne (k f : FieldId) (h : k ≠ f) (v : Val) (fs : Fields) :
    Fields.get ((k, v) :: fs) f = Fields.get fs f := by
  have hfk : (f == k) = false := beq_eq_false_iff_ne.mpr (Ne.symm h)
  simp [Fields.get, List.lookup, hfk]

/-- The value of a field after the references to a record are cleared: the value it had, cleared
    through `dropRef`. Fields hold one value per name. -/
theorem get_clearFields_eq (id : RecordId) (f : FieldId) :
    ∀ (fs : Fields), (fs.map Prod.fst).Nodup →
      Fields.get (clearFields id fs) f = ((Fields.get fs f).dropRef id).getD .null
  | [], _ => by simp [clearFields, Fields.get, List.lookup, Val.dropRef]
  | (k, v) :: fs, hnd => by
    have hnd' := List.nodup_cons.mp hnd
    have ih := get_clearFields_eq id f fs hnd'.2
    have hk : k ∉ fs.map Prod.fst := hnd'.1
    have hfs : Fields.get fs k = .null := by
      unfold Fields.get
      cases hl : fs.lookup k with
      | none => rfl
      | some u => exact absurd (List.mem_map.mpr ⟨(k, u), lookup_mem hl, rfl⟩) hk
    cases hd : v.dropRef id with
    | none =>
      have hcf : clearFields id ((k, v) :: fs) = clearFields id fs := by
        simp [clearFields, hd]
      rw [hcf, ih]
      by_cases hkf : k = f
      · subst hkf
        rw [get_cons_self, hd, hfs]; rfl
      · rw [get_cons_ne k f hkf v fs]
    | some w =>
      have hcf : clearFields id ((k, v) :: fs) = (k, w) :: clearFields id fs := by
        simp [clearFields, hd]
      rw [hcf]
      by_cases hkf : k = f
      · subst hkf
        rw [get_cons_self, get_cons_self, hd]
        rfl
      · rw [get_cons_ne k f hkf w, get_cons_ne k f hkf v, ih]

/-- Clearing a record's references keeps only the values it had: a value that is keyed after the
    clearing is keyed before it, and unchanged. -/
theorem clearRef_keyed {id : RecordId} {r : Record} (hnd : (r.fields.map Prod.fst).Nodup)
    (f : FieldId) (hk : ((clearRef id r).fields.get f).keyed = true) :
    (clearRef id r).fields.get f = r.fields.get f ∧ (r.fields.get f).keyed = true := by
  by_cases hr : r.refersTo id = true
  · have hc : clearRef id r =
        { r with fields := clearFields id r.fields, version := r.version + 1 } := by
      unfold clearRef; simp [hr]
    rw [hc] at hk ⊢
    dsimp only at hk ⊢
    rw [get_clearFields_eq id f r.fields hnd] at hk ⊢
    exact dropRef_getD_keyed id _ hk
  · have hc : clearRef id r = r := by unfold clearRef; simp [hr]
    rw [hc] at hk ⊢
    exact ⟨rfl, hk⟩

/-- Clearing references keeps agreement only where it was: records that agree on fields after the
    clearing agreed before it. -/
theorem agreeOn_clearRef {id : RecordId} {r₁ r₂ : Record} (hn₁ : (r₁.fields.map Prod.fst).Nodup)
    (hn₂ : (r₂.fields.map Prod.fst).Nodup) (us : List FieldId)
    (h : agreeOn us (clearRef id r₁).fields (clearRef id r₂).fields = true) :
    agreeOn us r₁.fields r₂.fields = true := by
  unfold agreeOn at h ⊢
  rw [List.all_eq_true] at h ⊢
  intro f hf
  have hf' := h f hf
  simp only [Bool.and_eq_true, beq_iff_eq] at hf'
  obtain ⟨hk1, heq⟩ := hf'
  obtain ⟨e1, k1⟩ := clearRef_keyed hn₁ f hk1
  have hk2 : ((clearRef id r₂).fields.get f).keyed = true := by
    rw [← heq]; exact hk1
  obtain ⟨e2, k2⟩ := clearRef_keyed hn₂ f hk2
  simp only [Bool.and_eq_true, beq_iff_eq]
  refine ⟨k1, ?_⟩
  rw [← e1, ← e2, heq]

/-! ## Lemmas: names stay distinct -/

/-- Keys of a filtered and mapped list are keys of the list, each once. -/
theorem nodup_filterMap_fst {γ δ : Type} (g : FieldId × γ → Option (FieldId × δ))
    (hg : ∀ x y, g x = some y → y.1 = x.1) :
    ∀ (l : List (FieldId × γ)), (l.map Prod.fst).Nodup →
      ((l.filterMap g).map Prod.fst).Nodup
  | [], _ => by simp
  | x :: l, hnd => by
    have hnd' := List.nodup_cons.mp hnd
    have ih := nodup_filterMap_fst g hg l hnd'.2
    rw [List.filterMap_cons]
    cases hx : g x with
    | none => exact ih
    | some y =>
      have hy : y.1 = x.1 := hg x y hx
      rw [List.map_cons, List.nodup_cons]
      refine ⟨fun hmem => ?_, ih⟩
      obtain ⟨z, hz, hzy⟩ := List.mem_map.mp hmem
      obtain ⟨a, ha, hga⟩ := List.mem_filterMap.mp hz
      have haz : z.1 = a.1 := hg a z hga
      have hax : a.1 = x.1 := by rw [← haz, hzy, hy]
      exact hnd'.1 (List.mem_map.mpr ⟨a, ha, hax⟩)

/-- Where a name came from in an accumulated set of fields. -/
theorem mem_set_keys (acc : Fields) (p : FieldId × Val) (x : FieldId)
    (h : x ∈ (acc.set p).map Prod.fst) : x ∈ acc.map Prod.fst ∨ x = p.1 := by
  unfold Fields.set at h
  split at h
  · rw [List.mem_map] at h
    obtain ⟨z, hz, hzx⟩ := h
    exact Or.inl (List.mem_map.mpr ⟨z, (List.mem_filter.mp hz).1, hzx⟩)
  · rw [List.map_cons, List.mem_cons] at h
    rcases h with h | h
    · exact Or.inr h
    · rw [List.mem_map] at h
      obtain ⟨z, hz, hzx⟩ := h
      exact Or.inl (List.mem_map.mpr ⟨z, (List.mem_filter.mp hz).1, hzx⟩)

/-- Setting one field keeps the names distinct. -/
theorem set_nodup (fs : Fields) (fv : FieldId × Val) (hnd : (fs.map Prod.fst).Nodup) :
    ((fs.set fv).map Prod.fst).Nodup := by
  unfold Fields.set
  have hfilt : ((fs.filter (fun g => g.1 != fv.1)).map Prod.fst).Nodup :=
    List.Nodup.sublist (List.Sublist.map _ (List.filter_sublist)) hnd
  split
  · exact hfilt
  · rw [List.map_cons, List.nodup_cons]
    refine ⟨fun hmem => ?_, hfilt⟩
    rw [List.mem_map] at hmem
    obtain ⟨z, hz, hzk⟩ := hmem
    have := (List.mem_filter.mp hz).2
    simp at this
    exact this hzk

/-- A patch keeps the names distinct. -/
theorem patch_nodup (ps : Fields) : ∀ (fs : Fields), (fs.map Prod.fst).Nodup →
    ((fs.patch ps).map Prod.fst).Nodup := by
  induction ps with
  | nil => intro fs h; exact h
  | cons p ps ih =>
    intro fs h
    unfold Fields.patch at ih ⊢
    rw [List.foldl_cons]
    exact ih (fs.set p) (set_nodup fs p h)

/-- The names a normalised list keeps are names it was given. -/
theorem normalise_keys_sub : ∀ (ps acc : Fields) (x : FieldId),
    x ∈ (ps.foldl Fields.set acc).map Prod.fst → x ∈ acc.map Prod.fst ∨ x ∈ ps.map Prod.fst
  | [], _, _, h => Or.inl h
  | p :: ps, acc, x, h => by
    rw [List.foldl_cons] at h
    rcases normalise_keys_sub ps (acc.set p) x h with h1 | h1
    · rcases mem_set_keys acc p x h1 with h2 | h2
      · exact Or.inl h2
      · exact Or.inr (by rw [List.map_cons]; exact List.mem_cons.mpr (Or.inl h2))
    · exact Or.inr (by rw [List.map_cons]; exact List.mem_cons.mpr (Or.inr h1))

theorem normalise_nodup (fs : Fields) : ((normalise fs).map Prod.fst).Nodup :=
  patch_nodup fs [] List.nodup_nil

/-- The fields of a new record name each field once, when the schema names each field once. -/
theorem newRecord_nodup (schema : Schema) (p : PrincipalId) (fs : Fields)
    (hs : (schema.map Prod.fst).Nodup) :
    ((newRecord schema p fs).fields.map Prod.fst).Nodup := by
  unfold newRecord withDefaults
  rw [List.map_append, List.nodup_append]
  refine ⟨normalise_nodup fs,
    nodup_filterMap_fst (fun fd => if fs.any (fun fv => fv.1 == fd.1) || fd.2.default == .null
      then none else some (fd.1, fd.2.default)) (fun x y hxy => ?_) schema hs,
    fun a ha b hb hab => ?_⟩
  · split at hxy
    · cases hxy
    · cases hxy; rfl
  · rw [List.mem_map] at ha hb
    obtain ⟨c, hc, hca⟩ := ha
    obtain ⟨d, hd, hdb⟩ := hb
    obtain ⟨fd, -, hg⟩ := List.mem_filterMap.mp hd
    split at hg
    · cases hg
    · rename_i hcond
      cases hg
      have hbfd : b = fd.1 := by simpa using hdb.symm
      have hfsnone : fs.any (fun fv => fv.1 == fd.1) = false := by
        revert hcond
        cases fs.any (fun fv => fv.1 == fd.1) <;> simp
      rcases normalise_keys_sub fs [] a (List.mem_map.mpr ⟨c, hc, hca⟩) with h0 | h0
      · simp at h0
      · obtain ⟨fv, hfv, hfva⟩ := List.mem_map.mp h0
        have hmatch : fs.any (fun fv' => fv'.1 == fd.1) = true := by
          rw [List.any_eq_true]
          exact ⟨fv, hfv, by simp [hfva, hab, hbfd]⟩
        rw [hfsnone] at hmatch
        cases hmatch

theorem clearRef_nodup (id : RecordId) (r : Record) (hnd : (r.fields.map Prod.fst).Nodup) :
    ((clearRef id r).fields.map Prod.fst).Nodup := by
  unfold clearRef
  split
  · exact nodup_filterMap_fst (fun fv => (fv.2.dropRef id).map (fun v => (fv.1, v)))
      (fun x y hxy => by
        cases hd : x.2.dropRef id with
        | none => simp only [hd, Option.map_none] at hxy; cases hxy
        | some v =>
          simp only [hd, Option.map_some] at hxy
          cases hxy; rfl) r.fields hnd
  · exact hnd

theorem keys_makeRequired (s : Schema) (f : FieldId) :
    (makeRequired s f).map Prod.fst = s.map Prod.fst := by
  unfold makeRequired
  rw [List.map_map]
  apply List.map_congr_left
  intro fd _
  dsimp only [Function.comp]
  split <;> rfl

theorem keys_makeUnique (s : Schema) (f : FieldId) :
    (makeUnique s f).map Prod.fst = s.map Prod.fst := by
  unfold makeUnique
  rw [List.map_map]
  apply List.map_congr_left
  intro fd _
  dsimp only [Function.comp]
  split <;> rfl

theorem mem_makeRequired {s : Schema} {f : FieldId} {fd : FieldId × FieldDef}
    (h : fd ∈ makeRequired s f) : ∃ fd₀ ∈ s, fd₀.1 = fd.1 ∧ fd₀.2.unique = fd.2.unique := by
  unfold makeRequired at h
  obtain ⟨a, ha, he⟩ := List.mem_map.mp h
  split at he
  · subst he; exact ⟨a, ha, rfl, rfl⟩
  · subst he; exact ⟨a, ha, rfl, rfl⟩

theorem mem_makeUnique {s : Schema} {f : FieldId} {fd : FieldId × FieldDef}
    (h : fd ∈ makeUnique s f) :
    ∃ fd₀ ∈ s, fd₀.1 = fd.1 ∧ (fd.1 = f ∨ fd₀.2.unique = fd.2.unique) := by
  unfold makeUnique at h
  obtain ⟨a, ha, he⟩ := List.mem_map.mp h
  split at he
  · subst he
    rename_i hf
    exact ⟨a, ha, by simpa using hf, Or.inl (by simpa using hf)⟩
  · subst he
    exact ⟨a, ha, rfl, Or.inr rfl⟩

/-! ## Lemmas: the records a write leaves -/

/-- A record replaced by id: the replacement, or a record the replacement does not touch. -/
theorem mem_replace {rs : Records} {id : RecordId} {r : Record} {x : RecordId} {z : Record}
    (h : (x, z) ∈ rs.map (fun q => if q.1 == id then (q.1, r) else q)) :
    (x = id ∧ z = r) ∨ ((x, z) ∈ rs ∧ x ≠ id) := by
  obtain ⟨a, ha, he⟩ := List.mem_map.mp h
  by_cases hq : (a.1 == id) = true
  · simp only [hq, if_true, Prod.mk.injEq] at he
    obtain ⟨h1, h2⟩ := he
    left
    have hid : a.1 = id := by simpa using hq
    exact ⟨by rw [← h1]; exact hid, h2.symm⟩
  · simp only [hq, Bool.false_eq_true, if_false] at he
    right
    have hne : a.1 ≠ id := by simpa using hq
    subst he
    exact ⟨ha, hne⟩

/-- Clearing and deleting keep an old record's name in the new records. -/
theorem mem_afterDelete {rs : Records} {id : RecordId} {x : RecordId} {z : Record}
    (h : (x, z) ∈ (rs.filter (fun q => q.1 != id)).map (fun q => (q.1, clearRef id q.2))) :
    ∃ a ∈ rs, a.1 = x ∧ a.1 ≠ id ∧ z = clearRef id a.2 := by
  obtain ⟨a, ha, he⟩ := List.mem_map.mp h
  obtain ⟨hmem, hne⟩ := List.mem_filter.mp ha
  refine ⟨a, hmem, by simp at he; exact he.1 ▸ rfl, by simpa using hne, by simp at he; exact he.2.symm⟩

/-! ## The invariant through accepted steps -/

/-- The names an accepted step leaves distinct. -/
theorem stepTenant_preserves_distinct (t t' : Tenant) (p : PrincipalId) (i : Intent)
    (hd : t.Distinct) (h : stepTenant t p i = .ok t') : t'.Distinct := by
  obtain ⟨hs, hr⟩ := hd
  cases i with
  | create fs =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · rename_i hp
        cases h
        refine ⟨hs, ?_⟩
        intro q hq
        rcases List.mem_cons.mp hq with rfl | hq'
        · exact newRecord_nodup t.schema p fs hs
        · exact hr q hq'
  | update id fs ev =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · rename_i old hl
      split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · rename_i hp
            cases h
            refine ⟨hs, ?_⟩
            intro ⟨x, z⟩ hq
            rcases mem_replace hq with ⟨-, rfl⟩ | ⟨hq', -⟩
            · exact patch_nodup fs old.fields (hr (id, old) (lookup_mem hl))
            · exact hr (x, z) hq'
  | delete id ev =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · cases h
            refine ⟨hs, ?_⟩
            intro ⟨x, z⟩ hq
            obtain ⟨a, ha, -, -, rfl⟩ := mem_afterDelete hq
            exact clearRef_nodup id a.2 (hr a ha)
  | setRequired f strict =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · cases h
        refine ⟨?_, hr⟩
        rw [keys_makeRequired]; exact hs
  | setUnique f strict =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · cases h
            refine ⟨?_, hr⟩
            rw [keys_makeUnique]; exact hs
  | defineUnique fs =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · cases h
            exact ⟨hs, hr⟩

/-! ## A strict schema change leaves the tenant conforming -/

/-- The check of a schema change passes: every record conforms to the schema, and no two distinct
    records agree on a unique field of it or on a composite constraint. -/
theorem schemaViolated_false {L : Limits} {schema : Schema} {us : List (List FieldId)}
    {rs : Records} (h : schemaViolated L schema us rs = false) :
    (∀ q ∈ rs, conforms schema q.2 = true) ∧
      ∀ q₁ ∈ rs, ∀ q₂ ∈ rs, q₁.1 ≠ q₂.1 →
        (∀ fd ∈ schema, fd.2.unique = true → agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
        (∀ u ∈ us, agreeOn u q₁.2.fields q₂.2.fields = false) := by
  unfold schemaViolated at h
  obtain ⟨hab, hus⟩ := Bool.or_eq_false_iff.mp h
  obtain ⟨ha, hany⟩ := Bool.or_eq_false_iff.mp hab
  have hconf : rs.all (fun q => conforms schema q.2) = true := by simpa using ha
  refine ⟨fun q hq => List.all_eq_true.mp hconf q hq, ?_⟩
  intro q₁ h₁ q₂ h₂ hne
  refine ⟨?_, ?_⟩
  · intro fd hfd hu
    have hf := List.any_eq_false.mp hany fd hfd
    have hd : (dupFields rs [fd.1] ||
        rs.any (fun q => decide (L.maxUnique < (q.2.fields.get fd.1).size))) = false := by
      cases hx : (dupFields rs [fd.1] ||
          rs.any (fun q => decide (L.maxUnique < (q.2.fields.get fd.1).size))) with
      | false => rfl
      | true => exact absurd ((Bool.and_eq_true _ _).mpr ⟨hu, hx⟩) hf
    exact dupFields_false (Bool.or_eq_false_iff.mp hd).1 h₁ h₂ hne
  · intro u hu'
    have hu'' : dupFields rs u = false := Bool.eq_false_iff.mpr (List.any_eq_false.mp hus u hu')
    exact dupFields_false hu'' h₁ h₂ hne

/-- A strict schema change is accepted only if the tenant that results conforms: every record
    conforms to the resulting schema, and the unique fields and composite constraints hold across
    the records. This is the rule the kernel's `Reject` applies to every schema change, so a
    violation a lax change left behind refuses a later strict change whatever it is about. -/
theorem strict_schema_change_conforms (t t' : Tenant) (p : PrincipalId) (i : Intent)
    (hi : i.isSchema = true) (hs : i.strict = true) (h : stepTenant t p i = .ok t') :
    (∀ q ∈ t'.records, conforms t'.schema q.2 = true) ∧ t'.UniqueRecords := by
  cases i with
  | create fs => simp [Intent.isSchema] at hi
  | update id fs ev => simp [Intent.isSchema] at hi
  | delete id ev => simp [Intent.isSchema] at hi
  | setRequired f s =>
    simp only [Intent.strict] at hs
    subst hs
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · rename_i hn
        cases h
        have hv : schemaViolated t.limits (makeRequired t.schema f) t.uniques t.records = false := by
          cases hb : schemaViolated t.limits (makeRequired t.schema f) t.uniques t.records with
          | false => rfl
          | true => exfalso; apply hn; simp [schemaChangeRefused, hb]
        exact schemaViolated_false hv
  | setUnique f s =>
    simp only [Intent.strict] at hs
    subst hs
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · rename_i hn
            cases h
            have hv : schemaViolated t.limits (makeUnique t.schema f) t.uniques t.records = false := by
              cases hb : schemaViolated t.limits (makeUnique t.schema f) t.uniques t.records with
              | false => rfl
              | true => exfalso; apply hn; simp [schemaChangeRefused, hb]
            exact schemaViolated_false hv
  | defineUnique fs =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · rename_i hn
            cases h
            have hv : schemaViolated t.limits t.schema (t.uniques ++ [fs]) t.records = false := by
              cases hb : schemaViolated t.limits t.schema (t.uniques ++ [fs]) t.records with
              | false => rfl
              | true => exfalso; apply hn; simp [schemaChangeRefused, hb]
            exact schemaViolated_false hv

/-- A schema change that leaves the records as they are, with the nextid and records named. -/
theorem setRequired_spec (t t' : Tenant) (p : PrincipalId) (f : FieldId) (s : Bool)
    (h : stepTenant t p (.setRequired f s) = .ok t') :
    t'.nextId = t.nextId ∧ t'.records = t.records := by
  simp only [stepTenant] at h
  repeat' split at h
  all_goals (try cases h)
  all_goals exact ⟨rfl, rfl⟩

/-! ## The schema changes, and conformance under the unique flag -/

/-- A field made unique changes only the schema, and only its unique flag. -/
theorem setUnique_spec (t t' : Tenant) (p : PrincipalId) (f : FieldId) (s : Bool)
    (h : stepTenant t p (.setUnique f s) = .ok t') :
    t'.nextId = t.nextId ∧ t'.records = t.records ∧ t'.schema = makeUnique t.schema f ∧
      t'.uniques = t.uniques := by
  simp only [stepTenant] at h
  repeat' split at h
  all_goals (try cases h)
  all_goals exact ⟨rfl, rfl, rfl, rfl⟩

/-- A composite constraint changes only the list of constraints. -/
theorem defineUnique_spec (t t' : Tenant) (p : PrincipalId) (fs : List FieldId)
    (h : stepTenant t p (.defineUnique fs) = .ok t') :
    t'.nextId = t.nextId ∧ t'.records = t.records ∧ t'.schema = t.schema ∧
      t'.uniques = t.uniques ++ [fs] := by
  simp only [stepTenant] at h
  repeat' split at h
  all_goals (try cases h)
  all_goals exact ⟨rfl, rfl, rfl, rfl⟩

theorem lookup_map_ty (g : FieldId × FieldDef → FieldId × FieldDef)
    (hg : ∀ x, (g x).1 = x.1 ∧ (g x).2.ty = x.2.ty) (s : Schema) (k : FieldId) :
    ((s.map g).lookup k).map FieldDef.ty = (s.lookup k).map FieldDef.ty := by
  induction s with
  | nil => rfl
  | cons x rest ih =>
    obtain ⟨h1, h2⟩ := hg x
    simp only [List.map_cons, List.lookup, h1]
    split
    · simp [h2]
    · exact ih

theorem lookup_makeUnique_ty (s : Schema) (f k : FieldId) :
    ((makeUnique s f).lookup k).map FieldDef.ty = (s.lookup k).map FieldDef.ty := by
  unfold makeUnique
  apply lookup_map_ty
  intro x
  split <;> simp

theorem fieldOk_makeUnique (s : Schema) (f : FieldId) (fv : FieldId × Val) :
    fieldOk (makeUnique s f) fv = fieldOk s fv := by
  unfold fieldOk
  have h := lookup_makeUnique_ty s f fv.1
  cases h1 : (makeUnique s f).lookup fv.1 <;> cases h2 : s.lookup fv.1 <;>
    simp_all

/-- The unique flag does not change what a record conforms to. -/
theorem conforms_makeUnique {s : Schema} {f : FieldId} {r : Record} (h : conforms s r = true) :
    conforms (makeUnique s f) r = true := by
  unfold conforms at h ⊢
  simp only [Bool.and_eq_true, List.all_eq_true] at h ⊢
  obtain ⟨h1, h2⟩ := h
  refine ⟨fun fv hfv => ?_, fun fd hfd => ?_⟩
  · rw [fieldOk_makeUnique]
    exact h1 fv hfv
  · unfold makeUnique at hfd
    obtain ⟨a, ha, he⟩ := List.mem_map.mp hfd
    have h3 := h2 a ha
    subst he
    unfold requiredOk at h3 ⊢
    split <;> simpa using h3

/-! ## The uniqueness invariant through accepted steps -/

/-- An accepted strict step keeps the unique fields and the composite constraints unique. -/
theorem stepTenant_preserves_unique (t t' : Tenant) (p : PrincipalId) (i : Intent)
    (hwf : t.WF) (hstrict : i.strict = true) (h : stepTenant t p i = .ok t') :
    t'.UniqueRecords := by
  obtain ⟨-, hids, -, hd, hU⟩ := hwf
  cases i with
  | create fs =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · rename_i hp
        cases h
        unfold Tenant.UniqueRecords
        intro q₁ hq₁ q₂ hq₂ hne
        have hnew : ∀ q ∈ t.records, q.1 ≠ t.nextId := fun q hq => Nat.ne_of_lt (hids q hq)
        have hP := problem_unique_none hp
        rcases List.mem_cons.mp hq₁ with rfl | h₁ <;>
          rcases List.mem_cons.mp hq₂ with rfl | h₂
        · exact absurd rfl hne
        · exact ⟨fun fd hfd hu => hP.1 fd hfd hu q₂ h₂ (hnew q₂ h₂),
            fun u hu' => hP.2 u hu' q₂ h₂ (hnew q₂ h₂)⟩
        · exact ⟨fun fd hfd hu => by
            rw [agreeOn_comm]; exact hP.1 fd hfd hu q₁ h₁ (hnew q₁ h₁),
            fun u hu' => by rw [agreeOn_comm]; exact hP.2 u hu' q₁ h₁ (hnew q₁ h₁)⟩
        · exact hU q₁ h₁ q₂ h₂ hne
  | update id fs ev =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · rename_i old hl
      split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · rename_i hp
            cases h
            unfold Tenant.UniqueRecords
            rintro ⟨x, z⟩ hq₁ ⟨y, z'⟩ hq₂ hne
            have hP := problem_unique_none hp
            rcases mem_replace hq₁ with ⟨hx, rfl⟩ | ⟨h₁, hx₁⟩ <;>
              rcases mem_replace hq₂ with ⟨hy, rfl⟩ | ⟨h₂, hy₂⟩
            · exact absurd (hx.trans hy.symm) hne
            · exact ⟨fun fd hfd hu => hP.1 fd hfd hu (y, z') h₂ hy₂,
                fun u hu' => hP.2 u hu' (y, z') h₂ hy₂⟩
            · exact ⟨fun fd hfd hu => by
                rw [agreeOn_comm]; exact hP.1 fd hfd hu (x, z) h₁ hx₁,
                fun u hu' => by rw [agreeOn_comm]; exact hP.2 u hu' (x, z) h₁ hx₁⟩
            · exact hU (x, z) h₁ (y, z') h₂ hne
  | delete id ev =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · cases h
            unfold Tenant.UniqueRecords
            rintro ⟨x, z⟩ hq₁ ⟨y, z'⟩ hq₂ hne
            obtain ⟨a, ha, hxa, -, rfl⟩ := mem_afterDelete hq₁
            obtain ⟨b, hb, hyb, -, rfl⟩ := mem_afterDelete hq₂
            have hab : a.1 ≠ b.1 := by rw [hxa, hyb]; exact hne
            have hU' := hU a ha b hb hab
            refine ⟨fun fd hfd hu => ?_, fun u hu' => ?_⟩
            · show agreeOn [fd.1] (clearRef id a.2).fields (clearRef id b.2).fields = false
              cases hag : agreeOn [fd.1] (clearRef id a.2).fields (clearRef id b.2).fields with
              | false => rfl
              | true =>
                have h2 := agreeOn_clearRef (hd.2 a ha) (hd.2 b hb) [fd.1] hag
                rw [hU'.1 fd hfd hu] at h2
                cases h2
            · show agreeOn u (clearRef id a.2).fields (clearRef id b.2).fields = false
              cases hag : agreeOn u (clearRef id a.2).fields (clearRef id b.2).fields with
              | false => rfl
              | true =>
                have h2 := agreeOn_clearRef (hd.2 a ha) (hd.2 b hb) u hag
                rw [hU'.2 u hu'] at h2
                cases h2
  | setRequired f strict =>
    exact (strict_schema_change_conforms t t' p (.setRequired f strict) rfl hstrict h).2
  | setUnique f strict =>
    exact (strict_schema_change_conforms t t' p (.setUnique f strict) rfl hstrict h).2
  | defineUnique fs =>
    exact (strict_schema_change_conforms t t' p (.defineUnique fs) rfl hstrict h).2

/-! ## Refusals -/

/-- A create that would duplicate another record's unique value, or composite values, and is
    otherwise valid, is refused with `uniqueViolated`, and no state follows. -/
theorem create_duplicate_refused (w : World) (t : Tenant) (c : Ctx) (fs : Fields)
    (ht : w c.tenant = some t) (hc : canCreate t c.principal = true)
    (hfree : problem t.limits t.schema t.uniques false t.records t.nextId
      (newRecord t.schema c.principal fs) = none)
    (hclash : (∃ fd ∈ t.schema, fd.2.unique = true ∧ ∃ q ∈ t.records, q.1 ≠ t.nextId ∧
        agreeOn [fd.1] (newRecord t.schema c.principal fs).fields q.2.fields = true) ∨
      (∃ u ∈ t.uniques, ∃ q ∈ t.records, q.1 ≠ t.nextId ∧
        agreeOn u (newRecord t.schema c.principal fs).fields q.2.fields = true)) :
    step w (.create fs) c = .error .uniqueViolated ∧
      ∀ w', step w (.create fs) c ≠ .ok w' := by
  have hs : step w (.create fs) c = .error .uniqueViolated := by
    simp [step, ht, stepTenant, hc, problem_uniq_refusal hfree hclash]
  exact ⟨hs, fun w' h => by rw [hs] at h; cases h⟩

/-- An update that would duplicate another record's unique value, or composite values, and is
    otherwise valid, is refused with `uniqueViolated`, and no state follows. -/
theorem update_duplicate_refused (w : World) (t : Tenant) (c : Ctx) (id : RecordId) (fs : Fields)
    (ev : Option Nat) (old : Record)
    (ht : w c.tenant = some t) (hl : t.records.lookup id = some old)
    (hcan : canEdit t c.principal old = true) (hv : versionClash ev old = false)
    (hfree : problem t.limits t.schema t.uniques false t.records id (patched old fs) = none)
    (hclash : (∃ fd ∈ t.schema, fd.2.unique = true ∧ ∃ q ∈ t.records, q.1 ≠ id ∧
        agreeOn [fd.1] (patched old fs).fields q.2.fields = true) ∨
      (∃ u ∈ t.uniques, ∃ q ∈ t.records, q.1 ≠ id ∧
        agreeOn u (patched old fs).fields q.2.fields = true)) :
    step w (.update id fs ev) c = .error .uniqueViolated ∧
      ∀ w', step w (.update id fs ev) c ≠ .ok w' := by
  have hs : step w (.update id fs ev) c = .error .uniqueViolated := by
    simp [step, ht, stepTenant, hl, hcan, hv, problem_uniq_refusal hfree hclash]
  exact ⟨hs, fun w' h => by rw [hs] at h; cases h⟩

/-- A composite constraint that existing records already violate is refused with `uniqueViolated`,
    and no state follows. -/
theorem define_unique_refused_over_duplicates (w : World) (t : Tenant) (c : Ctx) (fs : List FieldId)
    (ht : w c.tenant = some t) (hp : c.principal = t.owner)
    (hok : uniqueFieldsOk t.schema fs = true) (hdup : dupFields t.records fs = true) :
    step w (.defineUnique fs) c = .error .uniqueViolated ∧
      ∀ w', step w (.defineUnique fs) c ≠ .ok w' := by
  have hs : step w (.defineUnique fs) c = .error .uniqueViolated := by
    simp [step, ht, stepTenant, hp, hok, hdup]
  exact ⟨hs, fun w' h => by rw [hs] at h; cases h⟩

/-- Making a field unique strictly, over records that already agree on it, is refused with
    `migrationRejected` (the strict schema change's refusal, as the kernel gives it), and no state
    follows. -/
theorem set_unique_refused_over_duplicates (w : World) (t : Tenant) (c : Ctx) (f : FieldId)
    (d : FieldDef)
    (ht : w c.tenant = some t) (hp : c.principal = t.owner)
    (hd : t.schema.lookup f = some d) (hl : d.ty.isList = false)
    (hdup : dupFields t.records [f] = true) :
    step w (.setUnique f true) c = .error .migrationRejected ∧
      ∀ w', step w (.setUnique f true) c ≠ .ok w' := by
  have hs : step w (.setUnique f true) c = .error .migrationRejected := by
    have hany : (makeUnique t.schema f).any (fun fd => fd.2.unique &&
        (dupFields t.records [fd.1] ||
          t.records.any (fun q => decide (t.limits.maxUnique < (q.2.fields.get fd.1).size)))) = true := by
      refine List.any_eq_true.mpr ⟨(f, { d with unique := true }), ?_, ?_⟩
      · unfold makeUnique
        exact List.mem_map.mpr ⟨(f, d), lookup_mem hd, by simp⟩
      · simp [hdup]
    have hv : schemaChangeRefused t.limits (makeUnique t.schema f) t.uniques t.records true = true := by
      unfold schemaChangeRefused schemaViolated
      simp [hany]
    simp [step, ht, stepTenant, hp, hd, hl, hv]
  exact ⟨hs, fun w' h => by rw [hs] at h; cases h⟩

/-! ## A lax change can break uniqueness -/

/-- Two records that agree on field 1, which is not unique. -/
def laxUniqueTenant : Tenant :=
  { schema := [(1, { ty := .int, required := false })], owner := 0, editors := [], writers := [],
    records := [(0, { owner := 0, version := 1, fields := [(1, .int 5)] }),
                (1, { owner := 0, version := 1, fields := [(1, .int 5)] })],
    nextId := 2 }

/-- The exclusion of the non-strict field change is necessary for uniqueness, as it is for
    conformance: a lax `setUnique` over two records that agree on the field leaves a tenant that is
    not well formed. -/
theorem lax_unique_change_can_break_wf :
    ∃ (t t' : Tenant) (p : PrincipalId) (f : FieldId),
      t.WF ∧ stepTenant t p (.setUnique f false) = .ok t' ∧ ¬ t'.UniqueRecords := by
  refine ⟨laxUniqueTenant, { laxUniqueTenant with schema := makeUnique laxUniqueTenant.schema 1 },
    0, 1, ?_, rfl, ?_⟩
  · unfold laxUniqueTenant Tenant.WF Tenant.RefsOk Tenant.Distinct Tenant.UniqueRecords
    decide
  · intro h
    have h1 := (h (0, { owner := 0, version := 1, fields := [(1, .int 5)] }) (by decide)
      (1, { owner := 0, version := 1, fields := [(1, .int 5)] }) (by decide) (by decide)).1
    have h2 := h1 (1, { ty := .int, required := false, unique := true }) (by decide) rfl
    revert h2
    decide

end Strata
