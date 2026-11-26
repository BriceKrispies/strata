/-
The domain events and the effects of an accepted transition (slice 9, extended by slice 15).

`Strata/Model.lean` derives the events of each intent and decides the transition; this file holds the types they
are made of, so that the trigger decisions (`Strata/Triggers.lean`) can be stated over them before the transition
calls them. The kernel's `EventBody`, `DomainEvent` and `Effect` (`crates/model-core/src/types.rs`) are the source.
-/

import Strata.ModelBase
import Strata.LedgerTypes
import Strata.SchemaChange

namespace Strata

/-- Who a membership event names: a user, or a team, whose principal the kernel writes as `team:<id>`. -/
inductive Member where
  | user (p : PrincipalId)
  | team (t : TeamId)
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
  /-- A principal's effective roles changed: its direct roles and those of its teams (slice 9). -/
  | memberChanged (who : Member) (roles : List RoleId)
  /-- An override of a setting was set or removed, naming the setting and its subject and carrying no value
      (slice 12, the kernel's `SettingChanged`: a person's value can be personal data). -/
  | settingChanged (name : String) (subject : SettingSubject)
  /-- An entry was posted to a ledger account, or an expiry debited one: the account, the amount, the balances
      around it and the entry's number (slice 13, the kernel's `LedgerEntryPosted`). -/
  | ledgerEntryPosted (e : LedgerPosted)
  /-- An automation was suspended because its definer may no longer confer `role`, a role of its run-as (slice 15, the
      kernel's `AutomationSuspended`, WFL-1529). -/
  | automationSuspended (automation : Nat) (definer : PrincipalId) (role : RoleId)
  deriving Repr

/-- A domain event: its sequence number (gapless per tenant), the revision it was committed at, the principal
    that caused it, and its body. The kernel's time and correlation are not modelled. -/
structure DomainEvent where
  seq : Nat
  revision : Nat
  principal : Principal
  body : EventBody
  deriving Repr

/-- Why a trigger declined to start a run (the kernel's `AutomationExceptionReason` for the reasons the trigger decision
    gives; the worker's own reasons are not modelled). -/
inductive AutoReason where
  | hopLimit
  | runAsSuspended
  | automationRemoved
  | conditionFailed
  | automationInactive
  deriving DecidableEq, Repr

/-- A run that a trigger starts (the kernel's `AutomationStart`): the trigger and its automation, the sequence of the
    event, the hop of that event, the automation principal the steps act as, the definer whose authority bounds them,
    the schema version the run starts on, the run key (the SHA-256 hex of the canonical JSON of the key value, when the
    automation declares one), and the parameters the event binds, by name in key order. The parameter `at` (the event
    time) is not modelled. -/
structure AutoStart where
  trigger : Nat
  automation : Nat
  eventSeq : Nat
  hop : Nat
  causedBy : Principal
  onBehalfOf : Option PrincipalId
  definitionVersion : Nat
  runKey : Option String
  params : List (String × Val)
  deriving DecidableEq, Repr

/-- A trigger that matched an event and starts no run, for a reason the tenant owner sees (the kernel's
    `AutomationDecline`). Its detail text is not modelled. -/
structure AutoDecline where
  trigger : Nat
  automation : Nat
  eventSeq : Nat
  hop : Nat
  reason : AutoReason
  deriving DecidableEq, Repr

/-- An effect of an accepted transition (the kernel's `Effect`): a `Publish` for each event, and the automation hand-offs
    of the triggers (slice 15). The session revocation is not modelled. -/
inductive Effect where
  | publish (seq : Nat)
  | startAutomation (s : AutoStart)
  | declineAutomation (d : AutoDecline)
  deriving DecidableEq, Repr
