//! The Strata runtime: the hand-written, generic code that generated code plugs into.
//!
//! Generated code (from `specifications/spec.jsonc`) provides types, records, contracts and
//! implementation-level machines. This crate provides everything those need to run, once, for
//! every machine and contract:
//!
//! - [`machine`]: the [`Machine`] trait a generated machine implements, and the [`Outcome`] its
//!   `step` returns.
//! - [`host`]: the [`Host`] that runs a machine's steps in storage transactions, with an outbox
//!   for emitted events and persisted timers.
//! - [`store`]: the storage traits the host uses, with [`memory`] and [`sqlite`]
//!   implementations.
//! - [`clock`]: the [`Clock`] every timer and retry reads, with a [`TestClock`] for tests.
//! - [`telemetry`]: the [`Telemetry`] hooks the runtime reports through.
//!
//! The crate knows nothing about tenants, identity or content: it depends on no other Strata
//! crate (a test checks this). See `docs/runtime.md` for the full contract.

#![forbid(unsafe_code)]

pub mod clock;
pub mod host;
pub mod machine;
pub mod memory;
pub mod sqlite;
pub mod store;
pub mod telemetry;

pub use clock::{Clock, SystemClock, TestClock, Time};
pub use host::{
    Committed, DispatchError, DispatchReport, Dispatcher, Envelope, Handled, Host, HostError,
    RetryPolicy, TickReport,
};
pub use machine::{
    Delivery, Emit, Isolation, Machine, Outcome, Receiver, RecordError, StepError, TimerCommand,
};
pub use memory::MemoryStore;
pub use sqlite::{Database, SqliteMachineStore};
pub use store::{
    MachineStore, NewMessage, NewTimer, OutboxEntry, OutboxStore, RecordStore, StorageError,
    StoredRecord, TimerEntry, TimerStore, Tx,
};
pub use telemetry::{JsonLines, Level, NoTelemetry, Telemetry};
