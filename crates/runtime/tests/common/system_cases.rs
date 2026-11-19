//! The system host on a store: one queue for two hand-written machines, delivered in
//! `(due, seq)` order at each entry's own time, an ignored event writing nothing, and a step that
//! fails in one receiver committing nothing in any.
//!
//! The counter counts `bump`s of its key and tells the watcher of the same key 10 ms later
//! (`bumped`, a delayed emit); the watcher counts what it hears and settles 5 ms after (a timer).
//! `both` reaches the watcher, which counts it, and the counter, which refuses it.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use strata_runtime::{
    Delivery, DispatchError, Dispatcher, Envelope, Isolation, Machine, MachineStore, Member,
    Outcome, RecordError, Router, Routing, StepError, System, TestClock, Time,
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", content = "payload")]
enum Out {
    #[serde(rename = "bumped")]
    Bumped { key: String },
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
    fn step(s: &Phase, v: &Vars, e: &In, now: Time) -> Result<Outcome<Counter>, StepError> {
        match e {
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
];

struct Outside;

impl Dispatcher<Value> for Outside {
    async fn dispatch(&self, _: &Envelope<Value>) -> Result<(), DispatchError> {
        Ok(())
    }
}

fn system<S: MachineStore>(store: Arc<S>, clock: &TestClock) -> System<S, Outside> {
    let router = Router::new(
        vec![Member::of::<Counter>(), Member::of::<Watcher>()],
        ROUTES,
    );
    System::new(store, router, Arc::new(Outside), Arc::new(clock.clone())).unwrap()
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
