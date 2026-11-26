/-
The abstract model of the Strata kernel.

This is a deliberately small model: several object types per tenant, three scalar value kinds,
references between records (within a type and across types), lists of either, versions, patch
updates, constant defaults, the size limits as an abstract bound, unique fields and composite
unique constraints, ownership-based authorization and three kinds of schema change. It is not the
Rust kernel and proving things about it proves nothing about the Rust kernel by itself. What
connects the two is stated in `docs/verification.md` and exercised by the correspondence fixtures.

Everything here is a total, computable function, so the model can also be run (`Main.lean`).

Shape of the state. A tenant has one table per object type; each table holds the schema of the
type, its composite unique constraints and its records. Record ids come from one counter for the
whole tenant, as the kernel's do, so an id names at most one record of the tenant. A reference
names a record by its type and its id: a field whose type targets type `T` holds a record of `T`,
and a record of any other type is not one it can name (the kernel's `lookup.record(target, id)`).
-/

namespace Strata

abbrev TenantId := Nat
abbrev PrincipalId := Nat
abbrev RecordId := Nat
abbrev FieldId := Nat
/-- An object type of a tenant (the kernel's `ObjectTypeId`). -/
abbrev TypeId := Nat

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
  /-- A reference to the record with this id, of the type the field names. -/
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
  /-- A reference to a record of object type `target`. -/
  | ref (target : TypeId) (onDelete : OnDelete)
  /-- A list whose elements have type `elem`. The kernel's schema check refuses nested lists, and
      no element value has a list type. -/
  | list (elem : Ty)
  deriving DecidableEq, Repr

/-- Which record an expression reads a field from: the record being checked (the kernel's
    `record`), the stored record an update replaces (`old`, absent on a create) or the new record
    (`new`, the same as `record` on a write). -/
inductive Scope where
  | record
  | old
  | new
  deriving DecidableEq, Repr

inductive CmpOp where
  | eq
  | ne
  | lt
  | le
  | gt
  | ge
  deriving DecidableEq, Repr

/-- The expression language of constraints and expression defaults (slice 5): the kernel's
    `tenant-rule-language` restricted to literals, field reads of the three scopes, the principal,
    comparisons, the three-valued connectives and `isNull`. Its evaluation, and the errors it can
    raise, are in the section "Expressions" below. -/
inductive Expr where
  /-- A literal value. -/
  | lit (v : Val)
  /-- A field of the record in the scope; `null` when there is no such record or it lacks the field. -/
  | field (s : Scope) (f : FieldId)
  /-- The system attribute `$created_by` of a record in the scope: the string `p<n>` of the user
      who created it, `null` when no user did (the system or an anonymous caller) or there is no
      record (slice 6). -/
  | createdBy (s : Scope)
  /-- The acting principal, as the kernel names it: the string `p<n>` for the principal `n` (the
      fixtures name principals so); `null` for the system and for an anonymous caller, which name
      no user. -/
  | principal
  | cmp (op : CmpOp) (a b : Expr)
  /-- Kleene `and`: `false` if either operand is `false`, `null` if one is `null`, else `true`. -/
  | and (a b : Expr)
  /-- Kleene `or`: `true` if either operand is `true`, `null` if one is `null`, else `false`. -/
  | or (a b : Expr)
  | not (a : Expr)
  | isNull (a : Expr)
  deriving DecidableEq, Repr

/-! ## Principals, roles and policies (slice 6)

Authorization is data. A principal is a user, the system or an anonymous caller. Principals hold
roles directly (`Tenant.members`). A role grants an action on the records of one object type, or
a capability (`Tenant.grants`), and a policy grants actions on a resource under a condition
(`Tenant.policies`). `Tenant.allows` is the decision, mirroring the kernel's `rules::authorize`
without its scopes, clock and lookups. -/

/-- Who acts. A user is named by its id; the system holds no role; an anonymous caller holds the
    built-in `public` role and nothing else (the kernel's `Principal`). -/
inductive Principal where
  | user (id : PrincipalId)
  | system
  | anonymous
  deriving DecidableEq, Repr

/-- The user a principal is, if it is one: the principal an expression reads as `principal` and the
    creator a record is given. -/
def Principal.userId? : Principal → Option PrincipalId
  | .user p => some p
  | _ => none

/-- A role, by id (the kernel's `RoleId`). -/
abbrev RoleId := Nat

/-- A capability, by id (the kernel's `Grant::Capability`). -/
abbrev CapabilityId := Nat

/-- The built-in role of the owner, which may do anything in its tenant. -/
def ownerRole : RoleId := 0

/-- The built-in role every principal but the system holds. -/
def publicRole : RoleId := 1

/-- An action (the kernel's `Action`). Manage is the schema's; the others are the actions on the
    records of an object type. -/
inductive Action where
  | read
  | create
  | update
  | delete
  | manage
  deriving DecidableEq, Repr

/-- What a policy names (the kernel's `Resource`): the records of one object type, the records of
    every type, or the schema. -/
inductive Resource where
  | object (ty : TypeId)
  | anyObject
  | schema
  deriving DecidableEq, Repr

/-- What a role grants: a capability, or an action on the records of an object type (the kernel's
    `Grant`). A manage action is never granted on an object type. -/
inductive Grant where
  | capability (c : CapabilityId)
  | object (ty : TypeId) (a : Action)
  deriving DecidableEq, Repr

/-- A policy: a role holds the actions it names on the resource, when its condition holds. A policy
    with no condition holds unconditionally. Its condition is an expression of the slice 5 language,
    evaluated over the record and the principal (the kernel's `PolicyDef`, whose id is left out: it
    names the policy in a refusal and decides nothing). -/
structure Policy where
  role : RoleId
  resource : Resource
  actions : List Action
  condition : Option Expr := none
  deriving Repr

/-- The resource a policy's resource reaches: the same resource, or every object type for
    `anyObject`. -/
def Resource.covers : Resource → Resource → Bool
  | .anyObject, .object _ => true
  | a, b => a == b

structure FieldDef where
  ty : Ty
  required : Bool
  /-- Whether the field is declared unique: no two records of its type hold the same keyed value
      in it (`Val.keyed`). A unique value has a bound of its own on its encoded size, and a list
      field is never unique (`setUnique` and `defineUnique` refuse one). -/
  unique : Bool := false
  /-- The expression a create fills the field with when the intent does not name it. The constant
      default is `.lit v`; `.lit .null`, the default of a field with none, fills nothing. -/
  default : Expr := .lit .null
  deriving DecidableEq, Repr

/-- Whether a list element has a type. `null` has none. -/
def Elem.hasType : Elem → Ty → Bool
  | .int _, .int => true
  | .str _, .str => true
  | .bool _, .bool => true
  | .ref _, .ref _ _ => true
  | _, _ => false

/-- Whether a value has a type. `null` has none: absence is handled by `required`. A list has a
    list type when every element has the element type. -/
def Val.hasType : Val → Ty → Bool
  | .int _, .int => true
  | .str _, .str => true
  | .bool _, .bool => true
  | .ref _, .ref _ _ => true
  | .list vs, .list e => vs.all (fun v => v.hasType e)
  | _, _ => false

def Val.isList : Val → Bool
  | .list _ => true
  | _ => false

def Ty.isList : Ty → Bool
  | .list _ => true
  | _ => false

/-- The object type a reference of this type names: the target of a reference field, or of the
    elements of a list of references. -/
def Ty.target? : Ty → Option TypeId
  | .ref t _ => some t
  | .list e => e.target?
  | _ => none

/-- A field of this type refuses the delete of a record it names. -/
def Ty.restricts : Ty → Bool
  | .ref _ .restrict => true
  | .list (.ref _ .restrict) => true
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
  /-- The user who created the record (the kernel's `created_by`): `none` when the creator is the
      system or an anonymous caller, which name no user (slice 6). -/
  owner : Option PrincipalId
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

/-! ## Expressions (slice 5)

An expression is evaluated in an environment: the record being checked, the stored record an
update replaces, and the principal. A value is a `Val`, `null` included, and an evaluation can fail:
the result is `Except Unit Val`, where `error ()` is the kernel's `EvalError`. The rules are the
kernel's (`crates/model-core/src/tenant_rule_language.rs`, `kleene`, `equality`, `order`):

- a field read of an absent record is `null`, and so is a field the record lacks;
- a comparison with `null` on either side is `null`; comparing two kinds that differ is an error;
  an ordering (`<`, `<=`, `>`, `>=`) is defined for integers and strings only, so an ordering of
  booleans, references or lists is an error;
- `not` and the connectives take booleans and `null`, and any other operand is an error; `and` and
  `or` are Kleene: a decisive operand (`false` for `and`, `true` for `or`) decides the result
  whatever the other operand is, even an error, because the kernel stops at it; otherwise `null`
  if either is `null`, else the non-decisive value.

Left to right, the first error decides, and a decisive operand before it does not. The truth tables
are lemmas in `Strata/Constraints.lean`. -/

/-- The outcome of evaluating an expression: a value, or an evaluation error. -/
abbrev EvalResult := Except Unit Val

/-- What an expression may observe: the record being checked, the stored record an update replaces
    (none on a create), and the principal (none for the system). -/
structure ExprEnv where
  record : Option Record := none
  old : Option Record := none
  new : Option Record := none
  principal : Option PrincipalId := none

/-- The record an expression reads from a scope, if there is one. -/
def ExprEnv.scoped (env : ExprEnv) : Scope → Option Record
  | .record => env.record
  | .old => env.old
  | .new => env.new

/-- Two values of the same kind (`null` is of no kind). -/
def Val.sameKind : Val → Val → Bool
  | .int _, .int _ => true
  | .str _, .str _ => true
  | .bool _, .bool _ => true
  | .ref _, .ref _ => true
  | .list _, .list _ => true
  | .null, .null => true
  | _, _ => false

/-- The truth of an ordering test from whether the left is less than the right and whether they are
    equal. -/
def ordTest : CmpOp → Bool → Bool → Bool
  | .lt, lt, _ => lt
  | .le, lt, eq => lt || eq
  | .gt, lt, eq => !(lt || eq)
  | .ge, lt, _ => !lt
  | .eq, _, eq => eq
  | .ne, _, eq => !eq

/-- A comparison: `null` when either side is `null`; an error when the kinds differ, or when an
    ordering is asked of a kind that has none. -/
def compareVals (op : CmpOp) (x y : Val) : EvalResult :=
  if x == .null || y == .null then .ok .null
  else if !x.sameKind y then .error ()
  else match op with
    | .eq => .ok (.bool (x == y))
    | .ne => .ok (.bool (!(x == y)))
    | _ => match x, y with
      | .int a, .int b => .ok (.bool (ordTest op (decide (a < b)) (decide (a = b))))
      | .str a, .str b => .ok (.bool (ordTest op (decide (a < b)) (decide (a = b))))
      | _, _ => .error ()

/-- An operand of `not` or of a connective: a boolean, or `null`. -/
def truthOf : Val → EvalResult
  | .bool b => .ok (.bool b)
  | .null => .ok .null
  | _ => .error ()

/-- Kleene connective: `d` is the decisive value (`false` for `and`, `true` for `or`). The left
    operand decides when it is `d`, whatever the right one is. -/
def connective (d : Bool) (x y : EvalResult) : EvalResult :=
  match x with
  | .error _ => .error ()
  | .ok (.bool b) =>
    if b == d then .ok (.bool d)
    else match y with
      | .error _ => .error ()
      | .ok w => truthOf w
  | .ok .null =>
    match y with
    | .error _ => .error ()
    | .ok (.bool c) => if c == d then .ok (.bool d) else .ok .null
    | .ok .null => .ok .null
    | .ok _ => .error ()
  | .ok _ => .error ()

/-- Evaluate an expression in an environment. -/
def evalExpr (env : ExprEnv) : Expr → EvalResult
  | .lit v => .ok v
  | .field s f => .ok (match env.scoped s with
      | some r => r.fields.get f
      | none => .null)
  | .createdBy s => .ok (match env.scoped s with
      | some r => match r.owner with
        | some n => .str ("p" ++ toString n)
        | none => .null
      | none => .null)
  | .principal => .ok (match env.principal with
      | some p => .str ("p" ++ toString p)
      | none => .null)
  | .cmp op a b => match evalExpr env a, evalExpr env b with
    | .ok x, .ok y => compareVals op x y
    | _, _ => .error ()
  | .and a b => connective false (evalExpr env a) (evalExpr env b)
  | .or a b => connective true (evalExpr env a) (evalExpr env b)
  | .not a => match evalExpr env a with
    | .ok (.bool b) => .ok (.bool (!b))
    | .ok .null => .ok .null
    | _ => .error ()
  | .isNull a => match evalExpr env a with
    | .ok .null => .ok (.bool true)
    | .ok _ => .ok (.bool false)
    | .error _ => .error ()

/-- The environment a write's constraints are checked in: the candidate record (as `record` and as
    `new`), the stored record it replaces (none on a create), and the caller. -/
def writeEnv (p : Principal) (candidate : Record) (old : Option Record) : ExprEnv :=
  { record := some candidate, old := old, new := some candidate, principal := p.userId? }

/-- The environment a stored record is checked in when a schema change re-validates it: the record
    as `record` and `new`, no old record, and the system as principal (as the kernel's `Reject`
    does). -/
def storedEnv (r : Record) : ExprEnv :=
  { record := some r, new := some r }

/-- The fields a create writes beyond the named ones: the default of each field of the schema the
    intent does not name, when it evaluates to something other than `null`. Any default that cannot
    be evaluated fails the whole create. The defaults are evaluated with no record (a field reads as
    `null`) and the caller as principal. -/
def defaultsOf (env : ExprEnv) : Schema → Fields → Except Unit Fields
  | [], _ => .ok []
  | (f, d) :: ss, fs =>
    match defaultsOf env ss fs with
    | .error _ => .error ()
    | .ok rest =>
      if fs.any (fun fv => fv.1 == f) then .ok rest
      else match evalExpr env d.default with
        | .error _ => .error ()
        | .ok .null => .ok rest
        | .ok v => .ok ((f, v) :: rest)

/-! ## Field checks -/

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
  /-- A written record refers to a record that does not exist in the type its field names. -/
  | danglingReference
  /-- A deleted record is still referred to through a field that forbids it. -/
  | stillReferenced
  /-- The intent named a version and the record is at another. -/
  | versionConflict
  /-- A written value or record is over a size bound. -/
  | limitExceeded
  /-- A written record agrees with another record of its type on a unique field, or on every field
      of a composite unique constraint. -/
  | uniqueViolated
  /-- A schema change defines something the schema cannot hold: a field the tenant does not have,
      a unique field that is a list, or a composite constraint of the wrong size, with a repeated
      field or a list field. -/
  | invalidSchema
  /-- The intent names an object type the tenant does not have, and the caller could act on that
      type. -/
  | unknownObject
  /-- A constraint of the record's type is `false` on the record written (slice 5). -/
  | constraintViolated
  /-- An expression that a write or a default needs cannot be evaluated (a type error, or an
      operand it cannot read): the kernel's `Evaluation` (slice 5). -/
  | evaluation
  deriving DecidableEq, Repr

/-- The first constraint that fails on the record, in the order given: `false` is
    `constraintViolated`, an evaluation error is `evaluation`; `true` and `null` pass. -/
def constraintProblem (env : ExprEnv) : List Expr → Option Err
  | [] => none
  | c :: cs => match evalExpr env c with
    | .error _ => some Err.evaluation
    | .ok (.bool false) => some Err.constraintViolated
    | .ok _ => constraintProblem env cs

/-! ## Records of the tenant

The records of one object type form a list of `(id, record)` pairs with distinct ids. Within a
tenant, a reference is resolved against the records of the type it names. -/

abbrev Records := List (RecordId × Record)

/-- Whether a record with this id exists in the list. -/
def Records.has (rs : Records) (id : RecordId) : Bool :=
  (rs.lookup id).isSome

/-- The records of each object type of the tenant, as a function of the type. A type the tenant
    does not have has none. -/
abbrev Env := TypeId → Records

/-- A reference of type `ty` names a record that exists in the type it targets. A value that holds
    no reference to a type names nothing to check. -/
def refHas (env : Env) (ty : Ty) (id : RecordId) : Bool :=
  match ty.target? with
  | some t => (env t).has id
  | none => true

/-- A list element names no record, or names one that exists in the target type. -/
def Elem.resolves (env : Env) (ty : Ty) : Elem → Bool
  | .ref id => refHas env ty id
  | _ => true

/-- A value names only records that exist in its target type: as a reference, or through the
    elements of a list. -/
def Val.resolves (env : Env) (ty : Ty) : Val → Bool
  | .ref id => refHas env ty id
  | .list vs => vs.all (Elem.resolves env ty)
  | _ => true

/-- Every reference a record holds names an existing record of the type the field names. -/
def refsOk (env : Env) (schema : Schema) (r : Record) : Bool :=
  r.fields.all (fun fv => match schema.lookup fv.1 with
    | some d => fv.2.resolves env d.ty
    | none => true)

/-- The values a record gives a field: one, or none when it does not mention it. (A field named
    twice has two; the kernel's records are maps, and the model's writes keep one per name, so
    a create's fields and a patch's keep one value per name (`normalise`, `Fields.set`).) -/
def Fields.valuesOf (fs : Fields) (f : FieldId) : List Val :=
  (fs.filter (fun fv => fv.1 == f)).map Prod.snd

/-! ## Unique fields and composite constraints

A unique field, and each composite constraint, is a set of fields on which no two records of one
type may agree. Two records agree on a set of fields when each field holds a keyed value (present
and not a list) in both, and the same one. A composite constraint whose fields are not all present
in a record does not apply to it, which `Val.keyed` gives for free. A unique field is the one-field
case of this, so one notion serves both. The records a clash is checked against are those of the
type being written, which is the list `rs` the functions below take. -/

/-- A value a unique constraint can clash on: present, and not a list. A list is never unique
    (the kernel refuses a unique list field), so no list clashes. -/
def Val.keyed : Val → Bool
  | .null => false
  | .list _ => false
  | _ => true

/-- Two records agree on the fields `us`: each holds a keyed value in each of them, and the same. -/
def agreeOn (us : List FieldId) (fs gs : Fields) : Bool :=
  us.all (fun f => (fs.get f).keyed && fs.get f == gs.get f)

/-- Some two distinct records of the list agree on the fields `us`: the stored data violates the
    constraint. -/
def dupFields (rs : Records) (us : List FieldId) : Bool :=
  rs.any (fun q₁ => rs.any (fun q₂ => q₁.1 != q₂.1 && agreeOn us q₁.2.fields q₂.2.fields))

/-- The value `v`, keyed, that another record, not the one with id `self`, holds in field `f`. -/
def clashAt (rs : Records) (self : RecordId) (f : FieldId) (v : Val) : Bool :=
  v.keyed && rs.any (fun q => q.1 != self && q.2.fields.get f == v)

/-- What is wrong with one value of a field defined by `d`, if anything, in the kernel's order:
    over the string or list bound, a unique value over its bound, the wrong type, a unique value
    another record holds (only when `uniq`), a reference or a reference element naming no record of
    its target type. `uniq` says whether the unique checks are made: a write that is refused for
    some other reason and whose unique checks are not made is reported as it would be without
    them. -/
def valueProblem (L : Limits) (uniq : Bool) (env : Env) (rs : Records) (self : RecordId)
    (f : FieldId) (d : FieldDef) (v : Val) : Option Err :=
  if v.overLimit L then some .limitExceeded
  else if d.unique && decide (L.maxUnique < v.size) then some .limitExceeded
  else if !v.hasType d.ty then some .invalid
  else if uniq && d.unique && clashAt rs self f v then some .uniqueViolated
  else if !v.resolves env d.ty then some .danglingReference
  else none

/-- What is wrong with a record's values of one field of the schema, if anything. -/
def fieldProblem (L : Limits) (uniq : Bool) (env : Env) (rs : Records) (self : RecordId)
    (fs : Fields) (fd : FieldId × FieldDef) : Option Err :=
  if fd.2.required && (fs.valuesOf fd.1).isEmpty then some .invalid
  else (fs.valuesOf fd.1).findSome? (valueProblem L uniq env rs self fd.1 fd.2)

/-- The composite unique constraints, after the fields: a record that agrees with another record
    of its type on every field of one of them is refused (when `uniq`). -/
def compositeProblem (uniq : Bool) (rs : Records) (self : RecordId) (fs : Fields)
    (us : List (List FieldId)) : Option Err :=
  if uniq && us.any (fun u => rs.any (fun q => q.1 != self && agreeOn u fs q.2.fields)) then
    some .uniqueViolated
  else none

/-- The refusal for writing record `r` as record `self` of a type with schema `schema` and
    composite constraints `us`, among the records `rs` of that type and `env` for the targets of
    its references, or `none` if it may be written, with the unique checks made when `uniq`.

    When several things are wrong, which refusal is given is part of the outcome: a record over
    the record bound comes first, then a field the schema does not know, then the fields of the
    schema in schema order, each refused for being missing, over a bound, of the wrong type, a
    unique value held by another record, or naming no record; then the composite constraints.
    (The kernel's schema is ordered by field id, and the correspondence fixtures declare it in that
    order.) -/
def problem (L : Limits) (schema : Schema) (us : List (List FieldId)) (uniq : Bool) (env : Env)
    (rs : Records) (self : RecordId) (r : Record) : Option Err :=
  if decide (L.maxRecord < r.size) then some .limitExceeded
  else if r.fields.all (fun fv => (schema.lookup fv.1).isSome) then
    match schema.findSome? (fieldProblem L uniq env rs self r.fields) with
    | some e => some e
    | none => compositeProblem uniq rs self r.fields us
  else some .invalid

/-! ## References and deletes -/

/-- Whether a value names record `id`: it is a reference to it, or a list holding one. -/
def Val.refersTo (id : RecordId) : Val → Bool
  | .ref x => x == id
  | .list vs => vs.contains (Elem.ref id)
  | _ => false

/-- A field of a record of a type with schema `schema` names record `id` of object type `t`: the
    field's type targets `t`, and its value names `id`. -/
def fieldRefers (schema : Schema) (t : TypeId) (id : RecordId) (fv : FieldId × Val) : Bool :=
  match schema.lookup fv.1 with
  | some d => d.ty.target? == some t && fv.2.refersTo id
  | none => false

/-- A record of a type with schema `schema` names record `id` of object type `t` through a field. -/
def recRefers (schema : Schema) (t : TypeId) (id : RecordId) (r : Record) : Bool :=
  r.fields.any (fieldRefers schema t id)

/-- A reference to a deleted record through field `f`, held in value `v`, forbids the delete:
    some definition of `f` restricts deletion, or is required while `v` is a single reference. A
    required list never forbids it: the element is removed and the list stays. (The kernel asks
    whether the field's type is a list; on a record of the declared types that is whether the
    value is one.) -/
def forbidsDelete (schema : Schema) (f : FieldId) (v : Val) : Bool :=
  schema.any (fun fd => fd.1 == f && ((fd.2.required && !v.isList) || fd.2.ty.restricts))

/-- Record `q` of object type `S`, with schema `schema`, refers to record `id` of type `t` through a
    field that forbids its deletion. A record does not block its own delete. -/
def blocks (S : TypeId) (schema : Schema) (t : TypeId) (id : RecordId)
    (q : RecordId × Record) : Bool :=
  !(S == t && q.1 == id) &&
    q.2.fields.any (fun fv => fieldRefers schema t id fv && forbidsDelete schema fv.1 fv.2)

/-- A value with the references to `id` taken out: a single reference to it is gone (`none`), a
    list loses those elements and stays, possibly empty. -/
def Val.dropRef (id : RecordId) : Val → Option Val
  | .ref x => if x = id then none else some (.ref x)
  | .list vs => some (.list (vs.filter (fun v => v != Elem.ref id)))
  | v => some v

/-- The fields with the references to `id` cleared, through the fields that name records of type
    `t`. -/
def clearFields (schema : Schema) (t : TypeId) (id : RecordId) (fs : Fields) : Fields :=
  fs.filterMap (fun fv =>
    if fieldRefers schema t id fv then (fv.2.dropRef id).map (fun v => (fv.1, v))
    else some fv)

/-- A record of a type with schema `schema` that refers to record `id` of type `t`, with every such
    reference cleared and its version raised; any other record as it is. -/
def clearRef (schema : Schema) (t : TypeId) (id : RecordId) (r : Record) : Record :=
  if recRefers schema t id r then
    { r with fields := clearFields schema t id r.fields, version := r.version + 1 }
  else r

/-! ## Object types and the tenant -/

/-- One object type of a tenant: its schema, its composite unique constraints and its records. -/
structure ObjType where
  schema : Schema
  /-- The composite unique constraints, each a list of fields (the kernel's `UniqueDef`). -/
  uniques : List (List FieldId) := []
  records : Records := []
  /-- The constraints of the type, in the kernel's order (by constraint id): each must not be
      `false` on a record of the type, checked after every other check of a write (slice 5). -/
  constraints : List Expr := []
  deriving Repr

/-- Everything one tenant owns. -/
structure Tenant where
  /-- The object types, by id. Each id occurs once. -/
  types : List (TypeId × ObjType)
  /-- The users holding roles directly (the kernel's `members`), each with its roles. The owner is a
      member holding `ownerRole`; the editors and writers of the fixture hold their roles so. -/
  members : List (PrincipalId × List RoleId)
  /-- What each role grants: a capability, or an action on the records of an object type (the
      kernel's role permissions). -/
  grants : List (RoleId × Grant)
  /-- The policies of the tenant (the kernel's `policies`). -/
  policies : List Policy
  /-- The id the next created record receives, whatever its type. -/
  nextId : Nat
  /-- The size bounds. The kernel's are constants (`Limits.kernel`). -/
  limits : Limits := Limits.kernel
  deriving Repr

/-- The whole state: each tenant id has a tenant or does not. -/
abbrev World := TenantId → Option Tenant

/-- The object type `ty` of the tenant, if it has one. -/
def Tenant.typ? (t : Tenant) (ty : TypeId) : Option ObjType := t.types.lookup ty

/-- The records of each object type of the tenant. -/
def Tenant.env (t : Tenant) : Env := fun ty => ((t.typ? ty).map ObjType.records).getD []

/-- The record of type `ty` with id `id`, if the tenant holds it. -/
def Tenant.recordAt (t : Tenant) (ty : TypeId) (id : RecordId) : Option Record :=
  (t.typ? ty).bind (fun o => o.records.lookup id)

/-- The object types with the entry of `ty` replaced by `o`: the first entry of that type, which is the
    one the lookup of `ty` finds (a tenant holds one entry per type, the kernel's map). -/
def putTypeList : List (TypeId × ObjType) → TypeId → ObjType → List (TypeId × ObjType)
  | [], _, _ => []
  | (k, p) :: es, ty, o => if k == ty then (ty, o) :: es else (k, p) :: putTypeList es ty o

/-- The tenant with object type `ty` replaced by `o`. -/
def Tenant.putType (t : Tenant) (ty : TypeId) (o : ObjType) : Tenant :=
  { t with types := putTypeList t.types ty o }

/-- Some record of any type of the tenant refers to record `id` of type `ty` through a field that
    forbids its deletion. -/
def Tenant.blockedDelete (t : Tenant) (ty : TypeId) (id : RecordId) : Bool :=
  t.types.any (fun e => e.2.records.any (blocks e.1 e.2.schema ty id))

/-- Every record of a table with the references to record `id` of type `ty` cleared. -/
def delClear (schema : Schema) (ty : TypeId) (id : RecordId) (rs : Records) : Records :=
  rs.map (fun q => (q.1, clearRef schema ty id q.2))

/-- The object type `j` (with contents `o`) after record `id` of type `ty` is deleted: in its own
    table the record is gone, and in every table the references to it are cleared. -/
def delObj (ty : TypeId) (id : RecordId) (j : TypeId) (o : ObjType) : ObjType :=
  let kept := if j == ty then o.records.filter (fun q => q.1 != id) else o.records
  { o with records := delClear o.schema ty id kept }

/-- The tenant after record `id` of type `ty` is deleted. -/
def Tenant.afterDelete (t : Tenant) (ty : TypeId) (id : RecordId) : Tenant :=
  { t with types := t.types.map (fun e => (e.1, delObj ty id e.1 e.2)) }

/-- Whether a delete of record `id` of type `ty` would leave a record it rewrites violating a
    constraint of its type (WG-5664). The rewritten records are the other records that refer to
    `id` through a clearing field; a rewritten record breaks a constraint when one of its constraints
    is `false` or cannot be evaluated (`constraintProblem` says so). The kernel refuses such a delete
    with `constraintViolated`, as a restrict refuses it with `stillReferenced`, so the invariant
    `Tenant.Satisfied` survives every accepted delete. -/
def Tenant.clearBreaks (t : Tenant) (ty : TypeId) (id : RecordId) : Bool :=
  t.types.any (fun e => e.2.records.any (fun q =>
    !(e.1 == ty && q.1 == id) && recRefers e.2.schema ty id q.2 &&
      (constraintProblem (storedEnv (clearRef e.2.schema ty id q.2)) e.2.constraints).isSome))

/-- The roles a principal holds (the kernel's `roles_of`): none for the system, `public` for an
    anonymous caller, and for a user its direct roles and `public`. -/
def Tenant.rolesOf (t : Tenant) : Principal → List RoleId
  | .system => []
  | .anonymous => [publicRole]
  | .user p => publicRole :: (t.members.lookup p).getD []

/-- A policy's condition holds on the environment: absent, or an expression that is exactly `true`.
    `false`, `null` and an evaluation error do not (the kernel's `authorize`). -/
def condHolds (env : ExprEnv) : Option Expr → Bool
  | none => true
  | some c => match evalExpr env c with
    | .ok (.bool true) => true
    | _ => false

/-- The decision of the kernel's `authorize`, without scopes: the owner may do anything; otherwise
    a role the principal holds grants the action on the object type, or a policy of such a role
    names the action on the resource and its condition holds. Default deny. -/
def Tenant.allows (t : Tenant) (p : Principal) (a : Action) (r : Resource) (env : ExprEnv) : Bool :=
  let roles := t.rolesOf p
  roles.contains ownerRole ||
    (match r with
     | .object ty => a != .manage &&
         t.grants.any (fun g => roles.contains g.1 && g.2 == .object ty a)
     | _ => false) ||
    t.policies.any (fun pol => roles.contains pol.role && pol.actions.contains a &&
      pol.resource.covers r && condHolds env pol.condition)

/-- A principal is permitted an action on a resource in an environment: the owner role, a grant of a role it
    holds that names the action on the object type (never a manage action), or a policy of such a role
    that names the action, reaches the resource and has a holding condition. -/
def Tenant.Permits (t : Tenant) (p : Principal) (a : Action) (r : Resource) (env : ExprEnv) : Prop :=
  ownerRole ∈ t.rolesOf p ∨
  (∃ ty, r = .object ty ∧ a ≠ .manage ∧ ∃ g ∈ t.grants, g.1 ∈ t.rolesOf p ∧ g.2 = .object ty a) ∨
  ∃ pol ∈ t.policies, pol.role ∈ t.rolesOf p ∧ a ∈ pol.actions ∧ pol.resource.covers r = true ∧
    condHolds env pol.condition = true

/-- A policy's resource reaches the one a decision names: the same resource, or every object type
    for `anyObject` when the decision names an object type. -/
theorem Resource.covers_iff (x r : Resource) :
    x.covers r = true ↔ x = r ∨ (x = .anyObject ∧ ∃ ty, r = .object ty) := by
  cases x <;> cases r <;> simp [Resource.covers]

/-- The decision is the permission: the Bool decision holds exactly when the principal is permitted
    the action on the resource (`Tenant.Permits`). -/
theorem Tenant.allows_iff (t : Tenant) (p : Principal) (a : Action) (r : Resource) (env : ExprEnv) :
    t.allows p a r env = true ↔ t.Permits p a r env := by
  unfold Tenant.allows Tenant.Permits
  cases r <;> simp [Bool.or_eq_true, Bool.and_eq_true, List.any_eq_true, Resource.covers_iff,
    condHolds, bne_iff_ne, beq_iff_eq, or_assoc, and_assoc]

/-- The environment an authorization decision is made in (the kernel's `Subject`): the record it
    tests (the stored one, else the new one), the stored record and the new one, and the user. -/
def subjectEnv (p : Principal) (old new : Option Record) : ExprEnv :=
  { record := old.or new, old := old, new := new, principal := p.userId? }

/-- May `p` create a record of type `ty` that would be `r`? -/
def canCreate (t : Tenant) (p : Principal) (ty : TypeId) (r : Record) : Bool :=
  t.allows p .create (.object ty) (subjectEnv p none (some r))

/-- May `p` update record `old` of type `ty`, which would become `new`? -/
def canUpdate (t : Tenant) (p : Principal) (ty : TypeId) (old new : Record) : Bool :=
  t.allows p .update (.object ty) (subjectEnv p (some old) (some new))

/-- May `p` delete record `old` of type `ty`? -/
def canDelete (t : Tenant) (p : Principal) (ty : TypeId) (old : Record) : Bool :=
  t.allows p .delete (.object ty) (subjectEnv p (some old) (some old))

/-- May `p` attempt a create on type `ty` under some grant or policy, whatever the condition of
    the policy and whatever the record? The kernel's `may_attempt`, asked before the defaults of
    the type are evaluated (WG-5665): a caller whom nothing could allow is refused `unauthorized`
    whether or not a default would fail. -/
def canAttemptCreate (t : Tenant) (p : Principal) (ty : TypeId) : Bool :=
  let roles := t.rolesOf p
  roles.contains ownerRole ||
    t.grants.any (fun g => roles.contains g.1 && g.2 == .object ty .create) ||
    t.policies.any (fun pol => roles.contains pol.role && pol.actions.contains .create &&
      pol.resource.covers (.object ty))

/-- A create that the record allows is one the caller could attempt. -/
theorem canCreate_attempt (t : Tenant) (p : Principal) (ty : TypeId) (r : Record)
    (h : canCreate t p ty r = true) : canAttemptCreate t p ty = true := by
  unfold canCreate Tenant.allows at h
  unfold canAttemptCreate
  simp only [Bool.or_eq_true, Bool.and_eq_true] at h ⊢
  rcases h with (h | h) | h
  · exact Or.inl (Or.inl h)
  · exact Or.inl (Or.inr h.2)
  · exact Or.inr (List.any_eq_true.mpr (by
      obtain ⟨pol, hp, hq⟩ := List.any_eq_true.mp h
      exact ⟨pol, hp, by simp only [Bool.and_eq_true] at hq ⊢; exact hq.1⟩))

/-- A caller whom no grant or policy could allow to create is allowed no record of the type. -/
theorem canCreate_false_of_attempt (t : Tenant) (p : Principal) (ty : TypeId) (r : Record)
    (h : canAttemptCreate t p ty = false) : canCreate t p ty r = false := by
  cases hc : canCreate t p ty r with
  | false => rfl
  | true => have := canCreate_attempt t p ty r hc; rw [h] at this; cases this

/-- May `p` perform `a` on type `ty` when there is no record to test the permission against? Only
    what does not depend on a record: a grant, or a policy with no condition. The kernel asks this
    of a record intent whose record is missing (`stored`), and of an unknown type (`object_def`). -/
def canWithoutRecord (t : Tenant) (p : Principal) (ty : TypeId) (a : Action) : Bool :=
  t.allows p a (.object ty) (subjectEnv p none none)

/-- May `p` change the schema (the kernel's `Manage` on the schema)? -/
def canManage (t : Tenant) (p : Principal) : Bool :=
  t.allows p .manage .schema (subjectEnv p none none)

/-- The refusal for an update or delete of a record that does not exist. The permission is asked
    first: one who could not act on a record of the type gets `unauthorized`, the same as for a
    record that exists, and only one who could learns that this one is missing (KRN-1080). -/
def missingRecord (t : Tenant) (p : Principal) (ty : TypeId) (a : Action) : Err :=
  if canWithoutRecord t p ty a then .notFound else .unauthorized

/-- The refusal for an intent that names an object type the tenant does not have. The kernel asks
    whether the caller could act on that type with the action before it says the type is missing:
    one who could gets `unknownObject`, anyone else `unauthorized`, whether or not the type exists
    (`object_def`). -/
def unknownType (t : Tenant) (p : Principal) (ty : TypeId) (a : Action) : Err :=
  if canWithoutRecord t p ty a then .unknownObject else .unauthorized

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

/-- The fields a create writes: the named ones that are not `null`, then the defaults of the fields
    of the schema that the intent does not name at all (`defaultsOf`). A field named as `null` gets
    no default, the kernel's rule. An evaluation error in a default is an error of the create. -/
def withDefaults (env : ExprEnv) (schema : Schema) (fs : Fields) : Except Unit Fields :=
  (defaultsOf env schema fs).map (fun ds => normalise fs ++ ds)

/-- The record a create by `p` writes, at version 1, or the error of its defaults. -/
def newRecord (schema : Schema) (p : Principal) (fs : Fields) : Except Unit Record :=
  (withDefaults { principal := p.userId? } schema fs).map
    (fun fs' => { owner := p.userId?, version := 1, fields := fs' })

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

/-- Whether the records of one object type would violate its schema: some record does not conform
    (a required field missing, a value of the wrong type); a unique field is held by two records,
    or holds a value over the unique bound; a composite constraint is held by two records. -/
def tableViolated (L : Limits) (o : ObjType) : Bool :=
  !(o.records.all (fun q => conforms o.schema q.2)) ||
    o.schema.any (fun fd => fd.2.unique &&
      (dupFields o.records [fd.1] ||
        o.records.any (fun q => decide (L.maxUnique < (q.2.fields.get fd.1).size)))) ||
    o.uniques.any (fun u => dupFields o.records u) ||
    o.records.any (fun q => (constraintProblem (storedEnv q.2) o.constraints).isSome)

/-- Whether the records of any object type of the tenant would violate the schema of their type.
    The kernel's `Reject` checks every record of every type against the resulting schema, so a
    violation in any type refuses a strict change to any type. -/
def schemaViolated (L : Limits) (types : List (TypeId × ObjType)) : Bool :=
  types.any (fun e => tableViolated L e.2)

/-- The one refusal every strict schema change makes: `Reject` refuses the change when the records
    of the object types it re-validates would violate their schema (`schemaViolated`, over the
    types given, which the caller scopes with `scopedTypes`). A lax change never refuses. -/
def schemaChangeRefused (L : Limits) (types : List (TypeId × ObjType)) (strict : Bool) : Bool :=
  strict && schemaViolated L types

/-- The object types a field refers to (`Ty.target?`, which the kernel's `ref_target` agrees with). -/
def fieldTargets (schema : Schema) (f : FieldId) : List TypeId :=
  match schema.lookup f with
  | some d => d.ty.target?.toList
  | none => []

/-- The object types a schema change re-validates: the types whose ids are listed (the kernel's
    `touched_objects`). A change to a field that refers to another type re-validates that type's
    records too; a change to a composite constraint re-validates only its own type. -/
def scopedTypes (types : List (TypeId × ObjType)) (ids : List TypeId) : List (TypeId × ObjType) :=
  types.filter (fun e => ids.contains e.1)

/-- A composite definition the schema can hold: two to four fields, none repeated, each one of the
    schema and not a list (the kernel's `unique_problems`). -/
def uniqueFieldsOk (schema : Schema) (fs : List FieldId) : Bool :=
  decide (2 ≤ fs.length) && decide (fs.length ≤ 4) && decide fs.Nodup &&
    fs.all (fun f => match schema.lookup f with
      | some d => !d.ty.isList
      | none => false)

inductive Intent where
  /-- Create a record of object type `ty`. A field of its schema the intent does not name is
      filled from its default. -/
  | create (ty : TypeId) (fields : Fields)
  /-- Patch a record of type `ty`: each named field takes the given value, a named `null` removes
      the field, fields not named are kept. `expected`, when given, must be the record's version. -/
  | update (ty : TypeId) (id : RecordId) (fields : Fields) (expected : Option Nat)
  /-- Delete a record of type `ty`. `expected`, when given, must be the record's version. -/
  | delete (ty : TypeId) (id : RecordId) (expected : Option Nat)
  /-- Make a field of type `ty` required. `strict` rejects the change if any record of the tenant
      would not conform; otherwise existing records are left as they are. -/
  | setRequired (ty : TypeId) (f : FieldId) (strict : Bool)
  /-- Make a field of type `ty` unique. `strict` rejects the change if existing records already
      agree on it; otherwise existing records are left as they are. -/
  | setUnique (ty : TypeId) (f : FieldId) (strict : Bool)
  /-- Define a composite unique constraint of type `ty` over these fields. Refused if existing
      records already agree on all of them. -/
  | defineUnique (ty : TypeId) (fs : List FieldId)
  deriving Repr

/-- The explicit environment of a step: which tenant, and who is acting. -/
structure Ctx where
  tenant : TenantId
  principal : Principal
  deriving Repr

/-- One intent against one tenant. An expected version is checked after the permission, so only
    one who may act on the record learns that it is at another version. A record intent that
    names an object type the tenant does not have is refused with `unknownObject` for the owner and
    with `unauthorized` for everyone else (`unknownType`), before the record is looked up. A schema
    change that names such a type is refused with `invalidSchema` for the owner and with
    `unauthorized` for everyone else. -/
def stepTenant (t : Tenant) (p : Principal) : Intent → Except Err Tenant
  | .create ty fs =>
    match t.typ? ty with
    | none => .error (unknownType t p ty .create)
    | some o =>
      -- The permission comes before the defaults (WG-5665, the kernel's `create_record`): a caller
      -- whom nothing could allow to create is refused whether or not a default would fail. A
      -- caller who could goes on to the defaults, and then asks for the decision on the record.
      if !canAttemptCreate t p ty then .error .unauthorized
      else
      match newRecord o.schema p fs with
      | .error _ => .error .evaluation
      | .ok r =>
        if !canCreate t p ty r then .error .unauthorized
        else
          match problem t.limits o.schema o.uniques true t.env o.records t.nextId r with
          | some e => .error e
          | none =>
            match constraintProblem (writeEnv p r none) o.constraints with
            | some e => .error e
            | none =>
              let o' := { o with records := (t.nextId, r) :: o.records }
              .ok { t.putType ty o' with nextId := t.nextId + 1 }
  | .update ty id fs ev =>
    match t.typ? ty with
    | none => .error (unknownType t p ty .update)
    | some o =>
      match o.records.lookup id with
      | none => .error (missingRecord t p ty .update)
      | some old =>
        let r := patched old fs
        if !canUpdate t p ty old r then .error .unauthorized
        else if versionClash ev old then .error .versionConflict
        else
          match problem t.limits o.schema o.uniques true t.env o.records id r with
          | some e => .error e
          | none =>
            match constraintProblem (writeEnv p r (some old)) o.constraints with
            | some e => .error e
            | none =>
              let rs' := o.records.map (fun q => if q.1 == id then (q.1, r) else q)
              .ok (t.putType ty { o with records := rs' })
  | .delete ty id ev =>
    match t.typ? ty with
    | none => .error (unknownType t p ty .delete)
    | some o =>
      match o.records.lookup id with
      | none => .error (missingRecord t p ty .delete)
      | some old =>
        if !canDelete t p ty old then .error .unauthorized
        else if versionClash ev old then .error .versionConflict
        else if t.blockedDelete ty id then .error .stillReferenced
        else if t.clearBreaks ty id then .error .constraintViolated
        else .ok (t.afterDelete ty id)
  | .setRequired ty f strict =>
    if !canManage t p then .error .unauthorized
    else
      match t.typ? ty with
      | none => .error .invalidSchema
      | some o =>
        let t' := t.putType ty { o with schema := makeRequired o.schema f }
        if schemaChangeRefused t.limits (scopedTypes t'.types (ty :: fieldTargets o.schema f)) strict then .error .migrationRejected
        else .ok t'
  | .setUnique ty f strict =>
    if !canManage t p then .error .unauthorized
    else
      match t.typ? ty with
      | none => .error .invalidSchema
      | some o =>
        match o.schema.lookup f with
        | none => .error .invalidSchema
        | some d =>
          if d.ty.isList then .error .invalidSchema
          else
            let t' := t.putType ty { o with schema := makeUnique o.schema f }
            if schemaChangeRefused t.limits (scopedTypes t'.types (ty :: fieldTargets o.schema f)) strict then .error .migrationRejected
            else .ok t'
  | .defineUnique ty fs =>
    if !canManage t p then .error .unauthorized
    else
      match t.typ? ty with
      | none => .error .invalidSchema
      | some o =>
        if !uniqueFieldsOk o.schema fs then .error .invalidSchema
        else if dupFields o.records fs then .error .uniqueViolated
        else
          let t' := t.putType ty { o with uniques := o.uniques ++ [fs] }
          if schemaChangeRefused t.limits (scopedTypes t'.types [ty]) true then .error .migrationRejected
          else .ok t'

/-- The transition function of the model: one intent, in one context, against the world. -/
def step (w : World) (i : Intent) (c : Ctx) : Except Err World :=
  match w c.tenant with
  | none => .error .noTenant
  | some t =>
    match stepTenant t c.principal i with
    | .error e => .error e
    | .ok t' => .ok (fun x => if x = c.tenant then some t' else w x)

/-- Whether the acting principal is allowed to make this intent at all: the object type exists,
    and the permission holds on the record (or, for a schema change, the owner). -/
def authorized (t : Tenant) (p : Principal) : Intent → Bool
  | .create ty fs => match t.typ? ty with
    | none => false
    | some o => match newRecord o.schema p fs with
      | .ok r => canCreate t p ty r
      | .error _ => false
  | .update ty id fs _ => match t.recordAt ty id with
    | some old => canUpdate t p ty old (patched old fs)
    | none => false
  | .delete ty id _ => match t.recordAt ty id with
    | some old => canDelete t p ty old
    | none => false
  | .setRequired ty _ _ => canManage t p && (t.typ? ty).isSome
  | .setUnique ty _ _ => canManage t p && (t.typ? ty).isSome
  | .defineUnique ty _ => canManage t p && (t.typ? ty).isSome

/-- The object types a schema intent re-validates, as the kernel's `touched_objects` gives them: the
    type it names, and, for a field, the types the field refers to. Other intents touch none. -/
def Intent.touched (t : Tenant) : Intent → List TypeId
  | .setRequired ty f _ => ty :: (match t.typ? ty with
      | some o => fieldTargets o.schema f
      | none => [])
  | .setUnique ty f _ => ty :: (match t.typ? ty with
      | some o => fieldTargets o.schema f
      | none => [])
  | .defineUnique ty _ => [ty]
  | _ => []

/-- A schema change that leaves existing records as they are is the one kind of intent that is
    allowed to break record conformance or uniqueness. Every other intent is strict. -/
def Intent.strict : Intent → Bool
  | .setRequired _ _ s => s
  | .setUnique _ _ s => s
  | _ => true

/-- The intents that change the schema and leave the records as they are. -/
def Intent.isSchema : Intent → Bool
  | .setRequired _ _ _ => true
  | .setUnique _ _ _ => true
  | .defineUnique _ _ => true
  | _ => false

/-- Referential integrity: every reference held by a record of the tenant, of any type, names an
    existing record of the type its field names. -/
def Tenant.RefsOk (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ q ∈ o.records, refsOk t.env o.schema q.2 = true

/-- Every record of the tenant conforms to the schema of its type. -/
def Tenant.Conform (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ q ∈ o.records, conforms o.schema q.2 = true

/-- Every record id of the tenant is below the counter. -/
def Tenant.IdsBelow (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ q ∈ o.records, q.1 < t.nextId

/-- The names are distinct: each object type is listed once, each schema names each field once,
    and each record holds each field at most once (the kernel's records are maps). -/
def Tenant.Distinct (t : Tenant) : Prop :=
  (t.types.map Prod.fst).Nodup ∧
    ∀ T o, t.typ? T = some o → (o.schema.map Prod.fst).Nodup ∧
      ∀ q ∈ o.records, (q.2.fields.map Prod.fst).Nodup

/-- No two distinct records of one type agree on a unique field, or on every field of a composite
    unique constraint of that type. -/
def Tenant.UniqueRecords (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ q₁ ∈ o.records, ∀ q₂ ∈ o.records, q₁.1 ≠ q₂.1 →
    (∀ fd ∈ o.schema, fd.2.unique = true → agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
    (∀ u ∈ o.uniques, agreeOn u q₁.2.fields q₂.2.fields = false)

/-- The state invariant of one tenant: every record conforms to the schema of its type, was given
    an id below the counter, refers only to existing records, and the names are distinct and the
    unique constraints hold. -/
def Tenant.WF (t : Tenant) : Prop :=
  t.Conform ∧ t.IdsBelow ∧ t.RefsOk ∧ t.Distinct ∧ t.UniqueRecords

def World.WF (w : World) : Prop :=
  ∀ x t, w x = some t → t.WF

def World.RefsOk (w : World) : Prop :=
  ∀ x t, w x = some t → t.RefsOk

/-! ## Constraints as an invariant (slice 5) -/

/-- An expression that reads no old record and no principal: its value on a record does not depend
    on which write checks it, so a constraint of this kind holds of a stored record exactly when
    it holds of the record as a write checked it. -/
def Expr.stateOnly : Expr → Bool
  | .lit _ => true
  | .field s _ => s != .old
  | .createdBy s => s != .old
  | .principal => false
  | .cmp _ a b => a.stateOnly && b.stateOnly
  | .and a b => a.stateOnly && b.stateOnly
  | .or a b => a.stateOnly && b.stateOnly
  | .not a => a.stateOnly
  | .isNull a => a.stateOnly

/-- Every constraint of the tenant holds of every stored record: none is `false` or unevaluable. -/
def Tenant.Satisfied (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ q ∈ o.records,
    constraintProblem (storedEnv q.2) o.constraints = none

/-- Every constraint of the tenant is state-only (`Expr.stateOnly`). -/
def Tenant.StateConstraints (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ c ∈ o.constraints, c.stateOnly = true

end Strata
