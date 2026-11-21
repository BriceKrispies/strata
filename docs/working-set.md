# The tenant working set

What a Strata runtime should hold in memory in front of its store, how it decides, and exactly when
holding it changes no answer. This is a design, with its safety properties proved in Lean
(`lean/Strata/WorkingSet.lean`). Nothing here is implemented yet; the follow-up work is listed at
the end, ordered by what unblocks what. The design has been revised after four adversarial
reviews; the answers are collected in the last section, and everything above it is the current
design.

## The problem

The owner's request: make the runtime load as much of the tenant database as it can into memory,
chosen by sound algorithms rather than blindly; consider one runtime per tenant; make it testable
end to end with the enterprise corpus and the simulator; prove it against the kernel model.

Restated as an engineering problem: a cell process serves many tenants from their stores. Every
request today goes to the store, and the store decodes what it returns on every call. Memory is
finite and shared. Choose what to keep decoded in memory so that the expected cost of going to the
store is as small as possible, within an accounted budget that is never exceeded, without ever
committing a change the store would not, and with a stated, checkable condition under which a read
never serves a value the store would not serve.

## Where the framing should change

The request is the right direction. Several parts of it, taken literally, would make the design
worse. Plainly:

1. **One runtime per tenant: as a unit of ownership always, as a process only where it pays.** The
   specification permits a process per tenant: a cell is "a process or host that serves a subset of
   the tenants" (D-8300), and a tenant that needs a performance guarantee is served by a cell of its
   own (STO-016, D-8304). It does not require it, and as the default it costs more than it buys: the
   fixed cost of a process (the async runtime, connection pools, the compiled platform code, the key
   service session) is paid per tenant, most tenants of a multi-tenant platform are small and idle
   most of the time, and memory held by an idle tenant's process cannot go to a busy neighbour. My
   recommendation is a **tenant runtime object** in every case, the single owner of the tenant's
   working set and commit path inside its cell, and a process of its own only for the tenants
   placed in a dedicated cell.

2. **"Load as much as possible" is the wrong objective; "minimise the cost of misses within a
   budget" is the right one.** Memory in a cell is shared, so a byte held for one tenant is a byte
   not held for another. Loading everything is optimal only when everything fits and everything is
   used; the design reaches that state by itself when it holds (a whole object type becomes
   resident when it is hot and fits) and degrades gracefully when it does not. A tenant larger than
   its share, or a cell restart that loads every tenant at once, is where "as much as possible"
   goes wrong.

3. **A cell budget divided by marginal value beats fixed per-tenant budgets.** A static split
   wastes memory on idle tenants. A pure global pool lets one tenant's scan evict everyone else;
   the specification bounds noisy neighbours with quotas, cell process limits and dedicated cells
   (D-8304, STO-013, BIL-150) but sets no per-tenant memory quota, so the per-tenant memory ceiling
   below is a **new decision to propose**, not an existing rule.

4. **Heaps are an implementation detail; the unit of residency, the coherence key and the cost
   model are the decisions.** GreedyDual-Size-Frequency is naturally a priority queue and a good
   candidate, but a wrong unit or a wrong coherence key with a perfect heap still loses, and the
   review showed the coherence key is where the first revision was wrong.

5. **The largest saving is probably the header, not the records.** Every `load` on SQLite re-reads
   the tenant row and decodes the whole schema and member list from JSON
   (`crates/storage-sqlite/src/lib.rs:764-797`), and one request loads several times
   (`crates/storage-api/src/lib.rs:1924-1964`, `crates/backend/src/ops.rs:496-537`). The
   specification requires a compiled schema to be reused while the schema version is unchanged
   (CPY-014); nothing does. Build and measure this first; it may be most of the win.

6. **On SQLite the store is already partly in memory.** The page cache holds recently read pages,
   and up to 256 tenant databases stay open (`DEFAULT_MAX_OPEN`, `crates/storage-sqlite/src/lib.rs:704`).
   A resident entry saves decryption (SQLCipher, D-8305), JSON decoding, opening sealed fields
   (D-350) and the query, not a disk read. On PostgreSQL cells a miss is a round trip to the
   database server in a REPEATABLE READ transaction (`crates/storage-postgres/src/lib.rs:26-28`),
   which is the case where residency saves the most. Miss cost is measured per backend and per kind
   of key, never assumed.

7. **"Fully test it with the corpus" needs things that do not exist.** The corpus is tenant
   metadata, 1,290 entities and no records (`tests/corpus/enterprise/README.md`), and nothing loads
   it into a tenant. The simulator runs `World::step` over complete in-memory state
   (`crates/simulation/src/engine.rs:1-6`, `crates/kernel/src/world.rs:1-2`), single-threaded and
   logical, with no store, no other writer, no restore and no key destruction, so it cannot observe
   a working set or refute a coherence claim. The experiments section says what has to be added.

## The current state

| What | Where | What it does |
|---|---|---|
| The store contract | `crates/storage-api/src/lib.rs:780-904` | `load(tenant, needs)` returns a partial snapshot whose coverage states exactly the needs; `load_complete`; `commit` guarded by the base revision (lines 10-37 state the guarantees) |
| Revision can repeat | `crates/storage-api/src/lib.rs:800-808`; `memory.rs:1573-1607`, `storage-sqlite/src/lib.rs:4850-4860` | A restored snapshot sets the tenant revision back; this is why `object_revisions` exists (D-5064). In `MemoryStore` the per-type counters add an epoch raised on restore |
| Header always included | `crates/storage-api/src/lib.rs:1676` (`partial_state`) | Every load carries schema, members, counters, setting values |
| Coverage | `crates/kernel/src/state.rs:65-90` | A snapshot answers a need if it loaded it or `AllOf` its type; otherwise the kernel refuses with `InsufficientState` (D-2, K7) |
| The command path | `crates/storage-api/src/lib.rs:1876-2030` (`execute_guarded`) | Load header, compute needs, load, refine, decide, commit guarded by base revision; on conflict decide again, up to `MAX_CONFLICT_RETRIES` (8, line 1797) |
| The read path | `crates/backend/src/ops.rs:31` (`header`), `:496` (`read_loaded`), `view_load.rs:29` | Header, then the read's needs, then what the kernel reports missing, up to 32 rounds |
| Whole-tenant loads | `crates/backend/src/lab.rs:530`, `products.rs:139`, `product_segments.rs:88`, `subject_export.rs:458` | `load_complete` |
| Writes outside commits | `storage-sqlite/src/lib.rs:4583-4590` (`anonymise_ledger_principal`), `restore_snapshot`, `reset_tenant`, provisioning (directory state); `memory.rs:1335-1345` (mutates in place) | Change what a load returns without a commit or a revision bump |
| Summary read | `crates/storage-sqlite/src/lib.rs:2740-2758` (`tenant_summary`) | Calls `header()` (full schema decode) and `COUNT(*)`: no cheap way to read a tenant's revision exists |
| Existing in-process copies | `crates/backend/src/aggregates.rs:1-35`, `settings.rs:37-40` (`PlatformCache`) | Aggregate results keyed by object-type counters; platform settings for a bounded age |
| Open tenant files | `crates/storage-sqlite/src/lib.rs:704`, `:1694-1770` | An LRU of open databases by count |
| Sealing | `crates/storage-api/src/sealed.rs:1-35`, `:212-217` | Personal fields sealed on write, opened on every load; the cipher session caches nothing so that a destroyed key opens nothing later |
| The simulator | `crates/simulation/src/lib.rs`, `engine.rs:248-300` | Logical time over `World::step`; no store, no loads, no memory |

There is no tenant state cache today.

## Constraints

- **The kernel stays pure** (K1). The working set is shell, behind the `Store` trait. The kernel
  refuses to decide on a snapshot that does not cover its needs (D-2, K7), so honest coverage can
  cause a miss but never a wrong decision; it does not protect against stale values, which is the
  coherence rule's job.
- **The store is the authority.** Commits are serialised per tenant by the base-revision check (S2).
- **The simulator runs what the deployment runs**, so the working set is part of the store path.
- **Copies have rules** (CPY-001 to CPY-014): tenant-first keys, supersession on commit, an outage
  is a slowdown never an error (CPY-011), concurrent misses compute once (CPY-012), erasure reaches
  copies (CPY-013). The working set is a new copy class and needs a row in the copy class table.
- **Erasure is by key destruction** (D-350): destroying a subject key changes no stored record or
  header and raises no generation, and from then on nothing opens.

## Candidate designs

**A. Whole-tenant residency.** Each tenant runtime holds its complete `TenantState`, applies its own
commits with `apply_delta`, and the cell evicts whole tenants by recency. Simplest coherence and no
need rounds; but every cold open is a full load, a tenant larger than its share falls from all hits
to all misses, and memory scales with data rather than use.

**B. Record and answer working set per tenant, fixed budget.** Header pinned; records (present and
absent) and complete need answers, each stamped; evicted by a cost- and size-aware policy within a
fixed per-tenant budget. Memory follows use and large tenants degrade gradually; fixed budgets
waste memory on idle tenants.

**C. B with a cell arbiter (recommended).** As B, with budgets moved between tenants by the marginal
value of a byte, inside a floor (the tenant's pinned bytes) and a proposed ceiling. A reaches its
best case inside C when an object type or a whole tenant is hot and fits.

**D. An external shared copy store (rejected for this layer).** A network hop per decision, a
second cross-process coherence protocol, and decoded tenant data outside the cell. The shared copy
store of D-17 is for view results keyed by versions.

### Policies weighed

| Policy | Strength | Weakness for Strata |
|---|---|---|
| LRU / CLOCK | Simple, cheap, lock-friendly | Size- and cost-blind; one `AllOf` scan flushes the set |
| 2Q / ARC | Scan-resistant, adaptive | Size- and cost-blind; ghost lists cost memory per tenant |
| LFU with decay | Stable popularity | Slow to adapt; cost- and size-blind |
| GreedyDual-Size(-Frequency) | Priority `L + f * c / s` with inflation `L`; cost- and size-aware | Needs a measured cost per key kind; a heap with lazy deletion |
| Knapsack admission (value per byte) | The right objective | 0/1 knapsack is NP-hard; greedy by density is optimal only fractionally |
| W-TinyLFU admission | A small sketch refuses one-hit wonders before they evict anything | By frequency alone it can refuse a rare, expensive, small entry GDSF would keep |
| Structural pinning | The header and what every decision reads are never evicted | Pinned bytes are a floor the budget must cover |

Recommendation: pin the structure; admit by comparing the candidate's GreedyDual priority, computed
with the TinyLFU sketch's frequency estimate, against the victim's (so the sketch supplies the
frequency and the admission test is cost- and size-aware, which answers the rare-expensive-entry
objection); evict by GreedyDual-Size-Frequency. CLOCK is the baseline every experiment must beat.

## The recommended design

### Where it sits: below sealing

A `ResidentStore` implements `Store` and wraps the **adapter**, underneath `SealedStore`:

```
backend / simulator  ->  SealedStore (opens on every load)  ->  ResidentStore  ->  SQLite | PostgreSQL | memory
```

Resident entries hold what the adapter returns to a load: sealed tokens for personal fields and
blind hashes in answer keys over searchable personal fields. **Key documents are never resident.**
The cipher session reads the tenant-key document and the subject-key documents it needs from the
store on every call (`subject_keys.rs:292-316`) and keeps no unwrapped key beyond the call
(`sealed.rs:212-217`); the working set does not cache those reads. So a key destroyed by any
process opens nothing from the next call on, in every cell, with no event, no lease renewal and no
bound to state: CPY-013 holds for resident tokens as it holds for stored ones.

Key documents are not load results, so writing them raises no generation: neither the key creation
`seal_commit` does on demand inside a commit (`sealed.rs:690-700`, `:930`) nor a destruction
(`subject_keys.rs:255-270`). A commit that names a new data subject therefore cannot conflict with
its own key write. Nothing resident depends on a key document, so nothing needs invalidating when
one changes.

**What a hit costs.** For a tenant without personal fields a hit is memory only. For a tenant with
sealed fields a hit still makes the key session's store reads (the tenant-key document, and one
subject-key document per subject of the sealed fields served), one unwrap of the tenant key (local
with the in-process key service, a remote call with a managed one) and one open per sealed field
served. That is a store round trip on every request of such a tenant, cheaper than a miss (no
records, no schema decode) but not free; the cost runs measure it. Correct erasure is worth more
than the saved read.

**Excluded from residency.** A lookup of a searchable personal field also matches the plain value
for rows written before sealing (`sealed.rs:20-25`), so its need names a plaintext personal value.
That applies to every need kind that sealing translates: `ByField`, `ByFields`, `IndexPage` and
`PairPage` (`sealed.rs:772-880`). The rule is decided with the personal-field plan sealing already
builds for the tenant: a need whose fields include a personal field is resident only in its
translated, blind-hash form, and the legacy plain-value part is never an answer key and never in
the hot-set manifest; it is answered from the store every time. Blind hashes are keyed by the tenant
key, and no tenant-key rotation exists today, so blind-hash keys are stable; adding tenant-key
rotation would have to drop or re-key those answers.

Placing the working set above sealing is rejected: it would hold opened values, and a key
destruction raises no generation, so an opened entry stays current and goes on being served after
its key is destroyed. The sealing model in the Lean file shows both placements on the same
schedules, with stamped entries, value writes that raise the generation, and key destructions that
do not (`below_sealing_erasure_safe`, `above_sealing_serves_erased`).

### What can be resident

| Key | Value | Pinned | Notes |
|---|---|---|---|
| `header(tenant)` | schema, members, name, setting values | yes | Without counters. CPY-014's compiled schema hangs off it, keyed by schema version and schema hash |
| `counters(tenant)` | revision, record counter, event sequence | no | Changed by every commit; taken from the generation read or the commit receipt |
| `record(tenant, object, id)` | the record, or its absence | no | |
| `answer(tenant, need)` | complete id set for `ByField`, `ByFields`, `IndexPage`, `PairPage` | no | Valid only as a complete set; legacy plain-value personal lookups excluded |
| `page(tenant, object, after, limit)` | `PageOf`: up to `limit` ids above `after` | no | Ordered; not invalidated by updates |
| `all(tenant, object)` | every id of the type | no | `AllOf` |
| `ledger(tenant, need)` | ledger state for a `Need::Ledger` | no | Ledger entries change outside commits (anonymisation), which raises the generation |
| derived structures | indexes, reference closures, policy dependencies | by choice | Pinned when measured access says every decision reads them |

### The tenant generation (store contract, follow-up item 1)

The revision is not safe to key on: a restored snapshot sets it back
(`memory.rs:1573-1607`, `storage-sqlite/src/lib.rs:4850-4860`), the store contract says it can
repeat (`lib.rs:800-808`), a tenant deleted and created again starts over at 1, and a tenant file
can be restored from outside the cell (OPS-3121, D-8300).

The generation lives entirely in the tenant's own storage, in a small row of its own (not in the
tenant row beside the schema and member JSON, so reading it is a constant-cost read):

```rust
/// Changes, to a value never used before, with every change to what a load returns.
pub struct Generation {
    /// Random, 128 bits. Drawn only at events the control plane controls (below).
    pub nonce: u128,
    /// Raised by one in every write transaction that changes what a load returns. Never reset:
    /// a nonce rotation leaves it as it is.
    pub counter: u64,
}
```

- **The nonce is rotated only at control-plane events**, each a write by the process the control
  plane has chosen: creating the tenant; acquiring its lease; `restore_snapshot`; a point-in-time
  restore by the archive tooling (after the file is in place, before the tenant is routed); an
  import on a move (D-8302). **Opening a handle rotates nothing**: `tenant_db`'s cold open is also
  the path after an LRU handle eviction (`storage-sqlite/src/lib.rs:1694-1768`, 256 handles by
  default), and a reopen keeps the nonce, the counter and the working set. A read-only open writes
  nothing. PostgreSQL has no open; there the events are row updates by the acquiring process.
- **The counter** is raised by every write transaction that changes what `load` or
  `load_complete` returns for the tenant, and by nothing else. Which `Store` methods those are is
  not written by hand: the conformance case below decides it for every method of the trait. Read
  from the code today, the methods that change load results within the tenant's storage are
  `commit`, `commit_with_segments`, `commit_segments` (if the differential case shows it changes
  load results), `anonymise_ledger_principal` and `restore_snapshot` (which also rotates).
  `remove_principal` deletes sign-in rows only (`storage-sqlite/src/lib.rs:4594-4616`) and raises
  nothing; nor do key documents (`doc_put`, `doc_remove`), sessions, accounts, throttle rows, jobs,
  automation records, audit lines or the hot-set manifest. Tenant setting values change only
  through commits and restores. Index maintenance on open (`index_page::rebuild_if_marked`,
  `pair_index::sync_stored`) is a layout step, not a `Store` method, and changes no answer.
- **Directory state is outside the tenant's generation.** Whether `load` finds a tenant at all
  depends on its control-directory row being `active` (`directory_file`,
  `storage-sqlite/src/lib.rs:1648-1659`), which `begin_provisioning`, `finish_provisioning`,
  `rollback_provisioning`, `reset_tenant`, `reset` and tenant removal change
  (`storage-sqlite/src/lib.rs:1885-1940`), outside any tenant-file transaction. **Decision**: the
  working set holds nothing for a tenant that is not active (an absent tenant is never cached), and
  every directory-state change of a tenant is a control-plane event that first revokes the
  tenant's lease; a tenant that becomes active again is acquired afresh, which rotates its nonce.
  So a directory change never meets a resident entry.
- **Fencing writes is an API-wide change.** Each counter-raising method takes the writer's expected
  generation and, under a lease, the lease's fence token (below), and compares both in its write
  transaction. On PostgreSQL that is one more row updated in a transaction that already serialises
  on the tenant's advisory lock (`docs/storage.md`), so no new point of contention; on SQLite it is
  in the same `BEGIN IMMEDIATE` transaction.
- **Processes other than the lease holder** may read freely and open without rotating or
  migrating. They make no counter-raising write except through the lease holder's API (the plan for
  platform-wide data migrations: they run as operations submitted to each tenant's holder, so no
  per-tenant revocation is needed) or, exceptionally, under the lease itself (the control plane
  revokes it, the tool acquires it, which rotates the nonce and issues a new fence token, and the
  cell reacquires afterwards; this drops the cell's working set for that tenant). Layout migrations
  (`open_file` runs them, `storage-sqlite/src/lib.rs:1674-1688`) run only in the lease holder; a
  non-holder that finds a layout it does not support refuses to open the file. In a cell of several
  processes (D-8300 allows a cell to be a host), exactly one process holds a tenant's lease and is
  its writer.
- **`generation(tenant) -> StoreResult<Option<Generation>>`** reads the generation row.
- **`Commit` gains `expected_generation: Option<Generation>`**. `None` is accepted only when
  `base_revision` is also `None` (a commit that creates the tenant); the store refuses a `None`
  with a base revision as `StoreError::Invalid`. A `Some` is compared with the stored generation
  in the write transaction that compares the base revision (under the tenant lock on PostgreSQL, in
  the `BEGIN IMMEDIATE` transaction on SQLite, under the lock in `MemoryStore`'s `commit_in`,
  `memory.rs:2123`). A mismatch is `StoreError::Conflict`, extended with the generation found. The
  same transaction raises the counter.
- **`Receipt` gains `generation: Generation`**, the generation the commit produced. The runtime
  checks it equals `expected` with the counter raised by one; anything else drops the tenant's
  entries and is logged as a contract violation.
- **Conformance cases**:
  - **differential and exhaustive over the trait**: for every method of `Store`, on a populated
    tenant and a second tenant, take `load_complete` and the ledger of both before and after the
    call, and require each tenant's generation to change exactly when its load result changed. A
    method added to the trait without a case fails the suite (the case list is checked against the
    trait's methods);
  - a commit with a stale expected generation, or with a wrong fence token under a lease, conflicts
    and changes nothing; a `None` expected generation with a base revision is refused;
  - an **LRU handle cycle** (open, evict the handle, reopen) leaves the generation unchanged;
  - an **out-of-band restore** (replace the file with an older copy while the store is closed,
    then run the restore's control-plane step) yields a generation never observed before;
  - a lease acquisition rotates the nonce and issues a new fence token, after which the previous
    holder's counter-raising writes are refused.

In the Lean model the generation is a counter that every store change raises (assumption W4,
`Advances`). The Rust pair realises the only property the proofs use: a changed state never shows a
generation already seen, up to a nonce collision (128 random bits).

**Outside the guarantee**: a file replaced without the control-plane step (no rotation), and above
all a file replaced underneath a process that keeps a handle open. A restore by rename leaves an
open handle on the old inode with no sign at all, so the restore runbook (OPS-3121) revokes the
tenant's lease and has the cell close its handles before the file is replaced. What the cell does
if it sees signs anyway (a counter lower than one it has seen under the same nonce, a page or decode
error): drop the tenant's entries, stop serving it, alarm, and reacquire, which rotates the nonce.
`w4_violation_breaks_checked_read` is the model's statement that without W4 even checked reads can
be wrong.

### Two modes, and what each guarantees

**Checked mode** (no lease). Every request reads the generation at its start, at each round and at
its end. Guarantees, under W4:

- a single-key read returns the store's value as of its generation read (`checkedRead_linearizes`),
  however stale the runtime was;
- a read request over several keys and up to 32 rounds returns a snapshot consistent at one point
  inside the request's interval: every key's value at one generation. In the model that point is
  the finish (`request_snapshot`, with `runWS_reqOk`); in Rust each round's load returns its own
  generation, which the runtime compares with the request's, and a round that finds it changed
  rereads everything the request has read at the new generation;
- a refusal linearizes at its confirming generation read (`decideWS_refusal`);
- an accepted commit is the kernel's step on the state it was applied to (`submitWS_world`). This is
  safety only: the claim that staleness costs at most one conflict per request is argued, not
  proved.

There is no uncovered path for reads in checked mode. W4 is still assumed: it is the store's
obligation, not a condition on the environment.

**Leased mode**. The cell holds a lease per tenant (with its fence token, below) and makes no
generation read on the read path:
a request's rounds compare against the generation the runtime knows. A refusal is still confirmed
by one generation read, and a commit is still guarded by the store. Guarantees: commits and
refusals as in checked mode; reads and request snapshots return the store's value as long as every
counter-raising write to the tenant is made by the holder, which the fence token enforces for
writers using the `Store` API (`runWS_reads` under `covered`);
when a writer is not fenced (the lease was lost without the cell knowing), reads are stale for at
most the renewal interval, and the first commit or renewal discovers it (`unfenced_breaks_reads`
shows the staleness is real without the lease). The model's request rounds use generation reads,
that is checked mode; the leased variant is argued from `covered`, not modelled separately.

### Leases (follow-up item 13)

The nonce is not a fence: `generation(tenant)` returns it, so anyone can present it. It refuses
stale writers, not unauthorised ones. The fence is a separate **fence token**.

- **Record**: in the control directory, per tenant: holder (cell and process), lease number, term
  end.
- **Fence token**: the lease number, issued by the control directory to the acquirer, written into
  the tenant's storage at acquisition, and **not returned by `generation()`** or any other read of
  the `Store` API. Every counter-raising write made under a lease presents it, and the store
  compares it in the write transaction; a write with another token, or none, is refused. This holds
  against any process using the `Store` API; a process with raw access to the database file or
  server is outside the guarantee, as it is for every other store guarantee.
- **Acquire**: a cell process takes the lease when it is free or its term ended more than a drift
  margin ago by the directory's clock. In its first write to the tenant, before serving, it writes
  the new fence token and rotates the nonce.
- **Renew**: every renewal interval R (shorter than the term T) the holder renews in the directory
  and makes a generation read; a nonce other than its own means it has lost the tenant.
- **Expiry**: a holder that has not renewed stops serving the tenant at its local term end minus
  the drift margin. A request in flight at expiry may finish a commit (fenced anyway); a read it
  would answer from resident entries after expiry is answered by a checked read instead, or refused
  with a retryable error naming the tenant's new cell.
- **Revocation**: by notification (the holder acknowledges, closes its handles and stops serving),
  or by timeout (the control plane acts only after an acknowledgement or after the term end plus
  the margin). Every directory-state change of the tenant starts with a revocation.
- **Two processes believing they hold it** (a restored or lost directory, a partition): only the one
  whose fence token is in the storage can make counter-raising writes; the other's first such write
  or renewal fails, and it drops the tenant. Its reads in the meantime are stale by at most R.
- **Move** (D-8302): revoke at the old cell; it stops serving at acknowledgement or expiry; export
  and import (accepted on matching hashes); the import rotates the nonce; the new cell acquires.

What leased-mode reads rest on, then: every counter-raising write to the tenant is made by the
lease holder, which the fence token enforces for every writer using the `Store` API. Writes outside
the API (raw database access) are outside the guarantee.

### The commit path

Rust already decides above the store and commits separately, guarded by the base revision
(`execute_guarded`, `lib.rs:1924-2030`). The design keeps that shape and adds the generation:

1. Decide on what the working set serves, at the generation the runtime knows for the tenant, over
   as many rounds as the kernel's needs take. The decision's expected generation is the one its
   first round read. Rounds may mix generations if something changes in between; the commit guard
   then refuses, since the tenant is no longer at the first round's generation.
2. A refusal is confirmed by a generation read before it is returned: the store's generation is
   compared with the generation the decision's **first round** read, and if they differ the
   runtime refreshes and decides again. In the model `decideWS` is a single step, so the
   multi-round form of this rule is argued, not proved.
3. An acceptance is committed with `expected_generation`. The store applies it only if the tenant
   is still at that generation, in the same transaction that raises it (`submitWS`). An out-of-commit
   write landing between the decision and the commit (the reviewer's `old + 2` case) changes the
   generation, so the commit is refused rather than confirmed at a wrong stamp.
4. After an accepted commit: drop the entries the delta invalidates, confirm the tenant's other
   current entries at the receipt's generation, take the counters from the receipt, re-admit a
   pinned header the commit changed.
5. A conflict refreshes the tenant and the request is decided again (`execute_guarded`'s retry). In
   leased mode a conflict caused by the runtime's own staleness should happen at most once per
   request, since the refresh makes it current; that is argued, not proved. With key creation
   raising no generation, a commit naming a new data subject does not conflict with itself.
6. A commit whose outcome is unknown (timeout, lost connection) drops the tenant's entries and
   assumes no generation (`submitLost`).

Concurrency inside a runtime is outside the model (W2): each tenant runtime's refresh,
commit-and-carry and snapshot assembly is one critical section against the tenant's other requests.
Simultaneous misses on one key are coalesced (CPY-012): one load per key per generation.

### What a decision reads, now that records reference records

The kernel model gained references (integration's `lean/Strata/References.lean` and the
`problem` check in `Model.lean`): a create or update is valid only if every record the new value
references exists, and a delete is refused while a record references the deleted one through a field
that forbids it, and otherwise clears the references to it in every other record. That changes what
a decision reads and what a delete writes:

| Intent | Reads |
|---|---|
| create | the header, the counters, and the record key of every id the new record references (only whether it exists; a resident absence answers it) |
| update | the header, the record itself, and the record key of every id the new value references |
| delete | the header, the record itself, and the whole record list, to find the records that forbid the delete and the records whose references are cleared |
| strict schema change | the header and the whole record list |

A create or update therefore reads a few more single records, not every record: validity depends
on the other records only through whether the referenced ones exist (`problem_eq_via`). A delete
reads `all`, which every record write invalidates, so deletes will mostly miss in the model's
design; in Rust the natural fix is a resident referrer answer per reference field (a `ByField`
need on the field's value), which the coarse answer rule invalidates on any record write of the
type and which a finer rule could keep.

Validity depending on other records does not by itself make a resident entry stale: the working
set holds records and answers, never validity verdicts, and every decision is recomputed from
values served at one generation. The new staleness is the delete's writes to other records, and
the delta now names them.

### What a delta invalidates

The model states the **criterion** an invalidation map must meet: a key of the committed tenant
that the map does not invalidate has the same store answer after the delta, in every world
(`InvSound`). The theorems take the map as a parameter. One map is proved to meet it
(`touchedIn_sound`):

- a create invalidates the counters, the new id (cached as absent), every unordered answer, `all`,
  and the pages that start below the new id;
- an update invalidates the record, every unordered answer and `all`, and **no page** (ids and
  their order are unchanged);
- a delete invalidates the record, **every record whose references to it it clears** (the delta
  names them, as the Rust `StateDelta` names every record it writes), every unordered answer,
  `all`, and the pages that start below it. Before references, a delete wrote one record; the
  earlier rule, which invalidated only the deleted record, would now leave stale referrers
  resident, so it was refined rather than kept;
- a schema change invalidates the header and everything derived from the schema version;
- in Rust, the counters change on every commit, so `counters` is invalidated by every delta.

The page rule is where the criterion does work: a page survives every update. In Rust record ids
are monotonic (`next_record`), so a create is always above every existing id and never "below the
start"; what the finer rule buys there is no invalidation on update, and none on a delete below the
page's start. Rust pages are per object type, so the rule applies per type. A finer rule for unordered answers (an update invalidates `ByField(f, v)` only if
the old or new value of `f` is `v`) needs the old record in the delta; it is a refinement, and each
Rust key kind's criterion is follow-up item 4, held by a property test against `apply_delta`.

### Memory accounting

A byte of residency in the budget is an **accounted byte**: a deterministic function of the entry
(the encoded canonical length of the value times a per-kind factor, plus a fixed per-entry
overhead), the same for admission, eviction and the arbiter. The budget theorems are about
accounted bytes and hold for any size function.

What accounted bytes do not cover, and how each is bounded:

- **In-flight snapshots.** Records are `Arc`s (`crates/model-core/src/types.rs:5310`); a request
  holds clones while it runs. Bounded by a per-cell in-flight reserve. Its size has no measure yet:
  the calibration run records the largest snapshot per request kind on the corpus tenant, and the
  reserve is the admission limit on concurrent requests times that.
- **Policy structures**: the sketch, the heap, the sampling ghosts: fixed per tenant runtime,
  charged to its floor.
- **The hot-set manifest** while being written: bounded by its key count.
- **The adapter's memory**: SQLite's page cache per open database (set explicitly with
  `cache_size`) times `DEFAULT_MAX_OPEN`; PostgreSQL connection buffers. Charged to the cell before
  the arbiter divides it.

The claim is that measured resident memory stays within accounted bytes plus these reserves, within
a tolerance the calibration run establishes; the experiments refute the design if it does not.

### Admission and eviction

- **Pins are a property of the key**: a stale pinned entry that is reloaded stays pinned, and only
  an explicit pin or unpin changes the pin set (`stepWS_pins`). No admission (the same key
  included), eviction or resize removes a resident pinned key (`admit_keeps_pinned`,
  `evict_keeps_pinned`, `resize_keeps_pinned`); a pinned admission succeeds whenever the pinned
  reserve fits (`pin_resident`). Correctness still removes stale pinned entries; they come back on
  the next read, pinned.
- **Unpinned entries** pass the cost-aware admission test and are evicted by GreedyDual-Size-
  Frequency.
- **The runtime enforces the budget whatever the policy says** (`stepWS_within`).

**A cliff to know about.** If a pinned entry grows past what the tenant's grant can hold (a schema
change that makes the header larger than the floor), the pinned admission fails, the key stays
pinned, and nothing for it is resident: every request decodes the header from the store again, the
state before this design. The arbiter must therefore raise a tenant's floor when its pinned bytes
grow, before the next pinned admission; a header that does not fit is reported as a placement
alarm, not silently tolerated.

### Cold start

The header is loaded and pinned when the cell acquires a tenant; records and answers fill on demand.
Each tenant runtime periodically writes a hot-set manifest: keys and access counts, no values, in a
table no load returns. Its keys are record ids, blind hashes and non-personal values, never the
excluded plaintext lookups. It records which records are read how often, which is behavioural data
about the tenant: it is tenant data, removed with the tenant, and an erasure removes the ids of the
erased subject's records from it. On open the runtime prefetches from it within its floor plus a
warmup allowance, in the background, rate-limited per cell.

### Under memory pressure and on one machine

The arbiter lowers a tenant's budget; the policy names victims; then every unpinned entry goes; the
budget never goes below the pinned bytes. Under the arbiter's floor rule (each grant at least the
tenant's pinned bytes), grants that sum to the arbiter's total keep the cell within it
(`machine_resize`); a grant below the floor is exceeded by at most the floor (`resize_budget_le`).

Nothing proves the arbiter can always meet every floor: when the cell's pinned bytes exceed the
arbiter's total, the cell is over-placed. What happens then is specified, not proved: the arbiter
grants floors in order of the tenants' plans and recent cost and leaves the rest unpinned (their
pinned keys stay pinned but are not resident, the cliff above), raises a placement alarm, and the
control plane moves tenants out. Correctness is unaffected: every read and commit still goes
through the store when nothing is resident.

Each tenant runtime estimates its miss-cost curve by sampling its access stream into small ghost
caches; the arbiter gives memory in chunks to the tenant whose curve shows the largest saving per
byte, between its floor and its ceiling. **Proposed decision**: a per-tenant memory ceiling derived
from the plan (BIL-150), as a new requirement beside STO-013.

## What is proved

`lean/Strata/WorkingSet.lean`, imported from `lean/Strata.lean`, over the kernel model of
`lean/Strata/Model.lean`. Parameters: the policy (`Policy σ`), the size function, the invalidation
map (any map meeting `InvSound`). Operations: unchecked and checked reads, pins, unpins, evictions,
invalidations, resizes, decisions, guarded commits, commits with lost receipts, fenced and unfenced
writes by others, and generation reads, in any order, so anything may happen between a decision
and its commit. Assumptions W1 to W4 are at the head of the file; W4 (every change raises the
generation) is the store obligation the contract above has to deliver. No `sorry`, no added axioms;
`lake env lean Audit.lean` lists only `propext`, `Classical.choice` and `Quot.sound` for all 36
audited theorems.

| Property | Theorem |
|---|---|
| A decision from the header, counters, the records it names and (for a delete or a strict schema change) the record list is the kernel's step, references included | `problem_eq_via`, `decideTenant_faithful`, `decideOn_fetch`, `decideOn_congr` |
| One invalidation map meets the criterion, with a finer rule for ordered pages | `touchedIn_sound` |
| A checked single-key read returns the store's value, whatever the runtime held | `checkedRead_linearizes` |
| A read request over several rounds returns, key by key, the store at the moment it finishes | `request_snapshot`, `stepWS_reqOk`, `runWS_reqOk` |
| An unchecked read of a safe tenant returns the store's value; in covered schedules, every one | `readWS_value`, `runWS_reads` |
| A refusal is the kernel's refusal on the current state | `decideWS_refusal` |
| An accepted commit is a kernel step on the state it is applied to (safety only) | `submitWS_world` |
| At every step of every schedule (given W4): the above, and the committed world changes only by such steps or by other writers | `runWS_faithful` |
| The invariants (coherence, knowledge never ahead of the store, pending decisions valid) hold at every step | `stepWS_inv` |
| An unchecked read after an unfenced write serves the old value | `unfenced_breaks_reads` |
| Without W4, even a checked read serves the old value | `w4_violation_breaks_checked_read` |
| Accounted bytes never exceed the budget | `stepWS_within`, `runWS_within` |
| Pins change only when asked; pinned keys survive admission, eviction, resize | `stepWS_pins`, `admit_keeps_pinned`, `evict_keeps_pinned`, `resize_keeps_pinned`, `pin_resident` |
| Budgets on one machine, under the floor rule | `resize_budget_exact`, `resize_budget_le`, `machine_resize` |
| Placement against sealing, on schedules of admissions, value writes, key destructions and reads | `below_sealing_erasure_safe`, `above_sealing_serves_erased` |

The central statements:

```lean
theorem checkedRead_linearizes (sys : Sys σ) (k : Key) (hs : Sound sys.cache.entries sys.store) :
    (stepWS pol size inv sys (.checkedRead k)).2 = .value (fetch sys.store.world k)

theorem submitWS_world (sys : Sys σ) (h : Inv sys) :
    (submitWS pol size inv sys).1.store.world = sys.store.world ∨
    ∃ p, sys.pending = some p ∧
      step sys.store.world p.intent p.ctx = .ok (submitWS pol size inv sys).1.store.world

theorem runWS_faithful (hinv : InvSound inv) (ops : List Op) :
    ∀ (sys : Sys σ), Inv sys → RunFaithful pol size inv sys ops

theorem request_snapshot (sys : Sys σ) (r : Req) (hrq : sys.req = some r)
    (hr : ReqOk sys.req sys.store) :
    (stepWS pol size inv sys .finish).2 =
      .snapshot ((r.got.map Prod.fst).map (fun k => (k, fetch sys.store.world k)))

theorem runWS_reads (hinv : InvSound inv) (ops : List Op) :
    ∀ (sys : Sys σ) (dirty : TenantId → Bool), Inv sys →
      (∀ y, dirty y = false → SafeAt sys.cache.entries sys.known sys.store y) →
      covered dirty ops = true → ReadsTrue pol size inv sys ops
```

`RunFaithful` says, at every step: a safe unchecked read and every checked read return the store's
value; a refusal is the kernel's; and the committed world is unchanged, or (only at a submit) is the
kernel's step of the pending decision on the state it was applied to, or (only at another writer's
write) is that write. `covered` is a
property of the environment, not something the runtime can observe: no unchecked read of a tenant
written behind the runtime's back since its last generation read. It is what a lease provides;
checked mode does not need it.

### What is not proved, and what turned out false

- **The generation's realisation is not proved.** The model assumes W4; the Rust pair of a rotated
  nonce and a counter is argued above and held by conformance cases, including an out-of-band
  restore. A restore under an open file is outside the guarantee, and the model shows the
  consequence (`w4_violation_breaks_checked_read`).
- **Leases are not modelled.** Their effect is modelled (fenced writes keep `covered`; the commit
  guard is the fence for writes); expiry, renewal and clocks are specified in prose.
- **"The pinned closure is always resident" is false**: a budget below the pinned bytes, a header
  grown past its grant, and the stale entries correctness removes.
- **No policy optimality, no arbiter feasibility** is proved.
- **Concurrency inside the runtime** is outside the model (W2).
- **The model's kernel is small**: one object type, three scalar value kinds and references, no
  indexes, uniqueness, policies reading other records, segments, journal, idempotency, jobs or
  settings, and a four-constructor delta (a delete now names the records it clears). No theorem
  weakened when references arrived: the faithfulness and invalidation theorems were restated over
  the new kernel and still hold exactly. `decideTenant_faithful` holds partly because `decideTenant` was
  written next to `stepTenant`; its value is in fixing which keys a decision reads. `InvSound` is
  the general part; each Rust key kind's criterion is follow-up item 4.
- **The sealing model is separate from the main one** and has one key per value; it shows the
  placement decision on schedules with writes, generations and destructions, not the Rust token
  format.
- **"At most one conflict from staleness"** is argued, not proved; `submitWS_world` is safety.
- **Accounted bytes are not resident memory**; the gap is measured, not proved.
- **Correspondence with the Rust code is evidence, not proof** (D20 in `docs/decisions.md`).

## Experiments that would test it

The simulator is single-threaded and logical; its costs come from a calibrated model that consumes
traces and carries the empirical evidence class (`crates/simulation/src/lib.rs`).

### Coherence runs (logical, in the simulator)

The simulator drives the store path through `ResidentStore` with a **fault schedule** beside its
existing faults (`engine.rs:49-58`), each a function of the seed and the subject (D19):

- an unfenced write (a direct store mutation outside the runtime);
- a restore to an earlier snapshot, fenced and unfenced, and an out-of-band file restore followed
  by a reopen;
- a delete and re-create of a tenant;
- an out-of-commit write between a decision and its commit;
- a key destruction in the middle of reads of the subject's records;
- an ambiguous commit;
- a lost lease: a second runtime acquiring the tenant while the first still holds resident entries;
- an LRU handle cycle under load (handles evicted and reopened), which must change nothing;
- a commit naming a new data subject (key creation inside the commit), which must not conflict;
- a read whose miss load lands between the runtime's resident entries and another commit.

Measures: in checked mode with every fault, and in leased mode with only fenced faults, every
outcome and the final state hash are identical to a run without the working set; zero budget
violations, checked after every step; every commit's base generation equals the generation it was
applied at; and, in leased mode with an unfenced write and no renewal, the run **does** diverge
until the next renewal, by at most R, which confirms the fault injection works and measures the
staleness bound. Refutation: any divergence in checked mode, or in leased mode with fenced faults,
or a leased divergence longer than R.

### Cost runs (on real stores)

`MemoryStore` misses are nearly free, so policy and arbiter questions are answered on SQLite with
SQLCipher and on PostgreSQL: wall time per miss and per hit by key kind (including the key-document reads,
the unwrap and the opens of a hit for a tenant with sealed fields), the generation read's cost in checked mode, bytes
decoded per request, and resident memory sampled from the process against accounted bytes.

**Data.** The corpus installed into one tenant and populated by a seeded generator, plus a
population of small tenants from platform templates. The popularity distributions are the author's
choice until real traces exist, and the conclusions say so. **Real traces** come from the journal
(commits) and a sampled read trace (tenant, need kind, hit or miss, no values) emitted by
`ResidentStore`.

**Workloads.** Steady reads with a range of skews; scans interleaved with point reads; write
bursts; one cell with 1,000 small tenants and 10 large ones under one budget (arbiter, static split,
whole-tenant residency, a process per tenant at equal memory); cold restart with and without
manifests; schema changes and erasures in the middle.

**What would refute the design.**

- Measured RSS exceeding accounted bytes plus reserves by more than the calibrated tolerance.
- Header residency alone reaching most of the cost-weighted saving: build the header and stop.
- The cost-aware TinyLFU and GDSF combination not beating CLOCK clearly on SQLCipher and
  PostgreSQL: use CLOCK.
- The arbiter not beating a static split: static budgets with ceilings.
- Whole-tenant residency matching the design within the same budget: choose it.
- The generation read costing a large share of the saving in checked mode: leases become a
  prerequisite.
- The key-document reads, unwrap and opens making a hit for a tenant with sealed fields cost a large
  share of a miss: those tenants keep only the header resident.

## Follow-up work, by dependency

Each item is written to be filed as a task: title, scope, what it depends on, and acceptance
criteria that a test or a check can fail. Items are ordered by dependency; none carries an estimate.

### 1. Tenant generation and guarded commits in the store contract

- **Scope**: `crates/storage-api` (trait, `Commit`, `Receipt`, `StoreError::Conflict`,
  `MemoryStore`, conformance suite), `crates/storage-sqlite`, `crates/storage-postgres`,
  `crates/backend` callers of `commit`. Adds `Generation { nonce, counter }` in a row of its own per
  tenant; `generation(tenant)`; nonce rotation at create, `restore_snapshot`, import and lease
  acquisition (the acquisition entry point is a store method item 13 calls); the counter raised by
  every write that changes load results and by nothing else; `Commit.expected_generation`
  (`None` only with `base_revision: None`); `Receipt.generation`; `Conflict` carrying the
  generation found; the fence-token column and its comparison (unused until item 13). No working
  set yet.
- **Depends on**: nothing.
- **Acceptance**:
  - the exhaustive differential conformance case passes on memory, SQLite and PostgreSQL: for every
    `Store` method, each tenant's generation changes exactly when its `load_complete` result or
    ledger changed; the case list is checked against the trait's methods, so a new method without a
    case fails;
  - a commit with a stale `expected_generation` returns `Conflict` with the found generation and
    changes nothing; a `None` with a base revision returns `Invalid`;
  - `Receipt.generation` equals the expected generation with the counter raised by one;
  - an LRU handle cycle leaves the generation unchanged; an out-of-band restore followed by the
    restore step yields a generation never observed before;
  - `generation()` reads one small row: no schema decode (checked by a test that it succeeds on a
    tenant whose schema document is unreadable);
  - existing conformance and backend tests pass unchanged apart from the new fields.

### 2. Header residency (CPY-014)

- **Scope**: a `ResidentStore` wrapper in `crates/storage-api` holding, per tenant, the decoded
  header (schema, members, name, setting values) and compiled schema keyed by generation, schema
  version and schema hash; counters always from the store or the receipt.
- **Depends on**: 1.
- **Acceptance**: the storage conformance suite passes with `ResidentStore` over each adapter; a
  test counts schema decodes and shows one per tenant per generation across repeated loads; a
  commit that changes the schema, and one that does not, each leave the header current (compared
  with an unwrapped store's `load`).

### 3. `ResidentStore` below sealing

- **Scope**: record entries (present and absent), stamps, request rounds that compare each load's
  generation with the request's and reread on a change, refusal confirmation against the first
  round's generation, guarded commit and receipt check, carry-forward with an invalidation map,
  lost-receipt drop, miss coalescing (CPY-012); key documents never resident; the plaintext
  exclusion for `ByField`, `ByFields`, `IndexPage` and `PairPage` from the sealing plan; placement
  under `SealedStore` in `App::new`.
- **Depends on**: 1, 2.
- **Acceptance**: the storage conformance suite passes through `ResidentStore` under
  `SealedStore`; a test destroys a subject key in a second store handle and shows the next read
  through the first returns the erased token; a commit naming a new data subject succeeds with no
  conflict; a property test interleaving commits from a second handle between request rounds shows
  every returned snapshot equals an unwrapped `load` at one generation; no plaintext personal value
  appears in any resident key (checked by scanning keys in a test with legacy plain rows).

### 4. Invalidation map with its criterion

- **Scope**: the map from a `StateDelta` to invalidated keys, per key kind (record, answer, page,
  all, ledger, header, counters).
- **Depends on**: 3.
- **Acceptance**: a property test over generated tenants and deltas: for every key the map does not
  invalidate, an unwrapped `load` of that key is equal before and after `apply_delta`; one case per
  key kind, including updates that leave pages untouched.

### 5. Answer, page, all and ledger entries

- **Scope**: resident set-valued answers, invalidated by item 4's map.
- **Depends on**: 4.
- **Acceptance**: conformance suite passes; item 3's interleaving property test extended to every
  need kind passes.

### 6. Accounting and calibration

- **Scope**: the size function, the in-flight reserve, adapter memory (`cache_size`, open handles),
  a calibration harness over the corpus tenant.
- **Depends on**: 3, 10.
- **Acceptance**: the harness reports accounted bytes against measured process memory per key kind;
  a test shows accounted bytes never exceed the budget across a generated schedule; the calibrated
  tolerance is recorded in the harness output.

### 7. Policies

- **Scope**: a policy trait; CLOCK; cost-aware TinyLFU admission with GreedyDual-Size-Frequency
  eviction.
- **Depends on**: 3, 6.
- **Acceptance**: unit tests that pinned keys are never chosen as victims and that the budget check
  refuses an entry that does not fit whatever the policy returns; both policies pass the
  conformance suite through `ResidentStore`.

### 8. Differential check against the Lean model

- **Scope**: the working-set model as a program, run beside `ResidentStore` on generated schedules
  with the fault kinds of the experiments (extending `crates/verification/src/lean.rs`).
- **Depends on**: 3.
- **Acceptance**: the Lean program and the Rust run print the same outcomes for every generated
  schedule; a mutant that skips the commit guard is caught.

### 9. Simulator drives the store path

- **Scope**: `crates/simulation` runs `execute_guarded` and the read path through a `Store`, with the
  fault schedule (unfenced write, fenced and unfenced restore, recreate, out-of-commit write between
  decision and commit, key destruction, ambiguous commit, lost lease, LRU handle cycle).
- **Depends on**: nothing for the store path; 3 for working-set runs.
- **Acceptance**: existing simulator traces are unchanged on the store path without faults; each
  fault kind is injectable from a seed and appears in the trace.

### 10. Corpus loader and population generator

- **Scope**: install the 14 enterprise products into a tenant; populate records from a seed.
- **Depends on**: nothing.
- **Acceptance**: the loaded tenant's schema contains every corpus entity; a fixed seed gives a
  byte-identical populated tenant hash on two runs.

### 11. Sampled read trace

- **Scope**: `ResidentStore` emits tenant, need kind, hit or miss, no values, at a sampling rate.
- **Depends on**: 3.
- **Acceptance**: a test shows no field value appears in the trace; the simulator can replay a
  recorded trace.

### 12. The experiments

- **Scope**: the coherence runs and cost runs of the experiments section, with their refutation
  criteria.
- **Depends on**: 5, 7, 9, 10; real-trace runs also on 11.
- **Acceptance**: a report per run with the measures listed; zero divergences and zero budget
  violations in the coherence runs; each refutation criterion evaluated and recorded as met or not.

### 13. Leases and the fence token

- **Scope**: lease records in the control directory (holder process, lease number, term end);
  acquisition writing the fence token and rotating the nonce; fence-token comparison on every
  counter-raising write; renewal with a generation read; expiry at local term end minus a margin;
  revocation by notification and by timeout; revocation before every directory-state change; the
  move sequence; layout migrations only in the holder; holder APIs for platform migrations.
- **Depends on**: 1, and directory routing (D-8301).
- **Acceptance**: a test with two store handles shows the non-holder's counter-raising write is
  refused even when it presents the current generation; a holder that misses renewal stops serving
  from resident entries at its term end; a revocation followed by a restore leaves no resident
  entry in the old holder; a directory-state change of a tenant is refused unless its lease was
  revoked first.

### 14. Hot-set manifest and warmup

- **Scope**: the manifest table outside load results, the exclusions, the erasure rule, rate-limited
  prefetch.
- **Depends on**: 3.
- **Acceptance**: writing the manifest leaves the generation unchanged (item 1's differential case);
  an erasure removes the erased subject's record ids from it; a restart prefetch never exceeds the
  floor plus the warmup allowance.

### 15. Cell arbiter

- **Scope**: miss-cost curve sampling, floors, ceilings, the over-placement rule.
- **Depends on**: 7, and item 12 showing it beats a static split.
- **Acceptance**: a test that the arbiter never grants below a tenant's pinned bytes while the
  cell's floors fit, and raises the placement alarm when they do not; the sum of grants never
  exceeds the arbiter's total.

### 16. Restore runbook

- **Scope**: OPS-3121 procedure: revoke the lease, close handles, replace the file, run the rotation
  step, route.
- **Depends on**: 1, 13.
- **Acceptance**: a scripted restore in a test environment leaves no process holding the old file,
  and the restored tenant's generation was never observed before.

### 17. Specification changes

- **Scope**: a copy class row for the working set (CPY-001); the generation obligation as a storage
  requirement; the per-tenant memory ceiling as a proposed decision; the directory-state decision.
- **Depends on**: nothing.
- **Acceptance**: the specification passes its lint with the new entries; each new requirement
  names the check that holds it (item 1's conformance case, item 13's tests).

## Answers to the reviews

### Integration merge: references in the kernel model

The merged kernel model checks references on create and update and clears or refuses them on
delete. Changes: `decideTenant` reads the records a new value references (through
`problemVia`, equal to the kernel's `problem` by `problem_eq_via`) and, for a delete, the record
list; `Delta.remove` names the records it clears; the invalidation map invalidates them; the
proofs of `decideTenant_faithful`, `decideOn_fetch` and `touchedIn_sound` were redone over the new
kernel. Nothing weakened. The one rule that would have become unsound, a delete invalidating only
the deleted record, was replaced.

### Fourth review

| # | Point | Answer |
|---|---|---|
| 1 | The nonce is not a fence | **Fixed.** A separate fence token (the lease number), stored in the tenant's storage, not returned by `generation()`, compared on every counter-raising write under a lease. The leased-mode claims now rest on "every counter-raising write is made by the holder", enforced for writers using the `Store` API |
| 2 | Method list hand-written | **Fixed.** Confirmed: `remove_principal` touches sign-in rows only (`storage-sqlite/src/lib.rs:4594-4616`); `reset`, `reset_tenant` and provisioning change load results through the directory. The list is now decided by an exhaustive differential conformance case over the trait; the reading from the code is given as a starting point; directory-state changes are handled by a decision (revoke before, acquire afresh after) |
| 3 | `expected_generation: None` | **Fixed.** Accepted only with `base_revision: None`, refused otherwise |
| 4 | Multi-round refusals; request prose | **Fixed.** The refusal confirmation compares with the first round's generation (argued, not proved); the request claim is "consistent at one point inside the request interval" |
| - | Migrations per tenant, `open_file` migrations, multi-process cells, the plaintext exclusion's scope, tenant-key rotation | **Addressed in text** |

### Third review

| # | Finding | Answer |
|---|---|---|
| 1 | Rotating on every open breaks leased mode and cannot be enforced | **Fixed.** Confirmed: the cold open is the LRU-reopen path (`storage-sqlite/src/lib.rs:1694-1768`). The nonce rotates only at control-plane events (create, lease acquisition, restore, archive restore, import); a handle reopen keeps the nonce, the counter and the working set; the counter never resets; other processes write through the holder or under the lease; an LRU handle cycle conformance case is added |
| 2 | Guarded commit conflicts with sealing's own key writes | **Fixed by the simpler rule.** Confirmed (`sealed.rs:1002-1005`, `:690-700`). Key documents are not load results and their writes raise no generation, creation and destruction alike; nothing resident depends on them |
| 3 | Resident key documents bring back an erasure delay | **Fixed by the recommended rule.** Key documents are never resident; every call reads them, so a destruction anywhere takes effect at the next call. The hit cost for tenants with sealed fields is stated (a store round trip for the key documents, an unwrap, the opens). The sealing model now has value writes raising a generation, stamped entries and destructions raising none; `below_sealing_erasure_safe` is no longer true by construction |
| 4 | Linearizability proved per key only | **Fixed.** Read requests over several rounds are in the model (`begin`, `round`, `finish`); `request_snapshot` proves the snapshot is the store at the finishing state, at every step of every schedule (`runWS_reqOk`). Single-key claims are labelled as such |
| - | Restore by rename; generation in the tenant row; API-wide fencing; page rule in Rust; one-conflict claim; `StepFaithful` disjunct | **Addressed**: the runbook closes handles first; the generation has its own row; the counter-raising methods are listed with the PostgreSQL lock argument; the page rule's Rust gain is stated; the one-conflict claim is marked unproved; `StepFaithful`'s step disjunct is now tied to `submit` and `submitLost` |
| - | Wording | **Corrected**: the constraints line, the leased-mode refusal path, and the second-review rows below |

### Second review

| # | Finding | Answer |
|---|---|---|
| 1 | W4 relocates the old hypothesis; epoch and counter share no transaction; a restored directory repeats the epoch | **Fixed in the design, stated in the model.** Confirmed. The generation is entirely in the tenant's storage: a 128-bit nonce and a counter (the rotation events were narrowed in the third review). One transaction; no generation lives in the directory, though the lease does. A restore under an open file is outside the guarantee, with the cell's behaviour specified, and `w4_violation_breaks_checked_read` states the boundary. An out-of-band restore conformance case is added |
| 2 | `commitWS` fused check and commit; Rust guards on revision only | **Fixed.** Confirmed (`commit_in`, `memory.rs:2123`; `Commit` and `Receipt` carry no generation, `lib.rs:221-351`). Decide and submit are separate operations; the store applies a commit only at the generation it was decided at; `Commit.expected_generation` and `Receipt.generation` are part of follow-up item 1. Commit transparency is restated as `submitWS_world` and `runWS_faithful` |
| 3 | "Covered" is unobservable | **Fixed.** Checked mode is stated as linearizability (`checkedRead_linearizes`, `request_snapshot`, `decideWS_refusal`, `submitWS_world`) with no environment condition beyond W4, the store's obligation; `covered` is presented as the environment condition a lease provides, with a staleness bound of one renewal interval and specified behaviour on the uncovered path |
| 4 | Leases have no semantics | **Fixed in the design**: record, acquire, the nonce as fence token compared in the write transaction, renewal, expiry (including mid-request), revocation by notification and by timeout, two cells, move. Not modelled beyond their effect |
| 5 | A hit is not store-free with sealed fields; legacy plain-value lookups | **Fixed, then revised in the third review**: key documents are not resident after all, so a hit for such a tenant makes the key-document reads, an unwrap and the opens, measured in the cost runs; legacy plain-value needs are excluded from answers and the manifest; the manifest's behavioural data is stated and erased with its subject |
| 6 | The sealing theorems were trivial | **Replaced twice**; the third-review version adds value writes, generations and stamps, so the below-sealing theorem is not true by construction. Earlier version: `below_sealing_erasure_safe` (every read is the stored value opened with the live keys) and `above_sealing_serves_erased` (for any value admitted while its key lived, a read after destroying the key serves it) |
| 7 | `touchedIn` is weak | **Fixed in part.** An ordered page key is in the model with a finer rule (no invalidation on update, none on create or delete below the start), proved against the criterion. Unordered answers stay coarse; follow-up item 4 keeps the per-kind criterion |
| 8 | Carry assumed the commit is the only generation change | **Fixed.** The guard makes an intervening write refuse the commit; the receipt carries its generation and the runtime checks it |
| - | Header grown past its grant; floors not always feasible; in-flight reserve unmeasured | **Addressed in text**: the cliff and the arbiter's floor raise; the over-placement rule; the reserve's calibration |
| - | Claims stronger than proved | **Corrected**: see the first-review table, rows 1, 6 and 7 |

### First review, as now stated

| # | Finding | Answer |
|---|---|---|
| 1 | Revision is not a safe coherence key | **Fixed in the design; in the model the old hypothesis became assumption W4**, the store obligation that every change raises the generation. It is not gone: it is stated, the design says how the store delivers it, and `w4_violation_breaks_checked_read` shows what fails without it |
| 2 | Pins drop on staleness | **Fixed.** Pins are a set on the cache (`stepWS_pins`, `admit_keeps_pinned`) |
| 3 | Header lacks revision and counters | **Fixed.** Counters are their own key |
| 4 | No cheap revision read | **Fixed in the design** (`generation(tenant)`); the claim "no store call under a lease" holds for tenants without sealed fields, and with sealed fields means no store round trip but an unwrap per call |
| 5 | Out-of-commit writers | **Fixed in the design** (the counter) |
| 6 | Erasure above sealing | **Fixed in the design** (below sealing, key documents resident and invalidated by destruction); the first revision's theorems for it were trivial and are replaced (second review, row 6) |
| 7 | Transparency over a schedule space without staleness | **Fixed as restated**: not "transparent in every schedule" but, given W4, every accepted commit is a kernel step on the state it is applied to and every refusal and checked read is true, at every step of every schedule; unchecked reads in covered schedules |
| 8 | Model gaps; hand-listed invalidation | **Partly fixed, partly left open** (criterion and page rule fixed; the small kernel open) |
| 9 | `resize` and the machine sum | **Fixed** (`resize_budget_le`, `machine_resize`) |
| 10 | Accounting | **Fixed in the design**, measured not proved |
| 11 | Overstated spec claims | **Fixed** |
| 12 | Experiments | **Fixed in the design** |
| - | TinyLFU, CPY-012, PostgreSQL, `PairPage`, `Ledger`, loose references | **Addressed** |
