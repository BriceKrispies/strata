#![allow(clippy::err_expect)]
//! Staged upgrades over the schema-change cap (D-DS-16, WG-5472): the planner splits an approved
//! upgrade into stages that each fit one commit, the kernel accepts each stage in turn, and no early
//! stage changes what a member can see. The bundles are generated, so the property runs over many
//! shapes of product.

use serde_json::{json, Value as Json};
use strata_kernel::*;
use strata_substrate::bundle::{
    install_into, install_product, plan_upgrade, Bundle, BundleError, BundleErrorKind,
    InstalledRecord, Planned, ProductBundle, ProductInstall, ProductInstallError, ProductTarget,
    product_version_cmp,
};
use strata_substrate::stages::{plan_stages, staging_role, StagePlan};

const RELEASE: &str = "0.2-draft";
const CAP: usize = strata_kernel::limits::MAX_SCHEMA_CHANGES;

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

fn read(doc: &Json) -> ProductBundle {
    ProductBundle::from_json(&doc.to_string()).unwrap_or_else(|e| panic!("{e}"))
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

/// Installs `doc` (a version of product `bulk`) into `world`, and returns the record it leaves.
fn install(world: &mut World, doc: &Json) -> InstalledRecord {
    let bundle = read(doc);
    let tenant = tenant();
    let schema = world.tenants[&tenant].schema.clone();
    let target = ProductTarget {
        tenant: &tenant,
        principal: admin(),
        now: 1_800_000_000_000,
        release: RELEASE,
        schema: &schema,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    };
    let result = install_product(&bundle, &target, &mut |ctx: Context, intent: Intent| {
        let (next, outcome) = world.step(&intent, &ctx)?;
        *world = next;
        Ok::<Output, DomainError>(outcome.output)
    });
    match result.unwrap_or_else(|e| panic!("{e:?}")) {
        ProductInstall::Applied {
            record: Some(record),
            ..
        } => record,
        other => panic!("the install was not applied: {other:?}"),
    }
}

/// Plans the upgrade of the installed `record` to `doc`, against the tenant of `world`.
fn plan(
    world: &World,
    doc: &Json,
    record: &InstalledRecord,
) -> Result<Planned, BundleError> {
    let bundle = read(doc);
    let tenant = tenant();
    let schema = world.tenants[&tenant].schema.clone();
    let target = ProductTarget {
        tenant: &tenant,
        principal: admin(),
        now: 1_800_000_000_000,
        release: RELEASE,
        schema: &schema,
        installed: Some(record),
        expected_version: None,
        idempotency_key: None,
    };
    plan_upgrade(&bundle, &target)
}

fn change_of(planned: Result<Planned, BundleError>) -> strata_substrate::bundle::ProductChange {
    match planned.unwrap_or_else(|e| panic!("{e}")) {
        Planned::Change(change) => *change,
        other => panic!("the upgrade is not a change: {other:?}"),
    }
}

/// Runs the changes of one stage as one schema intent, the way a staged job commits it.
fn commit(world: World, changes: &[SchemaChange], correlation: &str) -> Result<World, DomainError> {
    let intent = Intent::ChangeSchema {
        changes: changes.to_vec(),
        expected_version: None,
        existing: ExistingData::Reject,
    };
    let (next, _) = world.step(&intent, &context_at(1_800_000_000_000, correlation))?;
    Ok(next)
}

fn field(id: &str, ty: &str) -> Json {
    json!({ "id": id, "label": id, "ty": ty, "required": false, "unique": false })
}

/// A product `bulk` at `version`: `objects` object types (`bulk__o0` has two fields and is the same
/// in every version), `queries` queries over them, `ops` operations invoked by the editor role, and
/// `policies` policies over `bulk__o0`. A policy with `any` names every object type.
struct Shape {
    version: String,
    objects: usize,
    fields: usize,
    queries: usize,
    ops: usize,
    policies: usize,
    any_object: bool,
    queries_on_first: bool,
}

fn shape(version: &str) -> Shape {
    Shape {
        version: version.to_string(),
        objects: 1,
        fields: 2,
        queries: 0,
        ops: 0,
        policies: 1,
        any_object: false,
        queries_on_first: false,
    }
}

fn product_doc(s: &Shape) -> Json {
    let mut objects = serde_json::Map::new();
    for i in 0..s.objects {
        let id = format!("bulk__o{i}");
        // The first object is the same in every version, so an upgrade does not change it unless the
        // shape asks for more fields on it.
        let fields = if i == 0 { 2 } else { s.fields };
        let mut map = serde_json::Map::new();
        map.insert("f0".into(), field("f0", "Int"));
        for f in 1..fields.max(1) {
            map.insert(format!("f{f}"), field(&format!("f{f}"), "Str"));
        }
        objects.insert(
            id.clone(),
            json!({ "id": id, "label": id, "fields": map, "constraints": {} }),
        );
    }
    let mut policies = serde_json::Map::new();
    for p in 0..s.policies {
        let id = format!("bulk__p{p}");
        let resource = if p == 0 && s.any_object {
            json!("*")
        } else {
            json!({ "Object": "bulk__o0" })
        };
        policies.insert(
            id.clone(),
            json!({
                "id": id, "role": "bulk__editor", "resource": resource,
                "actions": ["Read"], "condition": "record.f0 >= 0"
            }),
        );
    }
    let mut queries = serde_json::Map::new();
    for q in 0..s.queries {
        let id = format!("bulk__q{q}");
        let object = if s.queries_on_first {
            "bulk__o0".to_string()
        } else {
            format!("bulk__o{}", q % s.objects)
        };
        queries.insert(
            id.clone(),
            json!({
                "id": id, "object": object, "filter": "record.f0 >= 0",
                "params": [], "order": [{ "field": "f0", "descending": true }], "limit": 10
            }),
        );
    }
    let operations: Vec<Json> = (0..s.ops)
        .map(|k| {
            json!({
                "id": format!("bulk__op{k}"), "label": "Op", "inputs": [],
                "invokers": { "signed-in": false, "roles": ["bulk__editor"] },
                "conditions": [],
                "steps": [{ "CreateRecord": {
                    "id": "rec", "object": format!("bulk__o{}", k % s.objects),
                    "fields": { "f0": "0" }
                } }]
            })
        })
        .collect();
    json!({
        "name": "Bulk",
        "version": s.version,
        "product": "bulk",
        "platform": { "min": "0.2" },
        "content": {
            "objects": objects,
            "roles": { "bulk__editor": { "id": "bulk__editor", "label": "Editor" } },
            "role-permissions": { "bulk__editor": ["content-read"] },
            "policies": policies,
            "queries": queries,
            "operations": operations
        }
    })
}

/// The first version of `bulk`, installed into a fresh tenant.
fn first_version() -> (World, InstalledRecord) {
    let mut world = world();
    let record = install(&mut world, &product_doc(&shape("1.0.0")));
    (world, record)
}

/// Roles, operations and policies of an early stage: the only things a stage may hold that are not
/// created entities (the staging role, and operations invoked by it).
fn early_violation(change: &SchemaChange, created_objects: &[String]) -> Option<String> {
    let new_object = |id: &ObjectTypeId| created_objects.contains(&id.to_string());
    match change {
        SchemaChange::RecordProduct(_) => None,
        SchemaChange::DefineRole(role) if role.id.0 == staging_role("bulk") => None,
        SchemaChange::DefineOperation(op)
            if !op.invokers.signed_in
                && op.invokers.roles.len() == 1
                && op.invokers.roles.contains(&RoleId(staging_role("bulk"))) =>
        {
            None
        }
        SchemaChange::DefineObject { id, .. } if new_object(id) => None,
        SchemaChange::DefineField { object, .. }
        | SchemaChange::DefineConstraint { object, .. }
        | SchemaChange::DefineUnique { object, .. }
        | SchemaChange::DefineRelationship { object, .. }
        | SchemaChange::SetRetention { object, .. }
        | SchemaChange::SetPublication { object, .. }
        | SchemaChange::SetLanguageField { object, .. }
        | SchemaChange::SetTranslationGroup { object, .. }
            if new_object(object) =>
        {
            None
        }
        // Every query of this product is new (the first version has none), so a query is early.
        SchemaChange::DefineQuery(_) => None,
        other => Some(format!("{other:?}")),
    }
}

fn created_objects_of(doc: &Json, installed: &[String]) -> Vec<String> {
    doc["content"]["objects"]
        .as_object()
        .map(|map| {
            map.keys()
                .filter(|id| !installed.contains(id))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn owned_ids(record: &InstalledRecord) -> Vec<(String, String)> {
    record
        .owns
        .iter()
        .map(|e| (e.kind.clone(), e.id.clone()))
        .collect()
}

fn record_of(change: &SchemaChange) -> Option<&InstalledProduct> {
    match change {
        SchemaChange::RecordProduct(record) => Some(record),
        _ => None,
    }
}

/// Plans `doc` as an upgrade of `record`, over the cap, and returns the plan.
fn staged(world: &World, doc: &Json, record: &InstalledRecord) -> Result<Option<StagePlan>, BundleError> {
    let change = change_of(plan(world, doc, record));
    plan_stages(&change, record, &world.tenants[&tenant()].schema)
}

#[test]
fn a_small_upgrade_is_one_commit_and_is_not_staged() {
    let (world, record) = first_version();
    let mut s = shape("2.0.0");
    s.objects = 2;
    s.queries = 3;
    let change = change_of(plan(&world, &product_doc(&s), &record));
    assert!(change.changes.len() + 1 <= CAP);
    let planned = plan_stages(&change, &record, &world.tenants[&tenant()].schema);
    assert_eq!(planned.unwrap(), None, "an upgrade under the cap is not staged");
}

#[test]
fn a_product_at_the_entity_limits_installs_in_stages() {
    let (world, record) = first_version();
    let mut s = shape("2.0.0");
    s.objects = 150;
    s.fields = 4;
    s.queries = 150;
    s.ops = 150;
    let doc = product_doc(&s);
    let plan = staged(&world, &doc, &record)
        .unwrap_or_else(|e| panic!("{e}"))
        .expect("the upgrade is over the cap");

    assert!(plan.stages.len() >= 3, "three or more stages, got {}", plan.stages.len());
    for stage in &plan.stages {
        assert!(stage.changes.len() <= CAP, "{} holds {}", stage.version, stage.changes.len());
    }
    let last = plan.stages.last().unwrap();
    assert_eq!(last.version, "2.0.0", "the last stage records the target");
    assert!(
        last.changes.iter().any(|c| matches!(c, SchemaChange::RemoveRole(r) if r.0 == staging_role("bulk"))),
        "the last stage removes the staging role"
    );

    // The versions of the stages rise from the installed version to the target.
    let mut previous = record.version.clone();
    for stage in &plan.stages {
        assert_eq!(product_version_cmp(&previous, &stage.version), std::cmp::Ordering::Less);
        previous = stage.version.clone();
    }
    assert_eq!(plan.stages[0].version, "2.0.0-stage.1");

    // Each stage records its own version and owns what it has created so far; the staging role is
    // owned by the early stages and not by the target.
    let early = &plan.stages[0];
    let record_of_early = record_of(early.changes.last().unwrap()).expect("a record ends the stage");
    assert_eq!(record_of_early.version, "2.0.0-stage.1");
    assert!(owned_kernel(record_of_early)
        .contains(&("role".to_string(), staging_role("bulk"))));
    let target_record = record_of(last.changes.last().unwrap()).expect("a record ends the stage");
    assert_eq!(target_record.version, "2.0.0");
    assert!(!owned_kernel(target_record)
        .contains(&("role".to_string(), staging_role("bulk"))));
    assert!(owned_kernel(target_record)
        .contains(&("object".to_string(), "bulk__o149".to_string())));
}

/// The entities a record the kernel holds says the product owns.
fn owned_kernel(record: &InstalledProduct) -> Vec<(String, String)> {
    record
        .owns
        .iter()
        .map(|e| (e.kind.as_str().to_string(), e.id.clone()))
        .collect()
}

#[test]
fn a_three_stage_upgrade_passes_the_kernel_one_stage_at_a_time() {
    let (mut world, record) = first_version();
    let mut s = shape("2.0.0");
    s.objects = 150;
    s.fields = 4;
    s.queries = 150;
    s.ops = 150;
    let doc = product_doc(&s);
    let plan = staged(&world, &doc, &record)
        .unwrap_or_else(|e| panic!("{e}"))
        .expect("the upgrade is over the cap");
    assert!(plan.stages.len() >= 3);
    for (n, stage) in plan.stages.iter().enumerate() {
        world = commit(world, &stage.changes, &format!("bulk:{}", stage.version))
            .unwrap_or_else(|e| panic!("stage {n} ({}) was refused: {e:?}", stage.version));
    }
    // The product is installed at the target, and the staging role is gone.
    let installed = InstalledRecord::of(&world.tenants[&tenant()].schema, "bulk")
        .expect("the product is installed");
    assert_eq!(installed.version, "2.0.0");
    assert!(!owned_ids(&installed).contains(&("role".to_string(), staging_role("bulk"))));
    assert!(world.tenants[&tenant()]
        .schema
        .roles
        .get(&RoleId(staging_role("bulk")))
        .is_none());
}

#[test]
fn an_early_operation_is_refused_by_the_kernel_without_the_staging_role() {
    let (world, _) = first_version();
    let op = TenantOperationDef {
        id: TenantOperationId::new("bulk__early_op").unwrap(),
        label: "Early".into(),
        inputs: vec![],
        invokers: TenantOperationInvokers {
            signed_in: false,
            roles: std::collections::BTreeSet::from([RoleId(staging_role("bulk"))]),
        },
        subject: None,
        conditions: vec![],
        steps: vec![],
    };
    let without_role = commit(world.clone(), &[SchemaChange::DefineOperation(op.clone())], "early");
    assert!(without_role.is_err(), "an operation whose only invoker is an undefined role is refused");

    let role = SchemaChange::DefineRole(RoleDef {
        id: RoleId(staging_role("bulk")),
        label: "Staging".into(),
    });
    let with_role = commit(world, &[role, SchemaChange::DefineOperation(op)], "early");
    assert!(with_role.is_ok(), "the operation is accepted once the staging role is defined");
}

#[test]
fn assign_role_refuses_the_staging_role_of_a_product() {
    let (world, _) = first_version();
    let staging = RoleId(staging_role("bulk"));
    let role = SchemaChange::DefineRole(RoleDef {
        id: staging.clone(),
        label: "Staging".into(),
    });
    let world = commit(world, &[role], "staging").unwrap_or_else(|e| panic!("{e:?}"));
    let intent = Intent::AssignRole {
        principal: PrincipalId::new("user:admin").unwrap(),
        role: staging,
    };
    let refused = world
        .step(&intent, &context_at(1_800_000_000_000, "assign"))
        .err()
        .expect("the staging role is not assigned");
    assert!(
        refused.message.contains("reserved for a product's staged upgrade"),
        "{}",
        refused.message
    );
}

#[test]
fn a_last_stage_over_the_cap_is_refused_with_its_counts() {
    let (world, record) = first_version();
    let mut s = shape("2.0.0");
    s.policies = 1 + CAP; // every policy is a last-stage change
    let err = staged(&world, &product_doc(&s), &record).expect_err("the last stage is over the cap");
    assert_eq!(err.kind, BundleErrorKind::FinalStageTooLarge);
    assert!(err.message.contains("upgrade.final-stage-too-large"));
    assert!(err.message.contains(&format!("DefinePolicy {}", 1 + CAP)), "{}", err.message);
}

#[test]
fn an_any_object_policy_sends_new_types_to_the_last_stage_and_refuses_an_overflow() {
    let (world, record) = first_version();
    // Four new object types with sixty fields each: all of it is last, and that is over the cap.
    let mut s = shape("2.0.0");
    s.objects = 5;
    s.fields = 61;
    s.any_object = true;
    let err = staged(&world, &product_doc(&s), &record).expect_err("the overflow is refused");
    assert_eq!(err.kind, BundleErrorKind::FinalStageTooLarge);
}

#[test]
fn an_any_object_policy_keeps_queries_on_an_existing_type_early() {
    let (world, record) = first_version();
    let mut s = shape("2.0.0");
    s.queries = CAP + 40;
    s.queries_on_first = true;
    s.any_object = true;
    let plan = staged(&world, &product_doc(&s), &record)
        .unwrap_or_else(|e| panic!("{e}"))
        .expect("the queries are over the cap");
    for stage in &plan.stages[..plan.stages.len() - 1] {
        for change in &stage.changes {
            assert!(
                !matches!(change, SchemaChange::DefineObject { .. } | SchemaChange::DefinePolicy(_)),
                "an early stage holds {change:?}"
            );
        }
    }
    assert!(plan.stages.len() >= 2);
}

#[test]
fn no_early_stage_changes_what_a_member_can_see_over_generated_products() {
    // A deterministic generator over the shapes of a product: new types, fields on existing types,
    // queries and operations, with or without an any-object policy, and policies that may overflow.
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = |n: usize| -> usize {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 33) as usize) % n
    };
    let (world, record) = first_version();
    let installed_objects = vec!["bulk__o0".to_string()];
    let mut staged_cases = 0;
    for _ in 0..40 {
        let mut s = shape("2.0.0");
        s.objects = 1 + next(4);
        s.fields = 1 + next(60);
        s.queries = next(230);
        s.queries_on_first = next(2) == 0;
        s.ops = next(40);
        s.policies = 1 + next(3);
        s.any_object = next(3) == 0;
        let doc = product_doc(&s);
        let created = created_objects_of(&doc, &installed_objects);
        match staged(&world, &doc, &record) {
            Ok(None) => {}
            Err(err) => assert_eq!(err.kind, BundleErrorKind::FinalStageTooLarge, "{}", err.message),
            Ok(Some(plan)) => {
                staged_cases += 1;
                for stage in &plan.stages[..plan.stages.len() - 1] {
                    assert!(stage.changes.len() <= CAP);
                    for change in &stage.changes {
                        if let Some(why) = early_violation(change, &created) {
                            panic!("early stage {} holds {why}", stage.version);
                        }
                        if s.any_object {
                            assert!(
                                !matches!(change, SchemaChange::DefineObject { .. }),
                                "a new type reached by an any-object policy is early"
                            );
                        }
                    }
                }
                let last = plan.stages.last().unwrap();
                assert!(last.changes.len() <= CAP);
            }
        }
    }
    assert!(staged_cases > 0, "the generator reaches the staged path");
}
