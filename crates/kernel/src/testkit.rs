//! A small two-tenant world for tests in this and downstream crates. Built only by stepping
//! intents through the kernel, so constructing it already exercises the transition function.
//!
//! Each tenant has `person` and `note` object types. Notes have a unique slug, a title limited
//! to 24 characters, an optional rank, an optional reference to a person, and a status workflow
//! (`draft` -> `published` needs the `editor` role). The `writer` role edits its own notes, the
//! public reads published ones.

use crate::*;
use std::collections::{BTreeMap, BTreeSet};

pub fn tenant_id(s: &str) -> TenantId {
    TenantId::new(s).expect("tenant id")
}
pub fn object(s: &str) -> ObjectTypeId {
    ObjectTypeId::new(s).expect("object id")
}
pub fn field(s: &str) -> FieldId {
    FieldId::new(s).expect("field id")
}
pub fn role(s: &str) -> RoleId {
    RoleId::new(s).expect("role id")
}
pub fn user(s: &str) -> Principal {
    Principal::User(PrincipalId::new(s).expect("principal id"))
}
pub fn principal_id(s: &str) -> PrincipalId {
    PrincipalId::new(s).expect("principal id")
}
pub fn text(s: &str) -> Value {
    Value::Str(s.to_string())
}

pub fn ctx(tenant: &str, principal: Principal, now: i64) -> Context {
    Context {
        tenant: tenant_id(tenant),
        principal,
        session: None,
        now: Timestamp(now),
        correlation: format!("test-{now}"),
    }
}

pub fn var(scope: Scope, name: &str) -> Expr {
    Expr::Var(scope, name.to_string())
}
pub fn lit(s: &str) -> Expr {
    Expr::Lit(text(s))
}
pub fn eq(a: Expr, b: Expr) -> Expr {
    Expr::Cmp(CmpOp::Eq, Box::new(a), Box::new(b))
}

pub fn field_def(id: &str, ty: FieldType, required: bool) -> FieldDef {
    FieldDef {
        id: field(id),
        label: id.to_string(),
        ty,
        required,
        unique: false,
        default: None,
    }
}

fn policy(
    id: &str,
    role_name: &str,
    object_name: &str,
    actions: &[Action],
    condition: Option<Expr>,
) -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new(id).expect("policy id"),
        role: role(role_name),
        resource: Resource::Object(object(object_name)),
        actions: actions.iter().copied().collect(),
        condition,
    })
}

/// The schema changes that define the notes model.
pub fn notes_schema() -> Vec<SchemaChange> {
    let own = eq(var(Scope::Record, "$created_by"), Expr::Principal);
    let all = [Action::Read, Action::Create, Action::Update, Action::Delete];
    vec![
        SchemaChange::DefineRole(RoleDef {
            id: role("writer"),
            label: "Writer".into(),
        }),
        SchemaChange::DefineRole(RoleDef {
            id: role("editor"),
            label: "Editor".into(),
        }),
        SchemaChange::DefineObject {
            id: object("person"),
            label: "Person".into(),
        },
        SchemaChange::DefineField {
            object: object("person"),
            field: field_def("name", FieldType::Str, true),
        },
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
                unique: true,
                ..field_def("slug", FieldType::Str, true)
            },
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: field_def("body", FieldType::Str, false),
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: field_def("rank", FieldType::Int, false),
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
            field: field_def("published_at", FieldType::Time, false),
        },
        SchemaChange::DefineRelationship {
            object: object("note"),
            field: field("author"),
            label: "Author".into(),
            target: object("person"),
            on_delete: OnDelete::SetNull,
            required: false,
        },
        SchemaChange::DefineConstraint {
            object: object("note"),
            constraint: ConstraintDef {
                id: ConstraintId::new("title_length").expect("id"),
                expr: Expr::Cmp(
                    CmpOp::Le,
                    Box::new(Expr::Len(Box::new(var(Scope::New, "title")))),
                    Box::new(Expr::Lit(Value::Int(24))),
                ),
                message: "A title has at most 24 characters".into(),
            },
        },
        SchemaChange::DefineWorkflow(WorkflowDef {
            id: WorkflowId::new("note_status").expect("id"),
            object: object("note"),
            field: field("status"),
            initial: BTreeSet::from(["draft".to_string()]),
            transitions: vec![
                WorkflowTransition {
                    name: "publish".into(),
                    from: "draft".into(),
                    to: "published".into(),
                    guard: Some(Expr::HasRole(role("editor"))),
                    set: BTreeMap::from([(field("published_at"), Expr::Now)]),
                },
                WorkflowTransition {
                    name: "unpublish".into(),
                    from: "published".into(),
                    to: "draft".into(),
                    guard: None,
                    set: BTreeMap::from([(field("published_at"), Expr::Lit(Value::Null))]),
                },
            ],
        }),
        policy(
            "public_reads_published",
            "public",
            "note",
            &[Action::Read],
            Some(eq(var(Scope::Record, "status"), lit("published"))),
        ),
        policy(
            "public_reads_people",
            "public",
            "person",
            &[Action::Read],
            None,
        ),
        policy(
            "writers_read_notes",
            "writer",
            "note",
            &[Action::Read, Action::Create],
            None,
        ),
        policy(
            "writers_edit_own",
            "writer",
            "note",
            &[Action::Update, Action::Delete],
            Some(own),
        ),
        policy(
            "writers_add_people",
            "writer",
            "person",
            &[Action::Create],
            None,
        ),
        policy("editors_manage_notes", "editor", "note", &all, None),
        policy("editors_manage_people", "editor", "person", &all, None),
        SchemaChange::DefineQuery(QueryDef {
            id: QueryId::new("notes_by_rank").expect("id"),
            object: object("note"),
            filter: None,
            params: BTreeSet::new(),
            order: vec![OrderKey {
                field: "rank".into(),
                descending: true,
            }],
            limit: None,
        }),
        SchemaChange::DefineQuery(QueryDef {
            id: QueryId::new("note_by_slug").expect("id"),
            object: object("note"),
            filter: Some(eq(var(Scope::Record, "slug"), Expr::Param("slug".into()))),
            params: BTreeSet::from(["slug".to_string()]),
            order: vec![],
            limit: Some(1),
        }),
    ]
}

/// The tenants of [`notes_world`], and the principals each one knows.
pub const TENANTS: [&str; 2] = ["alpha", "beta"];

/// Principals by index: owner, editor, two writers, a stranger with no membership.
pub fn principals(tenant: &str) -> Vec<Principal> {
    ["owner", "editor", "writer1", "writer2", "stranger"]
        .iter()
        .map(|name| user(&format!("{tenant}:{name}")))
        .chain([Principal::Anonymous])
        .collect()
}

/// Two tenants with the notes schema and members, no records.
pub fn notes_world() -> World {
    let mut world = World::default();
    for (i, tenant) in TENANTS.iter().enumerate() {
        let owner = principal_id(&format!("{tenant}:owner"));
        let mut now = 1_000 + i as i64 * 100;
        let mut run = |world: &World, principal: Principal, intent: Intent| {
            now += 1;
            world
                .step(&intent, &ctx(tenant, principal, now))
                .expect("fixture intent is accepted")
                .0
        };
        world = run(
            &world,
            Principal::System,
            Intent::CreateTenant {
                name: tenant.to_string(),
                owner: owner.clone(),
            },
        );
        let owner = Principal::User(owner);
        world = run(
            &world,
            owner.clone(),
            Intent::ChangeSchema {
                changes: notes_schema(),
                expected_version: None,
                existing: ExistingData::Reject,
            },
        );
        for (name, role_name) in [
            ("editor", "editor"),
            ("writer1", "writer"),
            ("writer2", "writer"),
        ] {
            world = run(
                &world,
                owner.clone(),
                Intent::AssignRole {
                    principal: principal_id(&format!("{tenant}:{name}")),
                    role: role(role_name),
                },
            );
        }
    }
    world
}

/// A create-note intent with the given title and slug.
pub fn new_note(title: &str, slug: &str) -> Intent {
    Intent::CreateRecord {
        object: object("note"),
        fields: BTreeMap::from([(field("title"), text(title)), (field("slug"), text(slug))]),
    }
}
