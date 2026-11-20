//! The system host on a store: one queue for two hand-written machines, delivered in
//! `(due, seq)` order at each entry's own time, an ignored event writing nothing, and a step that
//! fails in one receiver committing nothing in any.
//!
//! The counter counts `bump`s of its key and tells the watcher of the same key 10 ms later
//! (`bumped`, a delayed emit); the watcher counts what it hears and settles 5 ms after (a timer).
//! `both` reaches the watcher, which counts it, and the counter, which refuses it.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use strata_runtime::telemetry::Field;
use strata_runtime::{
    Delivered, Delivery, DispatchError, Dispatcher, Envelope, HostError, Isolation, Level, Machine,
    MachineStore, Member, NewEntry, NoTelemetry, Outcome, PurgeReport, RecordError, Retention,
    RetryPolicy, Router, Routing, StepError, System, Telemetry, TestClock, Time,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", content = "payload")]
enum In {
    #[serde(rename = "bump")]
    Bump { key: String },
    #[serde(rename = "noise")]
    Noise {},
    #[serde(rename = "both")]
    Both { key: String },
    #[serde(rename = "bumped")]
    Bumped { key: String },
    #[serde(rename = "settled")]
    Settled {},
    #[serde(rename = "ping")]
    Ping { key: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", content = "payload")]
enum Out {
    #[serde(rename = "bumped")]
    Bumped { key: String },
    #[serde(rename = "pinged")]
    Pinged {},
}

/// The longest key a record may have: the machines' stand-in for a generated record constraint.
const MAX_KEY: usize = 8;

/// The counter's `instances.max`.
const MAX_COUNTERS: u64 = 3;

fn validate(record: &Rec) -> Result<(), String> {
    if record.key.chars().count() > MAX_KEY {
        return Err(format!("key is longer than {MAX_KEY} characters"));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "settled")]
    Settled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Vars {
    n: u64,
    at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Rec {
    key: String,
    phase: Phase,
    n: u64,
    at: i64,
}

fn from_rec(r: &Rec) -> Result<(Phase, Vars), RecordError> {
    Ok((r.phase, Vars { n: r.n, at: r.at }))
}

fn to_rec(key: &str, phase: &Phase, vars: &Vars) -> Rec {
    Rec {
        key: key.to_string(),
        phase: *phase,
        n: vars.n,
        at: vars.at,
    }
}

struct Counter;

impl Machine for Counter {
    const NAME: &'static str = "counter";
    const RECORD: &'static str = "counters";
    const ISOLATION: Isolation = Isolation::Serializable;
    const CREATES: bool = true;
    const MAX_INSTANCES: Option<u64> = Some(MAX_COUNTERS);
    type State = Phase;
    type Vars = Vars;
    type Event = In;
    type Emitted = Out;
    type Key = String;
    type Record = Rec;

    fn initial(_: &String) -> (Phase, Vars) {
        (Phase::Open, Vars { n: 0, at: 0 })
    }
    fn from_record(r: &Rec) -> Result<(Phase, Vars), RecordError> {
        from_rec(r)
    }
    fn to_record(k: &String, s: &Phase, v: &Vars, _: Option<&Rec>) -> Result<Rec, RecordError> {
        Ok(to_rec(k, s, v))
    }
    fn is_elapsed(e: &In) -> bool {
        matches!(e, In::Settled {})
    }
    fn validate_record(r: &Rec) -> Result<(), String> {
        validate(r)
    }
    fn step(s: &Phase, v: &Vars, e: &In, now: Time) -> Result<Outcome<Counter>, StepError> {
        match e {
            In::Ping { .. } => Ok(Outcome::to(*s, v.clone()).emit_after(
                "pager",
                Delivery::AtMostOnce,
                Out::Pinged {},
                Duration::from_millis(10),
            )),
            In::Bump { key } => Ok(Outcome::to(
                *s,
                Vars {
                    n: v.n + 1,
                    at: now.0,
                },
            )
            .emit_after(
                "outside",
                Delivery::AtLeastOnce,
                Out::Bumped { key: key.clone() },
                Duration::from_millis(10),
            )),
            In::Both { .. } => Err(StepError::Rejected("the counter refuses both".into())),
            _ => Ok(Outcome::ignore(*s, v.clone())),
        }
    }
}

struct Watcher;

impl Machine for Watcher {
    const NAME: &'static str = "watcher";
    const RECORD: &'static str = "watchers";
    const ISOLATION: Isolation = Isolation::Serializable;
    const CREATES: bool = true;
    type State = Phase;
    type Vars = Vars;
    type Event = In;
    type Emitted = Out;
    type Key = String;
    type Record = Rec;

    fn initial(_: &String) -> (Phase, Vars) {
        (Phase::Open, Vars { n: 0, at: 0 })
    }
    fn from_record(r: &Rec) -> Result<(Phase, Vars), RecordError> {
        from_rec(r)
    }
    fn to_record(k: &String, s: &Phase, v: &Vars, _: Option<&Rec>) -> Result<Rec, RecordError> {
        Ok(to_rec(k, s, v))
    }
    fn is_elapsed(e: &In) -> bool {
        matches!(e, In::Settled {})
    }
    fn validate_record(r: &Rec) -> Result<(), String> {
        validate(r)
    }
    fn step(s: &Phase, v: &Vars, e: &In, now: Time) -> Result<Outcome<Watcher>, StepError> {
        match (s, e) {
            (Phase::Open, In::Bumped { .. } | In::Both { .. }) => Ok(Outcome::to(
                Phase::Open,
                Vars {
                    n: v.n + 1,
                    at: now.0,
                },
            )
            .set_timer("settle", Duration::from_millis(5), In::Settled {})),
            (Phase::Open, In::Settled {}) => {
                Ok(Outcome::to(Phase::Settled, Vars { n: v.n, at: now.0 }))
            }
            _ => Ok(Outcome::ignore(*s, v.clone())),
        }
    }
}

const ROUTES: strata_runtime::RouteTable = &[
    ("bump", &[Routing::by_key("counter", "key")]),
    ("noise", &[Routing::every("counter")]),
    (
        "both",
        &[
            Routing::by_key("watcher", "key"),
            Routing::by_key("counter", "key"),
        ],
    ),
    ("bumped", &[Routing::by_key("watcher", "key")]),
    ("ping", &[Routing::by_key("counter", "key")]),
];

/// The receivers outside the machines: remembers who was dispatched to.
#[derive(Default)]
struct Outside {
    receivers: Mutex<Vec<String>>,
}

impl Outside {
    fn reached(&self, receiver: &str) -> usize {
        let seen = self.receivers.lock().unwrap();
        seen.iter().filter(|r| *r == receiver).count()
    }
}

impl Dispatcher<Value> for Outside {
    async fn dispatch(&self, envelope: &Envelope<Value>) -> Result<(), DispatchError> {
        let mut seen = self.receivers.lock().unwrap();
        seen.push(envelope.receiver.as_str().to_string());
        Ok(())
    }
}

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

fn system_on<S: MachineStore>(
    store: Arc<S>,
    clock: &TestClock,
    outside: &Arc<Outside>,
    telemetry: Arc<dyn Telemetry>,
) -> System<S, Outside> {
    let router = Router::new(
        vec![Member::of::<Counter>(), Member::of::<Watcher>()],
        ROUTES,
    );
    System::new(store, router, outside.clone(), Arc::new(clock.clone()))
        .unwrap()
        .with_telemetry(telemetry)
}

/// A system host on `store` dispatching to `dispatcher`.
fn system_to<S: MachineStore, D: Dispatcher<Value>>(
    store: Arc<S>,
    clock: &TestClock,
    dispatcher: Arc<D>,
) -> System<S, D> {
    let router = Router::new(
        vec![Member::of::<Counter>(), Member::of::<Watcher>()],
        ROUTES,
    );
    System::new(store, router, dispatcher, Arc::new(clock.clone())).unwrap()
}

/// What reached the receivers outside the machines: receiver, outbox id and event.
#[derive(Default)]
struct Received {
    seen: Mutex<Vec<(String, Option<u64>, Value)>>,
}

impl Received {
    fn all(&self) -> Vec<(String, Option<u64>, Value)> {
        self.seen.lock().unwrap().clone()
    }
}

impl Dispatcher<Value> for Received {
    async fn dispatch(&self, envelope: &Envelope<Value>) -> Result<(), DispatchError> {
        self.seen.lock().unwrap().push((
            envelope.receiver.as_str().to_string(),
            envelope.id,
            envelope.event.clone(),
        ));
        Ok(())
    }
}

/// A process that stops during a dispatch: `Hands` hands the envelope over (the receiver has it)
/// and the process stops before the host records the dispatch; `Nothing` stops before anything
/// leaves. Either way the dispatch never returns; the test drops the host's future, as a stop does.
enum Stop {
    Hands(Received),
    Nothing,
}

impl Dispatcher<Value> for Stop {
    async fn dispatch(&self, envelope: &Envelope<Value>) -> Result<(), DispatchError> {
        if let Stop::Hands(received) = self {
            received.dispatch(envelope).await?;
        }
        std::future::pending::<()>().await;
        Ok(())
    }
}

/// Fails the first `failures` dispatches, then delivers.
struct FailFirst {
    failures: Mutex<u32>,
    received: Received,
}

impl Dispatcher<Value> for FailFirst {
    async fn dispatch(&self, envelope: &Envelope<Value>) -> Result<(), DispatchError> {
        {
            let mut left = self.failures.lock().unwrap();
            if *left > 0 {
                *left -= 1;
                return Err(DispatchError("the receiver is unreachable".to_string()));
            }
        }
        self.received.dispatch(envelope).await
    }
}

/// Drain the outbox of a host that stops during a dispatch: the drain does not finish, and its
/// future is dropped.
async fn drain_and_stop<S: MachineStore>(host: &System<S, Stop>) {
    let stopped = tokio::time::timeout(Duration::from_millis(500), host.drain_outbox()).await;
    assert!(stopped.is_err(), "the stopping dispatch must not return");
}

fn system<S: MachineStore>(store: Arc<S>, clock: &TestClock) -> System<S, Outside> {
    system_on(
        store,
        clock,
        &Arc::new(Outside::default()),
        Arc::new(NoTelemetry),
    )
}

fn key(k: &str) -> String {
    serde_json::to_string(k).unwrap()
}

async fn view<S: MachineStore>(s: &System<S, Outside>, machine: &str, k: &str) -> Value {
    let (state, vars) = s.instance(machine, &key(k)).await.unwrap().unwrap();
    json!({ "state": state, "vars": vars })
}

/// Entries are delivered in due order at their own time, however late the host steps them;
/// the emit's delay and the timer count from the step's time.
pub async fn delivers_at_deadlines<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(0));
    let host = system(open(), &clock);
    host.submit_at(json!({"event": "bump", "payload": {"key": "a"}}), Time(0))
        .await
        .unwrap();
    host.submit_at(json!({"event": "bump", "payload": {"key": "b"}}), Time(12))
        .await
        .unwrap();

    // Everything is late: the clock is far ahead of every deadline.
    clock.set(Time(1_000));
    let mut order = Vec::new();
    while let Some(step) = host.step().await.unwrap() {
        assert!(step.failed.is_none(), "{step:?}");
        let event: Value = serde_json::from_str(&step.entry.event).unwrap();
        order.push((
            step.entry.due.0,
            event["event"].as_str().unwrap().to_string(),
        ));
    }
    assert_eq!(
        order,
        [
            (0, "bump".to_string()),
            (10, "bumped".to_string()),
            (12, "bump".to_string()),
            (15, "settled".to_string()),
            (22, "bumped".to_string()),
            (27, "settled".to_string()),
        ]
    );
    assert_eq!(
        view(&host, "counter", "a").await,
        json!({"state": "open", "vars": {"n": 1, "at": 0}})
    );
    assert_eq!(
        view(&host, "watcher", "a").await,
        json!({"state": "settled", "vars": {"n": 1, "at": 15}})
    );
    assert_eq!(
        view(&host, "watcher", "b").await,
        json!({"state": "settled", "vars": {"n": 1, "at": 27}})
    );
    assert!(host.queued(10).await.unwrap().is_empty());
    // Both bumps also left for their receiver outside the machines.
    assert_eq!(host.drain_outbox().await.unwrap().delivered, 2);
}

/// An event no transition takes writes nothing: no record version moves.
pub async fn ignores_without_writing<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(0));
    let store = open();
    let host = system(store.clone(), &clock);
    host.submit(json!({"event": "bump", "payload": {"key": "a"}}))
        .await
        .unwrap();
    host.step().await.unwrap().unwrap();
    let version = || {
        store
            .transact(Isolation::Serializable, |tx| {
                tx.load("counters", &key("a")).map(|r| r.map(|r| r.version))
            })
            .unwrap()
    };
    assert_eq!(version(), Some(1));
    host.submit(json!({"event": "noise", "payload": {}}))
        .await
        .unwrap();
    let step = host.step().await.unwrap().unwrap();
    assert_eq!(step.reached.len(), 1);
    assert_eq!(
        step.reached[0].delivered,
        strata_runtime::Delivered::Ignored
    );
    assert_eq!(version(), Some(1));
}

/// When one receiver of an entry fails, no receiver's effects remain; the entry is a dead
/// letter and the queue goes on.
pub async fn a_failed_step_commits_nothing<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(0));
    let host = system(open(), &clock);
    host.submit(json!({"event": "both", "payload": {"key": "a"}}))
        .await
        .unwrap();
    host.submit(json!({"event": "bump", "payload": {"key": "a"}}))
        .await
        .unwrap();
    let failed = host.step().await.unwrap().unwrap();
    assert!(failed.failed.as_deref().unwrap().contains("refuses both"));
    // The watcher stepped first, but nothing of it was kept: no record, no timer.
    assert!(host.keys("watcher").await.unwrap().is_empty());
    let dead = host.dead_letters().await.unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].entry.seq, failed.entry.seq);
    // The next entry is delivered as usual.
    let next = host.step().await.unwrap().unwrap();
    assert!(next.failed.is_none());
    assert_eq!(
        view(&host, "counter", "a").await,
        json!({"state": "open", "vars": {"n": 1, "at": 0}})
    );
    let queued = host.queued(10).await.unwrap();
    assert_eq!(queued.len(), 1, "only the bump's delayed emit: {queued:?}");
}

/// An entry of the queue that cannot be read is set aside as a dead letter and reported at level
/// error, and the entries behind it are still delivered and the outbox still drained.
pub async fn quarantines_an_unreadable_entry<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(0));
    let store = open();
    let errors = Arc::new(Errors::default());
    let host = system_on(
        store.clone(),
        &clock,
        &Arc::new(Outside::default()),
        errors.clone(),
    );
    let push = |event: &str| {
        store
            .transact(Isolation::Serializable, |tx| {
                tx.push(&NewEntry {
                    due: Time(0),
                    event: event.to_string(),
                    target: None,
                    timer: None,
                    source: "input".to_string(),
                    source_key: String::new(),
                })
            })
            .unwrap()
    };
    // Not JSON; then JSON of an event shape the machines no longer read; then a good entry.
    let broken = push("{ not json");
    let reshaped = push(r#"{"event":"bump","payload":{"key":7}}"#);
    host.submit(json!({"event": "bump", "payload": {"key": "a"}}))
        .await
        .unwrap();
    clock.set(Time(1_000));
    let tick = host.tick().await.unwrap();
    assert_eq!(tick.dead, 2, "{tick:?}");
    assert!(tick.steps >= 1, "the good entry was delivered: {tick:?}");
    let dead = host.dead_letters().await.unwrap();
    let seqs: Vec<u64> = dead.iter().map(|d| d.entry.seq).collect();
    assert_eq!(seqs, [broken, reshaped]);
    assert!(dead.iter().all(|d| !d.error.is_empty()));
    assert_eq!(
        errors
            .names
            .lock()
            .unwrap()
            .iter()
            .filter(|n| *n == "queue.dead_lettered")
            .count(),
        2
    );
    assert_eq!(
        view(&host, "counter", "a").await,
        json!({"state": "open", "vars": {"n": 1, "at": 0}})
    );
    // The outbox is still drained by the same tick: the good entry's delayed emit left for its
    // receiver.
    assert_eq!(tick.dispatched.delivered, 1, "{tick:?}");
}

/// Two hosts on one store: an entry is stepped once, and an at-most-once message is dispatched
/// once, by the host that removes it.
pub async fn two_hosts_dispatch_once<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(0));
    let outside = Arc::new(Outside::default());
    let one = system_on(open(), &clock, &outside, Arc::new(NoTelemetry));
    let two = system_on(open(), &clock, &outside, Arc::new(NoTelemetry));
    one.submit(json!({"event": "bump", "payload": {"key": "a"}}))
        .await
        .unwrap();
    one.submit(json!({"event": "ping", "payload": {"key": "a"}}))
        .await
        .unwrap();
    // Both race for the head; whoever loses finds it taken and reaches nothing.
    let (first, second) = tokio::join!(one.step(), two.step());
    first.unwrap();
    second.unwrap();
    let (first, second) = tokio::join!(one.step(), two.step());
    first.unwrap();
    second.unwrap();
    assert_eq!(
        view(&one, "counter", "a").await,
        json!({"state": "open", "vars": {"n": 1, "at": 0}}),
        "the bump counted once"
    );
    // A lost race leaves the entry for the winner: nothing was dead-lettered.
    assert!(one.dead_letters().await.unwrap().is_empty());

    // The ping's at-most-once message to the pager, due 10 ms later, is claimed by one host.
    clock.set(Time(10));
    let (first, second) = tokio::join!(one.drain_outbox(), two.drain_outbox());
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_eq!(outside.reached("pager"), 1, "{first:?} {second:?}");
}

/// An event that stands for elapsed time cannot be submitted from outside; the timer that sets it
/// still delivers it.
pub async fn refuses_an_elapsed_event_from_outside<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(0));
    let host = system(open(), &clock);
    let refused = host
        .submit(json!({"event": "settled", "payload": {}}))
        .await;
    match refused {
        Err(HostError::Elapsed { event, .. }) => assert_eq!(event, "settled"),
        other => panic!("expected the elapsed event to be refused, got {other:?}"),
    }
    assert!(host
        .submit_at(json!({"event": "settled", "payload": {}}), Time(5))
        .await
        .is_err());
    assert!(host.queued(10).await.unwrap().is_empty());

    // The same event, delivered by the timer the watcher sets, still works.
    host.submit(json!({"event": "bumped", "payload": {"key": "w"}}))
        .await
        .unwrap();
    clock.set(Time(1_000));
    while let Some(step) = host.step().await.unwrap() {
        assert!(step.failed.is_none(), "{step:?}");
    }
    assert_eq!(view(&host, "watcher", "w").await["state"], json!("settled"));
}

/// A record that breaks a constraint is refused: the step fails and nothing of it is committed.
pub async fn refuses_a_record_that_breaks_a_constraint<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(0));
    let store = open();
    let host = system(store.clone(), &clock);
    host.submit(json!({"event": "bump", "payload": {"key": "much-too-long"}}))
        .await
        .unwrap();
    let step = host.step().await.unwrap().unwrap();
    assert!(
        step.failed.as_deref().unwrap().contains("longer than"),
        "{step:?}"
    );
    assert!(host.keys("counter").await.unwrap().is_empty());
    assert!(host.queued(10).await.unwrap().is_empty(), "no delayed emit");
    assert_eq!(store.pending().unwrap(), 0, "no outbox message");
    assert_eq!(host.dead_letters().await.unwrap().len(), 1);
    // A key within the limit goes through.
    host.submit(json!({"event": "bump", "payload": {"key": "short"}}))
        .await
        .unwrap();
    assert!(host.step().await.unwrap().unwrap().failed.is_none());
    assert_eq!(host.keys("counter").await.unwrap().len(), 1);
}

/// An at-least-once message whose sender stops after handing it over and before removing it from
/// the outbox is dispatched again, with the same id, by the host that comes back: the duplicate
/// delivery Exegra's `duplicate-event` models.
pub async fn redelivers_at_least_once_after_a_stop_between_dispatch_and_removal<S: MachineStore>(
    open: impl Fn() -> Arc<S>,
) {
    let clock = TestClock::at(Time(0));
    let stop = Arc::new(Stop::Hands(Received::default()));
    let first = system_to(open(), &clock, stop.clone());
    first
        .submit(json!({"event": "bump", "payload": {"key": "a"}}))
        .await
        .unwrap();
    first.step().await.unwrap().unwrap();
    // The bump's at-least-once message to the receiver outside is due 10 ms later.
    clock.set(Time(10));
    drain_and_stop(&first).await;
    let Stop::Hands(handed) = &*stop else {
        unreachable!()
    };
    let handed = handed.all();
    assert_eq!(handed.len(), 1, "{handed:?}");
    let (receiver, id, event) = handed[0].clone();
    assert_eq!(receiver, "outside");
    assert!(
        id.is_some(),
        "an at-least-once envelope carries its outbox id"
    );

    // The host comes back on the same store and drains again.
    let received = Arc::new(Received::default());
    let again = system_to(open(), &clock, received.clone());
    let report = again.drain_outbox().await.unwrap();
    assert_eq!(report.delivered, 1, "{report:?}");
    assert_eq!(
        received.all(),
        [(receiver, id, event)],
        "the same message, the same id"
    );
    // Delivered and removed: nothing is left to dispatch.
    assert_eq!(again.drain_outbox().await.unwrap().delivered, 0);
}

/// An at-most-once message is lost when its sender stops after claiming it and before it is
/// dispatched: the host that comes back dispatches nothing. That is Exegra's `lose-event`.
pub async fn loses_at_most_once_on_a_stop_between_commit_and_dispatch<S: MachineStore>(
    open: impl Fn() -> Arc<S>,
) {
    let clock = TestClock::at(Time(0));
    let store = open();
    let first = system_to(store.clone(), &clock, Arc::new(Stop::Nothing));
    first
        .submit(json!({"event": "ping", "payload": {"key": "a"}}))
        .await
        .unwrap();
    first.step().await.unwrap().unwrap();
    // Committed: the ping's at-most-once message to the pager is in the outbox, due at 10 ms.
    assert_eq!(store.pending().unwrap(), 1);
    clock.set(Time(10));
    drain_and_stop(&first).await;

    let received = Arc::new(Received::default());
    let again = system_to(open(), &clock, received.clone());
    let report = again.drain_outbox().await.unwrap();
    assert_eq!(report.delivered, 0, "{report:?}");
    assert!(received.all().is_empty(), "the message is lost");
    assert_eq!(store.pending().unwrap(), 0, "and gone from the outbox");
}

/// A failed at-least-once dispatch is retried after the backoff (1 s, then 2 s more), not before:
/// the delays Exegra's `retry-event` gives.
pub async fn retries_at_least_once_with_backoff<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(0));
    let dispatcher = Arc::new(FailFirst {
        failures: Mutex::new(2),
        received: Received::default(),
    });
    let host = system_to(open(), &clock, dispatcher.clone());
    host.submit(json!({"event": "bump", "payload": {"key": "a"}}))
        .await
        .unwrap();
    host.step().await.unwrap().unwrap();
    clock.set(Time(10));
    assert_eq!(host.drain_outbox().await.unwrap().deferred, 1);
    clock.set(Time(10 + 999));
    assert_eq!(
        host.drain_outbox().await.unwrap(),
        Default::default(),
        "not yet due"
    );
    clock.set(Time(10 + 1_000));
    assert_eq!(host.drain_outbox().await.unwrap().deferred, 1);
    clock.set(Time(10 + 1_000 + 1_999));
    assert_eq!(
        host.drain_outbox().await.unwrap(),
        Default::default(),
        "not yet due"
    );
    clock.set(Time(10 + 3_000));
    assert_eq!(host.drain_outbox().await.unwrap().delivered, 1);
    assert_eq!(dispatcher.received.all().len(), 1);
}

/// A creating machine has at most `instances.max` instances: an entry that would create one more
/// fails its step (a dead letter, nothing committed), and its existing instances still step.
pub async fn refuses_an_instance_beyond_max<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(0));
    let host = system(open(), &clock);
    for k in ["a", "b", "c", "d"] {
        host.submit(json!({"event": "bump", "payload": {"key": k}}))
            .await
            .unwrap();
    }
    let mut failed = Vec::new();
    for _ in 0..4 {
        let step = host.step().await.unwrap().unwrap();
        failed.push(step.failed);
    }
    assert!(failed[..3].iter().all(Option::is_none), "{failed:?}");
    assert!(
        failed[3].as_deref().unwrap().contains("instances.max"),
        "{failed:?}"
    );
    assert_eq!(
        host.keys("counter").await.unwrap().len(),
        MAX_COUNTERS as usize
    );
    assert_eq!(host.dead_letters().await.unwrap().len(), 1);
    // An existing instance is not refused.
    host.submit(json!({"event": "bump", "payload": {"key": "a"}}))
        .await
        .unwrap();
    while let Some(step) = host.step().await.unwrap() {
        if step.entry.source == "input" {
            assert!(step.failed.is_none(), "{step:?}");
        }
    }
    assert_eq!(view(&host, "counter", "a").await["vars"]["n"], json!(2));
}

/// A machine with no key that creates its own record: Exegra starts its one instance at the
/// beginning, so the host creates it, under `SINGLETON_KEY`, when it is built.
struct Solo;

impl Machine for Solo {
    const NAME: &'static str = "solo";
    const RECORD: &'static str = "solos";
    const ISOLATION: Isolation = Isolation::Serializable;
    const CREATES: bool = true;
    const SINGLETON_KEY: Option<&'static str> = Some("\"solo\"");
    type State = Phase;
    type Vars = Vars;
    type Event = In;
    type Emitted = Out;
    type Key = String;
    type Record = Rec;

    fn initial(_: &String) -> (Phase, Vars) {
        (Phase::Open, Vars { n: 0, at: 0 })
    }
    fn from_record(r: &Rec) -> Result<(Phase, Vars), RecordError> {
        from_rec(r)
    }
    fn to_record(k: &String, s: &Phase, v: &Vars, _: Option<&Rec>) -> Result<Rec, RecordError> {
        Ok(to_rec(k, s, v))
    }
    fn step(s: &Phase, v: &Vars, e: &In, now: Time) -> Result<Outcome<Solo>, StepError> {
        match e {
            In::Noise {} => Ok(Outcome::to(
                *s,
                Vars {
                    n: v.n + 1,
                    at: now.0,
                },
            )
            .emit("outside", Delivery::AtLeastOnce, Out::Pinged {})),
            _ => Ok(Outcome::ignore(*s, v.clone())),
        }
    }
}

/// A keyed machine that ends: `bumped` settles it, and a settled instance is finished.
struct Mortal;

impl Machine for Mortal {
    const NAME: &'static str = "mortal";
    const RECORD: &'static str = "mortals";
    const ISOLATION: Isolation = Isolation::Serializable;
    const CREATES: bool = true;
    type State = Phase;
    type Vars = Vars;
    type Event = In;
    type Emitted = Out;
    type Key = String;
    type Record = Rec;

    fn initial(_: &String) -> (Phase, Vars) {
        (Phase::Open, Vars { n: 0, at: 0 })
    }
    fn from_record(r: &Rec) -> Result<(Phase, Vars), RecordError> {
        from_rec(r)
    }
    fn to_record(k: &String, s: &Phase, v: &Vars, _: Option<&Rec>) -> Result<Rec, RecordError> {
        Ok(to_rec(k, s, v))
    }
    fn is_terminal(s: &Phase) -> bool {
        *s == Phase::Settled
    }
    fn step(s: &Phase, v: &Vars, e: &In, now: Time) -> Result<Outcome<Mortal>, StepError> {
        match (s, e) {
            (_, In::Both { .. }) => Err(StepError::Rejected("the mortal refuses both".into())),
            (Phase::Open, In::Bump { .. }) => Ok(Outcome::to(
                Phase::Open,
                Vars {
                    n: v.n + 1,
                    at: now.0,
                },
            )),
            (Phase::Open, In::Bumped { .. }) => {
                Ok(Outcome::to(Phase::Settled, Vars { n: v.n, at: now.0 }))
            }
            _ => Ok(Outcome::ignore(*s, v.clone())),
        }
    }
}

const LIFE_ROUTES: strata_runtime::RouteTable = &[
    ("bump", &[Routing::by_key("mortal", "key")]),
    ("bumped", &[Routing::by_key("mortal", "key")]),
    ("both", &[Routing::by_key("mortal", "key")]),
    ("noise", &[Routing::every("solo")]),
];

fn life_system<S: MachineStore, D: Dispatcher<Value>>(
    store: Arc<S>,
    clock: &TestClock,
    dispatcher: Arc<D>,
) -> System<S, D> {
    let router = Router::new(
        vec![Member::of::<Mortal>(), Member::of::<Solo>()],
        LIFE_ROUTES,
    );
    System::new(store, router, dispatcher, Arc::new(clock.clone())).unwrap()
}

fn noise() -> Value {
    json!({"event": "noise", "payload": {}})
}

/// Fails every dispatch.
struct AlwaysFails;

impl Dispatcher<Value> for AlwaysFails {
    async fn dispatch(&self, _: &Envelope<Value>) -> Result<(), DispatchError> {
        Err(DispatchError("the receiver is down".to_string()))
    }
}

/// Submit `events` at time `at` and step until nothing is due; every step must succeed.
async fn run_at<S: MachineStore>(
    host: &System<S, Outside>,
    clock: &TestClock,
    at: i64,
    events: &[Value],
) {
    clock.set(Time(at));
    for event in events {
        host.submit(event.clone()).await.unwrap();
    }
    while let Some(step) = host.step().await.unwrap() {
        assert!(step.failed.is_none(), "{step:?}");
    }
}

/// An unkeyed machine that creates its records has its one instance from the start: the first
/// event reaches it with nothing set up, by a route to every instance.
pub async fn a_singleton_takes_its_first_event_without_seeding<S: MachineStore>(
    open: impl Fn() -> Arc<S>,
) {
    let clock = TestClock::at(Time(0));
    let host = life_system(open(), &clock, Arc::new(Outside::default()));
    assert_eq!(host.keys("solo").await.unwrap(), vec![key("solo")]);
    assert_eq!(
        view(&host, "solo", "solo").await,
        json!({"state": "open", "vars": {"n": 0, "at": 0}})
    );
    host.submit(noise()).await.unwrap();
    let step = host.step().await.unwrap().unwrap();
    assert!(step.failed.is_none(), "{step:?}");
    let reached: Vec<_> = step
        .reached
        .iter()
        .map(|r| (r.machine, r.delivered))
        .collect();
    assert_eq!(reached, [("solo", Delivered::Fired)]);
    assert_eq!(
        view(&host, "solo", "solo").await,
        json!({"state": "open", "vars": {"n": 1, "at": 0}})
    );
    assert_eq!(host.keys("solo").await.unwrap().len(), 1);
}

/// Two systems built over one store share the one instance: the second finds the record, creates
/// nothing and does not start it over.
pub async fn two_systems_over_one_store_make_one_singleton<S: MachineStore>(
    open: impl Fn() -> Arc<S>,
) {
    let clock = TestClock::at(Time(0));
    let first = life_system(open(), &clock, Arc::new(Outside::default()));
    run_at(&first, &clock, 0, &[noise()]).await;
    let second = life_system(open(), &clock, Arc::new(Outside::default()));
    assert_eq!(second.keys("solo").await.unwrap(), vec![key("solo")]);
    assert_eq!(
        view(&second, "solo", "solo").await,
        json!({"state": "open", "vars": {"n": 1, "at": 0}}),
        "the second system must not start the instance over"
    );
    run_at(&second, &clock, 0, &[noise()]).await;
    assert_eq!(view(&first, "solo", "solo").await["vars"]["n"], json!(2));
    assert_eq!(first.keys("solo").await.unwrap().len(), 1);
}

/// A message whose dispatch fails on every attempt is attempted `max_attempts` times, then moved
/// to the dead letters with its receiver and the last error, and is not dispatched again.
pub async fn a_message_that_always_fails_is_dead_lettered_after_the_cap<S: MachineStore>(
    open: impl Fn() -> Arc<S>,
) {
    let clock = TestClock::at(Time(0));
    let host = life_system(open(), &clock, Arc::new(AlwaysFails));
    host.submit(noise()).await.unwrap();
    host.step().await.unwrap().unwrap();
    let cap = RetryPolicy::default().max_attempts;
    for attempt in 1..=cap {
        // Past the longest backoff, so the message is due again.
        clock.set(Time(i64::from(attempt) * 1_000_000));
        let report = host.drain_outbox().await.unwrap();
        let (deferred, dead) = if attempt < cap { (1, 0) } else { (0, 1) };
        assert_eq!(
            (report.deferred, report.dead),
            (deferred, dead),
            "attempt {attempt}: {report:?}"
        );
    }
    let dead = host.dead_letters().await.unwrap();
    assert_eq!(dead.len(), 1, "{dead:?}");
    let error = &dead[0].error;
    assert!(error.contains(&format!("{cap} attempts")), "{error}");
    assert!(error.contains("the receiver is down"), "{error}");
    assert!(error.contains("outside"), "{error}");
    assert_eq!(dead[0].entry.source, "solo");
    clock.set(Time(100_000_000));
    assert_eq!(
        host.drain_outbox().await.unwrap(),
        Default::default(),
        "a dead-lettered message is not dispatched again"
    );
    let held = host.diagnostics().await.unwrap();
    assert_eq!((held.outbox, held.dead_letters), (0, 1));
}

fn short_retention() -> Retention {
    Retention {
        finished_records: Duration::from_millis(1_000),
        dead_letters: Duration::from_millis(1_000),
    }
}

/// The purge removes the dead letters that are past retention and keeps the younger ones.
pub async fn purges_dead_letters_past_retention<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(0));
    let host = life_system(open(), &clock, Arc::new(Outside::default()));
    // A refused step is a dead letter set aside at the clock's time.
    for (at, k) in [(0, "x"), (900, "y")] {
        clock.set(Time(at));
        host.submit(json!({"event": "both", "payload": {"key": k}}))
            .await
            .unwrap();
        let step = host.step().await.unwrap().unwrap();
        assert!(step.failed.is_some(), "{step:?}");
    }
    assert_eq!(host.dead_letters().await.unwrap().len(), 2);

    clock.set(Time(1_500));
    let report = host.purge(short_retention()).await.unwrap();
    assert_eq!(
        report,
        PurgeReport {
            records: 0,
            dead_letters: 1
        }
    );
    let left = host.dead_letters().await.unwrap();
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(left[0].entry.event.contains("\"y\""), "{left:?}");
    assert_eq!(host.diagnostics().await.unwrap().dead_letters, 1);
    assert_eq!(
        host.purge(short_retention()).await.unwrap(),
        PurgeReport::default(),
        "nothing else is past retention"
    );

    clock.set(Time(2_000));
    assert_eq!(host.purge(short_retention()).await.unwrap().dead_letters, 1);
    assert!(host.dead_letters().await.unwrap().is_empty());
}

/// A write that sets a dead letter aside deletes, in its own transaction, the dead letters older
/// than the retention bound (ninety days): the older one is gone after that write, the younger one
/// and the new one are kept, with no call of the purge.
#[allow(dead_code)]
pub async fn a_write_deletes_the_dead_letters_past_the_bound<S: MachineStore>(
    open: impl Fn() -> Arc<S>,
) {
    let bound = i64::try_from(strata_runtime::RETENTION.as_millis()).unwrap();
    let clock = TestClock::at(Time(0));
    let host = life_system(open(), &clock, Arc::new(Outside::default()));
    let refuse = |k: &'static str| json!({"event": "both", "payload": {"key": k}});
    // Two dead letters, set aside at 0 and at half the bound.
    for (at, k) in [(0, "old"), (bound / 2, "young")] {
        clock.set(Time(at));
        host.submit(refuse(k)).await.unwrap();
        assert!(host.step().await.unwrap().unwrap().failed.is_some());
    }
    assert_eq!(host.dead_letters().await.unwrap().len(), 2);
    // The next dead letter is written just past the bound of the first and not of the second.
    clock.set(Time(bound + 1_000));
    host.submit(refuse("new")).await.unwrap();
    assert!(host.step().await.unwrap().unwrap().failed.is_some());
    let kept = host.dead_letters().await.unwrap();
    let events: Vec<&str> = kept.iter().map(|d| d.entry.event.as_str()).collect();
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(!events.iter().any(|e| e.contains("\"old\"")), "{events:?}");
    assert!(events.iter().any(|e| e.contains("\"young\"")), "{events:?}");
    assert!(events.iter().any(|e| e.contains("\"new\"")), "{events:?}");
}

/// The purge removes the record of a finished instance once its retention has passed, and keeps
/// an instance that is not finished, a finished one that is younger, and the singleton.
///
/// Not run on the storage-api adapter: it keeps the save time its own clock gives, not the
/// step's time.
#[allow(dead_code)]
pub async fn purges_finished_records_past_retention<S: MachineStore>(open: impl Fn() -> Arc<S>) {
    let clock = TestClock::at(Time(0));
    let host = life_system(open(), &clock, Arc::new(Outside::default()));
    let bump = |k: &str| json!({"event": "bump", "payload": {"key": k}});
    let bumped = |k: &str| json!({"event": "bumped", "payload": {"key": k}});
    run_at(&host, &clock, 0, &[bump("a"), bumped("a"), bump("b")]).await;
    run_at(&host, &clock, 900, &[bump("c"), bumped("c")]).await;
    assert_eq!(
        host.keys("mortal").await.unwrap(),
        vec![key("a"), key("b"), key("c")]
    );

    clock.set(Time(1_500));
    let report = host.purge(short_retention()).await.unwrap();
    assert_eq!(
        report,
        PurgeReport {
            records: 1,
            dead_letters: 0
        },
        "only a, finished at 0, is past a retention of 1000"
    );
    assert_eq!(host.keys("mortal").await.unwrap(), vec![key("b"), key("c")]);
    assert_eq!(
        host.purge(short_retention()).await.unwrap(),
        PurgeReport::default()
    );

    clock.set(Time(2_500));
    assert_eq!(host.purge(short_retention()).await.unwrap().records, 1);
    assert_eq!(host.keys("mortal").await.unwrap(), vec![key("b")]);
    assert_eq!(host.keys("solo").await.unwrap(), vec![key("solo")]);
    clock.set(Time(1_000_000_000));
    assert_eq!(
        host.purge(short_retention()).await.unwrap(),
        PurgeReport::default()
    );
    assert_eq!(host.keys("mortal").await.unwrap(), vec![key("b")]);
}
