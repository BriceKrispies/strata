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

/-- A step keeps every slot array within the counter. -/
theorem stepTenantI_inv (t : ITenant) (p : PrincipalId) (i : Intent) (t' : ITenant)
    (hinv : t.Inv) (h : stepTenantI t p i = .ok t') : t'.Inv := by
  cases i with
  | create ty fs =>
    simp only [stepTenantI] at h
    cases hty : t.typ? ty with
    | none => simp [hty] at h
    | some o =>
      simp only [hty] at h
      split at h
      · cases h
      · split at h
        · cases h
        · simp only [Except.ok.injEq] at h
          subst h
          have hsz := hinv ty o hty
          intro T q hq
          have := inv_putType t ty { o with slots := (padTo o.slots t.nextId).push (some _) }
            (t.nextId + 1) (by omega) (by simp [size_padTo _ _ hsz]) hinv T q hq
          exact this
  | update ty id fs ev =>
    simp only [stepTenantI] at h
    cases hty : t.typ? ty with
    | none => simp [hty] at h
    | some o =>
      simp only [hty] at h
      split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · split at h
            · cases h
            · simp only [Except.ok.injEq] at h
              subst h
              have hsz := hinv ty o hty
              intro T q hq
              exact inv_putType t ty { o with slots := setSlot o.slots id (some _) } t.nextId
                (Nat.le_refl _) (by simp [setSlot, hsz]) hinv T q hq
  | delete ty id ev =>
    simp only [stepTenantI] at h
    cases hty : t.typ? ty with
    | none => simp [hty] at h
    | some o =>
      simp only [hty] at h
      split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · split at h
            · cases h
            · simp only [Except.ok.injEq] at h
              subst h
              intro T q hq
              unfold ITenant.typ? at hq
              simp only [lookup_map_entry] at hq
              cases hl : t.types.lookup T with
              | none => simp [hl] at hq
              | some o2 =>
                simp only [hl, Option.map_some, Option.some.injEq] at hq
                subst hq
                have := hinv T o2 hl
                unfold delObjI
                split <;> simp [setSlot, this]
  | setRequired ty f strict =>
    simp only [stepTenantI] at h
    split at h
    · cases h
    · cases hty : t.typ? ty with
      | none => simp [hty] at h
      | some o =>
        simp only [hty] at h
        split at h
        · cases h
        · simp only [Except.ok.injEq] at h
          subst h
          intro T q hq
          exact inv_putType t ty { o with schema := makeRequired o.schema f } t.nextId (Nat.le_refl _)
            (hinv ty o hty) hinv T q hq
  | setUnique ty f strict =>
    simp only [stepTenantI] at h
    split at h
    · cases h
    · cases hty : t.typ? ty with
      | none => simp [hty] at h
      | some o =>
        simp only [hty] at h
        cases hl : o.schema.lookup f with
        | none => simp [hl] at h
        | some d =>
          simp only [hl] at h
          split at h
          · cases h
          · split at h
            · cases h
            · simp only [Except.ok.injEq] at h
              subst h
              intro T q hq
              exact inv_putType t ty { o with schema := makeUnique o.schema f } t.nextId
                (Nat.le_refl _) (hinv ty o hty) hinv T q hq
  | defineUnique ty fs =>
    simp only [stepTenantI] at h
    split at h
    · cases h
    · cases hty : t.typ? ty with
      | none => simp [hty] at h
      | some o =>
        simp only [hty] at h
        split at h
        · cases h
        · split at h
          · cases h
          · split at h
            · cases h
            · simp only [Except.ok.injEq] at h
              subst h
              intro T q hq
              exact inv_putType t ty { o with uniques := o.uniques ++ [fs] } t.nextId (Nat.le_refl _)
                (hinv ty o hty) hinv T q hq

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

/-- An implementation object type with no records. -/
def IObj.ofEmpty (o : ObjType) : IObj := { schema := o.schema, uniques := o.uniques, slots := #[] }

/-- A tenant with no records, as the implementation starts it: every slot array empty. -/
def ITenant.ofEmpty (t : Tenant) : ITenant :=
  { types := t.types.map (fun e => (e.1, IObj.ofEmpty e.2)), owner := t.owner,
    editors := t.editors, writers := t.writers, nextId := t.nextId, limits := t.limits }

theorem ofEmpty_abs (t : Tenant) (h : ∀ e ∈ t.types, e.2.records = []) :
    (ITenant.ofEmpty t).abs = t := by
  cases t with
  | mk types owner editors writers nextId limits =>
    simp only [ITenant.abs, ITenant.ofEmpty, List.map_map]
    congr 1
    conv => rhs; rw [← List.map_id types]
    apply List.map_congr_left
    intro e he
    obtain ⟨k, o⟩ := e
    have := h (k, o) he
    obtain ⟨sc, un, rc⟩ := o
    simp only at this
    subst this
    simp [Function.comp, absEntry, IObj.abs, IObj.ofEmpty, absSlots, ascend]

theorem ofEmpty_inv (t : Tenant) : (ITenant.ofEmpty t).Inv := by
  intro ty o h
  unfold ITenant.typ? ITenant.ofEmpty at h
  have := lookup_map_entry t.types (fun _ o => IObj.ofEmpty o) ty
  rw [this] at h
  cases hl : t.types.lookup ty with
  | none => simp [hl] at h
  | some o2 =>
    simp only [hl, Option.map_some, Option.some.injEq] at h
    subst h
    simp [IObj.ofEmpty]

/-- The implementation's start state for a fixture tenant: its records are empty, so each type
    starts as an empty slot array. -/
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
