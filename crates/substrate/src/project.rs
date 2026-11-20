//! View definitions interpreted: route matching, the reads a view needs, and projection of the
//! results into a plain view model for the browser.

use crate::markdown;
use crate::plain::to_plain;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::expr::attribute;
use strata_kernel::ext::*;
use strata_kernel::refs_in;
use strata_kernel::types::{AggregateId, AggregateRequest, AggregateResult};
use strata_kernel::{
    ArgSource, FieldId, ObjectTypeId, PageCursor, Principal, ReadRequest, Record, RecordId,
    SchemaVersion, SettingId, StateRevision, TenantSchema, TenantState, Value, ViewDef, ViewId,
    ViewSection,
};

/// Find the view whose route pattern matches a path, with the values of its `:name` segments.
/// When several match, the one with the most literal segments wins.
pub fn match_route<'a>(
    schema: &'a TenantSchema,
    path: &str,
) -> Option<(&'a ViewDef, BTreeMap<String, String>)> {
    let wanted: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut best: Option<(usize, &ViewDef, BTreeMap<String, String>)> = None;
    for view in schema.views.values() {
        let pattern: Vec<&str> = view.route.split('/').filter(|s| !s.is_empty()).collect();
        if pattern.len() != wanted.len() {
            continue;
        }
        let mut params = BTreeMap::new();
        let mut literal = 0;
        let matched = pattern
            .iter()
            .zip(&wanted)
            .all(|(p, w)| match p.strip_prefix(':') {
                Some(name) => {
                    params.insert(name.to_string(), (*w).to_string());
                    true
                }
                None => {
                    literal += 1;
                    p == w
                }
            });
        if matched && best.as_ref().is_none_or(|(score, _, _)| literal > *score) {
            best = Some((literal, view, params));
        }
    }
    best.map(|(_, view, params)| (view, params))
}

/// The raw result of one view section, before projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionData {
    pub single: bool,
    pub rows: Vec<Arc<Record>>,
    /// Referenced records the principal may read, by referencing field.
    pub included: BTreeMap<FieldId, BTreeMap<RecordId, Arc<Record>>>,
    pub markdown: Vec<FieldId>,
    /// The cursor that fetches the next page of this section, as [`encode_cursor`] writes it;
    /// `None` at the end of the list and for a single-row section (CMS-164).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// The result of the aggregate the section names, in place of rows (D-5067, AGG-041).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<AggregateResult>,
}

/// The raw result of a view: what the backend hands the BFF.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewData {
    pub view: ViewId,
    pub title: String,
    pub params: BTreeMap<String, String>,
    pub sections: BTreeMap<String, SectionData>,
    /// Object types whose changes can alter this view.
    pub depends_on: BTreeSet<ObjectTypeId>,
    pub revision: StateRevision,
    pub schema_version: SchemaVersion,
}

/// Section names in evaluation order: sections that feed others come first.
pub fn section_order(view: &ViewDef) -> Vec<&String> {
    let depends = |section: &ViewSection| {
        section
            .args
            .values()
            .any(|a| matches!(a, ArgSource::Section { .. }))
    };
    let (late, early): (Vec<_>, Vec<_>) = view
        .sections
        .iter()
        .partition(|(_, section)| depends(section));
    early
        .into_iter()
        .chain(late)
        .map(|(name, _)| name)
        .collect()
}

/// The read request for one section, given the route and request parameters and the single rows
/// of the sections already evaluated. An argument that comes from a setting is null here: see
/// [`section_request_with`].
pub fn section_request(
    section: &ViewSection,
    route: &BTreeMap<String, String>,
    request: &BTreeMap<String, String>,
    done: &BTreeMap<String, Option<Arc<Record>>>,
) -> ReadRequest {
    section_request_with(section, route, request, done, &BTreeMap::new())
}

/// The settings a view reads: the setting of each section argument that comes from one (SET-1540).
pub fn view_settings(view: &ViewDef) -> BTreeSet<SettingId> {
    view.sections
        .values()
        .flat_map(|section| section.args.values())
        .filter_map(|source| match source {
            ArgSource::Setting(setting) => Some(setting.clone()),
            _ => None,
        })
        .collect()
}

/// The value of each setting a view reads, resolved for the reader (SET-1540): what an argument
/// that comes from a setting is given. A setting the tenant does not define resolves to nothing,
/// and its argument is null. Pure: the same tenant, reader and view give the same values.
pub fn resolved_settings(
    tenant: &TenantState,
    principal: &Principal,
    view: &ViewDef,
) -> BTreeMap<SettingId, Value> {
    view_settings(view)
        .into_iter()
        .filter_map(|setting| {
            let found = strata_kernel::settings::resolve(tenant, principal, &setting).ok()?;
            Some((setting, found.value))
        })
        .collect()
}

/// [`section_request`] with the settings the reader resolves, as [`resolved_settings`] gives them.
pub fn section_request_with(
    section: &ViewSection,
    route: &BTreeMap<String, String>,
    request: &BTreeMap<String, String>,
    done: &BTreeMap<String, Option<Arc<Record>>>,
    settings: &BTreeMap<SettingId, Value>,
) -> ReadRequest {
    let text = |value: Option<&String>| value.map_or(Value::Null, |s| Value::Str(s.clone()));
    let params = section
        .args
        .iter()
        .map(|(name, source)| {
            let value = match source {
                ArgSource::Route(key) => text(route.get(key)),
                ArgSource::Request(key) => text(request.get(key)),
                ArgSource::Const(value) => value.clone(),
                ArgSource::Setting(setting) => {
                    settings.get(setting).cloned().unwrap_or(Value::Null)
                }
                ArgSource::Section {
                    section,
                    attribute: name,
                } => done
                    .get(section)
                    .and_then(|row| row.as_deref())
                    .and_then(|row| attribute(Some(row), name).ok())
                    .unwrap_or(Value::Null),
            };
            (name.clone(), value)
        })
        .collect();
    ReadRequest::Named {
        query: section.query.clone(),
        params,
        page_size: None,
        cursor: None,
    }
}

/// The request of a section for one page: the section's own request with the page size and the
/// cursor of the delivery request (CMS-164). The read answers with the cursor of the page after.
pub fn with_page(
    request: ReadRequest,
    size: Option<u32>,
    after: Option<PageCursor>,
) -> ReadRequest {
    match request {
        ReadRequest::Named { query, params, .. } => ReadRequest::Named {
            query,
            params,
            page_size: size,
            cursor: after,
        },
        other => other,
    }
}

/// A page cursor as the browser holds it: `a<id>` continues after a record, `o<n>` skips `n`
/// rows, `t<hex>` continues a query planned on the index of its order field after an index
/// position, the hex digits of the position's JSON. Opaque to the client.
pub fn encode_cursor(cursor: &PageCursor) -> String {
    match cursor {
        PageCursor::After(id) => format!("a{}", id.0),
        PageCursor::Offset(rows) => format!("o{rows}"),
        PageCursor::At(position) => {
            let text = serde_json::to_string(position).unwrap_or_default();
            let mut out = String::with_capacity(1 + 2 * text.len());
            out.push('t');
            for byte in text.bytes() {
                out.push_str(&format!("{byte:02x}"));
            }
            out
        }
    }
}

/// The longest `t` cursor [`decode_cursor`] reads: an index position holds one indexed value,
/// whose key is bounded, and a record id.
const MAX_POSITION_CURSOR: usize = 2 * 1024;

/// The cursor an [`encode_cursor`] text stands for; `None` for any other text.
pub fn decode_cursor(text: &str) -> Option<PageCursor> {
    let (kind, rest) = text.split_at_checked(1)?;
    if kind == "t" {
        if rest.is_empty() || rest.len() > MAX_POSITION_CURSOR || rest.len() % 2 != 0 {
            return None;
        }
        let bytes = (0..rest.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(rest.get(i..i + 2)?, 16).ok())
            .collect::<Option<Vec<u8>>>()?;
        return serde_json::from_slice::<strata_kernel::query_plan::IndexPosition>(&bytes)
            .ok()
            .map(PageCursor::At);
    }
    let digits = rest;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match kind {
        "a" => digits
            .parse()
            .ok()
            .map(|id| PageCursor::After(RecordId(id))),
        "o" => digits.parse().ok().map(PageCursor::Offset),
        _ => None,
    }
}

/// The record as a reader who asked for `selected` fields receives it: those fields it holds and
/// no others. Narrowing happens after the read, so a field the kernel redacted is not there to
/// keep, and the selection can only take away from what the reader may see.
pub fn select_fields(record: &Arc<Record>, selected: &BTreeSet<FieldId>) -> Arc<Record> {
    if record.fields.keys().all(|field| selected.contains(field)) {
        return record.clone();
    }
    let mut narrowed = Record::clone(record);
    narrowed.fields.retain(|field, _| selected.contains(field));
    Arc::new(narrowed)
}

/// The request for the aggregate a section names: the arguments of the section, as
/// [`section_request`] reads them, and no filter (D-5067).
pub fn aggregate_request(
    section: &ViewSection,
    aggregate: &AggregateId,
    route: &BTreeMap<String, String>,
    request: &BTreeMap<String, String>,
    done: &BTreeMap<String, Option<Arc<Record>>>,
) -> AggregateRequest {
    aggregate_request_with(section, aggregate, route, request, done, &BTreeMap::new())
}

/// [`aggregate_request`] with the settings the reader resolves ([`resolved_settings`]).
pub fn aggregate_request_with(
    section: &ViewSection,
    aggregate: &AggregateId,
    route: &BTreeMap<String, String>,
    request: &BTreeMap<String, String>,
    done: &BTreeMap<String, Option<Arc<Record>>>,
    settings: &BTreeMap<SettingId, Value>,
) -> AggregateRequest {
    let params = match section_request_with(section, route, request, done, settings) {
        ReadRequest::Named { params, .. } => params,
        ReadRequest::Records { .. } => BTreeMap::new(),
    };
    AggregateRequest {
        query: section.query.clone(),
        aggregate: aggregate.clone(),
        params,
        filter: BTreeMap::new(),
    }
}

/// An aggregate result as the browser's view model holds it: each group with its key (null for
/// the records that have none), its value and, for a top list, its rows, and the object types the
/// result depends on (AGG-042).
pub fn project_aggregate(result: &AggregateResult) -> Json {
    let groups: Vec<Json> = result
        .groups
        .iter()
        .map(|group| {
            json!({
                "key": group.key.as_ref().map(to_plain),
                "value": group.value.as_ref().map(to_plain),
                "rows": group
                    .rows
                    .iter()
                    .map(|row| serde_json::from_str::<Json>(row.as_str()).unwrap_or(Json::Null))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "query": result.query.as_str(),
        "aggregate": result.aggregate.as_str(),
        "scanned": result.scanned,
        "groups": groups,
        "depends_on": result.depends_on.iter().map(|o| o.as_str()).collect::<Vec<_>>(),
        "revision": result.revision.0,
        "schema_version": result.schema_version.0,
    })
}

/// The record ids a section's rows refer to through its included fields, by target object.
pub fn included_refs(
    schema: &TenantSchema,
    object: &ObjectTypeId,
    section: &ViewSection,
    rows: &[Arc<Record>],
) -> BTreeMap<FieldId, (ObjectTypeId, Vec<RecordId>)> {
    let mut out = BTreeMap::new();
    let Some(def) = schema.objects.get(object) else {
        return out;
    };
    for field in &section.include {
        let Some((target, _, _)) = def.fields.get(field).and_then(|f| f.ty.ref_target()) else {
            continue;
        };
        let ids: BTreeSet<RecordId> = rows
            .iter()
            .flat_map(|row| refs_in(row.get(field)))
            .collect();
        out.insert(field.clone(), (target.clone(), ids.into_iter().collect()));
    }
    out
}

fn project_record(record: &Record) -> Map<String, Json> {
    let mut out = Map::new();
    out.insert("id".into(), json!(record.id.0));
    out.insert("object".into(), json!(record.object.as_str()));
    out.insert("version".into(), json!(record.version));
    out.insert("created_at".into(), json!(record.created_at.0));
    out.insert("updated_at".into(), json!(record.updated_at.0));
    out.insert(
        "created_by".into(),
        json!(record.created_by.as_ref().map(|p| p.as_str())),
    );
    let fields: Map<String, Json> = record
        .fields
        .iter()
        .map(|(field, value)| (field.as_str().to_string(), to_plain(value)))
        .collect();
    out.insert("fields".into(), Json::Object(fields));
    out
}

fn project_row(record: &Record, section: &SectionData) -> Json {
    let mut out = project_record(record);
    let mut refs = Map::new();
    for (field, included) in &section.included {
        let resolve = |id: &RecordId| included.get(id).map(|r| Json::Object(project_record(r)));
        let projected = match record.get(field) {
            Value::Ref(id) => resolve(id).unwrap_or(Json::Null),
            Value::List(items) => Json::Array(
                items
                    .iter()
                    .filter_map(|item| {
                        if let Value::Ref(id) = item {
                            resolve(id)
                        } else {
                            None
                        }
                    })
                    .collect(),
            ),
            _ => continue,
        };
        refs.insert(field.as_str().to_string(), projected);
    }
    out.insert("refs".into(), Json::Object(refs));
    let mut blocks = Map::new();
    let mut words = 0;
    for field in &section.markdown {
        if let Value::Str(source) = record.get(field) {
            let parsed = markdown::parse(source);
            words += markdown::word_count(&parsed);
            blocks.insert(
                field.as_str().to_string(),
                serde_json::to_value(parsed).expect("blocks serialise"),
            );
        }
    }
    if !section.markdown.is_empty() {
        out.insert("blocks".into(), Json::Object(blocks));
        // 230 words a minute, rounded up, never less than one.
        out.insert("reading_minutes".into(), json!(words.div_ceil(230).max(1)));
    }
    Json::Object(out)
}

/// Project raw view data into the plain view model the browser renders. A single section
/// becomes one object or `null`; any other section becomes an array.
pub fn project(data: &ViewData) -> Json {
    let sections: Map<String, Json> = data
        .sections
        .iter()
        .map(|(name, section)| {
            let value = if let Some(aggregate) = &section.aggregate {
                project_aggregate(aggregate)
            } else if section.single {
                section
                    .rows
                    .first()
                    .map_or(Json::Null, |row| project_row(row, section))
            } else {
                Json::Array(
                    section
                        .rows
                        .iter()
                        .map(|row| project_row(row, section))
                        .collect(),
                )
            };
            (name.clone(), value)
        })
        .collect();
    let next: Map<String, Json> = data
        .sections
        .iter()
        .filter_map(|(name, section)| Some((name.clone(), json!(section.next.as_ref()?))))
        .collect();
    json!({
        "view": data.view.as_str(),
        "title": data.title,
        "params": data.params,
        "data": sections,
        "next": next,
        "depends_on": data.depends_on.iter().map(|o| o.as_str()).collect::<Vec<_>>(),
        "revision": data.revision.0,
        "schema_version": data.schema_version.0,
    })
}
