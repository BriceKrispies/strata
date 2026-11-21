//! Where the backend's hand-written limits, lifetimes and throttle logic meet the generated model
//! of the specification (`strata-model`): the session lifetime defaults, the quotas, and the
//! account throttle and the session counted step for step against the generated machines
//! `guess-throttle-flow-impl` and `session-authority-flow-impl`. Each known difference is listed with its reason; a difference not
//! listed fails.

use std::time::Duration;
use strata_model::machines::guess_throttle_flow_impl::{
    Emitted, Event, GuessThrottleFlowImpl, PwAttemptPayload, State, Vars,
};
use strata_runtime::machine::TimerCommand;
use strata_runtime::{Machine, Time};

// ---------------------------------------------------------------------------------------
// Session lifetime
// ---------------------------------------------------------------------------------------

// The specification disagrees with itself about the session lifetimes, and the owner has not
// decided which text stands. The backend follows the machine text; the session trace below holds
// its defaults against the machine thresholds, so a decision for d-700 is one edit in
// DEFAULT_SESSION_*_MS and one in the machine comparison there.
//
// Decision d-700 (specifications/spec.jsonc, "Platform defaults for throttles, retries, sessions
// and recovery") says: "A session ends twelve hours after it was last used or fourteen days after
// sign-in, whichever comes first." That is idle 12 hours, absolute 14 days.
// Machine session-authority-flow-impl says: "the runtime finds it idle for 2 hours (7200000 ms;
// ...) or 12 hours (43200000 ms) after sign-in". That is idle 2 hours, absolute 12 hours.

// ---------------------------------------------------------------------------------------
// Quotas
// ---------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------
// The account throttle against the generated machine
// ---------------------------------------------------------------------------------------

/// What an attempt came to, as the machine says it (`pw-accepted` or `pw-refused`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Accepted,
    Refused,
}

#[derive(Clone, Copy, Debug)]
enum Act {
    Wrong,
    Right,
    Wait(i64),
}

/// What can be seen of a counter from outside: whether it refuses everything, and how many
/// failures it holds while open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Seen {
    closed: bool,
    failures: u64,
}

/// The generated machine, driven through its `step`.
struct Model {
    state: State,
    vars: Vars,
    /// The delay of the last lockout timer the machine set.
    lockout: Option<Duration>,
}

impl Model {
    fn new() -> Self {
        let (state, vars) = <GuessThrottleFlowImpl as Machine>::initial(&"account".to_string());
        Model {
            state,
            vars,
            lockout: None,
        }
    }

    fn step(&mut self, event: Event, now: i64) -> Vec<Emitted> {
        let outcome =
            <GuessThrottleFlowImpl as Machine>::step(&self.state, &self.vars, &event, Time(now))
                .expect("the throttle machine refused a step");
        for timer in &outcome.timers {
            if let TimerCommand::Set { after, .. } = timer {
                self.lockout = Some(*after);
            }
        }
        self.state = outcome.state;
        self.vars = outcome.vars;
        outcome.emits.into_iter().map(|emit| emit.event).collect()
    }

    fn attempt(&mut self, guess: &str, now: i64) -> Verdict {
        let emitted = self.step(
            Event::PwAttempt(PwAttemptPayload {
                guess: guess.to_string(),
                limit_key: "account".to_string(),
            }),
            now,
        );
        match emitted.as_slice() {
            [Emitted::PwAccepted(_)] => Verdict::Accepted,
            [Emitted::PwRefused(_)] => Verdict::Refused,
            other => panic!("an attempt emitted {other:?}"),
        }
    }

    fn seen(&self) -> Seen {
        Seen {
            closed: self.state == State::GtiClosed,
            failures: self.vars.failures,
        }
    }
}

/// A fixed-seed generator (xorshift), so that the traces are the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// The time the hand-written counter forgets failures after is `ThrottleLimits::window_ms`, 15
/// minutes; the machine has no clock. Traces stay inside it, and the window is its own test.
const TRACE_BUDGET_MS: i64 = 800_000;

fn trace(rng: &mut Rng) -> Vec<Act> {
    let mut acts = Vec::new();
    let mut elapsed = 0;
    while acts.len() < 60 {
        let act = match rng.next() % 20 {
            0..=13 => Act::Wrong,
            14..=15 => Act::Right,
            16 => Act::Wait(31_000),
            17 => Act::Wait(29_999),
            _ => Act::Wait(1 + (rng.next() % 2_000) as i64),
        };
        elapsed += match act {
            Act::Wait(ms) => ms,
            _ => 1,
        };
        if elapsed > TRACE_BUDGET_MS {
            break;
        }
        acts.push(act);
    }
    acts
}

// ---------------------------------------------------------------------------------------
// Error mapping (WG-4893, WG-4939)
// ---------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------
// The session against the generated machine
// ---------------------------------------------------------------------------------------

mod session_trace {
    use super::Rng;
    use strata_backend::app::{DEFAULT_SESSION_IDLE_MS, DEFAULT_SESSION_TTL_MS};
    use strata_backend::auth::{mark_revoked, session_state_at};
    use strata_kernel::testkit::{principal_id, tenant_id};
    use strata_kernel::{SessionId, Timestamp};
    use strata_model::machines::session_authority_flow_impl as sa;
    use strata_protocol::ws::SessionState;
    use strata_runtime::{Machine, Time};
    use strata_storage_api::SessionRecord;

    /// What a person or the runtime does to a session.
    #[derive(Clone, Copy, Debug)]
    enum Act {
        /// Time passes and the session is used (a request slides the idle time of a live session).
        Use(i64),
        /// Time passes and nothing touches the session.
        Wait(i64),
        /// The gateway looks the session up.
        Lookup,
        /// One of the events the machine receives about the session.
        End(Ending),
    }

    /// The machine's events that end the session, and those that concern another session (or
    /// only list this one) and leave it alone.
    #[derive(Clone, Copy, Debug)]
    enum Ending {
        Revoked,
        SignedOut,
        Recovered,
        Deactivated,
        OwnerRevokedThis,
        OwnerRevokedOther,
        PasswordChangedOther,
        PasswordChangedThis,
        AuthenticatedSameBrowser,
        AuthenticatedOtherBrowser,
        Listed,
    }

    impl Ending {
        /// The machine's event, and whether the backend ends the session on it.
        fn event(self) -> (sa::Event, bool) {
            match self {
                Ending::Revoked => (sa::Event::SrRevoked(sa::SrRevokedPayload {}), true),
                Ending::SignedOut => (sa::Event::SrSignedOut(sa::SrSignedOutPayload {}), true),
                Ending::Recovered => (sa::Event::SrRecovered(sa::SrRecoveredPayload {}), true),
                Ending::Deactivated => {
                    (sa::Event::SrDeactivated(sa::SrDeactivatedPayload {}), true)
                }
                Ending::OwnerRevokedThis => (owner("this"), true),
                Ending::OwnerRevokedOther => (owner("other"), false),
                Ending::PasswordChangedOther => (password("other"), true),
                Ending::PasswordChangedThis => (password("this"), false),
                Ending::AuthenticatedSameBrowser => (authenticated(true), true),
                Ending::AuthenticatedOtherBrowser => (authenticated(false), false),
                Ending::Listed => (sa::Event::SrList(sa::SrListPayload {}), false),
            }
        }
    }

    fn owner(target: &str) -> sa::Event {
        sa::Event::SrOwnerRevoked(sa::SrOwnerRevokedPayload {
            target: target.to_string(),
        })
    }

    fn password(initiator: &str) -> sa::Event {
        sa::Event::SrPasswordChanged(sa::SrPasswordChangedPayload {
            initiator: initiator.to_string(),
        })
    }

    fn authenticated(same_browser: bool) -> sa::Event {
        sa::Event::SrAuthenticated(sa::SrAuthenticatedPayload { same_browser })
    }

    /// The generated machine, driven through its `step`.
    struct Model {
        state: sa::State,
        vars: sa::Vars,
    }

    impl Model {
        fn new() -> Self {
            let key = strata_model::types::SessionId("s_trace".to_string());
            let (state, vars) = <sa::SessionAuthorityFlowImpl as Machine>::initial(&key);
            Model { state, vars }
        }

        /// Delivers an event and returns what the machine emitted. An event no transition takes
        /// (a guard that does not hold) changes nothing and emits nothing.
        fn step(&mut self, event: &sa::Event, now: i64) -> Vec<sa::Emitted> {
            match <sa::SessionAuthorityFlowImpl as Machine>::step(
                &self.state,
                &self.vars,
                event,
                Time(now),
            ) {
                Ok(outcome) if !outcome.ignored => {
                    self.state = outcome.state;
                    self.vars = outcome.vars;
                    outcome.emits.into_iter().map(|emit| emit.event).collect()
                }
                Ok(_) | Err(_) => Vec::new(),
            }
        }

        fn expiry(idle_ms: i64, age_ms: i64) -> sa::Event {
            sa::Event::SrExpired(sa::SrExpiredPayload {
                idle_ms: idle_ms as u64,
                age_ms: age_ms as u64,
            })
        }

        /// Whether the machine would end the session on this expiry, without delivering it.
        fn would_expire(&self, idle_ms: i64, age_ms: i64, now: i64) -> bool {
            matches!(
                <sa::SessionAuthorityFlowImpl as Machine>::step(
                    &self.state,
                    &self.vars,
                    &Model::expiry(idle_ms, age_ms),
                    Time(now)
                ),
                Ok(outcome) if !outcome.ignored && outcome.state == sa::State::SaRevokedI
            )
        }

        fn live(&self) -> bool {
            self.state == sa::State::SaLiveI
        }
    }

    fn record(created: i64) -> SessionRecord {
        SessionRecord {
            id: SessionId::new("s_trace").unwrap(),
            token_hash: "hash".to_string(),
            csrf_token: "csrf".to_string(),
            tenant: tenant_id("acme"),
            principal: principal_id("acme:person"),
            display_name: "Person".to_string(),
            created_at: Timestamp(created),
            last_seen_at: Timestamp(created),
            expires_at: Timestamp(created + DEFAULT_SESSION_TTL_MS),
            revoked_at: None,
            authenticated_at: Timestamp(created),
            network: None,
        }
    }

    fn trace(rng: &mut Rng) -> Vec<Act> {
        let mut acts = Vec::new();
        while acts.len() < 40 {
            acts.push(match rng.next() % 24 {
                0..=7 => Act::Lookup,
                8..=12 => Act::Use(1 + (rng.next() % 600_000) as i64),
                13 => Act::Wait(DEFAULT_SESSION_IDLE_MS - 1),
                14 => Act::Wait(DEFAULT_SESSION_IDLE_MS),
                15 => Act::Use(DEFAULT_SESSION_IDLE_MS - 1),
                16 => Act::Wait(DEFAULT_SESSION_TTL_MS - DEFAULT_SESSION_IDLE_MS),
                17 => Act::Use(DEFAULT_SESSION_TTL_MS / 4),
                18 => Act::Wait(1 + (rng.next() % 3_600_000) as i64),
                _ => Act::End(match rng.next() % 11 {
                    0 => Ending::Revoked,
                    1 => Ending::SignedOut,
                    2 => Ending::Recovered,
                    3 => Ending::Deactivated,
                    4 => Ending::OwnerRevokedThis,
                    5 => Ending::OwnerRevokedOther,
                    6 => Ending::PasswordChangedOther,
                    7 => Ending::PasswordChangedThis,
                    8 => Ending::AuthenticatedSameBrowser,
                    9 => Ending::AuthenticatedOtherBrowser,
                    _ => Ending::Listed,
                }),
            });
        }
        acts
    }

    /// A session used often enough never to be idle, until its absolute lifetime ends it.
    fn steady() -> Vec<Act> {
        let mut acts = Vec::new();
        for _ in 0..10 {
            acts.push(Act::Use(DEFAULT_SESSION_IDLE_MS - 2));
            acts.push(Act::Lookup);
        }
        acts
    }

    /// The session the backend holds and the machine the specification generates agree at every
    /// step of fixed-seed traces: whether a lookup finds the session valid, whether it has ended
    /// after every request and every event about it, and at which moment the idle or the absolute
    /// lifetime ends it. The machine's two thresholds (2 hours idle, 12 hours absolute) are held
    /// against the backend's defaults, so a change to either side alone fails here.
    ///
    /// Verifies: IDN-011
    /// Verifies: IDN-013
    #[test]
    fn the_session_follows_the_generated_machine() {
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        let (mut ended_by_event, mut ended_by_idle, mut ended_by_age) = (0, 0, 0);
        let (mut valid_lookups, mut invalid_lookups) = (0, 0);
        for index in 0..400 {
            let acts = if index % 8 == 0 {
                steady()
            } else {
                trace(&mut rng)
            };
            let created = 5_000_000;
            let (mut model, mut hand) = (Model::new(), record(created));
            let mut now = created;
            for act in &acts {
                now += match act {
                    Act::Use(ms) | Act::Wait(ms) => *ms,
                    _ => 1,
                };
                let state = session_state_at(DEFAULT_SESSION_IDLE_MS, &hand, now);
                let idle = now - hand.last_seen_at.0;
                let age = now - hand.created_at.0;
                // The two sides agree on expiry before it is delivered: the machine ends a live
                // session on exactly the moments the backend calls it expired.
                if model.live() {
                    assert_eq!(
                        model.would_expire(idle, age, now),
                        state == SessionState::Expired,
                        "trace {index}, {act:?} at {now}: backend says {state:?}, the machine \
                         disagrees about expiry at idle {idle} ms, age {age} ms (trace {acts:?})"
                    );
                }
                // The runtime finds the expiry and delivers it, as the machine's description says.
                if state == SessionState::Expired && model.live() {
                    model.step(&Model::expiry(idle, age), now);
                    assert!(!model.live(), "the machine did not end an expired session");
                    if age >= DEFAULT_SESSION_TTL_MS {
                        ended_by_age += 1;
                    } else {
                        ended_by_idle += 1;
                    }
                }
                match act {
                    Act::Use(_) => {
                        if state == SessionState::Active {
                            hand.last_seen_at = Timestamp(now);
                        }
                    }
                    Act::Wait(_) => {}
                    Act::Lookup => {
                        let emitted = model.step(&sa::Event::SrLookup(sa::SrLookupPayload {}), now);
                        let answered_valid = emitted
                            .iter()
                            .any(|e| matches!(e, sa::Emitted::SrLookupAnswer(a) if a.valid));
                        let said_valid = emitted
                            .iter()
                            .any(|e| matches!(e, sa::Emitted::SrSessionValid(_)));
                        assert_eq!(
                            answered_valid, said_valid,
                            "the machine's lookup answer and its valid event differ (trace {acts:?})"
                        );
                        assert_eq!(
                            state == SessionState::Active,
                            answered_valid,
                            "trace {index}, lookup at {now}: the backend says {state:?}, the machine \
                             answers valid = {answered_valid} (trace {acts:?})"
                        );
                        if answered_valid {
                            valid_lookups += 1;
                        } else {
                            invalid_lookups += 1;
                        }
                    }
                    Act::End(ending) => {
                        let (event, ends) = ending.event();
                        let was_live = model.live();
                        let emitted = model.step(&event, now);
                        if ends && was_live {
                            assert!(
                                !model.live(),
                                "{ending:?} did not end a live session (trace {acts:?})"
                            );
                            assert!(
                                emitted
                                    .iter()
                                    .any(|e| matches!(e, sa::Emitted::SrNotice(_))),
                                "ending a live session emits the notice (trace {acts:?})"
                            );
                            ended_by_event += 1;
                        } else if !ends {
                            assert_eq!(
                                model.live(),
                                was_live,
                                "{ending:?} changed a session it does not concern (trace {acts:?})"
                            );
                        }
                        // The backend ends a session that is not ended yet (`auth::revoke`).
                        if ends {
                            mark_revoked(&mut hand, Timestamp(now));
                        }
                    }
                }
                let after = session_state_at(DEFAULT_SESSION_IDLE_MS, &hand, now);
                assert_eq!(
                    !model.live(),
                    after != SessionState::Active,
                    "trace {index}, {act:?} at {now}: the backend says {after:?}, the machine is \
                     live = {} (trace {acts:?})",
                    model.live()
                );
                assert_eq!(
                    model.vars.revoked,
                    !model.live(),
                    "the machine's revoked flag follows its state (trace {acts:?})"
                );
            }
        }
        // The traces reach each way a session ends and both answers of a lookup, so the
        // agreement is not agreement about nothing.
        assert!(
            ended_by_event > 100,
            "only {ended_by_event} ended by an event"
        );
        assert!(
            ended_by_idle > 20,
            "only {ended_by_idle} ended by idle time"
        );
        assert!(ended_by_age > 5, "only {ended_by_age} ended by age");
        assert!(valid_lookups > 100, "only {valid_lookups} valid lookups");
        assert!(
            invalid_lookups > 100,
            "only {invalid_lookups} invalid lookups"
        );
    }
}
