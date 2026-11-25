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
    ArgSource, FieldDef, FieldId, FieldType, ObjectTypeDef, ObjectTypeId, PageCursor, Principal,
    ReadRequest, Record, RecordId, SchemaVersion, SettingId, StateRevision, TenantSchema,
    TenantState, UiPresentation, Value, ViewDef, ViewId, ViewSection,
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

/// The reference fields of a section's object, as the reader may follow them: the target object
/// type, and the title of each target record the reader may read. A target that is not here is not
/// readable, and the reply says so without its id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefTargets {
    pub object: ObjectTypeId,
    pub titles: BTreeMap<RecordId, Option<String>>,
}

/// What a reply may say of one principal: a display name and an avatar (a file address), each
/// `None` when the reader may not know it, and whether the principal is the reader.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonInfo {
    pub name: Option<String>,
    pub avatar: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reader: bool,
}

/// The setting that says who may see the display names of the members a view names: `members`
/// (the default: signed-in readers), `everyone` (anonymous readers too) or `nobody`.
pub const MEMBER_NAMES_SETTING: &str = "member_names";

/// The display name a reader may be given for a member, as the member-name setting allows. A
/// reader is always given their own name; any other value of the setting reads as `members`.
pub fn name_for_reader(
    mode: &str,
    signed_in: bool,
    is_reader: bool,
    name: Option<String>,
) -> Option<String> {
    let allowed = is_reader
        || match mode {
            "everyone" => true,
            "nobody" => false,
            _ => signed_in,
        };
    name.filter(|name| allowed && !name.trim().is_empty())
}

/// The field whose text titles a record of an object type: a field named `title` or `name` when
/// it is a required string, else the first required string field; none when there is no such
/// field. The title the object type's presentation declares comes first, when it is a field.
pub fn title_field<'a>(
    def: &'a ObjectTypeDef,
    presentation: Option<&UiPresentation>,
) -> Option<&'a FieldId> {
    let required_str = |field: &&FieldDef| field.required && matches!(field.ty, FieldType::Str);
    if let Some(declared) = presentation.and_then(|p| p.title.as_ref()) {
        if let Some(field) = def.fields.get(declared) {
            return Some(&field.id);
        }
    }
    ["title", "name"]
        .iter()
        .find_map(|wanted| {
            def.fields
                .values()
                .filter(required_str)
                .find(|field| field.id.as_str() == *wanted)
        })
        .or_else(|| def.fields.values().find(required_str))
        .map(|field| &field.id)
}

/// The title of a record: the text of its [`title_field`], when it holds some.
pub fn title_of(
    def: &ObjectTypeDef,
    presentation: Option<&UiPresentation>,
    record: &Record,
) -> Option<String> {
    match record.get(title_field(def, presentation)?) {
        Value::Str(text) if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    }
}

/// The raw result of one view section, before projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionData {
    pub single: bool,
    pub rows: Vec<Arc<Record>>,
    /// Referenced records the principal may read, by referencing field.
    pub included: BTreeMap<FieldId, BTreeMap<RecordId, Arc<Record>>>,
    pub markdown: Vec<FieldId>,
    /// Each reference field of the section's object: where it points and the titles of the
    /// targets the reader may read (included or not).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ref_targets: BTreeMap<FieldId, RefTargets>,
    /// The cursor that fetches the next page of this section, as [`encode_cursor`] writes it;
    /// `None` at the end of the list and for a single-row section (CMS-164).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// The result of the aggregate the section names, in place of rows (D-5067, AGG-041).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<AggregateResult>,
    /// For a descendants query (WG-5510), the level of each row below its root, in row order: each row
    /// carries it as `depth`. Absent for every other query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depths: Option<Vec<u32>>,
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
    /// The settings the view's arguments read, for this reader (SET-1540, WG-5600): a notice of a
    /// change of one of them reads the view again.
    pub settings: BTreeSet<SettingId>,
    pub revision: StateRevision,
    pub schema_version: SchemaVersion,
    /// What the kernel will check on a record of each object type the reader asked about (the
    /// request names them in `rules`), for this reader: see `strata_kernel::editor_rules`. Sent
    /// in the model as `record_rules`; left out while nothing was asked.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub record_rules: BTreeMap<String, Json>,
    /// What the reader may know of each principal the rows name, by principal. [`project`] sends
    /// handles in their place.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub people: BTreeMap<String, PersonInfo>,
    /// The preset the tenant's own configuration names (see `theme::tenant_theme`); the page's
    /// settings record, when it has one that says, comes first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_theme: Option<String>,
    /// The fields drawn as a person, by object type (from the tenant's presentations): their
    /// string values are principals, which [`project`] sends as handles into `people`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub person_fields: BTreeMap<ObjectTypeId, BTreeSet<FieldId>>,
    /// The answers of preflight for the row actions of the view, by section, record and action
    /// (see [`crate::affordance`]); left out when the view has no row action.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub affordances: crate::affordance::Affordances,
    /// Whether an affordance reads the principal or the clock: the reply is then individual to the
    /// reader and is never stored as a shared copy.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub individual: bool,
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

/// An aggregate result as the browser's view model holds it: each group with its key and second
/// key (null for the records that have none), its value and, for a top list, its rows, and the
/// object types the result depends on (AGG-042).
pub fn project_aggregate(result: &AggregateResult) -> Json {
    let groups: Vec<Json> = result
        .groups
        .iter()
        .map(|group| {
            json!({
                "key": group.key.as_ref().map(to_plain),
                "key_second": group.key_second.as_ref().map(to_plain),
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

/// The people a reply names: each principal gets an opaque handle (`p1`, `p2`, ...) in order of
/// first appearance, and the principal string goes no further.
struct People<'a> {
    known: &'a BTreeMap<String, PersonInfo>,
    person_fields: &'a BTreeMap<ObjectTypeId, BTreeSet<FieldId>>,
    handles: BTreeMap<String, String>,
    table: Map<String, Json>,
}

impl<'a> People<'a> {
    fn new(data: &'a ViewData) -> Self {
        Self {
            known: &data.people,
            person_fields: &data.person_fields,
            handles: BTreeMap::new(),
            table: Map::new(),
        }
    }

    fn handle(&mut self, principal: &str) -> String {
        if let Some(handle) = self.handles.get(principal) {
            return handle.clone();
        }
        let handle = format!("p{}", self.handles.len() + 1);
        let info = self.known.get(principal).cloned().unwrap_or_default();
        let mut entry = Map::new();
        entry.insert("name".into(), json!(info.name));
        entry.insert("avatar".into(), json!(info.avatar));
        if info.reader {
            entry.insert("self".into(), json!(true));
        }
        self.table.insert(handle.clone(), Json::Object(entry));
        self.handles.insert(principal.to_string(), handle.clone());
        handle
    }
}

/// A reference as the reply sends it: the target's object, id and title when the reader may read
/// it, else `{"ref": null}`, with no id.
fn ref_value(targets: &RefTargets, id: RecordId) -> Json {
    match targets.titles.get(&id) {
        Some(title) => json!({ "ref": {
            "object": targets.object.as_str(),
            "id": id.0,
            "title": title,
        } }),
        None => json!({ "ref": null }),
    }
}

fn project_record(
    record: &Record,
    people: &mut People<'_>,
    targets: Option<&BTreeMap<FieldId, RefTargets>>,
) -> Map<String, Json> {
    let declared = people.person_fields;
    let persons = declared.get(&record.object);
    let mut out = Map::new();
    out.insert("id".into(), json!(record.id.0));
    out.insert("object".into(), json!(record.object.as_str()));
    out.insert("version".into(), json!(record.version));
    out.insert("created_at".into(), json!(record.created_at.0));
    out.insert("updated_at".into(), json!(record.updated_at.0));
    out.insert(
        "created_by".into(),
        match &record.created_by {
            Some(principal) => json!({ "person": people.handle(principal.as_str()) }),
            None => Json::Null,
        },
    );
    let fields: Map<String, Json> = record
        .fields
        .iter()
        .map(|(field, value)| {
            let plain = match (targets.and_then(|all| all.get(field)), value) {
                (_, Value::Str(principal))
                    if !principal.is_empty() && persons.is_some_and(|f| f.contains(field)) =>
                {
                    json!({ "person": people.handle(principal) })
                }
                (Some(target), Value::Ref(id)) => ref_value(target, *id),
                (Some(target), Value::List(items)) => Json::Array(
                    items
                        .iter()
                        .map(|item| match item {
                            Value::Ref(id) => ref_value(target, *id),
                            other => to_plain(other),
                        })
                        .collect(),
                ),
                _ => to_plain(value),
            };
            (field.as_str().to_string(), plain)
        })
        .collect();
    out.insert("fields".into(), Json::Object(fields));
    out
}

fn project_row(
    record: &Record,
    section: &SectionData,
    people: &mut People<'_>,
    depth: Option<u32>,
) -> Json {
    let mut out = project_record(record, people, Some(&section.ref_targets));
    if let Some(depth) = depth {
        out.insert("depth".into(), json!(depth));
    }
    let mut refs = Map::new();
    for (field, included) in &section.included {
        let mut resolve = |id: &RecordId| {
            included
                .get(id)
                .map(|r| Json::Object(project_record(r, people, None)))
        };
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
///
/// A principal is never sent: a record's author is `{"person": "<handle>"}`, and the reply's
/// `people` table says what the reader may know of each handle. A reference field is
/// `{"ref": {"object", "id", "title"}}`, or `{"ref": null}` when the reader may not read the
/// target.
pub fn project(data: &ViewData) -> Json {
    let mut people = People::new(data);
    let mut sections = Map::new();
    for (name, section) in &data.sections {
        let value = if let Some(aggregate) = &section.aggregate {
            project_aggregate(aggregate)
        } else if section.single {
            section.rows.first().map_or(Json::Null, |row| {
                project_row(row, section, &mut people, None)
            })
        } else {
            Json::Array(
                section
                    .rows
                    .iter()
                    .enumerate()
                    .map(|(index, row)| {
                        let depth = section.depths.as_ref().and_then(|depths| depths.get(index));
                        project_row(row, section, &mut people, depth.copied())
                    })
                    .collect(),
            )
        };
        sections.insert(name.clone(), value);
    }
    let next: Map<String, Json> = data
        .sections
        .iter()
        .filter_map(|(name, section)| Some((name.clone(), json!(section.next.as_ref()?))))
        .collect();
    let mut model = json!({
        "view": data.view.as_str(),
        "title": data.title,
        "params": data.params,
        "data": sections,
        "people": people.table,
        "depends_on": data.depends_on.iter().map(|o| o.as_str()).collect::<Vec<_>>(),
        "revision": data.revision.0,
        "schema_version": data.schema_version.0,
    });
    // The cursors of the sections that have a further page: a member only when there is one, as
    // the view contracts declare no `next` output for a model with no further page.
    if !next.is_empty() {
        model["next"] = json!(next);
    }
    // The settings the arguments read, so that a change of one of them reads the view again
    // (WG-5600). Named only when there are some, as the view contracts declare no output for them.
    if !data.settings.is_empty() {
        model["settings"] = json!(data
            .settings
            .iter()
            .map(|setting| setting.as_str())
            .collect::<Vec<_>>());
    }
    // The editor's rules are a member of the model of their own, the output `record-rules` of the
    // view contract (CT-251), not a section of `data`: a section is a record or a list of them.
    if !data.record_rules.is_empty() {
        model["record_rules"] = json!(data.record_rules);
    }
    // The row actions' answers (WG-5466): by section, record id and action id, each allowed,
    // refused with its condition's code and wording, or unknown. Individual when one reads the
    // principal or the clock.
    if !data.affordances.is_empty() {
        model["affordances"] = json!(data.affordances);
    }
    if data.individual {
        model["individual"] = json!(true);
    }
    model
}

#[cfg(test)]
mod people_and_refs {
    use super::*;
    use strata_kernel::testkit::{field, field_def, object, principal_id};
    use strata_kernel::Timestamp;

    fn record(id: u64, author: &str, fields: Vec<(&str, Value)>) -> Arc<Record> {
        Arc::new(Record {
            id: RecordId(id),
            object: object("post"),
            version: 1,
            created_by: Some(principal_id(author)),
            created_at: Timestamp(0),
            updated_at: Timestamp(0),
            fields: fields.into_iter().map(|(f, v)| (field(f), v)).collect(),
        })
    }

    fn data(rows: Vec<Arc<Record>>, ref_targets: BTreeMap<FieldId, RefTargets>) -> ViewData {
        let section = SectionData {
            single: false,
            rows,
            included: BTreeMap::new(),
            markdown: Vec::new(),
            ref_targets,
            next: None,
            aggregate: None,
            depths: None,
        };
        ViewData {
            view: ViewId::new("home").unwrap(),
            title: "Home".into(),
            params: BTreeMap::new(),
            sections: BTreeMap::from([("posts".to_string(), section)]),
            depends_on: BTreeSet::new(),
            settings: BTreeSet::new(),
            revision: StateRevision(1),
            schema_version: SchemaVersion(1),
            record_rules: BTreeMap::new(),
            tenant_theme: None,
            person_fields: BTreeMap::from([(object("post"), BTreeSet::from([field("editor")]))]),
            affordances: BTreeMap::new(),
            individual: false,
            people: BTreeMap::from([
                (
                    "user:ada".to_string(),
                    PersonInfo {
                        name: Some("Ada Okafor".into()),
                        avatar: None,
                        reader: true,
                    },
                ),
                ("user:bob".to_string(), PersonInfo::default()),
            ]),
        }
    }

    /// The reply carries no principal string; authors are handles into `people`.
    #[test]
    fn a_view_names_the_settings_its_arguments_read_only_when_it_reads_some() {
        let mut with = data(Vec::new(), BTreeMap::new());
        with.settings = BTreeSet::from([SettingId::new("page_size").unwrap()]);
        assert_eq!(project(&with)["settings"], json!(["page_size"]));
        let without = data(Vec::new(), BTreeMap::new());
        assert!(project(&without).get("settings").is_none());
    }

    #[test]
    fn a_reply_carries_handles_and_no_principal_string() {
        let rows = vec![
            record(
                1,
                "user:ada",
                vec![("editor", Value::Str("user:bob".into()))],
            ),
            record(2, "user:bob", vec![]),
            record(3, "user:ada", vec![]),
        ];
        let model = project(&data(rows, BTreeMap::new()));
        let text = model.to_string();
        assert!(!text.contains("user:"), "{text}");
        let posts = &model["data"]["posts"];
        assert_eq!(posts[0]["created_by"], json!({ "person": "p1" }));
        assert_eq!(
            posts[0]["fields"]["editor"],
            json!({ "person": "p2" }),
            "a field the presentation draws as a person is a handle too"
        );
        assert_eq!(posts[1]["created_by"], json!({ "person": "p2" }));
        assert_eq!(posts[2]["created_by"], json!({ "person": "p1" }));
        assert_eq!(
            model["people"],
            json!({
                "p1": { "name": "Ada Okafor", "avatar": null, "self": true },
                "p2": { "name": null, "avatar": null },
            })
        );
    }

    /// A readable target is `{"ref": {object, id, title}}`; an unreadable one is
    /// `{"ref": null}` with no id.
    #[test]
    fn an_unreadable_reference_target_is_ref_null() {
        let targets = BTreeMap::from([(
            field("author"),
            RefTargets {
                object: object("author"),
                titles: BTreeMap::from([(RecordId(7), Some("Ines Vale".to_string()))]),
            },
        )]);
        let rows = vec![
            record(1, "user:ada", vec![("author", Value::Ref(RecordId(7)))]),
            record(2, "user:ada", vec![("author", Value::Ref(RecordId(9)))]),
            record(
                3,
                "user:ada",
                vec![(
                    "author",
                    Value::List(vec![Value::Ref(RecordId(7)), Value::Ref(RecordId(9))]),
                )],
            ),
        ];
        let model = project(&data(rows, targets));
        let posts = &model["data"]["posts"];
        assert_eq!(
            posts[0]["fields"]["author"],
            json!({ "ref": { "object": "author", "id": 7, "title": "Ines Vale" } })
        );
        assert_eq!(posts[1]["fields"]["author"], json!({ "ref": null }));
        assert_eq!(
            posts[2]["fields"]["author"],
            json!([
                { "ref": { "object": "author", "id": 7, "title": "Ines Vale" } },
                { "ref": null }
            ])
        );
        assert!(!posts[1].to_string().contains('9'));
    }

    /// The title of a target is its `title` or `name` field, else its first required
    /// string field.
    #[test]
    fn a_title_field_is_chosen_by_name_then_by_requirement() {
        let def = |fields: Vec<FieldDef>| ObjectTypeDef {
            id: object("thing"),
            label: "Thing".into(),
            fields: fields.into_iter().map(|f| (f.id.clone(), f)).collect(),
            constraints: BTreeMap::new(),
            retention: None,
            publication: None,
            language_field: None,
            translation_group: None,
            unique: None,
            searchable: None,
        };
        let slug_and_title = def(vec![
            field_def("slug", FieldType::Str, true),
            field_def("title", FieldType::Str, true),
        ]);
        assert_eq!(title_field(&slug_and_title, None), Some(&field("title")));
        let only_slug = def(vec![
            field_def("slug", FieldType::Str, true),
            field_def("note", FieldType::Str, false),
        ]);
        assert_eq!(title_field(&only_slug, None), Some(&field("slug")));
        let declared = UiPresentation {
            object: object("thing"),
            title: Some(field("note")),
            fields: BTreeMap::new(),
        };
        assert_eq!(
            title_field(&only_slug, Some(&declared)),
            Some(&field("note")),
            "a declared title wins"
        );
        let none = def(vec![field_def("rank", FieldType::Int, true)]);
        assert_eq!(title_field(&none, None), None);
    }

    /// A name follows the member-name setting: signed-in members by default, everyone or nobody
    /// when the tenant says so, and a reader always has their own.
    #[test]
    fn names_follow_the_member_name_setting() {
        let ada = || Some("Ada Okafor".to_string());
        // The default: signed-in readers see names, anonymous readers do not.
        assert_eq!(name_for_reader("members", true, false, ada()), ada());
        assert_eq!(name_for_reader("members", false, false, ada()), None);
        assert_eq!(name_for_reader("unheard-of", false, false, ada()), None);
        // Everyone: an anonymous reader sees names too.
        assert_eq!(name_for_reader("everyone", false, false, ada()), ada());
        // Nobody: no one sees another member's name, a reader still sees their own.
        assert_eq!(name_for_reader("nobody", true, false, ada()), None);
        assert_eq!(name_for_reader("nobody", true, true, ada()), ada());
        // A member with no name, or an empty one, is "A member" to the client.
        assert_eq!(name_for_reader("everyone", true, false, None), None);
        assert_eq!(
            name_for_reader("everyone", true, false, Some(" ".into())),
            None
        );
    }
}
