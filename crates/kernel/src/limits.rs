//! The stated maximums on what a tenant can make large (KRN-1280 to KRN-1289). Every number the
//! kernel enforces is here and nowhere else; the specification states the same numbers.

use crate::types::Value;

/// The longest string value, in bytes.
pub const MAX_STRING_BYTES: usize = 65_536;
/// The most items in one list value.
pub const MAX_LIST_LEN: usize = 1_000;
/// The largest record, in bytes of its JSON encoding.
pub const MAX_RECORD_BYTES: usize = 262_144;
/// The most fields one object type may define.
pub const MAX_FIELDS_PER_OBJECT: usize = 100;
/// The most object types a schema may define.
pub const MAX_OBJECTS: usize = 200;
/// The most policies a schema may define.
pub const MAX_POLICIES: usize = 500;
/// The most named queries a schema may define.
pub const MAX_QUERIES: usize = 500;
/// The most changes one `ChangeSchema` intent may carry.
pub const MAX_SCHEMA_CHANGES: usize = 200;
/// The most ids one `Records` read may name.
pub const MAX_READ_IDS: usize = 200;
/// The longest JSON encoding of a value in a unique field, in bytes. It is below the storage
/// index's own cut-off, so a unique lookup is always an index lookup.
pub const MAX_UNIQUE_BYTES: usize = 200;
/// The page size of a named query read that names none and whose query declares no limit.
pub const DEFAULT_PAGE_SIZE: u32 = 50;
/// The largest page size a named query read may ask for, and the largest `limit` a named query
/// may declare.
pub const MAX_PAGE_SIZE: u32 = 200;
/// The most records a named query that is not bounded by its order may examine.
pub const MAX_SCAN_RECORDS: u32 = 10_000;
/// The most triggers a schema may define (CT-790).
pub const MAX_TRIGGERS: usize = 100;
/// The most steps one automation may declare (automation-def, CT-792).
pub const MAX_AUTOMATION_STEPS: usize = 50;
/// The most attempts the retry policy of one automation step may name (D-6529).
pub const MAX_STEP_ATTEMPTS: u32 = 10;
/// The longest delay between two attempts of an automation step, in milliseconds: one day (D-6529).
pub const MAX_STEP_DELAY_MS: u64 = 86_400_000;
/// The longest time one attempt of an automation step may take, in milliseconds: fifteen
/// minutes (D-6529).
pub const MAX_STEP_TIMEOUT_MS: u64 = 900_000;
/// The longest wait of an automation step, in milliseconds: 366 days (D-6535, WFL-1537).
pub const MAX_WAIT_MS: u64 = 366 * 86_400_000;
/// The most members one batch of a fan-out step handles (D-6535, WFL-1538).
pub const MAX_FAN_OUT_BATCH: u32 = 100;
/// The most steps one tenant operation may declare (D-6548, OPR-1549).
pub const MAX_OPERATION_STEPS: usize = 16;
/// The most posting steps one tenant operation may declare (D-6548, OPR-1549).
pub const MAX_OPERATION_POSTINGS: usize = 8;
/// The most nodes one condition of a tenant operation may have (D-6548, OPR-1549).
pub const MAX_CONDITION_NODES: usize = 200;
/// The most setting definitions a schema may hold (D-5046).
pub const MAX_SETTINGS: usize = 500;
/// The most capabilities a schema may declare (D-6590).
pub const MAX_CAPABILITIES: usize = 200;
/// The most setting overrides a tenant may hold at every level together. The overrides travel
/// with the tenant's state, so the number is bounded.
pub const MAX_SETTING_OVERRIDES: usize = 100_000;
/// The fewest fields of one composite unique constraint (D-6558, CMS-1556).
pub const MIN_UNIQUE_FIELDS: usize = 2;
/// The most fields of one composite unique constraint (D-6558, CMS-1556).
pub const MAX_UNIQUE_FIELDS: usize = 4;

/// The first limit a value breaks, as a description, or `None`.
pub fn value_problem(value: &Value) -> Option<String> {
    match value {
        Value::Str(s) if s.len() > MAX_STRING_BYTES => Some(format!(
            "a string of {} bytes is longer than the maximum of {MAX_STRING_BYTES}",
            s.len()
        )),
        Value::List(items) if items.len() > MAX_LIST_LEN => Some(format!(
            "a list of {} items is longer than the maximum of {MAX_LIST_LEN}",
            items.len()
        )),
        Value::List(items) => items.iter().find_map(value_problem),
        _ => None,
    }
}

/// The JSON encoding of a value, as the storage index keys it.
pub fn encoded_len(value: &Value) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |s| s.len())
}
