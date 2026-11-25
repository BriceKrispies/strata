/-
The executable kernel, stage 2: the indexed step is the stage 1 step, and keeps the indexes
consistent.

`Good r ri` says the indexed outcome `r` and the stage 1 outcome `ri` agree: the same refusal, or the
same tenant, and an accepted indexed tenant keeps `XTenant.Ok`. Each intent has a theorem
(`create_good`, ..., `defineUnique_good`); `stepTenantX_good` collects them.
-/

import Strata.Impl.XLemmas

namespace Strata
namespace Impl

/-- The indexed outcome agrees with the stage 1 outcome, and an accepted indexed tenant is
    consistent. -/
def Good (x : XTenant) (r : Except (Err × XTenant) XTenant) (ri : Except Err ITenant) : Prop :=
  (r.mapError Prod.fst).map XTenant.core = ri ∧ (∀ e x', r = .error (e, x') → x' = x) ∧
    ∀ y, r = .ok y → y.Ok

theorem good_error (x : XTenant) (e : Err) : Good x (.error (e, x)) (.error e) :=
  ⟨rfl, fun e' x' h => by cases h; rfl, fun y h => by cases h⟩

theorem create_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (p : Principal) (ty : TypeId) (fs : Fields) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.create ty fs)) (stepTenantI core p (.create ty fs)) := by
  have hok' := hok
  obtain ⟨hinv, hnd, hu, hc, hr⟩ := hok'
  dsimp only at hinv hnd hu hc hr
  simp only [stepParts, stepTenantI]
  cases hty : core.typ? ty with
  | none => exact good_error _ _
  | some o =>
    simp only [hty]
    cases hr : newRecord o.schema p fs with
    | error u => simp only [hr]; exact good_error _ _
    | ok r =>
      simp only [hr]
      by_cases hca : canCreateI core p ty r = true
      · simp only [hca, Bool.not_true, if_false, Bool.false_eq_true]
        have hp := problemX_eq ⟨core, ux, cx, rx⟩ hok ty o hty core.limits true core.nextId r
        dsimp only at hp
        rw [hp]
        cases hq : problemI core.limits o.schema o.uniques true core.slotsOf o.slots core.nextId
            r with
        | some e => exact good_error _ e
        | none =>
          simp only []
          cases hk : constraintProblem (writeEnv p r none) o.constraints with
          | some e => simp only [hk]; exact good_error _ e
          | none =>
            simp only [hk]
            have hsz := hinv ty o hty
            have hmod := ITenant.modifyType_eq core ty
              (fun o' => { o' with slots := (padTo o'.slots core.nextId).push (some r) }) o hty
            refine ⟨?_, fun e x' h => (by cases h), ?_⟩
            · simp only [Except.mapError, Except.map, XTenant.reindex]
              rw [hmod]
            · intro y hy
              simp only [Except.ok.injEq] at hy
              subst hy
              rw [hmod]
              refine XTenant.ok_reindex ⟨core, ux, cx, rx⟩ _ ty core.nextId o none (some r) hok ?_
                hty (getRec_of_ge _ _ hsz) ?_
              · exact CellChange.withNextId (cc_putType core ty core.nextId (some r) o
                  { o with slots := (padTo o.slots core.nextId).push (some r) }
                  hty rfl rfl
                  (fun i hi => by rw [getRec_push_padTo _ _ _ hsz]; simp [hi])
                  (by rw [getRec_push_padTo _ _ _ hsz]; simp only [if_true])) _
              · exact inv_putType core ty _ (core.nextId + 1) (by omega)
                  (by simp [size_padTo _ _ hsz]) hinv
      · have : canCreateI core p ty r = false := by simpa using hca
        simp only [this, Bool.not_false, if_true]
        exact good_error _ _

theorem update_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (p : Principal) (ty : TypeId) (id : RecordId)
    (fs : Fields) (ev : Option Nat) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.update ty id fs ev))
      (stepTenantI core p (.update ty id fs ev)) := by
  have hok' := hok
  obtain ⟨hinv, hnd, hu, hc, hr⟩ := hok'
  dsimp only at hinv hnd hu hc hr
  simp only [stepParts, stepTenantI]
  cases hty : core.typ? ty with
  | none => exact good_error _ _
  | some o =>
    simp only [hty]
    cases hg : getRec o.slots id with
    | none => exact good_error _ _
    | some old =>
      simp only [hg]
      by_cases hca : canUpdateI core p ty old (patched old fs) = true
      · simp only [hca, Bool.not_true, if_false, Bool.false_eq_true]
        by_cases hv : versionClash ev old = true
        · simp only [hv, if_true]
          exact good_error _ _
        · simp only [hv, if_false, Bool.false_eq_true]
          have hp := problemX_eq ⟨core, ux, cx, rx⟩ hok ty o hty core.limits true id (patched old fs)
          dsimp only at hp
          rw [hp]
          cases hq : problemI core.limits o.schema o.uniques true core.slotsOf o.slots id
              (patched old fs) with
          | some e => exact good_error _ e
          | none =>
            simp only []
            cases hk : constraintProblem (writeEnv p (patched old fs) (some old)) o.constraints with
            | some e => simp only [hk]; exact good_error _ e
            | none =>
            simp only [hk]
            have hsz := hinv ty o hty
            have hlt := lt_of_getRec _ _ _ hg
            have hmod := ITenant.modifyType_eq core ty
              (fun o' => { o' with slots := setSlot o'.slots id (some (patched old fs)) }) o hty
            refine ⟨?_, fun e x' h => (by cases h), ?_⟩
            · simp only [Except.mapError, Except.map, XTenant.reindex]
              rw [hmod]
            · intro y hy
              simp only [Except.ok.injEq] at hy
              subst hy
              rw [hmod]
              refine XTenant.ok_reindex ⟨core, ux, cx, rx⟩ _ ty id o (some old)
                (some (patched old fs)) hok ?_ hty hg ?_
              · exact cc_putType core ty id (some (patched old fs)) o
                  { o with slots := setSlot o.slots id (some (patched old fs)) } hty rfl rfl
                  (fun i hi => by rw [getRec_setSlot _ _ _ hlt]; simp [hi])
                  (by rw [getRec_setSlot _ _ _ hlt]; simp only [if_true])
              · exact inv_putType core ty _ core.nextId (Nat.le_refl _)
                  (by simp [size_setSlot, hsz]) hinv
      · have : canUpdateI core p ty old (patched old fs) = false := by simpa using hca
        simp only [this, Bool.not_false, if_true]
        exact good_error _ _

end Impl
end Strata
