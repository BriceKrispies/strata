import Strata.Model
import Strata.References

/-!
Several object types per tenant (slice 4). A record intent that names an object type the tenant
does not have is refused: `unknownObject` for the owner, who may act on every type, and
`unauthorized` for anyone else, so a caller who may not act on a type cannot tell whether it
exists. A schema change that names such a type is refused with `invalidSchema` for the owner (the
kernel finds no type when it applies the change) and `unauthorized` for anyone else (the kernel
requires the schema permission first). A write whose
reference names a record that is not in the type the field names is never accepted; cross-type
integrity of stored records is `stepTenant_preserves_refs` and `step_preserves_refs` in
`Strata.References`, and cross-type deletes are `delete_refused_while_referenced` and
`delete_leaves_no_reference` there, stated for any referring type.
-/

namespace Strata

/-- An intent naming an object type the tenant does not have is refused with the same error for
    every record intent. -/
theorem unknown_type_refused (t : Tenant) (p : Principal) (ty : TypeId) (h : t.typ? ty = none) :
    (∀ fs, stepTenant t p (.create ty fs) = .error (unknownType t p ty .create)) ∧
    (∀ id fs ev, stepTenant t p (.update ty id fs ev) = .error (unknownType t p ty .update)) ∧
    (∀ id ev, stepTenant t p (.delete ty id ev) = .error (unknownType t p ty .delete)) :=
  ⟨fun fs => by simp [stepTenant, h], fun id fs ev => by simp [stepTenant, h],
    fun id ev => by simp [stepTenant, h]⟩

/-- A schema change naming an object type the tenant does not have. The kernel checks that the
    caller may manage the schema first (`unauthorized` for one who may not), then finds no such type
    when it applies the change (`invalid_schema`). -/
theorem unknown_type_schema_refused (t : Tenant) (p : Principal) (ty : TypeId)
    (h : t.typ? ty = none) :
    (canManage t p = true →
      (∀ f s, stepTenant t p (.setRequired ty f s) = .error .invalidSchema) ∧
      (∀ f s, stepTenant t p (.setUnique ty f s) = .error .invalidSchema) ∧
      (∀ fs, stepTenant t p (.defineUnique ty fs) = .error .invalidSchema)) ∧
    (canManage t p = false →
      (∀ f s, stepTenant t p (.setRequired ty f s) = .error .unauthorized) ∧
      (∀ f s, stepTenant t p (.setUnique ty f s) = .error .unauthorized) ∧
      (∀ fs, stepTenant t p (.defineUnique ty fs) = .error .unauthorized)) := by
  refine ⟨fun hp => ⟨fun f s => ?_, fun f s => ?_, fun fs => ?_⟩,
    fun hp => ⟨fun f s => ?_, fun f s => ?_, fun fs => ?_⟩⟩ <;>
    simp [stepTenant, h, hp]

/-- A holder of the owner role is refused an unknown type with `unknownObject`: it may act on every
    type. -/
theorem unknown_type_owner (t : Tenant) (p : Principal) (ty : TypeId) (a : Action)
    (hown : ownerRole ∈ t.rolesOf p) :
    unknownType t p ty a = .unknownObject := by
  have h : canWithoutRecord t p ty a = true :=
    (Tenant.allows_iff t p a (.object ty) (subjectEnv p none none)).mpr (Or.inl hown)
  simp [unknownType, h]

/-- Anyone who may not act on a type with the action is refused an unknown type with
    `unauthorized`, whether or not the type exists. -/
theorem unknown_type_other (t : Tenant) (p : Principal) (ty : TypeId) (a : Action)
    (hn : canWithoutRecord t p ty a = false) : unknownType t p ty a = .unauthorized := by
  simp [unknownType, hn]

/-- A caller who may neither create nor edit learns nothing about the object types: an unknown
    type and an existing one both refuse a create with `unauthorized`, and an update of a missing
    record in an existing type is refused with the same error as one in an unknown type. -/
theorem unknown_type_hidden (t : Tenant) (p : Principal) (ty : TypeId) (fs : Fields)
    (id : RecordId) (ev : Option Nat)
    (hc : ∀ r, canCreate t p ty r = false) (hn : canWithoutRecord t p ty .create = false)
    (he : canWithoutRecord t p ty .update = false) :
    (t.typ? ty = none → stepTenant t p (.create ty fs) = .error .unauthorized) ∧
    (∀ o, t.typ? ty = some o → (∃ r, newRecord o.schema p fs = .ok r) →
      stepTenant t p (.create ty fs) = .error .unauthorized) ∧
    (t.typ? ty = none → stepTenant t p (.update ty id fs ev) = .error .unauthorized) ∧
    (∀ o, t.typ? ty = some o → o.records.lookup id = none →
      stepTenant t p (.update ty id fs ev) = .error .unauthorized) := by
  refine ⟨fun h => ?_, fun o ho hn' => ?_, fun h => ?_, fun o ho hl => ?_⟩
  · simp [stepTenant, h, unknownType, hn]
  · obtain ⟨r, hr⟩ := hn'
    simp [stepTenant, ho, hr, hc]
  · simp [stepTenant, h, unknownType, he]
  · simp [stepTenant, ho, hl, missingRecord, he]

/-- The references of a record created in a tenant resolve in the types they name, if the
    create is accepted. -/
theorem create_refs_resolve (t t' : Tenant) (p : Principal) (ty : TypeId) (fs : Fields)
    (o : ObjType) (ho : t.typ? ty = some o) (h : stepTenant t p (.create ty fs) = .ok t') :
    ∃ r, newRecord o.schema p fs = .ok r ∧ refsOk t.env o.schema r = true := by
  obtain ⟨r, hN, hP, -, -⟩ := create_ok_parts ho h
  exact ⟨r, hN, (problem_none hP).2⟩

/-- The references of a record updated in a tenant resolve in the types they name, if the update
    is accepted. -/
theorem update_refs_resolve (t t' : Tenant) (p : Principal) (ty : TypeId) (id : RecordId)
    (fs : Fields) (ev : Option Nat) (o : ObjType) (old : Record) (ho : t.typ? ty = some o)
    (hl : o.records.lookup id = some old)
    (h : stepTenant t p (.update ty id fs ev) = .ok t') :
    refsOk t.env o.schema (patched old fs) = true := by
  obtain ⟨hP, -, -⟩ := update_ok_parts ho hl h
  exact (problem_none hP).2

/-- **Dangling writes are refused across types.** A create whose record holds a reference that
    does not name a record of the type its field names is never accepted. -/
theorem write_refused_dangling_across_types (t : Tenant) (p : Principal) (ty : TypeId)
    (fs : Fields) (o : ObjType) (ho : t.typ? ty = some o) (r : Record)
    (hN : newRecord o.schema p fs = .ok r)
    (fv : FieldId × Val) (hfv : fv ∈ r.fields) (d : FieldDef)
    (hd : o.schema.lookup fv.1 = some d) (hbad : fv.2.resolves t.env d.ty = false) :
    ∀ t', stepTenant t p (.create ty fs) ≠ .ok t' := by
  intro t' h
  obtain ⟨r', hN', hr⟩ := create_refs_resolve t t' p ty fs o ho h
  have hrr : r = r' := Except.ok.inj (hN.symm.trans hN')
  subst hrr
  unfold refsOk at hr
  have := (List.all_eq_true.mp hr) fv hfv
  rw [hd] at this
  simp [hbad] at this
/-- **Dangling updates are refused across types.** An update whose patched record holds a
    reference that does not name a record of the type its field names is never accepted. -/
theorem update_refused_dangling_across_types (t : Tenant) (p : Principal) (ty : TypeId)
    (id : RecordId) (fs : Fields) (ev : Option Nat) (o : ObjType) (old : Record)
    (ho : t.typ? ty = some o) (hl : o.records.lookup id = some old)
    (fv : FieldId × Val) (hfv : fv ∈ (patched old fs).fields) (d : FieldDef)
    (hd : o.schema.lookup fv.1 = some d) (hbad : fv.2.resolves t.env d.ty = false) :
    ∀ t', stepTenant t p (.update ty id fs ev) ≠ .ok t' := by
  intro t' h
  have hr := update_refs_resolve t t' p ty id fs ev o old ho hl h
  unfold refsOk at hr
  have hall := (List.all_eq_true.mp hr) fv hfv
  rw [hd] at hall
  simp [hbad] at hall

end Strata
