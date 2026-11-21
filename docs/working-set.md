# The tenant working set

What a Strata runtime should hold in memory in front of its store, how it decides, and exactly when
holding it changes no answer. This is a design, with its safety properties proved in Lean
(`lean/Strata/WorkingSet.lean`). Nothing here is implemented yet; the follow-up work is listed at
the end, ordered by what unblocks what. The second revision answers an adversarial review; the
answers are collected in the last section, and the design above it is the revised one.

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
- **Erasure is by key destruction** (D-350): destroying a subject key changes no stored byte and no
  revision, and from then on nothing opens.

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

Resident entries hold what the adapter holds: sealed tokens for personal fields, and blind hashes
in answer keys over searchable personal fields (sealing translates such lookups into blind-hash
lookups before they reach the adapter, `sealed.rs:20-25`). Every serve passes through
`SealedStore`, whose cipher session caches nothing, so a destroyed key opens nothing from the
working set either, with no event to distribute. The price is that a hit still opens sealed fields;
only tenants with personal fields pay it, and only on those fields. Placing the working set above
sealing is rejected: it holds plaintext, and a key destruction is neither a world change nor a
generation change, so it would go on serving erased values (`above_sealing_serves_erased`).

### What can be resident

| Key | Value | Pinned | Notes |
|---|---|---|---|
| `header(tenant)` | schema, members, name, setting values | yes | Without counters. CPY-014's compiled schema hangs off it, keyed by schema version and schema hash |
| `counters(tenant)` | revision, record counter, event sequence | no | Changed by every commit; never carried forward, always taken from the generation read or the commit receipt |
| `record(tenant, object, id)` | the record, or its absence | no | |
| `answer(tenant, need)` | complete id set for `ByField`, `ByFields`, `PageOf`, `IndexPage`, `PairPage` | no | Valid only as a complete set; records are separate entries, so an id in two answers is counted once |
| `all(tenant, object)` | every id of the type | no | `AllOf` |
| `ledger(tenant, need)` | ledger state for a `Need::Ledger` | no | Ledger entries change outside commits (anonymisation), which must raise the generation |
| derived structures | indexes, reference closures, policy dependencies | by choice | Pinned when measured access says every decision reads them |

### The coherence key: a generation, not the revision

The revision is not safe to key on: a restored snapshot sets it back
(`memory.rs:1573-1607`, `storage-sqlite/src/lib.rs:4850-4860`), the store contract says it can
repeat (`lib.rs:800-808`), a tenant deleted and created again starts over at 1, and a tenant file
can be restored from outside the cell (OPS-3121, D-8300). Two different states at the same
revision would let a stale entry pass the check.

The design adds a **tenant generation** to the store contract: a pair `(epoch, counter)`, compared
for equality, that rises on every change to anything a load returns, whoever makes it.

- `counter` lives in the tenant's own storage and is raised by every write transaction that
  changes load results: commits, `anonymise_ledger_principal`, `remove_principal`, setting writes,
  index rebuilds that change answers, and any future method of that kind.
- `epoch` lives in the control directory (D-8301), outside the tenant file, and is raised by every
  operation that replaces the tenant's storage wholesale: restoring a snapshot, a point-in-time
  restore from the archive, a move between cells or backends (D-8302), deleting and creating a
  tenant. Because it is outside the file, a file restored from outside the cell cannot bring an old
  epoch back.
- A new contract method `generation(tenant)` reads both: two small integer reads, no schema decode,
  no count. On SQLite it is a single-row read of the tenant row's counter plus the directory's
  epoch (cached by the cell and invalidated by directory notifications); on PostgreSQL a column on
  the tenant's row and the directory row.
- A conformance test checks the obligation: after every mutating method of the contract, the
  generation of the tenant it names has strictly risen, and no other tenant's has changed.

`MemoryStore`'s epoch plus per-type counter (`memory.rs:301-318`) is the same idea for one
consumer; the generation generalises it to the whole tenant.

### When reads are checked: leases, fencing and the cheap read

A runtime knows a generation per tenant, which may be behind the store's. It learns the current
one from every miss (a load returns it), every commit receipt, every fenced write, and every
explicit `generation` read. Reads of a tenant are served from resident entries only when they are
stamped with the known generation.

- **Fenced writes** are those the runtime is told about: every out-of-commit writer in the cell
  process goes through the same `ResidentStore` method, which raises the generation and refreshes
  the runtime; control-plane operations (restore, move, recreate) revoke the cell's lease on the
  tenant through the directory before acting, and the cell refreshes on revocation.
- **Unfenced writes** are everything else: an operator tool writing the file directly, a restore
  that bypassed the control plane. Nothing in the model or the design can serve correct reads
  across those without asking the store (`unfenced_breaks_reads`).
- So there are two modes, chosen per cell. **Leased**: the cell holds a lease per tenant in the
  directory, every write path is fenced, and reads need no store call at all. **Checked**: no
  lease; every request starts with `generation(tenant)`, which is cheap by construction, and the
  saving is everything after it. The first revision claimed both "must check" and "usually no store
  call"; the honest statement is: no store call under a lease, one cheap call without.

### The commit path, staleness and conflicts

A commit decides on what the working set serves, after making the runtime current for the tenant:
if the known generation is behind, the runtime drops the tenant's entries and learns the
generation, which is the conflict the store's base-revision guard would report, answered by one
refresh instead of up to `MAX_CONFLICT_RETRIES` decisions on stale state. A commit through the
runtime therefore has exactly the kernel's outcome in every schedule, however stale the runtime was
(`commitWS_transparent`); a conflict from staleness is a slowdown of one refresh, never an error
(CPY-011). Conflicts from a genuinely concurrent writer still go through the store's guard and
`execute_guarded`'s retry, as today.

After the store accepts a commit, the runtime drops every entry the delta invalidates, confirms
every other current entry of the tenant at the new generation, takes the counters from the receipt,
and re-admits a pinned header the commit changed.

A commit whose outcome is unknown (the store applied it, the caller saw an error: a timeout, a lost
connection) is an **ambiguous commit**: the runtime drops every entry of the tenant and does not
assume a generation; the next read misses and learns it. This is modelled (`Op.ambiguous`), and
leaves reads of the tenant safe.

A snapshot assembled from resident entries and one load for the misses must be of one generation:
the load returns its generation; if it differs from the known one, the resident entries are
discarded and the request is answered from a load of everything it needs. In the model this is the
rule that a miss learns the generation (`sync`).

Concurrency inside a runtime is outside the model (W2): the Rust makes each tenant runtime's
refresh, commit-and-carry, and snapshot assembly one critical section against the tenant's other
requests, so a request sees either one generation's entries or another's, never a mix.
Simultaneous misses on one key are coalesced (CPY-012): one load per key per generation, the
others wait for it.

### What a delta invalidates

The first revision listed the rule by hand. The model now states the **criterion** an invalidation
map must meet: a key of the committed tenant that the map does not invalidate has the same store
answer after the delta, in every world (`InvSound`). Every coherence and transparency theorem takes
the map as a parameter and assumes only the criterion, so a finer map is safe the moment its
criterion is proved. One map, with answers included, is proved to meet it (`touchedIn_sound`):

- a create invalidates the counters, the new id (cached as absent), every answer and `all`;
- an update or delete invalidates the record, every answer and `all` of its type;
- a schema change invalidates the header and everything derived from the schema version;
- in Rust, the counters change on every commit, so `counters` is invalidated by every delta.

Answers are invalidated per object type, the granularity of D-5064's counters. A finer rule (an
update invalidates `ByField(f, v)` only if the old or new value of `f` is `v`) needs the old record
in the delta and its own criterion proof; it is a later refinement, not a prerequisite.

### Memory accounting

A byte of residency in the budget is an **accounted byte**: a deterministic function of the entry
(the encoded canonical length of the value times a per-kind factor, plus a fixed per-entry
overhead), the same function for admission, eviction and the arbiter. The budget theorems are about
accounted bytes and hold for any size function.

What accounted bytes do not cover, and how each is bounded:

- **In-flight snapshots.** Records are `Arc`s (`crates/model-core/src/types.rs:5310`); a request
  holds clones while it runs, so evicting an entry frees nothing until the request ends. Bounded by
  a per-cell in-flight reserve: the admission limit on concurrent requests times the largest
  snapshot a request may load.
- **Policy structures.** The TinyLFU sketch, the GreedyDual heap, the sampling ghosts of the
  arbiter: a fixed size per tenant runtime, charged to the tenant's floor.
- **The hot-set manifest**, while being written: bounded by its key count.
- **The adapter's own memory.** SQLite's page cache per open database (set explicitly with
  `cache_size` rather than left at its default) times `DEFAULT_MAX_OPEN`; PostgreSQL connection
  buffers. Charged to the cell before the arbiter divides it.

The cell's memory limit (STO-013) is split into: fixed process cost, adapter memory, the in-flight
reserve, and the arbiter's total. The design's claim is that measured resident memory stays within
the accounted total plus these reserves, within a tolerance that the calibration run establishes
and the experiments then hold the design to.

### Admission and eviction

- **Pins are a property of the key**, held by the working set, not of an entry. A stale pinned
  entry that is reloaded stays pinned; nothing but an explicit pin or unpin changes the pin set
  (`stepWS_pins`). No admission, eviction or resize removes a resident pinned key
  (`admit_keeps_pinned`, which now covers replacing the same key, `evict_keeps_pinned`,
  `resize_keeps_pinned`); a pinned admission succeeds whenever the pinned reserve fits
  (`pin_resident`). Correctness still removes pinned entries when they are stale (a refresh, an
  invalidation, an ambiguous commit); they come back on the next read, pinned.
- **Unpinned entries** pass the cost-aware admission test above and are evicted by
  GreedyDual-Size-Frequency.
- **The runtime enforces the budget whatever the policy says**: an entry that does not fit after the
  policy's victims are gone is not admitted, and the request is served from the store
  (`stepWS_within`).

### Cold start

The header is loaded and pinned when a tenant is first routed to the cell; records and answers fill
on demand. Each tenant runtime periodically writes a hot-set manifest (keys and sketch counts, no
values) to a table of its tenant storage that no load returns, so writing it raises no generation;
below sealing, its keys hold blind hashes and record ids, never plaintext personal values. On open
the runtime prefetches from it in priority order within its floor plus a warmup allowance, in the
background, rate-limited per cell so a cell restart does not open every tenant file at once.

### Under memory pressure and on one machine

The arbiter lowers a tenant's budget; the policy names victims; then every unpinned entry goes; the
budget never goes below the pinned bytes. The arbiter therefore grants each tenant at least its
pinned bytes: under that rule, grants that sum to the arbiter's total keep the cell within it
(`machine_resize`), and a grant below the floor is exceeded by at most the floor
(`resize_budget_le`). If the floors of a cell's tenants exceed the arbiter's total, the cell is
over-placed, a placement error to alarm on.

Each tenant runtime estimates its miss-cost curve by sampling its access stream into small ghost
caches at a few sizes; the arbiter gives chunks of memory to the tenant whose curve shows the
largest saving per byte, between its floor and its ceiling. **Proposed decision**: a per-tenant
memory ceiling derived from the plan (BIL-150), recorded as a new requirement beside STO-013.

## What is proved

`lean/Strata/WorkingSet.lean`, imported from `lean/Strata.lean`, over the kernel model of
`lean/Strata/Model.lean`. Parameters: the policy (`Policy σ`, any state type), the size function,
and the invalidation map (any map meeting `InvSound`). The schedule has reads, pins, unpins,
evictions, invalidations, resizes, commits, ambiguous commits, fenced writes, unfenced writes and
generation checks; writes by others are interleaved between the runtime's operations. Assumptions
W1 to W4 are at the head of the file. No `sorry`, no added axioms; `lake env lean Audit.lean` lists
only `propext`, `Classical.choice` and `Quot.sound` for all 28 audited theorems.

| Property | Theorem |
|---|---|
| A decision from the header, counters and needed records is the kernel's step | `decideTenant_faithful`, `decideOn_fetch` |
| A decision reads only its tenant's keys | `decideOn_congr` |
| One invalidation map, answers included, meets the criterion | `touchedIn_sound` |
| A served entry is stamped with the known generation; a safe read is the store's value | `hit_stamp`, `serve_eq_fetch` |
| A check, a miss or a commit makes reads of the tenant safe | `sync_safe_self`, `commitWS_safe`, `stepWS_safe` |
| Commits have the kernel's outcome and world in every schedule, however stale the runtime | `commitWS_transparent` |
| The committed world is transparent in every schedule | `runWS_world` |
| Every outcome is transparent in every covered schedule | `runWS_transparent` |
| A read after an unfenced write with no check serves the old value | `unfenced_breaks_reads` |
| Accounted bytes never exceed the budget, across every operation | `stepWS_within`, `runWS_within` |
| Pins change only when asked; pinned keys survive admission, eviction, resize | `stepWS_pins`, `admit_keeps_pinned`, `evict_keeps_pinned`, `resize_keeps_pinned`, `pin_resident` |
| Budgets on one machine | `resize_budget_exact`, `resize_budget_le`, `machine_resize` |
| Below sealing a read is the store value opened with the live keys; above sealing it is not | `below_sealing_serves_current`, `above_sealing_serves_erased` |

"Covered" (`covered`) is checked by a function over the schedule: no read of a tenant that has had
an unfenced write since the runtime last learned its generation (by a check, a commit, an ambiguous
commit's drop, or a fenced write). The statements:

```lean
theorem runWS_world (hinv : InvSound inv) (ops : List Op) :
    ∀ (sys : Sys σ), Sound sys.cache.entries sys.store →
      (runWS pol size inv sys ops).1.store.world = (directRun sys.store.world ops).1

theorem runWS_transparent (hinv : InvSound inv) (ops : List Op) :
    ∀ (sys : Sys σ) (dirty : TenantId → Bool), Sound sys.cache.entries sys.store →
      (∀ y, dirty y = false → SafeAt sys.cache.entries sys.known sys.store y) →
      covered dirty ops = true →
      (runWS pol size inv sys ops).1.store.world = (directRun sys.store.world ops).1 ∧
      (runWS pol size inv sys ops).2 = (directRun sys.store.world ops).2

theorem unfenced_breaks_reads (pol : Policy Unit) (size : Key → Payload → Nat)
    (inv : Delta → Key → Bool) :
    Sound cexSys.cache.entries cexSys.store ∧
    (runWS pol size inv cexSys [.unfenced 1 none, .read (.header 1)]).2 ≠
      (directRun cexSys.store.world [.unfenced 1 none, .read (.header 1)]).2
```

### What is not proved, and what turned out false

- **Revision as the coherence key is unsound** (review finding 1): two states at one revision. The
  model's store now changes only through operations that raise the generation; that the Rust store
  does so is an obligation of the contract, held by a conformance test, not by the proofs.
- **"The pinned closure is always resident" is false.** A budget smaller than the pinned bytes; a
  commit that grows the header past a budget the old header fitted; and, by design, correctness
  removes stale pinned entries until the next read. What holds is the conditional set above.
- **Reads across an unfenced write are not transparent** without a check or a lease
  (`unfenced_breaks_reads`). This is stated as a counterexample, not hidden by the schedule space.
- **No policy optimality is proved.**
- **Concurrency inside the runtime** is outside the model (W2); the Rust needs one critical section
  per tenant runtime for refresh, commit-and-carry and snapshot assembly.
- **The model's kernel is small**: one object type, three value kinds, no indexes, uniqueness,
  references, policies reading other records, segments, journal, idempotency, jobs or settings, and
  a four-constructor delta. `decideTenant_faithful` holds partly because `decideTenant` was written
  next to `stepTenant`; its value is that it fixes which keys a decision reads. What carries over
  is the shape: invalidation is any map meeting `InvSound`, and each Rust key kind needs its
  criterion proved or property-tested against the real `apply_delta`.
- **Accounted bytes are not resident memory**; the gap is bounded by the reserves above and
  measured, not proved.
- **Correspondence with the Rust code is evidence, not proof** (D20 in `docs/decisions.md`).

## Experiments that would test it

The simulator is single-threaded and logical; its costs come from a calibrated model that consumes
traces and carries the empirical evidence class (`crates/simulation/src/lib.rs`). Two families of
run, with different purposes:

### Coherence runs (logical, in the simulator)

The simulator drives the store path (`execute_guarded`, `read_loaded`) through `ResidentStore`, and
gains a **fault schedule** beside its existing commit-conflict and delivery faults
(`engine.rs:49-58`), each a function of the seed and the subject (D19):

- an unfenced write to a tenant (a direct store mutation outside the runtime);
- a restore of a tenant to an earlier snapshot, fenced and unfenced;
- a delete and re-create of a tenant;
- a key destruction in the middle of reads of the subject's records;
- an ambiguous commit (the store applies it, the runtime is told it failed);
- a read whose snapshot load lands between the runtime's resident entries and a concurrent commit
  (an interleaving of one request's steps with another's, scheduled by the engine).

Measures: a differential check that every outcome and the final state hash are identical with and
without the working set, in leased mode with only fenced faults and in checked mode with all of
them; zero budget violations, checked after every step; and, for unfenced faults in leased mode
with checks switched off, that the run **does** diverge, which confirms the fault injection works.
Refutation: any divergence in a covered run.

### Cost runs (on real stores)

`MemoryStore` misses are nearly free, so policy and arbiter questions are answered on SQLite with
SQLCipher and on PostgreSQL, with the same seeded schedules replayed against each, measuring wall
time per miss by key kind, bytes decoded per request, and resident memory sampled from the process
against accounted bytes.

**Data.** The corpus installed into one tenant and populated by a seeded generator: record counts
per object type, popularity within a type, field sizes from the corpus field definitions; plus a
population of small tenants from platform templates. The popularity distributions are the author's
choice until real traces exist, and the conclusions say so. **Real traces** would come from two
sources: the journal (`crates/simulation/src/journal.rs`), which records commits but not reads, and
a sampled read trace (tenant, need, outcome, no values) emitted by `ResidentStore` in production,
which is a follow-up task.

**Workloads.** Steady reads with a range of skews; scans (`AllOf`) interleaved with point reads;
write bursts; one cell with 1,000 small tenants and 10 large ones under one budget (arbiter, static
split, whole-tenant residency A, and a process per tenant at equal total memory); cold restart with
and without manifests; schema changes and erasures in the middle.

**Measures.** Hit rate by entries, by bytes, and by cost; store loads per request; bytes decoded per
request; steps to 90 percent of steady hit rate after a restart; per-tenant hit rate against
allocated bytes; measured RSS against accounted bytes plus reserves.

**What would refute the design.**

- Measured RSS exceeding accounted bytes plus reserves by more than the calibrated tolerance.
- Header residency alone reaching most of the cost-weighted saving: build the header and stop.
- The cost-aware TinyLFU and GDSF combination not beating CLOCK clearly on cost-weighted hit rate on
  SQLCipher and PostgreSQL: use CLOCK.
- The arbiter not beating a static split: use static budgets with ceilings.
- Whole-tenant residency matching the design within the same budget on the corpus tenant: choose A.
- The checked mode's generation read costing a large share of the saving on SQLite: leases become a
  prerequisite rather than an option.

## Follow-up work, by dependency

1. **Tenant generation** in the store contract: `generation(tenant)`, the in-file counter raised by
   every mutating method, the directory epoch raised by restore, move and recreate, and the
   conformance test that every mutating method raises it. Depends on nothing; everything below
   depends on it.
2. **Header residency** (CPY-014), counters split out. Depends on 1.
3. **`ResidentStore` below sealing**: record entries (present and absent), stamps, refresh on a
   generation change, carry-forward on commit, ambiguous-commit drop, miss coalescing (CPY-012);
   passes the storage conformance suite. Depends on 1 and 2.
4. **Invalidation map in Rust** with a property test of its criterion against `apply_delta`, for
   each key kind. Depends on 3.
5. **Answer, all and ledger entries**. Depends on 4.
6. **Size accounting and calibration**, including the in-flight reserve and the adapter's memory.
   Depends on 3.
7. **Policies**: CLOCK baseline, then cost-aware TinyLFU admission with GDSF eviction. Depends on 3
   and 6.
8. **Differential check** of the Lean model, as a program, against `ResidentStore` on generated
   schedules with the fault kinds above (extending `crates/verification/src/lean.rs`). Depends on 3.
9. **Simulator drives the store path**, with the fault schedule. Independent of 1 to 8.
10. **Corpus loader and population generator**. Independent.
11. **Sampled read trace** from `ResidentStore`. Depends on 3.
12. **The experiments**. Depends on 5, 7, 9 and 10; real-trace runs also on 11.
13. **Leases and fencing**: tenant leases in the directory, revocation before restore, move and
    recreate. Depends on 1 and on directory routing (D-8301).
14. **Hot-set manifest and rate-limited warmup**. Depends on 3.
15. **Cell arbiter** with miss-cost curves, floors and ceilings. Depends on 7, and on 12 showing it
    beats a static split.
16. **Specification changes**: a copy class row for the working set (CPY-001); the generation
    obligation as a storage requirement; the per-tenant memory ceiling as a proposed decision.
    Can proceed alongside 1.

## Answers to the review of the first revision

| # | Finding | Answer |
|---|---|---|
| 1 | Revision is not a safe coherence key | **Fixed.** Confirmed (`memory.rs:1573-1607`, `storage-sqlite/src/lib.rs:4850-4860`, `lib.rs:800-808`). Stamps are a tenant generation (directory epoch, in-file counter) that rises on every change; the model's store changes only through operations that raise it (`Store.set`, `Store.applyDelta`), restore and recreate are `external`/`unfenced` operations, and `sound_of_advance`'s hypothesis is gone |
| 2 | Pins drop on staleness | **Fixed.** Confirmed in the old `admit` (`removeKey` then an unpinned entry). Pins are a set on the cache; `stepWS_pins` proves only pin/unpin change it; `admit_keeps_pinned` now covers the same key |
| 3 | Header lacks revision and the other counters | **Fixed.** The cached header holds structure only; counters are their own key, invalidated by every Rust commit and taken from the generation read or the receipt, so no stale revision reaches `execute_guarded` |
| 4 | No cheap revision read | **Fixed in the design.** Confirmed (`tenant_summary` decodes the header, `storage-sqlite/src/lib.rs:2740-2758`). `generation(tenant)` is follow-up item 1; leased mode needs no store call, checked mode one cheap call |
| 5 | Out-of-commit writers | **Fixed in the design.** Confirmed (`anonymise_ledger_principal`, `remove_principal`, `restore_snapshot`, in-place mutation in `MemoryStore`). Each must raise the generation, held by a conformance test; in the model they are fenced or unfenced writes. The manifest is written outside load results and raises nothing |
| 6 | Erasure with plaintext above sealing | **Fixed.** Confirmed (`sealed.rs:212-217`). The working set sits below sealing; `below_sealing_serves_current` and `above_sealing_serves_erased` state the two placements |
| 7 | Transparency over a schedule space where staleness cannot arise | **Fixed for the model, scoped for the rest.** External, unfenced and fenced writes, restores, recreates, ambiguous commits and stale knowledge are in the schedule; commits and the world are transparent in every schedule, outcomes in covered ones, and the uncovered case is a counterexample. Concurrency within one request is outside the model (W2) and handled by the snapshot-assembly rule and a critical section |
| 8 | Model gaps; hand-listed invalidation | **Partly fixed, partly left open.** Invalidation is now a parameter meeting a semantic criterion, with answer keys in the model and one map proved. The small kernel (one type, no indexes, references, uniqueness, segments, ledger, jobs) is left open: it is the kernel model's scope (D20), and each Rust key kind's criterion is follow-up item 4 |
| 9 | `resize` can exceed the grant; machine sum unproved | **Fixed.** `resize_budget_le` bounds the excess by the pinned bytes; `machine_resize` proves the cell sum under the arbiter's floor rule; the doc says accounted bytes |
| 10 | Accounting misses in-flight `Arc`s, policy structures, adapter memory | **Fixed in the design**: reserves for each, a measured-RSS tolerance, and an experiment that refutes on exceeding it. Not provable in the model |
| 11 | Overstated spec claims | **Fixed.** The spec permits a process per tenant (D-8300, STO-016); the memory ceiling is a proposed decision |
| 12 | Experiments cannot refute coherence; MemoryStore costs; author-chosen workloads | **Fixed in the design**: a fault schedule with a divergence check, cost runs on SQLCipher and PostgreSQL, and real traces from the journal plus a sampled read trace. Workloads stay author-chosen until those traces exist, and the doc says so |
| - | TinyLFU refusing rare, expensive, small entries | **Addressed**: admission compares GreedyDual priorities using the sketch's frequency, so cost and size count |
| - | CPY-012 coalescing undesigned | **Addressed**: one load per key per generation, the others wait |
| - | PostgreSQL never cited; `PairPage`, `Ledger` missing | **Fixed** |
| - | Loose references | **Fixed**: `ws.rs:280` (the WebSocket event-head cache, not tenant data) removed; the `MemoryStore` load counter is `loads()` at `memory.rs:190` |
