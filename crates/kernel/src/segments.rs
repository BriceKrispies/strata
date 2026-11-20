//! Segments: a named audience of people defined by a predicate in the tenant rule language over
//! the record that stands for a person, with its membership kept current by the commits that
//! change it (SEG-001 to SEG-038, SEG-1540 to SEG-1550, D-5020 to D-5033, D-6555, D-6556).
//!
//! Everything here is pure. The membership tables live in a storage block of their own beside
//! the tenant state (the segment, membership, wake, batch and erased rows of a tenant), so a
//! decision is given the rows it needs and returns a [`SegmentWrite`] that the store applies in
//! the transaction of the commit, guarded by what the decision saw: the version of every segment
//! it evaluated and the membership row of every person it decided. A store that finds another
//! row refuses the write as a conflict, and the shell decides again.
//!
//! The pieces, in the order a commit uses them:
//!
//! - [`define`] and [`remove`] decide a definition (SEG-001 to SEG-007, SEG-017, SEG-029,
//!   SEG-1546 to SEG-1548). [`analyze`] derives, at definition time, every object type and field
//!   a predicate can read, which is what lets a commit find the people a change affects without a
//!   scan.
//! - [`plan_change`] names, from the events of an accepted transition, the people whose subject
//!   record changed and the referenced records whose referrers must be found through the record
//!   index (`Need::ByField`); [`targets`] bounds them to [`BATCH`] persons a segment and leaves
//!   the rest to a [`Batch`] continuation (SEG-028).
//! - [`needs_for`] plans the records the decision loads: the subject record of each person and the
//!   records its predicate reaches, nothing else (SEG-027, CT-295). The shell loads them and asks
//!   again until nothing is missing; a predicate follows at most one reference, so that is at most
//!   two rounds.
//! - [`decide_targets`] evaluates and writes the entry and exit events and the membership delta
//!   (SEG-008 to SEG-010, SEG-030, SEG-031, SEG-037, CT-296), and the wake rows of an indexable
//!   time-dependent predicate (SEG-1545).
//! - [`batch_load`] and [`batch_targets`] run one continuation (CT-297); [`start_refresh`] and
//!   [`skip_refresh`] are what a refresh job does (SEG-018 to SEG-021, SEG-1544).
//! - [`erase`] removes a person from every segment (SEG-036 to SEG-038).
//! - [`members_page`], [`count`], [`my_segments`], [`test_predicate`] and [`preview`] are the reads.
//!
//! The rule language follows at most one reference (`record.<ref>.<field>`), so the bound of two
//! reference hops (SEG-002, SEG-004) is never reached by a predicate that type-checks; the hop
//! count is still checked, so that a language that grows deeper references keeps the bound.

use crate::definitions::expr_nodes;
use crate::expr::{self, Env, Shape, TypeEnv};
use crate::intent::{Check, CheckExt, Context, DomainError, DomainErrorExt, ErrorKind};
use crate::read::visible_in;
use crate::rules::{holds_any, holds_power, insufficient, roles_of, Lookup};
use crate::schema::{Action, FieldType, Grant, Resource, TenantSchema};
use crate::state::{Coverage, CoverageExt, Need, Principal, Record, RecordExt, State, TenantState};
use crate::types::{
    ArithOp, DomainEvent, EventBody, Expr, FieldId, FieldTypeExt, ObjectTypeId, PrincipalId,
    RecordId, RoleId, SchemaVersion, Scope, StateRevision, TenantId, Timestamp, Value,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
pub use strata_model_core::types::{
    SegmentCount, SegmentDecl, SegmentDirection, SegmentId, SegmentKey, SegmentMember,
    SegmentMembershipEvent, SegmentPreview, SegmentReason, SegmentSummary, SegmentTestResult,
};

/// The capability that defining, changing, removing, testing and previewing a segment needs
/// (SEG-007).
pub const SEGMENTS_DEFINE: &str = "segments-define";
/// The capability that listing and counting the members of a segment needs (SEG-033).
pub const SEGMENTS_READ: &str = "segments-read";
/// The capability that listing one's own visible segments needs; every signed-in person holds it
/// (SEG-1540, D-6555).
pub const SEGMENTS_READ_OWN: &str = "segments-read-own";

/// The most persons one transaction evaluates for one segment (SEG-028, D-5026).
pub const BATCH: usize = 100;
/// The most segments a tenant holds (SEG-029).
pub const MAX_SEGMENTS: usize = 100;
/// The most segments whose predicates read any one object type (SEG-029).
pub const MAX_SEGMENTS_PER_OBJECT: usize = 25;
/// The most time-dependent segments that are not indexable (SEG-1547).
pub const MAX_FULL_REFRESH_SEGMENTS: usize = 5;
/// The most nodes of a predicate (SEG-004).
pub const MAX_PREDICATE_NODES: usize = 200;
/// The most reference fields a predicate may follow from the subject record (SEG-004).
pub const MAX_HOPS: usize = 2;
/// The shortest and longest refresh interval of a time-dependent predicate (SEG-017).
pub const MIN_REFRESH_SECONDS: u32 = 60;
pub const MAX_REFRESH_SECONDS: u32 = 86_400;
/// The shortest refresh interval of a time-dependent predicate that is not indexable (SEG-1546).
pub const MIN_FULL_REFRESH_SECONDS: u32 = 900;
/// The most memberships a count examines when the kept count cannot answer (SEG-1541).
pub const COUNT_SCAN_BOUND: usize = 10_000;
/// The most persons one preview evaluates (SEG-1542).
pub const MAX_PREVIEW: u32 = 1_000;
/// The most members one page of `list-segment-members` names.
pub const MAX_MEMBERS_PAGE: u32 = 100;
/// The page of `list-segment-members` when the caller names none.
pub const DEFAULT_MEMBERS_PAGE: u32 = 50;
/// The longest segment name, in characters.
pub const MAX_NAME_CHARS: usize = 120;

// ---------------------------------------------------------------------------------------------
// The rows of the storage block
// ---------------------------------------------------------------------------------------------

/// A segment as the tenant holds it (record `rec-segment`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub id: SegmentId,
    pub name: String,
    /// The object type whose records stand for people.
    pub subject: ObjectTypeId,
    pub predicate: Expr,
    /// Raised by every change of the predicate (D-5027).
    pub version: u64,
    /// Whether the predicate reads the time (D-5024).
    pub time_dependent: bool,
    /// Whether the time-dependent predicate is refreshed through wake rows (D-6555).
    pub indexable: bool,
    #[serde(default)]
    pub refresh_seconds: Option<u32>,
    /// The principal that defined it, whose read permissions bound what the predicate names
    /// (SEG-005, D-5028).
    #[serde(default)]
    pub defined_by: Option<PrincipalId>,
    /// The required unique string field of the subject type that holds the principal (SEG-1548).
    pub principal_field: FieldId,
    /// Whether its members may list it (SEG-1540); false unless the definer says so.
    #[serde(default)]
    pub visible_to_members: bool,
    /// The schema version the definition was checked against (SEG-001).
    pub schema_version: SchemaVersion,
    /// The number of memberships, kept by the store as it applies membership rows (SEG-1541).
    #[serde(default)]
    pub member_count: u64,
    /// The time of the last refresh skipped for want of job quota (SEG-1544).
    #[serde(default)]
    pub refresh_skipped_at: Option<Timestamp>,
}

/// One membership (record `rec-segment-membership`): the segment, the person, the version that
/// decided it and the time of entry, and no field of the person (SEG-034).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Membership {
    pub segment: SegmentId,
    pub person: PrincipalId,
    pub version: u64,
    pub since: Timestamp,
}

/// When a person must next be evaluated for an indexable time-dependent segment (record
/// `rec-segment-wake`, SEG-1545).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Wake {
    pub segment: SegmentId,
    pub person: PrincipalId,
    pub at: Timestamp,
}

/// Which persons a continuation evaluates.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BatchScope {
    /// Every person of the subject type, in record id order: a changed predicate (SEG-022), a
    /// full refresh (SEG-019), and a change that touched more subject records than one
    /// transaction evaluates.
    All,
    /// The subject records whose reference field holds a record whose fields the predicate
    /// reads, in record id order: the remainder of a change to that record (SEG-028).
    Referencing { field: FieldId, record: RecordId },
    /// The persons whose wake time has passed (SEG-1545).
    Due,
}

impl BatchScope {
    /// The key of the continuation within its segment: one continuation a scope.
    pub fn key(&self) -> String {
        match self {
            BatchScope::All => "all".to_string(),
            BatchScope::Referencing { field, record } => format!("ref:{field}:{}", record.0),
            BatchScope::Due => "due".to_string(),
        }
    }
}

/// A continuation of a segment (record `rec-segment-batch`): the version it evaluates under and
/// the position it reached. It is the hand-off to the job worker, which runs it with
/// [`batch_load`] and [`batch_targets`] in a transaction of its own (CT-297).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Batch {
    pub segment: SegmentId,
    pub scope: BatchScope,
    pub version: u64,
    /// The last subject record evaluated; the next batch starts after it.
    #[serde(default)]
    pub after: Option<RecordId>,
    pub reason: SegmentReason,
    /// The hop count of the change that caused it (SEG-031).
    #[serde(default)]
    pub hop: u64,
    /// When it was created or last advanced.
    pub at: Timestamp,
}

/// One membership row a decision changes, with the row it saw. The store refuses the write as a
/// conflict when the stored row is not `before`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipChange {
    pub segment: SegmentId,
    pub person: PrincipalId,
    pub before: Option<Membership>,
    pub after: Option<Membership>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeChange {
    Put(Wake),
    Remove {
        segment: SegmentId,
        person: PrincipalId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchChange {
    Put(Batch),
    Remove { segment: SegmentId, key: String },
}

/// The rows of one person in one segment that a decision rested on, changed or not. The store
/// refuses the write as a conflict when either is no longer what the decision saw, so a decision
/// never writes over, or keeps, a membership decided by a commit it did not see.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub segment: SegmentId,
    pub person: PrincipalId,
    pub membership: Option<Membership>,
    pub wake: Option<Timestamp>,
}

/// An entry or exit event as the segment block keeps it: the body of D-5023 with who and when.
///
/// The events are kept in the segment block's own ordered log, written in the transaction of
/// the change, and not in the kernel's event sequence: segment definitions and batches are not
/// transitions of the journal, so an event in the kernel's sequence would make a replay of the
/// journal disagree with the live state (K-11). The log has the order and the published flag of
/// an outbox, and the job worker reads it as the outbox of entry and exit events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentEvent {
    pub tenant: TenantId,
    pub at: Timestamp,
    pub principal: Principal,
    pub correlation: String,
    pub event: SegmentMembershipEvent,
}

/// Everything a segment decision writes to the storage block, applied by the store in one
/// transaction with the commit it belongs to, or alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentWrite {
    /// The version of each segment the decision rested on (`None`: it did not exist). The store
    /// refuses the write as a conflict when a segment is at another version.
    #[serde(default)]
    pub expect: BTreeMap<SegmentId, Option<u64>>,
    /// The membership and wake rows of every pair the decision evaluated.
    #[serde(default)]
    pub seen: Vec<Seen>,
    /// The entry and exit events, in the order they were decided.
    #[serde(default)]
    pub events: Vec<SegmentEvent>,
    #[serde(default)]
    pub put_segments: Vec<Segment>,
    /// Removed with every membership, wake and batch row of theirs (SEG-026).
    #[serde(default)]
    pub remove_segments: Vec<SegmentId>,
    #[serde(default)]
    pub memberships: Vec<MembershipChange>,
    #[serde(default)]
    pub wakes: Vec<WakeChange>,
    /// Segments whose pending batches are all dropped before `batches` is applied: a new version
    /// supersedes every continuation of the old one (SEG-025).
    #[serde(default)]
    pub clear_batches: Vec<SegmentId>,
    #[serde(default)]
    pub batches: Vec<BatchChange>,
    /// Persons erased: no later decision enters them (SEG-037).
    #[serde(default)]
    pub erased: Vec<PrincipalId>,
    /// Refreshes skipped for want of job quota (SEG-1544).
    #[serde(default)]
    pub refresh_skipped: Vec<(SegmentId, Timestamp)>,
}

impl SegmentWrite {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
            && self.put_segments.is_empty()
            && self.remove_segments.is_empty()
            && self.memberships.is_empty()
            && self.wakes.is_empty()
            && self.clear_batches.is_empty()
            && self.batches.is_empty()
            && self.erased.is_empty()
            && self.refresh_skipped.is_empty()
    }

    /// Both writes, as one.
    pub fn merge(&mut self, other: SegmentWrite) {
        for (id, version) in other.expect {
            self.expect.entry(id).or_insert(version);
        }
        self.seen.extend(other.seen);
        self.events.extend(other.events);
        self.put_segments.extend(other.put_segments);
        self.remove_segments.extend(other.remove_segments);
        self.memberships.extend(other.memberships);
        self.wakes.extend(other.wakes);
        self.clear_batches.extend(other.clear_batches);
        self.batches.extend(other.batches);
        self.erased.extend(other.erased);
        self.refresh_skipped.extend(other.refresh_skipped);
    }
}

/// The rows a decision was given for the persons it decides: their memberships and wake rows in
/// the segments concerned, and which of them are erased. Only the pairs in `covered` were loaded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rows {
    pub covered: BTreeSet<(SegmentId, PrincipalId)>,
    pub memberships: BTreeMap<(SegmentId, PrincipalId), Membership>,
    pub wakes: BTreeMap<(SegmentId, PrincipalId), Timestamp>,
    pub erased: BTreeSet<PrincipalId>,
}

/// The whole storage block of one tenant, as the in-memory store and tests hold it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SegmentTables {
    pub segments: BTreeMap<SegmentId, Segment>,
    pub memberships: BTreeMap<(SegmentId, PrincipalId), Membership>,
    pub wakes: BTreeMap<(SegmentId, PrincipalId), Timestamp>,
    pub batches: BTreeMap<(SegmentId, String), Batch>,
    pub erased: BTreeSet<PrincipalId>,
    /// The entry and exit events, in order; the position of an event is its index plus one.
    pub events: Vec<SegmentEvent>,
    /// How many events of the log have been handed on.
    pub published: u64,
}

/// Why a store refused a write: what it expected and what it found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteConflict(pub String);

impl SegmentTables {
    /// The segments, in id order.
    pub fn catalog(&self) -> Vec<Segment> {
        self.segments.values().cloned().collect()
    }

    /// The rows of `pairs`.
    pub fn rows(&self, pairs: &[(SegmentId, PrincipalId)]) -> Rows {
        let mut rows = Rows::default();
        for pair in pairs {
            rows.covered.insert(pair.clone());
            if let Some(m) = self.memberships.get(pair) {
                rows.memberships.insert(pair.clone(), m.clone());
            }
            if let Some(at) = self.wakes.get(pair) {
                rows.wakes.insert(pair.clone(), *at);
            }
            if self.erased.contains(&pair.1) {
                rows.erased.insert(pair.1.clone());
            }
        }
        rows
    }

    /// Check that `write` rests on these tables, without changing them.
    pub fn check(&self, write: &SegmentWrite) -> Result<(), WriteConflict> {
        for (id, expected) in &write.expect {
            let found = self.segments.get(id).map(|s| s.version);
            if found != *expected {
                return Err(WriteConflict(format!(
                    "segment {id} is at version {found:?}, the decision saw {expected:?}"
                )));
            }
        }
        for change in &write.memberships {
            let key = (change.segment, change.person.clone());
            let found = self.memberships.get(&key);
            if found != change.before.as_ref() {
                return Err(WriteConflict(format!(
                    "the membership of {} in {} changed since it was read",
                    change.person, change.segment
                )));
            }
        }
        for seen in &write.seen {
            let key = (seen.segment, seen.person.clone());
            if self.memberships.get(&key) != seen.membership.as_ref()
                || self.wakes.get(&key).copied() != seen.wake
            {
                return Err(WriteConflict(format!(
                    "the rows of {} in {} changed since they were read",
                    seen.person, seen.segment
                )));
            }
        }
        Ok(())
    }

    /// Apply a write the way every store applies it: refuse it whole on a conflict, otherwise
    /// apply every row and keep each segment's member count.
    pub fn apply(&mut self, write: &SegmentWrite) -> Result<(), WriteConflict> {
        self.check(write)?;
        for segment in &write.put_segments {
            let count = self.segments.get(&segment.id).map_or(0, |s| s.member_count);
            let mut kept = segment.clone();
            kept.member_count = count;
            self.segments.insert(segment.id, kept);
        }
        for id in &write.remove_segments {
            self.segments.remove(id);
            self.memberships.retain(|(segment, _), _| segment != id);
            self.wakes.retain(|(segment, _), _| segment != id);
            self.batches.retain(|(segment, _), _| segment != id);
        }
        for change in &write.memberships {
            let key = (change.segment, change.person.clone());
            let delta: i64 = match (&change.before, &change.after) {
                (None, Some(_)) => 1,
                (Some(_), None) => -1,
                _ => 0,
            };
            match &change.after {
                Some(m) => {
                    self.memberships.insert(key, m.clone());
                }
                None => {
                    self.memberships.remove(&key);
                }
            }
            if let Some(segment) = self.segments.get_mut(&change.segment) {
                segment.member_count = (segment.member_count as i64 + delta).max(0) as u64;
            }
        }
        for change in &write.wakes {
            match change {
                WakeChange::Put(w) => {
                    self.wakes.insert((w.segment, w.person.clone()), w.at);
                }
                WakeChange::Remove { segment, person } => {
                    self.wakes.remove(&(*segment, person.clone()));
                }
            }
        }
        for id in &write.clear_batches {
            self.batches.retain(|(segment, _), _| segment != id);
        }
        for change in &write.batches {
            match change {
                BatchChange::Put(b) => {
                    if self.segments.contains_key(&b.segment) {
                        self.batches.insert((b.segment, b.scope.key()), b.clone());
                    }
                }
                BatchChange::Remove { segment, key } => {
                    self.batches.remove(&(*segment, key.clone()));
                }
            }
        }
        for person in &write.erased {
            self.erased.insert(person.clone());
        }
        for (id, at) in &write.refresh_skipped {
            if let Some(segment) = self.segments.get_mut(id) {
                segment.refresh_skipped_at = Some(*at);
            }
        }
        self.events.extend(write.events.iter().cloned());
        Ok(())
    }

    /// The events after position `after`, with their positions.
    pub fn events_after(&self, after: u64, limit: usize) -> Vec<(u64, SegmentEvent)> {
        self.events
            .iter()
            .enumerate()
            .map(|(index, event)| (index as u64 + 1, event))
            .filter(|(position, _)| *position > after)
            .take(limit)
            .map(|(position, event)| (position, event.clone()))
            .collect()
    }

    /// The memberships of one segment after `after`, in principal order.
    pub fn members_after(
        &self,
        segment: SegmentId,
        after: Option<&PrincipalId>,
        limit: usize,
    ) -> Vec<Membership> {
        self.memberships
            .iter()
            .filter(|((s, p), _)| *s == segment && after.is_none_or(|a| p > a))
            .take(limit)
            .map(|(_, m)| m.clone())
            .collect()
    }

    /// Every membership of one person.
    pub fn memberships_of(&self, person: &PrincipalId) -> Vec<Membership> {
        self.memberships
            .values()
            .filter(|m| &m.person == person)
            .cloned()
            .collect()
    }

    /// The wake rows of one segment due at `at`, earliest first.
    pub fn due(&self, segment: SegmentId, at: Timestamp, limit: usize) -> Vec<Wake> {
        let mut due: Vec<Wake> = self
            .wakes
            .iter()
            .filter(|((s, _), when)| *s == segment && **when <= at)
            .map(|((s, p), when)| Wake {
                segment: *s,
                person: p.clone(),
                at: *when,
            })
            .collect();
        due.sort_by(|a, b| (a.at, &a.person).cmp(&(b.at, &b.person)));
        due.truncate(limit);
        due
    }
}

// ---------------------------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------------------------

/// The segment-id of a segment a bundle declares: derived from the tenant and the key, so an
/// install gives the same id every time (D-6582). A name-based identifier of the SHA-256 of both,
/// with the version bits of version 8 and the variant bits of RFC 9562.
pub fn derive_segment_id(tenant: &TenantId, key: &SegmentKey) -> SegmentId {
    let mut hasher = Sha256::new();
    hasher.update(b"strata-segment\0");
    hasher.update(tenant.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(key.0.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    segment_id_of(bytes)
}

/// A segment-id from its sixteen bytes. The kernel has no uuid dependency of its own, so the id
/// is written in its text form and decoded as the generated type decodes it.
pub fn segment_id_of(bytes: [u8; 16]) -> SegmentId {
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let text = format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    );
    serde_json::from_value(serde_json::Value::String(text))
        .expect("a hyphenated hex text of sixteen bytes is a uuid")
}

// ---------------------------------------------------------------------------------------------
// What a predicate reads
// ---------------------------------------------------------------------------------------------

/// What a predicate reads, derived when the segment is defined (D-5021, D-5025).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reads {
    /// The fields of the subject record it names.
    pub subject_fields: BTreeSet<FieldId>,
    /// Whether it reads a system attribute (`$id`, `$version`, `$updated_at`, ...) of the
    /// subject record, which every write of the record changes.
    pub system_attributes: bool,
    /// The reference fields of the subject record it follows, each with the object type it
    /// points at and the fields read there.
    pub hops: BTreeMap<FieldId, (ObjectTypeId, BTreeSet<FieldId>)>,
    /// Whether it reads the time.
    pub time: bool,
    /// Whether it reads the principal or the roles of the person.
    pub roles: bool,
    pub nodes: usize,
}

impl Reads {
    /// Every object type it reads: the subject type and the types its references reach.
    pub fn objects(&self, subject: &ObjectTypeId) -> BTreeSet<ObjectTypeId> {
        let mut out = BTreeSet::from([subject.clone()]);
        out.extend(self.hops.values().map(|(object, _)| object.clone()));
        out
    }

    /// Every (object type, field) it reads, as `object.field`.
    pub fn field_names(&self, subject: &ObjectTypeId) -> Vec<String> {
        let mut out: Vec<String> = self
            .subject_fields
            .iter()
            .map(|f| format!("{subject}.{f}"))
            .collect();
        for (reference, (object, fields)) in &self.hops {
            if !self.subject_fields.contains(reference) {
                out.push(format!("{subject}.{reference}"));
            }
            out.extend(fields.iter().map(|f| format!("{object}.{f}")));
        }
        out.sort();
        out.dedup();
        out
    }
}

/// What [`analyze`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Analysis {
    pub reads: Reads,
    pub indexable: bool,
}

fn reads_now(expr: &Expr) -> bool {
    match expr {
        Expr::Now => true,
        Expr::Lit(_)
        | Expr::Var(..)
        | Expr::Through(..)
        | Expr::Param(_)
        | Expr::Principal
        | Expr::HasRole(_) => false,
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => reads_now(a),
        Expr::And(items) | Expr::Or(items) => items.iter().any(reads_now),
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => reads_now(a) || reads_now(b),
        Expr::If(a, b, c) => reads_now(a) || reads_now(b) || reads_now(c),
    }
}

/// Whether an expression is `now` moved by an amount that does not read the time: `now`,
/// `now + x`, `x + now` or `now - x`.
fn affine(expr: &Expr) -> bool {
    match expr {
        Expr::Now => true,
        Expr::Arith(ArithOp::Add, a, b) => {
            (affine(a) && !reads_now(b)) || (affine(b) && !reads_now(a))
        }
        Expr::Arith(ArithOp::Sub, a, b) => affine(a) && !reads_now(b),
        _ => false,
    }
}

/// Whether every read of the time is one side of a comparison whose other side does not read the
/// time (D-6555), so that each evaluation also gives the next instant the value can change.
pub fn indexable(expr: &Expr) -> bool {
    match expr {
        Expr::Now => false,
        Expr::Cmp(_, a, b) => {
            (affine(a) && !reads_now(b))
                || (affine(b) && !reads_now(a))
                || (indexable(a) && indexable(b))
        }
        Expr::Lit(_)
        | Expr::Var(..)
        | Expr::Through(..)
        | Expr::Param(_)
        | Expr::Principal
        | Expr::HasRole(_) => true,
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => indexable(a),
        Expr::And(items) | Expr::Or(items) => items.iter().all(indexable),
        Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => indexable(a) && indexable(b),
        Expr::If(a, b, c) => indexable(a) && indexable(b) && indexable(c),
    }
}

/// The names a predicate reads, before they are resolved against the schema, and what it reads
/// that a segment may not (SEG-002, SEG-004).
#[derive(Default)]
struct Raw {
    subject: BTreeSet<String>,
    system: bool,
    hops: BTreeMap<String, BTreeSet<String>>,
    time: bool,
    roles: bool,
}

fn walk(expr: &Expr, raw: &mut Raw, problems: &mut Vec<String>) {
    match expr {
        Expr::Lit(_) => {}
        Expr::Var(Scope::Record, name) => {
            if name.starts_with('$') {
                raw.system = true;
            } else {
                raw.subject.insert(name.clone());
            }
        }
        Expr::Through(Scope::Record, reference, name) => {
            raw.subject.insert(reference.clone());
            raw.hops
                .entry(reference.clone())
                .or_default()
                .insert(name.clone());
        }
        Expr::Var(scope, name) | Expr::Through(scope, name, _) => problems.push(format!(
            "the predicate reads {scope:?}.{name}; a segment predicate reads the record of the person, not old or new (SEG-002)"
        )),
        Expr::Param(name) => problems.push(format!(
            "the predicate reads param.{name}; a segment predicate has no parameters (SEG-002)"
        )),
        Expr::Now => raw.time = true,
        Expr::Principal | Expr::HasRole(_) => raw.roles = true,
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => walk(a, raw, problems),
        Expr::And(items) | Expr::Or(items) => {
            for item in items {
                walk(item, raw, problems);
            }
        }
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => {
            walk(a, raw, problems);
            walk(b, raw, problems);
        }
        Expr::If(a, b, c) => {
            walk(a, raw, problems);
            walk(b, raw, problems);
            walk(c, raw, problems);
        }
    }
}

/// Check a predicate against its subject type and derive what it reads (SEG-002 to SEG-004): it
/// type-checks against the subject type and is a truth value; it reads only the person's record,
/// the records its single reference fields reach, the principal, the roles and the time; it has
/// at most [`MAX_PREDICATE_NODES`] nodes and follows at most [`MAX_HOPS`] references.
pub fn analyze(
    schema: &TenantSchema,
    subject: &ObjectTypeId,
    predicate: &Expr,
) -> Result<Analysis, Vec<String>> {
    let mut problems = Vec::new();
    let Some(object) = schema.objects.get(subject) else {
        return Err(vec![format!("unknown subject object type {subject}")]);
    };
    let nodes = expr_nodes(predicate);
    if nodes > MAX_PREDICATE_NODES {
        problems.push(format!(
            "the predicate has {nodes} nodes, more than the maximum of {MAX_PREDICATE_NODES} (SEG-004)"
        ));
    }
    let roles: BTreeSet<RoleId> = schema.roles.keys().cloned().collect();
    let no_params = BTreeSet::new();
    let env = TypeEnv {
        fields: &object.fields,
        params: &no_params,
        roles: &roles,
        objects: &schema.objects,
    };
    match expr::check(predicate, &env) {
        Ok(Shape::Bool | Shape::Any) => {}
        Ok(shape) => problems.push(format!("the predicate is {shape:?}, not a truth value")),
        Err(e) => problems.push(format!("the predicate: {e}")),
    }
    let mut raw = Raw::default();
    walk(predicate, &mut raw, &mut problems);
    let mut reads = Reads {
        system_attributes: raw.system,
        time: raw.time,
        roles: raw.roles,
        nodes,
        ..Reads::default()
    };
    for name in &raw.subject {
        match FieldId::new(name.as_str()) {
            Ok(id) if object.fields.contains_key(&id) => {
                reads.subject_fields.insert(id);
            }
            _ => problems.push(format!("unknown field {subject}.{name}")),
        }
    }
    for (reference, names) in &raw.hops {
        let Ok(id) = FieldId::new(reference.as_str()) else {
            continue;
        };
        let Some((target, _, false)) = object.fields.get(&id).and_then(|f| f.ty.ref_target())
        else {
            problems.push(format!(
                "{subject}.{reference} is not a reference to one record (SEG-002)"
            ));
            continue;
        };
        let Some(target_def) = schema.objects.get(target) else {
            problems.push(format!("unknown object type {target}"));
            continue;
        };
        let mut fields = BTreeSet::new();
        for name in names {
            match FieldId::new(name.as_str()) {
                Ok(field) if target_def.fields.contains_key(&field) => {
                    fields.insert(field);
                }
                _ => problems.push(format!("unknown field {target}.{name}")),
            }
        }
        reads.hops.insert(id, (target.clone(), fields));
    }
    // The language reaches one reference from the subject record; a chain longer than the bound
    // is refused here should it ever reach further.
    let depth = usize::from(!reads.hops.is_empty());
    if depth > MAX_HOPS {
        problems.push(format!(
            "the predicate reaches {depth} references from the subject record, more than {MAX_HOPS} (SEG-004)"
        ));
    }
    if problems.is_empty() {
        Ok(Analysis {
            indexable: reads.time && indexable(predicate),
            reads,
        })
    } else {
        Err(problems)
    }
}

/// The reads of a stored segment against a schema. A segment whose predicate no longer checks
/// reads nothing; [`schema_change_problems`] keeps a schema change from bringing that about.
pub fn reads_of(schema: &TenantSchema, segment: &Segment) -> Reads {
    analyze(schema, &segment.subject, &segment.predicate)
        .map(|a| a.reads)
        .unwrap_or_default()
}

/// The problems of the principal field of a subject type (SEG-1548): a required unique string.
pub fn principal_field_problems(
    schema: &TenantSchema,
    subject: &ObjectTypeId,
    field: &FieldId,
) -> Vec<String> {
    let Some(object) = schema.objects.get(subject) else {
        return vec![format!("unknown subject object type {subject}")];
    };
    match object.fields.get(field) {
        None => vec![format!("unknown field {subject}.{field}")],
        Some(def) if def.ty == FieldType::Str && def.required && def.unique => Vec::new(),
        Some(_) => vec![format!(
            "{subject}.{field} holds the principal of the person, so it must be a required unique string field (SEG-1548)"
        )],
    }
}

/// What the definer of a predicate may not read (SEG-005, D-5028), taking the strict reading:
/// every object type the predicate reads must be readable by the definer without a condition,
/// since a conditional read would let the members show records the definer cannot see; and no
/// field the predicate names, nor the principal field the members are listed by, may carry a
/// per-field read rule (D-6586), since whether it is visible depends on the reader and the
/// record, which a definition cannot know.
pub fn definer_problems(
    tenant: &TenantState,
    definer: &Principal,
    subject: &ObjectTypeId,
    principal_field: &FieldId,
    reads: &Reads,
) -> Vec<String> {
    let mut problems = Vec::new();
    for object in reads.objects(subject) {
        if !holds_power(
            tenant,
            definer,
            Action::Read,
            &Resource::Object(object.clone()),
        ) {
            problems.push(format!(
                "the definer may not read every record of {object}, which the predicate reads (SEG-005)"
            ));
        }
    }
    let mut named: Vec<(ObjectTypeId, FieldId)> = reads
        .subject_fields
        .iter()
        .map(|f| (subject.clone(), f.clone()))
        .collect();
    named.push((subject.clone(), principal_field.clone()));
    for (object, fields) in reads.hops.values() {
        named.extend(fields.iter().map(|f| (object.clone(), f.clone())));
    }
    for (object, field) in named {
        let hidden = tenant
            .schema
            .objects
            .get(&object)
            .and_then(|o| o.fields.get(&field))
            .is_some_and(|def| def.read.is_some());
        if hidden {
            problems.push(format!(
                "{object}.{field} has a read rule, so who may see it depends on the reader; a segment may not name it (SEG-005)"
            ));
        }
    }
    problems
}

/// Whether a principal holds a capability; the owner holds every one.
pub fn holds_capability(tenant: &TenantState, principal: &Principal, name: &str) -> bool {
    holds_any(
        tenant,
        principal,
        &BTreeSet::from([Grant::Capability(name.to_string())]),
    )
}

fn require_capability(
    tenant: &TenantState,
    principal: &Principal,
    name: &str,
    evidence: &mut Vec<Check>,
) -> Result<(), DomainError> {
    let rule = format!("permission:capability:{name}");
    if holds_capability(tenant, principal, name) {
        evidence.push(Check::pass(rule, format!("holds {name}")));
        Ok(())
    } else {
        evidence.push(Check::fail(rule, format!("does not hold {name}")));
        Err(DomainError::new(
            ErrorKind::Unauthorized,
            format!("this needs the capability {name}"),
        )
        .with(evidence.clone()))
    }
}

/// Whether the definer of a segment still passes the checks of its definition. The question
/// D-5028 leaves open, whether a later withdrawal of the definer's permission suspends the
/// segment, is answered the strict way: while the definer would no longer be allowed to define
/// it, nobody may read its members or count them. Its membership is still kept, so it equals
/// the evaluation again the moment the permission returns.
pub fn suspended(tenant: &TenantState, segment: &Segment) -> bool {
    let Some(definer) = &segment.defined_by else {
        return true;
    };
    let definer = Principal::User(definer.clone());
    let reads = reads_of(&tenant.schema, segment);
    !holds_capability(tenant, &definer, SEGMENTS_DEFINE)
        || !definer_problems(
            tenant,
            &definer,
            &segment.subject,
            &segment.principal_field,
            &reads,
        )
        .is_empty()
}

// ---------------------------------------------------------------------------------------------
// Definition
// ---------------------------------------------------------------------------------------------

/// What `define-segment` asks for (CT-290).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    pub id: SegmentId,
    pub name: String,
    pub subject: ObjectTypeId,
    pub predicate: Expr,
    #[serde(default)]
    pub refresh_seconds: Option<u32>,
    pub principal_field: FieldId,
    #[serde(default)]
    pub visible_to_members: bool,
}

/// A definition from the declaration of a bundle (D-6580 to D-6582): the id derived from the
/// tenant and the key, the predicate parsed from its text. The principal field is the one the
/// declaration names, else the field `principal` of the subject type.
pub fn from_decl(
    tenant: &TenantId,
    key: &SegmentKey,
    decl: &SegmentDecl,
    principal_field: Option<FieldId>,
    visible_to_members: bool,
) -> Result<Definition, String> {
    let predicate = expr::parse(decl.predicate.as_str())?;
    let principal_field = match principal_field {
        Some(field) => field,
        None => FieldId::new("principal").map_err(|e| e.to_string())?,
    };
    Ok(Definition {
        id: derive_segment_id(tenant, key),
        name: decl.name.clone(),
        subject: decl.subject_type.clone(),
        predicate,
        refresh_seconds: decl.refresh_seconds,
        principal_field,
        visible_to_members,
    })
}

/// What [`define`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Defined {
    pub segment: Segment,
    pub write: SegmentWrite,
    /// Whether the predicate is new or changed, so that batches evaluate every person under it.
    pub rebuild: bool,
    pub evidence: Vec<Check>,
}

fn invalid(message: impl Into<String>, evidence: &[Check]) -> DomainError {
    DomainError::new(ErrorKind::InvalidIntent, message).with(evidence.to_vec())
}

/// The problems of a predicate that a definition, a test and a preview all refuse (SEG-003,
/// SEG-004, SEG-005): what [`analyze`] finds, then what the caller may not read.
fn predicate_check(
    tenant: &TenantState,
    caller: &Principal,
    subject: &ObjectTypeId,
    principal_field: &FieldId,
    predicate: &Expr,
    evidence: &mut Vec<Check>,
) -> Result<Analysis, DomainError> {
    let analysis = analyze(&tenant.schema, subject, predicate).map_err(|problems| {
        for problem in &problems {
            evidence.push(Check::fail("segment:predicate", problem.clone()));
        }
        invalid(problems.join("; "), evidence)
    })?;
    evidence.push(Check::pass(
        "segment:predicate",
        format!(
            "{} nodes, reads {}",
            analysis.reads.nodes,
            analysis.reads.field_names(subject).join(", ")
        ),
    ));
    let forbidden = definer_problems(tenant, caller, subject, principal_field, &analysis.reads);
    if !forbidden.is_empty() {
        for problem in &forbidden {
            evidence.push(Check::fail("segment:field-forbidden", problem.clone()));
        }
        return Err(
            DomainError::new(ErrorKind::Unauthorized, forbidden.join("; ")).with(evidence.clone()),
        );
    }
    Ok(analysis)
}

/// Decide a definition or a change of one (CT-290). The caller holds `segments-define`
/// (SEG-007); the name is not empty and is not another segment's; the principal field is a
/// required unique string field (SEG-1548); the predicate checks and reads only what SEG-002
/// allows (SEG-003, SEG-004) and only what the caller may read (SEG-005); a time-dependent
/// predicate declares a refresh interval of one minute to one day, fifteen minutes at least when
/// it is not indexable, and any other predicate declares none (SEG-017, SEG-020, SEG-1546); the
/// tenant stays within one hundred segments, twenty-five reading any one object type and five
/// full refreshes (SEG-029, SEG-1547). The subject type and the principal field of an existing
/// segment do not change: a segment over other people is another segment.
///
/// A new or changed predicate raises the version and starts the batches that evaluate every
/// person under it (SEG-022, D-5027); a change of name or of visibility keeps the version.
pub fn define(
    tenant: &TenantState,
    catalog: &[Segment],
    def: Definition,
    ctx: &Context,
) -> Result<Defined, DomainError> {
    let mut evidence = Vec::new();
    if tenant.id != ctx.tenant {
        return Err(DomainError::new(
            ErrorKind::TenantMismatch,
            "the snapshot is of another tenant",
        ));
    }
    require_capability(tenant, &ctx.principal, SEGMENTS_DEFINE, &mut evidence)?;
    let name = def.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        evidence.push(Check::fail(
            "segment:name",
            "a name has 1 to 120 characters",
        ));
        return Err(invalid("a segment name has 1 to 120 characters", &evidence));
    }
    if catalog.iter().any(|s| s.id != def.id && s.name == name) {
        evidence.push(Check::fail("segment:name", format!("{name} is taken")));
        return Err(invalid(
            format!("another segment is named {name}"),
            &evidence,
        ));
    }
    let existing = catalog.iter().find(|s| s.id == def.id);
    if let Some(old) = existing {
        if old.subject != def.subject || old.principal_field != def.principal_field {
            evidence.push(Check::fail(
                "segment:subject",
                "the subject type and principal field of a segment do not change",
            ));
            return Err(invalid(
                "the subject type and principal field of a segment do not change; remove it and define another",
                &evidence,
            ));
        }
    }
    let field_problems =
        principal_field_problems(&tenant.schema, &def.subject, &def.principal_field);
    if !field_problems.is_empty() {
        for problem in &field_problems {
            evidence.push(Check::fail("segment:principal-field", problem.clone()));
        }
        return Err(invalid(field_problems.join("; "), &evidence));
    }
    let analysis = predicate_check(
        tenant,
        &ctx.principal,
        &def.subject,
        &def.principal_field,
        &def.predicate,
        &mut evidence,
    )?;
    let time = analysis.reads.time;
    match (time, def.refresh_seconds) {
        (true, None) => {
            evidence.push(Check::fail("segment:refresh", "no refresh interval"));
            return Err(invalid(
                "a predicate that reads the time declares a refresh interval (SEG-017)",
                &evidence,
            ));
        }
        (true, Some(seconds)) => {
            let floor = if analysis.indexable {
                MIN_REFRESH_SECONDS
            } else {
                MIN_FULL_REFRESH_SECONDS
            };
            if seconds < floor || seconds > MAX_REFRESH_SECONDS {
                evidence.push(Check::fail(
                    "segment:refresh",
                    format!("{seconds} s is outside {floor} s to {MAX_REFRESH_SECONDS} s"),
                ));
                return Err(invalid(
                    format!(
                        "the refresh interval of this predicate is from {floor} to {MAX_REFRESH_SECONDS} seconds (SEG-017, SEG-1546)"
                    ),
                    &evidence,
                ));
            }
        }
        (false, Some(_)) => {
            evidence.push(Check::fail(
                "segment:refresh",
                "the predicate does not read the time",
            ));
            return Err(invalid(
                "a predicate that does not read the time has no refresh interval (SEG-020)",
                &evidence,
            ));
        }
        (false, None) => {}
    }
    // The bounds, counted over the tenant's segments with this one in place of its old self.
    let others: Vec<&Segment> = catalog.iter().filter(|s| s.id != def.id).collect();
    if others.len() + 1 > MAX_SEGMENTS {
        evidence.push(Check::fail(
            "limit:segments",
            format!("{MAX_SEGMENTS} already"),
        ));
        return Err(DomainError::new(
            ErrorKind::LimitExceeded,
            format!("a tenant holds at most {MAX_SEGMENTS} segments (SEG-029)"),
        )
        .with(evidence));
    }
    for object in analysis.reads.objects(&def.subject) {
        let reading = others
            .iter()
            .filter(|s| {
                reads_of(&tenant.schema, s)
                    .objects(&s.subject)
                    .contains(&object)
            })
            .count();
        if reading + 1 > MAX_SEGMENTS_PER_OBJECT {
            evidence.push(Check::fail(
                "limit:segments-per-object",
                format!("{reading} segments read {object}"),
            ));
            return Err(DomainError::new(
                ErrorKind::LimitExceeded,
                format!("at most {MAX_SEGMENTS_PER_OBJECT} segments read {object} (SEG-029)"),
            )
            .with(evidence));
        }
    }
    if time && !analysis.indexable {
        let full = others
            .iter()
            .filter(|s| s.time_dependent && !s.indexable)
            .count();
        if full + 1 > MAX_FULL_REFRESH_SEGMENTS {
            evidence.push(Check::fail(
                "limit:full-refresh",
                format!("{full} segments refresh in full"),
            ));
            return Err(DomainError::new(
                ErrorKind::LimitExceeded,
                format!(
                    "a tenant holds at most {MAX_FULL_REFRESH_SEGMENTS} time-dependent segments that are not indexable (SEG-1547)"
                ),
            )
            .with(evidence));
        }
    }
    evidence.push(Check::pass("segment:bounds", "within the tenant's bounds"));

    let rebuild = existing.is_none_or(|old| old.predicate != def.predicate);
    let version = match existing {
        None => 1,
        Some(old) if rebuild => old.version + 1,
        Some(old) => old.version,
    };
    let defined_by = match &ctx.principal {
        Principal::User(id) => Some(id.clone()),
        _ => existing.and_then(|old| old.defined_by.clone()),
    };
    let segment = Segment {
        id: def.id,
        name: name.to_string(),
        subject: def.subject.clone(),
        predicate: def.predicate.clone(),
        version,
        time_dependent: time,
        indexable: analysis.indexable,
        refresh_seconds: def.refresh_seconds,
        defined_by,
        principal_field: def.principal_field.clone(),
        visible_to_members: def.visible_to_members,
        schema_version: tenant.schema.version,
        member_count: existing.map_or(0, |old| old.member_count),
        refresh_skipped_at: existing.and_then(|old| old.refresh_skipped_at),
    };
    let mut write = SegmentWrite::default();
    write.expect.insert(def.id, existing.map(|old| old.version));
    write.put_segments.push(segment.clone());
    if rebuild {
        write.clear_batches.push(def.id);
        write.batches.push(BatchChange::Put(Batch {
            segment: def.id,
            scope: BatchScope::All,
            version,
            after: None,
            reason: SegmentReason::PredicateChange,
            hop: 0,
            at: ctx.now,
        }));
    }
    Ok(Defined {
        segment,
        write,
        rebuild,
        evidence,
    })
}

/// Decide the removal of a segment (CT-291): its memberships, wake rows and batches go with it
/// and no exit event is written (SEG-026, D-5033). The schedule of a time-dependent segment is
/// the refresh job's, which finds no segment and creates nothing.
pub fn remove(
    tenant: &TenantState,
    catalog: &[Segment],
    id: SegmentId,
    ctx: &Context,
) -> Result<SegmentWrite, DomainError> {
    let mut evidence = Vec::new();
    require_capability(tenant, &ctx.principal, SEGMENTS_DEFINE, &mut evidence)?;
    let Some(segment) = catalog.iter().find(|s| s.id == id) else {
        return Err(DomainError::new(
            ErrorKind::UnknownRecord,
            format!("no segment {id}"),
        ));
    };
    let mut write = SegmentWrite::default();
    write.expect.insert(id, Some(segment.version));
    write.remove_segments.push(id);
    Ok(write)
}

/// The problems a schema change would give the segments of a tenant (SEG-006): a field a segment
/// names, or the principal field it is listed by, that the new schema removes or retypes.
pub fn schema_change_problems(
    catalog: &[Segment],
    old: &TenantSchema,
    new: &TenantSchema,
) -> Vec<String> {
    let mut all = Vec::new();
    for segment in catalog {
        let mut problems = Vec::new();
        let reads = reads_of(old, segment);
        let mut named: Vec<(ObjectTypeId, FieldId)> = reads
            .subject_fields
            .iter()
            .map(|f| (segment.subject.clone(), f.clone()))
            .collect();
        named.push((segment.subject.clone(), segment.principal_field.clone()));
        for (object, fields) in reads.hops.values() {
            named.extend(fields.iter().map(|f| (object.clone(), f.clone())));
        }
        for (object, field) in named {
            let before = old.objects.get(&object).and_then(|o| o.fields.get(&field));
            let after = new.objects.get(&object).and_then(|o| o.fields.get(&field));
            match (before, after) {
                (Some(_), None) => problems.push(format!(
                    "segment {} names {object}.{field}, which the change removes (SEG-006)",
                    segment.name
                )),
                (Some(b), Some(a)) if b.ty != a.ty => problems.push(format!(
                    "segment {} names {object}.{field}, which the change retypes (SEG-006)",
                    segment.name
                )),
                _ => {}
            }
        }
        if problems.is_empty() {
            if let Err(found) = analyze(new, &segment.subject, &segment.predicate) {
                for problem in found {
                    problems.push(format!("segment {}: {problem} (SEG-006)", segment.name));
                }
            }
            problems.extend(principal_field_problems(
                new,
                &segment.subject,
                &segment.principal_field,
            ));
        }
        all.extend(problems);
    }
    all
}

// ---------------------------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------------------------

/// The outcome of one evaluation (CT-294).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub member: bool,
    /// The next instant the value can change, for an indexable time-dependent predicate.
    pub wake: Option<Timestamp>,
    /// The records it read: the subject record and the records its references reached.
    pub reads: Vec<RecordId>,
}

fn number(value: &Value) -> Option<i64> {
    match value {
        Value::Int(n) | Value::Time(n) => Some(*n),
        _ => None,
    }
}

/// The comparisons of `now` with a value that does not read it, and the instants at which each
/// can change its value, collected into `out`.
fn cross(moving: &Expr, fixed: &Expr, env: &Env<'_>, at_zero: &Env<'_>, out: &mut Vec<i64>) {
    if let (Ok(k), Ok(v)) = (expr::eval(moving, at_zero), expr::eval(fixed, env)) {
        if let (Some(k), Some(v)) = (number(&k), number(&v)) {
            if let Some(t) = v.checked_sub(k) {
                out.push(t);
                if let Some(next) = t.checked_add(1) {
                    out.push(next);
                }
            }
        }
    }
}

fn thresholds(expr: &Expr, env: &Env<'_>, at_zero: &Env<'_>, out: &mut Vec<i64>) {
    match expr {
        Expr::Cmp(_, a, b) if affine(a) && !reads_now(b) => cross(a, b, env, at_zero, out),
        Expr::Cmp(_, a, b) if affine(b) && !reads_now(a) => cross(b, a, env, at_zero, out),
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => thresholds(a, env, at_zero, out),
        Expr::And(items) | Expr::Or(items) => {
            for item in items {
                thresholds(item, env, at_zero, out);
            }
        }
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => {
            thresholds(a, env, at_zero, out);
            thresholds(b, env, at_zero, out);
        }
        Expr::If(a, b, c) => {
            thresholds(a, env, at_zero, out);
            thresholds(b, env, at_zero, out);
            thresholds(c, env, at_zero, out);
        }
        _ => {}
    }
}

/// The next instant after `env.now` at which an indexable predicate can change its value for the
/// person: the earliest instant at which one of its comparisons with the time can change. Each
/// comparison `now + k <op> v` changes at `v - k` or one millisecond later; waking at either is
/// never late, and an early wake evaluates, finds nothing changed and computes the next.
pub fn next_wake(predicate: &Expr, env: &Env<'_>) -> Option<Timestamp> {
    let at_zero = Env {
        record: env.record,
        old: env.old,
        new: env.new,
        params: env.params,
        now: Timestamp(0),
        principal: env.principal,
        roles: env.roles,
        lookup: env.lookup,
    };
    let mut found = Vec::new();
    thresholds(predicate, env, &at_zero, &mut found);
    found
        .into_iter()
        .filter(|t| *t > env.now.0)
        .min()
        .map(Timestamp)
}

/// Evaluate the predicate of a segment for one person on their subject record, at the time given
/// (CT-294, SEG-015, SEG-016): the principal is the person and the roles are theirs; a reference
/// is followed only into a record the snapshot covers. A record it needs and the snapshot does
/// not cover is `InsufficientState` naming it; any other failure, the step budget included, is an
/// `Evaluation` error naming the person and the segment (SEG-030).
pub fn evaluate(
    segment: &Segment,
    state: &State,
    record: &Record,
    person: &PrincipalId,
    now: Timestamp,
) -> Result<Evaluation, DomainError> {
    let tenant = state
        .tenant
        .as_deref()
        .ok_or_else(|| DomainError::new(ErrorKind::TenantNotFound, "no such tenant"))?;
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let principal = Principal::User(person.clone());
    let roles = roles_of(tenant, &principal);
    let params = BTreeMap::new();
    let env = Env {
        record: Some(record),
        old: None,
        new: None,
        params: &params,
        now,
        principal: &principal,
        roles: &roles,
        lookup: Some(lookup),
    };
    let member = match expr::holds(&segment.predicate, &env) {
        Ok(member) => member,
        Err(e) => {
            if let Some(need) = expr::missing(&segment.predicate, &env) {
                return Err(insufficient(need));
            }
            return Err(DomainError::new(
                ErrorKind::Evaluation,
                format!(
                    "segment {} ({}), person {person}: {e}",
                    segment.name, segment.id
                ),
            )
            .with(vec![Check::fail(
                format!("segment:evaluate:{}", segment.id),
                format!("person {person}: {e}"),
            )]));
        }
    };
    let wake = if segment.time_dependent && segment.indexable {
        next_wake(&segment.predicate, &env)
    } else {
        None
    };
    let mut reads = vec![record.id];
    let hops = reads_of(&tenant.schema, segment);
    for reference in hops.hops.keys() {
        if let Value::Ref(id) = record.get(reference) {
            reads.push(*id);
        }
    }
    Ok(Evaluation {
        member,
        wake,
        reads,
    })
}

// ---------------------------------------------------------------------------------------------
// Commit: which persons a change affects, and what they need
// ---------------------------------------------------------------------------------------------

/// Where the subject record of a person to evaluate is found.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Locate {
    /// This record, when it still holds the person in its principal field.
    Id(RecordId),
    /// The record of the subject type whose principal field holds the person.
    Principal,
    /// The person has no subject record any more.
    Gone,
}

/// One person to evaluate for one segment.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Target {
    pub segment: SegmentId,
    pub person: PrincipalId,
    pub locate: Locate,
}

/// The referrers of a changed record that a segment must evaluate: the subject records whose
/// reference field holds it. They are found through the record index, at most [`BATCH`] + 1 at a
/// time in record id order (`Need::ByField` of `object`, `field`, `Value::Ref(record)`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Referencing {
    pub segment: SegmentId,
    pub object: ObjectTypeId,
    pub field: FieldId,
    pub record: RecordId,
}

impl Referencing {
    /// The need that names every referrer; a store answers it in record id order and may bound
    /// it to the first [`BATCH`] + 1.
    pub fn need(&self) -> Need {
        Need::ByField {
            object: self.object.clone(),
            field: self.field.clone(),
            value: Value::Ref(self.record),
        }
    }
}

/// What a change asks of the segments (CT-295): the persons whose subject record changed, and
/// the referenced records whose referrers are found through the index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangePlan {
    pub direct: Vec<Target>,
    pub referencing: Vec<Referencing>,
}

/// The records an accepted transition changed, as (before, after), from its events.
pub fn changes(events: &[DomainEvent]) -> Vec<(Option<Arc<Record>>, Option<Arc<Record>>)> {
    events
        .iter()
        .filter_map(|event| match &event.body {
            EventBody::RecordCreated { record } => Some((None, Some(record.clone()))),
            EventBody::RecordUpdated { before, after, .. } => {
                Some((Some(before.clone()), Some(after.clone())))
            }
            EventBody::RecordDeleted { before } => Some((Some(before.clone()), None)),
            _ => None,
        })
        .collect()
}

/// The person a subject record stands for: the principal its principal field holds.
pub fn person_of(record: &Record, principal_field: &FieldId) -> Option<PrincipalId> {
    match record.get(principal_field) {
        Value::Str(text) => PrincipalId::new(text.as_str()).ok(),
        _ => None,
    }
}

fn changed_fields(before: &Record, after: &Record) -> BTreeSet<FieldId> {
    before
        .fields
        .keys()
        .chain(after.fields.keys())
        .filter(|f| before.get(f) != after.get(f))
        .cloned()
        .collect()
}

/// Name, from the events of an accepted transition, what the segments must evaluate (CT-295,
/// D-5025): for a segment over object type S, each person whose S record was created, deleted
/// or changed in a field the predicate reads (its principal field, a field it names, a
/// reference it follows, or any field when it reads a system attribute), under the principal
/// before and after; for a record of a type the predicate reaches through reference field r,
/// changed in a field the predicate reads there, the referrers of that record through r; and for
/// a change of a person's roles, the person in every segment whose predicate reads roles. No
/// subject type is scanned.
pub fn plan_change(
    catalog: &[Segment],
    schema: &TenantSchema,
    events: &[DomainEvent],
) -> ChangePlan {
    let mut plan = ChangePlan::default();
    let changed = changes(events);
    let roles_changed: Vec<&PrincipalId> = events
        .iter()
        .filter_map(|event| match &event.body {
            EventBody::MemberChanged { principal, .. } => Some(principal),
            _ => None,
        })
        .collect();
    for segment in catalog {
        let reads = reads_of(schema, segment);
        let pf = &segment.principal_field;
        let mut direct: BTreeSet<Target> = BTreeSet::new();
        for (before, after) in &changed {
            let object = before.as_ref().or(after.as_ref()).map(|r| r.object.clone());
            let Some(object) = object else { continue };
            if object == segment.subject {
                let relevant = match (before, after) {
                    (Some(b), Some(a)) => {
                        reads.system_attributes
                            || changed_fields(b, a).iter().any(|f| {
                                f == pf
                                    || reads.subject_fields.contains(f)
                                    || reads.hops.contains_key(f)
                            })
                    }
                    _ => true,
                };
                if !relevant {
                    continue;
                }
                if let Some(a) = after {
                    if let Some(person) = person_of(a, pf) {
                        direct.insert(Target {
                            segment: segment.id,
                            person,
                            locate: Locate::Id(a.id),
                        });
                    }
                }
                if let Some(b) = before {
                    if let Some(person) = person_of(b, pf) {
                        let still = after
                            .as_ref()
                            .and_then(|a| person_of(a, pf))
                            .is_some_and(|p| p == person);
                        if !still {
                            direct.insert(Target {
                                segment: segment.id,
                                person,
                                locate: Locate::Gone,
                            });
                        }
                    }
                }
            }
            if let (Some(b), Some(a)) = (before, after) {
                let fields = changed_fields(b, a);
                for (reference, (target, read)) in &reads.hops {
                    if *target == object && fields.iter().any(|f| read.contains(f)) {
                        plan.referencing.push(Referencing {
                            segment: segment.id,
                            object: segment.subject.clone(),
                            field: reference.clone(),
                            record: a.id,
                        });
                    }
                }
            }
        }
        if reads.roles {
            for person in &roles_changed {
                if !direct.iter().any(|t| &t.person == *person) {
                    direct.insert(Target {
                        segment: segment.id,
                        person: (*person).clone(),
                        locate: Locate::Principal,
                    });
                }
            }
        }
        plan.direct.extend(direct);
    }
    plan.referencing.sort();
    plan.referencing.dedup();
    plan
}

/// Bound a plan to [`BATCH`] persons a segment (SEG-028): the persons whose subject record
/// changed first, then the referrers the store found (in record id order, at most [`BATCH`] + 1
/// for each referenced record, taken as they are after the change: `after` holds the records the
/// transition changed). What does not fit is left to a continuation in the same write: the
/// remaining referrers of a record from the last one taken, or, when the changed subject
/// records alone are more than a batch, every person of the subject type.
pub fn targets(
    plan: &ChangePlan,
    catalog: &[Segment],
    referenced: &[(Referencing, Vec<Arc<Record>>)],
    after: &BTreeMap<(ObjectTypeId, RecordId), Option<Arc<Record>>>,
    hop: u64,
    now: Timestamp,
) -> (Vec<Target>, Vec<Batch>) {
    let mut out = Vec::new();
    let mut batches = Vec::new();
    for segment in catalog {
        let mut taken: BTreeSet<PrincipalId> = BTreeSet::new();
        let mut mine: Vec<Target> = plan
            .direct
            .iter()
            .filter(|t| t.segment == segment.id)
            .cloned()
            .collect();
        if mine.len() > BATCH {
            mine.truncate(BATCH);
            batches.push(Batch {
                segment: segment.id,
                scope: BatchScope::All,
                version: segment.version,
                after: None,
                reason: SegmentReason::Change,
                hop,
                at: now,
            });
        }
        for t in &mine {
            taken.insert(t.person.clone());
        }
        out.extend(mine);
        for (item, found) in referenced.iter().filter(|(r, _)| r.segment == segment.id) {
            let mut last = None;
            let mut cut = false;
            for (index, record) in found.iter().enumerate() {
                if taken.len() >= BATCH || index >= BATCH {
                    cut = true;
                    break;
                }
                let current = after
                    .get(&(record.object.clone(), record.id))
                    .cloned()
                    .unwrap_or_else(|| Some(record.clone()));
                last = Some(record.id);
                let Some(current) = current else { continue };
                let Some(person) = person_of(&current, &segment.principal_field) else {
                    continue;
                };
                if taken.insert(person.clone()) {
                    out.push(Target {
                        segment: segment.id,
                        person,
                        locate: Locate::Id(record.id),
                    });
                }
            }
            if cut {
                batches.push(Batch {
                    segment: segment.id,
                    scope: BatchScope::Referencing {
                        field: item.field.clone(),
                        record: item.record,
                    },
                    version: segment.version,
                    after: last,
                    reason: SegmentReason::Change,
                    hop,
                    at: now,
                });
            }
        }
    }
    (out, batches)
}

/// The pairs whose rows a decision over `targets` needs.
pub fn pairs(targets: &[Target]) -> Vec<(SegmentId, PrincipalId)> {
    let set: BTreeSet<(SegmentId, PrincipalId)> = targets
        .iter()
        .map(|t| (t.segment, t.person.clone()))
        .collect();
    set.into_iter().collect()
}

/// The records a decision over `targets` still needs that `state` does not cover (CT-295,
/// SEG-027): the subject record of each person, then the records its predicate reaches from it.
/// Empty when the decision can be made. A predicate follows one reference, so a shell that loads
/// what this names and asks again is done after two rounds.
pub fn needs_for(catalog: &[Segment], state: &State, targets: &[Target]) -> Vec<Need> {
    let Some(tenant) = state.tenant.as_deref() else {
        return Vec::new();
    };
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let mut wanted: BTreeSet<Need> = BTreeSet::new();
    let mut hops_of: BTreeMap<SegmentId, Reads> = BTreeMap::new();
    for target in targets {
        let Some(segment) = catalog.iter().find(|s| s.id == target.segment) else {
            continue;
        };
        let need = match &target.locate {
            Locate::Id(id) => Need::Record {
                object: segment.subject.clone(),
                id: *id,
            },
            Locate::Principal => Need::ByField {
                object: segment.subject.clone(),
                field: segment.principal_field.clone(),
                value: Value::Str(target.person.as_str().to_string()),
            },
            Locate::Gone => continue,
        };
        if !state.coverage.covers(&need) {
            wanted.insert(need);
            continue;
        }
        let Ok(Some(record)) = locate(segment, &lookup, target) else {
            continue;
        };
        let reads = hops_of
            .entry(segment.id)
            .or_insert_with(|| reads_of(&tenant.schema, segment));
        for (reference, (object, _)) in &reads.hops {
            if let Value::Ref(id) = record.get(reference) {
                let need = Need::Record {
                    object: object.clone(),
                    id: *id,
                };
                if !state.coverage.covers(&need) {
                    wanted.insert(need);
                }
            }
        }
    }
    wanted.into_iter().collect()
}

/// The subject record of a target, when the snapshot holds it and it still stands for the person.
fn locate<'a>(
    segment: &Segment,
    lookup: &Lookup<'a>,
    target: &Target,
) -> Result<Option<&'a Arc<Record>>, DomainError> {
    let found = match &target.locate {
        Locate::Id(id) => lookup.record(&segment.subject, *id)?,
        Locate::Principal => lookup
            .by_field(
                &segment.subject,
                &segment.principal_field,
                &Value::Str(target.person.as_str().to_string()),
            )?
            .into_iter()
            .next(),
        Locate::Gone => None,
    };
    Ok(found.filter(|r| person_of(r, &segment.principal_field).as_ref() == Some(&target.person)))
}

/// A snapshot with the changes of a transition laid over it: the state after the commit, as far
/// as the snapshot reaches. Every changed record is covered, so a predicate reads its new value.
pub fn overlay(state: &State, changed: &[(Option<Arc<Record>>, Option<Arc<Record>>)]) -> State {
    let Some(tenant) = state.tenant.as_deref() else {
        return state.clone();
    };
    let mut next = tenant.clone();
    let mut covered: BTreeSet<Need> = match &state.coverage {
        Coverage::Partial(needs) => needs.clone(),
        Coverage::Complete => BTreeSet::new(),
    };
    for (before, after) in changed {
        let Some(any) = after.as_ref().or(before.as_ref()) else {
            continue;
        };
        covered.insert(Need::Record {
            object: any.object.clone(),
            id: any.id,
        });
        match after {
            Some(record) => {
                next.records
                    .entry(record.object.clone())
                    .or_default()
                    .insert(record.id, record.clone());
            }
            None => {
                if let Some(records) = next.records.get_mut(&any.object) {
                    records.remove(&any.id);
                }
            }
        }
    }
    State {
        tenant_id: state.tenant_id.clone(),
        tenant: Some(Arc::new(next)),
        coverage: match state.coverage {
            Coverage::Complete => Coverage::Complete,
            Coverage::Partial(_) => Coverage::Partial(covered),
        },
    }
}

/// The records a transition changed, by object type and id, as they are after it (`None`:
/// deleted).
pub fn after_map(
    changed: &[(Option<Arc<Record>>, Option<Arc<Record>>)],
) -> BTreeMap<(ObjectTypeId, RecordId), Option<Arc<Record>>> {
    changed
        .iter()
        .filter_map(|(before, after)| {
            let any = after.as_ref().or(before.as_ref())?;
            Some(((any.object.clone(), any.id), after.clone()))
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Commit: the decision
// ---------------------------------------------------------------------------------------------

/// What a decision about memberships is made for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occasion {
    pub reason: SegmentReason,
    /// The hop count of the change; every event carries it plus one (SEG-031, SEG-032).
    pub hop: u64,
    /// The revision of the change.
    pub revision: StateRevision,
    pub now: Timestamp,
}

/// What [`decide_targets`] decided.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decided {
    pub write: SegmentWrite,
    /// One entry or exit event for each person whose membership changed, in target order.
    pub events: Vec<SegmentMembershipEvent>,
    /// Evaluations that failed: the membership of the person stays as it was (SEG-030).
    pub failures: Vec<Check>,
}

/// Decide the memberships of `targets` on the state after a change (CT-296): one entry event for
/// each person who starts to satisfy the predicate of a segment and one exit event for each who
/// stops (SEG-008, SEG-009), nothing for a person whose membership is as it was (SEG-010), a
/// membership row raised to the current version for a person who stays a member under a changed
/// predicate, the wake row of an indexable predicate, and no entry for an erased person
/// (SEG-037). An evaluation that fails leaves the person as they were and is reported
/// (SEG-030). `rows` must cover every pair of the targets; a record the snapshot does not cover
/// is `InsufficientState`.
pub fn decide_targets(
    catalog: &[Segment],
    state: &State,
    targets: &[Target],
    rows: &Rows,
    occasion: &Occasion,
) -> Result<Decided, DomainError> {
    let tenant = state
        .tenant
        .as_deref()
        .ok_or_else(|| DomainError::new(ErrorKind::TenantNotFound, "no such tenant"))?;
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let mut decided = Decided::default();
    let mut seen: BTreeSet<(SegmentId, PrincipalId)> = BTreeSet::new();
    for target in targets {
        let key = (target.segment, target.person.clone());
        if !seen.insert(key.clone()) {
            continue;
        }
        let Some(segment) = catalog.iter().find(|s| s.id == target.segment) else {
            continue;
        };
        decided
            .write
            .expect
            .entry(segment.id)
            .or_insert(Some(segment.version));
        if !rows.covered.contains(&key) {
            return Err(DomainError::new(
                ErrorKind::InsufficientState,
                format!(
                    "the membership of {} in segment {} was not loaded",
                    target.person, segment.id
                ),
            ));
        }
        decided.write.seen.push(Seen {
            segment: segment.id,
            person: target.person.clone(),
            membership: rows.memberships.get(&key).cloned(),
            wake: rows.wakes.get(&key).copied(),
        });
        if rows.erased.contains(&target.person) {
            continue;
        }
        let record = locate(segment, &lookup, target)?;
        let outcome = match record {
            None => Evaluation {
                member: false,
                wake: None,
                reads: Vec::new(),
            },
            Some(record) => match evaluate(segment, state, record, &target.person, occasion.now) {
                Ok(outcome) => outcome,
                Err(error) if error.kind == ErrorKind::Evaluation => {
                    decided.failures.extend(error.evidence);
                    continue;
                }
                Err(error) => return Err(error),
            },
        };
        let current = rows.memberships.get(&key);
        let event = |direction| SegmentMembershipEvent {
            segment: segment.id,
            person: target.person.clone(),
            direction,
            reason: occasion.reason,
            version: segment.version,
            revision: occasion.revision,
            hop: occasion.hop + 1,
        };
        match (current, outcome.member) {
            (None, true) => {
                decided.write.memberships.push(MembershipChange {
                    segment: segment.id,
                    person: target.person.clone(),
                    before: None,
                    after: Some(Membership {
                        segment: segment.id,
                        person: target.person.clone(),
                        version: segment.version,
                        since: occasion.now,
                    }),
                });
                decided.events.push(event(SegmentDirection::Entered));
            }
            (Some(was), false) => {
                decided.write.memberships.push(MembershipChange {
                    segment: segment.id,
                    person: target.person.clone(),
                    before: Some(was.clone()),
                    after: None,
                });
                decided.events.push(event(SegmentDirection::Exited));
            }
            (Some(was), true) if was.version != segment.version => {
                decided.write.memberships.push(MembershipChange {
                    segment: segment.id,
                    person: target.person.clone(),
                    before: Some(was.clone()),
                    after: Some(Membership {
                        version: segment.version,
                        ..was.clone()
                    }),
                });
            }
            _ => {}
        }
        let old_wake = rows.wakes.get(&key).copied();
        let new_wake = if segment.time_dependent && segment.indexable {
            outcome.wake
        } else {
            None
        };
        match (old_wake, new_wake) {
            (old, Some(at)) if old != Some(at) => decided.write.wakes.push(WakeChange::Put(Wake {
                segment: segment.id,
                person: target.person.clone(),
                at,
            })),
            (Some(_), None) => decided.write.wakes.push(WakeChange::Remove {
                segment: segment.id,
                person: target.person.clone(),
            }),
            _ => {}
        }
    }
    Ok(decided)
}

/// The membership events as the log keeps them, stamped with the context of the decision
/// (D-5023).
pub fn stamp(events: &[SegmentMembershipEvent], ctx: &Context) -> Vec<SegmentEvent> {
    events
        .iter()
        .map(|event| SegmentEvent {
            tenant: ctx.tenant.clone(),
            at: ctx.now,
            principal: ctx.principal.clone(),
            correlation: ctx.correlation.clone(),
            event: event.clone(),
        })
        .collect()
}

/// [`decide_targets`] with its events stamped into the write, which is then the whole of what
/// the store applies.
pub fn decide_write(
    catalog: &[Segment],
    state: &State,
    targets: &[Target],
    rows: &Rows,
    occasion: &Occasion,
    ctx: &Context,
) -> Result<Decided, DomainError> {
    let mut decided = decide_targets(catalog, state, targets, rows, occasion)?;
    decided.write.events = stamp(&decided.events, ctx);
    Ok(decided)
}

// ---------------------------------------------------------------------------------------------
// Continuations, refresh and erasure
// ---------------------------------------------------------------------------------------------

/// What one batch needs loaded before [`batch_targets`] can name its persons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchLoad {
    /// At most `limit` subject records after `after`, in id order (`Need::PageOf`).
    Page {
        object: ObjectTypeId,
        after: Option<RecordId>,
        limit: u32,
    },
    /// At most `limit` subject records after `after` whose `field` holds `record`, in id order:
    /// the record index (`Need::ByField`), bounded.
    Referencing {
        object: ObjectTypeId,
        field: FieldId,
        record: RecordId,
        after: Option<RecordId>,
        limit: u32,
    },
    /// At most `limit` wake rows of the segment due at `at`.
    Due {
        segment: SegmentId,
        at: Timestamp,
        limit: u32,
    },
}

/// Whether a batch is of the current version of its segment: a batch of a superseded version
/// writes nothing (SEG-025).
pub fn batch_current(catalog: &[Segment], batch: &Batch) -> bool {
    catalog
        .iter()
        .any(|s| s.id == batch.segment && s.version == batch.version)
}

/// What a batch loads: one more than a batch, so that it knows whether another follows.
pub fn batch_load(segment: &Segment, batch: &Batch, now: Timestamp) -> BatchLoad {
    let limit = BATCH as u32 + 1;
    match &batch.scope {
        BatchScope::All => BatchLoad::Page {
            object: segment.subject.clone(),
            after: batch.after,
            limit,
        },
        BatchScope::Referencing { field, record } => BatchLoad::Referencing {
            object: segment.subject.clone(),
            field: field.clone(),
            record: *record,
            after: batch.after,
            limit,
        },
        BatchScope::Due => BatchLoad::Due {
            segment: segment.id,
            at: now,
            limit,
        },
    }
}

/// The persons of one batch (CT-297) from what [`batch_load`] loaded: at most [`BATCH`] subject
/// records (or due wake rows), and the batch that follows it, or its removal when this is the
/// last. A batch that is not current (see [`batch_current`]) evaluates nobody; its caller writes
/// nothing for it (SEG-025), since the row of its scope belongs to the newer version.
pub fn batch_targets(
    catalog: &[Segment],
    batch: &Batch,
    records: &[Arc<Record>],
    due: &[Wake],
    now: Timestamp,
) -> (Vec<Target>, BatchChange) {
    let remove = BatchChange::Remove {
        segment: batch.segment,
        key: batch.scope.key(),
    };
    let Some(segment) = catalog
        .iter()
        .find(|s| s.id == batch.segment && s.version == batch.version)
    else {
        return (Vec::new(), remove);
    };
    let mut out = Vec::new();
    match &batch.scope {
        BatchScope::Due => {
            for wake in due.iter().take(BATCH) {
                out.push(Target {
                    segment: segment.id,
                    person: wake.person.clone(),
                    locate: Locate::Principal,
                });
            }
            // Due rows are rewritten by the decision, so the next batch starts from the top again.
            let next = if due.len() > BATCH {
                BatchChange::Put(Batch {
                    at: now,
                    ..batch.clone()
                })
            } else {
                remove
            };
            (out, next)
        }
        BatchScope::All | BatchScope::Referencing { .. } => {
            let mut last = None;
            for record in records.iter().take(BATCH) {
                last = Some(record.id);
                if let Some(person) = person_of(record, &segment.principal_field) {
                    out.push(Target {
                        segment: segment.id,
                        person,
                        locate: Locate::Id(record.id),
                    });
                }
            }
            let next = if records.len() > BATCH {
                BatchChange::Put(Batch {
                    after: last,
                    at: now,
                    ..batch.clone()
                })
            } else {
                remove
            };
            (out, next)
        }
    }
}

/// The continuation a refresh job starts at `now` (SEG-018, SEG-019, SEG-1545): every person for
/// a time-dependent predicate that is not indexable, the persons whose wake time has passed for
/// one that is, and nothing for a predicate that does not read the time (SEG-020). A full
/// refresh does not restart a continuation that already evaluates every person.
pub fn start_refresh(segment: &Segment, pending: &[Batch], now: Timestamp) -> Option<Batch> {
    if !segment.time_dependent {
        return None;
    }
    let scope = if segment.indexable {
        BatchScope::Due
    } else {
        BatchScope::All
    };
    let running = pending.iter().any(|b| {
        b.segment == segment.id && b.version == segment.version && b.scope == BatchScope::All
    });
    if running {
        return None;
    }
    Some(Batch {
        segment: segment.id,
        scope,
        version: segment.version,
        after: None,
        reason: SegmentReason::Refresh,
        hop: 0,
        at: now,
    })
}

/// What a refresh for which the daily job quota has no room writes: the time it was skipped, on
/// the segment, and nothing else; no commit is refused for it (SEG-1544).
pub fn skip_refresh(segment: &Segment, now: Timestamp) -> SegmentWrite {
    SegmentWrite {
        refresh_skipped: vec![(segment.id, now)],
        ..SegmentWrite::default()
    }
}

/// The erasure of a person (SEG-036 to SEG-038, D-5031): every membership of the person removed,
/// with one exit event for each, reason erasure, naming the person in the sealed form the caller
/// gives; the wake rows removed; and the person marked erased, so that no later decision enters
/// them.
pub fn erase(
    memberships: &[Membership],
    wakes: &[Wake],
    person: &PrincipalId,
    sealed: &PrincipalId,
    occasion: &Occasion,
) -> (SegmentWrite, Vec<SegmentMembershipEvent>) {
    let mut write = SegmentWrite::default();
    let mut events = Vec::new();
    for m in memberships.iter().filter(|m| &m.person == person) {
        write.memberships.push(MembershipChange {
            segment: m.segment,
            person: person.clone(),
            before: Some(m.clone()),
            after: None,
        });
        events.push(SegmentMembershipEvent {
            segment: m.segment,
            person: sealed.clone(),
            direction: SegmentDirection::Exited,
            reason: SegmentReason::Erasure,
            version: m.version,
            revision: occasion.revision,
            hop: occasion.hop + 1,
        });
    }
    for w in wakes.iter().filter(|w| &w.person == person) {
        write.wakes.push(WakeChange::Remove {
            segment: w.segment,
            person: person.clone(),
        });
    }
    write.erased.push(person.clone());
    (write, events)
}

// ---------------------------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------------------------

/// Whether a reader may see the members of a segment at all (SEG-033, D-5029): the reader holds
/// `segments-read`; the segment is not suspended; and the reader may read, without a condition,
/// every object type the predicate reaches beyond the subject type, since the membership of a
/// person shows what the predicate computes from those records. Readability of each subject
/// record is decided per member by [`member_visible`].
pub fn may_read_members(
    tenant: &TenantState,
    reader: &Principal,
    segment: &Segment,
) -> Result<Vec<Check>, DomainError> {
    let mut evidence = Vec::new();
    require_capability(tenant, reader, SEGMENTS_READ, &mut evidence)?;
    if suspended(tenant, segment) {
        evidence.push(Check::fail(
            "segment:suspended",
            "the definer may no longer define this segment",
        ));
        return Err(DomainError::new(
            ErrorKind::Unauthorized,
            format!(
                "segment {} is suspended: its definer may no longer read what it names (D-5028)",
                segment.name
            ),
        )
        .with(evidence));
    }
    let reads = reads_of(&tenant.schema, segment);
    for (object, _) in reads.hops.values() {
        if !holds_power(
            tenant,
            reader,
            Action::Read,
            &Resource::Object(object.clone()),
        ) {
            evidence.push(Check::fail(
                "segment:reach",
                format!("may not read every record of {object}"),
            ));
            return Err(DomainError::new(
                ErrorKind::Unauthorized,
                format!(
                    "the members of {} show what its predicate reads of {object}, which the reader may not read in full",
                    segment.name
                ),
            )
            .with(evidence));
        }
    }
    evidence.push(Check::pass("segment:read", "may read the members"));
    Ok(evidence)
}

/// Whether a reader may be shown one member: the reader may read the person's subject record
/// (KRN-004), and nothing the predicate names or the principal field carries a read rule, which
/// [`definer_problems`] refused at definition and a schema change cannot add without changing the
/// field's definition. A field rule added since is still honoured here: such a member is hidden.
pub fn member_visible(
    lookup: Lookup<'_>,
    reader: &Principal,
    segment: &Segment,
    record: &Record,
    now: Timestamp,
) -> Result<bool, DomainError> {
    if !visible_in(lookup, reader, record, now)? {
        return Ok(false);
    }
    let schema = &lookup.tenant.schema;
    let reads = reads_of(schema, segment);
    let hidden = reads
        .subject_fields
        .iter()
        .chain(std::iter::once(&segment.principal_field))
        .any(|f| {
            schema
                .objects
                .get(&segment.subject)
                .and_then(|o| o.fields.get(f))
                .is_some_and(|def| def.read.is_some())
        });
    Ok(!hidden)
}

/// One page of the members of a segment as a reader is shown it (CT-292): of `rows` (a page of
/// memberships in principal order from the store), those whose subject records the reader may
/// read; `state` covers the subject record of each (`Need::ByField` of the principal field).
/// The next cursor is the last person examined when the page was full.
pub fn members_page(
    state: &State,
    reader: &Principal,
    segment: &Segment,
    rows: &[Membership],
    limit: usize,
    now: Timestamp,
) -> Result<(Vec<SegmentMember>, Option<PrincipalId>), DomainError> {
    let tenant = state
        .tenant
        .as_deref()
        .ok_or_else(|| DomainError::new(ErrorKind::TenantNotFound, "no such tenant"))?;
    may_read_members(tenant, reader, segment)?;
    let limit = limit.max(1);
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let mut shown = Vec::new();
    for m in rows.iter().take(limit) {
        let target = Target {
            segment: segment.id,
            person: m.person.clone(),
            locate: Locate::Principal,
        };
        let Some(record) = locate(segment, &lookup, &target)? else {
            continue;
        };
        if member_visible(lookup, reader, segment, record, now)? {
            shown.push(SegmentMember {
                person: m.person.clone(),
                since: m.since,
                version: m.version,
            });
        }
    }
    let next = if rows.len() > limit {
        rows.get(limit - 1).map(|m| m.person.clone())
    } else {
        None
    };
    Ok((shown, next))
}

/// How a count is answered (SEG-1541, D-6555).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CountPlan {
    /// From the count kept with the segment: the reader may read every subject record.
    Kept,
    /// By examining the memberships, at most [`COUNT_SCAN_BOUND`] of them.
    Examine,
}

/// How the members of a segment are counted for a reader, after [`may_read_members`].
pub fn count_plan(tenant: &TenantState, reader: &Principal, segment: &Segment) -> CountPlan {
    let reads = reads_of(&tenant.schema, segment);
    let hidden_field = reads
        .subject_fields
        .iter()
        .chain(std::iter::once(&segment.principal_field))
        .any(|f| {
            tenant
                .schema
                .objects
                .get(&segment.subject)
                .and_then(|o| o.fields.get(f))
                .is_some_and(|def| def.read.is_some())
        });
    if !hidden_field
        && holds_power(
            tenant,
            reader,
            Action::Read,
            &Resource::Object(segment.subject.clone()),
        )
    {
        CountPlan::Kept
    } else {
        CountPlan::Examine
    }
}

/// Count the members of a segment a reader may read (CT-814, SEG-1541): the kept count when the
/// reader reads every subject record; otherwise the visible members among `rows` (every
/// membership, which the store returns up to one more than the bound), refused with
/// `LimitExceeded` (`scan-bound-exceeded`) when there are more than [`COUNT_SCAN_BOUND`].
pub fn count(
    state: &State,
    reader: &Principal,
    segment: &Segment,
    rows: Option<&[Membership]>,
    now: Timestamp,
) -> Result<SegmentCount, DomainError> {
    let tenant = state
        .tenant
        .as_deref()
        .ok_or_else(|| DomainError::new(ErrorKind::TenantNotFound, "no such tenant"))?;
    may_read_members(tenant, reader, segment)?;
    if count_plan(tenant, reader, segment) == CountPlan::Kept {
        return Ok(SegmentCount {
            segment: segment.id,
            count: segment.member_count,
            examined: 0,
        });
    }
    let rows = rows.unwrap_or(&[]);
    if rows.len() > COUNT_SCAN_BOUND {
        return Err(DomainError::new(
            ErrorKind::LimitExceeded,
            format!(
                "scan-bound-exceeded: counting {} for this reader examines more than {COUNT_SCAN_BOUND} memberships",
                segment.name
            ),
        )
        .with(vec![Check::fail(
            format!("scan-bound:segment:{}", segment.id),
            "the reader may not read every subject record",
        )]));
    }
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let mut visible = 0u64;
    for m in rows {
        let target = Target {
            segment: segment.id,
            person: m.person.clone(),
            locate: Locate::Principal,
        };
        if let Some(record) = locate(segment, &lookup, &target)? {
            if member_visible(lookup, reader, segment, record, now)? {
                visible += 1;
            }
        }
    }
    Ok(SegmentCount {
        segment: segment.id,
        count: visible,
        examined: rows.len() as u32,
    })
}

/// The segments marked visible to members that a signed-in person belongs to (CT-813,
/// SEG-1540), from the person's memberships. Every signed-in person holds `segments-read-own`
/// (D-6555); an anonymous caller is refused.
pub fn my_segments(
    catalog: &[Segment],
    caller: &Principal,
    memberships: &[Membership],
) -> Result<Vec<SegmentSummary>, DomainError> {
    let Principal::User(person) = caller else {
        return Err(DomainError::new(
            ErrorKind::Unauthorized,
            format!("{SEGMENTS_READ_OWN} is held by signed-in persons"),
        ));
    };
    let mut out = Vec::new();
    for m in memberships.iter().filter(|m| &m.person == person) {
        if let Some(segment) = catalog
            .iter()
            .find(|s| s.id == m.segment && s.visible_to_members)
        {
            out.push(SegmentSummary {
                segment: segment.id,
                name: segment.name.clone(),
                since: m.since,
            });
        }
    }
    Ok(out)
}

/// A draft predicate as test and preview take it (CT-293, CT-815), checked as a definition is
/// (SEG-003 to SEG-005).
pub fn draft(
    tenant: &TenantState,
    caller: &Principal,
    subject: &ObjectTypeId,
    principal_field: &FieldId,
    predicate: &Expr,
) -> Result<(Segment, Reads), DomainError> {
    let mut evidence = Vec::new();
    require_capability(tenant, caller, SEGMENTS_DEFINE, &mut evidence)?;
    let problems = principal_field_problems(&tenant.schema, subject, principal_field);
    if !problems.is_empty() {
        return Err(invalid(problems.join("; "), &evidence));
    }
    let analysis = predicate_check(
        tenant,
        caller,
        subject,
        principal_field,
        predicate,
        &mut evidence,
    )?;
    let segment = Segment {
        id: segment_id_of([0; 16]),
        name: "draft".to_string(),
        subject: subject.clone(),
        predicate: predicate.clone(),
        version: 0,
        time_dependent: analysis.reads.time,
        indexable: analysis.indexable,
        refresh_seconds: None,
        defined_by: None,
        principal_field: principal_field.clone(),
        visible_to_members: false,
        schema_version: tenant.schema.version,
        member_count: 0,
        refresh_skipped_at: None,
    };
    Ok((segment, analysis.reads))
}

/// Test a draft predicate against one person (CT-293, SEG-035): no state changes; the value and
/// the names of the fields it read; only for a person whose subject record the caller may read,
/// any other being `UnknownRecord`, as a person without one is.
pub fn test_predicate(
    state: &State,
    caller: &Principal,
    draft_segment: &Segment,
    reads: &Reads,
    person: &PrincipalId,
    now: Timestamp,
) -> Result<SegmentTestResult, DomainError> {
    let tenant = state
        .tenant
        .as_deref()
        .ok_or_else(|| DomainError::new(ErrorKind::TenantNotFound, "no such tenant"))?;
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let target = Target {
        segment: draft_segment.id,
        person: person.clone(),
        locate: Locate::Principal,
    };
    let record = locate(draft_segment, &lookup, &target)?;
    let Some(record) = record.filter(|r| visible_in(lookup, caller, r, now).unwrap_or(false))
    else {
        return Err(DomainError::new(
            ErrorKind::UnknownRecord,
            format!(
                "no {} record of {person} that the caller may read",
                draft_segment.subject
            ),
        ));
    };
    let outcome = evaluate(draft_segment, state, record, person, now)?;
    Ok(SegmentTestResult {
        member: outcome.member,
        reads: reads.field_names(&draft_segment.subject),
    })
}

/// Preview a draft predicate over at most [`MAX_PREVIEW`] persons after a cursor (CT-815,
/// SEG-1542): no state changes; only subject records the caller may read are evaluated and
/// counted; `records` is the page the store loaded (`Need::PageOf` of `limit` + 1) and `state`
/// covers the records its references reach. The next cursor is the last record examined when
/// more follow.
pub fn preview(
    state: &State,
    caller: &Principal,
    draft_segment: &Segment,
    records: &[Arc<Record>],
    limit: u32,
    now: Timestamp,
) -> Result<(SegmentPreview, Option<RecordId>), DomainError> {
    let tenant = state
        .tenant
        .as_deref()
        .ok_or_else(|| DomainError::new(ErrorKind::TenantNotFound, "no such tenant"))?;
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let limit = limit.clamp(1, MAX_PREVIEW) as usize;
    let mut matched = 0u32;
    let mut examined = 0u32;
    let mut last = None;
    for record in records.iter().take(limit) {
        last = Some(record.id);
        if !visible_in(lookup, caller, record, now)? {
            continue;
        }
        let Some(person) = person_of(record, &draft_segment.principal_field) else {
            continue;
        };
        examined += 1;
        match evaluate(draft_segment, state, record, &person, now) {
            Ok(outcome) if outcome.member => matched += 1,
            Ok(_) => {}
            Err(error) if error.kind == ErrorKind::Evaluation => {}
            Err(error) => return Err(error),
        }
    }
    let next = if records.len() > limit { last } else { None };
    Ok((SegmentPreview { matched, examined }, next))
}

/// The hop targets every record of `records` needs for a draft or a batch: what [`needs_for`]
/// names for targets located by id.
pub fn record_needs(segment: &Segment, state: &State, records: &[Arc<Record>]) -> Vec<Need> {
    let targets: Vec<Target> = records
        .iter()
        .filter_map(|r| {
            person_of(r, &segment.principal_field).map(|person| Target {
                segment: segment.id,
                person,
                locate: Locate::Id(r.id),
            })
        })
        .collect();
    needs_for(std::slice::from_ref(segment), state, &targets)
}

/// Every person of a subject type that satisfies a predicate, evaluated from scratch on a
/// complete state: what membership must equal (SEG-011). For tests and replay.
pub fn evaluate_all(
    segment: &Segment,
    state: &State,
    now: Timestamp,
    erased: &BTreeSet<PrincipalId>,
) -> BTreeSet<PrincipalId> {
    let Some(tenant) = state.tenant.as_deref() else {
        return BTreeSet::new();
    };
    let mut out = BTreeSet::new();
    if let Some(records) = tenant.records.get(&segment.subject) {
        for record in records.values() {
            let Some(person) = person_of(record, &segment.principal_field) else {
                continue;
            };
            if erased.contains(&person) {
                continue;
            }
            if let Ok(outcome) = evaluate(segment, state, record, &person, now) {
                if outcome.member {
                    out.insert(person);
                }
            }
        }
    }
    out
}
