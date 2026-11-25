#![allow(clippy::err_expect)] // `unwrap_err` would print a whole world when it fails.
//! The bundle of a product (`tenant-bundle`, D-6580): it is read, the sections the kernel can take
//! are installed into a tenant that exists, the sections it cannot take are refused by name, and
//! the install is idempotent and records which product owns each entity (D-6582, D-6583).

use serde_json::{json, Value as Json};
use strata_kernel::*;
use strata_substrate::bundle::{
    install_into, install_product, plan_install, remove_product, Bundle, BundleError,
    BundleErrorKind, InstalledRecord, Planned, ProductBundle, ProductInstall, ProductInstallError,
    ProductTarget,
};

fn tenant() -> TenantId {
    TenantId::new("acme").unwrap()
}

fn admin() -> Principal {
    Principal::User(PrincipalId::new("user:admin").unwrap())
}

/// A tenant that exists and has no product.
fn world() -> World {
    let doc = json!({
        "id": "acme",
        "name": "Acme",
        "owner": "user:admin",
        "created_at": "2026-01-01T00:00:00Z"
    });
    let bundle = Bundle::from_json(&doc.to_string()).unwrap();
    install_into(&World::default(), &bundle).unwrap_or_else(|e| panic!("{e}"))
}

/// A small product in the shape specc writes: an object type with a constraint, a role with
/// permissions in each of the three ways a bundle writes one, a policy, a query and a workflow.
fn tiers() -> Json {
    json!({
        "name": "Tiers",
        "version": "1.0.0",
        "product": "tiers",
        "platform": { "min": "0.2", "below": "0.4" },
        "content": {
            "objects": {
                "tiers__member": {
                    "id": "tiers__member",
                    "label": "Member",
                    "fields": {
                        "name": { "id": "name", "label": "Name", "ty": "Str", "required": true, "unique": false },
                        "points": { "id": "points", "label": "Points", "ty": "Int", "required": true, "unique": false, "default": "0" },
                        "status": { "id": "status", "label": "Status", "ty": "Str", "required": false, "unique": false }
                    },
                    "constraints": {
                        "points_not_negative": { "id": "points_not_negative", "expr": "new.points >= 0", "message": "Points are not negative." }
                    }
                }
            },
            "roles": { "tiers__editor": { "id": "tiers__editor", "label": "Tier editor" } },
            "role-permissions": {
                "tiers__editor": [
                    "content-read",
                    { "object": "tiers__member", "actions": ["read", "create", "update"] },
                    { "Capability": "content-write" }
                ]
            },
            "policies": {
                "tiers__editor_members": {
                    "id": "tiers__editor_members",
                    "role": "tiers__editor",
                    "resource": { "Object": "tiers__member" },
                    "actions": ["Read", "Create", "Update"],
                    "condition": "record.points >= 0"
                }
            },
            "queries": {
                "tiers__top": {
                    "id": "tiers__top",
                    "object": "tiers__member",
                    "filter": "record.points >= 100",
                    "params": [],
                    "order": [{ "field": "points", "descending": true }],
                    "limit": 10
                }
            },
            "workflows": {
                "tiers__promotion": {
                    "id": "tiers__promotion",
                    "object": "tiers__member",
                    "field": "status",
                    "initial": ["new"],
                    "transitions": [
                        { "name": "promote", "from": "new", "to": "gold", "guard": "record.points >= 100", "set": {} }
                    ]
                }
            }
        }
    })
}

fn read(doc: &Json) -> Result<ProductBundle, BundleError> {
    ProductBundle::from_json(&doc.to_string())
}

fn refused(doc: &Json) -> BundleError {
    read(doc).err().expect("the bundle is refused")
}

type Installed = Result<ProductInstall, ProductInstallError<DomainError>>;

/// Installs into `world`, which takes the schema change when there is one. Returns the answer and
/// how many intents were run.
fn install(
    world: &mut World,
    bundle: &ProductBundle,
    installed: Option<&InstalledRecord>,
    release: &str,
) -> (Installed, usize) {
    let tenant = tenant();
    let schema = world.tenants[&tenant].schema.clone();
    let target = ProductTarget {
        tenant: &tenant,
        principal: admin(),
        now: 1_800_000_000_000,
        release,
        schema: &schema,
        installed,
        expected_version: None,
        idempotency_key: None,
    };
    let mut calls = 0;
    let result = install_product(bundle, &target, &mut |ctx: Context,
                                                        intent: Intent|
     -> Result<Output, DomainError> {
        calls += 1;
        let (next, outcome) = world.step(&intent, &ctx)?;
        *world = next;
        Ok(outcome.output)
    });
    (result, calls)
}

fn refusal(result: Installed) -> BundleError {
    match result {
        Err(ProductInstallError::Refused(error)) => error,
        other => panic!("the install was not refused: {other:?}"),
    }
}

fn object_id(id: &str) -> ObjectTypeId {
    ObjectTypeId::new(id).unwrap()
}

/// The bundle of `tiers` with one segment declared in its namespace (WG-5166).
fn tiers_with_segment() -> Json {
    let mut doc = tiers();
    doc["content"]["segments"] = json!({
        "tiers__gold": {
            "name": "Gold",
            "subject-type": "tiers__member",
            "predicate": "record.points >= 100",
            "principal-field": "name",
            "visible-to-members": true
        }
    });
    doc
}

fn gold_key() -> strata_kernel::segments::SegmentKey {
    strata_kernel::segments::SegmentKey::new("tiers__gold").unwrap()
}

// Verifies: INT-155
#[test]
fn a_segments_section_is_read_planned_and_owned_but_is_no_schema_change() {
    let doc = tiers_with_segment();
    let bundle = read(&doc).unwrap();
    assert!(bundle.unbuilt.is_empty(), "no section is refused any more");
    assert_eq!(bundle.segments.len(), 1);
    // The predicate is read with the rule language: a bundle whose predicate does not parse is
    // refused when it is read.
    let mut broken = tiers_with_segment();
    broken["content"]["segments"]["tiers__gold"]["predicate"] = json!("record.points >=");
    assert_eq!(refused(&broken).kind, BundleErrorKind::InvalidBundle);

    let mut world = world();
    let schema = world.tenants[&tenant()].schema.clone();
    let owner = tenant();
    let target = target_of(&owner, &schema, "0.2");
    let Planned::Change(plan) = plan_install(&bundle, &target).unwrap() else {
        panic!("the install is a change")
    };
    // The segment is defined through the segment store, with the id derived from the tenant and
    // the key (D-6582); the schema change carries none of it.
    assert_eq!(plan.segments.define.len(), 1);
    assert!(plan.segments.remove.is_empty());
    let defs = plan.segments.definitions(&tenant()).unwrap();
    assert_eq!(
        defs[0].id,
        strata_kernel::segments::derive_segment_id(&tenant(), &gold_key())
    );
    assert_eq!(defs[0].name, "Gold");
    assert!(defs[0].visible_to_members);
    assert_eq!(defs[0].principal_field, FieldId::new("name").unwrap());
    assert!(plan
        .created
        .iter()
        .any(|e| e.kind == "segment" && e.id == "tiers__gold"));
    // The ownership record the one schema change writes lists it.
    let record = plan.record.clone().expect("a product leaves a record");
    assert!(record
        .owns
        .iter()
        .any(|e| e.kind == "segment" && e.id == "tiers__gold"));

    let (result, calls) = install(&mut world, &bundle, None, "0.2");
    result.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(calls, 1, "one schema change for the whole bundle");
    let held = InstalledRecord::of(&world.tenants[&tenant()].schema, "tiers").unwrap();
    assert!(held
        .owns
        .iter()
        .any(|e| e.kind == "segment" && e.id == "tiers__gold"));
    // The same bundle again is the same install: nothing runs.
    let (again, calls) = install(&mut world, &bundle, None, "0.2");
    assert!(matches!(again, Ok(ProductInstall::Unchanged { .. })));
    assert_eq!(calls, 0);
}

fn owned(record: &InstalledRecord) -> Vec<(String, String)> {
    record
        .owns
        .iter()
        .map(|e| (e.kind.clone(), e.id.clone()))
        .collect()
}

// The bundles specc writes from the fixture products (tests/bundle.rs of the spec-compiler).
const LEDGER_TRIGGER: &str =
    include_str!("../../spec-compiler/tests/fixtures/products/expected/ledger-trigger.bundle.json");

// ---- what a product holds beyond its record types (WG-5076, WG-5084, WG-5103) ----------------------

fn schema_of(world: &World) -> std::sync::Arc<TenantSchema> {
    world.tenants[&tenant()].schema.clone()
}

fn context_at(now: i64, correlation: &str) -> Context {
    Context {
        tenant: tenant(),
        principal: admin(),
        session: None,
        now: Timestamp(now),
        correlation: correlation.into(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

/// A tenant that exists and has the role `member`, which the policies of a product name.
fn world_with_member() -> World {
    let intent = Intent::ChangeSchema {
        changes: vec![SchemaChange::DefineRole(RoleDef {
            id: RoleId::new("member").unwrap(),
            label: "Member".into(),
        })],
        expected_version: None,
        existing: ExistingData::Reject,
    };
    world()
        .step(&intent, &context_at(1_700_000_000_000, "fixture"))
        .expect("accepted")
        .0
}

type Executor<'a> = &'a mut dyn FnMut(Context, Intent) -> Result<Output, DomainError>;

fn target_of<'a>(
    tenant: &'a TenantId,
    schema: &'a TenantSchema,
    release: &'a str,
) -> ProductTarget<'a> {
    ProductTarget {
        tenant,
        principal: admin(),
        now: 1_800_000_000_000,
        release,
        schema,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    }
}

/// Runs `work` against the tenant of `world`, which takes each intent as it comes. Returns the
/// answer and how many intents were run.
fn with_target<R>(
    world: &mut World,
    release: &str,
    work: impl FnOnce(&ProductTarget<'_>, Executor<'_>) -> R,
) -> (R, usize) {
    let tenant = tenant();
    let schema = world.tenants[&tenant].schema.clone();
    let target = target_of(&tenant, &schema, release);
    let mut calls = 0;
    let result = work(&target, &mut |ctx: Context,
                                     intent: Intent|
     -> Result<Output, DomainError> {
        calls += 1;
        let (next, outcome) = world.step(&intent, &ctx)?;
        *world = next;
        Ok(outcome.output)
    });
    (result, calls)
}

fn installed(world: &mut World, doc: &Json) -> InstalledRecord {
    let bundle = read(doc).unwrap();
    let (result, _) = with_target(world, "0.2", |t, e| install_product(&bundle, t, e));
    match result.unwrap_or_else(|e| panic!("{e}")) {
        ProductInstall::Applied {
            record: Some(record),
            ..
        } => record,
        other => panic!("the install was not applied: {other:?}"),
    }
}

/// The product that reacts to the ledger: its object type, templates, trigger and automation.
fn ledger_trigger() -> Json {
    serde_json::from_str(LEDGER_TRIGGER).unwrap()
}

// Verifies: INT-1921
#[test]
fn ledger_trigger_installs_with_its_templates_its_trigger_and_its_automation() {
    let mut world = world_with_member();
    let record = installed(&mut world, &ledger_trigger());
    let schema = schema_of(&world);
    assert!(schema
        .objects
        .contains_key(&object_id("ledger_trigger__crossing")));
    assert_eq!(schema.triggers.as_ref().map(|t| t.len()), Some(1));
    assert_eq!(schema.automations.as_ref().map(|t| t.len()), Some(1));
    let templates: Vec<String> = strata_kernel::templates::list(&schema)
        .into_iter()
        .map(|t| t.id.to_string())
        .collect();
    assert_eq!(
        templates,
        vec![
            "ledger_trigger__crossing_mail",
            "ledger_trigger__crossing_notice"
        ]
    );
    // The record names what the product owns, templates included, and the schema holds it.
    let kinds: Vec<&str> = record.owns.iter().map(|e| e.kind.as_str()).collect();
    for kind in [
        "role",
        "object",
        "policy",
        "mail-template",
        "notice-template",
        "trigger",
        "automation",
    ] {
        assert!(kinds.contains(&kind), "{kind}: {kinds:?}");
    }
    assert_eq!(InstalledRecord::of(&schema, "ledger-trigger"), Some(record));
}

// Verifies: INT-2344
#[test]
fn an_automation_that_sends_a_template_nobody_declares_is_refused_before_it_runs() {
    let mut doc = tiers();
    doc["content"]["automations"] = json!([{
        "id": "tiers__greet",
        "label": "Greet",
        "run-as": { "roles": [] },
        "start": "send",
        "steps": { "send": { "action": { "SendMail": { "template": "tiers__nothing", "to": "param.owner" } } } }
    }]);
    let bundle = read(&doc).unwrap();
    let mut world = world();
    let (result, calls) = install(&mut world, &bundle, None, "0.2");
    let error = refusal(result);
    assert_eq!(error.kind, BundleErrorKind::InvalidBundle);
    assert!(
        error.message.contains("tiers__nothing"),
        "{}",
        error.message
    );
    assert_eq!(calls, 0);
}

// ---- upgrade (INT-1581, INT-1583) --------------------------------------------------------------------

// ---- removal (INT-1581, INT-1582, INT-1583) -------------------------------------------------------------

// ---- the whole-tenant bundle carries the same sections -----------------------------------------------------

/// `tiers` with an aggregate pair on its query and a view section that names one of them, in the
/// shape specc writes (members in the specification's kebab case).
fn tiers_with_aggregates() -> Json {
    let mut doc = tiers();
    doc["content"]["queries"]["tiers__top"]["declared-fields"] = json!(["points", "status"]);
    doc["content"]["queries"]["tiers__top"]["aggregates"] = json!([
        { "id": "total", "kind": "sum", "field": "points", "descending": false, "scan-bound": 500 },
        { "id": "by_status", "kind": "count", "group-by": "status", "descending": false, "scan-bound": 500 }
    ]);
    doc["content"]["views"] = json!({
        "tiers__board": {
            "id": "tiers__board",
            "route": "/board",
            "title": "Board",
            "sections": {
                "total": {
                    "query": "tiers__top", "args": {}, "include": [], "markdown": [],
                    "single": false, "aggregate": "total"
                }
            }
        }
    });
    doc
}

// Verifies: AGG-001
// Verifies: AGG-041
#[test]
fn a_product_query_declares_aggregates_and_a_view_section_names_one() {
    let bundle = read(&tiers_with_aggregates()).unwrap();
    let query = &bundle.queries[&QueryId::new("tiers__top").unwrap()];
    assert_eq!(query.aggregates.as_ref().map(Vec::len), Some(2));
    assert_eq!(query.declared_fields.as_ref().map(Vec::len), Some(2));
    let mut world = world();
    let (result, _) = install(&mut world, &bundle, None, "0.2-draft");
    result.unwrap_or_else(|e| panic!("{e}"));
    let schema = schema_of(&world);
    let installed = &schema.queries[&QueryId::new("tiers__top").unwrap()];
    let aggregates = installed.aggregates.as_ref().unwrap();
    assert_eq!(aggregates[0].kind, AggregateKind::Sum);
    assert_eq!(aggregates[0].scan_bound, 500);
    assert_eq!(
        aggregates[1].group_by,
        Some(FieldId::new("status").unwrap())
    );
    let section = &schema.views[&ViewId::new("tiers__board").unwrap()].sections["total"];
    assert_eq!(section.aggregate, Some(AggregateId::new("total").unwrap()));
}

// Verifies: CTK-005
#[test]
fn a_removal_deletes_the_templates_and_tokens_the_record_owns() {
    let token =
        json!([{ "name": "tiers__first_name", "expression": "record.name", "fallback": "friend" }]);
    let mut doc = with_notice(PLACES_A_TOKEN, token);
    doc["content"]["mail-templates"] = json!([{
        "template": "tiers__welcome",
        "subject": "Welcome",
        "body": "[{\"type\":\"paragraph\",\"inlines\":[{\"type\":\"text\",\"text\":\"Hello.\"}]}]"
    }]);
    let mut world = world();
    let record = installed(&mut world, &doc);
    let owns = owned(&record);
    for (kind, id) in [
        ("token", "tiers__first_name"),
        ("notice-template", "tiers__promoted"),
        ("mail-template", "tiers__welcome"),
    ] {
        assert!(
            owns.contains(&(kind.to_string(), id.to_string())),
            "{kind} {id}: {owns:?}"
        );
    }
    let schema = schema_of(&world);
    assert_eq!(strata_kernel::templates::tokens(&schema).len(), 1);
    assert_eq!(strata_kernel::templates::list(&schema).len(), 2);

    let (result, calls) = with_target(&mut world, "0.2", |t, e| {
        remove_product("tiers", false, t, e)
    });
    result.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(calls, 1);
    let schema = schema_of(&world);
    assert!(strata_kernel::templates::tokens(&schema).is_empty());
    assert!(strata_kernel::templates::list(&schema).is_empty());
    assert!(InstalledRecord::of(&schema, "tiers").is_none());
}

// ---- the blocks of a template (D-7900) --------------------------------------------------------

/// A product with one notice template whose body is `body`, and the tokens it defines.
fn with_notice(body: &str, tokens: Json) -> Json {
    let mut doc = tiers();
    doc["content"]["notice-templates"] = json!([{
        "template": "tiers__promoted",
        "title": "Promoted",
        "body": body
    }]);
    if !tokens.is_null() {
        doc["content"]["tokens"] = tokens;
    }
    doc
}

const PLACES_A_TOKEN: &str =
    r#"[{"type":"paragraph","inlines":[{"type":"token","name":"tiers__first_name"}]}]"#;

// Verifies: CMS-004
#[test]
fn what_the_markdown_parser_produces_is_a_valid_template_body() {
    let source = "# Title\n\nSome *emphasis*, **strong**, `code` and a [link](https://example.test/a) \
and a [bad one](javascript:alert(1)).\n\n> quoted\n> text\n\n- one\n- two\n\n1. first\n2. second\n\n\
```rust\nlet a = 1;\n```\n\n![A badge](/media/badge.png \"Silver\")\n\n---\n";
    let blocks = strata_substrate::markdown::parse(source);
    assert!(blocks.len() >= 7);
    let json = serde_json::to_string(&blocks).unwrap();
    let analysis = strata_kernel::blocks::analyse(&json);
    assert!(analysis.problems.is_empty(), "{:?}", analysis.problems);
    // The bounds are the parser's.
    assert_eq!(
        strata_kernel::blocks::MAX_QUOTE_DEPTH,
        strata_substrate::markdown::MAX_QUOTE_DEPTH
    );
    assert_eq!(
        strata_kernel::blocks::MAX_INLINE_DEPTH,
        strata_substrate::markdown::MAX_INLINE_DEPTH
    );
    // A link the parser keeps is a link the schema allows, and the other way round.
    for href in [
        "https://a.test",
        "http://a.test",
        "mailto:a@b.test",
        "/path",
        "#top",
        "//host",
        "javascript:x",
        "data:text/html,x",
        "ftp://a.test",
    ] {
        let parsed = strata_substrate::markdown::parse(&format!("[t]({href})"));
        let kept = serde_json::to_string(&parsed).unwrap().contains("\"link\"");
        assert_eq!(kept, strata_kernel::blocks::allowed_href(href), "{href}");
    }
}

// ---- the holds of a product (D-8400) ----------------------------------------------------------

fn shop() -> Json {
    json!({
        "name": "Shop",
        "version": "1.0.0",
        "product": "shop",
        "platform": { "min": "0.2", "below": "0.4" },
        "content": {
            "roles": { "shop__clerk": { "id": "shop__clerk", "label": "Clerk" } },
            "holds": [{
                "id": "shop__hold_clerk",
                "label": "A clerk is approved before they work",
                "target": { "Role": { "role": "shop__clerk" } },
                "when": "param.action == 'assign'",
                "approver": "change-approve"
            }]
        }
    })
}

// Verifies: ADM-1601
// Verifies: ADM-1640
#[test]
fn a_product_declares_a_hold_rule_that_holds_the_change_it_names_and_leaves_with_the_product() {
    let mut world = world();
    let record = installed(&mut world, &shop());
    assert!(owned(&record).contains(&("hold-rule".to_string(), "shop__hold_clerk".to_string())));
    let schema = schema_of(&world);
    let rule = &schema.hold_rules.as_ref().expect("the rule is installed")
        [&HoldRuleId::new("shop__hold_clerk").unwrap()];
    assert!(
        rule.when.is_some(),
        "the condition was read as an expression"
    );

    // The rule holds an assignment of its role: the owner files it and nobody gets the role.
    let alice = PrincipalId::new("user:alice").unwrap();
    let clerk = RoleId::new("shop__clerk").unwrap();
    let (after, outcome) = world
        .step(
            &Intent::AssignRole {
                principal: alice.clone(),
                role: clerk.clone(),
            },
            &context_at(1_800_000_000_001, "assign"),
        )
        .expect("a held change is accepted as a filing");
    assert!(matches!(outcome.output, Output::Held { .. }));
    assert!(!after.tenants[&tenant()].members.contains_key(&alice));

    // A product that is removed takes its rule with it.
    let (result, calls) = with_target(&mut world, "0.2", |t, e| {
        remove_product("shop", false, t, e)
    });
    result.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(calls, 1);
    assert!(schema_of(&world).hold_rules.is_none());
}

// ---- the separations of a product (D-8482) ----------------------------------------------------

fn pay_and_approve() -> Json {
    json!({
        "name": "Pay",
        "version": "1.0.0",
        "product": "pay",
        "platform": { "min": "0.2", "below": "0.4" },
        "content": {
            "capabilities": [
                { "id": "pay__release", "name": "Release a payment", "description": "Pays out." },
                { "id": "pay__approve", "name": "Approve a payment", "description": "Approves." }
            ],
            "roles": {
                "pay__payer": { "id": "pay__payer", "label": "Payer" },
                "pay__approver": { "id": "pay__approver", "label": "Approver" }
            },
            "role-permissions": {
                "pay__payer": ["pay__release"],
                "pay__approver": ["pay__approve"]
            },
            "separations": [{
                "id": "pay__release_and_approve",
                "left": "pay__release",
                "left-duty": "releases payments",
                "right": "pay__approve",
                "right-duty": "approves payments"
            }]
        }
    })
}

// Verifies: ADM-1645
// Verifies: ADM-1646
#[test]
fn a_product_declares_a_separation_that_is_held_to_and_leaves_with_the_product() {
    let mut world = world();
    let record = installed(&mut world, &pay_and_approve());
    assert!(owned(&record).contains(&(
        "separation".to_string(),
        "pay__release_and_approve".to_string()
    )));
    let schema = schema_of(&world);
    let declared = schema
        .separations
        .as_ref()
        .expect("the separation is installed");
    assert_eq!(declared.len(), 1);

    // The one rule of the platform's own separations holds the product's: no one holds both.
    let alice = PrincipalId::new("user:alice").unwrap();
    let assign = |role: &str| Intent::AssignRole {
        principal: alice.clone(),
        role: RoleId::new(role).unwrap(),
    };
    let (world_with_payer, _) = world
        .step(
            &assign("pay__payer"),
            &context_at(1_800_000_000_001, "payer"),
        )
        .expect("the payer is assigned");
    let refusal = world_with_payer
        .step(
            &assign("pay__approver"),
            &context_at(1_800_000_000_002, "approver"),
        )
        .expect_err("the approver would be the payer");
    assert_eq!(refusal.kind, ErrorKind::ConflictingRoles);
    assert!(
        refusal.message.contains("releases payments")
            && refusal.message.contains("approves payments"),
        "{}",
        refusal.message
    );

    // A product that is removed takes its separation with it.
    let (result, calls) = with_target(&mut world, "0.2", |t, e| remove_product("pay", false, t, e));
    result.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(calls, 1);
    assert!(schema_of(&world).separations.is_none());
}
