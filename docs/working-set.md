# The tenant working set

What a Strata runtime should hold in memory in front of its store, how it decides, and why holding
it can never change an answer. This is a design, with its safety properties proved in Lean
(`lean/Strata/WorkingSet.lean`). Nothing here is implemented yet; the follow-up work is listed at
the end, ordered by what unblocks what.

## The problem

The owner's request: make the runtime load as much of the tenant database as it can into memory,
chosen by sound algorithms rather than blindly; consider one runtime per tenant; make it testable
end to end with the enterprise corpus and the simulator; prove it against the kernel model.

Restated as an engineering problem: a cell process serves many tenants from their stores. Every
request today goes to the store, and the store decodes what it returns on every call. Memory is
finite and shared. Choose what to keep decoded in memory so that the expected cost of going to the
store is as small as possible, within a memory budget that is never exceeded, without ever serving
a value the store would not serve.

## Where the framing should change

The request is the right direction. Several parts of it, taken literally, would make the design
worse. Plainly:

1. **One runtime per tenant: yes as a unit of ownership, no as a process.** The specification
   already decided that a cell is a process serving a subset of tenants, and that a tenant who needs
   a guarantee is placed alone in a dedicated cell (D-8300, D-8304, STO-016). One OS process per
   tenant multiplies the fixed cost of a process (the async runtime, connection pools, the compiled
   platform code, the key service session) by the number of tenants, and most tenants of a
   multi-tenant platform are small and idle most of the time. A process per tenant also makes memory
   sharing impossible: an idle tenant's process holds its memory while a busy neighbour misses.
   What should be per tenant is a **tenant runtime object** inside the cell: the single owner of
   that tenant's working set, its commit path and its revision. A dedicated cell is then the same
   object alone in a process, which is what D-8304 already offers on the top plan.

2. **"Load as much as possible" is the wrong objective; "minimise the cost of misses within a
   budget" is the right one.** Memory in a cell is shared, so a byte held for one tenant is a byte
   not held for another. Loading everything is optimal only when everything fits and everything is
   used; the design below reaches that state by itself when it holds (a whole object type becomes
   resident when it is hot and fits) and degrades gracefully when it does not. A tenant whose data
   is larger than its share, or a cell restart that tries to load every tenant at once, is exactly
   where "as much as possible" goes wrong: the second is a storm of reads against every tenant file
   at the moment the cell is least able to absorb it.

3. **A single global budget, divided by marginal value, beats fixed per-tenant budgets, but not
   alone.** A static split wastes memory on idle tenants. A pure global pool lets one tenant's scan
   evict everyone else, which D-8304 forbids. The recommendation is a cell-level arbiter that moves
   memory to the tenant whose next byte saves the most miss cost, inside a floor (its pinned
   structure) and a ceiling (from its plan, BIL-150) per tenant.

4. **Heaps are an implementation detail; the unit of residency and the cost model are the
   decisions.** GreedyDual-Size-Frequency is naturally a priority queue, and it is a good candidate.
   But what gets cached (decoded headers, records, complete answers to a need) and how the cost of a
   miss is measured matter far more than the queue. A wrong unit with a perfect heap still loses.

5. **The largest saving is not the records; it is the header.** Every `load` on SQLite re-reads
   the tenant row and decodes the whole tenant schema and member list from JSON
   (`crates/storage-sqlite/src/lib.rs:764-797`), and one request loads several times: the header,
   then each round of needs (`crates/storage-api/src/lib.rs:1924-1964`, `crates/backend/src/ops.rs:496-537`).
   With the enterprise corpus (14 products, 573 KB of product source, 73 object types, 83 policies)
   that schema is large. The specification requires it to be reused while the schema version is
   unchanged (CPY-014, tier one of D-17), and nothing in the code does so. This should be built
   first and measured before anything else; it may be most of the win.

6. **On SQLite the store is already partly in memory.** The operating system's page cache, and
   SQLite's own, hold recently read pages. What a resident entry saves on a miss is not a disk read
   but decryption (SQLCipher pages, D-8305), JSON decoding, opening sealed fields (D-350) and the
   query itself. The cost of a miss is therefore something to measure per kind of key, not to
   assume, and the policy must take it as an input.

7. **"Fully test it with the corpus" needs two things that do not exist.** The corpus is tenant
   metadata, 1,290 entities and no records (`tests/corpus/enterprise/README.md`), and nothing loads
   it into a tenant yet. The simulator runs `World::step` over complete in-memory state
   (`crates/simulation/src/engine.rs:1-6`, `crates/kernel/src/world.rs:1-2`) and never calls
   `Store::load`, so it cannot observe a working set at all. Both are prerequisites, listed below.

## The current state

| What | Where | What it does |
|---|---|---|
| The store contract | `crates/storage-api/src/lib.rs:780-904` | `load(tenant, needs)` returns a partial snapshot whose coverage states exactly the needs; `load_complete`; `commit` guarded by the base revision (lines 10-37 state the guarantees) |
| Header always included | `crates/storage-api/src/lib.rs:1676` (`partial_state`) | Every load carries schema, members and counters, whatever the needs |
| Coverage | `crates/kernel/src/state.rs:65-90` | A snapshot answers a need if it loaded that need or `AllOf` its object type; the kernel refuses with `InsufficientState` otherwise (D-2, K7) |
| The command path | `crates/storage-api/src/lib.rs:1876-1990` (`execute_guarded`) | Load header, compute needs, load, refine, decide, commit; reload and retry on conflict |
| The read path | `crates/backend/src/ops.rs:31` (`header`), `:496` (`read_loaded`), `crates/backend/src/view_load.rs:29` | Load the header, then the needs of the read, then whatever the kernel reports missing, up to 32 rounds |
| Whole-tenant loads | `crates/backend/src/lab.rs:530` (`complete`), `products.rs:139`, `product_segments.rs:88`, `subject_export.rs:458` | `load_complete` for Impact Lab, product installs and exports |
| Revisions | `TenantState.revision`, `Store::object_revisions` (`lib.rs:810`; `memory.rs:301`, bumped in `commit_in` at `memory.rs:2176`) | A tenant revision per commit, and a change counter per object type (D-5064) |
| Existing in-process copies | `crates/backend/src/aggregates.rs:1-35` (`COPY_LIFETIME_MS` 60 s, `MOST_COPIES` 2,048), `settings.rs:37-40` (`PlatformCache`, 5 s), `ws.rs:280` (`cached_head`) | Aggregate results keyed by object-type counters; platform settings with a bounded age |
| Open tenant files | `crates/storage-sqlite/src/lib.rs:704` (`DEFAULT_MAX_OPEN` 256), `:1694-1770` | An LRU of open tenant databases, by count, not bytes |
| Header decode | `crates/storage-sqlite/src/lib.rs:764-797` | Schema and members decoded from JSON on every load |
| Sealing | `crates/storage-api/src/sealed.rs:1-35`, `:975` | Personal fields sealed on write and opened on every load; an erased subject's tokens stay sealed |
| The simulator | `crates/simulation/src/lib.rs`, `engine.rs:248-300` | Logical time over `World::step`; no store, no loads, no memory |

So there is no tenant state cache today. Every request goes to the store, each `load` decodes the
header, and the only memory held across requests is aggregate copies, a settings cache, and open
file handles.

## Constraints from the rest of Strata

- **The kernel stays pure** (`docs/assurance.md`, K1). The working set is shell: it lives behind
  the `Store` trait, and the kernel never knows it exists. This is also what makes it safe: the
  kernel already refuses to decide on a snapshot that does not cover its needs (D-2, K7), so a
  working set that serves honest coverage can cause a miss but never a wrong decision.
- **The store is the authority.** Commits are serialised per tenant by the base-revision check
  (S2). The working set may never accept a commit the store would refuse, or serve a value from
  before the tenant's latest commit.
- **The simulator runs what the deployment runs.** The working set must therefore be part of the
  store path the simulator drives, not something the simulator models.
- **Copies have rules.** The specification's copy classes (CPY-001 to CPY-014) apply: keys begin
  with the tenant (CPY-003), a commit supersedes every copy depending on what it changed (CPY-008),
  an outage of the copy is a slowdown, never an error (CPY-011), concurrent misses compute once
  (CPY-012), erasure reaches copies (CPY-013). The working set is a new copy class and needs a row
  in the copy class table.

## Candidate designs

### A. Whole-tenant residency

Each tenant runtime holds its complete `TenantState` (`load_complete`), applies its own commits to
it with `apply_delta`, and the cell evicts whole tenants by recency when over budget. This is the
literal reading of "each tenant gets its own runtime and loads as much as it can".

- For: the simplest possible coherence argument (one revision per tenant; a commit replaces the
  resident state); every hit is a full hit; `Coverage::Complete` everywhere, so no need rounds.
- Against: cold start costs a full load of the tenant, every time it is opened; a tenant larger than
  its share cannot be resident at all, so it falls off a cliff from all hits to all misses; the unit
  of eviction is a whole tenant, so a cell with many medium tenants thrashes; memory scales with the
  data, not with what is used. A large enterprise tenant spends most of its memory on records nobody
  reads.

### B. A record and answer working set per tenant, with a fixed budget

Each tenant runtime holds its header pinned, plus individual records (present or absent) and
complete answers to needs (the ids that answer `ByField`, `PageOf`, `IndexPage`, `AllOf`), each
stamped with the revision it is valid at, evicted by a cost- and size-aware policy within a fixed
budget per tenant.

- For: memory follows use; large tenants degrade gradually; partial loads compose with the kernel's
  coverage model; isolation between tenants is total.
- Against: fixed budgets waste memory on idle tenants and starve busy ones; the budget per tenant
  is a configuration guess that the workload will contradict.

### C. B, with a cell-level arbiter (recommended)

As B, but the budgets are not fixed: a cell arbiter divides the cell's memory between tenant
runtimes by the marginal value of a byte to each, inside per-tenant floors and ceilings. A whole
object type, or a whole tenant, becomes resident when it is hot and fits, so A is reached as a
special case without being the rule.

- For: the memory of idle tenants is lent to busy ones; noisy neighbours are bounded by ceilings;
  the floor guarantees that a tenant can always run.
- Against: the arbiter needs an estimate of each tenant's miss-cost curve, which is a sampling
  structure of its own; it is the most moving parts of the three.

### D. An external shared copy store (rejected for this layer)

Hold tenant state in a shared cache service in front of the stores. Rejected here: it adds a
network hop to every decision, a second coherence protocol across processes, and a second place
where sealed personal data lives decoded. The specification's shared copy store (D-17, tier two)
is for view results, keyed by versions; the working set belongs in the cell process, next to the
kernel that reads it.

### Policies weighed

| Policy | Strength | Weakness for Strata |
|---|---|---|
| LRU / CLOCK | Simple, cheap per access, CLOCK lock-friendly | Size- and cost-blind; one `AllOf` scan of a large type flushes the set |
| 2Q / ARC | Scan-resistant, adapts recency against frequency | Size- and cost-blind; ARC's ghost lists cost memory per tenant |
| LFU with decay | Captures stable popularity | Slow to adapt; cost- and size-blind without extension |
| GreedyDual-Size(-Frequency) | Priority `L + frequency * cost / size`, with inflation `L` for ageing; cost- and size-aware, a heap | Needs a cost per key; a heap per tenant with lazy deletion |
| Knapsack (value per byte) admission | The right objective: value of a byte | Exact 0/1 knapsack is NP-hard; the greedy by density is optimal only fractionally |
| W-TinyLFU admission | A small frequency sketch refuses one-hit wonders before they evict anything; very scan-resistant | Admission only; needs an eviction policy behind it |
| Structural pinning | The header, compiled schema, and the closures the kernel reads on every decision are never evicted | Pinned bytes are a floor the budget must cover |

The recommendation combines the last three columns: pin the structure, admit through a TinyLFU
sketch, evict by GreedyDual-Size-Frequency with measured costs. Plain CLOCK is kept as the
baseline every experiment must beat; if the combination does not beat it clearly, CLOCK wins on
simplicity.

## The recommended design

### Where it sits

A `ResidentStore` wraps a store and implements `Store`. It sits above `SealedStore`, so a hit
skips opening sealed fields. Backend, simulator and conformance suite all use it without change,
and the storage conformance suite run against it is its first test. One `TenantResidency` per
tenant holds that tenant's entries; a cell-level arbiter holds the budgets.

### What can be resident

| Key | Value | Pinned | Notes |
|---|---|---|---|
| `header(tenant)` | decoded schema, members, counters, setting values | yes | What every load returns (`partial_state`); CPY-014's compiled schema hangs off it, keyed by schema version and schema hash, as aggregate copies already are |
| `record(tenant, object, id)` | the record, **or its absence** | no | Absence is cached too: a lookup of a missing id is as much a fact at a revision as a present one |
| `answer(tenant, need)` | the complete set of ids answering a `ByField`, `ByFields`, `PageOf`, `IndexPage` or `AllOf` need | no | Valid only as a complete set; the records themselves are `record` entries, so an id held in two answers is counted once |
| derived structures | the indexes, reference closures and policy dependencies the kernel reads on most decisions | by choice | Pinned when the tenant's measured access says every decision reads them |

A request's snapshot is assembled from resident entries where they are current and from one
`load` of everything that is not, so a request still makes at most one store call per need round,
and usually none.

### Memory accounting

A byte of residency is a byte of heap held by the working set because of an entry: the decoded
value (for a record, the `Arc<Record>` and its field map; for an answer, the id set), plus the
entry's own key and bookkeeping, plus its node in the policy's structures. It is computed, not
sampled, by a deterministic size function of the entry (assumption W3 in the Lean model): the
encoded canonical length of the value times a per-kind factor, plus a fixed per-entry overhead.
The factors are calibrated once against a counting allocator in a test harness over the corpus
tenant, and recalibrated when the record representation changes. The same function is used for
admission, eviction and the arbiter, so accounting can never disagree with itself. What is not
counted against tenants: the cell's fixed costs, which come out of the cell's total before the
arbiter divides it.

### Admission and eviction

- **Pinned entries** are admitted unconditionally when their bytes fit inside the tenant's floor,
  and are never chosen for eviction. A pinned admission may give up every unpinned entry of the
  tenant to fit (`pin_resident`).
- **Unpinned entries** pass a W-TinyLFU frequency sketch per tenant (a count-min sketch with
  periodic halving, a few bytes per tracked key): a new key is admitted only if its estimated
  frequency exceeds that of the victim it would displace. This keeps a large `AllOf` load, which a
  named query makes today (`docs/storage.md`, Limits), from flushing the set.
- **Eviction** is GreedyDual-Size-Frequency: priority `H = L + f * c / s`, where `f` is the
  sketch's frequency, `c` the measured reload cost of the key's kind for this tenant, `s` its bytes,
  and `L` the priority of the last evicted entry. A min-heap with lazy deletion holds the
  priorities.
- **Whatever the policy says, the runtime enforces the budget**: if an entry does not fit after the
  policy's victims are gone, it is not admitted, and the request is served from the store. The
  policy is advice; the budget, the pinning and the revision stamps are not (`stepWS_within`, and
  every theorem below quantifies over the policy).

### Coherence against committed revisions

- **The store is the authority.** Every entry carries the tenant revision at which it was read or
  last confirmed. An entry is served only if its stamp equals the tenant's current revision
  (`hit_current`); otherwise it is a miss and the store answers (`serve_eq_fetch`).
- **Commits go through the tenant runtime**, which is the single writer of its tenant within the
  cell (the directory routes a tenant to one cell, D-8301). After the store accepts a commit, the
  runtime drops every entry the delta touches, confirms every other current entry of the tenant at
  the new revision, and re-admits a pinned header that the commit changed (`carry`, `commitWS`).
- **What a delta touches**, exactly: a create touches the header (the record counter), the new
  record's id (which may be cached as absent) and every answer over its object type; an update or
  delete touches that record and every answer over its object type; a schema change touches the
  header and everything derived from the schema version. Answers are invalidated per object type,
  the same granularity as the change counters of D-5064. That the delta names every key it changes
  is a theorem in the model (`untouched_unchanged`); the Rust counterpart is a property test over
  generated deltas.
- **Another writer** (an operator tool, a restore, a cell move in progress): if the store moves on
  behind the runtime's back, entries simply stop being served as long as the runtime reads the
  tenant's current revision before serving (`sound_of_advance`). The header read every load
  already makes is that check. With the single-writer lease it can be skipped; without it, it must
  not be.
- **Explicit invalidations**: restoring a snapshot, moving a tenant, and erasure. An erasure that
  destroys a subject key invalidates the tenant's resident records (CPY-013), since resident
  records hold opened values; erasures are rare enough that the whole tenant's records are dropped
  rather than tracking subjects per entry.

### Cold start

The header is loaded and pinned synchronously when a tenant is first routed to the cell. Records
and answers fill on demand. To shorten the cold period without loading blindly, each tenant
runtime periodically writes a **hot-set manifest** (keys and sketch counts, no data) to its own
tenant storage; on open, the runtime prefetches from the manifest in priority order, within its
floor plus a warmup allowance, in the background and rate-limited per cell so that a cell restart
does not open every tenant file at once. A tenant that is never asked for is never warmed.

### Under memory pressure

The arbiter lowers a tenant's budget; the policy names victims; if that is not enough, every
unpinned entry goes; the budget never goes below the pinned bytes (`resize`,
`resize_budget_exact`). The cell's own limit (STO-013) is the arbiter's total. If the pinned floors
of the cell's tenants exceed it, the cell is over-placed, which is a placement error to alarm on:
tenants are still served correctly, but every non-header read goes to the store.

### Sharing a machine

- Each tenant runtime estimates its miss-cost curve (miss cost as a function of budget) by spatial
  sampling of its access stream into small ghost caches at a few sizes (SHARDS-style sampling,
  measured not assumed), and reports it to the arbiter.
- The arbiter divides the cell's budget in chunks to the tenant whose curve shows the largest
  saving per byte (utility-based partitioning over the convex hull of each curve), between the
  tenant's floor (pinned bytes) and ceiling (from its plan, BIL-150).
- A tenant that needs a guarantee is placed in a dedicated cell (STO-016) rather than given a
  bigger slice.

The proof that this composes is `machine_within`: working sets that each keep their own budget,
under budgets that sum to the cell's memory, together stay within it.

## What is proved

`lean/Strata/WorkingSet.lean`, imported from `lean/Strata.lean`, builds on the kernel model of
`lean/Strata/Model.lean`. The policy is a parameter (`Policy σ`, with a state of any type, an
admission test, a victim choice and its own bookkeeping), so every theorem holds for any policy.
The model's operations are reads (read-through on a miss, admitting the value), pins, evictions,
explicit invalidations, budget changes and commits made through the working set. Assumptions are
W1 to W4 at the head of the file. No `sorry`, no added axioms; `lake env lean Audit.lean` lists
only `propext`, `Classical.choice` and `Quot.sound`.

| Property | Theorem | Statement in words |
|---|---|---|
| Coverage soundness | `decideTenant_faithful`, `decideOn_fetch` | A decision made from the header, the records the intent names and (for a strict schema change only) the record list, then applied as a delta, is exactly the kernel's `step` |
| Invalidation completeness | `untouched_unchanged` | The value under any key the delta does not touch is the same before and after the commit |
| Coherence | `hit_current` | A served entry is stamped with the tenant's current revision |
| | `serve_eq_fetch` | Every read through a sound working set returns the store's current value |
| | `stepWS_sound`, `runWS_sound`, `commit_sound` | Soundness is preserved by every operation, so across every schedule |
| | `read_after_commit` | After a commit, a read returns the committed value |
| | `sound_of_advance` | Another writer behind the working set breaks nothing, given the revision read |
| Budget | `stepWS_within`, `runWS_within` | Resident bytes never exceed the budget, across admit, evict, invalidate, resize and commit |
| | `resize_budget_exact` | The arbiter gets any budget at or above the pinned bytes |
| | `machine_within` | Per-tenant budgets that sum to the machine's memory keep the machine within it |
| Pinning and isolation | `admit_keeps_pinned`, `evict_keeps_pinned`, `resize_keeps_pinned` | No admission, eviction or resize removes a pinned entry |
| | `pin_resident` | A pinned admission succeeds whenever the pinned entries fit in the budget |
| | `commit_keeps_other_tenants`, `commit_keeps_untouched` | A commit leaves other tenants' entries alone and keeps the current entries it does not touch |
| Transparency | `stepWS_transparent`, `runWS_transparent` | For every schedule and every policy, the outcomes and the committed world equal those of the same schedule with no working set |

The two central statements, as proved:

```lean
theorem runWS_transparent (ops : List Op) :
    ∀ (sys : Sys σ), Sound sys.cache.entries sys.store →
      (runWS pol size sys ops).1.store.world = (directRun sys.store.world ops).1 ∧
      (runWS pol size sys ops).2 = (directRun sys.store.world ops).2

theorem runWS_within (ops : List Op) :
    ∀ (sys : Sys σ), Within size sys.cache →
      Within size (runWS pol size sys ops).1.cache
```

An empty working set satisfies both hypotheses (`empty_sound`, `empty_within`).

### What is not proved, and what turned out false

- **"The pinned closure is always resident" is false as stated.** Two counterexamples: a budget
  smaller than the pinned bytes (nothing can make them fit), and a commit that grows the header
  (a schema change adding fields) so that the re-admitted header no longer fits in a budget the old
  one did. What is proved instead is conditional: pinned entries are never evicted, and a pinned
  admission succeeds whenever the pinned reserve fits (`pin_resident`). The runtime must therefore
  hold each tenant's floor at least at its pinned bytes, and the arbiter must raise a floor before
  a schema change that grows it, or accept that the tenant runs header-unpinned until it does.
- **Carry-forward must confirm only current entries.** Confirming every untouched entry at the new
  revision, including one already stale, would be unsound: an entry stamped at revision 3, when
  the tenant is at 5 and its value changed at 4, would be confirmed at 6 and served. `carry`
  confirms only entries stamped at the revision the commit was based on, and the soundness proof
  depends on exactly that.
- **No policy optimality is proved.** GreedyDual-Size has a known competitive ratio against the
  offline optimum under its cost model, and greedy by value density is optimal only for the
  fractional knapsack; neither is proved here, and neither is a safety property. Whether the policy
  is good is a question for the experiments, not the proofs.
- **Concurrency inside the runtime is outside the model.** The model is sequential. The Rust must
  make each tenant runtime's commit and carry-forward one critical section against reads of that
  tenant, so that a read sees either the old revision's entries or the new one's, never a mix.
- **The model has one object type and one set-valued key.** Set answers appear only as `all`. The
  per-object-type invalidation of answers in the Rust design is the same argument at a finer key,
  and needs its own property test until the kernel model grows object types.
- **Correspondence with the Rust code is evidence, not proof** (D20 in `docs/decisions.md`). A differential run of the
  model as a program against `ResidentStore` is a follow-up task.

## Experiments that would test it

All runs are deterministic and logical (the simulator's discipline). Costs come from a calibrated
cost model consuming traces, carrying the empirical evidence class (`crates/simulation/src/lib.rs`).

**Setup.** The corpus installed into one tenant (its 14 products), populated by a seeded generator
with record counts per object type, Zipf-skewed popularity of records within a type, and field
sizes drawn from the corpus field definitions; plus a population of small tenants built from
platform templates. The simulator drives the store path (`execute_guarded`, `read_loaded`) through
`ResidentStore` over `MemoryStore`, so the store's `loads` counter (`memory.rs:189`) counts misses.

**Workloads.**

1. Read-heavy steady state on the corpus tenant: view reads and record reads, Zipf exponents from
   0.6 to 1.2.
2. Scans: named queries over large types (`AllOf`) interleaved with point reads, to test scan
   resistance.
3. Write bursts: creates and updates concentrated on a few types, to measure invalidation churn.
4. One cell, many tenants: 1,000 small tenants with a long tail of activity and 10 large ones,
   under one cell budget; compare the arbiter, a static even split, and whole-tenant residency (A).
5. Cold start: a cell restart with every tenant routed back, with and without manifests.
6. Schema changes and erasures in the middle of steady state.
7. Replay of recorded journals (`crates/simulation/src/journal.rs`), when production journals exist.

**Measures.** Hit rate by entries, by bytes, and by cost (miss cost weighted); store loads per
request; bytes decoded per request (the reload latency proxy); steps to reach 90 percent of steady
hit rate after a restart; per-tenant hit rate against allocated bytes; budget violations (checked
after every step, must be zero); and a differential check that every outcome and the final state
hash are identical with and without the working set.

**What would refute the design.**

- Any difference in outcome or state hash between runs with and without the working set: a bug,
  and if it survives a fix, a flaw in the coherence rule.
- Any budget violation.
- Header residency alone reaching most of the cost-weighted saving of the full design: then build
  the header and stop.
- W-TinyLFU with GreedyDual-Size-Frequency not beating CLOCK on cost-weighted hit rate on skewed
  workloads by a clear margin: use CLOCK.
- The arbiter not beating a static split on workload 4: use static budgets with ceilings.
- Whole-tenant residency (A) matching the design's cost-weighted hit rate within the same budget on
  the corpus tenant: choose A, since it is simpler.

## Follow-up work, by dependency

1. **Header residency** (CPY-014): decoded header and compiled schema held per tenant, keyed by
   revision, schema version and schema hash, inside a new store wrapper. Depends on nothing.
2. **`ResidentStore`**: record entries (present and absent), stamps, carry-forward on commit, the
   explicit invalidations; passes the storage conformance suite. Depends on 1.
3. **Answer entries**: complete need answers, invalidated per object type. Depends on 2.
4. **Size accounting and its calibration harness**. Depends on 2.
5. **Policies** behind a policy trait: CLOCK as the baseline, then W-TinyLFU admission with
   GreedyDual-Size-Frequency eviction. Depends on 2 and 4.
6. **Differential check** of the Lean working-set model, as a program, against `ResidentStore` on
   generated schedules (extending `crates/verification/src/lean.rs`). Depends on 2.
7. **Simulator drives the store path** instead of `World::step` (also required by the simulator
   direction in `docs/assurance.md`). Independent of 1 to 6.
8. **Corpus loader and population generator**: install the 14 products into a tenant and populate
   it with seeded records. Independent.
9. **The experiments above**. Depends on 3, 5, 7 and 8.
10. **Hot-set manifest and rate-limited warmup**. Depends on 2; evaluated in 9.
11. **Cell arbiter** with miss-cost curve sampling, floors and ceilings. Depends on 5, and on 9
    showing that it beats a static split.
12. **Single-writer tenant lease** within a cell, so the revision check before serving can be
    skipped. Depends on the control directory routing (D-8301).
13. **A row for the working set in the copy class table** of the specification (owner, key,
    freshness bound, erasure point; CPY-001). Can proceed alongside 1.
