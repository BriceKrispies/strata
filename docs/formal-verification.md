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
| `http_result` login, second factor waiting | the code prompt, no session yet | not covered | 7 |
| `http_result` login, refused | the request ends; the form shows the refusal and drops the password (`loginRefused`) | covered | 3 (the refusal's words are wording, and focus is presentation, slice 4) |
| `action logout` | the sign-out request, when the screen offers sign-out | covered | 1 |
| `http_result` logout, accepted | no session; the person's state cleared; the address replaced | partly | 1; 5 (live socket reset) |
| `http_result` logout, refused | the request ends (the warning is presentation) | covered | 1 |
| `submit` search | navigates to the address the catalogue names for the values, and asks for its view | covered | 3 (`searchSubmit`) |
| `input` login | the typed value (`setField`); sends nothing | covered | 3 (`inputLogin`) |

#### Live connection

| Message | What update does | Status | Slice |
|---|---|---|---|
| `ws_open` | resets the refusal count and the stop flag | not covered | 5 |
| `ws_message` welcome | the live state starts; the view is read again on the first connection | not covered | 5 |
| `ws_message` event (a notice or a heartbeat) | applies the frame; the view is read again when the live bindings say so; the manifest on a schema notice; the session on a membership notice | not covered | 5 (tenant bindings: 11) |
| `ws_message` resync | carries on from the head; view and manifest read again | not covered | 5 |
| `ws_message` pong | a head beyond the connection's own reads the view again | not covered | 5 |
| `ws_closed`, not refused | the live state closes; a reconnect timer | not covered | 5 |
| `ws_closed`, refused | the session is read with a refusal counted; after five, live updates stop | not covered | 5 (the session answer is covered in 1) |
| `timer` heartbeat, reconnect, emit | a ping; a reconnect attempt unless stopped; a delayed frame | not covered | 5 |

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
| `action passkeys.add`, `passkeys.remove`; `http_result` passkeys.list, passkeys.register_start, passkeys.register_finish, passkeys.remove; `passkey_result` (registration) | the person's passkeys | not covered | 8 |
| `action factors.enrol_start`, `factors.enrol_cancel`, `factors.remove_totp`, `factors.new_codes`, `factors.dismiss_codes`, `factors.use`, `factors.use_passkey`, `factors.stepup_cancel` | the authenticator, recovery codes, the factor choice, the step-up prompt | not covered | 7 |
| `submit` second, totpconfirm, stepup; `input` second, totpconfirm, stepup | the second-factor code, the enrolment confirmation, the re-authentication | not covered | 7 |
| `http_result` factors.summary (entered with the desk), enrol_start, enrol_finish, remove_totp, new_codes | the factors section's state | not covered | 7 |
| `http_result` factors.pk_challenge; `passkey_result` (second factor) | a passkey as the second factor | not covered | 7 |
| `http_result` factors.second | the second factor completes a sign-in | not covered | 7 |
| `http_result` factors.stepup; any refusal `step_up_required` | the refused action is asked again once confirmed; the step-up prompt is shown | not covered | 7 |

#### Sessions list

| Message | What update does | Status | Slice |
|---|---|---|---|
| `action sessions.revoke`, `sessions.revoke_others`; `http_result` sessions.list (entered with the desk), sessions.revoke, sessions.revoke_others | the person's own sessions: list, end one, end the others | not covered | 8 |

#### Sign-in methods: links, invitations, identity providers

| Message | What update does | Status | Slice |
|---|---|---|---|
| `submit` link, `input` link; `http_result` links.request, links.sign_in | emailed sign-in: the request; the opened link is sent for checking; signs in | not covered | 6 |
| `submit` invite, `input` invite; `http_result` invites.accept | an owner's invitation: the password and its acceptance; signs nobody in | not covered | 6 |
| `action oidc.start`; `http_result` oidc.providers (entered on the sign-in page), oidc.start | the providers list; the start, which navigates the browser to the provider | not covered | 6 |
| `http_result` oidc.sign_in | the provider's answer is sent; signs in as a login does | not covered | 6 |

#### Records and the editor

| Message | What update does | Status | Slice |
|---|---|---|---|
| `submit` editor; `input` editor | save in a status; the two-step saves of a new record | not covered | 9 |
| `action editor.tab`, `editor.publish`, `editor.unpublish`, `editor.reload` | tabs, status changes, reload | not covered | 9 |
| `http_result` editor.save | saved (the ids and version taken) or refused (the wording, focus); an unknown outcome keeps the key | not covered | 9 |
| `http_result` editor.preview; the preview tab | the preview of the body | not covered | 9 |
| `action` row action (generated row buttons) and its confirmation | the row's action; the two-press confirmation | not covered | 9 |
| `http_result` post.action | the row action done or refused | not covered | 9 |
| The client's record-rule check before a save (`record-rules.js`) | a refusal before any request, in the kernel's words | not covered | 9 |

#### Settings

| Message | What update does | Status | Slice |
|---|---|---|---|
| `http_result` settings.definitions (entered with the page) | the settings list | not covered | 10 |
| `submit` settings, `submit` settings:level:id, `input` settings; `http_result` settings.write | a save of one setting at one level, at the revision read; read again after the answer | not covered | 10 |
| setting notice (`ws_message` event, kind setting) | the view is read again when the view reads the setting | not covered | 10 (with 5) |

#### Tenant interface

| Message | What update does | Status | Slice |
|---|---|---|---|
| `http_result` ui.manifest | a manifest not older than the held one is accepted; its parts are asked for | not covered | 11 |
| `http_result` ui.part | the part is held; a form whose field was removed names what was lost | not covered | 11 |
| schema notice (`ws_message` event) | the manifest is read again | not covered | 11 (with 5) |
| `action tenant.action` | a tenant action: confirmation, row prefill, opening a form | not covered | 11 |
| `submit` tenant:id, `input` tenant:id | a tenant form's values and submission | not covered | 11 |
| `http_result` tenant.submit, tenant.action | the operation's answer: toast, navigation, or the refusal's words | not covered | 11 |

#### Impact Lab

| Message | What update does | Status | Slice |
|---|---|---|---|
| `action lab.tab`, `lab.preset`, `lab.cancel`, `lab.apply`, `lab.replay`, `lab.verify`, `lab.diagnostics` | the Lab's controls; apply only when the projection is current | not covered | 12 |
| `submit` lab, `input` lab | the projection's form | not covered | 12 |
| `http_result` lab.overview (entered with the page), lab.run, lab.result, lab.apply, lab.replay, lab.verify, lab.diagnostics | the Lab's answers; polling the run | not covered | 12 |
| `timer` lab.poll | polls the run | not covered | 12 |
| `ws_message` run (verification, progress, cancelled, finished) | the run's frames | not covered | 12 |

### 7.2 Effects

| Effect | Emitted by | Status | Slice |
|---|---|---|---|
| `http` (method, path, body, csrf, idempotency key, correlation) | every request | partly | 1 (id and kind); 3 (csrf token, idempotency key, correlation still open; the sign-in and search bodies are the form's values) |
| `timer` request | every request (30 000 ms) | covered | 1 |
| `timer` toast | every toast (5 s, or 9 s for a warning) | partly | 4 (expiry is covered; raising a toast is not yet a constructor) |
| `timer` lab.poll | the Lab's run poll | not covered | 12 |
| `timer` heartbeat, reconnect, emit | the live connection and its delayed frames | not covered | 5 |
| `push_url` | an address change, with the replace flag | covered | 1 |
| `scroll_top` | navigation | covered | 1 |
| `ws_connect` | a socket open with the request token (at init with a session, and when a waiting socket connects) | not covered | 5 |
| `ws_send` | subscribe, ack, ping, and the Lab's cancel | not covered | 5 (cancel: 12) |
| `ws_close` | closing the socket | not covered | 5 |
| `store` | the theme choice in storage | covered | 4 (`themeCycle` stores the mode's key) |
| `passkey` (`create`, `get`) | the browser's passkey prompt | not covered | 6 (sign-in), 7 (second factor), 8 (registration) |
| `log` | the record-rule disagreement, and the worker's diagnostics | not covered | 9 |
| `navigate_away` | the redirect to an identity provider; emitted by `oidc.js` but absent from the `Effect` type in `types.js` (F5) | not covered | 6 |

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
| 5 | Live connection: the socket machine, the welcome, notices, resync, pong, the session frame when active, refused upgrades, the socket effects and timers | 1, 2 |
| 6 | Sign-in methods: passkey sign-in, emailed links, invitations, identity providers (OIDC and SAML), `navigate_away` | 3, 5 |
| 7 | Second factors and step-up: the waiting sign-in, the factors section, recovery codes, the step-up prompt and its retry, the passkey as a second factor | 3, 6 |
| 8 | The person's sessions and passkeys on the desk | 3, 7 |
| 9 | Records: the editor's saves, publish and unpublish, preview, row actions, paging, the client's record-rule check and its log | 3, 5 |
| 10 | Settings: the definitions, saves by level, setting notices | 3, 5 |
| 11 | Tenant interface: manifest, parts, tenant screens by address, tenant forms and actions, lost drafts, the held part (`Loop`) | 3, 5 |
| 12 | Impact Lab: overview, runs and their frames, polling, apply, replay, verify, diagnostics | 3, 5, 7 |
| 13 | View derivation: the page builders per screen kind, the title and navigation, the tenant's composition and display; `render_sound` over the page | 1, 9, 10, 11, 12 |
| 14 | Browser storage: the view cache with stamps, the offline queue, the leader tab, drafts at rest (`docs/frontend.md`, follow-ups 4 to 9) | 3, 5 |

### 7.7 Counts

The messages table has 78 rows (a row is one message, or one family of messages with the same handling):
22 covered, 10 partly, 46 not covered (after slice 4). The effects table has 14 rows: 4 covered, 2 partly, 8 not
covered. All 13 top-level message types that `step` switches on have rows, and so do the 10 effect
types `types.js` declares and `navigate_away`, which it does not declare. Slice 1 covers the covered
rows of routing, the view, the session and sign-in and sign-out; the other rows are slices 2 to 14.

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
