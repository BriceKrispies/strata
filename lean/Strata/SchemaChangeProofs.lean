/-
Theorems about the schema change in general (slice 11), over `Strata/SchemaChange.lean`. The lemmas that the state
invariants use are in `Strata/SchemaChangeInv.lean`.
-/

import Strata.Uniqueness

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
    changes leave (`applyChanges`) with the records of the touched types rewritten (`rewriteTypes`). -/
theorem changeSchemaT_ok (valid : List (TypeId × ObjType) → Bool) (t t' : Tenant) (p : Principal)
    (strict : Bool) (expected : Option Nat) (cs : List Change)
    (h : changeSchemaT valid t p strict expected cs = .ok t') :
    t'.schemaVersion = t.schemaVersion + 1 ∧
      ∃ types1, applyChanges valid t.types cs = .ok types1 ∧
        t'.types = rewriteTypes t.types (cs.flatMap Change.touched) types1 := by
  obtain ⟨types1, hap, -, rfl, -, -⟩ := changeSchemaT_parts h
  exact ⟨rfl, types1, hap, rfl⟩

/-- An accepted change leaves object types the validity predicate accepts, before the records are rewritten. With the
    concrete rules of `schemaRulesOk`, the schema the change leaves follows them: a reference names an object type of
    the tenant, a unique field is not a list, a composite constraint names declared fields, a constraint and a workflow
    read declared fields. -/
theorem applyChanges_valid (valid : List (TypeId × ObjType) → Bool) :
    ∀ (types : List (TypeId × ObjType)) (cs : List Change) (r : List (TypeId × ObjType)),
      applyChanges valid types cs = .ok r → valid r = true :=
  sc_applyChanges_valid valid

theorem changeSchemaT_valid (valid : List (TypeId × ObjType) → Bool) (t t' : Tenant) (p : Principal)
    (strict : Bool) (expected : Option Nat) (cs : List Change)
    (h : changeSchemaT valid t p strict expected cs = .ok t') :
    ∃ types1, valid types1 = true ∧ t'.types = rewriteTypes t.types (cs.flatMap Change.touched) types1 := by
  obtain ⟨_, types1, hap, hty⟩ := changeSchemaT_ok valid t t' p strict expected cs h
  exact ⟨types1, applyChanges_valid valid t.types cs types1 hap, hty⟩

/-- Rewriting the records changes no schema rule. -/
theorem schemaRulesOk_rewrite (ps : List Policy) (orig : List (TypeId × ObjType)) (touched : List TypeId)
    (types : List (TypeId × ObjType)) :
    schemaRulesOk ps (rewriteTypes orig touched types) = schemaRulesOk ps types := by
  have hk : ∀ X, (rewriteTypes orig touched types).any (fun e => e.1 == X) = types.any (fun e => e.1 == X) := by
    intro X
    rw [rewriteTypes_eq, List.any_map]
    rfl
  have hp : ∀ pl, policyOk (rewriteTypes orig touched types) pl = policyOk types pl := by
    intro pl
    unfold policyOk
    cases pl.resource with
    | object T =>
      simp only [sc_lookup_rewriteTypes]
      cases types.lookup T with
      | none => rfl
      | some o =>
        obtain ⟨hs, -⟩ := rwObj_record_view orig touched T o
        simp only [Option.map_some, hs]
    | anyObject => rfl
    | schema => rfl
    | members => rfl
  have hp' : policyOk (rewriteTypes orig touched types) = policyOk types := funext hp
  unfold schemaRulesOk
  simp only [hk, hp']
  rw [rewriteTypes_eq, List.all_map]
  have key : ∀ (f g : TypeId × ObjType → Bool), (∀ e, f e = g e) → types.all f = types.all g := by
    intro f g h; rw [funext h]
  rw [key _ _ (fun e => ?_)]
  obtain ⟨hs, hu, hc, hw, -⟩ := rwObj_record_view orig touched e.1 e.2
  simp only [Function.comp, hs, hu, hc, hw]

/-- The concrete instance: an accepted change leaves a schema that `schemaRulesOk` accepts. -/
theorem changeSchemaT_rules (t t' : Tenant) (p : Principal) (strict : Bool) (expected : Option Nat)
    (cs : List Change) (h : changeSchemaT (schemaRulesOk t.policies) t p strict expected cs = .ok t') :
    schemaRulesOk t.policies t'.types = true := by
  obtain ⟨types1, hv, hty⟩ := changeSchemaT_valid _ t t' p strict expected cs h
  rw [hty, schemaRulesOk_rewrite]
  exact hv

/-- An accepted strict change re-validated the types it touches: the violation check of the change came out false
    over the object types the changes leave (`changesScope`), and so did the check of the references. -/
theorem changeSchemaT_strict_not_violated (valid : List (TypeId × ObjType) → Bool) (t t' : Tenant)
    (p : Principal) (expected : Option Nat) (cs : List Change)
    (h : changeSchemaT valid t p true expected cs = .ok t') :
    ∃ types1, applyChanges valid t.types cs = .ok types1 ∧
      schemaViolated t.limits (changesScope t'.types cs) = false ∧
      refsViolated (changesScope t'.types cs) (recordsIn t'.types) = false := by
  obtain ⟨types1, hap, -, rfl, hch, -⟩ := changeSchemaT_parts h
  refine ⟨types1, hap, ?_⟩
  have hch' : (schemaViolated t.limits (changesScope (rewriteTypes t.types (cs.flatMap Change.touched) types1) cs)
      || refsViolated (changesScope (rewriteTypes t.types (cs.flatMap Change.touched) types1) cs)
        (recordsIn (rewriteTypes t.types (cs.flatMap Change.touched) types1))) = false := by
    simpa [changeRefused] using hch
  rw [Bool.or_eq_false_iff] at hch'
  exact hch'

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
  obtain ⟨_, _, hv, -⟩ := (changeSchemaT_strict_not_violated valid t t' p expected cs h)
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
    (h : l.lookup ty ≠ none) : (putTypeList l ty o).lookup ty = some o :=
  sc_lookup_putTypeList_self l ty o h

/-! ## Removing a field -/

/-- Removing a field drops its definition from its object type; the records are rewritten afterwards. -/
theorem removeField_drops (types types' : List (TypeId × ObjType)) (ty : TypeId) (f : FieldId) (o : ObjType)
    (ho : types.lookup ty = some o) (h : applyChange types (.removeField ty f) = .ok types') :
    ∀ o', types'.lookup ty = some o' → (∀ fd ∈ o'.schema, !(fd.1 == f) = true) ∧ o'.records = o.records := by
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
    · rfl
  · simp at h

/-- The fields of a stripped record are declared. -/
theorem stripRecord_declared (schema : Schema) (r : Record) :
    ∀ fv ∈ (stripRecord schema r).fields, declaredIn schema fv.1 = true := by
  intro fv hfv
  unfold stripRecord at hfv
  split at hfv
  · rename_i hlen
    have hsub : (r.fields.filter (fun fv => schema.any (fun fd => fd.1 == fv.1))).Sublist r.fields :=
      List.filter_sublist
    have hlen' : (r.fields.filter (fun fv => schema.any (fun fd => fd.1 == fv.1))).length = r.fields.length :=
      beq_iff_eq.mp hlen
    have heq := hsub.eq_of_length hlen'
    rw [← heq] at hfv
    exact by simpa [declaredIn] using (List.mem_filter.mp hfv).2
  · exact by simpa [declaredIn] using (List.mem_filter.mp hfv).2

/-- An accepted schema change of one `removeField` drops the stored values of the field from every record of the type. -/
theorem changeSchemaT_removeField (valid : List (TypeId × ObjType) → Bool) (t t' : Tenant) (p : Principal)
    (strict : Bool) (expected : Option Nat) (ty : TypeId) (f : FieldId)
    (h : changeSchemaT valid t p strict expected [.removeField ty f] = .ok t') :
    ∀ o', t'.typ? ty = some o' → ∀ q ∈ o'.records, ∀ fv ∈ q.2.fields, !(fv.1 == f) = true := by
  obtain ⟨_, types1, hap, hty⟩ := changeSchemaT_ok valid t t' p strict expected [.removeField ty f] h
  obtain ⟨r', hr', hr_eq, _⟩ := applyChanges_singleton valid t.types types1 _ hap
  obtain ⟨o, ho⟩ : ∃ o, t.types.lookup ty = some o := by
    cases hh : t.types.lookup ty with
    | none => simp [applyChange, hh] at hr'
    | some o => exact ⟨o, rfl⟩
  intro o' ho' q hq fv hfv
  have hlook : (rewriteTypes t.types ([Change.removeField ty f].flatMap Change.touched) types1).lookup ty = some o' := by
    have h1 := ho'
    unfold Tenant.typ? at h1
    rw [hty] at h1
    exact h1
  rw [sc_lookup_rewriteTypes] at hlook
  have hl1 : types1.lookup ty = some (match types1.lookup ty with | some x => x | none => o) := by
    cases hh : types1.lookup ty with
    | none => rw [hh] at hlook; simp at hlook
    | some x => rfl
  cases hh : types1.lookup ty with
  | none => rw [hh] at hlook; simp at hlook
  | some o1 =>
    rw [hh] at hlook
    simp only [Option.map_some, Option.some.injEq] at hlook
    subst hlook
    have hdrops := (removeField_drops t.types r' ty f o ho hr' o1 (by rw [← hr_eq]; exact hh)).1
    have htouch : ty ∈ [Change.removeField ty f].flatMap Change.touched := by simp [Change.touched]
    have horig : t.types.any (fun e0 => e0.1 == ty) = true := sc_any_of_lookup _ _ _ ho
    have hrw : rwObj t.types ([Change.removeField ty f].flatMap Change.touched) ty o1 =
        { o1 with records := o1.records.map (fun q => (q.1, stripRecord o1.schema q.2)) } := by
      unfold rwObj
      rw [if_pos]
      simp only [Bool.and_eq_true]
      exact ⟨by simpa using htouch, horig⟩
    rw [hrw] at hq
    obtain ⟨q1, -, rfl⟩ := List.mem_map.mp hq
    have hd := stripRecord_declared o1.schema q1.2 fv hfv
    unfold declaredIn at hd
    obtain ⟨fd, hfd, hfdk⟩ := List.any_eq_true.mp hd
    have h1 := hdrops fd hfd
    have : fd.1 = fv.1 := by simpa using hfdk
    rw [this] at h1
    exact h1

/-! ## Removing an object type -/

/-- A removal of a type with records, without deleting them, is refused: the change list is `migrationRejected`. -/
theorem removeType_refused_with_records (types : List (TypeId × ObjType)) (ty : TypeId) (o : ObjType)
    (ho : types.lookup ty = some o) (hr : o.records ≠ []) :
    removalsRefused types [.removeType ty false] = true := by
  simp [removalsRefused, ho, hr]

/-- An accepted removal of a type without deletion of its records had no records. -/
theorem removeType_ok_empty (valid : List (TypeId × ObjType) → Bool) (t t' : Tenant) (p : Principal)
    (strict : Bool) (expected : Option Nat) (ty : TypeId) (o : ObjType)
    (ho : t.types.lookup ty = some o)
    (h : changeSchemaT valid t p strict expected [.removeType ty false] = .ok t') : o.records = [] := by
  obtain ⟨-, -, -, -, -, hrem⟩ := changeSchemaT_parts h
  by_cases hne : o.records = []
  · exact hne
  · rw [removeType_refused_with_records t.types ty o ho hne] at hrem
    cases hrem

theorem lookup_filter_removed (l : List (TypeId × ObjType)) (ty : TypeId) :
    (l.filter (fun e => !(e.1 == ty))).lookup ty = none := by
  rw [sc_lookup_filter]; simp

/-- Removing a type and its records is accepted, and then the type is gone. -/
theorem removeType_deleted_gone (types types' : List (TypeId × ObjType)) (ty : TypeId) (o : ObjType)
    (ho : types.lookup ty = some o) (h : applyChange types (.removeType ty true) = .ok types') :
    types'.lookup ty = none := by
  simp only [applyChange, ho, Except.ok.injEq] at h
  subst h
  exact lookup_filter_removed types ty

/-! ## Defining an object type -/

/-- Defining an object type the tenant has leaves it as it is. -/
theorem defineType_existing (types : List (TypeId × ObjType)) (ty : TypeId)
    (h : types.any (fun e => e.1 == ty) = true) : applyChange types (.defineType ty) = .ok types := by
  simp [applyChange, h]


/-! ## The single-type intents are changes of the general form

`setRequired` and `setUnique` write the schema that a `defineField` of the field's redefinition writes: the definition
the field has, or the plain optional field when its type lacks it, with `required` or `unique` set. This is how the
kernel's harness maps them (`ChangeSchema [DefineField ..]`). The two differ in what they check around the write:
the single-type intents ask `schemaChangeRefused` over the type and the type its field names, the general change the
schema rules, the impact rule, the rewrite of the records and the check of the references as well. -/

theorem sc_lookup_eq_of_nodup (s : Schema) (hnd : (s.map Prod.fst).Nodup) (fd : FieldId × FieldDef) (h : fd ∈ s) :
    s.lookup fd.1 = some fd.2 := by
  induction s with
  | nil => simp at h
  | cons e es ih =>
    obtain ⟨k, v⟩ := e
    obtain ⟨hk, hrest⟩ := List.nodup_cons.mp hnd
    rcases List.mem_cons.mp h with rfl | h'
    · simp [List.lookup]
    · have hne : fd.1 ≠ k := by
        intro heq
        apply hk
        rw [← heq]
        exact List.mem_map.mpr ⟨fd, h', rfl⟩
      have hb : (fd.1 == k) = false := by simpa using hne
      simp only [List.lookup, hb]
      exact ih hrest h'

theorem sc_lookup_none_not_key (s : Schema) (f : FieldId) (h : s.lookup f = none) :
    ∀ fd ∈ s, (fd.1 == f) = false := by
  induction s with
  | nil => intro fd hfd; simp at hfd
  | cons e es ih =>
    obtain ⟨k, v⟩ := e
    by_cases hkf : f = k
    · subst hkf; simp [List.lookup] at h
    · have hb : (f == k) = false := by simpa using hkf
      simp only [List.lookup, hb] at h
      intro fd hfd
      rcases List.mem_cons.mp hfd with rfl | hfd'
      · have : k ≠ f := fun e => hkf e.symm
        simpa using this
      · exact ih h fd hfd'

theorem sc_absent_not_any (s : Schema) (f : FieldId) (hd : s.lookup f = none) :
    ¬ s.any (fun fd => fd.1 == f) = true := by
  intro h
  obtain ⟨fd, hfd, hk⟩ := List.any_eq_true.mp h
  have := sc_lookup_none_not_key s f hd fd hfd
  rw [this] at hk
  cases hk

/-- A field its type declares: `setRequired` writes the schema of a `defineField` of its definition made required. -/
theorem setRequired_schema_declared (s : Schema) (f : FieldId) (d : FieldDef) (hnd : (s.map Prod.fst).Nodup)
    (hd : s.lookup f = some d) : makeRequired (ensureField s f) f = setField s f { d with required := true } := by
  rw [ensureField_eq_of_lookup s f d hd]
  have hany : s.any (fun fd => fd.1 == f) = true := sc_declared_of_lookup s f d hd
  unfold setField makeRequired
  rw [if_pos hany]
  apply List.map_congr_left
  intro fd hfd
  by_cases hb : (fd.1 == f) = true
  · have hk : fd.1 = f := by simpa using hb
    have hl := sc_lookup_eq_of_nodup s hnd fd hfd
    rw [hk, hd] at hl
    have hdd : d = fd.2 := by simpa using hl
    simp only [hb, if_true]
    rw [← hk, ← hdd]
  · simp [hb]

/-- A field its type lacks: `setRequired` writes the schema of a `defineField` of the plain field made required. -/
theorem map_insertField (g : FieldId × FieldDef → FieldId × FieldDef) (hg : ∀ x, (g x).1 = x.1)
    (s : Schema) (e : FieldId × FieldDef) : (insertField s e).map g = insertField (s.map g) (g e) := by
  induction s with
  | nil => rfl
  | cons x xs ih =>
    unfold insertField
    by_cases h : e.1 < x.1
    · have h' : (g e).1 < (g x).1 := by rw [hg, hg]; exact h
      simp [h, h']
    · have h' : ¬ (g e).1 < (g x).1 := by rw [hg, hg]; exact h
      simp [h, h', ih]

theorem setRequired_schema_absent (s : Schema) (f : FieldId) (hd : s.lookup f = none) :
    makeRequired (ensureField s f) f = setField s f { plainField with required := true } := by
  have hany := sc_absent_not_any s f hd
  have hnot := sc_lookup_none_not_key s f hd
  unfold ensureField setField makeRequired
  rw [if_neg hany, if_neg hany]
  rw [map_insertField _ (fun x => by dsimp only; split <;> rfl)]
  have h1 : s.map (fun fd => if fd.1 == f then (fd.1, { fd.2 with required := true }) else fd) = s.map id :=
    List.map_congr_left (fun fd hfd => by simp [hnot fd hfd])
  rw [h1, List.map_id]
  simp

/-- A field its type declares: `setUnique` writes the schema of a `defineField` of its definition made unique. -/
theorem setUnique_schema_declared (s : Schema) (f : FieldId) (d : FieldDef) (hnd : (s.map Prod.fst).Nodup)
    (hd : s.lookup f = some d) : makeUnique (ensureField s f) f = setField s f { d with unique := true } := by
  rw [ensureField_eq_of_lookup s f d hd]
  have hany : s.any (fun fd => fd.1 == f) = true := sc_declared_of_lookup s f d hd
  unfold setField makeUnique
  rw [if_pos hany]
  apply List.map_congr_left
  intro fd hfd
  by_cases hb : (fd.1 == f) = true
  · have hk : fd.1 = f := by simpa using hb
    have hl := sc_lookup_eq_of_nodup s hnd fd hfd
    rw [hk, hd] at hl
    have hdd : d = fd.2 := by simpa using hl
    simp only [hb, if_true]
    rw [← hk, ← hdd]
  · simp [hb]

/-- A field its type lacks: `setUnique` writes the schema of a `defineField` of the plain field made unique. -/
theorem setUnique_schema_absent (s : Schema) (f : FieldId) (hd : s.lookup f = none) :
    makeUnique (ensureField s f) f = setField s f { plainField with unique := true } := by
  have hany := sc_absent_not_any s f hd
  have hnot := sc_lookup_none_not_key s f hd
  unfold ensureField setField makeUnique
  rw [if_neg hany, if_neg hany]
  rw [map_insertField _ (fun x => by dsimp only; split <;> rfl)]
  have h1 : s.map (fun fd => if fd.1 == f then (fd.1, { fd.2 with unique := true }) else fd) = s.map id :=
    List.map_congr_left (fun fd hfd => by simp [hnot fd hfd])
  rw [h1, List.map_id]
  simp

/-- The single-type intents write one object type, as the change writes it: with the schema of the redefinition. -/
theorem applyChange_defineField_putType (types : List (TypeId × ObjType)) (ty : TypeId) (f : FieldId)
    (d : FieldDef) (o : ObjType) (ho : types.lookup ty = some o) :
    applyChange types (.defineField ty f d) = .ok (putTypeList types ty { o with schema := setField o.schema f d }) := by
  simp [applyChange, ho]

end Strata
