//! The Strata runtime: the hand-written, generic code that generated code plugs into.
//!
//! Generated code (from `specifications/spec.jsonc`) provides types, records, contracts and
//! implementation-level machines. This crate provides everything those need to run, once, for
//! every machine and contract:
//!
//! - [`machine`]: the [`Machine`] trait a generated machine implements, and the [`Outcome`] its
//!   `step` returns.
//! - [`system`]: the [`System`] host that runs every machine of a deployment on one queue,
//!   delivering inputs, emits and timers in `(due, seq)` order to the instances the generated
//!   [`Router`] names, one atomic step per entry, as Exegra's simulator does. A failed step
//!   commits nothing and its entry becomes a dead letter (Exegra ends the run instead); the
//!   module documentation lists every remaining difference.
//! - [`host`]: the single-machine [`Host`] that runs one machine's steps in storage
//!   transactions, with an outbox for emitted events, handed to a hand-written dispatcher, and
//!   persisted timers.
//! - [`store`]: the storage traits the host uses, with [`memory`] and [`sqlite`]
//!   implementations.
//! - [`clock`]: the [`Clock`] every timer and retry reads, with a [`TestClock`] for tests.
//! - [`routes`] and [`client`]: serving a contract over axum and calling one from another
//!   component.
//! - [`server`] and [`jobs`]: process bootstrap and background loops shared by the services.
//! - [`telemetry`]: the [`Telemetry`] hooks the runtime reports through.
//!
//! The crate knows nothing about tenants, identity or content: it depends on no other Strata
//! crate (a test checks this). See `docs/runtime.md` for the full contract.

#![forbid(unsafe_code)]

pub mod client;
pub mod clock;
pub mod host;
pub mod jobs;
pub mod machine;
pub mod memory;
pub mod routes;
pub mod server;
pub mod sqlite;
pub mod store;
pub mod system;
pub mod telemetry;

pub use client::{
    random_key, CallBackoff, CallError, CallRetry, Client, Credentials, KeySource, NoCredentials,
};
pub use clock::{Clock, SystemClock, TestClock, Time};
pub use host::{
    Committed, DispatchError, DispatchReport, Dispatcher, Envelope, Handled, Host, HostError,
    RetryPolicy, TickReport,
};
pub use machine::{
    Delivery, EffectRef, Emit, Isolation, Machine, MemoryOutcome, Outcome, Receiver, RecordError,
    StepError, TimerCommand,
};
pub use memory::MemoryStore;
pub use routes::{
    Access, Authorizer, Caller, Contract, ContractError, ContractSpec, Exposure, Facing, Failure,
    FailureBody, FailureCode, GatewayRoute, Presented, Principal, Refusal, Routes, RuntimeBody,
    Scheme, ServiceToken, Site, TenantScope, CSRF_TOKEN, TENANT,
};
pub use sqlite::{Database, SqliteMachineStore};
pub use store::{
    DeadLetter, MachineStore, NewEntry, NewMessage, NewTimer, OutboxEntry, OutboxStore, QueueEntry,
    QueueStore, RecordStore, StorageError, StoredRecord, TimerEntry, TimerStore, Tx,
};
pub use strata_kernel::TenantId;
pub use system::{
    Delivered, Member, Reached, RouteTable, Router, Routing, StepReport, System, SystemTick,
};
pub use telemetry::{JsonLines, Level, NoTelemetry, Telemetry};
