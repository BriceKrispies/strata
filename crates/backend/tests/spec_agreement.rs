//! Where the backend's hand-written limits, lifetimes and throttle logic meet the generated model
//! of the specification (`strata-model`): the session lifetime defaults, the quotas, and the
//! account throttle counted step for step against the generated machine
//! `guess-throttle-flow-impl`. Each known difference is listed with its reason; a difference not
//! listed fails.

use std::time::Duration;
use strata_backend::app::{ThrottleLimits, DEFAULT_SESSION_IDLE_MS, DEFAULT_SESSION_TTL_MS};
use strata_backend::limits::Limits;
use strata_model::machines::guess_throttle_flow_impl::{
    Emitted, Event, GuessThrottleFlowImpl, PwAttemptPayload, PwLockoutEndedPayload, State, Vars,
};
use strata_model::quotas::{self, Quota};
use strata_runtime::machine::TimerCommand;
use strata_runtime::{Machine, Time};
use strata_storage_api::{throttle_step, ThrottleRow, ThrottleRule};

// ---------------------------------------------------------------------------------------
// Session lifetime
// ---------------------------------------------------------------------------------------

/// The one place the session lifetimes the default is checked against are written.
///
/// The specification disagrees with itself, and the owner has not decided which text stands:
///
///   - decision `d-700` (`specifications/spec.jsonc`, "Platform defaults for throttles, retries,
///     sessions and recovery"): "A session ends twelve hours after it was last used or fourteen
///     days after sign-in, whichever comes first." That is idle 12 hours, absolute 14 days.
///   - machine `session-authority-flow-impl`, its description: "the runtime finds it idle for 2
///     hours (7200000 ms; ...) or 12 hours (43200000 ms) after sign-in". That is idle 2 hours,
///     absolute 12 hours. The description of the state `sa-live-i` and of the component record
///     repeat 2 hours and 12 hours.
///
/// The backend follows the machine text. These constants hold those values, so that the test
/// passes today and a decision for d-700 is one edit here (and in `DEFAULT_SESSION_*_MS`):
/// idle 12 * 3_600_000, absolute 14 * 24 * 3_600_000.
const SPEC_SESSION_IDLE_MS: i64 = 2 * 3_600_000;
const SPEC_SESSION_ABSOLUTE_MS: i64 = 12 * 3_600_000;

#[test]
fn the_default_session_lifetimes_are_the_specs() {
    assert_eq!(
        DEFAULT_SESSION_IDLE_MS, SPEC_SESSION_IDLE_MS,
        "idle lifetime"
    );
    assert_eq!(
        DEFAULT_SESSION_TTL_MS, SPEC_SESSION_ABSOLUTE_MS,
        "absolute lifetime"
    );
}

/// The binary takes its defaults from the same constants, not from numbers of its own.
#[test]
fn the_binary_reads_the_defaults_from_the_constants() {
    let main = include_str!("../src/main.rs");
    assert!(main.contains("DEFAULT_SESSION_TTL_MS"));
    assert!(main.contains("DEFAULT_SESSION_IDLE_MS"));
    assert!(
        !main.contains("12 * 3_600_000") && !main.contains("2 * 3_600_000"),
        "main.rs writes a session lifetime of its own"
    );
}

// ---------------------------------------------------------------------------------------
// Quotas
// ---------------------------------------------------------------------------------------

/// One limit the backend applies, and the generated quota that limits the same thing.
struct Pair {
    /// What the backend calls it.
    backend: &'static str,
    /// The backend's count and window in milliseconds.
    backend_limit: (u64, i64),
    quota: &'static Quota,
}

fn quota_ms(quota: &Quota) -> (u64, i64) {
    (
        quota.limit,
        i64::try_from(quota.window.as_millis()).unwrap_or(i64::MAX),
    )
}

fn pairs() -> Vec<Pair> {
    let throttle = ThrottleLimits::default();
    // The source limit every sign-in method and every start of one counts against
    // (`signin::rules`, `signin::start_rules`, the passkey challenge).
    let source = (u64::from(throttle.source_max), throttle.window_ms);
    let logins = Limits::default();
    vec![
        Pair {
            backend: "ThrottleLimits source limit, password sign-in",
            backend_limit: source,
            quota: &quotas::Q_SIGN_IN_WITH_PASSWORD,
        },
        Pair {
            backend: "ThrottleLimits source limit, sign-in with a link",
            backend_limit: source,
            quota: &quotas::Q_SIGN_IN_WITH_LINK,
        },
        Pair {
            backend: "ThrottleLimits source limit, sign-in with a passkey",
            backend_limit: source,
            quota: &quotas::Q_SIGN_IN_WITH_PASSKEY,
        },
        Pair {
            backend: "ThrottleLimits source limit, passkey challenge",
            backend_limit: source,
            quota: &quotas::Q_ISSUE_PASSKEY_CHALLENGE,
        },
        Pair {
            backend: "ThrottleLimits source limit, start of an OpenID Connect sign-in",
            backend_limit: source,
            quota: &quotas::Q_START_OIDC_SIGN_IN,
        },
        Pair {
            backend: "ThrottleLimits source limit, start of a SAML sign-in",
            backend_limit: source,
            quota: &quotas::Q_START_SAML_SIGN_IN,
        },
        Pair {
            backend: "ThrottleLimits source limit, request for a sign-in link",
            backend_limit: source,
            quota: &quotas::Q_REQUEST_SIGN_IN_LINK,
        },
        Pair {
            backend: "ThrottleLimits source limit, account recovery request",
            backend_limit: source,
            quota: &quotas::Q_RECOVER_ACCOUNT,
        },
        Pair {
            backend: "Limits::logins_per_source_per_minute",
            backend_limit: (
                u64::try_from(logins.logins_per_source_per_minute).unwrap_or(u64::MAX),
                60_000,
            ),
            quota: &quotas::Q_SIGN_IN_WITH_PASSWORD,
        },
    ]
}

/// The pairs whose numbers differ, listed so that they are visible. Neither side is taken as
/// right: the owner decides which number stands. Each entry names the backend limit and says why
/// the two are not simply equal.
///
///   - Request for a sign-in link: the backend counts 100 per 15 minutes per source (the shared
///     source limit); the quota allows 20 per hour.
///   - Account recovery request: the same, 100 per 15 minutes against 20 per hour.
///   - `Limits::logins_per_source_per_minute`: 30 per minute, counted by `Admission::login_attempt`
///     on every attempt; the quota is 100 per 15 minutes (900 seconds). The backend counts a
///     different window, so the two are different limits on the same contract, and the stricter
///     one depends on the pace of the attempts.
///
/// Not listed because no quota exists for them: the socket caps, the browser log rate, Argon2
/// memory, the Impact Lab queue, the verification interval and the second-factor prompts
/// (D-700 states five an hour, but the specification has no quota for it).
const KNOWN_MISMATCHES: &[&str] = &[
    "ThrottleLimits source limit, request for a sign-in link",
    "ThrottleLimits source limit, account recovery request",
    "Limits::logins_per_source_per_minute",
];

#[test]
fn backend_limits_match_the_generated_quotas_except_those_listed() {
    let mismatched: Vec<&str> = pairs()
        .iter()
        .filter(|pair| pair.backend_limit != quota_ms(pair.quota))
        .map(|pair| pair.backend)
        .collect();
    assert_eq!(
        mismatched,
        KNOWN_MISMATCHES,
        "the backend's limits and the generated quotas differ from the listed mismatches; \
         (backend, quota) of each: {:?}",
        pairs()
            .iter()
            .map(|p| (p.backend, p.backend_limit, p.quota.id, quota_ms(p.quota)))
            .collect::<Vec<_>>()
    );
}

/// The source limit the backend applies comes from the generated model, not from a number of
/// its own.
#[test]
fn the_source_limit_comes_from_the_generated_quota() {
    assert_eq!(
        u64::from(ThrottleLimits::default().source_max),
        quotas::Q_SIGN_IN_WITH_PASSWORD.limit
    );
}

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

/// The backend's counter, driven through `throttle_step` and cleared by a right password, as
/// `signin::sign_in` does for the account (`ThrottleSettle::Clear`).
struct Hand {
    row: Option<ThrottleRow>,
    rule: ThrottleRule,
}

impl Hand {
    /// The account rule as `signin::rules` builds it from the default `ThrottleLimits`
    /// (that function takes an `App`, which a test cannot make without a store).
    fn new() -> Self {
        let limits = ThrottleLimits::default();
        Hand {
            row: None,
            rule: ThrottleRule {
                key: "account".to_string(),
                max: limits.account_max,
                window_ms: limits.window_ms,
                lock_ms: limits.account_lock_ms,
                lock_resets: true,
            },
        }
    }

    fn attempt(&mut self, guess: &str, now: i64) -> Verdict {
        match throttle_step(self.row, &self.rule, now) {
            Err(_) => Verdict::Refused,
            Ok(_) if guess == "right" => {
                self.row = None;
                Verdict::Accepted
            }
            Ok(row) => {
                self.row = Some(row);
                Verdict::Refused
            }
        }
    }

    /// Whether the lock has ended: the next attempt starts a fresh count (`lock_resets`).
    fn lock_ended(&self, now: i64) -> bool {
        self.row
            .is_some_and(|row| row.locked_until != 0 && now >= row.locked_until)
    }

    fn seen(&self, now: i64) -> Seen {
        match self.row {
            Some(row) if row.locked_until > now => Seen {
                closed: true,
                failures: u64::from(row.failures),
            },
            Some(row) if row.locked_until != 0 => Seen {
                closed: false,
                failures: 0,
            },
            Some(row) => Seen {
                closed: false,
                failures: u64::from(row.failures),
            },
            None => Seen {
                closed: false,
                failures: 0,
            },
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

/// The throttle the backend runs and the machine the specification generates give the same
/// verdict at every attempt, and show the same counter after every step, over fixed-seed traces.
///
/// Verifies: IDN-008
#[test]
fn the_account_throttle_follows_the_generated_machine() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut locked, mut unlocked, mut accepted, mut refused_while_closed) = (0, 0, 0, 0);
    for index in 0..400 {
        let acts = trace(&mut rng);
        let (mut model, mut hand) = (Model::new(), Hand::new());
        let mut now = 1_000_000;
        for act in &acts {
            now += match act {
                Act::Wait(ms) => *ms,
                _ => 1,
            };
            // The machine measures no time: its lockout timer is delivered when the hand-written
            // lock ends. This is the one place the two clocks meet (difference 1 below).
            if hand.lock_ended(now) && model.state == State::GtiClosed {
                model.step(Event::PwLockoutEnded(PwLockoutEndedPayload {}), now);
                unlocked += 1;
            }
            let guess = match act {
                Act::Wrong => Some("wrong"),
                Act::Right => Some("right"),
                Act::Wait(_) => None,
            };
            if let Some(guess) = guess {
                let was_closed = model.state == State::GtiClosed;
                let (by_model, by_hand) = (model.attempt(guess, now), hand.attempt(guess, now));
                assert_eq!(
                    by_hand, by_model,
                    "trace {index}, {act:?} at {now}: backend and machine disagree on the verdict \
                     (trace {acts:?})"
                );
                if by_model == Verdict::Accepted {
                    accepted += 1;
                }
                if was_closed {
                    refused_while_closed += 1;
                }
            }
            assert_eq!(
                hand.seen(now),
                model.seen(),
                "trace {index}, {act:?} at {now}: backend and machine disagree on the counter \
                 (trace {acts:?})"
            );
            if model.state == State::GtiClosed {
                locked += 1;
            }
        }
    }
    // The traces reach the states that matter, so agreement is not agreement about nothing.
    assert!(locked > 100, "the counter closed in only {locked} steps");
    assert!(unlocked > 20, "a lock ended only {unlocked} times");
    assert!(accepted > 100, "only {accepted} attempts were accepted");
    assert!(
        refused_while_closed > 100,
        "only {refused_while_closed} attempts met a closed counter"
    );
}

/// Differences between the throttle and the machine, by design, each shown to hold:
///
///   1. Time. The machine has no clock: it closes after ten failures and opens on the event
///      `pw-lockout-ended`, which its own timer fires after 30 model ticks (read as 30 seconds
///      at the model's time scale of one tick to a second). The backend's lock is
///      `account_lock_ms`, 30 seconds (D-700). The trace test delivers the event when the
///      backend's lock ends. Here: the lock is 30 000 ms and the machine's timer 30 ticks.
///   2. The window. The backend forgets failures older than `window_ms` (15 minutes, D-700); the
///      machine counts failures for ever. Here: nine failures, then a pause past the window, then
///      one more: the backend holds one failure and stays open, the machine closes.
#[test]
fn the_throttle_and_the_machine_differ_only_by_time() {
    // Difference 1.
    let mut model = Model::new();
    for _ in 0..10 {
        model.attempt("wrong", 0);
    }
    assert_eq!(model.state, State::GtiClosed);
    assert_eq!(model.lockout, Some(Duration::from_millis(30)));
    assert_eq!(ThrottleLimits::default().account_lock_ms, 30_000);
    assert_eq!(ThrottleLimits::default().account_max, 10);

    // Difference 2.
    let (mut model, mut hand) = (Model::new(), Hand::new());
    for at in 0..9 {
        model.attempt("wrong", at);
        hand.attempt("wrong", at);
    }
    let later = ThrottleLimits::default().window_ms + 1_000;
    model.attempt("wrong", later);
    hand.attempt("wrong", later);
    assert_eq!(
        hand.seen(later),
        Seen {
            closed: false,
            failures: 1
        }
    );
    assert_eq!(
        model.seen(),
        Seen {
            closed: true,
            failures: 10
        }
    );
}
