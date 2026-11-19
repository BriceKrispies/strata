//! Seeded workload generation from a tenant's own schema.
//!
//! The generator knows nothing about any particular application. It reads the schema of the
//! tenant it is given (object types, field types, uniqueness, workflows, simple constraints,
//! members) and produces creates, edits, workflow moves and deletes that a member of that
//! tenant could plausibly attempt. It runs each one against a scratch copy of the world, with
//! the real kernel, so that later operations refer to records that actually exist.
//!
//! The result is a sample, and anything counted over it is labelled `Empirical`.

use crate::engine::{Rng, Scheduled};
use std::collections::BTreeMap;
use strata_kernel::ext::*;
use strata_kernel::{
    CmpOp, Context, Expr, FieldDef, FieldId, FieldType, Intent, ObjectTypeDef, Principal, Record,
    RecordId, Scope, TenantId, TenantSchema, TenantState, Timestamp, Value, World,
};

/// What the constraints of an object type say about one of its string fields.
#[derive(Default)]
struct Hints {
    slug: bool,
    choices: Vec<Value>,
    /// Constrained in a way the generator does not understand; left alone when optional.
    opaque: bool,
}

fn field_of(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Var(Scope::New | Scope::Record, name) => Some(name),
        Expr::Len(inner) | Expr::Lower(inner) => field_of(inner),
        _ => None,
    }
}

fn collect_hints(expr: &Expr, hints: &mut BTreeMap<String, Hints>) {
    match expr {
        Expr::IsSlug(inner) => {
            if let Some(name) = field_of(inner) {
                hints.entry(name.to_string()).or_default().slug = true;
            }
        }
        Expr::In(item, list) => {
            if let (Some(name), Expr::Lit(Value::List(items))) = (field_of(item), list.as_ref()) {
                hints.entry(name.to_string()).or_default().choices = items.clone();
            }
        }
        Expr::StartsWith(text, _) | Expr::Contains(text, _) => {
            if let Some(name) = field_of(text) {
                hints.entry(name.to_string()).or_default().opaque = true;
            }
        }
        Expr::Cmp(CmpOp::Eq, left, _) => {
            // `new.key == 'main'` style: a fixed value the generator would not guess.
            if let (Some(name), Expr::Var(..)) = (field_of(left), left.as_ref()) {
                hints.entry(name.to_string()).or_default().opaque = true;
            }
        }
        Expr::Not(inner) | Expr::IsNull(inner) => collect_hints(inner, hints),
        Expr::And(items) | Expr::Or(items) => {
            items.iter().for_each(|item| collect_hints(item, hints))
        }
        Expr::If(a, b, c) => [a, b, c].iter().for_each(|item| collect_hints(item, hints)),
        _ => {}
    }
}

fn hints_for(object: &ObjectTypeDef) -> BTreeMap<String, Hints> {
    let mut hints = BTreeMap::new();
    for constraint in object.constraints.values() {
        collect_hints(&constraint.expr, &mut hints);
    }
    hints
}

const WORDS: [&str; 16] = [
    "grain", "ledger", "margin", "kiln", "draft", "joint", "ember", "plane", "thread", "glaze",
    "measure", "bench", "proof", "ink", "copper", "linen",
];

fn phrase(rng: &mut Rng, words: u64) -> String {
    let mut out = String::new();
    for i in 0..words {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(WORDS[rng.below(WORDS.len() as u64) as usize]);
    }
    let mut chars = out.chars();
    chars.next().map_or(String::new(), |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A value for a field, or `None` to leave it out.
fn synthesise(
    rng: &mut Rng,
    tenant: &TenantState,
    schema: &TenantSchema,
    object: &ObjectTypeDef,
    field: &FieldDef,
    hints: &BTreeMap<String, Hints>,
    serial: u64,
    now: i64,
) -> Option<Value> {
    let hint = hints.get(field.id.as_str());
    let workflow = schema
        .workflows_of(&object.id)
        .find(|w| w.field == field.id);
    Some(match &field.ty {
        FieldType::Str => {
            if let Some(workflow) = workflow {
                let initial: Vec<&String> = workflow.initial.iter().collect();
                Value::Str(initial[rng.below(initial.len() as u64) as usize].clone())
            } else if let Some(choices) = hint.filter(|h| !h.choices.is_empty()).map(|h| &h.choices)
            {
                choices[rng.below(choices.len() as u64) as usize].clone()
            } else if hint.is_some_and(|h| h.opaque) {
                return None;
            } else if field.unique || hint.is_some_and(|h| h.slug) {
                Value::Str(format!(
                    "{}-{serial}",
                    WORDS[rng.below(WORDS.len() as u64) as usize]
                ))
            } else {
                let words = 2 + rng.below(4);
                Value::Str(phrase(rng, words))
            }
        }
        FieldType::Int => Value::Int(rng.below(100) as i64),
        FieldType::Bool => Value::Bool(rng.below(4) == 0),
        FieldType::Time => Value::Time(now),
        FieldType::Ref { target, .. } => {
            let ids: Vec<RecordId> = tenant.records_of(target).map(|r| r.id).collect();
            if ids.is_empty() {
                return None;
            }
            Value::Ref(ids[rng.below(ids.len() as u64) as usize])
        }
        FieldType::List(inner) => match inner.as_ref() {
            FieldType::Str => Value::List(
                (0..rng.below(3))
                    .map(|_| Value::Str(WORDS[rng.below(WORDS.len() as u64) as usize].to_string()))
                    .collect(),
            ),
            _ => return None,
        },
    })
}

fn pick<'a, T>(rng: &mut Rng, items: &'a [T]) -> Option<&'a T> {
    if items.is_empty() {
        None
    } else {
        Some(&items[rng.below(items.len() as u64) as usize])
    }
}

fn actor_for(rng: &mut Rng, tenant: &TenantState, record: Option<&Record>) -> Principal {
    let members: Vec<Principal> = tenant
        .members
        .keys()
        .cloned()
        .map(Principal::User)
        .collect();
    if rng.below(12) == 0 {
        return Principal::Anonymous;
    }
    let creator = record
        .and_then(|r| r.created_by.clone())
        .filter(|id| tenant.members.contains_key(id))
        .map(Principal::User);
    match creator {
        Some(creator) if rng.below(10) < 7 => creator,
        _ => pick(rng, &members).cloned().unwrap_or(Principal::Anonymous),
    }
}

/// Generate `count` operations against `tenant`, one every `spacing_ms` starting at `start`.
pub fn generate(
    world: &World,
    tenant: &TenantId,
    seed: u64,
    count: usize,
    start: i64,
    spacing_ms: i64,
) -> Vec<Scheduled> {
    let mut rng = Rng(seed);
    let mut scratch = world.clone();
    let mut schedule = Vec::with_capacity(count);
    for serial in 0..count as u64 {
        let Some(state) = scratch.tenants.get(tenant).cloned() else {
            break;
        };
        let schema = &state.schema;
        let now = start + serial as i64 * spacing_ms;
        // Object types are weighted by how many records they already have, so the generated
        // activity resembles the tenant's existing proportions.
        let mut weighted: Vec<&ObjectTypeDef> = Vec::new();
        for object in schema.objects.values() {
            let weight = 1 + state
                .records
                .get(&object.id)
                .map_or(0, BTreeMap::len)
                .min(12);
            weighted.extend(std::iter::repeat_n(object, weight));
        }
        let Some(object) = pick(&mut rng, &weighted).copied() else {
            break;
        };
        let hints = hints_for(object);
        let existing: Vec<&std::sync::Arc<Record>> = state.records_of(&object.id).collect();
        let roll = rng.below(100);
        let workflows: Vec<_> = schema.workflows_of(&object.id).collect();

        let (label, principal, intent) = if roll < 30 || existing.is_empty() {
            let mut fields = BTreeMap::new();
            for field in object.fields.values() {
                let wanted = field.required || rng.below(2) == 0;
                // A field with a default is left to the kernel, as a real client would leave it.
                if !wanted || field.default.is_some() {
                    continue;
                }
                if let Some(value) =
                    synthesise(&mut rng, &state, schema, object, field, &hints, serial, now)
                {
                    fields.insert(field.id.clone(), value);
                }
            }
            (
                format!("create a {}", object.label.to_lowercase()),
                actor_for(&mut rng, &state, None),
                Intent::CreateRecord {
                    object: object.id.clone(),
                    fields,
                },
            )
        } else if roll < 50 && !workflows.is_empty() {
            let record = *pick(&mut rng, &existing).expect("non-empty");
            let workflow = *pick(&mut rng, &workflows).expect("non-empty");
            let current = record
                .get(&workflow.field)
                .as_str()
                .unwrap_or("")
                .to_string();
            let moves: Vec<_> = workflow
                .transitions
                .iter()
                .filter(|t| t.from == current)
                .collect();
            match pick(&mut rng, &moves) {
                Some(step) => (
                    format!(
                        "{} {} {}",
                        step.name,
                        object.label.to_lowercase(),
                        record.id
                    ),
                    actor_for(&mut rng, &state, Some(record)),
                    Intent::UpdateRecord {
                        object: object.id.clone(),
                        id: record.id,
                        fields: BTreeMap::from([(
                            workflow.field.clone(),
                            Value::Str(step.to.clone()),
                        )]),
                        expected_version: None,
                    },
                ),
                None => continue,
            }
        } else if roll < 95 {
            let record = *pick(&mut rng, &existing).expect("non-empty");
            // Edit one plain field: text the generator can write, or a number.
            let editable: Vec<&FieldDef> = object
                .fields
                .values()
                .filter(|f| matches!(f.ty, FieldType::Str | FieldType::Int | FieldType::Bool))
                .filter(|f| !f.unique && workflows.iter().all(|w| w.field != f.id))
                .filter(|f| {
                    hints
                        .get(f.id.as_str())
                        .is_none_or(|h| !h.opaque && !h.slug)
                })
                .collect();
            let Some(field) = pick(&mut rng, &editable).copied() else {
                continue;
            };
            let clear = !field.required && rng.below(5) == 0;
            let value = if clear {
                Value::Null
            } else {
                synthesise(&mut rng, &state, schema, object, field, &hints, serial, now)
                    .unwrap_or(Value::Null)
            };
            (
                format!(
                    "edit {} of {} {}",
                    field.label.to_lowercase(),
                    object.label.to_lowercase(),
                    record.id
                ),
                actor_for(&mut rng, &state, Some(record)),
                Intent::UpdateRecord {
                    object: object.id.clone(),
                    id: record.id,
                    fields: BTreeMap::from([(field.id.clone(), value)]),
                    expected_version: None,
                },
            )
        } else {
            let record = *pick(&mut rng, &existing).expect("non-empty");
            (
                format!("delete {} {}", object.label.to_lowercase(), record.id),
                actor_for(&mut rng, &state, Some(record)),
                Intent::DeleteRecord {
                    object: object.id.clone(),
                    id: record.id,
                    expected_version: None,
                },
            )
        };

        let context = Context {
            tenant: tenant.clone(),
            principal,
            session: None,
            now: Timestamp(now),
            correlation: format!("sim-{seed}-{serial}"),
        };
        if let Ok((next, _)) = scratch.step(&intent, &context) {
            scratch = next;
        }
        schedule.push(Scheduled {
            label,
            context,
            intent,
        });
    }
    schedule
}

/// The field ids a generated or recorded intent writes, for reports.
pub fn written_fields(intent: &Intent) -> Vec<FieldId> {
    match intent {
        Intent::CreateRecord { fields, .. } | Intent::UpdateRecord { fields, .. } => {
            fields.keys().cloned().collect()
        }
        _ => Vec::new(),
    }
}
