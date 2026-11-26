/-
The executable read (slice 10): `readI` on the stage 1 tenant, `readX` on the indexed tenant.

`readI` is the spec's `read` (`Strata/Reads.lean`) with the records taken from the slots. A record is an
array index, not a list scan, and a query's scan walks the slots from the highest id down, which is
the order the spec's list holds them in (`slotRows_eq`). The decision of who may read is the spec's
`Tenant.allows` on the tenant's membership, written out on the fields the stage 1 tenant holds
(`ITenant.canRead`), so no spec tenant is built to answer a read. A read changes nothing, so the
indexes of the stage 2 tenant are not consulted: `readX` is `readI` of its core.

`readI_refines`: `readI t` is `read t.abs`, for every tenant, with no invariant needed.
-/

import Strata.Impl.XWorld
import Strata.Reads

namespace Strata
namespace Impl

/-- The records of the slots below `n`, the highest id first (the order of `absSlots`). -/
def slotRows (a : Array (Option Record)) : Nat → Records
  | 0 => []
  | n + 1 => (match getRec a n with
      | some r => [(n, r)]
      | none => []) ++ slotRows a n

theorem slotRows_take (a : Array (Option Record)) :
    ∀ n, n ≤ a.size → slotRows a n = ascend 0 (a.toList.take n)
  | 0, _ => by simp [slotRows, ascend]
  | n + 1, h => by
      have ih := slotRows_take a n (by omega)
      have hlt : n < a.toList.length := by simp; omega
      have htake : a.toList.take (n + 1) = a.toList.take n ++ [a.toList[n]] := by
        rw [List.take_succ, List.getElem?_eq_getElem hlt]; rfl
      have hlen : (a.toList.take n).length = n := by simp; omega
      rw [htake, ascend_append, hlen, slotRows, ih]
      have hget : getRec a n = a.toList[n] := by
        unfold getRec
        have : a[n]? = some a.toList[n] := by simp [Array.getElem?_toList]
        simp [this]
      congr 1
      rw [hget]
      cases a.toList[n] <;> simp [ascend, cellOf]

theorem slotRows_eq (a : Array (Option Record)) : slotRows a a.size = absSlots a := by
  rw [slotRows_take a a.size (Nat.le_refl _)]
  have : a.toList.take a.size = a.toList := List.take_of_length_le (by simp)
  unfold absSlots
  rw [this]

/-- The records of an object type of an implementation tenant, the highest id first. -/
def ITenant.rowsOf (t : ITenant) (ty : TypeId) : Records :=
  slotRows (t.slotsOf ty) (t.slotsOf ty).size

theorem ITenant.recordsOf_abs (t : ITenant) (ty : TypeId) : t.abs.recordsOf ty = t.rowsOf ty := by
  unfold Tenant.recordsOf ITenant.rowsOf
  rw [typ?_abs, slotRows_eq]
  cases h : t.typ? ty <;> simp [ITenant.slotsOf, h, IObj.abs, absSlots, ascend]

/-- Whether a principal may read a stored record of an object type: `Tenant.allows` for the `read`
    action, on the tenant's membership, grants and policies. -/
def ITenant.canRead (t : ITenant) (p : Principal) (ty : TypeId) (r : Record) : Bool :=
  let roles := t.header.rolesOf p
  roles.contains ownerRole ||
    (Action.read != .manage &&
      t.grants.any (fun g => roles.contains g.1 && g.2 == .object ty .read)) ||
    t.policies.any (fun pol => roles.contains pol.role && pol.actions.contains .read &&
      pol.resource.covers (.object ty) && condHolds (readEnv p r) pol.condition)

theorem ITenant.canRead_abs (t : ITenant) (p : Principal) (ty : TypeId) (r : Record) :
    t.abs.canRead p ty r = t.canRead p ty r := by
  unfold Tenant.canRead Tenant.allows ITenant.canRead
  rfl

/-- The row of a record id as the principal reads it, from the slots. -/
def ITenant.rowOfI (t : ITenant) (rs : FieldRules) (p : Principal) (ty : TypeId) (id : RecordId) :
    Option (RecordId × Record) :=
  match getRec (t.slotsOf ty) id with
  | some r => if t.canRead p ty r then some (id, redact rs p ty r) else none
  | none => none

theorem ITenant.rowOfI_abs (t : ITenant) (rs : FieldRules) (p : Principal) (ty : TypeId)
    (id : RecordId) : t.rowOfI rs p ty id = rowOf t.abs rs p ty id := by
  unfold ITenant.rowOfI rowOf Tenant.recordAt
  rw [typ?_abs]
  cases h : t.typ? ty with
  | none => simp [ITenant.slotsOf, h, getRec]
  | some o =>
    simp only [Option.map_some, Option.bind_some, ITenant.slotsOf, h]
    rw [IObj.abs, lookup_absSlots]
    simp only [ITenant.canRead_abs] <;> rfl

/-- Record reads on the slots. -/
def readRecordsI (t : ITenant) (spec : ReadSpec) (p : Principal) (ty : TypeId) (ids : List RecordId) :
    Except ReadErr (List (RecordId × Record)) :=
  if maxReadIds < ids.length then .error .limitExceeded
  else if (t.typ? ty).isNone then .error .unknownObject
  else .ok (ids.filterMap (t.rowOfI spec.rules p ty))

theorem readRecordsI_abs (t : ITenant) (spec : ReadSpec) (p : Principal) (ty : TypeId)
    (ids : List RecordId) : readRecordsI t spec p ty ids = readRecords t.abs spec p ty ids := by
  have hf : t.rowOfI spec.rules p ty = rowOf t.abs spec.rules p ty := funext (t.rowOfI_abs _ _ _)
  unfold readRecordsI readRecords
  rw [typ?_abs, hf]
  simp

/-- The unsorted rows of a named query, from the slots. -/
def ITenant.scanRowsI (t : ITenant) (spec : ReadSpec) (p : Principal) (q : QueryDef) :
    List (RecordId × Record) :=
  (t.rowsOf q.ty).filterMap (fun e =>
    if t.canRead p q.ty e.2 then
      let shown := redact spec.rules p q.ty e.2
      if condHolds (readEnv p shown) q.filter then some (e.1, shown) else none
    else none)

theorem ITenant.scanRowsI_abs (t : ITenant) (spec : ReadSpec) (p : Principal) (q : QueryDef) :
    t.scanRowsI spec p q = scanRows t.abs spec p q := by
  unfold ITenant.scanRowsI scanRows
  rw [ITenant.recordsOf_abs]
  simp only [ITenant.canRead_abs]

/-- Every row of a named query, in order. -/
def ITenant.queryAllI (t : ITenant) (spec : ReadSpec) (p : Principal) (q : QueryDef) :
    List (RecordId × Record) :=
  (t.scanRowsI spec p q).mergeSort (fun a b => keysLe (orderDirs q) (orderKey q a) (orderKey q b))

theorem ITenant.queryAllI_abs (t : ITenant) (spec : ReadSpec) (p : Principal) (q : QueryDef) :
    t.queryAllI spec p q = queryAll t.abs spec p q := by
  unfold ITenant.queryAllI queryAll
  rw [t.scanRowsI_abs]

/-- The number of records of an object type, for the scan bound. -/
def ITenant.countI (t : ITenant) (ty : TypeId) : Nat := (t.rowsOf ty).length

theorem ITenant.countI_abs (t : ITenant) (ty : TypeId) :
    t.countI ty = ((t.abs.typ? ty).map (fun o => o.records.length)).getD 0 := by
  unfold ITenant.countI ITenant.rowsOf
  rw [typ?_abs, slotRows_eq]
  cases h : t.typ? ty <;> simp [ITenant.slotsOf, h, IObj.abs, absSlots, ascend]

/-- A named query's page, from the slots: `queryPage` with the scan and its count read from them. -/
def queryPageI (t : ITenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
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
          if maxScan < t.countI q.ty then
            .error .limitExceeded
          else
            let rows := t.queryAllI spec p q
            .ok { rows := (rows.drop off).take size,
                  next := if off + size < rows.length then some (.offset (off + size)) else none }

theorem queryPageI_abs (t : ITenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor) :
    queryPageI t spec p qid params page cursor = queryPage t.abs spec p qid params page cursor := by
  unfold queryPageI queryPage
  simp only [ITenant.countI_abs, ITenant.queryAllI_abs] <;> rfl

/-- Answer a read request on an implementation tenant. -/
def readI (t : ITenant) (spec : ReadSpec) (p : Principal) : ReadReq → Except ReadErr ReadResult
  | .records ty ids => (readRecordsI t spec p ty ids).map (fun rs => { rows := rs, next := none })
  | .named q params page cursor => queryPageI t spec p q params page cursor

/-- The executable read is the spec's read on the abstraction. -/
theorem readI_refines (t : ITenant) (spec : ReadSpec) (p : Principal) (req : ReadReq) :
    readI t spec p req = read t.abs spec p req := by
  cases req with
  | records ty ids => simp only [readI, read, readRecordsI_abs]
  | named q params page cursor => simp only [readI, read, queryPageI_abs]

/-- Answer a read request on an indexed tenant: the read of its core, which the indexes do not touch. -/
def readX (x : XTenant) (spec : ReadSpec) (p : Principal) (req : ReadReq) :
    Except ReadErr ReadResult :=
  readI x.core spec p req

theorem readX_refines (x : XTenant) (spec : ReadSpec) (p : Principal) (req : ReadReq) :
    readX x spec p req = read x.abs spec p req :=
  readI_refines x.core spec p req

end Impl
end Strata
