//! Staged upgrades of a product (D-DS-16, WG-5472): the planner that splits an upgrade over the
//! schema-change cap (KRN-1283, `limits::MAX_SCHEMA_CHANGES`) into dependency-ordered stages.
//!
//! The planner is a pure function of the planned change and the installed record, so a dry run and
//! a staged job see the same stages. The rule that makes staging invisible to members is about
//! reachability: an early stage may create only entities that nothing existing names or serves, and
//! every change to an existing entity, every grant, interface element, trigger, automation, segment,
//! setting, ledger declaration, capability, unique constraint on an existing type and removal is in
//! the last stage. The last stage is the change a single commit would have made, in the same order.
//!
//! - An early stage holds the creation of object types (not one an any-object policy reaches), their
//!   fields, relationships, constraints and unique constraints, and queries, views, mail and notice
//!   templates, workflows on new types, and operations. An early operation names the reserved staging
//!   role as its only invoker; the last stage replaces its invokers with the real ones.
//! - An early change may mention only entities that are created early, or that the plan neither
//!   changes nor removes. Any other mention moves the change to the last stage, so a fixpoint runs
//!   until nothing moves. The kernel refuses a reference to an entity that is not there yet, and an
//!   early definition built on an entity the last stage then changes would be checked against the
//!   wrong definition.
//! - Each stage records the product at `<target>-stage.<n>`, owning the entities created so far and
//!   the staging role; the last stage records the target version and drops the staging role in the
//!   same commit. `foreign_change` and INT-1582 are unchanged.
//!
//! Implements: D-DS-16, WG-5472.

use crate::product::{BundleError, InstalledRecord, OwnedEntity, ProductChange, SegmentChanges};
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::*;

pub use strata_kernel::products::{is_staging_role, STAGING_ROLE_SUFFIX};

/// The reserved staging role of a product. It holds no permission and nobody holds it, and
/// `assign-role` refuses it.
pub fn staging_role(product: &str) -> String {
    format!("{product}{STAGING_ROLE_SUFFIX}")
}

/// One stage of a staged upgrade: the schema changes of one `ChangeSchema` intent, which end with
/// the record of the product at this stage's version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stage {
    /// The product version this stage records: `<target>-stage.<n>`, or the target for the last.
    pub version: String,
    pub changes: Vec<SchemaChange>,
    /// The entities this stage creates, which the record owns from this stage on.
    pub created: Vec<OwnedEntity>,
}

/// A product upgrade split into stages. The last stage records the target version. The segments
/// are not schema changes; they are defined after the last stage commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagePlan {
    pub stages: Vec<Stage>,
    pub segments: SegmentChanges,
}

/// The counts of a stage that is refused for being too large: its change count and the number of
/// changes of each kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageCounts {
    pub changes: usize,
    pub kinds: BTreeMap<String, usize>,
}

/// The message of the refusal `upgrade.final-stage-too-large`.
pub fn final_stage_too_large(counts: &StageCounts) -> String {
    let kinds = counts
        .kinds
        .iter()
        .map(|(kind, n)| format!("{kind} {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "upgrade.final-stage-too-large: the last stage of the upgrade holds {} schema changes, more than the cap of {} (KRN-1283): {kinds}; the product must be split across releases",
        counts.changes,
        strata_kernel::limits::MAX_SCHEMA_CHANGES
    )
}

/// Plans a product upgrade over the cap. `Ok(None)` when the upgrade fits in one commit, which is
/// then run as it is. The change must carry the record of the target version (an upgrade does).
pub fn plan_stages(
    change: &ProductChange,
    installed: &InstalledRecord,
    schema: &TenantSchema,
) -> Result<Option<StagePlan>, BundleError> {
    let cap = strata_kernel::limits::MAX_SCHEMA_CHANGES;
    let Some(target) = &change.record else {
        return Err(invalid_bundle(
            "a staged upgrade needs the record of the target version",
        ));
    };
    let body: Vec<&SchemaChange> = change
        .changes
        .iter()
        .filter(|c| !matches!(c, SchemaChange::RecordProduct(_)))
        .collect();
    // The record is one change of the commit as well.
    if body.len() < cap {
        return Ok(None);
    }

    let product = installed.product.as_str();
    let staging = staging_role(product);
    let created: BTreeSet<(String, String)> = change
        .created
        .iter()
        .map(|e| (e.kind.clone(), e.id.clone()))
        .collect();
    let touched: BTreeSet<String> = change
        .changed
        .iter()
        .chain(change.removed.iter())
        .map(|e| e.id.clone())
        .collect();
    let reach_all = body
        .iter()
        .any(|c| matches!(c, SchemaChange::DefinePolicy(p) if p.resource == Resource::AnyObject))
        || schema
            .policies
            .values()
            .any(|p| p.resource == Resource::AnyObject);

    let mut candidates: Vec<Candidate> = Vec::new();
    for (index, item) in body.iter().enumerate() {
        if let Some(candidate) = candidate(item, index, &created, reach_all, &staging) {
            candidates.push(candidate);
        }
    }

    // The fixpoint: a candidate stays early only while everything it mentions is early or untouched.
    // A mention is a string of the early form that equals the id of a created entity (every kind,
    // so a collision of ids only keeps a change last) or of a changed or removed entity.
    let mut created_by_id: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    for key in &created {
        created_by_id
            .entry(key.1.as_str())
            .or_default()
            .push(key.clone());
    }
    let mentions: Vec<BTreeSet<String>> = candidates
        .iter()
        .map(|c| mentioned_strings(&c.early_form))
        .collect::<Result<_, _>>()?;
    let mut early: Vec<bool> = vec![true; candidates.len()];
    loop {
        let early_keys: BTreeSet<(String, String)> = candidates
            .iter()
            .zip(&early)
            .filter(|(_, e)| **e)
            .filter_map(|(c, _)| c.defines.clone())
            .map(|e| (e.kind, e.id))
            .collect();
        let mut changed = false;
        for (i, _) in candidates.iter().enumerate() {
            if !early[i] {
                continue;
            }
            let bad = mentions[i]
                .iter()
                .any(|m| match created_by_id.get(m.as_str()) {
                    Some(keys) => keys.iter().any(|key| !early_keys.contains(key)),
                    None => touched.contains(m),
                });
            if bad {
                early[i] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Early candidates in dependency order, the same order a single commit would apply them in.
    let mut early_items: Vec<(usize, &Candidate)> = candidates
        .iter()
        .zip(&early)
        .filter(|(_, e)| **e)
        .map(|(c, _)| (c.rank as usize * 1_000_000 + c.index, c))
        .collect();
    early_items.sort_by_key(|(order, _)| *order);
    let early_candidates: BTreeSet<usize> = early_items.iter().map(|(_, c)| c.index).collect();

    // The last stage: what is not early, in the order of the single commit; an early operation
    // keeps its real definition here, which replaces its staging invoker.
    let mut last: Vec<SchemaChange> = Vec::new();
    for (index, item) in body.iter().enumerate() {
        if !early_candidates.contains(&index) || matches!(item, SchemaChange::DefineOperation(_)) {
            last.push((*item).clone());
        }
    }
    last.push(SchemaChange::RemoveRole(
        RoleId::new(staging.clone())
            .map_err(|e| invalid_bundle(format!("the staging role name: {e:?}")))?,
    ));
    let last_count = last.len() + 1;
    if last_count > cap || early_items.is_empty() {
        return Err(BundleError {
            kind: crate::product::BundleErrorKind::FinalStageTooLarge,
            message: final_stage_too_large(&counts_of(&last, 1)),
        });
    }
    last.push(SchemaChange::RecordProduct(
        target.to_kernel().map_err(invalid_bundle)?,
    ));

    // Pack the early changes: the first stage also holds the staging role.
    let per_stage = cap - 1;
    let first_stage = cap - 2;
    let mut groups: Vec<Vec<&Candidate>> = vec![Vec::new()];
    for (_, candidate) in &early_items {
        let room = if groups.len() == 1 {
            first_stage
        } else {
            per_stage
        };
        if groups.last().is_some_and(|g| g.len() >= room) {
            groups.push(Vec::new());
        }
        groups.last_mut().expect("a group").push(candidate);
    }

    let mut owned: BTreeSet<OwnedEntity> = installed.owns.iter().cloned().collect();
    let mut stages = Vec::new();
    for (n, group) in groups.iter().enumerate() {
        let number = n + 1;
        let version = format!("{}-stage.{number}", target.version);
        let mut changes = Vec::new();
        let mut made = Vec::new();
        if number == 1 {
            changes.push(SchemaChange::DefineRole(RoleDef {
                id: RoleId::new(staging.clone())
                    .map_err(|e| invalid_bundle(format!("the staging role name: {e:?}")))?,
                label: format!("{product} staged upgrade (reserved)"),
            }));
            owned.insert(OwnedEntity {
                kind: "role".to_string(),
                id: staging.clone(),
            });
        }
        for candidate in group {
            changes.push(candidate.early_form.clone());
            if let Some(entity) = &candidate.defines {
                owned.insert(entity.clone());
                made.push(entity.clone());
            }
        }
        let record = InstalledRecord {
            product: installed.product.clone(),
            version: version.clone(),
            platform: target.platform.clone(),
            owns: owned.iter().cloned().collect(),
            schema_version: None,
        };
        changes.push(SchemaChange::RecordProduct(
            record.to_kernel().map_err(invalid_bundle)?,
        ));
        stages.push(Stage {
            version,
            changes,
            created: made,
        });
    }

    let defined_early: BTreeSet<(String, String)> = early_items
        .iter()
        .filter_map(|(_, c)| c.defines.clone())
        .map(|e| (e.kind, e.id))
        .collect();
    let last_created: Vec<OwnedEntity> = change
        .created
        .iter()
        .filter(|e| !defined_early.contains(&(e.kind.clone(), e.id.clone())))
        .cloned()
        .collect();
    stages.push(Stage {
        version: target.version.clone(),
        changes: last,
        created: last_created,
    });

    Ok(Some(StagePlan {
        stages,
        segments: change.segments.clone(),
    }))
}

/// A change that may be early: its position in the single commit, its rank (dependency order
/// among early changes), the entity it defines, and the form it takes in an early stage.
struct Candidate {
    index: usize,
    rank: u8,
    defines: Option<OwnedEntity>,
    early_form: SchemaChange,
}

fn entity(kind: &str, id: impl ToString) -> OwnedEntity {
    OwnedEntity {
        kind: kind.to_string(),
        id: id.to_string(),
    }
}

/// The early candidacy of one change of the commit, by its kind alone (reachability is decided by
/// the fixpoint). `None` leaves it to the last stage.
fn candidate(
    item: &SchemaChange,
    index: usize,
    created: &BTreeSet<(String, String)>,
    reach_all: bool,
    staging: &str,
) -> Option<Candidate> {
    let made = |kind: &str, id: &str| created.contains(&(kind.to_string(), id.to_string()));
    let object_made = |object: &ObjectTypeId| made("object", &object.to_string());
    let mk = |rank: u8, defines: Option<OwnedEntity>, early_form: SchemaChange| Candidate {
        index,
        rank,
        defines,
        early_form,
    };
    match item {
        SchemaChange::DefineObject { id, .. } if made("object", &id.to_string()) && !reach_all => {
            Some(mk(1, Some(entity("object", id)), item.clone()))
        }
        SchemaChange::DefineField { object, .. }
        | SchemaChange::DefineRelationship { object, .. }
        | SchemaChange::DefineConstraint { object, .. }
        | SchemaChange::DefineUnique { object, .. }
        | SchemaChange::SetRetention { object, .. }
        | SchemaChange::SetPublication { object, .. }
        | SchemaChange::SetLanguageField { object, .. }
        | SchemaChange::SetTranslationGroup { object, .. }
            if object_made(object) =>
        {
            Some(mk(2, None, item.clone()))
        }
        SchemaChange::DefineQuery(query) if made("query", &query.id.to_string()) => {
            Some(mk(3, Some(entity("query", &query.id)), item.clone()))
        }
        SchemaChange::DefineView(view) if made("view", &view.id.to_string()) => {
            Some(mk(3, Some(entity("view", &view.id)), item.clone()))
        }
        SchemaChange::DefineMailTemplate(decl)
            if made("mail-template", &decl.template.to_string()) =>
        {
            Some(mk(
                3,
                Some(entity("mail-template", &decl.template)),
                item.clone(),
            ))
        }
        SchemaChange::DefineNoticeTemplate(decl)
            if made("notice-template", &decl.template.to_string()) =>
        {
            Some(mk(
                3,
                Some(entity("notice-template", &decl.template)),
                item.clone(),
            ))
        }
        SchemaChange::DefineWorkflow(workflow) if made("workflow", &workflow.id.to_string()) => {
            Some(mk(4, Some(entity("workflow", &workflow.id)), item.clone()))
        }
        SchemaChange::DefineOperation(op) if made("operation", &op.id.to_string()) => {
            let mut early = op.clone();
            early.invokers = TenantOperationInvokers {
                signed_in: false,
                roles: BTreeSet::from([RoleId::new(staging.to_string()).ok()?]),
            };
            Some(mk(
                5,
                Some(entity("operation", &op.id)),
                SchemaChange::DefineOperation(early),
            ))
        }
        _ => None,
    }
}

/// Every string a change holds, which is how it names other entities (ids and references). The
/// planner treats each one as a mention; a mention that is too much only keeps a change last.
fn mentioned_strings(change: &SchemaChange) -> Result<BTreeSet<String>, BundleError> {
    let value = serde_json::to_value(change)
        .map_err(|e| invalid_bundle(format!("a schema change does not serialise: {e}")))?;
    let mut out = BTreeSet::new();
    collect_strings(&value, &mut out);
    Ok(out)
}

fn collect_strings(value: &serde_json::Value, out: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::String(s) => {
            out.insert(s.clone());
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_strings(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| collect_strings(v, out)),
        _ => {}
    }
}

/// The counts of the changes that make up a stage, by the name of their kind.
fn counts_of(changes: &[SchemaChange], extra: usize) -> StageCounts {
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for change in changes {
        let name = format!("{change:?}");
        let kind: String = name.chars().take_while(|c| c.is_alphanumeric()).collect();
        *kinds.entry(kind).or_insert(0) += 1;
    }
    if extra > 0 {
        *kinds.entry("RecordProduct".to_string()).or_insert(0) += extra;
    }
    StageCounts {
        changes: changes.len() + extra,
        kinds,
    }
}

fn invalid_bundle(message: impl Into<String>) -> BundleError {
    BundleError {
        kind: crate::product::BundleErrorKind::InvalidBundle,
        message: message.into(),
    }
}
