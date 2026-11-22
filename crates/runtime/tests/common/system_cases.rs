//! The system host on a store: one queue for hand-written machines, a singleton made once by two
//! systems over one store, a message that always fails dead-lettered after the cap, and dead
//! letters purged past retention.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use strata_runtime::{
    Delivery, DispatchError, Dispatcher, Envelope, Isolation, Machine, MachineStore, Member,
    Outcome, PurgeReport, RecordError, Retention, RetryPolicy, Router, Routing, StepError, System,
    TestClock, Time,
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

/// The receivers outside the machines: remembers who was dispatched to.
#[derive(Default)]
struct Outside {
    receivers: Mutex<Vec<String>>,
}

impl Dispatcher<Value> for Outside {
    async fn dispatch(&self, envelope: &Envelope<Value>) -> Result<(), DispatchError> {
        let mut seen = self.receivers.lock().unwrap();
        seen.push(envelope.receiver.as_str().to_string());
        Ok(())
    }
}

fn key(k: &str) -> String {
    serde_json::to_string(k).unwrap()
}

async fn view<S: MachineStore>(s: &System<S, Outside>, machine: &str, k: &str) -> Value {
    let (state, vars) = s.instance(machine, &key(k)).await.unwrap().unwrap();
    json!({ "state": state, "vars": vars })
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
