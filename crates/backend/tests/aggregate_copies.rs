//! Stored aggregate results (D-5064, D-5065, AGG-030 to AGG-034, CT-310, CT-311): a result is kept
//! for an audience class that the kernel derives from the policies of the tenant, served to the
//! readers of that class and to nobody else, never served after a commit to an object type it
//! depends on, and never kept when the class is one reader alone. Also the tenant a request names
//! (contract aggregate-run-signed-in).
//!
//! The backend is driven in process: the tests build an `App` on a memory store and call
//! `strata_backend::aggregates::run` with an identity, so they can read what the copies hold.
//!
//! The tenant has `note` (a title, a status, a rank and a score), `person`, the roles `writer`,
//! `editor` and `auditor`, and a query `stats` with a count, a sum of ranks and a sum of scores.

use serde_json::Value as Json;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use strata_backend::aggregates::CopyStats;
use strata_backend::app::{ApiError, App, Config, Shared};
use strata_backend::auth::Identity;
use strata_kernel::testkit::{
    ctx, eq, field, field_def, lit, object, principal_id, role, tenant_id, text, user, var,
};
use strata_kernel::*;
use strata_protocol::api::{AggregateRunRequest, ErrorCode};
use strata_protocol::ws::SessionState;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, Store};
use strata_substrate::project::project_aggregate;

fn config() -> Config {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    Config {
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
        mail: Arc::new(strata_backend::mail::DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    }
}

/// What a tenant's Read policies and the read rule of `score` are.
#[derive(Clone, Copy, Debug)]
struct Shape {
    /// The public reads published notes.
    public_reads: bool,
    /// Writers read every note (1), their own notes (2), or none (0).
    writer_reads: u8,
    /// Editors read every note.
    editor_reads: bool,
    /// Who reads `score`: everyone (0), editors (1), auditors (2).
    score_rule: u8,
}

const TYPICAL: Shape = Shape {
    public_reads: true,
    writer_reads: 1,
    editor_reads: true,
    score_rule: 1,
};

const NAMES: [&str; 3] = ["writer", "editor", "auditor"];

fn policy(id: &str, role_name: &str, action: Action, condition: Option<Expr>) -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new(id).unwrap(),
        role: role(role_name),
        resource: Resource::Object(object("note")),
        actions: [action].into_iter().collect(),
        condition,
    })
}

fn aggregate(id: &str, kind: AggregateKind, field_name: Option<&str>) -> AggregateDef {
    AggregateDef {
        id: AggregateId::new(id).unwrap(),
        kind,
        field: field_name.map(field),
        group_by: None,
        n: None,
        descending: false,
        scan_bound: 1_000,
    }
}

fn schema_changes(shape: Shape) -> Vec<SchemaChange> {
    let either =
        |name: &str| Expr::Or(vec![Expr::HasRole(role(name)), Expr::HasRole(owner_role())]);
    let mut changes: Vec<SchemaChange> = NAMES
        .iter()
        .map(|name| {
            SchemaChange::DefineRole(RoleDef {
                id: role(name),
                label: name.to_string(),
            })
        })
        .collect();
    changes.extend([
        SchemaChange::DefineObject {
            id: object("note"),
            label: "Note".into(),
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: field_def("title", FieldType::Str, true),
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: FieldDef {
                default: Some(lit("draft")),
                ..field_def("status", FieldType::Str, true)
            },
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: field_def("rank", FieldType::Int, false),
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: FieldDef {
                read: match shape.score_rule {
                    0 => None,
                    1 => Some(either("editor")),
                    _ => Some(either("auditor")),
                },
                ..field_def("score", FieldType::Int, false)
            },
        },
        SchemaChange::DefineObject {
            id: object("person"),
            label: "Person".into(),
        },
        SchemaChange::DefineField {
            object: object("person"),
            field: field_def("name", FieldType::Str, true),
        },
        policy("anyone_creates", "public", Action::Create, None),
        SchemaChange::DefinePolicy(PolicyDef {
            id: PolicyId::new("anyone_adds_people").unwrap(),
            role: role("public"),
            resource: Resource::Object(object("person")),
            actions: [Action::Create].into_iter().collect(),
            condition: None,
        }),
    ]);
    if shape.public_reads {
        changes.push(policy(
            "public_reads",
            "public",
            Action::Read,
            Some(eq(var(Scope::Record, "status"), lit("published"))),
        ));
    }
    match shape.writer_reads {
        1 => changes.push(policy("writer_reads", "writer", Action::Read, None)),
        2 => changes.push(policy(
            "writer_reads",
            "writer",
            Action::Read,
            Some(eq(var(Scope::Record, "$created_by"), Expr::Principal)),
        )),
        _ => {}
    }
    if shape.editor_reads {
        changes.push(policy("editor_reads", "editor", Action::Read, None));
    }
    changes.push(SchemaChange::DefineQuery(QueryDef {
        id: QueryId::new("stats").unwrap(),
        object: object("note"),
        filter: None,
        params: Default::default(),
        order: vec![],
        limit: None,
        aggregates: Some(vec![
            aggregate("count_all", AggregateKind::Count, None),
            aggregate("sum_rank", AggregateKind::Sum, Some("rank")),
            aggregate("sum_score", AggregateKind::Sum, Some("score")),
        ]),
        declared_fields: Some(vec![field("rank"), field("status"), field("score")]),
    }));
    changes
}

fn member(tenant: &str, i: usize) -> Principal {
    user(&format!("{tenant}:u{i}"))
}

fn boss(tenant: &str) -> Principal {
    user(&format!("{tenant}:owner"))
}

struct Site {
    store: Arc<MemoryStore>,
    app: Shared,
    clock: Cell<i64>,
}

impl Site {
    fn new() -> Site {
        let store = Arc::new(MemoryStore::new());
        let app = App::new(config(), store.clone());
        Site {
            store,
            app,
            clock: Cell::new(1_000),
        }
    }

    fn context(&self, tenant: &str, who: Principal) -> Context {
        self.clock.set(self.clock.get() + 1);
        ctx(tenant, who, self.clock.get())
    }

    fn exec(&self, tenant: &str, who: Principal, intent: Intent) -> Output {
        let context = self.context(tenant, who);
        execute(&*self.store, &intent, &context)
            .unwrap_or_else(|e| panic!("refused: {e}"))
            .transition
            .output
    }

    /// Found a tenant of this shape whose four members hold the given roles (bits of `NAMES`).
    fn found(&self, tenant: &str, shape: Shape, held: [u8; 4]) {
        self.exec(
            tenant,
            Principal::System,
            Intent::CreateTenant {
                name: tenant.to_string(),
                owner: principal_id(&format!("{tenant}:owner")),
            },
        );
        self.exec(
            tenant,
            boss(tenant),
            Intent::ChangeSchema {
                changes: schema_changes(shape),
                expected_version: None,
                existing: ExistingData::Reject,
            },
        );
        for (i, mask) in held.iter().enumerate() {
            for (bit, name) in NAMES.iter().enumerate() {
                if mask & (1 << bit) != 0 {
                    self.exec(
                        tenant,
                        boss(tenant),
                        Intent::AssignRole {
                            principal: principal_id(&format!("{tenant}:u{i}")),
                            role: role(name),
                        },
                    );
                }
            }
        }
    }

    fn note(&self, tenant: &str, who: Principal, rank: i64, published: bool) -> RecordId {
        let fields = BTreeMap::from([
            (field("title"), text("t")),
            (
                field("status"),
                text(if published { "published" } else { "draft" }),
            ),
            (field("rank"), Value::Int(rank)),
            (field("score"), Value::Int(rank * 10)),
        ]);
        match self.exec(
            tenant,
            who,
            Intent::CreateRecord {
                object: object("note"),
                fields,
            },
        ) {
            Output::Record { id, .. } => id,
            other => panic!("a created record is the output, not {other:?}"),
        }
    }

    fn identity(tenant: &str, who: &Principal) -> Identity {
        Identity {
            tenant: tenant_id(tenant),
            principal: who.clone(),
            session: None,
            state: SessionState::Anonymous,
            correlation: "t-aggregate-copies".into(),
            verified_address: None,
            idempotency_key: None,
        }
    }

    fn request(named: Option<&str>, aggregate: &str) -> AggregateRunRequest {
        AggregateRunRequest {
            tenant: named.map(str::to_string),
            query: "stats".into(),
            aggregate: aggregate.into(),
            filter: Default::default(),
        }
    }

    /// The route's answer for a reader.
    fn ask(&self, tenant: &str, who: &Principal, aggregate: &str) -> Json {
        let identity = Site::identity(tenant, who);
        strata_backend::aggregates::run(&self.app, &identity, &Site::request(None, aggregate))
            .ok()
            .expect("the aggregate is answered")
    }

    /// The same aggregate computed by the kernel for a reader, from the whole tenant.
    fn fresh(&self, tenant: &str, who: &Principal, aggregate_id: &str) -> Json {
        let state = self.store.load_complete(&tenant_id(tenant)).unwrap();
        let request = AggregateRequest {
            query: QueryId::new("stats").unwrap(),
            aggregate: AggregateId::new(aggregate_id).unwrap(),
            params: Default::default(),
            filter: Default::default(),
        };
        let result = aggregates::run(&state, &request, &ctx(tenant, who.clone(), 5_000_000))
            .unwrap_or_else(|e| panic!("{}", e.describe()));
        project_aggregate(&result)
    }

    fn copies(&self) -> CopyStats {
        self.app.aggregate_copies.stats()
    }
}

fn count(answer: &Json) -> i64 {
    answer["groups"][0]["value"].as_i64().unwrap()
}

// Verifies: AGG-030
// Verifies: AGG-032
// Verifies: AGG-033
#[test]
fn two_readers_with_the_same_roles_share_one_stored_result_and_another_role_does_not() {
    let site = Site::new();
    // u0 and u1 are writers, u2 is an editor, u3 holds no role.
    site.found("alpha", TYPICAL, [0b001, 0b001, 0b010, 0]);
    site.note("alpha", boss("alpha"), 1, true);
    site.note("alpha", boss("alpha"), 2, false);

    let first = site.ask("alpha", &member("alpha", 0), "count_all");
    assert_eq!(
        site.copies(),
        CopyStats {
            held: 1,
            served: 0,
            stored: 1
        }
    );
    let second = site.ask("alpha", &member("alpha", 1), "count_all");
    assert_eq!(
        site.copies(),
        CopyStats {
            held: 1,
            served: 1,
            stored: 1
        },
        "the second writer is served what the first one's request stored"
    );
    assert_eq!(first, second);
    assert_eq!(count(&first), 2, "writers read every note");

    // An editor has another class: the result is computed and stored again, and an editor reads
    // the same notes but is not served a copy made for writers.
    let editor = site.ask("alpha", &member("alpha", 2), "count_all");
    assert_eq!(
        site.copies(),
        CopyStats {
            held: 2,
            served: 1,
            stored: 2
        }
    );
    assert_eq!(count(&editor), 2);

    // The public reads published notes only, and a person with no role is the public.
    let public = site.ask("alpha", &Principal::Anonymous, "count_all");
    assert_eq!(count(&public), 1);
    assert_eq!(
        site.copies(),
        CopyStats {
            held: 3,
            served: 1,
            stored: 3
        }
    );
    let nobody = site.ask("alpha", &member("alpha", 3), "count_all");
    assert_eq!(nobody, public);
    assert_eq!(
        site.copies(),
        CopyStats {
            held: 3,
            served: 2,
            stored: 3
        }
    );
    // Another aggregate of the same query is another copy.
    site.ask("alpha", &member("alpha", 0), "sum_rank");
    assert_eq!(site.copies().held, 4);
}

// Verifies: AGG-2860
// Verifies: AGG-2861
// Verifies: AGG-034
#[test]
fn a_policy_that_mentions_the_reader_makes_the_result_individual_and_nothing_is_stored() {
    let site = Site::new();
    let shape = Shape {
        writer_reads: 2,
        ..TYPICAL
    };
    site.found("alpha", shape, [0b001, 0b001, 0b010, 0]);
    site.note("alpha", member("alpha", 0), 1, false);
    site.note("alpha", member("alpha", 1), 2, false);
    site.note("alpha", member("alpha", 1), 3, false);

    let a = site.ask("alpha", &member("alpha", 0), "count_all");
    let b = site.ask("alpha", &member("alpha", 1), "count_all");
    let a_again = site.ask("alpha", &member("alpha", 0), "count_all");
    assert_eq!(
        (count(&a), count(&b)),
        (1, 2),
        "each writer counts their own notes"
    );
    assert_eq!(a, a_again);
    assert_eq!(
        site.copies(),
        CopyStats {
            held: 0,
            served: 0,
            stored: 0
        },
        "a writer's result is that writer's alone, so nothing is stored for it"
    );
    // A reader whom the policy does not concern still shares.
    site.ask("alpha", &Principal::Anonymous, "count_all");
    site.ask("alpha", &Principal::Anonymous, "count_all");
    assert_eq!(
        site.copies(),
        CopyStats {
            held: 1,
            served: 1,
            stored: 1
        }
    );
}

// Verifies: AGG-031
// Verifies: AGG-2862
#[test]
fn a_commit_to_an_object_type_the_aggregate_reads_is_never_followed_by_a_stale_answer() {
    let site = Site::new();
    site.found("alpha", TYPICAL, [0b001, 0, 0, 0]);
    let first = site.note("alpha", boss("alpha"), 1, true);
    let reader = member("alpha", 0);

    assert_eq!(count(&site.ask("alpha", &reader, "count_all")), 1);
    assert_eq!(count(&site.ask("alpha", &reader, "count_all")), 1);
    assert_eq!(site.copies().served, 1);

    // A write, then a read straight after: the answer already holds it.
    site.note("alpha", boss("alpha"), 2, true);
    let served_before = site.copies().served;
    assert_eq!(count(&site.ask("alpha", &reader, "count_all")), 2);
    assert_eq!(
        site.copies().served,
        served_before,
        "the old copy was not found"
    );
    assert_eq!(count(&site.ask("alpha", &reader, "count_all")), 2);
    assert_eq!(site.copies().served, served_before + 1);

    // An update and a delete are commits to the object type as well.
    site.exec(
        "alpha",
        boss("alpha"),
        Intent::UpdateRecord {
            object: object("note"),
            id: first,
            fields: BTreeMap::from([(field("rank"), Value::Int(40))]),
            expected_version: None,
        },
    );
    let sum = site.ask("alpha", &reader, "sum_rank");
    assert_eq!(sum, site.fresh("alpha", &reader, "sum_rank"));
    assert_eq!(sum["groups"][0]["value"], 42);
    site.exec(
        "alpha",
        boss("alpha"),
        Intent::DeleteRecord {
            object: object("note"),
            id: first,
            expected_version: None,
        },
    );
    assert_eq!(count(&site.ask("alpha", &reader, "count_all")), 1);

    // A commit to an object type the aggregate does not read leaves the copy valid: the version is
    // the counter of the object types it depends on, not the revision of the tenant.
    site.ask("alpha", &reader, "count_all");
    let served = site.copies().served;
    site.exec(
        "alpha",
        boss("alpha"),
        Intent::CreateRecord {
            object: object("person"),
            fields: BTreeMap::from([(field("name"), text("Ada"))]),
        },
    );
    let after = site.ask("alpha", &reader, "count_all");
    assert_eq!(site.copies().served, served + 1);
    // The revision the answer states is the tenant's now, and the answer is what a fresh run gives.
    assert_eq!(after, site.fresh("alpha", &reader, "count_all"));

    // A change of the schema is a new key too.
    site.exec(
        "alpha",
        boss("alpha"),
        Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineField {
                object: object("note"),
                field: field_def("body", FieldType::Str, false),
            }],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    let served = site.copies().served;
    assert_eq!(count(&site.ask("alpha", &reader, "count_all")), 1);
    assert_eq!(site.copies().served, served);
}

// Verifies: AGG-032
#[test]
fn a_field_read_rule_that_differs_between_two_roles_gives_each_its_own_stored_result() {
    let site = Site::new();
    // u0 is an auditor; u1 holds nothing. Only auditors read `score`.
    let shape = Shape {
        score_rule: 2,
        ..TYPICAL
    };
    site.found("alpha", shape, [0b100, 0, 0, 0]);
    site.note("alpha", boss("alpha"), 3, true);
    site.note("alpha", boss("alpha"), 4, true);

    let auditor = site.ask("alpha", &member("alpha", 0), "sum_score");
    let nobody = site.ask("alpha", &member("alpha", 1), "sum_score");
    assert_eq!(auditor["groups"][0]["value"], 70);
    assert_eq!(
        nobody["groups"][0]["value"], 0,
        "a note whose score the reader may not read is left out, not counted with a hidden value"
    );
    assert_eq!(
        site.copies(),
        CopyStats {
            held: 2,
            served: 0,
            stored: 2
        }
    );
    // Each is then served to its own class only.
    assert_eq!(site.ask("alpha", &member("alpha", 0), "sum_score"), auditor);
    assert_eq!(site.ask("alpha", &member("alpha", 1), "sum_score"), nobody);
    assert_eq!(site.copies().served, 2);
}

// Verifies: AGG-040
#[test]
fn a_copy_is_never_served_to_another_tenant() {
    let site = Site::new();
    site.found("alpha", TYPICAL, [0b001, 0, 0, 0]);
    site.found("beta", TYPICAL, [0b001, 0, 0, 0]);
    site.note("alpha", boss("alpha"), 1, true);
    site.note("beta", boss("beta"), 1, true);
    site.note("beta", boss("beta"), 2, true);

    // The same roles in two tenants of one schema: the classes are equal, the keys are not.
    let in_alpha = site.ask("alpha", &member("alpha", 0), "count_all");
    let in_beta = site.ask("beta", &member("beta", 0), "count_all");
    assert_eq!((count(&in_alpha), count(&in_beta)), (1, 2));
    assert_eq!(
        site.copies(),
        CopyStats {
            held: 2,
            served: 0,
            stored: 2
        }
    );
    let public_alpha = site.ask("alpha", &Principal::Anonymous, "count_all");
    let public_beta = site.ask("beta", &Principal::Anonymous, "count_all");
    assert_eq!((count(&public_alpha), count(&public_beta)), (1, 2));
    assert_eq!(site.copies().served, 0);

    // A commit in one tenant leaves the other tenant's copy valid.
    site.note("alpha", boss("alpha"), 3, true);
    assert_eq!(count(&site.ask("beta", &member("beta", 0), "count_all")), 2);
    assert_eq!(site.copies().served, 1);
    assert_eq!(
        count(&site.ask("alpha", &member("alpha", 0), "count_all")),
        2
    );
}

// Verifies: TEN-1140
#[test]
fn a_request_that_names_another_tenant_is_refused_as_a_tenant_mismatch() {
    let site = Site::new();
    site.found("alpha", TYPICAL, [0b001, 0, 0, 0]);
    site.found("beta", TYPICAL, [0b001, 0, 0, 0]);
    let identity = Site::identity("alpha", &member("alpha", 0));
    let refused: ApiError = strata_backend::aggregates::run(
        &site.app,
        &identity,
        &Site::request(Some("beta"), "count_all"),
    )
    .err()
    .expect("another tenant is refused");
    assert!(matches!(refused.0.code, ErrorCode::Forbidden));
    assert_eq!(refused.0.kind.as_deref(), Some("tenant_mismatch"));
    assert_eq!(site.copies(), CopyStats::default());
    // The refusal is an audit entry of the tenant the request was made at, with the request's
    // correlation identifier.
    let lines = site
        .store
        .audit_list(&tenant_id("alpha"), Some("audit"), 10)
        .expect("the audit trail");
    let entry = lines
        .iter()
        .find(|line| line.event == "aggregate.tenant_mismatch")
        .expect("the refusal is an audit entry");
    assert_eq!(entry.correlation, "t-aggregate-copies");
    let data: Json = serde_json::from_str(&entry.data).expect("the entry data is JSON");
    assert_eq!(data["tenant"], "alpha");
    assert!(
        site.store
            .audit_list(&tenant_id("beta"), Some("audit"), 10)
            .expect("the other tenant's trail")
            .is_empty(),
        "nothing is written to the tenant that was named"
    );
    // The tenant of the request, named, is accepted.
    strata_backend::aggregates::run(
        &site.app,
        &identity,
        &Site::request(Some("alpha"), "count_all"),
    )
    .ok()
    .expect("the tenant of the request is accepted");
}

// ---- the property: a result served from the store equals the result computed fresh ----------------

/// A fixed-seed generator (xorshift64*), so a run is the same run every time.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n
    }
}

// Verifies: AGG-031
// Verifies: AGG-032
// Verifies: AGG-033
#[test]
fn a_result_served_from_the_store_equals_the_result_computed_fresh_for_that_reader() {
    let mut rng = Rng(0x2026_1121_a661_0001);
    let aggregates = ["count_all", "sum_rank", "sum_score"];
    let mut served = 0;
    let mut individual_cases = 0;
    for case in 0..40 {
        // A random schema, with and without a policy or a rule that mentions the reader, and
        // random roles for the four members.
        let shape = Shape {
            public_reads: rng.below(4) != 0,
            writer_reads: rng.below(3) as u8,
            editor_reads: rng.below(2) == 0,
            score_rule: rng.below(3) as u8,
        };
        individual_cases += usize::from(shape.writer_reads == 2);
        let held = [
            rng.below(8) as u8,
            rng.below(8) as u8,
            rng.below(8) as u8,
            rng.below(8) as u8,
        ];
        let site = Site::new();
        site.found("alpha", shape, held);
        let readers: Vec<Principal> = std::iter::once(boss("alpha"))
            .chain((0..4).map(|i| member("alpha", i)))
            .chain([Principal::Anonymous])
            .collect();
        let mut notes: Vec<RecordId> = Vec::new();
        for step in 0..14 {
            // A write, a read straight after it, or two reads in a row.
            match rng.below(5) {
                0 => {
                    let who = if rng.below(3) == 0 {
                        boss("alpha")
                    } else {
                        member("alpha", rng.below(4) as usize)
                    };
                    let rank = rng.below(9) as i64;
                    notes.push(site.note("alpha", who, rank, rng.below(2) == 0));
                }
                1 if !notes.is_empty() => {
                    let id = notes[rng.below(notes.len() as u64) as usize];
                    site.exec(
                        "alpha",
                        boss("alpha"),
                        Intent::UpdateRecord {
                            object: object("note"),
                            id,
                            fields: BTreeMap::from([
                                (field("status"), text("published")),
                                (field("rank"), Value::Int(rng.below(9) as i64)),
                            ]),
                            expected_version: None,
                        },
                    );
                }
                2 if !notes.is_empty() => {
                    let at = rng.below(notes.len() as u64) as usize;
                    let id = notes.remove(at);
                    site.exec(
                        "alpha",
                        boss("alpha"),
                        Intent::DeleteRecord {
                            object: object("note"),
                            id,
                            expected_version: None,
                        },
                    );
                }
                _ => {}
            }
            let reader = &readers[rng.below(readers.len() as u64) as usize];
            let which = aggregates[rng.below(3) as usize];
            for _ in 0..2 {
                assert_eq!(
                    site.ask("alpha", reader, which),
                    site.fresh("alpha", reader, which),
                    "case {case} step {step}: {which} as {reader:?} under {shape:?} with {held:?}"
                );
            }
        }
        served += site.copies().served;
    }
    assert!(
        served > 0,
        "the store served nothing: the property tested nothing"
    );
    assert!(individual_cases > 0, "no schema mentioned the reader");
}
