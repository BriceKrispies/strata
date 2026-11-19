//! The wiring backends end to end, on the crate specc generates from
//! `tests/fixtures/wiring/spec.jsonc` (`crates/spec-compiler-wiring-fixture`):
//!
//! - generated routes served in-process on an ephemeral port, with in-test implementations of
//!   the component traits, called through the generated clients and `Deps`: a successful call
//!   with its audit entry, a validation refusal, policy refusals and a quota refusal;
//! - the machine handlers bringing contract requests and another machine's emitted event to the
//!   machines, through the memory store.
//!
//! That an undeclared call does not compile is the `compile_fail` example in
//! `src/backend/deps.rs`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::http::HeaderMap;
use strata_runtime::{
    Authorizer, CallError, Caller, Client, Contract, DispatchError, Dispatcher, Envelope,
    FailureCode, Host, MemoryStore, NoCredentials, Refusal, Routes, ServiceToken, TestClock, Time,
};
use strata_spec_compiler_wiring_fixture::contracts::{
    self, CancelJobRequest, CancelJobResponse, MakeReportRequest, MakeReportResponse,
    SubmitJobError, SubmitJobRequest, SubmitJobResponse,
};
use strata_spec_compiler_wiring_fixture::{
    audits, deps, handlers, machines, prelude, quotas, routes,
};

/// The header the test's callers name themselves in: `kind/subject` or `kind/subject/role`.
const PRINCIPAL: &str = "x-principal";

/// Admits anyone to a contract whose requirement is `none`, and otherwise the principal named in
/// the `x-principal` header.
struct Principals;

impl Authorizer for Principals {
    async fn authorize(&self, requirement: &str, headers: &HeaderMap) -> Result<Caller, Refusal> {
        let named = headers.get(PRINCIPAL).and_then(|v| v.to_str().ok());
        let Some(named) = named else {
            if requirement == "none" {
                return Ok(Caller::default());
            }
            return Err(Refusal {
                code: FailureCode::Unauthenticated,
                message: "name a principal".into(),
            });
        };
        let parts: Vec<&str> = named.split('/').collect();
        let mut attributes = BTreeMap::from([("kind".to_string(), parts[0].to_string())]);
        if let Some(role) = parts.get(2) {
            attributes.insert("role".to_string(), role.to_string());
        }
        Ok(Caller {
            subject: parts.get(1).map(|s| s.to_string()),
            correlation: String::new(),
            attributes,
        })
    }
}

/// The scheduler: accepts every job, and asks the reports component for a report through the
/// only client its `Deps` holds.
struct Scheduler {
    deps: deps::SchedulerDeps,
    submitted: Mutex<Vec<String>>,
}

impl contracts::Scheduler for Scheduler {
    async fn submit_job(
        &self,
        request: SubmitJobRequest,
    ) -> Result<SubmitJobResponse, SubmitJobError> {
        let report = self
            .deps
            .reports()
            .make_report(&MakeReportRequest {
                job: request.job.clone(),
            })
            .await;
        self.submitted.lock().unwrap().push(request.job);
        Ok(SubmitJobResponse {
            accepted: report.is_ok_and(|r| r.text.starts_with("report")),
        })
    }

    async fn cancel_job(
        &self,
        _request: CancelJobRequest,
    ) -> Result<CancelJobResponse, contracts::CancelJobError> {
        Ok(CancelJobResponse {})
    }
}

struct Reports;

impl contracts::Reports for Reports {
    async fn make_report(
        &self,
        request: MakeReportRequest,
    ) -> Result<MakeReportResponse, contracts::MakeReportError> {
        Ok(MakeReportResponse {
            text: format!("report of {}", request.job),
        })
    }
}

/// The scheduler's hooks: quota counters in memory at a standing time, and the audit log.
#[derive(Default)]
struct Hooks {
    counters: Mutex<BTreeMap<(String, String), quotas::Counter>>,
    log: Mutex<Vec<audits::JobSubmittedAuditEntry>>,
}

impl routes::SchedulerHooks for Hooks {
    fn now(&self) -> prelude::Timestamp {
        prelude::Timestamp(7)
    }

    async fn count(
        &self,
        quota: &'static quotas::Quota,
        caller: &strata_spec_compiler_wiring_fixture::support::Caller,
    ) -> Result<quotas::QuotaOutcome, String> {
        let mut counters = self.counters.lock().unwrap();
        let scope = (quota.id.to_string(), caller.id.clone());
        let outcome = quota.check(counters.get(&scope).copied(), prelude::Timestamp(7));
        if let quotas::QuotaOutcome::Allowed(counter) = outcome {
            counters.insert(scope, counter);
        }
        Ok(outcome)
    }

    async fn append_job_submitted_audit(
        &self,
        entry: audits::JobSubmittedAuditEntry,
    ) -> Result<(), String> {
        self.log.lock().unwrap().push(entry);
        Ok(())
    }
}

/// Serves `router` on an ephemeral local port and returns its base address.
async fn serve(router: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(strata_runtime::server::serve(
        listener,
        router,
        std::future::pending(),
    ));
    base
}

/// A scheduler client that names the principal `named` (`kind/subject[/role]`).
fn as_principal(base: &str, named: &str) -> routes::SchedulerClient<ServiceToken> {
    let token = ServiceToken {
        header: PRINCIPAL,
        secret: named.to_string(),
    };
    routes::SchedulerClient::new(Client::new(base, token).unwrap())
}

fn runtime_code<E>(result: Result<SubmitJobResponse, CallError<E>>) -> Option<FailureCode> {
    match result {
        Err(CallError::Runtime { code, .. }) => Some(code),
        _ => None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn generated_routes_and_clients_round_trip_with_their_checks() {
    assert_eq!(
        <routes::SubmitJob as Contract>::SPEC.path,
        "/api/jobs/submit"
    );
    assert_eq!(
        <routes::MakeReport as Contract>::SPEC.path,
        "/reports/make-report"
    );

    // The scheduler needs the reports address before the server is up, so bind first.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let reports = routes::ReportsClient::new(Client::new(&base, NoCredentials).unwrap());
    let scheduler = Arc::new(Scheduler {
        deps: deps::SchedulerDeps::new(reports),
        submitted: Mutex::new(Vec::new()),
    });
    let hooks = Arc::new(Hooks::default());
    let served =
        routes::scheduler_routes(Routes::new(Principals), scheduler.clone(), hooks.clone());
    let served = routes::reports_routes(served, Arc::new(Reports), Arc::new(()));
    tokio::spawn(strata_runtime::server::serve(
        listener,
        served.into_router(),
        std::future::pending(),
    ));

    let alice = as_principal(&base, "operator/alice");
    let job = |job: &str, priority: u32| SubmitJobRequest {
        job: job.to_string(),
        priority,
    };

    // A call that passes every check reaches the implementation, which reaches reports through
    // its Deps, and leaves an audit entry with the hooks' time.
    let answer = alice.submit_job(&job("j1", 3)).await.unwrap();
    assert!(answer.accepted);
    let log = hooks.log.lock().unwrap().clone();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].actor, "alice");
    assert_eq!(log[0].job, "j1");
    assert_eq!(log[0].at, prelude::Timestamp(7));

    // A request that breaks the validation rule is refused before anything else.
    assert_eq!(
        runtime_code(alice.submit_job(&job("j0", 0)).await),
        Some(FailureCode::InvalidRequest)
    );
    // A guest operator is denied by a policy; a member matches no policy and the default denies.
    assert_eq!(
        runtime_code(
            as_principal(&base, "operator/gus/guest")
                .submit_job(&job("j2", 3))
                .await
        ),
        Some(FailureCode::Forbidden)
    );
    assert_eq!(
        runtime_code(
            as_principal(&base, "member/mia")
                .submit_job(&job("j2", 3))
                .await
        ),
        Some(FailureCode::Forbidden)
    );
    // Without a principal the authorizer refuses at once.
    let anonymous = routes::SchedulerClient::new(Client::new(&base, NoCredentials).unwrap());
    assert!(matches!(
        anonymous.submit_job(&job("j2", 3)).await,
        Err(CallError::Runtime {
            code: FailureCode::Unauthenticated,
            ..
        })
    ));

    // The quota allows two calls per principal a minute; refused calls above were not counted.
    alice.submit_job(&job("j2", 3)).await.unwrap();
    assert!(matches!(
        alice.submit_job(&job("j3", 3)).await,
        Err(CallError::Contract(SubmitJobError::Busy))
    ));
    // Another principal has a quota of its own.
    as_principal(&base, "operator/olga")
        .submit_job(&job("j3", 3))
        .await
        .unwrap();

    assert_eq!(*scheduler.submitted.lock().unwrap(), ["j1", "j2", "j3"]);
    assert_eq!(hooks.log.lock().unwrap().len(), 3);

    // A contract without checks is served as it is, at its default path.
    anonymous
        .cancel_job(&CancelJobRequest { job: "j1".into() })
        .await
        .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn a_component_with_no_hooks_is_served_with_unit_hooks() {
    let base = serve(
        routes::reports_routes(Routes::new(Principals), Arc::new(Reports), Arc::new(()))
            .into_router(),
    )
    .await;
    let client = routes::ReportsClient::new(Client::new(&base, NoCredentials).unwrap());
    let answer = client
        .make_report(&MakeReportRequest { job: "j9".into() })
        .await
        .unwrap();
    assert_eq!(answer.text, "report of j9");
}

/// Tracker emits nothing; its dispatcher is never asked.
struct Nowhere;

impl Dispatcher<machines::tracker::Emitted> for Nowhere {
    async fn dispatch(
        &self,
        envelope: &Envelope<machines::tracker::Emitted>,
    ) -> Result<(), DispatchError> {
        match envelope.event {}
    }
}

/// Job's dispatcher: brings what job emits to tracker through the generated delivery.
struct ToTracker {
    tracker: Host<machines::tracker::Tracker, MemoryStore, Nowhere>,
}

impl Dispatcher<machines::job::Emitted> for ToTracker {
    async fn dispatch(
        &self,
        envelope: &Envelope<machines::job::Emitted>,
    ) -> Result<(), DispatchError> {
        assert_eq!(envelope.receiver.0, "tracker");
        match handlers::tracker::deliver_from_job(&self.tracker, &envelope.event).await {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(DispatchError("not for tracker".into())),
            Err(e) => Err(DispatchError(e.to_string())),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn machine_handlers_bring_requests_and_published_events_to_the_machines() {
    let store = Arc::new(MemoryStore::new());
    let clock = Arc::new(TestClock::at(Time(1_000)));
    let tracker = Host::new(store.clone(), Arc::new(Nowhere), clock.clone()).unwrap();
    let job = Host::new(
        store.clone(),
        Arc::new(ToTracker {
            tracker: tracker.clone(),
        }),
        clock,
    )
    .unwrap();

    let request = SubmitJobRequest {
        job: "j1".into(),
        priority: 2,
    };
    let (key, event) = handlers::job::submit_job_event(&request);
    assert_eq!(key, "j1");
    assert!(matches!(
        event,
        machines::job::Event::JobSubmitted(machines::job::JobSubmittedPayload { priority: 2, .. })
    ));

    // The request steps the job, whose emitted job-started reaches tracker through the
    // generated delivery.
    let handled = handlers::job::submit_job(&job, &request).await.unwrap();
    assert_eq!(handled.state, machines::job::State::Running);
    assert_eq!(handled.vars.attempts, 1);
    assert_eq!(handled.dispatched.delivered, 1);
    let (state, vars, version) = tracker.load(&"j1".to_string()).await.unwrap().unwrap();
    assert_eq!(state, machines::tracker::State::Watching);
    assert_eq!(vars.seen, 1);
    assert_eq!(version, 1);

    // An emitted event of another instance keys tracker by its own payload.
    let other = machines::job::Emitted::JobStarted(machines::job::JobStartedPayload {
        job: "j2".into(),
        attempts: 4,
    });
    let (key, _) = handlers::tracker::from_job(&other).unwrap();
    assert_eq!(key, "j2");

    // A cancel request ends the job.
    let handled = handlers::job::cancel_job(&job, &CancelJobRequest { job: "j1".into() })
        .await
        .unwrap();
    assert_eq!(handled.state, machines::job::State::Done);
}

/// The system names JSON members in snake_case and inspect-job-camel overrides it with
/// camelCase: a request whose specification names are kebab-case goes to JSON in the wire case
/// and comes back unchanged (WG-4881); a json member is carried as it is.
#[test]
fn kebab_named_requests_cross_the_wire_in_their_wire_case_and_back() {
    use serde_json::json;
    use strata_spec_compiler_wiring_fixture::contracts::{
        InspectJobCamelRequest, InspectJobRequest, InspectJobResponse, MakeReportError,
    };
    use strata_spec_compiler_wiring_fixture::types::JobSummary;

    let request = InspectJobRequest {
        job_id: "j-1".into(),
        filter_params: Some(json!({ "page-size": 2, "tags": ["a"] })),
    };
    let wire = serde_json::to_value(&request).unwrap();
    assert_eq!(
        wire,
        json!({ "job_id": "j-1", "filter_params": { "page-size": 2, "tags": ["a"] } })
    );
    assert_eq!(
        serde_json::from_value::<InspectJobRequest>(wire).unwrap(),
        request
    );

    let response = InspectJobResponse {
        summary: JobSummary {
            display_name: "Nightly".into(),
            started_at: None,
            extra_info: json!(null),
        },
    };
    let wire = serde_json::to_value(&response).unwrap();
    assert_eq!(
        wire,
        json!({ "summary": { "display_name": "Nightly", "extra_info": null } })
    );
    assert_eq!(
        serde_json::from_value::<InspectJobResponse>(wire).unwrap(),
        response
    );

    let camel = InspectJobCamelRequest {
        job_id: "j-2".into(),
    };
    let wire = serde_json::to_value(&camel).unwrap();
    assert_eq!(wire, json!({ "jobId": "j-2" }));
    assert_eq!(
        serde_json::from_value::<InspectJobCamelRequest>(wire).unwrap(),
        camel
    );

    assert_eq!(
        serde_json::to_value(MakeReportError::NotFound).unwrap(),
        json!("not_found")
    );
}
