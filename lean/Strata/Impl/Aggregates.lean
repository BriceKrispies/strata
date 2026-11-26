/-
The executable aggregate (slice 10): `aggregateI` on the stage 1 tenant, `aggregateX` on the indexed tenant.

`aggregateI` is the spec's `aggregate` (`Strata/Aggregates.lean`) with the records taken from the slots
(`ITenant.src`, the source of `Strata/Impl/Search.lean`). The checks on the query, the aggregate, the
parameters and the object type read the schema, which the spec tenant `t.abs` holds unchanged, so they are
made on `t.abs`; the records and who may read them are the slots.

`aggregateI_refines`: `aggregateI t = aggregate t.abs` for every tenant. The indexed tenant answers as its
core, as for every read.
-/

import Strata.Impl.Search
import Strata.Aggregates

namespace Strata
namespace Impl

/-- Run one aggregate on an implementation tenant, from the slots. -/
def aggregateI (t : ITenant) (spec : ReadSpec) (p : Principal) (req : AggReq) :
    Except AggErr AggResult :=
  match spec.queries.lookup req.query with
  | none => .error .unknownQuery
  | some q =>
    match q.aggregates.lookup req.aggregate with
    | none => .error .unknownQuery
    | some a =>
      if req.params.any (fun kv => !(q.params.contains kv.1)) then .error .invalidIntent
      else match t.abs.typ? q.ty with
      | none => .error .unknownObject
      | some o => aggregateOn t.src spec p req q a o

/-- The executable aggregate is the spec's aggregate on the abstraction. -/
theorem aggregateI_refines (t : ITenant) (spec : ReadSpec) (p : Principal) (req : AggReq) :
    aggregateI t spec p req = aggregate t.abs spec p req := by
  unfold aggregateI aggregate
  simp only [ITenant.src_abs]
  rfl

/-- Run an aggregate on an indexed tenant: the aggregate of its core. -/
def aggregateX (x : XTenant) (spec : ReadSpec) (p : Principal) (req : AggReq) :
    Except AggErr AggResult :=
  aggregateI x.core spec p req

theorem aggregateX_refines (x : XTenant) (spec : ReadSpec) (p : Principal) (req : AggReq) :
    aggregateX x spec p req = aggregate x.abs spec p req :=
  aggregateI_refines x.core spec p req

end Impl
end Strata
