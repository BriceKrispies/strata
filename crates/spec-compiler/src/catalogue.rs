//! The capability catalogue: `specc catalogue` writes one compact Markdown file that lists what a
//! product feature can build on, so that its author reads that file instead of the whole
//! specification.
//!
//! The file holds, each section sorted by id and each entry on one line:
//!
//! - every contract whose `exposure` is `public`: inputs, outputs, error names, the capabilities
//!   its `auth.requires` lists, the auth scheme, whether it is idempotent, and the component that
//!   implements it with that component's `implementation.status` (`not-implemented` is written
//!   `NOT IMPLEMENTED`, so a contract nothing serves yet is visible at a glance);
//! - the domain events, the events the specification marks `domain: true` (the events of a
//!   tenant's event log, which a product module can subscribe to), with their payload fields;
//! - every capability with the roles that hold it, every quota and every plan;
//! - the named types the listed contracts and domain events use, each with a one-line shape.
//!
//! The specification does not declare workflow step kinds or trigger kinds as data, so the file
//! has no section for them.
//!
//! The text depends only on the specification and the path it is named by; every list is sorted
//! by id with a plain string comparison, and no map is iterated in its own order.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde_json::{Map, Value};

use crate::codetext::line_text;
use crate::diagnostic::{Diagnostic, SPEC_ENTITY};
use crate::emit::{GeneratedFile, HEADER_MARKERS};
use crate::jsonc;

/// Code of a text that is not JSONC.
pub const SYNTAX: &str = "catalogue.spec-syntax";
/// Code of a specification that is not a JSON object.
pub const SHAPE: &str = "catalogue.spec-shape";

/// The path of the generated file, relative to the output directory.
pub const PATH: &str = "catalogue.md";

/// The generated files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueOutput {
    /// The files, sorted by path (one).
    pub files: Vec<GeneratedFile>,
}

/// Generates the catalogue from a specification's text, or refuses with every diagnostic.
/// `spec_path` is the path written into the header.
pub fn generate(spec_path: &str, text: &str) -> Result<CatalogueOutput, Vec<Diagnostic>> {
    let spec_path = spec_path.replace('\\', "/");
    let value = jsonc::parse(text).map_err(|e| {
        vec![Diagnostic::new(
            SYNTAX,
            SPEC_ENTITY,
            format!("the specification is not valid JSONC: {e}"),
        )]
    })?;
    let Some(top) = value.as_object() else {
        return Err(vec![Diagnostic::new(
            SHAPE,
            SPEC_ENTITY,
            "the specification is not a JSON object",
        )]);
    };
    let body = render(top);
    Ok(CatalogueOutput {
        files: vec![GeneratedFile {
            path: PATH.to_string(),
            contents: format!(
                "{} from {}. Do not edit; change the specification and regenerate.\n\n{body}",
                HEADER_MARKERS[1],
                line_text(&spec_path)
            ),
        }],
    })
}

// ---- reading ------------------------------------------------------------------------------

/// The array under `key`, or nothing.
fn items<'a>(top: &'a Map<String, Value>, key: &str) -> &'a [Value] {
    match top.get(key) {
        Some(Value::Array(list)) => list.as_slice(),
        _ => &[],
    }
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// The strings of the array under `key` (other items are skipped).
fn texts<'a>(v: &'a Value, key: &str) -> Vec<&'a str> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// A string or a number as text; anything else as `?`.
fn scalar(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => line_text(s),
        Some(Value::Number(n)) => n.to_string(),
        _ => "?".to_string(),
    }
}

/// The entries of `items` sorted by their `id` (a plain string comparison).
fn by_id(items: &[Value]) -> Vec<&Value> {
    let mut out: Vec<&Value> = items.iter().collect();
    out.sort_by(|a, b| text(a, "id").cmp(&text(b, "id")));
    out
}

/// `name:type, other?:type` for the `{name, type, optional}` entries under `key`.
fn fields(v: &Value, key: &str) -> String {
    let Some(list) = v.get(key).and_then(Value::as_array) else {
        return String::new();
    };
    list.iter()
        .map(|f| {
            let optional = f.get("optional").and_then(Value::as_bool) == Some(true);
            format!(
                "{}{}:{}",
                line_text(text(f, "name").unwrap_or("?")),
                if optional { "?" } else { "" },
                line_text(text(f, "type").unwrap_or("?"))
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The text of a list, or `-` when it is empty.
fn or_dash(s: String) -> String {
    if s.is_empty() {
        "-".to_string()
    } else {
        s
    }
}

/// Adds the declared type ids named inside a type expression (`list<grant-id>`,
/// `map<field-id, expr>`, ...) to `used`.
fn note_types(expr: &str, declared: &BTreeSet<&str>, used: &mut BTreeSet<String>) {
    for token in expr.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
        if declared.contains(token) {
            used.insert(token.to_string());
        }
    }
}

/// The type ids of the `{name, type}` entries under `key`.
fn note_field_types(v: &Value, key: &str, declared: &BTreeSet<&str>, used: &mut BTreeSet<String>) {
    if let Some(list) = v.get(key).and_then(Value::as_array) {
        for f in list {
            if let Some(t) = text(f, "type") {
                note_types(t, declared, used);
            }
        }
    }
}

/// A one-line shape of a declared type.
fn shape(t: &Value) -> String {
    match text(t, "kind").unwrap_or("?") {
        k @ ("id" | "scalar") => format!("{k} {}", line_text(text(t, "repr").unwrap_or("?"))),
        "enum" => texts(t, "values")
            .into_iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("|"),
        "struct" => format!("{{{}}}", fields(t, "fields")),
        "union" => t
            .get("variants")
            .and_then(Value::as_array)
            .map(|variants| variants.iter().map(variant).collect::<Vec<_>>().join("|"))
            .unwrap_or_default(),
        other => line_text(other),
    }
}

/// One variant of a union: `name`, `name(type)` for a newtype or `name{fields}`.
fn variant(v: &Value) -> String {
    let name = line_text(text(v, "name").unwrap_or("?"));
    let has_fields = v
        .get("fields")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());
    if !has_fields {
        return name;
    }
    if v.get("newtype").and_then(Value::as_bool) == Some(true) {
        let ty = v
            .get("fields")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|f| text(f, "type"))
            .unwrap_or("?");
        return format!("{name}({})", line_text(ty));
    }
    format!("{name}{{{}}}", fields(v, "fields"))
}

// ---- rendering ----------------------------------------------------------------------------

fn render(top: &Map<String, Value>) -> String {
    let declared: BTreeSet<&str> = items(top, "types")
        .iter()
        .filter_map(|t| text(t, "id"))
        .collect();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut out = String::new();

    let statuses: BTreeMap<&str, &str> = items(top, "components")
        .iter()
        .filter_map(|c| {
            let id = text(c, "id")?;
            let status = c.get("implementation").and_then(|i| text(i, "status"))?;
            Some((id, status))
        })
        .collect();

    let _ = writeln!(out, "# Capability catalogue\n");
    let _ = writeln!(
        out,
        "What a product feature can build on: the public contracts, the domain events, the capabilities, the quotas, the plans and the types they use. After `by`, the status is the implementing component's: NOT IMPLEMENTED means no running code serves the contract yet.\n"
    );

    // Public contracts.
    let public: Vec<&Value> = by_id(items(top, "contracts"))
        .into_iter()
        .filter(|c| text(c, "exposure") == Some("public"))
        .collect();
    let _ = writeln!(out, "## Public contracts ({})\n", public.len());
    let _ = writeln!(
        out,
        "id: name | in name:type (? optional) | out | err | needs capabilities | auth scheme | idem | by component (status)\n"
    );
    for c in &public {
        note_field_types(c, "inputs", &declared, &mut used);
        note_field_types(c, "outputs", &declared, &mut used);
        let errors: Vec<String> = c
            .get("errors")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|e| line_text(text(e, "name").unwrap_or("?")))
                    .collect()
            })
            .unwrap_or_default();
        let auth = c.get("auth");
        let needs: Vec<String> = auth
            .map(|a| texts(a, "requires").into_iter().map(line_text).collect())
            .unwrap_or_default();
        let scheme = auth
            .and_then(|a| text(a, "scheme"))
            .map_or_else(|| "-".to_string(), line_text);
        let idempotent = c.get("idempotent").and_then(Value::as_bool) == Some(true);
        let by = match text(c, "implementedBy") {
            None => "none".to_string(),
            Some(component) => {
                let status = match statuses.get(component) {
                    Some(&"not-implemented") => "NOT IMPLEMENTED".to_string(),
                    Some(status) => line_text(status),
                    None => "unknown".to_string(),
                };
                format!("{} ({status})", line_text(component))
            }
        };
        let _ = writeln!(
            out,
            "- {}: {} | in {} | out {} | err {} | needs {} | auth {} | idem {} | by {}",
            line_text(text(c, "id").unwrap_or("?")),
            line_text(text(c, "name").unwrap_or("?")),
            or_dash(fields(c, "inputs")),
            or_dash(fields(c, "outputs")),
            or_dash(errors.join(",")),
            or_dash(needs.join(",")),
            scheme,
            if idempotent { "yes" } else { "no" },
            by
        );
    }

    // Domain events.
    let _ = writeln!(out, "\n## Domain events\n");
    let _ = writeln!(
        out,
        "The events of a tenant's event log (`domain: true`); a product module subscribes to these. Machine events are left out.\n"
    );
    for e in by_id(items(top, "events"))
        .into_iter()
        .filter(|e| e.get("domain").and_then(Value::as_bool) == Some(true))
    {
        note_field_types(e, "payload", &declared, &mut used);
        let _ = writeln!(
            out,
            "- {}: {}",
            line_text(text(e, "id").unwrap_or("?")),
            or_dash(fields(e, "payload"))
        );
    }

    // Capabilities and the roles that hold them.
    let mut holders: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for role in items(top, "roles") {
        let Some(role_id) = text(role, "id") else {
            continue;
        };
        for capability in texts(role, "capabilities") {
            holders.entry(capability).or_default().push(role_id);
        }
    }
    let _ = writeln!(out, "\n## Capabilities\n");
    let _ = writeln!(out, "id [core]: description | roles that hold it\n");
    for c in by_id(items(top, "capabilities")) {
        let id = text(c, "id").unwrap_or("?");
        let mut roles = holders.get(id).cloned().unwrap_or_default();
        roles.sort_unstable();
        roles.dedup();
        let core = c.get("core").and_then(Value::as_bool) == Some(true);
        let _ = writeln!(
            out,
            "- {}{}: {} | {}",
            line_text(id),
            if core { " [core]" } else { "" },
            line_text(text(c, "description").unwrap_or("")),
            or_dash(
                roles
                    .into_iter()
                    .map(line_text)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        );
    }

    // Quotas.
    let _ = writeln!(out, "\n## Quotas\n");
    let _ = writeln!(out, "id: limit per window, on exceed\n");
    for q in by_id(items(top, "quotas")) {
        let _ = writeln!(
            out,
            "- {}: {} per {}, {}",
            line_text(text(q, "id").unwrap_or("?")),
            scalar(q.get("limit")),
            scalar(q.get("window")),
            scalar(q.get("onExceed"))
        );
    }

    // Plans.
    let _ = writeln!(out, "\n## Plans\n");
    for p in by_id(items(top, "plans")) {
        let adds = texts(p, "capabilities");
        let _ = writeln!(
            out,
            "- {}: {}{}",
            line_text(text(p, "id").unwrap_or("?")),
            line_text(text(p, "description").unwrap_or("")),
            if adds.is_empty() {
                String::new()
            } else {
                format!(
                    " | adds {}",
                    adds.into_iter()
                        .map(line_text)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        );
    }

    // The types the contracts and events above name.
    let _ = writeln!(out, "\n## Types\n");
    for t in by_id(items(top, "types")) {
        let id = text(t, "id").unwrap_or("?");
        if used.contains(id) {
            let _ = writeln!(out, "- {}: {}", line_text(id), shape(t));
        }
    }
    out
}
