//! Sealed personal fields: the store seals on write and opens on load (D-350, D-6584, D-6585,
//! PRV-358, PRV-360, PRV-361, PRV-1588, and the decisions of the sealing design, D-7720 to
//! D-7728).
//!
//! The kernel decides over plain values: its rules, unique checks, searches and segments read a
//! personal field as any other, and the Lean model and the generated-function comparison are over
//! the same plain values as before. [`SealedStore`] wraps an adapter and is the one place where
//! values are sealed and opened:
//!
//! - **On write.** Every value of a field that declares personal data (`FieldDef::personal`) is
//!   replaced, in the records of the delta, in the record images of the events and in the field
//!   values of a `create-record` or `update-record` intent in the journal, by a token: the value
//!   sealed under the key of its data subject (the creator of the record, or the principal a text
//!   field names). Snapshots copy the stored records, so they hold the tokens too. A field whose
//!   subject is several principals is sealed under each key in turn, so that destroying any one of
//!   them makes it unreadable.
//! - **Hashes.** The journal entry's `delta_hash` and `events_hash` are recomputed over the sealed
//!   delta and events (D-350): a destroyed key changes no stored byte, so no hash breaks. Sealing is
//!   deterministic (the nonce is derived from the key, the binding and the value), so a replay that
//!   decides again and seals again with the same keys reproduces the hash
//!   ([`SealedStore::journal_hashes`]). A tenant whose schema declares no personal field is written
//!   exactly as before.
//! - **The record index.** A token carries the blind hashes of its plain value when the field is
//!   searchable (a keyed hash under the tenant key, D-6585); the index holds those and nothing
//!   else of a sealed value. A lookup of a personal field is translated: a searchable one looks up
//!   the blind hash (and the plain value, for rows written before sealing), any other is answered
//!   by reading the type and comparing the opened values, and an index page over a personal field
//!   likewise. The answer is cut to exactly what the kernel asked for.
//! - **On load.** Every token is opened. A token whose key was destroyed, or which does not open
//!   for the tenant and the field it is read as (another tenant's key, a value moved between
//!   fields), stays as it is: the token is the explicit "erased" value, and a read or a replay
//!   goes on with it (PRV-361, `sk-sealed`).
//! - **The audit trail.** Members of an audit line that name a person are sealed under that
//!   person's key before the line is stored, so the chain covers the sealed form (PRV-360,
//!   [`crate::audit_chain`]); lines are opened when listed.
//!
//! The keys and the cryptography are the [`FieldCipher`]'s: this module knows the token format and
//! where values are, not how they are sealed.

use crate::audit_chain::ChainedAuditEntry;
use crate::jobs::{
    ExceptionKind, ExceptionRecord, Job, JobChange, JobReport, Lease, NewJob, Reported, RunRecord,
    RunState, Settled, StepRecord, WorkerWrites,
};
use crate::schedules::{AutomationPurged, AutomationRetention, Schedule, ScheduleSet};
use crate::segments::SegmentCommit;
use crate::{
    partial_state, Account, AuditEntry, Capabilities, Commit, FederatedIdentity, Generation,
    Health, IdempotentResult, JournalEntry, Loaded, OidcFlow, OutboxEntry, OutboxFault,
    PasskeyChallenge, PasskeyRecord, PendingSignIn, PlatformSetting, PlatformWrite,
    PrincipalRemoval, Receipt, RecoveryRequest, SamlRequest, SessionRecord, SignInLink,
    SnapshotInfo, StorageMigration, Store, StoreError, StoreResult, TenantSummary, ThrottleRule,
    ThrottleSettle, ThrottleVerdict, TotpRecord,
};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::segments as seg;
use strata_kernel::{
    canon, DataSubject, DeltaOp, DomainEvent, EventBody, FieldId, Intent, Need, ObjectTypeId,
    PersonalDeclaration, Principal, PrincipalId, Record, RecordId, SessionId, SettingId, State,
    StateDelta, TenantId, TenantSchema, TenantState, Timestamp, Value, WorkflowId,
};

/// The start of every sealed value.
pub const TOKEN_PREFIX: &str = "sealed:v1:";

/// The start of the value a lookup of a blind hash names (never stored in a record).
pub const BLIND_PREFIX: &str = "blind:v1:";

/// The most layers a value is opened through: a field whose subject is more principals than this
/// stays sealed.
const MAX_LAYERS: usize = 16;

/// The scope of the members of an audit line, in place of an object type.
pub const AUDIT_SCOPE: &str = "audit";

/// Members of an audit line that name a person: each is sealed under the key of the person it
/// names, when it holds a principal id.
pub const AUDIT_SUBJECT_MEMBERS: &[&str] = &["principal", "actor", "subject", "by", "owner"];

/// Members of an audit line that hold a person's data: sealed under the key of the person the
/// first subject member names, when one does.
pub const AUDIT_PERSONAL_MEMBERS: &[&str] = &[
    "login",
    "email",
    "address",
    "display_name",
    "userName",
    "previous",
    "value",
];

/// The parts of a token: `sealed:v1:<key id>:<blind hashes, comma-separated>:<payload>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token<'a> {
    pub key_id: &'a str,
    /// The blind hashes, comma-separated; empty for a field that is not searchable.
    pub blinds: &'a str,
    pub payload: &'a str,
}

impl<'a> Token<'a> {
    /// The blind hashes one by one.
    pub fn blind_hashes(&self) -> impl Iterator<Item = &'a str> {
        self.blinds.split(',').filter(|hash| !hash.is_empty())
    }
}

/// The parts of a token, or `None` when `text` is not one.
pub fn parse_token(text: &str) -> Option<Token<'_>> {
    let rest = text.strip_prefix(TOKEN_PREFIX)?;
    let mut parts = rest.splitn(3, ':');
    let key_id = parts.next()?;
    let blinds = parts.next()?;
    let payload = parts.next()?;
    if key_id.is_empty() || payload.is_empty() {
        return None;
    }
    Some(Token {
        key_id,
        blinds,
        payload,
    })
}

/// The token of a payload sealed under `key_id` with `blinds`.
pub fn format_token(key_id: &str, blinds: &str, payload: &str) -> String {
    format!("{TOKEN_PREFIX}{key_id}:{blinds}:{payload}")
}

/// Whether a value is a token: sealed, opened or not.
pub fn is_sealed(value: &Value) -> bool {
    matches!(value, Value::Str(text) if parse_token(text).is_some())
}

/// The value a lookup of a blind hash names.
pub fn blind_value(hash: &str) -> Value {
    Value::Str(format!("{BLIND_PREFIX}{hash}"))
}

/// Whether a lookup value names a blind hash.
pub fn is_blind(value: &Value) -> bool {
    matches!(value, Value::Str(text) if text.starts_with(BLIND_PREFIX))
}

/// Whether a stored token answers a lookup of a blind hash.
pub fn blind_matches(stored: &str, wanted: &Value) -> bool {
    let Value::Str(wanted) = wanted else {
        return false;
    };
    let Some(hash) = wanted.strip_prefix(BLIND_PREFIX) else {
        return false;
    };
    parse_token(stored).is_some_and(|token| token.blind_hashes().any(|held| held == hash))
}

/// What the record index holds of a field value: `Some` of the blind lookup values of a token
/// (none for a field that is not searchable), `None` for a value that is not sealed and is
/// indexed as it is (WG-5112).
pub fn index_values(value: &Value) -> Option<Vec<Value>> {
    let Value::Str(text) = value else {
        return None;
    };
    let token = parse_token(text)?;
    Some(token.blind_hashes().map(blind_value).collect())
}

/// What sealing needs of the keys of one tenant for the length of one store call: the subject
/// keys, the tenant key the blind hashes are made under, and the cipher. Plain values cross this
/// boundary as canonical JSON text.
pub trait CipherSession {
    /// Seal `plain` under the key of `subject`, bound to the tenant, the key, `scope` and `name`
    /// (the object type and the field, or the audit scope and a member) and `blinds`. Returns the
    /// key id and the payload. With `create` a subject without a live key gets one; without it,
    /// `None` says the subject has no live key (it was erased, or never had one).
    /// Implements: H-019
    fn seal_one(
        &mut self,
        subject: &PrincipalId,
        scope: &str,
        name: &str,
        plain: &str,
        blinds: &str,
        create: bool,
    ) -> StoreResult<Option<(String, String)>>;

    /// Open one layer: the plain text, or `None` when the key was destroyed or is unknown, or the
    /// payload does not open for this tenant, scope, name and blinds.
    /// Implements: H-020
    fn open_one(
        &mut self,
        scope: &str,
        name: &str,
        key_id: &str,
        blinds: &str,
        payload: &str,
    ) -> StoreResult<Option<String>>;

    /// The blind hash of `plain` for `scope` and `name`, under the tenant key. `None` when the
    /// tenant has no key yet and `create` is false: then nothing of it was sealed either.
    fn blind(
        &mut self,
        scope: &str,
        name: &str,
        plain: &str,
        create: bool,
    ) -> StoreResult<Option<String>>;
}

/// The key service as the store sees it (component `key-service`): a session per tenant and call.
pub trait FieldCipher: Send + Sync {
    /// A session for `tenant`, reading and writing its keys through `store` (the adapter, not the
    /// sealed store). It caches nothing beyond itself, so a destroyed key opens nothing in any
    /// later call.
    fn session<'a>(
        &'a self,
        store: &'a dyn Store,
        tenant: &'a TenantId,
    ) -> Box<dyn CipherSession + 'a>;
}

/// The personal declarations of a schema, by object type and field.
#[derive(Default)]
struct Plan {
    objects: BTreeMap<ObjectTypeId, BTreeMap<FieldId, PersonalDeclaration>>,
}

impl Plan {
    fn of(schema: &TenantSchema) -> Plan {
        let mut plan = Plan::default();
        for object in schema.objects.values() {
            let declared: BTreeMap<FieldId, PersonalDeclaration> = object
                .fields
                .values()
                .filter_map(|field| {
                    field
                        .personal
                        .as_ref()
                        .map(|personal| (field.id.clone(), personal.clone()))
                })
                .collect();
            if !declared.is_empty() {
                plan.objects.insert(object.id.clone(), declared);
            }
        }
        plan
    }

    fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    fn declaration(&self, object: &ObjectTypeId, field: &FieldId) -> Option<&PersonalDeclaration> {
        self.objects
            .get(object)
            .and_then(|fields| fields.get(field))
    }
}

/// The principals a text value names: one, or each of a list, in order and once; a token names
/// none.
fn principals_in(value: Option<&Value>) -> Vec<PrincipalId> {
    fn one(text: &str) -> Option<PrincipalId> {
        if text.starts_with(TOKEN_PREFIX) {
            None
        } else {
            PrincipalId::new(text).ok()
        }
    }
    match value {
        Some(Value::Str(text)) => one(text.as_str()).into_iter().collect(),
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::Str(text) => one(text.as_str()),
                _ => None,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// The data subjects of a personal field of `record`. A field that names no principal (no
/// creator, or a reference to the record of a person, which the store cannot resolve) has none,
/// and its value is stored as it is.
fn subjects_of(declaration: &PersonalDeclaration, record: &Record) -> Vec<PrincipalId> {
    match &declaration.subject {
        DataSubject::Creator => record.created_by.iter().cloned().collect(),
        DataSubject::Field { field } => principals_in(record.fields.get(field)),
    }
}

/// The schema a commit's records are sealed under: the last schema the delta writes, or the one
/// it was decided over.
fn schema_after(delta: &StateDelta) -> Option<Arc<TenantSchema>> {
    delta.ops.iter().rev().find_map(|op| match op {
        DeltaOp::PutSchema(schema) => Some(schema.clone()),
        _ => None,
    })
}

fn decode_plain<T: serde::de::DeserializeOwned>(plain: &str) -> Option<T> {
    serde_json::from_str(plain).ok()
}

/// Seals values for one commit or one replayed transition.
struct Sealer<'s> {
    plan: &'s Plan,
    session: Box<dyn CipherSession + 's>,
    /// Whether a subject without a live key gets one (a commit) or not (a replay).
    create: bool,
    /// A value could not be sealed because its subject has no live key (a replay only).
    missing: bool,
}

impl Sealer<'_> {
    /// The blind hashes of a plain value: of each item of a list, or of the value.
    fn blinds(&mut self, scope: &str, name: &str, value: &Value) -> StoreResult<String> {
        let items: Vec<&Value> = match value {
            Value::List(items) => items.iter().collect(),
            single => vec![single],
        };
        let mut hashes = BTreeSet::new();
        for item in items {
            if matches!(item, Value::Null) {
                continue;
            }
            if let Some(hash) =
                self.session
                    .blind(scope, name, &canon::canonical_json(item), self.create)?
            {
                hashes.insert(hash);
            }
        }
        Ok(hashes.into_iter().collect::<Vec<_>>().join(","))
    }

    /// `value` sealed under the keys of `subjects`, the last one outermost; the value itself when
    /// there is no subject, when it is null or already a token, or when a key is missing.
    fn value(
        &mut self,
        subjects: &[PrincipalId],
        scope: &str,
        name: &str,
        value: &Value,
        searchable: bool,
    ) -> StoreResult<Value> {
        if subjects.is_empty() || matches!(value, Value::Null) || is_sealed(value) {
            return Ok(value.clone());
        }
        let blinds = if searchable {
            self.blinds(scope, name, value)?
        } else {
            String::new()
        };
        let mut plain = canon::canonical_json(value);
        let mut token = String::new();
        for (index, subject) in subjects.iter().enumerate() {
            let marks = if index + 1 == subjects.len() {
                blinds.as_str()
            } else {
                ""
            };
            match self
                .session
                .seal_one(subject, scope, name, &plain, marks, self.create)?
            {
                Some((key_id, payload)) => {
                    token = format_token(&key_id, marks, &payload);
                    plain = canon::canonical_json(&Value::Str(token.clone()));
                }
                None => {
                    self.missing = true;
                    return Ok(value.clone());
                }
            }
        }
        Ok(Value::Str(token))
    }

    /// The record with its personal fields sealed, or `None` when nothing of it is sealed.
    fn record(&mut self, record: &Record) -> StoreResult<Option<Record>> {
        let plan = self.plan;
        let Some(fields) = plan.objects.get(&record.object) else {
            return Ok(None);
        };
        let mut sealed: Option<Record> = None;
        for (field, declaration) in fields {
            let Some(plain) = record.fields.get(field) else {
                continue;
            };
            // The subjects are read from the plain record, before any field of it is sealed.
            let subjects = subjects_of(declaration, record);
            let value = self.value(
                &subjects,
                record.object.as_str(),
                field.as_str(),
                plain,
                declaration.searchable,
            )?;
            if value != *plain {
                sealed
                    .get_or_insert_with(|| record.clone())
                    .fields
                    .insert(field.clone(), value);
            }
        }
        Ok(sealed)
    }

    fn shared(&mut self, record: &mut Arc<Record>) -> StoreResult<()> {
        if let Some(sealed) = self.record(record)? {
            *record = Arc::new(sealed);
        }
        Ok(())
    }

    fn delta(&mut self, delta: &mut StateDelta) -> StoreResult<()> {
        for op in &mut delta.ops {
            if let DeltaOp::PutRecord(record) = op {
                self.shared(record)?;
            }
        }
        Ok(())
    }

    fn events(&mut self, events: &mut [DomainEvent]) -> StoreResult<()> {
        for event in events {
            match &mut event.body {
                EventBody::RecordCreated { record } => self.shared(record)?,
                EventBody::RecordUpdated { before, after, .. } => {
                    self.shared(before)?;
                    self.shared(after)?;
                }
                EventBody::RecordDeleted { before } => self.shared(before)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// The field values of a record intent, sealed as the record the decision wrote seals them:
    /// the same subjects, scope and field give the same token, so that a replay of the entry
    /// after an erasure still produces the recorded sealed delta.
    fn intent(
        &mut self,
        intent: &mut Intent,
        principal: &Principal,
        now: Timestamp,
        written: &StateDelta,
    ) -> StoreResult<()> {
        let updated = match &*intent {
            Intent::UpdateRecord { id, .. } => Some(*id),
            _ => None,
        };
        let plan = self.plan;
        let (object, fields): (&ObjectTypeId, &mut BTreeMap<FieldId, Value>) = match intent {
            Intent::CreateRecord { object, fields }
            | Intent::UpdateRecord { object, fields, .. } => (&*object, fields),
            _ => return Ok(()),
        };
        let Some(declared) = plan.objects.get(object) else {
            return Ok(());
        };
        let basis: Record = written
            .ops
            .iter()
            .find_map(|op| match op {
                DeltaOp::PutRecord(record)
                    if record.object == *object
                        && match updated {
                            Some(id) => record.id == id,
                            None => record.version == 1,
                        } =>
                {
                    Some(Record::clone(record))
                }
                _ => None,
            })
            .unwrap_or_else(|| Record {
                id: RecordId(0),
                object: object.clone(),
                version: 0,
                created_by: match principal {
                    Principal::User(id) => Some(id.clone()),
                    Principal::System | Principal::Anonymous => None,
                },
                created_at: now,
                updated_at: now,
                fields: fields.clone(),
            });
        for (field, value) in fields.iter_mut() {
            if let Some(declaration) = declared.get(field) {
                let subjects = subjects_of(declaration, &basis);
                *value = self.value(
                    &subjects,
                    object.as_str(),
                    field.as_str(),
                    value,
                    declaration.searchable,
                )?;
            }
        }
        Ok(())
    }
}

/// Opens tokens for one store call.
struct Opener<'s> {
    session: Box<dyn CipherSession + 's>,
}

impl Opener<'_> {
    /// The value with every layer opened that opens; `None` when it was not a token or no layer
    /// opened.
    fn value(&mut self, scope: &str, name: &str, value: &Value) -> StoreResult<Option<Value>> {
        let mut current: Option<Value> = None;
        // A layer opens to the token of the next one; a value has as many layers as subjects.
        for _ in 0..MAX_LAYERS {
            let Value::Str(text) = current.as_ref().unwrap_or(value) else {
                break;
            };
            let Some(token) = parse_token(text) else {
                break;
            };
            let opened =
                self.session
                    .open_one(scope, name, token.key_id, token.blinds, token.payload)?;
            match opened.as_deref().and_then(decode_plain::<Value>) {
                Some(plain) => current = Some(plain),
                None => break,
            }
        }
        Ok(current)
    }

    fn record(&mut self, record: &Record) -> StoreResult<Option<Record>> {
        let mut opened: Option<Record> = None;
        for (field, value) in &record.fields {
            if !is_sealed(value) {
                continue;
            }
            if let Some(plain) = self.value(record.object.as_str(), field.as_str(), value)? {
                opened
                    .get_or_insert_with(|| record.clone())
                    .fields
                    .insert(field.clone(), plain);
            }
        }
        Ok(opened)
    }

    fn shared(&mut self, record: &mut Arc<Record>) -> StoreResult<()> {
        if let Some(opened) = self.record(record)? {
            *record = Arc::new(opened);
        }
        Ok(())
    }

    fn tenant(&mut self, tenant: &mut TenantState) -> StoreResult<()> {
        for records in tenant.records.values_mut() {
            for record in records.values_mut() {
                self.shared(record)?;
            }
        }
        Ok(())
    }

    fn event(&mut self, event: &mut DomainEvent) -> StoreResult<()> {
        match &mut event.body {
            EventBody::RecordCreated { record } => self.shared(record),
            EventBody::RecordUpdated { before, after, .. } => {
                self.shared(before)?;
                self.shared(after)
            }
            EventBody::RecordDeleted { before } => self.shared(before),
            _ => Ok(()),
        }
    }

    fn intent(&mut self, intent: &mut Intent) -> StoreResult<()> {
        if let Intent::CreateRecord { object, fields }
        | Intent::UpdateRecord { object, fields, .. } = intent
        {
            for (field, value) in fields.iter_mut() {
                if let Some(plain) = self.value(object.as_str(), field.as_str(), value)? {
                    *value = plain;
                }
            }
        }
        Ok(())
    }

    /// The members of an audit line opened.
    fn audit(&mut self, entry: &mut AuditEntry) -> StoreResult<()> {
        if !entry.data.contains(TOKEN_PREFIX) {
            return Ok(());
        }
        let Ok(serde_json::Value::Object(mut members)) =
            serde_json::from_str::<serde_json::Value>(&entry.data)
        else {
            return Ok(());
        };
        let mut changed = false;
        for (name, member) in members.iter_mut() {
            let Some(token) = member.as_str().and_then(parse_token) else {
                continue;
            };
            let opened = self.session.open_one(
                AUDIT_SCOPE,
                name,
                token.key_id,
                token.blinds,
                token.payload,
            )?;
            if let Some(plain) = opened
                .as_deref()
                .and_then(decode_plain::<serde_json::Value>)
            {
                *member = plain;
                changed = true;
            }
        }
        if changed {
            entry.data = serde_json::Value::Object(members).to_string();
        }
        Ok(())
    }
}

/// Whether a value can be found through the index by its blind hash: a single value.
fn single(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::List(_))
}

/// An adapter whose personal fields are sealed (see the module documentation).
pub struct SealedStore {
    inner: Arc<dyn Store>,
    cipher: Arc<dyn FieldCipher>,
}

impl SealedStore {
    pub fn new(inner: Arc<dyn Store>, cipher: Arc<dyn FieldCipher>) -> SealedStore {
        SealedStore { inner, cipher }
    }

    /// The adapter underneath, which reads and writes values as stored.
    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }

    fn session<'a>(&'a self, tenant: &'a TenantId) -> Box<dyn CipherSession + 'a> {
        self.cipher.session(self.inner.as_ref(), tenant)
    }

    fn opener<'a>(&'a self, tenant: &'a TenantId) -> Opener<'a> {
        Opener {
            session: self.session(tenant),
        }
    }

    /// The commit as it is stored: personal fields sealed and the journal's hashes over the
    /// sealed form. A commit of a tenant whose schema declares no personal field is unchanged.
    /// The audit lines of a commit, sealed as a line of [`Store::audit_append`] is (LDG-2925): the
    /// members that name a person are sealed under that person's key.
    fn seal_commit_audit<'c>(&self, commit: Cow<'c, Commit>) -> StoreResult<Cow<'c, Commit>> {
        if commit.audit.is_empty() {
            return Ok(commit);
        }
        let tenant = commit.delta.tenant.clone();
        let mut sealed = commit.into_owned();
        let mut lines = Vec::with_capacity(sealed.audit.len());
        for line in &sealed.audit {
            lines.push(self.seal_audit(&tenant, line)?.into_owned());
        }
        sealed.audit = lines;
        Ok(Cow::Owned(sealed))
    }

    fn seal_commit<'c>(&self, commit: &'c Commit) -> StoreResult<Cow<'c, Commit>> {
        let tenant = &commit.delta.tenant;
        let schema = match schema_after(&commit.delta) {
            Some(schema) => schema,
            None => {
                if commit.delta.base_revision.is_none() {
                    return Ok(Cow::Borrowed(commit));
                }
                let loaded = self.inner.load_at(tenant, &[])?;
                let found_generation = loaded.generation;
                let header = loaded.state;
                let Some(current) = header.tenant else {
                    // The adapter refuses a commit to a tenant that does not exist.
                    return Ok(Cow::Borrowed(commit));
                };
                if Some(current.revision) != commit.delta.base_revision {
                    return Err(StoreError::Conflict {
                        expected: commit.delta.base_revision,
                        found: Some(current.revision),
                        found_generation,
                    });
                }
                current.schema.clone()
            }
        };
        let plan = Plan::of(&schema);
        if plan.is_empty() {
            return Ok(Cow::Borrowed(commit));
        }
        let mut sealer = Sealer {
            plan: &plan,
            session: self.session(tenant),
            create: true,
            missing: false,
        };
        let mut sealed = commit.clone();
        sealer.delta(&mut sealed.delta)?;
        sealer.events(&mut sealed.events)?;
        let principal = commit.journal.context.principal.clone();
        let now = commit.journal.context.now;
        sealer.intent(&mut sealed.journal.intent, &principal, now, &commit.delta)?;
        sealed.journal.delta_hash = canon::hash_of(&sealed.delta);
        sealed.journal.events_hash = canon::hash_of(&sealed.events);
        Ok(Cow::Owned(sealed))
    }

    /// The hashes a journal entry written through this store records for a transition: of its
    /// delta and its events as stored (D-350), sealed again with the live keys and creating none.
    /// `before` is the schema the transition was decided over. `None` when a value cannot be sealed
    /// again because its subject has no live key: the replay reports the entry rather than a
    /// mismatch (PRV-361).
    pub fn journal_hashes(
        &self,
        tenant: &TenantId,
        before: Option<&TenantSchema>,
        delta: &StateDelta,
        events: &[DomainEvent],
    ) -> StoreResult<Option<(String, String)>> {
        let after = schema_after(delta);
        let plan = match (after.as_deref(), before) {
            (Some(schema), _) | (None, Some(schema)) => Plan::of(schema),
            (None, None) => Plan::default(),
        };
        if plan.is_empty() {
            return Ok(Some((canon::hash_of(delta), canon::hash_of(&events))));
        }
        let mut sealer = Sealer {
            plan: &plan,
            session: self.session(tenant),
            create: false,
            missing: false,
        };
        let mut sealed_delta = delta.clone();
        let mut sealed_events = events.to_vec();
        sealer.delta(&mut sealed_delta)?;
        sealer.events(&mut sealed_events)?;
        if sealer.missing {
            return Ok(None);
        }
        Ok(Some((
            canon::hash_of(&sealed_delta),
            canon::hash_of(&sealed_events),
        )))
    }

    fn open_state(&self, state: State) -> StoreResult<State> {
        let Some(tenant) = state.tenant.as_deref() else {
            return Ok(state);
        };
        let sealed = tenant
            .records
            .values()
            .flat_map(|records| records.values())
            .any(|record| record.fields.values().any(is_sealed));
        if !sealed {
            return Ok(state);
        }
        let mut opened = TenantState::clone(tenant);
        self.opener(&state.tenant_id).tenant(&mut opened)?;
        Ok(State {
            tenant: Some(Arc::new(opened)),
            ..state
        })
    }

    /// The needs that answer `need` against sealed rows, or `None` when it is asked as it is.
    fn translate(
        plan: &Plan,
        session: &mut (dyn CipherSession + '_),
        need: &Need,
    ) -> StoreResult<Option<Vec<Need>>> {
        match need {
            Need::ByField {
                object,
                field,
                value,
            } => {
                let Some(declaration) = plan.declaration(object, field) else {
                    return Ok(None);
                };
                if !declaration.searchable || !single(value) {
                    return Ok(Some(vec![Need::AllOf(object.clone())]));
                }
                // The plain value finds the rows written before sealing.
                let mut out = vec![need.clone()];
                let plain = canon::canonical_json(value);
                if let Some(hash) = session.blind(object.as_str(), field.as_str(), &plain, false)? {
                    out.push(Need::ByField {
                        object: object.clone(),
                        field: field.clone(),
                        value: blind_value(&hash),
                    });
                }
                Ok(Some(out))
            }
            Need::ByFields { object, fields } => {
                if !fields
                    .keys()
                    .any(|field| plan.declaration(object, field).is_some())
                {
                    return Ok(None);
                }
                if fields.iter().any(|(field, value)| {
                    plan.declaration(object, field)
                        .is_some_and(|declaration| !declaration.searchable || !single(value))
                }) {
                    return Ok(Some(vec![Need::AllOf(object.clone())]));
                }
                let mut blinded = fields.clone();
                for (field, value) in blinded.iter_mut() {
                    if plan.declaration(object, field).is_none() {
                        continue;
                    }
                    let plain = canon::canonical_json(&*value);
                    match session.blind(object.as_str(), field.as_str(), &plain, false)? {
                        Some(hash) => *value = blind_value(&hash),
                        // No tenant key: nothing of the tenant was sealed.
                        None => return Ok(Some(vec![need.clone()])),
                    }
                }
                Ok(Some(vec![
                    need.clone(),
                    Need::ByFields {
                        object: object.clone(),
                        fields: blinded,
                    },
                ]))
            }
            Need::IndexPage { object, field, .. } => Ok(plan
                .declaration(object, field)
                .map(|_| vec![Need::AllOf(object.clone())])),
            // A pair page over a personal field, or ordered by one, is answered from the opened
            // records of the type: a token is neither the value that is looked up nor in its order.
            Need::PairPage {
                object,
                field,
                order,
                ..
            } => Ok((plan.declaration(object, field).is_some()
                || plan.objects.get(object).is_some_and(|fields| {
                    fields
                        .keys()
                        .any(|personal| personal.as_str() == order.as_str())
                }))
            .then(|| vec![Need::AllOf(object.clone())])),
            Need::Record { .. } | Need::AllOf(_) | Need::PageOf { .. } | Need::Ledger(_) => {
                Ok(None)
            }
        }
    }

    /// One attempt at a load with lookups: `None` when the tenant moved between the header and
    /// the rows (another schema, or another generation), and the translation may be wrong. The
    /// generation returned is the one of the rows the state was cut from.
    fn load_translated(&self, tenant: &TenantId, needs: &[Need]) -> StoreResult<Option<Loaded>> {
        let header_loaded = self.inner.load_at(tenant, &[])?;
        let header = header_loaded.state;
        let Some(head) = header.tenant.as_deref() else {
            return Ok(Some(Loaded {
                state: header,
                generation: header_loaded.generation,
            }));
        };
        let plan = Plan::of(&head.schema);
        if plan.is_empty() {
            return self
                .open_loaded(self.inner.load_at(tenant, needs)?)
                .map(Some);
        }
        let mut wanted: BTreeSet<Need> = BTreeSet::new();
        let mut translated = false;
        {
            let mut session = self.session(tenant);
            for need in needs {
                match Self::translate(&plan, session.as_mut(), need)? {
                    Some(list) => {
                        translated = true;
                        wanted.extend(list);
                    }
                    None => {
                        wanted.insert(need.clone());
                    }
                }
            }
        }
        if !translated {
            return self
                .open_loaded(self.inner.load_at(tenant, needs)?)
                .map(Some);
        }
        let list: Vec<Need> = wanted.into_iter().collect();
        let loaded = self.open_loaded(self.inner.load_at(tenant, &list)?)?;
        let Some(found) = loaded.state.tenant.as_deref() else {
            return Ok(Some(loaded));
        };
        if loaded.generation != header_loaded.generation
            || (found.revision != head.revision && found.schema != head.schema)
        {
            return Ok(None);
        }
        // Exactly what the kernel asked for, judged on the opened values.
        Ok(Some(Loaded {
            state: partial_state(found, needs),
            generation: loaded.generation,
        }))
    }

    fn open_loaded(&self, loaded: Loaded) -> StoreResult<Loaded> {
        Ok(Loaded {
            state: self.open_state(loaded.state)?,
            generation: loaded.generation,
        })
    }

    /// Seal the members of an audit line that name a person.
    fn seal_audit<'e>(
        &self,
        tenant: &TenantId,
        entry: &'e AuditEntry,
    ) -> StoreResult<Cow<'e, AuditEntry>> {
        let Ok(serde_json::Value::Object(mut members)) =
            serde_json::from_str::<serde_json::Value>(&entry.data)
        else {
            return Ok(Cow::Borrowed(entry));
        };
        let named = |member: &serde_json::Value| {
            member
                .as_str()
                .filter(|text| !text.starts_with(TOKEN_PREFIX))
                .and_then(|text| PrincipalId::new(text).ok())
        };
        let first = AUDIT_SUBJECT_MEMBERS
            .iter()
            .find_map(|name| members.get(*name).and_then(named));
        let Some(first) = first else {
            return Ok(Cow::Borrowed(entry));
        };
        // A tenant that does not exist gets no key: the adapter refuses the line.
        if self.inner.tenant_summary(tenant)?.is_none() {
            return Ok(Cow::Borrowed(entry));
        }
        let mut session = self.session(tenant);
        let mut seal = |name: &str,
                        subject: &PrincipalId,
                        member: &serde_json::Value|
         -> StoreResult<Option<serde_json::Value>> {
            Ok(session
                .seal_one(subject, AUDIT_SCOPE, name, &member.to_string(), "", true)?
                .map(|(key_id, payload)| {
                    serde_json::Value::String(format_token(&key_id, "", &payload))
                }))
        };
        let mut changed = false;
        for name in AUDIT_SUBJECT_MEMBERS {
            let Some(member) = members.get(*name).cloned() else {
                continue;
            };
            let Some(subject) = named(&member) else {
                continue;
            };
            if let Some(sealed) = seal(name, &subject, &member)? {
                members.insert((*name).to_string(), sealed);
                changed = true;
            }
        }
        for name in AUDIT_PERSONAL_MEMBERS {
            let Some(member) = members.get(*name).cloned() else {
                continue;
            };
            if member.is_null()
                || member
                    .as_str()
                    .is_some_and(|text| text.starts_with(TOKEN_PREFIX))
            {
                continue;
            }
            if let Some(sealed) = seal(name, &first, &member)? {
                members.insert((*name).to_string(), sealed);
                changed = true;
            }
        }
        if !changed {
            return Ok(Cow::Borrowed(entry));
        }
        Ok(Cow::Owned(AuditEntry {
            data: serde_json::Value::Object(members).to_string(),
            ..entry.clone()
        }))
    }
}

impl Store for SealedStore {
    fn load_at(&self, tenant: &TenantId, needs: &[Need]) -> StoreResult<Loaded> {
        let lookups = needs.iter().any(|need| {
            matches!(
                need,
                Need::ByField { .. }
                    | Need::ByFields { .. }
                    | Need::IndexPage { .. }
                    | Need::PairPage { .. }
            )
        });
        if !lookups {
            return self.open_loaded(self.inner.load_at(tenant, needs)?);
        }
        for _ in 0..3 {
            if let Some(loaded) = self.load_translated(tenant, needs)? {
                return Ok(loaded);
            }
        }
        Err(StoreError::Unavailable(
            "the schema kept changing while the records were read; try again".into(),
        ))
    }

    fn load_complete_at(&self, tenant: &TenantId) -> StoreResult<Loaded> {
        self.open_loaded(self.inner.load_complete_at(tenant)?)
    }

    fn generation(&self, tenant: &TenantId) -> StoreResult<Option<Generation>> {
        self.inner.generation(tenant)
    }

    fn rotate_nonce(&self, tenant: &TenantId) -> StoreResult<Generation> {
        self.inner.rotate_nonce(tenant)
    }

    fn commit(&self, commit: &Commit) -> StoreResult<Receipt> {
        let sealed = self.seal_commit_audit(self.seal_commit(commit)?)?;
        self.inner.commit(&sealed)
    }

    fn commit_with_segments(
        &self,
        commit: &Commit,
        write: &seg::SegmentWrite,
    ) -> StoreResult<Receipt> {
        let sealed = self.seal_commit_audit(self.seal_commit(commit)?)?;
        self.inner.commit_with_segments(&sealed, write)
    }

    fn events_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<DomainEvent>> {
        let mut events = self.inner.events_after(tenant, after, limit)?;
        let mut opener = self.opener(tenant);
        for event in &mut events {
            opener.event(event)?;
        }
        Ok(events)
    }

    fn outbox(&self, limit: usize) -> StoreResult<Vec<OutboxEntry>> {
        let mut entries = self.inner.outbox(limit)?;
        let tenants: BTreeSet<TenantId> = entries
            .iter()
            .map(|entry| entry.event.tenant.clone())
            .collect();
        for tenant in &tenants {
            let mut opener = self.opener(tenant);
            for entry in entries.iter_mut().filter(|e| e.event.tenant == *tenant) {
                opener.event(&mut entry.event)?;
            }
        }
        Ok(entries)
    }

    fn journal_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<JournalEntry>> {
        let mut entries = self.inner.journal_after(tenant, after, limit)?;
        let mut opener = self.opener(tenant);
        for entry in &mut entries {
            opener.intent(&mut entry.intent)?;
        }
        Ok(entries)
    }

    fn load_snapshot(&self, tenant: &TenantId, id: u64) -> StoreResult<TenantState> {
        let mut state = self.inner.load_snapshot(tenant, id)?;
        self.opener(tenant).tenant(&mut state)?;
        Ok(state)
    }

    fn segment_referencing(
        &self,
        tenant: &TenantId,
        object: &ObjectTypeId,
        field: &FieldId,
        record: RecordId,
        after: Option<RecordId>,
        limit: usize,
    ) -> StoreResult<Vec<Arc<Record>>> {
        let mut found = self
            .inner
            .segment_referencing(tenant, object, field, record, after, limit)?;
        let mut opener = self.opener(tenant);
        for record in &mut found {
            opener.shared(record)?;
        }
        Ok(found)
    }

    fn audit_append(&self, tenant: &TenantId, entry: &AuditEntry) -> StoreResult<()> {
        let sealed = self.seal_audit(tenant, entry)?;
        self.inner.audit_append(tenant, &sealed)
    }

    fn audit_list(
        &self,
        tenant: &TenantId,
        kind: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<AuditEntry>> {
        let mut lines = self.inner.audit_list(tenant, kind, limit)?;
        let mut opener = self.opener(tenant);
        for line in &mut lines {
            opener.audit(line)?;
        }
        Ok(lines)
    }

    // Everything else passes through: it holds no value of a personal field of a record.

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn health(&self) -> StoreResult<Health> {
        self.inner.health()
    }

    fn liveness(&self) -> StoreResult<()> {
        self.inner.liveness()
    }

    fn migrations(&self) -> StoreResult<Vec<StorageMigration>> {
        self.inner.migrations()
    }

    fn tenants(&self) -> StoreResult<Vec<TenantSummary>> {
        self.inner.tenants()
    }

    fn tenant_summary(&self, tenant: &TenantId) -> StoreResult<Option<TenantSummary>> {
        self.inner.tenant_summary(tenant)
    }

    fn idempotent_result(
        &self,
        tenant: &TenantId,
        principal: &str,
        key: &str,
        now: Timestamp,
    ) -> StoreResult<Option<IdempotentResult>> {
        self.inner.idempotent_result(tenant, principal, key, now)
    }

    fn outbox_faults(&self) -> Vec<OutboxFault> {
        self.inner.outbox_faults()
    }

    fn outbox_pending_of(&self, tenant: &TenantId) -> StoreResult<u64> {
        self.inner.outbox_pending_of(tenant)
    }

    fn mark_published(&self, tenant: &TenantId, position: u64) -> StoreResult<()> {
        self.inner.mark_published(tenant, position)
    }

    fn journal_count(&self, tenant: &TenantId) -> StoreResult<u64> {
        self.inner.journal_count(tenant)
    }

    fn put_session(&self, session: &SessionRecord) -> StoreResult<()> {
        self.inner.put_session(session)
    }

    fn session_seen(&self, id: &SessionId, at: Timestamp) -> StoreResult<()> {
        self.inner.session_seen(id, at)
    }

    fn session_authenticated(
        &self,
        id: &SessionId,
        at: Timestamp,
        network: Option<&str>,
    ) -> StoreResult<()> {
        self.inner.session_authenticated(id, at, network)
    }

    fn session_by_token(&self, token_hash: &str) -> StoreResult<Option<SessionRecord>> {
        self.inner.session_by_token(token_hash)
    }

    fn session(&self, id: &SessionId) -> StoreResult<Option<SessionRecord>> {
        self.inner.session(id)
    }

    fn live_sessions(&self, now: Timestamp) -> StoreResult<Vec<SessionRecord>> {
        self.inner.live_sessions(now)
    }

    fn live_sessions_of(
        &self,
        tenant: &TenantId,
        principal: Option<&PrincipalId>,
        now: Timestamp,
    ) -> StoreResult<Vec<SessionRecord>> {
        self.inner.live_sessions_of(tenant, principal, now)
    }

    fn purge_sessions(&self, before: Timestamp) -> StoreResult<u64> {
        self.inner.purge_sessions(before)
    }

    fn throttle_reserve(
        &self,
        tenant: &TenantId,
        rules: &[ThrottleRule],
        now: Timestamp,
    ) -> StoreResult<ThrottleVerdict> {
        self.inner.throttle_reserve(tenant, rules, now)
    }

    fn throttle_settle(
        &self,
        tenant: &TenantId,
        key: &str,
        max: u32,
        how: ThrottleSettle,
    ) -> StoreResult<()> {
        self.inner.throttle_settle(tenant, key, max, how)
    }

    fn purge_throttle(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_throttle(now)
    }

    fn put_account(&self, account: &Account) -> StoreResult<()> {
        self.inner.put_account(account)
    }

    fn create_account(&self, account: &Account) -> StoreResult<bool> {
        self.inner.create_account(account)
    }

    fn update_account(&self, expected: &Account, replacement: &Account) -> StoreResult<bool> {
        self.inner.update_account(expected, replacement)
    }

    fn account(&self, tenant: &TenantId, login: &str) -> StoreResult<Option<Account>> {
        self.inner.account(tenant, login)
    }

    fn remove_account(&self, tenant: &TenantId, login: &str) -> StoreResult<bool> {
        self.inner.remove_account(tenant, login)
    }

    fn remove_principal(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<PrincipalRemoval> {
        self.inner.remove_principal(tenant, principal)
    }

    fn anonymise_ledger_principal(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        expected: Option<&Generation>,
    ) -> StoreResult<strata_kernel::ledger::Anonymised> {
        self.inner
            .anonymise_ledger_principal(tenant, principal, expected)
    }

    fn account_of(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<Account>> {
        self.inner.account_of(tenant, principal)
    }

    fn put_passkey(&self, passkey: &PasskeyRecord) -> StoreResult<bool> {
        self.inner.put_passkey(passkey)
    }

    fn passkey(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<PasskeyRecord>> {
        self.inner.passkey(tenant, id)
    }

    fn passkeys(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Vec<PasskeyRecord>> {
        self.inner.passkeys(tenant, principal)
    }

    fn update_passkey(
        &self,
        tenant: &TenantId,
        id: &str,
        credential: &str,
        used_at: Timestamp,
    ) -> StoreResult<bool> {
        self.inner.update_passkey(tenant, id, credential, used_at)
    }

    fn remove_passkey(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        id: &str,
    ) -> StoreResult<bool> {
        self.inner.remove_passkey(tenant, principal, id)
    }

    fn put_passkey_challenge(&self, challenge: &PasskeyChallenge) -> StoreResult<()> {
        self.inner.put_passkey_challenge(challenge)
    }

    fn take_passkey_challenge(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
    ) -> StoreResult<Option<PasskeyChallenge>> {
        self.inner.take_passkey_challenge(tenant, id_hash, now)
    }

    fn purge_passkey_challenges(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_passkey_challenges(now)
    }

    fn put_sign_in_link(&self, link: &SignInLink) -> StoreResult<()> {
        self.inner.put_sign_in_link(link)
    }

    fn take_sign_in_link(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<SignInLink>> {
        self.inner.take_sign_in_link(tenant, id_hash)
    }

    fn purge_sign_in_links(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_sign_in_links(now)
    }

    fn put_oidc_flow(&self, flow: &OidcFlow) -> StoreResult<()> {
        self.inner.put_oidc_flow(flow)
    }

    fn take_oidc_flow(&self, tenant: &TenantId, id_hash: &str) -> StoreResult<Option<OidcFlow>> {
        self.inner.take_oidc_flow(tenant, id_hash)
    }

    fn purge_oidc_flows(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_oidc_flows(now)
    }

    fn put_saml_request(&self, request: &SamlRequest) -> StoreResult<()> {
        self.inner.put_saml_request(request)
    }

    fn take_saml_request(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<SamlRequest>> {
        self.inner.take_saml_request(tenant, id_hash)
    }

    fn purge_saml_requests(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_saml_requests(now)
    }

    fn put_saml_assertion(
        &self,
        tenant: &TenantId,
        provider: &str,
        id_hash: &str,
        expires_at: Timestamp,
    ) -> StoreResult<bool> {
        self.inner
            .put_saml_assertion(tenant, provider, id_hash, expires_at)
    }

    fn purge_saml_assertions(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_saml_assertions(now)
    }

    fn put_identity(&self, identity: &FederatedIdentity) -> StoreResult<bool> {
        self.inner.put_identity(identity)
    }

    fn identity(
        &self,
        tenant: &TenantId,
        provider: &str,
        subject: &str,
    ) -> StoreResult<Option<FederatedIdentity>> {
        self.inner.identity(tenant, provider, subject)
    }

    fn put_totp(&self, totp: &TotpRecord) -> StoreResult<bool> {
        self.inner.put_totp(totp)
    }

    fn totp(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<Option<TotpRecord>> {
        self.inner.totp(tenant, principal)
    }

    fn confirm_totp(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        step: i64,
    ) -> StoreResult<bool> {
        self.inner.confirm_totp(tenant, principal, step)
    }

    fn use_totp_step(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        step: i64,
    ) -> StoreResult<bool> {
        self.inner.use_totp_step(tenant, principal, step)
    }

    fn remove_totp(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<bool> {
        self.inner.remove_totp(tenant, principal)
    }

    fn replace_recovery_codes(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        hashes: &[String],
        at: Timestamp,
    ) -> StoreResult<()> {
        self.inner
            .replace_recovery_codes(tenant, principal, hashes, at)
    }

    fn spend_recovery_code(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        hash: &str,
    ) -> StoreResult<bool> {
        self.inner.spend_recovery_code(tenant, principal, hash)
    }

    fn recovery_codes_left(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<u32> {
        self.inner.recovery_codes_left(tenant, principal)
    }

    fn put_pending(&self, pending: &PendingSignIn) -> StoreResult<()> {
        self.inner.put_pending(pending)
    }

    fn attempt_pending(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
        max: u32,
    ) -> StoreResult<Option<PendingSignIn>> {
        self.inner.attempt_pending(tenant, id_hash, now, max)
    }

    fn take_pending(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
    ) -> StoreResult<Option<PendingSignIn>> {
        self.inner.take_pending(tenant, id_hash, now)
    }

    fn put_recovery(&self, request: &RecoveryRequest) -> StoreResult<()> {
        self.inner.put_recovery(request)
    }

    fn recovery(&self, tenant: &TenantId, id_hash: &str) -> StoreResult<Option<RecoveryRequest>> {
        self.inner.recovery(tenant, id_hash)
    }

    fn take_recovery(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<RecoveryRequest>> {
        self.inner.take_recovery(tenant, id_hash)
    }

    fn cancel_recovery(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        cancel_hash: &str,
    ) -> StoreResult<bool> {
        self.inner.cancel_recovery(tenant, id_hash, cancel_hash)
    }

    fn purge_factor_challenges(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_factor_challenges(now)
    }

    fn doc_put(
        &self,
        tenant: &TenantId,
        kind: &str,
        key: &str,
        value: &str,
        create_only: bool,
    ) -> StoreResult<bool> {
        self.inner.doc_put(tenant, kind, key, value, create_only)
    }

    fn doc_get(&self, tenant: &TenantId, kind: &str, key: &str) -> StoreResult<Option<String>> {
        self.inner.doc_get(tenant, kind, key)
    }

    fn doc_list(&self, tenant: &TenantId, kind: &str) -> StoreResult<Vec<(String, String)>> {
        self.inner.doc_list(tenant, kind)
    }

    fn doc_remove(&self, tenant: &TenantId, kind: &str, key: &str) -> StoreResult<bool> {
        self.inner.doc_remove(tenant, kind, key)
    }

    fn audit_chain(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<ChainedAuditEntry>> {
        self.inner.audit_chain(tenant, after, limit)
    }

    fn verified_address(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<String>> {
        self.inner.verified_address(tenant, principal)
    }

    fn create_snapshot(
        &self,
        tenant: &TenantId,
        label: &str,
        at: Timestamp,
    ) -> StoreResult<SnapshotInfo> {
        self.inner.create_snapshot(tenant, label, at)
    }

    fn snapshots(&self, tenant: &TenantId) -> StoreResult<Vec<SnapshotInfo>> {
        self.inner.snapshots(tenant)
    }

    fn restore_snapshot(
        &self,
        tenant: &TenantId,
        id: u64,
        expected: Option<&Generation>,
    ) -> StoreResult<()> {
        self.inner.restore_snapshot(tenant, id, expected)
    }

    fn reset(&self) -> StoreResult<()> {
        self.inner.reset()
    }

    fn reset_tenant(&self, tenant: &TenantId) -> StoreResult<()> {
        self.inner.reset_tenant(tenant)
    }

    fn jobs_create(
        &self,
        tenant: &TenantId,
        jobs: &[NewJob],
        now: Timestamp,
    ) -> StoreResult<Vec<String>> {
        self.inner.jobs_create(tenant, jobs, now)
    }

    fn job(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<Job>> {
        self.inner.job(tenant, id)
    }

    fn jobs_list(
        &self,
        tenant: &TenantId,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<Job>> {
        self.inner.jobs_list(tenant, before, limit)
    }

    fn job_lease(
        &self,
        tenant: &TenantId,
        worker: &str,
        now: Timestamp,
    ) -> StoreResult<Option<Lease>> {
        self.inner.job_lease(tenant, worker, now)
    }

    fn job_report(
        &self,
        tenant: &TenantId,
        id: &str,
        token: u64,
        report: &JobReport,
        writes: &WorkerWrites,
        now: Timestamp,
    ) -> StoreResult<Reported> {
        self.inner
            .job_report(tenant, id, token, report, writes, now)
    }

    fn job_cancel(
        &self,
        tenant: &TenantId,
        id: &str,
        reason: &str,
        now: Timestamp,
    ) -> StoreResult<JobChange> {
        self.inner.job_cancel(tenant, id, reason, now)
    }

    fn job_retry(&self, tenant: &TenantId, id: &str, now: Timestamp) -> StoreResult<JobChange> {
        self.inner.job_retry(tenant, id, now)
    }

    fn jobs_due(&self, now: Timestamp, limit: usize) -> StoreResult<Vec<TenantId>> {
        self.inner.jobs_due(now, limit)
    }

    fn automation_write(
        &self,
        tenant: &TenantId,
        writes: &WorkerWrites,
        now: Timestamp,
    ) -> StoreResult<()> {
        self.inner.automation_write(tenant, writes, now)
    }

    fn automation_run(&self, tenant: &TenantId, run: &str) -> StoreResult<Option<RunRecord>> {
        self.inner.automation_run(tenant, run)
    }

    fn automation_runs(
        &self,
        tenant: &TenantId,
        automation: Option<&WorkflowId>,
        state: Option<RunState>,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<RunRecord>> {
        self.inner
            .automation_runs(tenant, automation, state, before, limit)
    }

    fn automation_steps(
        &self,
        tenant: &TenantId,
        run: &str,
        after: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<StepRecord>> {
        self.inner.automation_steps(tenant, run, after, limit)
    }

    fn automation_exceptions(
        &self,
        tenant: &TenantId,
        kind: Option<ExceptionKind>,
        unsettled: bool,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<ExceptionRecord>> {
        self.inner
            .automation_exceptions(tenant, kind, unsettled, before, limit)
    }

    fn automation_exception_settle(
        &self,
        tenant: &TenantId,
        id: &str,
        release: bool,
        now: Timestamp,
    ) -> StoreResult<Settled> {
        self.inner
            .automation_exception_settle(tenant, id, release, now)
    }

    fn begin_provisioning(&self, tenant: &TenantId) -> StoreResult<()> {
        self.inner.begin_provisioning(tenant)
    }

    fn finish_provisioning(&self, tenant: &TenantId) -> StoreResult<()> {
        self.inner.finish_provisioning(tenant)
    }

    fn rollback_provisioning(&self) -> StoreResult<Vec<TenantId>> {
        self.inner.rollback_provisioning()
    }

    fn segment_catalog(&self, tenant: &TenantId) -> StoreResult<Vec<seg::Segment>> {
        self.inner.segment_catalog(tenant)
    }

    fn segment_rows(
        &self,
        tenant: &TenantId,
        pairs: &[(seg::SegmentId, PrincipalId)],
    ) -> StoreResult<seg::Rows> {
        self.inner.segment_rows(tenant, pairs)
    }

    fn segment_members(
        &self,
        tenant: &TenantId,
        segment: seg::SegmentId,
        after: Option<&PrincipalId>,
        limit: usize,
    ) -> StoreResult<Vec<seg::Membership>> {
        self.inner.segment_members(tenant, segment, after, limit)
    }

    fn segment_memberships_of(
        &self,
        tenant: &TenantId,
        person: &PrincipalId,
    ) -> StoreResult<Vec<seg::Membership>> {
        self.inner.segment_memberships_of(tenant, person)
    }

    fn segment_wakes_of(
        &self,
        tenant: &TenantId,
        person: &PrincipalId,
    ) -> StoreResult<Vec<seg::Wake>> {
        self.inner.segment_wakes_of(tenant, person)
    }

    fn segment_due(
        &self,
        tenant: &TenantId,
        segment: seg::SegmentId,
        at: Timestamp,
        limit: usize,
    ) -> StoreResult<Vec<seg::Wake>> {
        self.inner.segment_due(tenant, segment, at, limit)
    }

    fn segment_batches(&self, tenant: &TenantId) -> StoreResult<Vec<seg::Batch>> {
        self.inner.segment_batches(tenant)
    }

    fn segment_subject_count(&self, tenant: &TenantId, object: &ObjectTypeId) -> StoreResult<u64> {
        self.inner.segment_subject_count(tenant, object)
    }

    fn segment_events_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<(u64, seg::SegmentEvent)>> {
        self.inner.segment_events_after(tenant, after, limit)
    }

    fn commit_segments(&self, commit: &SegmentCommit) -> StoreResult<Receipt> {
        self.inner.commit_segments(commit)
    }

    fn object_revisions(
        &self,
        tenant: &TenantId,
        objects: &[ObjectTypeId],
    ) -> StoreResult<BTreeMap<ObjectTypeId, u64>> {
        self.inner.object_revisions(tenant, objects)
    }

    fn tenant_plan(&self, tenant: &TenantId) -> StoreResult<Option<String>> {
        self.inner.tenant_plan(tenant)
    }

    fn set_tenant_plan(&self, tenant: &TenantId, plan: &str) -> StoreResult<()> {
        self.inner.set_tenant_plan(tenant, plan)
    }

    fn stored_bytes(&self, tenant: &TenantId) -> StoreResult<u64> {
        self.inner.stored_bytes(tenant)
    }

    fn schedule_set(
        &self,
        tenant: &TenantId,
        set: &ScheduleSet,
        now: Timestamp,
    ) -> StoreResult<Schedule> {
        self.inner.schedule_set(tenant, set, now)
    }

    fn schedule(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<Schedule>> {
        self.inner.schedule(tenant, id)
    }

    fn schedules(&self, tenant: &TenantId) -> StoreResult<Vec<Schedule>> {
        self.inner.schedules(tenant)
    }

    fn run_key_holder(
        &self,
        tenant: &TenantId,
        automation: &WorkflowId,
        key: &str,
    ) -> StoreResult<Option<String>> {
        self.inner.run_key_holder(tenant, automation, key)
    }

    fn open_run_key_starts(
        &self,
        tenant: &TenantId,
        automation: &WorkflowId,
        key: &str,
    ) -> StoreResult<Vec<Job>> {
        self.inner.open_run_key_starts(tenant, automation, key)
    }

    fn purge_automation(
        &self,
        now: Timestamp,
        retention: &AutomationRetention,
    ) -> StoreResult<AutomationPurged> {
        self.inner.purge_automation(now, retention)
    }

    fn platform_settings(&self) -> StoreResult<Vec<PlatformSetting>> {
        self.inner.platform_settings()
    }

    fn platform_setting_write(
        &self,
        setting: &SettingId,
        value: Option<&Value>,
        expected_revision: u64,
        at: Timestamp,
    ) -> StoreResult<PlatformWrite> {
        self.inner
            .platform_setting_write(setting, value, expected_revision, at)
    }

    fn segment_cursor(&self, tenant: &TenantId) -> StoreResult<u64> {
        self.inner.segment_cursor(tenant)
    }

    /// A probe of a field that holds sealed values cannot use the inner store's index, which holds
    /// the tokens: it is answered from the opened records of the type, by the kernel's own
    /// definition of a probe. Any other probe is the inner store's.
    fn search_candidates(
        &self,
        tenant: &TenantId,
        probe: &strata_kernel::search::Probe,
    ) -> StoreResult<Vec<strata_kernel::search::Candidate>> {
        let header = self.inner.load(tenant, &[])?;
        let Some(head) = header.tenant.as_deref() else {
            return Ok(Vec::new());
        };
        let plan = Plan::of(&head.schema);
        let sealed = plan
            .objects
            .get(&probe.object)
            .is_some_and(|fields| probe.fields.iter().any(|field| fields.contains_key(field)));
        if !sealed {
            return self.inner.search_candidates(tenant, probe);
        }
        let state = self.load(tenant, &[Need::AllOf(probe.object.clone())])?;
        let Some(opened) = state.tenant.as_deref() else {
            return Ok(Vec::new());
        };
        let Some(def) = opened.schema.objects.get(&probe.object) else {
            return Ok(Vec::new());
        };
        use strata_kernel::ext::TenantStateExt;
        Ok(strata_kernel::search::candidates_in(
            opened.records_of(&probe.object),
            def,
            probe,
        ))
    }
}
