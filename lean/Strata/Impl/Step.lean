/-
The executable kernel, stage 1: one intent against one tenant, and against the world.

`stepTenantI` is the spec's `stepTenant` (`Strata/Model.lean`) on slots. The permission tests are
the spec's functions (`canCreate`, `canUpdate`, `canDelete`, `canManage`, `missingRecord`,
`unknownType`), taken on the tenant's header (`ITenant.header`): its members, grants and policies,
which is the part of a tenant those functions read. Record lookups, the write checks and the scans
go through `Strata/Impl/Store.lean` and `Strata/Impl/Problem.lean`. The schema changes check the
types they touch; they call the spec's `schemaChangeRefused` and `dupFields` on the abstraction,
once per change.

The refinement theorems say that the implementation's result, read through `ITenant.abs` and
`absW`, is the spec's result for the abstracted state, outcome for outcome, and that an accepted
step keeps the slot invariant (`ITenant.Inv`).
-/

import Strata.Impl.Problem
import Strata.Impl.SchemaI
import Strata.Impl.OpI

namespace Strata
namespace Impl

/-- The part of an implementation tenant that the permission tests read: its members, grants and
    policies, with no object types. The spec's decisions take a tenant and read no object type of it
    to decide, so they are taken on this header. -/
def ITenant.header (t : ITenant) : Tenant :=
  { types := [], members := t.members, grants := t.grants, policies := t.policies,
    nextId := t.nextId, limits := t.limits, roles := t.roles, teams := t.teams,
    invitations := t.invitations }

/-- May `p` attempt a create on type `ty` (the spec's `canAttemptCreate`, WG-5665). -/
def canAttemptCreateI (t : ITenant) (p : Principal) (ty : TypeId) : Bool :=
  canAttemptCreate t.header p ty

/-- May `p` create a record of type `ty` that would be `r` (the spec's `canCreate`). -/
def canCreateI (t : ITenant) (p : Principal) (ty : TypeId) (r : Record) : Bool :=
  canCreate t.header p ty r

/-- May `p` update record `old` of type `ty`, which would become `new` (the spec's `canUpdate`). -/
def canUpdateI (t : ITenant) (p : Principal) (ty : TypeId) (old new : Record) : Bool :=
  canUpdate t.header p ty old new

/-- May `p` delete record `old` of type `ty` (the spec's `canDelete`). -/
def canDeleteI (t : ITenant) (p : Principal) (ty : TypeId) (old : Record) : Bool :=
  canDelete t.header p ty old

/-- The refusal for an update or delete of a record that does not exist (the spec's
    `missingRecord`). -/
def missingRecordI (t : ITenant) (p : Principal) (ty : TypeId) (a : Action) : Err :=
  missingRecord t.header p ty a

/-- The refusal for an intent that names an object type the tenant does not have (the spec's
    `unknownType`). -/
def unknownTypeI (t : ITenant) (p : Principal) (ty : TypeId) (a : Action) : Err :=
  unknownType t.header p ty a

/-- Some record of any type refers to record `id` of type `ty` through a field that forbids its
    deletion (the spec's `blockedDelete`): a scan of every slot. -/
def blockedI (t : ITenant) (ty : TypeId) (id : RecordId) : Bool :=
  t.types.any (fun e => anyFrom e.2.slots (blocks e.1 e.2.schema ty id) e.2.slots.size)

/-- Whether a delete of `id` of type `ty` would leave a record it rewrites violating a constraint
    (the spec's `Tenant.clearBreaks`, WG-5664), on the slots: some other record that refers to it
    through a clearing field fails a constraint of its type once cleared. -/
def clearBreaksI (t : ITenant) (ty : TypeId) (id : RecordId) : Bool :=
  t.types.any (fun e => anyFrom e.2.slots (fun q => !(e.1 == ty && q.1 == id) &&
    recRefers e.2.schema ty id q.2 &&
    (constraintProblem (storedEnv (clearRef e.2.schema ty id q.2)) e.2.constraints).isSome)
    e.2.slots.size)

/-- The object type `j` after record `id` of type `ty` is deleted (the spec's `delObj`): in its own
    table the slot is cleared, and in every table the references to it are cleared. -/
def delObjI (ty : TypeId) (id : RecordId) (j : TypeId) (o : IObj) : IObj :=
  let cleared := o.slots.map (Option.map (clearRef o.schema ty id))
  { o with slots := if j == ty then setSlot cleared id none else cleared }

/-- One intent against one tenant, on slots. -/
def stepTenantI (t : ITenant) (p : Principal) : Intent → Except Err ITenant
  | .create ty fs =>
    match t.typ? ty with
    | none => .error (unknownTypeI t p ty .create)
    | some o =>
      if !canAttemptCreateI t p ty then .error .unauthorized
      else
      match newRecord o.schema p fs with
      | .error _ => .error .evaluation
      | .ok r =>
        if !canCreateI t p ty r then .error .unauthorized
        else if !initialsOk o.workflows r then .error .workflowViolation
        else
          match problemI t.limits o.schema o.uniques true t.slotsOf o.slots t.nextId r with
          | some e => .error e
          | none =>
            match constraintProblem (writeEnv p r none) o.constraints with
            | some e => .error e
            | none =>
              let o' := { o with slots := (padTo o.slots t.nextId).push (some r) }
              .ok { t.putType ty o' with nextId := t.nextId + 1 }
  | .update ty id fs ev =>
    match t.typ? ty with
    | none => .error (unknownTypeI t p ty .update)
    | some o =>
      match getRec o.slots id with
      | none => .error (missingRecordI t p ty .update)
      | some old =>
        let r := patched old fs
        if !canUpdateI t p ty old r then .error .unauthorized
        else if versionClash ev old then .error .versionConflict
        else
          match fireAll p old r o.workflows with
          | .error e => .error e
          | .ok r' =>
            if !(sameFields r.fields r'.fields || canUpdateI t p ty old r') then .error .unauthorized
            else
              match problemI t.limits o.schema o.uniques true t.slotsOf o.slots id r' with
              | some e => .error e
              | none =>
                if !statesOk o.workflows r' then .error .workflowViolation
                else
                  match constraintProblem (writeEnv p r' (some old)) o.constraints with
                  | some e => .error e
                  | none => .ok (t.putType ty { o with slots := setSlot o.slots id (some r') })
  | .delete ty id ev =>
    match t.typ? ty with
    | none => .error (unknownTypeI t p ty .delete)
    | some o =>
      match getRec o.slots id with
      | none => .error (missingRecordI t p ty .delete)
      | some old =>
        if !canDeleteI t p ty old then .error .unauthorized
        else if versionClash ev old then .error .versionConflict
        else if blockedI t ty id then .error .stillReferenced
        else if clearBreaksI t ty id then .error .constraintViolated
        else .ok { t with types := t.types.map (fun e => (e.1, delObjI ty id e.1 e.2)) }
  | .setRequired ty f strict =>
    if !canManage t.header p then .error .unauthorized
    else
      match t.typ? ty with
      | none => .error .invalidSchema
      | some o =>
        let t' := t.putType ty { o with schema := makeRequired (ensureField o.schema f) f }
        if schemaChangeRefused t.limits
            (scopedTypes t'.abs.types (ty :: fieldTargets (ensureField o.schema f) f))
            strict then .error .migrationRejected
        else .ok t'
  | .setUnique ty f strict =>
    if !canManage t.header p then .error .unauthorized
    else
      match t.typ? ty with
      | none => .error .invalidSchema
      | some o =>
        match (ensureField o.schema f).lookup f with
        | none => .error .invalidSchema
        | some d =>
          if d.ty.isList then .error .invalidSchema
          else
            let t' := t.putType ty { o with schema := makeUnique (ensureField o.schema f) f }
            if schemaChangeRefused t.limits
                (scopedTypes t'.abs.types (ty :: fieldTargets (ensureField o.schema f) f)) strict then
              .error .migrationRejected
            else .ok t'
  | .defineUnique ty fs =>
    if !canManage t.header p then .error .unauthorized
    else
      match t.typ? ty with
      | none => .error .invalidSchema
      | some o =>
        if !uniqueFieldsOk o.schema fs then .error .invalidSchema
        else if dupFields (absSlots o.slots) fs then .error .uniqueViolated
        else
          let t' := t.putType ty { o with uniques := o.uniques ++ [fs] }
          if schemaChangeRefused t.limits (scopedTypes t'.abs.types [ty]) true then
            .error .migrationRejected
          else .ok t'
  | .changeSchema ev strict cs =>
    changeSchemaI (schemaRulesOk t.policies) (canManage t.header p) t strict ev cs
  | .createTenant _ _ => .error .tenantExists
  | .member j => (stepMember t.mt (j.ctxOf p) j).map t.withM
  | .writeSetting n lvl team v ev =>
    (writeSettingT t.abs p n lvl team v ev).map (fun s =>
      { t with overrides := s.overrides, settingRevs := s.settingRevs })
  | .postLedger now ps => (postLedgerT t.abs p now ps).map (fun s => { t with ledger := s.ledger })
  | .postLedgerTx now tx => (postLedgerTxT t.abs p now tx).map (fun s => { t with ledger := s.ledger })
  | .expireLedger now u c => (expireLedgerT t.abs p now u c).map (fun s => { t with ledger := s.ledger })
  | .invokeOperation now op ins key =>
    (invokeOperationT t.abs p now op ins key).map (fun t' => rebuildI t t')

/-- The tenant a `CreateTenant` makes, as the implementation holds it: no object types, the genesis
    membership, a counter that starts at 1, and the revision and event sequence of its one event (slice 9). -/
def ITenant.genesis (owner : PrincipalId) : ITenant :=
  { types := [], members := [(owner, [ownerRole])], grants := [], policies := [], nextId := 1,
    revision := 1, eventSeq := 1 }

/-- The implementation's commit of an accepted step (the spec's `commitT`): the decision's tenant with its
    revision raised, its event sequence raised by the events the spec derives for the step, and its schema
    version raised by a schema change. The events are read through the abstraction, so the implementation
    emits the spec's events for the tenant it decided on (see the slice 9 review note). -/
def commitI (t t' : ITenant) (p : Principal) (i : Intent) : ITenant :=
  { t' with revision := t.revision + 1,
            eventSeq := t.eventSeq + (eventBodies t.abs p i t'.abs).length,
            schemaVersion := if i.changesSchema then t.schemaVersion + 1 else t.schemaVersion }

theorem commitI_abs (t t' : ITenant) (p : Principal) (i : Intent) :
    (commitI t t' p i).abs = commitT t.abs t'.abs p i := rfl

/-- The tenant a `CreateTenant` makes (the spec's `createdTenant`). -/
def createdTenantI (p : Principal) (owner : PrincipalId) (name : String) : Except Err ITenant :=
  if p != .system then .error .unauthorized
  else if isBlank name then .error .invalidIntent
  else .ok (ITenant.genesis owner)

/-- The implementation's transition function on the world: one intent, in one context. -/
def stepI (w : IWorld) (i : Intent) (c : Ctx) : Except Err IWorld :=
  if reservedRefused c.principal i then .error .invalidIntent
  else
  match w c.tenant with
  | none =>
    match i with
    | .createTenant owner name =>
      match createdTenantI c.principal owner name with
      | .error e => .error e
      | .ok t' => .ok (fun x => if x = c.tenant then some t' else w x)
    | _ => .error .noTenant
  | some t =>
    match stepTenantI t c.principal i with
    | .error e => .error e
    | .ok t' => .ok (fun x => if x = c.tenant then some t' else w x)

/-! ## Agreement of the restated functions -/

theorem canCreateI_eq (t : ITenant) (p : Principal) (ty : TypeId) (r : Record) :
    canCreateI t p ty r = canCreate t.abs p ty r := rfl

theorem canAttemptCreateI_eq (t : ITenant) (p : Principal) (ty : TypeId) :
    canAttemptCreateI t p ty = canAttemptCreate t.abs p ty := rfl

theorem canUpdateI_eq (t : ITenant) (p : Principal) (ty : TypeId) (old new : Record) :
    canUpdateI t p ty old new = canUpdate t.abs p ty old new := rfl

theorem canDeleteI_eq (t : ITenant) (p : Principal) (ty : TypeId) (old : Record) :
    canDeleteI t p ty old = canDelete t.abs p ty old := rfl

theorem canManage_header (t : ITenant) (p : Principal) :
    canManage t.header p = canManage t.abs p := rfl

theorem missingRecordI_eq (t : ITenant) (p : Principal) (ty : TypeId) (a : Action) :
    missingRecordI t p ty a = missingRecord t.abs p ty a := rfl

theorem unknownTypeI_eq (t : ITenant) (p : Principal) (ty : TypeId) (a : Action) :
    unknownTypeI t p ty a = unknownType t.abs p ty a := rfl

theorem problemI_abs (t : ITenant) (o : IObj) (self : RecordId) (r : Record) :
    problem t.limits o.schema o.uniques true t.abs.env (absSlots o.slots) self r =
      problemI t.limits o.schema o.uniques true t.slotsOf o.slots self r := by
  rw [env_abs]
  exact problem_eq _ _ _ _ t.slotsOf o.slots self r

theorem blocked_eq (t : ITenant) (ty : TypeId) (id : RecordId) :
    blockedI t ty id = t.abs.blockedDelete ty id := by
  unfold blockedI Tenant.blockedDelete ITenant.abs
  simp only [List.any_map]
  congr 1
  funext e
  simp only [Function.comp, absEntry, IObj.abs, any_absSlots]

theorem clearBreaks_eq (t : ITenant) (ty : TypeId) (id : RecordId) :
    clearBreaksI t ty id = t.abs.clearBreaks ty id := by
  unfold clearBreaksI Tenant.clearBreaks ITenant.abs
  simp only [List.any_map]
  congr 1
  funext e
  simp only [Function.comp, absEntry, IObj.abs, any_absSlots]

@[simp] theorem abs_members (t : ITenant) : t.abs.members = t.members := rfl
@[simp] theorem abs_grants (t : ITenant) : t.abs.grants = t.grants := rfl
@[simp] theorem abs_policies (t : ITenant) : t.abs.policies = t.policies := rfl
@[simp] theorem abs_limits (t : ITenant) : t.abs.limits = t.limits := rfl
@[simp] theorem abs_nextId (t : ITenant) : t.abs.nextId = t.nextId := rfl
@[simp] theorem abs_types (t : ITenant) : t.abs.types = t.types.map absEntry := rfl

theorem afterDelete_abs (t : ITenant) (ty : TypeId) (id : RecordId) :
    ({ t with types := t.types.map (fun (e : TypeId × IObj) => (e.1, delObjI ty id e.1 e.2)) }).abs =
      t.abs.afterDelete ty id := by
  unfold ITenant.abs Tenant.afterDelete
  simp only [List.map_map]
  congr 1
  apply List.map_congr_left
  intro e _
  simp only [Function.comp, absEntry, delObjI, IObj.abs, delObj]
  by_cases h : e.1 = ty
  · have hb : (e.1 == ty) = true := by simp [h]
    simp only [hb, if_true, setSlot, delClear, absSlots_clear]
  · have hb : (e.1 == ty) = false := by simp [h]
    simp only [hb, Bool.false_eq_true, if_false, delClear, absSlots_map]

/-- `Except.map` pushed into an `if`. -/
theorem except_map_ite (f : ITenant → Tenant) {c : Prop} [Decidable c] (a b : Except Err ITenant) :
    Except.map f (if c then a else b) = if c then Except.map f a else Except.map f b := by
  split <;> rfl

/-! ## Refinement of one tenant's step -/

/-- The implementation's result, read through the abstraction, is the spec's result on the
    abstracted state: the same refusal, or the abstraction of the accepted tenant. No type's slot
    array is longer than the counter (`hinv`), so a created record gets the spec's id. -/
theorem stepTenantI_refines (t : ITenant) (p : Principal) (i : Intent) (hinv : t.Inv) :
    (stepTenantI t p i).map ITenant.abs = stepTenant t.abs p i := by
  cases i with
  | createTenant owner name => simp [stepTenantI, stepTenant, Except.map]
  | changeSchema ev strict cs =>
    simp only [stepTenantI, stepTenant]
    exact changeSchemaI_refines _ t p strict ev cs _ (canManage_header t p)
  | member j =>
    simp only [stepTenantI, stepTenant, stepMemberT, ITenant.abs_mt]
    cases stepMember t.mt (j.ctxOf p) j with
    | error e => rfl
    | ok m => rfl
  | writeSetting n lvl team v ev =>
    simp only [stepTenantI, stepTenant]
    cases h : writeSettingT t.abs p n lvl team v ev with
    | error e => rfl
    | ok s =>
      obtain ⟨ov, rv, rfl⟩ := writeSettingT_ok_shape h
      rfl
  | postLedger now ps =>
    simp only [stepTenantI, stepTenant]
    cases h : postLedgerT t.abs p now ps with
    | error e => rfl
    | ok s =>
      obtain ⟨L, rfl⟩ := postLedgerT_ok_shape h
      rfl
  | postLedgerTx now tx =>
    simp only [stepTenantI, stepTenant]
    cases h : postLedgerTxT t.abs p now tx with
    | error e => rfl
    | ok s =>
      obtain ⟨L, rfl⟩ := postLedgerTxT_ok_shape h
      rfl
  | expireLedger now u c =>
    simp only [stepTenantI, stepTenant]
    cases h : expireLedgerT t.abs p now u c with
    | error e => rfl
    | ok s =>
      obtain ⟨L, rfl⟩ := expireLedgerT_ok_shape h
      rfl
  | invokeOperation now op ins key =>
    simp only [stepTenantI, stepTenant]
    cases h : invokeOperationT t.abs p now op ins key with
    | error e => rfl
    | ok s =>
      simp only [Except.map]
      rw [rebuildI_abs t s p now op ins key hinv h]
  | create ty fs =>
    cases hty : t.typ? ty with
    | none =>
      have hty' : t.abs.typ? ty = none := by rw [typ?_abs, hty]; rfl
      simp [stepTenantI, stepTenant, hty, hty', unknownTypeI_eq, Except.map]
    | some o =>
      have hty' : t.abs.typ? ty = some o.abs := by rw [typ?_abs, hty]; rfl
      have hw : o.abs.workflows = o.workflows := rfl
      have hsz := hinv ty o hty
      by_cases hA : (!canAttemptCreate t.abs p ty) = true
      · simp [stepTenantI, stepTenant, hty, hty', canAttemptCreateI_eq, hA, Except.map]
      have hA' : (!canAttemptCreate t.abs p ty) = false := by simpa using hA
      cases hr : newRecord o.schema p fs with
      | error u =>
        have hr' : newRecord o.abs.schema p fs = .error u := hr
        simp only [stepTenantI, stepTenant, hty, hty', hr, hr', canAttemptCreateI_eq, hA',
          if_false, Bool.false_eq_true]
        rfl
      | ok r =>
        have hr' : newRecord o.abs.schema p fs = .ok r := hr
        have hp : problem t.abs.limits o.abs.schema o.abs.uniques true t.abs.env o.abs.records
            t.abs.nextId r =
            problemI t.limits o.schema o.uniques true t.slotsOf o.slots t.nextId r :=
          problemI_abs t o t.nextId r
        simp only [stepTenantI, stepTenant, hty, hty', hr, hr', canCreateI_eq,
          canAttemptCreateI_eq, hA', if_false, Bool.false_eq_true]
        by_cases hc : (!canCreate t.abs p ty r) = true
        · simp [hc, Except.map]
        · simp only [hc, if_false, Bool.false_eq_true]
          by_cases hi : (!initialsOk o.workflows r) = true
          · simp [hi, hw, Except.map]
          · simp only [hi, hw, if_false, Bool.false_eq_true, hp]
            cases hq : problemI t.limits o.schema o.uniques true t.slotsOf o.slots t.nextId r with
            | some e => simp [Except.map]
            | none =>
              cases hk : constraintProblem (writeEnv p r none) o.constraints with
              | some e =>
                have hk' : constraintProblem (writeEnv p r none) o.abs.constraints = some e := hk
                simp [hk', Except.map]
              | none =>
                have hk' : constraintProblem (writeEnv p r none) o.abs.constraints = none := hk
                simp only [hk', Except.map]
                congr 1
                simp only [ITenant.putType, ITenant.abs, putTypeI_abs, Tenant.putType]
                congr 2
                simp only [IObj.abs, absSlots_push, size_padTo _ _ hsz, absSlots_padTo]
  | update ty id fs ev =>
    cases hty : t.typ? ty with
    | none =>
      have hty' : t.abs.typ? ty = none := by rw [typ?_abs, hty]; rfl
      simp [stepTenantI, stepTenant, hty, hty', unknownTypeI_eq, Except.map]
    | some o =>
      have hty' : t.abs.typ? ty = some o.abs := by rw [typ?_abs, hty]; rfl
      have hw : o.abs.workflows = o.workflows := rfl
      have hl : o.abs.records.lookup id = getRec o.slots id := lookup_absSlots o.slots id
      cases hg : getRec o.slots id with
      | none =>
        simp only [stepTenantI, stepTenant, hty, hty', hg, hl, missingRecordI_eq]
        rfl
      | some old =>
        have hslot := getRec_some o.slots id old hg
        have hpr : ∀ r', problem t.abs.limits o.abs.schema o.abs.uniques true t.abs.env o.abs.records
            id r' = problemI t.limits o.schema o.uniques true t.slotsOf o.slots id r' :=
          fun r' => problemI_abs t o id r'
        simp only [stepTenantI, stepTenant, hty, hty', hg, hl, canUpdateI_eq]
        by_cases hc : (!canUpdate t.abs p ty old (patched old fs)) = true
        · simp [hc, Except.map]
        · by_cases hv : versionClash ev old = true
          · simp [hc, hv, Except.map]
          · simp only [hc, hv, if_false, Bool.false_eq_true]
            cases hF : fireAll p old (patched old fs) o.workflows with
            | error e => simp [hF, hw, Except.map]
            | ok r' =>
              simp only [hF, hw, Bool.false_eq_true, if_false, canUpdateI_eq]
              by_cases hs : (!(sameFields (patched old fs).fields r'.fields ||
                  canUpdate t.abs p ty old r')) = true
              · simp [hs, Except.map]
              · simp only [hs, if_false, Bool.false_eq_true, hpr]
                cases hq : problemI t.limits o.schema o.uniques true t.slotsOf o.slots id r' with
                | some e => simp [Except.map]
                | none =>
                  by_cases hsOk : (!statesOk o.workflows r') = true
                  · simp [hsOk, Except.map]
                  · simp only [hsOk, if_false, Bool.false_eq_true]
                    cases hk : constraintProblem (writeEnv p r' (some old)) o.constraints with
                    | some e =>
                      have hk' : constraintProblem (writeEnv p r' (some old)) o.abs.constraints
                          = some e := hk
                      simp [hk', Except.map]
                    | none =>
                      have hk' : constraintProblem (writeEnv p r' (some old)) o.abs.constraints
                          = none := hk
                      simp only [hk', Except.map]
                      congr 1
                      simp only [ITenant.putType, ITenant.abs, putTypeI_abs, Tenant.putType]
                      congr 2
                      simp only [IObj.abs, setSlot, absSlots_setIfInBounds _ _ _ _ hslot]
  | delete ty id ev =>
    cases hty : t.typ? ty with
    | none =>
      have hty' : t.abs.typ? ty = none := by rw [typ?_abs, hty]; rfl
      simp [stepTenantI, stepTenant, hty, hty', unknownTypeI_eq, Except.map]
    | some o =>
      have hty' : t.abs.typ? ty = some o.abs := by rw [typ?_abs, hty]; rfl
      have hl : o.abs.records.lookup id = getRec o.slots id := lookup_absSlots o.slots id
      cases hg : getRec o.slots id with
      | none =>
        simp only [stepTenantI, stepTenant, hty, hty', hg, hl, missingRecordI_eq]
        rfl
      | some old =>
        simp only [stepTenantI, stepTenant, hty, hty', hg, hl, canDeleteI_eq, blocked_eq]
        by_cases hc : (!canDelete t.abs p ty old) = true
        · simp [hc, Except.map]
        · by_cases hv : versionClash ev old = true
          · simp [hc, hv, Except.map]
          · by_cases hb : t.abs.blockedDelete ty id = true
            · simp [hc, hv, hb, Except.map]
            · by_cases hcb : t.abs.clearBreaks ty id = true
              · simp [clearBreaks_eq, hc, hv, hb, hcb, Except.map]
              · simp only [clearBreaks_eq, hc, hv, hb, hcb, if_false, Bool.false_eq_true, Except.map]
                congr 1
                exact afterDelete_abs t ty id
  | setRequired ty f strict =>
    by_cases ho : (!canManage t.header p) = true
    · have ho2 : canManage t.abs p = false := by rw [← canManage_header]; simpa using ho
      simp [stepTenantI, stepTenant, canManage_header, ho2, Except.map]
    · have ho' : (!canManage t.abs p) = false := by rw [← canManage_header]; simpa using ho
      cases hty : t.typ? ty with
      | none =>
        have hty' : t.abs.typ? ty = none := by rw [typ?_abs, hty]; rfl
        simp [stepTenantI, stepTenant, hty, hty', ho, ho', canManage_header, Except.map]
      | some o =>
        have hty' : t.abs.typ? ty = some o.abs := by rw [typ?_abs, hty]; rfl
        have hpt := putType_abs t ty { o with schema := makeRequired (ensureField o.schema f) f }
        simp only [stepTenantI, stepTenant, hty, hty', ho, ho', canManage_header, if_false,
          Bool.false_eq_true]
        simp only [hpt, IObj.abs_schema, IObj.abs, abs_members, abs_grants, abs_policies,
          abs_limits, ho, ho', if_false, Bool.false_eq_true]
        split <;> simp_all [Except.map] <;> rfl
  | setUnique ty f strict =>
    by_cases ho : (!canManage t.header p) = true
    · have ho2 : canManage t.abs p = false := by rw [← canManage_header]; simpa using ho
      simp [stepTenantI, stepTenant, canManage_header, ho2, Except.map]
    · have ho' : (!canManage t.abs p) = false := by rw [← canManage_header]; simpa using ho
      cases hty : t.typ? ty with
      | none =>
        have hty' : t.abs.typ? ty = none := by rw [typ?_abs, hty]; rfl
        simp [stepTenantI, stepTenant, hty, hty', ho, ho', canManage_header, Except.map]
      | some o =>
        have hty' : t.abs.typ? ty = some o.abs := by rw [typ?_abs, hty]; rfl
        have hpt := putType_abs t ty { o with schema := makeUnique (ensureField o.schema f) f }
        have hl : (ensureField o.abs.schema f).lookup f = (ensureField o.schema f).lookup f := rfl
        simp only [stepTenantI, stepTenant, hty, hty', ho, ho', canManage_header, if_false,
          Bool.false_eq_true, hl]
        cases (ensureField o.schema f).lookup f with
        | none => simp [Except.map]
        | some d =>
          simp only []
          by_cases hd : d.ty.isList = true
          · simp [hd, Except.map]
          · simp only [hd, if_false, Bool.false_eq_true]
            simp only [hpt, IObj.abs_schema, IObj.abs, abs_members, abs_grants, abs_policies,
              abs_limits]
            split <;> simp_all [Except.map] <;> rfl
  | defineUnique ty fs =>
    by_cases ho : (!canManage t.header p) = true
    · have ho2 : canManage t.abs p = false := by rw [← canManage_header]; simpa using ho
      simp [stepTenantI, stepTenant, canManage_header, ho2, Except.map]
    · have ho' : (!canManage t.abs p) = false := by rw [← canManage_header]; simpa using ho
      cases hty : t.typ? ty with
      | none =>
        have hty' : t.abs.typ? ty = none := by rw [typ?_abs, hty]; rfl
        simp [stepTenantI, stepTenant, hty, hty', ho, ho', canManage_header, Except.map]
      | some o =>
        have hty' : t.abs.typ? ty = some o.abs := by rw [typ?_abs, hty]; rfl
        have hpt := putType_abs t ty { o with uniques := o.uniques ++ [fs] }
        simp only [stepTenantI, stepTenant, hty, hty', ho, ho', canManage_header, if_false,
          Bool.false_eq_true]
        by_cases hu : (!uniqueFieldsOk o.schema fs) = true
        · simp [hu, Except.map, IObj.abs]
        · by_cases hd : dupFields (absSlots o.slots) fs = true
          · simp [hu, hd, Except.map, IObj.abs]
          · simp only [hu, hd, if_false, Bool.false_eq_true, IObj.abs]
            simp only [hpt, IObj.abs_uniques, IObj.abs, abs_limits]
            split <;> simp_all [Except.map] <;> rfl

end Impl
end Strata
