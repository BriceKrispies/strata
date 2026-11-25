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
