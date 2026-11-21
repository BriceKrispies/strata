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

theorem valueProblem_none {rs : Records} {ty : Ty} {v : Val}
    (h : valueProblem rs ty v = none) : v.hasType ty = true ∧ v.resolves rs = true := by
  unfold valueProblem at h
  cases h1 : v.hasType ty <;> cases h2 : v.resolves rs <;> simp_all

theorem fieldProblem_none {rs : Records} {fs : Fields} {fd : FieldId × FieldDef}
    (h : fieldProblem rs fs fd = none) :
    (fd.2.required = true → (fs.valuesOf fd.1).isEmpty = false) ∧
      ∀ v ∈ fs.valuesOf fd.1, valueProblem rs fd.2.ty v = none := by
  unfold fieldProblem at h
  split at h
  · cases h
  · rename_i hc
    refine ⟨fun hr => ?_, fun v hv => List.findSome?_eq_none_iff.mp h v hv⟩
    simpa [hr] using hc

/-- A record with nothing to refuse conforms to the schema and holds no dangling reference. -/
theorem problem_none {schema : Schema} {rs : Records} {r : Record}
    (h : problem schema rs r = none) : conforms schema r = true ∧ refsOk rs r = true := by
  unfold problem at h
  split at h
  · rename_i hknown
    have hall := List.findSome?_eq_none_iff.mp h
    -- every value of a known field passes its field's checks
    have hval : ∀ fv ∈ r.fields, ∃ d, schema.lookup fv.1 = some d ∧
        valueProblem rs d.ty fv.2 = none := by
      intro fv hfv
      have hk := List.all_eq_true.mp hknown fv hfv
      cases hl : schema.lookup fv.1 with
      | none => rw [hl] at hk; cases hk
      | some d =>
        refine ⟨d, rfl, ?_⟩
        have hmem := lookup_mem hl
        exact (fieldProblem_none (hall _ hmem)).2 fv.2 (mem_valuesOf (by simpa using hfv))
    refine ⟨?_, ?_⟩
    · unfold conforms
      simp only [Bool.and_eq_true, List.all_eq_true]
      refine ⟨fun fv hfv => ?_, fun fd hfd => ?_⟩
      · obtain ⟨d, hl, hp⟩ := hval fv hfv
        simp [fieldOk, hl, (valueProblem_none hp).1]
      · unfold requiredOk
        cases hreq : fd.2.required
        · simp
        · have ⟨hne, hvs⟩ := fieldProblem_none (hall fd hfd)
          have hne := hne hreq
          cases hv : r.fields.valuesOf fd.1 with
          | nil => rw [hv] at hne; cases hne
          | cons v vs =>
            have hp := hvs v (by rw [hv]; exact List.mem_cons_self)
            have ht := (valueProblem_none hp).1
            rw [get_eq_head, hv]
            cases v <;> simp_all [null_untyped]
    · unfold refsOk
      rw [List.all_eq_true]
      intro fv hfv
      obtain ⟨d, _, hp⟩ := hval fv hfv
      exact (valueProblem_none hp).2
  · cases h

/-! ## Lemmas: references across changes to the set of records -/

/-- References still resolve when every record that existed still exists. -/
theorem refsOk_mono {rs rs' : Records} {r : Record}
    (hinc : ∀ x, rs.has x = true → rs'.has x = true) (h : refsOk rs r = true) :
    refsOk rs' r = true := by
  unfold refsOk at *
  rw [List.all_eq_true] at *
  intro fv hfv
  have := h fv hfv
  cases hv : fv.2 <;> simp_all [Val.resolves]

/-- Clearing the references to `id` keeps every other reference resolving, among records that
    keep every record but `id`. -/
theorem refsOk_clear {rs rs' : Records} {id : RecordId} {r : Record}
    (hinc : ∀ x, rs.has x = true → x ≠ id → rs'.has x = true) (h : refsOk rs r = true) :
    refsOk rs' (clearRef id r) = true := by
  unfold refsOk clearRef at *
  rw [List.all_eq_true] at *
  intro fv hfv
  simp only [List.mem_filter] at hfv
  obtain ⟨hfv, hne⟩ := hfv
  have := h fv hfv
  cases hv : fv.2 with
  | ref x =>
    rw [hv] at this hne
    simp only [Val.resolves] at this ⊢
    exact hinc x this (by intro hx; subst hx; simp at hne)
  | _ => simp [Val.resolves]

/-- Clearing references through fields that do not forbid it keeps a record conforming. -/
theorem conforms_clear {schema : Schema} {id : RecordId} {r : Record}
    (hfree : ∀ fv ∈ r.fields, fv.2 = .ref id → forbidsDelete schema fv.1 = false)
    (h : conforms schema r = true) : conforms schema (clearRef id r) = true := by
  unfold conforms clearRef at *
  simp only [Bool.and_eq_true, List.all_eq_true] at *
  obtain ⟨hf, hr⟩ := h
  refine ⟨fun fv hfv => hf fv (List.mem_filter.mp hfv).1, fun fd hfd => ?_⟩
  have := hr fd hfd
  unfold requiredOk at *
  cases hreq : fd.2.required
  · simp
  · simp only [hreq, Bool.not_true, Bool.false_or, bne_iff_ne, ne_eq] at this ⊢
    have hne : r.fields.get fd.1 ≠ .ref id := by
      intro heq
      have hmem := get_mem heq (by simp)
      have hf := hfree _ hmem rfl
      have : forbidsDelete schema fd.1 = true := by
        unfold forbidsDelete
        exact List.any_eq_true.mpr ⟨fd, hfd, by simp [hreq]⟩
      simp_all
    rw [get_filter_ne _ _ _ hne]
    exact this

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

/-! ## The slice's properties -/

/-- Referential integrity is kept by every accepted step on a tenant, strict or not: a write
    names only existing records, a delete is refused or clears what named the deleted record,
    and a schema change touches no record. -/
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
            Records.has ((t.nextId, { owner := p, fields := normalise fs }) :: t.records) x = true := by
          intro x hx
          obtain ⟨r, hr⟩ := has_iff.mp hx
          exact has_iff.mpr ⟨r, List.mem_cons_of_mem _ hr⟩
        intro q hq
        cases hq with
        | head => exact refsOk_mono hinc (problem_none hp).2
        | tail _ hq' => exact refsOk_mono hinc (hrefs q hq')
  | update id fs =>
    simp only [stepTenant] at h
    split at h
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
  | delete id =>
    simp only [stepTenant] at h
    split at h
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
    required field, cannot be deleted: one who may delete it is refused with `stillReferenced`. -/
theorem delete_refused_while_referenced (t : Tenant) (p : PrincipalId) (id : RecordId)
    (old : Record) (q : RecordId × Record)
    (hl : t.records.lookup id = some old) (hcan : canEdit t p old = true)
    (hq : q ∈ t.records) (hb : blocks t.schema id q = true) :
    stepTenant t p (.delete id) = .error .stillReferenced := by
  have hany : t.records.any (blocks t.schema id) = true := List.any_eq_true.mpr ⟨q, hq, hb⟩
  simp [stepTenant, hl, hcan, hany]

/-- An accepted delete removes the record and leaves no record naming it, whatever the state. -/
theorem delete_leaves_no_reference (t t' : Tenant) (p : PrincipalId) (id : RecordId)
    (h : stepTenant t p (.delete id) = .ok t') :
    t'.records.has id = false ∧ ∀ q ∈ t'.records, ∀ fv ∈ q.2.fields, fv.2 ≠ .ref id := by
  simp only [stepTenant] at h
  split at h
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
          have := (List.mem_filter.mp hfv).2
          simpa using this

/-- Conformance survives the clearing a delete does: a record that blocks nothing has no
    reference to the deleted record through a field that forbids its clearing. -/
theorem conforms_afterDelete (t : Tenant) (id : RecordId) (q : RecordId × Record)
    (hq : q ∈ t.records) (hne : q.1 ≠ id) (hnb : t.records.any (blocks t.schema id) = false)
    (hc : conforms t.schema q.2 = true) : conforms t.schema (clearRef id q.2) = true := by
  apply conforms_clear _ hc
  intro fv hfv hv
  cases hf : forbidsDelete t.schema fv.1
  · rfl
  · have : blocks t.schema id q = true := by
      unfold blocks
      simp only [Bool.and_eq_true, bne_iff_ne, ne_eq]
      exact ⟨hne, List.any_eq_true.mpr ⟨fv, hfv, by simp [hv, hf]⟩⟩
    have := List.any_eq_true.mpr ⟨q, hq, this⟩
    rw [hnb] at this
    cases this

end Strata
