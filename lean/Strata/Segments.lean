/-
Segments (slice 16, WG-5672): the kernel's segments in the model, for the fragment of the expression language the
model has (`crates/kernel/src/segments.rs`, `segment_flow.rs`).

The decisions:

- `segDefine`, `segRemove`: a definition in the kernel's order (capability, name, subject and principal field, the
  predicate's checks and the definer's read power, the refresh interval, the bounds), and the removal of a segment with
  its memberships and no exit event (SEG-026).
- `segRecordPass`: after an accepted step, the persons whose subject record it changed are decided, in each segment in id
  order and each person in the order of their string; an entry or exit event for each membership that changes.
- `segRebuildPass`: the rebuild a changed predicate starts (SEG-022): every person of the subject type, in record id order.
- `segTransition` ties them together for the segment intents; `transition` (Model.lean) runs `segAfterRecords` after each
  accepted step.
- `decideSegTrigger` and `segTriggerEffects`: the triggers on the segment log (`segment_flow::decide`): the entry and
  exit events feed the triggers of the automations, with the parameters `seq` (the position in the log), `hop`, `person`,
  `segment` and `reason`.

Abstractions, each a difference from the kernel and recorded in `docs/verification.md`:

1. No clock: the predicate reads no time, so a segment has no refresh interval and no wake row; a refresh interval is
   refused (SEG-020). The time of an event is not bound (`param.at`).
2. No reference hop: a predicate reads the person's record only (the model's expression language has no `through`).
3. No read rules and no visibility to members: the definer's power is the record-level read decision (`holdsRead`), and
   reads of the membership are not modelled.
4. A decision evaluates every person the kernel plans in one step; the kernel's batches (BATCH persons per transaction)
   and their continuations are not modelled. The fixtures stay below the batch size.
5. Erasure (SEG-036 to SEG-038) is not modelled (slice 17). Roles do not enter the membership of a predicate of the
   model's fragment, since `principal` is the person's own id, so a change of roles plans no change.
6. The principal field holds `p<n>`; another string names no person in the model.
7. The correlation of an event is not modelled, and a trigger's principal expression is null for an automation, as for
   record triggers (slice 15).
-/

import Strata.Triggers

namespace Strata

/-- The most segments a tenant holds (SEG-029). -/
def maxSegments : Nat := 100

/-- The most segments whose predicates read any one object type (SEG-029). -/
def maxSegmentsPerObject : Nat := 25

/-- The most nodes of a predicate (SEG-004). -/
def maxPredicateNodes : Nat := 200

/-- The longest segment name, in characters (SEG-001). -/
def maxNameChars : Nat := 120

/-- The capability that defining, removing and rebuilding a segment needs (SEG-007). -/
def capSegmentsDefine : CapabilityId := 7

/-- The string a person's principal field holds: `p<n>` (the kernel's principal ids, as the fixtures name them). -/
def personStr (p : PrincipalId) : String := "p" ++ toString p

/-- The person a string names, when it is `p<n>` for a canonical number `n`. -/
def personOfStr (s : String) : Option PrincipalId :=
  match s.toList with
  | 'p' :: rest =>
    let digits := String.ofList rest
    match digits.toNat? with
    | some n => if toString n = digits then some n else none
    | none => none
  | _ => none

/-- The person a subject record stands for: the person its principal field names (the kernel's `person_of`). -/
def recordPerson (pf : FieldId) (r : Record) : Option PrincipalId :=
  match r.fields.get pf with
  | .str text => personOfStr text
  | _ => none

/-! ## The checks of a predicate -/

/-- The shape of a value of the predicate language, as the kernel's `expr::check` gives it for the model's fragment; `any`
    is null and what the check cannot pin down. -/
inductive SegShape where
  | bool
  | int
  | str
  | ref
  | list
  | any
  deriving DecidableEq, Repr

def shapeOfVal : Val → SegShape
  | .null => .any
  | .int _ => .int
  | .str _ => .str
  | .bool _ => .bool
  | .ref _ => .ref
  | .list _ => .list

def shapeOfTy : Ty → SegShape
  | .int => .int
  | .str => .str
  | .bool => .bool
  | .ref _ _ => .ref
  | .list _ => .list

/-- Two operands of a comparison that the kernel accepts together. -/
def segCompat (a b : SegShape) : Bool := a == .any || b == .any || a == b

/-- An operand of an ordering: an integer or a string (the model has no time). -/
def segOrdered (a : SegShape) : Bool := a == .any || a == .int || a == .str

/-- An operand of a connective: a truth value. -/
def segBoolish (a : SegShape) : Bool := a == .any || a == .bool

/-- The shape of a predicate over the schema of its subject type, as the kernel's `expr::check` gives it with the record
    scope only and no parameters. `none` for an expression the check refuses: a read of `old` or `new`, a parameter, a
    field the schema lacks, or an operand of the wrong shape (SEG-002, SEG-003). -/
def predShape (sc : Schema) : Expr → Option SegShape
  | .lit v => some (shapeOfVal v)
  | .field .record f => (sc.lookup f).map (fun d => shapeOfTy d.ty)
  | .field _ _ => none
  | .createdBy .record => some .str
  | .createdBy _ => none
  | .principal => some .str
  | .param _ => none
  | .cmp op a b => do
    let sa ← predShape sc a
    let sb ← predShape sc b
    match op with
    | .eq => if segCompat sa sb then some .bool else none
    | .ne => if segCompat sa sb then some .bool else none
    | _ => if segCompat sa sb && segOrdered sa && segOrdered sb then some .bool else none
  | .and a b => do
    let sa ← predShape sc a
    let sb ← predShape sc b
    if segBoolish sa && segBoolish sb then some .bool else none
  | .or a b => do
    let sa ← predShape sc a
    let sb ← predShape sc b
    if segBoolish sa && segBoolish sb then some .bool else none
  | .not a => do
    let sa ← predShape sc a
    if segBoolish sa then some .bool else none
  | .isNull a => do
    let _ ← predShape sc a
    some .bool

/-- The nodes of an expression (the kernel's `expr_nodes`): one for each node, the leaves included. -/
def segNodes : Expr → Nat
  | .lit _ => 1
  | .field _ _ => 1
  | .createdBy _ => 1
  | .principal => 1
  | .param _ => 1
  | .cmp _ a b => 1 + segNodes a + segNodes b
  | .and a b => 1 + segNodes a + segNodes b
  | .or a b => 1 + segNodes a + segNodes b
  | .not a => 1 + segNodes a
  | .isNull a => 1 + segNodes a

/-- The record fields a predicate reads (the kernel's `Reads.subject_fields`). -/
def predFields : Expr → List FieldId
  | .field .record f => [f]
  | .cmp _ a b => predFields a ++ predFields b
  | .and a b => predFields a ++ predFields b
  | .or a b => predFields a ++ predFields b
  | .not a => predFields a
  | .isNull a => predFields a
  | _ => []

/-- Whether a predicate reads a system attribute of the subject record (the kernel's `system_attributes`): the creator. -/
def predSystem : Expr → Bool
  | .createdBy .record => true
  | .cmp _ a b => predSystem a || predSystem b
  | .and a b => predSystem a || predSystem b
  | .or a b => predSystem a || predSystem b
  | .not a => predSystem a
  | .isNull a => predSystem a
  | _ => false

/-- A predicate is admitted for its subject schema: within the node bound, and a truth value (SEG-002, SEG-004). -/
def predicateOk (sc : Schema) (e : Expr) : Bool :=
  decide (segNodes e ≤ maxPredicateNodes) &&
    (match predShape sc e with
     | some s => segBoolish s
     | none => false)

/-- The principal field is a required unique string field of the subject type (SEG-1548). -/
def principalFieldOk (o : ObjType) (pf : FieldId) : Bool :=
  match o.schema.lookup pf with
  | some d => d.ty == .str && d.required && d.unique
  | none => false

/-! ## Powers of the definer -/

/-- Whether the principal holds the segment capability: the owner does, and so does a role it holds that is granted it
    (the kernel's `holds_capability`). -/
def Tenant.holdsSegDefine (t : Tenant) (p : Principal) : Bool :=
  (t.rolesOf p).contains ownerRole ||
    t.grants.any (fun g => (t.rolesOf p).contains g.1 && decide (g.2 = .capability capSegmentsDefine))

/-- Whether the principal may read every record of an object type without a condition: the owner, a grant of its role on
    the type, or an unconditional policy of its role that covers the type (the kernel's `holds_power` with no condition,
    SEG-005). -/
def Tenant.holdsRead (t : Tenant) (p : Principal) (ty : TypeId) : Bool :=
  let roles := t.rolesOf p
  roles.contains ownerRole ||
    t.grants.any (fun g => roles.contains g.1 && decide (g.2 = .object ty Action.read)) ||
    t.policies.any (fun pol => roles.contains pol.role && pol.actions.contains Action.read &&
      pol.resource.covers (.object ty) && pol.condition.isNone)

/-! ## Lookups and the membership rows -/

/-- The segment with an id, if the tenant defines it. -/
def Tenant.segment? (t : Tenant) (id : SegmentId) : Option SegmentDef := t.segments.lookup id

/-- The membership of a person in a segment, if the rows hold one. -/
def memberOf? (ms : List Membership) (s : SegmentId) (person : PrincipalId) : Option Membership :=
  ms.find? (fun m => m.segment == s && m.person == person)

/-- The rows without the membership of a person in a segment. -/
def dropMember (ms : List Membership) (s : SegmentId) (person : PrincipalId) : List Membership :=
  ms.filter (fun m => !(m.segment == s && m.person == person))

/-- The rows with a membership in place of the one of its segment and person. -/
def setMember (ms : List Membership) (m : Membership) : List Membership :=
  m :: dropMember ms m.segment m.person

/-- The subject record of a person in a segment: the record of the subject type whose principal field holds the person's
    string (the kernel's `locate` by principal). -/
def segPersonRecord (t : Tenant) (s : SegmentDef) (person : PrincipalId) : Option Record :=
  match t.typ? s.subject with
  | none => none
  | some o =>
    (o.records.find? (fun q => q.2.fields.get s.principalField == .str (personStr person))).map (fun q => q.2)

/-- Whether the predicate of a segment holds for a person on the record: `some b`, or `none` when it cannot be evaluated
    (SEG-030). The principal is the person; a predicate holds only when it is exactly `true`. -/
def segHolds (s : SegmentDef) (r : Record) (person : PrincipalId) : Option Bool :=
  match evalExpr { record := some r, principal := some person } s.predicate with
  | .ok v => some (v == .bool true)
  | .error _ => none

/-- The decision for one person in one segment on the state `t`: whether the person is a member, or `none` when the
    evaluation fails. A person with no subject record is no member. -/
def segDecide (t : Tenant) (s : SegmentDef) (person : PrincipalId) : Option Bool :=
  match segPersonRecord t s person with
  | none => some false
  | some r => segHolds s r person

/-- The event of a membership change, as the kernel's `SegmentMembershipEvent` (its time and correlation are not modelled). -/
def segEvent (s : SegmentDef) (person : PrincipalId) (direction : SegDirection) (reason : SegReason) (revision hop : Nat)
    (p : Principal) : SegEvent :=
  { segment := s.id, person := person, direction := direction, reason := reason, version := s.version,
    revision := revision, hop := hop + 1, principal := p }

/-- The membership decision of one person (the kernel's `decide_targets`, for one pair): an entry for a person who becomes a
    member, an exit for one who stops, and a row raised to the current version for a member under a changed predicate; no
    change when the membership is as it was, and none when the evaluation fails (SEG-008 to SEG-010, SEG-030). -/
def segPair (t : Tenant) (revision hop : Nat) (reason : SegReason) (p : Principal) (s : SegmentDef)
    (person : PrincipalId) (acc : List Membership × List SegEvent) : List Membership × List SegEvent :=
  let (ms, evs) := acc
  match segDecide t s person with
  | none => acc
  | some member =>
    match memberOf? ms s.id person, member with
    | none, true =>
      (setMember ms { segment := s.id, person := person, version := s.version },
       evs ++ [segEvent s person .entered reason revision hop p])
    | some _, false =>
      (dropMember ms s.id person, evs ++ [segEvent s person .exited reason revision hop p])
    | some w, true =>
      if w.version != s.version then (setMember ms { w with version := s.version }, evs) else acc
    | none, false => acc

/-- Decides a list of persons of one segment in order, from the rows and events so far. -/
def segPairs (t : Tenant) (revision hop : Nat) (reason : SegReason) (p : Principal) (s : SegmentDef)
    (persons : List PrincipalId) (acc : List Membership × List SegEvent) : List Membership × List SegEvent :=
  persons.foldl (fun a person => segPair t revision hop reason p s person a) acc

/-! ## The definition -/

/-- The definition of a segment, decided in the kernel's order (`segments::define`): the caller holds the segment
    capability (`Unauthorized`); the name is 1 to 120 characters after trimming and is not another segment's name
    (`InvalidIntent`); an existing segment keeps its subject type and principal field (`InvalidIntent`); the principal
    field is a required unique string field (`InvalidIntent`); the predicate checks (`InvalidIntent`); the definer may read
    every record the predicate reads (`Unauthorized`); a refresh interval is refused for a predicate that reads no time
    (`InvalidIntent`, SEG-020); the tenant stays within its bounds (`LimitExceeded`, SEG-029). A new or changed predicate
    raises the version; the memberships are not decided here (the rebuild decides them, SEG-022). -/
def segDefineSegs (t : Tenant) (p : Principal) (id : SegmentId) (name : String) (subject : TypeId) (pf : FieldId)
    (predicate : Expr) (refresh : Option Nat) : Except Err (List (SegmentId × SegmentDef)) :=
  if !t.holdsSegDefine p then .error .unauthorized
  else
    let nm := trimWs name
    if isBlank nm || decide (maxNameChars < nm.length) then .error .invalidIntent
    else if t.segments.any (fun s => s.1 != id && s.2.name == nm) then .error .invalidIntent
    else
      let old := t.segment? id
      if old.any (fun o => o.subject != subject || o.principalField != pf) then .error .invalidIntent
      else
        match t.typ? subject with
        | none => .error .invalidIntent
        | some o =>
          if !principalFieldOk o pf then .error .invalidIntent
          else if !predicateOk o.schema predicate then .error .invalidIntent
          else if !t.holdsRead p subject then .error .unauthorized
          else if refresh.isSome then .error .invalidIntent
          else if decide (maxSegments < (t.segments.filter (fun s => s.1 != id)).length + 1) then
            .error .limitExceeded
          else if decide (maxSegmentsPerObject <
              (t.segments.filter (fun s => s.1 != id && s.2.subject == subject)).length + 1) then
            .error .limitExceeded
          else
            let version : Nat := match old with
              | none => 1
              | some s => if s.predicate == predicate then s.version else s.version + 1
            let definedBy : Option PrincipalId := match p with
              | .user u => some u
              | _ => old.bind (fun s => s.definedBy)
            let d : SegmentDef := { id := id, name := nm, subject := subject, predicate := predicate,
                                    version := version, principalField := pf, definedBy := definedBy }
            .ok ((id, d) :: t.segments.filter (fun s => s.1 != id))

/-- The definition of a segment as a tenant: the segments of `segDefineSegs` replace those of the tenant. -/
def segDefine (t : Tenant) (p : Principal) (id : SegmentId) (name : String) (subject : TypeId) (pf : FieldId)
    (predicate : Expr) (refresh : Option Nat) : Except Err Tenant :=
  (segDefineSegs t p id name subject pf predicate refresh).map (fun s => { t with segments := s })

/-- The removal of a segment (`segments::remove`): the caller holds the capability (`Unauthorized`); a segment the tenant
    does not hold is `NotFound`. Its memberships go with it and no exit event is written (SEG-026); its log is kept. -/
def segRemove (t : Tenant) (p : Principal) (id : SegmentId) : Except Err Tenant :=
  if !t.holdsSegDefine p then .error .unauthorized
  else if (t.segment? id).isNone then .error .notFound
  else .ok { t with segments := t.segments.filter (fun s => s.1 != id),
                     memberships := t.memberships.filter (fun m => m.segment != id) }

/-! ## The decisions of a step -/

/-- The persons of a list, sorted by the string they hold (the order of the kernel's `Target` set). -/
def sortPersons (ps : List PrincipalId) : List PrincipalId :=
  ps.mergeSort (fun a b => !(decide (personStr b < personStr a)))

/-- The records a step changed, as (object type, before, after), from its events (the kernel's `changes`). -/
def recordChanges (evs : List DomainEvent) : List (TypeId × Option Record × Option Record) :=
  evs.filterMap (fun ev => match ev.body with
    | .recordCreated ty _ r => some (ty, none, some r)
    | .recordUpdated ty _ b a _ => some (ty, some b, some a)
    | .recordDeleted ty _ b => some (ty, some b, none)
    | _ => none)

/-- The fields whose values differ between two records (the kernel's `changed_fields`). -/
def changedFields (b a : Record) : List FieldId :=
  (b.fields.map Prod.fst ++ a.fields.map Prod.fst).filter (fun f => b.fields.get f != a.fields.get f)

/-- Whether a change of a record of object type `ty` (from `before` to `after`) can change the membership of a person in a
    segment: a record of the subject type, changed in a field the predicate reads or its principal field, or a predicate
    that reads the creator, or a created or deleted record (the kernel's `plan_change`, SEG-027). -/
def segRelevant (s : SegmentDef) (ty : TypeId) (before after : Option Record) : Bool :=
  decide (ty = s.subject) &&
    (match before, after with
     | some b, some a =>
       predSystem s.predicate ||
         (changedFields b a).any (fun f => f == s.principalField || (predFields s.predicate).contains f)
     | _, _ => true)

/-- Whether a predicate reads the principal (the kernel's `reads.roles`, set by `principal` and `has_role`): then a change
    of a person's roles re-evaluates the person (SEG-027). -/
def predRoles : Expr → Bool
  | .principal => true
  | .cmp _ a b => predRoles a || predRoles b
  | .and a b => predRoles a || predRoles b
  | .or a b => predRoles a || predRoles b
  | .not a => predRoles a
  | .isNull a => predRoles a
  | _ => false

/-- The users whose roles a step changed: the persons of its `MemberChanged` events (a team names no user, and a team's
    own event concerns no person). -/
def memberUsers (evs : List DomainEvent) : List PrincipalId :=
  evs.filterMap (fun ev => match ev.body with
    | .memberChanged (.user u) _ => some u
    | _ => none)

/-- The persons whose membership in a segment a step re-evaluates (the kernel's `plan_change`): the persons of the relevant
    records it changed, before and after, and, for a segment whose predicate reads the principal, the users whose roles it
    changed. In the order of their strings. -/
def segChangedPersons (s : SegmentDef) (evs : List DomainEvent) : List PrincipalId :=
  let fromRecords := (recordChanges evs).flatMap (fun c =>
    if segRelevant s c.1 c.2.1 c.2.2 then
      (c.2.2.bind (recordPerson s.principalField)).toList ++ (c.2.1.bind (recordPerson s.principalField)).toList
    else [])
  let fromMembers := if predRoles s.predicate then memberUsers evs else []
  sortPersons ((fromRecords ++ fromMembers).eraseDups)

/-- The memberships and the events an accepted step decides (`segRecordPass`): for each segment in id order, the persons its
    changed records concern, decided on the tenant after the step at its revision. -/
def segRecordPass (t : Tenant) (evs : List DomainEvent) (p : Principal) (hop : Nat) :
    List Membership × List SegEvent :=
  let segs := (t.segments.map Prod.snd).mergeSort (fun a b => decide (a.id ≤ b.id))
  segs.foldl (fun acc s => segPairs t t.revision hop .change p s (segChangedPersons s evs) acc)
    (t.memberships, [])

/-- The memberships and the events of a rebuild (`segRebuildPass`): every person of the subject type, in record id order,
    decided on the tenant at its revision; a segment the tenant does not hold rebuilds nothing. -/
def segRebuildPass (t : Tenant) (p : Principal) (id : SegmentId) (hop : Nat) : List Membership × List SegEvent :=
  match t.segment? id with
  | none => (t.memberships, [])
  | some s =>
    match t.typ? s.subject with
    | none => (t.memberships, [])
    | some o =>
      let records := o.records.mergeSort (fun a b => decide (a.1 ≤ b.1))
      let persons := (records.filterMap (fun q => recordPerson s.principalField q.2)).eraseDups
      segPairs t t.revision hop .predicateChange p s persons (t.memberships, [])

/-! ## The triggers on the segment log -/

/-- Whether a trigger's event matches a logged segment event (the kernel's `segment_flow::matches`). -/
def TriggerOn.matchesSeg : TriggerOn → SegEvent → Bool
  | .segmentEntered s, e => e.direction == .entered && s == e.segment
  | .segmentExited s, e => e.direction == .exited && s == e.segment
  | _, _ => false

/-- The parameters a segment event binds for a trigger (`segment_flow::event_params`): `hop`, `person`, `reason`,
    `segment` and `seq`, in key order. `at` is not bound. -/
def segEventParams (pos : Nat) (e : SegEvent) : List (String × Val) :=
  [("hop", .int (e.hop : Int)),
   ("person", .str (personStr e.person)),
   ("reason", .str (match e.reason with
     | .change => "change"
     | .predicateChange => "predicate-change")),
   ("segment", .str (toString e.segment)),
   ("seq", .int (pos : Int))]

/-- The effect of a trigger on a logged segment event at `pos` (`segment_flow::decide`): a paused trigger decides nothing;
    a missing automation declines `automationRemoved`, a suspended one `runAsSuspended`; no subject record of the person,
    or one the automation principal may not read, decides nothing; a condition that cannot be evaluated declines
    `conditionFailed`, and one that does not hold decides nothing; past the hop limit the trigger declines `hopLimit`; a
    run key that is null or cannot be evaluated declines `conditionFailed`; otherwise the run starts. -/
def decideSegTrigger (t : Tenant) (pos : Nat) (e : SegEvent) (tr : TriggerDef) : Option Effect :=
  let decline (r : AutoReason) : Option Effect :=
    some (.declineAutomation { trigger := tr.id, automation := tr.automation, eventSeq := pos, hop := e.hop,
                               reason := r })
  if tr.paused then none
  else match t.automationDef? tr.automation with
  | none => decline .automationRemoved
  | some a =>
    if a.suspendedFor.isSome then decline .runAsSuspended
    else
      let who : Principal := .automation a.id
      match t.segment? e.segment with
      | none => none
      | some s =>
        match segPersonRecord t s e.person with
        | none => none
        | some r =>
          if !t.canRead who s.subject r then none
          else
            let env : ExprEnv := { record := none, principal := none, params := segEventParams pos e }
            let condition : Option (Option Effect) := match tr.condition with
              | none => some none
              | some c => match evalExpr env c with
                | .ok (.bool true) => some none
                | .ok _ => none
                | .error _ => some (decline .conditionFailed)
            match condition with
            | none => none
            | some (some eff) => some eff
            | some none =>
              if e.hop > hopLimit then decline .hopLimit
              else
                let start (key : Option String) : Option Effect :=
                  some (.startAutomation { trigger := tr.id, automation := tr.automation, eventSeq := pos,
                                           hop := e.hop, causedBy := .automation a.id, onBehalfOf := a.definedBy,
                                           definitionVersion := t.schemaVersion, runKey := key,
                                           params := segEventParams pos e })
                match a.runKey with
                | none => start none
                | some k => match evalExpr env k with
                  | .error _ => decline .conditionFailed
                  | .ok .null => decline .conditionFailed
                  | .ok v => start (some (runKeyHash (valCanon v)))

/-- The events of a log, each with its position: the first of a log of `n` events has the position `n + 1`. -/
def numberFrom : Nat → List SegEvent → List (Nat × SegEvent)
  | _, [] => []
  | k, e :: es => (k + 1, e) :: numberFrom (k + 1) es

/-- The effects of the triggers on new segment events, in position order and then trigger id order. -/
def segTriggerEffects (t : Tenant) (entries : List (Nat × SegEvent)) : List Effect :=
  let trs := (t.triggers.map Prod.snd).mergeSort (fun a b => decide (a.id ≤ b.id))
  entries.flatMap (fun (pos, e) => trs.filterMap (fun tr =>
    if tr.on.matchesSeg e then decideSegTrigger t pos e tr else none))

/-- Commits decided memberships and new events to a tenant, with the effects of the triggers on the new events. -/
def segCommit (t : Tenant) (ms : List Membership) (new : List SegEvent) : Tenant × List Effect :=
  let t' := { t with memberships := ms, segLog := t.segLog ++ new }
  (t', segTriggerEffects t' (numberFrom t.segLog.length new))

/-- The segment decisions of an accepted record step: its memberships, its log and the effects of its triggers. A tenant
    without segments is unchanged. -/
def segAfterRecords (t : Tenant) (evs : List DomainEvent) (p : Principal) (hop : Nat) : Tenant × List Effect :=
  let (ms, new) := segRecordPass t evs p hop
  segCommit t ms new

/-! ## The segment intents -/

/-- The intents the segments take. They are the kernel's segment decisions, which the storage block carries and not a
    tenant transition: a definition, a removal, and the rebuild a predicate change starts. -/
inductive SegIntent where
  | define (id : SegmentId) (name : String) (subject : TypeId) (pf : FieldId) (predicate : Expr)
      (refresh : Option Nat)
  | remove (id : SegmentId)
  | rebuild (id : SegmentId)
  deriving Repr

/-- The segment pass of an accepted step keeps every field of its tenant except its memberships and its log (slice 16). -/
theorem segAfterRecords_shape (t : Tenant) (evs : List DomainEvent) (p : Principal) (hop : Nat) :
    ∃ ms l, (segAfterRecords t evs p hop).1 = { t with memberships := ms, segLog := l } := by
  unfold segAfterRecords segCommit
  rcases segRecordPass t evs p hop with ⟨ms, new⟩
  exact ⟨ms, _, rfl⟩

end Strata
