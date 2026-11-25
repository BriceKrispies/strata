//! The interface definition of a tenant as it is served: parts named by their hashes, and a
//! manifest named by the hash of its list of parts (D-8250, INT-3253).
//!
//! The kernel holds the interface elements in the tenant schema (`strata_kernel::ui`). This module
//! groups them into the parts a client fetches:
//!
//! - one part for each screen: the screen, with the forms, actions and live bindings on it;
//! - one part holding every error message, when the tenant has any.
//!
//! A part is a JSON document named by the lower-case hex SHA-256 of its canonical encoding
//! ([`strata_kernel::canon::hash_of`]). The manifest lists every part, in the order of the ids of
//! the screens and then the error messages, each with its hash and, for a screen, the facts a
//! router and a page frame need before they fetch the part: its route, its view, its title, its
//! layout, who may open it and its query parameters. The manifest is named by the hash of that
//! list, so a change to one screen changes one part and the manifest, and a change elsewhere in
//! the schema that no screen reads (a policy, a record type no section or form reads) changes
//! neither: a client compares one hash to know whether
//! what it holds is current (WG-5332), and a part it holds stays valid for as long as its hash is
//! listed (WG-5307). The schema version the manifest was computed at is carried beside the hash,
//! not inside it.
//!
//! A screen part also carries the shape of what it binds: for each section of its view, the object
//! type and, for each field, its id, type and whether it is required (and its display type, when
//! one is declared); for each form and action, the operation and its inputs. Each shape has a
//! fingerprint, the SHA-256 of its canonical JSON, so a change to a field's type changes the
//! shape, the fingerprint and the part's hash, and a client that formats values by type redraws.
//!
//! Pure: the same schema gives the same definition, byte for byte.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as Json};
use std::collections::BTreeMap;
use strata_kernel::canon::hash_of;
use strata_kernel::{FieldType, TenantSchema, UiElement, ViewId};

/// The kind of a part that holds one screen.
pub const SCREEN_PART: &str = "screen";
/// The kind of the part that holds the error messages.
pub const ERRORS_PART: &str = "errors";
/// The id of the part that holds the error messages.
pub const ERRORS_ID: &str = "errors";

/// One entry of the manifest: a part, its hash and, for a screen, what is needed to route to it
/// and to title it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// `screen` or `errors`.
    pub kind: String,
    /// The id of the screen, or `errors`.
    pub id: String,
    /// The hash of the part.
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// The view of the tenant schema the screen shows (the `view` of its view model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// The title the screen states; `${path}` parts are read from the view model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// How the interpreter lays the screen out (`ui-screen-kind`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// Who may open it (`ui-auth`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<Vec<String>>,
}

/// The manifest of a definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The hash of `parts` (INT-3253).
    pub hash: String,
    /// The schema version the manifest was computed at; not part of the hash.
    pub schema_version: u64,
    pub parts: Vec<ManifestEntry>,
}

/// The definition of a tenant: its manifest and every part, by hash.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Definition {
    pub manifest: Manifest,
    pub parts: BTreeMap<String, Json>,
}

impl Definition {
    /// The part with this hash, when the definition holds one.
    pub fn part(&self, hash: &str) -> Option<&Json> {
        self.parts.get(hash)
    }
}

impl Manifest {
    /// The screen whose view is `view`, if the definition has one.
    pub fn screen_of_view(&self, view: &str) -> Option<&ManifestEntry> {
        self.parts
            .iter()
            .find(|entry| entry.kind == SCREEN_PART && entry.view.as_deref() == Some(view))
    }
}

/// The value of an enumeration as the specification writes it.
fn text_of<T: Serialize>(value: &T) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
}

/// The elements of one kind that are on `screen`, by id, as JSON.
fn on_screen(
    schema: &TenantSchema,
    screen: &str,
    pick: fn(&TenantSchema, &UiElement) -> Option<(String, Json)>,
) -> Map<String, Json> {
    strata_kernel::ui::elements(schema)
        .filter_map(|(id, element)| {
            let (on, value) = pick(schema, element)?;
            (on == screen).then(|| (id.clone(), value))
        })
        .collect()
}

/// A field type as a shape writes it: `str`, `int`, `bool`, `time`, `file`, `language`,
/// `ref<object>` and `list<type>`.
fn type_name(ty: &FieldType) -> String {
    match ty {
        FieldType::Str => "str".into(),
        FieldType::Int => "int".into(),
        FieldType::Bool => "bool".into(),
        FieldType::Time => "time".into(),
        FieldType::Ref { target, .. } => format!("ref<{target}>"),
        FieldType::List(inner) => format!("list<{}>", type_name(inner)),
        FieldType::File { .. } => "file".into(),
        FieldType::Language => "language".into(),
    }
}

/// A shape with its fingerprint: the SHA-256 of the canonical JSON of the shape without it.
fn fingerprinted(mut shape: Map<String, Json>) -> Json {
    let fingerprint = hash_of(&Json::Object(shape.clone()));
    shape.insert("fingerprint".into(), json!(fingerprint));
    Json::Object(shape)
}

/// The shape of what a section reads: its object type, its title field and each of its fields by
/// id (type, whether required, and the display type the object type's presentation declares for
/// it, with its props).
fn object_shape(schema: &TenantSchema, object: &strata_kernel::ObjectTypeId) -> Map<String, Json> {
    let presentation = strata_kernel::ui::presentation_of(schema, object);
    let fields: Vec<Json> = schema
        .objects
        .get(object)
        .into_iter()
        .flat_map(|def| def.fields.values())
        .map(|field| {
            let mut entry = Map::new();
            entry.insert("id".into(), json!(field.id.as_str()));
            entry.insert("type".into(), json!(type_name(&field.ty)));
            entry.insert("required".into(), json!(field.required));
            if let Some(shown) = presentation.and_then(|p| p.fields.get(&field.id)) {
                entry.insert("displayType".into(), json!(shown.display));
                if let Some(props) = shown.props.as_ref().filter(|props| !props.is_empty()) {
                    entry.insert("props".into(), json!(props));
                }
            }
            Json::Object(entry)
        })
        .collect();
    let mut shape = Map::new();
    shape.insert("object".into(), json!(object.as_str()));
    let title = schema
        .objects
        .get(object)
        .and_then(|def| crate::project::title_field(def, presentation));
    if let Some(title) = title {
        shape.insert("title".into(), json!(title.as_str()));
    }
    shape.insert("fields".into(), Json::Array(fields));
    shape
}

/// The shape of one row mark or row join of a section (WG-5599): the object type of its query,
/// the aggregate, the reference field the rows are keyed by, and for a join the second field its
/// groups are made by. A mark has no second field.
fn lookup_shape(
    schema: &TenantSchema,
    query: &strata_kernel::QueryId,
    aggregate: &strata_kernel::AggregateId,
) -> Json {
    let object = schema
        .queries
        .get(query)
        .map(|q| q.object.as_str().to_string());
    let agg = strata_kernel::aggregates::section_aggregate(schema, query, aggregate);
    let mut out = Map::new();
    out.insert("object".into(), json!(object));
    out.insert("aggregate".into(), json!(aggregate.as_str()));
    out.insert(
        "key".into(),
        json!(agg.and_then(|a| a.group_by.as_ref()).map(|f| f.as_str())),
    );
    out.insert(
        "second".into(),
        json!(agg
            .and_then(|a| a.group_by_second.as_ref())
            .map(|f| f.as_str())),
    );
    Json::Object(out)
}

/// The shapes of the sections of a view, by section name.
fn section_shapes(schema: &TenantSchema, view: &ViewId) -> Map<String, Json> {
    let Some(def) = schema.views.get(view) else {
        return Map::new();
    };
    def.sections
        .iter()
        .filter_map(|(name, section)| {
            let query = schema.queries.get(&section.query)?;
            let mut shape = object_shape(schema, &query.object);
            if let Some(aggregate) = &section.aggregate {
                shape.insert("aggregate".into(), json!(aggregate.as_str()));
                shape.remove("fields");
            }
            // The row marks and joins of a list section (WG-5599): by name, each with its shape.
            if let Some(marks) = section.marks.as_ref().filter(|m| !m.is_empty()) {
                let by_name: Map<String, Json> = marks
                    .iter()
                    .map(|(mark, m)| (mark.clone(), lookup_shape(schema, &m.query, &m.aggregate)))
                    .collect();
                shape.insert("marks".into(), Json::Object(by_name));
            }
            if let Some(joins) = section.joins.as_ref().filter(|j| !j.is_empty()) {
                let by_name: Map<String, Json> = joins
                    .iter()
                    .map(|(join, j)| (join.clone(), lookup_shape(schema, &j.query, &j.aggregate)))
                    .collect();
                shape.insert("joins".into(), Json::Object(by_name));
            }
            Some((
                name.clone(),
                json!({ "single": section.single, "shape": fingerprinted(shape) }),
            ))
        })
        .collect()
}

/// The shape of an operation a form or action invokes: its id and each input's name, type and
/// whether it is required.
fn operation_shape(schema: &TenantSchema, operation: &str) -> Json {
    let mut shape = Map::new();
    shape.insert("operation".into(), json!(operation));
    let def = schema
        .operations
        .iter()
        .flatten()
        .find(|(id, _)| id.0 == operation)
        .map(|(_, def)| def);
    if let Some(def) = def {
        let inputs: Vec<Json> = def
            .inputs
            .iter()
            .map(|input| {
                json!({
                    "name": input.name,
                    "type": type_name(&input.ty),
                    "required": input.required,
                })
            })
            .collect();
        shape.insert("inputs".into(), Json::Array(inputs));
    }
    fingerprinted(shape)
}

/// The element's JSON with its shape added.
fn with_shape(element: Json, shape: Json) -> Json {
    match element {
        Json::Object(mut map) => {
            map.insert("shape".into(), shape);
            Json::Object(map)
        }
        other => other,
    }
}

fn form_of(schema: &TenantSchema, element: &UiElement) -> Option<(String, Json)> {
    match element {
        UiElement::Form(form) => Some((
            form.screen.clone(),
            with_shape(json!(form), operation_shape(schema, &form.operation.0)),
        )),
        _ => None,
    }
}

fn action_of(schema: &TenantSchema, element: &UiElement) -> Option<(String, Json)> {
    match element {
        UiElement::Action(action) => Some((
            action.screen.clone(),
            with_shape(json!(action), operation_shape(schema, &action.operation.0)),
        )),
        _ => None,
    }
}

fn live_of(_: &TenantSchema, element: &UiElement) -> Option<(String, Json)> {
    match element {
        UiElement::LiveBinding(live) => Some((live.screen.clone(), json!(live))),
        _ => None,
    }
}

/// The definition of a tenant, from its schema.
pub fn definition(schema: &TenantSchema) -> Definition {
    let mut parts = BTreeMap::new();
    let mut entries = Vec::new();
    for (id, element) in strata_kernel::ui::elements(schema) {
        let UiElement::Screen(screen) = element else {
            continue;
        };
        let part = json!({
            "kind": SCREEN_PART,
            "id": id,
            "screen": screen,
            "sections": section_shapes(schema, &screen.view),
            "forms": on_screen(schema, id, form_of),
            "actions": on_screen(schema, id, action_of),
            "live": on_screen(schema, id, live_of),
        });
        let hash = hash_of(&part);
        entries.push(ManifestEntry {
            kind: SCREEN_PART.to_string(),
            id: id.clone(),
            hash: hash.clone(),
            route: Some(screen.route.clone()),
            view: Some(screen.view.to_string()),
            title: Some(screen.title.clone()),
            layout: text_of(&screen.kind),
            auth: text_of(&screen.auth),
            query: Some(screen.query.clone()),
        });
        parts.insert(hash, part);
    }
    let messages: Map<String, Json> = strata_kernel::ui::elements(schema)
        .filter_map(|(id, element)| match element {
            UiElement::ErrorMessage(message) => Some((id.clone(), json!(message))),
            _ => None,
        })
        .collect();
    if !messages.is_empty() {
        let part = json!({ "kind": ERRORS_PART, "id": ERRORS_ID, "messages": messages });
        let hash = hash_of(&part);
        entries.push(ManifestEntry {
            kind: ERRORS_PART.to_string(),
            id: ERRORS_ID.to_string(),
            hash: hash.clone(),
            route: None,
            view: None,
            title: None,
            layout: None,
            auth: None,
            query: None,
        });
        parts.insert(hash, part);
    }
    Definition {
        manifest: Manifest {
            hash: hash_of(&entries),
            schema_version: schema.version.0,
            parts: entries,
        },
        parts,
    }
}

/// A page title as a screen states it: the text, with each `${a.b.c}` replaced by the text or
/// number at that dotted path of the view model (nothing when there is none). An unclosed `${` is
/// kept as text.
pub fn title_text(template: &str, model: &Json) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        let Some(len) = rest[start + 2..].find('}') else {
            break;
        };
        out.push_str(&rest[..start]);
        let path = &rest[start + 2..start + 2 + len];
        match path.split('.').fold(model, |at, key| &at[key]) {
            Json::String(text) => out.push_str(text),
            Json::Number(number) => out.push_str(&number.to_string()),
            _ => {}
        }
        rest = &rest[start + 3 + len..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_reads_its_paths_from_the_model() {
        let model = json!({ "data": { "ticket": { "fields": { "title": "Leak", "n": 3 } } } });
        assert_eq!(
            title_text(
                "Ticket ${data.ticket.fields.title} (${data.ticket.fields.n})",
                &model
            ),
            "Ticket Leak (3)"
        );
        assert_eq!(title_text("Tickets", &model), "Tickets");
        assert_eq!(title_text("${data.none}", &model), "");
        assert_eq!(title_text("open ${", &model), "open ${");
    }
}
