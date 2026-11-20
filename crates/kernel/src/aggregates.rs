//! Aggregates of named queries (D-5060 to D-5067, AGG-001 to AGG-042): counts, sums, minimums,
//! maximums and top lists over the records of a named query, optionally grouped by one field.
//!
//! An aggregate is a read. It examines exactly the records that a named query examines for the
//! same reader, and leaves out every record the reader may not read in whole: the record itself
//! (the Read policies, [`visible_in`]), the query filter, and every field the aggregate names (the
//! field it reads, the field it groups by and the fields the request filters on). A record the
//! reader may read in part is left out rather than counted with a hidden value (D-5061, AGG-017),
//! so the answer is what the same computation gives one record at a time over what the reader
//! may read (AGG-016), and a count, a sum, a group or a place in a top list never reveals a
//! record the reader may not read.
//!
//! The work is a scan. The records the kernel examines are the records of the object type, in id
//! order, up to a window of [`limits::MAX_SCAN_RECORDS`] + 1 loaded through one `Need::PageOf`
//! ([`needs`]); a smaller window cannot serve a count, because a count has to see every record
//! the reader may read. Of those records, the ones the reader may read and the filter selects are
//! counted against the scan bound the aggregate declares (D-5062): more than the bound is refused
//! with the rule `scan-bound-exceeded` and no result at all (AGG-020, AGG-021), and so is an object
//! type that holds more records than the window, whatever the reader may read of them (AGG-2324),
//! since the records beyond the window cannot be examined and not examining them would answer
//! in part.
//!
//! The result names the object types it depends on ([`dependencies`], AGG-042), and the audience
//! class of a reader for it is derived by [`crate::audience`]. `run` is a pure computation for the
//! principal of its context; a result may be stored for one audience class and served to the
//! readers of that class under a key holding the revision of each of those object types (D-5064,
//! D-5065, AGG-030 to AGG-034), which is the work of the shell (`strata-backend`).
//!
//! The definition-time checks ([`problems`]) are run by `TenantSchemaExt::problems`, so a query or
//! a view section that declares something this module refuses is refused when it is defined.

use crate::expr::{self, Env, Expr};
use crate::intent::{Check, CheckExt, Context, DomainError, DomainErrorExt, ErrorKind};
use crate::limits;
use crate::read::visible_in;
use crate::rules::{missing_error, roles_of, Lookup};
use crate::schema::{Action, FieldType, ObjectTypeDef, Resource, TenantSchema};
use crate::state::{Need, Principal, Record, RecordExt, State};
use crate::types::{
    AggregateDef, AggregateGroup, AggregateId, AggregateKind, AggregateRequest, AggregateResult,
    FieldId, FieldTypeExt, ObjectTypeId, QueryDef, RecordId, Value, ValueExt, ValueMap,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The rule a refusal names when an aggregate would examine more records than its bound (AGG-020).
pub const SCAN_BOUND_EXCEEDED: &str = "scan-bound-exceeded";
/// The rule a refusal names when a sum does not fit the type of its field (AGG-022).
pub const SUM_OVERFLOW: &str = "sum-overflow";
/// The rule a refusal names when an aggregate has more groups than the maximum (AGG-2323).
pub const GROUP_BOUND_EXCEEDED: &str = "group-bound-exceeded";

/// The aggregates a query declares; none for a query stored before aggregates.
pub(crate) fn aggregates_of(def: &QueryDef) -> &[AggregateDef] {
    def.aggregates.as_deref().unwrap_or_default()
}

/// The fields a query declares that a request filter or an aggregate may name.
pub(crate) fn declared_of(def: &QueryDef) -> &[FieldId] {
    def.declared_fields.as_deref().unwrap_or_default()
}

/// The state an aggregate is expected to need: one window of the object type of the query, the
/// same window a scan of a named query loads. A request the kernel will refuse needs nothing.
pub fn needs(schema: &TenantSchema, request: &AggregateRequest) -> Vec<Need> {
    let Some(def) = schema.queries.get(&request.query) else {
        return Vec::new();
    };
    if !aggregates_of(def).iter().any(|a| a.id == request.aggregate) {
        return Vec::new();
    }
    vec![Need::PageOf {
        object: def.object.clone(),
        after: None,
        limit: limits::MAX_SCAN_RECORDS + 1,
    }]
}

fn refusal(kind: ErrorKind, rule: &str, message: String) -> DomainError {
    DomainError::new(kind, message.clone()).with(vec![Check::fail(rule, message)])
}

/// The value a record has in a field, or `None` when it has none or one the declared type of the
/// field does not admit (a grandfathered value of the wrong type is left out, as a filter leaves
/// out a record it cannot be evaluated on).
fn value_in(
    record: &Record,
    field: Option<&FieldId>,
    types: &BTreeMap<FieldId, FieldType>,
) -> Option<Value> {
    let field = field?;
    let value = record.get(field);
    if value.is_null() || !types.get(field)?.admits(value) {
        return None;
    }
    Some(value.clone())
}

/// Whether a record's value matches a request filter value: equal, or, for a list field, holding
/// it (the lookup by field of the storage contract).
fn matches(have: &Value, want: &Value) -> bool {
    match have {
        Value::List(items) if !matches!(want, Value::List(_)) => items.contains(want),
        other => other == want,
    }
}

/// The value as plain JSON, for the rows of a top list.
fn plain(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::json!(b),
        Value::Int(i) => serde_json::json!(i),
        Value::Str(s) => serde_json::json!(s),
        Value::Time(t) => serde_json::json!(t),
        Value::Ref(id) => serde_json::json!(id.0),
        Value::List(items) => serde_json::Value::Array(items.iter().map(plain).collect()),
    }
}

/// What one selected record contributes to an aggregate.
struct Row {
    id: RecordId,
    /// The value of the group field, `None` for a record that has none.
    group: Option<Value>,
    /// The value of the field the aggregate reads, `None` for a record that has none.
    value: Option<Value>,
}

/// The value of one group: the count, sum, minimum or maximum, and for a top list the rows.
fn finish(
    agg: &AggregateDef,
    rows: &[&Row],
) -> Result<(Option<Value>, Vec<ValueMap>), DomainError> {
    match agg.kind {
        AggregateKind::Count => Ok((Some(Value::Int(rows.len() as i64)), Vec::new())),
        AggregateKind::Sum => {
            // Summed exactly and narrowed once, so a sum that fits is never refused because of
            // the order its parts were added in, and one that does not is never wrapped.
            let mut total: i128 = 0;
            for row in rows {
                if let Some(Value::Int(n)) = &row.value {
                    total = total.checked_add(i128::from(*n)).ok_or_else(|| {
                        refusal(
                            ErrorKind::LimitExceeded,
                            SUM_OVERFLOW,
                            "a sum does not fit the type of its field".to_string(),
                        )
                    })?;
                }
            }
            let total = i64::try_from(total).map_err(|_| {
                refusal(
                    ErrorKind::LimitExceeded,
                    SUM_OVERFLOW,
                    format!("a sum of {total} does not fit the type of its field"),
                )
            })?;
            Ok((Some(Value::Int(total)), Vec::new()))
        }
        AggregateKind::Minimum => Ok((
            rows.iter().filter_map(|r| r.value.as_ref()).min().cloned(),
            Vec::new(),
        )),
        AggregateKind::Maximum => Ok((
            rows.iter().filter_map(|r| r.value.as_ref()).max().cloned(),
            Vec::new(),
        )),
        AggregateKind::Top => {
            let mut ranked: Vec<(&Value, RecordId)> = rows
                .iter()
                .filter_map(|r| r.value.as_ref().map(|v| (v, r.id)))
                .collect();
            // Equal values come in ascending order of record id whichever way the values go.
            ranked.sort_by(|(a, ia), (b, ib)| {
                let ordering = a.cmp(b);
                let ordering = if agg.descending {
                    ordering.reverse()
                } else {
                    ordering
                };
                ordering.then(ia.cmp(ib))
            });
            ranked.truncate(agg.n.unwrap_or(1) as usize);
            let listed = ranked
                .into_iter()
                .map(|(value, id)| {
                    ValueMap(serde_json::json!({ "id": id.0, "value": plain(value) }).to_string())
                })
                .collect();
            Ok((None, listed))
        }
    }
}

/// Visit every node of an expression.
pub(crate) fn walk(expr: &Expr, visit: &mut impl FnMut(&Expr)) {
    visit(expr);
    match expr {
        Expr::Lit(_)
        | Expr::Var(..)
        | Expr::Through(..)
        | Expr::Param(_)
        | Expr::Now
        | Expr::Principal
        | Expr::HasRole(_) => {}
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => walk(a, visit),
        Expr::And(items) | Expr::Or(items) => items.iter().for_each(|e| walk(e, visit)),
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => {
            walk(a, visit);
            walk(b, visit);
        }
        Expr::If(a, b, c) => {
            walk(a, visit);
            walk(b, visit);
            walk(c, visit);
        }
    }
}

/// The object types an aggregate depends on (AGG-042): the object type of its query, and every
/// object type that something the result is made of follows a reference into. That is the query
/// filter, the Read policies of the object type (the ones that name Read on it or on every object
/// type, so a change to a record that a policy follows changes who may be counted) and the read
/// rules of the fields the aggregate can name: the one it reads, the one it groups by and every
/// field its query declares. The set depends on neither the reader nor the request.
fn depends_on(schema: &TenantSchema, def: &QueryDef, agg: &AggregateDef) -> BTreeSet<ObjectTypeId> {
    let mut found = BTreeSet::from([def.object.clone()]);
    let Some(object) = schema.objects.get(&def.object) else {
        return found;
    };
    let mut follow = |expr: &Expr| {
        if let Expr::Through(_, field, _) = expr {
            let target = FieldId::new(field.clone())
                .ok()
                .and_then(|id| object.fields.get(&id))
                .and_then(|f| f.ty.ref_target());
            if let Some((target, _, _)) = target {
                found.insert(target.clone());
            }
        }
    };
    if let Some(filter) = &def.filter {
        walk(filter, &mut follow);
    }
    for policy in schema.policies.values() {
        let reaches = match &policy.resource {
            Resource::AnyObject => true,
            Resource::Object(named) => *named == def.object,
            Resource::Schema | Resource::Members | Resource::Ledger { .. } => false,
        };
        if reaches && policy.actions.contains(&Action::Read) {
            if let Some(condition) = &policy.condition {
                walk(condition, &mut follow);
            }
        }
    }
    let named: BTreeSet<&FieldId> = declared_of(def)
        .iter()
        .chain(agg.field.iter())
        .chain(agg.group_by.iter())
        .collect();
    for field in named {
        if let Some(rule) = object.fields.get(field).and_then(|f| f.read.as_ref()) {
            walk(rule, &mut follow);
        }
    }
    found
}

/// The object types the aggregate `aggregate` of the query `query` depends on under `schema`
/// (AGG-042), or `None` when the query or the aggregate does not exist. A stored result is keyed by
/// the revision of each of them (D-5064), so this is asked before the result is computed; it is
/// the set that a result of the same aggregate names in `depends_on`.
pub fn dependencies(
    schema: &TenantSchema,
    query: &crate::types::QueryId,
    aggregate: &AggregateId,
) -> Option<BTreeSet<ObjectTypeId>> {
    let def = schema.queries.get(query)?;
    let agg = aggregates_of(def).iter().find(|a| a.id == *aggregate)?;
    Some(depends_on(schema, def, agg))
}

/// Run one aggregate of a named query for the principal of `ctx`.
///
/// Refused with `UnknownQuery` when the query or the aggregate does not exist (AGG-003) and
/// `InvalidIntent` when the request names a parameter the query lacks or filters on a field the
/// query does not declare (AGG-004), and with `LimitExceeded` carrying the rule
/// [`SCAN_BOUND_EXCEEDED`], [`SUM_OVERFLOW`] or [`GROUP_BOUND_EXCEEDED`]; no result, whole or in
/// part, comes with a refusal (AGG-021). `InsufficientState` names a record the snapshot lacks, as
/// for every read.
pub fn run(
    state: &State,
    request: &AggregateRequest,
    ctx: &Context,
) -> Result<AggregateResult, DomainError> {
    if state.tenant_id != ctx.tenant {
        return Err(DomainError::new(
            ErrorKind::TenantMismatch,
            "snapshot is of another tenant",
        ));
    }
    let tenant = state
        .tenant
        .as_deref()
        .filter(|tenant| tenant.id == ctx.tenant)
        .ok_or_else(|| DomainError::new(ErrorKind::TenantNotFound, "no such tenant"))?;
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let schema = &tenant.schema;
    let query = &request.query;
    let def = schema
        .queries
        .get(query)
        .ok_or_else(|| DomainError::new(ErrorKind::UnknownQuery, format!("no query {query}")))?;
    let agg = aggregates_of(def)
        .iter()
        .find(|a| a.id == request.aggregate)
        .ok_or_else(|| {
            DomainError::new(
                ErrorKind::UnknownQuery,
                format!("query {query} declares no aggregate {}", request.aggregate),
            )
        })?;
    if let Some(extra) = request
        .params
        .keys()
        .find(|name| !def.params.contains(*name))
    {
        return Err(DomainError::new(
            ErrorKind::InvalidIntent,
            format!("query {query} has no parameter {extra}"),
        ));
    }
    let object = schema.objects.get(&def.object).ok_or_else(|| {
        DomainError::new(
            ErrorKind::UnknownObject,
            format!("no object type {}", def.object),
        )
    })?;
    let declared = declared_of(def);
    for field in request.filter.keys() {
        if !declared.contains(field) {
            return Err(refusal(
                ErrorKind::InvalidIntent,
                &format!("aggregate-filter:{query}.{field}"),
                format!("query {query} does not declare {field} as a field to filter on"),
            ));
        }
    }

    // The fields the aggregate names: the one it reads, the one it groups by and the filter's.
    let mut named: BTreeSet<&FieldId> = request.filter.keys().collect();
    named.extend(agg.field.iter());
    named.extend(agg.group_by.iter());
    let mut types = BTreeMap::new();
    let mut rules: Vec<(&FieldId, &Expr)> = Vec::new();
    for field in named {
        let field_def = object.fields.get(field).ok_or_else(|| {
            DomainError::new(
                ErrorKind::UnknownField,
                format!("{} has no field {field}", object.id),
            )
        })?;
        types.insert(field.clone(), field_def.ty.clone());
        if let Some(rule) = &field_def.read {
            rules.push((field, rule));
        }
    }

    let window = lookup.page_of(&def.object, None, limits::MAX_SCAN_RECORDS + 1)?;
    if window.len() > limits::MAX_SCAN_RECORDS as usize {
        return Err(refusal(
            ErrorKind::LimitExceeded,
            SCAN_BOUND_EXCEEDED,
            format!(
                "aggregate {} of query {query} reads {}, which holds more records than the scan \
                 window of {}",
                agg.id,
                def.object,
                limits::MAX_SCAN_RECORDS
            ),
        ));
    }
    let bound = agg.scan_bound.min(limits::MAX_SCAN_RECORDS) as usize;

    let roles = roles_of(tenant, &ctx.principal);
    let no_params = BTreeMap::new();
    let reads_everything = matches!(ctx.principal, Principal::System);
    let mut selected: Vec<Arc<Record>> = Vec::new();
    for record in window {
        if !visible_in(lookup, &ctx.principal, record, ctx.now)? {
            continue;
        }
        if let Some(filter) = &def.filter {
            let env = Env {
                record: Some(record),
                old: Some(record),
                new: Some(record),
                params: &request.params,
                now: ctx.now,
                principal: &ctx.principal,
                roles: &roles,
                lookup: Some(lookup),
            };
            let outcome = expr::holds(filter, &env);
            if outcome.is_err() {
                if let Some(missing) = missing_error(filter, &env) {
                    return Err(missing);
                }
            }
            if !outcome.unwrap_or(false) {
                continue;
            }
        }
        if !request
            .filter
            .iter()
            .all(|(field, want)| matches(record.get(field), want))
        {
            continue;
        }
        // The read rule of every field the aggregate names, whether or not the record holds a
        // value in it: a record is left out for what the reader may not read, so no figure tells
        // the reader which records hold a value there (AGG-017, D-5061).
        if !reads_everything {
            let env = Env {
                record: Some(record),
                old: Some(record),
                new: Some(record),
                params: &no_params,
                now: ctx.now,
                principal: &ctx.principal,
                roles: &roles,
                lookup: Some(lookup),
            };
            let mut shown_all = true;
            for (_, rule) in &rules {
                let shown = expr::holds(rule, &env);
                if shown.is_err() {
                    if let Some(missing) = missing_error(rule, &env) {
                        return Err(missing);
                    }
                }
                if !shown.unwrap_or(false) {
                    shown_all = false;
                    break;
                }
            }
            if !shown_all {
                continue;
            }
        }
        selected.push(record.clone());
    }
    if selected.len() > bound {
        return Err(refusal(
            ErrorKind::LimitExceeded,
            SCAN_BOUND_EXCEEDED,
            format!(
                "aggregate {} of query {query} would examine more than its scan bound of {bound} \
                 records",
                agg.id
            ),
        ));
    }

    let rows: Vec<Row> = selected
        .iter()
        .map(|record| Row {
            id: record.id,
            group: value_in(record, agg.group_by.as_ref(), &types),
            value: value_in(record, agg.field.as_ref(), &types),
        })
        .collect();
    let mut keyed: BTreeMap<&Value, Vec<&Row>> = BTreeMap::new();
    let mut keyless: Vec<&Row> = Vec::new();
    for row in &rows {
        match (&agg.group_by, &row.group) {
            (Some(_), Some(key)) => keyed.entry(key).or_default().push(row),
            _ => keyless.push(row),
        }
    }
    let mut groups = Vec::new();
    // Groups in ascending order of the group value, the records with none last (D-5063). An
    // ungrouped aggregate is one group, present when there are no records at all.
    for (key, members) in &keyed {
        let (value, listed) = finish(agg, members)?;
        groups.push(AggregateGroup {
            key: Some((*key).clone()),
            value,
            rows: listed,
        });
    }
    if agg.group_by.is_none() || !keyless.is_empty() {
        let (value, listed) = finish(agg, &keyless)?;
        groups.push(AggregateGroup {
            key: None,
            value,
            rows: listed,
        });
    }
    if groups.len() > limits::MAX_AGGREGATE_GROUPS {
        return Err(refusal(
            ErrorKind::LimitExceeded,
            GROUP_BOUND_EXCEEDED,
            format!(
                "aggregate {} of query {query} has {} groups, more than the maximum of {}",
                agg.id,
                groups.len(),
                limits::MAX_AGGREGATE_GROUPS
            ),
        ));
    }
    Ok(AggregateResult {
        query: query.clone(),
        aggregate: agg.id.clone(),
        groups,
        scanned: selected.len() as u32,
        depends_on: depends_on(schema, def, agg),
        revision: tenant.revision,
        schema_version: schema.version,
    })
}

/// The type of a field is one a minimum, a maximum or a top list can order.
fn orderable(ty: &FieldType) -> bool {
    matches!(ty, FieldType::Int | FieldType::Str | FieldType::Time)
}

/// The type of a field is one a group can be made of: a single scalar value.
fn groupable(ty: &FieldType) -> bool {
    matches!(
        ty,
        FieldType::Int
            | FieldType::Str
            | FieldType::Bool
            | FieldType::Time
            | FieldType::Ref { .. }
            | FieldType::Language
    )
}

/// A field an aggregate names exists, is declared by the query, and has a type the aggregate
/// admits.
fn named_field(
    problems: &mut Vec<String>,
    at: &str,
    declared: &[FieldId],
    object: &ObjectTypeDef,
    what: &str,
    field: &FieldId,
    admits: fn(&FieldType) -> bool,
) {
    if !declared.contains(field) {
        problems.push(format!("{at}: {what} {field} is not a declared field"));
    }
    match object.fields.get(field) {
        None => problems.push(format!("{at}: {what} {field}, which {} lacks", object.id)),
        Some(def) if !admits(&def.ty) => problems.push(format!(
            "{at}: {what} {field} of type {} is not one this aggregate admits",
            def.ty.name()
        )),
        Some(_) => {}
    }
}

/// What is wrong with the aggregates and declared fields of the queries of a schema, and with the
/// view sections that name an aggregate, added to `problems`. Run by
/// `TenantSchemaExt::problems`, so each is a refusal of the change that makes it (AGG-002,
/// AGG-023, AGG-2320 to AGG-2322).
pub fn problems(schema: &TenantSchema, problems: &mut Vec<String>) {
    for query in schema.queries.values() {
        let aggregates = aggregates_of(query);
        let declared = declared_of(query);
        if aggregates.is_empty() && declared.is_empty() {
            continue;
        }
        let at = format!("query {}", query.id);
        // An unknown object type is reported with the query.
        let Some(object) = schema.objects.get(&query.object) else {
            continue;
        };
        let mut seen = BTreeSet::new();
        for field in declared {
            if !object.fields.contains_key(field) {
                problems.push(format!("{at}: declares {field}, which {} lacks", object.id));
            }
            if !seen.insert(field) {
                problems.push(format!("{at}: declares {field} twice"));
            }
        }
        if aggregates.len() > limits::MAX_AGGREGATES_PER_QUERY {
            problems.push(format!(
                "limit: {at} declares {} aggregates, more than the maximum of {}",
                aggregates.len(),
                limits::MAX_AGGREGATES_PER_QUERY
            ));
        }
        let mut ids = BTreeSet::new();
        for agg in aggregates {
            let at = format!("{at}.{}", agg.id);
            if !ids.insert(&agg.id) {
                problems.push(format!("{at}: aggregate declared twice"));
            }
            if agg.scan_bound == 0 || agg.scan_bound > limits::MAX_SCAN_RECORDS {
                problems.push(format!(
                    "limit: {at}: a scan bound of {} is outside 1 to {}",
                    agg.scan_bound,
                    limits::MAX_SCAN_RECORDS
                ));
            }
            match (agg.kind, &agg.field) {
                (AggregateKind::Count, None) => {}
                (AggregateKind::Count, Some(field)) => {
                    problems.push(format!("{at}: a count names no field, not {field}"))
                }
                (AggregateKind::Sum, Some(field)) => {
                    let admits: fn(&FieldType) -> bool = |ty| matches!(ty, FieldType::Int);
                    named_field(problems, &at, declared, object, "sum field", field, admits)
                }
                (_, Some(field)) => {
                    named_field(problems, &at, declared, object, "field", field, orderable)
                }
                (kind, None) => problems.push(format!("{at}: a {} names a field", kind.as_str())),
            }
            if let Some(field) = &agg.group_by {
                named_field(
                    problems,
                    &at,
                    declared,
                    object,
                    "group field",
                    field,
                    groupable,
                );
            }
            match (agg.kind, agg.n) {
                (AggregateKind::Top, Some(n)) if (1..=limits::MAX_TOP_LEN).contains(&n) => {}
                (AggregateKind::Top, n) => problems.push(format!(
                    "{at}: a top list is of 1 to {} records, not {n:?}",
                    limits::MAX_TOP_LEN
                )),
                (_, Some(n)) => {
                    problems.push(format!("{at}: only a top list states a length, not {n}"))
                }
                (_, None) => {}
            }
        }
    }
    for view in schema.views.values() {
        for (name, section) in &view.sections {
            let Some(wanted) = &section.aggregate else {
                continue;
            };
            let at = format!("view {}.{name}", view.id);
            let Some(query) = schema.queries.get(&section.query) else {
                continue;
            };
            if !aggregates_of(query).iter().any(|a| a.id == *wanted) {
                problems.push(format!(
                    "{at}: query {} declares no aggregate {wanted}",
                    query.id
                ));
            }
            if !section.include.is_empty() || !section.markdown.is_empty() || section.single {
                problems.push(format!(
                    "{at}: a section that names an aggregate delivers no rows, so it includes \
                     and renders no field and is not single"
                ));
            }
        }
    }
}

/// The aggregate a view section names, when its query declares it.
pub fn section_aggregate<'a>(
    schema: &'a TenantSchema,
    query: &crate::types::QueryId,
    aggregate: &AggregateId,
) -> Option<&'a AggregateDef> {
    schema
        .queries
        .get(query)
        .and_then(|def| aggregates_of(def).iter().find(|a| a.id == *aggregate))
}
