/-
The abstract model of the Strata kernel.

This is a deliberately small model: one object type per tenant, three scalar value kinds and
references between its records, ownership-based authorization and one kind of schema change. It is not the Rust kernel and
proving things about it proves nothing about the Rust kernel by itself. What connects the two
is stated in `docs/verification.md` and exercised by the correspondence fixtures.

Everything here is a total, computable function, so the model can also be run (`Main.lean`).
-/

namespace Strata

abbrev TenantId := Nat
abbrev PrincipalId := Nat
abbrev RecordId := Nat
abbrev FieldId := Nat

/-- A value. `null` is the absence of a value. -/
inductive Val where
  | null
  | int (i : Int)
  | str (s : String)
  | bool (b : Bool)
  /-- A reference to the record with this id. -/
  | ref (id : RecordId)
  deriving DecidableEq, Repr

/-- What deleting a record does to a record that refers to it through a field of this kind:
    `restrict` refuses the delete, `setNull` clears the field. -/
inductive OnDelete where
  | restrict
  | setNull
  deriving DecidableEq, Repr

inductive Ty where
  | int
  | str
  | bool
  /-- A reference to a record of the tenant's (single) object type. -/
  | ref (onDelete : OnDelete)
  deriving DecidableEq, Repr

structure FieldDef where
  ty : Ty
  required : Bool
  deriving DecidableEq, Repr

/-- Whether a value has a type. `null` has none: absence is handled by `required`. -/
def Val.hasType : Val → Ty → Bool
  | .int _, .int => true
  | .str _, .str => true
  | .bool _, .bool => true
  | .ref _, .ref _ => true
  | _, _ => false

abbrev Fields := List (FieldId × Val)
abbrev Schema := List (FieldId × FieldDef)

structure Record where
  /-- The principal who created the record. -/
  owner : PrincipalId
  fields : Fields
  deriving DecidableEq, Repr

/-- The value of a field in a record; `null` when the record does not mention it. -/
def Fields.get (fs : Fields) (f : FieldId) : Val :=
  match fs.lookup f with
  | some v => v
  | none => .null

/-- A field of a record is known to the schema and has the declared type. -/
def fieldOk (schema : Schema) (fv : FieldId × Val) : Bool :=
  match schema.lookup fv.1 with
  | some d => fv.2.hasType d.ty
  | none => false

/-- A required field of the schema is present in the record. -/
def requiredOk (fs : Fields) (fd : FieldId × FieldDef) : Bool :=
  !fd.2.required || fs.get fd.1 != .null

/-- A record conforms to a schema: only known fields, of the right type, nothing required missing. -/
def conforms (schema : Schema) (r : Record) : Bool :=
  r.fields.all (fieldOk schema) && schema.all (requiredOk r.fields)

inductive Err where
  | noTenant
  | unauthorized
  | notFound
  | invalid
  | migrationRejected
  /-- A written record refers to a record that does not exist. -/
  | danglingReference
  /-- A deleted record is still referred to through a field that forbids it. -/
  | stillReferenced
  deriving DecidableEq, Repr

/-! ## References

A record is a set of facts about one object; a reference is a fact that names another record.
The model states two outcomes: a record is written only if every record it names exists, and a
delete never leaves a record naming the deleted one (it is refused, or the naming field is
cleared). Which records name which is not indexed; the statements quantify over all records. -/

abbrev Records := List (RecordId × Record)

/-- Whether a record with this id exists. -/
def Records.has (rs : Records) (id : RecordId) : Bool :=
  (rs.lookup id).isSome

/-- A value names no record, or names one that exists. -/
def Val.resolves (rs : Records) : Val → Bool
  | .ref id => rs.has id
  | _ => true

/-- Every reference a record holds names an existing record. -/
def refsOk (rs : Records) (r : Record) : Bool :=
  r.fields.all (fun fv => fv.2.resolves rs)

/-- The values a record gives a field: one, or none when it does not mention it. (A field named
    twice has two; the kernel's records are maps, and the scenarios never name a field twice.) -/
def Fields.valuesOf (fs : Fields) (f : FieldId) : List Val :=
  (fs.filter (fun fv => fv.1 == f)).map Prod.snd

/-- What is wrong with one value of a field of type `ty`, if anything: the wrong type, or a
    reference to no record. -/
def valueProblem (rs : Records) (ty : Ty) (v : Val) : Option Err :=
  if !v.hasType ty then some .invalid
  else if !v.resolves rs then some .danglingReference
  else none

/-- What is wrong with a record's values of one field of the schema, if anything. -/
def fieldProblem (rs : Records) (fs : Fields) (fd : FieldId × FieldDef) : Option Err :=
  if fd.2.required && (fs.valuesOf fd.1).isEmpty then some .invalid
  else (fs.valuesOf fd.1).findSome? (valueProblem rs fd.2.ty)

/-- The refusal for writing record `r` among records `rs`, or `none` if it may be written.

    When several things are wrong, which refusal is given is part of the outcome: a field the
    schema does not know comes first, then the fields of the schema in schema order, each
    refused for being missing, of the wrong type or naming no record. (The kernel's schema is
    ordered by field id, and the correspondence fixtures declare it in that order.) -/
def problem (schema : Schema) (rs : Records) (r : Record) : Option Err :=
  if r.fields.all (fun fv => (schema.lookup fv.1).isSome) then
    schema.findSome? (fieldProblem rs r.fields)
  else some .invalid

/-- A reference through field `f` forbids deleting its target: some definition of `f` is
    required or restricts deletion. -/
def forbidsDelete (schema : Schema) (f : FieldId) : Bool :=
  schema.any (fun fd => fd.1 == f && (fd.2.required || fd.2.ty == .ref .restrict))

/-- Record `q`, other than `id` itself, refers to `id` through a field that forbids its deletion. -/
def blocks (schema : Schema) (id : RecordId) (q : RecordId × Record) : Bool :=
  q.1 != id && q.2.fields.any (fun fv => fv.2 == .ref id && forbidsDelete schema fv.1)

/-- The record with every reference to `id` cleared. -/
def clearRef (id : RecordId) (r : Record) : Record :=
  { r with fields := r.fields.filter (fun fv => fv.2 != .ref id) }

/-- Everything one tenant owns. -/
structure Tenant where
  schema : Schema
  /-- May do anything in the tenant, including changing its schema. -/
  owner : PrincipalId
  /-- May create, and may change or delete any record. -/
  editors : List PrincipalId
  /-- May create, and may change or delete the records they created. -/
  writers : List PrincipalId
  records : Records
  /-- The id the next created record receives. -/
  nextId : Nat
  deriving Repr

/-- The whole state: each tenant id has a tenant or does not. -/
abbrev World := TenantId → Option Tenant

inductive Intent where
  | create (fields : Fields)
  /-- Replace a record's fields. (The Rust kernel patches; a patch that names every field is a
      replacement, and that is how the correspondence fixtures use it.) -/
  | update (id : RecordId) (fields : Fields)
  | delete (id : RecordId)
  /-- Make a field required. `strict` rejects the change if an existing record would not
      conform; otherwise existing records are left as they are. -/
  | setRequired (f : FieldId) (strict : Bool)
  deriving Repr

/-- The explicit environment of a step: which tenant, and who is acting. -/
structure Ctx where
  tenant : TenantId
  principal : PrincipalId
  deriving Repr

def canCreate (t : Tenant) (p : PrincipalId) : Bool :=
  p == t.owner || t.editors.contains p || t.writers.contains p

/-- May `p` change or delete record `r`? -/
def canEdit (t : Tenant) (p : PrincipalId) (r : Record) : Bool :=
  p == t.owner || t.editors.contains p || (t.writers.contains p && r.owner == p)

/-- May `p` change or delete a record when there is no record to test the permission against?
    Only those whose permission does not depend on the record: the owner and the editors. A writer
    may act only on a record it created, so with no record it may not. -/
def canEditAny (t : Tenant) (p : PrincipalId) : Bool :=
  p == t.owner || t.editors.contains p

/-- The refusal for an update or delete of a record that does not exist. The permission is asked
    first: one who could not act on a record gets `unauthorized`, the same as for a record that
    exists, and only one who could act on any record learns that this one is missing (KRN-1080). -/
def missingRecord (t : Tenant) (p : PrincipalId) : Err :=
  if canEditAny t p then .notFound else .unauthorized

/-- Absent values are not stored. -/
def normalise (fs : Fields) : Fields :=
  fs.filter (fun fv => fv.2 != .null)

def makeRequired (schema : Schema) (f : FieldId) : Schema :=
  schema.map (fun fd => if fd.1 == f then (fd.1, { fd.2 with required := true }) else fd)

/-- One intent against one tenant. -/
def stepTenant (t : Tenant) (p : PrincipalId) : Intent → Except Err Tenant
  | .create fs =>
    if !canCreate t p then .error .unauthorized
    else
      let r : Record := { owner := p, fields := normalise fs }
      match problem t.schema t.records r with
      | some e => .error e
      | none => .ok { t with records := (t.nextId, r) :: t.records, nextId := t.nextId + 1 }
  | .update id fs =>
    match t.records.lookup id with
    | none => .error (missingRecord t p)
    | some old =>
      if !canEdit t p old then .error .unauthorized
      else
        let r : Record := { owner := old.owner, fields := normalise fs }
        match problem t.schema t.records r with
        | some e => .error e
        | none =>
          .ok { t with records := t.records.map (fun q => if q.1 == id then (q.1, r) else q) }
  | .delete id =>
    match t.records.lookup id with
    | none => .error (missingRecord t p)
    | some old =>
      if !canEdit t p old then .error .unauthorized
      else if t.records.any (blocks t.schema id) then .error .stillReferenced
      else .ok { t with records :=
        (t.records.filter (fun q => q.1 != id)).map (fun q => (q.1, clearRef id q.2)) }
  | .setRequired f strict =>
    if p != t.owner then .error .unauthorized
    else
      let schema' := makeRequired t.schema f
      if strict && !(t.records.all (fun q => conforms schema' q.2)) then .error .migrationRejected
      else .ok { t with schema := schema' }

/-- The transition function of the model: one intent, in one context, against the world. -/
def step (w : World) (i : Intent) (c : Ctx) : Except Err World :=
  match w c.tenant with
  | none => .error .noTenant
  | some t =>
    match stepTenant t c.principal i with
    | .error e => .error e
    | .ok t' => .ok (fun x => if x = c.tenant then some t' else w x)

/-- Whether the acting principal is allowed to make this intent at all. -/
def authorized (t : Tenant) (p : PrincipalId) : Intent → Bool
  | .create _ => canCreate t p
  | .update id _ => match t.records.lookup id with
    | some old => canEdit t p old
    | none => false
  | .delete id => match t.records.lookup id with
    | some old => canEdit t p old
    | none => false
  | .setRequired _ _ => p == t.owner

/-- A schema change that leaves existing records as they are is the one intent that is allowed
    to break record conformance. Every other intent is strict. -/
def Intent.strict : Intent → Bool
  | .setRequired _ s => s
  | _ => true

/-- Referential integrity: every reference held by a record of the tenant names an existing
    record. -/
def Tenant.RefsOk (t : Tenant) : Prop :=
  ∀ q ∈ t.records, refsOk t.records q.2 = true

/-- The state invariant of one tenant: every record conforms to the schema, was given an id
    below the counter, and refers only to existing records. -/
def Tenant.WF (t : Tenant) : Prop :=
  (∀ q ∈ t.records, conforms t.schema q.2 = true) ∧ (∀ q ∈ t.records, q.1 < t.nextId) ∧
    t.RefsOk

def World.WF (w : World) : Prop :=
  ∀ x t, w x = some t → t.WF

def World.RefsOk (w : World) : Prop :=
  ∀ x t, w x = some t → t.RefsOk

end Strata
