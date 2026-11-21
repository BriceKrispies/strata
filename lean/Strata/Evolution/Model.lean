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

Adaptable changes (rename, move a field, total split) come with a `rewrite` of the dependents.
A dependent reads through `Read` expressions, so a rewrite substitutes the reads of a moved or
split field and leaves the dependent's function alone. A dependent may be pinned to an earlier
version of the schema; `accepted` admits a breaking change whose impact is wholly pinned when the
change has a left inverse, and `project` is then the read-only view of the new data in the old
shape (a lens get). Changes can be compared by the entities they touch (`ents`): disjoint changes
commute.
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

/-- The data: for each object id, a record from field ids to values. -/
abbrev Data := Nat → Nat → Val

structure Field where
  id : Nat
  name : String
  ty : Ty

structure Obj where
  id : Nat
  name : String
  fields : List Field

/-- What a reader reads: a cell (object id, field id), or a combination of two reads. A cell is
    the whole of what an author writes; a combination appears when a rewrite replaces the read of
    a split field by the combination of its parts. -/
inductive Read where
  | cell (o f : Nat)
  | join (jn : Val → Val → Val) (a b : Read)

/-- The cells a read depends on. -/
def Read.cells : Read → List (Nat × Nat)
  | .cell o f => [(o, f)]
  | .join _ a b => a.cells ++ b.cells

def Read.eval (data : Data) : Read → Val
  | .cell o f => data o f
  | .join jn a b => jn (a.eval data) (b.eval data)

/-- A query or a policy. `reads` are what it depends on; `base` is the object it is evaluated
    over; `fn` computes its value from the values read, in order. -/
structure Reader where
  id : Nat
  isPolicy : Bool
  base : Nat
  reads : List Read
  fn : List Val → Val

/-- Every cell the reader depends on. -/
def Reader.cells (r : Reader) : List (Nat × Nat) := r.reads.flatMap Read.cells

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

/-- `pins` are the ids of the dependents pinned to the version of the schema they were validated
    against: the dependents a breaking change may leave behind (reading through a projection). -/
structure Schema where
  objs : List Obj
  deps : List Dep
  pins : List Nat := []

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
  | .reader r => hasObj s r.base ∧ ∀ p ∈ r.cells, hasField s p.1 p.2
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
  r.fn (r.reads.map (Read.eval data))

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
  /-- Change a field's name. Its id stays, so a dependent's reads still resolve. -/
  | renameField (o f : Nat) (name : String)
  | removeField (o f : Nat)
  /-- Move a field down the type order; values are clamped. -/
  | narrow (o f : Nat) (ty : Ty)
  /-- Point a reference field at another object type; stored references are cleared. -/
  | retargetRef (o f t : Nat)
  | removeObject (o : Nat)
  /-- Remove a dependent (a query, a policy or a view) by id. -/
  | removeDep (q : Nat)
  /-- Move field `f` of object `o` to object `o'` along the reference field `r` of `o` that
      points at `o'`. The field arrives as `fld` (a fresh id in `o'`, the same type); a record of
      `o` and the record of `o'` it references are read together as one context, so the value
      is copied across and the old cell is cleared. -/
  | moveField (o f r o' : Nat) (fld : Field)
  /-- A total split: field `f` of `o` becomes the fields `f1` and `f2`, with `sp` giving their
      values from the original and `jn` combining them back, `jn (sp v) = v` for every `v`. -/
  | splitField (o f : Nat) (f1 f2 : Field) (sp : Val → Val × Val) (jn : Val → Val → Val)
  /-- Re-encode a field: its type becomes `ty` and each stored value `v` becomes `enc v`. Readers
      see a reinterpreted value, so it is breaking; `dec` is the way back, a left inverse of
      `enc` when `∀ v, dec (enc v) = v`. -/
  | retype (o f : Nat) (ty : Ty) (enc dec : Val → Val)

def updObj (o : Nat) (g : Obj → Obj) (objs : List Obj) : List Obj :=
  objs.map fun ob => if ob.id = o then g ob else ob

def updField (f : Nat) (g : Field → Field) (ob : Obj) : Obj :=
  { ob with fields := ob.fields.map fun fl => if fl.id = f then g fl else fl }

/-- The object types after the change. -/
def applyObjs (c : Change) (objs : List Obj) : List Obj :=
  match c with
  | .addField o fld _ => updObj o (fun ob => { ob with fields := ob.fields ++ [fld] }) objs
  | .widen o f ty => updObj o (updField f fun fl => { fl with ty := ty }) objs
  | .renameField o f n => updObj o (updField f fun fl => { fl with name := n }) objs
  | .removeField o f =>
    updObj o (fun ob => { ob with fields := ob.fields.filter (fun fl => fl.id != f) }) objs
  | .narrow o f ty => updObj o (updField f fun fl => { fl with ty := ty }) objs
  | .retargetRef o f t => updObj o (updField f fun fl => { fl with ty := .ref t }) objs
  | .removeObject o => objs.filter fun ob => ob.id != o
  | .removeDep _ => objs
  | .moveField o f _ o' fld =>
    updObj o' (fun ob => { ob with fields := ob.fields ++ [fld] })
      (updObj o (fun ob => { ob with fields := ob.fields.filter (fun fl => fl.id != f) }) objs)
  | .splitField o f f1 f2 _ _ =>
    updObj o (fun ob => { ob with fields := ob.fields.filter (fun fl => fl.id != f) ++ [f1, f2] })
      objs
  | .retype o f ty _ _ => updObj o (updField f fun fl => { fl with ty := ty }) objs

/-- Dependent ids the change deletes. -/
def removedDeps : Change → List Nat
  | .removeDep q => [q]
  | _ => []

/-- Whether the schema keeps a dependent: only a deleted one is dropped. -/
def keepDep (c : Change) (d : Dep) : Bool := decide (d.id ∉ removedDeps c)

/-- Where a read of cell `(o1, f1)` goes under the change: a moved field is read from its new
    cell, a split field is read as the combination of its two parts, every other cell is read
    as before. -/
def cellSub : Change → Nat → Nat → Read
  | .moveField o f _ o' fld, o1, f1 => if o1 = o ∧ f1 = f then .cell o' fld.id else .cell o1 f1
  | .splitField o f f1 f2 _ jn, o1, f' =>
    if o1 = o ∧ f' = f then .join jn (.cell o f1.id) (.cell o f2.id) else .cell o1 f'
  | _, o1, f1 => .cell o1 f1

def Read.subst (c : Change) : Read → Read
  | .cell o f => cellSub c o f
  | .join jn a b => .join jn (a.subst c) (b.subst c)

/-- The rewrite of a reader for a change: every read is substituted. The reader's function is
    untouched; ids, base and kind are untouched. -/
def rewriteReader (c : Change) (r : Reader) : Reader :=
  { r with reads := r.reads.map (Read.subst c) }

/-- The rewrite of a dependent for a change. A view reads readers by id and is not touched. Only
    a move and a split rewrite anything: a rename keeps the ids its dependents resolve through,
    so its rewrite is the identity. -/
def rewrite (c : Change) : Dep → Dep
  | .reader r => .reader (rewriteReader c r)
  | .view v => .view v

/-- The schema after the change: new object types, the surviving dependents rewritten. Pins are
    kept: a pin ends when its dependent is migrated, which is not a schema change. -/
def apply (c : Change) (s : Schema) : Schema :=
  { s with objs := applyObjs c s.objs, deps := (s.deps.filter (keepDep c)).map (rewrite c) }

/-- The data after the change: cells that no longer mean what they meant are rewritten
    (defaulted, clamped, cleared, copied, re-encoded), all other cells are as they were. -/
def migrate (c : Change) (data : Data) : Data :=
  match c with
  | .addField o fld v => fun o' f' => if o' = o ∧ f' = fld.id then v else data o' f'
  | .narrow o f ty => fun o' f' => if o' = o ∧ f' = f then narrowVal ty (data o' f') else data o' f'
  | .removeField o f => fun o' f' => if o' = o ∧ f' = f then .null else data o' f'
  | .retargetRef o f _ => fun o' f' => if o' = o ∧ f' = f then .null else data o' f'
  | .removeObject o => fun o' f' => if o' = o then .null else data o' f'
  | .moveField o f _ o' fld => fun o1 f1 =>
    if o1 = o' ∧ f1 = fld.id then data o f else if o1 = o ∧ f1 = f then .null else data o1 f1
  | .splitField o f f1 f2 sp _ => fun o1 f' =>
    if o1 = o ∧ f' = f1.id then (sp (data o f)).1
    else if o1 = o ∧ f' = f2.id then (sp (data o f)).2
    else if o1 = o ∧ f' = f then .null else data o1 f'
  | .retype o f _ enc _ => fun o' f' => if o' = o ∧ f' = f then enc (data o' f') else data o' f'
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
  | .moveField o f _ _ _ => [(o, f)]
  | .splitField o f .. => [(o, f)]
  | .retype o f .. => [(o, f)]

/-- Whether the change deletes object `b`. -/
def objGone : Change → Nat → Bool
  | .removeObject o, b => decide (b = o)
  | _, _ => false

/-- A reader is hit when it reads a touched cell or is evaluated over a deleted object. -/
def readerHit (c : Change) (s : Schema) (r : Reader) : Bool :=
  r.cells.any (fun p => decide (p ∈ footprint c s)) || objGone c r.base

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

/-- `compat`: nothing existing is rewritten or reinterpreted and no dependent needs a rewrite
    (add a field, widen).
    `adaptable`: the change is applied together with a rewrite of the dependents (rename, which
    rewrites by the identity; move a field; split a field).
    `breaking`: a cell is removed, lost or reinterpreted, or an entity is deleted. -/
def classify : Change → Class
  | .addField .. => .compat
  | .widen .. => .compat
  | .renameField .. => .adaptable
  | .moveField .. => .adaptable
  | .splitField .. => .adaptable
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
  | .moveField o f r o' fld =>
    hasField s o f ∧ hasField s o r ∧ r ≠ f ∧ hasObj s o' ∧ validTy s fld.ty ∧
      (∀ ob ∈ s.objs, ob.id = o' → fld.id ∉ ob.fields.map Field.id) ∧
      ∀ ob ∈ s.objs, ob.id = o →
        (∀ fl ∈ ob.fields, fl.id = r → fl.ty = .ref o') ∧
        ∀ fl ∈ ob.fields, fl.id = f → fl.ty = fld.ty
  | .splitField o f f1 f2 sp jn =>
    hasField s o f ∧ f1.id ≠ f2.id ∧ validTy s f1.ty ∧ validTy s f2.ty ∧
      (∀ ob ∈ s.objs, ob.id = o →
        f1.id ∉ ob.fields.map Field.id ∧ f2.id ∉ ob.fields.map Field.id) ∧
      ∀ v, jn (sp v).1 (sp v).2 = v
  | .retype o f ty _ _ => hasField s o f ∧ validTy s ty

/-- Whether the change has a left inverse on the data its dependents read: the old shape can be
    rebuilt from the new data, so a projection is defined. Removals, a narrowing (it clamps) and
    clearing a reference lose what they touch and have none. -/
def hasLeftInverse : Change → Prop
  | .addField .. => True
  | .widen .. => True
  | .renameField .. => True
  | .moveField .. => True
  | .splitField _ _ _ _ sp jn => ∀ v, jn (sp v).1 (sp v).2 = v
  | .retype _ _ _ enc dec => ∀ v, dec (enc v) = v
  | _ => False

/-- The acceptance rule: the local checks pass and, for a breaking change, every dependent it
    reaches is pinned and the change has a projection for what the dependent reads (a left
    inverse). With no pins, a breaking change is accepted exactly when its impact is empty. -/
def accepted (c : Change) (s : Schema) : Prop :=
  pre c s ∧ (classify c = .breaking → ∀ q ∈ impact c s, q ∈ s.pins ∧ hasLeftInverse c)

/-! ## Projections (the lens get) -/

/-- The read-only view of the new data in the shape the pinned dependent expects. For a change
    with `hasLeftInverse` it undoes what `migrate` did to the touched cells; the other changes
    have nothing to undo or no inverse, and the view is the data itself. -/
def project : Change → Data → Data
  | .moveField o f _ o' fld, d => fun o1 f1 => if o1 = o ∧ f1 = f then d o' fld.id else d o1 f1
  | .splitField o f f1 f2 _ jn, d =>
    fun o1 f' => if o1 = o ∧ f' = f then jn (d o f1.id) (d o f2.id) else d o1 f'
  | .retype o f _ _ dec, d => fun o1 f' => if o1 = o ∧ f' = f then dec (d o1 f') else d o1 f'
  | _, d => d

/-! ## What a change touches -/

inductive Ent where
  | cell (o f : Nat)
  | obj (o : Nat)
  | dep (q : Nat)
  deriving DecidableEq

/-- The cells, object types and dependents the change touches other than through its impact:
    labels too (a rename touches its field), and the field list of an object whose fields are
    added or removed. -/
def ents0 (c : Change) (s : Schema) : List Ent :=
  match c with
  | .addField o fld _ => [.obj o, .cell o fld.id]
  | .widen o f _ => [.cell o f]
  | .renameField o f _ => [.cell o f]
  | .removeField o f => [.obj o, .cell o f]
  | .narrow o f _ => [.cell o f]
  | .retargetRef o f _ => [.cell o f]
  | .removeObject o => .obj o :: (footprint c s).map fun p => .cell p.1 p.2
  | .removeDep q => [.dep q]
  | .moveField o f r o' fld => [.obj o, .cell o f, .cell o r, .obj o', .cell o' fld.id]
  | .splitField o f f1 f2 _ _ => [.obj o, .cell o f, .cell o f1.id, .cell o f2.id]
  | .retype o f _ _ _ => [.cell o f]

/-- The footprint of a change in the sense of the specification: what it touches together with
    its impact set. -/
def ents (c : Change) (s : Schema) : List Ent :=
  ents0 c s ++ (impact c s).map Ent.dep

/-- Two changes are disjoint on a schema when their footprints share nothing. -/
def Disjoint (c₁ c₂ : Change) (s : Schema) : Prop :=
  ∀ e, e ∈ ents c₁ s → e ∉ ents c₂ s

end Strata.Evolution
