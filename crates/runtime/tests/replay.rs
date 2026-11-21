//! Idempotent replay over real HTTP: a key is scoped to the tenant, the caller and the request,
//! a key reused with another request is refused, a duplicate of a running call waits for it, a
//! timed-out call still records its answer for the retry, the cache is bounded by bytes, and the
//! client does not invent a key when it has no randomness.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use strata_runtime::{
    Access, CallError, Client, Contract, ContractError, ContractSpec, NoCredentials, Routes,
    RuntimeBody, ServiceToken,
};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
enum Oops {
    #[serde(rename = "busy")]
    Busy,
}

impl ContractError for Oops {
    fn status(&self) -> u16 {
        409
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Req {
    value: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Res {
    runs: u32,
    value: String,
}

/// Idempotent, a long timeout.
struct Fast;

impl Contract for Fast {
    const SPEC: ContractSpec = ContractSpec {
        id: "CT-T1",
        path: "/fast",
        access: Access::INTERNAL_ANONYMOUS,
        timeout: Duration::from_secs(5),
        idempotent: true,
    };
    type Request = Req;
    type Response = Res;
    type Error = Oops;
    type Body = RuntimeBody;
}

/// Idempotent, a timeout shorter than its handler.
struct Slow;

impl Contract for Slow {
    const SPEC: ContractSpec = ContractSpec {
        id: "CT-T2",
        path: "/slow",
        access: Access::INTERNAL_ANONYMOUS,
        timeout: Duration::from_millis(100),
        idempotent: true,
    };
    type Request = Req;
    type Response = Res;
    type Error = Oops;
    type Body = RuntimeBody;
}

/// Serve both contracts; the handler counts its runs and answers the count after `delay`.
async fn serve(max_bytes: Option<usize>, delay: Duration) -> (String, Arc<AtomicU32>) {
    let runs = Arc::new(AtomicU32::new(0));
    let mut routes = Routes::new(ServiceToken {
        header: "x-token",
        secret: "s".into(),
    });
    if let Some(max_bytes) = max_bytes {
        routes = routes.with_replay_bytes(max_bytes);
    }
    let fast = runs.clone();
    let slow = runs.clone();
    let routes = routes
        .serve::<Fast, _, _>(move |_caller, request: Req| {
            let runs = fast.clone();
            async move {
                let runs = runs.fetch_add(1, Ordering::SeqCst) + 1;
                tokio::time::sleep(delay).await;
                Ok(Res {
                    runs,
                    value: request.value,
                })
            }
        })
        .serve::<Slow, _, _>(move |_caller, request: Req| {
            let runs = slow.clone();
            async move {
                let runs = runs.fetch_add(1, Ordering::SeqCst) + 1;
                tokio::time::sleep(Duration::from_millis(400)).await;
                Ok(Res {
                    runs,
                    value: request.value,
                })
            }
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, routes.into_router()).await.unwrap();
    });
    (format!("http://{address}"), runs)
}

/// POST `value` to `path` with an idempotency key and a tenant; the status and the JSON body.
async fn post(
    base: &str,
    path: &str,
    key: &str,
    tenant: &str,
    value: &str,
) -> (u16, serde_json::Value) {
    let response = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header("idempotency-key", key)
        .header("x-tenant-id", tenant)
        .header("content-type", "application/json")
        .body(
            serde_json::to_string(&Req {
                value: value.into(),
            })
            .unwrap(),
        )
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_key_from_two_tenants_runs_the_handler_for_each() {
    let (base, runs) = serve(None, Duration::ZERO).await;
    let (status, first) = post(&base, "/fast", "k", "tenant-a", "v").await;
    assert_eq!((status, first["runs"].as_u64()), (200, Some(1)));
    let (status, other) = post(&base, "/fast", "k", "tenant-b", "v").await;
    assert_eq!((status, other["runs"].as_u64()), (200, Some(2)));
    // The same tenant and request is replayed, not run.
    let (status, again) = post(&base, "/fast", "k", "tenant-a", "v").await;
    assert_eq!((status, again["runs"].as_u64()), (200, Some(1)));
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_reused_with_another_body_is_refused_not_replayed() {
    let (base, runs) = serve(None, Duration::ZERO).await;
    let (status, _) = post(&base, "/fast", "k", "t", "one").await;
    assert_eq!(status, 200);
    let (status, refused) = post(&base, "/fast", "k", "t", "two").await;
    assert_eq!(status, 409);
    assert_eq!(refused["kind"], "runtime");
    assert_eq!(refused["code"], "idempotency_conflict");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    // The refusal did not disturb the first answer.
    let (status, again) = post(&base, "/fast", "k", "t", "one").await;
    assert_eq!((status, again["value"].as_str()), (200, Some("one")));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_duplicates_wait_for_the_first_and_share_its_answer() {
    let (base, runs) = serve(None, Duration::from_millis(300)).await;
    let (a, b, c) = tokio::join!(
        post(&base, "/fast", "k", "t", "v"),
        post(&base, "/fast", "k", "t", "v"),
        post(&base, "/fast", "k", "t", "v"),
    );
    for (status, body) in [a, b, c] {
        assert_eq!((status, body["runs"].as_u64()), (200, Some(1)));
    }
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_timed_out_call_is_not_run_again_by_its_retry() {
    let (base, runs) = serve(None, Duration::ZERO).await;
    // The handler needs 400 ms, the contract allows 100.
    let (status, body) = post(&base, "/slow", "k", "t", "v").await;
    assert_eq!(status, 504);
    assert_eq!(body["code"], "timeout");
    // A retry while the handler still runs waits, times out too, and starts nothing.
    let (status, _) = post(&base, "/slow", "k", "t", "v").await;
    assert_eq!(status, 504);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    // Once the handler has finished, the retry gets its recorded answer.
    // A retry that comes too early waits, times out and starts nothing, so retry until the
    // recorded answer comes, for at most five seconds.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let (status, answer) = loop {
        let (status, answer) = post(&base, "/slow", "k", "t", "v").await;
        if status != 504 || std::time::Instant::now() >= deadline {
            break (status, answer);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!((status, answer["runs"].as_u64()), (200, Some(1)));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cache_is_bounded_by_bytes_and_forgets_the_oldest_answer() {
    // An answer is charged 96 bytes, its scope (5 + 1 + 0 + 2 here) and its body (22): 126. Two fit
    // in 300 bytes, three do not.
    let (base, runs) = serve(Some(300), Duration::ZERO).await;
    for key in ["k1", "k2", "k3"] {
        let (status, _) = post(&base, "/fast", key, "t", "v").await;
        assert_eq!(status, 200);
    }
    assert_eq!(runs.load(Ordering::SeqCst), 3);
    // The two newest are still replayed.
    for (key, expected) in [("k3", 3), ("k2", 2)] {
        let (_, body) = post(&base, "/fast", key, "t", "v").await;
        assert_eq!(body["runs"].as_u64(), Some(expected));
    }
    assert_eq!(runs.load(Ordering::SeqCst), 3);
    // The oldest was forgotten, so its key runs the handler again.
    let (_, body) = post(&base, "/fast", "k1", "t", "v").await;
    assert_eq!(body["runs"].as_u64(), Some(4));
    assert_eq!(runs.load(Ordering::SeqCst), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_larger_than_the_bound_is_not_kept() {
    let (base, runs) = serve(Some(50), Duration::ZERO).await;
    post(&base, "/fast", "k", "t", "v").await;
    post(&base, "/fast", "k", "t", "v").await;
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_key_source_fails_the_call_and_sends_nothing() {
    let (base, runs) = serve(None, Duration::ZERO).await;
    let client = Client::new(&base, NoCredentials)
        .unwrap()
        .with_key_source(|| Err("no entropy".to_string()));
    let result = client
        .call::<Fast>(&Req { value: "v".into() })
        .await
        .map(|response| response.runs);
    assert!(matches!(result, Err(CallError::Key(why)) if why == "no entropy"));
    assert_eq!(runs.load(Ordering::SeqCst), 0);
}
