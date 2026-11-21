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
//! the schema (a record type, a policy) changes neither: a client compares one hash to know whether
//! what it holds is current (WG-5332), and a part it holds stays valid for as long as its hash is
//! listed (WG-5307). The schema version the manifest was computed at is carried beside the hash,
//! not inside it.
//!
//! Pure: the same schema gives the same definition, byte for byte.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as Json};
use std::collections::BTreeMap;
use strata_kernel::canon::hash_of;
use strata_kernel::{TenantSchema, UiElement};

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
    pick: fn(&UiElement) -> Option<(&str, Json)>,
) -> Map<String, Json> {
    strata_kernel::ui::elements(schema)
        .filter_map(|(id, element)| {
            let (on, value) = pick(element)?;
            (on == screen).then(|| (id.clone(), value))
        })
        .collect()
}

fn form_of(element: &UiElement) -> Option<(&str, Json)> {
    match element {
        UiElement::Form(form) => Some((form.screen.as_str(), json!(form))),
        _ => None,
    }
}

fn action_of(element: &UiElement) -> Option<(&str, Json)> {
    match element {
        UiElement::Action(action) => Some((action.screen.as_str(), json!(action))),
        _ => None,
    }
}

fn live_of(element: &UiElement) -> Option<(&str, Json)> {
    match element {
        UiElement::LiveBinding(live) => Some((live.screen.as_str(), json!(live))),
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
