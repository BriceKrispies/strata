# The tenant working set

What a Strata runtime should hold in memory in front of its store, how it decides, and exactly when
holding it changes no answer. This is a design, with its safety properties proved in Lean
(`lean/Strata/WorkingSet.lean`). Nothing here is implemented yet; the follow-up work is listed at
the end, ordered by what unblocks what. The design has been revised after three adversarial
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
| Writes outside commits | `storage-sqlite/src/lib.rs:4583-4590` (`anonymise_ledger_principal`), `:4594` (`remove_principal`), `restore_snapshot`; `memory.rs:1335-1345` (mutates in place) | Change what a load returns without a commit or a revision bump |
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
Such needs are never answer keys and never appear in the hot-set manifest; they are answered from
the store every time.

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
- **The counter** is raised by every write transaction that changes load results: commits,
  `anonymise_ledger_principal`, `remove_principal`, setting writes, index rebuilds that change
  answers. Writes outside load results raise nothing: key documents, the hot-set manifest,
  sessions, throttle rows, jobs.
- **Fencing every writer is an API-wide change.** Each method that raises the counter takes the
  writer's expected generation (at least its nonce) and compares it in its write transaction:
  `commit`, `remove_principal`, `anonymise_ledger_principal`, setting writes, index rebuilds. On
  PostgreSQL that is one more row updated in a transaction that already serialises on the tenant's
  advisory lock (`docs/storage.md`), so it adds no new point of contention; on SQLite it is in the
  same `BEGIN IMMEDIATE` transaction.
- **Processes other than the lease holder** (a migration job, an admin tool, the subject-rights
  service): they may read freely and open without rotating anything. A write that raises the
  generation either goes through the lease holder's API or is made under the lease (the control
  plane revokes it from the cell first, the tool acquires it, which rotates the nonce, and the cell
  reacquires afterwards). A write from anyone else is refused by the fence. There is no alternation:
  only a lease acquisition rotates.
- **`generation(tenant) -> StoreResult<Option<Generation>>`** reads the generation row.
- **`Commit` gains `expected_generation: Option<Generation>`**, compared with the stored generation
  in the write transaction that compares the base revision (under the tenant lock on PostgreSQL, in
  the `BEGIN IMMEDIATE` transaction on SQLite, under the lock in `MemoryStore`'s `commit_in`,
  `memory.rs:2123`). A mismatch is `StoreError::Conflict`, extended with the generation found. The
  same transaction raises the counter.
- **`Receipt` gains `generation: Generation`**, the generation the commit produced. The runtime
  checks it equals `expected` with the counter raised by one; anything else drops the tenant's
  entries and is logged as a contract violation.
- **Conformance cases**: every counter-raising method raises the generation of the tenant it names
  and changes no other's; writes outside load results (key documents, sessions) change no
  generation; a commit with a stale expected generation conflicts and changes nothing; an **LRU
  handle cycle** (open, evict the handle, reopen) leaves the generation unchanged; an
  **out-of-band restore** (replace the file with an older copy while the store is closed, then run
  the restore's control-plane step) yields a generation never observed before; a lease acquisition
  rotates the nonce and fences the previous holder's writes.

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
- a read request over several keys and up to 32 rounds returns, key by key, the store's value at
  the moment it finishes (`request_snapshot`, with `runWS_reqOk`): a round that finds the
  generation changed rereads everything the request has read at the new state, so the snapshot is
  never a mix of generations;
- a refusal linearizes at its confirming generation read (`decideWS_refusal`);
- an accepted commit is the kernel's step on the state it was applied to (`submitWS_world`). This is
  safety only: the claim that staleness costs at most one conflict per request is argued, not
  proved.

There is no uncovered path for reads in checked mode. W4 is still assumed: it is the store's
obligation, not a condition on the environment.

**Leased mode**. The cell holds a lease per tenant and makes no generation read on the read path:
a request's rounds compare against the generation the runtime knows. A refusal is still confirmed
by one generation read, and a commit is still guarded by the store. Guarantees: commits and
refusals as in checked mode; reads and request snapshots return the store's value as long as every
writer to the tenant is fenced, which is what the lease provides (`runWS_reads` under `covered`);
when a writer is not fenced (the lease was lost without the cell knowing), reads are stale for at
most the renewal interval, and the first commit or renewal discovers it (`unfenced_breaks_reads`
shows the staleness is real without the lease). The model's request rounds use generation reads,
that is checked mode; the leased variant is argued from `covered`, not modelled separately.

### Leases (follow-up item 13)

- **Record**: in the control directory, per tenant: holder cell, lease number, term end.
- **Acquire**: a cell takes the lease when it is free or its term ended more than a drift margin ago
  by the directory's clock. Having acquired, it rotates the nonce in its first write to the tenant,
  before serving.
- **Fence**: the nonce is the fence token. Every write that raises the generation, commits and
  out-of-commit writes alike, carries the writer's expected generation and is refused in the write
  transaction if the stored nonce is not the writer's. A cell whose nonce was replaced can make no
  such write. Key-document writes are not fenced (they raise no generation); a fenced-out cell can
  still create a subject key, which is harmless.
- **Renew**: every renewal interval R (shorter than the term T) the holder renews in the directory
  and makes a generation read; if the nonce is no longer its own, it has lost the tenant.
- **Expiry**: a cell that has not renewed stops serving the tenant at its local term end minus the
  drift margin. A request in flight at expiry may finish a commit (fenced anyway), but a read it
  would answer from resident entries after expiry is answered by a checked read instead, or
  refused with a retryable error naming the tenant's new cell.
- **Revocation**: by notification (the fast path: the cell acknowledges, closes its handles and
  stops serving), or by timeout: the control plane hands the tenant to another cell, restores it or
  moves it only after an acknowledgement or after the term end plus the margin.
- **Two cells believing they hold it** (a restored or lost directory, a partition): only the one
  whose nonce is in the storage can write; the other's first write or renewal fails, and it drops
  the tenant and gives up the lease. Its reads in the meantime are stale by at most R.
- **Move** (D-8302): revoke at the old cell; the old cell stops serving at acknowledgement or
  expiry; export and import (accepted on matching hashes); the import rotates the nonce; the new
  cell acquires.

### The commit path

Rust already decides above the store and commits separately, guarded by the base revision
(`execute_guarded`, `lib.rs:1924-2030`). The design keeps that shape and adds the generation:

1. Decide on what the working set serves, at the generation the runtime knows for the tenant, over
   as many rounds as the kernel's needs take. The decision's expected generation is the one its
   first round read. Rounds may mix generations if something changes in between; the commit guard
   then refuses, since the tenant is no longer at the first round's generation.
2. A refusal is confirmed by a generation read before it is returned; if the runtime was behind, it
   refreshes and decides again (`decideWS`). In checked mode the request's own generation read
   serves as the confirmation.
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

### What a delta invalidates

The model states the **criterion** an invalidation map must meet: a key of the committed tenant
that the map does not invalidate has the same store answer after the delta, in every world
(`InvSound`). The theorems take the map as a parameter. One map is proved to meet it
(`touchedIn_sound`):

- a create invalidates the counters, the new id (cached as absent), every unordered answer, `all`,
  and the pages that start below the new id;
- an update invalidates the record, every unordered answer and `all`, and **no page** (ids and
  their order are unchanged);
- a delete invalidates the record, every unordered answer, `all`, and the pages that start below it;
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
`lake env lean Audit.lean` lists only `propext`, `Classical.choice` and `Quot.sound` for all 35
audited theorems.

| Property | Theorem |
|---|---|
| A decision from the header, counters and needed records is the kernel's step | `decideTenant_faithful`, `decideOn_fetch`, `decideOn_congr` |
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
- **The model's kernel is small**: one object type, three value kinds, no indexes, uniqueness,
  references, policies reading other records, segments, journal, idempotency, jobs or settings,
  and a four-constructor delta. `decideTenant_faithful` holds partly because `decideTenant` was
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

1. **Tenant generation** in the store contract, specified above: `Generation` (nonce, counter) in
   a row of its own; nonce rotation only on create, lease acquisition, `restore_snapshot`, archive
   restore and import, never on a handle open; the counter raised by every write that changes load
   results and by nothing else (not key documents); the expected generation on every
   counter-raising method (`commit`, `remove_principal`, `anonymise_ledger_principal`, setting
   writes, index rebuilds); `generation(tenant)`; `Commit.expected_generation`; `Receipt.generation`;
   `Conflict` reporting the generation found; the conformance cases, including the LRU handle cycle
   and the out-of-band restore. Depends on nothing; everything below depends on it.
2. **Header residency** (CPY-014), counters split out. Depends on 1.
3. **`ResidentStore` below sealing**: record entries (key documents never resident), stamps,
   request rounds that reread on a generation change, refresh, refusal confirmation, guarded commit and receipt check, carry-forward,
   lost-receipt drop, miss coalescing; legacy plain-value lookups excluded; passes the storage
   conformance suite. Depends on 1 and 2.
4. **Invalidation map in Rust** with a property test of its criterion against `apply_delta`, per
   key kind. Depends on 3.
5. **Answer, page, all and ledger entries**. Depends on 4.
6. **Size accounting and calibration**, including the in-flight reserve. Depends on 3.
7. **Policies**: CLOCK, then cost-aware TinyLFU with GDSF. Depends on 3 and 6.
8. **Differential check** of the Lean model, as a program, against `ResidentStore` on generated
   schedules with the fault kinds above. Depends on 3.
9. **Simulator drives the store path**, with the fault schedule. Independent of 1 to 8.
10. **Corpus loader and population generator**. Independent.
11. **Sampled read trace**. Depends on 3.
12. **The experiments**. Depends on 5, 7, 9 and 10; real-trace runs also on 11.
13. **Leases**, as specified above: directory records, the nonce as fence token on every write,
    renewal with a generation read, expiry, revocation by notification and timeout, the move
    sequence. Depends on 1 and on directory routing (D-8301).
14. **Hot-set manifest and warmup**, with the exclusions and erasure rule above. Depends on 3.
15. **Cell arbiter**, floors, ceilings, the over-placement rule. Depends on 7, and on 12 showing
    it beats a static split.
16. **Restore runbook** (OPS-3121): revoke the lease and close handles before replacing a file, then
    the rotation step. Depends on 1 and 13.
17. **Specification changes**: a copy class row for the working set (CPY-001); the generation
    obligation as a storage requirement; the per-tenant memory ceiling as a proposed decision.
    Can proceed alongside 1.

## Answers to the reviews

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
