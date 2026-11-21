//! The interface sections of a product file turned into the `ui` section of its bundle (D-8250,
//! INT-3250, INT-3251).
//!
//! A product file declares its interface with the entities of the specification format: `screens`,
//! `forms`, `actions`, `liveBindings` and `errorMessages`. In a tenant there is no contract code
//! behind them: a product's contract is realised by its view (a read) or its composite operation (a
//! write). So [`ui_section`] writes each entity as a typed interface element (`ui-element`) bound
//! to the product's own entities, and refuses what it cannot bind:
//!
//! - a screen shows the view of the product whose route is the screen's route (its `viewModel`
//!   names the contract that documents the read, or that view); its `kind` is `<renderer>.<layout>`
//!   and the layout is one the interpreter of the web application draws (list, detail, editor,
//!   settings); `auth` is `none` (written `anyone`) or `session`; its title is its `title`, else its
//!   `name`;
//! - a form and an action submit to the operation of the product their `contract` names, and fill
//!   only inputs of that operation; an action reads the form it names on the same screen, through
//!   arguments `form.<field>` or `route.<param>`; a navigation after success names a screen of the
//!   product;
//! - a live binding listens to `ui-notice-received` (with no `when`, every notice kind; a `when`
//!   of `event.kind == '<kind>'`, `{ "value": "event.object", "in": "depends-on" }` and `any` of
//!   those) or to `ui-session-ended`, and refreshes the whole view;
//! - an error message is copied, and what it applies to must be a form or an action of the product.
//!
//! Refused, each naming the element and saying what to do:
//!
//! - `bundle.ui-unresolved`: a screen whose route answers no view of the product, a form or action
//!   whose contract names no operation of the product, an input the operation does not take, or a
//!   reference to a screen, form or action the product does not declare;
//! - `bundle.ui-unsupported`: a feature of the format the interpreter does not draw yet (a derived
//!   or initial field value, choices read from the view model, a per-row action, an action also
//!   offered on other screens, an optimistic action, an action whose result is kept on the screen,
//!   a partial refresh, a role-restricted screen, a `when` other than the forms above). The
//!   extension point for each is the interpreter (`web/src/kernel/tenant-ui.js`) and the type of
//!   its element in the platform specification.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::diagnostic::Diagnostic;

/// A reference of an interface element that the product does not declare.
pub const UI_UNRESOLVED: &str = "bundle.ui-unresolved";
/// A feature of an interface entity that the interpreter of the web application does not draw.
pub const UI_UNSUPPORTED: &str = "bundle.ui-unsupported";

/// The sections of a product file that hold interface entities, in the order they are read.
pub const UI_SECTIONS: &[&str] = &[
    "screens",
    "forms",
    "actions",
    "liveBindings",
    "errorMessages",
];

/// The layouts the interpreter draws (`ui-screen-kind`).
const LAYOUTS: &[&str] = &["list", "detail", "editor", "settings"];
/// The controls the interpreter draws (`ui-control`).
const CONTROLS: &[&str] = &[
    "text", "textarea", "password", "checkbox", "select", "radio", "date", "number",
];
/// The notice kinds of the live protocol.
const NOTICE_KINDS: &[&str] = &["upsert", "remove", "schema", "membership"];

/// The items of a section of the product file, by id, in the order of the file.
fn items<'a>(top: &'a Map<String, Value>, section: &str) -> Vec<(&'a str, &'a Map<String, Value>)> {
    top.get(section)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| Some((item.get("id")?.as_str()?, item.as_object()?)))
        .collect()
}

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

/// What the product declares that an interface element may name.
struct Product<'a> {
    /// The route of each view, by id.
    views: BTreeMap<&'a str, &'a str>,
    /// The inputs of each operation, by id.
    operations: BTreeMap<&'a str, BTreeSet<&'a str>>,
    screens: BTreeSet<&'a str>,
    /// The screen of each form, by id.
    forms: BTreeMap<&'a str, &'a str>,
    actions: BTreeSet<&'a str>,
}

impl<'a> Product<'a> {
    fn of(top: &'a Map<String, Value>) -> Self {
        let views = items(top, "views")
            .into_iter()
            .filter_map(|(id, view)| Some((id, text(view, "route")?)))
            .collect();
        let operations = items(top, "operations")
            .into_iter()
            .map(|(id, op)| {
                let inputs = op
                    .get("inputs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|input| input.get("name").and_then(Value::as_str))
                    .collect();
                (id, inputs)
            })
            .collect();
        Product {
            views,
            operations,
            screens: items(top, "screens")
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
            forms: items(top, "forms")
                .into_iter()
                .map(|(id, form)| (id, text(form, "screen").unwrap_or("")))
                .collect(),
            actions: items(top, "actions")
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
        }
    }
}

/// Collects the diagnostics of one product's interface.
struct Check<'a> {
    product: &'a str,
    out: &'a mut Vec<Diagnostic>,
}

impl Check<'_> {
    fn unresolved(&mut self, entity: &str, message: String) {
        self.out.push(Diagnostic::new(
            UI_UNRESOLVED,
            entity,
            format!(
                "\"{entity}\" of the product \"{}\": {message}",
                self.product
            ),
        ));
    }

    fn unsupported(&mut self, entity: &str, feature: &str) {
        self.out.push(Diagnostic::new(
            UI_UNSUPPORTED,
            entity,
            format!(
                "\"{entity}\" of the product \"{}\" uses {feature}, which the interpreter of the web application does not draw yet (D-8250). Remove it from the element, or add it to the interpreter and to the element's type in the platform specification first.",
                self.product
            ),
        ));
    }

    /// Refuses each key of `map` that names a feature the interpreter does not draw.
    fn features(&mut self, entity: &str, map: &Map<String, Value>, unsupported: &[(&str, &str)]) {
        for (key, feature) in unsupported {
            if map.contains_key(*key) {
                self.unsupported(entity, feature);
            }
        }
    }

    /// The outcome of a success: a message and a navigation to a screen of the product.
    fn outcome(&mut self, entity: &str, product: &Product<'_>, on: Option<&Value>) -> Value {
        let mut outcome = Map::new();
        let Some(on) = on.and_then(Value::as_object) else {
            return Value::Object(outcome);
        };
        if let Some(toast) = text(on, "toast") {
            outcome.insert("toast".to_string(), json!(toast));
        }
        if let Some(navigate) = on.get("navigate").and_then(Value::as_object) {
            if navigate.contains_key("params") || navigate.contains_key("path") {
                self.unsupported(entity, "a navigation with parameters after success");
            }
            match text(navigate, "screen") {
                Some(screen) if product.screens.contains(screen) => {
                    outcome.insert("navigate".to_string(), json!(screen));
                }
                Some(screen) => self.unresolved(
                    entity,
                    format!("navigates to \"{screen}\", which is not a screen of the product; name one of its screens."),
                ),
                None => {}
            }
        }
        Value::Object(outcome)
    }

    /// Where a refusal is shown.
    fn display(&mut self, on: Option<&Value>) -> Value {
        match on.and_then(|on| on.get("show")).and_then(Value::as_str) {
            Some("toast") => json!("toast"),
            _ => json!("inline"),
        }
    }

    /// The operation a form or an action submits to, and its inputs.
    fn operation(
        &mut self,
        entity: &str,
        product: &Product<'_>,
        map: &Map<String, Value>,
    ) -> Option<(String, BTreeSet<String>)> {
        let contract = text(map, "contract").unwrap_or("");
        match product.operations.get_key_value(contract) {
            Some((id, inputs)) => Some((
                (*id).to_string(),
                inputs.iter().map(|i| (*i).to_string()).collect(),
            )),
            None => {
                self.unresolved(
                    entity,
                    format!("submits to \"{contract}\", which is not an operation of the product. In a tenant a product's write is its composite operation: declare it in \"operations\" under that id, with the inputs the element fills."),
                );
                None
            }
        }
    }

    fn screen_ref(
        &mut self,
        entity: &str,
        product: &Product<'_>,
        map: &Map<String, Value>,
    ) -> String {
        let screen = text(map, "screen").unwrap_or("");
        if !product.screens.contains(screen) {
            self.unresolved(
                entity,
                format!("is on \"{screen}\", which is not a screen of the product."),
            );
        }
        screen.to_string()
    }

    fn screen(&mut self, id: &str, map: &Map<String, Value>, product: &Product<'_>) -> Value {
        let route = text(map, "route").unwrap_or("");
        let view = product
            .views
            .iter()
            .find(|(_, view_route)| **view_route == route)
            .map(|(view, _)| *view);
        if view.is_none() {
            self.unresolved(
                id,
                format!("answers the route \"{route}\", which no view of the product answers. A tenant screen shows the view of its route: declare a view with \"route\": \"{route}\" in \"views\"."),
            );
        }
        let layout = text(map, "kind")
            .map(|kind| kind.rsplit('.').next().unwrap_or(kind))
            .unwrap_or("list");
        if !LAYOUTS.contains(&layout) {
            self.unsupported(
                id,
                &format!(
                    "the layout \"{layout}\" (the interpreter draws {})",
                    LAYOUTS.join(", ")
                ),
            );
        }
        let auth = match text(map, "auth").unwrap_or("none") {
            "none" => "anyone",
            "session" => "session",
            other => {
                self.unsupported(id, &format!("the access \"{other}\" (a screen is open to anyone or to a signed-in person)"));
                "session"
            }
        };
        self.features(
            id,
            map,
            &[
                ("local", "screen-local values"),
                ("onEnter", "sections loaded on entry"),
                ("viewOutputs", "derived view outputs"),
            ],
        );
        let title = text(map, "title")
            .or_else(|| text(map, "name"))
            .unwrap_or("");
        let query = map.get("query").cloned().unwrap_or_else(|| json!([]));
        json!({ "Screen": {
            "route": route,
            "view": view.unwrap_or(""),
            "title": title,
            "kind": layout,
            "auth": auth,
            "query": query,
        } })
    }

    fn form(&mut self, id: &str, map: &Map<String, Value>, product: &Product<'_>) -> Value {
        let screen = self.screen_ref(id, product, map);
        let operation = self.operation(id, product, map);
        let mut fields = Vec::new();
        for field in map
            .get("fields")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
        {
            let name = text(field, "name").unwrap_or("");
            let at = format!("{id}.{name}");
            let input = text(field, "input").unwrap_or(name);
            if let Some((op, inputs)) = &operation {
                if !inputs.contains(input) {
                    self.unresolved(
                        &at,
                        format!("fills the input \"{input}\", which the operation \"{op}\" does not take; add it to the operation's inputs or remove the field."),
                    );
                }
            }
            self.features(
                &at,
                field,
                &[
                    ("derive", "a derived value"),
                    ("initial", "an initial value read from the view model"),
                    ("optionsFrom", "choices read from the view model"),
                ],
            );
            let control = text(field, "control").unwrap_or("text");
            if !CONTROLS.contains(&control) {
                self.unsupported(&at, &format!("the control \"{control}\""));
            }
            let mut out = Map::new();
            out.insert("name".to_string(), json!(name));
            out.insert("input".to_string(), json!(input));
            out.insert(
                "label".to_string(),
                json!(text(field, "label").unwrap_or(name)),
            );
            out.insert("control".to_string(), json!(control));
            if let Some(options) = field.get("options").and_then(Value::as_array) {
                let options: Vec<Value> = options
                    .iter()
                    .filter_map(Value::as_object)
                    .map(|o| json!({ "value": text(o, "value").unwrap_or(""), "label": text(o, "label").unwrap_or("") }))
                    .collect();
                out.insert("options".to_string(), json!(options));
            }
            match text(field, "normalize") {
                Some("trim") => {
                    out.insert("trim".to_string(), json!(true));
                }
                Some(other) => self.unsupported(&at, &format!("the normalisation \"{other}\"")),
                None => {}
            }
            fields.push(Value::Object(out));
        }
        let on_success = self.outcome(id, product, map.get("onSuccess"));
        let on_error = self.display(map.get("onError"));
        json!({ "Form": {
            "screen": screen,
            "operation": operation.map(|(op, _)| op).unwrap_or_default(),
            "fields": fields,
            "submit-label": text(map, "submitLabel").unwrap_or("Submit"),
            "on-success": on_success,
            "on-error": on_error,
        } })
    }

    fn action(&mut self, id: &str, map: &Map<String, Value>, product: &Product<'_>) -> Value {
        let screen = self.screen_ref(id, product, map);
        let operation = self.operation(id, product, map);
        self.features(
            id,
            map,
            &[
                ("forEach", "a per-row action"),
                ("alsoOn", "an action offered on other screens"),
                ("optimistic", "an optimistic action"),
                ("result", "a result kept on the screen"),
                ("confirmAnnounce", "an announcement of a confirmation"),
            ],
        );
        let form = text(map, "form");
        if let Some(form) = form {
            match product.forms.get(form) {
                Some(on) if *on == screen => {}
                Some(_) => self.unresolved(
                    id,
                    format!("reads the form \"{form}\", which is on another screen."),
                ),
                None => self.unresolved(
                    id,
                    format!("reads the form \"{form}\", which is not a form of the product."),
                ),
            }
        }
        let arguments = map
            .get("arguments")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if let Some((op, inputs)) = &operation {
            for input in arguments.keys() {
                if !inputs.contains(input) {
                    self.unresolved(
                        id,
                        format!("fills the input \"{input}\", which the operation \"{op}\" does not take."),
                    );
                }
            }
        }
        for (input, source) in &arguments {
            let source = source.as_str().unwrap_or("");
            let readable =
                source.starts_with("form.") && form.is_some() || source.starts_with("route.");
            if !readable {
                self.unsupported(id, &format!("the argument \"{input}\" from \"{source}\" (an argument is form.<field> of the form the action reads, or route.<param>)"));
            }
        }
        let mut action = Map::new();
        action.insert("screen".to_string(), json!(screen));
        action.insert("label".to_string(), json!(text(map, "label").unwrap_or(id)));
        action.insert(
            "operation".to_string(),
            json!(operation.map(|(op, _)| op).unwrap_or_default()),
        );
        if let Some(form) = form {
            action.insert("form".to_string(), json!(form));
        }
        action.insert("arguments".to_string(), Value::Object(arguments));
        if let Some(confirm) = text(map, "confirm") {
            action.insert("confirm".to_string(), json!(confirm));
        }
        action.insert(
            "on-success".to_string(),
            self.outcome(id, product, map.get("onSuccess")),
        );
        action.insert("on-error".to_string(), self.display(map.get("onError")));
        json!({ "Action": action })
    }

    /// The notice kinds and the object condition of a `when`, or `None` when it is not one of the
    /// forms the interpreter evaluates.
    fn when(condition: &Value, kinds: &mut BTreeSet<String>, objects: &mut bool) -> bool {
        match condition {
            Value::String(text) => {
                let Some(kind) = text
                    .trim()
                    .strip_prefix("event.kind == '")
                    .and_then(|rest| rest.strip_suffix('\''))
                else {
                    return false;
                };
                kinds.insert(kind.to_string());
                NOTICE_KINDS.contains(&kind)
            }
            Value::Object(map) if map.len() == 1 && map.contains_key("any") => map["any"]
                .as_array()
                .is_some_and(|all| all.iter().all(|c| Self::when(c, kinds, objects))),
            Value::Object(map)
                if map.get("value").and_then(Value::as_str) == Some("event.object")
                    && map.get("in").and_then(Value::as_str) == Some("depends-on")
                    && map.len() == 2 =>
            {
                *objects = true;
                true
            }
            _ => false,
        }
    }

    fn live(&mut self, id: &str, map: &Map<String, Value>, product: &Product<'_>) -> Value {
        let screen = self.screen_ref(id, product, map);
        if text(map, "refresh").unwrap_or("whole") != "whole" {
            self.unsupported(id, "a partial refresh");
        }
        let mut kinds = BTreeSet::new();
        let mut objects = false;
        let event = match text(map, "event").unwrap_or("") {
            "ui-notice-received" => {
                match map.get("when") {
                    None => {
                        kinds.extend(NOTICE_KINDS.iter().map(|k| (*k).to_string()));
                        objects = true;
                    }
                    Some(condition) => {
                        if !Self::when(condition, &mut kinds, &mut objects) {
                            self.unsupported(id, "a condition other than event.kind == '<kind>', the object of the event in depends-on, and any of those");
                        }
                    }
                }
                "notice"
            }
            "ui-session-ended" => "session-ended",
            other => {
                self.unsupported(id, &format!("the event \"{other}\" (a tenant screen listens to ui-notice-received and ui-session-ended)"));
                "notice"
            }
        };
        json!({ "LiveBinding": {
            "screen": screen,
            "event": event,
            "kinds": kinds.into_iter().collect::<Vec<_>>(),
            "objects": objects,
        } })
    }

    fn message(&mut self, id: &str, map: &Map<String, Value>, product: &Product<'_>) -> Value {
        let mut message = Map::new();
        message.insert("on".to_string(), json!(text(map, "on").unwrap_or("check")));
        message.insert(
            "kinds".to_string(),
            map.get("kinds").cloned().unwrap_or_else(|| json!([])),
        );
        message.insert("text".to_string(), json!(text(map, "text").unwrap_or("")));
        if let Some(fallback) = text(map, "fallback") {
            message.insert("fallback".to_string(), json!(fallback));
        }
        if let Some(field) = text(map, "field") {
            message.insert("field".to_string(), json!(field));
        }
        let applies: Vec<&str> = map
            .get("appliesTo")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        for owner in &applies {
            if !product.forms.contains_key(owner) && !product.actions.contains(owner) {
                self.unresolved(
                    id,
                    format!(
                        "applies to \"{owner}\", which is not a form or an action of the product."
                    ),
                );
            }
        }
        message.insert("applies-to".to_string(), json!(applies));
        json!({ "ErrorMessage": message })
    }
}

/// The interface elements of a product file, by id, as values of `ui-element`; the diagnostics of
/// anything that cannot be bound go to `out`. `top` is the product file as a JSON object.
pub fn ui_section(
    product: &str,
    top: &Map<String, Value>,
    out: &mut Vec<Diagnostic>,
) -> Vec<(String, Value)> {
    let declared = Product::of(top);
    let mut check = Check { product, out };
    let mut elements = Vec::new();
    for section in UI_SECTIONS {
        for (id, map) in items(top, section) {
            let element = match *section {
                "screens" => check.screen(id, map, &declared),
                "forms" => check.form(id, map, &declared),
                "actions" => check.action(id, map, &declared),
                "liveBindings" => check.live(id, map, &declared),
                _ => check.message(id, map, &declared),
            };
            elements.push((id.to_string(), element));
        }
    }
    elements
}
