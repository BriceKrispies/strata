//! Delayed emits (`{emit, after}`) through the host, on the in-memory store and on a SQLite file:
//! written to the outbox in the step's transaction with a not-before time, never dispatched at
//! commit, handed to the receiver by the outbox drain once the test clock reaches that time;
//! `at-least-once` retried with the same id until delivered, `at-most-once` dispatched once.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use strata_runtime::{
    Delivery, DispatchError, Dispatcher, Envelope, Host, Isolation, Machine, MachineStore, Outcome,
    RecordError, StepError, TestClock, Time,
};

const DELAY: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Waiting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Ask {
    Remind { text: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Said {
    Due { text: String },
    Ping,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ReminderRecord {
    id: String,
    asked: u32,
}

/// Every reminder asks the mailer for `Due` (at-least-once) and the pager for `Ping`
/// (at-most-once), both `DELAY` after the step.
struct Reminder;

impl Machine for Reminder {
    const NAME: &'static str = "reminder";
    const RECORD: &'static str = "reminders";
    const ISOLATION: Isolation = Isolation::Serializable;
    const CREATES: bool = true;

    type State = Phase;
    type Vars = u32;
    type Event = Ask;
    type Emitted = Said;
    type Key = String;
    type Record = ReminderRecord;

    fn initial(_: &String) -> (Phase, u32) {
        (Phase::Waiting, 0)
    }

    fn from_record(record: &ReminderRecord) -> Result<(Phase, u32), RecordError> {
        Ok((Phase::Waiting, record.asked))
    }

    fn to_record(
        key: &String,
        _: &Phase,
        asked: &u32,
        _: Option<&ReminderRecord>,
    ) -> Result<ReminderRecord, RecordError> {
        Ok(ReminderRecord {
            id: key.clone(),
            asked: *asked,
        })
    }

    fn step(
        state: &Phase,
        asked: &u32,
        event: &Ask,
        _: Time,
    ) -> Result<Outcome<Reminder>, StepError> {
        let Ask::Remind { text } = event;
        Ok(Outcome::to(*state, asked + 1)
            .emit_after(
                "mailer",
                Delivery::AtLeastOnce,
                Said::Due { text: text.clone() },
                DELAY,
            )
            .emit_after("pager", Delivery::AtMostOnce, Said::Ping, DELAY))
    }
}

#[derive(Default)]
struct Receivers {
    seen: Mutex<Vec<Envelope<Said>>>,
    failing: AtomicBool,
}

impl Receivers {
    fn seen(&self) -> Vec<Envelope<Said>> {
        self.seen.lock().unwrap().clone()
    }
}

impl Dispatcher<Said> for Receivers {
    async fn dispatch(&self, envelope: &Envelope<Said>) -> Result<(), DispatchError> {
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
) -> Host<Reminder, S, Receivers> {
    Host::new(store, receivers.clone(), Arc::new(clock.clone())).unwrap()
}

fn remind(text: &str) -> Ask {
    Ask::Remind { text: text.into() }
}

pub async fn waits_for_its_delay<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(1_000_000));
    let receivers = Arc::new(Receivers::default());
    let host = host(open(), &receivers, &clock);

    let committed = host.commit("a".into(), remind("call")).await.unwrap();
    assert!(
        committed.emits.is_empty(),
        "nothing is dispatched at commit"
    );
    assert_eq!(committed.delayed.len(), 2);
    assert_eq!(host.dispatch(committed).await.delivered, 0);
    assert_eq!(host.store().pending().unwrap(), 2);

    // Not before its time, also across a restart.
    clock.advance(DELAY - Duration::from_millis(1));
    assert_eq!(host.drain_outbox().await.unwrap().delivered, 0);
    let host = self::host(open(), &receivers, &clock);
    assert_eq!(host.tick().await.unwrap().dispatched.delivered, 0);
    assert!(receivers.seen().is_empty());

    clock.advance(Duration::from_millis(1));
    assert_eq!(host.drain_outbox().await.unwrap().delivered, 2);
    let seen = receivers.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].receiver.as_str(), "mailer");
    assert_eq!(seen[0].delivery, Delivery::AtLeastOnce);
    assert!(seen[0].id.is_some());
    assert_eq!(
        seen[0].event,
        Said::Due {
            text: "call".into()
        }
    );
    assert_eq!(seen[1].receiver.as_str(), "pager");
    assert_eq!(seen[1].delivery, Delivery::AtMostOnce);
    assert_eq!(seen[1].id, None);
    assert_eq!(seen[1].event, Said::Ping);
    assert_eq!(host.store().pending().unwrap(), 0);
    assert_eq!(host.drain_outbox().await.unwrap().delivered, 0);
}

pub async fn retries_only_at_least_once<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(1_000_000));
    let receivers = Arc::new(Receivers::default());
    let host = host(open(), &receivers, &clock);
    host.handle("a".into(), remind("call")).await.unwrap();

    // Both receivers are down when the delay is up.
    receivers.failing.store(true, Ordering::SeqCst);
    clock.advance(DELAY);
    let report = host.drain_outbox().await.unwrap();
    assert_eq!((report.deferred, report.dropped), (1, 1));
    assert_eq!(host.store().pending().unwrap(), 1);

    // The at-least-once emit comes back after the retry delay, with its id; the at-most-once
    // one is gone.
    receivers.failing.store(false, Ordering::SeqCst);
    assert_eq!(host.drain_outbox().await.unwrap().delivered, 0);
    clock.advance(Duration::from_secs(1));
    assert_eq!(host.drain_outbox().await.unwrap().delivered, 1);
    let seen = receivers.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].receiver.as_str(), "mailer");
    assert_eq!(host.store().pending().unwrap(), 0);
}

/// Two hosts on one store: a delayed at-most-once emit is dispatched once, by the host that
/// removes its entry; an at-least-once one may be offered by both but ends up delivered.
pub async fn two_hosts_dispatch_once<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(1_000_000));
    let receivers = Arc::new(Receivers::default());
    let one = host(open(), &receivers, &clock);
    let two = host(open(), &receivers, &clock);
    one.handle("a".into(), remind("call")).await.unwrap();
    clock.advance(DELAY);

    let (first, second) = tokio::join!(one.drain_outbox(), two.drain_outbox());
    first.unwrap();
    second.unwrap();
    let seen = receivers.seen();
    let pings = seen
        .iter()
        .filter(|e| e.receiver.as_str() == "pager")
        .count();
    assert_eq!(pings, 1, "{seen:?}");
    assert!(seen.iter().any(|e| e.receiver.as_str() == "mailer"));
    assert_eq!(one.store().pending().unwrap(), 0);
}
