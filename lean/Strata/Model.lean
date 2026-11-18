/-
The abstract model of the Strata kernel.

This is a deliberately small model: one object type per tenant, three scalar value kinds,
ownership-based authorization and one kind of schema change. It is not the Rust kernel and
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
  deriving DecidableEq, Repr

inductive Ty where
  | int
  | str
  | bool
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

/-- Everything one tenant owns. -/
structure Tenant where
  schema : Schema
  /-- May do anything in the tenant, including changing its schema. -/
  owner : PrincipalId
  /-- May create, and may change or delete any record. -/
  editors : List PrincipalId
  /-- May create, and may change or delete the records they created. -/
  writers : List PrincipalId
  records : List (RecordId × Record)
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

inductive Err where
  | noTenant
  | unauthorized
  | notFound
  | invalid
  | migrationRejected
  deriving DecidableEq, Repr

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
      if conforms t.schema r then
        .ok { t with records := (t.nextId, r) :: t.records, nextId := t.nextId + 1 }
      else .error .invalid
  | .update id fs =>
    match t.records.lookup id with
    | none => .error .notFound
    | some old =>
      if !canEdit t p old then .error .unauthorized
      else
        let r : Record := { owner := old.owner, fields := normalise fs }
        if conforms t.schema r then
          .ok { t with records := t.records.map (fun q => if q.1 == id then (q.1, r) else q) }
        else .error .invalid
  | .delete id =>
    match t.records.lookup id with
    | none => .error .notFound
    | some old =>
      if !canEdit t p old then .error .unauthorized
      else .ok { t with records := t.records.filter (fun q => q.1 != id) }
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

/-- The state invariant of one tenant: every record conforms to the schema and was given an id
    below the counter. -/
def Tenant.WF (t : Tenant) : Prop :=
  (∀ q ∈ t.records, conforms t.schema q.2 = true) ∧ (∀ q ∈ t.records, q.1 < t.nextId)

def World.WF (w : World) : Prop :=
  ∀ x t, w x = some t → t.WF

end Strata
