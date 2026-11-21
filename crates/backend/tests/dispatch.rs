//! The mail dispatcher: a templated send arrives once, suppression is per category, the allowance
//! holds bulk mail before transactional mail and never sign-in mail, a line break in a header is
//! refused, a send to a segment reaches each member once across its batches, an automation's
//! send-mail step sends once, and one tenant sees nothing of another's templates, suppressions or
//! log. Driven synchronously through `worker::tick` with an injected clock, on the in-memory
//! store, with the development sink.

use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use strata_backend::app::{App, Config, Shared};
use strata_backend::auth::Identity;
use strata_backend::dispatch::links::{self, LinkAction, LinkClaim};
use strata_backend::dispatch::reports::{self, DeliveryReport, ReportKind, ReportOutcome};
use strata_backend::dispatch::{
    self, policy, routes, store, AllowanceClass, MessageState, Recipient, SendInput,
};
use strata_backend::inbox;
use strata_backend::mail::{DevSink, Mail, MailError, Mailer};
use strata_backend::worker::{self, WorkerConfig};
use strata_kernel::segments::{Definition, SegmentId};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model::types::TemplateId;
use strata_protocol::api::ErrorCode;
use strata_protocol::ws::SessionState;
use strata_storage_api::jobs::{JobWork, NewJob, RetryPolicy, RunState};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, segments as kept, Account, SessionRecord, Store};

/// 2026-11-20T00:00:00Z: the allowance counts calendar months.
const T0: i64 = 1_795_132_800_000;
const GOLD: &str = "6a0f3c1e-2280-4000-8000-000000000001";
const ADA: &str = "ada@example.org";

fn app_with(mail: Arc<dyn Mailer>) -> Shared {
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
        mail,
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    };
    App::new(cfg, Arc::new(MemoryStore::new()))
}

fn app() -> Shared {
    app_with(Arc::new(DevSink::memory()))
}

/// A transport that refuses its first message for a temporary reason and then keeps what it is
/// given, as the development sink does.
#[derive(Debug, Default)]
struct Flaky {
    refused: AtomicU32,
    sink: DevSink,
}

impl Mailer for Flaky {
    fn send(&self, mail: &Mail) -> Result<(), MailError> {
        if self.refused.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(MailError("the mail queue is full".into()));
        }
        self.sink.send(mail)
    }

    fn outbox(&self) -> Vec<Mail> {
        self.sink.outbox()
    }
}

/// The body of `welcome`: "Hello " and the value `name`, placed as text.
const HELLO: &str = r#"[{"type":"paragraph","inlines":[{"type":"text","text":"Hello "},{"type":"value","name":"name","value_type":"str"}]}]"#;
/// The body of `plain`, which places no value.
const PLAIN: &str = r#"[{"type":"paragraph","inlines":[{"type":"text","text":"Hello there"}]}]"#;

fn template(id: &str, subject: &str) -> SchemaChange {
    SchemaChange::DefineMailTemplate(MailTemplateDecl {
        template: TemplateId(id.into()),
        subject: subject.into(),
        body: StructuredBlocks(if id == "plain" { PLAIN } else { HELLO }.into()),
    })
}

/// `notify`: sends the template `plain` to Ada in the category `receipts`, run as a writer,
/// who holds messages-send.
fn notify() -> AutomationDef {
    let send = StepName::new("send").unwrap();
    AutomationDef {
        state: None,
        id: WorkflowId::new("notify").unwrap(),
        label: "notify".into(),
        run_as: RunAs {
            roles: BTreeSet::from([role("writer")]),
        },
        run_key: None,
        start: send.clone(),
        steps: BTreeMap::from([(
            send,
            AutomationStepDef {
                action: AutomationStep::SendMail {
                    template: TemplateId("plain".into()),
                    to: Some(lit(ADA)),
                    address: None,
                    segment: None,
                    category: Some("receipts".into()),
                },
                next: None,
                retry: None,
                timeout: None,
                compensate: None,
            },
        )]),
        defined_by: None,
        defined_at: None,
        suspended_for: None,
    }
}

/// A tenant with the notes schema, a `member` type for segments, the template `welcome` (its
/// subject `subject`), writers who may send, and the categories `receipts` (transactional),
/// `news` and `offers` (promotional). Alpha also has the template `alpha_only` and the
/// automation `notify`.
fn install(app: &App, tenant: &str, subject: &str) {
    let store = app.store.as_ref();
    let owner = principal_id(&format!("{tenant}:owner"));
    let mut now = 100;
    let mut run = |principal: Principal, intent: Intent| {
        now += 1;
        execute(store, &intent, &ctx(tenant, principal, now)).expect("fixture intent");
    };
    run(
        Principal::System,
        Intent::CreateTenant {
            name: tenant.to_string(),
            owner: owner.clone(),
        },
    );
    let mut changes = notes_schema();
    changes.extend([
        SchemaChange::DefineObject {
            id: object("member"),
            label: "Member".into(),
        },
        SchemaChange::DefineField {
            object: object("member"),
            field: FieldDef {
                unique: true,
                ..field_def("principal", FieldType::Str, true)
            },
        },
        SchemaChange::DefineField {
            object: object("member"),
            field: field_def("tier", FieldType::Str, false),
        },
        template("welcome", subject),
        template("plain", "Hello there"),
        SchemaChange::DefineRolePermissions {
            role: role("writer"),
            permissions: vec![Permission::Capability("messages-send".into())],
        },
    ]);
    if tenant == "alpha" {
        changes.push(template("alpha_only", "Only alpha"));
        changes.push(SchemaChange::DefineAutomation(notify()));
    }
    run(
        Principal::User(owner),
        Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    let owner = signed_in(tenant, "owner");
    for (id, purpose) in [
        ("receipts", "transactional"),
        ("news", "promotional"),
        ("offers", "promotional"),
    ] {
        routes::define_category(
            app,
            &owner,
            &json!({ "category": id, "purpose": purpose, "name": id }),
        )
        .expect("the category is declared");
    }
}

fn signed_in(tenant: &str, who: &str) -> Identity {
    let principal = principal_id(&format!("{tenant}:{who}"));
    Identity {
        tenant: tenant_id(tenant),
        principal: Principal::User(principal.clone()),
        session: Some(SessionRecord {
            id: SessionId::new(format!("s-{who}")).unwrap(),
            token_hash: format!("hash-{who}"),
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
        correlation: "t-mail".into(),
        verified_address: None,
        idempotency_key: None,
    }
}

fn visitor(tenant: &str) -> Identity {
    Identity {
        tenant: tenant_id(tenant),
        principal: Principal::Anonymous,
        session: None,
        state: SessionState::Anonymous,
        correlation: "t-mail-visitor".into(),
        verified_address: None,
        idempotency_key: None,
    }
}

fn address(text: &str) -> Recipient {
    Recipient::Address {
        address: text.to_string(),
    }
}

fn input(to: Recipient, category: Option<&str>, key: &str) -> SendInput {
    SendInput {
        template: TemplateId("welcome".into()),
        recipient: to,
        category: category.map(str::to_string),
        class: None,
        idempotency_key: key.to_string(),
        values: BTreeMap::from([("name".to_string(), "Ada".to_string())]),
        reference: Some("autumn".into()),
    }
}

fn send(
    app: &App,
    tenant: &str,
    to: Recipient,
    category: Option<&str>,
    key: &str,
) -> Result<dispatch::Submitted, strata_backend::app::ApiError> {
    dispatch::submit(
        app,
        &tenant_id(tenant),
        &input(to, category, key),
        Timestamp(T0),
    )
}

fn at(t: i64) -> impl Fn() -> Timestamp {
    move || Timestamp(t)
}

/// Run the worker at `t` until no job is due.
fn drain(app: &App, t: i64) {
    let config = WorkerConfig::from_limits(&Default::default());
    for _ in 0..200 {
        if worker::tick(app, "w1", &config, &at(t)).jobs == 0 {
            return;
        }
    }
    panic!("the worker did not run dry");
}

fn outbox_to(app: &App) -> Vec<String> {
    app.cfg.mail.outbox().into_iter().map(|m| m.to).collect()
}

fn state(app: &App, tenant: &str, message: &str) -> MessageState {
    store::message(app, &tenant_id(tenant), message)
        .unwrap()
        .expect("the message is in the log")
        .state
}

fn message_of(done: &dispatch::Submitted) -> String {
    done.message
        .clone()
        .expect("a send to one recipient has a message")
}

// Verifies: EML-006
#[test]
fn a_templated_send_arrives_once_under_retry() {
    let flaky = Arc::new(Flaky::default());
    let app = app_with(flaky.clone());
    install(&app, "alpha", "Hello {{name}}");
    let first = send(&app, "alpha", address(ADA), Some("receipts"), "k-1").unwrap();
    // The same key with the same inputs is the same send; with other inputs it is refused.
    let again = send(&app, "alpha", address(ADA), Some("receipts"), "k-1").unwrap();
    assert_eq!(first, again);
    let other = send(
        &app,
        "alpha",
        address("bob@example.org"),
        Some("receipts"),
        "k-1",
    );
    assert_eq!(other.unwrap_err().0.code, ErrorCode::Conflict);

    // The first attempt is refused for a temporary reason: nothing arrives, and the message waits.
    drain(&app, T0);
    assert!(outbox_to(&app).is_empty());
    let message = message_of(&first);
    assert_eq!(state(&app, "alpha", &message), MessageState::Submitted);
    // The retry, after the first delay of the policy, delivers it.
    drain(&app, T0 + 10_000);
    let sent = app.cfg.mail.outbox();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, ADA);
    assert_eq!(sent[0].subject, "Hello Ada");
    assert_eq!(sent[0].text, "Hello Ada");
    assert_eq!(sent[0].html.as_deref(), Some("<p>Hello Ada</p>"));
    let record = store::message(&app, &tenant_id("alpha"), &message)
        .unwrap()
        .unwrap();
    assert_eq!(record.state, MessageState::Sent);
    assert_eq!(record.attempts, 2);
    assert_eq!(record.reference.as_deref(), Some("autumn"));

    // Another job carrying the same request (a report that was lost, a repeat) sends nothing more.
    let job = app
        .store
        .job(
            &tenant_id("alpha"),
            &strata_storage_api::jobs::job_id(
                &tenant_id("alpha"),
                &dispatch::job_key(&dispatch::key_hash("k-1")),
            ),
        )
        .unwrap()
        .expect("the job of the send");
    let JobWork::Mail { request } = job.work else {
        panic!("not a mail job: {:?}", job.work);
    };
    app.store
        .jobs_create(
            &tenant_id("alpha"),
            &[NewJob {
                key: "mail:repeat".into(),
                work: JobWork::Mail { request },
                due_at: Timestamp(T0 + 20_000),
                policy: RetryPolicy::default(),
            }],
            Timestamp(T0 + 20_000),
        )
        .unwrap();
    drain(&app, T0 + 20_000);
    assert_eq!(app.cfg.mail.outbox().len(), 1);
}

fn link(app: &App, tenant: &str, category: &str, action: LinkAction) -> serde_json::Value {
    let query = links::query(
        app,
        &tenant_id(tenant),
        &LinkClaim {
            address_hash: store::address_hash(ADA),
            category: category.to_string(),
            action,
        },
    );
    json!({ "address": format!("https://alpha.example/api/mail/subscription?{query}") })
}

// Verifies: EML-1562
#[test]
fn suppression_is_per_category() {
    let app = app();
    install(&app, "alpha", "Hello {{name}}");
    let tenant = tenant_id("alpha");

    // A promotional message carries the unsubscribe header and link (EML-020).
    send(&app, "alpha", address(ADA), Some("news"), "k-news-0").unwrap();
    drain(&app, T0);
    let first = app
        .cfg
        .mail
        .outbox()
        .pop()
        .expect("the newsletter was sent");
    assert!(first.headers.iter().any(
        |(name, value)| name == "List-Unsubscribe" && value.contains("/api/mail/subscription?")
    ));
    assert!(first.text.contains("/mail/subscription?"));

    // Ada unsubscribes from every promotional category, by a link and with no sign-in.
    let done = routes::subscription(
        &app,
        &visitor("alpha"),
        &link(
            &app,
            "alpha",
            store::ALL_PROMOTIONAL,
            LinkAction::Unsubscribe,
        ),
    )
    .unwrap();
    assert_eq!(done["state"], "unsubscribed");
    // A forged link is not found.
    let mut forged = link(
        &app,
        "alpha",
        store::ALL_PROMOTIONAL,
        LinkAction::Resubscribe,
    );
    forged["address"] = json!(forged["address"]
        .as_str()
        .unwrap()
        .replace("a=resubscribe", "a=unsubscribe"));
    assert_eq!(
        routes::subscription(&app, &visitor("alpha"), &forged)
            .unwrap_err()
            .0
            .code,
        ErrorCode::NotFound
    );

    let before = app.cfg.mail.outbox().len();
    let news = send(&app, "alpha", address(ADA), Some("news"), "k-news").unwrap();
    let offers = send(&app, "alpha", address(ADA), Some("offers"), "k-offers").unwrap();
    let receipt = send(&app, "alpha", address(ADA), Some("receipts"), "k-receipt").unwrap();
    drain(&app, T0);
    assert_eq!(
        state(&app, "alpha", &message_of(&news)),
        MessageState::Suppressed
    );
    assert_eq!(
        state(&app, "alpha", &message_of(&offers)),
        MessageState::Suppressed
    );
    assert_eq!(
        state(&app, "alpha", &message_of(&receipt)),
        MessageState::Sent
    );
    // Sign-in mail is not stopped by a promotional unsubscribe either.
    dispatch::send_sign_in(
        &app,
        &tenant,
        &Mail {
            to: ADA.into(),
            subject: "Your sign-in link".into(),
            text: "link".into(),
            ..Mail::default()
        },
    )
    .unwrap();
    assert_eq!(app.cfg.mail.outbox().len(), before + 2);

    // Resubscribing removes the unsubscribe, and the newsletter is sent again.
    routes::subscription(
        &app,
        &visitor("alpha"),
        &link(
            &app,
            "alpha",
            store::ALL_PROMOTIONAL,
            LinkAction::Resubscribe,
        ),
    )
    .unwrap();
    let news = send(&app, "alpha", address(ADA), Some("news"), "k-news-2").unwrap();
    drain(&app, T0);
    assert_eq!(state(&app, "alpha", &message_of(&news)), MessageState::Sent);

    // A complaint about a receipt suppresses the receipts only (EML-017).
    let complaint = DeliveryReport {
        event: "ev-1".into(),
        reference: store::provider_reference(&tenant, &message_of(&receipt)),
        kind: ReportKind::Complaint,
        at: Timestamp(T0),
    };
    assert_eq!(
        reports::apply(&app, &complaint, Timestamp(T0)).unwrap(),
        ReportOutcome::Applied
    );
    // The same event again changes nothing (EML-1567).
    assert_eq!(
        reports::apply(&app, &complaint, Timestamp(T0)).unwrap(),
        ReportOutcome::Seen
    );
    let receipt2 = send(&app, "alpha", address(ADA), Some("receipts"), "k-receipt-2").unwrap();
    let news3 = send(&app, "alpha", address(ADA), Some("news"), "k-news-3").unwrap();
    drain(&app, T0);
    assert_eq!(
        state(&app, "alpha", &message_of(&receipt2)),
        MessageState::Suppressed
    );
    assert_eq!(
        state(&app, "alpha", &message_of(&news3)),
        MessageState::Sent
    );

    // A permanent bounce suppresses every category, sign-in mail included.
    let bounce = DeliveryReport {
        event: "ev-2".into(),
        reference: store::provider_reference(&tenant, &message_of(&news3)),
        kind: ReportKind::Bounce { permanent: true },
        at: Timestamp(T0 + 1_000),
    };
    assert_eq!(
        reports::apply(&app, &bounce, Timestamp(T0)).unwrap(),
        ReportOutcome::Applied
    );
    // A report from outside the five-minute window is refused.
    let stale = DeliveryReport {
        event: "ev-3".into(),
        at: Timestamp(T0 - 400_000),
        ..bounce.clone()
    };
    assert_eq!(
        reports::apply(&app, &stale, Timestamp(T0)).unwrap(),
        ReportOutcome::Stale
    );
    let sent = app.cfg.mail.outbox().len();
    dispatch::send_sign_in(
        &app,
        &tenant,
        &Mail {
            to: ADA.into(),
            subject: "Your sign-in link".into(),
            text: "link".into(),
            ..Mail::default()
        },
    )
    .unwrap();
    assert_eq!(app.cfg.mail.outbox().len(), sent);
}

// Verifies: EML-1563
#[test]
fn the_allowance_holds_bulk_at_eighty_percent_and_transactional_at_the_whole() {
    let app = app();
    install(&app, "alpha", "Hello {{name}}");
    let tenant = tenant_id("alpha");
    store::set_allowance_limit(&app, &tenant, 10).unwrap();
    let bulk: Vec<String> = (0..10)
        .map(|i| {
            message_of(
                &send(
                    &app,
                    "alpha",
                    address(&format!("b{i}@example.org")),
                    Some("news"),
                    &format!("k-bulk-{i}"),
                )
                .unwrap(),
            )
        })
        .collect();
    drain(&app, T0);
    let held = |ids: &[String]| {
        ids.iter()
            .filter(|id| state(&app, "alpha", id) == MessageState::Held)
            .count()
    };
    assert_eq!(app.cfg.mail.outbox().len(), 8, "bulk mail stops at 80%");
    assert_eq!(held(&bulk), 2);

    // Transactional mail has the rest of the allowance, and no more.
    let receipts: Vec<String> = (0..3)
        .map(|i| {
            message_of(
                &send(
                    &app,
                    "alpha",
                    address(&format!("r{i}@example.org")),
                    Some("receipts"),
                    &format!("k-receipt-{i}"),
                )
                .unwrap(),
            )
        })
        .collect();
    drain(&app, T0);
    assert_eq!(app.cfg.mail.outbox().len(), 10);
    assert_eq!(held(&receipts), 1);

    // Sign-in mail is never held.
    dispatch::send_sign_in(
        &app,
        &tenant,
        &Mail {
            to: ADA.into(),
            subject: "Your sign-in link".into(),
            text: "link".into(),
            ..Mail::default()
        },
    )
    .unwrap();
    assert_eq!(app.cfg.mail.outbox().len(), 11);
    // The class is the platform's alone.
    let mut sign_in = input(address(ADA), Some("receipts"), "k-sign-in");
    sign_in.class = Some(AllowanceClass::SignIn);
    assert_eq!(
        dispatch::submit(&app, &tenant, &sign_in, Timestamp(T0))
            .unwrap_err()
            .0
            .code,
        ErrorCode::Invalid
    );

    // In the next month the held messages leave, each once.
    drain(&app, policy::next_period_start(T0));
    assert_eq!(app.cfg.mail.outbox().len(), 14);
    assert_eq!(held(&bulk) + held(&receipts), 0);
}

// Verifies: EML-1566
#[test]
fn a_line_break_in_a_header_is_refused_before_any_job() {
    let app = app();
    install(&app, "alpha", "Hello {{name}}");
    let tenant = tenant_id("alpha");
    let jobs_before = app.store.jobs_list(&tenant, None, 100).unwrap().len();
    let injected = send(
        &app,
        "alpha",
        address("ada@example.org\r\nBcc: eve@example.org"),
        Some("receipts"),
        "k-1",
    );
    assert_eq!(injected.unwrap_err().0.code, ErrorCode::Invalid);
    let two = send(
        &app,
        "alpha",
        address("ada@example.org, eve@example.org"),
        Some("receipts"),
        "k-2",
    );
    assert_eq!(two.unwrap_err().0.code, ErrorCode::Invalid);
    // A value that puts a line break into the rendered subject.
    let mut subject = input(address(ADA), Some("receipts"), "k-3");
    subject
        .values
        .insert("name".into(), "Ada\nBcc: eve@example.org".into());
    assert_eq!(
        dispatch::submit(&app, &tenant, &subject, Timestamp(T0))
            .unwrap_err()
            .0
            .code,
        ErrorCode::Invalid
    );
    assert_eq!(
        app.store.jobs_list(&tenant, None, 100).unwrap().len(),
        jobs_before
    );
    // The same for sign-in mail.
    let refused = dispatch::send_sign_in(
        &app,
        &tenant,
        &Mail {
            to: ADA.into(),
            subject: "Hello\r\nBcc: eve@example.org".into(),
            text: "link".into(),
            ..Mail::default()
        },
    );
    assert!(refused.is_err());
    drain(&app, T0);
    assert!(app.cfg.mail.outbox().is_empty());
}

// Verifies: EML-1561
#[test]
fn a_segment_send_reaches_each_member_once_across_batches() {
    let app = app();
    install(&app, "alpha", "Hello {{name}}");
    let tenant = tenant_id("alpha");
    let store_ = app.store.as_ref();
    let owner = Principal::User(principal_id("alpha:owner"));
    let segment: SegmentId = serde_json::from_value(json!(GOLD)).unwrap();
    kept::define(
        store_,
        &ctx("alpha", owner.clone(), 1_000),
        &Definition {
            id: segment,
            name: "Gold".into(),
            subject: object("member"),
            predicate: expr::parse("record.tier == 'gold'").unwrap(),
            refresh_seconds: None,
            principal_field: field("principal"),
            visible_to_members: false,
        },
    )
    .expect("the segment is defined");
    for i in 1..=150 {
        let person = format!("alpha:p{i:03}");
        execute(
            store_,
            &Intent::CreateRecord {
                object: object("member"),
                fields: BTreeMap::from([
                    (field("principal"), text(&person)),
                    (field("tier"), text("gold")),
                ]),
            },
            &ctx("alpha", owner.clone(), 2_000 + i),
        )
        .expect("the member is created");
        store_
            .put_account(&Account {
                tenant: tenant.clone(),
                login: format!("p{i:03}@example.org"),
                principal: principal_id(&person),
                display_name: person.clone(),
                password_hash: "unused".into(),
                verified_email: Some(format!("p{i:03}@example.org")),
            })
            .unwrap();
    }
    // One member has unsubscribed from the newsletter.
    routes::manage_suppression(
        &app,
        &signed_in("alpha", "owner"),
        &json!({ "address": "p007@example.org", "action": "add", "category": "news" }),
    )
    .unwrap();

    let segment_send = Recipient::Segment {
        segment,
        after: None,
    };
    let done = send(&app, "alpha", segment_send.clone(), Some("news"), "k-gold").unwrap();
    assert_eq!(done.recipients, 150);
    assert_eq!(done.batches, 2);
    assert_eq!(
        done.message, None,
        "the sender learns counts, not recipients"
    );
    drain(&app, T0);
    // A repeat of the send sends nothing more.
    send(&app, "alpha", segment_send, Some("news"), "k-gold").unwrap();
    drain(&app, T0);

    let sent = outbox_to(&app);
    let distinct: BTreeSet<&String> = sent.iter().collect();
    assert_eq!(sent.len(), 149);
    assert_eq!(distinct.len(), 149);
    assert!(!distinct.contains(&"p007@example.org".to_string()));
    let log = store::messages(&app, &tenant).unwrap();
    let of_send: Vec<_> = log
        .iter()
        .filter(|m| m.send.as_deref() == Some(done.send.as_str()))
        .collect();
    assert_eq!(of_send.len(), 150);
    assert_eq!(
        of_send
            .iter()
            .filter(|m| m.state == MessageState::Suppressed)
            .count(),
        1
    );
    // The owner counts the send by its reference.
    let counts = routes::count(
        &app,
        &signed_in("alpha", "owner"),
        &json!({ "group-by": "reference", "reference": "autumn" }),
    )
    .unwrap();
    assert_eq!(
        counts["counts"],
        json!([
            { "group": "autumn", "state": "sent", "count": 149 },
            { "group": "autumn", "state": "suppressed", "count": 1 },
        ])
    );
}

/// Start a run of `automation`, due at `due`.
fn start(app: &App, tenant: &str, automation: &str, due: i64) -> String {
    let job = NewJob {
        key: format!("start:test:{automation}"),
        work: JobWork::StartRun {
            trigger: None,
            automation: WorkflowId::new(automation).unwrap(),
            event_seq: Some(1),
            hop: 0,
            params: BTreeMap::new(),
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

// Verifies: EML-1560
#[test]
fn an_automations_send_mail_step_sends_once() {
    let app = app();
    install(&app, "alpha", "Hello {{name}}");
    let tenant = tenant_id("alpha");
    let run = start(&app, "alpha", "notify", T0);
    drain(&app, T0);
    let record = app
        .store
        .automation_run(&tenant, &run)
        .unwrap()
        .expect("the run");
    assert_eq!(record.state, RunState::Completed);
    let steps = app.store.automation_steps(&tenant, &run, None, 10).unwrap();
    let result = steps[0].result.clone().expect("the step's typed result");
    let message = result["SendMail"]["message"]
        .as_str()
        .expect("the result names the message")
        .to_string();
    assert_eq!(state(&app, "alpha", &message), MessageState::Sent);
    assert_eq!(outbox_to(&app), vec![ADA.to_string()]);

    // An attempt of the step made again (a crash between doing and reporting) names the same mail
    // job and sends nothing more.
    let key = worker::steps::step_key(
        &tenant,
        &WorkflowId::new("notify").unwrap(),
        &run,
        &StepName::new("send").unwrap(),
        steps[0].sequence,
    );
    let again = dispatch::step_send(
        &app,
        &tenant,
        &WorkflowId::new("notify").unwrap(),
        &TemplateId("plain".into()),
        &text(ADA),
        Some("receipts"),
        &key,
        Timestamp(T0 + 1),
    )
    .unwrap();
    assert_eq!(again, message);
    drain(&app, T0 + 1);
    assert_eq!(app.cfg.mail.outbox().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// The mail conformance table, against the local adapter (EML-001)
// ---------------------------------------------------------------------------------------------

use strata_backend::dispatch::adapter::{LocalAdapter, Refusal, ReportRefusal};
use strata_storage_api::{Commit, StoreError};

const CREDENTIAL: &str = "local-credential";
const REPORT_KEY: &[u8] = b"local-report-signing-key";

/// An app that sends through a local adapter with the given size limit, and the adapter.
fn with_adapter(limit: usize) -> (Shared, Arc<LocalAdapter>) {
    let adapter = Arc::new(LocalAdapter::new(CREDENTIAL, REPORT_KEY).with_limit(limit));
    let app = app_with(adapter.clone());
    install(&app, "alpha", "Hello {{name}}");
    (app, adapter)
}

/// A message to Ada that the dispatcher has sent: its id.
fn sent_message(app: &App) -> String {
    let done = send(app, "alpha", address(ADA), Some("receipts"), "k-sent").unwrap();
    drain(app, T0);
    let id = message_of(&done);
    assert_eq!(state(app, "alpha", &id), MessageState::Sent);
    id
}

fn plain_mail(subject: &str, text: &str) -> Mail {
    Mail {
        to: ADA.into(),
        subject: subject.into(),
        text: text.into(),
        ..Mail::default()
    }
}

/// The rows of the table `cases-mail-conformance` in the text of the specification, first column.
fn spec_mail_cases() -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specifications/spec.jsonc");
    let spec = strata_spec_parts::read_spec_text(&path).expect("the specification file");
    let line = spec
        .lines()
        .find(|line| {
            line.trim_start()
                .starts_with("{\"id\":\"cases-mail-conformance\"")
        })
        .expect("the specification has a cases-mail-conformance section");
    let section: serde_json::Value =
        serde_json::from_str(line.trim().trim_end_matches(',')).expect("the section is JSON");
    section["body"]
        .as_str()
        .expect("the section has a body")
        .lines()
        .filter(|row| row.starts_with('|'))
        .skip(2)
        .filter_map(|row| row.trim_matches('|').split('|').next())
        .map(|cell| cell.trim().to_string())
        .collect()
}

/// Run one row of the table against a fresh local adapter, directly and through the dispatcher.
fn mail_case(case: &str) {
    let tenant = tenant_id("alpha");
    match case {
        "accepted" => {
            let (app, adapter) = with_adapter(4_096);
            let first = adapter.submit("key-1", &plain_mail("Hi", "text")).unwrap();
            assert_eq!(first.reference, "local-1");
            assert!(!first.repeated);
            // Through the dispatcher the message is sent and the log holds its reference.
            let id = sent_message(&app);
            let record = store::message(&app, &tenant, &id).unwrap().unwrap();
            assert_eq!(
                record.provider_reference,
                Some(store::provider_reference(&tenant, &id))
            );
            assert_eq!(adapter.caught().len(), 2);
        }
        "repeated-key" => {
            let (app, adapter) = with_adapter(4_096);
            let first = adapter.submit("key-1", &plain_mail("Hi", "text")).unwrap();
            let again = adapter.submit("key-1", &plain_mail("Hi", "text")).unwrap();
            assert_eq!(again.reference, first.reference);
            assert!(again.repeated);
            assert_eq!(adapter.caught().len(), 1, "no second delivery");
            // The same send submitted twice to the dispatcher is one message at the adapter.
            send(&app, "alpha", address(ADA), Some("receipts"), "k-once").unwrap();
            send(&app, "alpha", address(ADA), Some("receipts"), "k-once").unwrap();
            drain(&app, T0);
            assert_eq!(adapter.caught().len(), 2);
        }
        "temporary-refusal" => {
            let (app, adapter) = with_adapter(4_096);
            adapter.refuse_next(1, false);
            assert!(matches!(
                adapter.submit("probe", &plain_mail("Hi", "text")),
                Err(Refusal::Temporary(_))
            ));
            adapter.refuse_next(1, false);
            let done = send(&app, "alpha", address(ADA), Some("receipts"), "k-temp").unwrap();
            drain(&app, T0);
            // Reported as temporary: the message waits, and the later attempt delivers it.
            assert_eq!(
                state(&app, "alpha", &message_of(&done)),
                MessageState::Submitted
            );
            assert!(adapter.caught().is_empty());
            drain(&app, T0 + 10_000);
            assert_eq!(state(&app, "alpha", &message_of(&done)), MessageState::Sent);
            assert_eq!(adapter.caught().len(), 1);
        }
        "permanent-refusal" => {
            let (app, adapter) = with_adapter(4_096);
            adapter.reject_address(ADA);
            assert!(matches!(
                adapter.submit("probe", &plain_mail("Hi", "text")),
                Err(Refusal::Permanent(_))
            ));
            let done = send(&app, "alpha", address(ADA), Some("receipts"), "k-perm").unwrap();
            drain(&app, T0);
            let record = store::message(&app, &tenant, &message_of(&done))
                .unwrap()
                .unwrap();
            assert_eq!(record.state, MessageState::Failed);
            assert!(record
                .fail_reason
                .as_deref()
                .unwrap_or_default()
                .contains("the mailbox does not exist"));
            assert!(adapter.caught().is_empty());
        }
        "provider-unreachable" => {
            let (app, adapter) = with_adapter(4_096);
            // The provider takes the message and its answer is lost: reported as temporary, and
            // the retry under the same key delivers once.
            adapter.refuse_next(1, true);
            let done = send(&app, "alpha", address(ADA), Some("receipts"), "k-dark").unwrap();
            drain(&app, T0);
            assert_eq!(
                state(&app, "alpha", &message_of(&done)),
                MessageState::Submitted
            );
            drain(&app, T0 + 10_000);
            assert_eq!(state(&app, "alpha", &message_of(&done)), MessageState::Sent);
            assert_eq!(adapter.caught().len(), 1, "delivered once");
        }
        "oversized" => {
            let (app, adapter) = with_adapter(300);
            let big = plain_mail("Hi", &"x".repeat(400));
            assert_eq!(adapter.submit("big", &big), Err(Refusal::Oversized));
            assert!(
                adapter.caught().is_empty(),
                "refused before anything is sent"
            );
            // A template message above the limit fails for good and nothing is caught.
            let mut long = input(address(ADA), Some("receipts"), "k-big");
            long.values.insert("name".to_string(), "A".repeat(400));
            let done = dispatch::submit(&app, &tenant, &long, Timestamp(T0)).unwrap();
            drain(&app, T0);
            assert_eq!(
                state(&app, "alpha", &message_of(&done)),
                MessageState::Failed
            );
            assert!(adapter.caught().is_empty());
        }
        "header-break" => {
            let (app, adapter) = with_adapter(4_096);
            assert!(matches!(
                adapter.submit("hb", &plain_mail("Hi\r\nBcc: x@y.example", "text")),
                Err(Refusal::HeaderBreak(_))
            ));
            let mut to_break = plain_mail("Hi", "text");
            to_break.to = "a@x.example\nBcc: b@y.example".into();
            assert!(matches!(
                adapter.submit("hb2", &to_break),
                Err(Refusal::HeaderBreak(_))
            ));
            assert!(adapter.caught().is_empty());
            // The dispatcher refuses the send before a job exists.
            let mut breaking = input(address(ADA), Some("receipts"), "k-hb");
            breaking
                .values
                .insert("name".to_string(), "Ada\r\nBcc: x@y.example".to_string());
            assert_eq!(
                dispatch::submit(&app, &tenant, &breaking, Timestamp(T0))
                    .unwrap_err()
                    .0
                    .code,
                ErrorCode::Invalid
            );
            assert!(adapter.caught().is_empty());
        }
        "bounce-report" | "complaint-report" => {
            let (app, adapter) = with_adapter(4_096);
            let id = sent_message(&app);
            let reference = store::provider_reference(&tenant, &id);
            let (kind, wanted) = if case == "bounce-report" {
                ("bounce", MessageState::Bounced)
            } else {
                ("complaint", MessageState::Complained)
            };
            let (body, signature) = adapter.signed_report("ev-1", &reference, kind, T0);
            let report = adapter.read_report(&body, &signature).unwrap();
            assert_eq!(report.reference, reference, "the event names the message");
            assert_eq!(
                reports::apply(&app, &report, Timestamp(T0)).unwrap(),
                ReportOutcome::Applied
            );
            assert_eq!(state(&app, "alpha", &id), wanted);
        }
        "forged-report" => {
            let (app, adapter) = with_adapter(4_096);
            let id = sent_message(&app);
            let reference = store::provider_reference(&tenant, &id);
            let (body, signature) = adapter.signed_report("ev-1", &reference, "bounce", T0);
            // A signature of another key, and a body changed after it was signed.
            let other = LocalAdapter::new(CREDENTIAL, b"another-key");
            let (_, forged) = other.signed_report("ev-1", &reference, "bounce", T0);
            assert_eq!(
                adapter.read_report(&body, &forged),
                Err(ReportRefusal::BadSignature)
            );
            let altered = String::from_utf8(body.clone())
                .unwrap()
                .replace("bounce", "delivered");
            assert_eq!(
                adapter.read_report(altered.as_bytes(), &signature),
                Err(ReportRefusal::BadSignature)
            );
            // Refused: the message is as it was.
            assert_eq!(state(&app, "alpha", &id), MessageState::Sent);
        }
        "wrong-credential" => {
            let (app, adapter) = with_adapter(4_096);
            adapter.present("a-credential-the-provider-rejects");
            assert!(matches!(
                adapter.submit("probe", &plain_mail("Hi", "text")),
                Err(Refusal::Configuration(_))
            ));
            let done = send(&app, "alpha", address(ADA), Some("receipts"), "k-cred").unwrap();
            drain(&app, T0);
            // A configuration failure: no message is marked failed, and nothing was sent.
            let held = state(&app, "alpha", &message_of(&done));
            assert_ne!(held, MessageState::Failed);
            assert!(!held.is_final());
            assert!(adapter.caught().is_empty());
            // Once the configuration is mended the same message is sent.
            adapter.present(CREDENTIAL);
            drain(&app, T0 + 10_000);
            assert_eq!(state(&app, "alpha", &message_of(&done)), MessageState::Sent);
        }
        other => panic!("the table has a row, {other}, that this test does not run"),
    }
}

// Verifies: EML-001
#[test]
fn every_row_of_cases_mail_conformance_passes_against_the_local_adapter() {
    const RUN: [&str; 11] = [
        "accepted",
        "repeated-key",
        "temporary-refusal",
        "permanent-refusal",
        "provider-unreachable",
        "oversized",
        "header-break",
        "bounce-report",
        "complaint-report",
        "forged-report",
        "wrong-credential",
    ];
    // A row without a check, or a check without a row, fails.
    assert_eq!(
        spec_mail_cases(),
        RUN.iter().map(|case| case.to_string()).collect::<Vec<_>>()
    );
    for case in RUN {
        mail_case(case);
    }
}

// Verifies: EML-005
#[test]
fn a_rolled_back_change_sends_nothing() {
    let app = app();
    install(&app, "alpha", "Hello {{name}}");
    let tenant = tenant_id("alpha");
    let owner = Principal::User(principal_id("alpha:owner"));
    let loaded = app.store.load_complete(&tenant).unwrap();
    let intent = new_note("Mail", "mail");
    let context = ctx("alpha", owner, T0);
    let transition = strata_kernel::transition(&loaded, &intent, &context).expect("accepted");
    let job_of = |key: &str, to: &str| {
        dispatch::mail_job(
            &app,
            &tenant,
            &input(address(to), Some("receipts"), key),
            Timestamp(T0),
        )
        .expect("a valid send")
        .0
    };
    let mut commit = Commit::of(
        &intent,
        &context,
        &transition,
        app.store.generation(&tenant).unwrap(),
    );
    commit.jobs.push(job_of("k-commit", ADA));
    // The same change decided on the same revision, which the first commit moves: it is refused.
    let mut stale = commit.clone();
    stale.jobs = vec![job_of("k-rolled-back", "bob@example.org")];

    app.store.commit(&commit).unwrap();
    assert!(matches!(
        app.store.commit(&stale),
        Err(StoreError::Conflict { .. })
    ));

    let job_exists = |key: &str| {
        let id =
            strata_storage_api::jobs::job_id(&tenant, &dispatch::job_key(&dispatch::key_hash(key)));
        app.store.job(&tenant, &id).unwrap().is_some()
    };
    assert!(job_exists("k-commit"), "the commit made its job");
    assert!(!job_exists("k-rolled-back"), "the refused commit made none");
    drain(&app, T0);
    assert_eq!(outbox_to(&app), vec![ADA.to_string()]);
}

/// Define `automation` in `alpha`, as its owner.
fn define(app: &App, automation: AutomationDef) {
    execute(
        app.store.as_ref(),
        &Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineAutomation(automation)],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx("alpha", Principal::User(principal_id("alpha:owner")), 900),
    )
    .expect("the automation is defined");
}

/// `notify` with its one step replaced by `action`, under the id `id`.
fn one_step(id: &str, action: AutomationStep) -> AutomationDef {
    let mut automation = notify();
    automation.id = WorkflowId::new(id).unwrap();
    automation
        .steps
        .get_mut(&StepName::new("send").unwrap())
        .unwrap()
        .action = action;
    automation
}

/// A person of `alpha` with an account whose verified address is `<name>@example.org`.
fn person(app: &App, name: &str) -> PrincipalId {
    let principal = principal_id(&format!("alpha:{name}"));
    app.store
        .put_account(&Account {
            tenant: tenant_id("alpha"),
            login: format!("{name}@example.org"),
            principal: principal.clone(),
            display_name: name.to_string(),
            password_hash: "unused".into(),
            verified_email: Some(format!("{name}@example.org")),
        })
        .unwrap();
    principal
}

// Verifies: NTF-1572, NTF-3200
#[test]
fn a_digest_step_sends_one_message_to_each_person_with_notices_in_its_window() {
    let app = app();
    install(&app, "alpha", "Hello {{name}}");
    let tenant = tenant_id("alpha");
    let store_ = app.store.as_ref();
    store_
        .doc_put(
            &tenant,
            "message-category",
            "receipts",
            r#"{"purpose":"transactional"}"#,
            false,
        )
        .unwrap();
    execute(
        store_,
        &Intent::ChangeSchema {
            changes: vec![
                SchemaChange::DefineMailTemplate(MailTemplateDecl {
                    template: TemplateId("daily".into()),
                    subject: "{{count}} new orders".into(),
                    body: StructuredBlocks(r#"[{"type":"paragraph","inlines":[{"type":"value","name":"notices","value_type":"str"}]}]"#.into()),
                }),
                SchemaChange::DefineNoticeTemplate(NoticeTemplateDecl {
                    template: TemplateId("order".into()),
                    title: "Order {{order}}".into(),
                    body: StructuredBlocks(r#"[{"type":"paragraph","inlines":[{"type":"text","text":"An order."}]}]"#.into()),
                }),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx("alpha", Principal::User(principal_id("alpha:owner")), 800),
    )
    .expect("the templates are defined");
    let (p1, p2, p3) = (person(&app, "p1"), person(&app, "p2"), person(&app, "p3"));
    // A digest without a recipient reaches every member of the tenant (D-8200), so the three
    // persons are members.
    for (n, who) in [&p1, &p2, &p3].into_iter().enumerate() {
        execute(
            store_,
            &Intent::AssignRole {
                principal: who.clone(),
                role: role("writer"),
            },
            &ctx(
                "alpha",
                Principal::User(principal_id("alpha:owner")),
                810 + n as i64,
            ),
        )
        .expect("the person becomes a member");
    }
    let found = store_.load(&tenant, &[]).unwrap().tenant.unwrap();
    let notice = |order: &str| strata_backend::inbox::Notice {
        topic: "app.orders".into(),
        category: Some("receipts".into()),
        template: "order".into(),
        values: BTreeMap::from([("order".to_string(), order.to_string())]),
        reference: None,
        record: None,
        mail: false,
    };
    let day = 86_400_000;
    // Two notices of p1 and one of p2 in the window, one of p3 before it and one of p1 after it.
    for (who, order, at) in [
        (&p1, "A1", T0 - 3_000),
        (&p1, "A2", T0 - 2_000),
        (&p2, "B1", T0 - 1_000),
        (&p3, "C1", T0 - day - 1),
        (&p1, "A3", T0),
    ] {
        strata_backend::inbox::deliver(
            &app,
            &tenant,
            &found,
            who,
            order,
            &notice(order),
            Timestamp(at),
        )
        .unwrap();
    }
    define(
        &app,
        one_step(
            "digest",
            AutomationStep::SendDigest {
                category: strata_model::types::MessageCategoryId("receipts".into()),
                template: TemplateId("daily".into()),
                since: strata_model::prelude::Timestamp(T0 - day),
                until: strata_model::prelude::Timestamp(T0),
                recipient: None,
                reference: Some(strata_model::types::SendReference("daily".into())),
                idempotency_key: None,
            },
        ),
    );
    let run = start(&app, "alpha", "digest", T0);
    drain(&app, T0);
    let record = app.store.automation_run(&tenant, &run).unwrap().unwrap();
    assert_eq!(record.state, RunState::Completed);
    let steps = app.store.automation_steps(&tenant, &run, None, 10).unwrap();
    assert_eq!(
        steps[0].result.clone().unwrap(),
        json!({ "SendDigest": { "messages": 2 } })
    );
    let mut sent = app.cfg.mail.outbox();
    sent.sort_by(|a, b| a.to.cmp(&b.to));
    assert_eq!(
        sent.iter().map(|m| m.to.as_str()).collect::<Vec<_>>(),
        ["p1@example.org", "p2@example.org"],
        "p3 has no notice in the window and gets no message"
    );
    assert_eq!(sent[0].subject, "2 new orders");
    assert!(sent[0].text.contains("Order A1") && sent[0].text.contains("Order A2"));
    assert!(
        !sent[0].text.contains("Order A3"),
        "a notice after the window is not in it"
    );
    assert_eq!(sent[1].subject, "1 new orders");

    // The step run again sends nothing more: each message is keyed by the step and the person.
    let key = worker::steps::step_key(
        &tenant,
        &WorkflowId::new("digest").unwrap(),
        &run,
        &StepName::new("send").unwrap(),
        steps[0].sequence,
    );
    let template = TemplateId("daily".into());
    let digest = strata_backend::digest::Digest {
        category: "receipts",
        template: &template,
        since: T0 - day,
        until: T0,
        recipient: None,
        reference: Some("daily"),
    };
    let again = strata_backend::digest::run(
        &app,
        &tenant,
        &WorkflowId::new("digest").unwrap(),
        &digest,
        &key,
        Timestamp(T0 + 1),
    )
    .unwrap();
    assert_eq!(again, 2);
    drain(&app, T0 + 1);
    assert_eq!(app.cfg.mail.outbox().len(), 2);
}
