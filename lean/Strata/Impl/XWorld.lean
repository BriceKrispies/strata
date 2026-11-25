/-
The executable kernel, stage 2: the indexed world.

`stepX` is the transition function of the indexed kernel: `stepI` (stage 1) with `stepTenantX` for the
tenant it addresses. The theorems compose the two refinements, indexed to stage 1
(`stepTenantX_good`) and stage 1 to the spec (`stepTenantI_refines`):

* `stepX_refines`: for every world whose indexes are consistent (`XWorld.Ok`), the indexed kernel's
  outcome, read through `absX`, is the spec's outcome for the abstracted world.
* `stepX_ok`: an accepted step keeps every tenant's indexes consistent, so the refinement applies to
  the next step too.
* `absX_xInitial`, `xInitial_ok`: the fixture world, started with no records and empty indexes,
  abstracts to the spec's fixture world, and is consistent.
-/

import Strata.Impl.XSchema
import Strata.Impl.World

namespace Strata
namespace Impl

/-- The indexed outcome of one intent against one tenant agrees with stage 1's, and keeps the
    indexes consistent. -/
theorem stepTenantXL_good (x : XTenant) (hok : x.Ok) (p : PrincipalId) (i : Intent) :
    Good x (stepTenantXL x p i) (stepTenantI x.core p i) := by
  obtain ⟨core, ux, cx, rx⟩ := x
  cases i with
  | create ty fs => exact create_good core ux cx rx hok p ty fs
  | update ty id fs ev => exact update_good core ux cx rx hok p ty id fs ev
  | delete ty id ev => exact delete_good core ux cx rx hok p ty id ev
  | setRequired ty f strict => exact setRequired_good core ux cx rx hok p ty f strict
  | setUnique ty f strict => exact setUnique_good core ux cx rx hok p ty f strict
  | defineUnique ty fs => exact defineUnique_good core ux cx rx hok p ty fs

/-- The indexed outcome (refusals without the tenant handed back) agrees with stage 1's, and an
    accepted indexed tenant is consistent. -/
theorem stepTenantX_good (x : XTenant) (hok : x.Ok) (p : PrincipalId) (i : Intent) :
    (stepTenantX x p i).map XTenant.core = stepTenantI x.core p i ∧
      ∀ y, stepTenantX x p i = .ok y → y.Ok := by
  have h := stepTenantXL_good x hok p i
  refine ⟨h.1, fun y hy => h.2.2 y ?_⟩
  unfold stepTenantX at hy
  cases hL : stepTenantXL x p i with
  | error e => rw [hL] at hy; cases hy
  | ok z =>
    rw [hL] at hy
    have hz : z = y := by simpa [Except.mapError] using hy
    rw [hz]

/-- The spec's tenant an indexed tenant stands for. -/
def XTenant.abs (x : XTenant) : Tenant := x.core.abs

/-- One intent against one tenant: the indexed kernel's outcome, read through the abstraction, is
    the spec's. -/
theorem stepTenantX_refines (x : XTenant) (hok : x.Ok) (p : PrincipalId) (i : Intent) :
    (stepTenantX x p i).map XTenant.abs = stepTenant x.abs p i := by
  have hg := (stepTenantX_good x hok p i).1
  have hs := stepTenantI_refines x.core p i hok.1
  unfold XTenant.abs
  rw [← hs, ← hg]
  cases stepTenantX x p i <;> rfl

/-- An accepted indexed step keeps the indexes consistent. -/
theorem stepTenantX_ok (x : XTenant) (hok : x.Ok) (p : PrincipalId) (i : Intent) (y : XTenant)
    (h : stepTenantX x p i = .ok y) : y.Ok :=
  (stepTenantX_good x hok p i).2 y h

/-- The whole indexed state: each tenant id has a tenant or does not. -/
abbrev XWorld := TenantId → Option XTenant

/-- The spec's world for an indexed world. -/
def absX (w : XWorld) : World := fun x => (w x).map XTenant.abs

/-- Every tenant of the world has consistent indexes. -/
def XWorld.Ok (w : XWorld) : Prop := ∀ x t, w x = some t → t.Ok

/-- The indexed kernel's transition function: one intent, in one context. -/
def stepX (w : XWorld) (i : Intent) (c : Ctx) : Except Err XWorld :=
  match w c.tenant with
  | none => .error .noTenant
  | some t =>
    match stepTenantX t c.principal i with
    | .error e => .error e
    | .ok t' => .ok (fun x => if x = c.tenant then some t' else w x)

/-- One intent against the world: the indexed kernel refines the spec. -/
theorem stepX_refines (w : XWorld) (i : Intent) (c : Ctx) (hw : w.Ok) :
    (stepX w i c).map absX = step (absX w) i c := by
  unfold stepX step
  cases hc : w c.tenant with
  | none => simp [absX, hc, Except.map]
  | some t =>
    have hs := stepTenantX_refines t (hw c.tenant t hc) c.principal i
    simp only [absX, hc, Option.map_some]
    cases h' : stepTenantX t c.principal i with
    | error e =>
      rw [h'] at hs
      simp only [Except.map] at hs
      rw [← hs]
      simp [Except.map]
    | ok t' =>
      rw [h'] at hs
      simp only [Except.map] at hs
      rw [← hs]
      simp only [Except.map, absX]
      congr 1
      funext x
      by_cases hx : x = c.tenant
      · simp [absX, hx]
      · simp [absX, hx]

/-- An accepted step keeps the indexes of every tenant consistent. -/
theorem stepX_ok (w : XWorld) (i : Intent) (c : Ctx) (w' : XWorld) (hw : w.Ok)
    (h : stepX w i c = .ok w') : w'.Ok := by
  unfold stepX at h
  cases hc : w c.tenant with
  | none => simp [hc] at h
  | some t =>
    rw [hc] at h
    cases h' : stepTenantX t c.principal i with
    | error e => simp [h'] at h
    | ok t'' =>
      simp only [h', Except.ok.injEq] at h
      subst h
      intro x s hx
      by_cases hxc : x = c.tenant
      · subst hxc
        simp at hx
        subst hx
        exact stepTenantX_ok t (hw c.tenant t hc) c.principal i t'' h'
      · simp only [hxc, if_false] at hx
        exact hw x s hx

/-- A tenant with no records and empty indexes. -/
def XTenant.ofCore (t : ITenant) : XTenant := ⟨t, ∅, ∅, ∅⟩

theorem ofCore_ok (t : ITenant) (hinv : t.Inv) (hnd : (t.types.map Prod.fst).Nodup)
    (hempty : ∀ ty o, t.typ? ty = some o → o.slots = #[]) : (XTenant.ofCore t).Ok := by
  have hk : ∀ {K : Type} (kf : TypeId → IObj → Record → List K) (c : Cell), t.keysBy kf c = [] := by
    intro K kf c
    unfold ITenant.keysBy ITenant.cellRec
    cases hT : t.typ? c.1 with
    | none => rfl
    | some o => simp [hempty _ _ hT, getRec]
  refine ⟨hinv, hnd, ?_, ?_, ?_⟩
  · intro k c
    have e : t.ukeys c = [] := hk (fun S o r => ukeysOf S o.schema r) c
    simp [MM.get, XTenant.ofCore, e]
  · intro k c
    have e : t.ckeys c = [] := hk (fun S o r => ckeysOf S o.uniques r) c
    simp [MM.get, XTenant.ofCore, e]
  · intro k c
    have e : t.rkeys c = [] := hk (fun S o r => rkeysOf o.schema r) c
    simp [MM.get, XTenant.ofCore, e]

/-- The indexed start state for a fixture tenant. -/
def xInitial : XWorld := fun x => (implInitial x).map XTenant.ofCore

theorem xInitial_ok : xInitial.Ok := by
  intro x t h
  unfold xInitial implInitial at h
  cases hw : initialWorld x with
  | none => simp [hw] at h
  | some s =>
    simp only [hw, Option.map_some, Option.some.injEq] at h
    subst h
    refine ofCore_ok _ (ofEmpty_inv s) ?_ ?_
    · unfold initialWorld initialTenant at hw
      split at hw
      · cases hw; decide
      · split at hw
        · cases hw; decide
        · cases hw
    · intro ty o ho
      unfold ITenant.typ? ITenant.ofEmpty at ho
      have := lookup_map_entry s.types (fun _ o => IObj.ofEmpty o) ty
      rw [this] at ho
      cases hl : s.types.lookup ty with
      | none => simp [hl] at ho
      | some o2 =>
        simp only [hl, Option.map_some, Option.some.injEq] at ho
        subst ho
        rfl

theorem absX_xInitial : absX xInitial = initialWorld := by
  funext x
  have := congrFun absW_implInitial x
  unfold absX xInitial absW at *
  cases h : implInitial x with
  | none => simp [h] at this ⊢; exact this
  | some t => simp [h, XTenant.ofCore, XTenant.abs] at this ⊢; exact this

end Impl
end Strata
