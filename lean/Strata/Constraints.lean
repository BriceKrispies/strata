/-
Slice 5 of the reference model: expressions and constraints.

What the slice claims, about the model in `Strata/Model.lean` (section "Expressions" and the write
checks of `stepTenant`):

- The three-valued connectives and comparisons, as truth tables (`and_*`, `or_*`, `not_*`,
  `cmp_*`, `isNull_*`). The kernel's rules: a decisive operand (`false` for `and`, `true` for `or`)
  decides before the other operand is looked at, so an evaluation error there is not seen; otherwise
  `null` is contagious; a non-boolean operand of a connective is an error.
- Constraints are checked last, after every other check of a write. A constraint that is `false` on
  the record refuses it with `constraintViolated`; one that cannot be evaluated refuses it with
  `evaluation`; `true` and `null` pass (`constraint_violated_refused`, `constraint_evaluation_refused`,
  `null_constraint_passes`). No state follows a refusal (`create_constraint_refused`,
  `update_constraint_refused`).
- Expression defaults. The permission to create is asked first: a caller whom nothing could allow to
  create on the type is refused `unauthorized` whatever the defaults are (`create_unauthorized_regardless_of_defaults`,
  `unauthorized_regardless_of_defaults`; WG-5665). A caller who could create gets the defaults next: a
  default that cannot be evaluated refuses the create with `evaluation` (`create_default_error_when_permitted`),
  and a default that violates a constraint refuses it with `constraintViolated` (`default_refused_by_constraint`).
- The constraint invariant. For the constraints the model admits (`Tenant.StateConstraints`: they read
  no old record and no principal, see `Expr.stateOnly`), every accepted create, update and schema change
  leaves every stored record satisfying every constraint of its type (`stepTenant_preserves_satisfied`,
  `step_preserves_satisfied`).
- A delete does not keep that invariant. The kernel clears the references a delete removes without
  checking the constraints of the records it rewrites, so an accepted delete can leave a record that
  violates a constraint (`delete_breaks_satisfied`, with a concrete tenant). This is a finding about the
  kernel's behaviour that the model reproduces, not a property the model proves.

No `sorry`, no added axioms. `Audit.lean` prints the axioms each theorem depends on.
-/

import Strata.StepParts
import Strata.Uniqueness

namespace Strata

/-! ## Truth tables of the connectives and comparisons

The tables are closed terms of the language, so each is decided by evaluation. Together they give
the rules the kernel's `tenant-rule-language` follows (`kleene` in `model-core`). -/

/-- `false and x` is `false` whatever `x` is, even an error: the left operand decides. -/
theorem and_false_shortcircuit : evalExpr {} (.and (.lit (.bool false))
    (.cmp .eq (.lit (.int 1)) (.lit (.str "x")))) = .ok (.bool false) := rfl

/-- `true or x` is `true` whatever `x` is, even an error. -/
theorem or_true_shortcircuit : evalExpr {} (.or (.lit (.bool true))
    (.cmp .eq (.lit (.int 1)) (.lit (.str "x")))) = .ok (.bool true) := rfl

theorem and_null_false : evalExpr {} (.and (.lit .null) (.lit (.bool false))) = .ok (.bool false) := rfl
theorem and_null_true : evalExpr {} (.and (.lit .null) (.lit (.bool true))) = .ok .null := rfl
theorem and_null_null : evalExpr {} (.and (.lit .null) (.lit .null)) = .ok .null := rfl
theorem and_true_null : evalExpr {} (.and (.lit (.bool true)) (.lit .null)) = .ok .null := rfl
theorem and_true_true : evalExpr {} (.and (.lit (.bool true)) (.lit (.bool true))) = .ok (.bool true) := rfl
theorem and_true_false : evalExpr {} (.and (.lit (.bool true)) (.lit (.bool false))) = .ok (.bool false) := rfl
theorem and_null_error : evalExpr {} (.and (.lit .null) (.lit (.int 1))) = .error () := rfl
theorem and_true_error : evalExpr {} (.and (.lit (.bool true)) (.lit (.int 1))) = .error () := rfl
theorem and_int_error : evalExpr {} (.and (.lit (.int 1)) (.lit (.bool false))) = .error () := rfl

theorem or_null_true : evalExpr {} (.or (.lit .null) (.lit (.bool true))) = .ok (.bool true) := rfl
theorem or_null_false : evalExpr {} (.or (.lit .null) (.lit (.bool false))) = .ok .null := rfl
theorem or_null_null : evalExpr {} (.or (.lit .null) (.lit .null)) = .ok .null := rfl
theorem or_false_null : evalExpr {} (.or (.lit (.bool false)) (.lit .null)) = .ok .null := rfl
theorem or_false_false : evalExpr {} (.or (.lit (.bool false)) (.lit (.bool false))) = .ok (.bool false) := rfl
theorem or_false_true : evalExpr {} (.or (.lit (.bool false)) (.lit (.bool true))) = .ok (.bool true) := rfl
theorem or_false_error : evalExpr {} (.or (.lit (.bool false)) (.lit (.int 1))) = .error () := rfl
theorem or_null_error : evalExpr {} (.or (.lit .null) (.lit (.int 1))) = .error () := rfl

theorem not_true : evalExpr {} (.not (.lit (.bool true))) = .ok (.bool false) := rfl
theorem not_null : evalExpr {} (.not (.lit .null)) = .ok .null := rfl
theorem not_int : evalExpr {} (.not (.lit (.int 1))) = .error () := rfl

theorem isNull_null : evalExpr {} (.isNull (.lit .null)) = .ok (.bool true) := rfl
theorem isNull_value : evalExpr {} (.isNull (.lit (.int 1))) = .ok (.bool false) := rfl

theorem cmp_null_operand : evalExpr {} (.cmp .eq (.lit .null) (.lit (.str "x"))) = .ok .null := rfl
theorem cmp_kind_error : evalExpr {} (.cmp .eq (.lit (.int 1)) (.lit (.str "x"))) = .error () := rfl
theorem cmp_int_lt : evalExpr {} (.cmp .lt (.lit (.int 1)) (.lit (.int 2))) = .ok (.bool true) := rfl
theorem cmp_str_ge : evalExpr {} (.cmp .ge (.lit (.str "b")) (.lit (.str "a"))) = .ok (.bool true) := rfl
theorem cmp_bool_order_error : evalExpr {} (.cmp .lt (.lit (.bool true)) (.lit (.bool false))) = .error () := rfl
theorem cmp_ref_eq : evalExpr {} (.cmp .eq (.lit (.ref 3)) (.lit (.ref 3))) = .ok (.bool true) := rfl

/-- The general form of the decisive rule: a `false` left operand makes `and` `false`, whatever the
    right one evaluates to. -/
theorem and_false_left (env : ExprEnv) (a b : Expr) (h : evalExpr env a = .ok (.bool false)) :
    evalExpr env (.and a b) = .ok (.bool false) := by
  simp [evalExpr, connective, h]

/-- The general form of the decisive rule for `or`. -/
theorem or_true_left (env : ExprEnv) (a b : Expr) (h : evalExpr env a = .ok (.bool true)) :
    evalExpr env (.or a b) = .ok (.bool true) := by
  simp [evalExpr, connective, h]

/-- A `null` left operand of `and` is `null` when the right one is `true` or `null`. -/
theorem and_null_left_nonfalse (env : ExprEnv) (a b : Expr) (h : evalExpr env a = .ok .null)
    (hb : evalExpr env b = .ok .null ∨ evalExpr env b = .ok (.bool true)) :
    evalExpr env (.and a b) = .ok .null := by
  rcases hb with hb | hb <;> simp [evalExpr, connective, h, hb]

/-- An evaluation error in an operand that is looked at is an error of the connective. -/
theorem and_error_left (env : ExprEnv) (a b : Expr) (h : evalExpr env a = .error ()) :
    evalExpr env (.and a b) = .error () := by
  simp [evalExpr, connective, h]

/-! ## Refusals of a write by a constraint -/

/-- A `false` constraint is the violation, and it is the only refusal a `false` gives. -/
theorem constraint_violated_refused (env : ExprEnv) (c : Expr) (cs : List Expr)
    (h : evalExpr env c = .ok (.bool false)) :
    constraintProblem env (c :: cs) = some .constraintViolated := by
  simp [constraintProblem, h]

/-- A constraint that cannot be evaluated refuses the write with `evaluation`. -/
theorem constraint_evaluation_refused (env : ExprEnv) (c : Expr) (cs : List Expr)
    (h : evalExpr env c = .error ()) :
    constraintProblem env (c :: cs) = some .evaluation := by
  simp [constraintProblem, h]

/-- `true` and `null` pass: a constraint that is `null` on a record does not refuse it. -/
theorem null_constraint_passes (env : ExprEnv) (c : Expr) (h : evalExpr env c = .ok .null) :
    constraintProblem env [c] = none := by
  simp [constraintProblem, h]

/-- A constraint is checked only when the constraints before it pass: the first failure decides. -/
theorem constraint_first_failure (env : ExprEnv) (c : Expr) (cs : List Expr)
    (h : evalExpr env c = .ok (.bool true)) :
    constraintProblem env (c :: cs) = constraintProblem env cs := by
  simp [constraintProblem, h]

/-- A create whose structural checks pass is refused by a constraint that fails on the record it
    writes, and no state follows. -/
theorem create_constraint_refused (t : Tenant) (p : Principal) (ty : TypeId) (fs : Fields)
    (o : ObjType) (ho : t.typ? ty = some o) (r : Record)
    (hN : newRecord o.schema p fs = .ok r) (hc : canCreate t p ty r = true)
    (hP : problem t.limits o.schema o.uniques true t.env o.records t.nextId r = none)
    (e : Err) (he : constraintProblem (writeEnv p r none) o.constraints = some e) :
    stepTenant t p (.create ty fs) = .error e ∧ ∀ t', stepTenant t p (.create ty fs) ≠ .ok t' := by
  have hs : stepTenant t p (.create ty fs) = .error e := by
    simp [stepTenant, ho, hN, hc, canCreate_attempt _ _ _ _ hc, hP, he]
  exact ⟨hs, fun t' h => by rw [hs] at h; cases h⟩

/-- An update whose structural checks pass is refused by a constraint that fails on the record it
    writes, and no state follows. -/
theorem update_constraint_refused (t : Tenant) (p : Principal) (ty : TypeId) (id : RecordId)
    (fs : Fields) (ev : Option Nat) (o : ObjType) (old : Record) (ho : t.typ? ty = some o)
    (hl : o.records.lookup id = some old) (hcan : canUpdate t p ty old (patched old fs) = true)
    (hv : versionClash ev old = false) (hP : problem t.limits o.schema o.uniques true t.env
      o.records id (patched old fs) = none)
    (e : Err) (he : constraintProblem (writeEnv p (patched old fs) (some old)) o.constraints = some e) :
    stepTenant t p (.update ty id fs ev) = .error e ∧
      ∀ t', stepTenant t p (.update ty id fs ev) ≠ .ok t' := by
  have hs : stepTenant t p (.update ty id fs ev) = .error e := by
    simp [stepTenant, ho, hl, hcan, hv, hP, he]
  exact ⟨hs, fun t' h => by rw [hs] at h; cases h⟩

/-- A create by a caller whom nothing could allow to create on the type is refused `unauthorized`,
    whatever its defaults: the permission is asked before the defaults are evaluated (WG-5665). -/
theorem create_unauthorized_regardless_of_defaults (t : Tenant) (p : Principal) (ty : TypeId)
    (fs : Fields) (o : ObjType) (ho : t.typ? ty = some o)
    (hA : canAttemptCreate t p ty = false) :
    stepTenant t p (.create ty fs) = .error .unauthorized := by
  simp [stepTenant, ho, hA]

/-- A create whose defaults fail is refused with `evaluation` for a caller who could create on the
    type: for such a caller the defaults are the first failure (the record's own decision follows). -/
theorem create_default_error_when_permitted (t : Tenant) (p : Principal) (ty : TypeId) (fs : Fields)
    (o : ObjType) (ho : t.typ? ty = some o) (hA : canAttemptCreate t p ty = true)
    (hN : newRecord o.schema p fs = .error ()) :
    stepTenant t p (.create ty fs) = .error .evaluation := by
  simp [stepTenant, ho, hA, hN]

/-- A create whose structural checks pass and whose defaults are written is accepted only when the
    constraints hold of the record with those defaults in it: the defaults are applied before the
    constraints are checked. -/
theorem create_checks_defaulted_record (t t' : Tenant) (p : Principal) (ty : TypeId)
    (fs : Fields) (o : ObjType) (ho : t.typ? ty = some o)
    (h : stepTenant t p (.create ty fs) = .ok t') :
    ∃ r, newRecord o.schema p fs = .ok r ∧
      constraintProblem (writeEnv p r none) o.constraints = none := by
  obtain ⟨r, hN, -, hC, -⟩ := create_ok_parts ho h
  exact ⟨r, hN, hC⟩

/-! ## A concrete tenant: defaults and constraints -/

/-- One object type whose field `1` is an optional integer that defaults to `-1`, and whose only
    constraint says that field is absent or not negative. -/
def dfltObj : ObjType :=
  { schema := [(1, { ty := .int, required := false, default := .lit (.int (-1)) })],
    constraints := [.or (.isNull (.field .record 1)) (.cmp .ge (.field .record 1) (.lit (.int 0)))] }

def dfltTenant : Tenant :=
  { types := [(1, dfltObj)], members := [(0, [ownerRole])], grants := [], policies := [], nextId := 1 }

/-- A create that leaves field `1` to its default writes `-1`, which the constraint refuses. -/
theorem default_refused_by_constraint :
    stepTenant dfltTenant (.user 0) (.create 1 []) = .error .constraintViolated := rfl

/-- A create that names field `1` with a value that satisfies the constraint is accepted. -/
theorem named_value_accepted :
    ∃ t', stepTenant dfltTenant (.user 0) (.create 1 [(1, .int 5)]) = .ok t' := ⟨_, rfl⟩

/-- A tenant with a field defaulting to an expression that fails: the caller is not a member, and the
    refusal is `evaluation` and not `unauthorized`, because the defaults come first. -/
def badDfltObj : ObjType :=
  { schema := [(1, { ty := .bool, required := false, default := .cmp .eq .principal (.lit (.int 1)) })] }

def badDfltTenant : Tenant :=
  { types := [(1, badDfltObj)], members := [(0, [ownerRole])], grants := [], policies := [], nextId := 1 }

/-- The caller is not a member, so it may not create on the type, and its default would fail: the
    refusal is `unauthorized`, the permission being asked first (WG-5665). -/
theorem unauthorized_regardless_of_defaults :
    stepTenant badDfltTenant (.user 9) (.create 1 []) = .error .unauthorized := rfl

/-! ## The constraint invariant -/

/-- An intent that is not a delete. -/
def Intent.noDelete : Intent → Bool
  | .delete _ _ _ => false
  | _ => true

/-- Constraints read the record and the new record, not the old one or the principal, so their value
    on a stored record does not depend on which write checks them. -/
theorem evalExpr_congr (env env' : ExprEnv) (hr : env.record = env'.record)
    (hn : env.new = env'.new) : ∀ (e : Expr), e.stateOnly = true → evalExpr env e = evalExpr env' e
  | .lit _, _ => rfl
  | .field s f, hs => by
    cases s with
    | record => simp [evalExpr, ExprEnv.scoped, hr]
    | old => simp [Expr.stateOnly] at hs
    | new => simp [evalExpr, ExprEnv.scoped, hn]
  | .createdBy s, hs => by
    cases s with
    | record => simp [evalExpr, ExprEnv.scoped, hr]
    | old => simp [Expr.stateOnly] at hs
    | new => simp [evalExpr, ExprEnv.scoped, hn]
  | .principal, hs => by simp [Expr.stateOnly] at hs
  | .cmp op a b, hs => by
    simp only [Expr.stateOnly, Bool.and_eq_true] at hs
    simp only [evalExpr, evalExpr_congr env env' hr hn a hs.1, evalExpr_congr env env' hr hn b hs.2]
  | .and a b, hs => by
    simp only [Expr.stateOnly, Bool.and_eq_true] at hs
    simp only [evalExpr, evalExpr_congr env env' hr hn a hs.1, evalExpr_congr env env' hr hn b hs.2]
  | .or a b, hs => by
    simp only [Expr.stateOnly, Bool.and_eq_true] at hs
    simp only [evalExpr, evalExpr_congr env env' hr hn a hs.1, evalExpr_congr env env' hr hn b hs.2]
  | .not a, hs => by
    simp only [Expr.stateOnly] at hs
    simp only [evalExpr, evalExpr_congr env env' hr hn a hs]
  | .isNull a, hs => by
    simp only [Expr.stateOnly] at hs
    simp only [evalExpr, evalExpr_congr env env' hr hn a hs]

theorem constraintProblem_congr (env env' : ExprEnv) (hr : env.record = env'.record)
    (hn : env.new = env'.new) : ∀ (cs : List Expr), (∀ c ∈ cs, c.stateOnly = true) →
    constraintProblem env cs = constraintProblem env' cs
  | [], _ => rfl
  | c :: cs, h => by
    have hc := evalExpr_congr env env' hr hn c (h c (List.mem_cons_self))
    have hcs := constraintProblem_congr env env' hr hn cs
      (fun x hx => h x (List.mem_cons_of_mem _ hx))
    simp only [constraintProblem, hc, hcs]

/-- The tenant with object type `ty` replaced by one with the same records and constraints (only its
    schema may differ) keeps the constraints of every record. -/
theorem satisfied_of_putType {t : Tenant} {ty : TypeId} {o o' : ObjType}
    (ho : t.typ? ty = some o) (hr : o'.records = o.records) (hc : o'.constraints = o.constraints)
    (hs : t.Satisfied) : (t.putType ty o').Satisfied := by
  intro T o'' hT q hq
  by_cases hTy : T = ty
  · subst hTy
    rw [typ?_putType_self t T o o' ho] at hT
    cases hT
    rw [hc]
    rw [hr] at hq
    exact hs T o ho q hq
  · rw [typ?_putType_other t ty T o' hTy] at hT
    exact hs T o'' hT q hq

/-- An accepted, non-delete step keeps every stored record satisfying the constraints of its type,
    given that the constraints are the state-only kind. -/
theorem stepTenant_preserves_satisfied (t t' : Tenant) (p : Principal) (i : Intent)
    (hs : t.Satisfied) (hst : t.StateConstraints) (hnd : i.noDelete = true)
    (h : stepTenant t p i = .ok t') : t'.Satisfied := by
  cases i with
  | create ty fs =>
    cases hT : t.typ? ty with
    | none => simp [stepTenant, hT] at h
    | some o =>
      obtain ⟨r, -, -, hC, ht⟩ := create_ok_parts hT h
      subst ht
      intro T o' hT' q hq
      change (t.putType ty { o with records := (t.nextId, r) :: o.records }).typ? T = some o' at hT'
      by_cases hTy : T = ty
      · subst hTy
        rw [typ?_putType_self t T o _ hT] at hT'
        cases hT'
        rcases List.mem_cons.mp hq with rfl | hq'
        · have hcongr := constraintProblem_congr (writeEnv p r none) (storedEnv r) rfl rfl
            o.constraints (fun c hc => hst T o hT c hc)
          show constraintProblem (storedEnv r) o.constraints = none
          rw [← hcongr]
          exact hC
        · exact hs T o hT q hq'
      · rw [typ?_putType_other t ty T _ hTy] at hT'
        exact hs T o' hT' q hq
  | update ty id fs ev =>
    cases hT : t.typ? ty with
    | none => simp [stepTenant, hT] at h
    | some o =>
      cases hL : o.records.lookup id with
      | none => simp [stepTenant, hT, hL, missingRecord] at h
      | some old =>
        obtain ⟨hP, hC, ht⟩ := update_ok_parts hT hL h
        subst ht
        intro T o' hT' q hq
        by_cases hTy : T = ty
        · subst hTy
          rw [typ?_putType_self t T o _ hT] at hT'
          cases hT'
          obtain ⟨q₀, hq₀, hmap⟩ := List.mem_map.mp hq
          by_cases hid : (q₀.1 == id) = true
          · simp only [hid, if_true] at hmap
            rw [← hmap]
            show constraintProblem (storedEnv (patched old fs)) o.constraints = none
            have hcongr := constraintProblem_congr (writeEnv p (patched old fs) (some old))
              (storedEnv (patched old fs)) rfl rfl o.constraints (fun c hc => hst T o hT c hc)
            rw [← hcongr]
            exact hC
          · simp only [hid, if_false] at hmap
            rw [← hmap]
            exact hs T o hT q₀ hq₀
        · rw [typ?_putType_other t ty T _ hTy] at hT'
          exact hs T o' hT' q hq
  | setRequired ty f strict =>
    cases hT : t.typ? ty with
    | none => simp only [stepTenant, hT] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hT] at h
      repeat' split at h
      all_goals try cases h
      exact satisfied_of_putType hT rfl rfl hs
  | setUnique ty f strict =>
    cases hT : t.typ? ty with
    | none => simp only [stepTenant, hT] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hT] at h
      repeat' split at h
      all_goals try cases h
      exact satisfied_of_putType hT rfl rfl hs
  | defineUnique ty fs =>
    cases hT : t.typ? ty with
    | none => simp only [stepTenant, hT] at h; split at h <;> cases h
    | some o =>
      simp only [stepTenant, hT] at h
      repeat' split at h
      all_goals try cases h
      exact satisfied_of_putType hT rfl rfl hs
  | delete ty id ev => simp [Intent.noDelete] at hnd

/-- The same for the world: an accepted non-delete step from a world whose tenants satisfy their
    constraints leaves the addressed tenant satisfying them, and the others as they were. -/
theorem step_preserves_satisfied (w w' : World) (i : Intent) (c : Ctx)
    (hs : ∀ x t, w x = some t → t.Satisfied) (hst : ∀ x t, w x = some t → t.StateConstraints)
    (hnd : i.noDelete = true) (h : step w i c = .ok w') : ∀ x t, w' x = some t → t.Satisfied := by
  intro x t' hx
  unfold step at h
  cases ht : w c.tenant with
  | none => rw [ht] at h; cases h
  | some t =>
    rw [ht] at h
    simp only at h
    cases hsT : stepTenant t c.principal i with
    | error e => rw [hsT] at h; cases h
    | ok t₁ =>
      rw [hsT] at h
      cases h
      by_cases hxc : x = c.tenant
      · simp [hxc] at hx
        subst hx
        exact stepTenant_preserves_satisfied t _ c.principal i (hs c.tenant t ht)
          (hst c.tenant t ht) hnd hsT
      · simp [hxc] at hx
        exact hs x t' hx

/-! ## A delete can break the invariant

The kernel's delete clears a reference a record holds to the deleted one, and writes that record
with its version raised, without checking the constraints of the record it rewrites. The model does
the same, so the invariant of the previous section does not survive a delete. -/

/-- Record `1` refers to record `2` and record `2` refers to record `1`, through a field that clears
    on delete; the constraint reads that field. -/
def brkObj : ObjType :=
  { schema := [(1, { ty := .ref 1 .setNull, required := false })],
    constraints := [.not (.isNull (.field .record 1))],
    records := [(1, { owner := some 0, version := 1, fields := [(1, .ref 2)] }),
                (2, { owner := some 0, version := 1, fields := [(1, .ref 1)] })] }

def brkTenant : Tenant :=
  { types := [(1, brkObj)], members := [(0, [ownerRole])], grants := [], policies := [], nextId := 3 }

/-- The object type of `brkTenant` after the delete of record `2`: record `1` has lost its field. -/
def brkAfterObj : ObjType :=
  { schema := brkObj.schema, constraints := brkObj.constraints,
    records := [(1, { owner := some 0, version := 2, fields := [] })] }

def brkAfter : Tenant :=
  { types := [(1, brkAfterObj)], members := [(0, [ownerRole])], grants := [], policies := [], nextId := 3 }

theorem brkTenant_typ (T : TypeId) (o : ObjType) (h : brkTenant.typ? T = some o) :
    T = 1 ∧ o = brkObj := by
  unfold brkTenant Tenant.typ? at h
  by_cases hT : T = 1
  · subst hT
    simp [List.lookup, brkObj] at h
    exact ⟨rfl, h.symm⟩
  · have hb : (T == 1) = false := by simpa using hT
    simp [List.lookup, hb] at h

theorem brkTenant_satisfied : brkTenant.Satisfied := by
  intro T o hT q hq
  obtain ⟨rfl, rfl⟩ := brkTenant_typ T o hT
  have hall : ∀ q ∈ brkObj.records,
      constraintProblem (storedEnv q.2) brkObj.constraints = none := by decide
  exact hall q hq

theorem brkTenant_stateConstraints : brkTenant.StateConstraints := by
  intro T o hT c hc
  obtain ⟨rfl, rfl⟩ := brkTenant_typ T o hT
  simp [brkObj] at hc
  subst hc
  decide

theorem brk_delete_result : stepTenant brkTenant (.user 0) (.delete 1 2 none) = .ok brkAfter := rfl

theorem brkAfter_not_satisfied : ¬ brkAfter.Satisfied := by
  intro h
  have hq := h 1 brkAfterObj rfl (1, { owner := some 0, version := 2, fields := [] })
    (by simp [brkAfterObj])
  exact absurd hq (by decide)

/-- A delete by the owner of an accepted state can leave a stored record violating a constraint. -/
theorem delete_breaks_satisfied :
    ∃ (t t' : Tenant) (p : Principal) (i : Intent), t.Satisfied ∧ t.StateConstraints ∧
      i.noDelete = false ∧ stepTenant t p i = .ok t' ∧ ¬ t'.Satisfied :=
  ⟨brkTenant, brkAfter, .user 0, .delete 1 2 none, brkTenant_satisfied, brkTenant_stateConstraints,
    rfl, brk_delete_result, brkAfter_not_satisfied⟩

end Strata
