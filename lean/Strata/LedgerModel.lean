import Strata.Settings

/-!
# The ledger: postings, expiry and balance reads (slice 13)

The kernel's ledger (`crates/kernel/src/ledger.rs`, `ledger_expiry.rs`, `ledger_policy.rs`). The data are in
`LedgerTypes.lean`; this file has the decisions, each in the kernel's check order:

- `postOne` is one posting (`ledger::post_in`) on the state the postings before it leave: the unit and the owner
  are declared (`notFound`); an adjustment or a reversal to one's own account is refused (`selfAction`); the
  amount is non-zero and within the bound; the key is 1 to 128 bytes and does not use the expiry prefix; the
  reason and the scope are within their bounds (`invalidIntent`); a key already recorded on the account is a
  replay when the amount, reason and correction are the same, and `versionConflict` otherwise; a correction
  names an entry the account holds (`invalidIntent`); the balance stays within the bound (`invalidIntent`); a
  floor or a cap crossed while not moving toward it, and a cap per period exceeded, are `constraintViolated`.
  An accepted posting writes the header and appends the entry, and emits `LedgerEntryPosted`.
- `postLedgerCore` is the intent `PostLedger`: `ledger-post` is held, then one to eight postings, each decided on
  the state the earlier ones leave; one refusal refuses all.
- `expireCore` is the intent `ExpireLedger`: the system or `ledger-configure`; a unit with an expiry; a cutoff
  whose buckets have all ended their life; then, for each account of the unit with a bucket started at or before
  the cutoff (at most 100 accounts), one debit for each such bucket.
- `balance` is the read of one account's balance (`ledger::read_balance`): the reach of the reader (`ledger-read`,
  a ledger policy of a role it holds, or ownership), the unit, a ledger policy that permits the account, and the
  account's header or, for one's own, zero.

The time of an intent is carried in the intent, as the member intents carry theirs (`Members.lean`). Calendar
periods are computed as the kernel computes them, in integers. Abstractions are listed in the slice 13 review
note: the order of the accounts an expiry takes in one transaction, which the kernel takes by the hash of the
account id, and the hash chain of the entries.
-/

namespace Strata

/-- The capability of a direct posting (the kernel's `ledger-post`). -/
def ledgerCapPost : CapabilityId := 3
/-- The capability of reading any account (the kernel's `ledger-read`). -/
def ledgerCapRead : CapabilityId := 4
/-- The capability of declaring units, pools and limits (the kernel's `ledger-configure`). -/
def ledgerCapConfigure : CapabilityId := 6

/-- The largest amount of a posting, and the largest balance: `2^53 - 1` (the kernel's `MAX_LEDGER_AMOUNT`). -/
def ledgerMaxAmount : Int := 9007199254740991
def ledgerMaxKeyBytes : Nat := 128
def ledgerMaxReasonBytes : Nat := 256
def ledgerMaxScopeBytes : Nat := 128
/-- The most postings one `PostLedger` intent names. -/
def ledgerMaxPostings : Nat := 8
/-- The most accounts one `ExpireLedger` intent handles. -/
def ledgerExpiryBatch : Nat := 100
/-- The most buckets an account header keeps. -/
def ledgerMaxBuckets : Nat := 36
/-- The prefix of the key of an expiry entry; an ordinary posting may not use it. -/
def expiryKeyPrefix : String := "expiry:"
/-- The reason of an expiry entry. -/
def expiryReason : String := "expiry"
/-- Milliseconds in an hour and in a day. -/
def hourMs : Int := 3600000
def dayMs : Int := 86400000

/-! ## Calendar periods (LDG-015) -/

/-- Days since 1970-01-01 of a civil date (proleptic Gregorian), after H. Hinnant (the kernel's `days_from_civil`). -/
def daysFromCivil (year month day : Int) : Int :=
  let y := if month ≤ 2 then year - 1 else year
  let era := Int.ediv y 400
  let yoe := y - era * 400
  let mp := Int.emod (month + 9) 12
  let doy := Int.ediv (153 * mp + 2) 5 + day - 1
  let doe := yoe * 365 + Int.ediv yoe 4 - Int.ediv yoe 100 + doy
  era * 146097 + doe - 719468

/-- The civil year and month of a count of days since 1970-01-01 (the kernel's `civil_month`). -/
def civilMonth (days : Int) : Int × Int :=
  let z := days + 719468
  let era := Int.ediv z 146097
  let doe := z - era * 146097
  let yoe := Int.ediv (doe - Int.ediv doe 1460 + Int.ediv doe 36524 - Int.ediv doe 146096) 365
  let doy := doe - (365 * yoe + Int.ediv yoe 4 - Int.ediv yoe 100)
  let mp := Int.ediv (5 * doy + 2) 153
  let month := if mp < 10 then mp + 3 else mp - 9
  let year := yoe + era * 400 + (if month ≤ 2 then 1 else 0)
  (year, month)

/-- The start, in milliseconds, of the calendar period that contains `t` (the kernel's `period_start`). -/
def periodStart : LedgerPeriod → Int → Int
  | .hour, t => Int.ediv t hourMs * hourMs
  | .day, t => Int.ediv t dayMs * dayMs
  | .week, t =>
    let days := Int.ediv t dayMs
    (days - Int.emod (days + 3) 7) * dayMs
  | .month, t =>
    let (y, m) := civilMonth (Int.ediv t dayMs)
    daysFromCivil y m 1 * dayMs

/-- The start of the period `n` periods after the one that starts at `s` (the kernel's `shift_periods`). -/
def shiftPeriods : LedgerPeriod → Int → Int → Int
  | .hour, s, n => s + n * hourMs
  | .day, s, n => s + n * dayMs
  | .week, s, n => s + n * 7 * dayMs
  | .month, s, n =>
    let (y, m) := civilMonth (Int.ediv s dayMs)
    let total := y * 12 + (m - 1) + n
    daysFromCivil (Int.ediv total 12) (Int.emod total 12 + 1) 1 * dayMs

/-- The cutoff of an expiry at time `t`: the start of the bucket whose life has just ended (the kernel's `cutoff`). -/
def expiryCutoff (e : LedgerExpiry) (t : Int) : Int :=
  shiftPeriods e.period (periodStart e.period t) (-(e.periods : Int))

/-! ## Buckets of expiring credit (LDG-1545) -/

/-- Adds `credit` to the bucket that starts at `start`, or opens it, keeping the buckets in order of their start. -/
def insertBucket (bs : List LedgerBucket) (start : Int) (credit : Nat) : List LedgerBucket :=
  if bs.any (fun b => b.periodStart == start) then
    bs.map (fun b => if b.periodStart == start then { b with remaining := b.remaining + credit } else b)
  else
    (bs ++ [({ periodStart := start, remaining := credit } : LedgerBucket)]).mergeSort
      (fun a b => decide (a.periodStart ≤ b.periodStart))

/-- At most `ledgerMaxBuckets` buckets: when there are more, the two oldest become one, dated by the newer. -/
def trimBuckets (bs : List LedgerBucket) : List LedgerBucket :=
  match bs with
  | o :: b :: rest =>
    if ledgerMaxBuckets < bs.length then { b with remaining := b.remaining + o.remaining } :: rest else bs
  | _ => bs

/-- A debit takes `left` from the buckets, oldest first (the kernel's loop in `apply_entry`). -/
def consumeOldest (bs : List LedgerBucket) (left : Nat) : List LedgerBucket :=
  match bs with
  | [] => []
  | b :: rest =>
    let taken := min left b.remaining
    { b with remaining := b.remaining - taken } :: consumeOldest rest (left - taken)

/-- The buckets of an account after an entry of `amount` at time `t`, with `before` the balance before it (the
    kernel's `apply_entry`): a credit that is not paid to a debt adds to the bucket of its period; a debit consumes
    the oldest credit first. Buckets left at zero are dropped. -/
def applyEntry (bs : List LedgerBucket) (before amount : Int) (t : Int) (e : LedgerExpiry) : List LedgerBucket :=
  if 0 < amount then
    let debt : Nat := if before < 0 then before.natAbs else 0
    let credit : Nat := amount.toNat - debt
    if 0 < credit then trimBuckets (insertBucket bs (periodStart e.period t) credit) else bs
  else
    (consumeOldest bs amount.natAbs).filter (fun b => 0 < b.remaining)

/-! ## Declarations and limits (LDG-011 to LDG-014) -/

/-- The expiry of a unit the schema declares, if it declares one. -/
def unitDecl? (d : LedgerDecls) (u : Nat) : Option LedgerUnitDecl :=
  d.units.find? (fun x => x.unit == u)

def Tenant.unitDeclared (t : Tenant) (u : Nat) : Bool :=
  match t.ledgerDecls with
  | some d => (unitDecl? d u).isSome
  | none => false

def Tenant.poolDeclared (t : Tenant) (n : Nat) : Bool :=
  match t.ledgerDecls with
  | some d => d.pools.contains n
  | none => false

/-- The expiry a unit declares (the kernel's `expiry_of`). -/
def Tenant.expiryOf (t : Tenant) (u : Nat) : Option LedgerExpiry :=
  match t.ledgerDecls with
  | some d => (unitDecl? d u).bind (fun x => x.expiry)
  | none => none

/-- Whether an owner exists: a member of the tenant for a person, a declared pool for a pool. -/
def Tenant.ownerOk (t : Tenant) : LedgerOwner → Bool
  | .person p => (t.members.map Prod.fst).contains p
  | .pool n => t.poolDeclared n

/-- The account limit of an owner, unit and scope the state holds, if there is one. -/
def accountLimit? (L : LedgerState) (o : LedgerOwner) (u : Nat) (s : String) : Option LedgerLimit :=
  L.limits.find? (fun l => l.unit == u && l.owner == some o && l.scope == s)

/-- The unit's limit, which holds for every account of the unit that does not set the field. -/
def unitLimit? (d : LedgerDecls) (u : Nat) : Option LedgerLimit :=
  d.limits.find? (fun l => l.unit == u && l.owner.isNone)

/-- The limits that apply to one account (the kernel's `Limits`): each field of the account's own limit, else the
    unit's. The cap per period and its flag come from the account's limit when it sets a cap per period, and
    from the unit's otherwise. -/
structure AppliedLimits where
  floor : Option Int
  cap : Option Int
  periodCap : Option (Nat × LedgerPeriod)
  additionsOnly : Bool
  deriving DecidableEq, Repr

def optOr {α : Type} (a b : Option α) : Option α :=
  match a with
  | some x => some x
  | none => b

def applyingLimits (t : Tenant) (ofAcc : Option LedgerLimit) (u : Nat) : AppliedLimits :=
  match t.ledgerDecls with
  | none => { floor := none, cap := none, periodCap := none, additionsOnly := false }
  | some d =>
    let ofUnit := unitLimit? d u
    let periodSource : Option LedgerLimit :=
      if (ofAcc.bind (fun l => l.periodCap)).isSome then ofAcc else ofUnit
    { floor := optOr (ofAcc.bind (fun l => l.floor)) (ofUnit.bind (fun l => l.floor)),
      cap := optOr (ofAcc.bind (fun l => l.cap)) (ofUnit.bind (fun l => l.cap)),
      periodCap := optOr (ofAcc.bind (fun l => l.periodCap)) (ofUnit.bind (fun l => l.periodCap)),
      additionsOnly := (periodSource.bind (fun l => l.additionsOnly)) == some true }

/-! ## Postings (CT-280) -/

/-- One posting of a `PostLedger` intent (the kernel's `LedgerPostingRequest`). -/
structure LedgerPosting where
  owner : LedgerOwner
  unit : Nat
  amount : Int
  key : String
  reason : String
  corrects : Option Nat := none
  scope : String := ""
  deriving DecidableEq, Repr

def LedgerAccount.is (a : LedgerAccount) (o : LedgerOwner) (u : Nat) (s : String) : Bool :=
  a.owner == o && a.unit == u && a.scope == s

def LedgerEntry.inAccount (e : LedgerEntry) (o : LedgerOwner) (u : Nat) (s : String) : Bool :=
  e.owner == o && e.unit == u && e.scope == s

/-- The credits of the calendar period of `p` that contains `t`, in the header's periods (the kernel's `credits_in`). -/
def creditsIn (ps : List LedgerPeriodTotal) (p : LedgerPeriod) (t : Int) : Nat :=
  ((ps.find? (fun x => x.period == p && x.start == periodStart p t)).map (fun x => x.credits)).getD 0

def actorUser (p : Principal) : Option Nat := p.userId?

/-- One posting on the state `L` (the kernel's `post_in`). Returns the new ledger state and the events it emits: a
    replay changes nothing and emits none; an accepted posting emits one `LedgerEntryPosted`. -/
def postOne (t : Tenant) (actor : Principal) (now : Int) (L : LedgerState) (q : LedgerPosting) :
    Except Err (LedgerState × List LedgerPosted) :=
  let adjusts : Bool :=
    match q.owner, actor with
    | .person p, .user u => p == u && (q.reason == "adjust" || q.reason == "reverse")
    | _, _ => false
  if !t.unitDeclared q.unit then .error .notFound
  else if !t.ownerOk q.owner then .error .notFound
  else if adjusts then .error .selfAction
  else if q.amount == 0 || decide (ledgerMaxAmount < q.amount) || decide (q.amount < -ledgerMaxAmount) then
    .error .invalidIntent
  else if q.key.utf8ByteSize == 0 || decide (ledgerMaxKeyBytes < q.key.utf8ByteSize) then .error .invalidIntent
  else if q.key.startsWith expiryKeyPrefix then .error .invalidIntent
  else if decide (ledgerMaxReasonBytes < q.reason.utf8ByteSize) then .error .invalidIntent
  else if decide (ledgerMaxScopeBytes < q.scope.utf8ByteSize) then .error .invalidIntent
  else
    let cur : Option LedgerAccount := L.accounts.find? (fun a => a.is q.owner q.unit q.scope)
    let recorded : Option LedgerEntry :=
      L.journal.find? (fun e => e.inAccount q.owner q.unit q.scope && e.key == q.key)
    match recorded with
    | some e =>
      if e.amount == q.amount && e.reason == q.reason && e.corrects == q.corrects then .ok (L, [])
      else .error .versionConflict
    | none =>
      let before : Int := (cur.map (fun a => a.balance)).getD 0
      let last : Nat := (cur.map (fun a => a.lastSeq)).getD 0
      let curPeriods : List LedgerPeriodTotal := (cur.map (fun a => a.periods)).getD []
      let curBuckets : List LedgerBucket := (cur.map (fun a => a.buckets)).getD []
      let badCorrection : Bool :=
        match q.corrects with
        | some c => c == 0 || decide (last < c)
        | none => false
      if badCorrection then .error .invalidIntent
      else
        let after : Int := before + q.amount
        if decide (ledgerMaxAmount < after) || decide (after < -ledgerMaxAmount) then .error .invalidIntent
        else
          let applying := applyingLimits t (accountLimit? L q.owner q.unit q.scope) q.unit
          let belowFloor : Bool :=
            match applying.floor with
            | some f => decide (after < f) && !(decide (before < f) && decide (0 < q.amount))
            | none => false
          let aboveCap : Bool :=
            match applying.cap with
            | some c => decide (c < after) && !(decide (c < before) && decide (q.amount < 0))
            | none => false
          if belowFloor || aboveCap then .error .constraintViolated
          else
            -- A reversal counts toward the cap per period only when the cap counts every addition.
            let counts : Bool := decide (0 < q.amount) && !(applying.additionsOnly && q.corrects.isSome)
            let overPeriod : Bool :=
              match applying.periodCap with
              | some (cap, p) => counts && decide (cap < creditsIn curPeriods p now + q.amount.toNat)
              | none => false
            if overPeriod then .error .constraintViolated
            else
              let periods : List LedgerPeriodTotal :=
                LedgerPeriod.all.map (fun p =>
                  { period := p, start := periodStart p now,
                    credits := creditsIn curPeriods p now + (if counts then q.amount.toNat else 0) })
              let buckets : List LedgerBucket :=
                match t.expiryOf q.unit with
                | some e => applyEntry curBuckets before q.amount now e
                | none => curBuckets
              let acct : LedgerAccount :=
                { owner := q.owner, unit := q.unit, scope := q.scope, balance := after,
                  lastSeq := last + 1, periods := periods, buckets := buckets }
              let entry : LedgerEntry :=
                { owner := q.owner, unit := q.unit, scope := q.scope, sequence := last + 1,
                  amount := q.amount, key := q.key, reason := q.reason, causedBy := actorUser actor,
                  time := now, corrects := q.corrects }
              let ev : LedgerPosted :=
                { owner := q.owner, unit := q.unit, scope := q.scope, amount := q.amount, balance := after, balanceBefore := before, entrySequence := last + 1, reason := q.reason }
              let accounts : List LedgerAccount :=
                if cur.isSome then L.accounts.map (fun a => if a.is q.owner q.unit q.scope then acct else a)
                else L.accounts ++ [acct]
              .ok ({ L with accounts := accounts, journal := L.journal ++ [entry] }, [ev])

/-- The intent `PostLedger` (CT-281): the caller holds `ledger-post`; one to eight postings, each on the state the
    earlier ones leave; the first refusal refuses all. Returns the ledger and the events in order. -/
def postLedgerCore (t : Tenant) (actor : Principal) (now : Int) (ps : List LedgerPosting) :
    Except Err (LedgerState × List LedgerPosted) :=
  if !t.holdsCap actor ledgerCapPost then .error .unauthorized
  else if ps.isEmpty || decide (ledgerMaxPostings < ps.length) then .error .invalidIntent
  else
    List.foldlM (fun (acc : LedgerState × List LedgerPosted) (q : LedgerPosting) =>
        (postOne t actor now acc.1 q).map (fun r => (r.1, acc.2 ++ r.2)))
      (t.ledger, []) ps

/-! ## Expiry (LDG-1545 to LDG-1547) -/

/-- The start of the oldest bucket of an account: what the due accounts are ordered and filtered by. -/
def oldestBucketStart? (a : LedgerAccount) : Option Int :=
  (a.buckets.head?).map (fun b => b.periodStart)

/-- The accounts of `unit` whose oldest bucket started at or before `cutoff`, in order of that start, at most
    `ledgerExpiryBatch` of them. Ties keep the order of the accounts. -/
def dueAccounts (accts : List LedgerAccount) (unit : Nat) (cutoff : Int) : List LedgerAccount :=
  let keyed : List (Int × LedgerAccount) :=
    (accts.filter (fun a => a.unit == unit)).filterMap (fun a =>
      match oldestBucketStart? a with
      | some s => if s ≤ cutoff then some (s, a) else none
      | none => none)
  ((keyed.mergeSort (fun x y => decide (x.1 ≤ y.1))).map (fun x => x.2)).take ledgerExpiryBatch

/-- The debit of every bucket of one account that started at or before `cutoff`, oldest first. Each debit takes the
    remaining credit of its bucket, and the entry is keyed by the bucket's start. Returns the account's new header
    and the entries and events it writes, in order. -/
def expireAccount (e : LedgerExpiry) (t cutoff : Int) (a : LedgerAccount) :
    LedgerAccount × List (LedgerEntry × LedgerPosted) :=
  let due : List LedgerBucket :=
    a.buckets.filter (fun b => decide (b.periodStart ≤ cutoff) && decide (0 < b.remaining))
  due.foldl (fun (acc : LedgerAccount × List (LedgerEntry × LedgerPosted)) (b : LedgerBucket) =>
      let h := acc.1
      let amount : Int := -(b.remaining : Int)
      let before : Int := h.balance
      let after : Int := before + amount
      let nextBuckets := applyEntry h.buckets before amount t e
      let h' : LedgerAccount :=
        { h with balance := after, lastSeq := h.lastSeq + 1, buckets := nextBuckets }
      let entry : LedgerEntry :=
        { owner := h.owner, unit := h.unit, scope := h.scope, sequence := h'.lastSeq, amount := amount,
          key := expiryKeyPrefix ++ toString b.periodStart, reason := expiryReason, causedBy := none,
          time := t, corrects := none }
      let ev : LedgerPosted :=
        { owner := h.owner, unit := h.unit, scope := h.scope, amount := amount, balance := after, balanceBefore := before, entrySequence := h'.lastSeq, reason := expiryReason }
      (h', acc.2 ++ [(entry, ev)]))
    (a, [])

/-- The intent `ExpireLedger` (CT-812): the system or a holder of `ledger-configure`; the unit declares an expiry;
    the cutoff is at most the latest that has ended its life at `t`; then each due account is expired. -/
def expireCore (t : Tenant) (actor : Principal) (now : Int) (unit : Nat) (cutoffAt : Int) :
    Except Err (LedgerState × List LedgerPosted) :=
  if !(actor == .system || t.holdsCap actor ledgerCapConfigure) then .error .unauthorized
  else match t.expiryOf unit with
  | none => .error .invalidIntent
  | some e =>
    if decide (expiryCutoff e now < cutoffAt) then .error .invalidIntent
    else
      let L := t.ledger
      let due : List LedgerAccount := dueAccounts L.accounts unit cutoffAt
      let results : List (LedgerAccount × List (LedgerEntry × LedgerPosted)) :=
        due.map (fun a => expireAccount e now cutoffAt a)
      let headers : List LedgerAccount := results.map (fun r => r.1)
      let added : List (LedgerEntry × LedgerPosted) := results.flatMap (fun r => r.2)
      let accounts : List LedgerAccount :=
        L.accounts.map (fun a =>
          match headers.find? (fun h => h.is a.owner a.unit a.scope) with
          | some h => h
          | none => a)
      .ok ({ L with accounts := accounts, journal := L.journal ++ added.map (fun x => x.1) },
        added.map (fun x => x.2))

/-! ## Reads of the balance (LDG-026, LDG-1540, LDG-1541) -/

/-- How far a reader reaches: every account, or only what the ledger policies permit. -/
inductive LedgerReach where
  | everything
  | policies
  deriving DecidableEq, Repr

/-- A person owns an account of its own id. -/
def ownsAccount (p : Principal) (o : LedgerOwner) : Bool :=
  match p, o with
  | .user u, .person v => u == v
  | _, _ => false

/-- A ledger policy binds a reader when it holds the policy's role (the kernel's `bounded`). -/
def Tenant.ledgerBounded (t : Tenant) (p : Principal) : Bool :=
  let roles := t.rolesOf p
  t.ledgerPolicies.any (fun pol => roles.contains pol.role)

/-- The reach of a reader over the account of `o` (the kernel's `may_read`). -/
def mayReadLedger (t : Tenant) (p : Principal) (o : LedgerOwner) : Except Err LedgerReach :=
  let bounded := t.ledgerBounded p
  if t.holdsCap p ledgerCapRead then .ok (if bounded then .policies else .everything)
  else if bounded then .ok .policies
  else if ownsAccount p o then .ok .everything
  else .error .unauthorized

/-- An atom of a ledger policy's condition holds on the account it reads (the kernel's evaluation over `record_of`). -/
def LedgerAtom.holds (c : LedgerAtom) (p : Principal) (o : LedgerOwner) (u : Nat) (s : String) : Bool :=
  match c with
  | .ownerIsCaller => ownsAccount p o
  | .unitIs n => u == n
  | .scopeIs x => s == x

/-- Whether the reader may see the account's balance under the ledger policies (the kernel's `account_permitted`):
    its owner, or a policy of a role it holds that names the unit or every unit and whose condition holds. -/
def Tenant.ledgerPermits (t : Tenant) (p : Principal) (o : LedgerOwner) (u : Nat) (s : String) : Bool :=
  let roles := t.rolesOf p
  ownsAccount p o ||
    t.ledgerPolicies.any (fun pol =>
      roles.contains pol.role && (pol.unit.isNone || pol.unit == some u) && pol.conds.all (fun c => c.holds p o u s))

/-- The balance of an account (the kernel's `read_balance`): the reach, then the unit, then the policies, then the
    header. For one's own account (`own`), an account that does not exist reads as zero at sequence zero. -/
def readLedgerBalance (t : Tenant) (p : Principal) (o : LedgerOwner) (u : Nat) (s : String) (own : Bool)
    (L : LedgerState) : Except Err (Int × Nat) :=
  match mayReadLedger t p o with
  | .error e => .error e
  | .ok reach =>
    if !t.unitDeclared u then .error .notFound
    else if reach == .policies && !t.ledgerPermits p o u s then .error .notFound
    else match L.accounts.find? (fun a => a.is o u s) with
      | some a => .ok (a.balance, a.lastSeq)
      | none => if own then .ok (0, 0) else .error .notFound

/-! ## The tenant's transition for the ledger intents -/

/-- The tenant after a `PostLedger` intent that is accepted: its ledger changes, nothing else. -/
def postLedgerT (t : Tenant) (p : Principal) (now : Int) (ps : List LedgerPosting) : Except Err Tenant :=
  (postLedgerCore t p now ps).map (fun r => { t with ledger := r.1 })

/-- The tenant after an `ExpireLedger` intent that is accepted: its ledger changes, nothing else. -/
def expireLedgerT (t : Tenant) (p : Principal) (now : Int) (u : Nat) (c : Int) : Except Err Tenant :=
  (expireCore t p now u c).map (fun r => { t with ledger := r.1 })

/-- The events a `PostLedger` intent emits, in order (empty when it is refused). -/
def postLedgerEvents (t : Tenant) (p : Principal) (now : Int) (ps : List LedgerPosting) : List LedgerPosted :=
  match postLedgerCore t p now ps with
  | .ok r => r.2
  | .error _ => []

/-- The events an `ExpireLedger` intent emits, in order (empty when it is refused). -/
def expireLedgerEvents (t : Tenant) (p : Principal) (now : Int) (u : Nat) (c : Int) : List LedgerPosted :=
  match expireCore t p now u c with
  | .ok r => r.2
  | .error _ => []

end Strata

namespace Strata

/-- An accepted `PostLedger` changes the tenant's ledger and nothing else of it. -/
theorem postLedgerT_ok_shape {t t' : Tenant} {p : Principal} {now : Int} {ps : List LedgerPosting}
    (h : postLedgerT t p now ps = .ok t') : ∃ L, t' = { t with ledger := L } := by
  unfold postLedgerT at h
  cases hc : postLedgerCore t p now ps with
  | error e => simp [hc, Except.map] at h
  | ok r =>
    simp only [hc, Except.map, Except.ok.injEq] at h
    exact ⟨r.1, h.symm⟩

/-- An accepted `ExpireLedger` changes the tenant's ledger and nothing else of it. -/
theorem expireLedgerT_ok_shape {t t' : Tenant} {p : Principal} {now : Int} {u : Nat} {c : Int}
    (h : expireLedgerT t p now u c = .ok t') : ∃ L, t' = { t with ledger := L } := by
  unfold expireLedgerT at h
  cases hc : expireCore t p now u c with
  | error e => simp [hc, Except.map] at h
  | ok r =>
    simp only [hc, Except.map, Except.ok.injEq] at h
    exact ⟨r.1, h.symm⟩

end Strata

namespace Strata

/-- An accepted `PostLedger` is by a holder of `ledger-post` (the intent's permission, `authorized`). -/
theorem postLedgerT_ok_auth {t t' : Tenant} {p : Principal} {now : Int} {ps : List LedgerPosting}
    (h : postLedgerT t p now ps = .ok t') : t.holdsCap p ledgerCapPost = true := by
  unfold postLedgerT postLedgerCore at h
  by_cases hc : t.holdsCap p ledgerCapPost = true
  · exact hc
  · have hf : t.holdsCap p ledgerCapPost = false := by simpa using hc
    simp [hf, Except.map] at h

/-- An accepted `ExpireLedger` is by the system or a holder of `ledger-configure` (the intent's permission). -/
theorem expireLedgerT_ok_auth {t t' : Tenant} {p : Principal} {now : Int} {u : Nat} {c : Int}
    (h : expireLedgerT t p now u c = .ok t') : (p == .system || t.holdsCap p ledgerCapConfigure) = true := by
  unfold expireLedgerT expireCore at h
  by_cases hc : (p == .system || t.holdsCap p ledgerCapConfigure) = true
  · exact hc
  · have hf : (p == .system || t.holdsCap p ledgerCapConfigure) = false := by simpa using hc
    simp [hf, Except.map] at h

end Strata
