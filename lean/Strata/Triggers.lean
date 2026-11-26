/-
The trigger decisions of the kernel (slice 15): what `triggers::after_decision` adds to an accepted transition.

Once the events of an accepted transition are final, the kernel (`crates/kernel/src/triggers.rs`):

1. when the transition changed members or the schema, suspends each automation whose definer may no longer confer
   a role of its run-as, with an `AutomationSuspended` event (`suspendLost`, WFL-1529, D-7204);
2. for each event, in sequence order, and each trigger whose event matches it, in trigger id order, decides the
   trigger (`decideTrigger`): a `StartAutomation` for a run, a `DeclineAutomation` for a run declined for a reason the
   owner sees, or nothing (a paused trigger, a false condition, an event the automation principal may not read).

The automation principal holds exactly the run-as roles of its automation (`Tenant.rolesOf`, `ModelBase.lean`), and
a condition and a run key are evaluated as that principal, with the record scopes the event binds (D-6523, D-7201).

Abstractions, all recorded in `docs/verification.md`: the field read rules are not consulted (the model's visibility
is the record's, `Tenant.canRead`); the time of an event (`param.at`) is not bound; the steps of an automation are not
modelled; a condition or run key that needs a record the snapshot lacks does not arise (the model reads the whole
tenant).
-/

import Strata.Events
import Strata.Reads
import Strata.Members
import Strata.Canon

namespace Strata

/-- The hop limit (D-700): a trigger starts a run for an event whose hop is at most this, and declines with
    `hopLimit` past it (SCH-300, SCH-1532). -/
def hopLimit : Nat := 8

/-- Whether an event matches a trigger's event (the kernel's `triggers::matches`, D-6522, D-7203): the kind and the
    object type, and for a record update naming a transition, a transition the update fired. A ledger posting
    matches the posting to the unit the trigger names (D-6523). -/
def TriggerOn.matches : TriggerOn → EventBody → Bool
  | .recordCreated ty, .recordCreated ty' _ _ => ty == ty'
  | .recordUpdated ty tr, .recordUpdated ty' _ _ _ fired => ty == ty' && (match tr with
      | none => true
      | some name => fired.any (fun f => f.2 == name))
  | .recordDeleted ty, .recordDeleted ty' _ _ => ty == ty'
  | .ledgerPosted unit, .ledgerEntryPosted e => unit == e.unit
  | _, _ => false

/-- The value of a ledger owner as the kernel binds it: the person's principal, or null for a pool (D-6523). -/
def ownerVal : LedgerOwner → Val
  | .person p => .str ("p" ++ toString p)
  | .pool _ => .null

/-- The name of a ledger unit as the kernel names it (`u<n>`). -/
def unitName (n : Nat) : String := "u" ++ toString n

/-- The parameters an event binds for a trigger (the kernel's `event_params`, D-6523), in key order: `seq` and `hop`
    for every event; for a ledger posting its owner, unit, amount, balances, entry number and reason; for a record
    update its transition, the one the trigger names or else the first the update fired. The time `at` is not bound. -/
def eventParams (body : EventBody) (seq hop : Nat) (on : TriggerOn) : List (String × Val) :=
  let ledger : List (String × Val) := match body with
    | .ledgerEntryPosted e =>
      [("owner", ownerVal e.owner), ("unit", .str (unitName e.unit)), ("amount", .int e.amount),
       ("balance", .int e.balance), ("balance_before", .int e.balanceBefore),
       ("entry_sequence", .int e.entrySequence), ("reason", .str e.reason)]
    | _ => []
  let transition : List (String × Val) := match body with
    | .recordUpdated _ _ _ _ fired =>
      let name : Option String := match on with
        | .recordUpdated _ (some n) => some n
        | _ => (fired.head?).map (fun f => f.2)
      [("transition", match name with
        | some n => .str n
        | none => .null)]
    | _ => []
  ((("seq", Val.int seq) :: ("hop", .int hop) :: ledger ++ transition).mergeSort
    (fun a b => decide (a.1 ≤ b.1)))

/-- The scopes an event binds for a principal (the kernel's `scopes`, D-6523, D-7201): a record created binds `record`
    and `new`, a record updated `old` (when the principal may read the record as it was) and `new`, a record deleted
    `old`; a ledger posting binds nothing. `none` when the principal may not read the record the event leaves, so the
    trigger does not observe the event. -/
structure AutoScopes where
  record : Option Record := none
  old : Option Record := none
  new : Option Record := none

def scopesOf (t : Tenant) (p : Principal) : EventBody → Option AutoScopes
  | .recordCreated ty _ r =>
    if t.canRead p ty r then some { record := some r, new := some r } else none
  | .recordUpdated ty _ before after _ =>
    if t.canRead p ty after then
      some { old := if t.canRead p ty before then some before else none, new := some after }
    else none
  | .recordDeleted ty _ before =>
    if t.canRead p ty before then some { old := some before } else none
  | .ledgerEntryPosted _ => some {}
  | _ => none

/-- The value of an element of a list, as the kernel's `Value`. -/
def elemCanon : Elem → CanonValue
  | .null => .null
  | .int i => .int i
  | .str s => .str s
  | .bool b => .bool b
  | .ref id => .ref id

/-- A model value as the kernel's `Value`, for the hash of a run key. -/
def valCanon : Val → CanonValue
  | .null => .null
  | .int i => .int i
  | .str s => .str s
  | .bool b => .bool b
  | .ref id => .ref id
  | .list vs => .list (vs.map elemCanon)

/-- The effect a trigger decides for an event, in the state `t` after the transition (the kernel's `decide`, in its
    order): a paused trigger decides nothing; a missing automation declines `automationRemoved`, a suspended one
    `runAsSuspended` and a draft `automationInactive`, whatever the condition; an event the automation principal may
    not read decides nothing; a condition that cannot be evaluated declines `conditionFailed`, and one that does not
    hold (false, or null) decides nothing; past the hop limit the trigger declines `hopLimit`; a run key that is null or
    cannot be evaluated declines `conditionFailed`; otherwise the run starts. -/
def decideTrigger (t : Tenant) (hop : Nat) (ev : DomainEvent) (tr : TriggerDef) : Option Effect :=
  let decline (r : AutoReason) : Option Effect :=
    some (.declineAutomation { trigger := tr.id, automation := tr.automation, eventSeq := ev.seq, hop := hop,
                               reason := r })
  if tr.paused then none
  else match t.automationDef? tr.automation with
  | none => decline .automationRemoved
  | some a =>
    if a.suspendedFor.isSome then decline .runAsSuspended
    else if a.state == .draft then decline .automationInactive
    else
      let who : Principal := .automation a.id
      match scopesOf t who ev.body with
      | none => none
      | some sc =>
        let env : ExprEnv := { record := sc.record, old := sc.old, new := sc.new,
                               principal := none,
                               params := eventParams ev.body ev.seq hop tr.on }
        -- A condition holds only when it is exactly `true` (the kernel's `holds`); an evaluation error declines.
        let condition : Option (Option Effect) := match tr.condition with
          | none => some none
          | some c => match evalExpr env c with
            | .ok (.bool true) => some none
            | .ok _ => none
            | .error _ => some (decline .conditionFailed)
        match condition with
        | none => none
        | some (some e) => some e
        | some none =>
          if hop > hopLimit then decline .hopLimit
          else
            let start (key : Option String) : Option Effect :=
              some (.startAutomation { trigger := tr.id, automation := tr.automation, eventSeq := ev.seq,
                                       hop := hop, causedBy := .automation a.id, onBehalfOf := a.definedBy,
                                       definitionVersion := t.schemaVersion, runKey := key,
                                       params := eventParams ev.body ev.seq hop tr.on })
            match a.runKey with
            | none => start none
            | some k => match evalExpr env k with
              | .error _ => decline .conditionFailed
              | .ok .null => decline .conditionFailed
              | .ok v => start (some (runKeyHash (valCanon v)))

/-- The effects of the triggers for the events of a transition, in order (the loop of the kernel's `after_decision`):
    for each event, each trigger in id order whose event matches it, decided in the state `t`. -/
def triggerEffects (t : Tenant) (hop : Nat) (evs : List DomainEvent) : List Effect :=
  let trs := (t.triggers.map Prod.snd).mergeSort (fun a b => decide (a.id ≤ b.id))
  evs.flatMap (fun ev => trs.filterMap (fun tr =>
    if tr.on.matches ev.body then decideTrigger t hop ev tr else none))

/-- The role of an automation's run-as that its definer may no longer confer: the first in id order that
    `mayConfer` refuses (the kernel's `lost_authority`), or none when the definer may confer all of them. -/
def lostRole? (m : MTenant) (definer : PrincipalId) (runAs : List RoleId) : Option RoleId :=
  (runAs.mergeSort (fun a b => decide (a ≤ b))).find? (fun r => !mayConfer m (.user definer) r)

/-- The automations whose definer may no longer confer a role of their run-as, each with that definer and the role, in
    automation id order. An automation that is already suspended, or has no definer, is not checked. -/
def lostAuthority (t : Tenant) : List (AutomationDef × PrincipalId × RoleId) :=
  ((t.automations.map Prod.snd).mergeSort (fun a b => decide (a.id ≤ b.id))).filterMap (fun a =>
    if a.suspendedFor.isSome then none
    else match a.definedBy with
      | none => none
      | some d => (lostRole? t.mt d a.runAs).map (fun r => (a, d, r)))

/-- Suspends, in the tenant, each automation that lost authority (its `suspendedFor` is the role), and the events the
    suspensions emit, in the same order (the kernel's `suspend_lost`). -/
def suspendLost (t : Tenant) : Tenant × List EventBody :=
  let lost := lostAuthority t
  if lost.isEmpty then (t, [])
  else
    let automations := t.automations.map (fun e =>
      match lost.find? (fun l => l.1.id == e.1) with
      | some l => (e.1, { e.2 with suspendedFor := some l.2.2 })
      | none => e)
    ({ t with automations := automations },
     lost.map (fun l => EventBody.automationSuspended l.1.id l.2.1 l.2.2))

end Strata
