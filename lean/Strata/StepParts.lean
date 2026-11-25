/-
The parts of an accepted record write (slice 5).

`stepTenant` decides a create or an update in a fixed order: the object type, the defaults of a
create (`newRecord`), the permission, the structural checks (`problem`), then the constraints
(`constraintProblem`). An accepted write therefore has each of these parts as its premise. The two
lemmas below state those premises, so that the properties about the records an accepted write
writes can be proved from them without repeating the decision.
-/
import Strata.Model

namespace Strata

/-- An accepted create writes the record its defaults give, the structural checks and the
    constraints pass on it, and the tenant has it under the next id with the counter one on. -/
theorem create_ok_parts {t t' : Tenant} {p : PrincipalId} {ty : TypeId} {fs : Fields}
    {o : ObjType} (ho : t.typ? ty = some o) (h : stepTenant t p (.create ty fs) = .ok t') :
    ∃ r, newRecord o.schema p fs = .ok r ∧
      problem t.limits o.schema o.uniques true t.env o.records t.nextId r = none ∧
      constraintProblem (writeEnv p r none) o.constraints = none ∧
      t' = { t.putType ty { o with records := (t.nextId, r) :: o.records } with
        nextId := t.nextId + 1 } := by
  simp only [stepTenant, ho] at h
  cases hN : newRecord o.schema p fs with
  | error e => simp [hN] at h
  | ok r =>
    simp only [hN] at h
    split at h
    · cases h
    · cases hP : problem t.limits o.schema o.uniques true t.env o.records t.nextId r with
      | some e => simp [hP] at h
      | none =>
        simp only [hP] at h
        cases hC : constraintProblem (writeEnv p r none) o.constraints with
        | some e => simp [hC] at h
        | none =>
          simp only [hC] at h
          exact ⟨r, rfl, hP, hC, (Except.ok.inj h).symm⟩

/-- An accepted update writes the stored record patched, the structural checks and the constraints
    pass on it, and the tenant has it in place of the stored one. -/
theorem update_ok_parts {t t' : Tenant} {p : PrincipalId} {ty : TypeId} {id : RecordId}
    {fs : Fields} {ev : Option Nat} {o : ObjType} {old : Record} (ho : t.typ? ty = some o)
    (hl : o.records.lookup id = some old) (h : stepTenant t p (.update ty id fs ev) = .ok t') :
    problem t.limits o.schema o.uniques true t.env o.records id (patched old fs) = none ∧
      constraintProblem (writeEnv p (patched old fs) (some old)) o.constraints = none ∧
      t' = t.putType ty { o with records := o.records.map (fun q =>
        if q.1 == id then (q.1, patched old fs) else q) } := by
  simp only [stepTenant, ho, hl] at h
  split at h
  · cases h
  · split at h
    · cases h
    · cases hP : problem t.limits o.schema o.uniques true t.env o.records id (patched old fs) with
      | some e => simp [hP] at h
      | none =>
        simp only [hP] at h
        cases hC : constraintProblem (writeEnv p (patched old fs) (some old)) o.constraints with
        | some e => simp [hC] at h
        | none =>
          simp only [hC] at h
          exact ⟨rfl, rfl, (Except.ok.inj h).symm⟩

/-! ## Defaults of a create -/

/-- The defaults of a create, read off the schema: a field the intent does not name whose default
    evaluates to a value other than `null` is given that value, in the list `defaultsOf` returns. -/
theorem defaultsOf_mem (env : ExprEnv) (fs : Fields) :
    ∀ (schema : Schema) (ds : Fields) (f : FieldId) (d : FieldDef) (v : Val),
      defaultsOf env schema fs = .ok ds → (f, d) ∈ schema → fs.any (fun fv => fv.1 == f) = false →
      evalExpr env d.default = .ok v → v ≠ .null → (f, v) ∈ ds
  | [], ds, f, d, v, h, hm, _, _, _ => by simp at hm
  | (g, e) :: ss, ds, f, d, v, h, hm, hn, he, hv => by
    unfold defaultsOf at h
    cases hr : defaultsOf env ss fs with
    | error x => simp [hr] at h
    | ok rest =>
      simp only [hr] at h
      rcases List.mem_cons.mp hm with hhead | htail
      · have hfg : f = g := (Prod.mk.inj hhead).1
        have hdg : d = e := (Prod.mk.inj hhead).2
        subst hfg
        subst hdg
        simp only [hn, Bool.false_eq_true, if_false, he] at h
        cases v with
        | null => exact absurd rfl hv
        | int i => simp at h; subst h; simp
        | str s => simp at h; subst h; simp
        | bool b => simp at h; subst h; simp
        | ref x => simp at h; subst h; simp
        | list vs => simp at h; subst h; simp
      · have hin : (f, v) ∈ rest := defaultsOf_mem env fs ss rest f d v hr htail hn he hv
        by_cases hg : fs.any (fun fv => fv.1 == g) = true
        · simp only [hg, if_true] at h
          have hds : rest = ds := Except.ok.inj h
          subst hds
          exact hin
        · have hg' : fs.any (fun fv => fv.1 == g) = false := Bool.eq_false_iff.mpr hg
          simp only [hg', Bool.false_eq_true, if_false] at h
          split at h
          · cases h
          · have hds := Except.ok.inj h
            subst hds
            simp [hin]
          · have hds := Except.ok.inj h
            subst hds
            simp [hin]

/-- What an accepted create's record is, read off the schema's defaults: the named fields, then
    the defaults. -/
theorem newRecord_parts {schema : Schema} {p : PrincipalId} {fs : Fields} {r : Record}
    (h : newRecord schema p fs = .ok r) :
    ∃ ds, defaultsOf { principal := some p } schema fs = .ok ds ∧
      r = { owner := p, version := 1, fields := normalise fs ++ ds } := by
  unfold newRecord withDefaults at h
  cases hD : defaultsOf { principal := some p } schema fs with
  | error x => simp [hD, Except.map] at h
  | ok ds =>
    refine ⟨ds, rfl, ?_⟩
    simp only [hD, Except.map] at h
    exact (Except.ok.inj h).symm

end Strata
