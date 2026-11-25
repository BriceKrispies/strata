//! How a named query is served (KRN-1286, KRN-1287, KRN-2520 to KRN-2526).
//!
//! A query is analysed when it is defined into one of three plans:
//!
//! - **Window**: no filter, ordered by nothing or by the record id ascending. A window of at most
//!   200 records of the type in id order after a cursor (`Need::PageOf`, KRN-1286).
//! - **Index**: the filter has, at its top level (alone or as one conjunct of an `and`), a
//!   condition one field's index answers, and the order is one the index walks in:
//!   - `field == operand` or `field in list`, ordered by nothing, by the record id ascending or
//!     by that field: the candidates of each value are in record id order;
//!   - one or two of `field < operand`, `<=`, `>`, `>=` on an integer or time field, ordered by
//!     that field, ascending or descending.
//!
//!   An operand is a literal, a parameter or `principal`. The field is a declared field of the
//!   object type that is not a list and has no read condition. The candidates come from
//!   `Need::IndexPage`: the records whose value of the field lies between two bounds, in the order
//!   of the value (descending when the query orders so) and then the record id, after a position,
//!   at most 200 at a time. The kernel then applies the whole filter, the read authorization and
//!   the field redaction to each candidate, exactly as a scan does, so the rows are those a scan
//!   returns, in the same order.
//! - **Pair index** (KRN-2920, D-7920): `field == operand` or `field in list` on one field,
//!   ordered by another attribute (an integer or time field, `$created_at` or `$updated_at`),
//!   ascending or descending: "posts by this author, newest first". The candidates come from
//!   `Need::PairPage`: the records whose value of the field is one value, in the order of the
//!   other attribute (a missing or `null` value included, first when ascending) and then the
//!   record id, after a position, at most 200 at a time; a list of values is one stream per value,
//!   merged in that order (at most [`MAX_IN_VALUES`] streams). The store keeps an index for
//!   exactly the pairs some defined query reads ([`index_pairs`]), built in the commit that defines
//!   the first such query and dropped with the last one, so writes cost what tenants query.
//! - **Scan**: everything else. Every record of the type is loaded (refused above 10,000,
//!   KRN-1288), filtered and sorted in memory, and the cursor is an offset. The plan says why, and
//!   a schema change that leaves a query a scan says so in its evidence (KRN-2524).
//!
//! **Paging under partial visibility** (KRN-2521). A reader may see few of the candidates, so a
//! planned read pages through candidates until its page is full, the candidates end, or it has
//! examined [`MAX_PLAN_CANDIDATES`] of them. The cursor carries the index position (the value of
//! the field and the record id) and never a count, so it resumes exactly after the last position
//! examined, whatever was inserted or deleted in between: no gap and no repeat.
//!
//! **What a planned read does not reveal** (KRN-2522). A field with a read condition is never
//! planned on. Filters and orders of every named query are evaluated on the record as the reader
//! receives it, after the fields the reader may not read are removed, so a hidden field takes no
//! part in which rows come back or in their order. A cursor of a query ordered by a field carries
//! only the position of a record the reader may read; when a request examined its whole bound
//! without reaching one, it is answered by the scan from where it started, with an offset cursor.
//!
//! **Index keys** (KRN-2525). Equality is answered from the canonical JSON key of the value,
//! which the store holds for every value of at most [`INDEX_KEY_BYTES`] bytes: a probe whose value
//! is longer is answered by a scan. Ranges are answered only for integers and times, whose keys
//! are their numbers. Strings are compared byte by byte, which is the order of Rust's `str` and
//! SQLite's `BINARY` collation; they are not range-planned, because a string longer than the key
//! limit has no key and a range or order over it would miss it. The pair index orders only
//! `null`, integers and times, whose keys are numbers; an equality value longer than the key limit
//! has no entry, and no probe the index serves can equal it. A record whose order value is of
//! another kind (a value a schema change kept) cannot be placed in the order: the store then
//! answers that pair's pages from the records of the type by [`pair_page`], so it is still found.
//!
//! **Loading what a read lacks** (KRN-2922, WG-5180). A filter, a read policy or a field's read
//! condition may follow a reference into a record the snapshot lacks. The read then fails with
//! `InsufficientState` naming that record, and [`read_missing`] names every record each row the
//! read examines refers to that way, together with the next page of candidates, so the shell loads
//! them in one round: exactly those records, never their whole object type.

use crate::expr::{self, Env};
use crate::intent::{
    Check, CheckExt, Context, DomainError, DomainErrorExt, ErrorKind, ReadRequest, ReadResult,
    SchemaChange,
};
use crate::limits;
use crate::read::{redact, visible_in};
use crate::rules::{insufficient, missing_error, roles_of, Lookup};
use crate::schema::{Action, FieldDef, FieldType, ObjectTypeDef, QueryDef, Resource, TenantSchema};
use crate::state::{CoverageExt, Need, Principal, Record, RecordExt, State, TenantStateExt};
use crate::types::{
    CmpOp, Expr, FieldId, FieldTypeExt, ObjectTypeId, OrderKey, PageCursor, QueryId, RecordId,
    RoleId, Value,
};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

pub use strata_model_core::types::{IndexBound, IndexPosition};

/// The longest canonical JSON of a value the store's record index holds a key for, in bytes. A
/// value longer than this has no index entry. The SQLite adapter's cut-off is this constant.
pub const INDEX_KEY_BYTES: usize = 240;

/// The candidates one `Need::IndexPage` asks for.
pub const INDEX_PAGE: u32 = limits::MAX_PAGE_SIZE;

/// The most candidates one planned read examines (KRN-2521): eight pages of candidates.
pub const MAX_PLAN_CANDIDATES: u32 = 8 * INDEX_PAGE;

/// The most values of a `field in list` probe; a longer list is answered by a scan.
pub const MAX_IN_VALUES: usize = 16;

/// What an index probe compares the field with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operand {
    Lit(Value),
    Param(String),
    /// The reading principal's id; `null` for the system and anonymous readers.
    Principal,
}

/// The condition of an index plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// `field == operand`.
    Eq(Operand),
    /// `field in [literal, ...]`.
    InList(Vec<Value>),
    /// `field in param`.
    InParam(String),
    /// `field <op> operand` for each bound, the field on the left.
    Range(Vec<(CmpOp, Operand)>),
}

/// The order an index plan returns rows in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanOrder {
    /// The record id ascending.
    Id,
    /// The value of the plan's field, then the record id ascending.
    Field { descending: bool },
    /// The value of another attribute (an integer or time field, `$created_at` or `$updated_at`),
    /// then the record id ascending; the probe is an equality or a list of values on the plan's
    /// field, and the candidates come from the pair index (field, attribute) (KRN-2920).
    By { attribute: String, descending: bool },
}

impl PlanOrder {
    /// Whether the plan's values come in descending order.
    pub fn descending(&self) -> bool {
        match self {
            PlanOrder::Id => false,
            PlanOrder::Field { descending } | PlanOrder::By { descending, .. } => *descending,
        }
    }
}

/// A pair index a store keeps (KRN-2921): the records of `object` by their value of `field`, each
/// value's records in the order of the attribute `order` and then the record id.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IndexPair {
    pub object: ObjectTypeId,
    pub field: FieldId,
    pub order: String,
}

/// A query served by the index of one field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexPlan {
    pub field: FieldId,
    pub probe: Probe,
    pub order: PlanOrder,
}

/// How a named query is served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryPlan {
    /// A window of the object type in id order (KRN-1286).
    Window,
    /// Candidates from the index of one field (KRN-2520).
    Index(IndexPlan),
    /// The bounded scan of the object type, and why the query is not served otherwise.
    Scan(String),
}

impl QueryPlan {
    pub fn is_scan(&self) -> bool {
        matches!(self, QueryPlan::Scan(_))
    }
}

// ---------------------------------------------------------------------------------------------
// The planner
// ---------------------------------------------------------------------------------------------

/// One top-level condition of a filter, as an index sees it.
enum Atom {
    Eq(Operand),
    In(Probe),
    Bound(CmpOp, Operand),
}

fn conjuncts<'e>(filter: &'e Expr, out: &mut Vec<&'e Expr>) {
    match filter {
        Expr::And(items) => {
            for item in items {
                conjuncts(item, out);
            }
        }
        other => out.push(other),
    }
}

fn field_of(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Var(_, name) => Some(name.as_str()),
        _ => None,
    }
}

/// A value an index holds one key for: not `null` and not a list.
fn scalar(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::List(_))
}

fn operand_of(expr: &Expr, def: &QueryDef) -> Option<Operand> {
    match expr {
        Expr::Lit(value) if scalar(value) => Some(Operand::Lit(value.clone())),
        Expr::Param(name) if def.params.contains(name) => Some(Operand::Param(name.clone())),
        Expr::Principal => Some(Operand::Principal),
        _ => None,
    }
}

/// The comparison with its sides swapped: `a < b` is `b > a`.
fn flip(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Lt => CmpOp::Gt,
        CmpOp::Le => CmpOp::Ge,
        CmpOp::Gt => CmpOp::Lt,
        CmpOp::Ge => CmpOp::Le,
        other => other,
    }
}

fn atom_of<'e>(expr: &'e Expr, def: &QueryDef) -> Option<(&'e str, Atom)> {
    match expr {
        Expr::Cmp(op, a, b) => {
            let (name, operand, op) = match (field_of(a), field_of(b)) {
                (Some(name), None) => (name, operand_of(b, def)?, *op),
                (None, Some(name)) => (name, operand_of(a, def)?, flip(*op)),
                _ => return None,
            };
            match op {
                CmpOp::Eq => Some((name, Atom::Eq(operand))),
                CmpOp::Ne => None,
                _ => Some((name, Atom::Bound(op, operand))),
            }
        }
        Expr::In(a, b) => {
            let name = field_of(a)?;
            match &**b {
                Expr::Lit(Value::List(items))
                    if items.iter().all(|item| !matches!(item, Value::List(_))) =>
                {
                    Some((name, Atom::In(Probe::InList(items.clone()))))
                }
                Expr::Param(param) if def.params.contains(param) => {
                    Some((name, Atom::In(Probe::InParam(param.clone()))))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The field an index plan may use, or why not.
fn indexable<'s>(object: &'s ObjectTypeDef, name: &str) -> Result<(FieldId, &'s FieldDef), String> {
    if name.starts_with('$') {
        return Err(format!(
            "{name} is a system attribute, which the record index does not hold"
        ));
    }
    let id = FieldId::new(name).map_err(|_| format!("{name} is not a field name"))?;
    let def = object
        .fields
        .get(&id)
        .ok_or_else(|| format!("{name} is not a field of {}", object.id))?;
    if def.read.is_some() {
        return Err(format!(
            "{name} has a read condition, and candidates taken from its index would show through \
             the order and the page boundaries what a reader may not read"
        ));
    }
    if matches!(def.ty, FieldType::List(_)) {
        return Err(format!(
            "{name} is a list field, whose index holds its items, not its value"
        ));
    }
    Ok((id, def))
}

/// The index orders the values of integer and time fields (KRN-2525).
fn rangeable(ty: &FieldType) -> bool {
    matches!(ty, FieldType::Int | FieldType::Time)
}

/// The plan of a named query (KRN-2520). Pure: a function of the schema and the definition.
pub fn plan_query(schema: &TenantSchema, def: &QueryDef) -> QueryPlan {
    const MANY_KEYS: &str = "it orders by more than one key, and an index serves an order by \
                             one field or by the record id";
    let order = match def.order.as_slice() {
        [] => None,
        [key] if key.field == "$id" && !key.descending => None,
        [key] => Some(key),
        _ => return QueryPlan::Scan(MANY_KEYS.into()),
    };
    let Some(filter) = &def.filter else {
        return match order {
            None => QueryPlan::Window,
            Some(key) => QueryPlan::Scan(format!(
                "it has no filter and orders by {}: a record without a value of the field takes \
                 part in the order, and the index holds no entry for a missing value",
                key.field
            )),
        };
    };
    let Some(object) = schema.objects.get(&def.object) else {
        return QueryPlan::Scan(format!("{} is not an object type", def.object));
    };
    let mut parts = Vec::new();
    conjuncts(filter, &mut parts);
    let atoms: Vec<(&str, Atom)> = parts.iter().filter_map(|p| atom_of(p, def)).collect();

    let Some(key) = order else {
        // In record id order: the candidates of one value are in id order, so an equality or a
        // list of values is served; a range is not.
        let mut why = None;
        for want_list in [false, true] {
            for (name, atom) in &atoms {
                let probe = match (atom, want_list) {
                    (Atom::Eq(operand), false) => Probe::Eq(operand.clone()),
                    (Atom::In(probe), true) => probe.clone(),
                    _ => continue,
                };
                match indexable(object, name) {
                    Ok((field, _)) => {
                        return QueryPlan::Index(IndexPlan {
                            field,
                            probe,
                            order: PlanOrder::Id,
                        })
                    }
                    Err(reason) => {
                        why.get_or_insert(reason);
                    }
                }
            }
        }
        return QueryPlan::Scan(why.unwrap_or_else(|| {
            "its filter has no condition `field == value` or `field in list` at its top level, \
             which is what an index serves in record id order"
                .into()
        }));
    };
    if key.field == "$id" {
        return QueryPlan::Scan(
            "it orders by the record id descending, and an index serves the id ascending".into(),
        );
    }
    // The index of the field it orders by serves a condition on that field.
    let same_why = match indexable(object, &key.field) {
        Err(reason) => format!("it orders by {}: {reason}", key.field),
        Ok((field, field_def)) => {
            let order = PlanOrder::Field {
                descending: key.descending,
            };
            let on: Vec<&Atom> = atoms
                .iter()
                .filter(|(name, _)| *name == key.field)
                .map(|(_, atom)| atom)
                .collect();
            for atom in &on {
                if let Atom::Eq(operand) = atom {
                    return QueryPlan::Index(IndexPlan {
                        field,
                        probe: Probe::Eq(operand.clone()),
                        order,
                    });
                }
            }
            for atom in &on {
                if let Atom::In(probe) = atom {
                    return QueryPlan::Index(IndexPlan {
                        field,
                        probe: probe.clone(),
                        order,
                    });
                }
            }
            let bounds: Vec<(CmpOp, Operand)> = on
                .iter()
                .filter_map(|atom| match atom {
                    Atom::Bound(op, operand) => Some((*op, operand.clone())),
                    _ => None,
                })
                .collect();
            if bounds.is_empty() {
                format!(
                    "it orders by {0}, but its filter has no condition on {0} at its top level, \
                     and an index serves the field it orders by",
                    key.field
                )
            } else if !rangeable(&field_def.ty) {
                format!(
                    "it compares {} with <, <=, > or >=, and the index orders only integer and \
                     time fields (text longer than {INDEX_KEY_BYTES} bytes has no index key, and \
                     references and truth values have no order)",
                    key.field
                )
            } else {
                return QueryPlan::Index(IndexPlan {
                    field,
                    probe: Probe::Range(bounds),
                    order,
                });
            }
        }
    };
    // An equality or a list of values on another field is served by the pair index of that field
    // and the attribute it orders by (KRN-2920).
    match pair_plan(object, &atoms, key) {
        Ok(plan) => QueryPlan::Index(plan),
        Err(pair_why) => QueryPlan::Scan(format!("{same_why}; and {pair_why}")),
    }
}

/// How an aggregate reaches the records its query filter selects (AGG-2326).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FilterAccess {
    /// No index answers the filter: the aggregate reads the window of the object type.
    Scan,
    /// The filter names the principal, which is not known yet (`needs` before the reader is).
    Unknown,
    /// The records whose `field` equals one of `values` (distinct, each keyable, a null value
    /// dropped as matching nothing), read through the record index.
    Values { field: FieldId, values: Vec<Value> },
}

/// How the query filter of `def` reaches its records. An equality or a list of values on an
/// indexable field at the top level of the filter is answered by the record index; the first such
/// condition is used, equalities before lists. Anything else, or a value the index cannot hold, is
/// [`FilterAccess::Scan`]. The whole filter is still evaluated on each record it returns.
pub(crate) fn filter_access(
    schema: &TenantSchema,
    def: &QueryDef,
    params: &BTreeMap<String, Value>,
    principal: Option<&Principal>,
) -> FilterAccess {
    let (Some(filter), Some(object)) = (&def.filter, schema.objects.get(&def.object)) else {
        return FilterAccess::Scan;
    };
    let mut parts = Vec::new();
    conjuncts(filter, &mut parts);
    let atoms: Vec<(&str, Atom)> = parts.iter().filter_map(|p| atom_of(p, def)).collect();
    let equalities = atoms.iter().filter(|(_, atom)| matches!(atom, Atom::Eq(_)));
    let lists = atoms.iter().filter(|(_, atom)| matches!(atom, Atom::In(_)));
    for (name, atom) in equalities.chain(lists) {
        let Ok((field, _)) = indexable(object, name) else {
            continue;
        };
        let items = match atom {
            Atom::Eq(operand) => match operand_value(operand, params, principal) {
                None => return FilterAccess::Unknown,
                Some(value) => vec![value],
            },
            Atom::In(Probe::InList(items)) => items.clone(),
            Atom::In(Probe::InParam(param)) => match params.get(param) {
                Some(Value::List(items)) => items.clone(),
                _ => Vec::new(),
            },
            Atom::In(_) | Atom::Bound(..) => continue,
        };
        let mut values = BTreeSet::new();
        for item in items {
            match item {
                Value::Null => {}
                Value::List(_) => return FilterAccess::Scan,
                value if !keyable(&value) => return FilterAccess::Scan,
                value => {
                    values.insert(value);
                }
            }
        }
        if values.len() > MAX_IN_VALUES {
            return FilterAccess::Scan;
        }
        return FilterAccess::Values {
            field,
            values: values.into_iter().collect(),
        };
    }
    FilterAccess::Scan
}

/// Whether the pair index orders by `name`, or why not: `$created_at`, `$updated_at`, and an
/// integer or time field an index may use (KRN-2920).
fn orderable(object: &ObjectTypeDef, name: &str) -> Result<(), String> {
    match name {
        "$created_at" | "$updated_at" => Ok(()),
        _ if name.starts_with('$') => Err(format!(
            "{name} is a system attribute the pair index does not order by (it orders by \
             $created_at, $updated_at and integer and time fields)"
        )),
        _ => {
            let (_, def) = indexable(object, name)?;
            if rangeable(&def.ty) {
                Ok(())
            } else {
                Err(format!(
                    "{name} is not an integer or time field, and the pair index orders only \
                     integers and times (text longer than {INDEX_KEY_BYTES} bytes has no key, and \
                     references and truth values have no order)"
                ))
            }
        }
    }
}

/// The plan of an equality or a list of values on one field ordered by another attribute, or why
/// there is none, as a clause.
fn pair_plan(
    object: &ObjectTypeDef,
    atoms: &[(&str, Atom)],
    key: &OrderKey,
) -> Result<IndexPlan, String> {
    if let Err(reason) = orderable(object, &key.field) {
        return Err(format!(
            "no pair index (field, {}) serves it: {reason}",
            key.field
        ));
    }
    let mut why = None;
    for want_list in [false, true] {
        for (name, atom) in atoms {
            if *name == key.field {
                continue;
            }
            let probe = match (atom, want_list) {
                (Atom::Eq(operand), false) => Probe::Eq(operand.clone()),
                (Atom::In(probe), true) => probe.clone(),
                _ => continue,
            };
            match indexable(object, name) {
                Ok((field, _)) => {
                    return Ok(IndexPlan {
                        field,
                        probe,
                        order: PlanOrder::By {
                            attribute: key.field.clone(),
                            descending: key.descending,
                        },
                    })
                }
                Err(reason) => {
                    why.get_or_insert(format!(
                        "the pair index ({name}, {}) cannot serve it: {reason}",
                        key.field
                    ));
                }
            }
        }
    }
    Err(why.unwrap_or_else(|| {
        format!(
            "its filter has no condition `field == value` or `field in list` at its top level on \
             another field, which a pair index (field, {}) would serve",
            key.field
        )
    }))
}

/// The pair indexes the plans of a schema's queries read (KRN-2921): what a store keeps.
pub fn index_pairs(schema: &TenantSchema) -> BTreeSet<IndexPair> {
    schema
        .queries
        .values()
        .filter_map(|def| match plan_query(schema, def) {
            QueryPlan::Index(IndexPlan {
                field,
                order: PlanOrder::By { attribute, .. },
                ..
            }) => Some(IndexPair {
                object: def.object.clone(),
                field,
                order: attribute,
            }),
            _ => None,
        })
        .collect()
}

/// How a plan reads, for a sentence.
fn describe(plan: &QueryPlan) -> String {
    match plan {
        QueryPlan::Window => "a window of the type in record id order".into(),
        QueryPlan::Index(IndexPlan {
            field,
            order: PlanOrder::By { attribute, .. },
            ..
        }) => format!("the pair index ({}, {attribute})", field.as_str()),
        QueryPlan::Index(plan) => format!("the index of {}", plan.field.as_str()),
        QueryPlan::Scan(_) => "a scan".into(),
    }
}

/// The one change that would let an index serve a query that is a scan (KRN-2923), found by
/// planning the query with each candidate change: ordering by its first key alone, by the record
/// id ascending, by `$created_at`, or the removal of one field's read condition. When none does,
/// it names the pair index the planner could not derive and why.
pub fn plan_hint(schema: &TenantSchema, def: &QueryDef) -> String {
    let planned = |schema: &TenantSchema, def: &QueryDef| match plan_query(schema, def) {
        QueryPlan::Scan(_) => None,
        plan => Some(describe(&plan)),
    };
    let reorder = |order: Vec<OrderKey>| QueryDef {
        order,
        ..def.clone()
    };
    if def.order.len() > 1 {
        if let Some(how) = planned(schema, &reorder(def.order[..1].to_vec())) {
            return format!(
                "ordering by {} alone would let {how} serve it",
                def.order[0].field
            );
        }
    }
    if let [key] = def.order.as_slice() {
        if key.field == "$id" && key.descending {
            let up = reorder(vec![OrderKey {
                field: "$id".into(),
                descending: false,
            }]);
            if let Some(how) = planned(schema, &up) {
                return format!("ordering by the record id ascending would let {how} serve it");
            }
        }
        if key.field != "$created_at" {
            let created = reorder(vec![OrderKey {
                field: "$created_at".into(),
                descending: key.descending,
            }]);
            if let Some(how) = planned(schema, &created) {
                return format!(
                    "ordering by $created_at{} instead of {} would let {how} serve it",
                    if key.descending { " descending" } else { "" },
                    key.field
                );
            }
        }
    }
    if let Some(object) = schema.objects.get(&def.object) {
        for (id, field) in &object.fields {
            if field.read.is_none() {
                continue;
            }
            let mut open = TenantSchema::clone(schema);
            if let Some(field) = open
                .objects
                .get_mut(&def.object)
                .and_then(|object| object.fields.get_mut(id))
            {
                field.read = None;
            }
            if let Some(how) = planned(&open, def) {
                return format!(
                    "removing the read condition of {} would let {how} serve it (a field with a \
                     read condition is never indexed, so that neither the order nor the page \
                     boundaries show it)",
                    id.as_str()
                );
            }
        }
    }
    // No single change found: name the pair the filter and the order point at, and why the
    // planner could not derive it.
    let Some(object) = schema.objects.get(&def.object) else {
        return format!("{} is not an object type", def.object);
    };
    let mut parts = Vec::new();
    if let Some(filter) = &def.filter {
        conjuncts(filter, &mut parts);
    }
    let atoms: Vec<(&str, Atom)> = parts.iter().filter_map(|p| atom_of(p, def)).collect();
    let equal = atoms
        .iter()
        .find(|(_, atom)| matches!(atom, Atom::Eq(_) | Atom::In(_)))
        .map(|(name, _)| *name);
    match (equal, def.order.as_slice()) {
        (Some(name), [key]) => match pair_plan(object, &atoms, key) {
            Err(why) => format!(
                "no single change was found; the pair index ({name}, {}) is the one its filter \
                 and order point at, and {why}",
                key.field
            ),
            Ok(_) => "no single change was found".into(),
        },
        _ => "no single change was found; an index serves a top-level condition `field == value` \
              or `field in list` on a field without a read condition, ordered by nothing, by the \
              record id, by that field, or by an integer or time field, $created_at or \
              $updated_at"
            .into(),
    }
}

/// The warnings of a schema change (KRN-2524): every query it defines that is a scan, and every
/// query it turns from a planned one into a scan, each with the reason. Passed checks, so they
/// never refuse the change.
pub fn scan_warnings(
    before: &TenantSchema,
    after: &TenantSchema,
    changes: &[SchemaChange],
) -> Vec<Check> {
    let defined: BTreeSet<&QueryId> = changes
        .iter()
        .filter_map(|change| match change {
            SchemaChange::DefineQuery(query) => Some(&query.id),
            _ => None,
        })
        .collect();
    let mut warnings = Vec::new();
    for (id, def) in &after.queries {
        let QueryPlan::Scan(reason) = plan_query(after, def) else {
            continue;
        };
        let was_scan = before
            .queries
            .get(id)
            .is_some_and(|old| plan_query(before, old).is_scan());
        if defined.contains(id) || !was_scan {
            warnings.push(Check::pass(
                format!("query-plan:{id}"),
                format!(
                    "warning: query {id} is served by a scan of {}, which reads every record of \
                     the type and is refused above {} records, because {reason}. To serve it \
                     from an index: {}",
                    def.object,
                    limits::MAX_SCAN_RECORDS,
                    plan_hint(after, def)
                ),
            ));
        }
    }
    warnings
}

// ---------------------------------------------------------------------------------------------
// The candidates of an index page: the reference meaning of Need::IndexPage
// ---------------------------------------------------------------------------------------------

/// Whether `value` lies between the bounds, in `Value`'s order.
pub fn within(value: &Value, lower: &IndexBound, upper: &IndexBound) -> bool {
    let above = match value.cmp(&lower.value) {
        Ordering::Greater => true,
        Ordering::Equal => lower.inclusive,
        Ordering::Less => false,
    };
    let below = match value.cmp(&upper.value) {
        Ordering::Less => true,
        Ordering::Equal => upper.inclusive,
        Ordering::Greater => false,
    };
    above && below
}

/// The order of an index page: the value (reversed when descending), then the record id
/// ascending, the tie order of every named query.
pub fn index_cmp(
    a: &Value,
    a_id: RecordId,
    b: &Value,
    b_id: RecordId,
    descending: bool,
) -> Ordering {
    let by_value = a.cmp(b);
    let by_value = if descending {
        by_value.reverse()
    } else {
        by_value
    };
    by_value.then(a_id.cmp(&b_id))
}

/// Whether `(value, id)` comes after `after` in an index page's order.
pub fn is_after(value: &Value, id: RecordId, after: &IndexPosition, descending: bool) -> bool {
    index_cmp(value, id, &after.value, after.id, descending) == Ordering::Greater
}

/// The answer to `Need::IndexPage` from a set of records: those whose value of `field` is a
/// single value (not `null`, not a list) between the bounds and after `after`, in the page's
/// order, at most `limit`. Every store answers exactly this (KRN-2525); the memory store and the
/// kernel's own lookup compute it here.
pub fn index_page<'a>(
    records: impl Iterator<Item = &'a Arc<Record>>,
    field: &FieldId,
    lower: &IndexBound,
    upper: &IndexBound,
    descending: bool,
    after: Option<&IndexPosition>,
    limit: u32,
) -> Vec<&'a Arc<Record>> {
    let mut found: Vec<&'a Arc<Record>> = records
        .filter(|record| {
            let value = record.get(field);
            scalar(value)
                && within(value, lower, upper)
                && after.is_none_or(|p| is_after(value, record.id, p, descending))
        })
        .collect();
    found.sort_by(|a, b| index_cmp(a.get(field), a.id, b.get(field), b.id, descending));
    found.truncate(limit as usize);
    found
}

/// The value of the attribute a pair index orders by: a field (`null` when missing), or
/// `$created_at` or `$updated_at`. The scan orders by exactly this value.
pub fn order_value(record: &Record, attribute: &str) -> Value {
    expr::attribute(Some(record), attribute).unwrap_or(Value::Null)
}

/// The answer to `Need::PairPage` from a set of records (KRN-2920, KRN-2921): those whose value
/// of `field` is the single value `value`, in the order of their value of the attribute `order`
/// (reversed when descending; a missing or `null` value is the least) and then the record id
/// ascending, after the position `after` (a value of `order` and a record id), at most `limit`.
/// Every store answers exactly this; the memory store, the kernel and a SQLite file whose pair
/// index cannot order a record compute it here.
#[allow(clippy::too_many_arguments)]
pub fn pair_page<'a>(
    records: impl Iterator<Item = &'a Arc<Record>>,
    field: &FieldId,
    value: &Value,
    order: &str,
    descending: bool,
    after: Option<&IndexPosition>,
    limit: u32,
) -> Vec<&'a Arc<Record>> {
    if !scalar(value) {
        return Vec::new();
    }
    let mut found: Vec<(Value, &'a Arc<Record>)> = records
        .filter(|record| record.get(field) == value)
        .map(|record| (order_value(record, order), record))
        .filter(|(key, record)| after.is_none_or(|p| is_after(key, record.id, p, descending)))
        .collect();
    found.sort_by(|(a, ra), (b, rb)| index_cmp(a, ra.id, b, rb.id, descending));
    found.truncate(limit as usize);
    found.into_iter().map(|(_, record)| record).collect()
}

/// A page of candidates through the snapshot, which must cover it.
fn fetch<'a>(lookup: Lookup<'a>, need: Need) -> Result<Vec<&'a Arc<Record>>, DomainError> {
    if !lookup.coverage.covers(&need) {
        return Err(insufficient(need));
    }
    Ok(match &need {
        Need::IndexPage {
            object,
            field,
            lower,
            upper,
            descending,
            after,
            limit,
        } => index_page(
            lookup.tenant.records_of(object),
            field,
            lower,
            upper,
            *descending,
            after.as_ref(),
            *limit,
        ),
        Need::PairPage {
            object,
            field,
            value,
            order,
            descending,
            after,
            limit,
        } => pair_page(
            lookup.tenant.records_of(object),
            field,
            value,
            order,
            *descending,
            after.as_ref(),
            *limit,
        ),
        _ => Vec::new(),
    })
}

/// The value an index position of the plan holds for a record: the value of the plan's field,
/// or of the attribute a pair plan orders by.
pub fn plan_value(plan: &IndexPlan, record: &Record) -> Value {
    match &plan.order {
        PlanOrder::By { attribute, .. } => order_value(record, attribute),
        PlanOrder::Id | PlanOrder::Field { .. } => record.get(&plan.field).clone(),
    }
}

fn position(plan: &IndexPlan, record: &Record) -> IndexPosition {
    IndexPosition {
        value: plan_value(plan, record),
        id: record.id,
    }
}

// ---------------------------------------------------------------------------------------------
// Resolving a plan against a request
// ---------------------------------------------------------------------------------------------

/// What a plan's probe comes to for one request.
enum Resolved {
    /// It names the principal and the principal is not known yet (`read_needs`).
    Unknown,
    /// No record can satisfy the probe, so none can satisfy the filter.
    Empty,
    /// The values cannot be served by the index (a list, a value longer than the key limit, a
    /// string range, too many values): the request is answered by the scan.
    Degraded,
    /// One stream of candidates per pair of bounds.
    Streams(Vec<(IndexBound, IndexBound)>),
}

fn operand_value(
    operand: &Operand,
    params: &BTreeMap<String, Value>,
    principal: Option<&Principal>,
) -> Option<Value> {
    Some(match operand {
        Operand::Lit(value) => value.clone(),
        Operand::Param(name) => params.get(name).cloned().unwrap_or(Value::Null),
        Operand::Principal => match principal? {
            Principal::User(id) => Value::Str(id.as_str().to_string()),
            _ => Value::Null,
        },
    })
}

fn keyable(value: &Value) -> bool {
    limits::encoded_len(value) <= INDEX_KEY_BYTES
}

fn exactly(value: Value) -> (IndexBound, IndexBound) {
    (
        IndexBound {
            value: value.clone(),
            inclusive: true,
        },
        IndexBound {
            value,
            inclusive: true,
        },
    )
}

/// `field in items`: one equality stream per distinct value. A `null` item matches nothing.
fn list_streams(items: &[Value]) -> Resolved {
    let mut values = BTreeSet::new();
    for item in items {
        match item {
            Value::Null => continue,
            Value::List(_) => return Resolved::Degraded,
            value if !keyable(value) => return Resolved::Degraded,
            value => {
                values.insert(value.clone());
            }
        }
    }
    if values.is_empty() {
        return Resolved::Empty;
    }
    if values.len() > MAX_IN_VALUES {
        return Resolved::Degraded;
    }
    Resolved::Streams(values.into_iter().map(exactly).collect())
}

fn tighter_lower(current: Option<IndexBound>, new: IndexBound) -> IndexBound {
    match current {
        None => new,
        Some(current) => match new.value.cmp(&current.value) {
            Ordering::Greater => new,
            Ordering::Less => current,
            Ordering::Equal => IndexBound {
                inclusive: current.inclusive && new.inclusive,
                value: current.value,
            },
        },
    }
}

fn tighter_upper(current: Option<IndexBound>, new: IndexBound) -> IndexBound {
    match current {
        None => new,
        Some(current) => match new.value.cmp(&current.value) {
            Ordering::Less => new,
            Ordering::Greater => current,
            Ordering::Equal => IndexBound {
                inclusive: current.inclusive && new.inclusive,
                value: current.value,
            },
        },
    }
}

/// A range: an ordering comparison holds only between two integers, two times or two strings,
/// so the stream is the values of the operand's kind between the tightest bounds.
fn range_streams(
    bounds: &[(CmpOp, Operand)],
    params: &BTreeMap<String, Value>,
    principal: Option<&Principal>,
) -> Resolved {
    let mut time = None;
    let (mut lower, mut upper) = (None, None);
    for (op, operand) in bounds {
        let Some(value) = operand_value(operand, params, principal) else {
            return Resolved::Unknown;
        };
        let is_time = match &value {
            Value::Int(_) => false,
            Value::Time(_) => true,
            // A string range is not in the index (KRN-2525).
            Value::Str(_) => return Resolved::Degraded,
            // Null compares to nothing; references and truth values have no order.
            _ => return Resolved::Empty,
        };
        if *time.get_or_insert(is_time) != is_time {
            return Resolved::Empty;
        }
        let bound = IndexBound {
            value,
            inclusive: matches!(op, CmpOp::Le | CmpOp::Ge),
        };
        match op {
            CmpOp::Gt | CmpOp::Ge => lower = Some(tighter_lower(lower, bound)),
            CmpOp::Lt | CmpOp::Le => upper = Some(tighter_upper(upper, bound)),
            CmpOp::Eq | CmpOp::Ne => {}
        }
    }
    let Some(time) = time else {
        return Resolved::Empty;
    };
    let (least, most) = if time {
        (Value::Time(i64::MIN), Value::Time(i64::MAX))
    } else {
        (Value::Int(i64::MIN), Value::Int(i64::MAX))
    };
    Resolved::Streams(vec![(
        lower.unwrap_or(IndexBound {
            value: least,
            inclusive: true,
        }),
        upper.unwrap_or(IndexBound {
            value: most,
            inclusive: true,
        }),
    )])
}

fn resolve(
    plan: &IndexPlan,
    params: &BTreeMap<String, Value>,
    principal: Option<&Principal>,
) -> Resolved {
    match &plan.probe {
        Probe::Eq(operand) => match operand_value(operand, params, principal) {
            None => Resolved::Unknown,
            Some(Value::Null) => Resolved::Empty,
            Some(Value::List(_)) => Resolved::Degraded,
            Some(value) if !keyable(&value) => Resolved::Degraded,
            Some(value) => Resolved::Streams(vec![exactly(value)]),
        },
        Probe::InList(items) => list_streams(items),
        // `field in p` with p null or not a list holds for no record.
        Probe::InParam(name) => match params.get(name) {
            Some(Value::List(items)) => list_streams(items),
            _ => Resolved::Empty,
        },
        Probe::Range(bounds) => range_streams(bounds, params, principal),
    }
}

/// Where a planned read starts.
enum Start {
    Beginning,
    AfterId(RecordId),
    At(IndexPosition),
}

fn start_of(order: &PlanOrder, cursor: Option<&PageCursor>) -> Option<Start> {
    match (order, cursor) {
        (_, None) => Some(Start::Beginning),
        (PlanOrder::Id, Some(PageCursor::After(id))) => Some(Start::AfterId(*id)),
        (PlanOrder::Field { .. } | PlanOrder::By { .. }, Some(PageCursor::At(position))) => {
            Some(Start::At(position.clone()))
        }
        _ => None,
    }
}

/// Where one stream starts. In id order every stream holds one value, so "after id n" is the
/// position (that value, n) of the stream.
fn stream_start(start: &Start, lower: &IndexBound) -> Option<IndexPosition> {
    match start {
        Start::Beginning => None,
        Start::AfterId(id) => Some(IndexPosition {
            value: lower.value.clone(),
            id: *id,
        }),
        Start::At(position) => Some(position.clone()),
    }
}

/// The need of one page of a stream: an index page of the plan's field, or, for a pair plan, a
/// pair page of the stream's one value (its bounds are that value).
fn index_need(
    def: &QueryDef,
    plan: &IndexPlan,
    lower: IndexBound,
    upper: IndexBound,
    after: Option<IndexPosition>,
) -> Need {
    match &plan.order {
        PlanOrder::By {
            attribute,
            descending,
        } => Need::PairPage {
            object: def.object.clone(),
            field: plan.field.clone(),
            value: lower.value,
            order: attribute.clone(),
            descending: *descending,
            after,
            limit: INDEX_PAGE,
        },
        order => Need::IndexPage {
            object: def.object.clone(),
            field: plan.field.clone(),
            lower,
            upper,
            descending: order.descending(),
            after,
            limit: INDEX_PAGE,
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Needs and reads
// ---------------------------------------------------------------------------------------------

/// The page size of a named query read (KRN-1285): the one the request names, else the one the
/// query declares, else the default.
pub(crate) fn page_size(def: &QueryDef, asked: Option<u32>) -> Result<u32, DomainError> {
    let size = asked.or(def.limit).unwrap_or(limits::DEFAULT_PAGE_SIZE);
    if size == 0 {
        return Err(DomainError::new(
            ErrorKind::InvalidIntent,
            "a page size is at least 1",
        ));
    }
    if size > limits::MAX_PAGE_SIZE {
        return Err(DomainError::new(
            ErrorKind::LimitExceeded,
            format!(
                "a page size of {size} is more than the maximum of {}",
                limits::MAX_PAGE_SIZE
            ),
        ));
    }
    Ok(size)
}

fn wrong_cursor() -> DomainError {
    DomainError::new(
        ErrorKind::InvalidIntent,
        "the cursor is not one this query issued",
    )
}

/// The state a named query read is expected to need: the window, the scan, or the first page of
/// candidates of each stream. Nothing for a request the kernel will refuse, and nothing for a
/// probe on `principal`, which the kernel asks for once it knows the reader.
pub(crate) fn named_needs(
    schema: &TenantSchema,
    def: &QueryDef,
    params: &BTreeMap<String, Value>,
    asked: Option<u32>,
    cursor: Option<&PageCursor>,
) -> Vec<Need> {
    if page_size(def, asked).is_err() {
        return Vec::new();
    }
    let window = |after: Option<RecordId>| {
        vec![Need::PageOf {
            object: def.object.clone(),
            after,
            limit: limits::MAX_PAGE_SIZE,
        }]
    };
    let scan = || {
        vec![Need::PageOf {
            object: def.object.clone(),
            after: None,
            limit: limits::MAX_SCAN_RECORDS + 1,
        }]
    };
    match (plan_query(schema, def), cursor) {
        (QueryPlan::Window, None) => window(None),
        (QueryPlan::Window, Some(PageCursor::After(after))) => window(Some(*after)),
        (QueryPlan::Window, Some(_)) => Vec::new(),
        (QueryPlan::Scan(_), None | Some(PageCursor::Offset(_))) => scan(),
        (QueryPlan::Scan(_), Some(_)) => Vec::new(),
        (QueryPlan::Index(plan), cursor) => match (resolve(&plan, params, None), cursor) {
            (_, Some(PageCursor::Offset(_))) | (Resolved::Degraded, None) => scan(),
            (Resolved::Streams(streams), cursor) => {
                let Some(start) = start_of(&plan.order, cursor) else {
                    return Vec::new();
                };
                streams
                    .into_iter()
                    .map(|(lower, upper)| {
                        let after = stream_start(&start, &lower);
                        index_need(def, &plan, lower, upper, after)
                    })
                    .collect()
            }
            _ => Vec::new(),
        },
    }
}

type Page = (Vec<Arc<Record>>, Option<PageCursor>);

/// How a planned read ended.
enum End {
    /// Every stream of candidates ran out.
    Exhausted,
    /// The page is full.
    Full,
    /// It examined [`MAX_PLAN_CANDIDATES`] candidates.
    Bound,
}

/// One stream of candidates: the records of one pair of bounds, fetched a page at a time.
struct Stream<'a> {
    lower: IndexBound,
    upper: IndexBound,
    /// The position of the last candidate fetched.
    after: Option<IndexPosition>,
    buffer: VecDeque<&'a Arc<Record>>,
    done: bool,
}

/// The reference fields an expression follows (`Expr::Through`), by name.
fn followed(expr: &Expr, out: &mut BTreeSet<String>) {
    match expr {
        Expr::Through(_, field, _) => {
            out.insert(field.clone());
        }
        Expr::Var(..)
        | Expr::Lit(_)
        | Expr::Param(_)
        | Expr::Now
        | Expr::Principal
        | Expr::HasRole(_) => {}
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Graphemes(a)
        | Expr::IsEmoji(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => followed(a, out),
        Expr::And(items) | Expr::Or(items) => {
            for item in items {
                followed(item, out);
            }
        }
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => {
            followed(a, out);
            followed(b, out);
        }
        Expr::If(a, b, c) => {
            followed(a, out);
            followed(b, out);
            followed(c, out);
        }
    }
}

/// What a read run by [`read_missing`] gathers (KRN-2922): every record a row it examines refers
/// to through a reference field that the query's filter, a read policy of the reader's roles or a
/// field's read condition follows, and that the snapshot lacks.
struct Collector {
    /// The reference fields of the object type those expressions follow.
    fields: BTreeSet<String>,
    found: BTreeSet<Need>,
}

impl Collector {
    fn new(
        schema: &TenantSchema,
        object: &ObjectTypeId,
        roles: &BTreeSet<RoleId>,
        filter: Option<&Expr>,
    ) -> Self {
        let mut fields = BTreeSet::new();
        if let Some(filter) = filter {
            followed(filter, &mut fields);
        }
        for policy in schema.policies.values() {
            let reaches = match &policy.resource {
                Resource::AnyObject => true,
                Resource::Object(target) => target == object,
                _ => false,
            };
            if reaches && roles.contains(&policy.role) && policy.actions.contains(&Action::Read) {
                if let Some(condition) = &policy.condition {
                    followed(condition, &mut fields);
                }
            }
        }
        if let Some(def) = schema.objects.get(object) {
            for field in def.fields.values() {
                if let Some(rule) = &field.read {
                    followed(rule, &mut fields);
                }
            }
        }
        Collector {
            fields,
            found: BTreeSet::new(),
        }
    }

    /// The records `record` refers to through the followed fields that the snapshot lacks.
    fn note(&mut self, lookup: Lookup<'_>, record: &Record) {
        let Some(def) = lookup.tenant.schema.objects.get(&record.object) else {
            return;
        };
        for name in &self.fields {
            let Ok(id) = FieldId::new(name.as_str()) else {
                continue;
            };
            let Value::Ref(target) = record.get(&id) else {
                continue;
            };
            let Some((object, _, false)) = def.fields.get(&id).and_then(|f| f.ty.ref_target())
            else {
                continue;
            };
            let need = Need::Record {
                object: object.clone(),
                id: *target,
            };
            if !lookup.coverage.covers(&need) {
                self.found.insert(need);
            }
        }
    }
}

/// A reader of one named query against one snapshot.
struct Reader<'a> {
    lookup: Lookup<'a>,
    ctx: &'a Context,
    roles: &'a BTreeSet<RoleId>,
    params: &'a BTreeMap<String, Value>,
    /// Set when [`read_missing`] runs the read: a row that lacks a record it refers to is noted
    /// and passed over instead of ending the read.
    collect: Option<&'a mut Collector>,
}

impl<'a> Reader<'a> {
    /// The outcome of a step on `record`, or, when collecting, `instead` for a step that lacks a
    /// record the row refers to, after noting the row's missing records.
    fn absorb<T>(
        &mut self,
        record: &Record,
        outcome: Result<T, DomainError>,
        instead: impl FnOnce() -> T,
    ) -> Result<T, DomainError> {
        let Some(collector) = self.collect.as_deref_mut() else {
            return outcome;
        };
        match outcome {
            Err(error)
                if matches!(error.kind, ErrorKind::InsufficientState)
                    && matches!(error.missing, Some(Need::Record { .. })) =>
            {
                collector.note(self.lookup, record);
                Ok(instead())
            }
            other => other,
        }
    }

    fn visible(&mut self, record: &Record) -> Result<bool, DomainError> {
        let outcome = visible_in(self.lookup, &self.ctx.principal, record, self.ctx.now);
        self.absorb(record, outcome, || false)
    }

    fn shown(&mut self, record: &Arc<Record>) -> Result<Arc<Record>, DomainError> {
        let outcome = redact(
            self.lookup,
            &self.ctx.principal,
            self.roles,
            self.ctx.now,
            record,
        );
        self.absorb(record, outcome, || record.clone())
    }

    /// Whether the filter holds of the record as the reader receives it (KRN-2522). A record the
    /// filter cannot be evaluated on is left out; a reference the snapshot lacks is
    /// `InsufficientState`.
    fn passes(&mut self, filter: Option<&Expr>, record: &Record) -> Result<bool, DomainError> {
        let outcome = self.evaluate(filter, record);
        self.absorb(record, outcome, || false)
    }

    fn evaluate(&self, filter: Option<&Expr>, record: &Record) -> Result<bool, DomainError> {
        let Some(filter) = filter else {
            return Ok(true);
        };
        let env = Env {
            record: Some(record),
            old: Some(record),
            new: Some(record),
            params: self.params,
            now: self.ctx.now,
            principal: &self.ctx.principal,
            roles: self.roles,
            lookup: Some(self.lookup),
        };
        let outcome = expr::holds(filter, &env);
        if outcome.is_err() {
            if let Some(missing) = missing_error(filter, &env) {
                return Err(missing);
            }
        }
        Ok(outcome.unwrap_or(false))
    }

    /// A window of the object type in id order (KRN-1286).
    fn window(
        &mut self,
        def: &QueryDef,
        after: Option<RecordId>,
        size: u32,
    ) -> Result<Page, DomainError> {
        let window = self
            .lookup
            .page_of(&def.object, after, limits::MAX_PAGE_SIZE)?;
        let window_end = if window.len() == limits::MAX_PAGE_SIZE as usize {
            window.last().map(|r| r.id)
        } else {
            None
        };
        let mut seen = Vec::new();
        for record in window {
            if self.visible(record)? {
                seen.push(record);
            }
        }
        let size = size as usize;
        let next = match seen.get(size.wrapping_sub(1)) {
            Some(last) if seen.len() > size => Some(last.id),
            _ => window_end,
        }
        .map(PageCursor::After);
        let rows = seen
            .into_iter()
            .take(size)
            .map(|record| self.shown(record))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((rows, next))
    }

    /// Every row of the query, sorted: the scan (KRN-1287, KRN-1288). The filter and the order
    /// see each record as the reader receives it.
    fn keyed(
        &mut self,
        def: &QueryDef,
        query: &QueryId,
    ) -> Result<Vec<(Vec<Value>, Arc<Record>)>, DomainError> {
        let window = self
            .lookup
            .page_of(&def.object, None, limits::MAX_SCAN_RECORDS + 1)?;
        if window.len() > limits::MAX_SCAN_RECORDS as usize {
            return Err(DomainError::new(
                ErrorKind::LimitExceeded,
                format!(
                    "query {query} orders {} records, more than the scan bound of {}",
                    def.object,
                    limits::MAX_SCAN_RECORDS
                ),
            )
            .with(vec![Check::fail(
                format!("scan-bound:{query}"),
                "the query is not served by a window of the object type",
            )]));
        }
        let mut keyed = Vec::new();
        for record in window {
            if !self.visible(record)? {
                continue;
            }
            let shown = self.shown(record)?;
            if !self.passes(def.filter.as_ref(), &shown)? {
                continue;
            }
            let mut key = Vec::with_capacity(def.order.len());
            for order in &def.order {
                match expr::attribute(Some(&shown), &order.field) {
                    Ok(value) => key.push(value),
                    Err(_) => break,
                }
            }
            if key.len() != def.order.len() {
                continue;
            }
            keyed.push((key, shown));
        }
        keyed.sort_by(|(a, ra), (b, rb)| {
            for (i, order) in def.order.iter().enumerate() {
                let ordering = a[i].cmp(&b[i]);
                let ordering = if order.descending {
                    ordering.reverse()
                } else {
                    ordering
                };
                if ordering.is_ne() {
                    return ordering;
                }
            }
            ra.id.cmp(&rb.id)
        });
        Ok(keyed)
    }

    fn scan(
        &mut self,
        def: &QueryDef,
        query: &QueryId,
        offset: usize,
        size: u32,
    ) -> Result<Page, DomainError> {
        let keyed = self.keyed(def, query)?;
        let size = size as usize;
        let next = (offset.saturating_add(size) < keyed.len())
            .then(|| PageCursor::Offset((offset + size) as u32));
        let rows = keyed
            .into_iter()
            .skip(offset)
            .take(size)
            .map(|(_, record)| record)
            .collect();
        Ok((rows, next))
    }

    /// The scan from the position a planned read started at, with an offset cursor: the answer
    /// of a request that examined its whole bound without reaching a record the reader may read
    /// (KRN-2522). The scan's rows are in the plan's order, so those at or before the position
    /// are a prefix of them.
    fn scan_from(
        &mut self,
        def: &QueryDef,
        query: &QueryId,
        plan: &IndexPlan,
        start: &Start,
        size: u32,
    ) -> Result<Page, DomainError> {
        let keyed = self.keyed(def, query)?;
        let descending = plan.order.descending();
        let beyond = |record: &Record| match start {
            Start::Beginning => true,
            Start::AfterId(id) => record.id > *id,
            Start::At(at) => is_after(&plan_value(plan, record), record.id, at, descending),
        };
        let before = keyed.iter().take_while(|(_, r)| !beyond(r)).count();
        let size = size as usize;
        let next = (before.saturating_add(size) < keyed.len())
            .then(|| PageCursor::Offset((before + size) as u32));
        let rows = keyed
            .into_iter()
            .skip(before)
            .take(size)
            .map(|(_, record)| record)
            .collect();
        Ok((rows, next))
    }

    /// A planned read (KRN-2520, KRN-2521): merge the streams of candidates in the plan's
    /// order, and keep each candidate the reader may see and the filter holds of, until the page
    /// is full, the candidates end or the bound is reached.
    fn index(
        &mut self,
        def: &QueryDef,
        query: &QueryId,
        plan: &IndexPlan,
        bounds: Vec<(IndexBound, IndexBound)>,
        start: Start,
        size: u32,
    ) -> Result<Page, DomainError> {
        let mut streams: Vec<Stream<'a>> = bounds
            .into_iter()
            .map(|(lower, upper)| {
                let after = stream_start(&start, &lower);
                Stream {
                    lower,
                    upper,
                    after,
                    buffer: VecDeque::new(),
                    done: false,
                }
            })
            .collect();
        let full = size as usize;
        let mut rows: Vec<Arc<Record>> = Vec::new();
        let mut examined: u32 = 0;
        let mut last_examined: Option<IndexPosition> = None;
        let mut last_visible: Option<IndexPosition> = None;
        let mut last_row: Option<IndexPosition> = None;
        let end = loop {
            for stream in streams.iter_mut() {
                if stream.buffer.is_empty() && !stream.done {
                    let need = index_need(
                        def,
                        plan,
                        stream.lower.clone(),
                        stream.upper.clone(),
                        stream.after.clone(),
                    );
                    let page = fetch(self.lookup, need)?;
                    stream.done = page.len() < INDEX_PAGE as usize;
                    if let Some(last) = page.last() {
                        stream.after = Some(position(plan, last));
                    }
                    stream.buffer.extend(page);
                }
            }
            let first = streams
                .iter()
                .enumerate()
                .filter_map(|(i, stream)| stream.buffer.front().map(|record| (i, *record)))
                .min_by(|(_, a), (_, b)| match &plan.order {
                    PlanOrder::Id => a.id.cmp(&b.id),
                    order => index_cmp(
                        &plan_value(plan, a),
                        a.id,
                        &plan_value(plan, b),
                        b.id,
                        order.descending(),
                    ),
                })
                .map(|(i, _)| i);
            let Some(record) = first.and_then(|i| streams[i].buffer.pop_front()) else {
                break End::Exhausted;
            };
            examined += 1;
            let here = position(plan, record);
            last_examined = Some(here.clone());
            if self.visible(record)? {
                last_visible = Some(here.clone());
                let shown = self.shown(record)?;
                if self.passes(def.filter.as_ref(), &shown)? {
                    rows.push(shown);
                    last_row = Some(here);
                    if rows.len() == full {
                        break End::Full;
                    }
                }
            }
            if examined >= MAX_PLAN_CANDIDATES {
                break End::Bound;
            }
        };
        let exhausted = streams
            .iter()
            .all(|stream| stream.done && stream.buffer.is_empty());
        let cursor = |at: IndexPosition| match &plan.order {
            PlanOrder::Id => PageCursor::After(at.id),
            PlanOrder::Field { .. } | PlanOrder::By { .. } => PageCursor::At(at),
        };
        let next = match end {
            End::Exhausted => None,
            _ if exhausted => None,
            End::Full => last_row.map(cursor),
            // A record id is never hidden from a reader, so in id order the cursor is the last
            // candidate examined. In the order of a field or another attribute it is the last
            // candidate the reader may read, whose value the reader may see.
            End::Bound => match &plan.order {
                PlanOrder::Id => last_examined.map(cursor),
                PlanOrder::Field { .. } | PlanOrder::By { .. } => match last_visible {
                    Some(at) => Some(cursor(at)),
                    // A run of `read_missing` passes over rows it cannot decide, so it gathers
                    // what the examined rows lack and does not fall back to the scan.
                    None if self.collect.is_some() => None,
                    None => return self.scan_from(def, query, plan, &start, size),
                },
            },
        };
        Ok((rows, next))
    }
}

/// Answer a named query read against a snapshot (KRN-1285 to KRN-1288, KRN-2520 to KRN-2522).
pub(crate) fn read_named(
    lookup: Lookup<'_>,
    query: &QueryId,
    params: &BTreeMap<String, Value>,
    asked: Option<u32>,
    cursor: Option<&PageCursor>,
    ctx: &Context,
) -> Result<ReadResult, DomainError> {
    read_named_with(lookup, query, params, asked, cursor, ctx, None)
}

/// [`read_named`], gathering into `collect` what the rows it examines lack when it is given.
fn read_named_with(
    lookup: Lookup<'_>,
    query: &QueryId,
    params: &BTreeMap<String, Value>,
    asked: Option<u32>,
    cursor: Option<&PageCursor>,
    ctx: &Context,
    collect: Option<&mut Collector>,
) -> Result<ReadResult, DomainError> {
    let tenant = lookup.tenant;
    let schema = &tenant.schema;
    let def = schema
        .queries
        .get(query)
        .ok_or_else(|| DomainError::new(ErrorKind::UnknownQuery, format!("no query {query}")))?;
    if let Some(extra) = params.keys().find(|name| !def.params.contains(*name)) {
        return Err(DomainError::new(
            ErrorKind::InvalidIntent,
            format!("query {query} has no parameter {extra}"),
        ));
    }
    let size = page_size(def, asked)?;
    let roles = roles_of(tenant, &ctx.principal);
    let mut reader = Reader {
        lookup,
        ctx,
        roles: &roles,
        params,
        collect,
    };
    let (rows, next) = match (plan_query(schema, def), cursor) {
        (QueryPlan::Window, None) => reader.window(def, None, size)?,
        (QueryPlan::Window, Some(PageCursor::After(after))) => {
            reader.window(def, Some(*after), size)?
        }
        (QueryPlan::Window, Some(_)) => return Err(wrong_cursor()),
        (QueryPlan::Scan(_), None) => reader.scan(def, query, 0, size)?,
        (QueryPlan::Scan(_), Some(PageCursor::Offset(offset))) => {
            reader.scan(def, query, *offset as usize, size)?
        }
        (QueryPlan::Scan(_), Some(_)) => return Err(wrong_cursor()),
        (QueryPlan::Index(plan), cursor) => {
            match (resolve(&plan, params, Some(&ctx.principal)), cursor) {
                // An offset continues the scan a planned read fell back to.
                (_, Some(PageCursor::Offset(offset))) => {
                    reader.scan(def, query, *offset as usize, size)?
                }
                (Resolved::Degraded, None) => reader.scan(def, query, 0, size)?,
                (Resolved::Degraded, Some(_)) => return Err(wrong_cursor()),
                (Resolved::Empty | Resolved::Unknown, _) => (Vec::new(), None),
                (Resolved::Streams(bounds), cursor) => {
                    let start = start_of(&plan.order, cursor).ok_or_else(wrong_cursor)?;
                    reader.index(def, query, &plan, bounds, start, size)?
                }
            }
        }
    };
    Ok(ReadResult {
        rows,
        revision: tenant.revision,
        schema_version: schema.version,
        next,
    })
}

/// Everything a read lacks in `state` that the kernel can name at once (KRN-2922, WG-5180): the
/// read is run with every row that lacks a record it refers to passed over, and the result is
/// every record those rows refer to that the snapshot lacks (through the reference fields the
/// query's filter, the reader's read policies and the fields' read conditions follow), together
/// with the window, page of candidates or record the run stopped at. Its rows are discarded.
///
/// A row passed over takes no part in the page, so the run examines at least the candidates the
/// read itself examines once they are loaded: loading what it names answers every one of them in
/// the same round, and a read takes at most one round per page of candidates it examines, whatever
/// its page size. The records are named one by one (`Need::Record`), never as their object type.
pub fn read_missing(state: &State, request: &ReadRequest, ctx: &Context) -> Vec<Need> {
    let Some(tenant) = state
        .tenant
        .as_deref()
        .filter(|tenant| tenant.id == ctx.tenant && state.tenant_id == ctx.tenant)
    else {
        return Vec::new();
    };
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let roles = roles_of(tenant, &ctx.principal);
    let mut found: BTreeSet<Need> = BTreeSet::new();
    match request {
        ReadRequest::Named {
            query,
            params,
            page_size,
            cursor,
        } => {
            let Some(def) = tenant.schema.queries.get(query) else {
                return Vec::new();
            };
            let mut collector =
                Collector::new(&tenant.schema, &def.object, &roles, def.filter.as_ref());
            let outcome = read_named_with(
                lookup,
                query,
                params,
                *page_size,
                cursor.as_ref(),
                ctx,
                Some(&mut collector),
            );
            if let Err(DomainError {
                kind: ErrorKind::InsufficientState,
                missing: Some(need),
                ..
            }) = outcome
            {
                found.insert(need);
            }
            found.extend(collector.found);
        }
        ReadRequest::Records { object, ids } => {
            let mut collector = Collector::new(&tenant.schema, object, &roles, None);
            for id in ids.iter().take(limits::MAX_READ_IDS) {
                match lookup.record(object, *id) {
                    Err(error) => found.extend(error.missing),
                    Ok(None) => {}
                    Ok(Some(record)) => {
                        let visible = visible_in(lookup, &ctx.principal, record, ctx.now);
                        let lacking = lacks(&visible)
                            || (matches!(visible, Ok(true))
                                && lacks(&redact(lookup, &ctx.principal, &roles, ctx.now, record)));
                        if lacking {
                            collector.note(lookup, record);
                        }
                    }
                }
            }
            found.extend(collector.found);
        }
    }
    found.into_iter().collect()
}

/// Whether a step failed for state the snapshot lacks.
fn lacks<T>(outcome: &Result<T, DomainError>) -> bool {
    matches!(
        outcome,
        Err(DomainError {
            kind: ErrorKind::InsufficientState,
            ..
        })
    )
}
