//! A hand-written lease machine driven through the host, on the in-memory store and on a SQLite
//! file: persistence, at-least-once redelivery after a crash between commit and dispatch, timers
//! fired by the test clock, and at-most-once emits that are never retried. A second machine over
//! the same records that does not create them shows the host refusing an event for a key with no
//! record.
//!
//! The lease: `Grant` hands out a new token and arms an expiry timer; a `Result` is accepted
//! only with the current token; when the timer fires, the lease is granted again to the same
//! holder with a fresh token. Every grant records the time it was made, which the host passes to
//! `step` from its clock.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use strata_runtime::telemetry::Field;
use strata_runtime::{
    Clock, Delivery, DispatchError, Dispatcher, Envelope, Host, HostError, Isolation, Level,
    Machine, MachineStore, NewTimer, Outcome, RecordError, StepError, Telemetry, TestClock, Time,
};

/// Telemetry that keeps the names of the events reported at level error.
#[derive(Default)]
struct Errors {
    names: Mutex<Vec<String>>,
}

impl Telemetry for Errors {
    fn event(&self, level: Level, name: &str, _: &[Field<'_>]) {
        if level == Level::Error {
            self.names.lock().unwrap().push(name.to_string());
        }
    }
    fn metric(&self, _: &str, _: f64, _: &[Field<'_>]) {}
    fn span(&self, _: &str, _: Duration, _: bool, _: &[Field<'_>]) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Idle,
    Granted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LeaseVars {
    token: u64,
    holder: Option<String>,
    accepted: u64,
    granted_at: Time,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum LeaseEvent {
    Grant { holder: String },
    Result { token: u64, value: String },
    Expired { token: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum LeaseEmit {
    Granted { holder: String, token: u64 },
    Accepted { token: u64, value: String },
    Stale { token: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LeaseRecord {
    id: String,
    phase: Phase,
    token: u64,
    holder: Option<String>,
    accepted: u64,
    granted_at: Time,
    /// Not the machine's: carried over from the previous record on every save.
    note: String,
}

const TERM: Duration = Duration::from_secs(30);

struct Lease;

fn grant(vars: &LeaseVars, holder: String, now: Time) -> Outcome<Lease> {
    let token = vars.token + 1;
    Outcome::to(
        Phase::Granted,
        LeaseVars {
            token,
            holder: Some(holder.clone()),
            accepted: vars.accepted,
            granted_at: now,
        },
    )
    .emit(
        "worker",
        Delivery::AtLeastOnce,
        LeaseEmit::Granted { holder, token },
    )
    .set_timer("expiry", TERM, LeaseEvent::Expired { token })
}

impl Machine for Lease {
    const NAME: &'static str = "lease";
    const RECORD: &'static str = "leases";
    const ISOLATION: Isolation = Isolation::Serializable;
    const CREATES: bool = true;

    type State = Phase;
    type Vars = LeaseVars;
    type Event = LeaseEvent;
    type Emitted = LeaseEmit;
    type Key = String;
    type Record = LeaseRecord;

    fn initial(_: &String) -> (Phase, LeaseVars) {
        (
            Phase::Idle,
            LeaseVars {
                token: 0,
                holder: None,
                accepted: 0,
                granted_at: Time(0),
            },
        )
    }

    fn from_record(record: &LeaseRecord) -> Result<(Phase, LeaseVars), RecordError> {
        if record.phase == Phase::Granted && record.holder.is_none() {
            return Err(RecordError("a granted lease without a holder".into()));
        }
        Ok((
            record.phase,
            LeaseVars {
                token: record.token,
                holder: record.holder.clone(),
                accepted: record.accepted,
                granted_at: record.granted_at,
            },
        ))
    }

    fn to_record(
        key: &String,
        state: &Phase,
        vars: &LeaseVars,
        previous: Option<&LeaseRecord>,
    ) -> Result<LeaseRecord, RecordError> {
        Ok(LeaseRecord {
            id: key.clone(),
            phase: *state,
            token: vars.token,
            holder: vars.holder.clone(),
            accepted: vars.accepted,
            granted_at: vars.granted_at,
            note: previous.map_or_else(|| "created by the lease".into(), |p| p.note.clone()),
        })
    }

    fn is_elapsed(event: &LeaseEvent) -> bool {
        matches!(event, LeaseEvent::Expired { .. })
    }

    fn step(
        state: &Phase,
        vars: &LeaseVars,
        event: &LeaseEvent,
        now: Time,
    ) -> Result<Outcome<Lease>, StepError> {
        let unchanged = || Outcome::to(*state, vars.clone());
        Ok(match (state, event) {
            (Phase::Idle, LeaseEvent::Grant { holder }) => grant(vars, holder.clone(), now),
            // Already granted: the grant is ignored.
            (Phase::Granted, LeaseEvent::Grant { .. }) => unchanged(),
            (Phase::Granted, LeaseEvent::Result { token, value }) if *token == vars.token => {
                Outcome::to(
                    Phase::Idle,
                    LeaseVars {
                        token: vars.token,
                        holder: None,
                        accepted: vars.accepted + 1,
                        granted_at: vars.granted_at,
                    },
                )
                .emit(
                    "ledger",
                    Delivery::AtLeastOnce,
                    LeaseEmit::Accepted {
                        token: *token,
                        value: value.clone(),
                    },
                )
                .cancel_timer("expiry")
            }
            (_, LeaseEvent::Result { token, .. }) => unchanged().emit(
                "worker",
                Delivery::AtMostOnce,
                LeaseEmit::Stale { token: *token },
            ),
            (Phase::Granted, LeaseEvent::Expired { token }) if *token == vars.token => {
                let holder = vars.holder.clone().unwrap_or_default();
                grant(vars, holder, now)
            }
            // A timer from an earlier grant.
            (_, LeaseEvent::Expired { .. }) => unchanged(),
        })
    }
}

/// The lease machine over the same records, except that it does not create them: it steps only
/// leases [`Lease`] has created.
struct Borrowed;

impl Machine for Borrowed {
    const NAME: &'static str = "borrowed-lease";
    const RECORD: &'static str = Lease::RECORD;
    const ISOLATION: Isolation = Isolation::Serializable;
    const CREATES: bool = false;

    type State = Phase;
    type Vars = LeaseVars;
    type Event = LeaseEvent;
    type Emitted = LeaseEmit;
    type Key = String;
    type Record = LeaseRecord;

    fn initial(key: &String) -> (Phase, LeaseVars) {
        Lease::initial(key)
    }

    fn from_record(record: &LeaseRecord) -> Result<(Phase, LeaseVars), RecordError> {
        Lease::from_record(record)
    }

    fn to_record(
        key: &String,
        state: &Phase,
        vars: &LeaseVars,
        previous: Option<&LeaseRecord>,
    ) -> Result<LeaseRecord, RecordError> {
        let Some(previous) = previous else {
            return Err(RecordError("borrowed-lease does not create leases".into()));
        };
        Lease::to_record(key, state, vars, Some(previous))
    }

    fn step(
        state: &Phase,
        vars: &LeaseVars,
        event: &LeaseEvent,
        now: Time,
    ) -> Result<Outcome<Borrowed>, StepError> {
        let o = Lease::step(state, vars, event, now)?;
        Ok(Outcome {
            state: o.state,
            vars: o.vars,
            emits: o.emits,
            timers: o.timers,
            sequence: o.sequence,
            ignored: o.ignored,
        })
    }
}

/// Records what reaches the receivers, and fails every dispatch while `failing` is set.
#[derive(Default)]
struct Receivers {
    seen: Mutex<Vec<Envelope<LeaseEmit>>>,
    attempts: AtomicUsize,
    failing: AtomicBool,
}

impl Receivers {
    fn seen(&self) -> Vec<Envelope<LeaseEmit>> {
        self.seen.lock().unwrap().clone()
    }

    fn fail(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }
}

impl Dispatcher<LeaseEmit> for Receivers {
    async fn dispatch(&self, envelope: &Envelope<LeaseEmit>) -> Result<(), DispatchError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.failing.load(Ordering::SeqCst) {
            return Err(DispatchError("receiver down".into()));
        }
        self.seen.lock().unwrap().push(envelope.clone());
        Ok(())
    }
}

fn host<S: MachineStore>(
    store: Arc<S>,
    receivers: &Arc<Receivers>,
    clock: &TestClock,
) -> Host<Lease, S, Receivers> {
    Host::new(store, receivers.clone(), Arc::new(clock.clone())).unwrap()
}

fn start() -> (TestClock, Arc<Receivers>) {
    (
        TestClock::at(Time(1_000_000)),
        Arc::new(Receivers::default()),
    )
}

fn key(id: &str) -> String {
    id.to_string()
}

pub async fn persists<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let (clock, receivers) = start();
    {
        let host = host(open(), &receivers, &clock);
        let handled = host
            .handle(
                key("a"),
                LeaseEvent::Grant {
                    holder: "w1".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(handled.state, Phase::Granted);
        assert_eq!(handled.vars.token, 1);
        assert_eq!(handled.vars.granted_at, clock.now());
        assert_eq!(handled.version, 1);
        assert_eq!(handled.dispatched.delivered, 1);
    }

    // A restarted process finds the instance where it was left.
    let host = host(open(), &receivers, &clock);
    let (state, vars, version) = host.load(&key("a")).await.unwrap().unwrap();
    assert_eq!(state, Phase::Granted);
    assert_eq!(vars.token, 1);
    assert_eq!(vars.holder.as_deref(), Some("w1"));
    assert_eq!(version, 1);
    assert!(host.load(&key("b")).await.unwrap().is_none());

    let handled = host
        .handle(
            key("a"),
            LeaseEvent::Result {
                token: 1,
                value: "done".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(handled.state, Phase::Idle);
    assert_eq!(handled.vars.accepted, 1);
    assert_eq!(handled.version, 2);

    let events: Vec<LeaseEmit> = receivers.seen().into_iter().map(|e| e.event).collect();
    assert_eq!(
        events,
        [
            LeaseEmit::Granted {
                holder: "w1".into(),
                token: 1
            },
            LeaseEmit::Accepted {
                token: 1,
                value: "done".into()
            },
        ]
    );
    assert!(receivers.seen().iter().all(|e| e.id.is_some()));
    assert_eq!(host.store().pending().unwrap(), 0);
}

pub async fn redelivers_after_a_crash<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let (clock, receivers) = start();
    let id = {
        let host = host(open(), &receivers, &clock);
        let committed = host
            .commit(
                key("a"),
                LeaseEvent::Grant {
                    holder: "w1".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(committed.emits.len(), 1);
        committed.emits[0]
            .id
            .expect("at-least-once has an outbox id")
        // The process stops here: committed, not dispatched.
    };
    assert!(receivers.seen().is_empty());

    let host = host(open(), &receivers, &clock);
    assert_eq!(host.load(&key("a")).await.unwrap().unwrap().1.token, 1);
    // Held back from the drain for the first retry delay after commit.
    assert_eq!(host.drain_outbox().await.unwrap().delivered, 0);
    clock.advance(Duration::from_secs(1));
    assert_eq!(host.drain_outbox().await.unwrap().delivered, 1);
    let seen = receivers.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].id, Some(id));
    assert_eq!(seen[0].delivery, Delivery::AtLeastOnce);
    assert_eq!(seen[0].receiver.as_str(), "worker");
    // Delivered once; the outbox is empty.
    assert_eq!(host.drain_outbox().await.unwrap().delivered, 0);
    assert_eq!(host.store().pending().unwrap(), 0);

    // A receiver that is down: deferred, then retried with the same id once the delay passes.
    receivers.fail(true);
    let committed = host
        .commit(
            key("b"),
            LeaseEvent::Grant {
                holder: "w2".into(),
            },
        )
        .await
        .unwrap();
    let retried = committed.emits[0].id;
    assert_eq!(host.dispatch(committed).await.deferred, 1);
    assert_eq!(host.store().pending().unwrap(), 1);
    receivers.fail(false);
    assert_eq!(host.drain_outbox().await.unwrap().delivered, 0);
    clock.advance(Duration::from_secs(1));
    assert_eq!(host.drain_outbox().await.unwrap().delivered, 1);
    assert_eq!(receivers.seen().last().unwrap().id, retried);
    assert_eq!(host.store().pending().unwrap(), 0);
}

pub async fn fires_timers<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let (clock, receivers) = start();
    let host = host(open(), &receivers, &clock);
    host.handle(
        key("a"),
        LeaseEvent::Grant {
            holder: "w1".into(),
        },
    )
    .await
    .unwrap();

    clock.advance(TERM - Duration::from_secs(1));
    assert_eq!(host.fire_due_timers().await.unwrap().fired, 0);
    clock.advance(Duration::from_secs(1));

    // The timer survives a restart and fires from the store.
    let host = self::host(open(), &receivers, &clock);
    let fired = host.fire_due_timers().await.unwrap();
    assert_eq!(fired.fired, 1);
    assert_eq!(fired.dispatched.delivered, 1);
    let (state, vars, _) = host.load(&key("a")).await.unwrap().unwrap();
    assert_eq!((state, vars.token), (Phase::Granted, 2));
    // The re-grant ran at the timer's deadline (here also the clock's time).
    assert_eq!(vars.granted_at, clock.now());
    assert_eq!(
        receivers.seen().last().unwrap().event,
        LeaseEmit::Granted {
            holder: "w1".into(),
            token: 2
        }
    );
    // Fired once per setting.
    assert_eq!(host.fire_due_timers().await.unwrap().fired, 0);

    // A result under the old token is refused, under the new one accepted.
    let stale = host
        .handle(
            key("a"),
            LeaseEvent::Result {
                token: 1,
                value: "late".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!((stale.state, stale.vars.token), (Phase::Granted, 2));
    assert_eq!(
        receivers.seen().last().unwrap().event,
        LeaseEmit::Stale { token: 1 }
    );
    let accepted = host
        .handle(
            key("a"),
            LeaseEvent::Result {
                token: 2,
                value: "done".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(accepted.state, Phase::Idle);

    // Accepting cancelled the expiry: nothing fires later.
    clock.advance(TERM * 3);
    assert_eq!(host.tick().await.unwrap().fired, 0);
    let (state, vars, _) = host.load(&key("a")).await.unwrap().unwrap();
    assert_eq!((state, vars.token, vars.accepted), (Phase::Idle, 2, 1));
}

pub async fn never_retries_at_most_once<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let (clock, receivers) = start();
    let host = host(open(), &receivers, &clock);
    // The lease is never granted, so every result is stale and nothing else is emitted.
    receivers.fail(true);
    let before = receivers.attempts.load(Ordering::SeqCst);
    let handled = host
        .handle(
            key("a"),
            LeaseEvent::Result {
                token: 99,
                value: "forged".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(handled.dispatched.dropped, 1);
    assert_eq!(host.store().pending().unwrap(), 0);

    receivers.fail(false);
    clock.advance(Duration::from_secs(600));
    assert_eq!(host.tick().await.unwrap().dispatched.delivered, 0);
    assert_eq!(receivers.attempts.load(Ordering::SeqCst), before + 1);
    assert!(!receivers
        .seen()
        .iter()
        .any(|e| matches!(e.event, LeaseEmit::Stale { .. })));

    // Committed but not dispatched before a crash: an at-most-once emit is simply gone.
    let committed = host
        .commit(
            key("a"),
            LeaseEvent::Result {
                token: 98,
                value: "forged".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(committed.emits[0].id, None);
    drop(committed);
    let host = self::host(open(), &receivers, &clock);
    clock.advance(Duration::from_secs(600));
    assert_eq!(host.tick().await.unwrap().dispatched.delivered, 0);
    assert_eq!(receivers.attempts.load(Ordering::SeqCst), before + 1);
}

pub async fn refuses_a_missing_record_when_not_creating<S: MachineStore>(
    open: impl Fn() -> Arc<S>,
) {
    let (clock, receivers) = start();
    let store = open();
    let borrowed: Host<Borrowed, S, Receivers> =
        Host::new(store.clone(), receivers.clone(), Arc::new(clock.clone())).unwrap();
    let grant = || LeaseEvent::Grant {
        holder: "w1".into(),
    };

    // No record for the key: refused, and nothing stored, scheduled or emitted.
    let refused = borrowed.handle(key("a"), grant()).await;
    match refused {
        Err(HostError::RecordMissing { machine, key }) => {
            assert_eq!(machine, "borrowed-lease");
            assert_eq!(key, "\"a\"");
        }
        Err(other) => panic!("expected RecordMissing, got {other}"),
        Ok(_) => panic!("an event for a key with no record was accepted"),
    }
    assert!(borrowed.load(&key("a")).await.unwrap().is_none());
    assert_eq!(store.pending().unwrap(), 0);
    clock.advance(TERM * 2);
    assert_eq!(borrowed.tick().await.unwrap().fired, 0);
    assert!(receivers.seen().is_empty());

    // Once the creating machine has made the record, the same event steps it.
    let lease = host(store.clone(), &receivers, &clock);
    lease
        .handle(
            key("a"),
            LeaseEvent::Result {
                token: 0,
                value: String::new(),
            },
        )
        .await
        .unwrap();
    let handled = borrowed.handle(key("a"), grant()).await.unwrap();
    assert_eq!((handled.state, handled.vars.token), (Phase::Granted, 1));
    assert_eq!(handled.version, 2);
}

/// A timer that no longer reads (an event whose shape changed) is set aside as a dead letter and
/// reported at level error; the other timers still fire and the outbox is still drained.
pub async fn quarantines_an_unreadable_timer<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let (clock, receivers) = start();
    let store = open();
    let errors = Arc::new(Errors::default());
    let host = host(store.clone(), &receivers, &clock).with_telemetry(errors.clone());
    let grant = |holder: &str| LeaseEvent::Grant {
        holder: holder.into(),
    };
    host.handle(key("a"), grant("w1")).await.unwrap();
    host.handle(key("b"), grant("w2")).await.unwrap();
    // `c` is committed but not dispatched: its message waits in the outbox.
    host.commit(key("c"), grant("w3")).await.unwrap();
    let fire_at = clock.now().after(TERM);
    let unreadable = r#"{"Gone":{"shape":1}}"#;
    store
        .transact(Isolation::Serializable, |tx| {
            tx.set_timer(&NewTimer {
                machine: "lease".to_string(),
                key: "\"a\"".to_string(),
                name: "expiry".to_string(),
                fire_at,
                event: unreadable.to_string(),
            })
        })
        .unwrap();

    clock.advance(TERM);
    let tick = host.tick().await.unwrap();
    assert_eq!((tick.fired, tick.quarantined), (2, 1), "{tick:?}");
    // b and c re-granted, and c's first message left the outbox in the same tick.
    assert_eq!(tick.dispatched.delivered, 3, "{tick:?}");
    assert_eq!(store.pending().unwrap(), 0);
    let dead = store.dead_letters().unwrap();
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert_eq!(dead[0].entry.event, unreadable);
    assert_eq!(dead[0].entry.timer.as_deref(), Some("expiry"));
    assert_eq!(
        dead[0].entry.target,
        Some(("lease".to_string(), "\"a\"".to_string()))
    );
    assert!(!dead[0].error.is_empty());
    assert!(errors
        .names
        .lock()
        .unwrap()
        .iter()
        .any(|n| n == "timer.quarantined"));
    // `a` was not touched, and the quarantined timer does not come back.
    assert_eq!(host.load(&key("a")).await.unwrap().unwrap().1.token, 1);
    assert_eq!(host.load(&key("b")).await.unwrap().unwrap().1.token, 2);
    clock.advance(TERM);
    let again = host.tick().await.unwrap();
    assert_eq!(again.quarantined, 0, "{again:?}");
}
