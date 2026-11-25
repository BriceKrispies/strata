import Strata.Model
import Strata.References

/-!
Several object types per tenant (slice 4). An intent that names an object type the tenant does not
have is refused: `unknownObject` for the owner, who may act on every type, and `unauthorized` for
anyone else, so a caller who may not act on a type cannot tell whether it exists. A write whose
reference names a record that is not in the type the field names is never accepted; cross-type
integrity of stored records is `stepTenant_preserves_refs` and `step_preserves_refs` in
`Strata.References`, and cross-type deletes are `delete_refused_while_referenced` and
`delete_leaves_no_reference` there, stated for any referring type.
-/

namespace Strata

/-- An intent naming an object type the tenant does not have is refused with the same error for
    every record intent. -/
theorem unknown_type_refused (t : Tenant) (p : PrincipalId) (ty : TypeId) (h : t.typ? ty = none) :
    (∀ fs, stepTenant t p (.create ty fs) = .error (unknownType t p)) ∧
    (∀ id fs ev, stepTenant t p (.update ty id fs ev) = .error (unknownType t p)) ∧
    (∀ id ev, stepTenant t p (.delete ty id ev) = .error (unknownType t p)) :=
  ⟨fun fs => by simp [stepTenant, h], fun id fs ev => by simp [stepTenant, h],
    fun id ev => by simp [stepTenant, h]⟩

/-- A schema change naming an object type the tenant does not have is refused the same way: the
    owner learns `unknownObject`, anyone else `unauthorized`. -/
theorem unknown_type_schema_refused (t : Tenant) (p : PrincipalId) (ty : TypeId)
    (h : t.typ? ty = none) :
    (∀ f s, stepTenant t p (.setRequired ty f s) = .error (unknownType t p)) ∧
    (∀ f s, stepTenant t p (.setUnique ty f s) = .error (unknownType t p)) ∧
    (∀ fs, stepTenant t p (.defineUnique ty fs) = .error (unknownType t p)) := by
  refine ⟨fun f s => ?_, fun f s => ?_, fun fs => ?_⟩ <;>
    (by_cases hp : p = t.owner <;> simp [stepTenant, h, hp, unknownType])

/-- For the owner the refusal of an unknown type is `unknownObject`. -/
theorem unknown_type_owner (t : Tenant) : unknownType t t.owner = .unknownObject := by
  simp [unknownType]

/-- For anyone else the refusal of an unknown type is `unauthorized`. -/
theorem unknown_type_other (t : Tenant) (p : PrincipalId) (hp : p ≠ t.owner) :
    unknownType t p = .unauthorized := by
  simp [unknownType, hp]

/-- A caller who may neither create nor edit learns nothing about the object types: an unknown
    type and an existing one both refuse a create with `unauthorized`, and an update of a missing
    record in an existing type is refused with the same error as one in an unknown type. -/
theorem unknown_type_hidden (t : Tenant) (p : PrincipalId) (ty : TypeId) (fs : Fields)
    (id : RecordId) (ev : Option Nat) (hc : canCreate t p = false) (he : canEditAny t p = false) :
    (t.typ? ty = none → stepTenant t p (.create ty fs) = .error .unauthorized) ∧
    (∀ o, t.typ? ty = some o → stepTenant t p (.create ty fs) = .error .unauthorized) ∧
    (t.typ? ty = none → stepTenant t p (.update ty id fs ev) = .error .unauthorized) ∧
    (∀ o, t.typ? ty = some o → o.records.lookup id = none →
      stepTenant t p (.update ty id fs ev) = .error .unauthorized) := by
  have hp : p ≠ t.owner := by
    intro hpo; subst hpo; simp [canEditAny] at he
  refine ⟨fun h => ?_, fun o ho => ?_, fun h => ?_, fun o ho hl => ?_⟩
  · simp [stepTenant, h, unknownType, hp]
  · simp [stepTenant, ho, hc]
  · simp [stepTenant, h, unknownType, hp]
  · simp [stepTenant, ho, hl, missingRecord, he]

/-- The references of a record created in a tenant resolve in the types they name, if the
    create is accepted. -/
theorem create_refs_resolve (t t' : Tenant) (p : PrincipalId) (ty : TypeId) (fs : Fields)
    (o : ObjType) (ho : t.typ? ty = some o) (h : stepTenant t p (.create ty fs) = .ok t') :
    refsOk t.env o.schema (newRecord o.schema p fs) = true := by
  simp only [stepTenant, ho] at h
  split at h
  · cases h
  · cases hP : problem t.limits o.schema o.uniques true t.env o.records t.nextId
        (newRecord o.schema p fs) with
    | some e => simp [hP] at h
    | none => exact (problem_none hP).2

/-- The references of a record updated in a tenant resolve in the types they name, if the update
    is accepted. -/
theorem update_refs_resolve (t t' : Tenant) (p : PrincipalId) (ty : TypeId) (id : RecordId)
    (fs : Fields) (ev : Option Nat) (o : ObjType) (old : Record) (ho : t.typ? ty = some o)
    (hl : o.records.lookup id = some old)
    (h : stepTenant t p (.update ty id fs ev) = .ok t') :
    refsOk t.env o.schema (patched old fs) = true := by
  simp only [stepTenant, ho, hl] at h
  split at h
  · cases h
  · split at h
    · cases h
    · cases hP : problem t.limits o.schema o.uniques true t.env o.records id (patched old fs) with
      | some e => simp [hP] at h
      | none => exact (problem_none hP).2

/-- **Dangling writes are refused across types.** A create whose record holds a reference that
    does not name a record of the type its field names is never accepted. -/
theorem write_refused_dangling_across_types (t : Tenant) (p : PrincipalId) (ty : TypeId)
    (fs : Fields) (o : ObjType) (ho : t.typ? ty = some o)
    (fv : FieldId × Val) (hfv : fv ∈ (newRecord o.schema p fs).fields) (d : FieldDef)
    (hd : o.schema.lookup fv.1 = some d) (hbad : fv.2.resolves t.env d.ty = false) :
    ∀ t', stepTenant t p (.create ty fs) ≠ .ok t' := by
  intro t' h
  have hr := create_refs_resolve t t' p ty fs o ho h
  unfold refsOk at hr
  have := (List.all_eq_true.mp hr) fv hfv
  rw [hd] at this
  simp [hbad] at this

/-- **Dangling updates are refused across types.** An update whose patched record holds a
    reference that does not name a record of the type its field names is never accepted. -/
theorem update_refused_dangling_across_types (t : Tenant) (p : PrincipalId) (ty : TypeId)
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
