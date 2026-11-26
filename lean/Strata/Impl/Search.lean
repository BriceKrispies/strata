/-
The executable search (slice 10): `searchI` on the stage 1 tenant, `searchX` on the indexed tenant.

`searchI` is the spec's `search` (`Strata/Search.lean`) run over the slots: the records of a type are the
slot rows (`ITenant.rowsOf`), and who may read a stored record is `ITenant.canRead`, both proved equal to
the spec's on the abstraction (`recordsOf_abs`, `canRead_abs`). The plan (which object types are searched,
and their searchable fields) reads only the object types and their schemas, which the spec tenant
`t.abs` holds unchanged, so it is computed on `t.abs`; it does not touch the records.

`searchI_refines`: `searchI t = search t.abs` for every tenant, with no invariant needed. The indexed tenant
answers as its core (a search changes nothing and the indexes are not consulted).
-/

import Strata.Impl.Read
import Strata.Search

namespace Strata
namespace Impl

/-- The source of an implementation tenant: its slot rows and its read decision on the slots. -/
def ITenant.src (t : ITenant) : Src :=
  { recs := fun ty => t.rowsOf ty, canRead := fun p ty r => t.canRead p ty r }

/-- The source of an implementation tenant is the source of its abstraction. -/
theorem ITenant.src_abs (t : ITenant) : t.src = Src.ofTenant t.abs := by
  have h1 : (fun ty => t.rowsOf ty) = t.abs.recordsOf :=
    funext (fun ty => (t.recordsOf_abs ty).symm)
  have h2 : (fun p ty r => t.canRead p ty r) = (fun p ty r => t.abs.canRead p ty r) :=
    funext (fun p => funext (fun ty => funext (fun r => (t.canRead_abs p ty r).symm)))
  unfold ITenant.src Src.ofTenant
  rw [h1, h2]

/-- Answer a search on an implementation tenant, from the slots. -/
def searchI (t : ITenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) :
    Except ReadErr SearchResult :=
  match searchPlan t.abs spec p req with
  | .error e => .error e
  | .ok (q, size, plan) => .ok (searchOn t.src spec.rules p q size plan)

/-- The executable search is the spec's search on the abstraction. -/
theorem searchI_refines (t : ITenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) :
    searchI t spec p req = search t.abs spec p req := by
  unfold searchI search
  simp only [ITenant.src_abs]
  rfl

/-- Answer a search on an indexed tenant: the search of its core. -/
def searchX (x : XTenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) :
    Except ReadErr SearchResult :=
  searchI x.core spec p req

theorem searchX_refines (x : XTenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) :
    searchX x spec p req = search x.abs spec p req :=
  searchI_refines x.core spec p req

end Impl
end Strata
