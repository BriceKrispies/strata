//! Structural versions, pins and projections, and the rebase of a change made from an older
//! schema (D-8213..D-8216; MIG-594..MIG-599, MIG-602, MIG-603, MIG-607).
//!
//! The names follow `lean/Strata/Evolution/Model.lean`: a retype with an `encode` and a `decode`
//! is Lean `retype`, [`has_left_inverse`] is `hasLeftInverse`, [`project_record`] is `project`
//! applied to the cells a pinned dependent reads, [`PinBook::pins`] is `Schema.pins`, and
//! [`rebase`] decides `Disjoint` from the footprint and the impact of two changes (Lean `ents`).
//!
//! # What is decided here and what the shell holds
//!
//! The generated [`TenantSchema`] has no place for versions and pins, and growing it means
//! changing the specification and every function generated from it. So the book of versions and
//! pins is a pure value of its own, [`PinBook`]: the kernel takes one and returns the next
//! ([`crate::transition::transition_pinned`]), and the shell stores it beside the schema. A
//! decision with the empty book is the decision of [`crate::transition::transition`]: nobody is
//! pinned, so a breaking change that reaches a dependent is refused.
//!
//! * A structural version rises by one for each cell a breaking change touches, and for no other
//!   change (MIG-594). A cell that is not in the book has version one.
//! * A pin is the versions a dependent was validated against, by cell (MIG-595). The shell pins a
//!   dependent with [`PinBook::pin`]; a pin ends when a change defines the dependent again (it is
//!   migrated), or when the deprecation window of a projection it reads through closes
//!   (MIG-598, [`expired_pins`]).
//! * A breaking change that reaches dependents is accepted when every one of them is pinned at the
//!   version in force and the change has a left inverse on the data: a retype whose `decode`
//!   returns, for every value stored, what `encode` was given. The book then keeps the projection
//!   `decode` (and `encode`) from the new version back to the old one, and a pinned dependent
//!   reads through it ([`project_record`], MIG-596, MIG-597). A removal has no left inverse
//!   (MIG-557), so it waits until no pin on it remains (MIG-599).
//! * Time comes from the context, never from a clock.
//!
//! A projection is applied to the record a dependent is evaluated on. A read that follows a
//! reference into another record reads that record as it is stored.

use crate::dependencies::{
    self, Assessment, Cell, ChangeClass, DependencyGraph, DependentId, Footprint,
};
use crate::expr::{self, Env, Expr};
use crate::schema::TenantSchemaExt;
use crate::state::{Principal, Record, RecordExt};
use crate::types::{
    FieldId, ObjectTypeId, RecordId, RoleId, SchemaChange, TenantSchema, Timestamp, Value, ValueExt,
};
use std::collections::{BTreeMap, BTreeSet};

/// The version of an entity that moves only on a breaking change (D-8213).
pub type StructuralVersion = u64;

/// The way back from one structural version of a cell to the one before it: what a pinned
/// dependent reads through (D-8214).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    /// Turns a value of the earlier shape into the later one (the migration).
    pub encode: Expr,
    /// Turns a value of the later shape back into the earlier one: a left inverse of `encode` on
    /// the data the change migrated.
    pub decode: Expr,
    /// When the deprecation window the change declared closes; none for a window that stays open.
    pub window_ends: Option<Timestamp>,
}

/// The structural versions of the cells of a schema, the pins of its dependents, and the
/// projections between versions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PinBook {
    /// The structural version of each cell that has taken a breaking change; absent means one.
    pub versions: BTreeMap<Cell, StructuralVersion>,
    /// For each pinned dependent, the version of each cell it was validated against (Lean
    /// `Schema.pins`, MIG-595).
    pub pins: BTreeMap<DependentId, BTreeMap<Cell, StructuralVersion>>,
    /// The projection from version `v + 1` of a cell back to version `v`, keyed by `v`.
    pub projections: BTreeMap<(Cell, StructuralVersion), Projection>,
}

impl PinBook {
    /// The structural version of a cell.
    pub fn version(&self, cell: &Cell) -> StructuralVersion {
        self.versions.get(cell).copied().unwrap_or(1)
    }

    /// Pin a dependent to the versions in force of the cells it reads in `schema`. Pinning a
    /// dependent the schema does not hold pins it to nothing.
    pub fn pin(&mut self, dependent: &DependentId, schema: &TenantSchema) {
        let graph = DependencyGraph::of(schema);
        let versions = graph
            .reads
            .get(dependent)
            .map(|reads| {
                reads
                    .cells
                    .iter()
                    .map(|c| (c.clone(), self.version(c)))
                    .collect()
            })
            .unwrap_or_default();
        self.pins.insert(dependent.clone(), versions);
    }

    /// Pin a dependent to given versions of given cells.
    pub fn pin_at(&mut self, dependent: DependentId, versions: BTreeMap<Cell, StructuralVersion>) {
        self.pins.insert(dependent, versions);
    }

    /// Whether the dependent is pinned.
    pub fn is_pinned(&self, dependent: &DependentId) -> bool {
        self.pins.contains_key(dependent)
    }

    /// The cells a pinned dependent reads at an earlier version than the one in force.
    pub fn behind(
        &self,
        dependent: &DependentId,
    ) -> BTreeMap<Cell, (StructuralVersion, StructuralVersion)> {
        self.pins
            .get(dependent)
            .into_iter()
            .flatten()
            .filter_map(|(cell, pinned)| {
                let now = self.version(cell);
                (*pinned < now).then(|| (cell.clone(), (*pinned, now)))
            })
            .collect()
    }
}

/// Whether a change has a left inverse on the data its dependents read (Lean `hasLeftInverse`):
/// the old shape can be rebuilt from the new data. A removal, a narrowing and clearing a
/// reference have none (MIG-557). A rename and a move have one by construction (their dependents are
/// rewritten, not projected); a retype has one only where [`check_left_inverse`] holds on the data.
pub fn has_left_inverse(change: &SchemaChange) -> bool {
    matches!(
        change,
        SchemaChange::RetypeField { .. }
            | SchemaChange::RenameField { .. }
            | SchemaChange::MoveField { .. }
    )
}

/// Evaluate an expression of a retype on one value: the value is the field `value` of the record
/// the expression is read on.
pub fn recode(expr: &Expr, value: &Value, now: Timestamp) -> Result<Value, String> {
    let name = FieldId::new("value").map_err(|e| e.to_string())?;
    let object = ObjectTypeId::new("recode").map_err(|e| e.to_string())?;
    let record = Record {
        id: RecordId(0),
        object,
        version: 1,
        created_by: None,
        created_at: now,
        updated_at: now,
        fields: BTreeMap::from([(name, value.clone())]),
    };
    let params = BTreeMap::new();
    let roles: BTreeSet<RoleId> = BTreeSet::new();
    let env = Env {
        record: Some(&record),
        old: None,
        new: None,
        params: &params,
        now,
        principal: &Principal::System,
        roles: &roles,
        lookup: None,
    };
    expr::eval(expr, &env).map_err(|e| e.to_string())
}

/// Whether `decode` undoes `encode` on every value given: `decode (encode v) = v` (Lean
/// `hasLeftInverse` of a retype, decided on the data that exists). Null is not re-encoded. The
/// refusal names the first value it fails on.
pub fn check_left_inverse<'v>(
    encode: &Expr,
    decode: &Expr,
    values: impl IntoIterator<Item = &'v Value>,
    now: Timestamp,
) -> Result<(), String> {
    for value in values {
        if value.is_null() {
            continue;
        }
        let encoded =
            recode(encode, value, now).map_err(|e| format!("encode of {value:?}: {e}"))?;
        let back =
            recode(decode, &encoded, now).map_err(|e| format!("decode of {encoded:?}: {e}"))?;
        if &back != value {
            return Err(format!(
                "decode does not undo encode: {value:?} encodes to {encoded:?} and decodes to {back:?}"
            ));
        }
    }
    Ok(())
}

/// The record a pinned dependent is evaluated on: every cell of the record that is at a later
/// version than the one the dependent was validated against is projected back, one version at a
/// time (Lean `project`). A dependent that is not pinned reads the record as it is. After the
/// deprecation window of a projection it needs has closed, the dependent is refused (MIG-598).
pub fn project_record(
    book: &PinBook,
    dependent: &DependentId,
    record: &Record,
    now: Timestamp,
) -> Result<Record, String> {
    let mut out = record.clone();
    for ((object, field), (pinned, current)) in book.behind(dependent) {
        if object != record.object {
            continue;
        }
        let cell = (object, field.clone());
        let mut value = record.get(&field).clone();
        for version in (pinned..current).rev() {
            let projection = book
                .projections
                .get(&(cell.clone(), version))
                .ok_or_else(|| {
                    format!(
                        "{dependent}: no projection of {}.{} to version {version}",
                        cell.0, cell.1
                    )
                })?;
            if projection.window_ends.is_some_and(|end| end <= now) {
                return Err(format!(
                    "{dependent}: the deprecation window of {}.{} closed; the dependent is refused (MIG-598)",
                    cell.0, cell.1
                ));
            }
            value = recode(&projection.decode, &value, now)?;
        }
        if !value.is_null() {
            out.fields.insert(field, value);
        }
    }
    Ok(out)
}

/// The pins that are held when the deprecation window of the projection they read through has
/// closed: reported, and refused when read (MIG-598).
pub fn expired_pins(book: &PinBook, now: Timestamp) -> Vec<(DependentId, Cell)> {
    let mut out = Vec::new();
    for dependent in book.pins.keys() {
        for (cell, (pinned, current)) in book.behind(dependent) {
            let closed = (pinned..current).any(|version| {
                book.projections
                    .get(&(cell.clone(), version))
                    .and_then(|p| p.window_ends)
                    .is_some_and(|end| end <= now)
            });
            if closed {
                out.push((dependent.clone(), cell));
            }
        }
    }
    out
}

/// A switch of a pinned dependent back to an earlier version of a cell (MIG-607): refused when a
/// value stored now cannot be read through that version without loss, that is when projecting it
/// back and encoding it forward again does not return it.
pub fn switch_back<'v>(
    book: &mut PinBook,
    dependent: &DependentId,
    cell: &Cell,
    to_version: StructuralVersion,
    stored: impl IntoIterator<Item = &'v Value> + Clone,
    now: Timestamp,
) -> Result<(), String> {
    let current = book.version(cell);
    if to_version >= current {
        return Err(format!(
            "{}.{} is at version {current}; nothing to switch back to",
            cell.0, cell.1
        ));
    }
    for value in stored {
        if value.is_null() {
            continue;
        }
        let mut back = value.clone();
        for version in (to_version..current).rev() {
            let projection = book
                .projections
                .get(&(cell.clone(), version))
                .ok_or_else(|| {
                    format!(
                        "no projection of {}.{} to version {version}",
                        cell.0, cell.1
                    )
                })?;
            back = recode(&projection.decode, &back, now)?;
        }
        let mut forward = back.clone();
        for version in to_version..current {
            let projection = &book.projections[&(cell.clone(), version)];
            forward = recode(&projection.encode, &forward, now)?;
        }
        if &forward != value {
            return Err(format!(
                "{dependent}: {value:?} reads as {back:?} in version {to_version} of {}.{}, which is not it; the switch back would lose data (MIG-607)",
                cell.0, cell.1
            ));
        }
    }
    book.pins
        .entry(dependent.clone())
        .or_default()
        .insert(cell.clone(), to_version);
    Ok(())
}

/// The verdict on a breaking change that reaches dependents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Every dependent it reaches is pinned at the version in force and the change has a left
    /// inverse: it is accepted, and the dependents read through the projection.
    Pinned,
    /// Refused, with the reason.
    Refused(String),
}

/// Decide a breaking change that reaches `assessment.impact`, against the book (Lean `accepted`):
/// every dependent reached is pinned at the versions in force for the cells the change touches,
/// its window has not closed, and the change has a left inverse. A removal is never accepted
/// here; it waits for its pins (MIG-599).
pub fn decide_breaking(
    change: &SchemaChange,
    assessment: &Assessment,
    book: &PinBook,
    now: Timestamp,
) -> Verdict {
    let impact: Vec<&DependentId> = assessment.impact.iter().collect();
    let unpinned: Vec<String> = impact
        .iter()
        .filter(|d| {
            let Some(pin) = book.pins.get(**d) else {
                return true;
            };
            // Pinned at an earlier version than the one in force: the pin does not protect from
            // the change that is being made now.
            assessment
                .footprint
                .cells
                .iter()
                .filter(|c| pin.contains_key(*c))
                .any(|c| pin[c] != book.version(c))
        })
        .map(|d| d.to_string())
        .collect();
    let expired: Vec<String> = expired_pins(book, now)
        .into_iter()
        .filter(|(d, _)| assessment.impact.contains(d))
        .map(|(d, _)| d.to_string())
        .collect();
    if !expired.is_empty() {
        return Verdict::Refused(format!(
            "{}: the pin of {} is held past its deprecation window (MIG-598)",
            assessment.description,
            expired.join(", ")
        ));
    }
    if !unpinned.is_empty() {
        return Verdict::Refused(format!(
            "{} is a breaking change that reaches {}",
            assessment.description,
            unpinned.join(", ")
        ));
    }
    if !matches!(change, SchemaChange::RetypeField { .. }) {
        return Verdict::Refused(format!(
            "{} has no left inverse (MIG-557), so it is not projected and waits until the pins of {} end (MIG-599)",
            assessment.description,
            impact.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", ")
        ));
    }
    Verdict::Pinned
}

/// The book after a change list is accepted, given its assessments and the schema after it: the
/// structural version of each cell a breaking change touched rises by one (MIG-594) and, for a
/// retype, the projection back is kept; a pin on a dependent the changes define again ends (it is
/// migrated, MIG-595).
pub fn advance(book: &PinBook, changes: &[SchemaChange], assessments: &[Assessment]) -> PinBook {
    let mut out = book.clone();
    for (change, assessment) in changes.iter().zip(assessments) {
        if assessment.class == ChangeClass::Breaking {
            for cell in &assessment.footprint.cells {
                let version = out.version(cell);
                if let SchemaChange::RetypeField {
                    encode,
                    decode,
                    deprecation_ends,
                    ..
                } = change
                {
                    out.projections.insert(
                        (cell.clone(), version),
                        Projection {
                            encode: encode.clone(),
                            decode: decode.clone(),
                            window_ends: *deprecation_ends,
                        },
                    );
                }
                out.versions.insert(cell.clone(), version + 1);
            }
        }
        for entity in &assessment.footprint.entities {
            out.pins.remove(entity);
        }
    }
    out
}

/// What a change list touches: the cells it redefines, removes or creates, the object types it
/// removes, the dependents it defines or removes, and the dependents it reaches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Touch {
    /// Cells touched, created or reached.
    pub cells: BTreeSet<Cell>,
    /// Object types removed.
    pub objects: BTreeSet<ObjectTypeId>,
    /// Dependents defined, removed or reached.
    pub dependents: BTreeSet<DependentId>,
    /// Cells read by the dependents the changes define: a new reader of a cell that another change
    /// touches overlaps it, though neither reaches the other's dependents.
    pub reads: BTreeSet<Cell>,
}

impl Touch {
    fn of(assessments: &[Assessment], after: &DependencyGraph) -> Touch {
        let mut out = Touch::default();
        for a in assessments {
            for entity in &a.footprint.entities {
                if let Some(reads) = after.reads.get(entity) {
                    out.reads.extend(reads.cells.iter().cloned());
                }
            }
            out.cells.extend(a.footprint.cells.iter().cloned());
            out.cells.extend(a.footprint.created.iter().cloned());
            out.objects.extend(a.footprint.objects.iter().cloned());
            out.dependents.extend(a.footprint.entities.iter().cloned());
            out.dependents.extend(a.impact.iter().cloned());
        }
        out
    }

    /// Whether the two have anything in common (Lean `¬ Disjoint`).
    pub fn overlaps(&self, other: &Touch) -> bool {
        self.cells.intersection(&other.cells).next().is_some()
            || self.cells.intersection(&other.reads).next().is_some()
            || self.reads.intersection(&other.cells).next().is_some()
            || self.objects.intersection(&other.objects).next().is_some()
            || self.dependents.intersection(&other.dependents).next().is_some()
            // A removed object type takes every cell of it.
            || self
                .objects
                .iter()
                .any(|o| other.cells.iter().any(|(c, _)| c == o))
            || other
                .objects
                .iter()
                .any(|o| self.cells.iter().any(|(c, _)| c == o))
    }
}

/// The outcome of rebasing a change made from an older revision onto the schema now in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rebase {
    /// Its footprint and impact are disjoint from the changes committed since its base: the two
    /// commute (MIG-602), and the change is decided on the current schema with the class and the
    /// impact it had at its base. `assessments` is that decision, made against the current schema.
    Commutes {
        /// The class and impact of each change, against the schema now in force.
        assessments: Vec<Assessment>,
    },
    /// The footprints overlap: the change is classified again against the schema now in force
    /// (MIG-603), and never applied with the class it had at its base. `at_base` and `now` show
    /// where they differ.
    Reclassified {
        /// The class and impact of each change at its base revision.
        at_base: Vec<Assessment>,
        /// The class and impact of each change against the schema now in force; empty when a
        /// change no longer applies to it.
        now: Vec<Assessment>,
        /// Why the change no longer applies to the schema now in force, if it does not.
        refused: Option<String>,
    },
}

/// Apply a list of changes in order to a copy of a schema.
fn apply_all(schema: &TenantSchema, changes: &[SchemaChange]) -> Result<TenantSchema, String> {
    let mut working = schema.clone();
    for change in changes {
        crate::transition::apply_change(&mut working, change)?;
    }
    Ok(working)
}

/// The assessments of a change list on a schema, and the dependency graph after it. Fails where a
/// change does not apply, or where the schema after the list is not consistent (the first problem
/// is the reason, as the kernel would refuse it).
fn assess_on(
    schema: &TenantSchema,
    changes: &[SchemaChange],
) -> Result<(Vec<Assessment>, DependencyGraph), String> {
    let after = apply_all(schema, changes)?;
    if let Some(problem) = after.problems().into_iter().next() {
        return Err(problem);
    }
    let assessments = crate::transition::assess_changes(schema, changes, &after);
    if assessments.len() != changes.len() {
        return Err("a change does not apply".to_string());
    }
    Ok((assessments, DependencyGraph::of(&after)))
}

/// Rebase `changes`, made from `base`, onto `current`, the schema after the `committed` changes
/// that came in since (D-8216, MIG-602, MIG-603). Pure: the shell that retries a change refused as
/// stale (`SchemaConflict`) gives the kernel the schema it was based on and the changes committed
/// since, and submits the change again at the current revision with what this says.
pub fn rebase(
    base: &TenantSchema,
    committed: &[SchemaChange],
    current: &TenantSchema,
    changes: &[SchemaChange],
) -> Rebase {
    let incoming = assess_on(base, changes);
    let since = assess_on(base, committed);
    let now = assess_on(current, changes);
    let disjoint = match (&incoming, &since) {
        (Ok((a, ga)), Ok((b, gb))) => !Touch::of(a, ga).overlaps(&Touch::of(b, gb)),
        // Either list does not apply to its base: nothing is known to commute.
        _ => false,
    };
    let now = now.map(|(assessments, _)| assessments);
    match (disjoint, now) {
        (true, Ok(assessments)) => Rebase::Commutes { assessments },
        (_, now) => Rebase::Reclassified {
            at_base: incoming.map(|(a, _)| a).unwrap_or_default(),
            refused: now.as_ref().err().cloned(),
            now: now.unwrap_or_default(),
        },
    }
}

/// The footprint of every change of a list as it is applied to `schema`, for a caller that wants
/// the footprints without the impact.
pub fn footprints(schema: &TenantSchema, changes: &[SchemaChange]) -> Vec<Footprint> {
    let mut working = schema.clone();
    let mut out = Vec::new();
    for change in changes {
        out.push(dependencies::footprint(change, &working));
        if crate::transition::apply_change(&mut working, change).is_err() {
            break;
        }
    }
    out
}
