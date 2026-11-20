//! Tenant definition bundles: a whole tenant written down as one JSON document.
//!
//! A bundle names the roles, object types, constraints, workflows, policies, queries, views,
//! members and seed records of a tenant, with expressions in the authoring syntax. `install`
//! turns it into ordinary kernel intents and hands them to the caller to execute, so a seeded
//! tenant has gone through exactly the same transition function as everything after it.
//!
//! The bundle of a product, the `tenant-bundle` that `specc bundle` writes from a product file, is
//! read, installed, upgraded and removed by [`crate::product`]; its names are re-exported here.

use crate::plain::{coerce_fields, parse_time};
use serde::Deserialize;
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::expr::parse;
use strata_kernel::*;

use crate::product::read_definitions;
pub use crate::product::*;

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
    /// The automations of the tenant (`automation-def`, D-6521), as JSON with expressions in the
    /// authoring syntax, in the shape the product bundle gives them.
    #[serde(default)]
    pub automations: Vec<Json>,
    /// The triggers of the tenant (`trigger-def`, D-6520).
    #[serde(default)]
    pub triggers: Vec<Json>,
    /// The tenant operations of the tenant (`tenant-operation-def`, D-6546).
    #[serde(default)]
    pub operations: Vec<Json>,
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
    /// How long records are kept; required while a field is personal (D-6584).
    #[serde(default)]
    pub retention: Option<RetentionRule>,
    /// The publication state and time of a content type (D-6592).
    #[serde(default)]
    pub publication: Option<PublicationDef>,
    /// The field that holds the language of each record (D-6593).
    #[serde(default)]
    pub language_field: Option<FieldId>,
    /// The field whose equal values group the translations of one record (D-6593).
    #[serde(default)]
    pub translation_group: Option<FieldId>,
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
    /// The personal data the field holds, whose subject it is and whether the index may hold it
    /// (D-6584); a type with such a field declares its retention.
    #[serde(default)]
    pub personal: Option<PersonalDeclaration>,
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
    /// An object type id, `*` for every object type, `$schema`, `$members`, `$ledger` or `$ledger:<unit>`.
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
                        personal: field.personal.clone(),
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
            if let Some(retention) = &object.retention {
                changes.push(SchemaChange::SetRetention {
                    object: object.id.clone(),
                    retention: retention.clone(),
                });
            }
            if let Some(publication) = &object.publication {
                changes.push(SchemaChange::SetPublication {
                    object: object.id.clone(),
                    publication: publication.clone(),
                });
            }
            if let Some(field) = &object.language_field {
                changes.push(SchemaChange::SetLanguageField {
                    object: object.id.clone(),
                    field: field.clone(),
                });
            }
            if let Some(field) = &object.translation_group {
                changes.push(SchemaChange::SetTranslationGroup {
                    object: object.id.clone(),
                    field: field.clone(),
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
                // D-6550: `$ledger` is the ledger of every unit, `$ledger:<unit>` that of one.
                "$ledger" => Resource::Ledger { unit: None },
                unit if unit.starts_with("$ledger:") => Resource::Ledger {
                    unit: Some(
                        strata_kernel::LedgerUnit::new(&unit["$ledger:".len()..])
                            .map_err(|e| format!("{at}: {e}"))?,
                    ),
                },
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
                aggregates: None,
                declared_fields: None,
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
                        aggregate: None,
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
        // Automations are defined before the triggers that name them.
        let mut sections = serde_json::Map::new();
        sections.insert("automations".into(), Json::Array(self.automations.clone()));
        sections.insert("triggers".into(), Json::Array(self.triggers.clone()));
        sections.insert("operations".into(), Json::Array(self.operations.clone()));
        let (triggers, automations, operations) =
            read_definitions(&mut sections).map_err(|e| e.message)?;
        changes.extend(
            automations
                .into_values()
                .map(SchemaChange::DefineAutomation),
        );
        changes.extend(triggers.into_values().map(SchemaChange::DefineTrigger));
        changes.extend(operations.into_values().map(SchemaChange::DefineOperation));
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
