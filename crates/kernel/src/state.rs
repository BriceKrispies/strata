//! Logical state: records, a tenant's state, and the snapshot a transition runs against.
//!
//! The types are generated ([`crate::types`]); this module holds what the kernel does with them.

pub use crate::types::{Coverage, Need, Principal, Record, State, TenantState};
use crate::types::{FieldId, ObjectTypeId, RecordId, TenantId, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

static NULL: Value = Value::Null;

/// The records of a tenant, by object type and id.
pub type Records = BTreeMap<ObjectTypeId, BTreeMap<RecordId, Arc<Record>>>;

/// The records of one object type, in id order.
pub type RecordsOf<'a> = std::iter::Flatten<
    std::option::IntoIter<std::collections::btree_map::Values<'a, RecordId, Arc<Record>>>,
>;

/// What the kernel asks of a [`Record`].
pub trait RecordExt {
    /// The value of a field; an absent field is `null`.
    fn get(&self, field: &FieldId) -> &Value;
}

impl RecordExt for Record {
    fn get(&self, field: &FieldId) -> &Value {
        self.fields.get(field).unwrap_or(&NULL)
    }
}

/// What the kernel asks of a [`TenantState`].
pub trait TenantStateExt {
    /// One record, if the state holds it.
    fn record(&self, object: &ObjectTypeId, id: RecordId) -> Option<&Arc<Record>>;
    /// Every record of an object type the state holds.
    fn records_of(&self, object: &ObjectTypeId) -> RecordsOf<'_>;
    /// How many records the state holds.
    fn record_count(&self) -> usize;
}

impl TenantStateExt for TenantState {
    fn record(&self, object: &ObjectTypeId, id: RecordId) -> Option<&Arc<Record>> {
        self.records
            .get(object)
            .and_then(|records| records.get(&id))
    }

    fn records_of(&self, object: &ObjectTypeId) -> RecordsOf<'_> {
        self.records
            .get(object)
            .map(BTreeMap::values)
            .into_iter()
            .flatten()
    }

    fn record_count(&self) -> usize {
        self.records.values().map(BTreeMap::len).sum()
    }
}

/// What the kernel asks of a [`Coverage`].
pub trait CoverageExt {
    /// Whether a snapshot with this coverage holds the answer to `need`.
    fn covers(&self, need: &Need) -> bool;
}

impl CoverageExt for Coverage {
    fn covers(&self, need: &Need) -> bool {
        match self {
            Coverage::Complete => true,
            Coverage::Partial(loaded) => {
                loaded.contains(need)
                    || match need {
                        Need::Record { object, .. }
                        | Need::ByField { object, .. }
                        | Need::ByFields { object, .. }
                        | Need::PageOf { object, .. }
                        | Need::IndexPage { object, .. }
                        | Need::PairPage { object, .. } => {
                            loaded.contains(&Need::AllOf(object.clone()))
                        }
                        Need::AllOf(_) | Need::Ledger(_) => false,
                    }
            }
        }
    }
}

/// The snapshots a store hands the kernel. `tenant` is `None` when the store attests that the
/// tenant does not exist.
pub trait StateExt {
    /// A snapshot holding the whole tenant.
    fn complete(tenant: Arc<TenantState>) -> Self;
    /// A snapshot attesting that the tenant does not exist.
    fn absent(tenant_id: TenantId) -> Self;
}

impl StateExt for State {
    fn complete(tenant: Arc<TenantState>) -> Self {
        State {
            tenant_id: tenant.id.clone(),
            tenant: Some(tenant),
            coverage: Coverage::Complete,
        }
    }

    fn absent(tenant_id: TenantId) -> Self {
        State {
            tenant_id,
            tenant: None,
            coverage: Coverage::Complete,
        }
    }
}
