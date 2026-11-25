# Formal verification: the IronFleet method applied to Strata

What it would take to prove Strata's backend, and its browser kernel, correct in the way IronFleet
proved its systems correct, with the specification written in Lean. This is research and a
proposed design. It is not a description of what exists; `docs/verification.md` is.

## 1. What IronFleet did

IronFleet (Hawblitzel et al., SOSP 2015; CACM 2017) built two distributed systems, IronRSL (a
Paxos replicated state machine) and IronKV (a sharded key-value store). It proved that their
running implementations refine a small, readable specification, and that they make progress. Both
are written and proved in Dafny and compiled to C#.

### Three layers, two refinement proofs

```
High-level spec      a centralised state machine: SpecInit, SpecNext, SpecRelation    (trusted, small)
      ^  refinement proved TLA-style (invariants, a refinement function PRef)
Distributed protocol N abstract host state machines + a set of network packets      (proved, not trusted)
      ^  refinement proved Floyd-Hoare style (a refinement function HRef per host)
Implementation       imperative, single-threaded code per host, real bytes and sockets (proved)
```

- **Spec.** One state machine that describes every allowed behaviour of the whole system, as if it
  ran on one machine. `SpecRelation` only talks about what is externally visible (the messages
  sent). It is kept tiny so that a sceptic can read it: **85 lines for IronRSL, 34 for IronKV**,
  plus 208 lines of temporal logic.
- **Protocol.** Each host is a declarative state machine (`HostInit`, `HostNext`) over abstract
  values: unbounded integers, immutable sequences, structured packets, no marshalling. In one step a
  host atomically receives, updates its state and sends. Proved: this layer refines the spec.
- **Implementation.** Real code with bounded integers, mutable arrays, marshalling and a UDP
  interface. Proved: each `ImplNext` refines one `HostNext`, and every byte array sent parses back
  to the packet the protocol meant. The two refinements compose into "the running system refines
  the spec".

### The reduction obligation (why one atomic step is enough)

Real hosts interleave. IronFleet constrains every implementation step to do **all receives before
all sends**, with **at most one time-dependent operation** (a clock read, or a receive that returns
nothing) between them. Hosts cannot see each other's state except through packets, so any real
interleaving can be reordered into one where each host's step is contiguous, without changing what
anyone observes. Dafny enforces the obligation on the code. The argument that the obligation is
enough is a paper proof, not a machine-checked one (the authors list that as future work).

### Liveness

Safety says nothing bad happens; liveness says the system eventually does what it must (a request
gets a reply). IronFleet embedded TLA in Dafny, built a library of TLA proof rules verified from
first principles (1,824 lines), and wrote its protocols with **always-enabled actions** ("if you
hold the lock, pass it on; otherwise do nothing"), which makes fairness easy to state and prove.
Assumptions: for IronKV, every host's main loop runs infinitely often and the network is fair; for
IronRSL, a quorum runs with a minimum frequency, never runs out of memory, and the network
eventually delivers among them synchronously.

### What is trusted

- The high-level spec, the statement of the top theorem (not its proof), and the short main loop
  that calls `ImplInit` and then `ImplNext` forever.
- The trusted UDP specification, the network not tampering with packets, and packet addresses.
- Dafny, the .NET compiler and runtime, Windows, the hardware.
- The C# client was not verified.

### Size (as reported, Figure 12)

| | Spec | Implementation | Proof |
|---|---|---|---|
| Total | 1,400 | 5,114 | 39,253 |

Proof annotation to executable code at the implementation layer: 3.6 to 1. Over the whole system
the proof is about 7.7 times the implementation. Performance: IronRSL's peak throughput was within
2.4x of an unverified MultiPaxos; IronKV was competitive with Redis. Both systems worked the first
time they were run, apart from the unverified client.

### Lessons the authors state

1. Write invariants over the **set of sent messages** (it only grows), not over mutable host state.
2. **Model imperative code functionally first**: prove a functional implementation, then replace
   value types with mutable ones as a narrow second proof.
3. Use automation judiciously: hide definitions from the solver, control unfolding, avoid
   alternating quantifiers.
4. Missing features (reconfiguration, crash recovery with disk) need more technique, not only more
   work.

### What came after

- **Verus** (SOSP 2024) ported IronKV from Dafny to Verus, which verifies Rust directly.
- **Anvil** (OSDI 2024) used Verus and a TLA-style embedding to prove liveness of Kubernetes
  controllers written in Rust.
- **Veil** (CAV 2025) is a Lean 4 framework for distributed protocols: model checking, SMT-backed
  automatic proofs and interactive proofs in one tool. As of March 2026 it did not support liveness
  natively, and it runs on Windows only through WSL2.
- **Cedar** (AWS) is the closest relative of what Strata does today: an executable Lean model with
  proofs, and the production Rust checked against the model by differential random testing,
  millions of cases a night. That gives high confidence, but it is not a proof of the Rust.
- **Aeneas** translates safe Rust into Lean so that the Rust itself can be proved. Microsoft uses it
  for SymCrypt's Rust (ML-KEM, SHA-3).

## 2. How Strata lines up with it

Strata already has the shape IronFleet needed:

| IronFleet | Strata today |
|---|---|
| Host step is a pure function of state and received messages | `strata_kernel::transition` and `read` are pure: no clock, no randomness, no I/O (`crates/verification/tests/purity.rs`) |
| Main loop receives, steps, sends | The backend loads a snapshot, runs `transition`, commits delta, events and journal in one transaction guarded by the tenant revision, then publishes from the outbox |
| Browser client unverified | The browser kernel is pure: `update(state, message) -> { state, effects }` and `view(state)` in `web/src/kernel`, effects run by a driver in the worker |
| Spec small and formal | The specification is about 10.8 MB of JSONC, a third of it prose; the only formal kernel semantics is `lean/Strata/Model.lean` (about 470 lines, about a tenth of the kernel's decisions) |
| Implementation proved against the protocol | The Rust kernel (30,765 lines) is compared with the Lean model on 87 committed scenarios (about 1,400 operations); nothing is proved about the Rust |
| Protocol proved against the spec | Exegra runs behaviour models (`m-` claims) on listed scenarios under injected faults: model checking of chosen runs, not proofs, and not connected to the code |

The kernel's purity is the most valuable property Strata has for this work: everything the kernel
decides can be proved without reasoning about I/O.

## 3. The design: IronFleet's layers in Lean

### Layer A: the kernel, specified and implemented in Lean

1. **Kernel spec (trusted, kept small).** A declarative Lean definition of what a transition and a
   read mean: given a tenant state, an intent and a context, which outcomes are allowed, and what
   state follows. Relations, not algorithms. The requirements of the kernel areas (`KRN-`, the
   authorisation, schema, ledger and so on) become theorem statements over it, and the review of a
   requirement becomes the review of a theorem statement. The aim is a spec a person reads in one
   sitting, as IronRSL's 85 lines are.
2. **Kernel implementation (proved, not trusted).** Executable Lean, written for speed (arrays,
   bounded integers, indexes), proved to refine the spec: for every input, its result is one the
   spec allows. Following lesson 2, a functional version first, then the optimised one as a narrow
   second proof.
3. **Running it.** Lean compiles to C. The implementation is compiled and linked into the backend,
   which calls it through a C ABI (`@[export]` functions). Data crosses the boundary as canonical
   bytes, with `decode (encode x) = x` proved in Lean, as IronFleet proved its marshalling. The Rust
   kernel stays during the move as a differential oracle (Cedar style, at scale), and is deleted
   once the Lean kernel is what runs.

The existing slices plan in `docs/verification.md` ("Gap analysis") is the list of kernel decisions
the spec has to cover; it is reused as the order of the spec work.

### Layer B: the browser kernel, the same way, compiled to WebAssembly

The browser kernel is `update` and `view`, pure. It gets its own Lean spec (the safety properties
already modelled in `lean/Strata/Client/` are the start), a Lean implementation proved against it,
and is compiled for the browser: Lean to C, then Emscripten to WebAssembly, loaded by the worker.
The driver (fetch, WebSocket, IndexedDB, timers) and the main thread's DOM patching stay JavaScript
and are trusted, as IronFleet's UDP layer was.

**State of Lean on WebAssembly (checked 2026-11-25).** Lean 4.36.0 is current. Upstream has not
built for WebAssembly since v4.15.0 (the last release with a `linux_wasm32` archive). An RFC to
re-enable the WebAssembly CI job (lean4 #16134) and its two pull requests (#16135, and #16142,
which moves it to Emscripten 6.0.13) are open, not merged. Community projects run Lean compiled to
WebAssembly (an in-browser language server on 4.35.0; a kernel on 4.32.2 reported to agree with the
native build on its test set). So the route works, but today Strata would own the toolchain:
building the Lean runtime with Emscripten, keeping it working across Lean releases, and checking
that the WebAssembly build decides exactly what the native build decides (the same differential
fixtures, run on both). The size of the resulting module and its speed are unmeasured and must be
measured before the decision is final.

Note what this does and does not need. Strata does not need Lean (the compiler) to run in the
browser; it needs one compiled Lean program, plus the Lean runtime, as WebAssembly. That is a
smaller problem than the one the upstream CI job addresses.

### Layer C: the backend as a distributed system

This is IronFleet proper.

1. **System spec (trusted).** A centralised state machine: each tenant is a sequence of accepted
   intents applied atomically by the kernel spec, sessions map to principals, and a subscriber
   observes an event only if the kernel says that principal may read the record. `SpecRelation`
   talks only about what clients can observe: replies, and frames on the socket.
2. **Protocol.** Each process is an abstract host state machine: browser worker, BFF, backend,
   outbox publisher, job worker, and the store. Messages are the requests, replies and frames of
   `crates/protocol`. Proved: the hosts together refine the system spec, with invariants over the
   set of sent messages (lesson 1). Liveness: a committed event is eventually published, a lease
   that expires is eventually retaken, a request with a live backend eventually gets a reply.
3. **Implementation.** Each host's step is a pure Lean function from its state and the inputs it
   received to its new state and the outputs it sends. That is the reduction obligation by
   construction: receives, then at most one time-dependent input, then sends. A thin trusted shell
   per process does the real I/O (sockets, HTTP framing, the SQLite calls) and calls the step
   function in a loop, as IronFleet's `Main` does.
4. **Storage.** SQLite is given a trusted specification of the transactional semantics Strata relies
   on (a committed transaction is durable and atomic; a revision guard rejects a stale write), the
   way IronFleet specified UDP. That specification is part of what a reader must trust.

## 4. What stays trusted

After all three layers, a reader must trust:

- the kernel spec, the browser-kernel spec and the system spec, and the statements of the top
  theorems;
- the I/O shells, and the trusted specifications of SQLite, the network and the browser APIs;
- Lean's kernel, Lean's compiler and its C output (the compiler is not verified), the C compiler,
  Emscripten and the browser's WebAssembly engine, the Rust FFI glue, the OS and the hardware.

IronFleet trusted the equivalent list. Nothing removes the last bullet; it can only be kept short
and named.

## 5. Decisions (owner, 2026-11-25)

1. **The kernel runs as compiled Lean.** Layer A replaces the 30,765-line Rust kernel with
   compiled Lean, called from the backend through a C ABI. The Rust kernel is kept only as a
   differential oracle until the Lean kernel runs in its place, then deleted. (Not chosen: proving
   the Rust itself with Aeneas or Verus.)
2. **Concurrency is kept and proved.** The backend keeps serving a tenant's requests concurrently,
   with optimistic transactions resolved by the revision guard and retry. The proof covers that:
   a host step may be interleaved with other steps of the same tenant inside the backend, and the
   argument that every such execution is equivalent to one where committed transactions happen one
   at a time, in revision order, is **machine-checked in Lean**. IronFleet's own reduction argument
   was a paper proof; this one is not allowed to be. (Not chosen: serialising each tenant onto one
   event loop.)
3. **The prototype's topology is proved first.** BFF, backend, outbox publisher, job worker and
   SQLite on one machine. The specification's target topology (a PostgreSQL database per tenant,
   identity service, gateway, replication, edge copies) comes after.
4. **The Lean spec is the authority for the kernel and the protocol.** The JSONC requirements of
   those areas point to the theorems that state them. The JSONC remains the authority for
   everything the Lean does not cover.

## 6. Order of work, by dependency

```
A1 kernel spec in Lean (slices of the gap analysis)        S1 FFI spike: Lean compiled into the backend
      |                                                    S2 WebAssembly spike: a Lean program in the worker,
A2 executable Lean kernel + refinement proof                  size and speed measured, native and Wasm agree
      |   (differential testing against Rust meanwhile)          |
A3 Lean kernel runs in the backend (needs S1); Rust kernel deleted
      |                                                    B1 browser-kernel spec (from lean/Strata/Client)
C1 system spec + protocol hosts + refinement                     |
      |                                                    B2 Lean browser kernel + proof, runs in the worker (needs S2)
C2 TLA-style liveness library in Lean (Veil has no liveness)
      |
C3 host step functions in Lean, trusted shells, SQLite spec
```

C4 (needs C1): the concurrency reduction, machine-checked: concurrent optimistic transactions of one
   tenant, guarded by revision and retried on conflict, are equivalent to a serial order of the
   committed ones, and the receive-before-send obligation of every host step is enough for the
   distributed reduction (IronFleet's section 3.6 argument, mechanised).

S1 and S2 retire the two biggest technical risks and depend on nothing, so they go first, beside
A1.

The work is tracked in Workgraph under WG-5648: S1 WG-5649, S2 WG-5650, A0 (differential random
testing of the Rust kernel against the model, at scale) WG-5651, A1 WG-5373, A2 WG-5652, codec
WG-5653, A3 WG-5658, A4 (requirements point to theorems) WG-5662, B1 WG-5654, B2 WG-5659, C1
WG-5655, C2 WG-5656, C3 WG-5661, C4 WG-5660.

## 7. S1 result: compiled Lean called from Rust (WG-5649)

`crates/lean-kernel-ffi` calls compiled Lean from the Rust workspace on Windows with the MSVC
Rust toolchain. Recipe (all of it in the crate's `build.rs`, needing only `lake` and `leanc` on
PATH):

1. `lake build` in `crates/lean-kernel-ffi/lean` compiles the Lean module to C.
2. `leanc` (Lean's bundled clang) compiles that C and `lean/shim.c`, and links both into
   `strata_ffi.dll` with an import library. The shim exposes a plain C ABI (pointer and length in;
   pointer, length and an owning handle out; release), so Rust never includes `lean.h`, whose
   functions are mostly inline and compiled with clang-only features. The toolchain's clang ships no
   libc headers, so the shim uses only `lean.h`.
3. The DLL imports the Lean runtime and `Init` from the toolchain's `libInit_shared.dll`, which
   depends only on Windows system DLLs. `build.rs` copies both DLLs next to the profile's binaries.

Runtime rules: `lean_initialize_runtime_module`, the module's initialiser
(`initialize_<package>_<Module>`, here `initialize_strataffi_StrataFfi`), then
`lean_io_mark_end_initialization`, once per process. Every other thread that calls into Lean is
registered with `lean_initialize_thread` and unregistered with `lean_finalize_thread` at exit; both
are exported by the runtime but not declared in `lean.h`. The crate does this with a thread-local
guard, so callers on tokio worker threads do nothing. The test
`many_tokio_worker_threads_call_lean_at_once` runs 64 blocking tasks on an 8-worker runtime, 200
calls each, all checked against a Rust reference. `crates/lean-kernel-ffi` is the one crate allowed
`unsafe`; every unsafe block is a call across the boundary.

Measured (release build, this machine, 2026-11-25):

| Payload | Boundary alone (identity function) | Lean step | Same step in Rust |
|---|---|---|---|
| 16 bytes | 69 ns | 258 ns | 121 ns |
| 1 MiB | 0.30 ms | 8.5 ms | 0.60 ms |

The boundary costs about 70 ns a call plus two copies of the bytes. The Lean code itself was about
2x slower than Rust on a small input and 14x on a byte loop over 1 MiB (`ByteArray.get!` and
`push`). The speed of the executable Lean kernel is therefore the risk to watch in A2, not the
boundary. Size: `strata_ffi.dll` is 66 KB for the spike; the backend gains a run-time dependency
on `libInit_shared.dll` (28.6 MB, the Lean runtime and standard library). The backend binary itself
does not grow, since the link is dynamic.

### A0: differential random testing at scale

The fixture scenarios (87 committed, about 1,400 operations) are a floor. A0 runs the same
comparison on many more random scenarios, in the way Cedar tests its Rust implementation against
its Lean model: a generator writes the scenarios in batches, each batch goes to
`lake exe strata_model` in one process and through the Rust kernel in process, and the outputs are
compared scenario by scenario.

```sh
cargo run -p strata-verification --example drt -- --seed <S> --count <N> [--batch 1000] \
    [--out target/drt] [--generator full|fixture] [--require-coverage]
```

Two generators are available, both deterministic in the seed (`crates/verification/src/lean.rs`,
`ScenarioStream`):

- `full` (the default) emits every intent of `lean/Main.lean` and every value form of the scenario
  format: patch with and without `v=`, update and delete with right and wrong versions, references
  in fields 4, 5 and the lists 0 and 8 to existing, deleted and never-issued records, lists of
  scalars and of references, strings and lists one below, at and above their bounds, records near
  the record bound, required-field changes, strangers, members of the other tenant and a tenant
  that does not exist. It is `lean_diff::generate`, one scenario per case seed.
- `fixture` is the generator of the committed fixtures (`scenarios`). It covers the create,
  full-replacement update, delete and schema-change intents, and only the first slice of the
  model: no patches, no expected versions, no lists, no bound tests. A run with it cannot find a
  divergence in the later slices, which is why `full` is the default. The committed fixtures are
  unchanged by the full generator.

The run prints the counts of scenarios, operations, batches and disagreements, and the elapsed
time. Exit status 1 means a disagreement. The first one is minimised by delta debugging (whole
scenarios, then the operations of each, while the model and the kernel still disagree) and written
to `<out>/h-drt-<seed>-<n>.txt` as a hand-written scenario. Appending that scenario to
`lean/fixtures/scenarios.txt` and running `bless_fixtures` turns it into a committed regression
case. A disagreement is a bug in the model or in the kernel until shown otherwise.

Each scenario starts from the model's initial world, so the minimiser's candidates are scenarios
of their own. Their Lean runs happen only when there is a disagreement to shrink.

**Coverage.** The run also prints how often the model's output showed each outcome kind: one row
per error the model can print (`err <kind>`, read from `showErr` in `Main.lean`) and one per
accepted intent kind (`create ok`, `update ok`, `patch ok`, `delete ok`, `require-strict ok`,
`require-lax ok`). A kind that never occurred is marked `<- MISSING` and reported as a coverage
gap. `--require-coverage` makes a gap fail the run, so a generator that stops reaching a rule
cannot pass quietly. A line the model could not read is counted as `unparsed`. Coverage says which
outcomes occurred, not whether each was reached through the rule it names; the unit test
`the_full_generator_emits_every_operation_form` checks the forms of the generator itself.

**Results (2026-11-25, release build).**

- `--seed 20261125 --count 51000 --batch 1000 --require-coverage` (full generator): 51,000
  scenarios, 1,015,305 operations, 51 batches, **0 disagreements**, every outcome kind present,
  92.1 s. At 50,000 scenarios (995,674 operations) the smallest counts were `update ok` 2,294,
  `patch ok` 5,848, `migration_rejected` 9,671 and `still_referenced` 11,419.
- Planted divergence: the model's update version-conflict refusal changed to `invalid`
  (`lean/Strata/Model.lean`, the `.update` arm). `--seed 7 --count 2000 --batch 500` found 289
  disagreements; the first was minimised from 24 operations to 2 and reproduced on its own
  (`lean err invalid`, `kernel err version_conflict`). The same planted change was missed with 0
  disagreements by the fixture generator, which is why it is not the default.
- Not yet exercised: uniqueness (the fixture schema has no unique field until slice 3 of WG-5373
  lands) and `require` on an undeclared field, where the kernel may define a field the model does
  not; that difference is to be settled in the model, not hidden by the generator.

## 7b. Boundary codec (WG-5653)

The values that cross between the Rust backend and the compiled Lean kernel have one canonical
byte encoding, proved in Lean and mirrored in Rust:

- `lean/Strata/Codec/Core.lean`: the combinators and primitives, with `Lawful e d` (below) proved
  for each;
- `lean/Strata/Codec/Boundary.lean`: the boundary types of `Strata.Model`;
- `lean/Strata/Codec/Fixtures.lean`: byte fixtures and the summary theorem `boundary_codecs`;
- `crates/lean-kernel-ffi/src/codec.rs`: the Rust encoder and decoder, and
  `crates/lean-kernel-ffi/tests/lean_kernel_ffi_it.rs`, which checks the same fixture bytes and
  round trips generated values.

The entry points are `encodeBytes e x : ByteArray` and `decodeBytes d b : Option α`. `decodeBytes`
reads the whole input, so trailing bytes do not decode.

### Lawfulness

`Lawful e d` holds when:
- round trip: `d (e x ++ rest) = some (x, rest)` for every `x` and every `rest`;
- canonical: `d bs = some (x, rest)` implies `bs = e x ++ rest`.

Together they give one encoding per value and no other bytes that decode. Prefix-freeness follows:
no proper prefix of an encoding decodes to a complete value, and an encoding with a byte appended
does not decode (`decodeBytes`).

### Format, byte by byte

- **Tag**: one byte, the constructor's index in declaration order.
- **Natural**: minimal LEB128. Seven bits per byte, low group first, bit 7 set on every byte but
  the last. A multi-byte natural never ends in `0x00`, so `0` is `00`, `128` is `80 01`, and
  `300` is `AC 02`.
- **Integer**: a sign tag, then a natural. `00` and the natural `i` if `i >= 0`; `01` and the
  natural `-i - 1` if negative. So `-1` is `01 00` and `-2` is `01 01`.
- **Boolean**: `00` false, `01` true.
- **String**: the natural length of its UTF-8 bytes, then the bytes. Decoding requires that the
  bytes are the UTF-8 of the string they decode to (valid UTF-8).
- **List**: the natural count, then the elements. **Option**: `00` for none, `01` then the value.
- **Structure**: its fields in declaration order (a pair is its two components).

The types, with their tags:

| Type | Encoding |
|---|---|
| `OnDelete` | `00` restrict, `01` setNull |
| `Ty` | `00` int, `01` str, `02` bool, `03` ref restrict, `04` ref setNull, `05` list then the element type |
| `Elem` | `00` null, `01` int, `02` str, `03` bool, `04` ref (natural) |
| `Val` | `00` null, `01` int, `02` str, `03` bool, `04` ref (natural), `05` list (count, elements) |
| `FieldDef` | `ty`, `required` (bool), `unique` (bool), `default` (`Val`) |
| `Limits` | `maxString`, `maxList`, `maxUnique`, `maxRecord` (naturals) |
| `Record` | `owner`, `version`, then the fields as a list of (field id, `Val`) |
| `Tenant` | `schema` (list of (field id, `FieldDef`)), `owner`, `editors` (list), `writers` (list), `records` (list of (record id, `Record`)), `nextId`, `limits`, `uniques` (list of lists of field ids) |
| `Ctx` | `tenant`, `principal` |
| `Intent` | `00` create (fields); `01` update (id, fields, option expected version); `02` delete (id, option expected version); `03` setRequired (field, strict); `04` setUnique (field, strict); `05` defineUnique (list of field ids) |
| `Err` | one byte: `00` noTenant, `01` unauthorized, `02` notFound, `03` invalid, `04` migrationRejected, `05` danglingReference, `06` stillReferenced, `07` versionConflict, `08` limitExceeded, `09` uniqueViolated, `0A` invalidSchema |
| step result (`Except Err Tenant`) | `00` then `Err`, or `01` then `Tenant` |

Fixtures (each proved by evaluation in `Fixtures.lean`; the Rust tests assert the same bytes):
- `encVal (.ref 300) = [4, 172, 2]`, that is `04 AC 02`;
- `encVal (.list [.int 5, .null]) = [5, 2, 1, 0, 5, 0]`;
- `encIntent (.update 5 [(1, .str "x")] none) = [1, 5, 1, 1, 2, 1, 120, 0]`;
- `encTenant fxTenant`, a tenant with one string field, owner 7, next id 1 and the kernel's limits,
  is `01 01 01 01 00 00 07 00 00 00 01 80 80 04 E8 07 C8 01 80 80 10 00` (22 bytes);
- `encResult (.error .unauthorized) = [0, 1]`.

`World` (a function from tenant ids to tenants) is not encoded. The boundary carries one tenant.

### Theorems

Combinators (each proves `Lawful` for its codec, from the `Lawful`s of its parts):
`lawful_nat`, `lawful_int`, `lawful_bool`, `lawful_str`, `lawful_pair`, `lawful_iso` (a codec
through an isomorphism), `lawful_opt`, `lawful_list`.

Boundary types: `lawful_onDelete`, `lawful_err`, `lawful_ty`, `lawful_elem`, `lawful_val`,
`lawful_fieldDef`, `lawful_limits`, `lawful_record`, `lawful_tenant`, `lawful_intent`,
`lawful_ctx`, `lawful_result`.

Entry points, for any `Lawful e d`:
- `decodeBytes_encodeBytes : decodeBytes d (encodeBytes e x) = some x`;
- `encodeBytes_of_decodeBytes : decodeBytes d b = some x → encodeBytes e x = b`.

`boundary_codecs` states both for `Tenant`, `Val`, `Elem`, `Ty`, `FieldDef`, `Limits`, `Record`,
`Intent`, `Ctx`, `Err`, the step result and `OnDelete`. `lake env lean Audit.lean` lists the axioms
of the codec theorems: `propext`, `Classical.choice` and `Quot.sound` only, with no `sorry`.

### The Rust mirror, and what it does not cover

`codec.rs` encodes the Lean boundary types restricted to the modelled fragment, with the same
bytes. Two Rust choices are narrower than Lean's unbounded types, and the decoder refuses what it
cannot represent: an integer outside `i64`, a natural outside `u64`, and a type nested more than
`MAX_TY_DEPTH` (64) lists deep. Every value the Rust types hold encodes to the bytes Lean encodes.

The Rust kernel's own types are wider than the model's and are not encoded directly: its `Value`
has `Time` and `File` values, its field ids are strings, and its `Intent` has more variants. The
mapping from kernel values to these boundary types is a separate piece of work (the correspondence
in `docs/verification.md`) and is not part of this codec.

## Temporal logic library

Built for C2 (WG-5656) in `lean/Strata/Temporal/`, imported from `lean/Strata.lean`. It has no
dependencies beyond Lean 4.36 core. Every theorem below is listed in `lean/Audit.lean`, and
`lake build` plus `lake env lean Audit.lean` show that each depends only on `propext`,
`Classical.choice` and `Quot.sound`.

- **Embedding** (`Formula.lean`). A behaviour is `Nat → σ`; a temporal formula is a predicate on
  behaviours. `lift` (state predicate), `act` (action on the first two states), `always`,
  `eventually`, `next`, `leadsTo`, the propositional connectives, and `suffix` for shifting a
  behaviour. Duality, `□` and `◇` basics: `always_elim`, `always_always`, `eventually_eventually`,
  `not_always_iff`, `not_eventually_iff`, `always_mono`, `always_imp`, `always_conj`,
  `eventually_disj`.
- **Fairness and the spec form** (`Fairness.lean`). `enabled A`, the stuttering box `stepBox`,
  `weakFair A` (`◇□Enabled A ⇒ □◇A`), `strongFair A` (`□◇Enabled A ⇒ □◇A`), and
  `spec Init N v F` for `Init ∧ □[N]_v ∧ F`. Fairness counts steps, not state changes, as IronFleet
  does. `strong_implies_weak`.
- **Leads-to** (`Rules.lean`). `leadsTo_refl`, `leadsTo_trans`, `leadsTo_disj` (`P ~> R` and
  `Q ~> R` give `P ∨ Q ~> R`), `leadsTo_of_always_imp` (`□(P → Q)` gives `P ~> Q`), `leadsTo_apply`,
  and well-founded induction `leadsTo_wf` on a measure.
- **Invariance** (`Rules.lean`). `invariant_of` (`Init` and steps preserving `I` give `□I`) and
  `invariant_of_spec` for a spec.
- **WF1 and the always-enabled form** (`Rules.lean`). `wf1` (Lamport's rule). `always_enabled_leadsTo`
  is WF1 for an action enabled in every state, so a progress step of the action becomes a leads-to.
  `always_enabled_infinitely_often` says such an action is taken infinitely often.
- **Worked example** (`LockService.lean`). IronFleet's lock service: `N` hosts pass a lock around a
  ring with always-enabled host actions ("if you hold it, grant it to the next host, otherwise do
  nothing") and a weakly fair network that delivers grants. `Lock.lock_liveness` proves every host
  eventually holds the lock under the spec's fairness. `Lock.lock_nonvacuous` exhibits a behaviour
  satisfying the spec for two hosts, so the assumptions are consistent.

## 7. Browser kernel: gap analysis

The browser kernel is `update(state, message) -> { state, effects }` in `web/src/kernel/update.js`
with `view(state)` in `view.js`, run by the worker driver (`web/src/worker/main.js`). This section lists
every message `update` handles and every effect it can emit, grouped by feature, and says for each
whether the Lean spec covers it. It is the counterpart of the kernel gap analysis in
`docs/verification.md`, and the list of work for Layer B (B1 WG-5654, then B2 WG-5659).

The spec is in `lean/Strata/Browser/`. `Types.lean` declares the state, the messages and the effects
of the whole kernel, abstractly (outcomes, not the JavaScript's algorithm). `Session.lean` states the
relation `Allowed cat s m s' es` for the routing and session slice, with its theorems. The relation is
what the B2 implementation will be proved against.

### How to read the tables

- **covered**: every outcome of the row is in `Allowed`, and no effect of the row waits for a later
  slice.
- **partly**: the slice 1 outcome is in `Allowed`, and a named part waits for the slice in the last
  column.
- **not covered**: no constructor of `Allowed` says anything about the row yet.
- Toasts, announcements, focus, menu and confirmation resets are presentation (slice 4). They do not
  make a row partly covered; the slice column names them where they matter.
- A row is one message, or a family of messages with the same handling; the family's names are listed.
  Replies are grouped by the request kind that decides them, as `onHttp` dispatches them.

### 7.1 Messages

#### Routing and the address

| Message | What update does | Status | Slice |
|---|---|---|---|
| `init`, bootstrapped session | route and session from the bootstrap; a view request | partly | 1 (route, session, view); 2 (bootstrapped view); 5 (socket start); 6 (link, invitation and provider captures) |
| `init`, cold start (no bootstrapped session) | a session read and a view read in flight; the view may answer first and is held (F4) | covered | 2 (the spec drops a view answer until the session is known, and reads it again after the session answer) |
| `http_result` view, answered while the session is not known | held with no session to check it against (F4) | covered | 2 (the spec drops it and reads the view again) |
| `http_result` session, cold start (session not yet known) | the session is set; the view is not read again, so a view answered earlier stays (F4) | covered | 2 (the spec reads the view again once the session is known) |
| `navigate` | moves the address; a view request; push and scroll unless the same address without replace; tenant parts and manifest for a tenant address; the provider list on the sign-in page | partly | 1 (address, view, push, scroll); 6 (providers); 11 (parts, manifest) |
| `popstate` | moves the address; a view request; no push, no scroll | partly | 1 (address, view); 11 (tenant parts) |
| `action reload` | a view request for the current address | covered | 1 |
| `timer request` (a request past its deadline) | the request ends as status 0, an unreachable site | covered | 1 (decoded to `failure` or `viewFailure`) |

#### The view

| Message | What update does | Status | Slice |
|---|---|---|---|
| `http_result` view, accepted | the model is held with the principal it answers for; the enter hooks of its screen run | partly | 1 (held model); 7, 8, 10, 12 (enter hooks) |
| `http_result` view, not found | status missing; no model | covered | 1 |
| `http_result` view, refused or failed | status error; no model; the sign-in page for an unauthenticated view is chosen by the page builder | partly | 1 (status); 13 (page choice) |
| `http_result` view, no longer awaited | ignored | covered | 1 |
| `action view.more` | a request for the next page of a list section, with its cursor | not covered | 9 |
| `http_result` view.more | rows appended without repeats; the cursor moves on; a refusal toasts | not covered | 9 |

#### The session

| Message | What update does | Status | Slice |
|---|---|---|---|
| `http_result` session, same person | nothing changes but the request | covered | 1 |
| `http_result` session, another person or none | the session changes in place and the person's state is kept (F3); the spec clears it and reads the view again | covered | 1 |
| `http_result` session, refused or unreachable | the request ends | covered | 1 |
| `ws_message` session, expired or signed out | ends the session and clears the person's state; the session is read again; the view is read again only when a guard or a live binding asks (F1); the spec always reads the view again | covered | 1 |
| `ws_message` session, while our own sign-out is pending | ignored | covered | 1 |
| `ws_message` session, active | nothing | not covered | 5 |

#### Sign-in and sign-out

| Message | What update does | Status | Slice |
|---|---|---|---|
| `submit` login, complete form | the sign-in request; the form is busy and keeps the values | covered | 1 (request); 3 (values kept, error cleared) |
| `submit` login, incomplete form | the values are kept and the form says so; no request | covered | 3 (`signInIncomplete`) |
| `http_result` login, accepted | signed in: session, the person's state cleared, the address moved to the reply's target | partly | 1; 5 (live socket reset) |
| `http_result` login, second factor waiting | the code prompt; no session yet | covered | 7 (`signInWaiting`) |
| `http_result` login, refused | the request ends; the form shows the refusal and drops the password (`loginRefused`) | covered | 3 (the refusal's words are wording, and focus is presentation, slice 4) |
| `action logout` | the sign-out request, when the screen offers sign-out | covered | 1 |
| `http_result` logout, accepted | no session; the person's state cleared; the address replaced | partly | 1; 5 (live socket reset) |
| `http_result` logout, refused | the request ends (the warning is presentation) | covered | 1 |
| `submit` search | navigates to the address the catalogue names for the values, and asks for its view | covered | 3 (`searchSubmit`) |
| `input` login | the typed value (`setField`); sends nothing | covered | 3 (`inputLogin`) |

#### Live connection

| Message | What update does | Status | Slice |
|---|---|---|---|
| `ws_open` | resets the refusal count and the stop flag | covered | 5 (`socketOpened`) |
| `ws_message` welcome | a first connection subscribes from the beginning, takes the head as the acknowledged number, resets the counts and starts the heartbeat at the interval the welcome announces, and reads the view again; a resumed one subscribes on | covered | 5 (`welcomeFirst`, `welcomeResume`; read from `client-live-flow-impl.js`) |
| `ws_message` event (a notice or a heartbeat) | acknowledges a notice in the count, moves the acknowledged number on when it is newer, and reads the view again when the live bindings say so; a notice out of the count closes the socket, resets the acknowledged number and reads the view again | partly | 5 (`frameAdvanceRefresh`, `frameAdvanceQuiet`, `frameSameRefresh`, `frameSameQuiet`, `frameGap`; the live bindings decide the flag, an input of the message; the session on a membership notice: not yet) |
| `ws_message` resync | carries on from the head; the view is read again | covered | 5 (`resyncRefresh`; the manifest read again: 11) |
| `ws_message` pong | a head beyond the connection's own notices reads the view again | covered | 5 (`pongBehind`, `pongCurrent`) |
| `ws_closed`, not refused | the live state waits; a reconnect timer | covered | 5 (`wsClosedLater`) |
| `ws_closed`, refused | the refusal is counted; the session is read again; after five, live updates stop (F7) | covered | 5 (`wsRefusedKeep`, `wsRefusedStop`; the count is taken at the close, see F7) |
| `timer` heartbeat, reconnect | a ping and the next heartbeat; a reconnect attempt unless stopped | covered | 5 (`heartbeatDue`, `reconnectFires`, and the stale cases) |
| `timer` emit (a delayed frame) | none of this machine's emits is delayed (`afterMs` is never set in `client-live-flow-impl.js`) | covered | 5 (nothing to add) |

#### Presentation

| Message | What update does | Status | Slice |
|---|---|---|---|
| `system_theme` | records the device's colour scheme | covered | 4 (`systemThemeSet`) |
| `action theme` | cycles auto, light, dark; stores the choice | covered | 4 (`themeCycle`; `theme_choice_stored`) |
| `action menu` | toggles the menu | partly | 4 (`menuToggle`; the menu closing on a navigation is not yet in `askView`) |
| `action dismiss` | removes a toast | covered | 4 (`dismissToast`) |
| `timer toast` | removes a toast when its time passes | partly | 4 (`toastExpires`; raising a toast is not yet a constructor) |
| `action dev.expire`, `http_result` dev | asks the gateway to expire the session; the reply is a shared request answered | partly | 4 (`devExpire`, `sharedAnswer`; the toast on the reply is not yet raised) |

#### Passkeys, second factors and step-up

| Message | What update does | Status | Slice |
|---|---|---|---|
| `action passkeys.sign_in`; `http_result` passkeys.challenge, passkeys.sign_in; `passkey_result` (sign-in) | passkey sign-in: challenge, browser prompt, answer; signs in as a login does | not covered | 6 |
| `action passkeys.add`, `passkeys.remove`; `http_result` passkeys.list, passkeys.register_start, passkeys.register_finish, passkeys.remove; `passkey_result` (registration) | the person's passkeys | partly | 8 (`passkeyAddAsk`, `personalCeremony`, `passkeyCompleted`, `passkeyDeclined`, `deskAsk` for the removal, the answers `personalAnswerReload` and `personalAnswerQuiet`; the list reply and the support check are not yet) |
| `action factors.enrol_start`, `factors.enrol_cancel`, `factors.remove_totp`, `factors.new_codes`, `factors.dismiss_codes`, `factors.use`, `factors.use_passkey`, `factors.stepup_cancel` | the authenticator, recovery codes, the factor choice, the step-up prompt | partly | 7 (`factorsAsk`, `factorsAnswer` for enrol_start, remove_totp and new_codes; the other factor actions and the stepup cancel are not yet, except `factors.stepup_cancel` which is covered by `stepUpCancel`) |
| `submit` second, totpconfirm, stepup; `input` second, totpconfirm, stepup | the second-factor code, the enrolment confirmation, the re-authentication | partly | 7 (`secondSubmit`, `stepUpSubmit`; the totpconfirm form and the typed values are not yet) |
| `http_result` factors.summary (entered with the desk), enrol_start, enrol_finish, remove_totp, new_codes | the factors section's state | partly | 7 (`factorsAnswer` for enrol_start, remove_totp and new_codes; the summary and enrol_finish are not yet) |
| `http_result` factors.pk_challenge; `passkey_result` (second factor) | a passkey as the second factor | not covered | 7 |
| `http_result` factors.second | the second factor completes a sign-in | covered | 7 (the success is `signInAnswer`; `secondRefused` for a refusal) |
| `http_result` factors.stepup; any refusal `step_up_required` | the refused action is asked again once confirmed; the step-up prompt is shown | partly | 7 (`stepUpRequired`, `stepUpAnswer`; the redispatch of the refused action is not yet) |

#### Sessions list

| Message | What update does | Status | Slice |
|---|---|---|---|
| `action sessions.revoke`, `sessions.revoke_others`; `http_result` sessions.list (entered with the desk), sessions.revoke, sessions.revoke_others | the person's own sessions: list, end one, end the others | partly | 8 (`deskAsk` for both revocations and `personalAnswerReload`/`personalAnswerQuiet` for their answers; the list reply, `sessions.list`, is not yet) |

#### Sign-in methods: links, invitations, identity providers

| Message | What update does | Status | Slice |
|---|---|---|---|
| `submit` link; `http_result` links.request | emailed sign-in: the request is sent and the page says a link is on its way (`linkAsk`, `linkAnswer`) | partly | 6 (the opened link's sign-in, `links.sign_in`, and its refusal are not yet in the relation) |
| `submit` invite; `http_result` invites.accept | an owner's invitation: the request; on acceptance the page goes on to sign-in and nobody is signed in (`inviteAsk`, `inviteAccepted`) | partly | 6 (the form's typed values, `input` invite, are not yet in the relation) |
| `action oidc.start`; `http_result` oidc.start | the start: the request; the reply's address is left for by the browser (`oidcStart`, `oidcRedirect`) | covered | 6 |
| `http_result` oidc.providers (entered on the sign-in page) | the providers list | not covered | 6 |
| `http_result` oidc.sign_in | the provider's answer is sent; signs in as a login does | not covered | 6 |

#### Records and the editor

| Message | What update does | Status | Slice |
|---|---|---|---|
| `submit` editor; `input` editor | save of an article the page's check passes (`editorSave`); a refused article sends nothing and the editor says so (`editorSaveRefused`); typed values (`inputEditor`); the two-step save of a new record is not yet | partly | 9 |
| `action editor.publish`; `editor.unpublish`, `editor.tab`, `editor.reload` | publish sends the article when the check passes (`editorPublish`, `editorPublishRefused`); unpublish, tabs and reload are not yet | partly | 9 |
| `http_result` editor.save | the answer reads the view again (`editorAnswerRefresh`); a refusal from the server ends the request, and its words and focus are not yet | partly | 9 |
| `http_result` editor.preview; the preview tab | the preview of the body | not covered | 9 |
| `action` row action (generated row buttons) and its confirmation | the row's action; the two-press confirmation | not covered | 9 |
| `http_result` post.action | the row action done or refused | not covered | 9 |
| The client's record-rule check before a save (`record-rules.js`) | a refusal before any request, in the kernel's words | covered | 9 (the check is `recordOk`, a catalogue input; the drift log of `record-rules.js` is a later effect) |

#### Settings

| Message | What update does | Status | Slice |
|---|---|---|---|
| `http_result` settings.definitions (entered with the page) | the settings list is held (`personalAnswerQuiet`); the page's entry that asks for it is a screen hook, not yet in the relation | partly | 10 |
| `submit` settings, `submit` settings:level:id; `http_result` settings.write | a save at one level is sent for a value the setting takes (`settingsWrite`); success reads the definitions again (`personalAnswerReload`); the typed values (`input` settings) and the refusal's words are not yet | partly | 10 |
| setting notice (`ws_message` event, kind setting) | the view is read again when the view reads the setting | partly | 10 (with 5: the view is read again when the live bindings say so, and the setting notice is one of them) |

#### Tenant interface

| Message | What update does | Status | Slice |
|---|---|---|---|
| `http_result` ui.manifest | a manifest not older than the held one is taken in; an older one is ignored (`manifestNewer`, `manifestOlder`); the parts it names are not asked for yet | partly | 11 (the parts: `ui.part`, not yet) |
| `http_result` ui.part | the part is held; a form whose field was removed names what was lost | not covered | 11 |
| schema notice (`ws_message` event) | the manifest is read again (`manifestAsk`; the notice itself is a live frame, slice 5) | partly | 11 |
| `action tenant.action` | a tenant action is sent when the catalogue says it needs no confirmation or prefill (`tenantAction`); the two-press confirmation and the row prefill are not yet | partly | 11 |
| `submit` tenant:id | a tenant form is sent for complete values (`tenantSubmit`); the typed values, `input` tenant:id, are not yet | partly | 11 |
| `http_result` tenant.submit, tenant.action | the answer reads the view again (`tenantAnswerRefresh`); the toast, the navigation and the refusal's words are not yet | partly | 11 |

#### Impact Lab

| Message | What update does | Status | Slice |
|---|---|---|---|
| `action lab.apply` | a finished projection that is held is applied (`labApply`); tabs, preset, cancel, replay, verify and diagnostics are not yet | partly | 12 |
| `submit` lab; `input` lab | a projection is started with a complete form when none runs (`labRun`); the typed values (`input` lab) are not yet | partly | 12 |
| `http_result` lab.run, lab.result, lab.apply | the start arms the poll (`labRunStarted`); a finished result is held (`labFinished`), a running one arms the next poll (`labStillRunning`); the apply is answered (`labApplyDone`); the overview, replay, verify and diagnostics are not yet | partly | 12 |
| `timer` lab.poll | polls the run while it runs; a poll that is not the running one changes nothing (`labPollFires`, `labPollStale`) | covered | 12 |
| `ws_message` run (verification, progress, cancelled, finished) | the run's frames | not covered | 12 |

### 7.2 Effects

| Effect | Emitted by | Status | Slice |
|---|---|---|---|
| `http` (method, path, body, csrf, idempotency key, correlation) | every request | partly | 1 (id and kind); 3 (csrf token, idempotency key, correlation still open; the sign-in and search bodies are the form's values) |
| `timer` request | every request (30 000 ms) | covered | 1 |
| `timer` toast | every toast (5 s, or 9 s for a warning) | partly | 4 (expiry is covered; raising a toast is not yet a constructor) |
| `timer` lab.poll | the Lab's run poll (see the message row above) | covered | 12 |
| `timer` heartbeat, reconnect | the live connection's heartbeat and reconnect timers | covered | 5 |
| `timer` emit | the delayed frames of the live connection (none for this machine) | covered | 5 |
| `push_url` | an address change, with the replace flag | covered | 1 |
| `scroll_top` | navigation | covered | 1 |
| `ws_connect` | a socket open with the request token (at init with a session, and when a waiting socket connects) | partly | 5 (`reconnectFires` emits it with no token; the request token, and the socket at init, are not yet in the relation) |
| `ws_send` | subscribe, ack, ping, and the Lab's cancel | partly | 5 (subscribe, ack and ping: covered; cancel: 12) |
| `ws_close` | closing the socket, after a gap in the notices | covered | 5 (`eventGap`) |
| `store` | the theme choice in storage | covered | 4 (`themeCycle` stores the mode's key) |
| `passkey` (`create`, `get`) | the browser's passkey prompt | partly | 8 (`create` for the registration is covered by `personalCeremony`); 6 (sign-in, `get`), 7 (second factor, `get`) |
| `log` | the record-rule disagreement, and the worker's diagnostics | not covered | 9 |
| `navigate_away` | the redirect to an identity provider; emitted by `oidc.js` but absent from the `Effect` type in `types.js` (F5) | covered | 6 (`oidcRedirect`; `types.js` still needs the declaration, F5) |

### 7.3 Where the JavaScript does not meet what the spec requires

These are read from the code and not yet run. Each is a requirement the relation states and the
JavaScript does not meet; B2 cannot be proved until the JavaScript changes, or the requirement is
changed by its owner.

- **F1: a held model survives a session end on an unguarded page.** `clearUserState` keeps `view`
  unless the current address is guarded (`view: guardedScreen(state) ? ... : state.view`), and the
  session frame re-reads the view only when a guard or a live binding asks (`refresh`). So on an
  unguarded page with no refresh, the model read for the former person is kept, and `view.js` draws
  it for the visitor. Whether it holds personal data depends on the screen (`people[*].self` and
  `affordances` come from the reply for that person). Requirement: `session_change_drops_model`.
- **F2: a view reply asked for before the session ended is applied after it.** `onHttp` accepts a
  `view` reply when `state.view.request === msg.id`. `view` is in neither `USER_KINDS` nor
  `USER_KIND_PREFIXES`, so the generation check does not drop it, and `clearUserState` resets
  `view.request` only on a guarded page. Requirement: a reply of an earlier generation changes nothing
  (`switched` forgets the request; `dropViewAnswer`).
- **F3: a session reply that names another person, or none, keeps the former person's state.**
  `onHttp`'s `session` case calls `sessionRead` with the new session and never calls
  `clearUserState`, so the sessions list, passkeys, factors, Lab, editor and settings forms of the
  former person stay in memory, as does the held model. Requirement: `sessionChanged`.
- **F4: a view answered before the session is known is held.** At a cold start the view reply can
  arrive before the session reply. `onHttp`'s `view` case accepts and holds it while `session` is still
  null, and the session reply does not read the view again. The page then shows a view read before the
  person was known, and nothing checks it against the session. The spec drops such an answer
  (`dropColdView`, `cold_answer_changes_no_model`), keeps no model before the session is known
  (`held_only_when_known`), and reads the view again once the session answers (`learnSession`).
- **F5: `navigate_away` is undeclared.** `oidc.js` emits it; `types.js` does not declare it; the worker
  forwards it to the main thread through its default branch. Slice 6 declares it.
- **F6 (minor): the sign-in form after a change of person.** The spec clears the whole sign-in form
  on a change of person (`switched`). `clearUserState` clears only the password and keeps the login
  name. Both are safe; the spec is the stricter, and B2 must choose one.
- **F7: a refused upgrade is counted when the session read answers.** `onHttp`'s `session` case counts
  `pending.refusal` (`wsRefusals`, and `wsStopped` at the limit) only when the session read comes back,
  so a lost session read is never counted. The spec counts the refusal when the socket closes
  (`wsRefusedKeep`, `wsRefusedStop`) and states `refusal_counts_and_stops`. B2 must count one of them;
  the spec's count is the one that cannot be lost.

**Status of the findings above.** All seven are fixed in `web/src/kernel/update.js`, each with a test
in `web/test/kernel.test.js` that drives `update` through the scenario's messages:

- F1: `a session end drops the view read for the former person, on a page that needs no session`.
- F2: `a view asked for before the person changed is not applied after the change`. The test changes
  the person by a session read; a session end already replaces the view's request id on the quiet
  refresh, so it did not show the fault.
- F3: `a session read naming another person, or nobody, drops what the former person held`.
- F4: `a cold start holds no view that answers before the session does, and reads it again once the
  session is known`.
- F5: `navigate_away` is declared in `web/src/kernel/types.js`, with no test of its own.
- F6: `a change of person clears the sign-in form, the typed name too`.
- F7: `a refused upgrade is counted when the socket closes, and the fifth stops live updates`.

### 7.4 What the spec leaves out, by design

- **The generated tables.** Routes, screens, contracts, forms, actions, error wording, live bindings and
  the client machines are generated from `specifications/` into `web/src/generated`. The kernel's
  decisions that come from them enter the relation as catalogue parameters (`screenOf`,
  `needsSession`, `signOutOn`, `validSignIn`) and, from their slices, as abstract tables. Their own
  contracts are a separate check, not part of the kernel's.
- **The driver.** `web/src/worker/main.js` performs the effects, supplies time (`at`) and randomness
  (`rand`), draws the correlation ids and the idempotency keys, and times the requests. It is trusted,
  as IronFleet's UDP layer was. Request ids are the spec's `nextId`; the deadline is
  `requestDeadlineMs`.
- **The page.** `web/src/main.js`, `components/dom.js` and `views/*.js` draw the view model with DOM
  APIs. Trusted; the view model's shape is the spec's concern from slice 13.
- **Wording.** Refusal words, toasts and announcements come from the generated errors and the page's
  own strings. They are checked by tests, not by the spec.
- **Liveness.** The relation says what a step may do. It does not say that a step happens. A message
  the relation has no case for has no allowed outcome until its slice adds one.
- **The request-side guard for guarded views.** The brief's example, "no effect sends an application
  request from a state without a session for a route that needs one", is not stated. The JavaScript
  does request a guarded view for a visitor (`navigate` and `fetchView` are unconditional), and the page
  relies on the gateway's `unauthenticated` answer to show the sign-in page (`view.js`). The spec states
  what the page shows instead: a held model of a screen that needs a person is held only for a person
  (`held_guarded_signed_in`), and a visitor is shown no such screen (`visitor_shows_no_guarded_page`).
  If the owner wants the request itself withheld, `navigate` and `popstate` need a guard, and the
  sign-in path changes with it. That is a decision for the owner, and this pass does not take it.

### 7.5 Slice 1: what it states, and how the existing models are restated

Slice 1 is routing and session. It was chosen because every later slice reads the session and the
address (the page's screen, its guard, the person whose data is held), and because the rule that drops
the held model at a change of person is the rule the browser's privacy depends on.

Theorems, all in `Strata.Browser` and listed in `lean/Audit.lean`:

- `held_is_own`: every view model the page holds was read for the person signed in now.
- `held_guarded_signed_in`: a model of a screen that needs a person is held only while someone is
  signed in.
- `page_shows_only_own`: the page shows a model only to the person it was read for.
- `visitor_shows_no_guarded_page`: a visitor is shown no screen that needs a person.
- `session_change_drops_model`: a change of the person leaves no held model.
- `requests_are_pending`: every request effect names a request the state awaits.
- `signed_in_view_reachable`: a non-vacuity witness, a reachable state that holds a model read for a
  person, so the theorems above are not empty.

No unfinished proof and no added axiom. `lean/Audit.lean` prints the axioms each theorem depends on:
`held_is_own`, `held_guarded_signed_in`, `page_shows_only_own`, `visitor_shows_no_guarded_page` and
`requests_are_pending` on `propext` and `Quot.sound`; `session_change_drops_model` on `propext` alone;
`signed_in_view_reachable` on `propext`, `Classical.choice` and `Quot.sound`.

How the existing theorems meet this one (no existing theorem is changed):

- `Strata.Client.Privacy` (FE-004, FE-005, FE-006). The held model is in memory; `Privacy` proves what
  is at rest and what a session can open. `session_change_drops_model` covers the in-memory side at each
  change of person. Slice 14 connects `page_shows_only_own` to `Privacy.served_own` (what a session can
  be served was written by its principal), once the page's held data is a value of the store.
- `Strata.Client.Drafts` (FE-017). The person's typed text in a draft is a form state, and slice 3 is
  where `Drafts.dstep` meets the form flow. The rule that drops other people's values at orphaning
  (`orphan_drops_server_values`) has the shape of `session_change_drops_model`; slice 3 states it for
  the form values.
- `Strata.Client.Cache` (FE-001 to FE-003, FE-009, FE-012, FE-015, FE-018, FE-019). The page's view model
  is the kernel's copy of a view; the cache's stamped entries are not in the kernel yet. Slice 14 restates
  `current_value` and `rest_above_floor` over the held model and the store.
- `Strata.Client.Queue` (FE-007 to FE-011, FE-014, FE-016). The kernel has no offline queue yet; the
  queue is planned in `docs/frontend.md` (follow-ups 4 to 9). Slice 14 adds it, with `submit_ok_guard`
  and `at_most_once` stated over the queue's messages.
- `Strata.UI.Visibility` (`render_sound`, `served_needs_content`, `admin_needs_role`; DS-007 to DS-014).
  The page draws what the server serves to the person, and only the nodes `render` serves to the viewer.
  Slice 13 states that by `render_sound`.
- `Strata.UI.Composition` (`checked_bindings_resolve`, `publishable_bound`; DS-001 to DS-004). A tenant
  page binds only the widgets the schema defines. Slice 11 restates `checked_bindings_resolve` over the
  held part.
- `Strata.UI.Loop` (`approve_applies_held`, `staged_invisible_until_last`; DS-015, DS-016, DS-021). The
  kernel holds the previous part of a screen during an upgrade (`held` in `tenant-ui.js`). Slice 11
  states that the page shows a held part only until its replacement arrives, in the terms of
  `staged_invisible_until_last`.
- `Strata.UI.Contrast` (`checkTheme_iff`; DS-005, DS-006). The kernel keeps the theme's mode, accent and
  paper, and does not check contrast; the check belongs to the theme. Slice 4 states no contrast theorem
  unless the kernel draws a colour it derived.

### 7.6 Slices, in dependency order

| Slice | Adds | Needs |
|---|---|---|
| 1 (done) | Routing and session: `init` with a bootstrapped session, `navigate`, `popstate`, `reload`, the view's answers, the session's answers and changes, sign-in and sign-out (their requests and answers), the session ending, a request's deadline; the held model and its theorems | |
| 2 (done) | Cold start: the session read at start, a view that answers before the session (dropped and read again), the session answer that makes the session known (`learnSession`); the bootstrapped view remains partly, see row `init` | 1 |
| 3 (partly done) | Forms, for the sign-in and the search: the typed values, the complete and incomplete submissions, the refusal, the search's navigation (`inputLogin`, `signInIncomplete`, `loginRefused`, `searchSubmit`). Still open in this slice: the csrf token and idempotency key of a request (with the `http` effect), the words of a refusal, focus, and the typed draft (`Drafts.lean`) | 1 |
| 4 (partly done) | Presentation: the theme and its store effect, the system theme, the menu, dismissing and expiring toasts, the dev expire request (`themeCycle`, `systemThemeSet`, `menuToggle`, `dismissToast`, `toastExpires`, `devExpire`, `sharedAnswer`). Still open: raising a toast (every row's toast), the menu closing on a navigation, and focus and announcements | 1 |
| 5 (partly done) | Live connection, read from `client-live-flow-impl.js` (`welcomeFirst`, `welcomeResume`, `eventRefresh`, `eventQuiet`, `eventGap`, `resyncRefresh`, `pongBehind`, `pongCurrent`, `wsClosedLater`, `wsRefusedKeep`, `wsRefusedStop`, `reconnectFires`, `heartbeatDue`, `socketOpened`). Still open: the session frame when active, the request token on connect, the session read on a membership notice, the resync and reset outside the open state, and the schema notice's manifest (11) | 1, 2 |
| 6 (partly done) | Sign-in methods: emailed links (request and answer), invitations (request and acceptance), the identity provider's start and redirect (`linkAsk`, `linkAnswer`, `inviteAsk`, `inviteAccepted`, `oidcStart`, `oidcRedirect`). Still open: the opened link's sign-in, passkey sign-in, the providers list, the provider's answer (`oidc.sign_in`), SAML, and the typed values of the invitation form | 3, 5 |
| 7 (partly done) | Second factors and step-up: the waiting sign-in and the second-factor code (`signInWaiting`, `secondSubmit`, `secondRefused`), the step-up (`stepUpRequired`, `stepUpSubmit`, `stepUpAnswer`, `stepUpCancel`), the factor requests and their answers (`factorsAsk`, `factorsAnswer`). Still open: the passkey as a second factor, the enrolment and recovery-code details, and the redispatch of a refused action once confirmed | 3, 6 |
| 8 (partly done) | The person's sessions and passkeys on the desk: the revocations and the passkey removal (`deskAsk`), the registration (`passkeyAddAsk`, `personalCeremony`, `passkeyCompleted`, `passkeyDeclined`), and their answers (`personalAnswerReload`, `personalAnswerQuiet`). Still open: the list reads, the support check of passkeys, and the toasts | 3, 7 |
| 9 (partly done) | Records: the editor's save and publish with the page's check (`editorSave`, `editorSaveRefused`, `editorPublish`, `editorPublishRefused`, `inputEditor`), and the answer that reads the view again (`editorAnswerRefresh`). Still open: unpublish, preview, row actions, paging, the two-step save of a new record, and the drift log | 3, 5 |
| 10 (partly done) | Settings: a save at one level for a value the setting takes (`settingsWrite`), its success reading the list again (`personalAnswerReload`), the list reply (`personalAnswerQuiet`). Still open: the typed values, the refusal's words, entering the page, and the setting notice's binding | 3, 5 |
| 11 (partly done) | Tenant interface: the manifest and its version (`manifestAsk`, `manifestNewer`, `manifestOlder`), tenant actions and forms sent for the person (`tenantAction`, `tenantSubmit`), and their answers (`tenantAnswerRefresh`). Still open: the parts, the tenant screens by address, the confirmations and row prefill, the typed values, the navigation of a tenant answer, and the lost drafts | 3, 5 |
| 12 (partly done) | Impact Lab: the start of a projection and its poll, the held result, the apply and its answer (`labRun`, `labRunStarted`, `labPollFires`, `labPollStale`, `labFinished`, `labStillRunning`, `labApply`, `labApplyDone`). Still open: the overview, the typed values, the tabs, presets, cancel, replay, verify, diagnostics, and the run's frames | 3, 5, 7 |
| 13 (partly done) | View derivation: the kind of page the view shows (`pageOf` in `Page.lean`: loading, missing, error, the sign-in page, a screen's data), with `page_data_is_own`, `visitor_sees_no_guarded_data` and `person_change_shows_loading`. Still open: the page builders per screen kind, the title and navigation, the composition and display of tenant pages, and `render_sound` over the page | 1, 9, 10, 11, 12 |
| 14 | Browser storage: the view cache with stamps, the offline queue, the leader tab, drafts at rest (`docs/frontend.md`, follow-ups 4 to 9) | 3, 5 |

### 7.7 Counts

The messages table has 80 rows (a row is one message, or one family of messages with the same handling):
35 covered, 33 partly, 12 not covered (after slice 12). The effects table has
15 rows: 9 covered, 5 partly, 1 not covered. All 13 top-level message types that `step` switches on have rows, and so do the 10 effect
types `types.js` declares and `navigate_away`, which it does not declare. Slice 1 covers the covered
rows of routing, the view, the session and sign-in and sign-out; the other rows are slices 2 to 14.

## 8. System spec and protocol (C1, WG-5655)

Layer C1 is written in `lean/Strata/System/`: `Spec.lean` (the trusted system specification) and
`Protocol.lean` (the host state machines, the invariant and the refinement). Both are imported from
`lean/Strata.lean`, and every theorem in them is listed in `lean/Audit.lean`. Each theorem depends
only on `propext` and `Quot.sound`. There is no `sorry`, no `admit`, no added axiom and no new
dependency.

### The system spec

`Spec.lean`, the trusted part, is 85 lines with its comments and docstrings (66 without). It is
parametrised by a `Kernel` (`step`: one intent by one principal on one tenant state, and
`readable`: whether the principal may read the tenant), so it does not depend on which kernel slices
exist. The instance used for the witness is the kernel's own `stepTenant`.

- `SysState`: the tenants (`World`), the committed events in order, the reply each request has
  received (`replies`), and the frames sent to each subscriber (`frames`). Clients observe only the
  replies and the frames.
- `SpecNext`, three steps. `serve`: a request with a session is answered with the kernel's verdict
  on its intent, applied to the tenant at its turn; an accepted verdict becomes the new tenant state
  and an event. `noSession`: a request without a session is answered and no tenant changes. `frame`:
  a subscriber observes a committed event of its tenant only if its principal may read the tenant
  after the change.
- `SpecReachable`: the states reached from `SysInit` by `Steps` (the reflexive, transitive closure
  of `SpecNext`).

Theorems about the spec: `spec_frames_readable` (every observed frame is a committed event of its
subscription, readable by its principal), and `spec_log_unique` (every committed event answers a
request, and a request commits at most one event).

### The hosts and the network

The network is the set of packets sent so far. A host receives a packet by naming it, so delay,
drop, duplication and reordering are all allowed, and nothing is tampered with. There are 11 kinds
of packet: `req`, `fwd`, `reply`, `clientReply`, `load`, `snap`, `commit`, `done`, `conflict`,
`committed` and `frame`.

- **BFF** (stateless): `req` becomes `fwd` with the session token; `reply` becomes `clientReply`.
- **Backend**: one transaction per request id, in phases `loading`, `ready v base`,
  `committing v base` and `finished o`. A transaction loads its tenant, computes the kernel's verdict
  on the snapshot it received, and sends a commit naming the revision it was based on. A `conflict`
  sends it back to `loading`. Transactions run concurrently; a repeated `fwd` changes nothing. A
  request without a session is answered with `noSession`.
- **Store**: holds each tenant's state by revision (`hist t k`, `rev t`), the committed events
  (`evs`), and the decision made for each request id (`decided`). A commit for an undecided request
  is decided when its base revision is still current, and answered with `conflict` otherwise. A
  commit for a decided request is answered with the decision again and changes nothing. The store
  applies an accepted verdict (new revision, new event) or records a refusal (no change).
- **Publisher**: a committed event goes to each subscriber whose principal may read the tenant after
  the change, as a `frame`.

Each protocol step is one host acting on at most one packet (or none, for a client request) and
sending its packets. There is no clock, so the time-dependent part of IronFleet's reduction
obligation is empty. That the general obligation suffices is C4.

### The refinement

`absState` maps a protocol state to a system state: the world is the store's current state of each
tenant, the log is the store's events, a request's reply is its store decision (or `noSession`), and
the frames are the `frame` packets sent. A protocol step maps to zero spec steps, or to exactly one
spec step at its linearisation point: a store decision (`serve`, or the refusal), a `noSession`
answer, or a publication (`frame`).

- `Good` is the invariant, ten fields over the sent packets and the host states. Its key fields: a
  snapshot is the store's state at the revision it names (`snapHist`); a commit's result is the
  kernel's verdict on that snapshot (`commitCorrect`); a commit belongs to a transaction with the same
  token, tenant, principal and intent (`commitTxn`); a done packet is the store's decision
  (`doneDecided`); a reply is a decision or a `noSession` answer (`replyDecided`).
- `good_step`: `Good` holds after every protocol step. `good_init`: it holds initially.
- `refine_star`: every protocol step maps to a `Star` of spec steps, as above.
- `protocol_refines`: every reachable protocol state satisfies `Good`, its abstraction is spec
  reachable, and every relayed reply was sent by the backend.

The `SpecRelation` of this layer is `observed_matches_spec`: the reply packet every client receives
is the spec's reply to its request, and every frame packet is a frame of the spec.

### Theorems

| Theorem | States |
|---|---|
| `refine_star` | every protocol step is zero spec steps, or the one spec step it linearises |
| `protocol_refines` | every reachable state is `Good`, spec reachable, and relays only sent replies |
| `observed_matches_spec` | each reply a client receives is the spec's reply; each frame is a spec frame |
| `observed_reply_is_decision` | a reply is the store's decision for its request, or a `noSession` answer |
| `frames_readable` | a subscriber receives a frame only for a committed event its principal may read |
| `commit_once` | a request has at most one committed event, however many times a commit is delivered |
| `commit_is_verdict` | a commit's result is the kernel's verdict on the tenant at the revision it was based on, which is no later than the current one |
| `rev_step` | one protocol step moves a tenant's revision by nothing or by one |
| `duplicate_commit_inert` | a repeated commit for a decided request changes nothing in the store |
| `refusal_reachable` | non-vacuity: a request from a stranger is refused, and the refusal reaches the client in nine protocol steps |

### Modelling decisions the code must meet

1. A refusal is also checked against the current revision (the store decides it only at the base
   revision). The Rust refusal path must do the same, or show that its refusal does not depend on
   the revision. Checked in C3.
2. The store keeps a decision record per request id. That is the idempotency record SQLite must
   provide, and it belongs to the trusted SQLite specification (C3).
3. The outbox is modelled as the store's messages to the publisher. Crash and restart, with replay of
   unpublished rows, are not modelled (IronFleet's lesson 4).
4. Sessions and subscriptions are static configuration. Tenant creation is not modelled: every tenant
   exists from the start.
5. The protocol has no fairness or liveness. Retries are unbounded, and nothing says they succeed.
   That is C2.
6. The protocol's host steps are atomic, and the refinement covers every interleaving of them. C4
   must show that finer implementation steps, interleaved within a host, reduce to these.

### What is left

- **C2** (liveness): done, see "Backend liveness" below.
- **C3** (implementation): one step function per host in Lean, the trusted shells (sockets, HTTP,
  SQLite calls), the SQLite specification of the decision record and the revision guard, and the
  refusal check of modelling decision 1.
- **C4** (reduction): the implementation's fine-grained interleavings, with receive-before-send and
  at most one time-dependent input per step, reduce to the atomic host steps of this layer. The
  concurrency of one tenant's transactions is already a linearisation in this layer (each commit or
  refusal is a spec step at the store's decision). C4 makes the implementation's own steps part of
  that argument.
- Crash recovery of the outbox, and the model of tenant creation.

### Backend liveness (C2, WG-5656)

`lean/Strata/System/Liveness.lean` proves liveness for the protocol of `Protocol.lean`, which is
unchanged. It uses the `Temporal` library. Every theorem is listed in `lean/Audit.lean`; each
depends only on `propext`, `Classical.choice` and `Quot.sound`. There is no `sorry`, no `admit`
and no new axiom. `lake build` succeeds.

**The fair spec** (`protoSpec`). The behaviours are `Init ∧ □[Next]` (`PInit`, `PNext`) and the
fairness `protoFair`. The fairness has one assumption per instance of each host action. An instance
is a rule of `PNext` with its parameters fixed: the request id, token, intent, tenant, snapshot,
base revision, or one store commit packet. The rule's action relation is in `Liveness.lean`
(`actFwd`, `actBegin`, `actRelay`, `actDone`, `actLoad`, `actCommit`, `actConflict`, `StoreStep`,
`actSnap`, `actPublish`). Every instance is weakly fair, except the snapshot receive `actSnap`,
which is strongly fair.

**Assumptions, in English.**

1. *Bounded workload* (hypothesis `hN` of every theorem). There is a bound `N` such that every
   request id a client sends is below `N`. Clients may reuse ids; nothing else is assumed of them.
2. *Weak fairness* of: the BFF forwarding each request it has received; the backend beginning the
   transaction for each forwarded request; the BFF relaying each reply; the backend finishing each
   decided transaction; the store answering each load; the backend sending each commit and
   handling each conflict; the store deciding each commit packet; the publisher sending each
   committed event to each subscriber that may read it.
3. *Strong fairness* of the backend's snapshot receive, per (request, tenant, snapshot, base).
   This is the one assumption stronger than weak fairness, and it is needed (see below).
4. *The network.* Nothing is lost or tampered with (as in the protocol). A host receives a packet
   by acting on it, and that host action is the fair action of assumption 2 or 3. There is no
   separate network-delivery action.
5. The kernel, the sessions and the subscriptions are the static configuration of the safety layer.

Fairness is per instance (per packet and per request), not per host. IronFleet states fairness
per host action. Here a host action names the packet it processes, because the network is a set
and a host that processes "some pending packet" could keep choosing the same one.

**Theorems** (`Liveness.lean`).

- `reply_leadsTo`: under 1 to 3, a request with a valid session (`C.sessions tok = some (t, p)`)
  eventually gets a reply: `req r tok i ~> ∃ o, clientReply r o`.
- `publish_leadsTo`: under 1 to 2, a committed event (`e ∈ evs`) eventually reaches every
  subscriber `sid` whose tenant matches and whose principal may read the post
  (`K.readable e.post p`): `e ∈ evs ~> frame sid e`.
- `decided_relayed_leadsTo`: under 1 to 2, a decided request's reply is eventually relayed:
  `decided r = some o ~> clientReply r o`.

The argument for `reply_leadsTo`, by contradiction. The store's revision of the tenant is
monotone and bounded by `N` (each accepted event decides a distinct request below `N`), so it is
eventually final, `R`. Once `R` is final, no commit on `R` is ever sent: the store would decide
it, and a decision would answer the request. The transaction loads again after each conflict, so the
store sends it a snapshot of `R`; that snapshot is taken infinitely often under strong fairness,
so the backend commits on `R`. That is the contradiction. The transaction is loading infinitely
often by the commit-conflict-load cycle, which weak fairness gives.

**Non-vacuity.** `protoSpec_nonvacuous` exhibits a behaviour satisfying `protoSpec`, with the
bound `N = 2` and request 1 refused and answered. The behaviour is the refusal run of
`refusal_reachable`, written state by state, then constant. In the final state every enabled host
action is a no-op, so the constant tail satisfies each fairness assumption. This shows the
assumptions are consistent and the theorems apply. The witness does not exercise an accepted
commit; the accept path is covered by the invariants only.

**What does not hold.**

1. *Reply under weak fairness of the snapshot receive.* The network is a set, so a snapshot is
   never retired. Take two requests, `r` and `q`, the bound `N = 2`, and the schedule: `r` loads at
   revision 0 and receives snapshot `S0`; `q` is accepted, so the revision becomes 1; `r` commits
   on base 0, which conflicts; `r` loads again, and the store sends `S1` on base 1. The adversary
   then always takes `S0`: `r` takes `S0`, commits on base 0, conflicts, and loads again, forever.
   Every instance enabled forever is taken, so weak fairness holds. `S1` is enabled only while the
   transaction is loading, which the schedule leaves each time, so weak fairness imposes nothing on
   it. Request `r` is never answered. Strong fairness forces `S1` to be taken, which commits on the
   final base, so the store decides it. This failure is argued here, not formalised; the proof uses
   assumption 3.

   *The protocol change that would make weak fairness suffice* (argued, not formalised): tag each
   load and snapshot with the transaction's attempt number, and let the backend accept only the
   snapshot of its current attempt. The snapshot of attempt `k+1` is sent after the conflict of
   attempt `k`, so its base strictly exceeds the base of attempt `k`. Revisions are bounded, so the
   attempts are finite, and the last attempt commits. This changes a definition of `Protocol.lean`,
   which this layer does not do.

2. *Reply without the bounded workload.* Take an unbounded stream of other requests, each accepted
   between a snapshot of `r` and `r`'s commit. Every attempt of `r` then conflicts, while each
   request is still fairly scheduled. This is argued, not formalised, so the bounded-workload
   hypothesis is needed for the reply theorem.

3. Not modelled: crash and restart of the outbox, tenant creation, and sessions or subscriptions
   that change at run time. The publish and decision theorems do not need assumption 3.

## 9. Reduction (C4, WG-5660)

IronFleet's reduction argument (section 3.6 of the paper) is machine-checked in `lean/Strata/Reduction/`:
`Transactions.lean` (part (a), one tenant's optimistic transactions), `Distributed.lean` (part (b), the
generic reduction) and `Backend.lean` (part (b) instantiated for the protocol of section 8). All three
are imported from `lean/Strata.lean`, and every theorem in them is listed in `lean/Audit.lean`. Each
depends only on `propext`, `Classical.choice` and `Quot.sound`.
There is no `sorry`, no `admit`, no added axiom and no new dependency.

### (a) Concurrent optimistic transactions of one tenant

The model (`TState`, `TNext`) is at the level the backend runs a tenant's requests. A transaction
loads the current revision (`load`), computes the kernel's verdict on its snapshot (`compute`), and
commits with the revision guard: `decideOk` and `decideErr` apply when the revision is still the one
loaded, and `conflict` sends the transaction back to loading when it is not. Any interleaving of these
steps across transactions is a behaviour (`TReachable`).

| Theorem | States |
|---|---|
| `serial_equivalence` | in every reachable state each revision is the serial execution of the committed transactions before it; each decision's result is the kernel's verdict on the state at its revision, which is the state after the previous commit; a committed result is the tenant at the next revision; a transaction is decided at most once |
| `conflict_no_effect` | a conflicting attempt leaves the tenant, its history and the decisions unchanged |
| `refusal_no_effect` | a refused decision leaves the tenant's history and revision unchanged, and the accepted decisions unchanged |
| `decideOk_refines_store`, `decideErr_refines_store` | a decision is the protocol's `PNext.storeAccept` or `PNext.storeRefuse` on the store |
| `conflict_refines_store` | a conflict is the protocol's `PNext.storeConflict`, which changes no store state |
| `step_refines_store` | every step is one of those protocol store steps, or no store step (a load or a compute) |

Relation to `Protocol.lean`: the protocol already decides a commit atomically (`storeAccept`,
`storeRefuse`, `storeConflict`), and its backend reads the tenant at a revision and sends a commit
guarded by that revision. The model here is the finer picture: a transaction's load and compute are
separate steps, so other transactions' commits can land between them. `step_refines_store` shows that
this finer picture refines the atomic store step, so each interleaving of the transaction steps is a
protocol run.

### (b) The distributed reduction, generic

`Distributed.lean` splits every host step into I/O events. An event `Ev` names its host, the number of
that host's step it belongs to, and its I/O: a packet received (`recv`), a time-dependent input
(`tick`), or a packet sent (`send`). A fine-grained execution is a list of events, with the events of
different hosts interleaved arbitrarily.

- **The obligation** (`Obl`): within each host step the events are receives, then at most one
  time-dependent event, then sends.
- **Well-formedness**: each host's events are in step order (`HostOrd`); no packet is received before
  it is sent (`NetOk`, the packets available from the start plus those sent by earlier events).
- **Fine-grained execution** (`FineExec`) of a host step relation `HStep`: each event belongs to a step
  that exists, the obligation holds for every step, the network rule holds, and each host's steps are
  run by `HStep` from its initial state with the inputs and outputs the events record.

| Theorem | States |
|---|---|
| `reduction` | every fine-grained execution is equivalent to a contiguous one: a permutation of the same events; the same events for each host in the same order (so each host receives the same packets in the same order, sends the same packets in the same order); each host step's events form one block; the result is a fine-grained execution |
| `reorder_core` | the reordering as an explicit list of blocks, in an order in which each host's steps increase |
| `run_congr` | a run of a fine-grained execution is a run of any execution with the same events for each step, so each host's local state sequence is the same |

The proof is by induction on the length of the trace. Take the first event at which some step is
complete: a send, or the last event of its step. Its step has no receive from another step: the events
before it send nothing, so their receives are of packets available from the start, and the step's events
can be moved in front of them without breaking any receive or any per-host order. The rest of the trace
is reordered by the induction hypothesis. The move is one block move per step, justified by the two
facts above (no send precedes the candidate, and each receive of the step is of a packet available
before it), not by a sequence of single swaps.

### The backend instance

`Backend.lean` instantiates `reduction` for the protocol of section 8:

- `Role`: the client, the BFF, the backend, the store and the publisher. `Own` is the state a host owns:
  the backend its transactions and its answers without a session, the store the tenant's history and
  decisions, the others nothing.
- `RStep`: one constructor per protocol step (`PNext`), with the owning host's state before and after,
  the packets it receives (at most one) and the packets it sends, and `idle`, a host step that does
  nothing.
- `bridge_backend`, `bridge_store`, `bridge_other`: each `RStep` is one protocol step (or none, for
  `idle`) that changes only the owning host's state and reads only that state, the packets received and
  the static configuration.
- `bridge_run`: the blocks of a reordered fine-grained execution, taken in order, are protocol steps from
  the initial state.

| Theorem | States |
|---|---|
| `backend_fine_refines` | every fine-grained execution of the backend, with `RStep` as the host step relation and the initial owned states, refines a protocol behaviour (`ProtoReachable`) and so a behaviour of the system spec (`SpecReachable`) |

Modelling decisions the instance makes:

1. The backend's hosts take no time-dependent input: `RStep` has no `tick` constructor, so a fine
   execution with a time-dependent event in a step that runs has no run. The protocol has no clock; the
   implementation must keep it so, which is C3's to show.
2. A host step that does nothing (`idle`) is a fine step of no protocol step. It changes no state, so it
   is invisible to the protocol.
3. The fine-grained execution is required to satisfy the obligation (`FineExec.obl`), which is the
   IronFleet requirement on the implementation's steps. Nothing else about the implementation is assumed.

### What is left

- **Liveness** (C2): the reduction says nothing about progress.
- **The implementation** (C3): this section reduces the fine-grained interleavings of the protocol's
  hosts to the protocol's steps. The Rust backend's own steps are not shown to satisfy the obligation or
  the `RStep` shape; that is the C3 shell's job, and its check is the obligation on each host step.
- **Part (a) and the instance**: the instance covers the protocol with any number of tenants (its steps are
  per request). Part (a) models one tenant and is not yet a lemma about the instance's backend steps; the
  link is `step_refines_store`, which says each of its store steps is a protocol store step.
## Sources

- C. Hawblitzel et al., *IronFleet: Proving Practical Distributed Systems Correct*, SOSP 2015:
  https://www.andrew.cmu.edu/user/bparno/papers/ironfleet.pdf (sections 2.5, 3, 4, 6, 7, 8,
  Figure 12)
- IronFleet code: https://github.com/microsoft/Ironclad/blob/master/ironfleet/README.md
- A. Lattuada et al., *Verus: A Practical Foundation for Systems Verification*, SOSP 2024, and the
  IronKV port: https://verus-lang.github.io/paper-sosp24-artifact/case-studies
- X. Sun et al., *Anvil: Verifying Liveness of Cluster Management Controllers*, OSDI 2024:
  https://www.usenix.org/conference/osdi24/presentation/sun-xudong
- G. Pîrlea et al., *Veil*, CAV 2025: https://github.com/verse-lab/veil,
  https://lean-lang.org/use-cases/veil/
- *How We Built Cedar: A Verification-Guided Approach*, FSE 2024: https://arxiv.org/abs/2407.01688
- Aeneas and SymCrypt:
  https://www.microsoft.com/en-us/research/blog/verifying-rust-cryptography-in-symcrypt-from-standards-to-code/
- Lean on WebAssembly: https://github.com/leanprover/lean4/issues/16134,
  https://github.com/leanprover/lean4/pull/16135, https://github.com/leanprover/lean4/pull/16142
