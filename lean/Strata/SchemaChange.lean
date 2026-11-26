/-
The schema change in general (slice 11): the changes of a `ChangeSchema` intent that define and remove
fields and object types, the schema validity rules and the check order of the kernel
(`transition::change_schema`, `plan_migration`, `apply_change`).

This file holds the definitions the intent of `step` is made from (`Intent.changeSchema` in
`Strata/Model.lean` calls `changeSchemaT`); the theorems are in `Strata/SchemaChangeProofs.lean` and
`Strata/SchemaChangeInv.lean`.
-/

import Strata.ModelBase

namespace Strata

/-- One change of a schema change, the model's share of the kernel's `SchemaChange`.
    `defineField` defines a field (a new one, or a redefinition that replaces the definition of the field of that
    name); `removeField` removes a field and drops its stored values; `defineType` defines an object type with
    no fields and no records (a type that exists is left as it is: the kernel's `DefineObject` then only changes
    the label, which the model does not hold); `removeType` removes one, and is refused while it has records
    unless `deleteRecords` says they go with it. -/
inductive Change where
  | defineField (ty : TypeId) (f : FieldId) (d : FieldDef)
  | removeField (ty : TypeId) (f : FieldId)
  | defineType (ty : TypeId)
  | removeType (ty : TypeId) (deleteRecords : Bool)
  deriving Repr

/-- The kernel's limit on the changes of one schema change (`limits::MAX_SCHEMA_CHANGES`). -/
def maxChanges : Nat := 200

/-- The object type a change names. -/
def Change.names : Change → TypeId
  | .defineField ty _ _ => ty
  | .removeField ty _ => ty
  | .defineType ty => ty
  | .removeType ty _ => ty

/-- The object types a change re-validates (the kernel's `touched_objects`): the type a field change names and, for a
    field that refers to another type, that type too. A change of an object type itself re-validates no records. -/
def Change.touched : Change → List TypeId
  | .defineField ty _ d => ty :: d.ty.target?.toList
  | .removeField ty _ => [ty]
  | .defineType _ => []
  | .removeType _ _ => []

/-- A summary line of a schema change, by ids: the kernel's `apply_change` line for a field definition
    (`define field <object>.<field>` or `redefine field ...`), a removed field, a defined or removed object type, or
    a composite constraint (`define unique constraint <object>`). -/
inductive SchemaLine where
  | field (ty : TypeId) (f : FieldId)
  | unique (ty : TypeId) (fs : List FieldId)
  | removeField (ty : TypeId) (f : FieldId)
  | defineType (ty : TypeId)
  | removeType (ty : TypeId)
  deriving DecidableEq, Repr

/-- The summary line of a change. -/
def Change.line : Change → SchemaLine
  | .defineField ty f _ => .field ty f
  | .removeField ty f => .removeField ty f
  | .defineType ty => .defineType ty
  | .removeType ty _ => .removeType ty

/-- A schema with field `f` defined as `d`: a redefinition replaces the definition in place, a new field goes at its place by id. -/
def setField (schema : Schema) (f : FieldId) (d : FieldDef) : Schema :=
  if schema.any (fun fd => fd.1 == f) then schema.map (fun fd => if fd.1 == f then (f, d) else fd)
  else insertField schema (f, d)

/-! ## The schema rules

The rules of `schema::problems` that a change of fields and object types can trigger, over the object types and
policies of a tenant. The kernel states more rules (counts, retention, queries, views, triggers, automations, the
other entity kinds); `docs/verification.md` (slice 11) lists what is not modelled and what the generator keeps out of
reach. -/

/-- The fields an expression reads, of any scope. -/
def Expr.reads : Expr → List FieldId
  | .lit _ => []
  | .field _ f => [f]
  | .createdBy _ => []
  | .principal => []
  | .cmp _ a b => a.reads ++ b.reads
  | .and a b => a.reads ++ b.reads
  | .or a b => a.reads ++ b.reads
  | .not a => a.reads
  | .isNull a => a.reads

/-- Whether the schema declares field `f`. -/
def declaredIn (schema : Schema) (f : FieldId) : Bool := schema.any (fun fd => fd.1 == f)

/-- A list of fields all declared. -/
def allDeclared (schema : Schema) (fs : List FieldId) : Bool := fs.all (declaredIn schema)

/-- A field type that is a list of lists (the kernel refuses nested lists). -/
def Ty.nested : Ty → Bool
  | .list (.list _) => true
  | _ => false

/-- A workflow the schema can hold: its field is a declared string field, and the guards and assignments of its
    transitions read and assign declared fields. -/
def workflowOk (schema : Schema) (w : Workflow) : Bool :=
  (match schema.lookup w.field with
    | some d => decide (d.ty = .str)
    | none => false) &&
  w.transitions.all (fun tr =>
    (tr.guard.all (fun g => allDeclared schema g.reads)) &&
    tr.set.all (fun fe => declaredIn schema fe.1 && allDeclared schema fe.2.reads))

/-- A policy the object types can hold: one on an object type names a type of the tenant and reads only fields it
    declares; one on every object reads none. -/
def policyOk (types : List (TypeId × ObjType)) (pl : Policy) : Bool :=
  match pl.resource with
  | .object T =>
    (match types.lookup T with
      | some o => pl.condition.all (fun c => allDeclared o.schema c.reads)
      | none => false)
  | .anyObject => pl.condition.all (fun c => c.reads.isEmpty)
  | _ => true

/-- The schema rules the model's changes can trigger, over the object types of a tenant and its policies: a field that
    refers to an object type the tenant does not have, a nested list, a unique field that is a list, a composite
    unique constraint that names a field its type does not declare (or one that is a list), a constraint that reads a
    field its type does not declare, a workflow whose field is not a declared string or whose transitions read or
    assign an undeclared field, a policy that names a missing object type or an undeclared field. `true` when the
    schema is valid. -/
def schemaRulesOk (ps : List Policy) (types : List (TypeId × ObjType)) : Bool :=
  types.all (fun e =>
    e.2.schema.all (fun fd => match fd.2.ty.target? with
      | some t => types.any (fun e' => e'.1 == t)
      | none => true) &&
    e.2.schema.all (fun fd => !fd.2.ty.nested) &&
    e.2.schema.all (fun fd => !(fd.2.unique && fd.2.ty.isList)) &&
    e.2.uniques.all (fun u => uniqueFieldsOk e.2.schema u) &&
    e.2.constraints.all (fun c => allDeclared e.2.schema c.reads) &&
    e.2.workflows.all (workflowOk e.2.schema)) &&
  ps.all (policyOk types)

/-! ## The changes -/

/-- Applies one change to the object types of a tenant. A change that does not apply is `invalidSchema`
    (the kernel's `apply_change` refusal): the type it names is missing, or the field it removes is not defined. A
    field change leaves the records as they are (the records are rewritten once, after every change has applied:
    `rewriteTypes`). -/
def applyChange (types : List (TypeId × ObjType)) : Change → Except Err (List (TypeId × ObjType))
  | .defineField ty f d => match types.lookup ty with
    | none => .error .invalidSchema
    | some o => .ok (putTypeList types ty { o with schema := setField o.schema f d })
  | .removeField ty f => match types.lookup ty with
    | none => .error .invalidSchema
    | some o =>
      if o.schema.any (fun fd => fd.1 == f) then
        .ok (putTypeList types ty { o with schema := o.schema.filter (fun fd => !(fd.1 == f)) })
      else .error .invalidSchema
  | .defineType ty =>
    if types.any (fun e => e.1 == ty) then .ok types
    else .ok (types ++ [(ty, { schema := [] })])
  | .removeType ty _ => match types.lookup ty with
    | none => .error .invalidSchema
    | some _ => .ok (types.filter (fun e => !(e.1 == ty)))

/-- The changes applied in order, then the validity predicate `valid` on the resulting object types. The first change
    that does not apply is refused, and the validity of the result is asked only once every change has applied (the
    kernel's `plan_migration`: `apply_change` for each change, then `schema::problems`). -/
def applyChanges (valid : List (TypeId × ObjType) → Bool) (types : List (TypeId × ObjType)) :
    List Change → Except Err (List (TypeId × ObjType))
  | [] => if valid types then .ok types else .error .invalidSchema
  | c :: cs => (applyChange types c) >>= fun types' => applyChanges valid types' cs

/-- A field type that a stored value of the old type still means: the same type, a reference keeping its target, or a
    list of such (the kernel's `same_or_wider`). -/
def Ty.sameOrWider : Ty → Ty → Bool
  | .ref a _, .ref b _ => a == b
  | .list a, .list b => a.sameOrWider b
  | a, b => a == b

/-- Whether a dependent of an object type reads field `f` (the kernel's dependency graph, over what the model holds): a
    composite unique constraint, a constraint, a workflow, or a policy on the type. -/
def readsField (ps : List Policy) (ty : TypeId) (o : ObjType) (f : FieldId) : Bool :=
  o.uniques.any (fun u => u.contains f) ||
  o.constraints.any (fun c => c.reads.contains f) ||
  o.workflows.any (fun w => w.field == f ||
    w.transitions.any (fun tr => tr.guard.any (fun g => g.reads.contains f) ||
      tr.set.any (fun fe => fe.1 == f || fe.2.reads.contains f))) ||
  ps.any (fun pl => pl.resource == .object ty && pl.condition.any (fun c => c.reads.contains f))

/-- The cells a list of changes removes or redefines with a type that is not the same or wider, each against the object
    types as they stand when the change applies (the kernel's footprint of a breaking change). -/
def breakingCells : List (TypeId × ObjType) → List Change → List (TypeId × FieldId)
  | _, [] => []
  | types, c :: cs =>
    let here : List (TypeId × FieldId) := match c with
      | .defineField ty f d =>
        match (types.lookup ty).bind (fun o => o.schema.lookup f) with
        | some old => if old.ty.sameOrWider d.ty then [] else [(ty, f)]
        | none => []
      | .removeField ty f => [(ty, f)]
      | _ => []
    here ++ (match applyChange types c with
      | .ok types' => breakingCells types' cs
      | .error _ => [])

/-- The kernel's refusal of a breaking change that reaches a dependent (D-8212), as `invalidSchema`: a field removed or
    redefined with another type that a dependent reads before and after the changes; and an object type removed and
    defined again while a field of a type, or a policy, still names it (D-8217). `types0` are the object types before the
    changes and `types1` the ones they leave. -/
def impactRefused (ps : List Policy) (types0 types1 : List (TypeId × ObjType)) (cs : List Change) : Bool :=
  (breakingCells types0 cs).any (fun c =>
    match types0.lookup c.1, types1.lookup c.1 with
    | some o0, some o1 => readsField ps c.1 o0 c.2 && readsField ps c.1 o1 c.2
    | _, _ => false) ||
  cs.any (fun c => match c with
    | .removeType X _ =>
      types1.any (fun e => e.1 == X) &&
      (types0.any (fun e0 => e0.2.schema.any (fun fd0 => fd0.2.ty.target? == some X &&
          match types1.lookup e0.1 with
          | some o1 => o1.schema.any (fun fd1 => fd1.1 == fd0.1 && fd1.2.ty.target? == some X)
          | none => false)) ||
        ps.any (fun pl => pl.resource == .object X))
    | _ => false)

/-- A record with the fields its type no longer declares dropped, and its version raised when any went (the kernel's
    rewrite of a record of a touched object type, `plan_migration`). -/
def stripRecord (schema : Schema) (r : Record) : Record :=
  if (r.fields.filter (fun fv => schema.any (fun fd => fd.1 == fv.1))).length == r.fields.length then r
  else { r with fields := r.fields.filter (fun fv => schema.any (fun fd => fd.1 == fv.1)),
                version := r.version + 1 }

/-- The object types after the records of the touched types that existed are rewritten against the schema they now
    have. -/
def rewriteTypes (orig : List (TypeId × ObjType)) (touched : List TypeId)
    (types : List (TypeId × ObjType)) : List (TypeId × ObjType) :=
  types.map (fun e =>
    if touched.contains e.1 && orig.any (fun e0 => e0.1 == e.1) then
      (e.1, { e.2 with records := e.2.records.map (fun q => (q.1, stripRecord e.2.schema q.2)) })
    else e)

/-- The object types of a tenant whose records a schema change re-validates: those its changes touch, in their
    own order (`scopedTypes`). -/
def changesScope (types : List (TypeId × ObjType)) (cs : List Change) : List (TypeId × ObjType) :=
  scopedTypes types (cs.flatMap Change.touched)

/-- The records of object type `ty` in a list of object types (the environment of `Tenant.env`). -/
def recordsIn (types : List (TypeId × ObjType)) : Env := fun ty => ((types.lookup ty).map ObjType.records).getD []

/-- Some record of the object types holds a reference to a record that is not there (the kernel's
    `DanglingReference`), against the records of `env`. -/
def refsViolated (types : List (TypeId × ObjType)) (env : Env) : Bool :=
  types.any (fun e => e.2.records.any (fun q => !refsOk env e.2.schema q.2))

/-- The refusal of a strict change (`Reject`): a record of a type the change re-validates violates the schema the
    change leaves, which is a table the schema check refuses or a dangling reference. A lax change never refuses. -/
def changeRefused (L : Limits) (scope : List (TypeId × ObjType)) (env : Env) (strict : Bool) : Bool :=
  strict && (schemaViolated L scope || refsViolated scope env)

/-- Whether a change list removes, without deleting its records, an object type that the tenant holds records of
    (the kernel's `deleted_records`, which asks the tenant as it was before the changes). -/
def removalsRefused (types0 : List (TypeId × ObjType)) (cs : List Change) : Bool :=
  cs.any (fun c => match c with
    | .removeType X false => match types0.lookup X with
      | some o => !o.records.isEmpty
      | none => false
    | _ => false)

/-- A schema change of a tenant (the kernel's `change_schema`), with the model's check order:
    1. the caller may manage the schema (`unauthorized`);
    2. an expected version that is not the schema version (`schemaConflict`);
    3. no changes (`invalidIntent`), or more than `maxChanges` (`limitExceeded`);
    4. the changes apply in order and the result is valid (`applyChanges`, `invalidSchema`);
    5. a breaking change that reaches a dependent (`invalidSchema`, `impactRefused`);
    6. a strict change (`Reject`) is refused (`migrationRejected`) when a record of a touched type violates the schema
       the change leaves; a lax change (`Grandfather`) keeps the records;
    7. an object type with records removed without deleting them (`migrationRejected`).
    On acceptance the schema version rises by one; the records of the touched types are rewritten
    (`rewriteTypes`). -/
def changeSchemaT (valid : List (TypeId × ObjType) → Bool) (t : Tenant) (p : Principal) (strict : Bool)
    (expected : Option Nat) (cs : List Change) : Except Err Tenant :=
  if !canManage t p then .error .unauthorized
  else if expected.any (fun v => v != t.schemaVersion) then .error .schemaConflict
  else if cs.isEmpty then .error .invalidIntent
  else if maxChanges < cs.length then .error .limitExceeded
  else
    match applyChanges valid t.types cs with
    | .error e => .error e
    | .ok types1 =>
      if impactRefused t.policies t.types types1 cs then .error .invalidSchema
      else
        let types2 := rewriteTypes t.types (cs.flatMap Change.touched) types1
        if changeRefused t.limits (changesScope types2 cs) (recordsIn types2) strict then .error .migrationRejected
        else if removalsRefused t.types cs then .error .migrationRejected
        else .ok { t with types := types2, schemaVersion := t.schemaVersion + 1 }

/-- The number of records of the touched types that the schema the change leaves violates: the grandfathered count of
    a lax change (`violatedCount`), over the object types the change left (`types'`). -/
def changeGrandfathered (L : Limits) (types' : List (TypeId × ObjType)) (cs : List Change) : Nat :=
  violatedCount L (changesScope types' cs) (recordsIn types')

end Strata
