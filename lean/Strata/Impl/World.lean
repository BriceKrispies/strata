/-
The executable kernel, stage 1: the world, the invariant, and the start state.

`stepI` is the spec's `step` (`Strata/Model.lean`) with `stepTenantI` for the tenant it addresses.
The theorems here carry the tenant refinement (`stepTenantI_refines`) to the world:

* `stepI_refines`: for every world in the invariant `IWorld.Inv`, the implementation's outcome,
  read through `absW`, is the spec's outcome for the abstracted world. The same error, or the
  abstracted world.
* `stepI_inv`: an accepted step keeps `IWorld.Inv`, so the theorem applies to the next step too.
* `absW_implInitial`: the fixture world, started as slots (one empty slot per id below the
  counter), abstracts back to the spec's fixture world.
-/

import Strata.Impl.Step
import Strata.Fixture

namespace Strata
namespace Impl

/-- A step keeps the slot count equal to the counter. -/
theorem stepTenantI_inv (t : ITenant) (p : PrincipalId) (i : Intent) (t' : ITenant)
    (hinv : t.slots.size = t.nextId) (h : stepTenantI t p i = .ok t') :
    t'.slots.size = t'.nextId := by
  cases i with
  | setUnique f strict =>
    simp only [stepTenantI] at h
    cases hl : t.schema.lookup f <;> simp only [hl] at h <;> repeat' split at h <;>
      (try simp only [Except.ok.injEq] at h) <;> (try subst h) <;>
      simp_all [setSlot, Array.size_setIfInBounds, Array.size_map]
  | _ =>
    simp only [stepTenantI] at h
    repeat' split at h
    all_goals (try simp only [Except.ok.injEq] at h)
    all_goals (try subst h)
    all_goals simp_all [setSlot, Array.size_setIfInBounds, Array.size_map]

/-- One intent against the world, on slots. -/
theorem stepI_refines (w : IWorld) (i : Intent) (c : Ctx) (hw : w.Inv) :
    (stepI w i c).map absW = step (absW w) i c := by
  unfold stepI step
  cases hc : w c.tenant with
  | none => simp [absW, hc, Except.map]
  | some t =>
    have hs := stepTenantI_refines t c.principal i (hw c.tenant t hc)
    simp only [absW, hc, Option.map_some]
    cases h' : stepTenantI t c.principal i with
    | error e =>
      rw [h'] at hs
      simp only [Except.map] at hs
      rw [← hs]
      simp [Except.map]
    | ok t' =>
      rw [h'] at hs
      simp only [Except.map] at hs
      rw [← hs]
      simp only [Except.map, absW]
      congr 1
      funext x
      by_cases hx : x = c.tenant
      · simp [absW, hx]
      · simp [absW, hx]

/-- An accepted step keeps the invariant: the addressed tenant keeps its slot count, the others
    are untouched. -/
theorem stepI_inv (w : IWorld) (i : Intent) (c : Ctx) (w' : IWorld) (hw : w.Inv)
    (h : stepI w i c = .ok w') : w'.Inv := by
  unfold stepI at h
  cases hc : w c.tenant with
  | none => simp [hc] at h
  | some t =>
    rw [hc] at h
    cases h' : stepTenantI t c.principal i with
    | error e => simp [h'] at h
    | ok t'' =>
      simp only [h', Except.ok.injEq] at h
      subst h
      intro x s hx
      by_cases hxc : x = c.tenant
      · subst hxc
        simp at hx
        subst hx
        exact stepTenantI_inv t c.principal i t'' (hw c.tenant t hc) h'
      · simp only [hxc, if_false] at hx
        exact hw x s hx

/-- The implementation's start state for a fixture tenant: its records are empty, so each
    tenant starts as one empty slot per id below the counter. -/
def implInitial : IWorld := fun x => (initialWorld x).map ITenant.ofEmpty

theorem implInitial_inv : implInitial.Inv := by
  intro x t h
  unfold implInitial at h
  cases hw : initialWorld x with
  | none => simp [hw] at h
  | some s =>
    simp only [hw, Option.map_some, Option.some.injEq] at h
    subst h
    exact ofEmpty_inv s

/-- The fixture world, started as slots, abstracts back to the spec's fixture world. -/
theorem absW_implInitial : absW implInitial = initialWorld := by
  funext x
  unfold absW implInitial
  cases hw : initialWorld x with
  | none => simp
  | some s =>
    simp only [Option.map_some, Option.map_some]
    rw [ofEmpty_abs s (initialWorld_records x s hw)]

end Impl
end Strata
