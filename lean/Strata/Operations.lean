/-
The abstract model of invoking an operation (slice 14): the decision of `InvokeOperation` in the kernel's check
order (`crates/kernel/src/operations.rs`, `invoke`), and its steps run in order as one atomic decision.

The steps change records and the ledger, and their events are the record events and ledger events of each change.
The decision returns them as `OpLog` entries; `Model.lean` turns them into the domain events of the kernel
(`opLogBody`), since `EventBody` is defined there.

A create step decides its record as a direct create does (`newRecord`, the initial workflow states, `problem`, the
constraints), with the invoker as creator. An update decides as a direct update does, but takes the workflow
transitions without their guards, the kernel's rule for an operation (`opFireOne`). A delete refuses as a direct delete
does, with the constraints of the rewritten records checked (WG-5664). A posting is a ledger posting of the invoker,
decided on the state the steps before it leave.

Not modelled here: the self-action refusal of a subject (`ADM-1613`), the find and branch steps, the ledger reversals
and transactions as steps, the owner, scope and reference expression forms of a posting, and the idempotency store of
the runtime, which answers a repeat of a key with the first result. The idempotency the model states is the ledger's:
each posting's key is `op:<operation>:<invoker>:<key>:<step>` (`postingKey`), so a repeat of the same invocation posts
each entry again as a replay.
-/

import Strata.LedgerModel
import Strata.Reads

namespace Strata

/-- The validity of an idempotency key (the kernel's `valid_key`): one to 128 visible ASCII characters. -/
def validKey (k : String) : Bool :=
  !k.isEmpty && decide (k.utf8ByteSize ≤ 128) &&
    k.toList.all (fun c => decide (33 ≤ c.toNat) && decide (c.toNat ≤ 126))

/-- The ledger key of the posting a step makes (the kernel's `posting_key`): the operation, the invoker, the key of
    the invocation and the step. -/
def postingKey (op : String) (invoker : PrincipalId) (key step : String) : String :=
  "op:" ++ op ++ ":p" ++ toString invoker ++ ":" ++ key ++ ":" ++ step

/-- What an accepted step changed, in order, as the domain events of the kernel's `Output` for it: a created or
    updated or deleted record (a delete first names the records it rewrites), and an entry posted. -/
inductive OpLog where
  | created (ty : TypeId) (id : RecordId) (r : Record)
  | updated (ty : TypeId) (id : RecordId) (before after : Record)
  | deleted (ty : TypeId) (id : RecordId) (before : Record)
  | posted (e : LedgerPosted)

/-- The state of one invocation in progress: the working copy of the tenant, the parameters (the inputs, then the
    record of each create step by its id; a later binding of a name wins), and the changes so far, in order. -/
structure OpState where
  tenant : Tenant
  params : List (String × Val)
  logs : List OpLog

/-- What the steps of one invocation read: the invoker (the creator and the posting owner), the principal, the
    time, the operation and its key, and the subject record the invocation is about. -/
structure OpCtx where
  invoker : PrincipalId
  principal : Principal
  now : Int
  op : String
  key : String
  subject : Option Record

/-- The environment of an expression of the operation: the subject as the record, the parameters, the invoker. -/
def OpCtx.env (c : OpCtx) (ps : List (String × Val)) : ExprEnv :=
  { record := c.subject, old := c.subject, new := c.subject, principal := some c.invoker, params := ps }

/-- Evaluate an expression of a step. Any failure is the kernel's `Evaluation`. -/
def opEval (c : OpCtx) (ps : List (String × Val)) (e : Expr) : Except Err Val :=
  match evalExpr (c.env ps) e with
  | .ok v => .ok v
  | .error _ => .error .evaluation

/-- The record a step names: a reference, or else `invalidIntent` (the kernel's `target`). -/
def opTarget (c : OpCtx) (ps : List (String × Val)) (e : Expr) : Except Err RecordId :=
  match opEval c ps e with
  | .ok (.ref id) => .ok id
  | .ok _ => .error .invalidIntent
  | .error err => .error err

/-- The referrer rewrites of a delete, as the kernel makes them: each record that refers to the deleted one, in the
    order of its object type and then of its id, rewritten as a direct delete clears it. -/
def opReferrerLogs (t : Tenant) (ty : TypeId) (id : RecordId) : List OpLog :=
  ((t.types.flatMap (fun e => e.2.records.filterMap (fun q =>
      if !(e.1 == ty && q.1 == id) && recRefers e.2.schema ty id q.2 then
        some (e.1, q.1, q.2, clearRef e.2.schema ty id q.2)
      else none))).mergeSort
    (fun a b => decide (a.1 < b.1 ∨ (a.1 = b.1 ∧ a.2.1 ≤ b.2.1)))).map
    (fun e => OpLog.updated e.1 e.2.1 e.2.2.1 e.2.2.2)

/-- One workflow of an updated record, as the kernel's operation takes it: when its field keeps its state nothing is
    done; otherwise a transition from the stored state to the new one must exist, whatever its guard, and its
    assignments are applied to the candidate. The guard is not asked of the invoker (D-6547). -/
def opFireOne (p : Principal) (old cand : Record) (w : Workflow) : Except Err Record :=
  let was := old.fields.get w.field
  let becomes := cand.fields.get w.field
  if was == becomes then .ok cand
  else
    let env : ExprEnv := { record := some old, old := some old, new := some cand, principal := p.userId? }
    match w.transitions.find? (fun tr => was == .str tr.source && becomes == .str tr.target) with
    | none => .error .workflowViolation
    | some tr => match evalSets env tr.set with
      | .error _ => .error .evaluation
      | .ok as => .ok (applySets cand as)

/-- Every workflow of the type of an updated record, in order (the kernel's loop over the workflows of the type). -/
def opFireAll (p : Principal) (old cand : Record) : List Workflow → Except Err Record
  | [] => .ok cand
  | w :: ws => match opFireOne p old cand w with
    | .error e => .error e
    | .ok c => opFireAll p old c ws

/-- A create step (the kernel's `create`): the named fields evaluated, the defaults of the others (`newRecord`, with
    the invoker as the principal), the initial workflow states, then the record's checks as a direct create's, at the
    next id the tenant's counter gives. The step's id then names the new record as a parameter. -/
def opCreate (c : OpCtx) (st : OpState) (id : String) (ty : TypeId) (fields : List (FieldId × Expr)) :
    Except Err OpState :=
  match st.tenant.typ? ty with
  | none => .error .unknownObject
  | some o =>
    match fields.mapM (fun q => (fun v => (q.1, v)) <$> opEval c st.params q.2) with
    | .error e => .error e
    | .ok vals =>
      match newRecord o.schema c.principal vals with
      | .error _ => .error .evaluation
      | .ok r =>
        if !initialsOk o.workflows r then .error .workflowViolation
        else match problem st.tenant.limits o.schema o.uniques true st.tenant.env o.records st.tenant.nextId r with
          | some e => .error e
          | none =>
            match constraintProblem (writeEnv c.principal r none) o.constraints with
            | some e => .error e
            | none =>
              let o' := { o with records := (st.tenant.nextId, r) :: o.records }
              .ok { tenant := { st.tenant.putType ty o' with nextId := st.tenant.nextId + 1 },
                    params := (id, Val.ref st.tenant.nextId) :: st.params,
                    logs := st.logs ++ [OpLog.created ty st.tenant.nextId r] }

/-- A patch of the record a step names (the kernel's `update`): each named field takes its value, `null` removes it,
    the version is raised, the workflow transitions are taken, and the record's checks are as a direct update's. -/
def opUpdate (c : OpCtx) (st : OpState) (ty : TypeId) (record : Expr) (fields : List (FieldId × Expr)) :
    Except Err OpState :=
  match st.tenant.typ? ty with
  | none => .error .unknownObject
  | some o =>
    match opTarget c st.params record with
    | .error e => .error e
    | .ok id =>
      match o.records.lookup id with
      | none => .error .notFound
      | some old =>
        match fields.mapM (fun q => (fun v => (q.1, v)) <$> opEval c st.params q.2) with
        | .error e => .error e
        | .ok vals =>
          let r0 : Record := { old with fields := Fields.patch old.fields vals, version := old.version + 1 }
          match opFireAll c.principal old r0 o.workflows with
          | .error e => .error e
          | .ok r =>
            if !statesOk o.workflows r then .error .workflowViolation
            else match problem st.tenant.limits o.schema o.uniques true st.tenant.env o.records id r with
              | some e => .error e
              | none =>
                match constraintProblem (writeEnv c.principal r (some old)) o.constraints with
                | some e => .error e
                | none =>
                  let rs' := o.records.map (fun q => if q.1 == id then (q.1, r) else q)
                  .ok { st with tenant := st.tenant.putType ty { o with records := rs' },
                                logs := st.logs ++ [OpLog.updated ty id old r] }

/-- A delete of the record a step names (the kernel's `delete`): refused while a restricting or required reference to
    it stands, and while a rewrite of a referrer would leave one of its constraints false or unevaluable (WG-5664, as
    a direct delete is); otherwise the records that refer to it are rewritten, then it is removed. -/
def opDelete (c : OpCtx) (st : OpState) (ty : TypeId) (record : Expr) : Except Err OpState :=
  match st.tenant.typ? ty with
  | none => .error .unknownObject
  | some _ =>
    match opTarget c st.params record with
    | .error e => .error e
    | .ok id =>
      match st.tenant.recordAt ty id with
      | none => .error .notFound
      | some old =>
        if st.tenant.blockedDelete ty id then .error .stillReferenced
        else if st.tenant.clearBreaks ty id then .error .constraintViolated
        else .ok { tenant := st.tenant.afterDelete ty id, params := st.params,
                   logs := st.logs ++ opReferrerLogs st.tenant ty id ++ [OpLog.deleted ty id old] }

/-- A posting step (the kernel's `post`): the owner is the invoker or a pool, the amount is an integer, and the posting
    is decided as a ledger posting by the invoker, on the state the steps before it leave. Its key is the operation's
    posting key for this step, so a repeat of the invocation is a replay of the posting. -/
def opPost (c : OpCtx) (st : OpState) (id : String) (owner : OpOwner) (unit : Nat) (amount : Expr)
    (reason : String) : Except Err OpState :=
  match opEval c st.params amount with
  | .error e => .error e
  | .ok (.int n) =>
    let lo : LedgerOwner := match owner with
      | .invoker => .person c.invoker
      | .pool k => .pool k
    let q : LedgerPosting := { owner := lo, unit := unit, amount := n,
                               key := postingKey c.op c.invoker c.key id, reason := reason }
    match postOne st.tenant c.principal c.now st.tenant.ledger q with
    | .error e => .error e
    | .ok (L, evs) =>
      .ok { tenant := { st.tenant with ledger := L }, params := st.params,
            logs := st.logs ++ evs.map OpLog.posted }
  | .ok _ => .error .invalidIntent

/-- One step of an operation on the state the steps before it leave. -/
def opStep (c : OpCtx) (st : OpState) : OpStep → Except Err OpState
  | .create id ty fs => opCreate c st id ty fs
  | .update _ ty rec fs => opUpdate c st ty rec fs
  | .delete _ ty rec => opDelete c st ty rec
  | .post id owner unit amt reason => opPost c st id owner unit amt reason

/-- The steps in order, each on the state the earlier ones leave; the first refusal refuses the invocation. -/
def runSteps (c : OpCtx) : List OpStep → OpState → Except Err OpState
  | [], st => .ok st
  | s :: ss, st => match opStep c st s with
    | .error e => .error e
    | .ok st' => runSteps c ss st'

/-- The value of an input by name, `null` when it is not given. -/
def inputOf (inputs : List (String × Val)) (name : String) : Val :=
  ((inputs.find? (fun q => q.1 == name)).map (fun q => q.2)).getD .null

/-- One input bound: `null` when it is not given and optional, refused when required; a value over a size bound is
    `limitExceeded`; a value of the wrong type is `invalid` (the kernel's `MissingRequired`, `TypeMismatch`). -/
def bindInput (t : Tenant) (inputs : List (String × Val)) (i : OpInput) : Except Err (String × Val) :=
  let v := inputOf inputs i.name
  if v == .null then
    if i.required then .error .invalid else .ok (i.name, .null)
  else if Val.overLimit t.limits v then .error .limitExceeded
  else if !v.hasType i.ty then .error .invalid
  else .ok (i.name, v)

/-- The inputs bound in declared order, the first refusal refusing the invocation. -/
def bindInputs (t : Tenant) (inputs : List (String × Val)) :
    List OpInput → Except Err (List (String × Val))
  | [] => .ok []
  | i :: is => match bindInput t inputs i with
    | .error e => .error e
    | .ok b => match bindInputs t inputs is with
      | .error e => .error e
      | .ok bs => .ok (b :: bs)

/-- The subject of an invocation: the record the input named by the operation's subject holds, when the principal may
    read it. A reference to a record that does not exist, or that the principal may not read, is `notFound` (the
    kernel's `UnknownRecord`, which hides which of the two it is). -/
def opSubject (t : Tenant) (p : Principal) (op : OpDef) (ps : List (String × Val)) :
    Except Err (Option Record) :=
  match op.subject with
  | none => .ok none
  | some (ty, name) => match inputOf ps name with
    | .ref id => match t.recordAt ty id with
      | some r => if t.canRead p ty r then .ok (some r) else .error .notFound
      | none => .error .notFound
    | _ => .error .notFound

/-- The conditions, in order, on the state before any step: a condition that is `false` or `null` refuses the
    invocation (`constraintViolated`), and one that cannot be evaluated is `evaluation`. -/
def opConditions (c : OpCtx) (ps : List (String × Val)) : List OpCond → Except Err Unit
  | [] => .ok ()
  | k :: ks => match evalExpr (c.env ps) k.expr with
    | .ok (.bool true) => opConditions c ps ks
    | .ok (.bool false) => .error .constraintViolated
    | .ok .null => .error .constraintViolated
    | .ok _ => .error .evaluation
    | .error _ => .error .evaluation

/-- An invocation of operation `name` by `p` at `now`, with these inputs and this idempotency key, against tenant `t`
    (the kernel's `invoke`, in its check order). Returns the tenant after the invocation and the changes of its steps
    in order. A refusal of any check, or of any step, refuses the whole invocation and changes nothing. -/
def invokeOpCore (t : Tenant) (p : Principal) (now : Int) (name : String) (inputs : List (String × Val))
    (key : String) : Except Err (Tenant × List OpLog) :=
  match t.operations.lookup name with
  | none => .error .invalidIntent
  | some op =>
    match p with
    | .user u =>
      if !op.signedIn && op.roles.all (fun r => !((t.rolesOf p).contains r)) then .error .unauthorized
      else if !validKey key then .error .invalidIntent
      else if !(inputs.all (fun q => op.inputs.any (fun i => i.name == q.1))) then .error .invalid
      else match bindInputs t inputs op.inputs with
        | .error e => .error e
        | .ok ps => match opSubject t p op ps with
          | .error e => .error e
          | .ok subj =>
            let c : OpCtx := { invoker := u, principal := p, now := now, op := name, key := key, subject := subj }
            match opConditions c ps op.conditions with
            | .error e => .error e
            | .ok _ => match runSteps c op.steps { tenant := t, params := ps, logs := [] } with
              | .error e => .error e
              | .ok st => .ok (st.tenant, st.logs)
    | _ => .error .unauthorized

/-- The tenant after an accepted invocation. -/
def invokeOperationT (t : Tenant) (p : Principal) (now : Int) (name : String) (inputs : List (String × Val))
    (key : String) : Except Err Tenant :=
  (invokeOpCore t p now name inputs key).map (fun r => r.1)

/-- The changes of an invocation, read from the tenant it leaves (empty when it is refused). -/
def invokeOpLogs (t : Tenant) (p : Principal) (now : Int) (name : String) (inputs : List (String × Val))
    (key : String) : List OpLog :=
  match invokeOpCore t p now name inputs key with
  | .ok r => r.2
  | .error _ => []

/-! ## Composition: what the steps of an invocation preserve -/

/-- A property of the tenant that every step preserves is preserved by the steps in order. -/
theorem runSteps_pres (P : Tenant -> Prop) (c : OpCtx)
    (hstep : ∀ s st st', opStep c st s = .ok st' -> P st.tenant -> P st'.tenant) :
    ∀ (ss : List OpStep) (st st' : OpState), runSteps c ss st = .ok st' -> P st.tenant -> P st'.tenant
  | [], st, st', h, hp => by
    simp only [runSteps, Except.ok.injEq] at h
    subst h
    exact hp
  | s :: ss, st, st', h, hp => by
    simp only [runSteps] at h
    cases hs : opStep c st s with
    | error e => simp [hs] at h
    | ok st1 =>
      simp only [hs] at h
      exact runSteps_pres P c hstep ss st1 st' h (hstep s st st1 hs hp)

/-- The steps of an invocation are applied in order: running `xs` then `ys` is running `xs ++ ys`. -/
theorem runSteps_append (c : OpCtx) (xs ys : List OpStep) (st : OpState) :
    runSteps c (xs ++ ys) st = (runSteps c xs st).bind (fun st1 => runSteps c ys st1) := by
  induction xs generalizing st with
  | nil => rfl
  | cons s xs ih =>
    simp only [List.cons_append, runSteps]
    cases hs : opStep c st s with
    | error e => rfl
    | ok st1 => simp only [hs]; exact ih st1

/-- A refused step refuses the run of the steps it belongs to, whatever follows it (the atomicity of an invocation). -/
theorem runSteps_refused_of_step (c : OpCtx) (s : OpStep) (ss : List OpStep) (st : OpState) (e : Err)
    (h : opStep c st s = .error e) : runSteps c (s :: ss) st = .error e := by
  simp [runSteps, h]

/-- An accepted invocation ran its operation's steps on the tenant, in order, and leaves the tenant they leave. -/
theorem invokeOpCore_ok {t t' : Tenant} {p : Principal} {now : Int} {name : String}
    {ins : List (String × Val)} {key : String} {logs : List OpLog}
    (h : invokeOpCore t p now name ins key = .ok (t', logs)) :
    ∃ op ps c st, t.operations.lookup name = some op ∧
      runSteps c op.steps { tenant := t, params := ps, logs := [] } = .ok st ∧ t' = st.tenant := by
  unfold invokeOpCore at h
  split at h
  · simp at h
  rename_i op hop
  split at h
  · rename_i u
    split at h
    · simp at h
    split at h
    · simp at h
    split at h
    · simp at h
    split at h
    · simp at h
    rename_i ps hb
    split at h
    · simp at h
    rename_i subj hs
    dsimp only at h
    split at h
    · simp at h
    split at h
    · simp at h
    rename_i st hr
    simp only [Except.ok.injEq, Prod.mk.injEq] at h
    obtain ⟨rfl, -⟩ := h
    exact ⟨op, ps, _, st, hop, hr, rfl⟩
  · simp at h

/-! ## What a step changes -/

/-- A step changes only the object types, the record counter and the ledger of the tenant; its membership and
    everything else are as they were. -/
theorem opStep_shape (c : OpCtx) (st st' : OpState) (s : OpStep) (h : opStep c st s = .ok st') :
    ∃ ty nid L, st'.tenant = { st.tenant with types := ty, nextId := nid, ledger := L } := by
  cases s <;> simp only [opStep, opCreate, opUpdate, opDelete, opPost] at h <;>
    repeat' split at h
  all_goals (try (simp at h; done))
  all_goals (simp only [Except.ok.injEq] at h; subst h; exact ⟨_, _, _, rfl⟩)

/-- The membership of the tenant is the same after a step. -/
theorem opStep_mt (c : OpCtx) (st st' : OpState) (s : OpStep) (h : opStep c st s = .ok st') :
    st'.tenant.mt = st.tenant.mt := by
  obtain ⟨ty, nid, L, hs⟩ := opStep_shape c st st' s h
  rw [hs]
  rfl

/-- A property of the tenant that every step preserves is preserved by an accepted invocation. -/
theorem invokeOperationT_pres (P : Tenant → Prop)
    (hstep : ∀ (c : OpCtx) (s : OpStep) (st st' : OpState), opStep c st s = .ok st' → P st.tenant → P st'.tenant)
    {t t' : Tenant} {p : Principal} {now : Int} {name : String} {ins : List (String × Val)} {key : String}
    (hP : P t) (h : invokeOperationT t p now name ins key = .ok t') : P t' := by
  unfold invokeOperationT at h
  cases hc : invokeOpCore t p now name ins key with
  | error e => simp [hc, Except.map] at h
  | ok r =>
    obtain ⟨t'', logs⟩ := r
    simp only [hc, Except.map, Except.ok.injEq] at h
    obtain ⟨op, ps, c, st, -, hr, ht⟩ := invokeOpCore_ok hc
    subst h
    rw [ht]
    exact runSteps_pres P c (hstep c) op.steps _ st hr hP

/-- An accepted invocation keeps the membership of the tenant: its steps change records, the counter and the ledger. -/
theorem invokeOperationT_mt {t t' : Tenant} {p : Principal} {now : Int} {name : String}
    {ins : List (String × Val)} {key : String} (h : invokeOperationT t p now name ins key = .ok t') :
    t'.mt = t.mt :=
  invokeOperationT_pres (fun x => x.mt = t.mt)
    (fun c s st st' hs hp => (opStep_mt c st st' s hs).trans hp) rfl h

end Strata
