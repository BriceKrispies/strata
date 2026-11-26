/-!
# Ledger data (slice 13)

The data the tenant holds for its ledger, as the kernel holds it (`crates/kernel/src/ledger.rs`,
`ledger_expiry.rs`, `ledger_policy.rs`): the units and pools the schema declares, with the expiry of
each unit; the limits of units and of accounts; the accounts, each with its header (balance, last
sequence number, the credits of the current calendar periods, the buckets of expiring credit); the
journal of entries; and the policies that bound what a reader sees.

This file has no functions. The decisions are in `LedgerModel.lean`, and the tenant holds these
types in `ModelBase.lean`.

Abstractions, listed in the slice 13 review note: an owner is a person (`PrincipalId`) or a pool
(a number), and an account is named by its owner, unit and scope, where the kernel names it by a hash
of those. Units are numbers, and a time is an integer count of milliseconds, the kernel's `Timestamp`.
The header's `opened_at`, the hash chain (`head_hash`, `previous_hash`, `hash`) and the transaction of
an entry are not held: the model decides no transaction, and the hashes are the kernel's own check.
-/

namespace Strata

/-- A calendar period of the ledger (the kernel's `LedgerPeriod`): the hour, the day, the week starting
    on Monday, or the calendar month, all in UTC. -/
inductive LedgerPeriod where
  | hour
  | day
  | week
  | month
  deriving DecidableEq, Repr

/-- The periods of a ledger, in the order an account header keeps their credits. -/
def LedgerPeriod.all : List LedgerPeriod := [.hour, .day, .week, .month]

/-- The expiry a unit declares (the kernel's `LedgerExpiry`): a bucket of credit lives for `periods` periods
    of the given calendar period after the one it was posted in. The kernel allows one to thirty-six. -/
structure LedgerExpiry where
  period : LedgerPeriod
  periods : Nat
  deriving DecidableEq, Repr

/-- The owner of an account: a person of the tenant, or a pool the schema declares. -/
inductive LedgerOwner where
  | person (p : Nat)
  | pool (n : Nat)
  deriving DecidableEq, Repr

/-- A limit (the kernel's `LedgerLimit`). A limit with no owner is the unit's, and holds for every account of
    the unit; one with an owner is the account's, and holds for the account of that owner and scope. A
    limit names the floor, the cap, the cap per period with its period, and whether that cap counts only
    additions. Each field is set or not; an account's own value overrides the unit's, field by field. -/
structure LedgerLimit where
  unit : Nat
  owner : Option LedgerOwner := none
  scope : String := ""
  floor : Option Int := none
  cap : Option Int := none
  periodCap : Option (Nat × LedgerPeriod) := none
  additionsOnly : Option Bool := none
  deriving DecidableEq, Repr

/-- A unit the schema declares, with its expiry if it has one (the kernel's `LedgerUnitDecl`). -/
structure LedgerUnitDecl where
  unit : Nat
  expiry : Option LedgerExpiry := none
  deriving DecidableEq, Repr

/-- The declarations of a ledger (the kernel's `LedgerDeclarations`): the units, the pools, and the limits of
    units (those with no owner). The schema changes that declare them are not modelled. -/
structure LedgerDecls where
  units : List LedgerUnitDecl := []
  pools : List Nat := []
  limits : List LedgerLimit := []
  deriving DecidableEq, Repr

/-- A bucket of expiring credit (the kernel's `LedgerBucket`): the start of the calendar period it was posted in,
    and the credit left in it. -/
structure LedgerBucket where
  periodStart : Int
  remaining : Nat
  deriving DecidableEq, Repr

/-- The credits an account posted in one calendar period (the kernel's `LedgerPeriodTotal`). -/
structure LedgerPeriodTotal where
  period : LedgerPeriod
  start : Int
  credits : Nat
  deriving DecidableEq, Repr

/-- An account and its header (the kernel's `LedgerAccount`): the owner, the unit and the scope that name it, the
    balance (the sum of its entries), the sequence number of its last entry, the credits of the periods, and
    the buckets of expiring credit (oldest first; empty when there are none). -/
structure LedgerAccount where
  owner : LedgerOwner
  unit : Nat
  scope : String
  balance : Int
  lastSeq : Nat
  periods : List LedgerPeriodTotal
  buckets : List LedgerBucket
  deriving DecidableEq, Repr

/-- One entry of the journal (the kernel's `LedgerEntry`): the account it is in, its number in the account,
    the signed amount, the idempotency key, the reason, the principal that caused it, the time, and the entry it
    corrects. -/
structure LedgerEntry where
  owner : LedgerOwner
  unit : Nat
  scope : String
  sequence : Nat
  amount : Int
  key : String
  reason : String
  causedBy : Option Nat
  time : Int
  corrects : Option Nat
  /-- The transaction the entry is a leg of, as the canonical text of the request (`txnId`); none for a posting.
      The kernel names a transaction by a hash of the same request. -/
  txn : Option String := none
  deriving DecidableEq, Repr

/-- The ledger state of a tenant (the kernel's `LedgerState`): the accounts, the journal, oldest entry first, and
    the limits of accounts. -/
structure LedgerState where
  accounts : List LedgerAccount := []
  journal : List LedgerEntry := []
  limits : List LedgerLimit := []
  deriving DecidableEq, Repr

/-- The ledger event of an accepted posting or expiry entry (the kernel's `LedgerEntryPosted`, with the account
    named by its owner, unit and scope): the amount, the balance before and after it, the entry's number and its
    reason. -/
structure LedgerPosted where
  owner : LedgerOwner
  unit : Nat
  scope : String
  amount : Int
  balance : Int
  balanceBefore : Int
  entrySequence : Nat
  reason : String
  deriving DecidableEq, Repr

/-- One atom of the condition of a ledger policy, over the account the policy reads: the reader is the owner, the
    unit is the number, or the scope is the text. Entry fields are not part of a balance read (the kernel's
    `record_of` without an entry), so a condition over them is not modelled. -/
inductive LedgerAtom where
  | ownerIsCaller
  | unitIs (n : Nat)
  | scopeIs (s : String)
  deriving DecidableEq, Repr

/-- A policy over the ledger (the kernel's `Resource::Ledger` with the action `read`): the role it is granted to,
    the unit it bounds (none for every unit), and its condition, which is the conjunction of its atoms (none is
    always true). The kernel's conditions may also disjoin; the model admits the conjunction only. -/
structure LedgerPolicy where
  role : Nat
  unit : Option Nat := none
  conds : List LedgerAtom := []
  deriving DecidableEq, Repr

end Strata
