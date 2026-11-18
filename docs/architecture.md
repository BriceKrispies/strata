# Strata architecture

Strata is a metadata-driven, multi-tenant application substrate. The kernel defines what the
system means. Everything else interprets, executes, persists, presents, proves or simulates that
meaning.

This describes the prototype. The specification (`specifications/spec.jsonc`) sets a different
target and divides the work differently: an identity service holds sessions, a gateway and a
runtime sit in front of the kernel, every tenant has a database of its own on PostgreSQL, and the
components are listed under `components`. Where this document and the specification disagree, the
specification states what is intended. The boundary rules below are the prototype's own.

```
Browser main thread (DOM, view adapter)
        |  postMessage
Dedicated Web Worker (functional JS kernel + effect driver: fetch, WebSocket)
        |  HTTP + WebSocket, session cookie, CSRF token
strata-bff   127.0.0.1:4100   cookies, CSRF, origin checks, view-model projection, static assets
        |  HTTP + WebSocket on loopback, service token
strata-backend 127.0.0.1:4101 sessions, load -> kernel -> atomic commit -> outbox -> subscribers
        |
strata-kernel (pure)   +   Store trait (strata-storage-api)  ->  SQLite adapter | memory adapter
```

## Crates and dependency direction

Arrows point from a crate to what it may import. Nothing points back up.

| Crate | Role | May depend on |
|---|---|---|
| `kernel` | Pure transition engine: identifiers, value algebra, expressions, schema-as-data, authorization, validation, `transition`, `read`, state deltas, events, effects, canonical hashing | `serde`, `serde_json`, `sha2` only |
| `substrate` | Pure interpreters over kernel data: tenant definition bundles, JSON coercion, view-model projection, constrained Markdown, theme token schema, subscription matching | `kernel` |
| `storage-api` | Persistence contract (`Store`), capability statement, in-memory reference adapter, reusable conformance suite | `kernel` |
| `storage-sqlite` | SQLite adapter. All SQL, pragmas and table layout live here | `storage-api`, `kernel`, `rusqlite` |
| `verification` | Invariant evaluation over kernel states, proof-obligation table, Lean orchestration, correspondence fixtures, evidence report | `kernel` |
| `simulation` | Deterministic discrete-event simulator that drives the real kernel; traces, branching, comparison, minimisation, journal replay | `kernel`, `verification` |
| `protocol` | Wire types shared by backend and BFF: internal API, WebSocket protocol v1, structured log lines | `kernel` |
| `backend` | Imperative shell: clock, random tokens, Argon2, store, outbox publisher, subscriptions, Impact Lab runs | everything above |
| `bff` | Browser-facing shell. Talks only to the backend's internal API | `protocol`, `substrate`, `kernel` (types only) |

`crates/verification/tests/purity.rs` fails the build when the kernel or substrate gains a
forbidden dependency or mentions an ambient capability (filesystem, network, clock, randomness,
environment, process, interior mutability, `unsafe`).

## The kernel contract

```rust
pub fn transition(state: &State, intent: &Intent, ctx: &Context) -> Result<Transition, DomainError>
pub fn read(state: &State, request: &ReadRequest, ctx: &Context) -> Result<ReadResult, DomainError>
pub fn needs(schema: Option<&TenantSchema>, intent: &Intent) -> Vec<Need>
pub fn apply_delta(tenant: Option<&TenantState>, delta: &StateDelta) -> Result<TenantState, DomainError>
```

- `State` is a snapshot of **one** tenant, with a `Coverage` statement saying which records it
  contains. `needs` tells the shell which records an intent requires. `transition` recomputes the
  needs and rejects with `InsufficientState` when the snapshot does not cover them, so a partial
  snapshot can never produce a wrong acceptance.
- `Context` carries tenant, principal, session id, timestamp and correlation id. The kernel reads
  no clock and draws no randomness. Record identifiers come from a per-tenant counter.
- A `Transition` holds a `StateDelta` (base revision, new revision, ordered operations), the
  domain events, effect descriptions and an evidence trail of every check that was evaluated. A
  `DomainError` carries the same trail for a rejection.
- Schema is data: object types, fields, relationships (reference fields), constraints, roles,
  policies, workflows, named queries and view definitions, under one `SchemaVersion`.

## Request path

1. Worker kernel emits an `http` effect; the driver sends it with a correlation id.
2. BFF resolves the site tenant from its own configuration (host name), checks origin and CSRF
   token, forwards the opaque session token and the correlation id to the backend.
3. Backend resolves the session to `(tenant, principal)`, builds the `Context`, loads the
   snapshot named by `needs`, runs `transition`, and commits delta + events + journal entry in one
   storage transaction guarded by the tenant revision. A conflict reloads and retries.
4. The outbox publisher reads committed, unpublished events, hands them to the subscription hub
   and marks them published. Events are never published before commit, and a crash between
   commit and publish leaves them in the outbox for the next start.
5. Each WebSocket subscriber gets an event only if the kernel says the principal can read the
   record (before or after the change). The browser kernel refetches the affected view.

## Simulation and verification

The simulator runs `strata_kernel::World::step`, the same function composition the backend uses.
It owns logical time, the schedule, a seeded generator and fault injection, and produces a
replayable trace. Counterfactual branching replays one schedule against a forked world with a
proposed schema change and compares the two structurally. Every finding carries one of five
evidence classes: proven (Lean, abstract model), reproduced, soundly approximated, empirically
estimated, unknown. See `docs/verification.md` for what is and is not claimed.

## Frontend

`web/src/kernel` is pure: `update(state, message) -> { state, effects }` and `view(state)`. The
worker driver executes effects (fetch, WebSocket, timers) and posts view models to the main
thread, which patches the DOM with safe DOM APIs. The main thread issues no application requests.
