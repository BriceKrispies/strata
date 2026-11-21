/-
A model of schema evolution, independent of the kernel model in `Strata/Model.lean`.

What is modelled:

* A schema holds object types (each with fields) and dependents: readers (queries and
  policies) and views over readers. Every entity has a stable numeric id; names are labels
  kept beside the id. Dependents refer to ids, never to names, so a rename cannot make a
  dependent refer to a different thing and the ABA problem (a name deleted and reused for a
  different entity) cannot arise: a reference is resolved by id, and a change of name is a
  change to a label nobody resolves through.
* A dependent is an entity with a read set of (object id, field id) pairs and an evaluation
  function over the values read. A query reading fields, and a policy whose condition reads
  fields (possibly one reference away: the far field is one more pair in the read set), are
  both readers. A view reads queries and so is a reader of readers. Dependencies are
  stratified (views over readers over objects), so they are acyclic by construction.
* Data is a context: for each object id, one record, a map from field ids to values. The
  value of a dependent on a record set is its value on each record, so one context suffices
  to state what a dependent reads.
* Changes, a footprint (the cells a change touches), the impact (the dependents that read
  them, transitively through views), a classification, `apply`, `migrate`, and the
  acceptance rule.

Extension point for pins and projections (WG-5327): `accepted` is the single place that decides
whether a change goes through. A pin or a projection adds a conjunct there and nothing else
changes.
-/

namespace Strata.Evolution

/-- Field types. `ref target` points at an object type by id. -/
inductive Ty where
  | nat
  | int
  | str
  | bool
  | ref (target : Nat)
  deriving DecidableEq, Repr

/-- The type order for widening: a type is below itself, and `nat` is below `int`. A widening
    goes up this order, a narrowing goes down it. -/
def Ty.le : Ty → Ty → Bool
  | .nat, .int => true
  | a, b => decide (a = b)

/-- Values. A number is one value whatever the numeric type of its field, so the embedding of
    `nat` into `int` is the identity on values: widening never rewrites data. -/
inductive Val where
  | null
  | num (n : Int)
  | str (s : String)
  | bool (b : Bool)
  | ref (id : Nat)
  deriving DecidableEq, Repr

/-- The value a narrowing migration leaves behind: narrowing to `nat` clamps negatives to zero,
    which loses information. -/
def narrowVal (ty : Ty) : Val → Val
  | .num n => if ty = .nat ∧ n < 0 then .num 0 else .num n
  | v => v

structure Field where
  id : Nat
  name : String
  ty : Ty

structure Obj where
  id : Nat
  name : String
  fields : List Field

/-- A query or a policy. `reads` are the (object id, field id) pairs it depends on; `base` is
    the object it is evaluated over; `fn` computes its value from the values read, in order. -/
structure Reader where
  id : Nat
  isPolicy : Bool
  base : Nat
  reads : List (Nat × Nat)
  fn : List Val → Val

/-- A view over queries: `uses` are reader ids, `fn` combines their values in order. -/
structure View where
  id : Nat
  uses : List Nat
  fn : List Val → Val

inductive Dep where
  | reader (r : Reader)
  | view (v : View)

def Dep.id : Dep → Nat
  | .reader r => r.id
  | .view v => v.id

structure Schema where
  objs : List Obj
  deps : List Dep

/-- The data: for each object id, a record from field ids to values. -/
abbrev Data := Nat → Nat → Val

/-! ## Lookups and well-formedness -/

def hasObj (s : Schema) (t : Nat) : Prop := t ∈ s.objs.map Obj.id

def hasField (s : Schema) (o f : Nat) : Prop :=
  ∃ ob ∈ s.objs, ob.id = o ∧ f ∈ ob.fields.map Field.id

instance (s : Schema) (t : Nat) : Decidable (hasObj s t) := by unfold hasObj; infer_instance
instance (s : Schema) (o f : Nat) : Decidable (hasField s o f) := by unfold hasField; infer_instance

/-- A type is valid when a reference points at an object type that exists. -/
def validTy (s : Schema) : Ty → Prop
  | .ref t => hasObj s t
  | _ => True

instance (s : Schema) (t : Ty) : Decidable (validTy s t) := by
  unfold validTy; split <;> infer_instance

/-- The first reader with this id, if there is one. -/
def findR : List Dep → Nat → Option Reader
  | [], _ => none
  | .reader r :: ds, q => if r.id = q then some r else findR ds q
  | .view _ :: ds, q => findR ds q

/-- A dependent is valid in a schema when everything it reads resolves: a reader's object and
    every (object, field) pair it reads exist; a view's queries exist as readers. -/
def ValidDep (s : Schema) : Dep → Prop
  | .reader r => hasObj s r.base ∧ ∀ p ∈ r.reads, hasField s p.1 p.2
  | .view v => ∀ q ∈ v.uses, (findR s.deps q).isSome = true

instance (s : Schema) (d : Dep) : Decidable (ValidDep s d) := by
  unfold ValidDep; split <;> infer_instance

/-- Well-formed: ids are unique (among objects, among the fields of an object, among
    dependents), every reference targets an existing object type, and every dependent is valid.
    Acyclicity of definitions holds by stratification, since views read readers and readers read
    fields. -/
structure WF (s : Schema) : Prop where
  objIds : (s.objs.map Obj.id).Nodup
  fieldIds : ∀ ob ∈ s.objs, (ob.fields.map Field.id).Nodup
  tys : ∀ ob ∈ s.objs, ∀ fl ∈ ob.fields, validTy s fl.ty
  depIds : (s.deps.map Dep.id).Nodup
  deps : ∀ d ∈ s.deps, ValidDep s d

/-! ## Evaluation -/

def evalReader (r : Reader) (data : Data) : Val :=
  r.fn (r.reads.map fun p => data p.1 p.2)

/-- The value of a dependent on the data. A view takes the values of its readers, found by id
    in the schema; a missing reader reads as `null`. -/
def evalDep (s : Schema) (data : Data) : Dep → Val
  | .reader r => evalReader r data
  | .view v =>
    v.fn (v.uses.map fun q => match findR s.deps q with
      | some r => evalReader r data
      | none => .null)

/-! ## Changes -/

inductive Change where
  /-- Add a field to an object; existing records get `default`. -/
  | addField (o : Nat) (fld : Field) (default : Val)
  /-- Move a field up the type order. -/
  | widen (o f : Nat) (ty : Ty)
  /-- Change a field's name. Its id stays, so nothing that reads it is touched. -/
  | renameField (o f : Nat) (name : String)
  | removeField (o f : Nat)
  /-- Move a field down the type order; values are clamped. -/
  | narrow (o f : Nat) (ty : Ty)
  /-- Point a reference field at another object type; stored references are cleared. -/
  | retargetRef (o f t : Nat)
  | removeObject (o : Nat)
  /-- Remove a dependent (a query, a policy or a view) by id. -/
  | removeDep (q : Nat)

def updObj (o : Nat) (g : Obj → Obj) (objs : List Obj) : List Obj :=
  objs.map fun ob => if ob.id = o then g ob else ob

def updField (f : Nat) (g : Field → Field) (ob : Obj) : Obj :=
  { ob with fields := ob.fields.map fun fl => if fl.id = f then g fl else fl }

/-- The schema after the change. -/
def apply (c : Change) (s : Schema) : Schema :=
  match c with
  | .addField o fld _ =>
    { s with objs := updObj o (fun ob => { ob with fields := ob.fields ++ [fld] }) s.objs }
  | .widen o f ty => { s with objs := updObj o (updField f fun fl => { fl with ty := ty }) s.objs }
  | .renameField o f n =>
    { s with objs := updObj o (updField f fun fl => { fl with name := n }) s.objs }
  | .removeField o f =>
    { s with objs :=
        updObj o (fun ob => { ob with fields := ob.fields.filter (fun fl => fl.id != f) }) s.objs }
  | .narrow o f ty => { s with objs := updObj o (updField f fun fl => { fl with ty := ty }) s.objs }
  | .retargetRef o f t =>
    { s with objs := updObj o (updField f fun fl => { fl with ty := .ref t }) s.objs }
  | .removeObject o => { s with objs := s.objs.filter fun ob => ob.id != o }
  | .removeDep q => { s with deps := s.deps.filter fun d => d.id != q }

/-- The data after the change: cells that no longer mean what they meant are rewritten
    (defaulted, clamped, cleared), all other cells are as they were. -/
def migrate (c : Change) (data : Data) : Data :=
  match c with
  | .addField o fld v => fun o' f' => if o' = o ∧ f' = fld.id then v else data o' f'
  | .narrow o f ty => fun o' f' => if o' = o ∧ f' = f then narrowVal ty (data o' f') else data o' f'
  | .removeField o f => fun o' f' => if o' = o ∧ f' = f then .null else data o' f'
  | .retargetRef o f _ => fun o' f' => if o' = o ∧ f' = f then .null else data o' f'
  | .removeObject o => fun o' f' => if o' = o then .null else data o' f'
  | _ => data

/-! ## Footprint, impact, classification, acceptance -/

/-- The existing (object, field) cells the change touches. A rename touches none: it changes a
    label, and nothing resolves through a label. Adding a field touches no existing cell. -/
def footprint (c : Change) (s : Schema) : List (Nat × Nat) :=
  match c with
  | .addField .. => []
  | .widen o f _ => [(o, f)]
  | .renameField .. => []
  | .removeField o f => [(o, f)]
  | .narrow o f _ => [(o, f)]
  | .retargetRef o f _ => [(o, f)]
  | .removeObject o =>
    s.objs.flatMap fun ob => if ob.id = o then ob.fields.map fun fl => (o, fl.id) else []
  | .removeDep _ => []

/-- Dependent ids the change deletes. -/
def removedDeps : Change → List Nat
  | .removeDep q => [q]
  | _ => []

/-- Whether the change deletes object `b`. -/
def objGone : Change → Nat → Bool
  | .removeObject o, b => decide (b = o)
  | _, _ => false

/-- A reader is hit when it reads a touched cell or is evaluated over a deleted object. -/
def readerHit (c : Change) (s : Schema) (r : Reader) : Bool :=
  r.reads.any (fun p => decide (p ∈ footprint c s)) || objGone c r.base

/-- Whether the reader with this id exists and is hit. -/
def idHit (c : Change) (s : Schema) (q : Nat) : Bool :=
  match findR s.deps q with
  | some r => readerHit c s r
  | none => false

/-- A dependent is hit directly, or, for a view, through a reader it uses that is hit or
    deleted. -/
def depHit (c : Change) (s : Schema) : Dep → Bool
  | .reader r => readerHit c s r
  | .view v =>
    v.uses.any fun q => idHit c s q || decide (q ∈ removedDeps c)

/-- The ids of the dependents that read something the change touches, transitively. A
    dependent the change deletes itself is not in the impact; `removedDeps` lists it. -/
def impact (c : Change) (s : Schema) : List Nat :=
  (s.deps.filter (depHit c s)).map Dep.id

inductive Class where
  | compat
  | adaptable
  | breaking
  deriving DecidableEq, Repr

/-- `compat`: the data is not rewritten and no existing cell is reinterpreted (rename, widen).
    `adaptable`: the data is extended but nothing existing is rewritten (add a field).
    `breaking`: a cell is removed, lost or reinterpreted, or an entity is deleted. -/
def classify : Change → Class
  | .renameField .. => .compat
  | .widen .. => .compat
  | .addField .. => .adaptable
  | _ => .breaking

/-- The local checks that make a change meaningful on a schema: its targets exist, new ids are
    fresh, new references resolve, the type moves the right way, and deleting an object leaves
    no reference to it. -/
def pre (c : Change) (s : Schema) : Prop :=
  match c with
  | .addField o fld _ =>
    hasObj s o ∧ (∀ ob ∈ s.objs, ob.id = o → fld.id ∉ ob.fields.map Field.id) ∧ validTy s fld.ty
  | .widen o f ty =>
    hasField s o f ∧ validTy s ty ∧
      ∀ ob ∈ s.objs, ob.id = o → ∀ fl ∈ ob.fields, fl.id = f → Ty.le fl.ty ty = true
  | .renameField o f _ => hasField s o f
  | .removeField o f => hasField s o f
  | .narrow o f ty =>
    hasField s o f ∧ validTy s ty ∧
      ∀ ob ∈ s.objs, ob.id = o → ∀ fl ∈ ob.fields, fl.id = f → Ty.le ty fl.ty = true ∧ ty ≠ fl.ty
  | .retargetRef o f t =>
    hasField s o f ∧ hasObj s t ∧
      ∀ ob ∈ s.objs, ob.id = o → ∀ fl ∈ ob.fields, fl.id = f → ∃ t0, fl.ty = .ref t0
  | .removeObject o =>
    hasObj s o ∧ ∀ ob ∈ s.objs, ob.id ≠ o → ∀ fl ∈ ob.fields, fl.ty ≠ .ref o
  | .removeDep q => ∃ d ∈ s.deps, d.id = q

/-- The acceptance rule: the local checks pass and, for a breaking change, no dependent reads
    what it breaks. Pins and projections (WG-5327) are further conjuncts here. -/
def accepted (c : Change) (s : Schema) : Prop :=
  pre c s ∧ (classify c = .breaking → impact c s = [])

end Strata.Evolution
