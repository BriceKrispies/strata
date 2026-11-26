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
theorem valueProblem_conds {L : Limits} {uniq : Bool} {env : Env} {rs : Records} {self : RecordId}
    {f : FieldId} {d : FieldDef} {v : Val} (h : valueProblem L uniq env rs self f d v = none) :
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
theorem valueProblem_uniq_dichotomy {L : Limits} {env : Env} {rs : Records} {self : RecordId} {f : FieldId}
    {d : FieldDef} {v : Val} (h : valueProblem L false env rs self f d v = none) :
    valueProblem L true env rs self f d v = none ∨
      valueProblem L true env rs self f d v = some .uniqueViolated := by
  obtain ⟨hov, hb, hty⟩ := valueProblem_conds h
  have hres : v.resolves env d.ty = true := (valueProblem_none h).2.2.1
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
theorem valueProblem_clash {L : Limits} {env : Env} {rs : Records} {self : RecordId} {f : FieldId}
    {d : FieldDef} {v : Val} (h : valueProblem L false env rs self f d v = none) (hu : d.unique = true)
    (hc : clashAt rs self f v = true) :
    valueProblem L true env rs self f d v = some .uniqueViolated := by
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
theorem fieldProblem_uniq_dichotomy {L : Limits} {env : Env} {rs : Records} {self : RecordId}
    {fs : Fields} {fd : FieldId × FieldDef} (h : fieldProblem L false env rs self fs fd = none) :
    fieldProblem L true env rs self fs fd = none ∨
      fieldProblem L true env rs self fs fd = some .uniqueViolated := by
  obtain ⟨-, hall⟩ := fieldProblem_none h
  unfold fieldProblem at h ⊢
  by_cases hc : (fd.2.required && (fs.valuesOf fd.1).isEmpty) = true
  · simp only [hc, if_true] at h; cases h
  · simp only [hc, Bool.false_eq_true, if_false]
    exact findSome_dichotomy _ _ _ (fun v hv => valueProblem_uniq_dichotomy (hall v hv))

/-- A field that passes its checks without the unique checks, and holds a value another record
    clashes with, is refused with `uniqueViolated`. -/
theorem fieldProblem_clash {L : Limits} {env : Env} {rs : Records} {self : RecordId} {fs : Fields}
    {fd : FieldId × FieldDef} (h : fieldProblem L false env rs self fs fd = none)
    (hu : fd.2.unique = true) (v : Val) (hv : v ∈ fs.valuesOf fd.1)
    (hc : clashAt rs self fd.1 v = true) :
    fieldProblem L true env rs self fs fd = some .uniqueViolated := by
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
    {env : Env} {rs : Records} {self : RecordId} {r : Record}
    (h : problem L schema us true env rs self r = none) :
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
    {env : Env} {rs : Records} {self : RecordId} {r : Record}
    (h : problem L schema us false env rs self r = none) :
    problem L schema us true env rs self r = none ∨
      problem L schema us true env rs self r = some .uniqueViolated := by
  unfold problem at h ⊢
  by_cases hs : decide (L.maxRecord < r.size) = true
  · simp only [hs, if_true] at h; cases h
  · simp only [hs, Bool.false_eq_true, if_false] at h ⊢
    by_cases hk : (r.fields.all (fun fv => (schema.lookup fv.1).isSome)) = true
    · simp only [hk, if_true] at h ⊢
      cases hf : schema.findSome? (fieldProblem L false env rs self r.fields) with
      | some e => simp [hf] at h
      | none =>
        rw [hf] at h
        have hall := List.findSome?_eq_none_iff.mp hf
        rcases findSome_dichotomy (fieldProblem L true env rs self r.fields) .uniqueViolated schema
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
    {env : Env} {rs : Records} {self : RecordId} {r : Record}
    (hfree : problem L schema us false env rs self r = none)
    (hclash : (∃ fd ∈ schema, fd.2.unique = true ∧
        ∃ q ∈ rs, q.1 ≠ self ∧ agreeOn [fd.1] r.fields q.2.fields = true) ∨
      (∃ u ∈ us, ∃ q ∈ rs, q.1 ≠ self ∧ agreeOn u r.fields q.2.fields = true)) :
    problem L schema us true env rs self r = some .uniqueViolated := by
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

/-- The defaults a create adds have distinct names, all in the schema and none of them named by the
    intent. -/
theorem defaultsOf_nodup (env : ExprEnv) (fs : Fields) : ∀ (schema : Schema) (ds : Fields),
    defaultsOf env schema fs = .ok ds → (schema.map Prod.fst).Nodup →
    (ds.map Prod.fst).Nodup ∧
      ∀ x ∈ ds.map Prod.fst, x ∈ schema.map Prod.fst ∧ x ∉ fs.map Prod.fst
  | [], ds, h, _ => by
    simp only [defaultsOf, Except.ok.injEq] at h
    subst h
    simp
  | (g, e) :: ss, ds, h, hs => by
    have hnd := List.nodup_cons.mp hs
    unfold defaultsOf at h
    cases hr : defaultsOf env ss fs with
    | error x => simp [hr] at h
    | ok rest =>
      simp only [hr] at h
      obtain ⟨ihn, ihs⟩ := defaultsOf_nodup env fs ss rest hr hnd.2
      by_cases hg : fs.any (fun fv => fv.1 == g) = true
      · simp only [hg, if_true] at h
        have hds : rest = ds := Except.ok.inj h
        subst hds
        refine ⟨ihn, fun x hx => ?_⟩
        obtain ⟨a, b⟩ := ihs x hx
        exact ⟨List.mem_cons_of_mem _ a, b⟩
      · have hg' : fs.any (fun fv => fv.1 == g) = false := Bool.eq_false_iff.mpr hg
        have hgn : g ∉ fs.map Prod.fst := by
          intro hmem
          obtain ⟨fv, hfv, hfg⟩ := List.mem_map.mp hmem
          have : fs.any (fun fv => fv.1 == g) = true := by
            rw [List.any_eq_true]; exact ⟨fv, hfv, by simp [hfg]⟩
          rw [hg'] at this; cases this
        simp only [hg', Bool.false_eq_true, if_false] at h
        split at h
        · cases h
        · have hds := Except.ok.inj h
          subst hds
          refine ⟨ihn, fun x hx => ?_⟩
          obtain ⟨a, b⟩ := ihs x hx
          exact ⟨List.mem_cons_of_mem _ a, b⟩
        · have hds := Except.ok.inj h
          subst hds
          refine ⟨List.nodup_cons.mpr ⟨fun hmem => ?_, ihn⟩, fun x hx => ?_⟩
          · obtain ⟨a, _⟩ := ihs g hmem
            exact hnd.1 a
          · rcases List.mem_cons.mp hx with rfl | hx'
            · exact ⟨List.mem_cons_self, hgn⟩
            · obtain ⟨a, b⟩ := ihs x hx'
              exact ⟨List.mem_cons_of_mem _ a, b⟩

/-- The fields of a new record name each field once, when the schema names each field once. -/
theorem newRecord_nodup (schema : Schema) (p : Principal) (fs : Fields) (r : Record)
    (hN : newRecord schema p fs = .ok r) (hs : (schema.map Prod.fst).Nodup) :
    (r.fields.map Prod.fst).Nodup := by
  obtain ⟨ds, hD, rfl⟩ := newRecord_parts hN
  simp only [List.map_append]
  rw [List.nodup_append]
  refine ⟨normalise_nodup fs, (defaultsOf_nodup _ fs schema ds hD hs).1, fun a ha b hb hab => ?_⟩
  have hbfs := ((defaultsOf_nodup _ fs schema ds hD hs).2 b hb).2
  rw [hab] at ha
  rcases normalise_keys_sub fs [] b ha with h0 | h0
  · simp at h0
  · exact hbfs h0

theorem fieldRefers_null (schema : Schema) (T : TypeId) (id : RecordId) (f : FieldId) :
    fieldRefers schema T id (f, .null) = false := by
  unfold fieldRefers
  cases schema.lookup f with
  | none => rfl
  | some d => simp [Val.refersTo]

theorem get_clearFields_eq (schema : Schema) (T : TypeId) (id : RecordId) (f : FieldId) :
    ∀ (fs : Fields), (fs.map Prod.fst).Nodup →
      Fields.get (clearFields schema T id fs) f =
        if fieldRefers schema T id (f, Fields.get fs f) then ((Fields.get fs f).dropRef id).getD .null
        else Fields.get fs f
  | [], _ => by
    have h0 : fieldRefers schema T id (f, Fields.get [] f) = false := fieldRefers_null _ _ _ _
    rw [if_neg (by simp [h0])]
    rfl
  | (k, v) :: fs, hnd => by
    have hnd' := List.nodup_cons.mp hnd
    have ih := get_clearFields_eq schema T id f fs hnd'.2
    have hk : k ∉ fs.map Prod.fst := hnd'.1
    have hfs : Fields.get fs k = .null := by
      unfold Fields.get
      cases hl : fs.lookup k with
      | none => rfl
      | some u => exact absurd (List.mem_map.mpr ⟨(k, u), lookup_mem hl, rfl⟩) hk
    by_cases hkf : k = f
    · have hg : Fields.get ((k, v) :: fs) f = v := by rw [← hkf]; exact get_cons_self k v fs
      have hgf : Fields.get fs f = .null := by rw [← hkf]; exact hfs
      rw [hg]
      by_cases hr : fieldRefers schema T id (f, v) = true
      · rw [if_pos hr]
        have hrk : fieldRefers schema T id (k, v) = true := by rw [hkf]; exact hr
        cases hd : v.dropRef id with
        | none =>
          have hcf : clearFields schema T id ((k, v) :: fs) = clearFields schema T id fs := by
            simp [clearFields, hrk, hd]
          rw [hcf, ih, hgf, fieldRefers_null]
          simp
        | some w =>
          have hcf : clearFields schema T id ((k, v) :: fs) = (k, w) :: clearFields schema T id fs := by
            simp [clearFields, hrk, hd]
          rw [hcf, ← hkf, get_cons_self]
          simp
      · have hr' : fieldRefers schema T id (f, v) = false := by simpa using hr
        rw [if_neg hr]
        have hrk : fieldRefers schema T id (k, v) = false := by rw [hkf]; exact hr'
        have hcf : clearFields schema T id ((k, v) :: fs) = (k, v) :: clearFields schema T id fs := by
          simp [clearFields, hrk]
        rw [hcf, ← hkf, get_cons_self]
    · rw [get_cons_ne k f hkf v fs]
      have hcf : Fields.get (clearFields schema T id ((k, v) :: fs)) f =
          Fields.get (clearFields schema T id fs) f := by
        by_cases hr : fieldRefers schema T id (k, v) = true
        · cases hd : v.dropRef id with
          | none =>
            have hc2 : clearFields schema T id ((k, v) :: fs) = clearFields schema T id fs := by
              simp [clearFields, hr, hd]
            rw [hc2]
          | some w =>
            have hc2 : clearFields schema T id ((k, v) :: fs) =
                (k, w) :: clearFields schema T id fs := by
              simp [clearFields, hr, hd]
            rw [hc2, get_cons_ne k f hkf w]
        · have hr' : fieldRefers schema T id (k, v) = false := by simpa using hr
          have hc2 : clearFields schema T id ((k, v) :: fs) = (k, v) :: clearFields schema T id fs := by
            simp [clearFields, hr']
          rw [hc2, get_cons_ne k f hkf v]
      rw [hcf, ih]


/-- Clearing a record's references keeps only the values it had: a value that is keyed after the
    clearing is keyed before it, and unchanged. -/
theorem clearRef_keyed {schema : Schema} {T : TypeId} {id : RecordId} {r : Record}
    (hnd : (r.fields.map Prod.fst).Nodup) (f : FieldId)
    (hk : ((clearRef schema T id r).fields.get f).keyed = true) :
    (clearRef schema T id r).fields.get f = r.fields.get f ∧ (r.fields.get f).keyed = true := by
  by_cases hr : recRefers schema T id r = true
  · have hc : clearRef schema T id r =
        { r with fields := clearFields schema T id r.fields, version := r.version + 1 } := by
      unfold clearRef; rw [if_pos hr]
    rw [hc] at hk ⊢
    dsimp only at hk ⊢
    rw [get_clearFields_eq schema T id f r.fields hnd] at hk ⊢
    by_cases hf : fieldRefers schema T id (f, r.fields.get f) = true
    · rw [if_pos hf] at hk ⊢
      exact dropRef_getD_keyed id _ hk
    · rw [if_neg hf] at hk ⊢
      exact ⟨rfl, hk⟩
  · have hc : clearRef schema T id r = r := by unfold clearRef; rw [if_neg hr]
    rw [hc] at hk ⊢
    exact ⟨rfl, hk⟩

/-- Clearing references keeps agreement only where it was: records that agree on fields after the
    clearing agreed before it. -/
theorem agreeOn_clearRef {schema : Schema} {T : TypeId} {id : RecordId} {r₁ r₂ : Record}
    (hn₁ : (r₁.fields.map Prod.fst).Nodup) (hn₂ : (r₂.fields.map Prod.fst).Nodup)
    (us : List FieldId)
    (h : agreeOn us (clearRef schema T id r₁).fields (clearRef schema T id r₂).fields = true) :
    agreeOn us r₁.fields r₂.fields = true := by
  unfold agreeOn at h ⊢
  rw [List.all_eq_true] at h ⊢
  intro f hf
  have hf' := h f hf
  simp only [Bool.and_eq_true, beq_iff_eq] at hf'
  obtain ⟨hk1, heq⟩ := hf'
  obtain ⟨e1, k1⟩ := clearRef_keyed (schema := schema) (T := T) (id := id) hn₁ f hk1
  have hk2 : (((clearRef schema T id r₂).fields).get f).keyed = true := by
    rw [← heq]; exact hk1
  obtain ⟨e2, k2⟩ := clearRef_keyed (schema := schema) (T := T) (id := id) hn₂ f hk2
  simp only [Bool.and_eq_true, beq_iff_eq]
  refine ⟨k1, ?_⟩
  rw [← e1, ← e2, heq]

theorem clearFields_nodup (schema : Schema) (T : TypeId) (id : RecordId) (fs : Fields)
    (hnd : (fs.map Prod.fst).Nodup) : ((clearFields schema T id fs).map Prod.fst).Nodup := by
  unfold clearFields
  refine nodup_filterMap_fst
    (fun fv => if fieldRefers schema T id fv = true then (fv.2.dropRef id).map (fun v => (fv.1, v))
      else some fv) ?_ fs hnd
  intro x y hxy
  by_cases hr : fieldRefers schema T id x = true
  · rw [if_pos hr] at hxy
    cases hd : x.2.dropRef id with
    | none => rw [hd, Option.map_none] at hxy; cases hxy
    | some v => rw [hd, Option.map_some, Option.some.injEq] at hxy; rw [← hxy]
  · rw [if_neg hr, Option.some.injEq] at hxy
    rw [← hxy]

theorem clearRef_nodup (schema : Schema) (T : TypeId) (id : RecordId) (r : Record)
    (hnd : (r.fields.map Prod.fst).Nodup) : ((clearRef schema T id r).fields.map Prod.fst).Nodup := by
  unfold clearRef
  split
  · exact clearFields_nodup schema T id r.fields hnd
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

/-- A record replaced by id: the replacement, or a record the replacement does not touch. -/
theorem mem_replace {env : Env} {rs : Records} {id : RecordId} {r : Record} {x : RecordId} {z : Record}
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

/-! ## Lemmas: the tables of a tenant -/

/-- What a table that passes the schema check satisfies: every record conforms to the schema, and
    no two distinct records agree on a unique field or on a composite constraint. -/
theorem tableViolated_false {L : Limits} {o : ObjType} (h : tableViolated L o = false) :
    (∀ q ∈ o.records, conforms o.schema q.2 = true) ∧
    (∀ fd ∈ o.schema, fd.2.unique = true → ∀ q₁ ∈ o.records, ∀ q₂ ∈ o.records, q₁.1 ≠ q₂.1 →
        agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
    (∀ u ∈ o.uniques, ∀ q₁ ∈ o.records, ∀ q₂ ∈ o.records, q₁.1 ≠ q₂.1 →
        agreeOn u q₁.2.fields q₂.2.fields = false) := by
  unfold tableViolated at h
  rw [Bool.or_eq_false_iff, Bool.or_eq_false_iff, Bool.or_eq_false_iff] at h
  obtain ⟨⟨⟨hA, hB⟩, hC⟩, hD⟩ := h
  refine ⟨?_, ?_, ?_⟩
  · intro q hq
    have hA' : (o.records.all (fun q => conforms o.schema q.2)) = true := by
      simpa using hA
    exact List.all_eq_true.mp hA' q hq
  · intro fd hfd hu q₁ hq₁ q₂ hq₂ hne
    have hB' := List.any_eq_false.mp hB fd hfd
    have hdup : dupFields o.records [fd.1] = false := by
      cases hdf : dupFields o.records [fd.1]
      · rfl
      · exfalso; apply hB'; simp [hu, hdf]
    exact dupFields_false hdup hq₁ hq₂ hne
  · intro u hu q₁ hq₁ q₂ hq₂ hne
    have hC' := List.any_eq_false.mp hC u hu
    have hdup : dupFields o.records u = false := by simpa using hC'
    exact dupFields_false hdup hq₁ hq₂ hne

/-- A schema that passes the check leaves every object type's table passing it. -/
theorem tableViolated_of_schemaViolated {L : Limits} {types : List (TypeId × ObjType)}
    (h : schemaViolated L types = false) {T : TypeId} {o : ObjType} (ho : (T, o) ∈ types) :
    tableViolated L o = false :=
  by simpa using List.any_eq_false.mp h (T, o) ho

theorem keys_putTypeList (l : List (TypeId × ObjType)) (ty : TypeId) (o' : ObjType) :
    (putTypeList l ty o').map Prod.fst = l.map Prod.fst := by
  induction l with
  | nil => rfl
  | cons e es ih =>
    obtain ⟨k, p⟩ := e
    unfold putTypeList
    split
    · rename_i h
      have hk : k = ty := by simpa using h
      subst hk
      rfl
    · simp [ih]

theorem types_keys_putType (t : Tenant) (ty : TypeId) (o' : ObjType) :
    (t.putType ty o').types.map Prod.fst = t.types.map Prod.fst :=
  keys_putTypeList t.types ty o'

theorem types_keys_afterDelete (t : Tenant) (ty id : TypeId) :
    (t.afterDelete ty id).types.map Prod.fst = t.types.map Prod.fst := by
  unfold Tenant.afterDelete
  rw [List.map_map]
  rfl

/-! ## Lemmas: a schema change that is accepted leaves the tables conforming -/

/-- An accepted schema change writes the entry of one object type, and that type is one it touches. -/
theorem schema_step_putType (t t' : Tenant) (p : Principal) (i : Intent)
    (hi : i.isSchema = true) (h : stepTenant t p i = .ok t') :
    ∃ ty o', t' = t.putType ty o' ∧ ty ∈ i.touched t := by
  cases i with
  | createTenant owner name => simp [Intent.isSchema] at hi
  | member j => simp [Intent.isSchema] at hi
  | writeSetting _ _ _ _ _ => simp [Intent.isSchema] at hi
  | postLedger _ _ => simp [Intent.isSchema] at hi
  | expireLedger _ _ _ => simp [Intent.isSchema] at hi
  | create ty fs => simp [Intent.isSchema] at hi
  | update ty id fs ev => simp [Intent.isSchema] at hi
  | delete ty id ev => simp [Intent.isSchema] at hi
  | changeSchema ev strict cs => simp [Intent.isSchema] at hi
  | setRequired ty f strict =>
    simp only [stepTenant] at h
    repeat' split at h
    all_goals (try cases h)
    all_goals exact ⟨_, _, rfl, by simp [Intent.touched]⟩
  | setUnique ty f strict =>
    simp only [stepTenant] at h
    repeat' split at h
    all_goals (try cases h)
    all_goals exact ⟨_, _, rfl, by simp [Intent.touched]⟩
  | defineUnique ty fs =>
    simp only [stepTenant] at h
    repeat' split at h
    all_goals (try cases h)
    all_goals exact ⟨_, _, rfl, by simp [Intent.touched]⟩

/-- A strict schema change that is accepted leaves every object type it touches (`Intent.touched`)
    passing the check, where the check is over those types alone. -/
theorem schemaViolated_of_strict (t t' : Tenant) (p : Principal) (i : Intent)
    (hi : i.isSchema = true) (hs : i.strict = true) (h : stepTenant t p i = .ok t') :
    schemaViolated t.limits (scopedTypes t'.types (i.touched t)) = false := by
  cases i with
  | createTenant owner name => simp [Intent.isSchema] at hi
  | member j => simp [Intent.isSchema] at hi
  | writeSetting _ _ _ _ _ => simp [Intent.isSchema] at hi
  | postLedger _ _ => simp [Intent.isSchema] at hi
  | expireLedger _ _ _ => simp [Intent.isSchema] at hi
  | create ty fs => simp [Intent.isSchema] at hi
  | update ty id fs ev => simp [Intent.isSchema] at hi
  | delete ty id ev => simp [Intent.isSchema] at hi
  | changeSchema ev strict cs => simp [Intent.isSchema] at hi
  | setRequired ty f strict =>
    simp only [Intent.strict] at hs
    subst hs
    simp only [stepTenant] at h
    split at h
    · cases h
    · cases hT : t.typ? ty with
      | none => simp [hT] at h
      | some o =>
        simp only [hT] at h
        split at h
        · cases h
        · rename_i hc
          have ht := Except.ok.inj h
          subst ht
          simpa [schemaChangeRefused, Intent.touched, hT] using hc
  | setUnique ty f strict =>
    simp only [Intent.strict] at hs
    subst hs
    simp only [stepTenant] at h
    split at h
    · cases h
    · cases hT : t.typ? ty with
      | none => simp [hT] at h
      | some o =>
        simp only [hT] at h
        split at h
        · cases h
        · split at h
          · cases h
          · split at h
            · cases h
            · rename_i hc
              have ht := Except.ok.inj h
              subst ht
              simpa [schemaChangeRefused, Intent.touched, hT] using hc
  | defineUnique ty fs =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · cases hT : t.typ? ty with
      | none => simp [hT] at h
      | some o =>
        simp only [hT] at h
        split at h
        · cases h
        · split at h
          · cases h
          · split at h
            · cases h
            · rename_i hc
              have ht := Except.ok.inj h
              subst ht
              simpa [schemaChangeRefused, Intent.touched] using hc

/-- An accepted strict schema change, against the records as they were: each object type it touches
    (`Intent.touched`) holds its records conforming and its constraints after it, and every other
    type's records are as they were. The check is over the touched types only, so a grandfathered
    record of an untouched type does not refuse the change. -/
theorem strict_schema_change_scoped (t t' : Tenant) (p : Principal) (i : Intent)
    (hi : i.isSchema = true) (hs : i.strict = true) (h : stepTenant t p i = .ok t') :
    (∀ T o, t'.typ? T = some o → T ∈ i.touched t →
      (∀ q ∈ o.records, conforms o.schema q.2 = true) ∧
      (∀ fd ∈ o.schema, fd.2.unique = true → ∀ q₁ ∈ o.records, ∀ q₂ ∈ o.records, q₁.1 ≠ q₂.1 →
        agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
      (∀ u ∈ o.uniques, ∀ q₁ ∈ o.records, ∀ q₂ ∈ o.records, q₁.1 ≠ q₂.1 →
        agreeOn u q₁.2.fields q₂.2.fields = false)) ∧
    (∀ T, T ∉ i.touched t → t'.typ? T = t.typ? T) := by
  have hv := schemaViolated_of_strict t t' p i hi hs h
  obtain ⟨ty, o', rfl, hty⟩ := schema_step_putType t _ p i hi h
  have touchedKey : ∀ T o, (t.putType ty o').typ? T = some o → T ∈ i.touched t →
      (∀ q ∈ o.records, conforms o.schema q.2 = true) ∧
      (∀ fd ∈ o.schema, fd.2.unique = true → ∀ q₁ ∈ o.records, ∀ q₂ ∈ o.records, q₁.1 ≠ q₂.1 →
        agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
      (∀ u ∈ o.uniques, ∀ q₁ ∈ o.records, ∀ q₂ ∈ o.records, q₁.1 ≠ q₂.1 →
        agreeOn u q₁.2.fields q₂.2.fields = false) := by
    intro T o ho hT
    have hmem : (T, o) ∈ (t.putType ty o').types := lookup_mem ho
    have hsc : (T, o) ∈ scopedTypes (t.putType ty o').types (i.touched t) :=
      List.mem_filter.mpr ⟨hmem, by simpa using hT⟩
    exact tableViolated_false (L := t.limits) (tableViolated_of_schemaViolated hv hsc)
  have untouchedNe : ∀ T, T ∉ i.touched t → T ≠ ty := fun T hT e => hT (by subst e; exact hty)
  exact ⟨fun T o ho hT => touchedKey T o ho hT,
    fun T hT => typ?_putType_other t ty T o' (untouchedNe T hT)⟩

/-- An accepted strict schema change on a tenant whose records conform: the records of each touched
    object type conform after it, the records of every other type are as they were (see
    `strict_schema_change_scoped`), and the tenant conforms. -/
theorem strict_schema_change_conforms (t t' : Tenant) (p : Principal) (i : Intent)
    (hi : i.isSchema = true) (hs : i.strict = true) (h : stepTenant t p i = .ok t')
    (hconf : t.Conform) :
    (∀ T o, t'.typ? T = some o → T ∈ i.touched t →
      (∀ q ∈ o.records, conforms o.schema q.2 = true) ∧
      (∀ fd ∈ o.schema, fd.2.unique = true → ∀ q₁ ∈ o.records, ∀ q₂ ∈ o.records, q₁.1 ≠ q₂.1 →
        agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
      (∀ u ∈ o.uniques, ∀ q₁ ∈ o.records, ∀ q₂ ∈ o.records, q₁.1 ≠ q₂.1 →
        agreeOn u q₁.2.fields q₂.2.fields = false)) ∧
    (∀ T, T ∉ i.touched t → t'.typ? T = t.typ? T) ∧
    t'.Conform := by
  obtain ⟨hT, hU⟩ := strict_schema_change_scoped t t' p i hi hs h
  refine ⟨hT, hU, fun T o ho q hq => ?_⟩
  by_cases hTouched : T ∈ i.touched t
  · exact (hT T o ho hTouched).1 q hq
  · rw [hU T hTouched] at ho
    exact hconf T o ho q hq

/-- An accepted strict schema change leaves the unique constraints of every table holding, on a
    tenant whose constraints hold: the touched types are checked, the others are as they were. -/
theorem strict_schema_change_unique (t t' : Tenant) (p : Principal) (i : Intent)
    (hi : i.isSchema = true) (hs : i.strict = true) (h : stepTenant t p i = .ok t')
    (hu : t.UniqueRecords) : t'.UniqueRecords := by
  obtain ⟨hT, hU⟩ := strict_schema_change_scoped t t' p i hi hs h
  intro T o ho q₁ hq₁ q₂ hq₂ hne
  by_cases hTouched : T ∈ i.touched t
  · have k := hT T o ho hTouched
    exact ⟨fun fd hfd hu' => k.2.1 fd hfd hu' q₁ hq₁ q₂ hq₂ hne,
      fun u hu' => k.2.2 u hu' q₁ hq₁ q₂ hq₂ hne⟩
  · rw [hU T hTouched] at ho
    exact hu T o ho q₁ hq₁ q₂ hq₂ hne

/-! ## The invariant through accepted steps -/

/-- The names an accepted step leaves distinct: the object types, each schema, and each record's
    fields. -/
theorem stepTenant_preserves_distinct (t t' : Tenant) (p : Principal) (i : Intent)
    (hd : t.Distinct) (h : stepTenant t p i = .ok t') : t'.Distinct := by
  obtain ⟨hkeys, hD⟩ := hd
  cases i with
  | changeSchema ev strict cs => exact changeSchema_distinct ⟨hkeys, hD⟩ h
  | createTenant owner name => simp [stepTenant] at h
  | member j => obtain ⟨m', rfl⟩ := stepTenant_member_eq h; exact ⟨hkeys, hD⟩
  | writeSetting n lvl team v ev => obtain ⟨ov, rv, rfl⟩ := writeSettingT_ok_shape h; exact ⟨hkeys, hD⟩
  | postLedger now ps => obtain ⟨L, rfl⟩ := postLedgerT_ok_shape h; exact ⟨hkeys, hD⟩
  | expireLedger now u c => obtain ⟨L, rfl⟩ := expireLedgerT_ok_shape h; exact ⟨hkeys, hD⟩
  | create ty fs =>
    cases hty : t.typ? ty with
    | none => simp [stepTenant, hty] at h
    | some o =>
      obtain ⟨r, hN, hP, -, ht⟩ := create_ok_parts hty h
      subst ht
      refine ⟨by rw [types_keys_putType]; exact hkeys, fun T o' hT' => ?_⟩
      change (t.putType ty { o with records := (t.nextId, r) :: o.records }).typ? T = some o' at hT'
      by_cases hTy : T = ty
      · subst hTy
        rw [typ?_putType_self t T o _ hty] at hT'
        cases hT'
        obtain ⟨hs, hr⟩ := hD T o hty
        refine ⟨hs, fun q hq => ?_⟩
        rcases List.mem_cons.mp hq with rfl | hq'
        · exact newRecord_nodup o.schema p fs r hN hs
        · exact hr q hq'
      · rw [typ?_putType_other t ty T _ hTy] at hT'
        exact hD T o' hT'
  | update ty id fs ev =>
    cases hty : t.typ? ty with
    | none => simp [stepTenant, hty] at h
    | some o =>
      cases hL : o.records.lookup id with
      | none => simp [stepTenant, hty, hL, missingRecord] at h
      | some old =>
        obtain ⟨r, hF, -, -, -, -, ht⟩ := update_ok_parts_gen hty hL h
        subst ht
        refine ⟨by rw [types_keys_putType]; exact hkeys, fun T o' hT' => ?_⟩
        by_cases hTy : T = ty
        · subst hTy
          rw [typ?_putType_self t T o _ hty] at hT'
          cases hT'
          obtain ⟨hs, hr⟩ := hD T o hty
          refine ⟨hs, fun q hq => ?_⟩
          obtain ⟨q₀, hq₀, hmap⟩ := List.mem_map.mp hq
          by_cases hid : (q₀.1 == id) = true
          · simp only [hid, if_true] at hmap
            rw [← hmap]
            have hold : (old.fields.map Prod.fst).Nodup := by
              have hm : (id, old) ∈ o.records := lookup_mem hL
              exact hr (id, old) hm
            exact fireAll_nodup (patch_nodup fs old.fields hold) hF
          · simp only [hid, if_false] at hmap
            rw [← hmap]
            exact hr q₀ hq₀
        · rw [typ?_putType_other t ty T _ hTy] at hT'
          exact hD T o' hT'
  | delete ty id ev =>
    have ht := stepTenant_delete_eq h
    subst ht
    refine ⟨by rw [types_keys_afterDelete]; exact hkeys, fun T o' hT' => ?_⟩
    rw [typ?_afterDelete] at hT'
    cases ho : t.typ? T with
    | none => rw [ho] at hT'; cases hT'
    | some o =>
      rw [ho] at hT'
      simp only [Option.map_some, Option.some.injEq] at hT'
      subst hT'
      obtain ⟨hs, hr⟩ := hD T o ho
      refine ⟨hs, fun q hq => ?_⟩
      obtain ⟨q₀, hq₀, hmap⟩ := List.mem_map.mp hq
      subst hmap
      have hq₀' : q₀ ∈ o.records := by
        by_cases hTy : (T == ty) = true
        · rw [if_pos hTy] at hq₀
          exact (List.mem_filter.mp hq₀).1
        · rw [if_neg hTy] at hq₀
          exact hq₀
      exact clearRef_nodup o.schema ty id q₀.2 (hr q₀ hq₀')
  | setRequired ty f strict =>
    cases hty : t.typ? ty with
    | none => simp only [stepTenant, hty] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hty] at h
      repeat' split at h
      all_goals try cases h
      refine ⟨by rw [types_keys_putType]; exact hkeys, fun T o' hT' => ?_⟩
      by_cases hTy : T = ty
      · subst hTy
        rw [typ?_putType_self t T o _ hty] at hT'
        cases hT'
        obtain ⟨hs, hr⟩ := hD T o hty
        exact ⟨by rw [keys_makeRequired]; exact keys_ensureField _ _ hs, hr⟩
      · rw [typ?_putType_other t ty T _ hTy] at hT'
        exact hD T o' hT'
  | setUnique ty f strict =>
    cases hty : t.typ? ty with
    | none => simp only [stepTenant, hty] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hty] at h
      repeat' split at h
      all_goals try cases h
      refine ⟨by rw [types_keys_putType]; exact hkeys, fun T o' hT' => ?_⟩
      by_cases hTy : T = ty
      · subst hTy
        rw [typ?_putType_self t T o _ hty] at hT'
        cases hT'
        obtain ⟨hs, hr⟩ := hD T o hty
        exact ⟨by rw [keys_makeUnique]; exact keys_ensureField _ _ hs, hr⟩
      · rw [typ?_putType_other t ty T _ hTy] at hT'
        exact hD T o' hT'
  | defineUnique ty fs =>
    cases hty : t.typ? ty with
    | none => simp only [stepTenant, hty] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hty] at h
      repeat' split at h
      all_goals try cases h
      refine ⟨by rw [types_keys_putType]; exact hkeys, fun T o' hT' => ?_⟩
      by_cases hTy : T = ty
      · subst hTy
        rw [typ?_putType_self t T o _ hty] at hT'
        cases hT'
        obtain ⟨hs, hr⟩ := hD T o hty
        exact ⟨hs, hr⟩
      · rw [typ?_putType_other t ty T _ hTy] at hT'
        exact hD T o' hT'



/-- The records of one object type with the unique constraints of the type hold: no two distinct
    records agree on a unique field, or on a composite constraint. -/
def UniqOn (schema : Schema) (us : List (List FieldId)) (rs : Records) : Prop :=
  ∀ q₁ ∈ rs, ∀ q₂ ∈ rs, q₁.1 ≠ q₂.1 →
    (∀ fd ∈ schema, fd.2.unique = true → agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
    (∀ u ∈ us, agreeOn u q₁.2.fields q₂.2.fields = false)

/-- A fresh record that the unique checks pass joins records that hold the constraints. -/
theorem uniqOn_cons_fresh {L : Limits} {env : Env} {schema : Schema} {us : List (List FieldId)}
    {rs : Records} {self : RecordId} {r : Record}
    (hU : UniqOn schema us rs) (hp : problem L schema us true env rs self r = none)
    (hids : ∀ q ∈ rs, q.1 ≠ self) : UniqOn schema us ((self, r) :: rs) := by
  intro q₁ hq₁ q₂ hq₂ hne
  have hnew := problem_unique_none hp
  rcases List.mem_cons.mp hq₁ with rfl | hq₁' <;> rcases List.mem_cons.mp hq₂ with rfl | hq₂'
  · exact absurd rfl hne
  · exact ⟨fun fd hfd hu => hnew.1 fd hfd hu q₂ hq₂' (hids q₂ hq₂'),
      fun u hu => hnew.2 u hu q₂ hq₂' (hids q₂ hq₂')⟩
  · refine ⟨fun fd hfd hu => ?_, fun u hu => ?_⟩
    · rw [agreeOn_comm]; exact hnew.1 fd hfd hu q₁ hq₁' (hids q₁ hq₁')
    · rw [agreeOn_comm]; exact hnew.2 u hu q₁ hq₁' (hids q₁ hq₁')
  · exact hU q₁ hq₁' q₂ hq₂' hne

/-- A record replaced by a write that the unique checks pass keeps the constraints. -/
theorem uniqOn_replace {L : Limits} {env : Env} {schema : Schema} {us : List (List FieldId)}
    {rs : Records} {id : RecordId} {r : Record}
    (hU : UniqOn schema us rs) (hp : problem L schema us true env rs id r = none) :
    UniqOn schema us (rs.map (fun q => if q.1 == id then (q.1, r) else q)) := by
  intro q₁ hq₁ q₂ hq₂ hne
  obtain ⟨a₁, ha₁, rfl⟩ := List.mem_map.mp hq₁
  obtain ⟨a₂, ha₂, rfl⟩ := List.mem_map.mp hq₂
  have hnew := problem_unique_none hp
  by_cases h1 : (a₁.1 == id) = true <;> by_cases h2 : (a₂.1 == id) = true
  · have e1 : a₁.1 = id := by simpa using h1
    have e2 : a₂.1 = id := by simpa using h2
    simp only [h1, h2, if_true] at hne
    exact absurd (e1.trans e2.symm) hne
  · have hn2 : a₂.1 ≠ id := by simpa using h2
    simp only [h1, h2, if_true, Bool.false_eq_true, if_false] at hne ⊢
    refine ⟨fun fd hfd hu => ?_, fun u hu => ?_⟩
    · exact hnew.1 fd hfd hu a₂ ha₂ hn2
    · exact hnew.2 u hu a₂ ha₂ hn2
  · have hn1 : a₁.1 ≠ id := by simpa using h1
    simp only [h1, h2, if_true, Bool.false_eq_true, if_false] at hne ⊢
    refine ⟨fun fd hfd hu => ?_, fun u hu => ?_⟩
    · rw [agreeOn_comm]; exact hnew.1 fd hfd hu a₁ ha₁ hn1
    · rw [agreeOn_comm]; exact hnew.2 u hu a₁ ha₁ hn1
  · have hn1 : a₁.1 ≠ id := by simpa using h1
    have hn2 : a₂.1 ≠ id := by simpa using h2
    simp only [h1, h2, Bool.false_eq_true, if_false] at hne ⊢
    exact hU a₁ ha₁ a₂ ha₂ hne


/-- Records that one delete leaves, with the references to the deleted record cleared, keep the
    constraints: two of them agreeing after the clearing agreed before it. -/
theorem uniqOn_delClear (schema : Schema) (us : List (List FieldId)) (ty : TypeId) (id : RecordId)
    {rs rs' : Records} (hU : UniqOn schema us rs) (hsub : ∀ q ∈ rs', q ∈ rs)
    (hnd : ∀ q ∈ rs, (q.2.fields.map Prod.fst).Nodup) :
    UniqOn schema us (delClear schema ty id rs') := by
  intro q₁ hq₁ q₂ hq₂ hne
  obtain ⟨a₁, ha₁, rfl⟩ := List.mem_map.mp hq₁
  obtain ⟨a₂, ha₂, rfl⟩ := List.mem_map.mp hq₂
  have hm₁ := hsub a₁ ha₁
  have hm₂ := hsub a₂ ha₂
  have hne' : a₁.1 ≠ a₂.1 := hne
  obtain ⟨hA, hB⟩ := hU a₁ hm₁ a₂ hm₂ hne'
  refine ⟨fun fd hfd hu => ?_, fun u hu => ?_⟩
  · show agreeOn [fd.1] (clearRef schema ty id a₁.2).fields (clearRef schema ty id a₂.2).fields = false
    cases hag : agreeOn [fd.1] (clearRef schema ty id a₁.2).fields (clearRef schema ty id a₂.2).fields
    · rfl
    · exfalso
      have := agreeOn_clearRef (hnd a₁ hm₁) (hnd a₂ hm₂) [fd.1] hag
      rw [hA fd hfd hu] at this
      cases this
  · show agreeOn u (clearRef schema ty id a₁.2).fields (clearRef schema ty id a₂.2).fields = false
    cases hag : agreeOn u (clearRef schema ty id a₁.2).fields (clearRef schema ty id a₂.2).fields
    · rfl
    · exfalso
      have := agreeOn_clearRef (hnd a₁ hm₁) (hnd a₂ hm₂) u hag
      rw [hB u hu] at this
      cases this

/-! ## The uniqueness invariant through accepted steps -/

/-- An accepted strict step keeps every object type's records unique under its constraints. -/
theorem stepTenant_preserves_unique (t t' : Tenant) (p : Principal) (i : Intent)
    (hwf : t.WF) (hstrict : i.strict = true) (h : stepTenant t p i = .ok t') :
    t'.UniqueRecords := by
  obtain ⟨-, hids, -, hd, hu⟩ := hwf
  cases i with
  | changeSchema ev strict cs =>
    have hs : strict = true := by simpa [Intent.strict] using hstrict
    subst hs
    obtain ⟨hT, hU⟩ := changeSchema_table h
    intro T o2 hT2 q₁ hq₁ q₂ hq₂ hne
    by_cases hTt : T ∈ cs.flatMap Change.touched
    · have hk := tableViolated_false (hT T o2 hT2 hTt)
      exact ⟨fun fd hfd hu' => hk.2.1 fd hfd hu' q₁ hq₁ q₂ hq₂ hne,
        fun u hu' => hk.2.2 u hu' q₁ hq₁ q₂ hq₂ hne⟩
    · rcases hU T o2 hT2 hTt with h0 | he
      · exact hu T o2 h0 q₁ hq₁ q₂ hq₂ hne
      · rw [he] at hq₁; cases hq₁
  | createTenant owner name => simp [stepTenant] at h
  | member j => obtain ⟨m', rfl⟩ := stepTenant_member_eq h; exact hu
  | writeSetting n lvl team v ev => obtain ⟨ov, rv, rfl⟩ := writeSettingT_ok_shape h; exact hu
  | postLedger now ps => obtain ⟨L, rfl⟩ := postLedgerT_ok_shape h; exact hu
  | expireLedger now u c => obtain ⟨L, rfl⟩ := expireLedgerT_ok_shape h; exact hu
  | create ty fs =>
    cases hty : t.typ? ty with
    | none => simp [stepTenant, hty] at h
    | some o =>
      obtain ⟨r, -, hP, -, ht⟩ := create_ok_parts hty h
      subst ht
      intro T o' hT' q₁ hq₁ q₂ hq₂ hne
      by_cases hTy : T = ty
      · subst hTy
        change (t.putType T { o with records := (t.nextId, r) :: o.records }).typ? T = some o' at hT'
        rw [typ?_putType_self t T o _ hty] at hT'
        cases hT'
        exact (uniqOn_cons_fresh (L := t.limits) (env := t.env) (hu T o hty) hP
          (fun q hq => Nat.ne_of_lt (hids T o hty q hq))) q₁ hq₁ q₂ hq₂ hne
      · change (t.putType ty { o with records := (t.nextId, r) :: o.records }).typ? T = some o' at hT'
        rw [typ?_putType_other t ty T _ hTy] at hT'
        exact hu T o' hT' q₁ hq₁ q₂ hq₂ hne
  | update ty id fs ev =>
    cases hty : t.typ? ty with
    | none => simp [stepTenant, hty] at h
    | some o =>
      cases hL : o.records.lookup id with
      | none => simp [stepTenant, hty, hL, missingRecord] at h
      | some old =>
        obtain ⟨r, -, -, hP, -, -, ht⟩ := update_ok_parts_gen hty hL h
        subst ht
        intro T o' hT' q₁ hq₁ q₂ hq₂ hne
        by_cases hTy : T = ty
        · subst hTy
          rw [typ?_putType_self t T o _ hty] at hT'
          cases hT'
          exact (uniqOn_replace (L := t.limits) (env := t.env) (hu T o hty) hP) q₁ hq₁ q₂ hq₂ hne
        · rw [typ?_putType_other t ty T _ hTy] at hT'
          exact hu T o' hT' q₁ hq₁ q₂ hq₂ hne
  | delete ty id ev =>
    have ht := stepTenant_delete_eq h
    subst ht
    intro T o' hT' q₁ hq₁ q₂ hq₂ hne
    rw [typ?_afterDelete] at hT'
    cases ho : t.typ? T with
    | none => rw [ho] at hT'; cases hT'
    | some o =>
      rw [ho] at hT'
      simp only [Option.map_some, Option.some.injEq] at hT'
      subst hT'
      have hsub : ∀ q ∈ (if (T == ty) = true then o.records.filter (fun q => q.1 != id)
          else o.records), q ∈ o.records := by
        intro q hq
        by_cases hTy : (T == ty) = true
        · rw [if_pos hTy] at hq; exact (List.mem_filter.mp hq).1
        · rw [if_neg hTy] at hq; exact hq
      exact uniqOn_delClear o.schema o.uniques ty id (hu T o ho) hsub
        (fun q hq => (hd.2 T o ho).2 q hq) q₁ hq₁ q₂ hq₂ hne
  | setRequired ty f strict =>
    exact strict_schema_change_unique t t' p _ rfl hstrict h hu
  | setUnique ty f strict =>
    exact strict_schema_change_unique t t' p _ rfl hstrict h hu
  | defineUnique ty fs =>
    exact strict_schema_change_unique t t' p _ rfl hstrict h hu

/-! ## Duplicates refused -/

theorem create_duplicate_refused (w : World) (t : Tenant) (c : Ctx) (ty : TypeId) (o : ObjType)
    (fs : Fields) (ht : w c.tenant = some t) (ho : t.typ? ty = some o)
    (r : Record) (hc : canCreate t c.principal ty r = true)
    (hN : newRecord o.schema c.principal fs = .ok r) (hw : initialsOk o.workflows r = true)
    (hfree : problem t.limits o.schema o.uniques false t.env o.records t.nextId
      r = none)
    (hclash : (∃ fd ∈ o.schema, fd.2.unique = true ∧ ∃ q ∈ o.records, q.1 ≠ t.nextId ∧
        agreeOn [fd.1] r.fields q.2.fields = true) ∨
      (∃ u ∈ o.uniques, ∃ q ∈ o.records, q.1 ≠ t.nextId ∧
        agreeOn u r.fields q.2.fields = true))
    (hr : callerReserved c.principal = false) :
    step w (.create ty fs) c = .error .uniqueViolated ∧
      ∀ w', step w (.create ty fs) c ≠ .ok w' := by
  have hs : step w (.create ty fs) c = .error .uniqueViolated := by
    simp [step, reservedRefused, Intent.named, hr, ht, stepTenant, ho, hN, hc, hw, canCreate_attempt _ _ _ _ hc, problem_uniq_refusal hfree hclash]
  exact ⟨hs, fun w' h => by rw [hs] at h; cases h⟩

theorem update_duplicate_refused (w : World) (t : Tenant) (c : Ctx) (ty : TypeId) (o : ObjType)
    (id : RecordId) (fs : Fields) (old : Record) (ev : Option Nat)
    (ht : w c.tenant = some t) (ho : t.typ? ty = some o) (hl : o.records.lookup id = some old)
    (hcan : canUpdate t c.principal ty old (patched old fs) = true) (hv : versionClash ev old = false)
    (hwf : fireAll c.principal old (patched old fs) o.workflows = .ok (patched old fs))
    (hst : statesOk o.workflows (patched old fs) = true)
    (hfree : problem t.limits o.schema o.uniques false t.env o.records id (patched old fs) = none)
    (hclash : (∃ fd ∈ o.schema, fd.2.unique = true ∧ ∃ q ∈ o.records, q.1 ≠ id ∧
        agreeOn [fd.1] (patched old fs).fields q.2.fields = true) ∨
      (∃ u ∈ o.uniques, ∃ q ∈ o.records, q.1 ≠ id ∧
        agreeOn u (patched old fs).fields q.2.fields = true))
    (hr : callerReserved c.principal = false) :
    step w (.update ty id fs ev) c = .error .uniqueViolated ∧
      ∀ w', step w (.update ty id fs ev) c ≠ .ok w' := by
  have hs : step w (.update ty id fs ev) c = .error .uniqueViolated := by
    simp [step, reservedRefused, Intent.named, hr, ht, stepTenant, ho, hl, hcan, hv, hwf, hst, sameFields, problem_uniq_refusal hfree hclash]
  exact ⟨hs, fun w' h => by rw [hs] at h; cases h⟩

theorem define_unique_refused_over_duplicates (w : World) (t : Tenant) (c : Ctx) (ty : TypeId)
    (o : ObjType) (fs : List FieldId) (ht : w c.tenant = some t) (ho : t.typ? ty = some o)
    (hp : canManage t c.principal = true) (hok : uniqueFieldsOk o.schema fs = true)
    (hdup : dupFields o.records fs = true)
    (hr : callerReserved c.principal = false) :
    step w (.defineUnique ty fs) c = .error .uniqueViolated ∧
      ∀ w', step w (.defineUnique ty fs) c ≠ .ok w' := by
  have hs : step w (.defineUnique ty fs) c = .error .uniqueViolated := by
    simp [step, reservedRefused, Intent.named, hr, ht, stepTenant, ho, hp, hok, hdup]
  exact ⟨hs, fun w' h => by rw [hs] at h; cases h⟩


theorem set_unique_refused_over_duplicates (w : World) (t : Tenant) (c : Ctx) (ty : TypeId)
    (o : ObjType) (f : FieldId) (d : FieldDef) (ht : w c.tenant = some t)
    (ho : t.typ? ty = some o) (hp : canManage t c.principal = true) (hd : o.schema.lookup f = some d)
    (hl : d.ty.isList = false) (hdup : dupFields o.records [f] = true)
    (hr : callerReserved c.principal = false) :
    step w (.setUnique ty f true) c = .error .migrationRejected ∧
      ∀ w', step w (.setUnique ty f true) c ≠ .ok w' := by
  have hens : ensureField o.schema f = o.schema := ensureField_eq_of_lookup _ _ d hd
  have hmem : (ty, { o with schema := makeUnique o.schema f }) ∈
      (t.putType ty { o with schema := makeUnique o.schema f }).types :=
    lookup_mem (typ?_putType_self t ty o _ ho)
  have hfd : (f, d) ∈ o.schema := lookup_mem hd
  have hfu : (f, { d with unique := true }) ∈ makeUnique o.schema f := by
    unfold makeUnique
    exact List.mem_map.mpr ⟨(f, d), hfd, by simp⟩
  have hviol : tableViolated t.limits { o with schema := makeUnique o.schema f } = true := by
    unfold tableViolated
    simp only [Bool.or_eq_true, List.any_eq_true]
    left; left; right
    exact ⟨(f, { d with unique := true }), hfu, by simp [hdup]⟩
  have hv : schemaChangeRefused t.limits
      (scopedTypes (t.putType ty { o with schema := makeUnique o.schema f }).types
        (ty :: fieldTargets o.schema f)) true = true := by
    unfold schemaChangeRefused schemaViolated
    simp only [Bool.true_and, List.any_eq_true]
    exact ⟨_, List.mem_filter.mpr ⟨hmem, by simp⟩, hviol⟩
  have hs : step w (.setUnique ty f true) c = .error .migrationRejected := by
    simp [step, reservedRefused, Intent.named, hr, ht, stepTenant, ho, hp, hd, hl, hv, hens]
  exact ⟨hs, fun w' h => by rw [hs] at h; cases h⟩

/-- A table of one integer field, with two records that agree on it. -/
def laxObj : ObjType :=
  { schema := [(1, { ty := .int, required := false })],
    records := [(0, { owner := some 0, version := 1, fields := [(1, .int 5)] }),
                (1, { owner := some 0, version := 1, fields := [(1, .int 5)] })] }

def laxUniqueTenant : Tenant :=
  { types := [(1, laxObj)], members := [(0, [ownerRole])], grants := [], policies := [], nextId := 2 }

theorem laxUniqueTenant_typ (T : TypeId) (o : ObjType) (h : laxUniqueTenant.typ? T = some o) :
    T = 1 ∧ o = laxObj := by
  unfold laxUniqueTenant Tenant.typ? at h
  by_cases hT : T = 1
  · subst hT
    simp [List.lookup, laxObj] at h
    exact ⟨rfl, h.symm⟩
  · have hb : (T == 1) = false := by simpa using hT
    simp [List.lookup, hb] at h

theorem laxUniqueTenant_wf : laxUniqueTenant.WF := by
  refine ⟨?_, ?_, ?_, ?_, ?_⟩
  · intro T o ho q hq
    obtain ⟨rfl, rfl⟩ := laxUniqueTenant_typ T o ho
    have hall : ∀ q ∈ laxObj.records, conforms laxObj.schema q.2 = true := by decide
    exact hall q hq
  · intro T o ho q hq
    obtain ⟨rfl, rfl⟩ := laxUniqueTenant_typ T o ho
    have hall : ∀ q ∈ laxObj.records, q.1 < laxUniqueTenant.nextId := by decide
    exact hall q hq
  · intro T o ho q hq
    obtain ⟨rfl, rfl⟩ := laxUniqueTenant_typ T o ho
    have hall : ∀ q ∈ laxObj.records, refsOk laxUniqueTenant.env laxObj.schema q.2 = true := by
      decide
    exact hall q hq
  · refine ⟨by decide, fun T o ho => ?_⟩
    obtain ⟨rfl, rfl⟩ := laxUniqueTenant_typ T o ho
    exact ⟨by decide, by decide⟩
  · intro T o ho q₁ hq₁ q₂ hq₂ hne
    obtain ⟨rfl, rfl⟩ := laxUniqueTenant_typ T o ho
    have hU : ∀ q₁ ∈ laxObj.records, ∀ q₂ ∈ laxObj.records, q₁.1 ≠ q₂.1 →
        (∀ fd ∈ laxObj.schema, fd.2.unique = true →
          agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
        (∀ u ∈ laxObj.uniques, agreeOn u q₁.2.fields q₂.2.fields = false) := by decide
    exact hU q₁ hq₁ q₂ hq₂ hne

/-- The exclusion of the non-strict field change is necessary for uniqueness, as it is for
    conformance: a lax `setUnique` over two records that agree on the field leaves a tenant that is
    not well formed. -/
theorem lax_unique_change_can_break_wf :
    ∃ (t t' : Tenant) (p : Principal),
      t.WF ∧ stepTenant t p (.setUnique 1 1 false) = .ok t' ∧ ¬ t'.UniqueRecords := by
  refine ⟨laxUniqueTenant, laxUniqueTenant.putType 1 { laxObj with
    schema := makeUnique laxObj.schema 1 }, .user 0, laxUniqueTenant_wf, rfl, ?_⟩
  intro hU
  have hT1 := typ?_putType_self laxUniqueTenant 1 laxObj
    { laxObj with schema := makeUnique laxObj.schema 1 } rfl
  have h1 := hU 1 _ hT1 (0, { owner := some 0, version := 1, fields := [(1, .int 5)] }) (by decide)
    (1, { owner := some 0, version := 1, fields := [(1, .int 5)] }) (by decide) (by decide)
  have h2 := h1.1 (1, { ty := .int, required := false, unique := true }) (by decide) rfl
  revert h2
  decide


/-! ## Restored slice-3 statements, typed (slice 4) -/

/-- Clearing and deleting keep an old record's name in the new records: each new record is the
    clearing of an old one, and so is named by it. -/
theorem mem_afterDelete {rs : Records} {id : RecordId} {x : RecordId} {z : Record}
    {g : Record → Record}
    (h : (x, z) ∈ (rs.filter (fun q => q.1 != id)).map (fun q => (q.1, g q.2))) :
    ∃ a ∈ rs, a.1 = x ∧ a.1 ≠ id ∧ z = g a.2 := by
  obtain ⟨a, ha, he⟩ := List.mem_map.mp h
  obtain ⟨hmem, hne⟩ := List.mem_filter.mp ha
  refine ⟨a, hmem, by simp at he; exact he.1 ▸ rfl, by simpa using hne, by simp at he; exact he.2.symm⟩

/-- The check of a schema change passes for every object type: every record of the type conforms
    to its schema, and no two distinct records agree on a unique field of it or on a composite
    constraint. -/
theorem schemaViolated_false {L : Limits} {types : List (TypeId × ObjType)}
    (h : schemaViolated L types = false) :
    ∀ e ∈ types, (∀ q ∈ e.2.records, conforms e.2.schema q.2 = true) ∧
      (∀ fd ∈ e.2.schema, fd.2.unique = true → ∀ q₁ ∈ e.2.records, ∀ q₂ ∈ e.2.records,
        q₁.1 ≠ q₂.1 → agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
      ∀ u ∈ e.2.uniques, ∀ q₁ ∈ e.2.records, ∀ q₂ ∈ e.2.records,
        q₁.1 ≠ q₂.1 → agreeOn u q₁.2.fields q₂.2.fields = false := by
  intro e he
  have hx : tableViolated L e.2 = false := by
    simpa [schemaViolated] using List.any_eq_false.mp h e he
  obtain ⟨h1, h2, h3⟩ := tableViolated_false hx
  exact ⟨h1, h2, h3⟩

/-- A field made required changes the counter and the records of no type. -/
theorem setRequired_spec (t t' : Tenant) (p : Principal) (ty : TypeId) (f : FieldId) (s : Bool)
    (h : stepTenant t p (.setRequired ty f s) = .ok t') :
    t'.nextId = t.nextId ∧ ∀ T, t'.env T = t.env T :=
  setRequired_records t t' p ty f s h

/-- A field made unique changes the counter, the records of no type, and the one type's schema
    and only that. -/
theorem setUnique_spec (t t' : Tenant) (p : Principal) (ty : TypeId) (f : FieldId) (s : Bool)
    (h : stepTenant t p (.setUnique ty f s) = .ok t') :
    t'.nextId = t.nextId ∧ (∀ T, t'.env T = t.env T) ∧
      ∀ o, t.typ? ty = some o →
        t'.typ? ty = some { o with schema := makeUnique (ensureField o.schema f) f } := by
  refine ⟨(setUnique_records t t' p ty f s h).1, (setUnique_records t t' p ty f s h).2, fun o hT => ?_⟩
  simp only [stepTenant, hT] at h
  repeat' split at h
  all_goals (try cases h)
  all_goals exact typ?_putType_self t ty o _ hT

/-- A composite constraint changes the counter, the records of no type, and the one type's
    constraints and only that. -/
theorem defineUnique_spec (t t' : Tenant) (p : Principal) (ty : TypeId) (fs : List FieldId)
    (h : stepTenant t p (.defineUnique ty fs) = .ok t') :
    t'.nextId = t.nextId ∧ (∀ T, t'.env T = t.env T) ∧
      ∀ o, t.typ? ty = some o → t'.typ? ty = some { o with uniques := o.uniques ++ [fs] } := by
  refine ⟨(defineUnique_records t t' p ty fs h).1, (defineUnique_records t t' p ty fs h).2,
    fun o hT => ?_⟩
  simp only [stepTenant, hT] at h
  repeat' split at h
  all_goals (try cases h)
  all_goals exact typ?_putType_self t ty o _ hT

theorem lookup_makeUnique_ty (s : Schema) (f k : FieldId) :
    ((makeUnique s f).lookup k).map FieldDef.ty = (s.lookup k).map FieldDef.ty := by
  unfold makeUnique
  exact lookup_map_ty s _ (fun e => by split <;> simp) k

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

end Strata
