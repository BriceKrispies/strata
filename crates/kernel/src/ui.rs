//! The interface definition of a tenant: the screens, forms, actions, live bindings and error
//! messages its products installed, held as tenant metadata in the tenant schema (D-8250,
//! INT-3250, INT-3252).
//!
//! The schema holds them in the optional member `ui`, a map from the id of an element (in the
//! namespace of the product that installed it) to a [`UiElement`]; the member is left out of the
//! encoding while it holds nothing, so a schema that never had one hashes as it did before. They
//! change through two schema changes, [`SchemaChange::DefineUiElement`] (define-or-replace) and
//! [`SchemaChange::RemoveUiElement`] (removing what does not exist is refused), under the schema
//! version like the rest of the schema, and a product owns each as an entity of kind `ui-element`.
//!
//! The kernel holds and checks them; it does not draw them. The checks are made on the schema that
//! holds the element ([`problems`]), so an element that names something that does not exist is
//! refused when it is defined, and a change that removes something an element names (a view, an
//! operation, another element) is refused while the element names it:
//!
//! - an id is lower-case letters, digits and underscores, beginning with a letter, at most 64 bytes;
//! - a screen answers a route (`/` and segments, each a literal or `:param`) that no other screen
//!   answers, and that is the route of the view it shows; its title is one line;
//! - a form and an action are on a screen, invoke an operation of the tenant, and fill only inputs
//!   of that operation; a form's fields have distinct names, and a select or radio has choices; an
//!   action's arguments read `form.<field>` of the form it names (on the same screen) or
//!   `route.<param>` of its screen; a navigation after success names a screen;
//! - a live binding is on a screen, and listens to notice kinds the protocol has (`upsert`,
//!   `remove`, `schema`, `membership`); one on the end of a session names none;
//! - an error message has text, and the forms and actions it applies to exist;
//! - a presentation describes an object type of the tenant (`ui.binding.unknown-object`), names
//!   only fields it has (`ui.binding.unknown-field`; `created_by`, `created_at` and `updated_at`
//!   are the system attributes a presentation may also name), draws each with a display type that
//!   applies to the field's type (`ui.binding.display-type`), and takes its title from a string
//!   field (`ui.binding.title`); one object type has one presentation (`ui.binding.duplicate`).
//!
//! How the definition is served (in parts, each named by its hash) is the substrate's
//! (`strata_substrate::ui`); this module only holds the elements.

use crate::schema::TenantSchema;
use crate::types::{
    FieldType, SchemaChange, UiAction, UiControl, UiDisplayType, UiElement, UiErrorMessage, UiForm,
    UiLiveBinding, UiLiveEvent, UiOutcome, UiPresentation, UiScreen,
};
use std::collections::{BTreeMap, BTreeSet};

/// The notice kinds of the live protocol a binding may listen to (docs/protocol.md).
pub const NOTICE_KINDS: &[&str] = &["upsert", "remove", "schema", "membership"];

/// The longest title or label an element may carry, in bytes.
const MAX_TEXT: usize = 200;

/// Whether `change` is one this module applies.
pub fn is_ui_change(change: &SchemaChange) -> bool {
    matches!(
        change,
        SchemaChange::DefineUiElement { .. } | SchemaChange::RemoveUiElement(_)
    )
}

/// The interface elements of a schema, by id; empty when it holds none.
pub fn elements(schema: &TenantSchema) -> impl Iterator<Item = (&String, &UiElement)> {
    schema.ui.iter().flat_map(BTreeMap::iter)
}

/// One element, by id.
pub fn element<'a>(schema: &'a TenantSchema, id: &str) -> Option<&'a UiElement> {
    schema.ui.as_ref().and_then(|ui| ui.get(id))
}

/// Apply one change of this module to a working copy and describe it.
pub fn apply(schema: &mut TenantSchema, change: &SchemaChange) -> Result<String, String> {
    Ok(match change {
        SchemaChange::DefineUiElement { id, element } => {
            schema
                .ui
                .get_or_insert_with(BTreeMap::new)
                .insert(id.clone(), element.clone());
            format!("define interface element {id}")
        }
        SchemaChange::RemoveUiElement(id) => {
            let removed = schema.ui.as_mut().and_then(|ui| ui.remove(id));
            if removed.is_none() {
                return Err(format!("no interface element {id}"));
            }
            if schema.ui.as_ref().is_some_and(BTreeMap::is_empty) {
                schema.ui = None;
            }
            format!("remove interface element {id}")
        }
        other => return Err(format!("{other:?} is not a change of an interface element")),
    })
}

/// Whether `name` is the id of an element, a field or a parameter: lower-case letters, digits and
/// underscores, beginning with a letter, at most 64 bytes (the pattern of the ids of a product,
/// D-6582).
pub fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The first reason a one-line text (a title, a label) is not one, or `None`. An empty text is
/// allowed where `empty` says so.
fn line_problem(text: &str, empty: bool) -> Option<&'static str> {
    if !empty && text.trim().is_empty() {
        Some("is empty")
    } else if text.contains(['\r', '\n']) {
        Some("has a line break")
    } else if text.len() > MAX_TEXT {
        Some("is longer than 200 bytes")
    } else {
        None
    }
}

/// The parameters of a route (`/a/:slug` has `slug`), or the reason it is not a route.
pub fn route_params(route: &str) -> Result<Vec<String>, String> {
    let Some(rest) = route.strip_prefix('/') else {
        return Err(format!("route {route:?} does not begin with /"));
    };
    if rest.is_empty() {
        return Ok(Vec::new());
    }
    let mut params = Vec::new();
    for segment in rest.split('/') {
        match segment.strip_prefix(':') {
            Some(name) if is_name(name) => params.push(name.to_string()),
            Some(name) => return Err(format!("route {route:?} has the parameter {name:?}, which is not a name")),
            None if !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') => {}
            None => return Err(format!("route {route:?} has the segment {segment:?}, which is not lower-case letters, digits, - and _")),
        }
    }
    Ok(params)
}

/// The screens of the schema, by id.
fn screens(schema: &TenantSchema) -> BTreeMap<&str, &UiScreen> {
    elements(schema)
        .filter_map(|(id, element)| match element {
            UiElement::Screen(screen) => Some((id.as_str(), screen)),
            _ => None,
        })
        .collect()
}

/// The forms of the schema, by id.
fn forms(schema: &TenantSchema) -> BTreeMap<&str, &UiForm> {
    elements(schema)
        .filter_map(|(id, element)| match element {
            UiElement::Form(form) => Some((id.as_str(), form)),
            _ => None,
        })
        .collect()
}

/// The inputs of the operation `operation`, or `None` when the tenant has no such operation.
fn operation_inputs(schema: &TenantSchema, operation: &str) -> Option<BTreeSet<String>> {
    schema
        .operations
        .iter()
        .flatten()
        .find(|(id, _)| id.as_str() == operation)
        .map(|(_, op)| op.inputs.iter().map(|input| input.name.clone()).collect())
}

fn outcome_problems(
    at: &str,
    outcome: &UiOutcome,
    screens: &BTreeMap<&str, &UiScreen>,
    problems: &mut Vec<String>,
) {
    if let Some(screen) = &outcome.navigate {
        if !screens.contains_key(screen.as_str()) {
            problems.push(format!(
                "{at}: navigates to {screen}, which is not a screen of the tenant"
            ));
        }
    }
    if let Some(problem) = outcome
        .toast
        .as_deref()
        .and_then(|t| line_problem(t, false))
    {
        problems.push(format!("{at}: its message after success {problem}"));
    }
}

fn screen_problems(
    schema: &TenantSchema,
    id: &str,
    screen: &UiScreen,
    routes: &mut BTreeMap<String, String>,
    problems: &mut Vec<String>,
) {
    let at = format!("screen {id}");
    match route_params(&screen.route) {
        Ok(_) => {}
        Err(problem) => problems.push(format!("{at}: {problem}")),
    }
    if let Some(other) = routes.insert(screen.route.clone(), id.to_string()) {
        problems.push(format!(
            "{at}: answers {}, which screen {other} answers too",
            screen.route
        ));
    }
    match schema.views.get(&screen.view) {
        None => problems.push(format!(
            "{at}: shows view {}, which the tenant does not define",
            screen.view
        )),
        Some(view) if view.route != screen.route => problems.push(format!(
            "{at}: answers {}, but its view {} answers {}",
            screen.route, screen.view, view.route
        )),
        Some(_) => {}
    }
    if let Some(problem) = line_problem(&screen.title, true) {
        problems.push(format!("{at}: its title {problem}"));
    }
    for name in &screen.query {
        if !is_name(name) {
            problems.push(format!("{at}: the query parameter {name:?} is not a name"));
        }
    }
}

fn form_problems(
    schema: &TenantSchema,
    id: &str,
    form: &UiForm,
    screens: &BTreeMap<&str, &UiScreen>,
    problems: &mut Vec<String>,
) {
    let at = format!("form {id}");
    if !screens.contains_key(form.screen.as_str()) {
        problems.push(format!(
            "{at}: is on {}, which is not a screen of the tenant",
            form.screen
        ));
    }
    let inputs = operation_inputs(schema, form.operation.as_str());
    if inputs.is_none() {
        problems.push(format!(
            "{at}: invokes operation {}, which the tenant does not define",
            form.operation
        ));
    }
    let mut names = BTreeSet::new();
    for field in &form.fields {
        let here = format!("{at}, field {}", field.name);
        if !is_name(&field.name) {
            problems.push(format!("{here}: the name is not a name"));
        }
        if !names.insert(field.name.as_str()) {
            problems.push(format!("{here}: two fields have this name"));
        }
        if let Some(inputs) = &inputs {
            if !inputs.contains(&field.input) {
                problems.push(format!(
                    "{here}: fills {}, which is not an input of operation {}",
                    field.input, form.operation
                ));
            }
        }
        if let Some(problem) = line_problem(&field.label, false) {
            problems.push(format!("{here}: its label {problem}"));
        }
        let choices = matches!(field.control, UiControl::Select | UiControl::Radio);
        match (&field.options, choices) {
            (None, true) => problems.push(format!("{here}: a select or radio has choices")),
            (Some(options), true) if options.is_empty() => {
                problems.push(format!("{here}: a select or radio has choices"))
            }
            (Some(_), false) => {
                problems.push(format!("{here}: only a select or radio has choices"))
            }
            _ => {}
        }
    }
    if let Some(problem) = line_problem(&form.submit_label, false) {
        problems.push(format!("{at}: its button label {problem}"));
    }
    outcome_problems(&at, &form.on_success, screens, problems);
}

fn action_problems(
    schema: &TenantSchema,
    id: &str,
    action: &UiAction,
    screens: &BTreeMap<&str, &UiScreen>,
    forms: &BTreeMap<&str, &UiForm>,
    problems: &mut Vec<String>,
) {
    let at = format!("action {id}");
    let screen = screens.get(action.screen.as_str());
    if screen.is_none() {
        problems.push(format!(
            "{at}: is on {}, which is not a screen of the tenant",
            action.screen
        ));
    }
    if let Some(problem) = line_problem(&action.label, false) {
        problems.push(format!("{at}: its label {problem}"));
    }
    let inputs = operation_inputs(schema, action.operation.as_str());
    if inputs.is_none() {
        problems.push(format!(
            "{at}: invokes operation {}, which the tenant does not define",
            action.operation
        ));
    }
    let form = match &action.form {
        Some(name) => match forms.get(name.as_str()) {
            Some(form) if form.screen == action.screen => Some(*form),
            Some(_) => {
                problems.push(format!(
                    "{at}: reads form {name}, which is on another screen"
                ));
                None
            }
            None => {
                problems.push(format!(
                    "{at}: reads form {name}, which is not a form of the tenant"
                ));
                None
            }
        },
        None => None,
    };
    let route: BTreeSet<String> = screen
        .map(|s| {
            route_params(&s.route)
                .unwrap_or_default()
                .into_iter()
                .chain(s.query.iter().cloned())
                .collect()
        })
        .unwrap_or_default();
    for (input, source) in &action.arguments {
        if let Some(inputs) = &inputs {
            if !inputs.contains(input) {
                problems.push(format!(
                    "{at}: fills {input}, which is not an input of operation {}",
                    action.operation
                ));
            }
        }
        let known = match source.split_once('.') {
            Some(("form", field)) => {
                form.is_some_and(|form| form.fields.iter().any(|f| f.name == field))
            }
            Some(("route", param)) => route.contains(param),
            _ => false,
        };
        if !known {
            problems.push(format!(
                "{at}: fills {input} from {source}, which is neither form.<a field of its form> nor route.<a parameter of its screen>"
            ));
        }
    }
    if let Some(problem) = action
        .confirm
        .as_deref()
        .and_then(|t| line_problem(t, false))
    {
        problems.push(format!("{at}: its confirmation {problem}"));
    }
    outcome_problems(&at, &action.on_success, screens, problems);
}

fn live_problems(
    id: &str,
    live: &UiLiveBinding,
    screens: &BTreeMap<&str, &UiScreen>,
    problems: &mut Vec<String>,
) {
    let at = format!("live binding {id}");
    if !screens.contains_key(live.screen.as_str()) {
        problems.push(format!(
            "{at}: is on {}, which is not a screen of the tenant",
            live.screen
        ));
    }
    match live.event {
        UiLiveEvent::Notice => {
            for kind in &live.kinds {
                if !NOTICE_KINDS.contains(&kind.as_str()) {
                    problems.push(format!("{at}: listens to the notice kind {kind:?}, which the live protocol does not have"));
                }
            }
            if live.kinds.is_empty() && !live.objects {
                problems.push(format!("{at}: listens to no notice"));
            }
        }
        UiLiveEvent::SessionEnded => {
            if !live.kinds.is_empty() || live.objects {
                problems.push(format!(
                    "{at}: the end of a session is no notice, and names no notice kind or object"
                ));
            }
        }
    }
}

/// Whether a display type applies to a field of this type (docs/design-system.md, Display types).
pub fn display_applies(display: UiDisplayType, ty: &FieldType) -> bool {
    use UiDisplayType as D;
    let is_ref = |ty: &FieldType| matches!(ty, FieldType::Ref { .. });
    match display {
        D::Text | D::RichText | D::Status | D::Choice | D::Person | D::Url | D::Email => {
            matches!(ty, FieldType::Str)
        }
        D::Datetime | D::Date => matches!(ty, FieldType::Time),
        D::Duration | D::Count | D::Number | D::Money | D::Percent => matches!(ty, FieldType::Int),
        D::Boolean => matches!(ty, FieldType::Bool),
        D::Reference => is_ref(ty),
        D::References => matches!(ty, FieldType::List(inner) if is_ref(inner)),
        D::Tags => matches!(ty, FieldType::List(inner) if matches!(**inner, FieldType::Str)),
        D::Language => matches!(ty, FieldType::Language),
        D::Media => matches!(ty, FieldType::File { .. }),
        D::Code => matches!(ty, FieldType::Str | FieldType::Int),
    }
}

/// The presentations of the schema by the object type they describe, and for each the fields that
/// are drawn as a person. Read by the view path to decide which values are principals.
pub fn person_fields(
    schema: &TenantSchema,
) -> BTreeMap<crate::types::ObjectTypeId, BTreeSet<crate::types::FieldId>> {
    let mut out: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
    for (_, element) in elements(schema) {
        if let UiElement::Presentation(presentation) = element {
            for (field, shown) in &presentation.fields {
                if shown.display == UiDisplayType::Person {
                    out.entry(presentation.object.clone())
                        .or_default()
                        .insert(field.clone());
                }
            }
        }
    }
    out
}

/// The presentation of an object type, when the tenant has one.
pub fn presentation_of<'a>(
    schema: &'a TenantSchema,
    object: &crate::types::ObjectTypeId,
) -> Option<&'a UiPresentation> {
    elements(schema).find_map(|(_, element)| match element {
        UiElement::Presentation(p) if p.object == *object => Some(p),
        _ => None,
    })
}

fn presentation_problems(
    schema: &TenantSchema,
    id: &str,
    presentation: &UiPresentation,
    seen: &mut BTreeSet<String>,
    problems: &mut Vec<String>,
) {
    use UiDisplayType as D;
    let at = format!("presentation {id}");
    let Some(object) = schema.objects.get(&presentation.object) else {
        problems.push(format!(
            "ui.binding.unknown-object: {at}: describes {}, which is not an object type of the tenant",
            presentation.object
        ));
        return;
    };
    if !seen.insert(presentation.object.as_str().to_string()) {
        problems.push(format!(
            "ui.binding.duplicate: {at}: {} already has a presentation; an object type has one",
            presentation.object
        ));
    }
    if let Some(title) = &presentation.title {
        match object.fields.get(title) {
            None => problems.push(format!(
                "ui.binding.unknown-field: {at}: takes its title from {title}, which is not a field of {}",
                presentation.object
            )),
            Some(field) if !matches!(field.ty, FieldType::Str) => problems.push(format!(
                "ui.binding.title: {at}: takes its title from {title}, which is not a string field"
            )),
            Some(_) => {}
        }
    }
    for (name, shown) in &presentation.fields {
        let display = shown.display;
        let applies = match object.fields.get(name) {
            Some(field) => display_applies(display, &field.ty),
            None => match name.as_str() {
                "created_by" => display == D::Person,
                "created_at" | "updated_at" => matches!(display, D::Datetime | D::Date),
                _ => {
                    problems.push(format!(
                        "ui.binding.unknown-field: {at}: declares {name}, which {} does not have",
                        presentation.object
                    ));
                    continue;
                }
            },
        };
        if !applies {
            let ty = object.fields.get(name).map_or_else(
                || "a system attribute".to_string(),
                |f| format!("{:?}", f.ty),
            );
            problems.push(format!(
                "ui.binding.display-type: {at}: {name} is {ty}, to which the display type {display:?} does not apply"
            ));
        }
    }
}

fn message_problems(
    schema: &TenantSchema,
    id: &str,
    message: &UiErrorMessage,
    problems: &mut Vec<String>,
) {
    let at = format!("error message {id}");
    if message.text.trim().is_empty() {
        problems.push(format!("{at}: has no text"));
    }
    for owner in &message.applies_to {
        let applies = matches!(
            element(schema, owner),
            Some(UiElement::Form(_) | UiElement::Action(_))
        );
        if !applies {
            problems.push(format!(
                "{at}: applies to {owner}, which is not a form or an action of the tenant"
            ));
        }
    }
}

/// Every problem of the interface elements of `schema`, appended to `problems`.
pub fn problems(schema: &TenantSchema, problems: &mut Vec<String>) {
    let screens = screens(schema);
    let forms = forms(schema);
    let mut routes = BTreeMap::new();
    let mut presented = BTreeSet::new();
    for (id, element) in elements(schema) {
        if !is_name(id) {
            problems.push(format!(
                "interface element {id:?}: an id is lower-case letters, digits and underscores, beginning with a letter, at most 64 bytes"
            ));
        }
        match element {
            UiElement::Screen(screen) => screen_problems(schema, id, screen, &mut routes, problems),
            UiElement::Form(form) => form_problems(schema, id, form, &screens, problems),
            UiElement::Action(action) => {
                action_problems(schema, id, action, &screens, &forms, problems)
            }
            UiElement::LiveBinding(live) => live_problems(id, live, &screens, problems),
            UiElement::ErrorMessage(message) => message_problems(schema, id, message, problems),
            UiElement::Presentation(presentation) => {
                presentation_problems(schema, id, presentation, &mut presented, problems)
            }
        }
    }
}
