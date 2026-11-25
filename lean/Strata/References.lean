/-
Slice 1 of the reference model: references between records.

What the slice claims, about the model in `Strata/Model.lean`:

- A write is accepted only if every record it names exists (`problem_none`), so referential
  integrity (`Tenant.RefsOk`) holds after every accepted step from a state where it held,
  strict or not (`stepTenant_preserves_refs`, `step_preserves_refs`).
- A delete of a record that another record names through a restricting or required field is
  refused with `stillReferenced` (`delete_refused_while_referenced`).
- An accepted delete leaves no record naming the deleted one, and the deleted one gone
  (`delete_leaves_no_reference`).

The first half of the file is lemmas about field lists and record lists.

No `sorry`, no added axioms. `Audit.lean` prints the axioms each theorem depends on.
-/

import Strata.Model

namespace Strata

/-! ## Lemmas: lookups in association lists -/

theorem lookup_cons_nat {β : Type} (a k : Nat) (v : β) (l : List (Nat × β)) :
    ((k, v) :: l).lookup a = if a = k then some v else l.lookup a := by
  by_cases h : a = k
  · subst h; simp [List.lookup]
  · have : (a == k) = false := by simpa using h
    simp [List.lookup, this, h]

theorem lookup_mem {β : Type} {l : List (Nat × β)} {a : Nat} {b : β}
    (h : l.lookup a = some b) : (a, b) ∈ l := by
  induction l with
  | nil => simp [List.lookup] at h
  | cons x xs ih =>
    obtain ⟨k, v⟩ := x
    rw [lookup_cons_nat] at h
    by_cases hk : a = k
    · subst hk; simp at h; subst h; simp
    · simp [hk] at h
      exact List.mem_cons_of_mem _ (ih h)

/-- A record with id `x` exists exactly when some entry has that id. -/
theorem has_iff {rs : Records} {x : RecordId} : rs.has x = true ↔ ∃ r, (x, r) ∈ rs := by
  induction rs with
  | nil => simp [Records.has, List.lookup]
  | cons q rs ih =>
    obtain ⟨k, v⟩ := q
    unfold Records.has at *
    rw [lookup_cons_nat]
    by_cases hk : x = k
    · subst hk; simp
    · simp [hk, ih]

theorem get_eq_head (fs : Fields) (f : FieldId) : fs.get f = (fs.valuesOf f).headD .null := by
  induction fs with
  | nil => simp [Fields.get, Fields.valuesOf, List.lookup]
  | cons q fs ih =>
    obtain ⟨k, v⟩ := q
    unfold Fields.get Fields.valuesOf at *
    rw [lookup_cons_nat]
    by_cases hk : f = k
    · subst hk; simp
    · have hk' : ¬ k = f := fun h => hk h.symm
      simp [hk, hk', ih]

theorem get_mem {fs : Fields} {f : FieldId} {v : Val} (h : fs.get f = v) (hv : v ≠ .null) :
    (f, v) ∈ fs := by
  unfold Fields.get at h
  cases hl : fs.lookup f with
  | none => rw [hl] at h; exact absurd h.symm hv
  | some u => rw [hl] at h; simp at h; subst h; exact lookup_mem hl

/-- Removing the entries whose value is `w` does not change a field whose value is not `w`. -/
theorem get_filter_ne (fs : Fields) (f : FieldId) (w : Val) (h : fs.get f ≠ w) :
    Fields.get (fs.filter (fun fv => fv.2 != w)) f = fs.get f := by
  induction fs with
  | nil => rfl
  | cons q fs ih =>
    obtain ⟨k, v⟩ := q
    unfold Fields.get at *
    rw [lookup_cons_nat] at h ⊢
    rw [List.filter_cons]
    by_cases hk : f = k
    · subst hk
      simp only [ite_true] at h ⊢
      have hb : (v != w) = true := by simpa using h
      simp only [hb, ite_true]
      rw [lookup_cons_nat]
      simp
    · simp only [hk, ite_false] at h ⊢
      by_cases hv : (v != w) = true
      · simp only [hv, ite_true]
        rw [lookup_cons_nat]
        simp only [hk, ite_false]
        exact ih h
      · simp only [hv]
        exact ih h

theorem mem_valuesOf {fs : Fields} {f : FieldId} {v : Val} (h : (f, v) ∈ fs) :
    v ∈ fs.valuesOf f := by
  simp only [Fields.valuesOf, List.mem_map, List.mem_filter]
  exact ⟨(f, v), ⟨h, by simp⟩, rfl⟩

theorem null_untyped (ty : Ty) : Val.null.hasType ty = false := by
  cases ty <;> rfl

/-! ## Lemmas: what a write that is not refused satisfies -/

theorem valueProblem_none {L : Limits} {uniq : Bool} {rs : Records} {self : RecordId}
    {f : FieldId} {d : FieldDef} {v : Val} (h : valueProblem L uniq rs self f d v = none) :
    v.overLimit L = false ∧ v.hasType d.ty = true ∧ v.resolves rs = true ∧
      (uniq = true → d.unique = true → clashAt rs self f v = false) := by
  unfold valueProblem at h
  cases h0 : v.overLimit L <;> cases h1 : (d.unique && decide (L.maxUnique < v.size)) <;>
    cases h2 : v.hasType d.ty <;> cases h3 : (uniq && d.unique && clashAt rs self f v) <;>
    cases h4 : v.resolves rs <;> simp_all

theorem fieldProblem_none {L : Limits} {uniq : Bool} {rs : Records} {self : RecordId}
    {fs : Fields} {fd : FieldId × FieldDef}
    (h : fieldProblem L uniq rs self fs fd = none) :
    (fd.2.required = true → (fs.valuesOf fd.1).isEmpty = false) ∧
      ∀ v ∈ fs.valuesOf fd.1, valueProblem L uniq rs self fd.1 fd.2 v = none := by
  unfold fieldProblem at h
  split at h
  · cases h
  · rename_i hc
    refine ⟨fun hr => ?_, fun v hv => List.findSome?_eq_none_iff.mp h v hv⟩
    simpa [hr] using hc

/-- What a record with nothing to refuse satisfies, part by part: within the record bound, every
    field known with a value that passes its checks, and every field of the schema passing its
    checks. -/
theorem problem_none_parts {L : Limits} {schema : Schema} {us : List (List FieldId)}
    {uniq : Bool} {rs : Records} {self : RecordId} {r : Record}
    (h : problem L schema us uniq rs self r = none) :
    r.size ≤ L.maxRecord ∧
      (∀ fv ∈ r.fields, ∃ d, schema.lookup fv.1 = some d ∧
        valueProblem L uniq rs self fv.1 d fv.2 = none) ∧
      ∀ fd ∈ schema, fieldProblem L uniq rs self r.fields fd = none := by
  unfold problem at h
  split at h
  · cases h
  · rename_i hsize
    split at h
    · rename_i hknown
      split at h
      · cases h
      · rename_i hall
        have hall' := List.findSome?_eq_none_iff.mp hall
        refine ⟨by simpa using hsize, fun fv hfv => ?_, hall'⟩
        have hk := List.all_eq_true.mp hknown fv hfv
        cases hl : schema.lookup fv.1 with
        | none => rw [hl] at hk; cases hk
        | some d =>
          refine ⟨d, rfl, ?_⟩
          have hmem := lookup_mem hl
          exact (fieldProblem_none (hall' _ hmem)).2 fv.2 (mem_valuesOf (by simpa using hfv))
    · cases h

/-- A record with nothing to refuse conforms to the schema and holds no dangling reference. -/
theorem problem_none {L : Limits} {schema : Schema} {us : List (List FieldId)} {uniq : Bool}
    {rs : Records} {self : RecordId} {r : Record}
    (h : problem L schema us uniq rs self r = none) :
    conforms schema r = true ∧ refsOk rs r = true := by
  obtain ⟨-, hval, hall⟩ := problem_none_parts h
  refine ⟨?_, ?_⟩
  · unfold conforms
    simp only [Bool.and_eq_true, List.all_eq_true]
    refine ⟨fun fv hfv => ?_, fun fd hfd => ?_⟩
    · obtain ⟨d, hl, hp⟩ := hval fv hfv
      simp [fieldOk, hl, (valueProblem_none hp).2.1]
    · unfold requiredOk
      cases hreq : fd.2.required
      · simp
      · have ⟨hne, hvs⟩ := fieldProblem_none (hall fd hfd)
        have hne := hne hreq
        cases hv : r.fields.valuesOf fd.1 with
        | nil => rw [hv] at hne; cases hne
        | cons v vs =>
          have hp := hvs v (by rw [hv]; exact List.mem_cons_self)
          have ht := (valueProblem_none hp).2.1
          rw [get_eq_head, hv]
          cases v <;> simp_all [null_untyped]
  · unfold refsOk
    rw [List.all_eq_true]
    intro fv hfv
    obtain ⟨d, _, hp⟩ := hval fv hfv
    exact (valueProblem_none hp).2.2.1

/-! ## Lemmas: values with references taken out -/

/-- A value still resolves when every record that existed still exists. -/
theorem resolves_mono {rs rs' : Records} (hinc : ∀ x, rs.has x = true → rs'.has x = true)
    {v : Val} (h : v.resolves rs = true) : v.resolves rs' = true := by
  cases v with
  | ref x => simp only [Val.resolves] at h ⊢; exact hinc x h
  | list vs =>
    simp only [Val.resolves, List.all_eq_true] at h ⊢
    intro e he
    have := h e he
    cases e <;> simp_all [Elem.resolves]
  | _ => rfl

/-- Taking the references to `id` out of a value that resolves leaves one that resolves among
    records that keep every record but `id`. -/
theorem resolves_dropRef {rs rs' : Records} {id : RecordId}
    (hinc : ∀ x, rs.has x = true → x ≠ id → rs'.has x = true) {v v' : Val}
    (h : v.resolves rs = true) (hd : v.dropRef id = some v') : v'.resolves rs' = true := by
  cases v with
  | ref x =>
    simp only [Val.dropRef] at hd
    split at hd
    · cases hd
    · rename_i hx
      cases hd
      simp only [Val.resolves] at h ⊢
      exact hinc x h hx
  | list vs =>
    simp only [Val.dropRef, Option.some.injEq] at hd
    subst hd
    simp only [Val.resolves, List.all_eq_true, List.mem_filter] at h ⊢
    intro e ⟨he, hne⟩
    have := h e he
    cases e with
    | ref x =>
      simp only [Elem.resolves] at this ⊢
      exact hinc x this (by rintro rfl; simp at hne)
    | _ => rfl
  | _ => simp only [Val.dropRef, Option.some.injEq] at hd; subst hd; rfl

/-- A value that does not name `id` is left as it is. -/
theorem dropRef_of_not_refersTo {id : RecordId} {v : Val} (h : v.refersTo id = false) :
    v.dropRef id = some v := by
  cases v with
  | ref x =>
    simp only [Val.refersTo, beq_eq_false_iff_ne, ne_eq] at h
    simp [Val.dropRef, h]
  | list vs =>
    simp only [Val.refersTo, List.contains_eq_any_beq, List.any_eq_false, beq_iff_eq] at h
    simp only [Val.dropRef, Option.some.injEq, Val.list.injEq]
    apply List.filter_eq_self.mpr
    intro e he
    have := h e he
    simp only [bne_iff_ne, ne_eq]
    intro heq
    subst heq
    simp at this
  | _ => rfl

/-- Taking references out keeps a value's type. -/
theorem hasType_dropRef {id : RecordId} {ty : Ty} {v v' : Val} (h : v.hasType ty = true)
    (hd : v.dropRef id = some v') : v'.hasType ty = true := by
  cases v with
  | ref x =>
    simp only [Val.dropRef] at hd
    split at hd
    · cases hd
    · cases hd; exact h
  | list vs =>
    simp only [Val.dropRef, Option.some.injEq] at hd
    subst hd
    cases ty with
    | list e =>
      simp only [Val.hasType, List.all_eq_true, List.mem_filter] at h ⊢
      exact fun x hx => h x hx.1
    | _ => simp [Val.hasType] at h
  | _ => simp only [Val.dropRef, Option.some.injEq] at hd; subst hd; exact h

/-- A present value that is not a single reference to `id` stays present. -/
theorem dropRef_present {id : RecordId} {v : Val} (h1 : v ≠ .null) (h2 : v ≠ .ref id) :
    ∃ v', v.dropRef id = some v' ∧ v' ≠ .null := by
  cases v with
  | null => exact absurd rfl h1
  | ref x =>
    have hx : x ≠ id := fun hx => h2 (by rw [hx])
    exact ⟨.ref x, by simp [Val.dropRef, hx], by simp⟩
  | list vs => exact ⟨_, rfl, by simp⟩
  | int i => exact ⟨_, rfl, h1⟩
  | str s => exact ⟨_, rfl, h1⟩
  | bool b => exact ⟨_, rfl, h1⟩

theorem mem_clearFields {id : RecordId} {fs : Fields} {fv : FieldId × Val}
    (h : fv ∈ clearFields id fs) : ∃ v, (fv.1, v) ∈ fs ∧ v.dropRef id = some fv.2 := by
  unfold clearFields at h
  obtain ⟨a, ha, hm⟩ := List.mem_filterMap.mp h
  cases hd : a.2.dropRef id with
  | none => rw [hd] at hm; cases hm
  | some v =>
    rw [hd] at hm
    simp only [Option.map_some, Option.some.injEq] at hm
    subst hm
    exact ⟨a.2, by simpa using ha, hd⟩

/-- A field that is present and not a single reference to `id` is still present after the
    references to `id` are cleared. -/
theorem get_clearFields {id : RecordId} {f : FieldId} :
    ∀ {fs : Fields}, fs.get f ≠ .null → fs.get f ≠ .ref id → (clearFields id fs).get f ≠ .null
  | [], h, _ => absurd rfl h
  | (k, v) :: fs, h1, h2 => by
    unfold Fields.get at h1 h2
    rw [lookup_cons_nat] at h1 h2
    unfold clearFields
    rw [List.filterMap_cons]
    by_cases hk : f = k
    · subst hk
      simp only [ite_true] at h1 h2
      obtain ⟨v', hd, hne⟩ := dropRef_present h1 h2
      simp only [hd, Option.map_some]
      unfold Fields.get
      rw [lookup_cons_nat]
      simpa using hne
    · simp only [hk, ite_false] at h1 h2
      have ih := get_clearFields (fs := fs) h1 h2
      unfold clearFields at ih
      cases hd : v.dropRef id with
      | none => simpa [hd] using ih
      | some v' =>
        simp only [Option.map_some]
        unfold Fields.get at ih ⊢
        rw [lookup_cons_nat]
        simpa [hk] using ih

/-! ## Lemmas: references across changes to the set of records -/

/-- References still resolve when every record that existed still exists. -/
theorem refsOk_mono {rs rs' : Records} {r : Record}
    (hinc : ∀ x, rs.has x = true → rs'.has x = true) (h : refsOk rs r = true) :
    refsOk rs' r = true := by
  unfold refsOk at *
  rw [List.all_eq_true] at *
  exact fun fv hfv => resolves_mono hinc (h fv hfv)

/-- Clearing the references to `id` keeps every other reference resolving, among records that
    keep every record but `id`. -/
theorem refsOk_clear {rs rs' : Records} {id : RecordId} {r : Record}
    (hinc : ∀ x, rs.has x = true → x ≠ id → rs'.has x = true) (h : refsOk rs r = true) :
    refsOk rs' (clearRef id r) = true := by
  unfold refsOk at h
  rw [List.all_eq_true] at h
  unfold clearRef
  split
  · simp only [refsOk, List.all_eq_true]
    intro fv hfv
    obtain ⟨v, hmem, hd⟩ := mem_clearFields hfv
    exact resolves_dropRef hinc (h _ hmem) hd
  · rename_i hn
    simp only [refsOk, List.all_eq_true]
    intro fv hfv
    have hnv : fv.2.refersTo id = false := by
      simp only [Record.refersTo, List.any_eq_true, not_exists, not_and, Bool.not_eq_true] at hn
      exact hn fv hfv
    exact resolves_dropRef hinc (h fv hfv) (dropRef_of_not_refersTo hnv)

/-- Clearing references through fields that do not forbid it keeps a record conforming. -/
theorem conforms_clear {schema : Schema} {id : RecordId} {r : Record}
    (hfree : ∀ fv ∈ r.fields, fv.2.refersTo id = true → forbidsDelete schema fv.1 fv.2 = false)
    (h : conforms schema r = true) : conforms schema (clearRef id r) = true := by
  unfold clearRef
  split
  · unfold conforms at *
    simp only [Bool.and_eq_true, List.all_eq_true] at *
    obtain ⟨hf, hr⟩ := h
    refine ⟨fun fv hfv => ?_, fun fd hfd => ?_⟩
    · obtain ⟨v, hmem, hd⟩ := mem_clearFields hfv
      have := hf _ hmem
      unfold fieldOk at *
      simp only at this ⊢
      cases hl : schema.lookup fv.1 with
      | none => rw [hl] at this; cases this
      | some d => rw [hl] at this; exact hasType_dropRef this hd
    · have := hr fd hfd
      unfold requiredOk at *
      cases hreq : fd.2.required
      · simp
      · simp only [hreq, Bool.not_true, Bool.false_or, bne_iff_ne, ne_eq] at this ⊢
        apply get_clearFields this
        intro heq
        have hmem := get_mem heq (by simp)
        have hf := hfree _ hmem (by simp [Val.refersTo])
        have : forbidsDelete schema fd.1 (.ref id) = true := by
          unfold forbidsDelete
          exact List.any_eq_true.mpr ⟨fd, hfd, by simp [hreq, Val.isList]⟩
        simp_all
  · exact h

/-- The records after a delete: `id` gone, every reference to it cleared. -/
abbrev afterDelete (rs : Records) (id : RecordId) : Records :=
  (rs.filter (fun q => q.1 != id)).map (fun q => (q.1, clearRef id q.2))

theorem has_afterDelete {rs : Records} {id x : RecordId} :
    Records.has (afterDelete rs id) x = true ↔ rs.has x = true ∧ x ≠ id := by
  simp only [has_iff, List.mem_map, List.mem_filter, Prod.mk.injEq]
  constructor
  · rintro ⟨r, q, ⟨hq, hne⟩, hx, -⟩
    subst hx
    exact ⟨⟨q.2, by simpa using hq⟩, by simpa using hne⟩
  · rintro ⟨⟨r, hr⟩, hne⟩
    exact ⟨clearRef id r, (x, r), ⟨hr, by simpa using hne⟩, rfl, rfl⟩

/-- Replacing one record's fields keeps the set of ids. -/
theorem has_replace {rs : Records} {id x : RecordId} {r : Record} :
    Records.has (rs.map (fun q => if q.1 == id then (q.1, r) else q)) x = rs.has x := by
  apply Bool.eq_iff_iff.mpr
  simp only [has_iff, List.mem_map]
  constructor
  · rintro ⟨r', q, hq, heq⟩
    by_cases hid : (q.1 == id) = true
    · simp only [hid, ite_true, Prod.mk.injEq] at heq
      obtain ⟨hx, -⟩ := heq
      exact ⟨q.2, by rw [← hx]; simpa using hq⟩
    · simp only [hid] at heq
      exact ⟨r', by rw [← heq]; exact hq⟩
  · rintro ⟨r', hr⟩
    by_cases hid : (x == id) = true
    · exact ⟨r, (x, r'), hr, by simp [hid]⟩
    · exact ⟨r', (x, r'), hr, by simp [hid]⟩

/-! ## Schema changes that leave the records alone -/

/-- A field made unique leaves every record as it is. -/
theorem setUnique_records (t t' : Tenant) (p : PrincipalId) (f : FieldId) (s : Bool)
    (h : stepTenant t p (.setUnique f s) = .ok t') : t'.records = t.records := by
  simp only [stepTenant] at h
  repeat' split at h
  all_goals (try cases h)
  all_goals rfl

/-- A composite unique constraint leaves every record as it is. -/
theorem defineUnique_records (t t' : Tenant) (p : PrincipalId) (fs : List FieldId)
    (h : stepTenant t p (.defineUnique fs) = .ok t') : t'.records = t.records := by
  simp only [stepTenant] at h
  repeat' split at h
  all_goals (try cases h)
  all_goals rfl

/-! ## The slice's properties -/

/-- Referential integrity is kept by every accepted step on a tenant, strict or not: a write
    names only existing records, single or in a list; a delete is refused or clears what named
    the deleted record (a field, or a list element); and a schema change touches no record. -/
theorem stepTenant_preserves_refs (t t' : Tenant) (p : PrincipalId) (i : Intent)
    (hrefs : t.RefsOk) (h : stepTenant t p i = .ok t') : t'.RefsOk := by
  cases i with
  | create fs =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · rename_i hp
        cases h
        have hinc : ∀ x, t.records.has x = true →
            Records.has ((t.nextId, newRecord t.schema p fs) :: t.records) x = true := by
          intro x hx
          obtain ⟨r, hr⟩ := has_iff.mp hx
          exact has_iff.mpr ⟨r, List.mem_cons_of_mem _ hr⟩
        intro q hq
        cases hq with
        | head => exact refsOk_mono hinc (problem_none hp).2
        | tail _ hq' => exact refsOk_mono hinc (hrefs q hq')
  | update id fs ev =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · rename_i hp
            cases h
            intro q hq
            simp only at hq ⊢
            obtain ⟨q₀, hq₀, rfl⟩ := List.mem_map.mp hq
            by_cases hid : (q₀.1 == id) = true
            · simp only [hid, ite_true]
              exact refsOk_mono (fun x hx => by rw [has_replace]; exact hx) (problem_none hp).2
            · simp only [hid]
              exact refsOk_mono (fun x hx => by rw [has_replace]; exact hx) (hrefs q₀ hq₀)
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
            intro q hq
            simp only at hq ⊢
            obtain ⟨q₀, hq₀, rfl⟩ := List.mem_map.mp hq
            exact refsOk_clear (fun x hx hne => has_afterDelete.mpr ⟨hx, hne⟩)
              (hrefs q₀ (List.mem_filter.mp hq₀).1)
  | setRequired f strict =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · split at h
      · cases h
      · cases h
        exact hrefs
  | setUnique f strict =>
    have hrec := setUnique_records t t' p f strict h
    intro q hq
    rw [hrec] at hq ⊢
    exact hrefs q hq
  | defineUnique fs =>
    have hrec := defineUnique_records t t' p fs h
    intro q hq
    rw [hrec] at hq ⊢
    exact hrefs q hq

/-- Referential integrity for the world: every accepted step keeps it, strict or not. -/
theorem step_preserves_refs (w w' : World) (i : Intent) (c : Ctx)
    (hrefs : w.RefsOk) (h : step w i c = .ok w') : w'.RefsOk := by
  intro x t' hx
  unfold step at h
  cases ht : w c.tenant with
  | none => rw [ht] at h; cases h
  | some t =>
    rw [ht] at h
    simp only at h
    cases hs : stepTenant t c.principal i with
    | error e => rw [hs] at h; cases h
    | ok t₁ =>
      rw [hs] at h
      cases h
      by_cases hxc : x = c.tenant
      · simp [hxc] at hx
        subst hx
        exact stepTenant_preserves_refs t _ c.principal i (hrefs c.tenant t ht) hs
      · simp [hxc] at hx
        exact hrefs x t' hx

/-- A record that another record names through a field that restricts deletion, or through a
    required single reference, cannot be deleted: one who may delete it, naming no version or
    the record's version, is refused with `stillReferenced`. -/
theorem delete_refused_while_referenced (t : Tenant) (p : PrincipalId) (id : RecordId)
    (ev : Option Nat) (old : Record) (q : RecordId × Record)
    (hl : t.records.lookup id = some old) (hcan : canEdit t p old = true)
    (hv : versionClash ev old = false)
    (hq : q ∈ t.records) (hb : blocks t.schema id q = true) :
    stepTenant t p (.delete id ev) = .error .stillReferenced := by
  have hany : t.records.any (blocks t.schema id) = true := List.any_eq_true.mpr ⟨q, hq, hb⟩
  simp [stepTenant, hl, hcan, hv, hany]

/-- An accepted delete removes the record and leaves no record naming it, through a single
    reference or a list element, whatever the state. -/
theorem delete_leaves_no_reference (t t' : Tenant) (p : PrincipalId) (id : RecordId)
    (ev : Option Nat) (h : stepTenant t p (.delete id ev) = .ok t') :
    t'.records.has id = false ∧ ∀ q ∈ t'.records, ∀ fv ∈ q.2.fields, fv.2.refersTo id = false := by
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
          refine ⟨?_, ?_⟩
          · cases hh : Records.has (afterDelete t.records id) id
            · rfl
            · exact absurd rfl (has_afterDelete.mp hh).2
          · intro q hq fv hfv
            obtain ⟨q₀, -, rfl⟩ := List.mem_map.mp hq
            simp only [clearRef] at hfv
            split at hfv
            · obtain ⟨v, -, hd⟩ := mem_clearFields hfv
              cases v with
              | ref x =>
                simp only [Val.dropRef] at hd
                split at hd
                · cases hd
                · rename_i hx
                  simp only [Option.some.injEq] at hd
                  rw [← hd]
                  simpa [Val.refersTo] using hx
              | list vs =>
                simp only [Val.dropRef, Option.some.injEq] at hd
                rw [← hd]
                simp [Val.refersTo]
              | _ =>
                simp only [Val.dropRef, Option.some.injEq] at hd
                rw [← hd]
                rfl
            · rename_i hn
              simp only [Record.refersTo, List.any_eq_true, not_exists, not_and,
                Bool.not_eq_true] at hn
              exact hn fv hfv

/-- Conformance survives the clearing a delete does: a record that blocks nothing has no
    reference to the deleted record through a field that forbids its clearing. -/
theorem conforms_afterDelete (t : Tenant) (id : RecordId) (q : RecordId × Record)
    (hq : q ∈ t.records) (hne : q.1 ≠ id) (hnb : t.records.any (blocks t.schema id) = false)
    (hc : conforms t.schema q.2 = true) : conforms t.schema (clearRef id q.2) = true := by
  apply conforms_clear _ hc
  intro fv hfv hv
  cases hf : forbidsDelete t.schema fv.1 fv.2
  · rfl
  · have : blocks t.schema id q = true := by
      unfold blocks
      simp only [Bool.and_eq_true, bne_iff_ne, ne_eq]
      exact ⟨hne, List.any_eq_true.mpr ⟨fv, hfv, by simp [hv, hf]⟩⟩
    have := List.any_eq_true.mpr ⟨q, hq, this⟩
    rw [hnb] at this
    cases this

end Strata
