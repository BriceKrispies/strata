/-
The schema change in general (slice 11, part a): the changes of a `ChangeSchema` intent that define and remove
fields and object types, the abstract schema validity predicate and the check order of the kernel
(`transition::change_schema`, `plan_migration`, `apply_change`).

This file is the model of the change itself. It is not yet an intent of `step`: the intent constructor and the
executable kernels follow in the next part of slice 11 (see `docs/verification.md`). The model's `setRequired`,
`setUnique` and `defineUnique` are unchanged and are not yet special cases of `changeSchemaT`.
-/

import Strata.Uniqueness

namespace Strata

/-- One change of a schema change, the model's share of the kernel's `SchemaChange`.
    `defineField` defines a field (a new one, or a redefinition that replaces the definition of the field of that
    name); `removeField` removes a field and drops its stored values; `defineType` defines an object type with
    no fields and no records; `removeType` removes one, and is refused while it has records unless
    `deleteRecords` says they go with it. -/
inductive Change where
  | defineField (ty : TypeId) (f : FieldId) (d : FieldDef)
  | removeField (ty : TypeId) (f : FieldId)
  | defineType (ty : TypeId)
  | removeType (ty : TypeId) (deleteRecords : Bool)
  deriving Repr

/-- The kernel's limit on the changes of one schema change (`limits::MAX_SCHEMA_CHANGES`). -/
def maxChanges : Nat := 200

/-- The object types a change re-validates (the kernel's `touched_objects`): the type it names and, for a field
    that refers to another type, that type too. -/
def Change.touched : Change → List TypeId
  | .defineField ty _ d => ty :: d.ty.target?.toList
  | .removeField ty _ => [ty]
  | .defineType ty => [ty]
  | .removeType ty _ => [ty]

/-- A schema with field `f` defined as `d`: a redefinition replaces the definition in place, a new field is last. -/
def setField (schema : Schema) (f : FieldId) (d : FieldDef) : Schema :=
  if schema.any (fun fd => fd.1 == f) then schema.map (fun fd => if fd.1 == f then (f, d) else fd)
  else schema ++ [(f, d)]

/-- The schema rules of `schema::problems` the model's changes can trigger, over the object types of a tenant:
    a field that refers to an object type the tenant does not have; a unique field that is a list; a
    composite unique constraint that names a field its type does not declare. This is the concrete instance of
    the abstract validity predicate (`valid` below): `true` when the schema is valid. -/
def schemaRulesOk (types : List (TypeId × ObjType)) : Bool :=
  types.all (fun e =>
    e.2.schema.all (fun fd => match fd.2.ty.target? with
      | some t => types.any (fun e' => e'.1 == t)
      | none => true) &&
    e.2.schema.all (fun fd => !(fd.2.unique && fd.2.ty.isList)) &&
    e.2.uniques.all (fun u => u.all (fun f => e.2.schema.any (fun fd => fd.1 == f))))

/-- Applies one change to the object types of a tenant. A change that does not apply is `invalidSchema`
    (the kernel's `apply_change` refusal): the type it names is missing (or, for a definition, already there), or
    the field it removes is not defined. Removing a type that has records is `migrationRejected` unless the change
    deletes them. Removing a field drops its stored values from every record of its type. -/
def applyChange (types : List (TypeId × ObjType)) : Change → Except Err (List (TypeId × ObjType))
  | .defineField ty f d => match types.lookup ty with
    | none => .error .invalidSchema
    | some o => .ok (putTypeList types ty { o with schema := setField o.schema f d })
  | .removeField ty f => match types.lookup ty with
    | none => .error .invalidSchema
    | some o =>
      if o.schema.any (fun fd => fd.1 == f) then
        .ok (putTypeList types ty
          { o with schema := o.schema.filter (fun fd => !(fd.1 == f)),
                   records := o.records.map (fun q =>
                     (q.1, { q.2 with fields := q.2.fields.filter (fun fv => !(fv.1 == f)) })) })
      else .error .invalidSchema
  | .defineType ty =>
    if types.any (fun e => e.1 == ty) then .error .invalidSchema
    else .ok (types ++ [(ty, { schema := [] })])
  | .removeType ty deleteRecords => match types.lookup ty with
    | none => .error .invalidSchema
    | some o =>
      if !deleteRecords && !o.records.isEmpty then .error .migrationRejected
      else .ok (types.filter (fun e => !(e.1 == ty)))

/-- The changes applied in order, then the validity predicate `valid` on the resulting object types. The first change
    that does not apply is refused, and the validity of the result is asked only once every change has applied (the
    kernel's `plan_migration`: `apply_change` for each change, then `schema::problems`). -/
def applyChanges (valid : List (TypeId × ObjType) → Bool) (types : List (TypeId × ObjType)) :
    List Change → Except Err (List (TypeId × ObjType))
  | [] => if valid types then .ok types else .error .invalidSchema
  | c :: cs => (applyChange types c) >>= fun types' => applyChanges valid types' cs

/-- The object types of a tenant whose records a schema change re-validates: those named by the changes, in their
    own order (`scopedTypes`). -/
def changesScope (types : List (TypeId × ObjType)) (cs : List Change) : List (TypeId × ObjType) :=
  scopedTypes types (cs.flatMap Change.touched)

/-- A schema change of a tenant (the kernel's `change_schema`), with the model's check order:
    1. the caller may manage the schema (`unauthorized`);
    2. an expected version that is not the schema version (`schemaConflict`);
    3. no changes (`invalidIntent`), or more than `maxChanges` (`limitExceeded`);
    4. the changes apply in order and the result is valid (`applyChanges`, `invalidSchema`, `migrationRejected`);
    5. a strict change (`Reject`) is refused (`migrationRejected`) when a record of a touched type violates the schema
       the change leaves; a lax change (`Grandfather`) keeps the records.
    On acceptance the schema version rises by one; the records of the touched types are otherwise as they were. -/
def changeSchemaT (valid : List (TypeId × ObjType) → Bool) (t : Tenant) (p : Principal) (strict : Bool)
    (expected : Option Nat) (cs : List Change) : Except Err Tenant :=
  if !canManage t p then .error .unauthorized
  else if expected.any (fun v => v != t.schemaVersion) then .error .schemaConflict
  else if cs.isEmpty then .error .invalidIntent
  else if maxChanges < cs.length then .error .limitExceeded
  else
    match applyChanges valid t.types cs with
    | .error e => .error e
    | .ok types' =>
      if schemaChangeRefused t.limits (changesScope types' cs) strict then .error .migrationRejected
      else .ok { t with types := types', schemaVersion := t.schemaVersion + 1 }

/-- The records of object type `ty` in a list of object types (the environment of `Tenant.env`). -/
def recordsIn (types : List (TypeId × ObjType)) : Env := fun ty => ((types.lookup ty).map ObjType.records).getD []

/-- The number of records of the touched types that the schema the change leaves violates: the grandfathered count of
    a lax change (`violatedCount`). Zero when the change is refused. -/
def changeGrandfathered (valid : List (TypeId × ObjType) → Bool) (t : Tenant) (cs : List Change) : Nat :=
  match applyChanges valid t.types cs with
  | .error _ => 0
  | .ok types' => violatedCount t.limits (changesScope types' cs) (recordsIn types')

end Strata
