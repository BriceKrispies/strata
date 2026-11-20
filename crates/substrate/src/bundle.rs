//! Tenant definition bundles: a whole tenant written down as one JSON document.
//!
//! A bundle names the roles, object types, constraints, workflows, policies, queries, views,
//! members and seed records of a tenant, with expressions in the authoring syntax. `install`
//! turns it into ordinary kernel intents and hands them to the caller to execute, so a seeded
//! tenant has gone through exactly the same transition function as everything after it.
//!
//! The same module reads the bundle of a product, the `tenant-bundle` that `specc bundle` writes
//! from a product file ([`ProductBundle`]), and installs the sections the kernel can take into a
//! tenant that exists ([`install_product`]), recording what the product owns ([`InstalledRecord`]).

use crate::plain::{coerce_fields, parse_time};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::expr::parse;
use strata_kernel::*;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub id: TenantId,
    pub name: String,
    pub owner: PrincipalId,
    /// When the tenant is created. Later steps follow one millisecond apart.
    pub created_at: String,
    #[serde(default)]
    pub roles: Vec<RoleSpec>,
    #[serde(default)]
    pub objects: Vec<ObjectSpec>,
    #[serde(default)]
    pub workflows: Vec<WorkflowSpec>,
    #[serde(default)]
    pub policies: Vec<PolicySpec>,
    #[serde(default)]
    pub queries: Vec<QuerySpec>,
    #[serde(default)]
    pub views: Vec<ViewSpec>,
    #[serde(default)]
    pub members: Vec<MemberSpec>,
    #[serde(default)]
    pub teams: Vec<TeamSpec>,
    #[serde(default)]
    pub records: Vec<RecordSpec>,
}

/// A role: a name and the permissions it grants, each a capability name or actions on an object
/// type.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSpec {
    pub id: RoleId,
    pub label: String,
    #[serde(default)]
    pub permissions: Vec<PermissionSpec>,
}

/// One permission of a role, as a bundle writes it: a capability name as a bare string, or an
/// object type with the actions on it (`{"object": "post", "actions": ["create"]}`). It becomes
/// the kernel's [`Permission`].
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum PermissionSpec {
    Capability(String),
    Object {
        object: ObjectTypeId,
        actions: BTreeSet<ObjectAction>,
    },
}

impl PermissionSpec {
    /// The kernel's permission.
    pub fn permission(&self) -> Permission {
        match self {
            PermissionSpec::Capability(name) => Permission::Capability(name.clone()),
            PermissionSpec::Object { object, actions } => Permission::Object {
                object: object.clone(),
                actions: actions.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectSpec {
    pub id: ObjectTypeId,
    pub label: String,
    pub fields: Vec<FieldSpec>,
    #[serde(default)]
    pub constraints: Vec<ConstraintSpec>,
    /// Composite unique constraints: two to four fields whose values no two records share
    /// (D-6558).
    #[serde(default)]
    pub unique: Vec<UniqueSpec>,
}

/// A composite unique constraint of an object type, as a bundle writes it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UniqueSpec {
    pub id: ConstraintId,
    pub fields: Vec<FieldId>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldSpec {
    pub id: FieldId,
    pub label: String,
    /// `string`, `int`, `bool`, `timestamp`, `ref:<object>` or `list:<type>`.
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub unique: bool,
    #[serde(default)]
    pub default: Option<String>,
    /// A condition over `record`, the principal and its roles: who may read the field's value.
    /// Left out, everyone who can read the record reads the field.
    #[serde(default)]
    pub read: Option<String>,
    /// For references: `restrict` (the default) or `set_null`.
    #[serde(default)]
    pub on_delete: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConstraintSpec {
    pub id: ConstraintId,
    pub check: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSpec {
    pub id: WorkflowId,
    pub object: ObjectTypeId,
    pub field: FieldId,
    pub initial: BTreeSet<String>,
    pub transitions: Vec<TransitionSpec>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionSpec {
    pub name: String,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub guard: Option<String>,
    #[serde(default)]
    pub set: BTreeMap<FieldId, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicySpec {
    pub id: PolicyId,
    pub role: RoleId,
    /// An object type id, `*` for every object type, `$schema` or `$members`.
    pub on: String,
    pub actions: Vec<String>,
    #[serde(default)]
    pub when: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuerySpec {
    pub id: QueryId,
    pub object: ObjectTypeId,
    #[serde(default, rename = "where")]
    pub filter: Option<String>,
    #[serde(default)]
    pub params: BTreeSet<String>,
    /// Field names; a leading `-` sorts descending.
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewSpec {
    pub id: ViewId,
    pub route: String,
    pub title: String,
    pub sections: BTreeMap<String, SectionSpec>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionSpec {
    pub query: QueryId,
    /// `route:<name>`, `request:<name>`, `section:<section>.<attribute>` or a literal in the
    /// expression syntax.
    #[serde(default)]
    pub args: BTreeMap<String, String>,
    #[serde(default)]
    pub include: Vec<FieldId>,
    #[serde(default)]
    pub markdown: Vec<FieldId>,
    #[serde(default)]
    pub single: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberSpec {
    pub principal: PrincipalId,
    pub roles: Vec<RoleId>,
}

/// A team: a named set of principals that holds roles together.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSpec {
    pub id: TeamId,
    pub label: String,
    #[serde(default)]
    pub roles: Vec<RoleId>,
    #[serde(default)]
    pub members: Vec<PrincipalId>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordSpec {
    pub object: ObjectTypeId,
    /// A name other records can refer to as `@key`.
    #[serde(default)]
    pub key: Option<String>,
    #[serde(rename = "as")]
    pub principal: PrincipalId,
    pub at: String,
    pub fields: serde_json::Map<String, Json>,
    /// Updates applied after creation, such as publishing.
    #[serde(default)]
    pub then: Vec<FollowUp>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowUp {
    #[serde(rename = "as")]
    pub principal: PrincipalId,
    pub at: String,
    pub set: serde_json::Map<String, Json>,
}

fn parse_type(text: &str, on_delete: OnDelete) -> Result<FieldType, String> {
    Ok(match text {
        "string" => FieldType::Str,
        "int" => FieldType::Int,
        "bool" => FieldType::Bool,
        "timestamp" => FieldType::Time,
        other => {
            if let Some(target) = other.strip_prefix("ref:") {
                FieldType::Ref {
                    target: ObjectTypeId::new(target).map_err(|e| e.to_string())?,
                    on_delete,
                }
            } else if let Some(inner) = other.strip_prefix("list:") {
                FieldType::List(Box::new(parse_type(inner, on_delete)?))
            } else {
                return Err(format!("unknown field type {other:?}"));
            }
        }
    })
}

fn parse_action(text: &str) -> Result<Action, String> {
    Ok(match text {
        "read" => Action::Read,
        "create" => Action::Create,
        "update" => Action::Update,
        "delete" => Action::Delete,
        "manage" => Action::Manage,
        other => return Err(format!("unknown action {other:?}")),
    })
}

fn optional(expr: &Option<String>) -> Result<Option<Expr>, String> {
    expr.as_deref().map(parse).transpose()
}

fn parse_arg(text: &str) -> Result<ArgSource, String> {
    if let Some(name) = text.strip_prefix("route:") {
        Ok(ArgSource::Route(name.to_string()))
    } else if let Some(name) = text.strip_prefix("request:") {
        Ok(ArgSource::Request(name.to_string()))
    } else if let Some(path) = text.strip_prefix("section:") {
        let (section, attribute) = path
            .split_once('.')
            .ok_or_else(|| format!("expected section:<name>.<attribute>, got {text:?}"))?;
        Ok(ArgSource::Section {
            section: section.to_string(),
            attribute: attribute.to_string(),
        })
    } else {
        match parse(text)? {
            Expr::Lit(value) => Ok(ArgSource::Const(value)),
            other => Err(format!(
                "a view argument is a source or a literal, got {other}"
            )),
        }
    }
}

impl Bundle {
    /// Parse a bundle document; a document that is not a bundle is refused.
    ///
    /// Implements: CT-237
    pub fn from_json(text: &str) -> Result<Bundle, String> {
        serde_json::from_str(text).map_err(|e| format!("tenant bundle: {e}"))
    }

    /// The schema changes that define this tenant's structure, in dependency order.
    pub fn schema_changes(&self) -> Result<Vec<SchemaChange>, String> {
        let mut changes = Vec::new();
        for role in &self.roles {
            changes.push(SchemaChange::DefineRole(RoleDef {
                id: role.id.clone(),
                label: role.label.clone(),
            }));
            if !role.permissions.is_empty() {
                changes.push(SchemaChange::DefineRolePermissions {
                    role: role.id.clone(),
                    permissions: role
                        .permissions
                        .iter()
                        .map(PermissionSpec::permission)
                        .collect(),
                });
            }
        }
        for object in &self.objects {
            changes.push(SchemaChange::DefineObject {
                id: object.id.clone(),
                label: object.label.clone(),
            });
        }
        for object in &self.objects {
            for field in &object.fields {
                let at = format!("{}.{}", object.id, field.id);
                let on_delete = match field.on_delete.as_deref() {
                    None | Some("restrict") => OnDelete::Restrict,
                    Some("set_null") => OnDelete::SetNull,
                    Some(other) => return Err(format!("{at}: unknown on_delete {other:?}")),
                };
                changes.push(SchemaChange::DefineField {
                    object: object.id.clone(),
                    field: FieldDef {
                        id: field.id.clone(),
                        label: field.label.clone(),
                        ty: parse_type(&field.ty, on_delete).map_err(|e| format!("{at}: {e}"))?,
                        required: field.required,
                        unique: field.unique,
                        default: optional(&field.default).map_err(|e| format!("{at}: {e}"))?,
                        read: optional(&field.read).map_err(|e| format!("{at}: {e}"))?,
                        personal: None,
                    },
                });
            }
            for constraint in &object.constraints {
                changes.push(SchemaChange::DefineConstraint {
                    object: object.id.clone(),
                    constraint: ConstraintDef {
                        id: constraint.id.clone(),
                        expr: parse(&constraint.check)
                            .map_err(|e| format!("{}.{}: {e}", object.id, constraint.id))?,
                        message: constraint.message.clone(),
                    },
                });
            }
            for unique in &object.unique {
                changes.push(SchemaChange::DefineUnique {
                    object: object.id.clone(),
                    unique: UniqueDef {
                        id: unique.id.clone(),
                        fields: unique.fields.clone(),
                        message: unique.message.clone(),
                    },
                });
            }
        }
        for workflow in &self.workflows {
            let mut transitions = Vec::new();
            for step in &workflow.transitions {
                let at = format!("workflow {}.{}", workflow.id, step.name);
                let mut set = BTreeMap::new();
                for (field, value) in &step.set {
                    set.insert(
                        field.clone(),
                        parse(value).map_err(|e| format!("{at}: {e}"))?,
                    );
                }
                transitions.push(WorkflowTransition {
                    name: step.name.clone(),
                    from: step.from.clone(),
                    to: step.to.clone(),
                    guard: optional(&step.guard).map_err(|e| format!("{at}: {e}"))?,
                    set,
                });
            }
            changes.push(SchemaChange::DefineWorkflow(WorkflowDef {
                id: workflow.id.clone(),
                object: workflow.object.clone(),
                field: workflow.field.clone(),
                initial: workflow.initial.clone(),
                transitions,
            }));
        }
        for policy in &self.policies {
            let at = format!("policy {}", policy.id);
            let resource = match policy.on.as_str() {
                "*" => Resource::AnyObject,
                "$schema" => Resource::Schema,
                "$members" => Resource::Members,
                object => {
                    Resource::Object(ObjectTypeId::new(object).map_err(|e| format!("{at}: {e}"))?)
                }
            };
            changes.push(SchemaChange::DefinePolicy(PolicyDef {
                id: policy.id.clone(),
                role: policy.role.clone(),
                resource,
                actions: policy
                    .actions
                    .iter()
                    .map(|a| parse_action(a))
                    .collect::<Result<_, _>>()
                    .map_err(|e| format!("{at}: {e}"))?,
                condition: optional(&policy.when).map_err(|e| format!("{at}: {e}"))?,
            }));
        }
        for query in &self.queries {
            changes.push(SchemaChange::DefineQuery(QueryDef {
                id: query.id.clone(),
                object: query.object.clone(),
                filter: optional(&query.filter).map_err(|e| format!("query {}: {e}", query.id))?,
                params: query.params.clone(),
                order: query
                    .order
                    .iter()
                    .map(|key| match key.strip_prefix('-') {
                        Some(field) => OrderKey {
                            field: field.to_string(),
                            descending: true,
                        },
                        None => OrderKey {
                            field: key.clone(),
                            descending: false,
                        },
                    })
                    .collect(),
                limit: query.limit,
            }));
        }
        for view in &self.views {
            let mut sections = BTreeMap::new();
            for (name, section) in &view.sections {
                let mut args = BTreeMap::new();
                for (param, source) in &section.args {
                    args.insert(
                        param.clone(),
                        parse_arg(source).map_err(|e| format!("view {}.{name}: {e}", view.id))?,
                    );
                }
                sections.insert(
                    name.clone(),
                    ViewSection {
                        query: section.query.clone(),
                        args,
                        include: section.include.clone(),
                        markdown: section.markdown.clone(),
                        single: section.single,
                    },
                );
            }
            changes.push(SchemaChange::DefineView(ViewDef {
                id: view.id.clone(),
                route: view.route.clone(),
                title: view.title.clone(),
                sections,
            }));
        }
        Ok(changes)
    }
}

/// Why an installation stopped.
#[derive(Debug)]
pub enum InstallError<E> {
    /// The bundle itself is malformed.
    Bundle(String),
    /// The executor refused a step.
    Step { step: String, error: E },
}

impl<E: std::fmt::Debug> std::fmt::Display for InstallError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Bundle(message) => write!(f, "{message}"),
            InstallError::Step { step, error } => write!(f, "{step}: {error:?}"),
        }
    }
}

/// Install a bundle by issuing intents to `execute`, which runs each one through the kernel and
/// returns its output. The same function seeds the live store and builds simulation worlds.
pub fn install<E>(
    bundle: &Bundle,
    execute: &mut dyn FnMut(Context, Intent) -> Result<Output, E>,
) -> Result<usize, InstallError<E>> {
    let bad = InstallError::Bundle;
    let changes = bundle.schema_changes().map_err(bad)?;
    let created_at = parse_time(&bundle.created_at).map_err(bad)?;
    let owner = Principal::User(bundle.owner.clone());
    let mut steps = 0usize;
    let mut run = |principal: Principal, at: i64, step: String, intent: Intent| {
        steps += 1;
        let ctx = Context {
            tenant: bundle.id.clone(),
            principal,
            session: None,
            now: Timestamp(at),
            correlation: format!("install:{}:{steps}", bundle.id),
            verified_address: None,
            idempotency_key: None,
            language: None,
            hop: None,
        };
        execute(ctx, intent).map_err(|error| InstallError::Step { step, error })
    };

    run(
        Principal::System,
        created_at,
        "create tenant".into(),
        Intent::CreateTenant {
            name: bundle.name.clone(),
            owner: bundle.owner.clone(),
        },
    )?;
    run(
        owner.clone(),
        created_at + 1,
        "define schema".into(),
        Intent::ChangeSchema {
            changes: changes.clone(),
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )?;
    let mut at = created_at + 1;
    for member in &bundle.members {
        for role in &member.roles {
            at += 1;
            run(
                owner.clone(),
                at,
                format!("assign {role} to {}", member.principal),
                Intent::AssignRole {
                    principal: member.principal.clone(),
                    role: role.clone(),
                },
            )?;
        }
    }

    for team in &bundle.teams {
        at += 1;
        run(
            owner.clone(),
            at,
            format!("define team {}", team.id),
            Intent::DefineTeam {
                team: team.id.clone(),
                label: team.label.clone(),
            },
        )?;
        for role in &team.roles {
            at += 1;
            run(
                owner.clone(),
                at,
                format!("assign {role} to team {}", team.id),
                Intent::AssignTeamRole {
                    team: team.id.clone(),
                    role: role.clone(),
                },
            )?;
        }
        for principal in &team.members {
            at += 1;
            run(
                owner.clone(),
                at,
                format!("add {principal} to team {}", team.id),
                Intent::AddTeamMember {
                    team: team.id.clone(),
                    principal: principal.clone(),
                },
            )?;
        }
    }

    // Field types come from the bundle's own object specs, parsed once more for coercion.
    let mut objects: BTreeMap<ObjectTypeId, ObjectTypeDef> = BTreeMap::new();
    for change in &changes {
        match change {
            SchemaChange::DefineObject { id, label } => {
                objects.insert(
                    id.clone(),
                    ObjectTypeDef {
                        id: id.clone(),
                        label: label.clone(),
                        fields: BTreeMap::new(),
                        constraints: BTreeMap::new(),
                        retention: None,
                        publication: None,
                        language_field: None,
                        translation_group: None,
                        unique: None,
                    },
                );
            }
            SchemaChange::DefineField { object, field } => {
                if let Some(def) = objects.get_mut(object) {
                    def.fields.insert(field.id.clone(), field.clone());
                }
            }
            _ => {}
        }
    }
    let mut keys: BTreeMap<String, RecordId> = BTreeMap::new();
    for (index, record) in bundle.records.iter().enumerate() {
        let label = record.key.clone().unwrap_or_else(|| format!("#{index}"));
        let step = format!("create {} {label}", record.object);
        let def = objects
            .get(&record.object)
            .ok_or_else(|| InstallError::Bundle(format!("{step}: unknown object type")))?;
        let fields = coerce_fields(def, &record.fields, &|key| keys.get(key).copied())
            .map_err(|e| InstallError::Bundle(format!("{step}: {e}")))?;
        let at =
            parse_time(&record.at).map_err(|e| InstallError::Bundle(format!("{step}: {e}")))?;
        let output = run(
            Principal::User(record.principal.clone()),
            at,
            step.clone(),
            Intent::CreateRecord {
                object: record.object.clone(),
                fields,
            },
        )?;
        let Output::Record { id, .. } = output else {
            return Err(InstallError::Bundle(format!(
                "{step}: executor returned {output:?}"
            )));
        };
        if let Some(key) = &record.key {
            keys.insert(key.clone(), id);
        }
        for follow in &record.then {
            let step = format!("update {} {label}", record.object);
            let fields = coerce_fields(def, &follow.set, &|key| keys.get(key).copied())
                .map_err(|e| InstallError::Bundle(format!("{step}: {e}")))?;
            let at =
                parse_time(&follow.at).map_err(|e| InstallError::Bundle(format!("{step}: {e}")))?;
            run(
                Principal::User(follow.principal.clone()),
                at,
                step,
                Intent::UpdateRecord {
                    object: record.object.clone(),
                    id,
                    fields,
                    expected_version: None,
                },
            )?;
        }
    }
    Ok(steps)
}

/// Build a world holding just this tenant, by installing the bundle into an empty world.
pub fn install_into(world: &World, bundle: &Bundle) -> Result<World, InstallError<DomainError>> {
    let mut current = world.clone();
    install(bundle, &mut |ctx, intent| {
        let (next, outcome) = current.step(&intent, &ctx)?;
        current = next;
        Ok(outcome.output)
    })?;
    Ok(current)
}

// ---- Product bundles: the `tenant-bundle` of the specification (D-6580, D-6582, D-6583) ------
//
// A product module is installed into a tenant that exists, as a bundle of declared metadata that
// specc writes from the product's own file (`specc bundle`). Where the whole-tenant `Bundle`
// above is one tenant written down once, a `ProductBundle` is read, projected and installed into a
// tenant that already has data, and every entity it installs is recorded as owned by its product.
//
// The kernel installs through schema changes. A section the kernel has no intent for yet is not
// dropped: reading it records it ([`ProductBundle::unbuilt`]), and installing a bundle that fills
// one is refused as an invalid bundle that names it (`install-bundle`, error `invalid-bundle`).

/// The members of a `tenant-bundle` document.
const BUNDLE_MEMBERS: &[&str] = &["name", "version", "product", "platform", "content"];

/// The sections of `bundle-sections` that the kernel takes through schema changes.
const BUILT_SECTIONS: &[&str] = &[
    "objects",
    "roles",
    "role-permissions",
    "policies",
    "queries",
    "views",
    "workflows",
];

/// The sections of `bundle-sections` that the kernel has no intent for yet, each with whether it
/// is a map (the others are lists).
const UNBUILT_SECTIONS: &[(&str, bool)] = &[
    ("capabilities", false),
    ("segments", true),
    ("settings", false),
    ("ledger-units", false),
    ("ledger-pools", false),
    ("ledger-limits", false),
    ("mail-templates", false),
    ("notice-templates", false),
    ("tokens", false),
    ("triggers", false),
    ("automations", false),
    ("operations", false),
];

/// The kind of document in the store's small-document table (`Store::doc_put`) that holds the
/// [`InstalledRecord`] of a product, keyed by the product id.
pub const CATALOGUE_KIND: &str = "product";

/// Which contract error a bundle problem is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleErrorKind {
    /// `invalid-bundle`: the document is malformed, or it fills a section this release does not
    /// install yet.
    InvalidBundle,
    /// `invalid`: an id outside the namespace of the product (INT-1582), or a platform release
    /// outside the range of the bundle (INT-1583).
    Invalid,
    /// `conflict`: an entity with an id of the bundle exists and the product does not own it, or
    /// another version of the product is installed (INT-1582, INT-1583).
    Conflict,
}

/// A bundle that cannot be read or installed, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleError {
    pub kind: BundleErrorKind,
    pub message: String,
}

impl BundleError {
    fn invalid_bundle(message: impl Into<String>) -> Self {
        BundleError {
            kind: BundleErrorKind::InvalidBundle,
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        BundleError {
            kind: BundleErrorKind::Invalid,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        BundleError {
            kind: BundleErrorKind::Conflict,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for BundleError {}

/// The platform releases a bundle is written for: from `min` up to, and excluding, `below`
/// (`platform-compat`, D-6583).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformRange {
    pub min: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<String>,
}

/// A section, or a declaration inside a section, that the bundle fills and the kernel cannot take
/// yet. `section` is the section (`segments`) or, for a declaration of an object type that no
/// schema change carries, `objects.retention`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unbuilt {
    pub section: String,
    /// How many entries of the section (or objects with the declaration) there are.
    pub entries: usize,
}

/// A tenant bundle of a product (`tenant-bundle`): the sections the kernel can install are typed
/// kernel values; the others are only counted in [`ProductBundle::unbuilt`].
#[derive(Clone, Debug)]
pub struct ProductBundle {
    pub name: String,
    pub version: String,
    /// The product that owns what the bundle installs. A bundle without one (a platform template)
    /// installs entities that belong to the tenant, and records no owner.
    pub product: Option<String>,
    pub platform: Option<PlatformRange>,
    pub objects: BTreeMap<ObjectTypeId, ObjectTypeDef>,
    pub roles: BTreeMap<RoleId, RoleDef>,
    pub role_permissions: BTreeMap<RoleId, Vec<Permission>>,
    pub policies: BTreeMap<PolicyId, PolicyDef>,
    pub queries: BTreeMap<QueryId, QueryDef>,
    pub views: BTreeMap<ViewId, ViewDef>,
    pub workflows: BTreeMap<WorkflowId, WorkflowDef>,
    /// What the bundle fills that the kernel cannot install yet, by section name.
    pub unbuilt: Vec<Unbuilt>,
}

/// One entity a product owns, as the tenant catalogue records it (`entity-ref`). `kind` is a value
/// of `entity-kind`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedEntity {
    pub kind: String,
    pub id: String,
}

/// An installed product, as the tenant catalogue holds it (`installed-product`): its version, the
/// platform range it declared and every entity it owns, which is what a removal finds (D-6582).
/// `schema_version` is the schema version the install produced, which an install of the same
/// version again returns (D-6583).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledRecord {
    pub product: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<PlatformRange>,
    pub owns: Vec<OwnedEntity>,
    #[serde(
        default,
        rename = "schema-version",
        skip_serializing_if = "Option::is_none"
    )]
    pub schema_version: Option<u64>,
}

impl InstalledRecord {
    /// The JSON text kept as the document [`CATALOGUE_KIND`] of the product.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| format!("installed product {}: {e}", self.product))
    }

    /// Reads the document kept by [`InstalledRecord::to_json`].
    pub fn from_json(text: &str) -> Result<InstalledRecord, String> {
        serde_json::from_str(text).map_err(|e| format!("installed product: {e}"))
    }
}

/// Whether `id` is a product id: lower-case words joined by hyphens, at most 64 bytes.
fn is_product_id(id: &str) -> bool {
    id.len() <= 64
        && id.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The text of an expression (the authoring syntax of the tenant rule language) as the AST a
/// kernel value holds, in place; a value that is not text is the AST already and is left alone.
fn text_to_ast(slot: &mut Json, at: &str) -> Result<(), String> {
    let ast = match &*slot {
        Json::String(text) => {
            let expr = parse(text).map_err(|e| format!("{at}: {e}"))?;
            Some(serde_json::to_value(&expr).map_err(|e| format!("{at}: {e}"))?)
        }
        _ => None,
    };
    if let Some(ast) = ast {
        *slot = ast;
    }
    Ok(())
}

/// The expressions of one object type: the default and read rule of each field, the check of each
/// constraint.
fn expressions_of_object(def: &mut Json, at: &str) -> Result<(), String> {
    if let Some(fields) = def.get_mut("fields").and_then(Json::as_object_mut) {
        for (id, field) in fields.iter_mut() {
            for key in ["default", "read"] {
                if let Some(slot) = field.get_mut(key) {
                    text_to_ast(slot, &format!("{at}.{id}.{key}"))?;
                }
            }
        }
    }
    if let Some(constraints) = def.get_mut("constraints").and_then(Json::as_object_mut) {
        for (id, constraint) in constraints.iter_mut() {
            if let Some(slot) = constraint.get_mut("expr") {
                text_to_ast(slot, &format!("{at}.{id}.expr"))?;
            }
        }
    }
    Ok(())
}

/// The condition of a policy.
fn expressions_of_policy(def: &mut Json, at: &str) -> Result<(), String> {
    match def.get_mut("condition") {
        Some(slot) => text_to_ast(slot, &format!("{at}.condition")),
        None => Ok(()),
    }
}

/// The filter of a query.
fn expressions_of_query(def: &mut Json, at: &str) -> Result<(), String> {
    match def.get_mut("filter") {
        Some(slot) => text_to_ast(slot, &format!("{at}.filter")),
        None => Ok(()),
    }
}

/// The guard and the assignments of each transition of a workflow.
fn expressions_of_workflow(def: &mut Json, at: &str) -> Result<(), String> {
    let Some(transitions) = def.get_mut("transitions").and_then(Json::as_array_mut) else {
        return Ok(());
    };
    for transition in transitions {
        let name = transition
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or("?")
            .to_string();
        if let Some(slot) = transition.get_mut("guard") {
            text_to_ast(slot, &format!("{at}.{name}.guard"))?;
        }
        if let Some(set) = transition.get_mut("set").and_then(Json::as_object_mut) {
            for (field, slot) in set.iter_mut() {
                text_to_ast(slot, &format!("{at}.{name}.set.{field}"))?;
            }
        }
    }
    Ok(())
}

/// A section that is a map from id to definition: each definition has its expression texts read
/// as ASTs by `convert`, must name itself with its key when it names itself at all, and is then
/// read as a `V`. A section that is left out or null is empty.
fn parse_map<K, V>(
    content: &mut serde_json::Map<String, Json>,
    name: &str,
    convert: &dyn Fn(&mut Json, &str) -> Result<(), String>,
) -> Result<BTreeMap<K, V>, BundleError>
where
    K: serde::de::DeserializeOwned + Ord,
    V: serde::de::DeserializeOwned,
{
    let bad =
        |message: String| BundleError::invalid_bundle(format!("tenant bundle: {name}: {message}"));
    let mut value = match content.remove(name) {
        None | Some(Json::Null) => return Ok(BTreeMap::new()),
        Some(value) => value,
    };
    let Json::Object(entries) = &mut value else {
        return Err(bad("must be a map from an id to its definition".to_string()));
    };
    for (id, def) in entries.iter_mut() {
        convert(def, id).map_err(|e| bad(format!("{id}: {e}")))?;
        if let Some(Json::String(own)) = def.get("id") {
            if own != id {
                return Err(bad(format!(
                    "{id} is keyed {id:?} but names itself {own:?}"
                )));
            }
        }
    }
    serde_json::from_value(value).map_err(|e| bad(e.to_string()))
}

/// One permission of a role as a bundle writes it: a capability name or `{object, actions}`
/// (untagged), or a `Permission` as the kernel writes it (`{"Capability": ...}`).
fn permission_from_json(item: &Json) -> Result<Permission, String> {
    match serde_json::from_value::<PermissionSpec>(item.clone()) {
        Ok(spec) => Ok(spec.permission()),
        Err(_) => serde_json::from_value::<Permission>(item.clone()).map_err(|e| e.to_string()),
    }
}

fn parse_role_permissions(
    content: &mut serde_json::Map<String, Json>,
) -> Result<BTreeMap<RoleId, Vec<Permission>>, BundleError> {
    let bad = |message: String| {
        BundleError::invalid_bundle(format!("tenant bundle: role-permissions: {message}"))
    };
    let value = match content.remove("role-permissions") {
        None | Some(Json::Null) => return Ok(BTreeMap::new()),
        Some(value) => value,
    };
    let Json::Object(entries) = value else {
        return Err(bad(
            "must be a map from a role id to its permissions".to_string()
        ));
    };
    let mut out = BTreeMap::new();
    for (role, list) in entries {
        let Json::Array(items) = list else {
            return Err(bad(format!("{role}: must be a list of permissions")));
        };
        let id = RoleId::new(role.as_str()).map_err(|e| bad(format!("{role}: {e}")))?;
        let permissions = items
            .iter()
            .map(permission_from_json)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| bad(format!("{role}: {e}")))?;
        out.insert(id, permissions);
    }
    Ok(out)
}

impl ProductBundle {
    /// Reads a `tenant-bundle` document: its members, and each section the kernel installs as
    /// kernel values (expressions written as text are read with the tenant rule language). A
    /// section the kernel cannot install yet is recorded in [`ProductBundle::unbuilt`], not read.
    /// Refuses a malformed document, an unknown member or section, a definition whose key is not
    /// its id, an expression that does not parse, and permissions for a role the bundle does not
    /// declare.
    pub fn from_json(text: &str) -> Result<ProductBundle, BundleError> {
        let bad =
            |message: String| BundleError::invalid_bundle(format!("tenant bundle: {message}"));
        let doc: Json = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
        let Json::Object(mut root) = doc else {
            return Err(bad("the document must be an object".to_string()));
        };
        for key in root.keys() {
            if !BUNDLE_MEMBERS.contains(&key.as_str()) {
                return Err(bad(format!(
                    "unknown member {key:?}; a bundle has {}",
                    BUNDLE_MEMBERS.join(", ")
                )));
            }
        }
        let required = |root: &serde_json::Map<String, Json>, key: &str| match root.get(key) {
            Some(Json::String(text)) if !text.is_empty() => Ok(text.clone()),
            _ => Err(bad(format!("{key} must be a non-empty string"))),
        };
        let name = required(&root, "name")?;
        let version = required(&root, "version")?;
        let product = match root.get("product") {
            None | Some(Json::Null) => None,
            Some(Json::String(id)) if is_product_id(id) => Some(id.clone()),
            Some(other) => {
                return Err(bad(format!(
                    "product must be lower-case words joined by hyphens, not {other}"
                )))
            }
        };
        let platform = match root.remove("platform") {
            None | Some(Json::Null) => None,
            Some(value) => Some(
                serde_json::from_value::<PlatformRange>(value)
                    .map_err(|e| bad(format!("platform: {e}")))?,
            ),
        };
        let Some(Json::Object(mut content)) = root.remove("content") else {
            return Err(bad("content must be an object of sections".to_string()));
        };
        for key in content.keys() {
            let known = BUILT_SECTIONS.contains(&key.as_str())
                || UNBUILT_SECTIONS
                    .iter()
                    .any(|(name, _)| *name == key.as_str());
            if !known {
                return Err(bad(format!(
                    "content has the section {key:?}, which is not a section of a bundle"
                )));
            }
        }
        let objects: BTreeMap<ObjectTypeId, ObjectTypeDef> =
            parse_map(&mut content, "objects", &expressions_of_object)?;
        let roles: BTreeMap<RoleId, RoleDef> = parse_map(&mut content, "roles", &|_, _| Ok(()))?;
        let role_permissions = parse_role_permissions(&mut content)?;
        let policies: BTreeMap<PolicyId, PolicyDef> =
            parse_map(&mut content, "policies", &expressions_of_policy)?;
        let queries: BTreeMap<QueryId, QueryDef> =
            parse_map(&mut content, "queries", &expressions_of_query)?;
        let views: BTreeMap<ViewId, ViewDef> = parse_map(&mut content, "views", &|_, _| Ok(()))?;
        let workflows: BTreeMap<WorkflowId, WorkflowDef> =
            parse_map(&mut content, "workflows", &expressions_of_workflow)?;
        for role in role_permissions.keys() {
            if !roles.contains_key(role) {
                return Err(bad(format!(
                    "role-permissions gives permissions to {role}, which the bundle does not declare in roles"
                )));
            }
        }
        let mut unbuilt = Vec::new();
        for &(section, is_map) in UNBUILT_SECTIONS {
            let entries = match content.remove(section) {
                None | Some(Json::Null) => 0,
                Some(Json::Array(items)) if !is_map => items.len(),
                Some(Json::Object(entries)) if is_map => entries.len(),
                Some(_) => {
                    return Err(bad(format!(
                        "{section} must be a {}",
                        if is_map { "map" } else { "list" }
                    )))
                }
            };
            if entries > 0 {
                unbuilt.push(Unbuilt {
                    section: section.to_string(),
                    entries,
                });
            }
        }
        // Declarations of an object type that no schema change carries yet.
        let declared = |present: &dyn Fn(&ObjectTypeDef) -> bool| {
            objects.values().filter(|o| present(o)).count()
        };
        for (what, entries) in [
            ("objects.retention", declared(&|o| o.retention.is_some())),
            (
                "objects.publication",
                declared(&|o| o.publication.is_some()),
            ),
            (
                "objects.language-field",
                declared(&|o| o.language_field.is_some()),
            ),
            (
                "objects.translation-group",
                declared(&|o| o.translation_group.is_some()),
            ),
        ] {
            if entries > 0 {
                unbuilt.push(Unbuilt {
                    section: what.to_string(),
                    entries,
                });
            }
        }
        unbuilt.sort_by(|a, b| a.section.cmp(&b.section));
        Ok(ProductBundle {
            name,
            version,
            product,
            platform,
            objects,
            roles,
            role_permissions,
            policies,
            queries,
            views,
            workflows,
            unbuilt,
        })
    }

    /// The namespace of the entities of the product: its id with each hyphen replaced by an
    /// underscore, followed by two underscores (D-6582).
    pub fn namespace(&self) -> Option<String> {
        self.product
            .as_ref()
            .map(|product| format!("{}__", product.replace('-', "_")))
    }

    /// Every entity the bundle installs, in install order: roles, object types, policies,
    /// workflows, queries and views, each by id.
    pub fn entities(&self) -> Vec<OwnedEntity> {
        let mut out = Vec::new();
        let mut add = |kind: &str, id: String| {
            out.push(OwnedEntity {
                kind: kind.to_string(),
                id,
            })
        };
        for id in self.roles.keys() {
            add("role", id.to_string());
        }
        for id in self.objects.keys() {
            add("object", id.to_string());
        }
        for id in self.policies.keys() {
            add("policy", id.to_string());
        }
        for id in self.workflows.keys() {
            add("workflow", id.to_string());
        }
        for id in self.queries.keys() {
            add("query", id.to_string());
        }
        for id in self.views.keys() {
            add("view", id.to_string());
        }
        out
    }

    /// The schema changes that install the bundle, in dependency order: roles and their
    /// permissions, object types with their fields and constraints, workflows, policies,
    /// queries and views.
    pub fn schema_changes(&self) -> Vec<SchemaChange> {
        let mut changes = Vec::new();
        for (id, role) in &self.roles {
            changes.push(SchemaChange::DefineRole(role.clone()));
            if let Some(permissions) = self.role_permissions.get(id) {
                if !permissions.is_empty() {
                    changes.push(SchemaChange::DefineRolePermissions {
                        role: id.clone(),
                        permissions: permissions.clone(),
                    });
                }
            }
        }
        for (id, object) in &self.objects {
            changes.push(SchemaChange::DefineObject {
                id: id.clone(),
                label: object.label.clone(),
            });
        }
        for (id, object) in &self.objects {
            for field in object.fields.values() {
                changes.push(SchemaChange::DefineField {
                    object: id.clone(),
                    field: field.clone(),
                });
            }
            for constraint in object.constraints.values() {
                changes.push(SchemaChange::DefineConstraint {
                    object: id.clone(),
                    constraint: constraint.clone(),
                });
            }
            for unique in object.unique.iter().flat_map(BTreeMap::values) {
                changes.push(SchemaChange::DefineUnique {
                    object: id.clone(),
                    unique: unique.clone(),
                });
            }
        }
        for workflow in self.workflows.values() {
            changes.push(SchemaChange::DefineWorkflow(workflow.clone()));
        }
        for policy in self.policies.values() {
            changes.push(SchemaChange::DefinePolicy(policy.clone()));
        }
        for query in self.queries.values() {
            changes.push(SchemaChange::DefineQuery(query.clone()));
        }
        for view in self.views.values() {
            changes.push(SchemaChange::DefineView(view.clone()));
        }
        changes
    }

    /// Refuses a bundle that fills a section the kernel cannot install yet, naming each, and a
    /// bundle with an id outside the namespace of its product.
    pub fn check_installable(&self) -> Result<(), BundleError> {
        if !self.unbuilt.is_empty() {
            let listed = self
                .unbuilt
                .iter()
                .map(|u| format!("\"{}\" ({} entries)", u.section, u.entries))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(BundleError::invalid_bundle(format!(
                "the bundle fills {listed}, which this release does not install yet; remove {} from the bundle, or install it on a release that takes it",
                if self.unbuilt.len() == 1 { "it" } else { "them" }
            )));
        }
        if let (Some(namespace), Some(product)) = (self.namespace(), &self.product) {
            for entity in self.entities() {
                if entity.id.strip_prefix(&namespace).is_none_or(str::is_empty) {
                    return Err(BundleError::invalid(format!(
                        "the {} \"{}\" is not in the namespace \"{namespace}\" of the product \"{product}\"; every entity a product installs has an id that begins with its namespace (INT-1582)",
                        entity.kind, entity.id
                    )));
                }
            }
        }
        Ok(())
    }
}

/// The leading number of each dot-separated part of a platform release (`0.2-draft` is `[0, 2]`).
fn release_parts(release: &str) -> Vec<u64> {
    release
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u64>()
                .unwrap_or(0)
        })
        .collect()
}

/// Orders two platform releases by their numbers, a missing part counting as zero.
fn release_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (a, b) = (release_parts(a), release_parts(b));
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => {}
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

impl PlatformRange {
    /// Whether `release` is from `min` up to, and excluding, `below`.
    pub fn admits(&self, release: &str) -> bool {
        release_cmp(release, &self.min) != std::cmp::Ordering::Less
            && self
                .below
                .as_deref()
                .is_none_or(|below| release_cmp(release, below) == std::cmp::Ordering::Less)
    }
}

/// What a product install runs against.
pub struct ProductTarget<'a> {
    pub tenant: &'a TenantId,
    /// Who installs: an owner of the tenant.
    pub principal: Principal,
    /// The time of the install, in milliseconds.
    pub now: i64,
    /// The platform release the install runs on.
    pub release: &'a str,
    /// The tenant's schema now.
    pub schema: &'a TenantSchema,
    /// The catalogue's record of the product, when it is installed ([`CATALOGUE_KIND`]).
    pub installed: Option<&'a InstalledRecord>,
}

/// What an install did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProductInstall {
    /// The same product and version was installed already: nothing changed, and this is the schema
    /// version of the first install (INT-1583).
    Unchanged { schema_version: u64 },
    /// The schema change was applied (one intent, so all of it or none), at this schema version.
    /// `record` is what the catalogue holds for the product from now on; it is `None` for a
    /// bundle that names no product.
    Applied {
        schema_version: u64,
        record: Option<InstalledRecord>,
    },
}

/// Why a product install stopped.
#[derive(Debug)]
pub enum ProductInstallError<E> {
    /// The bundle was refused before anything ran.
    Refused(BundleError),
    /// The executor refused the schema change.
    Step { step: String, error: E },
}

impl<E> From<BundleError> for ProductInstallError<E> {
    fn from(error: BundleError) -> Self {
        ProductInstallError::Refused(error)
    }
}

impl<E: std::fmt::Debug> std::fmt::Display for ProductInstallError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProductInstallError::Refused(error) => write!(f, "{error}"),
            ProductInstallError::Step { step, error } => write!(f, "{step}: {error:?}"),
        }
    }
}

/// Installs the bundle of a product into a tenant that exists, by issuing one `ChangeSchema` intent
/// to `execute` (the edits of a bundle are applied together or not at all, D-6581). In this order it
/// refuses: a section the kernel cannot install yet, an id outside the namespace of the product, a
/// platform release outside the bundle's range, another version of the product already installed,
/// and an entity with one of its ids that exists in the tenant and is not owned by the product. The
/// same product and version again is answered with the first schema version and runs nothing. The
/// caller keeps the returned record as the document [`CATALOGUE_KIND`] of the product.
pub fn install_product<E>(
    bundle: &ProductBundle,
    target: &ProductTarget<'_>,
    execute: &mut dyn FnMut(Context, Intent) -> Result<Output, E>,
) -> Result<ProductInstall, ProductInstallError<E>> {
    bundle.check_installable()?;
    if let Some(range) = &bundle.platform {
        if !range.admits(target.release) {
            return Err(BundleError::invalid(format!(
                "the platform release {} is outside the range the bundle supports (from {}{})",
                target.release,
                range.min,
                range
                    .below
                    .as_ref()
                    .map(|below| format!(" up to, and excluding, {below}"))
                    .unwrap_or_default()
            ))
            .into());
        }
    }
    let owner = bundle
        .product
        .as_ref()
        .map(|product| format!("the product \"{product}\""));
    if let (Some(product), Some(installed)) = (&bundle.product, target.installed) {
        if installed.product == *product {
            if installed.version == bundle.version {
                return Ok(ProductInstall::Unchanged {
                    schema_version: installed.schema_version.unwrap_or(target.schema.version.0),
                });
            }
            return Err(BundleError::conflict(format!(
                "version {} of the product \"{product}\" is installed; version {} is an upgrade, which upgrade-bundle applies",
                installed.version, bundle.version
            ))
            .into());
        }
    }
    let schema = target.schema;
    for entity in bundle.entities() {
        let exists = match entity.kind.as_str() {
            "role" => schema.roles.keys().any(|id| id.to_string() == entity.id),
            "object" => schema.objects.keys().any(|id| id.to_string() == entity.id),
            "policy" => schema.policies.keys().any(|id| id.to_string() == entity.id),
            "workflow" => schema
                .workflows
                .keys()
                .any(|id| id.to_string() == entity.id),
            "query" => schema.queries.keys().any(|id| id.to_string() == entity.id),
            _ => schema.views.keys().any(|id| id.to_string() == entity.id),
        };
        if exists {
            return Err(BundleError::conflict(format!(
                "the {} \"{}\" exists in the tenant and is not owned by {}; an install does not take over an entity",
                entity.kind,
                entity.id,
                owner.as_deref().unwrap_or("the bundle")
            ))
            .into());
        }
    }
    let changes = bundle.schema_changes();
    let schema_version = if changes.is_empty() {
        schema.version.0
    } else {
        let step = format!(
            "install {}",
            bundle.product.as_deref().unwrap_or(&bundle.name)
        );
        let ctx = Context {
            tenant: target.tenant.clone(),
            principal: target.principal.clone(),
            session: None,
            now: Timestamp(target.now),
            correlation: format!("{step}:{}", bundle.version),
            verified_address: None,
            idempotency_key: None,
            language: None,
            hop: None,
        };
        let intent = Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        };
        match execute(ctx, intent) {
            Ok(Output::Schema { version }) => version.0,
            Ok(other) => {
                return Err(BundleError::invalid_bundle(format!(
                    "{step}: the executor returned {other:?}, not a schema version"
                ))
                .into())
            }
            Err(error) => return Err(ProductInstallError::Step { step, error }),
        }
    };
    let record = bundle.product.as_ref().map(|product| InstalledRecord {
        product: product.clone(),
        version: bundle.version.clone(),
        platform: bundle.platform.clone(),
        owns: bundle.entities(),
        schema_version: Some(schema_version),
    });
    Ok(ProductInstall::Applied {
        schema_version,
        record,
    })
}
