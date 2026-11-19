# The runtime crate

`crates/runtime` (package `strata-runtime`) is the hand-written, generic code that code generated
from `specifications/spec.jsonc` plugs into. Generated code supplies types, records, contracts
and implementation-level machines. The runtime supplies everything those need to run, once, for
every machine and contract: the machine host, the stores it uses, contract routes and clients,
the clock, telemetry, and the process bootstrap the services share.

Two different things are called "runtime". The specification's component `runtime` (C-10) is the
service that runs the kernel for a tenant, and today that is `crates/backend`. This document is
about the crate `strata-runtime`, which any component can build on.

## The line

The runtime is generic over generated types through traits. It depends on no other Strata crate,
and `crates/runtime/tests/dependencies.rs` fails the build if it declares or names
`strata-kernel`, `strata-substrate`, `strata-backend`, `strata-bff` or `strata-protocol`, or any
other `strata-*` crate. Dependencies run the other way: `storage-sqlite`, `backend` and `bff` use
the runtime.

| Belongs to the runtime | Stays platform-specific |
|---|---|
| Running a machine's step in a transaction, persisting its record, the outbox, timers | What a machine does (generated `step`), and what a record holds |
| SQLite connection setup, pragmas, migration ledger, `IMMEDIATE` write and read transactions | The tenant store's tables and statements (`storage-sqlite`) |
| Serving a contract: authorization hook, body decoding, timeout, idempotent replay, wire format | Who may call: the `Authorizer` (service token, sessions, tenants) |
| Calling a contract: typed client, retries for idempotent contracts (`CallRetry`) | Credentials a caller presents |
| Clock, telemetry hooks, constant-time secret comparison | Identity, sessions, CSRF, tenant resolution, content, views |
| Process bootstrap: environment, bind, serve until shutdown, exit status, blocking work, background loops | Each service's configuration, routes and background work |

## What generated code implements

The compiler (`crates/spec-compiler`, `specc`) writes the generated crate formatted by rustfmt
from the toolchain on PATH, so it passes `cargo fmt --check`; when rustfmt cannot be run it
refuses with `specc.rustfmt-missing` instead of writing unformatted code.

### `Machine` (one per `behavior` with `level: "implementation"`)

```rust
pub trait Machine: Send + Sync + 'static {
    const NAME: &'static str;        // the behavior id
    const RECORD: &'static str;      // instances.storedIn
    const ISOLATION: Isolation;      // transaction.isolation
    const CREATES: bool;             // storage.creates

    type State: Clone + Debug + Send + Sync + 'static;
    type Vars: Clone + Debug + Send + Sync + 'static;
    type Event: Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static;
    type Emitted: Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static;
    type Key: Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static;    // instances.key
    type Record: Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static;

    fn initial(key: &Self::Key) -> (Self::State, Self::Vars);
    fn from_record(record: &Self::Record) -> Result<(Self::State, Self::Vars), RecordError>;
    fn to_record(key: &Self::Key, state: &Self::State, vars: &Self::Vars,
                 previous: Option<&Self::Record>) -> Result<Self::Record, RecordError>;
    fn step(state: &Self::State, vars: &Self::Vars, event: &Self::Event, now: Time)
        -> Result<Outcome<Self>, StepError>;
}
```

- Everything is pure. A machine has no clock of its own: `now` is the host clock's time when the
  transition's transaction runs (for a fired timer, when it is fired, which may be after its
  deadline). It is what the specification's guards and effects call `time`, so time in an
  implementation-level machine is milliseconds since the Unix epoch, and a timer's `after` is
  milliseconds too. Elapsed time also reaches a machine as an event fired by a timer it set (an
  event with `elapsed: true`).
- `step` takes the first enabled transition in declaration order. An ignored event returns the
  same state and variables with nothing emitted. Generated machines follow Exegra here: an event
  the state lists in `ignores`, an event no transition of the state is enabled for, and an event
  reaching a terminal state are all ignored (Exegra's `ignored` outcome), never an error. A
  failing evaluation, such as checked arithmetic that overflows, is
  `StepError::Rejected` and nothing is committed, as a failing step is not committed in Exegra.
  `StepError::NoTransition` and `StepError::Terminal` remain for hand-written machines that
  refuse instead.
- `to_record` receives the record as loaded so that fields the machine does not own are carried
  over.
- Where the machine is held in its record is `storage` in the specification: each variable in
  the field `storage.fields` maps it to (default the field of its own name), the state in
  `storage.state` (default `state`, a string field or an enum field, through
  `storage.stateValues` when the enum's values are not the state ids). A variable may sit in an
  optional field of its repr, read back as its initial value when unset, or in a field of a
  declared id or scalar type over its repr. The compiler refuses a machine whose storage does
  not fit with Exegra's `ready.storage-*` codes, so a specification Exegra accepts is one specc
  accepts.
- `CREATES` is `storage.creates`. A creating machine (`true`) starts a key with no record from
  `initial` and writes every required field itself. A non-creating machine (`false`, the
  default) steps only records that already exist, made by someone else such as a contract: its
  `to_record` copies every field it does not hold from `previous`, and refuses a missing
  `previous` with a `RecordError` (the host never passes one; see below).
- The key's JSON text is its storage key, so two keys are the same instance exactly when they
  serialise identically.

`Outcome<M>` holds the next `state` and `vars`, the `emits` in effect order (each an
`Emit { receiver: Receiver, delivery: Delivery, event, after: Option<Duration> }`), and the
`timers` (`TimerCommand::Set { name, after, event }` or `TimerCommand::Cancel { name }`).
Generated code builds one with `Outcome::to(state, vars).emit(receiver, delivery, event)
.emit_after(receiver, delivery, event, after).set_timer(name, after, event).cancel_timer(name)`.

A delayed emit (`emit_after`, the specification's `{emit, to, after}`) and a timer are different
things: the delayed emit goes to its receiver `after` the step, through the outbox; a timer fires
its event back at the same instance. Neither stands in for the other.

`Isolation` is `ReadCommitted < Snapshot < Serializable`. `Delivery` is `AtMostOnce` or
`AtLeastOnce`. A `Receiver` is the behavior, component or contract id an emit is addressed to.

### `Contract` (one per contract operation)

```rust
pub struct ContractSpec {
    pub id: &'static str,        // ref, e.g. "CT-012"
    pub path: &'static str,      // the route it is served at
    pub access: Access,          // exposure, auth, tenantScope, development
    pub timeout: Duration,       // timeoutMs
    pub idempotent: bool,        // idempotent
}

pub trait Contract: Send + Sync + 'static {
    const SPEC: ContractSpec;
    type Request: Serialize + DeserializeOwned + Send + Sync + 'static;
    type Response: Serialize + DeserializeOwned + Send + 'static;
    type Error: Serialize + DeserializeOwned + ContractError + Send + 'static;
}

pub trait ContractError {
    fn status(&self) -> u16;     // the HTTP status the error travels with
}
```

The generated async trait for a contract (methods returning `impl Future + Send`) is served by
registering one handler per operation:

```rust
Routes::new(authorizer)   // or Routes::gateway(authorizer): public contracts only
    .serve::<GrantLease, _, _>(move |caller, request| { let s = service.clone(); async move { s.grant(caller, request).await } })
    .into_router()   // an axum Router, merged into the service's router
```

and called from another component with
`Client::new(base, credentials)?.call::<GrantLease>(&request).await`.

`Routes::serve_checked` takes a handler whose error is a `Failure<C::Error>` instead of the
contract's error, so the handler can also refuse a call as the runtime does
(`Failure::Runtime { code, message }`); generated routes answer a broken validation rule with
`invalid_request` and a policy's deny with `forbidden` that way. `serve` is `serve_checked` with
every error a contract error. A runtime failure is never recorded for replay.

## What generated code calls

- `Outcome`, `Emit`, `TimerCommand`, `Delivery`, `Receiver`, `Isolation`, `StepError`,
  `RecordError` and `Time` from `step` and the record conversions.
- `Routes::serve_checked`, `Client::call` and `Client::call_retrying` for contracts (the
  `routes` module), and `Host::handle` to bring a request or another machine's event to a
  machine (the `handlers` module).
- Nothing else. Generated code never touches a store or a socket itself; a machine's time is the
  `now` the host passes to `step`, and the time an audit records is the routes' `hooks.now()`,
  the system clock unless the deployment overrides it.

## What stays hand-written

Per deployment, outside generated code:

- A `Dispatcher<E>` per machine: maps a `Receiver` id to a channel, a client call or another
  host.

  ```rust
  pub trait Dispatcher<E>: Send + Sync + 'static {
      fn dispatch(&self, envelope: &Envelope<E>) -> impl Future<Output = Result<(), DispatchError>> + Send;
  }
  ```

  `Envelope { id: Option<u64>, source, key, receiver, delivery, event }`. `id` is the outbox id
  of an `at-least-once` event and stays the same on every redelivery.
- An `Authorizer`: checks a contract's `Access` against what the request presents.

  ```rust
  pub trait Authorizer: Send + Sync + 'static {
      fn authorize(&self, access: &Access, request: &Presented<'_>)
          -> impl Future<Output = Result<Caller, Refusal>> + Send;
  }
  ```

  Generated code provides `routes::SpecAuthorizer<H: routes::AuthHoles>`, built from the
  specification's schemes: the deployment fills the holes (`site`, `session`, `visitor_token`,
  `service_secret`, `provisioning`, `delegation`, `signature`, `signed_address`, `development`),
  each of which admits nobody by default, and the CSRF, origin, role, step-up and tenant checks
  are generated. `ServiceToken { header, secret }` is provided for a service network: the service
  scheme needs the secret in the header, and a caller presenting it is subject `service` with the
  attribute `kind = service`; an anonymous contract that asks for no browser check admits anyone;
  every other scheme is refused. `Caller { subject, correlation, attributes }`: `attributes` is whatever else the
  authorizer established, by name; the runtime never reads it, and generated routes read
  `kind`, `tenant` and `role` from it for the specification's policies and audits.
- `Credentials` for outgoing calls (`fn apply(&self, RequestBuilder) -> RequestBuilder`);
  `NoCredentials` and `ServiceToken` are provided.
- Which store, clock and telemetry a host runs with, and where `Host::run` is spawned.
- Contracts with `realisation: "hand"` (holes): their trait implementation is written by hand
  and marked `/// Implements: H-nnn`; the runtime serves it like any other.
- Everything platform-specific listed in the table above.

## The machine host

`Host<M, S, D>` runs machine `M` against a store `S: MachineStore` and a dispatcher
`D: Dispatcher<M::Emitted>`, reading time from an `Arc<dyn Clock>`. `Host::new` fails with
`HostError::Unsupported` when the store cannot run `M::ISOLATION`.

| Method | Does |
|---|---|
| `handle(key, event)` | `commit`, then `dispatch` |
| `commit(key, event)` | the transaction below; returns `Committed` without dispatching |
| `dispatch(committed)` | dispatches its emits in order |
| `fire_due_timers()` | fires every due timer of `M` |
| `drain_outbox()` | dispatches every due outbox entry of `M` |
| `tick()` | `fire_due_timers`, then `drain_outbox` |
| `run(period, shutdown)` | `tick` every `period` until `shutdown` |
| `load(key)` | the stored state, variables and record version |

Store calls are synchronous and run on a blocking worker thread (`tokio::task::spawn_blocking`).

### The transition transaction

For one event, in one transaction opened at `M::ISOLATION` or stronger:

1. Load the record of `key` from collection `M::RECORD` and read it with `M::from_record`.
   With no record, a creating machine (`M::CREATES`) starts from `M::initial(key)`; a
   non-creating one refuses the event with `HostError::RecordMissing { machine, key }`, which
   `handle` and `commit` return to the caller: nothing is stored, scheduled or emitted. A timer
   whose key has no record any more is discarded like a timer whose step fails.
2. Run `M::step` with `now`, the clock's time read once for the transaction.
3. Save `M::to_record(..)` with the version that was loaded as the expected version; a different
   stored version is a conflict. The version starts at 1 and grows by one per save. An ignored
   event is saved too (the version grows; nothing else changes).
4. Apply the timer commands in order. `Set` stores a timer `(M::NAME, key, name)` firing at
   `now + after`, replacing a timer of the same name with a new id; `Cancel` removes it.
5. Write every delayed emit to the outbox with `not_before = next_attempt_at = now + after`,
   whatever its delivery. Write every immediate `at-least-once` emit to the outbox, held back
   from the drain until
   `now + retry.first`.
6. Commit.

If any step fails (a `StepError`, a `RecordError`, a missing record of a non-creating machine,
an encoding failure, a storage failure or a conflict), the transaction is rolled back: no record, timer or outbox entry is written and
nothing is dispatched. The error is returned as a `HostError`. The host does not retry a
conflicting transaction itself.

### Delivery

After commit, emits are dispatched in the order the step emitted them.

- **`at-least-once`.** Written to the outbox in the transaction. On a successful dispatch the
  entry is removed. On a failed dispatch it stays, with `attempts` increased and
  `next_attempt_at = now + retry.delay(attempts)`. `drain_outbox` offers every entry whose
  `next_attempt_at` has passed, with the same id, until a dispatch succeeds. If the process stops
  after commit and before dispatch, the entry is still in the outbox and is offered once the hold
  (`retry.first`) has passed. A receiver may therefore see an event more than once (a dispatch that
  succeeded but whose removal failed or was cut off, or two hosts draining one store), and must
  drop duplicates by `Envelope::id`. Retries never stop. An entry whose event cannot be decoded is
  kept at the longest delay and reported.
- **`at-most-once`.** Not written anywhere. Dispatched once after commit. A failed dispatch is
  counted as dropped and reported; a process stop before dispatch loses it. Never retried.
- **Delayed emits.** Never dispatched at commit (`Committed::delayed` lists their outbox ids).
  `drain_outbox` offers an entry only once the clock has reached its `not_before`. A delayed
  `at-least-once` emit is then treated exactly like the entry above: retried with the same id
  until a dispatch succeeds. A delayed `at-most-once` emit is removed from the outbox first and
  then dispatched once, with `Envelope::id = None`; a failed dispatch is dropped, and a stop
  between removal and dispatch loses it. Both survive a restart until they are due.

The default `RetryPolicy` is `first = 1 s`, `factor = 2`, `max = 300 s`:
`delay(n) = min(first * factor^(n-1), max)`.

There is no ordering guarantee between deliveries once a retry is involved: a deferred entry is
redelivered after later ones may have arrived.

### Timers

`fire_due_timers` lists the timers of `M` whose `fire_at` is at or before the clock's `now`,
earliest first. Each one is fired in its own transaction: the timer is removed by id, then its
event goes through the transition transaction above, and both commit together. A timer that was
cancelled or replaced since it was listed is not removed by id and nothing happens. So a timer
fires at most once per setting, and a fired timer's effects and its removal are atomic. A timer
whose step fails (`StepError`, `RecordError` or an encoding failure) can never succeed, so it is
removed in a separate transaction without effect, counted as discarded and reported. A storage
failure leaves the timer in place for the next tick.

Timers fire late, never early: they fire on the first `tick` at or after `fire_at`.

### Isolation

Each store states which isolation levels it can run (`RecordStore::supports`). Both provided
stores run every transaction serializably, which satisfies every declared level: the in-memory
store holds one mutex for the whole transaction and the SQLite store opens every transaction
`IMMEDIATE` on its single connection.

## Storage traits

Records, events and keys cross the storage traits as JSON text, so one store holds every machine
and an adapter is written once.

```rust
pub trait Tx {
    fn load(&mut self, collection: &str, key: &str) -> Result<Option<StoredRecord>, StorageError>;
    fn save(&mut self, collection: &str, key: &str, data: &str, expected: Option<u64>) -> Result<u64, StorageError>;
    fn enqueue(&mut self, message: &NewMessage) -> Result<u64, StorageError>;
    fn set_timer(&mut self, timer: &NewTimer) -> Result<u64, StorageError>;
    fn cancel_timer(&mut self, machine: &str, key: &str, name: &str) -> Result<bool, StorageError>;
    fn take_timer(&mut self, id: u64) -> Result<bool, StorageError>;
}

pub trait RecordStore: Send + Sync + 'static {
    fn supports(&self, isolation: Isolation) -> bool;
    fn transact<T, E>(&self, isolation: Isolation, work: impl FnOnce(&mut dyn Tx) -> Result<T, E>)
        -> Result<T, E> where E: From<StorageError>;
}

pub trait OutboxStore: Send + Sync + 'static {
    fn due_messages(&self, source: &str, now: Time, limit: usize) -> Result<Vec<OutboxEntry>, StorageError>;
    fn mark_delivered(&self, id: u64) -> Result<(), StorageError>;
    fn defer(&self, id: u64, attempts: u32, next_attempt_at: Time, error: &str) -> Result<(), StorageError>;
    fn pending(&self) -> Result<u64, StorageError>;
}

pub trait TimerStore: Send + Sync + 'static {
    fn due_timers(&self, machine: &str, now: Time, limit: usize) -> Result<Vec<TimerEntry>, StorageError>;
}

pub trait MachineStore: RecordStore + OutboxStore + TimerStore {}   // blanket-implemented
```

`transact` commits when `work` returns `Ok` and rolls back, keeping none of its writes, when it
returns `Err` or the commit fails. Outbox ids and timer ids are assigned by the store and never
reused.

- `MemoryStore`: the reference implementation, held in memory. A transaction works on a copy
  of the whole store that replaces the original only on success.
- `SqliteMachineStore::open(db: Database, now)`: on a SQLite `Database`, with the tables
  `runtime_records`, `runtime_outbox` and `runtime_timers` and the migration ledger
  `runtime_migrations`. Migration 2 gives `runtime_outbox` its `delivery` and `not_before`
  columns (existing entries: `at-least-once`, not before 0).

### SQLite

`sqlite::Database` is the one way Strata opens a SQLite file: a single connection behind a mutex,
a busy timeout of 5 s, `journal_mode = WAL`, `synchronous = FULL`, `foreign_keys = ON`.

```rust
impl Database {
    pub fn open(path: &Path) -> Result<Self, DbError>;
    pub fn open_in_memory() -> Result<Self, DbError>;
    pub fn location(&self) -> &str;
    pub fn durable(&self) -> bool;
    pub fn lock(&self) -> Result<MutexGuard<'_, Connection>, DbError>;
    pub fn migrate(&self, ledger: &str, migrations: &[Migration], now: i64) -> Result<(), DbError>;
    pub fn write<T, E>(&self, error: impl Fn(DbError) -> E,
                       work: impl FnOnce(&Transaction<'_>) -> Result<T, E>) -> Result<T, E>;
    pub fn read<T, E>(&self, error: impl Fn(DbError) -> E,
                      work: impl FnOnce(&Transaction<'_>) -> Result<T, E>) -> Result<T, E>;
}
```

`migrate` applies, in one `IMMEDIATE` transaction, the steps above the highest version recorded
in the named ledger table. `write` runs an `IMMEDIATE` transaction and commits on `Ok`; `read`
runs a read transaction that sees only whole commits. The tenant store (`SqliteStore`) runs on a
`Database` with the ledger `storage_migrations`, and `SqliteStore::database()` hands the same
database to `SqliteMachineStore`, so machine state can live in the tenant store's file.

## Contracts on the wire

- `POST <SPEC.path>` with the request as a JSON body.
- Success: status 200 and the response as JSON.
- Failure: the status and body the contract's `FailureBody` (`Contract::Body`) makes of it. The
  runtime's own body, `RuntimeBody`, answers a contract error with the error's `status()` and
  `{"kind": "contract", "error": <error>}`, and a runtime failure with
  `{"kind": "runtime", "code": <code>, "message": <text>}` at the code's status:
  `unauthenticated` 401, `forbidden` 403, `invalid_request` 400, `timeout` 504, `unavailable`
  503, `internal` 500, `idempotency_conflict` 409. A specification that states an error body has the generated crate supply
  a body of its own (below); the client reads failures back with the same `FailureBody`.

```rust
pub trait FailureBody<E>: Send + Sync + 'static {
    fn encode(failure: Failure<&E>, correlation: &str) -> serde_json::Result<(u16, Vec<u8>)>;
    fn decode(status: u16, body: &[u8]) -> Option<Failure<E>>;
}
```

`Access` is the contract's exposure (`Public`, `Internal`, `Local`) and its auth as data: a
`Scheme` (`Anonymous`, `Session`, `Service`, `ProvisioningToken`, `Signature`, `SignedAddress`,
`Delegation`, `InProcess`), the roles any one of which admits the caller, whether the tenant's
own policies decide in the handler, the CSRF and origin checks of a browser call, the step-up age,
the name of the secret a signature is verified with, the tenant scope and the development
condition. Nothing is a sentence and nothing is matched as a string.

The `Authorizer` is the one seam through which a route asks who the caller is:
`authorize(&Access, &Presented)`, where `Presented` is the request's headers, target (path and
query) and body. It refuses any scheme it cannot check in full. Generated code supplies
`routes::SpecAuthorizer`, built from the schemes, with the deployment's part (the session store,
the sites and their origins, the secrets) behind the `routes::AuthHoles` trait, every method of
which admits nobody until the deployment says otherwise. `ServiceToken` is the small authorizer
of a service network: the service scheme with the token, and anonymous calls that ask for no
browser check; everything else is refused.

`Access::requires` lists the capabilities a call needs (`auth.requires`). The generated
authorizer checks them last, in one function, `routes::permitted`: for every capability, the
tenant's plan entitles it or it is core, no feature flag gating it is off for the tenant, the
caller holds it, and a delegation the caller acts under passes it on within its token's scopes.
Two holes feed it: `entitlement(tenant)` (the tenant's plan and flag settings) and
`holding(principal)` (the principal's platform roles, its tenant roles and its teams with their
permissions, which are tenant data, its live grants, and the delegation it acts under). An
anonymous caller, a service and a party proved by signature or provisioning token hold what the
platform roles of their kind confer, which the specification fixes. `routes::OPERATIONS` is the
operation catalogue (AGT-001..003): every contract reachable from outside its process with its
description, access and the JSON Schema of its input; `routes::catalogue` marks each for a
principal with whether it may be attempted, by the same check. `ServiceToken` does not read
`requires`.

**Tenant capabilities.** A tenant role is tenant data: its permissions are capability strings
and actions on object types. `strata_kernel::rules::held_capabilities` gives the capabilities a
principal holds in a tenant, exactly those named by a role assigned to it or by a role of a team
it belongs to; the deployment's `holding` hole fills `Holding::tenant_roles` and `Holding::teams`
from it (`role_capabilities` gives one role's). A role confers a capability only by naming it.
Assigning a role, a team's role, a team membership or an invitation's roles is refused unless the
assigner holds **every** permission and capability the role grants (ADM-152; the owner holds all).
An invitation is redeemed with the signed-in account's verified address, which the backend puts in
the kernel `Context` (`verified_address`, from the account store); the intent carries none
(ADM-153). `POST /api/members/<operation>` is the route to these intents.

`Routes::new` is the service network's listener (public and internal contracts);
`Routes::gateway` faces browsers and the internet and serves public contracts only, so an
internal contract has no route there and its path answers 404. A local contract is never served.

Per call, the server: asks the `Authorizer` about `SPEC.access` and answers its refusal at once;
for an idempotent contract with an `idempotency-key` header, replays an answer already given
under the same scope (contract, tenant, caller subject, key) to the same request (its SHA-256
digest) without running the handler. The tenant is the authorizer's `tenant` attribute, else the
`x-tenant-id` header. The same key with another body is refused with `idempotency_conflict`
(409; generated code answers it as the error kind `conflict`, unless `errorBody.runtime` maps
`idempotency-conflict` elsewhere). A key is reserved while its first call runs: a duplicate waits
for it, up to the contract's timeout, and gets its answer. The server decodes the body
(`invalid_request` if it does not decode) and runs the handler on a task of its own; if it has not
finished within `SPEC.timeout` the call answers `timeout`, but nothing is cancelled: the handler
finishes and its answer is recorded, so the client's retry is answered from it and never runs a
half-applied handler again. Answers are recorded when the contract is idempotent and a key was
given; runtime failures (including `timeout`) are not. The recorded answers are bounded by bytes
(`Routes::with_replay_bytes`, 8 MiB by default), oldest first out.

The client sends `idempotency-key` (fresh per call, 128 random bits; if the operating system's
random source fails the call fails with `CallError::Key` and sends nothing, the same across its retries) for idempotent
contracts and the correlation id in `x-correlation-id` when given. It retries an idempotent
contract on a transport failure or a runtime `timeout` or `unavailable`; a contract that is not
idempotent is sent once. How often and how long it waits is a `CallRetry`:

```rust
pub struct CallRetry { pub attempts: u32, pub backoff: CallBackoff, pub timeout: Option<Duration> }
pub enum CallBackoff {
    Linear(Duration),                                   // wait step * n after the n-th failure
    Fixed(Duration),                                    // always the same wait
    Exponential { base: Duration, cap: Option<Duration> }, // base * 2^(n-1), at most cap
}
```

The default is 3 attempts with `Linear(100 ms)` and no timeout of its own (an attempt is bounded by
the contract's `SPEC.timeout`, plus one second for the transport). `Client::with_attempts(n)` sets
the attempts, `Client::with_retry(policy)` the whole policy, and
`Client::call_retrying::<C>(&request, correlation, &policy)` makes one call under another policy.
`CallRetry::timeout`, when set, replaces `SPEC.timeout` for each attempt.

A retry policy of the specification (`retry` entity) is generated as a `RetryPolicy` constant in
the generated crate's `retries` module, which depends on no runtime. Its numbers map one to one:
`attempts` to `CallRetry::attempts`, `backoff` (`Fixed { base }` or `Exponential { base, cap }`) to
`CallBackoff`, `timeout` to `CallRetry::timeout`, and its `idempotency_key` names the input that
identifies a repeat. The error of a call is
`CallError::{Contract(E), Runtime { code, message }, Transport(String), Codec(String)}`.

## Cross-cutting constructs in generated code

Policies, validations, retentions, quotas, retries and audits are generated into pure code in the
generated crate (modules `policies`, `validations`, `retentions`, `quotas`, `retries`, `audits`,
and `support` for the types they share). These modules do not use this runtime; the generated
routes (below) apply them to every call of a served contract, and a deployment that serves a
contract by hand applies them itself:

- **Who is acting.** `support::Caller { kind, id, tenant, role }` is what generated policies and
  audits read. Generated routes build it from the runtime's `Caller`: the subject is the id, the
  attributes `kind`, `tenant` and `role` the rest. They call
  `policies::authorize_<contract>(&caller, &request[, record])`, which returns
  `support::Decision::{Allow, Deny}`, and refuse a `Deny` with `FailureCode::Forbidden`.
- **Validation.** Call `request.validate()` (and `record.validate_rules()`) before running a
  handler; answer an `Err(ValidationError { id, message })` with `FailureCode::InvalidRequest`.
- **Quotas.** `quotas::QUOTAS` lists every quota; a counter store keeps one `quotas::Counter` per
  scope and calls `Quota::check(counter, now)`, which is pure. On `Exceeded`, answer with the
  contract error named by `Quota::error`.
- **Retention.** `retentions::RETENTIONS` is what a periodic job (`jobs::every`) executes: delete,
  call the record's `anonymise()`, or archive to the named component, for each record whose
  period has passed.
- **Audits.** `audits::audit_<id>(&caller, &request, &response, now)` gives the entry to append;
  `into_record` completes it with the storage key; for a `chained-hash` audit
  `audits::chain_<id>(&mut record, previous)` sets the chain field before the append and
  `verify_<id>` checks a stored log.

## Wiring in generated code

specc also generates the code that connects the contracts and machines to this runtime, in
three modules of the generated crate (`crates/spec-compiler-wiring-fixture` is an example):

- **`routes`.** Per contract a component implements, a unit type implementing `Contract`. Its
  `SPEC.id` is the contract's `ref` (its id without one), `auth` its `auth` (empty without one),
  `timeout` its `timeoutMs` (30 s without one), `idempotent` its `idempotent` (false without one).
  **The path** is the contract's `binding.path` when it states one, and otherwise
  `/<implementing component>/<contract id>`; a path must be plain segments (letters, digits,
  `-`, `_`, `.`, `~`) without parameters, since every contract is a `POST` of a JSON body.

  **Statuses.** Each contract error travels with the status the specification gives it: its
  `status`, else the status of its `kind` in `system.generation.errorKinds`, else the default
  `errorKind` or `errorStatus`. specc refuses an error with none (`ready.error-status-missing`).

  **The error body.** With `system.generation.errorBody`, `Contract::Body` is the generated
  `ErrorBody`: every failure is answered with the stated struct type (in Strata, `api-error`,
  which is `strata_protocol::api::ErrorReply`). An error whose payload is that type is answered
  with the payload, its code set to the error's kind; any other error with a body of its kind
  whose message is the error's name; a runtime failure with a body of the kind
  `errorBody.runtime` maps it to, at that kind's status. The correlation field is set to the
  caller's correlation id. The client reads a body back as the contract error that is alone with
  its kind, and otherwise as the runtime failure of that kind.

  **One contract at a time.** Per served contract:

  ```rust
  pub trait <Contract>Handler: Send + Sync + 'static {
      fn handle(&self, request: <Contract>Request)
          -> impl Future<Output = Result<<Contract>Response, <Contract>Error>> + Send;
  }   // implemented by every implementation of contracts::<C>
  pub trait <Contract>Hooks: Send + Sync + 'static [+ AuditClock + QuotaCounter + QuotaWait] {
      /* only what this contract needs */
  }
  pub fn serve_<contract><A, S, H>(routes: Routes<A>, handler: Arc<S>, hooks: Arc<H>) -> Routes<A>
  where A: Authorizer, S: <Contract>Handler, H: <Contract>Hooks;
  ```

  `AuditClock` (`now`), `QuotaCounter` (`count`) and `QuotaWait` (`wait`) are shared by every
  contract that needs them. A hooks trait with no method of its own is implemented for every type
  with its supertraits, `()` included when it has none.

  Per implementing component `<C>`, built from those pieces:

  ```rust
  pub trait <C>Hooks: <Contract>Hooks + ... {}   // implemented for every type that has them all
  pub fn <c>_routes<A, S, H>(routes: Routes<A>, service: Arc<S>, hooks: Arc<H>) -> Routes<A>
  where A: Authorizer, S: contracts::<C> + Send + Sync + 'static, H: <C>Hooks;
  pub struct <C>Client<K: Credentials = NoCredentials> { client: Client<K> }
  impl<K: Credentials> <C>Client<K> {
      pub fn new(client: Client<K>) -> Self;
      pub async fn <contract>(&self, request: &<Contract>Request)
          -> Result<<Contract>Response, CallError<<Contract>Error>>;   // one per contract
  }
  ```

  Each route runs, after the runtime's authorizer and decoding: `request.validate()` when the
  contract has validations (`invalid_request`); `policies::authorize_<contract>` with the record
  `hooks.<contract>_resource(&request)` loads when a policy names a `resource` (`forbidden`);
  `hooks.count(&QUOTA, &caller)` for each quota on the contract (a refusing quota answers with its
  contract error, whose payload `hooks.<quota>_exceeded(retry_after)` gives when it has one; a
  queueing or throttling quota calls `hooks.wait`); the handler; and on success
  `hooks.append_<audit>(audits::audit_<audit>(.., hooks.now()))` for each audit. A hook that
  fails answers `unavailable` before the call and `internal` after it. A client method of a
  contract with a retry policy calls under it.
- **`deps`.** Per implementing component, `<C>Deps` holds exactly one `<D>Client` per component
  `<D>` it calls (relationships of kind `calls` whose target implements contracts), built by
  `<C>Deps::new(<d>, ..)` and read by `deps.<d>()`. An implementation handed its `Deps` can reach
  only what the specification says it calls; a call it does not declare has no client and does
  not compile.
- **`handlers`.** Per machine, a module with the glue for each event whose arrival the
  specification states. An event that names a contract (`contract`, `role: request`) gets
  `<contract>_event(&request) -> (Key, Event)` and `<contract>(&host, &request)`, which calls
  `Host::handle`; the key and payload come from the request's inputs of the same names. An event
  another machine emits to this machine's id gets `from_<emitter>(&emitted)` and
  `deliver_from_<emitter>(&host, &emitted)`, for the emitter's `Dispatcher`. Events fired by the
  machine's own timers need nothing. Events whose arrival is not stated get nothing and are
  listed in the module's documentation.

## Clock and telemetry

```rust
pub struct Time(pub i64);                       // milliseconds since the Unix epoch
pub trait Clock: Send + Sync + 'static { fn now(&self) -> Time; }
```

`SystemClock` reads the wall clock. `TestClock::at(t)` stands still until `advance(d)` or
`set(t)`; clones share their time. Every deadline and retry decision reads the clock; only
telemetry durations use the process's monotonic clock.

```rust
pub trait Telemetry: Send + Sync + 'static {
    fn event(&self, level: Level, name: &str, fields: &[Field<'_>]);
    fn metric(&self, name: &str, value: f64, fields: &[Field<'_>]);
    fn span(&self, name: &str, elapsed: Duration, ok: bool, fields: &[Field<'_>]);
}
```

`NoTelemetry` drops everything (the host's and the routes' default). `JsonLines { service }`
writes one JSON object per line on standard output in the services' log shape. The runtime
reports names, keys, receivers, ids, counts and durations, never payloads or record contents:
`machine.transition` and `contract.call` spans, `outbox.deferred`, `outbox.unmarked`,
`outbox.unreadable`, `emit.dropped`, `timer.discarded` and `machine.tick_failed` events.

## Shared process code

- `server::env_var`, `server::env_number`, `server::bind`, `server::serve` (axum until a shutdown
  future, then finish requests in flight), `server::interrupted` (Ctrl-C),
  `server::exit_on_error`, `server::blocking` (store work off the executor).
- `jobs::every(period, work)` (the backend's session purge) and `jobs::pump(wake, idle, work)`
  (the backend's outbox publisher: woken by each commit, polling every 500 ms for commits made by
  another process).
- `routes::same_secret`, the constant-time comparison the backend's service-token gate and the
  BFF's request tokens use.

## What moved, and what did not

Moved into the runtime, with the crates now calling it:

- From `storage-sqlite`: opening the connection with its pragmas, the migration runner and its
  ledger, the `IMMEDIATE` write and read transaction handling. The tenant store's tables,
  statements and semantics are unchanged and stay there.
- From `backend`: serving the router until shutdown, the blocking-work helper behind
  `App::blocking`, the outbox publisher's wake-or-poll loop and the session purge's periodic loop,
  the start-up's environment reading, binding, Ctrl-C and exit.
- From `bff`: serving until shutdown, environment reading, binding, Ctrl-C and exit.
- From both: the constant-time secret comparison.

Left where it is, as platform-specific: the tenant outbox itself (the `events` table and its
`published` flag, the kernel's `DomainEvent`s and their visibility decision in the hub), sessions,
accounts and sign-in, the service-token gate's route list, CSRF and site resolution in the BFF,
the BFF's call to the backend (its error type is the protocol's `ErrorReply`, not a contract's),
view projection, the WebSocket relay, the Impact Lab, seeding and the structured logger in
`strata-protocol`.
