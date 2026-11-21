//! Conformance of the tenant generation (WG-5389, `docs/working-set.md`, "The tenant generation").
//!
//! Not rows of the conformance table: the adapters' tests call these by name, as they call
//! [`super::jobs_conform`].
//!
//! - [`generation_follows_every_store_method`]: the differential case over the whole trait.
//! - [`a_commit_names_the_generation_it_was_decided_on`]: load, a foreign commit, then the commit
//!   decided on the first load; stale and unnamed generations; the receipt's generation.
//! - [`interleaved_commits_and_loads_keep_each_revision_with_one_generation`]: a load's generation
//!   is the generation of the rows it returned, under concurrent commits.
//! - [`generations_are_reproducible_from_the_seed`]: the same seeded schedule gives the same
//!   generations, and the hashes of a transition do not depend on the nonce.

use super::*;
use crate::jobs::{JobReport, JobWork, NewJob, RetryPolicy, WorkerWrites};
use crate::schedules::{AutomationRetention, ScheduleSet};
use crate::{Generation, Receipt, StoreResult};
use std::cell::Cell;
use std::collections::BTreeSet;
use strata_kernel::segments as seg;

/// The identifier of the segment the read cases ask about. No segment has it: the reads answer
/// with nothing, which is the point of the differential case.
const SEGMENT: &str = "0b1f6c3e-52a4-4c55-9a77-0d9a4c7e6b10";

fn segment_id() -> seg::SegmentId {
    serde_json::from_value(serde_json::Value::String(SEGMENT.into())).expect("a segment id")
}

/// What a state and its generation are, as the differential case compares them: the hash of the
/// complete tenant state (records, schema, members, settings and ledger), and the generation read
/// with it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Observed {
    state: Option<String>,
    generation: Option<Generation>,
}

fn observe(store: &dyn Store, tenant: &TenantId) -> Observed {
    let loaded = store.load_complete_at(tenant).expect("load_complete_at");
    let alone = store.generation(tenant).expect("generation");
    assert_eq!(
        alone, loaded.generation,
        "generation() and load_complete_at disagree on {tenant} while nothing runs"
    );
    let partial = store.load_at(tenant, &[]).expect("load_at");
    assert_eq!(
        partial.generation, loaded.generation,
        "a partial load and a complete one disagree on the generation of {tenant}"
    );
    Observed {
        state: loaded.state.tenant.as_ref().map(|t| canon::hash_of(&**t)),
        generation: loaded.generation,
    }
}

/// What a case does to the two tenants.
#[derive(Clone, Copy, Debug)]
enum Expect {
    /// Nothing a load returns changes, so no generation does.
    Same,
    /// Alpha's load result changes: its counter is raised by one under the same nonce.
    Raises,
    /// Alpha's nonce is rotated, its counter and its load result stay.
    Rotates,
    /// Alpha is put back to an earlier state under a new nonce and a raised counter.
    Restores,
    /// The tenant is gone: no state and no generation.
    Gone(&'static str),
    /// Every tenant is gone.
    AllGone,
}

struct Fx<'a> {
    store: &'a dyn Store,
    alpha: TenantId,
    beta: TenantId,
    snapshot: u64,
    tick: Cell<i64>,
}

impl Fx<'_> {
    /// A time that has not been used.
    fn next(&self) -> i64 {
        self.tick.set(self.tick.get() + 1);
        self.tick.get()
    }

    /// Commit one note to `tenant`, decided on a load of its own and guarded by that load's
    /// generation, with `write` as its segment write when there is one.
    fn commit_note(
        &self,
        tenant: &TenantId,
        slug: &str,
        write: Option<&seg::SegmentWrite>,
    ) -> StoreResult<Receipt> {
        commit_note(self.store, tenant, slug, self.next(), write)
    }
}

fn commit_note(
    store: &dyn Store,
    tenant: &TenantId,
    slug: &str,
    at: i64,
    write: Option<&seg::SegmentWrite>,
) -> StoreResult<Receipt> {
    let intent = new_note(slug, slug);
    let schema = store
        .load(tenant, &[])?
        .tenant
        .expect("the tenant exists")
        .schema
        .clone();
    let loaded = store.load_at(tenant, &needs(Some(&schema), &intent))?;
    let context = ctx(
        tenant.as_str(),
        user(&format!("{}:writer1", tenant.as_str())),
        at,
    );
    let decided = transition(&loaded.state, &intent, &context).expect("the note is accepted");
    let commit = Commit::of(&intent, &context, &decided, loaded.generation);
    match write {
        Some(write) => store.commit_with_segments(&commit, write),
        None => store.commit(&commit),
    }
}

/// Two tenants with notes, a ledger and a snapshot, and one commit after the snapshot so that
/// restoring it changes the state.
fn populated(store: &dyn Store) -> (Pair<'_>, u64) {
    let mut pair = Pair::new(store);
    let alpha = tenant_id("alpha");
    let owner = user("alpha:owner");
    let unit = LedgerUnit::new("points").unwrap();
    pair.ok(
        "alpha",
        owner.clone(),
        Intent::ChangeSchema {
            changes: vec![
                SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
                    unit: unit.clone(),
                    description: "Points".into(),
                    expiry: None,
                }),
                SchemaChange::SetLedgerLimit(LedgerLimit {
                    unit: unit.clone(),
                    owner: None,
                    floor: Some(LedgerAmount(0)),
                    cap: None,
                    period_cap: None,
                    period: None,
                    scope: None,
                }),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    let writer = LedgerOwner::Principal {
        principal: principal_id("alpha:writer1"),
    };
    for (amount, key) in [(10, "a"), (5, "b")] {
        pair.ok(
            "alpha",
            owner.clone(),
            Intent::PostLedger {
                postings: vec![LedgerPostingRequest {
                    owner: writer.clone(),
                    unit: unit.clone(),
                    amount: LedgerAmount(amount),
                    idempotency_key: key.into(),
                    reason: "conformance".into(),
                    corrects: None,
                    scope: None,
                }],
            },
        );
    }
    pair.note("alpha", "First", "first");
    pair.note("beta", "Other", "first");
    let info = store
        .create_snapshot(&alpha, "populated", Timestamp(5_000))
        .expect("snapshot");
    pair.note("alpha", "After", "after");
    (pair, info.id)
}

/// The methods the `Store` trait declares, read from its source: every line that starts a method
/// at the trait's own indentation.
fn trait_methods() -> BTreeSet<String> {
    let source = include_str!("../lib.rs");
    let start = source
        .find("pub trait Store: Send + Sync {")
        .expect("the Store trait is in lib.rs");
    let body = &source[start..];
    let mut methods = BTreeSet::new();
    for line in body.lines().skip(1) {
        if line.starts_with('}') {
            break;
        }
        if let Some(rest) = line.strip_prefix("    fn ") {
            let end = rest.find(['(', '<']).expect("a method has parameters");
            methods.insert(rest[..end].to_string());
        }
    }
    methods
}

type Case<'a> = (&'static str, Expect, Box<dyn Fn(&Fx<'_>) + 'a>);

/// One case for every method of the trait, in the order they run: the ones that remove a tenant
/// come last.
fn cases<'a>() -> Vec<Case<'a>> {
    let job = |key: &str, due: i64| NewJob {
        key: key.to_string(),
        work: JobWork::Step {
            run: "run-1".into(),
            automation: WorkflowId::new("award").expect("id"),
            step: StepName::new("first").expect("name"),
            sequence: 1,
        },
        due_at: Timestamp(due),
        policy: RetryPolicy {
            attempts: 2,
            first_delay_ms: 100,
            max_delay_ms: 1_000,
            lease_ms: 50,
        },
    };
    let job = std::rc::Rc::new(job);
    let account = |tenant: &TenantId, login: &str, who: &str| Account {
        tenant: tenant.clone(),
        login: login.into(),
        principal: principal_id(who),
        display_name: "Person".into(),
        password_hash: "$argon2id$placeholder".into(),
        verified_email: None,
    };
    let account = std::rc::Rc::new(account);
    let later = Timestamp(1_000_000_000);
    let mut list: Vec<Case<'a>> = Vec::new();
    macro_rules! case {
        ($name:literal, $expect:expr, |$f:ident| $body:block) => {
            list.push(($name, $expect, Box::new(move |$f: &Fx<'_>| $body)));
        };
    }
    // Reads that name no tenant, and the layout.
    case!("capabilities", Expect::Same, |f| {
        f.store.capabilities();
    });
    case!("health", Expect::Same, |f| {
        let _ = f.store.health();
    });
    case!("liveness", Expect::Same, |f| {
        let _ = f.store.liveness();
    });
    case!("migrations", Expect::Same, |f| {
        let _ = f.store.migrations();
    });
    case!("tenants", Expect::Same, |f| {
        let _ = f.store.tenants();
    });
    case!("tenant_summary", Expect::Same, |f| {
        let _ = f.store.tenant_summary(&f.alpha);
    });
    case!("object_revisions", Expect::Same, |f| {
        let _ = f.store.object_revisions(&f.alpha, &[object("note")]);
    });
    case!("tenant_plan", Expect::Same, |f| {
        let _ = f.store.tenant_plan(&f.alpha);
    });
    case!("set_tenant_plan", Expect::Same, |f| {
        let _ = f.store.set_tenant_plan(&f.alpha, "premium");
    });
    case!("stored_bytes", Expect::Same, |f| {
        let _ = f.store.stored_bytes(&f.alpha);
    });
    // The loads and the generation.
    case!("load_at", Expect::Same, |f| {
        f.store
            .load_at(&f.alpha, &[Need::AllOf(object("note"))])
            .unwrap();
    });
    case!("load_complete_at", Expect::Same, |f| {
        f.store.load_complete_at(&f.alpha).unwrap();
    });
    case!("load", Expect::Same, |f| {
        f.store.load(&f.alpha, &[]).unwrap();
    });
    case!("load_complete", Expect::Same, |f| {
        f.store.load_complete(&f.alpha).unwrap();
    });
    case!("generation", Expect::Same, |f| {
        f.store.generation(&f.alpha).unwrap();
    });
    case!("rotate_nonce", Expect::Rotates, |f| {
        f.store.rotate_nonce(&f.alpha).unwrap();
    });
    // The writes that change what a load returns.
    case!("commit", Expect::Raises, |f| {
        f.commit_note(&f.alpha, "differential-commit", None)
            .unwrap();
    });
    case!("commit_with_segments", Expect::Raises, |f| {
        let write = seg::SegmentWrite {
            erased: vec![principal_id("alpha:editor")],
            ..Default::default()
        };
        f.commit_note(&f.alpha, "differential-segments", Some(&write))
            .unwrap();
    });
    case!("commit_segments", Expect::Same, |f| {
        // A segment write that no transition carries moves no record, and so no generation.
        let revision = f
            .store
            .load(&f.alpha, &[])
            .unwrap()
            .tenant
            .expect("alpha exists")
            .revision;
        let commit = crate::segments::SegmentCommit {
            tenant: f.alpha.clone(),
            revision,
            write: seg::SegmentWrite {
                erased: vec![principal_id("alpha:writer1")],
                ..Default::default()
            },
            work: WorkerWrites::default(),
            now: Timestamp(f.next()),
        };
        let _ = f.store.commit_segments(&commit);
    });
    case!("anonymise_ledger_principal", Expect::Raises, |f| {
        // A principal the ledger holds nothing of changes nothing, and so no generation.
        let before = f.store.generation(&f.alpha).unwrap();
        let nothing = f
            .store
            .anonymise_ledger_principal(&f.alpha, &principal_id("alpha:nobody"), None)
            .unwrap();
        assert_eq!(nothing, Default::default());
        assert_eq!(f.store.generation(&f.alpha).unwrap(), before);
        let done = f
            .store
            .anonymise_ledger_principal(&f.alpha, &principal_id("alpha:owner"), None)
            .unwrap();
        assert!(done.entries > 0, "the owner caused ledger entries");
    });
    case!("restore_snapshot", Expect::Restores, |f| {
        f.store
            .restore_snapshot(&f.alpha, f.snapshot, None)
            .unwrap();
    });
    // Everything that is not what a load returns: events, journal, sessions, credentials,
    // documents, audit lines, snapshots, jobs, schedules and the platform's settings.
    case!("idempotent_result", Expect::Same, |f| {
        let _ = f
            .store
            .idempotent_result(&f.alpha, "principal", "key", Timestamp(1));
    });
    case!("events_after", Expect::Same, |f| {
        let _ = f.store.events_after(&f.alpha, 0, 10);
    });
    case!("outbox", Expect::Same, |f| {
        let _ = f.store.outbox(10);
    });
    case!("outbox_faults", Expect::Same, |f| {
        f.store.outbox_faults();
    });
    case!("outbox_pending_of", Expect::Same, |f| {
        let _ = f.store.outbox_pending_of(&f.alpha);
    });
    case!("mark_published", Expect::Same, |f| {
        let _ = f.store.mark_published(&f.alpha, 1);
    });
    case!("journal_after", Expect::Same, |f| {
        let _ = f.store.journal_after(&f.alpha, 0, 10);
    });
    case!("journal_count", Expect::Same, |f| {
        let _ = f.store.journal_count(&f.alpha);
    });
    case!("put_session", Expect::Same, |f| {
        let _ = f
            .store
            .put_session(&session("gen-s1", "alpha", "alpha:owner", 9_000_000));
    });
    case!("session_seen", Expect::Same, |f| {
        let _ = f
            .store
            .session_seen(&SessionId::new("gen-s1").unwrap(), Timestamp(5));
    });
    case!("session_authenticated", Expect::Same, |f| {
        let _ = f.store.session_authenticated(
            &SessionId::new("gen-s1").unwrap(),
            Timestamp(6),
            Some("net"),
        );
    });
    case!("session_by_token", Expect::Same, |f| {
        let _ = f.store.session_by_token("hash-of-gen-s1");
    });
    case!("session", Expect::Same, |f| {
        let _ = f.store.session(&SessionId::new("gen-s1").unwrap());
    });
    case!("live_sessions", Expect::Same, |f| {
        let _ = f.store.live_sessions(Timestamp(100));
    });
    case!("live_sessions_of", Expect::Same, |f| {
        let _ = f.store.live_sessions_of(&f.alpha, None, Timestamp(100));
    });
    case!("purge_sessions", Expect::Same, |f| {
        let _ = f.store.purge_sessions(Timestamp(1));
    });
    case!("throttle_reserve", Expect::Same, |f| {
        let rule = crate::ThrottleRule {
            key: "account:gen".into(),
            max: 5,
            window_ms: 1_000,
            lock_ms: 1_000,
            lock_resets: true,
        };
        let _ = f.store.throttle_reserve(&f.alpha, &[rule], Timestamp(10));
    });
    case!("throttle_settle", Expect::Same, |f| {
        let _ = f
            .store
            .throttle_settle(&f.alpha, "account:gen", 5, crate::ThrottleSettle::Clear);
    });
    case!("purge_throttle", Expect::Same, |f| {
        let _ = f.store.purge_throttle(Timestamp(1_000_000));
    });
    {
        let account = account.clone();
        case!("put_account", Expect::Same, |f| {
            let _ = f
                .store
                .put_account(&account(&f.alpha, "gen-login", "alpha:editor"));
        });
    }
    {
        let account = account.clone();
        case!("create_account", Expect::Same, |f| {
            let _ = f
                .store
                .create_account(&account(&f.alpha, "gen-created", "alpha:writer1"));
        });
    }
    {
        let account = account.clone();
        case!("update_account", Expect::Same, |f| {
            let held = account(&f.alpha, "gen-created", "alpha:writer1");
            let mut next = held.clone();
            next.display_name = "Renamed".into();
            let _ = f.store.update_account(&held, &next);
        });
    }
    case!("account", Expect::Same, |f| {
        let _ = f.store.account(&f.alpha, "gen-login");
    });
    case!("remove_account", Expect::Same, |f| {
        let _ = f.store.remove_account(&f.alpha, "gen-created");
    });
    case!("remove_principal", Expect::Same, |f| {
        // Sign-in rows only: nothing a load returns.
        let _ = f
            .store
            .remove_principal(&f.alpha, &principal_id("alpha:editor"));
    });
    case!("account_of", Expect::Same, |f| {
        let _ = f.store.account_of(&f.alpha, &principal_id("alpha:editor"));
    });
    case!("put_passkey", Expect::Same, |f| {
        let _ = f.store.put_passkey(&PasskeyRecord {
            tenant: f.alpha.clone(),
            principal: principal_id("alpha:owner"),
            id: "gen-passkey".into(),
            rp_id: "example.org".into(),
            label: "Key".into(),
            credential: "{}".into(),
            created_at: Timestamp(1),
            last_used_at: None,
        });
    });
    case!("passkey", Expect::Same, |f| {
        let _ = f.store.passkey(&f.alpha, "gen-passkey");
    });
    case!("passkeys", Expect::Same, |f| {
        let _ = f.store.passkeys(&f.alpha, &principal_id("alpha:owner"));
    });
    case!("update_passkey", Expect::Same, |f| {
        let _ = f
            .store
            .update_passkey(&f.alpha, "gen-passkey", "{\"n\":1}", Timestamp(2));
    });
    case!("remove_passkey", Expect::Same, |f| {
        let _ = f
            .store
            .remove_passkey(&f.alpha, &principal_id("alpha:owner"), "gen-passkey");
    });
    case!("put_passkey_challenge", Expect::Same, |f| {
        let _ = f.store.put_passkey_challenge(&PasskeyChallenge {
            tenant: f.alpha.clone(),
            id_hash: "gen-challenge".into(),
            ceremony: PasskeyCeremony::SignIn,
            session: None,
            principal: Some(principal_id("alpha:owner")),
            origin: "https://example.org".into(),
            state: "{}".into(),
            expires_at: later,
        });
    });
    case!("take_passkey_challenge", Expect::Same, |f| {
        let _ = f
            .store
            .take_passkey_challenge(&f.alpha, "gen-challenge", Timestamp(10));
    });
    case!("purge_passkey_challenges", Expect::Same, |f| {
        let _ = f.store.purge_passkey_challenges(Timestamp(1));
    });
    case!("put_sign_in_link", Expect::Same, |f| {
        let _ = f.store.put_sign_in_link(&SignInLink {
            tenant: f.alpha.clone(),
            id_hash: "gen-link".into(),
            principal: principal_id("alpha:owner"),
            token_hash: "t".into(),
            binding_hash: "b".into(),
            origin: "https://example.org".into(),
            expires_at: later,
        });
    });
    case!("take_sign_in_link", Expect::Same, |f| {
        let _ = f.store.take_sign_in_link(&f.alpha, "gen-link");
    });
    case!("purge_sign_in_links", Expect::Same, |f| {
        let _ = f.store.purge_sign_in_links(Timestamp(1));
    });
    case!("put_oidc_flow", Expect::Same, |f| {
        let _ = f.store.put_oidc_flow(&crate::OidcFlow {
            tenant: f.alpha.clone(),
            id_hash: "gen-flow".into(),
            provider: "idp".into(),
            nonce: "n".into(),
            verifier: "v".into(),
            binding_hash: "b".into(),
            origin: "https://example.org".into(),
            principal: None,
            expires_at: later,
        });
    });
    case!("take_oidc_flow", Expect::Same, |f| {
        let _ = f.store.take_oidc_flow(&f.alpha, "gen-flow");
    });
    case!("purge_oidc_flows", Expect::Same, |f| {
        let _ = f.store.purge_oidc_flows(Timestamp(1));
    });
    case!("put_saml_request", Expect::Same, |f| {
        let _ = f.store.put_saml_request(&crate::SamlRequest {
            tenant: f.alpha.clone(),
            id_hash: "gen-saml".into(),
            provider: "idp".into(),
            request_id: "id-1".into(),
            binding_hash: "b".into(),
            origin: "https://example.org".into(),
            principal: None,
            expires_at: later,
        });
    });
    case!("take_saml_request", Expect::Same, |f| {
        let _ = f.store.take_saml_request(&f.alpha, "gen-saml");
    });
    case!("purge_saml_requests", Expect::Same, |f| {
        let _ = f.store.purge_saml_requests(Timestamp(1));
    });
    case!("put_saml_assertion", Expect::Same, |f| {
        let _ = f
            .store
            .put_saml_assertion(&f.alpha, "idp", "assertion-1", later);
    });
    case!("purge_saml_assertions", Expect::Same, |f| {
        let _ = f.store.purge_saml_assertions(Timestamp(1));
    });
    case!("put_identity", Expect::Same, |f| {
        let _ = f.store.put_identity(&FederatedIdentity {
            tenant: f.alpha.clone(),
            provider: "idp".into(),
            subject: "gen-subject".into(),
            principal: principal_id("alpha:owner"),
            created_at: Timestamp(1),
        });
    });
    case!("identity", Expect::Same, |f| {
        let _ = f.store.identity(&f.alpha, "idp", "gen-subject");
    });
    case!("put_totp", Expect::Same, |f| {
        let _ = f.store.put_totp(&TotpRecord {
            tenant: f.alpha.clone(),
            principal: principal_id("alpha:owner"),
            secret: "sealed".into(),
            confirmed: false,
            last_step: -1,
            created_at: Timestamp(1),
        });
    });
    case!("totp", Expect::Same, |f| {
        let _ = f.store.totp(&f.alpha, &principal_id("alpha:owner"));
    });
    case!("confirm_totp", Expect::Same, |f| {
        let _ = f
            .store
            .confirm_totp(&f.alpha, &principal_id("alpha:owner"), 5);
    });
    case!("use_totp_step", Expect::Same, |f| {
        let _ = f
            .store
            .use_totp_step(&f.alpha, &principal_id("alpha:owner"), 6);
    });
    case!("remove_totp", Expect::Same, |f| {
        let _ = f.store.remove_totp(&f.alpha, &principal_id("alpha:owner"));
    });
    case!("replace_recovery_codes", Expect::Same, |f| {
        let _ = f.store.replace_recovery_codes(
            &f.alpha,
            &principal_id("alpha:owner"),
            &["gen-code-1".to_string(), "gen-code-2".to_string()],
            Timestamp(1),
        );
    });
    case!("spend_recovery_code", Expect::Same, |f| {
        let _ = f
            .store
            .spend_recovery_code(&f.alpha, &principal_id("alpha:owner"), "gen-code-1");
    });
    case!("recovery_codes_left", Expect::Same, |f| {
        let _ = f
            .store
            .recovery_codes_left(&f.alpha, &principal_id("alpha:owner"));
    });
    case!("put_pending", Expect::Same, |f| {
        let _ = f.store.put_pending(&PendingSignIn {
            tenant: f.alpha.clone(),
            id_hash: "gen-pending".into(),
            principal: principal_id("alpha:owner"),
            method: "password".into(),
            factors: 1,
            attempts: 0,
            expires_at: later,
        });
    });
    case!("attempt_pending", Expect::Same, |f| {
        let _ = f
            .store
            .attempt_pending(&f.alpha, "gen-pending", Timestamp(10), 5);
    });
    case!("take_pending", Expect::Same, |f| {
        let _ = f.store.take_pending(&f.alpha, "gen-pending", Timestamp(10));
    });
    case!("put_recovery", Expect::Same, |f| {
        let _ = f.store.put_recovery(&RecoveryRequest {
            tenant: f.alpha.clone(),
            id_hash: "gen-recovery".into(),
            principal: principal_id("alpha:owner"),
            login: "gen-login".into(),
            token_hash: "t".into(),
            cancel_hash: "c".into(),
            requested_at: Timestamp(1),
            completes_at: Timestamp(1),
            expires_at: later,
        });
    });
    case!("recovery", Expect::Same, |f| {
        let _ = f.store.recovery(&f.alpha, "gen-recovery");
    });
    case!("take_recovery", Expect::Same, |f| {
        let _ = f.store.take_recovery(&f.alpha, "gen-recovery");
    });
    case!("cancel_recovery", Expect::Same, |f| {
        let _ = f.store.cancel_recovery(&f.alpha, "gen-recovery", "c");
    });
    case!("purge_factor_challenges", Expect::Same, |f| {
        let _ = f.store.purge_factor_challenges(Timestamp(1));
    });
    // Key documents and the hot-set manifest raise nothing (docs/working-set.md).
    case!("doc_put", Expect::Same, |f| {
        let _ = f.store.doc_put(&f.alpha, "client", "gen", "{}", false);
    });
    case!("doc_get", Expect::Same, |f| {
        let _ = f.store.doc_get(&f.alpha, "client", "gen");
    });
    case!("doc_list", Expect::Same, |f| {
        let _ = f.store.doc_list(&f.alpha, "client");
    });
    case!("doc_remove", Expect::Same, |f| {
        let _ = f.store.doc_remove(&f.alpha, "client", "gen");
    });
    case!("audit_append", Expect::Same, |f| {
        let _ = f.store.audit_append(
            &f.alpha,
            &AuditEntry {
                at: 1,
                kind: "audit".into(),
                event: "generation.check".into(),
                correlation: "c-1".into(),
                data: "{}".into(),
            },
        );
    });
    case!("audit_list", Expect::Same, |f| {
        let _ = f.store.audit_list(&f.alpha, None, 10);
    });
    case!("audit_chain", Expect::Same, |f| {
        let _ = f.store.audit_chain(&f.alpha, 0, 10);
    });
    case!("verified_address", Expect::Same, |f| {
        let _ = f
            .store
            .verified_address(&f.alpha, &principal_id("alpha:owner"));
    });
    case!("create_snapshot", Expect::Same, |f| {
        let _ = f
            .store
            .create_snapshot(&f.alpha, "differential", Timestamp(7_000));
    });
    case!("snapshots", Expect::Same, |f| {
        let _ = f.store.snapshots(&f.alpha);
    });
    case!("load_snapshot", Expect::Same, |f| {
        let _ = f.store.load_snapshot(&f.alpha, f.snapshot);
    });
    {
        let job = job.clone();
        case!("jobs_create", Expect::Same, |f| {
            let _ = f
                .store
                .jobs_create(&f.alpha, &[job("gen-a", 5_000)], Timestamp(4_000));
        });
    }
    case!("job", Expect::Same, |f| {
        let _ = f.store.job(&f.alpha, "no-such-job");
    });
    case!("jobs_list", Expect::Same, |f| {
        let _ = f.store.jobs_list(&f.alpha, None, 10);
    });
    case!("job_lease", Expect::Same, |f| {
        let _ = f.store.job_lease(&f.alpha, "w1", Timestamp(5_000));
    });
    case!("job_report", Expect::Same, |f| {
        let _ = f.store.job_report(
            &f.alpha,
            "no-such-job",
            1,
            &JobReport::Succeeded,
            &WorkerWrites::default(),
            Timestamp(5_001),
        );
    });
    case!("job_cancel", Expect::Same, |f| {
        let _ = f
            .store
            .job_cancel(&f.alpha, "no-such-job", "x", Timestamp(5_002));
    });
    case!("job_retry", Expect::Same, |f| {
        let _ = f.store.job_retry(&f.alpha, "no-such-job", Timestamp(5_003));
    });
    case!("jobs_due", Expect::Same, |f| {
        let _ = f.store.jobs_due(Timestamp(9_000), 10);
    });
    case!("automation_write", Expect::Same, |f| {
        let _ = f
            .store
            .automation_write(&f.alpha, &WorkerWrites::default(), Timestamp(5_004));
    });
    case!("automation_run", Expect::Same, |f| {
        let _ = f.store.automation_run(&f.alpha, "run-1");
    });
    case!("automation_runs", Expect::Same, |f| {
        let _ = f.store.automation_runs(&f.alpha, None, None, None, 10);
    });
    case!("automation_steps", Expect::Same, |f| {
        let _ = f.store.automation_steps(&f.alpha, "run-1", None, 10);
    });
    case!("automation_exceptions", Expect::Same, |f| {
        let _ = f
            .store
            .automation_exceptions(&f.alpha, None, false, None, 10);
    });
    case!("automation_exception_settle", Expect::Same, |f| {
        let _ = f
            .store
            .automation_exception_settle(&f.alpha, "no-such", false, Timestamp(5_005));
    });
    case!("schedule_set", Expect::Same, |f| {
        let _ = f.store.schedule_set(
            &f.alpha,
            &ScheduleSet {
                id: SEGMENT.to_string(),
                rule: "FREQ=DAILY".into(),
                paused: false,
                anchor: Timestamp(1_000),
                due: Some(Timestamp(5_000)),
            },
            Timestamp(2_000),
        );
    });
    case!("schedule", Expect::Same, |f| {
        let _ = f.store.schedule(&f.alpha, SEGMENT);
    });
    case!("schedules", Expect::Same, |f| {
        let _ = f.store.schedules(&f.alpha);
    });
    case!("run_key_holder", Expect::Same, |f| {
        let _ =
            f.store
                .run_key_holder(&f.alpha, &WorkflowId::new("award").expect("id"), "key-hash");
    });
    case!("open_run_key_starts", Expect::Same, |f| {
        let _ = f.store.open_run_key_starts(
            &f.alpha,
            &WorkflowId::new("award").expect("id"),
            "key-hash",
        );
    });
    case!("purge_automation", Expect::Same, |f| {
        let _ = f
            .store
            .purge_automation(Timestamp(1), &AutomationRetention::default());
    });
    case!("platform_settings", Expect::Same, |f| {
        let _ = f.store.platform_settings();
    });
    case!("platform_setting_write", Expect::Same, |f| {
        let _ = f.store.platform_setting_write(
            &SettingId::new("page_size").expect("a setting name"),
            Some(&Value::Int(50)),
            0,
            Timestamp(1),
        );
    });
    // The directory states are outside the tenant's generation: a provisioning round trip leaves
    // it as it was.
    case!("begin_provisioning", Expect::Same, |f| {
        let _ = f.store.begin_provisioning(&f.beta);
        let _ = f.store.finish_provisioning(&f.beta);
    });
    case!("finish_provisioning", Expect::Same, |f| {
        let _ = f.store.begin_provisioning(&f.beta);
        let _ = f.store.finish_provisioning(&f.beta);
    });
    case!("rollback_provisioning", Expect::Same, |f| {
        let _ = f.store.rollback_provisioning();
    });
    // The reads of the segment block, which an adapter without one answers with nothing.
    case!("segment_catalog", Expect::Same, |f| {
        let _ = f.store.segment_catalog(&f.alpha);
    });
    case!("segment_rows", Expect::Same, |f| {
        let _ = f
            .store
            .segment_rows(&f.alpha, &[(segment_id(), principal_id("alpha:owner"))]);
    });
    case!("segment_members", Expect::Same, |f| {
        let _ = f.store.segment_members(&f.alpha, segment_id(), None, 10);
    });
    case!("segment_memberships_of", Expect::Same, |f| {
        let _ = f
            .store
            .segment_memberships_of(&f.alpha, &principal_id("alpha:owner"));
    });
    case!("segment_wakes_of", Expect::Same, |f| {
        let _ = f
            .store
            .segment_wakes_of(&f.alpha, &principal_id("alpha:owner"));
    });
    case!("segment_due", Expect::Same, |f| {
        let _ = f
            .store
            .segment_due(&f.alpha, segment_id(), Timestamp(10), 10);
    });
    case!("segment_batches", Expect::Same, |f| {
        let _ = f.store.segment_batches(&f.alpha);
    });
    case!("segment_referencing", Expect::Same, |f| {
        let _ = f.store.segment_referencing(
            &f.alpha,
            &object("note"),
            &field("author"),
            RecordId(1),
            None,
            10,
        );
    });
    case!("segment_subject_count", Expect::Same, |f| {
        let _ = f.store.segment_subject_count(&f.alpha, &object("note"));
    });
    case!("segment_events_after", Expect::Same, |f| {
        let _ = f.store.segment_events_after(&f.alpha, 0, 10);
    });
    case!("segment_cursor", Expect::Same, |f| {
        let _ = f.store.segment_cursor(&f.alpha);
    });
    case!("search_candidates", Expect::Same, |f| {
        let _ = f.store.search_candidates(
            &f.alpha,
            &strata_kernel::search::Probe {
                object: object("note"),
                fields: vec![field("title")],
                tokens: vec!["first".into()],
                after: None,
                limit: 10,
            },
        );
    });
    // The methods that remove a tenant run last.
    case!("reset_tenant", Expect::Gone("beta"), |f| {
        f.store.reset_tenant(&f.beta).unwrap();
    });
    case!("reset", Expect::AllGone, |f| {
        f.store.reset().unwrap();
    });
    list
}

/// The differential case over the whole trait (WG-5389): for every method of [`Store`], run on a
/// populated tenant and a second tenant, each tenant's generation changes exactly when the tenant's
/// `load_complete` result (which holds its ledger) changed, to a generation never used before. The
/// control plane's own steps are the stated exceptions: a rotation changes the generation and not
/// the state, and a restore changes both whatever the rows were. The list of cases is checked
/// against the trait's methods, so a method added to the trait without a case fails.
// Verifies: STO-001
pub fn generation_follows_every_store_method(make: &dyn Fn() -> Box<dyn Store>) {
    let store = make();
    let (_pair, snapshot) = populated(store.as_ref());
    let fx = Fx {
        store: store.as_ref(),
        alpha: tenant_id("alpha"),
        beta: tenant_id("beta"),
        snapshot,
        tick: Cell::new(20_000),
    };
    let list = cases();

    let named: BTreeSet<String> = list.iter().map(|case| case.0.to_string()).collect();
    assert_eq!(named.len(), list.len(), "a method has one case, not two");
    let declared = trait_methods();
    assert!(
        declared.len() > 100,
        "the trait's methods were not read from its source: {declared:?}"
    );
    let missing: Vec<&String> = declared.difference(&named).collect();
    let extra: Vec<&String> = named.difference(&declared).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "the cases of the generation check are not the methods of the Store trait: \
         methods without a case: {missing:?}; cases without a method: {extra:?}"
    );

    // Every generation either tenant has held, in the order seen: a changed generation is one
    // that was never seen.
    let mut seen: BTreeSet<Generation> = BTreeSet::new();
    for tenant in [&fx.alpha, &fx.beta] {
        seen.extend(observe(fx.store, tenant).generation);
    }
    for (name, expect, run) in list {
        let before = [observe(fx.store, &fx.alpha), observe(fx.store, &fx.beta)];
        run(&fx);
        let after = [observe(fx.store, &fx.alpha), observe(fx.store, &fx.beta)];
        for (index, tenant) in [&fx.alpha, &fx.beta].into_iter().enumerate() {
            let state_changed = before[index].state != after[index].state;
            let generation_changed = before[index].generation != after[index].generation;
            let rotation = matches!(expect, Expect::Rotates) && index == 0;
            assert_eq!(
                generation_changed,
                state_changed || rotation,
                "{name}: the generation of {tenant} must change exactly when what a load returns \
                 does (state changed: {state_changed}; generation {:?} then {:?})",
                before[index].generation,
                after[index].generation,
            );
            if generation_changed {
                if let Some(now) = after[index].generation {
                    assert!(
                        seen.insert(now),
                        "{name}: the generation of {tenant} changed to one it held before: {now}"
                    );
                }
            }
        }
        let (a_before, a_after) = (&before[0], &after[0]);
        match expect {
            Expect::Same => {
                assert_eq!(a_before, a_after, "{name} changes nothing");
                assert_eq!(before[1], after[1], "{name} changes nothing");
            }
            Expect::Raises => {
                assert_ne!(a_before.state, a_after.state, "{name} changes alpha");
                let (was, now) = (a_before.generation.unwrap(), a_after.generation.unwrap());
                assert_eq!(now, was.raised(), "{name} raises the counter by one");
                assert_eq!(before[1], after[1], "{name} leaves the other tenant alone");
            }
            Expect::Rotates => {
                let (was, now) = (a_before.generation.unwrap(), a_after.generation.unwrap());
                assert_ne!(was.nonce, now.nonce, "{name} draws a new nonce");
                assert_eq!(was.counter, now.counter, "{name} leaves the counter");
                assert_eq!(a_before.state, a_after.state, "{name} changes no state");
                assert_eq!(before[1], after[1], "{name} leaves the other tenant alone");
            }
            Expect::Restores => {
                assert_ne!(a_before.state, a_after.state, "{name} changes alpha");
                let (was, now) = (a_before.generation.unwrap(), a_after.generation.unwrap());
                assert_ne!(was.nonce, now.nonce, "{name} draws a new nonce");
                assert_eq!(now.counter, was.counter + 1, "{name} raises the counter");
                assert_eq!(before[1], after[1], "{name} leaves the other tenant alone");
            }
            Expect::Gone(gone) => {
                let index = usize::from(gone == "beta");
                assert_eq!(
                    after[index],
                    Observed {
                        state: None,
                        generation: None
                    },
                    "{name} removes {gone}"
                );
                assert_eq!(
                    before[1 - index],
                    after[1 - index],
                    "{name} leaves the other tenant alone"
                );
            }
            Expect::AllGone => {
                let nothing = Observed {
                    state: None,
                    generation: None,
                };
                assert_eq!(after[0], nothing, "{name} removes every tenant");
                assert_eq!(after[1], nothing, "{name} removes every tenant");
            }
        }
    }
}

/// A commit is accepted only at the generation its decision was made on (WG-5389): a decision made
/// on one load and committed after a foreign commit is a conflict that carries the newer
/// generation and changes nothing; a stale generation at the right revision is the same conflict;
/// an unnamed generation with a base revision is invalid; and the receipt carries the generation
/// the commit produced.
// Verifies: STO-001
pub fn a_commit_names_the_generation_it_was_decided_on(store: &dyn Store) {
    let pair = Pair::new(store);
    let alpha = tenant_id("alpha");
    let intent = new_note("Decided", "decided");
    let schema = store
        .load(&alpha, &[])
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .clone();

    // Load, then a second handle commits to the tenant, then the transition decided on the first
    // load is committed with that load's generation.
    let first = store
        .load_at(&alpha, &needs(Some(&schema), &intent))
        .unwrap();
    let first_generation = first.generation.expect("the tenant has a generation");
    let context = ctx("alpha", user("alpha:writer1"), 9_000);
    let decided = transition(&first.state, &intent, &context).unwrap();
    let foreign = execute(
        store,
        &new_note("Foreign", "foreign"),
        &ctx("alpha", user("alpha:writer1"), 9_001),
    )
    .unwrap();
    let newer = store.generation(&alpha).unwrap().unwrap();
    assert_ne!(newer, first_generation);
    assert_eq!(
        foreign.receipt.generation, newer,
        "the receipt of the foreign commit is the generation the store holds"
    );
    let observed = observe(store, &alpha);
    let journal = store.journal_after(&alpha, 0, 1_000).unwrap().len();
    let events = store.events_after(&alpha, 0, 1_000).unwrap().len();
    let error = store
        .commit(&Commit::of(
            &intent,
            &context,
            &decided,
            Some(first_generation),
        ))
        .unwrap_err();
    match error {
        StoreError::Conflict {
            found_generation, ..
        } => assert_eq!(
            found_generation,
            Some(newer),
            "the conflict carries the generation found"
        ),
        other => panic!("a decision on an older load must conflict, got {other:?}"),
    }
    assert_eq!(
        observe(store, &alpha),
        observed,
        "a conflict changes nothing"
    );
    assert_eq!(
        store.journal_after(&alpha, 0, 1_000).unwrap().len(),
        journal
    );
    assert_eq!(store.events_after(&alpha, 0, 1_000).unwrap().len(), events);

    // A stale generation at the right revision: the revision alone would accept it.
    let current = store
        .load_at(&alpha, &needs(Some(&schema), &intent))
        .unwrap();
    let current_generation = current.generation.unwrap();
    let decided = transition(&current.state, &intent, &context).unwrap();
    let stale = Generation {
        nonce: current_generation.nonce ^ 1,
        counter: current_generation.counter,
    };
    let error = store
        .commit(&Commit::of(&intent, &context, &decided, Some(stale)))
        .unwrap_err();
    match error {
        StoreError::Conflict {
            found_generation, ..
        } => assert_eq!(found_generation, Some(current_generation)),
        other => panic!("a stale generation must conflict, got {other:?}"),
    }
    assert_eq!(observe(store, &alpha).generation, Some(current_generation));

    // An unnamed generation with a base revision is refused whole.
    let error = store
        .commit(&Commit::of(&intent, &context, &decided, None))
        .unwrap_err();
    assert!(
        matches!(error, StoreError::Invalid(_)),
        "a commit with a base revision and no generation is invalid, got {error:?}"
    );
    assert_eq!(observe(store, &alpha).generation, Some(current_generation));

    // The commit at the right generation applies, and its receipt is the expected generation with
    // the counter raised by one.
    let receipt = store
        .commit(&Commit::of(
            &intent,
            &context,
            &decided,
            Some(current_generation),
        ))
        .unwrap();
    assert_eq!(receipt.generation, current_generation.raised());
    assert_eq!(store.generation(&alpha).unwrap(), Some(receipt.generation));
    assert_eq!(
        store.load_at(&alpha, &[]).unwrap().generation,
        Some(receipt.generation)
    );

    // A commit that creates a tenant writes its first generation: counter 1, and the receipt is
    // what generation() then returns.
    let gamma = tenant_id("gamma");
    assert_eq!(store.generation(&gamma).unwrap(), None);
    let created = execute(
        store,
        &Intent::CreateTenant {
            name: "gamma".into(),
            owner: principal_id("gamma:owner"),
        },
        &ctx("gamma", Principal::System, 9_100),
    )
    .unwrap();
    assert_eq!(created.receipt.generation.counter, 1);
    assert_eq!(
        store.generation(&gamma).unwrap(),
        Some(created.receipt.generation)
    );
    // ... and every tenant's generation is its own.
    assert_ne!(
        created.receipt.generation.nonce, receipt.generation.nonce,
        "tenants draw their own nonces"
    );
    // A tenant created again after it was removed does not start under the nonce it had.
    store.reset_tenant(&gamma).unwrap();
    assert_eq!(store.generation(&gamma).unwrap(), None);
    let again = execute(
        store,
        &Intent::CreateTenant {
            name: "gamma".into(),
            owner: principal_id("gamma:owner"),
        },
        &ctx("gamma", Principal::System, 9_200),
    )
    .unwrap();
    assert_ne!(
        again.receipt.generation, created.receipt.generation,
        "a tenant removed and created again never repeats a generation"
    );
    pair.agree("beta");
}

/// A load's generation belongs to its rows (WG-5389): while writers commit and readers load, every
/// revision a load returns is paired with the one generation the commit that wrote it produced,
/// never with an earlier or a later one; and the writers' receipts are consecutive generations of
/// one nonce.
// Verifies: STO-001
pub fn interleaved_commits_and_loads_keep_each_revision_with_one_generation(store: &dyn Store) {
    let _pair = Pair::new(store);
    let alpha = tenant_id("alpha");
    let start = store.load_at(&alpha, &[]).unwrap();
    let start_generation = start.generation.unwrap();
    let start_revision = start.state.tenant.as_deref().unwrap().revision;

    const WRITERS: usize = 2;
    const NOTES: usize = 6;
    let done = std::sync::atomic::AtomicUsize::new(0);
    let (receipts, reads) = std::thread::scope(|scope| {
        let writers: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let alpha = &alpha;
                let done = &done;
                scope.spawn(move || {
                    let mut receipts = Vec::new();
                    for n in 0..NOTES {
                        let slug = format!("interleaved-{writer}-{n}");
                        let executed = execute(
                            store,
                            &new_note(&slug, &slug),
                            &ctx(
                                alpha.as_str(),
                                user("alpha:writer1"),
                                30_000 + (writer * NOTES + n) as i64,
                            ),
                        )
                        .expect("the note commits, after a retry when it lost a race");
                        receipts.push((executed.receipt.revision, executed.receipt.generation));
                    }
                    done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    receipts
                })
            })
            .collect();
        let reader = {
            let alpha = &alpha;
            let done = &done;
            scope.spawn(move || {
                let started = std::time::Instant::now();
                let mut reads = Vec::new();
                loop {
                    // A writer that failed never counts itself done: stop looking after a minute.
                    let finished = done.load(std::sync::atomic::Ordering::SeqCst) == WRITERS
                        || started.elapsed() > std::time::Duration::from_secs(60);
                    for loaded in [
                        store.load_at(alpha, &[]).unwrap(),
                        store.load_complete_at(alpha).unwrap(),
                    ] {
                        let revision = loaded.state.tenant.as_deref().unwrap().revision;
                        reads.push((revision, loaded.generation.unwrap()));
                    }
                    if finished {
                        return reads;
                    }
                    // Leave the store to the writers between two looks.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            })
        };
        let mut receipts = Vec::new();
        for writer in writers {
            receipts.extend(writer.join().unwrap());
        }
        (receipts, reader.join().unwrap())
    });

    let mut generation_of: BTreeMap<StateRevision, Generation> = BTreeMap::new();
    generation_of.insert(start_revision, start_generation);
    for (revision, generation) in &receipts {
        assert!(
            generation_of.insert(*revision, *generation).is_none(),
            "two commits produced revision {revision:?}"
        );
    }
    assert_eq!(generation_of.len(), 1 + WRITERS * NOTES);
    for (offset, (revision, generation)) in generation_of.iter().enumerate() {
        assert_eq!(
            *generation,
            Generation {
                nonce: start_generation.nonce,
                counter: start_generation.counter + offset as u64,
            },
            "the generation of revision {revision:?} is the counter of its commit"
        );
    }
    assert!(!reads.is_empty());
    for (revision, generation) in &reads {
        assert_eq!(
            generation_of.get(revision),
            Some(generation),
            "a load returned revision {revision:?} paired with generation {generation}"
        );
    }
    let end = store.load_at(&alpha, &[]).unwrap();
    assert_eq!(
        end.generation,
        generation_of.values().next_back().copied(),
        "after the writers, the store holds the generation of the last revision"
    );
}

/// The generations, and what is derived from the rows, of one fixed schedule: every receipt's
/// revision and generation, the hashes of each journalled transition, and what each tenant holds
/// at the end. The schedule draws nonces at creation, at a restore and at a rotation.
fn schedule_trace(store: &dyn Store) -> (Vec<String>, Vec<String>) {
    let (pair, snapshot) = populated(store);
    drop(pair);
    let alpha = tenant_id("alpha");
    let mut generations = Vec::new();
    let note = |slug: &str, at: i64| -> String {
        let receipt = commit_note(store, &alpha, slug, at, None).unwrap();
        format!(
            "{slug}: revision {} generation {}",
            receipt.revision.0, receipt.generation
        )
    };
    generations.push(note("one", 40_000));
    generations.push(note("two", 40_001));
    store.restore_snapshot(&alpha, snapshot, None).unwrap();
    generations.push(format!(
        "restored: {}",
        store.generation(&alpha).unwrap().unwrap()
    ));
    generations.push(note("three", 40_002));
    generations.push(format!("rotated: {}", store.rotate_nonce(&alpha).unwrap()));
    generations.push(note("four", 40_003));
    let created = execute(
        store,
        &Intent::CreateTenant {
            name: "gamma".into(),
            owner: principal_id("gamma:owner"),
        },
        &ctx("gamma", Principal::System, 40_004),
    )
    .unwrap();
    generations.push(format!("gamma: {}", created.receipt.generation));
    for tenant in ["alpha", "beta", "gamma"] {
        generations.push(format!(
            "end {tenant}: {:?}",
            store
                .generation(&tenant_id(tenant))
                .unwrap()
                .map(|g| g.to_string())
        ));
    }
    // What a transition hashes does not mention the generation.
    let mut hashes = Vec::new();
    for tenant in ["alpha", "beta", "gamma"] {
        for entry in store.journal_after(&tenant_id(tenant), 0, 1_000).unwrap() {
            hashes.push(format!(
                "{tenant} {}: {} {}",
                entry.revision.0, entry.delta_hash, entry.events_hash
            ));
        }
    }
    (generations, hashes)
}

/// Reproducible from the seed (WG-5389): two runs of one schedule against stores seeded alike give
/// identical receipts and generations; another seed gives other generations (nonces) and the very
/// same `delta_hash` and `events_hash` of every transition, whatever the nonce.
// Verifies: STO-001
pub fn generations_are_reproducible_from_the_seed(make: &dyn Fn(u64) -> Box<dyn Store>) {
    let (first, first_hashes) = schedule_trace(make(7).as_ref());
    let (again, again_hashes) = schedule_trace(make(7).as_ref());
    assert_eq!(first, again, "one seed, one run");
    assert_eq!(first_hashes, again_hashes);
    let (other, other_hashes) = schedule_trace(make(8).as_ref());
    assert_ne!(first, other, "another seed draws other nonces");
    assert_eq!(
        first_hashes, other_hashes,
        "delta_hash and events_hash do not depend on the nonce"
    );
    assert!(!first_hashes.is_empty());
}
