//! The outbound gate, the platform's SSRF boundary, and webhooks in both directions.
//!
//! The gate is driven with a resolver and a transport the test controls: every refused address
//! family and spelling, a host name that resolves to a refused address, a name that answers one
//! allowed and one refused address, a name whose second answer differs from its first, redirects
//! to refused and allowed addresses, the scheme, port and credential rules, and responses cut off
//! by size and by time. Then the webhook signature, inbound endpoints (signature, staleness,
//! replay, size, quota, secret rotation) and outbound deliveries made by the automation worker
//! (signed, retried, delivered once, listed for their own tenant only), on the in-memory store
//! with the kernel's notes fixture, synchronously through `worker::tick` with an injected clock.

use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use strata_backend::app::{App, Config, Shared};
use strata_backend::auth::Identity;
use strata_backend::mail::DevSink;
use strata_backend::outbound::gate::{
    Answer, Exchange, Failure, Gate, Method, Policy, Request, Resolve, Transport, TransportError,
};
use strata_backend::outbound::{self, deliveries, inbound, rule, signature};
use strata_backend::signin::oidc::{Fetch, Http};
use strata_backend::worker::{self, steps, WorkerConfig};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model::types::Identifier;
use strata_protocol::api::ErrorCode;
use strata_protocol::ws::SessionState;
use strata_storage_api::jobs::{ExceptionKind, JobWork, NewJob, RetryPolicy, RunState};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, SessionRecord, Store};

// ---------------------------------------------------------------------------------------------
// A resolver and a transport the test controls
// ---------------------------------------------------------------------------------------------

/// Answers from a table; a name may have a list of answers, one per lookup, the last repeated.
#[derive(Default)]
struct Resolver {
    answers: Mutex<BTreeMap<String, VecDeque<Vec<IpAddr>>>>,
    lookups: AtomicUsize,
}

impl Resolver {
    fn with(entries: &[(&str, &[&str])]) -> Arc<Resolver> {
        let resolver = Resolver::default();
        for (name, addresses) in entries {
            resolver.answer(name, addresses);
        }
        Arc::new(resolver)
    }

    fn answer(&self, name: &str, addresses: &[&str]) {
        let parsed = addresses.iter().map(|a| a.parse().unwrap()).collect();
        self.answers
            .lock()
            .unwrap()
            .entry(name.to_string())
            .or_default()
            .push_back(parsed);
    }
}

impl Resolve for Resolver {
    fn resolve(&self, host: &str, _timeout: Duration) -> Result<Vec<IpAddr>, String> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        let mut answers = self.answers.lock().unwrap();
        let queue = answers
            .get_mut(host)
            .ok_or_else(|| format!("{host} does not resolve"))?;
        if queue.len() > 1 {
            Ok(queue.pop_front().unwrap())
        } else {
            Ok(queue.front().cloned().unwrap_or_default())
        }
    }
}

/// One scripted answer.
#[derive(Clone, Default)]
struct Scripted {
    status: u16,
    headers: Vec<(String, String)>,
    chunks: Vec<Vec<u8>>,
    announced: Option<u64>,
    delay: Duration,
    open_error: Option<TransportError>,
}

fn status(code: u16) -> Scripted {
    Scripted {
        status: code,
        chunks: vec![b"ok".to_vec()],
        ..Scripted::default()
    }
}

fn redirect(code: u16, to: &str) -> Scripted {
    Scripted {
        status: code,
        headers: vec![("location".into(), to.into())],
        ..Scripted::default()
    }
}

/// What the transport was asked to do.
#[derive(Clone, Debug)]
struct Seen {
    url: String,
    name: Option<String>,
    address: SocketAddr,
    method: Method,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Answers by URL from scripts, one per exchange, the last repeated; records every exchange.
#[derive(Default)]
struct Wire {
    scripts: Mutex<BTreeMap<String, VecDeque<Scripted>>>,
    seen: Mutex<Vec<Seen>>,
    chunks_read: Arc<AtomicUsize>,
}

impl Wire {
    fn new() -> Arc<Wire> {
        Arc::new(Wire::default())
    }

    fn script(&self, url: &str, answer: Scripted) {
        self.scripts
            .lock()
            .unwrap()
            .entry(url.to_string())
            .or_default()
            .push_back(answer);
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

struct Played {
    script: Scripted,
    next: usize,
    read: Arc<AtomicUsize>,
}

impl Answer for Played {
    fn status(&self) -> u16 {
        self.script.status
    }
    fn headers(&self) -> &[(String, String)] {
        &self.script.headers
    }
    fn content_length(&self) -> Option<u64> {
        self.script.announced
    }
    fn next_chunk(&mut self, _deadline: Instant) -> Result<Option<Vec<u8>>, TransportError> {
        if !self.script.delay.is_zero() {
            std::thread::sleep(self.script.delay);
        }
        let chunk = self.script.chunks.get(self.next).cloned();
        if chunk.is_some() {
            self.next += 1;
            self.read.fetch_add(1, Ordering::SeqCst);
        }
        Ok(chunk)
    }
}

impl Transport for Wire {
    fn open(&self, exchange: &Exchange<'_>) -> Result<Box<dyn Answer>, TransportError> {
        self.seen.lock().unwrap().push(Seen {
            url: exchange.url.to_string(),
            name: exchange.name.map(str::to_string),
            address: exchange.address,
            method: exchange.method,
            headers: exchange.headers.to_vec(),
            body: exchange.body.to_vec(),
        });
        let script = {
            let mut scripts = self.scripts.lock().unwrap();
            let queue = scripts
                .get_mut(&exchange.url.to_string())
                .ok_or_else(|| TransportError::Failed("connection refused".into()))?;
            if queue.len() > 1 {
                queue.pop_front().unwrap()
            } else {
                queue.front().cloned().unwrap()
            }
        };
        if let Some(error) = script.open_error.clone() {
            return Err(error);
        }
        Ok(Box::new(Played {
            script,
            next: 0,
            read: self.chunks_read.clone(),
        }))
    }
}

fn policy() -> Policy {
    Policy {
        connect_timeout: Duration::from_secs(1),
        total_timeout: Duration::from_secs(5),
        max_bytes: 1024,
        max_redirects: 3,
        development: false,
    }
}

fn gate(resolver: &Arc<Resolver>, wire: &Arc<Wire>, policy: Policy) -> Gate {
    Gate::new(
        resolver.clone() as Arc<dyn Resolve>,
        wire.clone() as Arc<dyn Transport>,
        policy,
    )
}

fn get(url: &str) -> Request {
    Request::new(Method::Get, url, "test")
}

// ---------------------------------------------------------------------------------------------
// The outbound rule and the gate
// ---------------------------------------------------------------------------------------------

/// Every address family and spelling that names the platform's own network, a reserved block or
/// a metadata service, written as the host of an address.
const REFUSED_LITERALS: &[(&str, &str)] = &[
    ("https://127.0.0.1/", "loopback"),
    ("https://127.1/", "loopback, shortened"),
    ("https://2130706433/", "loopback, one decimal number"),
    ("https://0177.0.0.1/", "loopback, octal"),
    ("https://0x7f.0.0.1/", "loopback, hexadecimal octet"),
    ("https://0x7f000001/", "loopback, one hexadecimal number"),
    ("https://0x7f.1/", "loopback, hexadecimal and shortened"),
    ("https://127.255.255.254/", "loopback, end of block"),
    ("https://0.0.0.0/", "unspecified"),
    ("https://0/", "unspecified, one number"),
    ("https://10.0.0.1/", "private 10/8"),
    ("https://012.0.0.1/", "private 10/8, octal"),
    ("https://167772161/", "private 10/8, decimal"),
    ("https://172.16.0.1/", "private 172.16/12"),
    ("https://172.31.255.255/", "private 172.16/12, end"),
    ("https://192.168.1.1/", "private 192.168/16"),
    ("https://0xc0.0xa8.1.1/", "private 192.168/16, hexadecimal"),
    ("https://169.254.169.254/latest/meta-data/", "metadata"),
    ("https://2852039166/", "metadata, decimal"),
    ("https://0xa9.0xfe.0xa9.0xfe/", "metadata, hexadecimal"),
    ("https://169.254.170.2/v2/credentials", "ECS metadata"),
    ("https://169.254.1.1/", "link-local"),
    ("https://100.64.0.1/", "CGNAT"),
    ("https://100.127.255.255/", "CGNAT, end"),
    ("https://100.100.100.200/", "Alibaba metadata"),
    ("https://168.63.129.16/", "Azure wire server"),
    ("https://192.0.0.192/", "Oracle metadata"),
    ("https://192.0.0.8/", "protocol assignments"),
    ("https://192.0.2.10/", "documentation 1"),
    ("https://198.51.100.10/", "documentation 2"),
    ("https://203.0.113.10/", "documentation 3"),
    ("https://198.18.0.1/", "benchmarking"),
    ("https://192.88.99.1/", "6to4 relay"),
    ("https://224.0.0.1/", "multicast"),
    ("https://239.255.255.250/", "multicast, end"),
    ("https://240.0.0.1/", "reserved"),
    ("https://255.255.255.255/", "broadcast"),
    ("https://[::1]/", "IPv6 loopback"),
    ("https://[0:0:0:0:0:0:0:1]/", "IPv6 loopback, long form"),
    ("https://[::]/", "IPv6 unspecified"),
    ("https://[::ffff:127.0.0.1]/", "IPv4-mapped loopback"),
    (
        "https://[::ffff:7f00:1]/",
        "IPv4-mapped loopback, hexadecimal",
    ),
    ("https://[::ffff:10.0.0.1]/", "IPv4-mapped private"),
    ("https://[::ffff:169.254.169.254]/", "IPv4-mapped metadata"),
    ("https://[::127.0.0.1]/", "IPv4-compatible"),
    ("https://[::ffff:0:7f00:1]/", "IPv4-translated"),
    ("https://[64:ff9b::7f00:1]/", "NAT64 of loopback"),
    ("https://[64:ff9b::a9fe:a9fe]/", "NAT64 of metadata"),
    ("https://[64:ff9b::10.0.0.1]/", "NAT64 of private"),
    ("https://[64:ff9b:1::1]/", "local-use NAT64"),
    ("https://[fc00::1]/", "unique-local"),
    ("https://[fd12:3456::1]/", "unique-local fd"),
    ("https://[fd00:ec2::254]/", "AWS metadata over IPv6"),
    ("https://[fe80::1]/", "IPv6 link-local"),
    ("https://[febf::1]/", "IPv6 link-local, end"),
    ("https://[fec0::1]/", "site-local"),
    ("https://[ff02::1]/", "IPv6 multicast"),
    ("https://[2001:db8::1]/", "IPv6 documentation"),
    ("https://[3fff::1]/", "IPv6 documentation (RFC 9637)"),
    ("https://[2001::1]/", "Teredo"),
    ("https://[2001:10::1]/", "ORCHID"),
    ("https://[2002:7f00:1::1]/", "6to4"),
    ("https://[100::1]/", "discard-only"),
    ("https://[4000::1]/", "outside global unicast"),
];

// Verifies: WFL-008
// Verifies: WFL-252
// Verifies: WFL-2645
#[test]
fn every_refused_address_family_and_spelling_is_refused_before_any_connection() {
    let resolver = Resolver::with(&[]);
    let wire = Wire::new();
    let gate = gate(&resolver, &wire, policy());
    for (url, what) in REFUSED_LITERALS {
        match gate.call(&get(url)) {
            Err(Failure::AddressRefused(_)) => {}
            other => panic!("{what} ({url}) was not refused: {other:?}"),
        }
    }
    assert!(wire.seen().is_empty(), "nothing was connected to");
    assert_eq!(resolver.lookups.load(Ordering::SeqCst), 0);
}

// Verifies: WFL-250
#[test]
fn the_call_connects_to_the_judged_address_and_resolves_once() {
    // The second answer for the name is private: a client that resolved again would go there.
    let resolver = Resolver::with(&[]);
    resolver.answer("rebind.example", &["93.184.216.34"]);
    resolver.answer("rebind.example", &["127.0.0.1"]);
    let wire = Wire::new();
    wire.script("https://rebind.example/hook", status(204));
    let gate = gate(&resolver, &wire, policy());
    let answer = gate.call(&get("https://rebind.example/hook")).unwrap();
    assert_eq!(answer.status, 204);
    assert_eq!(answer.address, "93.184.216.34".parse::<IpAddr>().unwrap());
    assert_eq!(resolver.lookups.load(Ordering::SeqCst), 1, "resolved once");
    let seen = wire.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].address, "93.184.216.34:443".parse().unwrap());
    // The original name goes along for TLS and the Host header.
    assert_eq!(seen[0].name.as_deref(), Some("rebind.example"));
    // A numeric host is connected to as it is, with no name.
    wire.script("https://93.184.216.34/x", status(200));
    gate.call(&get("https://93.184.216.34/x")).unwrap();
    assert_eq!(wire.seen()[1].name, None);
}

// Verifies: WFL-251
// Verifies: WFL-2641
#[test]
fn a_redirect_to_a_refused_address_is_refused() {
    let resolver = Resolver::with(&[
        ("public.example", &["93.184.216.34"]),
        ("private.example", &["10.1.2.3"]),
    ]);
    let wire = Wire::new();
    let gate = gate(&resolver, &wire, policy());
    for (from, to) in [
        ("https://public.example/a", "https://127.0.0.1/admin"),
        (
            "https://public.example/b",
            "https://169.254.169.254/latest/meta-data/",
        ),
        ("https://public.example/c", "https://private.example/"),
        ("https://public.example/d", "https://[::ffff:10.0.0.1]/"),
        ("https://public.example/e", "http://public.example/"),
        ("https://public.example/f", "https://public.example:8080/"),
        (
            "https://public.example/g",
            "https://user:pw@public.example/",
        ),
        ("https://public.example/h", "https://localhost/"),
    ] {
        wire.script(from, redirect(302, to));
        assert!(
            matches!(gate.call(&get(from)), Err(Failure::RedirectRefused(_))),
            "{from} -> {to}"
        );
    }
    // Only the first hops were made.
    assert!(wire
        .seen()
        .iter()
        .all(|seen| seen.url.starts_with("https://public.example/")));
}

// Verifies: WFL-253
// Verifies: WFL-2642
#[test]
fn a_response_past_the_size_bound_is_cut_off() {
    let resolver = Resolver::with(&[("big.example", &["93.184.216.34"])]);
    let wire = Wire::new();
    // Announced too large: nothing of the body is read.
    wire.script(
        "https://big.example/announced",
        Scripted {
            status: 200,
            announced: Some(10_000),
            chunks: vec![vec![b'x'; 10_000]],
            ..Scripted::default()
        },
    );
    // Not announced: reading stops at the chunk that passes the bound.
    wire.script(
        "https://big.example/streamed",
        Scripted {
            status: 200,
            chunks: vec![vec![b'x'; 400]; 100],
            ..Scripted::default()
        },
    );
    wire.script(
        "https://big.example/fits",
        Scripted {
            status: 200,
            chunks: vec![vec![b'x'; 512], vec![b'y'; 512]],
            ..Scripted::default()
        },
    );
    let gate = gate(&resolver, &wire, policy());
    assert!(matches!(
        gate.call(&get("https://big.example/announced")),
        Err(Failure::TooLarge { limit: 1024, .. })
    ));
    assert_eq!(wire.chunks_read.load(Ordering::SeqCst), 0);
    match gate.call(&get("https://big.example/streamed")) {
        Err(Failure::TooLarge { received, limit }) => {
            assert_eq!(limit, 1024);
            assert_eq!(received, 1200);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        wire.chunks_read.load(Ordering::SeqCst),
        3,
        "reading stopped"
    );
    // Exactly the bound is allowed; a caller may ask for less, never more.
    assert_eq!(
        gate.call(&get("https://big.example/fits"))
            .unwrap()
            .body
            .len(),
        1024
    );
    let mut small = get("https://big.example/fits");
    small.max_bytes = Some(600);
    assert!(matches!(
        gate.call(&small),
        Err(Failure::TooLarge { limit: 600, .. })
    ));
    let mut greedy = get("https://big.example/fits");
    greedy.max_bytes = Some(1 << 40);
    assert!(gate.call(&greedy).is_ok());
}

// Verifies: WFL-254
#[test]
fn a_log_line_shows_no_query_and_no_userinfo() {
    let shown = rule::redacted_text("https://user:pw@hooks.example:8443/in/path?token=abc#x");
    assert_eq!(shown, "https://hooks.example:8443/in/path");
    assert!(!shown.contains("token") && !shown.contains("pw"));
}

// Verifies: FED-261
// Verifies: FED-2644
#[test]
fn an_identity_providers_name_that_resolves_inward_is_refused() {
    let resolver = Resolver::with(&[("id.acme.example", &["10.0.0.5"])]);
    let wire = Wire::new();
    wire.script(
        "https://id.acme.example/.well-known/openid-configuration",
        status(200),
    );
    let http = Http::with_gate(gate(&resolver, &wire, policy()));
    let refused = http
        .get_json("https://id.acme.example/.well-known/openid-configuration")
        .unwrap_err();
    assert!(refused.starts_with("address-refused"), "{refused}");
    assert!(wire.seen().is_empty());
    // The development exception allows it, and still connects to the address it resolved.
    let open = Http::with_gate(gate(
        &resolver,
        &wire,
        Policy {
            development: true,
            ..policy()
        },
    ));
    let _ = open.get_json("https://id.acme.example/.well-known/openid-configuration");
    assert_eq!(wire.seen()[0].address, "10.0.0.5:443".parse().unwrap());
}

// ---------------------------------------------------------------------------------------------
// The signature
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// The tenant fixture
// ---------------------------------------------------------------------------------------------

const T0: i64 = 1_800_000_000_000;

fn app() -> Shared {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cfg = Config {
        addr: "127.0.0.1:0".parse().unwrap(),
        service_token: "test-service-token-0123456789abcdef".into(),
        dev: true,
        tenants_dir: root.join("tenants"),
        lean_dir: root.join("lean"),
        session_ttl_ms: 3_600_000,
        session_idle_ms: 3_600_000,
        throttle: Default::default(),
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: Arc::new(DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    };
    App::new(cfg, Arc::new(MemoryStore::new()))
}

/// An app whose outbound calls go to `wire`, with names resolved by `resolver`.
fn app_with(resolver: &Arc<Resolver>, wire: &Arc<Wire>) -> Shared {
    let app = app();
    outbound::install(&app, gate(resolver, wire, policy())).unwrap();
    app
}

fn name(s: &str) -> StepName {
    StepName::new(s).unwrap()
}

fn plain(action: AutomationStep, next: Option<&str>) -> AutomationStepDef {
    AutomationStepDef {
        action,
        next: next.map(name),
        retry: None,
        timeout: None,
        compensate: None,
    }
}

fn automation(id: &str, start: &str, steps: Vec<(&str, AutomationStepDef)>) -> AutomationDef {
    AutomationDef {
        state: None,
        id: WorkflowId::new(id).unwrap(),
        label: id.to_string(),
        run_as: RunAs {
            roles: BTreeSet::from([role("writer")]),
        },
        run_key: None,
        start: name(start),
        steps: steps.into_iter().map(|(n, s)| (name(n), s)).collect(),
        defined_by: None,
        defined_at: None,
        suspended_for: None,
    }
}

/// A tenant with the notes schema and its people, and `automations`.
fn install(app: &App, tenant: &str, automations: Vec<AutomationDef>) {
    let store = app.store.as_ref();
    let owner = principal_id(&format!("{tenant}:owner"));
    execute(
        store,
        &Intent::CreateTenant {
            name: tenant.to_string(),
            owner: owner.clone(),
        },
        &ctx(tenant, Principal::System, 101),
    )
    .expect("tenant");
    let mut changes = notes_schema();
    changes.extend(automations.into_iter().map(SchemaChange::DefineAutomation));
    execute(
        store,
        &Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx(tenant, Principal::User(owner.clone()), 102),
    )
    .expect("schema");
    execute(
        store,
        &Intent::AssignRole {
            principal: principal_id(&format!("{tenant}:writer1")),
            role: role("writer"),
        },
        &ctx(tenant, Principal::User(owner), 103),
    )
    .expect("writer");
}

/// Define more automations after the tenant exists.
fn define(app: &App, tenant: &str, automations: Vec<AutomationDef>) {
    let owner = principal_id(&format!("{tenant}:owner"));
    execute(
        app.store.as_ref(),
        &Intent::ChangeSchema {
            changes: automations
                .into_iter()
                .map(SchemaChange::DefineAutomation)
                .collect(),
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx(tenant, Principal::User(owner), 104),
    )
    .expect("automations");
}

fn signed_in(tenant: &str, who: &str) -> Identity {
    let principal = principal_id(&format!("{tenant}:{who}"));
    Identity {
        tenant: tenant_id(tenant),
        principal: Principal::User(principal.clone()),
        session: Some(SessionRecord {
            id: SessionId::new(format!("s-{tenant}-{who}")).unwrap(),
            token_hash: format!("hash-{tenant}-{who}"),
            csrf_token: "csrf".into(),
            tenant: tenant_id(tenant),
            principal,
            display_name: who.to_string(),
            created_at: Timestamp(0),
            last_seen_at: Timestamp(0),
            expires_at: Timestamp(i64::MAX),
            revoked_at: None,
            authenticated_at: Timestamp(0),
            network: None,
        }),
        state: SessionState::Active,
        correlation: "t-outbound".into(),
        verified_address: None,
        idempotency_key: None,
    }
}

fn at(t: i64) -> impl Fn() -> Timestamp {
    move || Timestamp(t)
}

fn config() -> WorkerConfig {
    WorkerConfig::from_limits(&Default::default())
}

/// `hooked`: makes a note titled by the webhook's event identifier, then ends.
fn hooked() -> AutomationDef {
    automation(
        "hooked",
        "make",
        vec![
            (
                "make",
                plain(
                    AutomationStep::CreateRecord {
                        object: object("note"),
                        fields: BTreeMap::from([
                            (field("title"), lit("From a webhook")),
                            (
                                field("slug"),
                                Expr::Text(Box::new(Expr::Param("webhook_event".into()))),
                            ),
                        ]),
                    },
                    Some("done"),
                ),
            ),
            ("done", plain(AutomationStep::End, None)),
        ],
    )
}

fn notes(app: &App, tenant: &str) -> usize {
    app.store
        .load_complete(&tenant_id(tenant))
        .unwrap()
        .tenant
        .map(|t| t.records_of(&object("note")).count())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------------------------
// Inbound endpoints
// ---------------------------------------------------------------------------------------------

/// A delivery to `endpoint` signed with `secret` at `seconds`.
struct Signed {
    endpoint: String,
    body: Vec<u8>,
    event: String,
    timestamp: String,
    signature: String,
}

impl Signed {
    fn new(endpoint: &str, secret: &str, event: &str, seconds: i64, body: &[u8]) -> Self {
        Signed {
            endpoint: endpoint.into(),
            body: body.to_vec(),
            event: event.into(),
            timestamp: seconds.to_string(),
            signature: signature::header_value(&[secret.as_bytes().to_vec()], event, seconds, body),
        }
    }

    fn delivery(&self) -> inbound::InboundDelivery<'_> {
        inbound::InboundDelivery {
            endpoint: &self.endpoint,
            body: &self.body,
            event: &self.event,
            timestamp: &self.timestamp,
            signature: &self.signature,
        }
    }
}

fn receive(app: &App, signed: &Signed, now: i64) -> Result<Json, strata_backend::app::ApiError> {
    inbound::receive(
        app,
        &tenant_id("alpha"),
        Some("198.51.100.7"),
        &signed.delivery(),
        Timestamp(now),
    )
}

/// A tenant `alpha` with the automation `hooked` and an endpoint `orders` for it; the secret.
fn with_endpoint(app: &App) -> String {
    install(app, "alpha", vec![hooked()]);
    let created = inbound::create_endpoint(
        app,
        &signed_in("alpha", "owner"),
        &json!({ "endpoint": "orders", "workflow": "hooked" }),
    )
    .unwrap();
    assert_eq!(created["version"], 1);
    created["secret"].as_str().unwrap().to_string()
}

// Verifies: INT-150
// Verifies: INT-151
// Verifies: INT-1577
#[test]
fn a_signed_delivery_starts_its_run_once_and_a_replay_starts_nothing() {
    let app = app();
    let secret = with_endpoint(&app);
    let now = T0;
    let first = Signed::new("orders", &secret, "evt-1", now / 1000, b"{\"order\":1}");
    let answer = receive(&app, &first, now).unwrap();
    assert_eq!(answer["repeat"], false);
    let run = answer["run"].as_str().unwrap().to_string();
    worker::tick(&app, "w1", &config(), &at(now));
    let started = app
        .store
        .automation_run(&tenant_id("alpha"), &run)
        .unwrap()
        .expect("the run started");
    assert_eq!(started.state, RunState::Completed);
    assert_eq!(
        started.params.get("webhook_body"),
        Some(&Value::Str("{\"order\":1}".into()))
    );
    assert_eq!(notes(&app, "alpha"), 1);
    // The sender retries the same event, and again a minute later with a fresh signature.
    let again = receive(&app, &first, now + 1_000).unwrap();
    assert_eq!(again["repeat"], true);
    assert_eq!(again["run"], run.as_str());
    let later = Signed::new(
        "orders",
        &secret,
        "evt-1",
        (now + 60_000) / 1000,
        b"{\"order\":1}",
    );
    assert_eq!(
        receive(&app, &later, now + 60_000).unwrap()["run"],
        run.as_str()
    );
    worker::tick(&app, "w1", &config(), &at(now + 60_000));
    assert_eq!(notes(&app, "alpha"), 1, "one run for one event");
    assert_eq!(
        app.store
            .automation_runs(&tenant_id("alpha"), None, None, None, 10)
            .unwrap()
            .len(),
        1
    );
    // Another event is another run.
    let second = Signed::new("orders", &secret, "evt-2", now / 1000, b"{}");
    let other = receive(&app, &second, now + 2_000).unwrap();
    assert_ne!(other["run"], run.as_str());
}

// Verifies: INT-1578
#[test]
fn only_the_tenants_managers_create_endpoints_and_names_are_unique() {
    let app = app();
    install(&app, "alpha", vec![hooked()]);
    let owner = signed_in("alpha", "owner");
    let writer = signed_in("alpha", "writer1");
    let body = json!({ "endpoint": "orders", "workflow": "hooked" });
    assert_eq!(
        inbound::create_endpoint(&app, &writer, &body)
            .unwrap_err()
            .0
            .code,
        ErrorCode::Forbidden
    );
    inbound::create_endpoint(&app, &owner, &body).unwrap();
    assert_eq!(
        inbound::create_endpoint(&app, &owner, &body)
            .unwrap_err()
            .0
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        inbound::create_endpoint(
            &app,
            &owner,
            &json!({ "endpoint": "other", "workflow": "missing" })
        )
        .unwrap_err()
        .0
        .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        inbound::create_endpoint(
            &app,
            &owner,
            &json!({ "endpoint": "Bad/Name", "workflow": "hooked" })
        )
        .unwrap_err()
        .0
        .code,
        ErrorCode::Invalid
    );
    assert_eq!(
        inbound::rotate_endpoint(&app, &owner, &json!({ "endpoint": "nope" }))
            .unwrap_err()
            .0
            .code,
        ErrorCode::NotFound
    );
}

// ---------------------------------------------------------------------------------------------
// Outbound deliveries
// ---------------------------------------------------------------------------------------------

const TARGET: &str = "https://hooks.example/in?token=abc";

/// `notify`: delivers a webhook to `subscription`, then ends. One attempt for the step.
fn notify(subscription: &str) -> AutomationDef {
    let id: Identifier = serde_json::from_value(json!(subscription)).unwrap();
    let mut call = plain(
        AutomationStep::CallWebhook {
            subscription: id,
            body: BTreeMap::from([
                ("event".to_string(), lit("note-made")),
                ("seq".to_string(), Expr::Param("seq".into())),
            ]),
        },
        Some("done"),
    );
    call.retry = Some(StepRetry {
        attempts: 2,
        first_delay: None,
        max_delay: None,
    });
    automation(
        "notify",
        "call",
        vec![("call", call), ("done", plain(AutomationStep::End, None))],
    )
}

/// A tenant with a subscription to [`TARGET`] and the automation `notify`: the app, the
/// subscription and its secret.
fn with_subscription(
    tenant: &str,
    resolver: &Arc<Resolver>,
    wire: &Arc<Wire>,
) -> (Shared, String, String) {
    let app = app_with(resolver, wire);
    install(&app, tenant, Vec::new());
    let created = deliveries::create_subscription(
        &app,
        &signed_in(tenant, "owner"),
        &json!({ "target": TARGET }),
    )
    .unwrap();
    let subscription = created["subscription"].as_str().unwrap().to_string();
    let secret = created["secret"].as_str().unwrap().to_string();
    define(&app, tenant, vec![notify(&subscription)]);
    (app, subscription, secret)
}

fn start_notify(app: &App, tenant: &str, seq: i64, due: i64) -> String {
    let job = NewJob {
        key: format!("start:test:notify:{seq}"),
        work: JobWork::StartRun {
            trigger: None,
            automation: WorkflowId::new("notify").unwrap(),
            event_seq: Some(seq as u64),
            hop: 0,
            params: BTreeMap::from([("seq".to_string(), Value::Int(seq))]),
            caused_by: None,
            on_behalf_of: None,
            definition_version: None,
            run_key: None,
        },
        due_at: Timestamp(due),
        policy: RetryPolicy::default(),
    };
    app.store
        .jobs_create(&tenant_id(tenant), &[job], Timestamp(due))
        .unwrap()
        .remove(0)
}

fn deliveries_of(app: &App, tenant: &str) -> Vec<Json> {
    deliveries::list_deliveries(app, &signed_in(tenant, "owner"), &json!({})).unwrap()["deliveries"]
        .as_array()
        .unwrap()
        .clone()
}

fn posts(wire: &Wire) -> Vec<Seen> {
    wire.seen()
        .into_iter()
        .filter(|seen| seen.url.starts_with("https://hooks.example/"))
        .collect()
}

// Verifies: WFL-010
// Verifies: WFL-255
// Verifies: INT-152
// Verifies: INT-2640
#[test]
fn the_call_webhook_step_delivers_a_signed_body_and_the_log_shows_it() {
    let resolver = Resolver::with(&[("hooks.example", &["93.184.216.34"])]);
    let wire = Wire::new();
    wire.script(TARGET, status(200));
    let (app, subscription, secret) = with_subscription("alpha", &resolver, &wire);
    let run = start_notify(&app, "alpha", 7, T0);
    let pass = worker::tick(&app, "w1", &config(), &at(T0));
    assert_eq!(
        pass.jobs, 4,
        "the start, the step, the delivery and the end"
    );
    let run_record = app
        .store
        .automation_run(&tenant_id("alpha"), &run)
        .unwrap()
        .unwrap();
    assert_eq!(run_record.state, RunState::Completed);
    // The step's typed result names the delivery, queued.
    let steps = app
        .store
        .automation_steps(&tenant_id("alpha"), &run, None, 10)
        .unwrap();
    let result = steps[0].result.clone().unwrap();
    let delivery = result["CallWebhook"]["delivery"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(result["CallWebhook"]["status"], 0);
    assert_eq!(
        run_record.params.get("call_delivery"),
        Some(&Value::Str(delivery.clone()))
    );
    // One signed POST to the judged address.
    let sent = posts(&wire);
    assert_eq!(sent.len(), 1);
    let post = &sent[0];
    assert_eq!(post.method, Method::Post);
    assert_eq!(post.address, "93.184.216.34:443".parse().unwrap());
    assert_eq!(
        post.header(signature::HEADER_EVENT),
        Some(delivery.as_str())
    );
    let timestamp: i64 = post
        .header(signature::HEADER_TIMESTAMP)
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(timestamp, T0 / 1000);
    assert!(signature::verify(
        &[secret.as_bytes().to_vec()],
        &delivery,
        timestamp,
        &post.body,
        post.header(signature::HEADER_SIGNATURE).unwrap()
    ));
    let body: Json = serde_json::from_slice(&post.body).unwrap();
    assert_eq!(body, json!({ "event": "note-made", "seq": 7 }));
    // The owner's log: status, attempts, and the target without its query.
    let log = deliveries_of(&app, "alpha");
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["id"], delivery.as_str());
    assert_eq!(log[0]["status"], 200);
    assert_eq!(log[0]["attempts"], 1);
    assert_eq!(log[0]["state"], "delivered");
    assert_eq!(log[0]["subscription"], subscription.as_str());
    assert_eq!(log[0]["endpoint"], "https://hooks.example/in");
}

// Verifies: WFL-2643
// Verifies: INT-152
#[test]
fn a_failed_delivery_is_retried_with_backoff_and_dead_lettered_after_three() {
    let resolver = Resolver::with(&[("hooks.example", &["93.184.216.34"])]);
    let wire = Wire::new();
    wire.script(TARGET, status(500));
    wire.script(TARGET, status(503));
    wire.script(TARGET, status(200));
    let (app, _, _) = with_subscription("alpha", &resolver, &wire);
    start_notify(&app, "alpha", 1, T0);
    worker::tick(&app, "w1", &config(), &at(T0));
    let log = deliveries_of(&app, "alpha");
    assert_eq!(log[0]["state"], "retrying");
    assert_eq!(log[0]["status"], 500);
    assert_eq!(log[0]["attempts"], 1);
    // Not before five seconds.
    assert_eq!(worker::tick(&app, "w1", &config(), &at(T0 + 4_999)).jobs, 0);
    worker::tick(&app, "w1", &config(), &at(T0 + 5_000));
    assert_eq!(deliveries_of(&app, "alpha")[0]["attempts"], 2);
    // The third attempt waits ten seconds more, and delivers.
    assert_eq!(
        worker::tick(&app, "w1", &config(), &at(T0 + 14_999)).jobs,
        0
    );
    worker::tick(&app, "w1", &config(), &at(T0 + 15_000));
    let log = deliveries_of(&app, "alpha");
    assert_eq!(log[0]["state"], "delivered");
    assert_eq!(log[0]["status"], 200);
    assert_eq!(log[0]["attempts"], 3);
    assert_eq!(posts(&wire).len(), 3);
    // Every attempt names the same event, so a receiver drops a repeat.
    let events: BTreeSet<String> = posts(&wire)
        .iter()
        .map(|p| p.header(signature::HEADER_EVENT).unwrap().to_string())
        .collect();
    assert_eq!(events.len(), 1);

    // A receiver that keeps failing: three attempts, then dead-lettered.
    let wire = Wire::new();
    wire.script(TARGET, status(502));
    let (app, _, _) = with_subscription("beta", &resolver, &wire);
    start_notify(&app, "beta", 1, T0);
    for t in [T0, T0 + 5_000, T0 + 15_000, T0 + 60_000] {
        worker::tick(&app, "w1", &config(), &at(t));
    }
    let log = deliveries_of(&app, "beta");
    assert_eq!(log[0]["state"], "failed");
    assert_eq!(log[0]["attempts"], 3);
    assert_eq!(log[0]["outcome"], "status 502");
    assert_eq!(posts(&wire).len(), 3);
    let dead = app
        .store
        .automation_exceptions(
            &tenant_id("beta"),
            Some(ExceptionKind::DeadLettered),
            false,
            None,
            10,
        )
        .unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].reason, "attempts-exhausted");

    // A receiver that refuses (4xx) is not tried again; a refused address is not tried at all.
    let wire = Wire::new();
    wire.script(TARGET, status(410));
    let (app, _, _) = with_subscription("gamma", &resolver, &wire);
    start_notify(&app, "gamma", 1, T0);
    for t in [T0, T0 + 5_000, T0 + 15_000] {
        worker::tick(&app, "w1", &config(), &at(t));
    }
    assert_eq!(posts(&wire).len(), 1);
    assert_eq!(deliveries_of(&app, "gamma")[0]["state"], "failed");
}

// Verifies: WFL-1579
#[test]
fn a_subscription_to_an_address_outside_the_rule_is_refused() {
    let resolver = Resolver::with(&[]);
    let wire = Wire::new();
    let app = app_with(&resolver, &wire);
    install(&app, "alpha", Vec::new());
    let owner = signed_in("alpha", "owner");
    for target in [
        "http://hooks.example/in",
        "https://hooks.example:8080/in",
        "https://user:pw@hooks.example/in",
        "https://127.0.0.1/in",
        "https://0x7f.1/in",
        "https://localhost/in",
        "https://[::1]/in",
    ] {
        let refused = deliveries::create_subscription(&app, &owner, &json!({ "target": target }))
            .unwrap_err();
        assert_eq!(refused.0.code, ErrorCode::Invalid, "{target}");
    }
    // The secret is returned once and kept sealed.
    let created =
        deliveries::create_subscription(&app, &owner, &json!({ "target": TARGET })).unwrap();
    let secret = created["secret"].as_str().unwrap();
    assert!(secret.starts_with("whsec_") && secret.len() == 6 + 64);
    let stored = app
        .store
        .doc_get(
            &tenant_id("alpha"),
            deliveries::SUBSCRIPTION_KIND,
            created["subscription"].as_str().unwrap(),
        )
        .unwrap()
        .unwrap();
    assert!(!stored.contains(secret));
}
