/-
Reference integrity over several object types (slices 1, 2 and 4 of the reference model).

What the file claims, about the model in `Strata/Model.lean`:

- A write is accepted only if every record it names exists in the type its field names
  (`problem_none`), so referential integrity (`Tenant.RefsOk`) holds after every accepted step from
  a state where it held, strict or not, across types (`stepTenant_preserves_refs`,
  `step_preserves_refs`).
- A delete of a record that a record of any type names through a restricting or required field is
  refused with `stillReferenced` (`delete_refused_while_referenced`).
- An accepted delete leaves no record of any type naming the deleted one, and the deleted one gone
  (`delete_leaves_no_reference`).

The first part is lemmas about field lists and record lists; the second about values and the
references they hold; the third about the object types of a tenant.

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


/-! ## Lemmas: what a write that is not refused satisfies -/

theorem valueProblem_none {L : Limits} {uniq : Bool} {env : Env} {rs : Records} {self : RecordId}
    {f : FieldId} {d : FieldDef} {v : Val} (h : valueProblem L uniq env rs self f d v = none) :
    v.overLimit L = false ∧ v.hasType d.ty = true ∧ v.resolves env d.ty = true ∧
      (uniq = true → d.unique = true → clashAt rs self f v = false) := by
  unfold valueProblem at h
  cases h0 : v.overLimit L <;> cases h1 : (d.unique && decide (L.maxUnique < v.size)) <;>
    cases h2 : v.hasType d.ty <;> cases h3 : (uniq && d.unique && clashAt rs self f v) <;>
    cases h4 : v.resolves env d.ty <;> simp_all

theorem fieldProblem_none {L : Limits} {uniq : Bool} {env : Env} {rs : Records} {self : RecordId}
    {fs : Fields} {fd : FieldId × FieldDef}
    (h : fieldProblem L uniq env rs self fs fd = none) :
    (fd.2.required = true → (fs.valuesOf fd.1).isEmpty = false) ∧
      ∀ v ∈ fs.valuesOf fd.1, valueProblem L uniq env rs self fd.1 fd.2 v = none := by
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
    {uniq : Bool} {env : Env} {rs : Records} {self : RecordId} {r : Record}
    (h : problem L schema us uniq env rs self r = none) :
    r.size ≤ L.maxRecord ∧
      (∀ fv ∈ r.fields, ∃ d, schema.lookup fv.1 = some d ∧
        valueProblem L uniq env rs self fv.1 d fv.2 = none) ∧
      ∀ fd ∈ schema, fieldProblem L uniq env rs self r.fields fd = none := by
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

/-- A record with nothing to refuse conforms to the schema, and every reference it holds names an
    existing record of the type its field names. -/
theorem problem_none {L : Limits} {schema : Schema} {us : List (List FieldId)} {uniq : Bool}
    {env : Env} {rs : Records} {self : RecordId} {r : Record}
    (h : problem L schema us uniq env rs self r = none) :
    conforms schema r = true ∧ refsOk env schema r = true := by
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
    obtain ⟨d, hl, hp⟩ := hval fv hfv
    simp only [hl]
    exact (valueProblem_none hp).2.2.1

/-! ## Lemmas: references across the tables of a tenant -/

/-- A reference still resolves when every record that existed, in every type, still exists. -/
theorem resolves_mono {env env' : Env} (hinc : ∀ t x, Records.has (env t) x = true → Records.has (env' t) x = true)
    {ty : Ty} {v : Val} (h : v.resolves env ty = true) : v.resolves env' ty = true := by
  cases v with
  | ref x =>
    simp only [Val.resolves, refHas] at h ⊢
    cases hT : ty.target? with
    | none => rfl
    | some t => simp only [hT] at h ⊢; exact hinc t x h
  | list vs =>
    simp only [Val.resolves, List.all_eq_true] at h ⊢
    intro e he
    have hres := h e he
    cases e with
    | ref x =>
      simp only [Elem.resolves, refHas] at hres ⊢
      cases hT : ty.target? with
      | none => rfl
      | some t => simp only [hT] at hres ⊢; exact hinc t x hres
    | _ => rfl
  | _ => rfl

/-- Taking the references to some record out of a value leaves a value that resolves wherever the
    original did. -/
theorem resolves_dropRef_sub {env : Env} {ty : Ty} {id : RecordId} {v v' : Val}
    (h : v.resolves env ty = true) (hd : v.dropRef id = some v') : v'.resolves env ty = true := by
  cases v with
  | ref x =>
    simp only [Val.dropRef] at hd
    split at hd
    · cases hd
    · cases hd; exact h
  | list vs =>
    simp only [Val.dropRef, Option.some.injEq] at hd
    subst hd
    simp only [Val.resolves, List.all_eq_true, List.mem_filter] at h ⊢
    intro e he
    exact h e he.1
  | _ => simp only [Val.dropRef, Option.some.injEq] at hd; subst hd; exact h

/-- Dropping the references to `id` leaves no reference to `id`. -/
theorem dropRef_refersTo {id : RecordId} {v v' : Val} (hd : v.dropRef id = some v') :
    v'.refersTo id = false := by
  cases v with
  | ref x =>
    simp only [Val.dropRef] at hd
    split at hd
    · cases hd
    · rename_i hx
      cases hd
      simpa [Val.refersTo] using hx
  | list vs =>
    simp only [Val.dropRef, Option.some.injEq] at hd
    subst hd
    simp only [Val.refersTo, List.contains_eq_any_beq, List.any_eq_false, List.mem_filter]
    intro e he
    simp only [beq_iff_eq, bne_iff_ne, ne_eq] at he ⊢
    obtain ⟨-, hne⟩ := he
    intro hbad
    exact hne hbad.symm
  | _ => simp only [Val.dropRef, Option.some.injEq] at hd; subst hd; rfl

/-- A reference that resolves, and does not name `id` through a field of type `T`, still resolves
    once the records of type `T` lose record `id`: the tables gained nothing and lost only
    `(T, id)`. -/
theorem resolves_exc {env env' : Env} {T : TypeId} {id : RecordId}
    (hinc : ∀ t x, (t ≠ T ∨ x ≠ id) → Records.has (env t) x = true → Records.has (env' t) x = true)
    {ty : Ty} {v : Val} (h : v.resolves env ty = true)
    (hn : ty.target? ≠ some T ∨ v.refersTo id = false) : v.resolves env' ty = true := by
  cases v with
  | ref x =>
    simp only [Val.resolves, refHas] at h ⊢
    cases hT : ty.target? with
    | none => rfl
    | some t =>
      simp only [hT] at h ⊢
      by_cases htT : t = T
      · subst htT
        have hx : x ≠ id := by
          rcases hn with hn | hn
          · exact absurd hT hn
          · simpa [Val.refersTo] using hn
        exact hinc t x (Or.inr hx) h
      · exact hinc t x (Or.inl htT) h
  | list vs =>
    simp only [Val.resolves, List.all_eq_true] at h ⊢
    intro e he
    have hres := h e he
    cases e with
    | ref x =>
      simp only [Elem.resolves, refHas] at hres ⊢
      cases hT : ty.target? with
      | none => rfl
      | some t =>
        simp only [hT] at hres ⊢
        by_cases htT : t = T
        · subst htT
          have hx : x ≠ id := by
            rcases hn with hn | hn
            · exact absurd hT hn
            · intro hxid
              subst hxid
              have hmem : Elem.ref x ∈ vs := he
              simp only [Val.refersTo, List.contains_eq_any_beq, List.any_eq_false,
                beq_iff_eq] at hn
              exact hn (Elem.ref x) hmem rfl
          exact hinc t x (Or.inr hx) hres
        · exact hinc t x (Or.inl htT) hres
    | _ => rfl
  | _ => rfl

/-- A field of a record of a type with schema `schema` that does not name the deleted record
    through the type `T` gives `resolves_exc` what it needs. -/
theorem fieldRefers_false_hn {schema : Schema} {T : TypeId} {id : RecordId}
    {fv : FieldId × Val} {d : FieldDef} (hl : schema.lookup fv.1 = some d)
    (h : fieldRefers schema T id fv = false) :
    d.ty.target? ≠ some T ∨ fv.2.refersTo id = false := by
  unfold fieldRefers at h
  rw [hl] at h
  by_cases ht : d.ty.target? = some T
  · right
    simpa [ht] using h
  · left; exact ht

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

/-- Records are added: every reference still resolves. -/
theorem refsOk_mono {env env' : Env} {schema : Schema} {r : Record}
    (hinc : ∀ t x, Records.has (env t) x = true → Records.has (env' t) x = true) (h : refsOk env schema r = true) :
    refsOk env' schema r = true := by
  unfold refsOk at *
  rw [List.all_eq_true] at *
  intro fv hfv
  have := h fv hfv
  cases hl : schema.lookup fv.1 with
  | none => rfl
  | some d =>
    rw [hl] at this
    exact resolves_mono hinc this

/-- Each field of the clearing is a field of the original, unchanged, or changed through the
    references it holds to the deleted record in a field of the type `T`. -/
theorem mem_clearFields {schema : Schema} {T : TypeId} {id : RecordId} {fs : Fields}
    {fv : FieldId × Val} (h : fv ∈ clearFields schema T id fs) :
    ∃ v, (fv.1, v) ∈ fs ∧
      ((fieldRefers schema T id (fv.1, v) = false ∧ fv.2 = v) ∨
        (fieldRefers schema T id (fv.1, v) = true ∧ v.dropRef id = some fv.2)) := by
  unfold clearFields at h
  obtain ⟨a, ha, hm⟩ := List.mem_filterMap.mp h
  split at hm
  · rename_i hr
    cases hd : a.2.dropRef id with
    | none => simp [hd] at hm
    | some v =>
      simp only [hd, Option.map_some, Option.some.injEq] at hm
      subst hm
      exact ⟨a.2, by simpa using ha, Or.inr ⟨hr, hd⟩⟩
  · rename_i hr
    simp only [Option.some.injEq] at hm
    subst hm
    exact ⟨a.2, by simpa using ha, Or.inl ⟨by simpa using hr, rfl⟩⟩

/-- A field whose first entry is present, not `null`, and not a single reference to `id` through a
    field that forbids its clearing, keeps a present value after the clearing. -/
theorem get_clearFields {schema : Schema} {T : TypeId} {id : RecordId} {f : FieldId} :
    ∀ {fs : Fields}, (∃ v, fs.lookup f = some v ∧ v ≠ .null ∧
        (v ≠ .ref id ∨ fieldRefers schema T id (f, v) = false)) →
      ∃ v', (clearFields schema T id fs).lookup f = some v' ∧ v' ≠ .null
  | [], ⟨_, h, _, _⟩ => by simp [List.lookup] at h
  | (k, u) :: fs, ⟨w, hw, hwn, hwr⟩ => by
    by_cases hk : f = k
    · subst k
      rw [lookup_cons_nat, if_pos rfl] at hw
      have hvw : u = w := Option.some.inj hw
      subst hvw
      by_cases hr : fieldRefers schema T id (f, u) = true
      · have hv : u ≠ .ref id := by
          rcases hwr with h' | h'
          · exact h'
          · rw [hr] at h'; cases h'
        obtain ⟨v', hd, hne⟩ := dropRef_present hwn hv
        refine ⟨v', ?_, hne⟩
        simp [clearFields, hr, hd, lookup_cons_nat]
      · have hr' : fieldRefers schema T id (f, u) = false := by simpa using hr
        refine ⟨u, ?_, hwn⟩
        simp [clearFields, hr', lookup_cons_nat]
    · rw [lookup_cons_nat, if_neg hk] at hw
      obtain ⟨v', hv', hn'⟩ := get_clearFields (schema := schema) (T := T) (id := id) (f := f)
        (fs := fs) ⟨w, hw, hwn, hwr⟩
      refine ⟨v', ?_, hn'⟩
      unfold clearFields at hv' ⊢
      simp only [List.filterMap_cons]
      by_cases hr : fieldRefers schema T id (k, u) = true
      · rw [if_pos hr]
        cases hd : u.dropRef id with
        | none => exact hv'
        | some w' =>
          simp only [hd, Option.map_some]
          rw [lookup_cons_nat, if_neg hk]
          exact hv'
      · rw [if_neg hr]
        dsimp only
        rw [lookup_cons_nat, if_neg hk]
        exact hv'

/-- A required field of a record that conforms, whose value is not a reference to the deleted
    record through a field that forbids it, is still present after the clearing. -/
theorem requiredOk_clear {schema : Schema} {T : TypeId} {id : RecordId} {fs : Fields}
    {fd : FieldId × FieldDef}
    (hfree : ∀ fv ∈ fs, fieldRefers schema T id fv = true →
      forbidsDelete schema fv.1 fv.2 = false)
    (hmem : fd ∈ schema) (hreq : fd.2.required = true) (h : requiredOk fs fd = true) :
    requiredOk (clearFields schema T id fs) fd = true := by
  unfold requiredOk at h ⊢
  rw [hreq] at h ⊢
  simp only [Bool.not_true, Bool.false_or, bne_iff_ne, ne_eq] at h ⊢
  have hget : fs.get fd.1 ≠ .null := h
  cases hl : fs.lookup fd.1 with
  | none => simp [Fields.get, hl] at hget
  | some v =>
    have hv : v ≠ .null := by simpa [Fields.get, hl] using hget
    have hmemv : (fd.1, v) ∈ fs := lookup_mem hl
    have hr : v ≠ .ref id ∨ fieldRefers schema T id (fd.1, v) = false := by
      by_cases hvr : v = .ref id
      · right
        by_cases hc' : fieldRefers schema T id (fd.1, v) = true
        · have hf := hfree (fd.1, v) hmemv hc'
          have hforb : forbidsDelete schema fd.1 v = true := by
            unfold forbidsDelete
            exact List.any_eq_true.mpr ⟨fd, hmem, by simp [hreq, hvr, Val.isList]⟩
          rw [hforb] at hf
          cases hf
        · simpa using hc'
      · exact Or.inl hvr
    obtain ⟨v', hv', hn'⟩ := get_clearFields (schema := schema) (T := T) (id := id) (f := fd.1)
      (fs := fs) ⟨v, hl, hv, hr⟩
    simpa [Fields.get, hv'] using hn'

/-- Clearing the references to `id` keeps every other reference resolving, among the tables that
    keep every record but `(T, id)`. -/
theorem refsOk_clear {env env' : Env} {T : TypeId} {id : RecordId} {schema : Schema} {r : Record}
    (hinc : ∀ t x, (t ≠ T ∨ x ≠ id) → Records.has (env t) x = true → Records.has (env' t) x = true)
    (h : refsOk env schema r = true) :
    refsOk env' schema (clearRef schema T id r) = true := by
  unfold refsOk at h
  rw [List.all_eq_true] at h
  unfold clearRef
  split
  · unfold refsOk
    rw [List.all_eq_true]
    intro fv hfv
    obtain ⟨v, hmem, hc⟩ := mem_clearFields hfv
    have hv := h (fv.1, v) hmem
    cases hl : schema.lookup fv.1 with
    | none => rfl
    | some d =>
      rw [hl] at hv
      rcases hc with ⟨hf, hfv'⟩ | ⟨-, hd⟩
      · rw [hfv']
        exact resolves_exc hinc hv (fieldRefers_false_hn (fv := (fv.1, v)) hl hf)
      · exact resolves_exc hinc (resolves_dropRef_sub hv hd) (Or.inr (dropRef_refersTo hd))
  · rename_i hn
    unfold refsOk
    rw [List.all_eq_true]
    intro fv hfv
    have hf : fieldRefers schema T id fv = false := by
      by_cases hc' : fieldRefers schema T id fv = true
      · exact absurd (by unfold recRefers; exact List.any_eq_true.mpr ⟨fv, hfv, hc'⟩) hn
      · simpa using hc'
    have hv := h fv hfv
    cases hl : schema.lookup fv.1 with
    | none => rfl
    | some d =>
      rw [hl] at hv
      exact resolves_exc hinc hv (fieldRefers_false_hn hl hf)

/-- Clearing references through fields that do not forbid it keeps a record conforming. -/
theorem conforms_clear {schema : Schema} {T : TypeId} {id : RecordId} {r : Record}
    (hfree : ∀ fv ∈ r.fields, fieldRefers schema T id fv = true →
      forbidsDelete schema fv.1 fv.2 = false)
    (h : conforms schema r = true) : conforms schema (clearRef schema T id r) = true := by
  unfold clearRef
  split
  · unfold conforms at *
    simp only [Bool.and_eq_true, List.all_eq_true] at *
    obtain ⟨hf, hr⟩ := h
    refine ⟨fun fv hfv => ?_, fun fd hfd => ?_⟩
    · obtain ⟨v, hmem, hc⟩ := mem_clearFields hfv
      have hok := hf _ hmem
      unfold fieldOk at *
      cases hl : schema.lookup fv.1 with
      | none => rw [hl] at hok; cases hok
      | some d =>
        rw [hl] at hok
        rcases hc with ⟨-, hfv'⟩ | ⟨-, hd⟩
        · rw [hfv']; exact hok
        · exact hasType_dropRef hok hd
    · cases hreq : fd.2.required
      · simp [requiredOk, hreq]
      · exact requiredOk_clear hfree hfd hreq (hr fd hfd)
  · exact h

/-! ## Lemmas: record lists after a clearing, and membership -/

theorem has_cons_of (q : RecordId × Record) {rs : Records} {x : RecordId}
    (h : Records.has rs x = true) : Records.has (q :: rs) x = true := by
  rw [has_iff] at h ⊢
  obtain ⟨r, hr⟩ := h
  exact ⟨r, List.mem_cons_of_mem _ hr⟩

theorem has_delClear (schema : Schema) (ty : TypeId) (id : RecordId) (rs : Records)
    (x : RecordId) : Records.has (delClear schema ty id rs) x = Records.has rs x := by
  apply Bool.eq_iff_iff.mpr
  rw [has_iff, has_iff]
  constructor
  · rintro ⟨r, hr⟩
    unfold delClear at hr
    obtain ⟨a, ha, hae⟩ := List.mem_map.mp hr
    rw [Prod.mk.injEq] at hae
    exact ⟨a.2, by rw [← hae.1]; exact ha⟩
  · rintro ⟨r, hr⟩
    exact ⟨clearRef schema ty id r, List.mem_map.mpr ⟨(x, r), hr, rfl⟩⟩

theorem has_filter_ne (rs : Records) (id x : RecordId) :
    Records.has (rs.filter (fun q => q.1 != id)) x = (Records.has rs x && x != id) := by
  apply Bool.eq_iff_iff.mpr
  rw [has_iff, Bool.and_eq_true, has_iff, bne_iff_ne]
  constructor
  · rintro ⟨r, hr⟩
    obtain ⟨hmem, hp⟩ := List.mem_filter.mp hr
    exact ⟨⟨r, hmem⟩, by simpa [bne_iff_ne] using hp⟩
  · rintro ⟨⟨r, hr⟩, hne⟩
    exact ⟨r, List.mem_filter.mpr ⟨hr, by simpa [bne_iff_ne] using hne⟩⟩

/-! ## Lemmas: the object types of a tenant -/

/-- Replacing the entry of one object type: the lookup of every type is as it was, except that the
    type replaced now holds `o'`. -/
theorem lookup_putType (l : List (TypeId × ObjType)) (ty k : TypeId) (o' : ObjType) :
    (putTypeList l ty o').lookup k =
      if k == ty then (l.lookup k).map (fun _ => o') else l.lookup k := by
  induction l with
  | nil => simp [putTypeList, List.lookup]
  | cons e es ih =>
    obtain ⟨j, p⟩ := e
    by_cases hkt : k = ty <;> by_cases hkj : k = j <;> by_cases hjt : j = ty <;>
      simp_all [putTypeList, List.lookup, beq_iff_eq] <;> (try split) <;> simp_all [beq_iff_eq]

theorem typ?_putType (t : Tenant) (ty k : TypeId) (o' : ObjType) :
    (t.putType ty o').typ? k = if k == ty then (t.typ? k).map (fun _ => o') else t.typ? k := by
  unfold Tenant.typ? Tenant.putType
  exact lookup_putType t.types ty k o'

theorem typ?_putType_other (t : Tenant) (ty k : TypeId) (o' : ObjType) (h : k ≠ ty) :
    (t.putType ty o').typ? k = t.typ? k := by
  rw [typ?_putType]
  have hk : (k == ty) = false := by simpa using h
  simp [hk]

theorem typ?_putType_self (t : Tenant) (ty : TypeId) (o o' : ObjType) (h : t.typ? ty = some o) :
    (t.putType ty o').typ? ty = some o' := by
  rw [typ?_putType]
  simp [h]

/-- The records of each type after a write that replaces one object type's entry: that type's
    records are `o'`'s, every other type's are as they were. -/
theorem env_putType (t : Tenant) (ty : TypeId) (o o' : ObjType) (h : t.typ? ty = some o)
    (k : TypeId) : (t.putType ty o').env k = if k == ty then o'.records else t.env k := by
  unfold Tenant.env
  rw [typ?_putType]
  by_cases hk : k == ty
  · rw [if_pos hk]
    have hk' : k = ty := by simpa using hk
    subst hk'
    simp [h]
  · rw [if_neg hk]
    simp [hk]

/-- Applying a per-type function to the entries of a type list, the lookup of a type applies the
    function to its own entry. -/
theorem lookup_map_entry (l : List (TypeId × ObjType)) (g : TypeId → ObjType → ObjType)
    (k : TypeId) :
    (l.map (fun e => (e.1, g e.1 e.2))).lookup k = (l.lookup k).map (g k) := by
  induction l with
  | nil => simp [List.lookup]
  | cons e es ih =>
    obtain ⟨j, o⟩ := e
    simp only [List.map_cons, List.lookup]
    cases hkj : k == j <;> simp_all

theorem typ?_afterDelete (t : Tenant) (ty id k : TypeId) :
    (t.afterDelete ty id).typ? k = (t.typ? k).map (delObj ty id k) := by
  unfold Tenant.afterDelete Tenant.typ?
  exact lookup_map_entry t.types (delObj ty id) k

/-- A delete removes the deleted record from its table and leaves every other record of every
    table, so the records of a type have the deleted record's id exactly when they had it and are
    not the deleted record. -/
theorem has_delObj_self (ty id : TypeId) (o : ObjType) (x : RecordId) :
    Records.has (delObj ty id ty o).records x = (Records.has o.records x && x != id) := by
  simp only [delObj, beq_self_eq_true, if_true]
  rw [has_delClear, has_filter_ne]

theorem has_delObj_other (ty id k : TypeId) (o : ObjType) (x : RecordId) (hk : (k == ty) = false) :
    Records.has (delObj ty id k o).records x = Records.has o.records x := by
  simp only [delObj, hk, Bool.false_eq_true, if_false, has_delClear]

/-- The records of a type after a delete: the deleted record gone from its own type, and every
    reference to it cleared. -/
theorem env_afterDelete_has (t : Tenant) (ty : TypeId) (id : RecordId) (k : TypeId) (x : RecordId) :
    Records.has ((t.afterDelete ty id).env k) x = (Records.has (t.env k) x && !(k == ty && x == id)) := by
  cases h : t.typ? k with
  | none =>
    have hL : (t.afterDelete ty id).env k = [] := by
      unfold Tenant.env; rw [typ?_afterDelete, h]; rfl
    have hR : t.env k = [] := by
      unfold Tenant.env; rw [h]; rfl
    rw [hL, hR]; simp [Records.has, List.lookup]
  | some o =>
    have hL : (t.afterDelete ty id).env k = (delObj ty id k o).records := by
      unfold Tenant.env; rw [typ?_afterDelete, h]; rfl
    have hR : t.env k = o.records := by
      unfold Tenant.env; rw [h]; rfl
    rw [hL, hR]
    by_cases hk : k = ty
    · subst hk
      rw [has_delObj_self]
      simp [bne]
    · have hk' : (k == ty) = false := by simpa using hk
      rw [has_delObj_other _ _ _ _ _ hk']
      simp [hk']

/-- A delete keeps every record of every type but the deleted one. -/
theorem env_afterDelete_inc (t : Tenant) (ty : TypeId) (idr : RecordId) (T : TypeId) (x : RecordId)
    (h : T ≠ ty ∨ x ≠ idr) (hx : Records.has (t.env T) x = true) :
    Records.has ((t.afterDelete ty idr).env T) x = true := by
  rw [env_afterDelete_has, hx]
  rcases h with h | h
  · have : (T == ty) = false := by simpa using h
    simp [this]
  · have : (x == idr) = false := by simpa using h
    simp [this]

theorem lookup_map_ty (l : List (FieldId × FieldDef))
    (g : FieldId × FieldDef → FieldId × FieldDef)
    (hg : ∀ e, (g e).1 = e.1 ∧ (g e).2.ty = e.2.ty) (k : FieldId) :
    ((l.map g).lookup k).map (fun d => d.ty) = (l.lookup k).map (fun d => d.ty) := by
  induction l with
  | nil => simp [List.lookup]
  | cons e es ih =>
    obtain ⟨j, d⟩ := e
    have hgj := hg (j, d)
    simp only [List.map_cons, List.lookup]
    rw [hgj.1]
    cases hkj : k == j
    · simp only [hkj]; exact ih
    · simp only [hkj, Option.map_some, Option.some.injEq]; exact hgj.2

theorem makeRequired_ty (s : Schema) (f k : FieldId) :
    ((makeRequired s f).lookup k).map (fun d => d.ty) = (s.lookup k).map (fun d => d.ty) :=
  lookup_map_ty s (fun fd => if fd.1 == f then (fd.1, { fd.2 with required := true }) else fd)
    (fun e => ⟨by dsimp only; split <;> rfl, by dsimp only; split <;> rfl⟩) k

theorem makeUnique_ty (s : Schema) (f k : FieldId) :
    ((makeUnique s f).lookup k).map (fun d => d.ty) = (s.lookup k).map (fun d => d.ty) :=
  lookup_map_ty s (fun fd => if fd.1 == f then (fd.1, { fd.2 with unique := true }) else fd)
    (fun e => ⟨by dsimp only; split <;> rfl, by dsimp only; split <;> rfl⟩) k

/-- A schema change that leaves every type's reference targets as they were leaves every reference
    resolving the same way. -/
theorem refsOk_congr_ty {env : Env} {s s' : Schema} {r : Record}
    (hty : ∀ k, (s'.lookup k).map (fun d => d.ty) = (s.lookup k).map (fun d => d.ty)) :
    refsOk env s' r = refsOk env s r := by
  unfold refsOk
  congr 1
  funext fv
  have h := hty fv.1
  cases h1 : s.lookup fv.1 <;> cases h2 : s'.lookup fv.1 <;> simp_all

/-! ## Lemmas: a write to one object type -/

/-- A write that replaces the records of one object type, keeping every record id, or that adds a
    record that resolves: referential integrity of the tenant. -/
theorem refsOk_putType {t : Tenant} {ty : TypeId} {o o' : ObjType}
    (ho : t.typ? ty = some o) (hs : o'.schema = o.schema) (hrefs : t.RefsOk)
    (hinc : ∀ k x, Records.has (t.env k) x = true → Records.has ((t.putType ty o').env k) x = true)
    (hnew : ∀ q ∈ o'.records, q ∈ o.records ∨ refsOk t.env o.schema q.2 = true) :
    (t.putType ty o').RefsOk := by
  intro T o'' hT q hq
  by_cases hTy : T = ty
  · subst hTy
    rw [typ?_putType_self t T o o' ho] at hT
    cases hT
    rw [hs]
    rcases hnew q hq with hmem | hres
    · exact refsOk_mono (fun k x => hinc k x) (hrefs T o ho q hmem)
    · exact refsOk_mono (fun k x => hinc k x) hres
  · rw [typ?_putType_other t ty T o' hTy] at hT
    exact refsOk_mono (fun k x => hinc k x) (hrefs T o'' hT q hq)

/-- A schema change that leaves every record and every reference's type as they were keeps
    referential integrity of the tenant. -/
theorem refsOk_putType_schema {t : Tenant} {ty : TypeId} {o o' : ObjType}
    (ho : t.typ? ty = some o) (hr : o'.records = o.records) (hty : ∀ k,
      (o'.schema.lookup k).map (fun d => d.ty) = (o.schema.lookup k).map (fun d => d.ty))
    (hrefs : t.RefsOk) : (t.putType ty o').RefsOk := by
  have henv : (t.putType ty o').env = t.env := by
    funext k
    rw [env_putType t ty o o' ho k]
    by_cases hk : k == ty
    · rw [if_pos hk, hr]
      have hk' : k = ty := by simpa using hk
      subst hk'
      unfold Tenant.env
      rw [ho]
      simp
    · rw [if_neg hk]
  intro T o'' hT q hq
  by_cases hTy : T = ty
  · subst hTy
    rw [typ?_putType_self t T o o' ho] at hT
    cases hT
    rw [refsOk_congr_ty hty, henv]
    have hq' : q ∈ o.records := by rw [← hr]; exact hq
    exact hrefs T o ho q hq'
  · rw [typ?_putType_other t ty T o' hTy] at hT
    rw [henv]
    exact hrefs T o'' hT q hq


/-! ## Lemmas: the clearing of a delete -/

theorem fieldRefers_of_refersTo_false {schema : Schema} {T : TypeId} {id : RecordId}
    {fv : FieldId × Val} (h : fv.2.refersTo id = false) : fieldRefers schema T id fv = false := by
  unfold fieldRefers
  cases schema.lookup fv.1 with
  | none => rfl
  | some d => simp [h]

/-- After the references to `id` of type `T` are cleared from a record, no field of it names
    record `id` of type `T`. -/
theorem fieldRefers_clearRef (schema : Schema) (T : TypeId) (id : RecordId) (r : Record) :
    ∀ fv ∈ (clearRef schema T id r).fields, fieldRefers schema T id fv = false := by
  intro fv hfv
  unfold clearRef at hfv
  split at hfv
  · rename_i hr
    obtain ⟨v, hmem, hc⟩ := mem_clearFields hfv
    rcases hc with ⟨hf, hfv'⟩ | ⟨-, hd⟩
    · have hfv2 : fv = (fv.1, v) := Prod.ext rfl hfv'
      rw [hfv2]
      exact hf
    · exact fieldRefers_of_refersTo_false (dropRef_refersTo hd)
  · rename_i hn
    have hf : fieldRefers schema T id fv = false := by
      by_cases hc' : fieldRefers schema T id fv = true
      · exact absurd (by unfold recRefers; exact List.any_eq_true.mpr ⟨fv, hfv, hc'⟩) hn
      · simpa using hc'
    exact hf

/-- A record that no record blocks, and that is not the deleted record, conforms after the
    clearing. -/
theorem conforms_afterDelete (t : Tenant) (ty : TypeId) (id : RecordId) (k : TypeId)
    (o : ObjType) (q : RecordId × Record) (ho : t.typ? k = some o) (hq : q ∈ o.records)
    (hne : ¬ (k == ty && q.1 == id)) (hnb : t.blockedDelete ty id = false)
    (hc : conforms o.schema q.2 = true) : conforms o.schema (clearRef o.schema ty id q.2) = true := by
  apply conforms_clear _ hc
  intro fv hfv hv
  have h1 : o.records.any (blocks k o.schema ty id) = false := by
    have := (List.any_eq_false.mp hnb) (k, o) (lookup_mem ho)
    simpa using this
  have hblock : blocks k o.schema ty id q = false := by
    simpa using (List.any_eq_false.mp h1) q hq
  unfold blocks at hblock
  have hne' : (!(k == ty && q.1 == id)) = true := by
    cases hb : (k == ty && q.1 == id)
    · rfl
    · exact absurd hb hne
  rw [hne', Bool.true_and] at hblock
  have hfvb := (List.any_eq_false.mp hblock) fv hfv
  simp only [hv, Bool.true_and, Bool.not_eq_true] at hfvb
  exact hfvb

/-! ## Lemmas: the steps that delete -/

/-- An accepted delete leaves the tenant it computes: the record gone from its table, and every
    reference to it cleared. -/
theorem stepTenant_delete_eq {t t' : Tenant} {p : PrincipalId} {ty : TypeId} {id : RecordId}
    {ev : Option Nat} (h : stepTenant t p (.delete ty id ev) = .ok t') :
    t' = t.afterDelete ty id := by
  simp only [stepTenant] at h
  repeat' split at h
  all_goals first
    | exact (Except.ok.inj h).symm
    | cases h

/-! ## The slice's properties -/

/-- Referential integrity is kept by every accepted step on a tenant, strict or not: a write names
    only existing records of the types its fields name, single or in a list; a delete is refused or
    clears what named the deleted record in every type; and a schema change touches no record and
    changes no reference's type. -/
theorem stepTenant_preserves_refs (t t' : Tenant) (p : PrincipalId) (i : Intent)
    (hrefs : t.RefsOk) (h : stepTenant t p i = .ok t') : t'.RefsOk := by
  cases i with
  | create ty fs =>
    cases hT : t.typ? ty with
    | none => simp [stepTenant, hT] at h
    | some o =>
      simp only [stepTenant, hT] at h
      split at h
      · cases h
      · cases hP : problem t.limits o.schema o.uniques true t.env o.records t.nextId
            (newRecord o.schema p fs) with
        | some e => simp [hP] at h
        | none =>
          simp only [hP] at h
          have ht := Except.ok.inj h
          subst ht
          have hinc : ∀ k x, Records.has (t.env k) x = true →
              Records.has ((t.putType ty { o with records := (t.nextId, newRecord o.schema p fs) ::
                o.records }).env k) x = true := by
            intro k x hx
            rw [env_putType t ty o _ hT k]
            by_cases hk : k == ty
            · rw [if_pos hk]
              have hk' : k = ty := by simpa using hk
              subst hk'
              have hr : t.env k = o.records := by
                unfold Tenant.env; rw [hT]; simp
              rw [hr] at hx
              exact has_cons_of _ hx
            · rw [if_neg hk]; exact hx
          have hnew : ∀ q ∈ ({ o with records := (t.nextId, newRecord o.schema p fs) ::
              o.records } : ObjType).records, q ∈ o.records ∨ refsOk t.env o.schema q.2 = true := by
            intro q hq
            rcases List.mem_cons.mp hq with rfl | hq'
            · right; exact (problem_none hP).2
            · left; exact hq'
          exact refsOk_putType hT rfl hrefs hinc hnew
  | update ty id fs ev =>
    cases hT : t.typ? ty with
    | none => simp [stepTenant, hT] at h
    | some o =>
      cases hL : o.records.lookup id with
      | none => simp [stepTenant, hT, hL, missingRecord] at h
      | some old =>
        simp only [stepTenant, hT, hL] at h
        split at h
        · cases h
        · split at h
          · cases h
          · cases hP : problem t.limits o.schema o.uniques true t.env o.records id
                (patched old fs) with
            | some e => simp [hP] at h
            | none =>
              simp only [hP] at h
              have ht := Except.ok.inj h
              subst ht
              have hinc : ∀ k x, Records.has (t.env k) x = true →
                  Records.has ((t.putType ty { o with records := o.records.map (fun q =>
                    if q.1 == id then (q.1, patched old fs) else q) }).env k) x = true := by
                intro k x hx
                rw [env_putType t ty o _ hT k]
                by_cases hk : k == ty
                · rw [if_pos hk]
                  have hk' : k = ty := by simpa using hk
                  subst hk'
                  have hr : t.env k = o.records := by
                    unfold Tenant.env; rw [hT]; simp
                  rw [hr] at hx
                  rw [has_replace]
                  exact hx
                · rw [if_neg hk]; exact hx
              have hnew : ∀ q ∈ ({ o with records := o.records.map (fun q =>
                  if q.1 == id then (q.1, patched old fs) else q) } : ObjType).records,
                  q ∈ o.records ∨ refsOk t.env o.schema q.2 = true := by
                intro q hq
                obtain ⟨q₀, hq₀, hmap⟩ := List.mem_map.mp hq
                by_cases hid : (q₀.1 == id) = true
                · simp only [hid, if_true] at hmap
                  right
                  rw [← hmap]
                  exact (problem_none hP).2
                · simp only [hid, if_false] at hmap
                  left
                  rw [← hmap]
                  exact hq₀
              exact refsOk_putType hT rfl hrefs hinc hnew
  | delete ty id ev =>
    have ht := stepTenant_delete_eq h
    subst ht
    intro T o'' hT q hq
    rw [typ?_afterDelete] at hT
    cases ho : t.typ? T with
    | none => rw [ho] at hT; cases hT
    | some o =>
      rw [ho] at hT
      simp only [Option.map_some, Option.some.injEq] at hT
      subst hT
      obtain ⟨q₀, hq₀, hmap⟩ := List.mem_map.mp hq
      subst hmap
      have hq₀' : q₀ ∈ o.records := by
        by_cases hTy : (T == ty) = true
        · rw [if_pos hTy] at hq₀
          exact (List.mem_filter.mp hq₀).1
        · rw [if_neg hTy] at hq₀
          exact hq₀
      exact refsOk_clear (fun k x hx hxk => env_afterDelete_inc t ty id k x hx hxk)
        (hrefs T o ho q₀ hq₀')
  | setRequired ty f strict =>
    cases hT : t.typ? ty with
    | none => simp only [stepTenant, hT] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hT] at h
      repeat' split at h
      all_goals try cases h
      exact refsOk_putType_schema hT rfl (fun k => makeRequired_ty o.schema f k) hrefs
  | setUnique ty f strict =>
    cases hT : t.typ? ty with
    | none => simp only [stepTenant, hT] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hT] at h
      repeat' split at h
      all_goals try cases h
      exact refsOk_putType_schema hT rfl (fun k => makeUnique_ty o.schema f k) hrefs
  | defineUnique ty fs =>
    cases hT : t.typ? ty with
    | none => simp only [stepTenant, hT] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hT] at h
      repeat' split at h
      all_goals try cases h
      exact refsOk_putType_schema hT rfl (fun k => rfl) hrefs

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
    required single reference, cannot be deleted: one who may delete it, naming no version or the
    record's version, is refused with `stillReferenced`. The referring record may be of any type. -/
theorem delete_refused_while_referenced (t : Tenant) (p : PrincipalId) (ty : TypeId)
    (id : RecordId) (ev : Option Nat) (o : ObjType) (old : Record) (S : TypeId) (so : ObjType)
    (q : RecordId × Record)
    (ho : t.typ? ty = some o) (hl : o.records.lookup id = some old) (hcan : canEdit t p old = true)
    (hv : versionClash ev old = false)
    (hS : t.typ? S = some so) (hq : q ∈ so.records) (hb : blocks S so.schema ty id q = true) :
    stepTenant t p (.delete ty id ev) = .error .stillReferenced := by
  have hany : t.blockedDelete ty id = true := by
    unfold Tenant.blockedDelete
    exact List.any_eq_true.mpr ⟨(S, so), lookup_mem hS, List.any_eq_true.mpr ⟨q, hq, hb⟩⟩
  simp [stepTenant, ho, hl, hcan, hv, hany]

/-- An accepted delete removes the record and leaves no record of any type naming it, through a
    field of the type it names, whatever the state was. -/
theorem delete_leaves_no_reference (t t' : Tenant) (p : PrincipalId) (ty : TypeId)
    (id : RecordId) (ev : Option Nat) (h : stepTenant t p (.delete ty id ev) = .ok t') :
    (∀ o', t'.typ? ty = some o' → Records.has o'.records id = false) ∧
      ∀ k o', t'.typ? k = some o' → ∀ q ∈ o'.records, ∀ fv ∈ q.2.fields,
        fieldRefers o'.schema ty id fv = false := by
  have ht := stepTenant_delete_eq h
  subst ht
  refine ⟨?_, ?_⟩
  · intro o' hT
    rw [typ?_afterDelete] at hT
    cases ho : t.typ? ty with
    | none => rw [ho] at hT; cases hT
    | some o =>
      rw [ho] at hT
      simp only [Option.map_some, Option.some.injEq] at hT
      subst hT
      unfold delObj
      simp only [beq_self_eq_true, if_true]
      rw [has_delClear, has_filter_ne]
      simp
  · intro k o' hT q hq fv hfv
    rw [typ?_afterDelete] at hT
    cases ho : t.typ? k with
    | none => rw [ho] at hT; cases hT
    | some o =>
      rw [ho] at hT
      simp only [Option.map_some, Option.some.injEq] at hT
      subst hT
      unfold delObj at hq
      obtain ⟨q₀, -, hmap⟩ := List.mem_map.mp (by unfold delClear at hq; exact hq)
      subst hmap
      exact fieldRefers_clearRef _ ty id q₀.2 fv hfv


/-! ## Schema changes that leave the records alone (typed) -/

/-- Writing one object type's entry with the same records leaves every type's records as they
    were. -/
theorem env_putType_records (t : Tenant) (ty : TypeId) (o o' : ObjType) (h : t.typ? ty = some o)
    (hr : o'.records = o.records) (k : TypeId) : (t.putType ty o').env k = t.env k := by
  rw [env_putType t ty o o' h k]
  by_cases hk : k == ty
  · rw [if_pos hk]
    have hk' : k = ty := by simpa using hk
    subst hk'
    simp [Tenant.env, h, hr]
  · rw [if_neg hk]

/-- A field made required leaves the counter and every type's records as they are. -/
theorem setRequired_records (t t' : Tenant) (p : PrincipalId) (ty : TypeId) (f : FieldId)
    (s : Bool) (h : stepTenant t p (.setRequired ty f s) = .ok t') :
    t'.nextId = t.nextId ∧ ∀ T, t'.env T = t.env T := by
  constructor
  · simp only [stepTenant] at h
    repeat' split at h
    all_goals (try cases h)
    all_goals rfl
  · intro T
    cases hT : t.typ? ty with
    | none => simp only [stepTenant, hT] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hT] at h
      repeat' split at h
      all_goals (try cases h)
      all_goals exact env_putType_records t ty o { o with schema := makeRequired o.schema f } hT rfl T

/-- A field made unique leaves the counter and every type's records as they are. -/
theorem setUnique_records (t t' : Tenant) (p : PrincipalId) (ty : TypeId) (f : FieldId)
    (s : Bool) (h : stepTenant t p (.setUnique ty f s) = .ok t') :
    t'.nextId = t.nextId ∧ ∀ T, t'.env T = t.env T := by
  constructor
  · simp only [stepTenant] at h
    repeat' split at h
    all_goals (try cases h)
    all_goals rfl
  · intro T
    cases hT : t.typ? ty with
    | none => simp only [stepTenant, hT] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hT] at h
      repeat' split at h
      all_goals (try cases h)
      all_goals exact env_putType_records t ty o { o with schema := makeUnique o.schema f } hT rfl T

/-- A composite unique constraint leaves the counter and every type's records as they are. -/
theorem defineUnique_records (t t' : Tenant) (p : PrincipalId) (ty : TypeId) (fs : List FieldId)
    (h : stepTenant t p (.defineUnique ty fs) = .ok t') :
    t'.nextId = t.nextId ∧ ∀ T, t'.env T = t.env T := by
  constructor
  · simp only [stepTenant] at h
    repeat' split at h
    all_goals (try cases h)
    all_goals rfl
  · intro T
    cases hT : t.typ? ty with
    | none => simp only [stepTenant, hT] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hT] at h
      repeat' split at h
      all_goals (try cases h)
      all_goals exact env_putType_records t ty o { o with uniques := o.uniques ++ [fs] } hT rfl T

end Strata
