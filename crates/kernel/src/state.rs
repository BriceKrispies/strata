//! Logical state: records, a tenant's state, and the snapshot a transition runs against.

use crate::ids::{
    FieldId, ObjectTypeId, PrincipalId, RecordId, RoleId, StateRevision, TenantId, Timestamp,
};
use crate::schema::TenantSchema;
use crate::value::Value;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Who is acting.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Principal {
    /// The platform itself. May create tenants and nothing else.
    System,
    /// An unauthenticated visitor. Holds only the `public` role.
    Anonymous,
    User(PrincipalId),
}

/// A record of some object type. Absent fields are `null`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: RecordId,
    pub object: ObjectTypeId,
    /// Rises by one per accepted update. Used for optimistic concurrency.
    pub version: u64,
    pub created_by: Option<PrincipalId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub fields: BTreeMap<FieldId, Value>,
}

static NULL: Value = Value::Null;

impl Record {
    pub fn get(&self, field: &FieldId) -> &Value {
        self.fields.get(field).unwrap_or(&NULL)
    }
}

pub type Records = BTreeMap<ObjectTypeId, BTreeMap<RecordId, Arc<Record>>>;

/// Everything a tenant owns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantState {
    pub id: TenantId,
    pub name: String,
    pub revision: StateRevision,
    /// The id the next created record receives.
    pub next_record: u64,
    /// The sequence number of the last emitted domain event.
    pub event_seq: u64,
    pub schema: Arc<TenantSchema>,
    pub members: BTreeMap<PrincipalId, BTreeSet<RoleId>>,
    pub records: Records,
}

impl TenantState {
    pub fn record(&self, object: &ObjectTypeId, id: RecordId) -> Option<&Arc<Record>> {
        self.records.get(object).and_then(|records| records.get(&id))
    }

    pub fn records_of(&self, object: &ObjectTypeId) -> impl Iterator<Item = &Arc<Record>> {
        self.records.get(object).into_iter().flat_map(|m| m.values())
    }

    pub fn record_count(&self) -> usize {
        self.records.values().map(BTreeMap::len).sum()
    }
}

/// A piece of state a decision depends on.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Need {
    /// One record, if it exists.
    Record { object: ObjectTypeId, id: RecordId },
    /// Every record of `object` whose `field` equals `value` (or, for a list field, contains it).
    ByField { object: ObjectTypeId, field: FieldId, value: Value },
    /// Every record of `object`.
    AllOf(ObjectTypeId),
}

/// Which records a snapshot is known to contain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Coverage {
    /// Every record of the tenant.
    Complete,
    /// Exactly the records answering these needs.
    Partial(BTreeSet<Need>),
}

impl Coverage {
    pub fn covers(&self, need: &Need) -> bool {
        match self {
            Coverage::Complete => true,
            Coverage::Partial(loaded) => {
                loaded.contains(need)
                    || match need {
                        Need::Record { object, .. } | Need::ByField { object, .. } => {
                            loaded.contains(&Need::AllOf(object.clone()))
                        }
                        Need::AllOf(_) => false,
                    }
            }
        }
    }
}

/// The snapshot of one tenant that a transition or read runs against.
///
/// `tenant` is `None` when the store attests that the tenant does not exist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub tenant_id: TenantId,
    pub tenant: Option<Arc<TenantState>>,
    pub coverage: Coverage,
}

impl State {
    pub fn complete(tenant: Arc<TenantState>) -> Self {
        State { tenant_id: tenant.id.clone(), tenant: Some(tenant), coverage: Coverage::Complete }
    }

    pub fn absent(tenant_id: TenantId) -> Self {
        State { tenant_id, tenant: None, coverage: Coverage::Complete }
    }
}
