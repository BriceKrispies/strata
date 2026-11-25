//! Erasure of one data subject from the records and the memberships of a tenant (PRV-353,
//! PRV-1586, D-350, D-6584, D-6585).
//!
//! One decision erases at most [`BATCH`] records and is repeated until its output says there is
//! nothing more to do. The decision is idempotent: a record already erased is not changed again,
//! so a repeat after the last batch changes nothing and says so.
//!
//! What a decision does:
//!
//! - **Records.** For every object type with a field that declares personal data, the records
//!   whose data subject is the principal. A field names its subject as the creator of the record
//!   (`created-by`) or as another field of the record that holds the principal id as text; a
//!   field that holds a reference to the record of a person cannot be matched to a principal here
//!   and is reported, not skipped silently. An optional personal field is removed; a required one
//!   is replaced by the empty value of its type, so the record still fits its own declaration; a
//!   required reference or a required language has no empty value, and its record is held and
//!   reported. When the principal is the creator of a record that has a field declared personal
//!   to its creator, `created-by` is cleared as well. The version of every changed record
//!   is raised.
//! - **Events.** Each changed record yields a `record-updated` event whose `before` image has the
//!   erased fields (and `created-by`) removed, so the stream carries no personal value of the
//!   subject. Events committed before the erasure are not rewritten (see the receipt of the
//!   backend, which names them).
//! - **Memberships.** The principal's roles, its place in every team and every invitation it
//!   issued and nobody has redeemed or revoked (revoked, by the acting owner). The address of an
//!   invitation the principal redeemed is cleared. The sessions of the principal end with the
//!   commit.
//! - **Held operations.** A hold the subject filed or that acts for the subject is removed, with the
//!   change it froze, and the subject is cleared from a decision it made (D-8400).
//! - **Legal holds.** A record of a type the intent names in `held-objects` is not changed: the
//!   decision lists it in `not-erased` as held, so a hold on a record type suspends the erasure of
//!   what it covers and the rest of the subject is erased (PRV-355). A hold on the subject itself
//!   refuses the whole erasure before any decision (the backend, `subject_rights`).
//! - **Invitations addressed to the subject.** The kernel holds no address of a person, so the
//!   shell names the pending invitations addressed to the subject's verified address in
//!   `addressed-invitations`: each is revoked and its address cleared (WG-5117). The intent carries
//!   the ids, never the address.
//!
//! Needs are planned: records that name the principal through a text field come by the index
//! (`by-field`); records of a type whose subject is the creator come in pages of [`BATCH`] by
//! record id, one type at a time, through the cursor the output returns and the intent carries
//! back.

use crate::intent::{
    Check, CheckExt, Context, DeltaOp, DomainError, DomainErrorExt, Effect, ErrorKind, EventBody,
    Intent, Output,
};
use crate::rules::{assigned_roles, roles_of, Lookup};
use crate::schema::TenantSchemaExt;
use crate::state::{Need, Principal, Record, RecordExt};
use crate::types::ValueExt;
use crate::types::{
    owner_role, DataSubject, FieldDef, FieldId, FieldType, FieldTypeExt, InvitationId,
    ObjectTypeDef, ObjectTypeId, PrincipalId, RecordId, TeamDef, TenantSchema, Timestamp, Value,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub use strata_model_core::types::ErasedRecord;

/// The most records one decision changes, and the most one page of a creator scan holds.
pub const BATCH: usize = 100;

/// The result of one decision, to be merged into the transition.
pub struct Erasure {
    pub ops: Vec<DeltaOp>,
    pub events: Vec<EventBody>,
    pub effects: Vec<Effect>,
    pub evidence: Vec<Check>,
    pub output: Output,
}

/// Object types with a field declared personal, in id order, each with the text fields that name
/// the subject of some personal field and whether some personal field has the creator for its
/// subject. The export of a subject (`subject_export`) enumerates through the same declarations,
/// so the two never disagree about what belongs to a subject.
pub(crate) struct Declared<'a> {
    pub(crate) object: &'a ObjectTypeDef,
    pub(crate) links: BTreeSet<FieldId>,
    pub(crate) unresolved: BTreeSet<FieldId>,
    pub(crate) creator: bool,
}

pub(crate) fn declared(schema: &TenantSchema) -> Vec<Declared<'_>> {
    let mut out = Vec::new();
    for object in schema.objects.values() {
        let mut found = Declared {
            object,
            links: BTreeSet::new(),
            unresolved: BTreeSet::new(),
            creator: false,
        };
        for field in object.fields.values() {
            let Some(personal) = &field.personal else {
                continue;
            };
            match &personal.subject {
                DataSubject::Creator => found.creator = true,
                DataSubject::Field { field: link } => match object.fields.get(link) {
                    Some(def) if holds_text(&def.ty) => {
                        found.links.insert(link.clone());
                    }
                    _ => {
                        found.unresolved.insert(link.clone());
                    }
                },
            }
        }
        if found.creator || !found.links.is_empty() || !found.unresolved.is_empty() {
            out.push(found);
        }
    }
    out
}

/// A field that holds a principal id: text, or a list of text.
fn holds_text(ty: &FieldType) -> bool {
    match ty {
        FieldType::Str => true,
        FieldType::List(inner) => matches!(inner.as_ref(), FieldType::Str),
        _ => false,
    }
}

/// The first object type, in id order, whose records are scanned for a creator, from the one the
/// cursor names on.
pub(crate) fn scan_from<'a>(
    declared: &'a [Declared<'a>],
    resume_object: Option<&ObjectTypeId>,
) -> Option<&'a Declared<'a>> {
    declared
        .iter()
        .find(|d| d.creator && resume_object.is_none_or(|from| d.object.id >= *from))
}

pub(crate) fn page_need(object: &ObjectTypeId, after: Option<RecordId>) -> Need {
    Need::PageOf {
        object: object.clone(),
        after,
        limit: BATCH as u32,
    }
}

/// The state one decision of `intent` is expected to need.
pub fn needs(schema: &TenantSchema, intent: &Intent) -> Vec<Need> {
    let Intent::EraseSubject {
        subject,
        resume_object,
        resume_after,
        ..
    } = intent
    else {
        return Vec::new();
    };
    let declared = declared(schema);
    let mut out = BTreeSet::new();
    for d in &declared {
        for link in &d.links {
            out.insert(Need::ByField {
                object: d.object.id.clone(),
                field: link.clone(),
                value: Value::Str(subject.as_str().to_string()),
            });
        }
    }
    // The page of the type the cursor names, and the first page of each type after it: a decision
    // that changes nothing in one type goes on to the next, so that a repeat of a finished erasure
    // ends in one decision (PRV-2461).
    if let Some(first) = scan_from(&declared, resume_object.as_ref()) {
        let after = if resume_object.as_ref() == Some(&first.object.id) {
            *resume_after
        } else {
            None
        };
        out.insert(page_need(&first.object.id, after));
        for d in declared
            .iter()
            .filter(|d| d.creator && d.object.id > first.object.id)
        {
            out.insert(page_need(&d.object.id, None));
        }
    }
    out.into_iter().collect()
}

fn holds(value: &Value, subject: &PrincipalId) -> bool {
    match value {
        Value::Str(text) => text == subject.as_str(),
        Value::List(items) => items.iter().any(|item| holds(item, subject)),
        _ => false,
    }
}

/// The value a required personal field takes when it is erased, or `None` when its type has none.
fn empty_of(ty: &FieldType) -> Option<Value> {
    match ty {
        FieldType::Str | FieldType::File { .. } => Some(Value::Str(String::new())),
        FieldType::Int => Some(Value::Int(0)),
        FieldType::Bool => Some(Value::Bool(false)),
        FieldType::Time => Some(Value::Time(0)),
        FieldType::List(_) => Some(Value::List(Vec::new())),
        FieldType::Ref { .. } | FieldType::Language => None,
    }
}

/// Whether `field` of `record` belongs to the subject.
pub(crate) fn is_subject_of(field: &FieldDef, record: &Record, subject: &PrincipalId) -> bool {
    match field.personal.as_ref().map(|p| &p.subject) {
        Some(DataSubject::Creator) => record.created_by.as_ref() == Some(subject),
        Some(DataSubject::Field { field: link }) => holds(record.get(link), subject),
        None => false,
    }
}

/// The images of a record whose subject's data was erased: the stored record after, and the
/// event image before (the erased fields and `created-by` removed).
struct Erased {
    after: Record,
    before: Record,
    fields: Vec<FieldId>,
}

/// Erase the subject from one record. Returns the change, if the record holds anything of the
/// subject that can be erased, and a note for each personal field that cannot be (a required
/// reference or language has no empty value to put in its place).
fn erase_record(
    object: &ObjectTypeDef,
    record: &Record,
    subject: &PrincipalId,
    now: Timestamp,
) -> (Option<Erased>, Vec<String>) {
    let mut after = record.clone();
    let mut before = record.clone();
    let mut fields = Vec::new();
    let mut notes = Vec::new();
    let mut creator_link = false;
    for (id, field) in &object.fields {
        if !is_subject_of(field, record, subject) {
            continue;
        }
        if matches!(
            field.personal.as_ref().map(|p| &p.subject),
            Some(DataSubject::Creator)
        ) {
            creator_link = true;
        }
        let current = record.get(id);
        if field.required {
            match empty_of(&field.ty) {
                Some(empty) => {
                    if *current != empty {
                        after.fields.insert(id.clone(), empty);
                        before.fields.remove(id);
                        fields.push(id.clone());
                    }
                }
                None if !current.is_null() => notes.push(format!(
                    "held {}.{id} of record {}: a required {} has no empty value",
                    object.id,
                    record.id,
                    field.ty.name()
                )),
                None => {}
            }
        } else if after.fields.remove(id).is_some() {
            before.fields.remove(id);
            fields.push(id.clone());
        }
    }
    // The link to the creator goes with the creator's fields, and stays while one of them is held.
    let unlinked = creator_link && notes.is_empty() && record.created_by.is_some();
    if unlinked {
        after.created_by = None;
        before.created_by = None;
    }
    if fields.is_empty() && !unlinked {
        return (None, notes);
    }
    after.version += 1;
    after.updated_at = now;
    (
        Some(Erased {
            after,
            before,
            fields,
        }),
        notes,
    )
}

/// The principals who hold `owner` in effect, directly or through a team.
fn owners(
    members: &BTreeMap<PrincipalId, BTreeSet<crate::types::RoleId>>,
    schema: &TenantSchema,
) -> BTreeSet<PrincipalId> {
    let mut candidates: BTreeSet<&PrincipalId> = members.keys().collect();
    for team in schema.all_teams() {
        candidates.extend(team.members.iter());
    }
    candidates
        .into_iter()
        .filter(|p| assigned_roles(members, schema, p).contains(&owner_role()))
        .cloned()
        .collect()
}

fn refuse(kind: ErrorKind, rule: &str, message: impl Into<String>) -> DomainError {
    let message = message.into();
    DomainError::new(kind, message.clone()).with(vec![Check::fail(rule, message)])
}

/// Decide one batch of the erasure of `subject`.
pub fn decide(
    lookup: Lookup<'_>,
    ctx: &Context,
    subject: &PrincipalId,
    resume_object: Option<&ObjectTypeId>,
    resume_after: Option<RecordId>,
    held_objects: &[ObjectTypeId],
    addressed_invitations: &[InvitationId],
) -> Result<Erasure, DomainError> {
    let tenant = lookup.tenant;
    let schema: &TenantSchema = &tenant.schema;
    let mut evidence = Vec::new();

    // Only an owner of the tenant erases a person (the contract's authorisation).
    let Principal::User(actor) = &ctx.principal else {
        return Err(refuse(
            ErrorKind::Unauthorized,
            "erasure:owner_only",
            "a person who owns the tenant erases a subject",
        ));
    };
    if !roles_of(tenant, &ctx.principal).contains(&owner_role()) {
        return Err(refuse(
            ErrorKind::Unauthorized,
            "erasure:owner_only",
            "only an owner of the tenant erases a subject",
        ));
    }
    evidence.push(Check::pass(
        "erasure:owner_only",
        "the actor owns the tenant",
    ));

    // Memberships: the roles, the teams and the invitations.
    let mut ops = Vec::new();
    let mut events = Vec::new();
    let member_removed = tenant.members.contains_key(subject);
    let mut members = tenant.members.clone();
    members.remove(subject);
    let mut next_schema = schema.clone();
    let mut changed_teams = Vec::new();
    for team in schema.all_teams() {
        if team.members.contains(subject) {
            let mut def: TeamDef = team.clone();
            def.members.remove(subject);
            changed_teams.push(def.clone());
            next_schema.put_team(def);
        }
    }
    let mut touched_invitations = Vec::new();
    for offer in schema.all_invitations() {
        let issued_open = offer.created_by == *subject
            && offer.redeemed_by.is_none()
            && offer.revoked_by.is_none();
        let redeemed = offer.redeemed_by.as_ref() == Some(subject) && !offer.address.is_empty();
        // An invitation the shell found addressed to the subject, still pending (WG-5117): it is
        // revoked and loses its address, so it cannot be redeemed and holds nothing of the person.
        let addressed_open = addressed_invitations.contains(&offer.id)
            && offer.redeemed_by.is_none()
            && offer.revoked_by.is_none();
        if issued_open || redeemed || addressed_open {
            if let Some(stored) = next_schema.invitation_mut(&offer.id) {
                if issued_open || addressed_open {
                    stored.revoked_by = Some(actor.clone());
                }
                if redeemed || addressed_open {
                    stored.address = String::new();
                }
            }
            touched_invitations.push(offer.id.clone());
        }
    }
    // A hold names its requester and holds the input of the change, which can be personal.
    let holds_changed = match next_schema.holds.as_mut() {
        Some(holds) => {
            let before = holds.clone();
            holds.retain(|_, held| {
                held.requester != *subject && held.on_behalf_of.as_ref() != Some(subject)
            });
            for held in holds.values_mut() {
                if held.decided_by.as_ref() == Some(subject) {
                    held.decided_by = None;
                }
            }
            if holds.is_empty() {
                next_schema.holds = None;
            }
            before != next_schema.holds.clone().unwrap_or_default()
        }
        None => false,
    };
    let memberships_changed = member_removed
        || !changed_teams.is_empty()
        || !touched_invitations.is_empty()
        || holds_changed;
    if memberships_changed {
        let before = owners(&tenant.members, schema).len();
        if before > 0 && owners(&members, &next_schema).is_empty() {
            return Err(refuse(
                ErrorKind::InvalidIntent,
                "erasure:an_owner_remains",
                "a tenant keeps at least one owner: hand the ownership on before erasing the last owner",
            ));
        }
        evidence.push(Check::pass(
            "erasure:an_owner_remains",
            "an owner of the tenant remains",
        ));
    }
    if member_removed {
        ops.push(DeltaOp::PutMember {
            principal: subject.clone(),
            roles: BTreeSet::new(),
        });
    }
    if !changed_teams.is_empty() || !touched_invitations.is_empty() || holds_changed {
        ops.push(DeltaOp::PutSchema(Arc::new(next_schema)));
    }
    if member_removed || !changed_teams.is_empty() {
        events.push(EventBody::MemberChanged {
            principal: subject.clone(),
            roles: BTreeSet::new(),
        });
    }
    for team in &changed_teams {
        events.push(EventBody::MemberChanged {
            principal: PrincipalId::new(format!("team:{}", team.id))
                .expect("a team id is a valid principal token"),
            roles: team.roles.clone(),
        });
    }
    let teams = changed_teams.iter().map(|t| t.id.clone()).collect();

    // Records.
    let declared = declared(schema);
    let mut not_erased: Vec<String> = Vec::new();
    for d in &declared {
        for link in &d.unresolved {
            not_erased.push(format!(
                "unresolved {}.{link}: the field that names the subject is not text, so the principal cannot be matched to it",
                d.object.id
            ));
        }
    }
    let now = ctx.now;
    let mut plan: Vec<(ObjectTypeId, Erased)> = Vec::new();
    let mut held: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<(ObjectTypeId, RecordId)> = BTreeSet::new();
    let mut take = |d: &Declared<'_>,
                    record: &Record,
                    plan: &mut Vec<(ObjectTypeId, Erased)>,
                    held: &mut BTreeSet<String>| {
        if !seen.insert((d.object.id.clone(), record.id)) {
            return;
        }
        // A legal hold on the record type suspends the erasure of its records (PRV-355). Only a
        // record that holds something of the subject is reported.
        if held_objects.contains(&d.object.id) {
            if d.object
                .fields
                .values()
                .any(|field| is_subject_of(field, record, subject))
            {
                held.insert(format!(
                    "held {} record {}: a legal hold on the record type suspends its erasure (PRV-355)",
                    d.object.id, record.id
                ));
            }
            return;
        }
        let (erased, notes) = erase_record(d.object, record, subject, now);
        held.extend(notes);
        if let Some(erased) = erased {
            plan.push((d.object.id.clone(), erased));
        }
    };

    // Phase one: records that name the subject through a text field, by the index.
    let value = Value::Str(subject.as_str().to_string());
    for d in &declared {
        for link in &d.links {
            for record in lookup.by_field(&d.object.id, link, &value)? {
                take(d, record, &mut plan, &mut held);
            }
        }
    }
    let mut more = false;
    let mut next_object = None;
    let mut next_after = None;
    let phase_one_work = !plan.is_empty();
    if plan.len() > BATCH {
        plan.truncate(BATCH);
        more = true;
        next_object = resume_object.cloned();
        next_after = resume_after;
    } else if phase_one_work {
        // Records created by the subject are scanned in the calls that follow.
        if let Some(d) = scan_from(&declared, resume_object) {
            more = true;
            next_object = Some(d.object.id.clone());
            next_after = if resume_object == Some(&d.object.id) {
                resume_after
            } else {
                None
            };
        }
    } else {
        // Phase two: one page of records of one type, for the ones the subject created. A type
        // with nothing to change hands over to the next one in the same decision.
        let mut cursor = scan_from(&declared, resume_object);
        while let Some(d) = cursor {
            let after = if resume_object == Some(&d.object.id) {
                resume_after
            } else {
                None
            };
            let page = lookup.page_of(&d.object.id, after, BATCH as u32)?;
            for record in &page {
                if record.created_by.as_ref() == Some(subject) {
                    take(d, record, &mut plan, &mut held);
                }
            }
            let following = declared
                .iter()
                .find(|other| other.creator && other.object.id > d.object.id);
            if page.len() == BATCH {
                more = true;
                next_object = Some(d.object.id.clone());
                next_after = page.last().map(|record| record.id);
                break;
            }
            match following {
                Some(next) if plan.is_empty() => cursor = Some(next),
                Some(next) => {
                    more = true;
                    next_object = Some(next.object.id.clone());
                    next_after = None;
                    break;
                }
                None => break,
            }
        }
    }
    not_erased.extend(held);

    let mut changed = Vec::new();
    for (object, erased) in plan {
        changed.push(ErasedRecord {
            object,
            id: erased.after.id,
            fields: erased.fields,
        });
        let after = Arc::new(erased.after);
        ops.push(DeltaOp::PutRecord(after.clone()));
        events.push(EventBody::RecordUpdated {
            before: Arc::new(erased.before),
            after,
            transitions: None,
        });
    }
    evidence.push(Check::pass(
        "erasure:batch",
        format!(
            "{} records changed, at most {BATCH} in one decision; more to do: {more}",
            changed.len()
        ),
    ));
    Ok(Erasure {
        ops,
        events,
        effects: vec![Effect::RevokeSessions {
            principal: subject.clone(),
        }],
        evidence,
        output: Output::Erased {
            changed,
            member_removed,
            teams,
            invitations: touched_invitations,
            not_erased,
            more,
            next_object,
            next_after,
        },
    })
}
