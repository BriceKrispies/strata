/-
The executable kernel, stage 2: schema changes.

A schema change replaces one object type's schema or constraints and leaves its records where they
are. Making a field required changes no key of any index. Making a field unique makes more records
carry unique-index keys, so the records of the type are indexed for it (`MM.addCells`); defining a
composite constraint does the same for the composite index. The checks that refuse a strict change are
stage 1's, run on the abstraction once per change.
-/

import Strata.Impl.XDelete

namespace Strata
namespace Impl

/-- Replacing an object type by one with the same slots changes the keys of the cells of that type
    to those the new object gives, and no other cell's. -/
theorem keysBy_putSlots {K : Type} (kf : TypeId → IObj → Record → List K) (t : ITenant)
    (ty : TypeId) (o o' : IObj) (ho : t.typ? ty = some o) (hsl : o'.slots = o.slots) (c : Cell) :
    (t.putType ty o').keysBy kf c =
      if c.1 = ty then (match getRec o.slots c.2 with | some r => kf ty o' r | none => [])
      else t.keysBy kf c := by
  unfold ITenant.keysBy ITenant.cellRec
  by_cases hT : c.1 = ty
  · rw [typ?_putType, hT]
    simp [ho, hsl, hT]
    cases getRec o.slots c.2 <;> rfl
  · have : (c.1 == ty) = false := by simp [hT]
    rw [typ?_putType]
    simp [this, hT]

theorem lookup_ty_map (l : Schema) (g : FieldId × FieldDef → FieldId × FieldDef)
    (hk : ∀ e, (g e).1 = e.1) (ht : ∀ e, (g e).2.ty = e.2.ty) (k : FieldId) :
    ((l.map g).lookup k).map FieldDef.ty = (l.lookup k).map FieldDef.ty := by
  induction l with
  | nil => rfl
  | cons e es ih =>
    simp only [List.map_cons, List.lookup, hk]
    cases h : (k == e.1)
    · exact ih
    · simp [ht]

theorem lookup_ty_makeRequired (s : Schema) (f g : FieldId) :
    ((makeRequired s f).lookup g).map FieldDef.ty = (s.lookup g).map FieldDef.ty :=
  lookup_ty_map s _ (fun e => by by_cases h : e.1 == f <;> simp [h])
    (fun e => by by_cases h : e.1 == f <;> simp [h]) g

theorem lookup_ty_makeUnique (s : Schema) (f g : FieldId) :
    ((makeUnique s f).lookup g).map FieldDef.ty = (s.lookup g).map FieldDef.ty :=
  lookup_ty_map s _ (fun e => by by_cases h : e.1 == f <;> simp [h])
    (fun e => by by_cases h : e.1 == f <;> simp [h]) g

theorem rkeysOf_congr (s s' : Schema) (r : Record)
    (h : ∀ g, (s'.lookup g).map FieldDef.ty = (s.lookup g).map FieldDef.ty) :
    rkeysOf s' r = rkeysOf s r := by
  unfold rkeysOf
  congr 1
  funext fv
  have := h fv.1
  cases h1 : s.lookup fv.1 with
  | none =>
    cases h2 : s'.lookup fv.1 with
    | none => rfl
    | some d' => rw [h1, h2] at this; simp at this
  | some d =>
    cases h2 : s'.lookup fv.1 with
    | none => rw [h1, h2] at this; simp at this
    | some d' =>
      rw [h1, h2] at this
      simp only [Option.map_some, Option.some.injEq] at this
      simp [this]

theorem ukeysOf_makeRequired (S : TypeId) (s : Schema) (f : FieldId) (r : Record) :
    ukeysOf S (makeRequired s f) r = ukeysOf S s r := by
  unfold ukeysOf makeRequired
  rw [List.filterMap_map]
  congr 1
  funext fd
  by_cases h : fd.1 = f <;> simp [h, Function.comp]

theorem ukeysOf_makeUnique_sub (S : TypeId) (s : Schema) (f : FieldId) (r : Record)
    (k : TypeId × FieldId × Val) (hk : k ∈ ukeysOf S s r) : k ∈ ukeysOf S (makeUnique s f) r := by
  rw [mem_ukeysOf] at hk ⊢
  obtain ⟨h1, fd, hfd, h2, h3, h4, h5⟩ := hk
  refine ⟨h1, if fd.1 == f then (fd.1, { fd.2 with unique := true }) else fd, ?_, ?_, ?_, h4, h5⟩
  · unfold makeUnique
    exact List.mem_map.mpr ⟨fd, hfd, rfl⟩
  · by_cases h : (fd.1 == f) = true
    · rw [if_pos h]; exact h2
    · rw [if_neg h]; exact h2
  · by_cases h : (fd.1 == f) = true
    · rw [if_pos h]
    · rw [if_neg h]; exact h3

theorem mem_cellsOf (ty : TypeId) (n : Nat) (c : Cell) : c ∈ cellsOf ty n ↔ c.1 = ty ∧ c.2 < n := by
  obtain ⟨T, i⟩ := c
  unfold cellsOf
  simp only [List.mem_map, List.mem_range, Prod.mk.injEq]
  constructor
  · rintro ⟨j, hj, h1, h2⟩; subst h1; subst h2; exact ⟨rfl, hj⟩
  · rintro ⟨h1, h2⟩; subst h1; exact ⟨i, h2, rfl, rfl⟩

theorem keysBy_ty {K : Type} (kf : TypeId → IObj → Record → List K) (t : ITenant) (c : Cell)
    (o : IObj) (hT : t.typ? c.1 = some o) :
    t.keysBy kf c = (match getRec o.slots c.2 with | some r => kf c.1 o r | none => []) := by
  unfold ITenant.keysBy ITenant.cellRec
  simp only [hT]
  cases getRec o.slots c.2 <;> rfl

/-- An index that is exactly the keys of the cells keeps being so when an object type is replaced by
    one with the same slots and the same keys for every record. -/
theorem MM.inv_putSlots {K : Type} [BEq K] [Hashable K] [LawfulBEq K] [LawfulHashable K]
    (m : MM K) (kf : TypeId → IObj → Record → List K) (t : ITenant) (ty : TypeId) (o o' : IObj)
    (ho : t.typ? ty = some o) (hsl : o'.slots = o.slots) (hkf : ∀ r, kf ty o' r = kf ty o r)
    (hm : m.Inv (t.keysBy kf)) : m.Inv ((t.putType ty o').keysBy kf) := by
  intro k c
  rw [keysBy_putSlots kf t ty o o' ho hsl c, hm k c]
  by_cases hT : c.1 = ty
  · simp only [hT, if_true]
    have := keysBy_ty kf t c o (by rw [hT]; exact ho)
    rw [this, hT]
    cases getRec o.slots c.2 <;> simp [hkf]
  · simp [hT]

theorem rkeysOf_congr' (s s' : Schema) (r : Record)
    (h : ∀ g, (s'.lookup g).map FieldDef.ty = (s.lookup g).map FieldDef.ty ∨
      (s.lookup g = none ∧ (s'.lookup g).map FieldDef.ty = some Ty.bool)) :
    rkeysOf s' r = rkeysOf s r := by
  unfold rkeysOf
  congr 1
  funext fv
  rcases h fv.1 with h1 | ⟨h0, h1⟩
  · cases hl : s.lookup fv.1 with
    | none =>
      cases hl' : s'.lookup fv.1 with
      | none => rfl
      | some d' => rw [hl, hl'] at h1; simp at h1
    | some d =>
      cases hl' : s'.lookup fv.1 with
      | none => rw [hl, hl'] at h1; simp at h1
      | some d' =>
        rw [hl, hl'] at h1
        simp only [Option.map_some, Option.some.injEq] at h1
        simp [h1]
  · rw [h0]
    cases hl' : s'.lookup fv.1 with
    | none => simp [hl'] at h1
    | some d' =>
      rw [hl'] at h1
      simp only [Option.map_some, Option.some.injEq] at h1
      simp [Ty.target?, h1]

theorem ens_makeRequired_ty' (s : Schema) (f g : FieldId) :
    ((makeRequired (ensureField s f) f).lookup g).map FieldDef.ty = (s.lookup g).map FieldDef.ty ∨
      (s.lookup g = none ∧ ((makeRequired (ensureField s f) f).lookup g).map FieldDef.ty = some Ty.bool) := by
  rw [lookup_ty_makeRequired]
  rcases ensureField_lookup s f g with h | ⟨h0, h1⟩
  · left; rw [h]
  · right; refine ⟨h0, ?_⟩; rw [h1]; rfl

theorem ens_makeUnique_ty' (s : Schema) (f g : FieldId) :
    ((makeUnique (ensureField s f) f).lookup g).map FieldDef.ty = (s.lookup g).map FieldDef.ty ∨
      (s.lookup g = none ∧ ((makeUnique (ensureField s f) f).lookup g).map FieldDef.ty = some Ty.bool) := by
  rw [lookup_ty_makeUnique]
  rcases ensureField_lookup s f g with h | ⟨h0, h1⟩
  · left; rw [h]
  · right; refine ⟨h0, ?_⟩; rw [h1]; rfl

theorem ukeysOf_ensureField (S : TypeId) (s : Schema) (f : FieldId) (r : Record) :
    ukeysOf S (ensureField s f) r = ukeysOf S s r := by
  unfold ensureField
  split
  · rfl
  · unfold ukeysOf
    rw [List.filterMap_append]
    simp [plainField]

theorem ukeysOf_ens_makeRequired (S : TypeId) (s : Schema) (f : FieldId) (r : Record) :
    ukeysOf S (makeRequired (ensureField s f) f) r = ukeysOf S s r := by
  rw [ukeysOf_makeRequired, ukeysOf_ensureField]

theorem ukeysOf_ens_makeUnique_sub (S : TypeId) (s : Schema) (f : FieldId) (r : Record)
    (k : TypeId × FieldId × Val) (hk : k ∈ ukeysOf S s r) :
    k ∈ ukeysOf S (makeUnique (ensureField s f) f) r := by
  apply ukeysOf_makeUnique_sub
  rw [ukeysOf_ensureField]
  exact hk

theorem setRequired_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (p : Principal) (ty : TypeId) (f : FieldId)
    (strict : Bool) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.setRequired ty f strict))
      (stepTenantI core p (.setRequired ty f strict)) := by
  have hok' := hok
  obtain ⟨hinv, hnd, hu, hc, hr⟩ := hok'
  dsimp only at hinv hnd hu hc hr
  simp only [stepParts, stepTenantI]
  by_cases ho : (!canManage core.header p) = true
  · simp only [ho, if_true]; exact good_error _ _
  · simp only [ho, if_false, Bool.false_eq_true]
    cases hty : core.typ? ty with
    | none => exact good_error _ _
    | some o =>
      simp only [hty]
      by_cases hs : schemaChangeRefused core.limits
          (scopedTypes (core.putType ty { o with schema := makeRequired (ensureField o.schema f) f }).abs.types
            (ty :: fieldTargets (ensureField o.schema f) f)) strict = true
      · simp only [hs, if_true]; exact good_error _ _
      · simp only [hs, if_false, Bool.false_eq_true]
        refine ⟨rfl, fun e x' h => (by cases h), ?_⟩
        intro y hy
        simp only [Except.ok.injEq] at hy
        subst hy
        have hsl : ({ o with schema := makeRequired (ensureField o.schema f) f } : IObj).slots = o.slots := rfl
        refine ⟨?_, ?_, ?_, ?_, ?_⟩
        · exact inv_putType core ty _ core.nextId (Nat.le_refl _) (hinv ty o hty) hinv
        · show ((putTypeI core.types ty _).map Prod.fst).Nodup
          rw [putTypeI_keys]; exact hnd
        · exact MM.inv_putSlots ux (fun S o r => ukeysOf S o.schema r) core ty o _ hty hsl
            (fun r => ukeysOf_ens_makeRequired ty o.schema f r) hu
        · exact MM.inv_putSlots cx (fun S o r => ckeysOf S o.uniques r) core ty o _ hty hsl
            (fun r => rfl) hc
        · exact MM.inv_putSlots rx (fun S o r => rkeysOf o.schema r) core ty o _ hty hsl
            (fun r => rkeysOf_congr' _ _ r (ens_makeRequired_ty' o.schema f)) hr

theorem setUnique_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (p : Principal) (ty : TypeId) (f : FieldId)
    (strict : Bool) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.setUnique ty f strict))
      (stepTenantI core p (.setUnique ty f strict)) := by
  have hok' := hok
  obtain ⟨hinv, hnd, hu, hc, hr⟩ := hok'
  dsimp only at hinv hnd hu hc hr
  simp only [stepParts, stepTenantI]
  by_cases ho : (!canManage core.header p) = true
  · simp only [ho, if_true]; exact good_error _ _
  · simp only [ho, if_false, Bool.false_eq_true]
    cases hty : core.typ? ty with
    | none => exact good_error _ _
    | some o =>
      simp only [hty]
      cases hl : (ensureField o.schema f).lookup f with
      | none => exact good_error _ _
      | some d =>
        simp only [hl]
        by_cases hd : d.ty.isList = true
        · simp only [hd, if_true]; exact good_error _ _
        · simp only [hd, if_false, Bool.false_eq_true]
          by_cases hs : schemaChangeRefused core.limits
              (scopedTypes (core.putType ty { o with schema := makeUnique (ensureField o.schema f) f }).abs.types
                (ty :: fieldTargets (ensureField o.schema f) f)) strict = true
          · simp only [hs, if_true]; exact good_error _ _
          · simp only [hs, if_false, Bool.false_eq_true]
            refine ⟨rfl, fun e x' h => (by cases h), ?_⟩
            intro y hy
            simp only [Except.ok.injEq] at hy
            subst hy
            have hsl : ({ o with schema := makeUnique (ensureField o.schema f) f } : IObj).slots = o.slots := rfl
            refine ⟨?_, ?_, ?_, ?_, ?_⟩
            · exact inv_putType core ty _ core.nextId (Nat.le_refl _) (hinv ty o hty) hinv
            · show ((putTypeI core.types ty _).map Prod.fst).Nodup
              rw [putTypeI_keys]; exact hnd
            · -- the unique index gains the keys of the new unique field
              refine MM.inv_addCells ux (cellsOf ty o.slots.size)
                (ukeysUnder ty o (makeUnique (ensureField o.schema f) f))
                core.ukeys _ hu ?_
              intro c k
              have h1 := keysBy_putSlots (fun S o r => ukeysOf S o.schema r) core ty o
                { o with schema := makeUnique (ensureField o.schema f) f } hty hsl c
              show k ∈ (core.putType ty _).keysBy (fun S o r => ukeysOf S o.schema r) c ↔ _
              rw [h1, mem_cellsOf]
              by_cases hT : c.1 = ty
              · have h2 : core.ukeys c = (match getRec o.slots c.2 with
                    | some r => ukeysOf c.1 o.schema r | none => []) :=
                  keysBy_ty (fun S o r => ukeysOf S o.schema r) core c o (by rw [hT]; exact hty)
                rw [h2]
                simp only [hT, if_true]
                unfold ukeysUnder
                cases hg : getRec o.slots c.2 with
                | none => simp
                | some r =>
                  have hlt := lt_of_getRec _ _ _ hg
                  simp only [hlt, and_true, true_and]
                  constructor
                  · intro hk; exact Or.inr hk
                  · rintro (hk | hk)
                    · exact ukeysOf_ens_makeUnique_sub _ _ _ _ _ hk
                    · exact hk
              · have h2 : core.ukeys c = core.keysBy (fun S o r => ukeysOf S o.schema r) c := rfl
                rw [h2]
                simp [hT]
            · exact MM.inv_putSlots cx (fun S o r => ckeysOf S o.uniques r) core ty o _ hty hsl
                (fun r => rfl) hc
            · exact MM.inv_putSlots rx (fun S o r => rkeysOf o.schema r) core ty o _ hty hsl
                (fun r => rkeysOf_congr' _ _ r (ens_makeUnique_ty' o.schema f)) hr

theorem defineUnique_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (p : Principal) (ty : TypeId) (fs : List FieldId) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.defineUnique ty fs))
      (stepTenantI core p (.defineUnique ty fs)) := by
  have hok' := hok
  obtain ⟨hinv, hnd, hu, hc, hr⟩ := hok'
  dsimp only at hinv hnd hu hc hr
  simp only [stepParts, stepTenantI]
  by_cases ho : (!canManage core.header p) = true
  · simp only [ho, if_true]; exact good_error _ _
  · simp only [ho, if_false, Bool.false_eq_true]
    cases hty : core.typ? ty with
    | none => exact good_error _ _
    | some o =>
      simp only [hty]
      by_cases hu1 : (!uniqueFieldsOk o.schema fs) = true
      · simp only [hu1, if_true]; exact good_error _ _
      · simp only [hu1, if_false, Bool.false_eq_true]
        by_cases hd : dupFields (absSlots o.slots) fs = true
        · simp only [hd, if_true]; exact good_error _ _
        · simp only [hd, if_false, Bool.false_eq_true]
          by_cases hs : schemaChangeRefused core.limits
              (scopedTypes (core.putType ty { o with uniques := o.uniques ++ [fs] }).abs.types [ty])
              true = true
          · simp only [hs, if_true]; exact good_error _ _
          · simp only [hs, if_false, Bool.false_eq_true]
            refine ⟨rfl, fun e x' h => (by cases h), ?_⟩
            intro y hy
            simp only [Except.ok.injEq] at hy
            subst hy
            have hsl : ({ o with uniques := o.uniques ++ [fs] } : IObj).slots = o.slots := rfl
            refine ⟨?_, ?_, ?_, ?_, ?_⟩
            · exact inv_putType core ty _ core.nextId (Nat.le_refl _) (hinv ty o hty) hinv
            · show ((putTypeI core.types ty _).map Prod.fst).Nodup
              rw [putTypeI_keys]; exact hnd
            · exact MM.inv_putSlots ux (fun S o r => ukeysOf S o.schema r) core ty o _ hty hsl
                (fun r => rfl) hu
            · refine MM.inv_addCells cx (cellsOf ty o.slots.size) (ckeysUnder ty o fs)
                core.ckeys _ hc ?_
              intro c k
              have h1 := keysBy_putSlots (fun S o r => ckeysOf S o.uniques r) core ty o
                { o with uniques := o.uniques ++ [fs] } hty hsl c
              show k ∈ (core.putType ty _).keysBy (fun S o r => ckeysOf S o.uniques r) c ↔ _
              rw [h1, mem_cellsOf]
              by_cases hT : c.1 = ty
              · have h2 : core.ckeys c = (match getRec o.slots c.2 with
                    | some r => ckeysOf c.1 o.uniques r | none => []) :=
                  keysBy_ty (fun S o r => ckeysOf S o.uniques r) core c o (by rw [hT]; exact hty)
                rw [h2]
                simp only [hT, if_true]
                unfold ckeysUnder
                cases hg : getRec o.slots c.2 with
                | none => simp
                | some r =>
                  have hlt := lt_of_getRec _ _ _ hg
                  simp only [hlt, and_true, true_and]
                  unfold ckeysOf
                  rw [List.filterMap_append, List.mem_append]
              · have h2 : core.ckeys c = core.keysBy (fun S o r => ckeysOf S o.uniques r) c := rfl
                rw [h2]
                simp [hT]
            · exact MM.inv_putSlots rx (fun S o r => rkeysOf o.schema r) core ty o _ hty hsl
                (fun r => rfl) hr

end Impl
end Strata
