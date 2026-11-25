//! The affordances of row actions in a view reply (WG-5466, D-DS-32).
//!
//! A row action is a ui-action with a `row`: the list section whose rows it sits in. For each row
//! of that section, the view reply carries the answer of the kernel's preflight for the action's
//! operation, with the inputs the row supplies (`row.id` and `row.<field>` arguments). The answer
//! is advisory: the invocation decides, and a refusal at the click refreshes the affordance.
//!
//! The budget of preflight answers belongs to the session, so one session cannot make the kernel
//! answer without limit. This module is pure: the caller (the backend, the impure shell) holds each
//! session's remaining budget, passes it in, and keeps the budget that is left after the reply.
//! An answer past the budget is `unknown`.

use crate::project::SectionData;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::operations::{
    operation_of, preflight, reads_individual, Preflight, PreflightBudget, UnknownReason,
};
use strata_kernel::{
    Context, Coverage, FieldId, Record, RecordExt, TenantOperationId, TenantSchema, TenantState,
    UiElement, Value, ViewId,
};

/// The key a preflight is asked under: an affordance is never stored, so the key is fixed. It is
/// a valid invocation key, which preflight checks as the decision does.
const PREFLIGHT_KEY: &str = "preflight";

/// The preflight answers one session may ask for before every answer is `unknown`: the budget a
/// session that the backend has not seen starts with.
pub const SESSION_BUDGET: u32 = 5_000;

/// The answer for one row action on one row, as the view reply carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RowAffordance {
    /// Preflight allows it: the invocation's stages 1 to 4 pass for this row.
    Allowed,
    /// Preflight refuses it: `code` is the condition's code, `detail` its wording.
    Refused { code: String, detail: String },
    /// Preflight cannot say: `reason` is `input-not-supplied`, `withheld-read`, `not-loaded` or
    /// `budget`. An ordinary action is drawn enabled.
    Unknown { reason: String },
}

/// The affordances of a view: by section, then by record id (as text, the record's `id`), then by
/// action id.
pub type Affordances = BTreeMap<String, BTreeMap<String, BTreeMap<String, RowAffordance>>>;

/// A row action of a view: its id, the section it sits in, its operation and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowAction {
    pub id: String,
    pub section: String,
    pub operation: TenantOperationId,
    pub arguments: BTreeMap<String, String>,
}

/// The row actions a view shows: the actions with a `row` on a screen of this view.
pub fn row_actions(schema: &TenantSchema, view: &ViewId) -> Vec<RowAction> {
    let screens: BTreeSet<String> = strata_kernel::ui::elements(schema)
        .filter_map(|(id, element)| match element {
            UiElement::Screen(screen) if screen.view == *view => Some(id.clone()),
            _ => None,
        })
        .collect();
    strata_kernel::ui::elements(schema)
        .filter_map(|(id, element)| match element {
            UiElement::Action(action) if screens.contains(&action.screen) => {
                let section = action.row.clone()?;
                Some(RowAction {
                    id: id.clone(),
                    section,
                    operation: action.operation.clone(),
                    arguments: action.arguments.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

/// The inputs a row supplies to an action: each argument `row.id` (the record's id) or
/// `row.<field>` (the field's value, null when the record has none). Other sources are not the
/// row's and are not supplied.
pub fn row_inputs(
    record: &Record,
    arguments: &BTreeMap<String, String>,
) -> BTreeMap<String, Value> {
    arguments
        .iter()
        .filter_map(|(input, source)| {
            let field = source.strip_prefix("row.")?;
            let value = if field == "id" {
                Value::Ref(record.id)
            } else {
                FieldId::new(field)
                    .ok()
                    .map(|id| record.get(&id).clone())
                    .unwrap_or(Value::Null)
            };
            Some((input.clone(), value))
        })
        .collect()
}

fn reason_name(reason: &UnknownReason) -> &'static str {
    match reason {
        UnknownReason::InputNotSupplied(_) => "input-not-supplied",
        UnknownReason::WithheldRead => "withheld-read",
        UnknownReason::NotLoaded => "not-loaded",
        UnknownReason::Budget => "budget",
    }
}

/// The affordances of every row action of `view` on the rows of `sections`, asked of the kernel
/// for `tenant` under `coverage` and `ctx`, and whether any of them reads the principal or the
/// clock (so the reply is individual to the reader and never stored as a shared copy). The budget
/// the session has left goes in, and the budget left after these answers comes out.
pub fn row_affordances(
    tenant: &TenantState,
    coverage: &Coverage,
    ctx: &Context,
    view: &ViewId,
    sections: &BTreeMap<String, SectionData>,
    mut budget: PreflightBudget,
) -> (Affordances, bool, PreflightBudget) {
    let mut out: Affordances = BTreeMap::new();
    let mut individual = false;
    for action in row_actions(&tenant.schema, view) {
        let Some(section) = sections.get(&action.section) else {
            continue;
        };
        let reads_clock =
            operation_of(&tenant.schema, &action.operation).is_some_and(reads_individual);
        for record in &section.rows {
            let inputs = row_inputs(record, &action.arguments);
            let answer = match preflight(
                tenant,
                coverage,
                ctx,
                &action.operation,
                &inputs,
                PREFLIGHT_KEY,
                &mut budget,
            ) {
                Preflight::Allowed => RowAffordance::Allowed,
                Preflight::Refused { code, detail } => {
                    let wording = detail
                        .strip_prefix(&format!("{code}: "))
                        .unwrap_or(&detail)
                        .to_string();
                    RowAffordance::Refused {
                        code,
                        detail: wording,
                    }
                }
                Preflight::Unknown(reason) => RowAffordance::Unknown {
                    reason: reason_name(&reason).into(),
                },
            };
            individual |= reads_clock;
            out.entry(action.section.clone())
                .or_default()
                .entry(record.id.0.to_string())
                .or_default()
                .insert(action.id.clone(), answer);
        }
    }
    (out, individual, budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_kernel::RecordId;

    #[test]
    fn a_row_supplies_its_id_and_its_fields_and_nothing_else() {
        let record = Record {
            id: RecordId(42),
            object: strata_kernel::ObjectTypeId::new("message").unwrap(),
            version: 1,
            created_by: None,
            created_at: strata_kernel::Timestamp(0),
            updated_at: strata_kernel::Timestamp(0),
            fields: BTreeMap::from([(FieldId::new("title").unwrap(), Value::Str("Hello".into()))]),
        };
        let arguments = BTreeMap::from([
            ("message".to_string(), "row.id".to_string()),
            ("title".to_string(), "row.title".to_string()),
            ("body".to_string(), "row.body".to_string()),
            ("kind".to_string(), "form.kind".to_string()),
        ]);
        assert_eq!(
            row_inputs(&record, &arguments),
            BTreeMap::from([
                ("message".to_string(), Value::Ref(RecordId(42))),
                ("title".to_string(), Value::Str("Hello".into())),
                ("body".to_string(), Value::Null),
            ])
        );
    }

    #[test]
    fn an_affordance_is_said_by_its_state() {
        assert_eq!(
            serde_json::to_value(RowAffordance::Allowed).unwrap(),
            serde_json::json!({ "state": "allowed" })
        );
        assert_eq!(
            serde_json::to_value(RowAffordance::Refused {
                code: "not-published".into(),
                detail: "Only a published note is featured.".into(),
            })
            .unwrap(),
            serde_json::json!({
                "state": "refused",
                "code": "not-published",
                "detail": "Only a published note is featured."
            })
        );
        assert_eq!(
            serde_json::to_value(RowAffordance::Unknown {
                reason: "withheld-read".into()
            })
            .unwrap(),
            serde_json::json!({ "state": "unknown", "reason": "withheld-read" })
        );
    }

    /// Alpha's notes with a view of them and a row action on its `notes` section: discard, which
    /// the row supplies as its `note`, and which only a published note allows. Note 1 is published
    /// by the editor and note 2 is a draft of writer1.
    fn notes_with_row_action() -> strata_kernel::World {
        use std::collections::{BTreeMap as Map, BTreeSet};
        use strata_kernel::testkit::*;
        use strata_kernel::*;
        fn step(world: &World, who: &str, now: i64, intent: Intent) -> World {
            world
                .step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
                .expect("accepted")
                .0
        }
        let mut world = notes_world();
        world = step(
            &world,
            "owner",
            5_000,
            Intent::ChangeSchema {
                changes: vec![SchemaChange::DefineQuery(QueryDef {
                    id: QueryId::new("all_notes").unwrap(),
                    object: object("note"),
                    filter: None,
                    params: BTreeSet::new(),
                    order: vec![],
                    limit: None,
                    aggregates: None,
                    declared_fields: None,
                })],
                expected_version: None,
                existing: ExistingData::Reject,
            },
        );
        world = step(
            &world,
            "owner",
            5_001,
            Intent::ChangeSchema {
                changes: vec![SchemaChange::DefineView(ViewDef {
                    id: ViewId::new("notes_view").unwrap(),
                    route: "/notes".into(),
                    title: "Notes".into(),
                    sections: Map::from([(
                        "notes".to_string(),
                        ViewSection {
                            marks: None,
                            joins: None,
                            query: QueryId::new("all_notes").unwrap(),
                            args: Map::new(),
                            include: vec![],
                            markdown: vec![],
                            single: false,
                            aggregate: None,
                        },
                    )]),
                })],
                expected_version: None,
                existing: ExistingData::Reject,
            },
        );
        let discard = TenantOperationDef {
            id: TenantOperationId::new("discard").unwrap(),
            label: "Discard".into(),
            inputs: vec![TenantOperationInput {
                name: "note".into(),
                ty: FieldType::Ref {
                    target: object("note"),
                    on_delete: OnDelete::Restrict,
                },
                required: true,
                label: None,
            }],
            invokers: TenantOperationInvokers {
                signed_in: true,
                roles: BTreeSet::new(),
            },
            subject: Some(TenantOperationSubject {
                object: object("note"),
                input: "note".into(),
            }),
            conditions: vec![TenantOperationCondition {
                id: "published".into(),
                expr: eq(var(Scope::Record, "status"), lit("published")),
                error: "not-published".into(),
                message: "Only a published note can be discarded.".into(),
            }],
            steps: vec![TenantOperationStep::DeleteRecord {
                id: "gone".into(),
                object: object("note"),
                record: Expr::Param("note".into()),
            }],
        };
        world = step(
            &world,
            "owner",
            5_002,
            Intent::ChangeSchema {
                changes: vec![SchemaChange::DefineOperation(discard)],
                expected_version: None,
                existing: ExistingData::Reject,
            },
        );
        world = step(
            &world,
            "owner",
            5_003,
            Intent::ChangeSchema {
                changes: vec![
                    SchemaChange::DefineUiElement {
                        id: "notes_screen".into(),
                        element: UiElement::Screen(UiScreen {
                            route: "/notes".into(),
                            view: ViewId::new("notes_view").unwrap(),
                            title: "Notes".into(),
                            kind: UiScreenKind::List,
                            auth: UiAuth::Session,
                            query: vec![],
                        }),
                    },
                    SchemaChange::DefineUiElement {
                        id: "discard_row".into(),
                        element: UiElement::Action(UiAction {
                            screen: "notes_screen".into(),
                            label: "Discard".into(),
                            operation: TenantOperationId::new("discard").unwrap(),
                            form: None,
                            row: Some("notes".into()),
                            arguments: Map::from([("note".to_string(), "row.id".to_string())]),
                            confirm: None,
                            on_success: UiOutcome {
                                toast: None,
                                navigate: None,
                            },
                            on_error: UiErrorDisplay::Inline,
                            when_refused: Some(strata_model_core::types::UiWhenRefused::Hide),
                        }),
                    },
                ],
                expected_version: None,
                existing: ExistingData::Reject,
            },
        );
        world = step(
            &world,
            "editor",
            5_004,
            new_note("Field notes", "field-notes"),
        );
        world = step(
            &world,
            "editor",
            5_005,
            Intent::UpdateRecord {
                object: object("note"),
                id: RecordId(1),
                fields: Map::from([(field("status"), text("published"))]),
                expected_version: None,
            },
        );
        step(&world, "writer1", 5_006, new_note("Draft", "draft"))
    }

    /// The row action of the notes view answers each row: the published note allowed, the draft
    /// refused with the condition's code and wording, and nothing individual, since the condition
    /// reads neither the principal nor the clock. A session with no budget left is unknown.
    #[test]
    fn a_row_action_is_answered_on_each_row_of_its_section() {
        use strata_kernel::testkit::*;
        use strata_kernel::*;
        let world = notes_with_row_action();
        let tenant = &world.tenants[&tenant_id("alpha")];
        let rows = [RecordId(1), RecordId(2)].map(|id| {
            tenant
                .record(&object("note"), id)
                .cloned()
                .expect("the note")
        });
        let sections = BTreeMap::from([(
            "notes".to_string(),
            SectionData {
                single: false,
                rows: rows.to_vec(),
                included: BTreeMap::new(),
                markdown: vec![],
                ref_targets: BTreeMap::new(),
                next: None,
                aggregate: None,
                marks: BTreeMap::new(),
                joins: BTreeMap::new(),
            },
        )]);
        let view = ViewId::new("notes_view").unwrap();
        let context = ctx("alpha", user("alpha:writer1"), 9_000);
        let (answers, individual, left) = row_affordances(
            tenant,
            &Coverage::Complete,
            &context,
            &view,
            &sections,
            PreflightBudget::new(100),
        );
        assert_eq!(answers["notes"]["1"]["discard_row"], RowAffordance::Allowed);
        assert_eq!(
            answers["notes"]["2"]["discard_row"],
            RowAffordance::Refused {
                code: "not-published".into(),
                detail: "Only a published note can be discarded.".into(),
            }
        );
        assert!(!individual);
        assert_eq!(left.remaining(), 100 - 2, "two rows, one answer each");

        let (spent, _, _) = row_affordances(
            tenant,
            &Coverage::Complete,
            &context,
            &view,
            &sections,
            PreflightBudget::new(0),
        );
        assert_eq!(
            spent["notes"]["1"]["discard_row"],
            RowAffordance::Unknown {
                reason: "budget".into()
            }
        );
    }
}
