/-
The abstract model of the Strata kernel.

This is a deliberately small model: one object type per tenant, three scalar value kinds,
references between its records, lists of either, versions, patch updates, constant defaults, the
size limits as an abstract bound, unique fields and composite unique constraints, ownership-based
authorization and three kinds of schema change. It is not the Rust kernel and proving things about
it proves nothing about the Rust kernel by itself. What connects the two is stated in
`docs/verification.md` and exercised by the correspondence fixtures.

Everything here is a total, computable function, so the model can also be run (`Main.lean`).
-/

namespace Strata

abbrev TenantId := Nat
abbrev PrincipalId := Nat
abbrev RecordId := Nat
abbrev FieldId := Nat

/-- An element of a list value: a scalar or a reference. The kernel's lists never hold lists.
    (`null` is expressible, as in the kernel, and has no type.) -/
inductive Elem where
  | null
  | int (i : Int)
  | str (s : String)
  | bool (b : Bool)
  | ref (id : RecordId)
  deriving DecidableEq, Repr

/-- A value. `null` is the absence of a value. -/
inductive Val where
  | null
  | int (i : Int)
  | str (s : String)
  | bool (b : Bool)
  /-- A reference to the record with this id. -/
  | ref (id : RecordId)
  /-- A list of scalars or of references. -/
  | list (vs : List Elem)
  deriving DecidableEq, Repr

/-- What deleting a record does to a record that refers to it through a field of this kind:
    `restrict` refuses the delete, `setNull` clears the field (or, in a list, removes the
    element). -/
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
  /-- A list whose elements have type `elem`. The kernel's schema check refuses nested lists, and
      no element value has a list type. -/
  | list (elem : Ty)
  deriving DecidableEq, Repr

structure FieldDef where
  ty : Ty
  required : Bool
  /-- Whether the field is declared unique: no two records hold the same keyed value in it
      (`Val.keyed`). A unique value has a bound of its own on its encoded size, and a list field
      is never unique (`setUnique` and `defineUnique` refuse one). -/
  unique : Bool := false
  /-- The constant a create fills the field with when the intent does not name it; `null` for no
      default. -/
  default : Val := .null
  deriving DecidableEq, Repr

/-- Whether a list element has a type. `null` has none. -/
def Elem.hasType : Elem → Ty → Bool
  | .int _, .int => true
  | .str _, .str => true
  | .bool _, .bool => true
  | .ref _, .ref _ => true
  | _, _ => false

/-- Whether a value has a type. `null` has none: absence is handled by `required`. A list has a
    list type when every element has the element type. -/
def Val.hasType : Val → Ty → Bool
  | .int _, .int => true
  | .str _, .str => true
  | .bool _, .bool => true
  | .ref _, .ref _ => true
  | .list vs, .list e => vs.all (fun v => v.hasType e)
  | _, _ => false

def Val.isList : Val → Bool
  | .list _ => true
  | _ => false

def Ty.isList : Ty → Bool
  | .list _ => true
  | _ => false

/-- A field of this type refuses the delete of a record it names. -/
def Ty.restricts : Ty → Bool
  | .ref .restrict => true
  | .list (.ref .restrict) => true
  | _ => false

/-! ## Size limits

The kernel bounds a string's bytes, a list's length, a unique value's encoded bytes and a record's
encoded bytes (`limits.rs`, `rules::validate_record`). The model keeps the bounds as numbers of the
tenant and the first two measures exactly, and replaces the two encodings by an abstract measure,
`Val.size`: a string counts its UTF-8 bytes, any other scalar one, a list the sum of its elements.
Every JSON encoding is at least that long, so a value or record the model finds too large the
kernel finds too large as well; the converse holds away from the bound (`docs/verification.md`,
slice 2's review note). -/

structure Limits where
  maxString : Nat
  maxList : Nat
  maxUnique : Nat
  maxRecord : Nat
  deriving DecidableEq, Repr

/-- The kernel's numbers: `MAX_STRING_BYTES`, `MAX_LIST_LEN`, `MAX_UNIQUE_BYTES`,
    `MAX_RECORD_BYTES`. -/
def Limits.kernel : Limits :=
  { maxString := 65536, maxList := 1000, maxUnique := 200, maxRecord := 262144 }

def Elem.size : Elem → Nat
  | .str s => s.utf8ByteSize
  | _ => 1

/-- The abstract size of a value: a stand-in for its encoded length, never more than it. -/
def Val.size : Val → Nat
  | .str s => s.utf8ByteSize
  | .list vs => (vs.map Elem.size).sum
  | _ => 1

/-- A string element over the string bound. -/
def Elem.overLimit (L : Limits) : Elem → Bool
  | .str s => decide (L.maxString < s.utf8ByteSize)
  | _ => false

/-- A value breaks the string or list bound: a string too long, or a list too long or holding a
    string too long (the kernel's `limits::value_problem`). -/
def Val.overLimit (L : Limits) : Val → Bool
  | .str s => decide (L.maxString < s.utf8ByteSize)
  | .list vs => decide (L.maxList < vs.length) || vs.any (Elem.overLimit L)
  | _ => false

abbrev Fields := List (FieldId × Val)
abbrev Schema := List (FieldId × FieldDef)

structure Record where
  /-- The principal who created the record. -/
  owner : PrincipalId
  /-- 1 when created, raised by one by every accepted update of the record and by every delete
      that clears a reference it holds. -/
  version : Nat
  fields : Fields
  deriving DecidableEq, Repr

/-- The abstract size of a record: the sum of its values' sizes. -/
def Record.size (r : Record) : Nat :=
  (r.fields.map (fun fv => fv.2.size)).sum

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
  /-- The intent named a version and the record is at another. -/
  | versionConflict
  /-- A written value or record is over a size bound. -/
  | limitExceeded
  /-- A written record agrees with another record on a unique field, or on every field of a
      composite unique constraint. -/
  | uniqueViolated
  /-- A schema change defines something the schema cannot hold: a field the tenant does not have,
      a unique field that is a list, or a composite constraint of the wrong size, with a repeated
      field or a list field. -/
  | invalidSchema
  deriving DecidableEq, Repr

/-! ## References

A record is a set of facts about one object; a reference is a fact that names another record.
The model states two outcomes: a record is written only if every record it names exists, and a
delete never leaves a record naming the deleted one (it is refused, or the naming field is
cleared, or the naming list element removed). Which records name which is not indexed; the
statements quantify over all records. -/

abbrev Records := List (RecordId × Record)

/-- Whether a record with this id exists. -/
def Records.has (rs : Records) (id : RecordId) : Bool :=
  (rs.lookup id).isSome

/-- A list element names no record, or names one that exists. -/
def Elem.resolves (rs : Records) : Elem → Bool
  | .ref id => rs.has id
  | _ => true

/-- A value names only records that exist: as a reference, or through the elements of a list. -/
def Val.resolves (rs : Records) : Val → Bool
  | .ref id => rs.has id
  | .list vs => vs.all (Elem.resolves rs)
  | _ => true

/-- Every reference a record holds names an existing record. -/
def refsOk (rs : Records) (r : Record) : Bool :=
  r.fields.all (fun fv => fv.2.resolves rs)

/-- The values a record gives a field: one, or none when it does not mention it. (A field named
    twice has two; the kernel's records are maps, and the model's writes keep one per name, so
    a create's fields and a patch's keep one value per name (`normalise`, `Fields.set`).) -/
def Fields.valuesOf (fs : Fields) (f : FieldId) : List Val :=
  (fs.filter (fun fv => fv.1 == f)).map Prod.snd

/-! ## Unique fields and composite constraints

A unique field, and each composite constraint, is a set of fields on which no two records may
agree. Two records agree on a set of fields when each field holds a keyed value (present and not
a list) in both, and the same one. A composite constraint whose fields are not all present in a
record does not apply to it, which `Val.keyed` gives for free. A unique field is the one-field
case of this, so one notion serves both. -/

/-- A value a unique constraint can clash on: present, and not a list. A list is never unique
    (the kernel refuses a unique list field), so no list clashes. -/
def Val.keyed : Val → Bool
  | .null => false
  | .list _ => false
  | _ => true

/-- Two records agree on the fields `us`: each holds a keyed value in each of them, and the same. -/
def agreeOn (us : List FieldId) (fs gs : Fields) : Bool :=
  us.all (fun f => (fs.get f).keyed && fs.get f == gs.get f)

/-- Some two distinct records agree on the fields `us`: the stored data violates the constraint. -/
def dupFields (rs : Records) (us : List FieldId) : Bool :=
  rs.any (fun q₁ => rs.any (fun q₂ => q₁.1 != q₂.1 && agreeOn us q₁.2.fields q₂.2.fields))

/-- The value `v`, keyed, that another record, not the one with id `self`, holds in field `f`. -/
def clashAt (rs : Records) (self : RecordId) (f : FieldId) (v : Val) : Bool :=
  v.keyed && rs.any (fun q => q.1 != self && q.2.fields.get f == v)

/-- What is wrong with one value of a field defined by `d`, if anything, in the kernel's order:
    over the string or list bound, a unique value over its bound, the wrong type, a unique value
    another record holds (only when `uniq`), a reference or a reference element naming no record.
    `uniq` says whether the unique checks are made: a write that is refused for some other reason
    and whose unique checks are not made is reported as it would be without them. -/
def valueProblem (L : Limits) (uniq : Bool) (rs : Records) (self : RecordId) (f : FieldId)
    (d : FieldDef) (v : Val) : Option Err :=
  if v.overLimit L then some .limitExceeded
  else if d.unique && decide (L.maxUnique < v.size) then some .limitExceeded
  else if !v.hasType d.ty then some .invalid
  else if uniq && d.unique && clashAt rs self f v then some .uniqueViolated
  else if !v.resolves rs then some .danglingReference
  else none

/-- What is wrong with a record's values of one field of the schema, if anything. -/
def fieldProblem (L : Limits) (uniq : Bool) (rs : Records) (self : RecordId) (fs : Fields)
    (fd : FieldId × FieldDef) : Option Err :=
  if fd.2.required && (fs.valuesOf fd.1).isEmpty then some .invalid
  else (fs.valuesOf fd.1).findSome? (valueProblem L uniq rs self fd.1 fd.2)

/-- The composite unique constraints, after the fields: a record that agrees with another record
    on every field of one of them is refused (when `uniq`). -/
def compositeProblem (uniq : Bool) (rs : Records) (self : RecordId) (fs : Fields)
    (us : List (List FieldId)) : Option Err :=
  if uniq && us.any (fun u => rs.any (fun q => q.1 != self && agreeOn u fs q.2.fields)) then
    some .uniqueViolated
  else none

/-- The refusal for writing record `r` as record `self` among records `rs`, or `none` if it may be
    written, with the unique checks made when `uniq`.

    When several things are wrong, which refusal is given is part of the outcome: a record over
    the record bound comes first, then a field the schema does not know, then the fields of the
    schema in schema order, each refused for being missing, over a bound, of the wrong type, a
    unique value held by another record, or naming no record; then the composite constraints.
    (The kernel's schema is ordered by field id, and the correspondence fixtures declare it in that
    order.) -/
def problem (L : Limits) (schema : Schema) (us : List (List FieldId)) (uniq : Bool)
    (rs : Records) (self : RecordId) (r : Record) : Option Err :=
  if decide (L.maxRecord < r.size) then some .limitExceeded
  else if r.fields.all (fun fv => (schema.lookup fv.1).isSome) then
    match schema.findSome? (fieldProblem L uniq rs self r.fields) with
    | some e => some e
    | none => compositeProblem uniq rs self r.fields us
  else some .invalid

/-- Whether a value names record `id`: it is a reference to it, or a list holding one. -/
def Val.refersTo (id : RecordId) : Val → Bool
  | .ref x => x == id
  | .list vs => vs.contains (Elem.ref id)
  | _ => false

def Record.refersTo (id : RecordId) (r : Record) : Bool :=
  r.fields.any (fun fv => fv.2.refersTo id)

/-- A reference to a deleted record through field `f`, held in value `v`, forbids the delete:
    some definition of `f` restricts deletion, or is required while `v` is a single reference. A
    required list never forbids it: the element is removed and the list stays. (The kernel asks
    whether the field's type is a list; on a record of the declared types that is whether the
    value is one.) -/
def forbidsDelete (schema : Schema) (f : FieldId) (v : Val) : Bool :=
  schema.any (fun fd => fd.1 == f && ((fd.2.required && !v.isList) || fd.2.ty.restricts))

/-- Record `q`, other than `id` itself, refers to `id` through a field that forbids its deletion. -/
def blocks (schema : Schema) (id : RecordId) (q : RecordId × Record) : Bool :=
  q.1 != id && q.2.fields.any (fun fv => fv.2.refersTo id && forbidsDelete schema fv.1 fv.2)

/-- A value with the references to `id` taken out: a single reference to it is gone (`none`), a
    list loses those elements and stays, possibly empty. -/
def Val.dropRef (id : RecordId) : Val → Option Val
  | .ref x => if x = id then none else some (.ref x)
  | .list vs => some (.list (vs.filter (fun v => v != Elem.ref id)))
  | v => some v

def clearFields (id : RecordId) (fs : Fields) : Fields :=
  fs.filterMap (fun fv => (fv.2.dropRef id).map (fun v => (fv.1, v)))

/-- A record that refers to `id`, with every reference to it cleared and its version raised; any
    other record as it is. -/
def clearRef (id : RecordId) (r : Record) : Record :=
  if r.refersTo id then { r with fields := clearFields id r.fields, version := r.version + 1 }
  else r

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
  /-- The size bounds. The kernel's are constants (`Limits.kernel`). -/
  limits : Limits := Limits.kernel
  /-- The composite unique constraints, each a list of fields (the kernel's `UniqueDef`). -/
  uniques : List (List FieldId) := []
  deriving Repr

/-- The whole state: each tenant id has a tenant or does not. -/
abbrev World := TenantId → Option Tenant

inductive Intent where
  /-- Create a record. A field of the schema the intent does not name is filled from its default. -/
  | create (fields : Fields)
  /-- Patch a record: each named field takes the given value, a named `null` removes the field,
      fields not named are kept. `expected`, when given, must be the record's version. -/
  | update (id : RecordId) (fields : Fields) (expected : Option Nat)
  /-- Delete a record. `expected`, when given, must be the record's version. -/
  | delete (id : RecordId) (expected : Option Nat)
  /-- Make a field required. `strict` rejects the change if an existing record would not
      conform; otherwise existing records are left as they are. -/
  | setRequired (f : FieldId) (strict : Bool)
  /-- Make a field unique. `strict` rejects the change if existing records already agree on it;
      otherwise existing records are left as they are. -/
  | setUnique (f : FieldId) (strict : Bool)
  /-- Define a composite unique constraint over these fields. Refused if existing records already
      agree on all of them. -/
  | defineUnique (fs : List FieldId)
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

/-- One entry of a patch: field `fv.1` takes value `fv.2`, or is removed if that is `null`. -/
def Fields.set (fs : Fields) (fv : FieldId × Val) : Fields :=
  if fv.2 == .null then fs.filter (fun g => g.1 != fv.1)
  else fv :: fs.filter (fun g => g.1 != fv.1)

/-- A patch applied to fields, entry by entry. -/
def Fields.patch (fs ps : Fields) : Fields :=
  ps.foldl Fields.set fs

/-- Absent values are not stored, and a name given twice keeps its last value: the fields of a
    create are built as the kernel builds its records, a map from names to values. -/
def normalise (fs : Fields) : Fields :=
  fs.foldl Fields.set []

/-- The fields a create writes: the named ones that are not `null`, then the default of every field
    of the schema that the intent does not name at all and whose default is not `null`. (The
    kernel's rule: a field named as `null` gets no default.) -/
def withDefaults (schema : Schema) (fs : Fields) : Fields :=
  normalise fs ++ schema.filterMap (fun fd =>
    if fs.any (fun fv => fv.1 == fd.1) || fd.2.default == .null then none
    else some (fd.1, fd.2.default))

/-- The record a create by `p` writes, at version 1. -/
def newRecord (schema : Schema) (p : PrincipalId) (fs : Fields) : Record :=
  { owner := p, version := 1, fields := withDefaults schema fs }

/-- The record an accepted update writes: patched, one version on. -/
def patched (old : Record) (ps : Fields) : Record :=
  { owner := old.owner, version := old.version + 1, fields := old.fields.patch ps }

/-- The intent names a version and the record is at another. -/
def versionClash : Option Nat → Record → Bool
  | some v, r => v != r.version
  | none, _ => false

def makeRequired (schema : Schema) (f : FieldId) : Schema :=
  schema.map (fun fd => if fd.1 == f then (fd.1, { fd.2 with required := true }) else fd)

def makeUnique (schema : Schema) (f : FieldId) : Schema :=
  schema.map (fun fd => if fd.1 == f then (fd.1, { fd.2 with unique := true }) else fd)

/-- What a strict schema change over the stored records would violate, under the schema it leaves:
    some record does not conform (a required field missing, a value of the wrong type, a reference
    to nothing); a unique field is held by two records, or holds a value over the unique bound;
    a composite constraint is held by two records. This is the kernel's `Reject` check: it
    validates every existing record against the resulting schema, and every unique constraint with
    it, so a violation a lax change left behind refuses a later strict change. -/
def schemaViolated (L : Limits) (schema : Schema) (us : List (List FieldId)) (rs : Records) : Bool :=
  !(rs.all (fun q => conforms schema q.2)) ||
    schema.any (fun fd => fd.2.unique &&
      (dupFields rs [fd.1] || rs.any (fun q => decide (L.maxUnique < (q.2.fields.get fd.1).size)))) ||
    us.any (fun u => dupFields rs u)

/-- The one refusal every strict schema change makes: `Reject` refuses the change when the records
    would violate the resulting schema (`schemaViolated`). A lax change never refuses. -/
def schemaChangeRefused (L : Limits) (schema : Schema) (us : List (List FieldId)) (rs : Records)
    (strict : Bool) : Bool :=
  strict && schemaViolated L schema us rs

/-- A composite definition the schema can hold: two to four fields, none repeated, each one of the
    schema and not a list (the kernel's `unique_problems`). -/
def uniqueFieldsOk (schema : Schema) (fs : List FieldId) : Bool :=
  decide (2 ≤ fs.length) && decide (fs.length ≤ 4) && decide fs.Nodup &&
    fs.all (fun f => match schema.lookup f with
      | some d => !d.ty.isList
      | none => false)

/-- One intent against one tenant. An expected version is checked after the permission, so only
    one who may act on the record learns that it is at another version. -/
def stepTenant (t : Tenant) (p : PrincipalId) : Intent → Except Err Tenant
  | .create fs =>
    if !canCreate t p then .error .unauthorized
    else
      let r := newRecord t.schema p fs
      match problem t.limits t.schema t.uniques true t.records t.nextId r with
      | some e => .error e
      | none => .ok { t with records := (t.nextId, r) :: t.records, nextId := t.nextId + 1 }
  | .update id fs ev =>
    match t.records.lookup id with
    | none => .error (missingRecord t p)
    | some old =>
      if !canEdit t p old then .error .unauthorized
      else if versionClash ev old then .error .versionConflict
      else
        let r := patched old fs
        match problem t.limits t.schema t.uniques true t.records id r with
        | some e => .error e
        | none =>
          .ok { t with records := t.records.map (fun q => if q.1 == id then (q.1, r) else q) }
  | .delete id ev =>
    match t.records.lookup id with
    | none => .error (missingRecord t p)
    | some old =>
      if !canEdit t p old then .error .unauthorized
      else if versionClash ev old then .error .versionConflict
      else if t.records.any (blocks t.schema id) then .error .stillReferenced
      else .ok { t with records :=
        (t.records.filter (fun q => q.1 != id)).map (fun q => (q.1, clearRef id q.2)) }
  | .setRequired f strict =>
    if p != t.owner then .error .unauthorized
    else
      let schema' := makeRequired t.schema f
      if schemaChangeRefused t.limits schema' t.uniques t.records strict then .error .migrationRejected
      else .ok { t with schema := schema' }
  | .setUnique f strict =>
    if p != t.owner then .error .unauthorized
    else
      match t.schema.lookup f with
      | none => .error .invalidSchema
      | some d =>
        if d.ty.isList then .error .invalidSchema
        else if schemaChangeRefused t.limits (makeUnique t.schema f) t.uniques t.records strict then
          .error .migrationRejected
        else .ok { t with schema := makeUnique t.schema f }
  | .defineUnique fs =>
    if p != t.owner then .error .unauthorized
    else if !uniqueFieldsOk t.schema fs then .error .invalidSchema
    else if dupFields t.records fs then .error .uniqueViolated
    else if schemaChangeRefused t.limits t.schema (t.uniques ++ [fs]) t.records true then
      .error .migrationRejected
    else .ok { t with uniques := t.uniques ++ [fs] }

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
  | .update id _ _ => match t.records.lookup id with
    | some old => canEdit t p old
    | none => false
  | .delete id _ => match t.records.lookup id with
    | some old => canEdit t p old
    | none => false
  | .setRequired _ _ => p == t.owner
  | .setUnique _ _ => p == t.owner
  | .defineUnique _ => p == t.owner

/-- A schema change that leaves existing records as they are is the one kind of intent that is
    allowed to break record conformance or uniqueness. Every other intent is strict. -/
def Intent.strict : Intent → Bool
  | .setRequired _ s => s
  | .setUnique _ s => s
  | _ => true

/-- The intents that change the schema and leave the records as they are. -/
def Intent.isSchema : Intent → Bool
  | .setRequired _ _ => true
  | .setUnique _ _ => true
  | .defineUnique _ => true
  | _ => false

/-- Referential integrity: every reference held by a record of the tenant, single or in a list,
    names an existing record. -/
def Tenant.RefsOk (t : Tenant) : Prop :=
  ∀ q ∈ t.records, refsOk t.records q.2 = true

/-- The names are distinct: the schema names each field once, and each record holds each field at
    most once (the kernel's records are maps). -/
def Tenant.Distinct (t : Tenant) : Prop :=
  (t.schema.map Prod.fst).Nodup ∧ ∀ q ∈ t.records, (q.2.fields.map Prod.fst).Nodup

/-- No two distinct records of the tenant agree on a unique field, or on every field of a
    composite unique constraint. -/
def Tenant.UniqueRecords (t : Tenant) : Prop :=
  ∀ q₁ ∈ t.records, ∀ q₂ ∈ t.records, q₁.1 ≠ q₂.1 →
    (∀ fd ∈ t.schema, fd.2.unique = true → agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
    (∀ u ∈ t.uniques, agreeOn u q₁.2.fields q₂.2.fields = false)

/-- The state invariant of one tenant: every record conforms to the schema, was given an id
    below the counter, refers only to existing records, and the names are distinct and the
    unique constraints hold. -/
def Tenant.WF (t : Tenant) : Prop :=
  (∀ q ∈ t.records, conforms t.schema q.2 = true) ∧ (∀ q ∈ t.records, q.1 < t.nextId) ∧
    t.RefsOk ∧ t.Distinct ∧ t.UniqueRecords

def World.WF (w : World) : Prop :=
  ∀ x t, w x = some t → t.WF

def World.RefsOk (w : World) : Prop :=
  ∀ x t, w x = some t → t.RefsOk

end Strata
