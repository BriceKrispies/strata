/-
Theorems about the schema change in general (slice 11, part a), over `Strata/SchemaChange.lean`.
-/

import Strata.SchemaChange

namespace Strata

/-! ## The envelope -/

/-- A caller who may not manage the schema is refused `unauthorized`, whatever the changes. -/
theorem changeSchemaT_unauthorized (valid : List (TypeId × ObjType) → Bool) (t : Tenant) (p : Principal)
    (strict : Bool) (expected : Option Nat) (cs : List Change) (h : canManage t p = false) :
    changeSchemaT valid t p strict expected cs = .error .unauthorized := by
  simp [changeSchemaT, h]

/-- An expected version that is not the schema version is refused `schemaConflict` (the kernel's
    `change_schema`, checked before the changes are looked at). -/
theorem changeSchemaT_conflict (valid : List (TypeId × ObjType) → Bool) (t : Tenant) (p : Principal)
    (strict : Bool) (v : Nat) (cs : List Change) (hm : canManage t p = true)
    (hv : v ≠ t.schemaVersion) :
    changeSchemaT valid t p strict (some v) cs = .error .schemaConflict := by
  simp [changeSchemaT, hm, hv]

/-- No changes, or more than `maxChanges`, are refused (`invalidIntent`, `limitExceeded`). -/
theorem changeSchemaT_empty (valid : List (TypeId × ObjType) → Bool) (t : Tenant) (p : Principal)
    (strict : Bool) (expected : Option Nat) (hm : canManage t p = true)
    (he : expected.any (fun v => v != t.schemaVersion) = false) :
    changeSchemaT valid t p strict expected [] = .error .invalidIntent := by
  simp [changeSchemaT, hm, he]

theorem changeSchemaT_limit (valid : List (TypeId × ObjType) → Bool) (t : Tenant) (p : Principal)
    (strict : Bool) (expected : Option Nat) (cs : List Change) (hm : canManage t p = true)
    (he : expected.any (fun v => v != t.schemaVersion) = false) (hc : cs ≠ [])
    (hl : maxChanges < cs.length) :
    changeSchemaT valid t p strict expected cs = .error .limitExceeded := by
  simp [changeSchemaT, hm, he, hl]
  intro h
  exact absurd h (by simpa using hc)

/-! ## Accepted changes -/

/-- The changes apply in order: the list is folded from its head, each change on the object types the previous
    ones left, and the validity predicate is asked of the result. -/
theorem applyChanges_cons (valid : List (TypeId × ObjType) → Bool) (types : List (TypeId × ObjType))
    (c : Change) (cs : List Change) :
    applyChanges valid types (c :: cs) = (applyChange types c) >>= fun types' => applyChanges valid types' cs :=
  rfl

/-- An accepted schema change raises the schema version by exactly one, and its object types are the ones the
    changes leave, in order (`applyChanges`). -/
theorem changeSchemaT_ok (valid : List (TypeId × ObjType) → Bool) (t t' : Tenant) (p : Principal)
    (strict : Bool) (expected : Option Nat) (cs : List Change)
    (h : changeSchemaT valid t p strict expected cs = .ok t') :
    t'.schemaVersion = t.schemaVersion + 1 ∧
      ∃ types', applyChanges valid t.types cs = .ok types' ∧ t'.types = types' := by
  unfold changeSchemaT at h
  split at h
  · simp at h
  · split at h
    · simp at h
    · split at h
      · simp at h
      · split at h
        · simp at h
        · split at h
          · simp at h
          · rename_i types' hap
            split at h
            · simp at h
            · simp only [Except.ok.injEq] at h
              subst h
              exact ⟨rfl, types', hap, rfl⟩

/-- An accepted change leaves object types the validity predicate accepts. With the concrete rules of
    `schemaRulesOk`, the schema the change leaves follows them: a reference names an object type of the tenant, a
    unique field is not a list, a composite constraint names declared fields. -/
theorem applyChanges_valid (valid : List (TypeId × ObjType) → Bool) :
    ∀ (types : List (TypeId × ObjType)) (cs : List Change) (r : List (TypeId × ObjType)),
      applyChanges valid types cs = .ok r → valid r = true := by
  intro types cs
  induction cs generalizing types with
  | nil =>
    intro r h
    simp only [applyChanges] at h
    split at h
    · simp only [Except.ok.injEq] at h
      subst h
      assumption
    · simp at h
  | cons c cs ih =>
    intro r h
    simp only [applyChanges_cons] at h
    cases hc : applyChange types c with
    | error e =>
      rw [hc] at h
      change Except.error e = Except.ok r at h
      cases h
    | ok types' =>
      rw [hc] at h
      change applyChanges valid types' cs = Except.ok r at h
      exact ih types' r h

theorem changeSchemaT_valid (valid : List (TypeId × ObjType) → Bool) (t t' : Tenant) (p : Principal)
    (strict : Bool) (expected : Option Nat) (cs : List Change)
    (h : changeSchemaT valid t p strict expected cs = .ok t') :
    valid t'.types = true := by
  obtain ⟨_, types', hap, hty⟩ := changeSchemaT_ok valid t t' p strict expected cs h
  rw [hty]
  exact applyChanges_valid valid t.types cs types' hap

/-- The concrete instance: an accepted change leaves a schema that `schemaRulesOk` accepts. -/
theorem changeSchemaT_rules (t t' : Tenant) (p : Principal) (strict : Bool) (expected : Option Nat)
    (cs : List Change) (h : changeSchemaT schemaRulesOk t p strict expected cs = .ok t') :
    schemaRulesOk t'.types = true :=
  changeSchemaT_valid schemaRulesOk t t' p strict expected cs h

/-- An accepted strict change re-validated the types it touches: the violation check of the change came out false
    over the object types the changes leave (`changesScope`). -/
theorem changeSchemaT_strict_not_violated (valid : List (TypeId × ObjType) → Bool) (t t' : Tenant)
    (p : Principal) (expected : Option Nat) (cs : List Change)
    (h : changeSchemaT valid t p true expected cs = .ok t') :
    ∃ types', applyChanges valid t.types cs = .ok types' ∧
      schemaViolated t.limits (changesScope types' cs) = false := by
  unfold changeSchemaT at h
  split at h
  · simp at h
  · split at h
    · simp at h
    · split at h
      · simp at h
      · split at h
        · simp at h
        · split at h
          · simp at h
          · rename_i types' hap
            split at h
            · simp at h
            · rename_i hnot
              refine ⟨types', hap, ?_⟩
              simpa [schemaChangeRefused] using hnot

/-- A strict change (`Reject`) leaves the object types it re-validates conforming: every record of each of them
    conforms to the schema the change leaves, and its unique fields and composite constraints hold. -/
theorem changeSchemaT_strict_conforms (valid : List (TypeId × ObjType) → Bool) (t t' : Tenant) (p : Principal)
    (expected : Option Nat) (cs : List Change)
    (h : changeSchemaT valid t p true expected cs = .ok t') :
    ∀ e ∈ changesScope t'.types cs, (∀ q ∈ e.2.records, conforms e.2.schema q.2 = true) ∧
      (∀ fd ∈ e.2.schema, fd.2.unique = true → ∀ q₁ ∈ e.2.records, ∀ q₂ ∈ e.2.records,
        q₁.1 ≠ q₂.1 → agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
      ∀ u ∈ e.2.uniques, ∀ q₁ ∈ e.2.records, ∀ q₂ ∈ e.2.records,
        q₁.1 ≠ q₂.1 → agreeOn u q₁.2.fields q₂.2.fields = false := by
  obtain ⟨_, types', hap, hty⟩ := changeSchemaT_ok valid t t' p true expected cs h
  obtain ⟨types'', hap'', hv⟩ := changeSchemaT_strict_not_violated valid t t' p expected cs h
  rw [hap] at hap''
  cases hap''
  rw [hty]
  exact schemaViolated_false hv

/-- A change applied on its own: the object types it leaves are the result of the change, which the validity predicate
    accepts. -/
theorem applyChanges_singleton (valid : List (TypeId × ObjType) → Bool) (types r : List (TypeId × ObjType))
    (c : Change) (h : applyChanges valid types [c] = .ok r) :
    ∃ r', applyChange types c = .ok r' ∧ r = r' ∧ valid r' = true := by
  rw [applyChanges_cons] at h
  cases hc : applyChange types c with
  | error e =>
    rw [hc] at h
    change Except.error e = Except.ok r at h
    cases h
  | ok r' =>
    rw [hc] at h
    change applyChanges valid r' [] = Except.ok r at h
    simp only [applyChanges] at h
    split at h
    · rename_i hv
      simp only [Except.ok.injEq] at h
      exact ⟨r', rfl, h.symm, hv⟩
    · simp at h

theorem lookup_putTypeList_self (l : List (TypeId × ObjType)) (ty : TypeId) (o : ObjType)
    (h : l.lookup ty ≠ none) : (putTypeList l ty o).lookup ty = some o := by
  induction l with
  | nil => simp [List.lookup] at h
  | cons e es ih =>
    obtain ⟨k, p⟩ := e
    by_cases hk : k = ty
    · subst hk
      simp [putTypeList, List.lookup]
    · have hrest : es.lookup ty ≠ none := by
        intro hn
        apply h
        have hk2 : (ty == k) = false := by simp [Ne.symm hk]
        simp [List.lookup, hk, hk2, hn]
      have hk2 : (ty == k) = false := by simp [Ne.symm hk]
      simp [putTypeList, List.lookup, hk, hk2, ih hrest]

/-- Removing a field drops its definition from its object type and its stored values from every record of the type. -/
theorem removeField_drops (types types' : List (TypeId × ObjType)) (ty : TypeId) (f : FieldId) (o : ObjType)
    (ho : types.lookup ty = some o) (h : applyChange types (.removeField ty f) = .ok types') :
    ∀ o', types'.lookup ty = some o' →
      (∀ fd ∈ o'.schema, !(fd.1 == f) = true) ∧ ∀ q ∈ o'.records, ∀ fv ∈ q.2.fields, !(fv.1 == f) = true := by
  intro o' ho'
  simp only [applyChange, ho] at h
  split at h
  · rename_i hany
    simp only [Except.ok.injEq] at h
    subst h
    have hne : (types.lookup ty) ≠ none := by simp [ho]
    rw [lookup_putTypeList_self types ty _ hne] at ho'
    cases ho'
    constructor
    · intro fd hfd
      have := (List.mem_filter.mp hfd).2
      simpa using this
    · intro q hq fv hfv
      rcases List.mem_map.mp hq with ⟨q0, _, hq0⟩
      subst hq0
      have := (List.mem_filter.mp hfv).2
      simpa using this
  · simp at h

/-- A removal of a type with records is refused `migrationRejected` unless the change deletes them. -/
theorem removeType_refused_with_records (types : List (TypeId × ObjType)) (ty : TypeId) (o : ObjType)
    (ho : types.lookup ty = some o) (hr : o.records ≠ []) :
    applyChange types (.removeType ty false) = .error .migrationRejected := by
  simp [applyChange, ho, hr]

/-- An accepted removal of a type without deletion of its records had no records. -/
theorem removeType_ok_empty (types types' : List (TypeId × ObjType)) (ty : TypeId) (o : ObjType)
    (ho : types.lookup ty = some o) (h : applyChange types (.removeType ty false) = .ok types') :
    o.records = [] := by
  by_cases hne : o.records = []
  · exact hne
  · rw [removeType_refused_with_records types ty o ho hne] at h
    cases h

theorem lookup_filter_removed (l : List (TypeId × ObjType)) (ty : TypeId) :
    (l.filter (fun e => !(e.1 == ty))).lookup ty = none := by
  induction l with
  | nil => rfl
  | cons e es ih =>
    obtain ⟨k, p⟩ := e
    by_cases hk : k = ty
    · subst hk
      simp [List.filter_cons, ih]
    · have hk2 : (ty == k) = false := by simp [Ne.symm hk]
      simp [List.filter_cons, List.lookup, hk, hk2, ih]

/-- Removing a type and its records is accepted, and then the type is gone. -/
theorem removeType_deleted_gone (types types' : List (TypeId × ObjType)) (ty : TypeId) (o : ObjType)
    (ho : types.lookup ty = some o) (h : applyChange types (.removeType ty true) = .ok types') :
    types'.lookup ty = none := by
  simp only [applyChange, ho] at h
  split at h
  · cases h
  · cases h
    exact lookup_filter_removed types ty

/-- An accepted schema change of one `removeField` drops the stored values of the field from every record of the type. -/
theorem changeSchemaT_removeField (valid : List (TypeId × ObjType) → Bool) (t t' : Tenant) (p : Principal)
    (strict : Bool) (expected : Option Nat) (ty : TypeId) (f : FieldId)
    (h : changeSchemaT valid t p strict expected [.removeField ty f] = .ok t') :
    ∀ o', t'.typ? ty = some o' → ∀ q ∈ o'.records, ∀ fv ∈ q.2.fields, !(fv.1 == f) = true := by
  obtain ⟨_, types', hap, hty⟩ := changeSchemaT_ok valid t t' p strict expected [.removeField ty f] h
  obtain ⟨r', hr', hr_eq, _⟩ := applyChanges_singleton valid t.types types' _ hap
  obtain ⟨o, ho⟩ : ∃ o, t.types.lookup ty = some o := by
    cases hh : t.types.lookup ty with
    | none => simp [applyChange, hh] at hr'
    | some o => exact ⟨o, rfl⟩
  intro o' ho' q hq fv hfv
  have hlook : r'.lookup ty = some o' := by
    have h1 := ho'
    unfold Tenant.typ? at h1
    rw [hty, hr_eq] at h1
    exact h1
  exact (removeField_drops t.types r' ty f o ho hr' o' hlook).2 q hq fv hfv
