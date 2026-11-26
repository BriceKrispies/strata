/-
The abstract model of the Strata kernel, part 2: intents and the transition function.

`Strata/ModelBase.lean` defines the state (values, schemas, records, tenants, permissions) and
`Strata/Members.lean` the decisions of the membership intents. This file is the model's `Intent` and its
one transition function `step`: the record and schema intents of the earlier slices, the membership
intents of slice 7 (`Intent.member`) and the creation of a tenant (`Intent.createTenant`). Everything
that decides an outcome, the executable kernels, the working set and the harness, uses this `step`.
-/

import Strata.Members

namespace Strata

inductive Intent where
  /-- Create a record of object type `ty`. A field of its schema the intent does not name is
      filled from its default. -/
  | create (ty : TypeId) (fields : Fields)
  /-- Patch a record of type `ty`: each named field takes the given value, a named `null` removes
      the field, fields not named are kept. `expected`, when given, must be the record's version. -/
  | update (ty : TypeId) (id : RecordId) (fields : Fields) (expected : Option Nat)
  /-- Delete a record of type `ty`. `expected`, when given, must be the record's version. -/
  | delete (ty : TypeId) (id : RecordId) (expected : Option Nat)
  /-- Make a field of type `ty` required. `strict` rejects the change if any record of the tenant
      would not conform; otherwise existing records are left as they are. -/
  | setRequired (ty : TypeId) (f : FieldId) (strict : Bool)
  /-- Make a field of type `ty` unique. `strict` rejects the change if existing records already
      agree on it; otherwise existing records are left as they are. -/
  | setUnique (ty : TypeId) (f : FieldId) (strict : Bool)
  /-- Define a composite unique constraint of type `ty` over these fields. Refused if existing
      records already agree on all of them. -/
  | defineUnique (ty : TypeId) (fs : List FieldId)
  /-- Create a tenant, owned by `owner`: only the system may, and only when the tenant does not exist
      (slice 7). The name is checked for being blank and not stored. -/
  | createTenant (owner : PrincipalId) (name : String)
  /-- A membership intent of an existing tenant: roles, teams, invitations (slice 7). -/
  | member (j : MemberIntent)
  deriving Repr

/-- The user an intent names, if it names one: the owner of a new tenant, the user a role is assigned
    to or a team gains. -/
def Intent.named : Intent → Option PrincipalId
  | .createTenant owner _ => some owner
  | .member j => j.named
  | _ => none

/-- The kernel's `refuse_reserved_principals`: a caller who is a user with a reserved id, or an intent
    that names a reserved user. It is asked before the tenant is looked at, whatever the intent. -/
def reservedRefused (p : Principal) (i : Intent) : Bool :=
  callerReserved p || i.named.any reservedId

/-- The explicit environment of a step: which tenant, and who is acting. -/
structure Ctx where
  tenant : TenantId
  principal : Principal
  deriving Repr

/-- One intent against one tenant. An expected version is checked after the permission, so only
    one who may act on the record learns that it is at another version. A record intent that
    names an object type the tenant does not have is refused with `unknownObject` for the owner and
    with `unauthorized` for everyone else (`unknownType`), before the record is looked up. A schema
    change that names such a type is refused with `invalidSchema` for the owner and with
    `unauthorized` for everyone else. -/
def stepTenant (t : Tenant) (p : Principal) : Intent → Except Err Tenant
  | .create ty fs =>
    match t.typ? ty with
    | none => .error (unknownType t p ty .create)
    | some o =>
      -- The permission comes before the defaults (WG-5665, the kernel's `create_record`): a caller
      -- whom nothing could allow to create is refused whether or not a default would fail. A
      -- caller who could goes on to the defaults, and then asks for the decision on the record.
      if !canAttemptCreate t p ty then .error .unauthorized
      else
      match newRecord o.schema p fs with
      | .error _ => .error .evaluation
      | .ok r =>
        if !canCreate t p ty r then .error .unauthorized
        else if !initialsOk o.workflows r then .error .workflowViolation
        else
          match problem t.limits o.schema o.uniques true t.env o.records t.nextId r with
          | some e => .error e
          | none =>
            match constraintProblem (writeEnv p r none) o.constraints with
            | some e => .error e
            | none =>
              let o' := { o with records := (t.nextId, r) :: o.records }
              .ok { t.putType ty o' with nextId := t.nextId + 1 }
  | .update ty id fs ev =>
    match t.typ? ty with
    | none => .error (unknownType t p ty .update)
    | some o =>
      match o.records.lookup id with
      | none => .error (missingRecord t p ty .update)
      | some old =>
        let r := patched old fs
        if !canUpdate t p ty old r then .error .unauthorized
        else if versionClash ev old then .error .versionConflict
        else
          -- The workflows (slice 8): a changed state takes a declared transition, whose assignments are
          -- written; a write that the assignments changed is authorised again, as the record it now is.
          match fireAll p old r o.workflows with
          | .error e => .error e
          | .ok r' =>
            if !(sameFields r.fields r'.fields || canUpdate t p ty old r') then .error .unauthorized
            else
              match problem t.limits o.schema o.uniques true t.env o.records id r' with
              | some e => .error e
              | none =>
                if !statesOk o.workflows r' then .error .workflowViolation
                else
                  match constraintProblem (writeEnv p r' (some old)) o.constraints with
                  | some e => .error e
                  | none =>
                    let rs' := o.records.map (fun q => if q.1 == id then (q.1, r') else q)
                    .ok (t.putType ty { o with records := rs' })
  | .delete ty id ev =>
    match t.typ? ty with
    | none => .error (unknownType t p ty .delete)
    | some o =>
      match o.records.lookup id with
      | none => .error (missingRecord t p ty .delete)
      | some old =>
        if !canDelete t p ty old then .error .unauthorized
        else if versionClash ev old then .error .versionConflict
        else if t.blockedDelete ty id then .error .stillReferenced
        else if t.clearBreaks ty id then .error .constraintViolated
        else .ok (t.afterDelete ty id)
  | .setRequired ty f strict =>
    if !canManage t p then .error .unauthorized
    else
      match t.typ? ty with
      | none => .error .invalidSchema
      | some o =>
        let t' := t.putType ty { o with schema := makeRequired o.schema f }
        if schemaChangeRefused t.limits (scopedTypes t'.types (ty :: fieldTargets o.schema f)) strict then .error .migrationRejected
        else .ok t'
  | .setUnique ty f strict =>
    if !canManage t p then .error .unauthorized
    else
      match t.typ? ty with
      | none => .error .invalidSchema
      | some o =>
        match o.schema.lookup f with
        | none => .error .invalidSchema
        | some d =>
          if d.ty.isList then .error .invalidSchema
          else
            let t' := t.putType ty { o with schema := makeUnique o.schema f }
            if schemaChangeRefused t.limits (scopedTypes t'.types (ty :: fieldTargets o.schema f)) strict then .error .migrationRejected
            else .ok t'
  | .defineUnique ty fs =>
    if !canManage t p then .error .unauthorized
    else
      match t.typ? ty with
      | none => .error .invalidSchema
      | some o =>
        if !uniqueFieldsOk o.schema fs then .error .invalidSchema
        else if dupFields o.records fs then .error .uniqueViolated
        else
          let t' := t.putType ty { o with uniques := o.uniques ++ [fs] }
          if schemaChangeRefused t.limits (scopedTypes t'.types [ty]) true then .error .migrationRejected
          else .ok t'
  -- The tenant exists, so a creation is refused; a membership intent is the layer's decision.
  | .createTenant _ _ => .error .tenantExists
  | .member j => stepMemberT t p j
/-- Whether the acting principal is allowed to make this intent at all: the object type exists,
    and the permission holds on the record (or, for a schema change, the owner). -/
def authorized (t : Tenant) (p : Principal) : Intent → Bool
  | .create ty fs => match t.typ? ty with
    | none => false
    | some o => match newRecord o.schema p fs with
      | .ok r => canCreate t p ty r
      | .error _ => false
  | .update ty id fs _ => match t.recordAt ty id with
    | some old => canUpdate t p ty old (patched old fs)
    | none => false
  | .delete ty id _ => match t.recordAt ty id with
    | some old => canDelete t p ty old
    | none => false
  | .setRequired ty _ _ => canManage t p && (t.typ? ty).isSome
  | .setUnique ty _ _ => canManage t p && (t.typ? ty).isSome
  | .defineUnique ty _ => canManage t p && (t.typ? ty).isSome
  -- A creation is never accepted by a tenant that exists.
  | .createTenant _ _ => false
  -- Redeeming asks for a signed-in caller; every other membership intent, the permission to manage
  -- members.
  | .member (.redeemInvitation _ _ _) => p.userId?.isSome
  | .member _ => manageMembers t.mt p

/-- The object types a schema intent re-validates, as the kernel's `touched_objects` gives them: the
    type it names, and, for a field, the types the field refers to. Other intents touch none. -/
def Intent.touched (t : Tenant) : Intent → List TypeId
  | .setRequired ty f _ => ty :: (match t.typ? ty with
      | some o => fieldTargets o.schema f
      | none => [])
  | .setUnique ty f _ => ty :: (match t.typ? ty with
      | some o => fieldTargets o.schema f
      | none => [])
  | .defineUnique ty _ => [ty]
  | _ => []

/-- A schema change that leaves existing records as they are is the one kind of intent that is
    allowed to break record conformance or uniqueness. Every other intent is strict. -/
def Intent.strict : Intent → Bool
  | .setRequired _ _ s => s
  | .setUnique _ _ s => s
  | _ => true

/-- The intents that change the schema and leave the records as they are. -/
def Intent.isSchema : Intent → Bool
  | .setRequired _ _ _ => true
  | .setUnique _ _ _ => true
  | .defineUnique _ _ => true
  | _ => false

/-! ## Events, revisions and effects (slice 9)

An accepted step emits the domain events of the kernel's transition, in order, raises the tenant's revision
by one and its event sequence by the number of events, and asks for one `Publish` effect per event. The
events are derived from the tenant before the step, the principal, the intent, and the tenant the decision
wrote: each derivation is the kernel's emission for that intent (`transition::create_record`,
`update_record`, `delete_record`, `change_schema`, `create_tenant`). `step` keeps its type; `transition`
returns the events and effects alongside the world. -/

/-- A summary line of a schema change, by ids: the kernel's `apply_change` line for a field definition
    (`redefine field <object>.<field>`) or for a composite constraint (`define unique constraint <object>`). -/
inductive SchemaLine where
  | field (ty : TypeId) (f : FieldId)
  | unique (ty : TypeId) (fs : List FieldId)
  deriving DecidableEq, Repr

/-- The body of a domain event (the kernel's `EventBody`). A record event names the record as stored before
    and after; an update names the workflow transitions it fired, by workflow id, and a delete names the
    record it removed. -/
inductive EventBody where
  | tenantCreated (name : String)
  | schemaChanged (version : Nat) (summary : List SchemaLine) (grandfathered : Nat)
  | recordCreated (ty : TypeId) (id : RecordId) (r : Record)
  | recordUpdated (ty : TypeId) (id : RecordId) (before after : Record) (fired : List (String × String))
  | recordDeleted (ty : TypeId) (id : RecordId) (before : Record)
  deriving Repr

/-- A domain event: its sequence number (gapless per tenant), the revision it was committed at, the principal
    that caused it, and its body. The kernel's time and correlation are not modelled. -/
structure DomainEvent where
  seq : Nat
  revision : Nat
  principal : Principal
  body : EventBody
  deriving Repr

/-- An effect of an accepted transition (the kernel's `Effect`). Only `Publish` is modelled: slice 15 adds the
    automation effects, and the session revocation is not modelled. -/
inductive Effect where
  | publish (seq : Nat)
  deriving DecidableEq, Repr

/-- The records of the tenant that refer to record `id` of object type `ty` through a field, each with its
    stored form and the form the delete rewrites it to. The deleted record does not refer to itself. -/
def referrerEntries (t : Tenant) (ty : TypeId) (id : RecordId) :
    List (TypeId × RecordId × Record × Record) :=
  t.types.flatMap (fun e => e.2.records.filterMap (fun q =>
    if !(e.1 == ty && q.1 == id) && recRefers e.2.schema ty id q.2 then
      some (e.1, q.1, q.2, clearRef e.2.schema ty id q.2)
    else none))

/-- The update events of a delete's rewrites, in the kernel's order: by object type, then by record id (the
    kernel collects them in a map keyed so). -/
def referrerEvents (t : Tenant) (ty : TypeId) (id : RecordId) : List EventBody :=
  ((referrerEntries t ty id).mergeSort
    (fun a b => decide (a.1 < b.1 ∨ (a.1 = b.1 ∧ a.2.1 ≤ b.2.1)))).map
    (fun e => .recordUpdated e.1 e.2.1 e.2.2.1 e.2.2.2 [])

/-- The composite and single unique constraints the schema change names, as its summary line. -/
def Intent.line : Intent → Option SchemaLine
  | .setRequired ty f _ => some (.field ty f)
  | .setUnique ty f _ => some (.field ty f)
  | .defineUnique ty fs => some (.unique ty fs)
  | _ => none

/-- The events an accepted intent against a tenant that exists emits. `t` is the tenant before the step, `t'`
    the tenant the decision wrote (its revision and sequence not yet raised). -/
def eventBodies (t : Tenant) (p : Principal) : Intent → Tenant → List EventBody
  | .create ty _, t' => match t'.recordAt ty t.nextId with
    | some r => [.recordCreated ty t.nextId r]
    | none => []
  | .update ty id fs _, t' => match t.recordAt ty id, t'.recordAt ty id with
    | some old, some r' =>
      [.recordUpdated ty id old r'
        (firedAll p old (patched old fs) (((t.typ? ty).map ObjType.workflows).getD []))]
    | _, _ => []
  | .delete ty id _, _ => match t.recordAt ty id with
    | some old => referrerEvents t ty id ++ [.recordDeleted ty id old]
    | none => []
  | i, t' => match i.line with
    | some l => [.schemaChanged (t.schemaVersion + 1) [l] (violatedCount t.limits (scopedTypes t'.types (i.touched t)) t'.env)]
    | none => []

/-- The tenant an accepted intent leaves: the decision's tenant with its revision raised by one, its event
    sequence raised by the number of events the intent emits, and its schema version raised by one when the
    intent changes the schema. -/
def commitT (t t' : Tenant) (p : Principal) (i : Intent) : Tenant :=
  { t' with revision := t.revision + 1,
            eventSeq := t.eventSeq + (eventBodies t p i t').length,
            schemaVersion := if i.isSchema then t.schemaVersion + 1 else t.schemaVersion }


/-- The tenant a `CreateTenant` makes, when none exists: the system only, with a name that is not
    blank; it has the owner as its one member (`genesisTenant`). -/
def createdTenant (p : Principal) (owner : PrincipalId) (name : String) : Except Err Tenant :=
  if p != .system then .error .unauthorized
  else if isBlank name then .error .invalidIntent
  else .ok (genesisTenant owner)

/-- The transition function of the model: one intent, in one context, against the world. The reserved
    principal ids are refused first (`reservedRefused`), as in the kernel; then a tenant that does not
    exist is made by a `CreateTenant` and is otherwise `noTenant`, and an intent against a tenant that
    exists is `stepTenant`'s. -/
def step (w : World) (i : Intent) (c : Ctx) : Except Err World :=
  if reservedRefused c.principal i then .error .invalidIntent
  else
  match w c.tenant with
  | none =>
    match i with
    | .createTenant owner name =>
      match createdTenant c.principal owner name with
      | .error e => .error e
      | .ok t' => .ok (fun x => if x = c.tenant then some t' else w x)
    | _ => .error .noTenant
  | some t =>
    match stepTenant t c.principal i with
    | .error e => .error e
    | .ok t' => .ok (fun x => if x = c.tenant then some t' else w x)

/-- The outcome of an accepted step (slice 9): the world after it, the domain events it emits in order, with
    their sequence numbers and the revision they were committed at, and one `Publish` effect per event, in
    the same order. A refused step has no outcome. -/
structure Transition where
  world : World
  events : List DomainEvent
  effects : List Effect

/-- Numbers events from `seq` on, the `k`-th of the list (from `k`) having the sequence `seq + k`; all of them
    are committed at `rev` by `p`. -/
def numberEvents (seq rev : Nat) (p : Principal) : Nat → List EventBody → List DomainEvent
  | _, [] => []
  | k, b :: bs => { seq := seq + k, revision := rev, principal := p, body := b } ::
      numberEvents seq rev p (k + 1) bs

/-- The events of an accepted step, given the world before and after it: a creation of a tenant emits its
    `TenantCreated` event as the first of its sequence; any other step's events are `eventBodies` of the
    addressed tenant, numbered after its sequence and committed at its revision. -/
def stepEvents (w w' : World) (c : Ctx) (i : Intent) : List DomainEvent :=
  match w c.tenant, w' c.tenant with
  | none, some t' => match i with
    | .createTenant _ name => [{ seq := 1, revision := t'.revision, principal := c.principal,
                                 body := .tenantCreated name }]
    | _ => []
  | some t, some t' => numberEvents (t.eventSeq + 1) t'.revision c.principal 0 (eventBodies t c.principal i t')
  | _, _ => []

/-- The world after an accepted step, with the tenant it addressed committed (`commitT`): its revision, event
    sequence and schema version are raised. A creation writes the genesis tenant, which holds its own
    revision and sequence, so it is kept. Every other tenant is kept as `step` left it. -/
def commitWorld (w w' : World) (c : Ctx) (i : Intent) : World :=
  fun x => if x = c.tenant then
      match w c.tenant, w' c.tenant with
      | some t, some t' => some (commitT t t' c.principal i)
      | _, _ => w' x
    else w' x

/-- The transition of one intent (slice 9): the world after it, which is `step`'s with the tenant committed
    (`commitWorld`), the domain events it emits in order, and one `Publish` effect per event. A refused step
    has no transition, so it emits nothing and the revision stays where it was. `step` is the projection of
    this world onto the records and the membership, which `transition_step` states. -/
def transition (w : World) (i : Intent) (c : Ctx) : Except Err Transition :=
  match step w i c with
  | .error e => .error e
  | .ok w' =>
    let w'' := commitWorld w w' c i
    let evs := stepEvents w w'' c i
    .ok { world := w'', events := evs, effects := evs.map (fun e => .publish e.seq) }


/-- An accepted membership intent writes back the tenant's membership and nothing else: the tenant it
    makes is the one before with another `MTenant`. -/
theorem stepTenant_member_eq {t t' : Tenant} {p : Principal} {j : MemberIntent}
    (h : stepTenant t p (.member j) = .ok t') : ∃ m, t' = t.withM m := by
  simp only [stepTenant, stepMemberT] at h
  cases hs : stepMember t.mt (j.ctxOf p) j with
  | error e => simp [hs, Except.map] at h
  | ok m => simp only [hs, Except.map, Except.ok.injEq] at h; exact ⟨m, h.symm⟩

/-- The two ways a step is accepted: against a tenant that exists, by `stepTenant`; or by a
    `CreateTenant` for a tenant that does not, which makes the genesis tenant. -/
theorem step_cases {w w' : World} {i : Intent} {c : Ctx} (h : step w i c = .ok w') :
    (∃ t t', w c.tenant = some t ∧ stepTenant t c.principal i = .ok t' ∧
      w' = fun x => if x = c.tenant then some t' else w x) ∨
    (∃ owner name, i = .createTenant owner name ∧ w c.tenant = none ∧
      w' = fun x => if x = c.tenant then some (genesisTenant owner) else w x) := by
  unfold step at h
  split at h
  · cases h
  · cases hw : w c.tenant with
    | some t =>
      simp only [hw] at h
      cases hs : stepTenant t c.principal i with
      | error e => simp [hs] at h
      | ok t' =>
        simp only [hs, Except.ok.injEq] at h
        exact Or.inl ⟨t, t', rfl, hs, h.symm⟩
    | none =>
      simp only [hw] at h
      cases i with
      | createTenant owner name =>
        cases hc : createdTenant c.principal owner name with
        | error e => simp [hc] at h
        | ok t' =>
          simp only [hc, Except.ok.injEq] at h
          have : t' = genesisTenant owner := by
            unfold createdTenant at hc
            split at hc
            · cases hc
            · split at hc
              · cases hc
              · simpa using hc.symm
          subst this
          exact Or.inr ⟨owner, name, rfl, rfl, h.symm⟩
      | _ => simp at h

/-- Referential integrity: every reference held by a record of the tenant, of any type, names an
    existing record of the type its field names. -/
def Tenant.RefsOk (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ q ∈ o.records, refsOk t.env o.schema q.2 = true

/-- Every record of the tenant conforms to the schema of its type. -/
def Tenant.Conform (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ q ∈ o.records, conforms o.schema q.2 = true

/-- Every record id of the tenant is below the counter. -/
def Tenant.IdsBelow (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ q ∈ o.records, q.1 < t.nextId

/-- The names are distinct: each object type is listed once, each schema names each field once,
    and each record holds each field at most once (the kernel's records are maps). -/
def Tenant.Distinct (t : Tenant) : Prop :=
  (t.types.map Prod.fst).Nodup ∧
    ∀ T o, t.typ? T = some o → (o.schema.map Prod.fst).Nodup ∧
      ∀ q ∈ o.records, (q.2.fields.map Prod.fst).Nodup

/-- No two distinct records of one type agree on a unique field, or on every field of a composite
    unique constraint of that type. -/
def Tenant.UniqueRecords (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ q₁ ∈ o.records, ∀ q₂ ∈ o.records, q₁.1 ≠ q₂.1 →
    (∀ fd ∈ o.schema, fd.2.unique = true → agreeOn [fd.1] q₁.2.fields q₂.2.fields = false) ∧
    (∀ u ∈ o.uniques, agreeOn u q₁.2.fields q₂.2.fields = false)

/-- The state invariant of one tenant: every record conforms to the schema of its type, was given
    an id below the counter, refers only to existing records, and the names are distinct and the
    unique constraints hold. -/
def Tenant.WF (t : Tenant) : Prop :=
  t.Conform ∧ t.IdsBelow ∧ t.RefsOk ∧ t.Distinct ∧ t.UniqueRecords

def World.WF (w : World) : Prop :=
  ∀ x t, w x = some t → t.WF

def World.RefsOk (w : World) : Prop :=
  ∀ x t, w x = some t → t.RefsOk

/-- Workflow states (slice 8): every stored record holds, in each workflow field of its type that holds a
    string, one of the states of that workflow. -/
def Tenant.StatesOk (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ q ∈ o.records, statesOk o.workflows q.2 = true

def World.StatesOk (w : World) : Prop :=
  ∀ x t, w x = some t → t.StatesOk

/-! ## Constraints as an invariant (slice 5) -/

/-- An expression that reads no old record and no principal: its value on a record does not depend
    on which write checks it, so a constraint of this kind holds of a stored record exactly when
    it holds of the record as a write checked it. -/
def Expr.stateOnly : Expr → Bool
  | .lit _ => true
  | .field s _ => s != .old
  | .createdBy s => s != .old
  | .principal => false
  | .cmp _ a b => a.stateOnly && b.stateOnly
  | .and a b => a.stateOnly && b.stateOnly
  | .or a b => a.stateOnly && b.stateOnly
  | .not a => a.stateOnly
  | .isNull a => a.stateOnly

/-- Every constraint of the tenant holds of every stored record: none is `false` or unevaluable. -/
def Tenant.Satisfied (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ q ∈ o.records,
    constraintProblem (storedEnv q.2) o.constraints = none

/-- Every constraint of the tenant is state-only (`Expr.stateOnly`). -/
def Tenant.StateConstraints (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → ∀ c ∈ o.constraints, c.stateOnly = true

end Strata
