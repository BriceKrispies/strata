import Strata.ModelBase

/-!
# Reads (slice 10, first half): record visibility, field read rules, record reads and named queries

The kernel's second entry point. `read` never changes state: it answers a request on a tenant for
a principal, and what it returns is only what the principal may read, with the fields the read
rules hide taken out.

The read declarations of the schema (a field's read rule, a named query) are held beside the write
model, in a `ReadSpec`, rather than inside `FieldDef` or `ObjType`. The write codec and its
theorems are therefore unchanged; the codec of the read declarations is a later step (see the
slice 10 review note in `docs/verification.md`).

Abstractions, all listed in the review note:
- A named query is served as a scan: every record of its object type is examined and the rows are
  sorted by the order keys, then by id. The kernel's window and index plans, and their cursors, are
  not modelled. A cursor is an offset, and any other cursor is an invalid intent.
- A query's parameters are checked as declared (an undeclared name is refused), but the filter does
  not read them: `Expr` has no parameter form.
- Conditions see the record, the principal and nothing else: no clock, no lookups through
  references, no scopes, so `InsufficientState` does not arise.
- Order keys are compared by `Val.leb`, the model's order on values: the kernel's derived order (by kind, then by payload, lists element by element).
-/

namespace Strata

/-! ## The declarations -/

/-- The read rules of the schema, one per field that has one (the kernel's `field.read`): a present
    field of the object type is shown to a reader only when its rule is exactly `true` on the stored
    record. Keyed by object type and field. -/
abbrev FieldRules := List (TypeId × FieldId × Expr)

/-- A named query is identified by its name. -/
abbrev QueryId := String

/-- A named query (the kernel's `QueryDef`, in its scan form): the records of one object type that
    `filter` holds of on their redacted copy, sorted by the order keys (a field and whether it is
    descending), then by id, and paged. `limit` is the default page size. -/
structure QueryDef where
  ty : TypeId
  params : List String := []
  filter : Option Expr := none
  order : List (FieldId × Bool) := []
  limit : Option Nat := none
  deriving Repr

/-- The read declarations of a tenant's schema. -/
structure ReadSpec where
  rules : FieldRules := []
  queries : List (QueryId × QueryDef) := []
  deriving Repr

/-- The errors a read can give (the kernel's `LimitExceeded`, `UnknownObject`, `UnknownQuery` and
    `InvalidIntent`). Kept apart from `Err` so that the write codec is unchanged. -/
inductive ReadErr where
  | unknownObject
  | unknownQuery
  | invalidIntent
  | limitExceeded
  deriving DecidableEq, Repr

/-- A page cursor. A scan issues offsets; any other cursor is not one the query issued. -/
inductive Cursor where
  | offset (n : Nat)
  | after (id : RecordId)
  deriving DecidableEq, Repr

/-- A read request (the kernel's `ReadRequest`). -/
inductive ReadReq where
  /-- Named records of one object type. -/
  | records (ty : TypeId) (ids : List RecordId)
  /-- A named query with its parameters, page size and cursor. -/
  | named (q : QueryId) (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor)

/-- The answer to a read: the rows, in order, each with its id and its redacted record, and the cursor
    of the next page when there is one. -/
structure ReadResult where
  rows : List (RecordId × Record)
  next : Option Cursor
  deriving Repr

/-- The bound on the ids of one record read (the kernel's `MAX_READ_IDS`). -/
def maxReadIds : Nat := 200
/-- The bound on a page size (the kernel's `MAX_PAGE_SIZE`). -/
def maxPage : Nat := 200
/-- The bound on the records a named query may scan (the kernel's `MAX_SCAN_RECORDS`). -/
def maxScan : Nat := 10000
/-- The page size of a query that declares none and is asked for none (the kernel's `DEFAULT_PAGE_SIZE`). -/
def defaultPage : Nat := 50

/-! ## Values in order keys

The kernel sorts by its `Value` order, which is the derived order of the enum: by variant (null, bool,
int, str, time, ref, list), then by payload; two lists compare lexicographically, element by element,
a prefix before its extension. The model has no time, and its order on the other kinds is the same. -/

/-- The position of an element's kind in the order of values. -/
def Elem.rank : Elem → Nat
  | .null => 0
  | .bool _ => 1
  | .int _ => 2
  | .str _ => 3
  | .ref _ => 4

/-- The order of two elements: `true` when the first is at most the second. -/
def Elem.leb : Elem → Elem → Bool
  | .null, .null => true
  | .bool x, .bool y => decide (x.toNat ≤ y.toNat)
  | .int x, .int y => decide (x ≤ y)
  | .str x, .str y => decide (x ≤ y)
  | .ref x, .ref y => decide (x ≤ y)
  | a, b => decide (a.rank < b.rank)

theorem Elem.leb_total (a b : Elem) : (a.leb b || b.leb a) = true := by
  cases a <;> cases b <;> simp only [Elem.leb, Elem.rank, Bool.or_eq_true, decide_eq_true_eq] <;>
    simp_all [Nat.le_total, Int.le_total, String.le_total] <;> omega

theorem Elem.leb_trans {a b c : Elem} (h1 : a.leb b = true) (h2 : b.leb c = true) :
    a.leb c = true := by
  cases a <;> cases b <;> cases c <;>
    simp only [Elem.leb, Elem.rank, decide_eq_true_eq, Bool.toNat] at h1 h2 ⊢ <;>
    first
    | exact Nat.le_trans h1 h2
    | exact Int.le_trans h1 h2
    | exact String.le_trans h1 h2
    | simp_all
    | omega

/-- The lexicographic order of two lists, given an order on their elements: the first position at
    which they differ decides, and a list that is a prefix of the other comes first. -/
def lexLe {α : Type} (le : α → α → Bool) : List α → List α → Bool
  | [], _ => true
  | _ :: _, [] => false
  | x :: xs, y :: ys => if le x y && le y x then lexLe le xs ys else le x y

theorem lexLe_total {α : Type} (le : α → α → Bool) (ht : ∀ a b, (le a b || le b a) = true) :
    ∀ xs ys : List α, (lexLe le xs ys || lexLe le ys xs) = true
  | [], _ => by simp [lexLe]
  | _ :: _, [] => by simp [lexLe]
  | x :: xs, y :: ys => by
      have ih := lexLe_total le ht xs ys
      have hxy := ht x y
      simp only [lexLe]
      cases h1 : le x y <;> cases h2 : le y x <;> simp_all

theorem lexLe_trans {α : Type} (le : α → α → Bool) (ht : ∀ a b, (le a b || le b a) = true)
    (htr : ∀ {a b c}, le a b = true → le b c = true → le a c = true) :
    ∀ xs ys zs : List α, lexLe le xs ys = true → lexLe le ys zs = true → lexLe le xs zs = true
  | [], _, _, _, _ => by simp [lexLe]
  | _ :: _, [], _, h, _ => by simp [lexLe] at h
  | _ :: _, _ :: _, [], _, h => by simp [lexLe] at h
  | x :: xs, y :: ys, z :: zs, h1, h2 => by
      have ih := lexLe_trans le ht htr xs ys zs
      simp only [lexLe] at h1 h2 ⊢
      have k1 : le x y = true → le y z = true → le x z = true := fun a b => htr a b
      have k2 : le x z = true → le z y = true → le x y = true := fun a b => htr a b
      have k3 : le y x = true → le x z = true → le y z = true := fun a b => htr a b
      have k4 : le y z = true → le z x = true → le y x = true := fun a b => htr a b
      have k5 : le z x = true → le x y = true → le z y = true := fun a b => htr a b
      have k6 : le z y = true → le y x = true → le z x = true := fun a b => htr a b
      have t1 : (le x y || le y x) = true := ht x y
      have t2 : (le y z || le z y) = true := ht y z
      have t3 : (le x z || le z x) = true := ht x z
      cases hxy : le x y <;> cases hyx : le y x <;> cases hyz : le y z <;>
        cases hzy : le z y <;> cases hxz : le x z <;> cases hzx : le z x <;>
        simp_all

/-- The position of a kind in the order of values. -/
def Val.rank : Val → Nat
  | .null => 0
  | .bool _ => 1
  | .int _ => 2
  | .str _ => 3
  | .ref _ => 4
  | .list _ => 5

/-- The order of two values: `true` when the first is at most the second. -/
def Val.leb : Val → Val → Bool
  | .null, .null => true
  | .bool x, .bool y => decide (x.toNat ≤ y.toNat)
  | .int x, .int y => decide (x ≤ y)
  | .str x, .str y => decide (x ≤ y)
  | .ref x, .ref y => decide (x ≤ y)
  | .list xs, .list ys => lexLe Elem.leb xs ys
  | a, b => decide (a.rank < b.rank)

theorem Val.leb_total (a b : Val) : (a.leb b || b.leb a) = true := by
  cases a <;> cases b <;>
    first
    | exact lexLe_total Elem.leb Elem.leb_total _ _
    | (simp only [Val.leb, Val.rank, Bool.or_eq_true, decide_eq_true_eq]
       simp_all [Nat.le_total, Int.le_total, String.le_total] <;> omega)

theorem Val.leb_trans {a b c : Val} (h1 : a.leb b = true) (h2 : b.leb c = true) :
    a.leb c = true := by
  cases a <;> cases b <;> cases c <;>
    simp only [Val.leb, Val.rank, decide_eq_true_eq, Bool.toNat] at h1 h2 ⊢ <;>
    first
    | exact Nat.le_trans h1 h2
    | exact Int.le_trans h1 h2
    | exact String.le_trans h1 h2
    | exact lexLe_trans Elem.leb Elem.leb_total Elem.leb_trans _ _ _ h1 h2
    | simp_all
    | omega

/-- The order of one order key: ascending compares `a` with `b`, descending compares them the other way. -/
def dirLe : Bool → Val → Val → Bool
  | false, a, b => a.leb b
  | true, a, b => b.leb a

theorem dirLe_total (d : Bool) (a b : Val) : (dirLe d a b || dirLe d b a) = true := by
  cases d
  · exact Val.leb_total a b
  · exact Val.leb_total b a

theorem dirLe_trans (d : Bool) {a b c : Val} (h1 : dirLe d a b = true) (h2 : dirLe d b c = true) :
    dirLe d a c = true := by
  cases d
  · exact Val.leb_trans h1 h2
  · exact Val.leb_trans h2 h1

/-- The lexicographic order of keys, given the direction of each position. Keys of different
    lengths are never compared: a row's keys all have one length, the number of its order keys
    plus one for the id. -/
def keysLe : List Bool → List Val → List Val → Bool
  | [], _, _ => true
  | _ :: _, [], _ => false
  | _ :: _, _ :: _, [] => false
  | d :: ds, x :: xs, y :: ys =>
      if dirLe d x y && dirLe d y x then keysLe ds xs ys else dirLe d x y

theorem keysLe_total : ∀ (ds : List Bool) (xs ys : List Val),
    xs.length = ds.length → ys.length = ds.length →
    (keysLe ds xs ys || keysLe ds ys xs) = true
  | [], _, _, _, _ => by simp [keysLe]
  | d :: ds, [], _, h, _ => by simp at h
  | d :: ds, _ :: _, [], _, h => by simp at h
  | d :: ds, x :: xs, y :: ys, hx, hy => by
      have ih := keysLe_total ds xs ys (by simpa using hx) (by simpa using hy)
      have hxy := dirLe_total d x y
      have hyx := dirLe_total d y x
      simp only [keysLe]
      cases h1 : dirLe d x y <;> cases h2 : dirLe d y x <;> simp_all

theorem keysLe_trans : ∀ (ds : List Bool) (xs ys zs : List Val),
    xs.length = ds.length → ys.length = ds.length → zs.length = ds.length →
    keysLe ds xs ys = true → keysLe ds ys zs = true → keysLe ds xs zs = true
  | [], _, _, _, _, _, _, _, _ => by simp [keysLe]
  | d :: ds, [], _, _, h, _, _, _, _ => by simp at h
  | d :: ds, _ :: _, [], _, _, h, _, _, _ => by simp at h
  | d :: ds, _ :: _, _ :: _, [], _, _, h, _, _ => by simp at h
  | d :: ds, x :: xs, y :: ys, z :: zs, hx, hy, hz, h1, h2 => by
      have ih := keysLe_trans ds xs ys zs (by simpa using hx) (by simpa using hy)
        (by simpa using hz)
      simp only [keysLe] at h1 h2 ⊢
      -- The chains among x, y and z, each as a conditional rewrite with no free variables.
      have k1 : dirLe d x y = true → dirLe d y z = true → dirLe d x z = true :=
        fun h₁ h₂ => dirLe_trans d h₁ h₂
      have k2 : dirLe d x z = true → dirLe d z y = true → dirLe d x y = true :=
        fun h₁ h₂ => dirLe_trans d h₁ h₂
      have k3 : dirLe d y x = true → dirLe d x z = true → dirLe d y z = true :=
        fun h₁ h₂ => dirLe_trans d h₁ h₂
      have k4 : dirLe d y z = true → dirLe d z x = true → dirLe d y x = true :=
        fun h₁ h₂ => dirLe_trans d h₁ h₂
      have k5 : dirLe d z x = true → dirLe d x y = true → dirLe d z y = true :=
        fun h₁ h₂ => dirLe_trans d h₁ h₂
      have k6 : dirLe d z y = true → dirLe d y x = true → dirLe d z x = true :=
        fun h₁ h₂ => dirLe_trans d h₁ h₂
      have t1 : (dirLe d x y || dirLe d y x) = true := dirLe_total d x y
      have t2 : (dirLe d y z || dirLe d z y) = true := dirLe_total d y z
      have t3 : (dirLe d x z || dirLe d z x) = true := dirLe_total d x z
      cases hxy : dirLe d x y <;> cases hyx : dirLe d y x <;> cases hyz : dirLe d y z <;>
        cases hzy : dirLe d z y <;> cases hxz : dirLe d x z <;> cases hzx : dirLe d z x <;>
        simp_all

/-! ## Visibility and redaction -/

/-- The records of an object type of the tenant, with their ids. -/
def Tenant.recordsOf (t : Tenant) (ty : TypeId) : Records :=
  ((t.typ? ty).map ObjType.records).getD []

/-- The rule of a field of an object type, if it has one. -/
def FieldRules.rule (rs : FieldRules) (ty : TypeId) (f : FieldId) : Option Expr :=
  (rs.find? (fun e => e.1 == ty && e.2.1 == f)).map (fun e => e.2.2)

/-- The environment a read rule or a query filter is evaluated in: the stored record in every
    scope, the principal. -/
def readEnv (p : Principal) (r : Record) : ExprEnv :=
  { record := some r, old := some r, new := some r, principal := p.userId? }

/-- Whether a field of a stored record is shown to the principal (the kernel's `redact`, per
    field). The system reads everything; any other reader sees a field unless its rule is not exactly
    `true` on the record. -/
def shownField (rs : FieldRules) (p : Principal) (ty : TypeId) (r : Record) (f : FieldId) : Bool :=
  match p with
  | .system => true
  | _ => match rs.rule ty f with
    | none => true
    | some e => condHolds (readEnv p r) (some e)

/-- The record as the principal reads it: the fields the read rules hide are taken out. -/
def redact (rs : FieldRules) (p : Principal) (ty : TypeId) (r : Record) : Record :=
  { r with fields := r.fields.filter (fun fv => shownField rs p ty r fv.1) }

/-- Whether a principal may read a stored record of an object type (the kernel's `visible`): the
    `Read` decision of `Tenant.allows` on the record. -/
def Tenant.canRead (t : Tenant) (p : Principal) (ty : TypeId) (r : Record) : Bool :=
  t.allows p .read (.object ty) (readEnv p r)

/-- The row of a record id as the principal reads it: the redacted record, when the tenant holds the
    record and the principal may read it. A missing record and an invisible one give `none` alike, so
    the answer does not show which ids exist. -/
def rowOf (t : Tenant) (rs : FieldRules) (p : Principal) (ty : TypeId) (id : RecordId) :
    Option (RecordId × Record) :=
  match t.recordAt ty id with
  | some r => if t.canRead p ty r then some (id, redact rs p ty r) else none
  | none => none

/-! ## Record reads -/

/-- Read the records named by the ids, in the request's order (the kernel's `Records`): more than
    `maxReadIds` ids is refused before the type is looked at; an unknown type is refused; a missing
    or invisible record is left out silently. -/
def readRecords (t : Tenant) (spec : ReadSpec) (p : Principal) (ty : TypeId)
    (ids : List RecordId) : Except ReadErr (List (RecordId × Record)) :=
  if maxReadIds < ids.length then .error .limitExceeded
  else if (t.typ? ty).isNone then .error .unknownObject
  else .ok (ids.filterMap (rowOf t spec.rules p ty))

/-! ## Named queries -/

/-- The unsorted rows of a named query: each record of its type that the principal may read, redacted,
    and whose redacted copy makes the filter exactly `true`. -/
def scanRows (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef) :
    List (RecordId × Record) :=
  (t.recordsOf q.ty).filterMap (fun e =>
    if t.canRead p q.ty e.2 then
      let shown := redact spec.rules p q.ty e.2
      if condHolds (readEnv p shown) q.filter then some (e.1, shown) else none
    else none)

/-- The order keys of a row: the values of the order fields on the redacted copy (absent fields
    read as `null`), then the id. -/
def orderKey (q : QueryDef) (row : RecordId × Record) : List Val :=
  q.order.map (fun e => Fields.get row.2.fields e.1) ++ [.int row.1]

/-- The direction of each key of `orderKey`: the order fields' directions, then ascending for the id. -/
def orderDirs (q : QueryDef) : List Bool :=
  q.order.map (fun e => e.2) ++ [false]

/-- Every row of a named query, in order: sorted by the order keys, then by id. -/
def queryAll (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef) :
    List (RecordId × Record) :=
  (scanRows t spec p q).mergeSort (fun a b => keysLe (orderDirs q) (orderKey q a) (orderKey q b))

/-- The page of a named query (the kernel's `query_plan`, scan form). The checks come in the kernel's
    order: the query, its parameters, the page size, the cursor, the scan bound. -/
def queryPage (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor) :
    Except ReadErr ReadResult :=
  match spec.queries.lookup qid with
  | none => .error .unknownQuery
  | some q =>
    if params.any (fun kv => !(q.params.contains kv.1)) then .error .invalidIntent
    else
      let size := (page.or q.limit).getD defaultPage
      if size = 0 then .error .invalidIntent
      else if maxPage < size then .error .limitExceeded
      else
        match cursor with
        | some (.after _) => .error .invalidIntent
        | _ =>
          let off := match cursor with
            | some (.offset n) => n
            | _ => 0
          if maxScan < ((t.typ? q.ty).map (fun o => o.records.length)).getD 0 then
            .error .limitExceeded
          else
            let rows := queryAll t spec p q
            .ok { rows := (rows.drop off).take size,
                  next := if off + size < rows.length then some (.offset (off + size)) else none }

/-! ## The read -/

/-- The rows of a chain of pages over `L`, read from offset `off` in pages of `size`: the page at each
    offset is the rows there, `size` long, and the chain goes on to the next offset exactly when
    `queryPage` issues a next cursor (`off + size < length`). With a page size of 0 the chain stops. -/
def walk {α : Type} (L : List α) (size off : Nat) : List α :=
  if h : 0 < size ∧ off + size < L.length then
    (L.drop off).take size ++ walk L size (off + size)
  else (L.drop off).take size
termination_by L.length - off
decreasing_by omega

/-- Answer a read request on a tenant for a principal (the kernel's `read`). Total and computable. -/
def read (t : Tenant) (spec : ReadSpec) (p : Principal) : ReadReq → Except ReadErr ReadResult
  | .records ty ids => (readRecords t spec p ty ids).map (fun rs => { rows := rs, next := none })
  | .named q params page cursor => queryPage t spec p q params page cursor

end Strata
