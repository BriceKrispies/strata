import Strata.Reads

/-!
# Aggregates (slice 10, second half): counts, sums, minimums, maximums and top lists of a named query

The kernel's `aggregate` (`crates/kernel/src/aggregates.rs`). An aggregate is a read: it examines the
records the named query examines for the same reader, and leaves out every record the reader may not
read in whole: the record itself (the read decision, `Tenant.canRead`), the query filter, the request's
filter, and every field the aggregate names (the field it reads, the field it groups by, and the fields
the request filters on) whose read rule does not hold on the stored record. So a count, a sum, a group
or a place in a top list never reveals a record the reader may not read.

The checks come in the kernel's order: the query and the aggregate (`unknownQuery`), the parameters
(`invalidIntent`), the object type (`unknownObject`), the request's filter fields (`invalidIntent`), the
named fields (`unknownField`), the scan window (`limitExceeded`, rule `scan-bound-exceeded`), the
selected records against the aggregate's bound (the same rule), and the groups against the group bound
(`limitExceeded`, rule `group-bound-exceeded`). A sum that does not fit an integer is `limitExceeded`,
rule `sum-overflow`. No result comes with a refusal.

Abstractions, listed in the slice 10 review note:
- The model always scans. The kernel reads the records a filter's equality or list on an indexed field
  selects, through the record index, when the filter has one (`filter_access`); the selected records are
  the same, but the kernel's scan window is then the records that match, not the records of the type.
  On the fixtures the two agree, since neither reaches the window.
- The kinds are count, sum, minimum, maximum and top. The kernel's average and its second group field
  are not modelled; the fixture declares neither.
- A group's key and its values are read from the stored record, as the kernel reads them; the
  definition-time checks of the declarations (the field types, the declared fields) are not modelled.
-/

namespace Strata

/-- The rule a refused aggregate names (the kernel's `SCAN_BOUND_EXCEEDED`, `SUM_OVERFLOW` and
    `GROUP_BOUND_EXCEEDED`). -/
inductive AggRule where
  | scanBound
  | sumOverflow
  | groupBound
  deriving DecidableEq, Repr

/-- The errors an aggregate can give. -/
inductive AggErr where
  | unknownQuery
  | invalidIntent
  | unknownObject
  | unknownField
  | limitExceeded (rule : AggRule)
  deriving DecidableEq, Repr

/-- The most groups one aggregate may have (the kernel's `MAX_AGGREGATE_GROUPS`). -/
def maxAggGroups : Nat := 1000

/-- An aggregate request: the query and aggregate by name, the parameters (`k=v`) and the filter on declared
    fields (a field and the value it must hold or contain). -/
structure AggReq where
  query : QueryId
  aggregate : String
  params : List (String × Val) := []
  filter : List (FieldId × Val) := []

/-- One group: its key (absent when the records of the group have no value in the group field, and for an
    ungrouped aggregate), its value (the count, sum, minimum or maximum; absent for a top list or when no
    record has a value), and for a top list the rows, as record ids with their values. -/
structure AggGroup where
  key : Option Val
  value : Option Val
  rows : List (RecordId × Val)
  deriving Repr

/-- The answer to an aggregate: the groups in ascending order of their keys, and the number of records the
    aggregate selected. -/
structure AggResult where
  groups : List AggGroup
  scanned : Nat
  deriving Repr

/-- The element a scalar value is, for a list field: `null` and the scalars and references. -/
def elemOf : Val → Option Elem
  | .null => some .null
  | .int i => some (.int i)
  | .str s => some (.str s)
  | .bool b => some (.bool b)
  | .ref id => some (.ref id)
  | .list _ => none

/-- Whether a record's value matches a request filter value: equal, or, for a list field, holding the
    scalar (the kernel's `matches`). -/
def matchVal (got want : Val) : Bool :=
  match got, want with
  | .list vs, .list ws => decide (vs = ws)
  | .list vs, w => match elemOf w with
    | some e => vs.contains e
    | none => false
  | h, w => decide (h = w)

/-- The value of a field of a stored record, when the record has one of the field's type (the kernel's
    `value_in`): `null` and a value of another type are no value. -/
def valueIn (o : ObjType) (r : Record) : Option FieldId → Option Val
  | none => none
  | some f =>
    let v := Fields.get r.fields f
    if v = .null then none
    else match o.schema.lookup f with
      | some d => if v.hasType d.ty then some v else none
      | none => none

/-- The order of group keys: the records with a value first, in the order of their values, then the records
    without one (the kernel's tag `1` sorts last). -/
def keyLe : Option Val → Option Val → Bool
  | some a, some b => a.leb b
  | some _, none => true
  | none, some _ => false
  | none, none => true

/-- The least of some values, in the model's order on values. -/
def leastOf (vs : List Val) : Option Val :=
  vs.foldl (fun acc v => match acc with
    | none => some v
    | some m => if m.leb v then some m else some v) none

/-- The greatest of some values, in the model's order on values. -/
def greatestOf (vs : List Val) : Option Val :=
  vs.foldl (fun acc v => match acc with
    | none => some v
    | some m => if v.leb m then some m else some v) none

/-- The order of a top list: by value (descending when asked), then by ascending record id. -/
def topLe (desc : Bool) (x y : RecordId × Val) : Bool :=
  if x.2 = y.2 then decide (x.1 ≤ y.1)
  else if desc then y.2.leb x.2
  else x.2.leb y.2

/-- The value of one group, from the values of its records (the kernel's `finish`). A sum is refused when it
    does not fit an integer; a top list gives its rows and no value. -/
def finish (a : AggDef) (rows : List (RecordId × Option Val)) :
    Except AggErr (Option Val × List (RecordId × Val)) :=
  match a.kind with
  | .count => .ok (some (.int rows.length), [])
  | .sum =>
    let total : Int := rows.foldl (fun acc r => match r.2 with
      | some (.int i) => acc + i
      | _ => acc) 0
    if total < -9223372036854775808 ∨ 9223372036854775807 < total then
      .error (.limitExceeded .sumOverflow)
    else .ok (some (.int total), [])
  | .minimum => .ok (leastOf (rows.filterMap (fun r => r.2)), [])
  | .maximum => .ok (greatestOf (rows.filterMap (fun r => r.2)), [])
  | .top =>
    let ranked := (rows.filterMap (fun r => r.2.map (fun v => (r.1, v)))).mergeSort
      (fun x y => topLe a.descending x y)
    .ok (none, ranked.take (a.n.getD 1))

/-- Whether a stored record of the named query's type is selected for the reader: the reader may read it,
    the query filter holds on it, the request's filter holds, and, for everyone but the system, each named
    field's read rule holds on it. -/
def selectedBy (src : Src) (spec : ReadSpec) (p : Principal) (q : QueryDef) (req : AggReq)
    (named : List FieldId) (e : RecordId × Record) : Bool :=
  src.canRead p q.ty e.2 && condHolds (readEnv p e.2) q.filter &&
    req.filter.all (fun kv => matchVal (Fields.get e.2.fields kv.1) kv.2) &&
    (match p with
     | .system => true
     | _ => named.all (fun f => match spec.rules.rule q.ty f with
       | none => true
       | some rule => condHolds (readEnv p e.2) (some rule)))

/-- The aggregate of a declared aggregate of a query, over a source, once the query, the aggregate and the
    object type are known (the rest of the kernel's `aggregate`). Total and computable. -/
def aggregateOn (src : Src) (spec : ReadSpec) (p : Principal) (req : AggReq) (q : QueryDef)
    (a : AggDef) (o : ObjType) : Except AggErr AggResult :=
  if req.filter.any (fun kv => !(q.declared.contains kv.1)) then .error .invalidIntent
  else
          let named : List FieldId := (req.filter.map Prod.fst) ++ a.field.toList ++ a.groupBy.toList
          if named.any (fun f => (o.schema.lookup f).isNone) then .error .unknownField
          else if maxScan < (src.recs q.ty).length then .error (.limitExceeded .scanBound)
          else
            let bound := min a.scanBound maxScan
            let selected := (src.recs q.ty).filter (selectedBy src spec p q req named)
            if bound < selected.length then .error (.limitExceeded .scanBound)
            else
              let rows : List (RecordId × Option Val × Option Val) := selected.map (fun e =>
                (e.1, valueIn o e.2 a.groupBy, valueIn o e.2 a.field))
              let groups : Except AggErr (List AggGroup) :=
                match a.groupBy with
                | none =>
                  (finish a (rows.map (fun r => (r.1, r.2.2)))).map
                    (fun fv => [{ key := none, value := fv.1, rows := fv.2 }])
                | some _ =>
                  let keys := ((rows.map (fun r => r.2.1)).eraseDups).mergeSort keyLe
                  keys.mapM (fun k =>
                    let members := rows.filter (fun r => r.2.1 == k)
                    (finish a (members.map (fun r => (r.1, r.2.2)))).map
                      (fun fv => { key := k, value := fv.1, rows := fv.2 }))
              match groups with
              | .error e => .error e
              | .ok gs =>
                if maxAggGroups < gs.length then .error (.limitExceeded .groupBound)
                else .ok { groups := gs, scanned := selected.length }

/-- Run one aggregate of a named query for a principal (the kernel's `aggregate`): the query, the aggregate
    and the parameters are checked first, then the object type, then `aggregateOn`. Total and computable. -/
def aggregate (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq) :
    Except AggErr AggResult :=
  match spec.queries.lookup req.query with
  | none => .error .unknownQuery
  | some q =>
    match q.aggregates.lookup req.aggregate with
    | none => .error .unknownQuery
    | some a =>
      if req.params.any (fun kv => !(q.params.contains kv.1)) then .error .invalidIntent
      else match t.typ? q.ty with
      | none => .error .unknownObject
      | some o => aggregateOn (Src.ofTenant t) spec p req q a o

end Strata
