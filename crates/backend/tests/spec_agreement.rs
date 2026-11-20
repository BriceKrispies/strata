//! Where the backend's hand-written limits, lifetimes and throttle logic meet the generated model
//! of the specification (`strata-model`): the session lifetime defaults, the quotas, and the
//! account throttle and the session counted step for step against the generated machines
//! `guess-throttle-flow-impl` and `session-authority-flow-impl`. Each known difference is listed with its reason; a difference not
//! listed fails.

use std::time::Duration;
use strata_backend::app::{
    ApiError, ThrottleLimits, DEFAULT_SESSION_IDLE_MS, DEFAULT_SESSION_TTL_MS,
};
use strata_backend::limits::Limits;
use strata_kernel::{DomainError, ErrorKind};
use strata_model::machines::guess_throttle_flow_impl::{
    Emitted, Event, GuessThrottleFlowImpl, PwAttemptPayload, PwLockoutEndedPayload, State, Vars,
};
use strata_model::quotas::{self, Quota};
use strata_model_core::functions::{error_code_of, store_error_code};
use strata_model_core::types::StoreErrorKind;
use strata_runtime::machine::TimerCommand;
use strata_runtime::{Machine, Time};
use strata_storage_api::{throttle_step, StoreError, ThrottleRow, ThrottleRule};

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
            backend: "Limits::logins_per_source_per_minute",
            backend_limit: (
                u64::try_from(logins.logins_per_source_per_minute).unwrap_or(u64::MAX),
                60_000,
            ),
            quota: &quotas::Q_SIGN_IN_WITH_PASSWORD_BURST,
        },
    ]
}

/// The pairs whose numbers differ: none. The three that differed are resolved.
///
///   - Request for a sign-in link and account recovery request: the backend counted both against
///     the one budget of failed attempts per source (100 in 15 minutes), which is not the limit
///     the specification states for them (20 calls an hour of each contract, `q-request-sign-in-link`
///     and `q-recover-account`). Both are now counted by `strata_backend::quota::admit` from the
///     generated table, so the number is the specification's by construction; the shared budget
///     stays as the different limit it is (IDN-250) and is compared above only for the contracts
///     it measures the same thing for. `the_contract_quotas_are_enforced_from_the_generated_table`
///     holds that these contracts are routed.
///   - `Limits::logins_per_source_per_minute`: 30 per minute, counted by
///     `Admission::login_attempt`, against 100 in 15 minutes: a burst limit is not a budget. The
///     specification now states the burst as its own quota, `q-sign-in-with-password-burst`, and
///     the backend's default is that quota's limit.
///
/// Not listed because no quota exists for them: the socket caps, the browser log rate, Argon2
/// memory, the Impact Lab queue, the verification interval and the second-factor prompts
/// (D-700 states five an hour, but the specification has no quota for it).
const KNOWN_MISMATCHES: &[&str] = &[];

#[test]
fn backend_limits_match_the_generated_quotas() {
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

/// The contracts whose quotas the backend counts itself are routed to the one enforcement point,
/// and each routed contract has a quota in the generated table: a number is never restated.
#[test]
fn the_contract_quotas_are_enforced_from_the_generated_table() {
    use strata_backend::quota::{contract_of_route, ROUTES};
    assert_eq!(
        contract_of_route("/internal/sign-in/link/request"),
        Some("request-sign-in-link")
    );
    assert_eq!(
        contract_of_route("/internal/recovery/request"),
        Some("recover-account")
    );
    assert_eq!(
        contract_of_route("/internal/aggregate"),
        Some("aggregate-run")
    );
    for (route, contract) in ROUTES {
        assert!(
            quotas::for_contract(contract).next().is_some(),
            "{route} serves {contract}, which has no quota"
        );
    }
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

// ---------------------------------------------------------------------------------------
// Error mapping (WG-4893, WG-4939)
// ---------------------------------------------------------------------------------------

/// The HTTP status of each error kind: `system.generation.errorKinds` of the specification.
const ERROR_KIND_STATUS: &[(&str, u16)] = &[
    ("unauthenticated", 401),
    ("forbidden", 403),
    ("not_found", 404),
    ("conflict", 409),
    ("invalid", 422),
    ("throttled", 429),
    ("unavailable", 503),
    ("internal", 500),
];

/// Every kind of kernel rejection. The match has no wildcard, so a kind added to the
/// specification stops this test compiling until it is listed here and compared below.
fn every_kind() -> Vec<ErrorKind> {
    let all = vec![
        ErrorKind::TenantMismatch,
        ErrorKind::TenantExists,
        ErrorKind::TenantNotFound,
        ErrorKind::Unauthorized,
        ErrorKind::UnknownObject,
        ErrorKind::UnknownField,
        ErrorKind::UnknownRecord,
        ErrorKind::UnknownRole,
        ErrorKind::UnknownQuery,
        ErrorKind::TypeMismatch,
        ErrorKind::MissingRequired,
        ErrorKind::ConstraintViolated,
        ErrorKind::UniqueViolated,
        ErrorKind::DanglingReference,
        ErrorKind::StillReferenced,
        ErrorKind::WorkflowViolation,
        ErrorKind::VersionConflict,
        ErrorKind::SchemaConflict,
        ErrorKind::InvalidSchema,
        ErrorKind::MigrationRejected,
        ErrorKind::InsufficientState,
        ErrorKind::Evaluation,
        ErrorKind::InvalidIntent,
        ErrorKind::LimitExceeded,
        ErrorKind::IdempotencyMismatch,
    ];
    for kind in &all {
        match kind {
            ErrorKind::TenantMismatch
            | ErrorKind::TenantExists
            | ErrorKind::TenantNotFound
            | ErrorKind::Unauthorized
            | ErrorKind::UnknownObject
            | ErrorKind::UnknownField
            | ErrorKind::UnknownRecord
            | ErrorKind::UnknownRole
            | ErrorKind::UnknownQuery
            | ErrorKind::TypeMismatch
            | ErrorKind::MissingRequired
            | ErrorKind::ConstraintViolated
            | ErrorKind::UniqueViolated
            | ErrorKind::DanglingReference
            | ErrorKind::StillReferenced
            | ErrorKind::WorkflowViolation
            | ErrorKind::VersionConflict
            | ErrorKind::SchemaConflict
            | ErrorKind::InvalidSchema
            | ErrorKind::MigrationRejected
            | ErrorKind::InsufficientState
            | ErrorKind::Evaluation
            | ErrorKind::InvalidIntent
            | ErrorKind::LimitExceeded
            | ErrorKind::IdempotencyMismatch => {}
        }
    }
    all
}

/// The wire name of an error code, which the specification's `error-code` and the backend's
/// `ErrorCode` both write in snake_case.
fn wire<T: serde::Serialize>(code: &T) -> String {
    serde_json::to_value(code)
        .expect("an error code serialises")
        .as_str()
        .expect("an error code is a string")
        .to_string()
}

/// A kernel rejection becomes the error code and the HTTP status that the specification's
/// `error-code-of` and `system.generation.errorKinds` give, for every kind of rejection, for an
/// anonymous caller and a signed-in one alike.
#[test]
fn a_kernel_rejection_is_answered_with_the_code_and_status_the_specification_states() {
    for kind in every_kind() {
        for anonymous in [true, false] {
            let error = DomainError {
                kind,
                message: "refused".to_string(),
                evidence: Vec::new(),
                missing: None,
            };
            let backend = ApiError::domain(error, anonymous);
            let stated = error_code_of(&kind, anonymous);
            assert_eq!(
                wire(&backend.0.code),
                wire(&stated),
                "{kind:?} (anonymous: {anonymous})"
            );
            let status = ERROR_KIND_STATUS
                .iter()
                .find(|(name, _)| *name == wire(&stated))
                .map(|(_, status)| *status);
            assert_eq!(
                Some(backend.0.code.status()),
                status,
                "the status of {kind:?} (anonymous: {anonymous})"
            );
            assert_eq!(
                backend.0.kind.as_deref(),
                Some(format!("{kind:?}").as_str()),
                "the reply names the kind of the rejection"
            );
        }
    }
}

/// A store failure becomes the code that `store-error-code` states, and the generic message that
/// `store-api-error` states for it, with the detail kept out of the reply.
#[test]
fn a_store_failure_is_answered_with_the_code_the_specification_states() {
    let cases = [
        (
            StoreError::Conflict {
                expected: None,
                found: None,
            },
            StoreErrorKind::Conflict,
            "Someone else changed this at the same moment. Try again.",
        ),
        (
            StoreError::NotFound("a record".to_string()),
            StoreErrorKind::NotFound,
            "That could not be found.",
        ),
        (
            StoreError::Invalid("a detail".to_string()),
            StoreErrorKind::Invalid,
            "The request could not be stored.",
        ),
        (
            StoreError::Unavailable("busy".to_string()),
            StoreErrorKind::Unavailable,
            "The service is not available right now.",
        ),
        (
            StoreError::Corrupt("a page".to_string()),
            StoreErrorKind::Corrupt,
            "Something went wrong on our side.",
        ),
    ];
    for (failure, kind, message) in cases {
        let backend = ApiError::from(failure);
        assert_eq!(
            wire(&backend.0.code),
            wire(&store_error_code(&kind)),
            "{kind:?}"
        );
        assert_eq!(backend.0.message, message, "{kind:?}");
        assert!(backend.0.kind.is_none() && backend.0.failed.is_empty());
    }
}

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
