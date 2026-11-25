//! A product that declares a descendants query (WG-5510) installed into a tenant. Its threads are read level by
//! level through the kernel, and a view section that reads them delivers each reply to the client with its depth,
//! and the screen part of the section names the depth as a field of the row.
//!
//! The bundle is the output of `specc bundle` for
//! `crates/spec-compiler/tests/fixtures/products/bundle/thread.jsonc` (its golden file is
//! `crates/spec-compiler/tests/fixtures/products/expected/thread.bundle.json`). The refusals take that bundle and
//! change its tree: a parent field that is not a self-reference, and a max-depth above 32.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value as Json};
use strata_kernel::testkit::{ctx, field, object, user};
use strata_kernel::*;
use strata_substrate::bundle::{
    install_into, install_product, Bundle, ProductBundle, ProductTarget,
};
use strata_substrate::project::{project, section_request_with, SectionData, ViewData};
use strata_substrate::ui::definition;

const NOW: i64 = 1_800_000_000_000;

fn tenant() -> TenantId {
    TenantId::new("acme").unwrap()
}

fn admin() -> Principal {
    Principal::User(PrincipalId::new("user:admin").unwrap())
}

/// An empty tenant, owned by the admin.
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

fn thread_bundle_text() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../spec-compiler/tests/fixtures/products/expected/thread.bundle.json");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Installs a product bundle into the tenant as the admin, through the kernel.
fn install(world: &mut World, bundle: &ProductBundle) -> Result<(), String> {
    let tenant = tenant();
    let schema = (*world.tenants[&tenant].schema).clone();
    let target = ProductTarget {
        tenant: &tenant,
        principal: admin(),
        now: NOW,
        release: "0.2",
        schema: &schema,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    };
    let mut execute = |ctx: Context, intent: Intent| -> Result<Output, DomainError> {
        let (next, outcome) = world.step(&intent, &ctx)?;
        *world = next;
        Ok(outcome.output)
    };
    install_product(bundle, &target, &mut execute)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

/// Creates a comment as the admin: its slug, and its parent when it is a reply.
fn comment(world: &mut World, slug: &str, parent: Option<RecordId>) -> RecordId {
    let mut fields = BTreeMap::from([
        (field("slug"), Value::Str(slug.to_string())),
        (field("body"), Value::Str(format!("about {slug}"))),
    ]);
    if let Some(parent) = parent {
        fields.insert(field("parent"), Value::Ref(parent));
    }
    let intent = Intent::CreateRecord {
        object: object("thread__comment"),
        fields,
    };
    let (next, outcome) = world
        .step(&intent, &ctx("acme", admin(), NOW))
        .expect("the comment is accepted");
    *world = next;
    match outcome.output {
        Output::Record { id, .. } => id,
        _ => panic!("a record was expected"),
    }
}

/// The product's thread: `welcome` at the top, a reply `first` to it, `second` to `first`, `third` to
/// `second` (depth 3 below the root), and `aside`, another reply to the root.
fn a_thread(world: &mut World) -> RecordId {
    let root = comment(world, "welcome", None);
    let first = comment(world, "first", Some(root));
    let second = comment(world, "second", Some(first));
    comment(world, "third", Some(second));
    comment(world, "aside", Some(root));
    root
}

#[test]
fn the_thread_product_installs_and_its_descendants_read_with_depths() {
    let mut world = world();
    let bundle = ProductBundle::from_json(&thread_bundle_text()).unwrap();
    install(&mut world, &bundle).unwrap_or_else(|e| panic!("{e}"));
    let root = a_thread(&mut world);

    let state = world.state(&tenant());
    let request = ReadRequest::Named {
        query: QueryId::new("thread__replies").unwrap(),
        params: BTreeMap::from([("root".to_string(), Value::Ref(root))]),
        page_size: None,
        cursor: None,
    };
    let result = read(&state, &request, &ctx("acme", admin(), NOW)).expect("the thread reads");
    let slugs: Vec<&Value> = result
        .rows
        .iter()
        .map(|row| row.get(&field("slug")))
        .collect();
    assert_eq!(
        slugs,
        vec![
            &Value::Str("first".into()),
            &Value::Str("aside".into()),
            &Value::Str("second".into()),
            &Value::Str("third".into()),
        ],
        "level order: the replies to the root, then the level below them"
    );
    assert_eq!(result.depths, Some(vec![1, 1, 2, 3]));
}

#[test]
fn a_view_section_delivers_each_reply_with_its_depth_to_the_client_and_the_screen() {
    let mut world = world();
    let bundle = ProductBundle::from_json(&thread_bundle_text()).unwrap();
    install(&mut world, &bundle).unwrap_or_else(|e| panic!("{e}"));
    a_thread(&mut world);
    let schema = (*world.tenants[&tenant()].schema).clone();
    let view = &schema.views[&ViewId::new("thread__view").unwrap()];
    let route = BTreeMap::from([("slug".to_string(), "welcome".to_string())]);
    let state = world.state(&tenant());
    let context = ctx("acme", admin(), NOW);

    // The thread section is the single comment of the route; the replies take their root from its $id.
    let first = section_request_with(
        &view.sections["thread"],
        &route,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    let thread = read(&state, &first, &context).expect("the thread reads");
    let done = BTreeMap::from([("thread".to_string(), thread.rows.first().cloned())]);
    let request = section_request_with(
        &view.sections["replies"],
        &route,
        &BTreeMap::new(),
        &done,
        &BTreeMap::new(),
    );
    let replies = read(&state, &request, &context).expect("the replies read");
    assert_eq!(replies.depths, Some(vec![1, 1, 2, 3]));

    let section = |rows: Vec<Arc<Record>>, single: bool, depths: Option<Vec<u32>>| SectionData {
        single,
        rows,
        included: BTreeMap::new(),
        markdown: Vec::new(),
        ref_targets: BTreeMap::new(),
        next: None,
        aggregate: None,
        depths,
    };
    let data = ViewData {
        view: view.id.clone(),
        title: view.title.clone(),
        params: route.clone(),
        sections: BTreeMap::from([
            (
                "thread".to_string(),
                section(thread.rows.clone(), true, None),
            ),
            (
                "replies".to_string(),
                section(replies.rows.clone(), false, replies.depths.clone()),
            ),
        ]),
        depends_on: BTreeSet::new(),
        settings: BTreeSet::new(),
        revision: replies.revision,
        schema_version: replies.schema_version,
        record_rules: BTreeMap::new(),
        tenant_theme: None,
        person_fields: BTreeMap::new(),
        affordances: BTreeMap::new(),
        individual: false,
        people: BTreeMap::new(),
    };

    // The reply: each reply row carries its depth beside its id and fields.
    let reply = project(&data);
    let rows = reply["data"]["replies"]
        .as_array()
        .expect("the replies are a list");
    let depths: Vec<u64> = rows
        .iter()
        .map(|row| row["depth"].as_u64().unwrap())
        .collect();
    assert_eq!(depths, vec![1, 1, 2, 3]);
    let slugs: Vec<&str> = rows
        .iter()
        .map(|row| row["fields"]["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, vec!["first", "aside", "second", "third"]);
    assert_eq!(reply["data"]["thread"]["fields"]["slug"], "welcome");

    // The screen part: the replies section of the screen names the depth as a field its rows carry.
    let defined = definition(&schema);
    let names_depth = defined
        .parts
        .values()
        .any(|part| part["sections"]["replies"]["shape"]["depth"] == json!("int"));
    assert!(names_depth, "a screen part names the depth of the replies");
}

#[test]
fn a_tree_whose_parent_is_not_a_self_reference_is_refused_at_install() {
    let mut text: Json = serde_json::from_str(&thread_bundle_text()).unwrap();
    // The parent field becomes the body, a string: not a reference to the comment type.
    text["content"]["queries"]["thread__replies"]["tree"]["via"] = json!("body");
    let bundle = ProductBundle::from_json(&text.to_string()).unwrap();
    let error = install(&mut world(), &bundle).expect_err("the install is refused");
    assert!(error.contains("tree"), "{error}");
    assert!(error.contains("body"), "{error}");
}

#[test]
fn a_tree_deeper_than_32_levels_is_refused_at_install() {
    let mut text: Json = serde_json::from_str(&thread_bundle_text()).unwrap();
    text["content"]["queries"]["thread__replies"]["tree"]["max-depth"] = json!(33);
    let bundle = ProductBundle::from_json(&text.to_string()).unwrap();
    let error = install(&mut world(), &bundle).expect_err("the install is refused");
    assert!(error.contains("tree"), "{error}");
    assert!(error.contains("33") || error.contains("32"), "{error}");
}
