#![allow(clippy::err_expect)] // `unwrap_err` would print a whole world when it fails.
//! The publication template is tenant metadata only. These tests install it through the kernel
//! and check that the blog's behaviour follows from that metadata.

use serde_json::json;
use std::collections::BTreeMap;
use strata_kernel::*;
use strata_substrate::bundle::{install_into, Bundle};
use strata_substrate::project::{match_route, section_order, section_request};

fn bundle() -> Bundle {
    let mut doc: serde_json::Value =
        serde_json::from_str(include_str!("../../../tenants/_templates/publication.json")).unwrap();
    let extra = json!({
        "id": "gazette",
        "name": "Gazette",
        "owner": "user:admin",
        "created_at": "2026-01-01T00:00:00Z",
        "members": [
            { "principal": "user:ed", "roles": ["editor"] },
            { "principal": "user:ann", "roles": ["author"] },
            { "principal": "user:bob", "roles": ["author"] }
        ],
        "records": [
            { "object": "publication", "as": "user:admin", "at": "2026-01-02", "fields": { "key": "main", "name": "Gazette" } },
            { "object": "author", "key": "ann", "as": "user:ed", "at": "2026-01-02", "fields": { "name": "Ann", "slug": "ann", "principal": "user:ann" } },
            { "object": "category", "key": "craft", "as": "user:ed", "at": "2026-01-02", "fields": { "name": "Craft", "slug": "craft" } },
            {
                "object": "post", "key": "first", "as": "user:ann", "at": "2026-01-03T10:00:00Z",
                "fields": { "title": "First", "slug": "first", "body": "Hello *there*.", "author": "@ann", "category": "@craft", "featured": true },
                "then": [{ "as": "user:ann", "at": "2026-01-04T08:00:00Z", "set": { "status": "published" } }]
            },
            {
                "object": "post", "key": "draft", "as": "user:ann", "at": "2026-01-05T10:00:00Z",
                "fields": { "title": "Unfinished", "slug": "unfinished", "body": "Not yet.", "author": "@ann" }
            }
        ]
    });
    doc.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    Bundle::from_json(&doc.to_string()).unwrap()
}

fn world() -> World {
    install_into(&World::default(), &bundle()).unwrap_or_else(|e| panic!("{e}"))
}

fn ctx(principal: Principal) -> Context {
    Context {
        tenant: TenantId::new("gazette").unwrap(),
        principal,
        session: None,
        now: Timestamp(1_800_000_000_000),
        correlation: "t".into(),
    }
}

fn user(name: &str) -> Principal {
    Principal::User(PrincipalId::new(format!("user:{name}")).unwrap())
}

fn slugs(world: &World, principal: Principal, path: &str, section: &str) -> Vec<String> {
    let schema = &world.tenants[&TenantId::new("gazette").unwrap()].schema;
    let (view, route) = match_route(schema, path).expect("route matches");
    let mut done = BTreeMap::new();
    let mut found = Vec::new();
    for name in section_order(view) {
        let def = &view.sections[name];
        let request = section_request(def, &route, &BTreeMap::new(), &done);
        let rows = world.read(&request, &ctx(principal.clone())).unwrap().rows;
        if def.single {
            done.insert(name.clone(), rows.first().cloned());
        }
        if name == section {
            found = rows
                .iter()
                .map(|r| {
                    r.get(&FieldId::new("slug").unwrap())
                        .as_str()
                        .unwrap_or("")
                        .to_string()
                })
                .collect();
        }
    }
    found
}

/// Verifies: CMS-001
#[test]
fn the_template_installs_and_publishing_stamps_the_time() {
    let world = world();
    let tenant = &world.tenants[&TenantId::new("gazette").unwrap()];
    assert!(tenant.schema.problems().is_empty());
    let post = tenant
        .records_of(&ObjectTypeId::new("post").unwrap())
        .next()
        .unwrap();
    assert_eq!(
        post.get(&FieldId::new("status").unwrap()),
        &Value::Str("published".into())
    );
    assert_eq!(
        post.get(&FieldId::new("published_at").unwrap()),
        &Value::Time(1_767_513_600_000)
    );
}

/// Verifies: CMS-003
#[test]
fn readers_see_published_posts_and_authors_also_see_their_own_drafts() {
    let world = world();
    assert_eq!(
        slugs(&world, Principal::Anonymous, "/", "recent"),
        ["first"]
    );
    assert_eq!(
        slugs(&world, Principal::Anonymous, "/posts/unfinished", "post"),
        Vec::<String>::new()
    );
    assert_eq!(
        slugs(&world, user("ann"), "/posts/unfinished", "post"),
        ["unfinished"]
    );
    assert_eq!(
        slugs(&world, user("bob"), "/posts/unfinished", "post"),
        Vec::<String>::new()
    );
    assert_eq!(
        slugs(&world, user("ann"), "/desk", "posts"),
        ["unfinished", "first"]
    );
    assert_eq!(
        slugs(&world, user("ed"), "/desk", "posts"),
        ["unfinished", "first"]
    );
    assert_eq!(
        slugs(&world, Principal::Anonymous, "/desk", "posts"),
        Vec::<String>::new()
    );
}

#[test]
fn a_dependent_section_reads_through_the_single_row_it_depends_on() {
    let world = world();
    assert_eq!(
        slugs(&world, Principal::Anonymous, "/topics/craft", "posts"),
        ["first"]
    );
    assert_eq!(
        slugs(&world, Principal::Anonymous, "/topics/nowhere", "posts"),
        Vec::<String>::new()
    );
    assert_eq!(
        slugs(&world, Principal::Anonymous, "/authors/ann", "posts"),
        ["first"]
    );
}

/// Verifies: CMS-005
#[test]
fn the_theme_and_cover_rules_are_constraints_in_the_metadata() {
    let world = world();
    let update = |object: &str, id: u64, field: &str, value: &str| Intent::UpdateRecord {
        object: ObjectTypeId::new(object).unwrap(),
        id: RecordId(id),
        fields: BTreeMap::from([(FieldId::new(field).unwrap(), Value::Str(value.into()))]),
        expected_version: None,
    };
    // Every token of each list is accepted.
    for token in ["moss", "clay", "ink", "ochre", "plum"] {
        assert!(
            world
                .step(&update("publication", 1, "accent", token), &ctx(user("ed")))
                .is_ok(),
            "accent token {token} is refused"
        );
    }
    for token in ["linen", "stone", "chalk"] {
        assert!(
            world
                .step(&update("publication", 1, "paper", token), &ctx(user("ed")))
                .is_ok(),
            "paper token {token} is refused"
        );
    }
    // A value outside its list is refused by that list's constraint, whatever its shape.
    for value in [
        "url(evil)",
        "Clay",
        "clay ",
        "red",
        "moss;}",
        "ink\"><script>",
    ] {
        let refused = world
            .step(&update("publication", 1, "accent", value), &ctx(user("ed")))
            .expect_err("rejected");
        assert_eq!(
            refused.kind,
            ErrorKind::ConstraintViolated,
            "accent {value:?}"
        );
        assert!(
            refused
                .evidence
                .iter()
                .any(|c| c.rule == "constraint:publication.accent_token" && !c.passed),
            "accent {value:?} is not refused by accent_token"
        );
    }
    for value in ["url(evil)", "Linen", "red"] {
        let refused = world
            .step(&update("publication", 1, "paper", value), &ctx(user("ed")))
            .expect_err("rejected");
        assert_eq!(
            refused.kind,
            ErrorKind::ConstraintViolated,
            "paper {value:?}"
        );
        assert!(
            refused
                .evidence
                .iter()
                .any(|c| c.rule == "constraint:publication.paper_token" && !c.passed),
            "paper {value:?} is not refused by paper_token"
        );
    }
    assert_eq!(
        world
            .step(
                &update("publication", 1, "accent", "clay"),
                &ctx(user("ann"))
            )
            .expect_err("rejected")
            .kind,
        ErrorKind::Unauthorized
    );
    let remote_cover = world
        .step(
            &update("post", 4, "cover", "https://example.com/x.png"),
            &ctx(user("ann")),
        )
        .expect_err("rejected");
    assert!(remote_cover
        .evidence
        .iter()
        .any(|c| c.rule == "constraint:post.cover_is_local" && !c.passed));
    let undescribed = world
        .step(
            &update("post", 4, "cover", "/media/kiln.svg"),
            &ctx(user("ann")),
        )
        .expect_err("rejected");
    assert!(undescribed
        .evidence
        .iter()
        .any(|c| c.rule == "constraint:post.cover_is_described" && !c.passed));
}

#[test]
fn an_author_cannot_touch_another_authors_post() {
    let world = world();
    let edit = Intent::UpdateRecord {
        object: ObjectTypeId::new("post").unwrap(),
        id: RecordId(4),
        fields: BTreeMap::from([(
            FieldId::new("title").unwrap(),
            Value::Str("Hijacked".into()),
        )]),
        expected_version: None,
    };
    assert_eq!(
        world
            .step(&edit, &ctx(user("bob")))
            .expect_err("rejected")
            .kind,
        ErrorKind::Unauthorized
    );
    assert!(world.step(&edit, &ctx(user("ann"))).is_ok());
    assert!(world.step(&edit, &ctx(user("ed"))).is_ok());
}
