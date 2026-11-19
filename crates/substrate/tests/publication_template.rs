#![allow(clippy::err_expect)] // `unwrap_err` would print a whole world when it fails.
//! The publication template is tenant metadata only. These tests install it through the kernel
//! and check that the blog's behaviour follows from that metadata.

use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::*;
use strata_substrate::bundle::{install_into, Bundle};
use strata_substrate::project::{match_route, section_order, section_request};
use strata_substrate::theme;

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
                "fields": { "title": "First", "slug": "first", "body": "Hello *there*.", "author": "@ann", "category": "@craft" },
                "then": [
                    { "as": "user:ann", "at": "2026-01-04T08:00:00Z", "set": { "status": "published" } },
                    { "as": "user:ed", "at": "2026-01-04T09:00:00Z", "set": { "featured": true } }
                ]
            },
            {
                "object": "post", "key": "draft", "as": "user:ann", "at": "2026-01-05T10:00:00Z",
                "fields": { "title": "Unfinished", "slug": "unfinished", "body": "Not yet.", "author": "@ann" }
            },
            { "object": "author", "key": "bob", "as": "user:ed", "at": "2026-01-06", "fields": { "name": "Bob", "slug": "bob", "principal": "user:bob" } }
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
///
/// The tenant that results from installing the template holds, in its own metadata, every role,
/// object type with its fields and constraints, workflow, policy, query and view the template
/// defines, and it got there by kernel intents: the journal of the install is nothing but those.
#[test]
fn the_template_installs_and_publishing_stamps_the_time() {
    let world = world();
    let tenant = &world.tenants[&TenantId::new("gazette").unwrap()];
    assert!(tenant.schema.problems().is_empty());
    let bundle = bundle();
    let schema = &tenant.schema;
    let ids = |names: Vec<String>| -> BTreeSet<String> { names.into_iter().collect() };
    assert!(!bundle.objects.is_empty() && !bundle.views.is_empty());
    for object in &bundle.objects {
        let def = schema
            .objects
            .get(&object.id)
            .unwrap_or_else(|| panic!("object {} was not installed", object.id));
        assert_eq!(
            ids(def.fields.keys().map(|f| f.to_string()).collect()),
            ids(object.fields.iter().map(|f| f.id.to_string()).collect()),
            "fields of {}",
            object.id
        );
        assert_eq!(
            ids(def.constraints.keys().map(|c| c.to_string()).collect()),
            ids(object
                .constraints
                .iter()
                .map(|c| c.id.to_string())
                .collect()),
            "constraints of {}",
            object.id
        );
    }
    assert_eq!(
        ids(schema.objects.keys().map(|o| o.to_string()).collect()),
        ids(bundle.objects.iter().map(|o| o.id.to_string()).collect())
    );
    assert_eq!(
        ids(schema.workflows.keys().map(|w| w.to_string()).collect()),
        ids(bundle.workflows.iter().map(|w| w.id.to_string()).collect())
    );
    assert_eq!(
        ids(schema.policies.keys().map(|p| p.to_string()).collect()),
        ids(bundle.policies.iter().map(|p| p.id.to_string()).collect())
    );
    assert_eq!(
        ids(schema.queries.keys().map(|q| q.to_string()).collect()),
        ids(bundle.queries.iter().map(|q| q.id.to_string()).collect())
    );
    assert_eq!(
        ids(schema.views.keys().map(|v| v.to_string()).collect()),
        ids(bundle.views.iter().map(|v| v.id.to_string()).collect())
    );
    for role in &bundle.roles {
        assert!(schema.roles.contains_key(&role.id), "role {}", role.id);
    }
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

    // Not only through the views: by every named query the template defines, with every value it
    // could be asked about, and by a direct read of the draft, a principal whose policies permit
    // published content alone never gets a draft back.
    let tenant = &world.tenants[&TenantId::new("gazette").unwrap()];
    let post = ObjectTypeId::new("post").unwrap();
    let status = FieldId::new("status").unwrap();
    let published = Value::Str("published".into());
    let drafts: Vec<RecordId> = tenant
        .records_of(&post)
        .filter(|r| r.get(&status) != &published)
        .map(|r| r.id)
        .collect();
    assert!(!drafts.is_empty(), "the world holds a draft");
    let mut values: Vec<Value> = vec![Value::Str("Unfinished".into())];
    for record in tenant.records.values().flat_map(|records| records.values()) {
        values.push(Value::Ref(record.id));
        values.extend(
            record
                .fields
                .values()
                .filter(|v| v.as_str().is_some())
                .cloned(),
        );
    }
    let mut asked = 0;
    for principal in [Principal::Anonymous, user("bob"), user("nobody-at-all")] {
        let direct = ReadRequest::Records {
            object: post.clone(),
            ids: drafts.clone(),
        };
        assert!(
            world
                .read(&direct, &ctx(principal.clone()))
                .unwrap()
                .rows
                .is_empty(),
            "{principal:?} read a draft directly"
        );
        for query in tenant.schema.queries.values() {
            let candidates: Vec<BTreeMap<String, Value>> = match query.params.len() {
                0 => vec![BTreeMap::new()],
                1 => {
                    let name = query.params.iter().next().unwrap().clone();
                    values
                        .iter()
                        .map(|v| BTreeMap::from([(name.clone(), v.clone())]))
                        .collect()
                }
                _ => continue,
            };
            for params in candidates {
                let request = ReadRequest::Named {
                    query: query.id.clone(),
                    params,
                };
                let Ok(result) = world.read(&request, &ctx(principal.clone())) else {
                    continue;
                };
                asked += 1;
                for row in &result.rows {
                    assert!(
                        row.object != post || row.get(&status) == &published,
                        "{principal:?} got a draft from query {}: {row:?}",
                        query.id
                    );
                }
            }
        }
    }
    assert!(asked > 100, "only {asked} reads were made");
    // The same reads are not empty for those whose policies do permit drafts.
    for principal in [user("ann"), user("ed")] {
        let direct = ReadRequest::Records {
            object: post.clone(),
            ids: drafts.clone(),
        };
        assert!(
            !world
                .read(&direct, &ctx(principal.clone()))
                .unwrap()
                .rows
                .is_empty(),
            "{principal:?} cannot read the draft"
        );
    }
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
    // Every token of the token schema (`theme::ACCENTS`, `theme::PAPERS`) is accepted, and the
    // template's constraints say the same thing as the schema: no more and no fewer.
    for token in theme::ACCENTS {
        assert!(
            world
                .step(&update("publication", 1, "accent", token), &ctx(user("ed")))
                .is_ok(),
            "accent token {token} is refused"
        );
    }
    for token in theme::PAPERS {
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
    // Values next to the tokens: another case, trailing space, a prefix, an addition, nothing.
    for (field, tokens) in [
        ("accent", &theme::ACCENTS[..]),
        ("paper", &theme::PAPERS[..]),
    ] {
        for token in tokens.iter().copied() {
            for near in [
                token.to_uppercase(),
                format!("{token} "),
                token[..token.len() - 1].to_string(),
                format!("{token};"),
                format!("{token}x"),
                String::new(),
            ] {
                let refused = world
                    .step(&update("publication", 1, field, &near), &ctx(user("ed")))
                    .expect_err("rejected");
                assert!(
                    matches!(
                        refused.kind,
                        ErrorKind::ConstraintViolated | ErrorKind::MissingRequired
                    ),
                    "{field} {near:?} was refused as {:?}",
                    refused.kind
                );
            }
        }
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

const ANN_AUTHOR: u64 = 2;
const BOB_AUTHOR: u64 = 6;
const FEATURED_PUBLISHED_POST: u64 = 4;
const ANN_DRAFT: u64 = 5;
const FAR_FUTURE: i64 = 4_102_444_800_000;

fn new_post(slug: &str, author: u64, extra: &[(&str, Value)]) -> Intent {
    let mut fields = BTreeMap::from([
        (FieldId::new("title").unwrap(), Value::Str(slug.into())),
        (FieldId::new("slug").unwrap(), Value::Str(slug.into())),
        (FieldId::new("body").unwrap(), Value::Str("Text.".into())),
        (
            FieldId::new("author").unwrap(),
            Value::Ref(RecordId(author)),
        ),
    ]);
    for (field, value) in extra {
        fields.insert(FieldId::new(*field).unwrap(), value.clone());
    }
    Intent::CreateRecord {
        object: ObjectTypeId::new("post").unwrap(),
        fields,
    }
}

fn change_post(id: u64, fields: &[(&str, Value)]) -> Intent {
    Intent::UpdateRecord {
        object: ObjectTypeId::new("post").unwrap(),
        id: RecordId(id),
        fields: fields
            .iter()
            .map(|(field, value)| (FieldId::new(*field).unwrap(), value.clone()))
            .collect(),
        expected_version: None,
    }
}

/// Refused with `kind`, and the failed check named `rule` is in the evidence.
fn assert_refused(world: &World, intent: &Intent, who: &str, kind: ErrorKind, rule: &str) {
    let refused = world
        .step(intent, &ctx(user(who)))
        .err()
        .unwrap_or_else(|| panic!("{who} was allowed {intent:?}"));
    assert_eq!(refused.kind, kind, "{who}: {intent:?}");
    assert!(
        refused.evidence.iter().any(|c| c.rule == rule && !c.passed),
        "{who} was not refused by {rule}: {:?}",
        refused.evidence
    );
}

#[test]
fn an_author_cannot_publish_under_another_authors_byline() {
    let world = world();
    // Under her own byline she may write; under Bob's she may not, and the policy says so.
    assert!(world
        .step(&new_post("mine", ANN_AUTHOR, &[]), &ctx(user("ann")))
        .is_ok());
    assert_refused(
        &world,
        &new_post("forged", BOB_AUTHOR, &[]),
        "ann",
        ErrorKind::Unauthorized,
        "policy:authors_write_posts",
    );
    // An editor may file a post under any byline.
    assert!(world
        .step(&new_post("for-bob", BOB_AUTHOR, &[]), &ctx(user("ed")))
        .is_ok());
    // Changing the byline of a post she owns is refused too.
    let rebyline = change_post(ANN_DRAFT, &[("author", Value::Ref(RecordId(BOB_AUTHOR)))]);
    assert_refused(
        &world,
        &rebyline,
        "ann",
        ErrorKind::ConstraintViolated,
        "constraint:post.author_fixed",
    );
    assert!(world.step(&rebyline, &ctx(user("ed"))).is_ok());
}

#[test]
fn only_an_editor_features_a_post_or_sets_when_it_was_published() {
    let world = world();
    let far = Value::Time(FAR_FUTURE);
    // On creation.
    assert_refused(
        &world,
        &new_post("loud", ANN_AUTHOR, &[("featured", Value::Bool(true))]),
        "ann",
        ErrorKind::ConstraintViolated,
        "constraint:post.featured_by_editors",
    );
    assert_refused(
        &world,
        &new_post("early", ANN_AUTHOR, &[("published_at", far.clone())]),
        "ann",
        ErrorKind::ConstraintViolated,
        "constraint:post.published_at_by_editors",
    );
    // An author may leave the box unticked, which the form sends as false.
    assert!(world
        .step(
            &new_post("quiet", ANN_AUTHOR, &[("featured", Value::Bool(false))]),
            &ctx(user("ann"))
        )
        .is_ok());
    // On update of her own draft.
    assert_refused(
        &world,
        &change_post(ANN_DRAFT, &[("featured", Value::Bool(true))]),
        "ann",
        ErrorKind::ConstraintViolated,
        "constraint:post.featured_by_editors",
    );
    assert_refused(
        &world,
        &change_post(ANN_DRAFT, &[("published_at", far.clone())]),
        "ann",
        ErrorKind::ConstraintViolated,
        "constraint:post.published_at_by_editors",
    );
    // A post an editor featured stays featured when its author edits it, and she cannot
    // take the feature away.
    assert!(world
        .step(
            &change_post(
                FEATURED_PUBLISHED_POST,
                &[
                    ("title", Value::Str("Retitled".into())),
                    ("featured", Value::Bool(true))
                ]
            ),
            &ctx(user("ann"))
        )
        .is_ok());
    assert_refused(
        &world,
        &change_post(FEATURED_PUBLISHED_POST, &[("featured", Value::Bool(false))]),
        "ann",
        ErrorKind::ConstraintViolated,
        "constraint:post.featured_by_editors",
    );
    assert_refused(
        &world,
        &change_post(FEATURED_PUBLISHED_POST, &[("published_at", far.clone())]),
        "ann",
        ErrorKind::ConstraintViolated,
        "constraint:post.published_at_by_editors",
    );
    // Editors may do all of it.
    for intent in [
        new_post("pinned", ANN_AUTHOR, &[("featured", Value::Bool(true))]),
        change_post(ANN_DRAFT, &[("featured", Value::Bool(true))]),
        change_post(ANN_DRAFT, &[("published_at", far.clone())]),
    ] {
        assert!(world.step(&intent, &ctx(user("ed"))).is_ok(), "{intent:?}");
    }
}

#[test]
fn an_author_publishes_only_her_own_post_and_the_time_is_stamped_not_chosen() {
    let world = world();
    let publish = |extra: &[(&str, Value)]| {
        let mut fields = vec![("status", Value::Str("published".into()))];
        fields.extend(extra.iter().map(|(f, v)| (*f, v.clone())));
        change_post(ANN_DRAFT, &fields)
    };
    // Another author's post: refused before the workflow is reached.
    assert_refused(
        &world,
        &publish(&[]),
        "bob",
        ErrorKind::Unauthorized,
        "policy:authors_own_posts",
    );
    // Her own: allowed, and a far-future date she supplies along the way is replaced by the
    // time of publication.
    let (published, _) = world
        .step(
            &publish(&[("published_at", Value::Time(FAR_FUTURE))]),
            &ctx(user("ann")),
        )
        .expect("an author publishes her own post");
    let post = published.tenants[&TenantId::new("gazette").unwrap()]
        .record(&ObjectTypeId::new("post").unwrap(), RecordId(ANN_DRAFT))
        .unwrap()
        .clone();
    assert_eq!(
        post.get(&FieldId::new("published_at").unwrap()),
        &Value::Time(1_800_000_000_000)
    );
    // Unpublishing is hers too, and not Bob's.
    let back = change_post(ANN_DRAFT, &[("status", Value::Str("draft".into()))]);
    assert_refused(
        &published,
        &back,
        "bob",
        ErrorKind::Unauthorized,
        "policy:authors_own_posts",
    );
    assert!(published.step(&back, &ctx(user("ann"))).is_ok());
    // An editor publishes anyone's.
    assert!(world.step(&publish(&[]), &ctx(user("ed"))).is_ok());
}
