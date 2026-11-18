//! What a caller asks for, the context it asks in, and what the kernel answers with.

use crate::ids::{
    ConstraintId, FieldId, ObjectTypeId, PolicyId, PrincipalId, QueryId, RecordId, RoleId,
    SchemaVersion, SessionId, StateRevision, TenantId, Timestamp, ViewId, WorkflowId,
};
use crate::schema::{
    ConstraintDef, FieldDef, PolicyDef, QueryDef, RoleDef, TenantSchema, ViewDef, WorkflowDef,
};
use crate::state::{Need, Principal, Record};
use crate::value::Value;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

/// The explicit environment of a transition. Nothing else reaches the kernel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    pub tenant: TenantId,
    pub principal: Principal,
    pub session: Option<SessionId>,
    pub now: Timestamp,
    /// Follows one user intent from the browser through to the resulting events.
    pub correlation: String,
}

/// One edit to a tenant's schema. Every `Define*` is define-or-replace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaChange {
    DefineObject {
        id: ObjectTypeId,
        label: String,
    },
    DefineField {
        object: ObjectTypeId,
        field: FieldDef,
    },
    /// A reference field. Equivalent to `DefineField` with a `Ref` type; kept separate so a
    /// relationship reads as one in a change list.
    DefineRelationship {
        object: ObjectTypeId,
        field: FieldId,
        label: String,
        target: ObjectTypeId,
        on_delete: crate::value::OnDelete,
        required: bool,
    },
    RemoveField {
        object: ObjectTypeId,
        field: FieldId,
    },
    DefineConstraint {
        object: ObjectTypeId,
        constraint: ConstraintDef,
    },
    RemoveConstraint {
        object: ObjectTypeId,
        constraint: ConstraintId,
    },
    DefineRole(RoleDef),
    DefinePolicy(PolicyDef),
    RemovePolicy(PolicyId),
    DefineWorkflow(WorkflowDef),
    RemoveWorkflow(WorkflowId),
    DefineQuery(QueryDef),
    RemoveQuery(QueryId),
    DefineView(ViewDef),
    RemoveView(ViewId),
}

/// How a schema change treats records that do not conform to the new schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExistingData {
    /// Reject the change if any existing record would not conform.
    Reject,
    /// Accept the change. Non-conforming records stay as they are and are listed in the
    /// evidence; they fail validation the next time they are written.
    Grandfather,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Intent {
    CreateTenant {
        name: String,
        owner: PrincipalId,
    },
    ChangeSchema {
        changes: Vec<SchemaChange>,
        expected_version: Option<SchemaVersion>,
        existing: ExistingData,
    },
    AssignRole {
        principal: PrincipalId,
        role: RoleId,
    },
    RevokeRole {
        principal: PrincipalId,
        role: RoleId,
    },
    CreateRecord {
        object: ObjectTypeId,
        fields: BTreeMap<FieldId, Value>,
    },
    /// A patch: listed fields are replaced, `null` clears a field, other fields are kept.
    UpdateRecord {
        object: ObjectTypeId,
        id: RecordId,
        fields: BTreeMap<FieldId, Value>,
        expected_version: Option<u64>,
    },
    DeleteRecord {
        object: ObjectTypeId,
        id: RecordId,
        expected_version: Option<u64>,
    },
}

impl Intent {
    pub fn name(&self) -> &'static str {
        match self {
            Intent::CreateTenant { .. } => "create_tenant",
            Intent::ChangeSchema { .. } => "change_schema",
            Intent::AssignRole { .. } => "assign_role",
            Intent::RevokeRole { .. } => "revoke_role",
            Intent::CreateRecord { .. } => "create_record",
            Intent::UpdateRecord { .. } => "update_record",
            Intent::DeleteRecord { .. } => "delete_record",
        }
    }
}

/// One evaluated rule. The list of these explains an acceptance or a rejection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// What was checked, such as `policy:authors_edit_posts` or `required:post.title`.
    pub rule: String,
    pub passed: bool,
    pub detail: String,
}

impl Check {
    pub fn pass(rule: impl Into<String>, detail: impl Into<String>) -> Self {
        Check {
            rule: rule.into(),
            passed: true,
            detail: detail.into(),
        }
    }

    pub fn fail(rule: impl Into<String>, detail: impl Into<String>) -> Self {
        Check {
            rule: rule.into(),
            passed: false,
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ErrorKind {
    TenantMismatch,
    TenantExists,
    TenantNotFound,
    Unauthorized,
    UnknownObject,
    UnknownField,
    UnknownRecord,
    UnknownRole,
    UnknownQuery,
    TypeMismatch,
    MissingRequired,
    ConstraintViolated,
    UniqueViolated,
    DanglingReference,
    StillReferenced,
    WorkflowViolation,
    VersionConflict,
    SchemaConflict,
    InvalidSchema,
    MigrationRejected,
    InsufficientState,
    Evaluation,
    InvalidIntent,
}

/// A rejection, with the checks that led to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainError {
    pub kind: ErrorKind,
    pub message: String,
    pub evidence: Vec<Check>,
    /// For `InsufficientState`: the piece of state the snapshot lacked.
    pub missing: Option<Need>,
}

impl DomainError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        DomainError {
            kind,
            message: message.into(),
            evidence: Vec::new(),
            missing: None,
        }
    }

    pub fn with(mut self, evidence: Vec<Check>) -> Self {
        self.evidence = evidence;
        self
    }
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

/// One change to a tenant's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeltaOp {
    CreateTenant {
        name: String,
    },
    PutSchema(Arc<TenantSchema>),
    /// Set a principal's roles. An empty set removes the member.
    PutMember {
        principal: PrincipalId,
        roles: BTreeSet<RoleId>,
    },
    PutRecord(Arc<Record>),
    DeleteRecord {
        object: ObjectTypeId,
        id: RecordId,
    },
}

/// The logical change of one accepted transition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateDelta {
    pub tenant: TenantId,
    /// The revision this delta was computed against. `None` means the tenant must not exist.
    pub base_revision: Option<StateRevision>,
    pub new_revision: StateRevision,
    pub next_record: u64,
    pub event_seq: u64,
    pub ops: Vec<DeltaOp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventBody {
    TenantCreated {
        name: String,
    },
    SchemaChanged {
        version: SchemaVersion,
        summary: Vec<String>,
        grandfathered: u64,
    },
    MemberChanged {
        principal: PrincipalId,
        roles: BTreeSet<RoleId>,
    },
    RecordCreated {
        record: Arc<Record>,
    },
    RecordUpdated {
        before: Arc<Record>,
        after: Arc<Record>,
    },
    RecordDeleted {
        before: Arc<Record>,
    },
}

/// A fact about an accepted transition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainEvent {
    pub tenant: TenantId,
    /// Gapless per tenant, starting at 1.
    pub seq: u64,
    pub revision: StateRevision,
    pub at: Timestamp,
    pub principal: Principal,
    pub correlation: String,
    pub body: EventBody,
}

/// A request for something to happen outside the kernel. Adapters execute these after commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    /// Deliver the event with this sequence number to subscribers.
    Publish { seq: u64 },
    /// End every session of a principal who no longer holds a role in the tenant.
    RevokeSessions { principal: PrincipalId },
}

/// What an accepted transition produced, for the caller's reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Output {
    Tenant,
    Schema {
        version: SchemaVersion,
    },
    Member,
    Record {
        object: ObjectTypeId,
        id: RecordId,
        version: u64,
    },
    Deleted {
        object: ObjectTypeId,
        id: RecordId,
    },
}

/// The result of an accepted transition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub delta: StateDelta,
    pub events: Vec<DomainEvent>,
    pub effects: Vec<Effect>,
    pub evidence: Vec<Check>,
    pub output: Output,
}

/// A read request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadRequest {
    Named {
        query: QueryId,
        params: BTreeMap<String, Value>,
    },
    Records {
        object: ObjectTypeId,
        ids: Vec<RecordId>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadResult {
    pub rows: Vec<Arc<Record>>,
    pub revision: StateRevision,
    pub schema_version: SchemaVersion,
}
