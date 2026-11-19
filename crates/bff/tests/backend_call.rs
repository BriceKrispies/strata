//! The BFF's call to the backend's internal API, against a stand-in backend that answers as told.

use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value as Json_};
use std::path::PathBuf;
use std::sync::atomic::AtomicI64;
use strata_bff::routes::{backend, Visit};
use strata_bff::{token, Bff, Config};
use strata_kernel::TenantId;
use strata_protocol::api::ErrorCode;

fn bff_for(backend_url: String) -> Bff {
    Bff {
        cfg: Config {
            addr: "127.0.0.1:0".parse().unwrap(),
            backend: backend_url,
            service_token: "service-token-0123456789abcdef".into(),
            sites: Config::parse_sites("127.0.0.1=fieldnotes").unwrap(),
            origins: vec![],
            dist: PathBuf::new(),
            dev: true,
            trusted_proxies: vec![],
            limits: Default::default(),
        },
        admission: strata_bff::limits::Admission::new(&Default::default()),
        http: reqwest::Client::new(),
        sockets: AtomicI64::new(0),
        in_flight: AtomicI64::new(0),
        index: None,
        tokens: token::Issuer::new().unwrap(),
    }
}

fn visit(session: Option<&str>) -> Visit {
    Visit {
        site: TenantId::new("fieldnotes").unwrap(),
        token: session.map(str::to_string),
        correlation: "c-visit-7".into(),
        source: None,
        origin: None,
    }
}

async fn stand_in() -> String {
    async fn echo(headers: HeaderMap) -> Json<Json_> {
        let get = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        Json(json!({
            "service": get("x-strata-service"),
            "site": get("x-strata-site"),
            "correlation": get("x-correlation-id"),
            "session": get("x-strata-session"),
            "origin": get("x-strata-origin"),
            "source": get("x-strata-source"),
        }))
    }
    async fn refuse() -> (StatusCode, Json<Json_>) {
        (
            StatusCode::CONFLICT,
            Json(json!({ "code": "conflict", "message": "taken", "correlation": "c-backend" })),
        )
    }
    async fn garbled() -> &'static str {
        "this is not json"
    }
    let router = Router::new()
        .route("/echo", post(echo))
        .route("/refuse", post(refuse))
        .route("/garbled", post(garbled));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await });
    url
}

/// Verifies: H-007
#[tokio::test]
async fn the_internal_call_carries_its_credentials_and_reports_failures_as_unavailable() {
    let bff = bff_for(stand_in().await);

    let seen: Json_ = backend(&bff, &visit(Some("session-secret")), "/echo", &json!({}))
        .await
        .ok()
        .unwrap();
    assert_eq!(seen["service"], "service-token-0123456789abcdef");
    assert_eq!(seen["site"], "fieldnotes");
    assert_eq!(seen["correlation"], "c-visit-7");
    assert_eq!(seen["session"], "session-secret");
    let anonymous: Json_ = backend(&bff, &visit(None), "/echo", &json!({}))
        .await
        .ok()
        .unwrap();
    assert_eq!(anonymous["session"], Json_::Null, "no session, no header");
    assert_eq!(
        anonymous["origin"],
        Json_::Null,
        "no checked origin, no header"
    );
    let from_page = Visit {
        origin: Some("http://localhost:4200".into()),
        ..visit(None)
    };
    let at: Json_ = backend(&bff, &from_page, "/echo", &json!({}))
        .await
        .ok()
        .unwrap();
    assert_eq!(at["origin"], "http://localhost:4200");

    // The service's own error reply is passed on.
    let refused = backend::<Json_>(&bff, &visit(None), "/refuse", &json!({}))
        .await
        .err()
        .unwrap();
    assert_eq!(refused.0.code, ErrorCode::Conflict);
    assert_eq!(refused.0.correlation, "c-visit-7");

    // An answer that cannot be read, and a service that does not answer, are unavailable.
    let garbled = backend::<Json_>(&bff, &visit(None), "/garbled", &json!({}))
        .await
        .err()
        .unwrap();
    assert_eq!(garbled.0.code, ErrorCode::Unavailable);
    assert_eq!(garbled.0.correlation, "c-visit-7");
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = bff_for(format!("http://{}", closed.local_addr().unwrap()));
    drop(closed);
    let unreachable = backend::<Json_>(&dead, &visit(None), "/echo", &json!({}))
        .await
        .err()
        .unwrap();
    assert_eq!(unreachable.0.code, ErrorCode::Unavailable);
    assert_eq!(unreachable.0.correlation, "c-visit-7");
}

/// The visitor's source address goes to the backend with the call, and only when there is one.
#[tokio::test]
async fn the_internal_call_states_the_visitors_source() {
    let bff = bff_for(stand_in().await);
    let from = Visit {
        source: Some("203.0.113.9".into()),
        ..visit(None)
    };
    let seen: Json_ = backend(&bff, &from, "/echo", &json!({}))
        .await
        .ok()
        .unwrap();
    assert_eq!(seen["source"], "203.0.113.9");
    let none: Json_ = backend(&bff, &visit(None), "/echo", &json!({}))
        .await
        .ok()
        .unwrap();
    assert_eq!(none["source"], Json_::Null);
}
