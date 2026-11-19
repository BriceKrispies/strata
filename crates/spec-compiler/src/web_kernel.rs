//! The frontend kernel's specification-derived modules, for the browser.
//!
//! The kernel (`web/src/kernel`) is an Elm-style pure function, `update(state, message) ->
//! {state, effects}`. What the specification states about the pages (its `screens`, `forms`,
//! `actions` and `liveBindings`, the validations on the inputs a form fills and the policies on
//! the contracts an action calls) is generated here as plain, dependency-free ES modules in the
//! style of the kernel (`// @ts-check`, JSDoc types), which the hand-written kernel calls.
//! Rendering stays hand-written: these modules give data and pure functions, never markup.
//!
//! - `routes.js`: the route table (`ROUTES`), `route(screen)`, `matchRoute(path, search)` ->
//!   `{screen, params, query}` (the catch-all answers every other path, its parameter holding the
//!   path; a path that several routes answer goes to the one with the most literal segments),
//!   and `hrefFor(screen, params)`, the address of a screen with its route parameters and its
//!   non-empty query parameters.
//! - `contracts.js`: one request builder per contract a screen, form, action or client machine
//!   uses, which returns `{method, path, body?, write}` for the kernel's HTTP effect, and one
//!   decoder of its reply into the contract's outputs; `request(contract, inputs, address)` and
//!   `decode(contract, body, inputs)` dispatch on the contract id, and `BFF` records each one's route.
//!   Inputs and outputs are named in
//!   the contract's wire case. `write` (the request carries the session's request token) is true
//!   for every `POST`.
//! - `forms.js`: the forms (`FORMS`), `initialValues(form, view)`, `options(form, field, view)`,
//!   `input(form, values, edited, name, value)` (a change, re-deriving the derived fields),
//!   `validate(form, values)` (the validations of the contract inputs the fields fill: field ->
//!   message, the first failing one per field), `inputs(form, values, scope)` (the fields and the
//!   `arguments`) and `succeeded(form, scope)` -> `{navigate, toast}`; and `slug(text)`.
//! - `actions.js`: the actions (`ACTIONS`), `actionsOn(screen)`, `rows(action, view)` (the rows
//!   of a per-row action), `label(action, item)`, `confirmText(action, item)`,
//!   `args(action, scope)`, `visible(action, scope, inputs)` (the policy decision on its
//!   contract), `succeeded(action, scope)` and `placeResult(action, local, outputs)`.
//! - `live.js`: the live bindings (`BINDINGS`) and `refreshes(screen, event, payload, view)`.
//!
//! **How the BFF serves a contract.** The browser does not call the gateway's contract routes;
//! it calls the BFF, whose routes a contract names with the vocabulary property `bffRoute`:
//! `"<METHOD> <path>"`, optionally followed by a shape. Without a shape a `POST` sends the inputs
//! as its JSON body and a `GET` sends nothing. `view` is the view endpoint: a `GET` whose query
//! string is `path` (the address of the screen, which carries its route parameters) followed by
//! the contract's other inputs that have a value, in declaration order; its reply is the view
//! model. `records <op> <object>` is the records endpoint: the body is `{op, object, id, fields,
//! expected_version}`, where `id` is the input whose type is `record-id`, `expected_version` the
//! input `expected-version`, and `fields` the other inputs that have a value; its reply is
//! `{id, version}`, so an output of type `record-id` decodes from `id`, the output `version` from
//! `version`, and an output that repeats an input of the same name from that input. A contract a
//! module needs that names no `bffRoute` is refused with [`ROUTE`].
//!
//! **Expressions** (arguments, navigation parameters and addresses, derivations, policy
//! conditions, live-binding conditions, toast and label placeholders) are compiled with
//! `strata-spec-expr`'s JavaScript target. They read a scope object the kernel passes:
//! `{route, query, view, local, item, form, response, principal}`, where `principal` is `{id,
//! tenant, kind, role, roles}`. A policy condition that reads `record.<field>` cannot be decided
//! on the page and counts as matching; a contract without a matching policy and without a
//! `policyDefault` is shown (the server's decision is final).

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;
use strata_spec_expr::{self as expr, ExprKind, JsBinding, JsScope, Ty};

use crate::constructs::{Rule, Unit, Verdict};
use crate::diagnostic::{dedup, Diagnostic, SPEC_ENTITY};
use crate::emit::{rust_header, GeneratedFile};
use crate::ir::{Behavior, Contract, Effect, Spec, TypeKind};
use crate::jsonc;
use crate::naming;
use crate::resolve::TypeRef;
use crate::web::{expr_ty, js_type, key, member, object_type, raw_ty, sq, Cases};
use crate::wire::{self, WireCase};

/// Code of a construct the kernel modules cannot be generated from.
pub const KERNEL: &str = "specc.web-kernel";

/// Code of a contract the kernel needs that names no BFF route, or names one that cannot be read.
pub const ROUTE: &str = "specc.web-kernel-route";

// ---------------------------------------------------------------------------------------------
// The constructs, as written.

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Ui {
    #[serde(default)]
    screens: Vec<Screen>,
    #[serde(default)]
    forms: Vec<Form>,
    #[serde(default)]
    actions: Vec<Action>,
    #[serde(default)]
    live_bindings: Vec<LiveBinding>,
    #[serde(default)]
    contracts: Vec<ContractRoute>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContractRoute {
    id: String,
    bff_route: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Screen {
    id: String,
    route: String,
    #[serde(default)]
    query: Vec<String>,
    view_model: Option<String>,
    kind: Option<String>,
    auth: Option<String>,
    #[serde(default)]
    local: Vec<Local>,
}

#[derive(Deserialize)]
struct Local {
    name: String,
    #[serde(rename = "type")]
    type_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Form {
    id: String,
    screen: String,
    contract: String,
    fields: Vec<Field>,
    #[serde(default)]
    arguments: BTreeMap<String, String>,
    submit_label: Option<String>,
    on_success: Option<Outcome>,
    on_error: Option<OnError>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Field {
    name: String,
    input: String,
    label: Option<String>,
    control: String,
    options: Option<Vec<Choice>>,
    options_from: Option<OptionsFrom>,
    derive: Option<String>,
    until: Option<String>,
    transform: Option<String>,
}

#[derive(Deserialize)]
struct Choice {
    value: Value,
    label: String,
}

#[derive(Deserialize)]
struct OptionsFrom {
    path: String,
    value: Option<String>,
    label: Option<String>,
}

#[derive(Deserialize, Default)]
struct Outcome {
    navigate: Option<Navigate>,
    toast: Option<String>,
}

#[derive(Deserialize)]
struct Navigate {
    screen: Option<String>,
    #[serde(default)]
    params: BTreeMap<String, String>,
    path: Option<String>,
}

#[derive(Deserialize)]
struct OnError {
    show: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Action {
    id: String,
    screen: String,
    label: String,
    contract: String,
    form: Option<String>,
    for_each: Option<String>,
    #[serde(default)]
    arguments: BTreeMap<String, String>,
    confirm: Option<String>,
    optimistic: Option<bool>,
    result: Option<ResultInto>,
    on_success: Option<Outcome>,
    on_error: Option<OnError>,
}

#[derive(Deserialize)]
struct ResultInto {
    into: String,
    output: String,
}

#[derive(Deserialize)]
struct LiveBinding {
    id: String,
    screen: String,
    event: String,
    refresh: String,
    when: Option<Value>,
}

// ---------------------------------------------------------------------------------------------
// Entry point.

/// Generates the kernel modules (paths relative to the output directory), or refuses with every
/// diagnostic. A specification without screens gives no files. `machines` are the client
/// machines, whose emits may address a contract the kernel then calls.
pub fn generate(
    spec: &Spec,
    machines: &[Behavior],
    text: &str,
) -> Result<Vec<GeneratedFile>, Vec<Diagnostic>> {
    let value = jsonc::parse(text).map_err(|e| {
        vec![Diagnostic::new(
            KERNEL,
            SPEC_ENTITY,
            format!("the specification is not valid JSONC: {e}"),
        )]
    })?;
    let ui: Ui = match serde_json::from_value(value) {
        Ok(ui) => ui,
        Err(e) => {
            return Err(vec![Diagnostic::new(
                KERNEL,
                SPEC_ENTITY,
                format!("the screens, forms, actions or live bindings cannot be read: {e}"),
            )])
        }
    };
    if ui.screens.is_empty() {
        return Ok(Vec::new());
    }
    let mut k = Kernel::new(spec, &ui);
    let files = [
        ("routes.js", k.routes()),
        ("contracts.js", k.contracts(machines)),
        ("forms.js", k.forms()),
        ("actions.js", k.actions()),
        ("live.js", k.live()),
    ];
    if !k.out.is_empty() {
        return Err(dedup(k.out));
    }
    Ok(files
        .into_iter()
        .map(|(path, body)| GeneratedFile {
            path: path.to_string(),
            contents: format!("{}\n{body}", rust_header(&spec.path)),
        })
        .collect())
}

/// `camelCase` of an id, for a JavaScript name.
fn camel(id: &str) -> String {
    let pascal = naming::pascal(id).unwrap_or_else(|_| {
        id.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    });
    let mut chars = pascal.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_lowercase(), chars.as_str()),
        None => "unnamed".to_string(),
    }
}

/// How a contract is called through the BFF.
#[derive(Clone)]
enum Shape {
    /// The inputs as the JSON body of a `POST`; nothing for a `GET`.
    Json,
    /// The view endpoint.
    View,
    /// The records endpoint, with its operation and object.
    Records { op: String, object: String },
}

#[derive(Clone)]
struct BffRoute {
    method: String,
    path: String,
    shape: Shape,
}

fn parse_route(text: &str) -> Option<BffRoute> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let (method, path, rest) = match words.as_slice() {
        [method, path, rest @ ..] => (*method, *path, rest),
        _ => return None,
    };
    if !matches!(method, "GET" | "POST") || !path.starts_with('/') {
        return None;
    }
    let shape = match rest {
        [] => Shape::Json,
        ["view"] if method == "GET" => Shape::View,
        ["records", op, object] if method == "POST" => Shape::Records {
            op: (*op).to_string(),
            object: (*object).to_string(),
        },
        _ => return None,
    };
    Some(BffRoute {
        method: method.to_string(),
        path: path.to_string(),
        shape,
    })
}

// ---------------------------------------------------------------------------------------------
// Expression scopes.

/// A named value an expression reads below a root: an input, an output, a field.
#[derive(Clone)]
struct Slot {
    name: String,
    wire: String,
    ty: TypeRef,
    optional: bool,
}

/// One root of a scope: `view`, `item`, `form` and so on, read from `js`.
#[derive(Clone)]
struct Root {
    name: &'static str,
    js: String,
    slots: Vec<Slot>,
    /// The type of the root itself when an expression reads it by its name alone (`value`).
    direct: Option<TypeRef>,
}

/// The scope of an expression: its roots, and whether a read of `record.<field>` was seen.
struct UiScope<'a> {
    spec: &'a Spec,
    cases: &'a Cases,
    roots: Vec<Root>,
}

/// The overflow expression of the kernel modules: a `RangeError`.
const OVERFLOW: &str = "(() => { throw new RangeError('integer overflow'); })()";

impl UiScope<'_> {
    /// The fields of a struct type, in its wire case.
    fn struct_slots(&self, t: &TypeRef) -> Option<Vec<Slot>> {
        let id = match t {
            TypeRef::Declared(id) => id,
            TypeRef::Option(inner) => return self.struct_slots(inner),
            _ => return None,
        };
        let Some(TypeKind::Struct { fields }) = self.spec.type_decl(id).map(|d| &d.kind) else {
            return None;
        };
        let case = self.cases.get(id).copied().unwrap_or_default();
        Some(
            fields
                .iter()
                .map(|f| Slot {
                    name: f.name.clone(),
                    wire: case.name(&f.name),
                    ty: TypeRef::parse(&f.type_name),
                    optional: f.optional,
                })
                .collect(),
        )
    }

    /// The JavaScript and type of a path below a root.
    fn walk(&self, path: &[&str]) -> Option<(String, TypeRef, bool)> {
        let (first, rest) = path.split_first()?;
        let root = self.roots.iter().find(|r| r.name == *first)?;
        if rest.is_empty() {
            return root.direct.clone().map(|t| (root.js.clone(), t, false));
        }
        let (name, rest) = rest.split_first()?;
        let slot = root.slots.iter().find(|s| s.name == *name)?;
        let mut js = format!("{}{}", root.js, optional_member(&slot.wire));
        let mut ty = slot.ty.clone();
        let mut optional = slot.optional;
        for part in rest {
            let slots = self.struct_slots(&ty)?;
            let s = slots.into_iter().find(|s| s.name == *part)?;
            js.push_str(&optional_member(&s.wire));
            optional = optional || s.optional || matches!(ty, TypeRef::Option(_));
            ty = s.ty;
        }
        Some((js, ty, optional))
    }
}

/// A member access that tolerates an absent object.
fn optional_member(name: &str) -> String {
    let m = member(name);
    if m.starts_with('.') {
        format!("?{m}")
    } else {
        format!("?.{m}")
    }
}

impl JsScope for UiScope<'_> {
    fn resolve_js(&self, path: &[&str]) -> Option<JsBinding> {
        let (js, t, optional) = self.walk(path)?;
        let ty = if optional && !matches!(t, TypeRef::Option(_)) {
            Ty::Option(Box::new(raw_ty(&t)))
        } else {
            expr_ty(self.spec, &t)
        };
        Some(JsBinding { ty, js })
    }

    fn overflow_error_js(&self) -> String {
        OVERFLOW.to_string()
    }
}

/// Every dotted reference in an expression.
fn references(e: &expr::Expr, out: &mut Vec<Vec<String>>) {
    match &e.kind {
        ExprKind::Lit(_) => {}
        ExprKind::Ref(path) => out.push(path.clone()),
        ExprKind::Un(_, a) => references(a, out),
        ExprKind::Bin(_, a, b) => {
            references(a, out);
            references(b, out);
        }
    }
}

/// The candidate types an expression filling a value of type `t` may have: the type, then its
/// option (an optional value may be read from an optional member).
fn candidates(spec: &Spec, t: &TypeRef) -> Vec<Ty> {
    let base = expr_ty(spec, t);
    match base {
        Ty::Option(_) => vec![base],
        other => vec![other.clone(), Ty::Option(Box::new(other))],
    }
}

// ---------------------------------------------------------------------------------------------
// The generator.

struct Kernel<'a> {
    spec: &'a Spec,
    ui: &'a Ui,
    cases: Cases,
    /// The system's wire case: names of form fields.
    case: WireCase,
    routes: BTreeMap<String, BffRoute>,
    out: Vec<Diagnostic>,
}

impl<'a> Kernel<'a> {
    fn new(spec: &'a Spec, ui: &'a Ui) -> Self {
        let mut routes = BTreeMap::new();
        let mut out = Vec::new();
        for c in &ui.contracts {
            let Some(text) = &c.bff_route else { continue };
            match parse_route(text) {
                Some(r) => {
                    routes.insert(c.id.clone(), r);
                }
                None => out.push(Diagnostic::new(
                    ROUTE,
                    &c.id,
                    format!(
                        "Contract \"{}\" names BFF route \"{text}\", which is not \"GET|POST /path\" optionally followed by \"view\" (GET) or \"records <op> <object>\" (POST).",
                        c.id
                    ),
                )),
            }
        }
        Kernel {
            spec,
            ui,
            cases: wire::type_cases(spec),
            case: spec.system.wire_case,
            routes,
            out,
        }
    }

    fn refuse(&mut self, entity: &str, message: String) {
        self.out.push(Diagnostic::new(KERNEL, entity, message));
    }

    fn contract(&self, id: &str) -> Option<&'a Contract> {
        self.spec.contracts.iter().find(|c| c.id == id)
    }

    fn screen(&self, id: &str) -> Option<&'a Screen> {
        self.ui.screens.iter().find(|s| s.id == id)
    }

    fn contract_case(&self, c: &Contract) -> WireCase {
        wire::contract_case(self.spec, c)
    }

    fn slots(&self, c: &Contract, outputs: bool) -> Vec<Slot> {
        let case = self.contract_case(c);
        let fields = if outputs { &c.outputs } else { &c.inputs };
        fields
            .iter()
            .map(|f| Slot {
                name: f.name.clone(),
                wire: case.name(&f.name),
                ty: TypeRef::parse(f.type_name.as_deref().unwrap_or("json")),
                optional: f.optional,
            })
            .collect()
    }

    /// The scope of a screen: its route and query parameters, view model, locals and the
    /// principal.
    fn screen_roots(&self, screen: &Screen) -> Vec<Root> {
        let params = route_params(&screen.route);
        let strings = |names: &[String], optional: bool| -> Vec<Slot> {
            names
                .iter()
                .map(|n| Slot {
                    name: n.clone(),
                    wire: n.clone(),
                    ty: TypeRef::parse("string"),
                    optional,
                })
                .collect()
        };
        let mut route = strings(&params, false);
        route.extend(strings(&screen.query, true));
        let view = screen
            .view_model
            .as_deref()
            .and_then(|id| self.contract(id))
            .map(|c| self.slots(c, true))
            .unwrap_or_default();
        let local = screen
            .local
            .iter()
            .map(|l| Slot {
                name: l.name.clone(),
                wire: l.name.clone(),
                ty: TypeRef::parse(&l.type_name),
                optional: true,
            })
            .collect();
        let principal = ["id", "tenant", "role"]
            .iter()
            .map(|n| Slot {
                name: (*n).to_string(),
                wire: (*n).to_string(),
                ty: TypeRef::parse("string"),
                optional: false,
            })
            .collect();
        vec![
            Root {
                name: "route",
                js: "scope.route".into(),
                slots: route,
                direct: None,
            },
            Root {
                name: "query",
                js: "scope.query".into(),
                slots: strings(&screen.query, true),
                direct: None,
            },
            Root {
                name: "view",
                js: "scope.view".into(),
                slots: view,
                direct: None,
            },
            Root {
                name: "local",
                js: "scope.local".into(),
                slots: local,
                direct: None,
            },
            Root {
                name: "principal",
                js: "scope.principal".into(),
                slots: principal,
                direct: None,
            },
        ]
    }

    /// The `form` root of a form: each field under its name, typed as its input.
    fn form_root(&self, form: &Form, js: &str) -> Root {
        let inputs = self
            .contract(&form.contract)
            .map(|c| self.slots(c, false))
            .unwrap_or_default();
        Root {
            name: "form",
            js: js.to_string(),
            slots: form
                .fields
                .iter()
                .filter_map(|f| {
                    let input = inputs.iter().find(|i| i.name == f.input)?;
                    Some(Slot {
                        name: f.name.clone(),
                        wire: self.case.name(&f.name),
                        ty: input.ty.clone(),
                        optional: input.optional,
                    })
                })
                .collect(),
            direct: None,
        }
    }

    fn response_root(&self, contract: &str) -> Root {
        Root {
            name: "response",
            js: "scope.response".into(),
            slots: self
                .contract(contract)
                .map(|c| self.slots(c, true))
                .unwrap_or_default(),
            direct: None,
        }
    }

    /// An expression as JavaScript, typed as one of `types` (any type when empty).
    fn compile(
        &mut self,
        entity: &str,
        what: &str,
        src: &str,
        roots: &[Root],
        types: &[Ty],
    ) -> Option<String> {
        let parsed = match expr::parse(src) {
            Ok(p) => p,
            Err(e) => {
                self.refuse(
                    entity,
                    format!("{what} (`{src}`) does not parse: {}.", e.message),
                );
                return None;
            }
        };
        let scope = UiScope {
            spec: self.spec,
            cases: &self.cases,
            roots: roots.to_vec(),
        };
        let result = if types.is_empty() {
            expr::to_js(&parsed, &scope)
        } else {
            let mut last = None;
            let mut found = None;
            for ty in types {
                match expr::to_js_as(&parsed, &scope, ty) {
                    Ok(js) => {
                        found = Some(js);
                        break;
                    }
                    Err(e) => last = Some(e),
                }
            }
            match (found, last) {
                (Some(js), _) => Ok(js),
                (None, Some(e)) => Err(e),
                (None, None) => unreachable!("types is not empty"),
            }
        };
        match result {
            Ok(js) => Some(js),
            Err(e) => {
                self.refuse(
                    entity,
                    format!("{what} (`{src}`) cannot be generated: {}.", e.message),
                );
                None
            }
        }
    }

    /// A text with `${path}` placeholders as a JavaScript string expression.
    fn template(&mut self, entity: &str, what: &str, text: &str, roots: &[Root]) -> Option<String> {
        let scope = UiScope {
            spec: self.spec,
            cases: &self.cases,
            roots: roots.to_vec(),
        };
        let mut parts = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find("${") {
            if start > 0 {
                parts.push(sq(&rest[..start]));
            }
            let after = &rest[start + 2..];
            let Some(end) = after.find('}') else {
                self.refuse(
                    entity,
                    format!("{what} has an unclosed placeholder: \"{text}\"."),
                );
                return None;
            };
            let path: Vec<&str> = after[..end].trim().split('.').collect();
            match scope.walk(&path) {
                Some((js, _, _)) => parts.push(format!("text({js})")),
                None => {
                    self.refuse(
                        entity,
                        format!(
                            "{what} reads `{}`, which is not in scope.",
                            after[..end].trim()
                        ),
                    );
                    return None;
                }
            }
            rest = &after[end + 1..];
        }
        if !rest.is_empty() || parts.is_empty() {
            parts.push(sq(rest));
        }
        Some(parts.join(" + "))
    }

    /// The JavaScript of a path into a screen's view model and the type it reaches.
    fn view_path(&self, screen: &Screen, path: &str, base: &str) -> Option<(String, TypeRef)> {
        let path = path.strip_prefix("view.").unwrap_or(path);
        let mut roots = self.screen_roots(screen);
        for r in &mut roots {
            if r.name == "view" {
                r.js = base.to_string();
            }
        }
        let scope = UiScope {
            spec: self.spec,
            cases: &self.cases,
            roots,
        };
        let mut parts = vec!["view"];
        parts.extend(path.split('.'));
        scope.walk(&parts).map(|(js, t, _)| (js, t))
    }

    /// The struct id of the items of a list type.
    fn list_item(t: &TypeRef) -> Option<&TypeRef> {
        match t {
            TypeRef::List(inner) => Some(inner),
            TypeRef::Option(inner) => Self::list_item(inner),
            _ => None,
        }
    }

    // -----------------------------------------------------------------------------------------
    // routes.js

    fn routes(&mut self) -> String {
        let mut entries = Vec::new();
        let mut catch_alls = 0;
        for s in &self.ui.screens {
            let catch_all = s.route.starts_with('*');
            if catch_all {
                catch_alls += 1;
            }
            let (segments, params): (Vec<String>, Vec<String>) = if catch_all {
                let name = s.route.trim_start_matches('*');
                (
                    Vec::new(),
                    if name.is_empty() {
                        Vec::new()
                    } else {
                        vec![name.to_string()]
                    },
                )
            } else {
                (
                    s.route
                        .split('/')
                        .filter(|p| !p.is_empty())
                        .map(str::to_string)
                        .collect(),
                    route_params(&s.route),
                )
            };
            let list = |items: &[String]| {
                format!(
                    "[{}]",
                    items.iter().map(|i| sq(i)).collect::<Vec<_>>().join(", ")
                )
            };
            let opt = |v: &Option<String>| v.as_deref().map_or("null".to_string(), sq);
            entries.push(format!(
                "  {{ screen: {}, route: {}, segments: {}, params: {}, query: {}, auth: {}, viewModel: {}, kind: {}, catchAll: {catch_all} }},",
                sq(&s.id),
                sq(&s.route),
                list(&segments),
                list(&params),
                list(&s.query),
                sq(s.auth.as_deref().unwrap_or("none")),
                opt(&s.view_model),
                opt(&s.kind),
            ));
        }
        if catch_alls > 1 {
            self.refuse(
                SPEC_ENTITY,
                "More than one screen is the catch-all.".to_string(),
            );
        }
        format!(
            r#"// @ts-check
// The routes of the screens: which screen answers an address, with its parameters.

/**
 * A screen's route. `segments` are the route's path segments (`:name` a parameter), `params`
 * its route parameters (the catch-all's holds the path), `query` its query parameters.
 * @typedef {{{{ screen: string, route: string, segments: string[], params: string[], query: string[], auth: string, viewModel: string | null, kind: string | null, catchAll: boolean }}}} Route
 */

/** Every screen's route, in declaration order. @type {{Route[]}} */
export const ROUTES = [
{entries}
];

const BY_SCREEN = new Map(ROUTES.map((r) => [r.screen, r]));

/**
 * The route of a screen.
 * @param {{string}} screen
 * @returns {{Route | null}}
 */
export function route(screen) {{
  return BY_SCREEN.get(screen) ?? null;
}}

/**
 * The screen that answers an address, with its route and query parameters. Of the routes that
 * answer the path, the one with the most literal segments wins; the catch-all answers every
 * other path. Only the screen's own query parameters are kept.
 * @param {{string}} path
 * @param {{string | Record<string, string>}} [search] The query string (`?q=kiln`) or its parameters.
 * @returns {{{{ screen: string, params: Record<string, string>, query: Record<string, string> }} | null}}
 */
export function matchRoute(path, search = '') {{
  const parts = path.split('/').filter((s) => s !== '');
  /** @type {{{{ entry: Route, params: Record<string, string>, literals: number }} | null}} */
  let best = null;
  for (const entry of ROUTES) {{
    if (entry.catchAll || entry.segments.length !== parts.length) continue;
    /** @type {{Record<string, string>}} */
    const params = {{}};
    let literals = 0;
    let ok = true;
    for (let i = 0; i < parts.length && ok; i += 1) {{
      const pattern = entry.segments[i];
      if (pattern.startsWith(':')) {{
        try {{
          params[pattern.slice(1)] = decodeURIComponent(parts[i]);
        }} catch {{
          ok = false;
        }}
      }} else if (pattern === parts[i]) {{
        literals += 1;
      }} else {{
        ok = false;
      }}
    }}
    if (ok && (!best || literals > best.literals)) best = {{ entry, params, literals }};
  }}
  const entry = best?.entry ?? ROUTES.find((r) => r.catchAll) ?? null;
  if (!entry) return null;
  /** @type {{Record<string, string>}} */
  const params = best ? best.params : entry.params.length ? {{ [entry.params[0]]: path }} : {{}};
  const given = typeof search === 'string' ? new URLSearchParams(search) : new URLSearchParams(Object.entries(search));
  /** @type {{Record<string, string>}} */
  const query = {{}};
  for (const name of entry.query) {{
    const value = given.get(name);
    if (value !== null) query[name] = value;
  }}
  return {{ screen: entry.screen, params, query }};
}}

/**
 * The address of a screen: its route with these parameters, then its query parameters that
 * have a non-empty value, in declaration order. The catch-all has no address.
 * @param {{string}} screen
 * @param {{Record<string, unknown>}} [params]
 * @returns {{string}}
 */
export function hrefFor(screen, params = {{}}) {{
  const entry = BY_SCREEN.get(screen);
  if (!entry || entry.catchAll) throw new Error(`screen ${{screen}} has no address`);
  const path = `/${{entry.segments.map((s) => (s.startsWith(':') ? encodeURIComponent(String(params[s.slice(1)] ?? '')) : s)).join('/')}}`;
  const pairs = entry.query
    .filter((name) => params[name] != null && params[name] !== '')
    .map((name) => `${{encodeURIComponent(name)}}=${{encodeURIComponent(String(params[name]))}}`);
  return pairs.length ? `${{path}}?${{pairs.join('&')}}` : path;
}}
"#,
            entries = entries.join("\n")
        )
    }

    // -----------------------------------------------------------------------------------------
    // contracts.js

    /// The contracts the kernel calls: view models, forms, actions, and contracts the client
    /// machines address, in declaration order.
    fn used_contracts(&self, machines: &[Behavior]) -> Vec<&'a Contract> {
        let mut used = BTreeSet::new();
        for s in &self.ui.screens {
            if let Some(v) = &s.view_model {
                used.insert(v.clone());
            }
        }
        for f in &self.ui.forms {
            used.insert(f.contract.clone());
        }
        for a in &self.ui.actions {
            used.insert(a.contract.clone());
        }
        for b in machines {
            for t in &b.transitions {
                for e in &t.effects {
                    // A machine may address a contract served over the socket (its emits are
                    // frames the kernel sends); only one the BFF serves over HTTP is a request.
                    if let Effect::Emit { to: Some(to), .. } = e {
                        if self.contract(to).is_some() && self.routes.contains_key(to) {
                            used.insert(to.clone());
                        }
                    }
                }
            }
        }
        self.spec
            .contracts
            .iter()
            .filter(|c| used.contains(&c.id))
            .collect()
    }

    fn contracts(&mut self, machines: &[Behavior]) -> String {
        let used = self.used_contracts(machines);
        let mut out = String::from(
            r"// @ts-check
// The contracts the kernel calls, as the BFF serves them: one request builder and one reply
// decoder per contract. Inputs and outputs are named in the contract's wire case.

/**
 * What the kernel's HTTP effect sends. `write` requests carry the session's request token.
 * @typedef {{ method: 'GET' | 'POST', path: string, body?: any, write: boolean }} Request
 */

/**
 * The properties of an object that have a value.
 * @param {Record<string, unknown>} values
 * @returns {Record<string, any>}
 */
function defined(values) {
  return Object.fromEntries(Object.entries(values).filter(([, v]) => v !== undefined));
}

/**
 * The address of the view endpoint for a screen's address and its other inputs.
 * @param {string} path
 * @param {[string, unknown][]} query
 * @returns {string}
 */
function viewPath(path, query) {
  const pairs = [['path', path], ...query.filter(([, v]) => v !== undefined && v !== null)];
  return `/api/view?${pairs.map(([k, v]) => `${encodeURIComponent(String(k))}=${encodeURIComponent(String(v))}`).join('&')}`;
}

/**
 * The declared members of a reply that it holds.
 * @param {any} body
 * @param {string[]} names
 * @returns {Record<string, any>}
 */
function pick(body, names) {
  /** @type {Record<string, any>} */
  const out = {};
  for (const name of names) {
    if (body != null && body[name] !== undefined) out[name] = body[name];
  }
  return out;
}

",
        );
        let mut requests = Vec::new();
        let mut table = Vec::new();
        let mut decoders = Vec::new();
        for c in used {
            let Some(route) = self.routes.get(&c.id).cloned() else {
                self.out.push(Diagnostic::new(
                    ROUTE,
                    &c.id,
                    format!(
                        "Contract \"{}\" is called by the web application but names no bffRoute, so there is no request to build for it.",
                        c.id
                    ),
                ));
                continue;
            };
            let name = camel(&c.id);
            let decoder = format!(
                "decode{}",
                naming::pascal(&c.id).unwrap_or_else(|_| name.clone())
            );
            let inputs = self.slots(c, false);
            let outputs = self.slots(c, true);
            let input_t = self.object_jsdoc(&inputs);
            let output_t = self.object_jsdoc(&outputs);
            let doc_route = match &route.shape {
                Shape::Json => format!("{} {}", route.method, route.path),
                Shape::View => format!("{} {} (the view endpoint)", route.method, route.path),
                Shape::Records { op, object } => {
                    format!("{} {} ({op} {object})", route.method, route.path)
                }
            };
            let write = route.method == "POST";
            let method = sq(&route.method);
            let path = sq(&route.path);
            table.push(match &route.shape {
                Shape::Json => format!(
                    "  {}: {{ method: {method}, path: {path}, shape: 'json', op: null, object: null }},",
                    sq(&c.id)
                ),
                Shape::View => format!(
                    "  {}: {{ method: {method}, path: {path}, shape: 'view', op: null, object: null }},",
                    sq(&c.id)
                ),
                Shape::Records { op, object } => format!(
                    "  {}: {{ method: {method}, path: {path}, shape: 'records', op: {}, object: {} }},",
                    sq(&c.id),
                    sq(op),
                    sq(object)
                ),
            });
            match &route.shape {
                Shape::Json => {
                    let body = if route.method == "POST" {
                        let items: Vec<String> = inputs
                            .iter()
                            .map(|s| format!("{}: inputs{}", key(&s.wire), member(&s.wire)))
                            .collect();
                        format!(", body: defined({{ {} }})", items.join(", "))
                            .replace("defined({  })", "{}")
                    } else {
                        String::new()
                    };
                    out.push_str(&format!(
                        "/**\n * Contract `{}`: {}. {doc_route}.\n * @param {{{input_t}}} inputs\n * @returns {{Request}}\n */\nexport function {name}(inputs) {{\n  return {{ method: {method}, path: {path}{body}, write: {write} }};\n}}\n\n",
                        c.id, c.name
                    ));
                    requests.push(format!(
                        "    case {}:\n      return {name}(inputs);",
                        sq(&c.id)
                    ));
                }
                Shape::View => {
                    let route_names: BTreeSet<String> = self
                        .ui
                        .screens
                        .iter()
                        .filter(|s| s.view_model.as_deref() == Some(c.id.as_str()))
                        .flat_map(|s| {
                            if s.route.starts_with('*') {
                                vec![s.route.trim_start_matches('*').to_string()]
                            } else {
                                route_params(&s.route)
                            }
                        })
                        .collect();
                    let query: Vec<String> = inputs
                        .iter()
                        .filter(|s| !route_names.contains(&s.name))
                        .map(|s| format!("[{}, inputs{}]", sq(&s.wire), member(&s.wire)))
                        .collect();
                    out.push_str(&format!(
                        "/**\n * Contract `{}`: {}. {doc_route}; the address carries the route parameters.\n * @param {{string}} address\n * @param {{{input_t}}} inputs\n * @returns {{Request}}\n */\nexport function {name}(address, inputs) {{\n  return {{ method: {method}, path: viewPath(address, [{}]), write: false }};\n}}\n\n",
                        c.id,
                        c.name,
                        query.join(", ")
                    ));
                    requests.push(format!(
                        "    case {}:\n      return {name}(address, inputs);",
                        sq(&c.id)
                    ));
                }
                Shape::Records { op, object } => {
                    let id = inputs.iter().find(|s| {
                        matches!(&s.ty, TypeRef::Declared(t) if t == "record-id")
                            && s.name != "expected-version"
                    });
                    let version = inputs.iter().find(|s| s.name == "expected-version");
                    let fields: Vec<String> = inputs
                        .iter()
                        .filter(|s| {
                            Some(s.name.as_str()) != id.map(|i| i.name.as_str())
                                && s.name != "expected-version"
                        })
                        .map(|s| format!("{}: inputs{}", key(&s.wire), member(&s.wire)))
                        .collect();
                    let mut body =
                        vec![format!("op: {}", sq(op)), format!("object: {}", sq(object))];
                    if let Some(i) = id {
                        body.push(format!("id: inputs{}", member(&i.wire)));
                    }
                    if op != "delete" {
                        body.push(format!("fields: defined({{ {} }})", fields.join(", ")));
                    }
                    if let Some(v) = version {
                        body.push(format!("expected_version: inputs{}", member(&v.wire)));
                    }
                    out.push_str(&format!(
                        "/**\n * Contract `{}`: {}. {doc_route}.\n * @param {{{input_t}}} inputs\n * @returns {{Request}}\n */\nexport function {name}(inputs) {{\n  return {{ method: {method}, path: {path}, body: {{ {} }}, write: true }};\n}}\n\n",
                        c.id,
                        c.name,
                        body.join(", ")
                    ));
                    requests.push(format!(
                        "    case {}:\n      return {name}(inputs);",
                        sq(&c.id)
                    ));
                }
            }
            // The decoder.
            match &route.shape {
                Shape::Records { .. } => {
                    let items: Vec<String> = outputs
                        .iter()
                        .map(|o| {
                            let from = if matches!(&o.ty, TypeRef::Declared(t) if t == "record-id")
                            {
                                "body?.id".to_string()
                            } else if o.name == "version" {
                                "body?.version".to_string()
                            } else if inputs.iter().any(|i| i.name == o.name) {
                                format!("inputs{}", optional_member(&o.wire))
                            } else {
                                format!("body{}", optional_member(&o.wire))
                            };
                            format!("{}: {from}", key(&o.wire))
                        })
                        .collect();
                    let uses_inputs = items.iter().any(|i| i.contains("inputs"));
                    let uses_body = items.iter().any(|i| i.contains("body"));
                    out.push_str(&format!(
                        "/**\n * The outputs of `{}` from the records endpoint's reply (`{{id, version}}`) and the inputs it repeats.\n * @param {{any}} {}\n * @param {{any}} {}\n * @returns {{{output_t}}}\n */\nexport function {decoder}({}, {}) {{\n  return {};\n}}\n\n",
                        c.id,
                        if uses_body { "body" } else { "_body" },
                        if uses_inputs { "inputs" } else { "_inputs" },
                        if uses_body { "body" } else { "_body" },
                        if uses_inputs { "inputs" } else { "_inputs" },
                        if items.is_empty() { "{}".to_string() } else { format!("{{ {} }}", items.join(", ")) }
                    ));
                }
                _ => {
                    let names: Vec<String> = outputs.iter().map(|o| sq(&o.wire)).collect();
                    out.push_str(&format!(
                        "/**\n * The outputs of `{}` from its reply.\n * @param {{any}} body\n * @param {{any}} [_inputs]\n * @returns {{{output_t}}}\n */\nexport function {decoder}(body, _inputs) {{\n  return /** @type {{any}} */ (pick(body, [{}]));\n}}\n\n",
                        c.id,
                        names.join(", ")
                    ));
                }
            }
            decoders.push(format!(
                "    case {}:\n      return {decoder}(body, inputs);",
                sq(&c.id)
            ));
        }
        out.push_str(&format!(
            r"/**
 * The request of a contract with these inputs; `address` is the screen's address for a view
 * model.
 * @param {{string}} contract
 * @param {{any}} inputs
 * @param {{string}} [address]
 * @returns {{Request}}
 */
export function request(contract, inputs, address = '/') {{
  switch (contract) {{
{}
    default:
      throw new Error(`the web application does not call contract ${{contract}}`);
  }}
}}

/**
 * The outputs of a contract from its reply, given the inputs it was called with.
 * @param {{string}} contract
 * @param {{any}} body
 * @param {{any}} [inputs]
 * @returns {{any}}
 */
export function decode(contract, body, inputs = {{}}) {{
  switch (contract) {{
{}
    default:
      throw new Error(`the web application does not call contract ${{contract}}`);
  }}
}}

/**
 * How the BFF serves each contract: its route, and for the records endpoint the operation and
 * the object.
 * @type {{Record<string, {{ method: 'GET' | 'POST', path: string, shape: 'json' | 'view' | 'records', op: string | null, object: string | null }}>}}
 */
export const BFF = {{
{}
}};
",
            requests.join("\n"),
            decoders.join("\n"),
            table.join("\n")
        ));
        out
    }

    fn object_jsdoc(&self, slots: &[Slot]) -> String {
        let items: Vec<String> = slots
            .iter()
            .map(|s| {
                let option = matches!(s.ty, TypeRef::Option(_));
                format!(
                    "{}{}: {}",
                    key(&s.wire),
                    if s.optional || option { "?" } else { "" },
                    js_type(self.spec, &self.cases, &s.ty, false, 0)
                )
            })
            .collect();
        object_type(&items)
    }

    // -----------------------------------------------------------------------------------------
    // forms.js

    fn forms(&mut self) -> String {
        let mut table = Vec::new();
        let mut code = Vec::new();
        let mut cases = Vec::new();
        for form in &self.ui.forms {
            let Some(screen) = self.screen(&form.screen) else {
                self.refuse(
                    &form.id,
                    format!(
                        "Form \"{}\" names screen \"{}\", which is not declared.",
                        form.id, form.screen
                    ),
                );
                continue;
            };
            let Some(contract) = self.contract(&form.contract) else {
                self.refuse(
                    &form.id,
                    format!(
                        "Form \"{}\" names contract \"{}\", which is not declared.",
                        form.id, form.contract
                    ),
                );
                continue;
            };
            let inputs = self.slots(contract, false);
            let input_case = self.contract_case(contract);
            let name = camel(&form.id);
            // Table entry.
            let fields: Vec<String> = form
                .fields
                .iter()
                .map(|f| {
                    let mut items = vec![
                        format!("name: {}", sq(&self.case.name(&f.name))),
                        format!("input: {}", sq(&input_case.name(&f.input))),
                        format!("label: {}", sq(f.label.as_deref().unwrap_or(&f.name))),
                        format!("control: {}", sq(&f.control)),
                    ];
                    if f.derive.is_some() {
                        items.push(format!(
                            "derived: {}",
                            sq(if f.until.as_deref() == Some("edited") {
                                "until-edited"
                            } else {
                                "always"
                            })
                        ));
                    }
                    format!("{{ {} }}", items.join(", "))
                })
                .collect();
            let success = form.on_success.as_ref();
            table.push(format!(
                "  {}: {{ screen: {}, contract: {}, submitLabel: {}, show: {}, navigates: {}, fields: [\n    {},\n  ] }},",
                sq(&form.id),
                sq(&form.screen),
                sq(&form.contract),
                sq(form.submit_label.as_deref().unwrap_or("")),
                sq(form.on_error.as_ref().and_then(|e| e.show.as_deref()).unwrap_or("inline")),
                success.and_then(|s| s.navigate.as_ref()).is_some(),
                fields.join(",\n    ")
            ));

            let mut roots = self.screen_roots(screen);
            let mut lines = Vec::new();

            // initial
            let mut initial = Vec::new();
            for f in &form.fields {
                let wire = self.case.name(&f.name);
                let value = match (f.control.as_str(), &f.options, &f.options_from) {
                    ("checkbox", _, _) => "false".to_string(),
                    ("number", _, _) => "null".to_string(),
                    (_, Some(options), _) => options
                        .first()
                        .map_or("''".to_string(), |o| json_js(&o.value)),
                    (_, _, Some(_)) => {
                        format!("this.options{}(view)[0]?.value ?? ''", member(&wire))
                    }
                    _ => "''".to_string(),
                };
                initial.push(format!("{}: {value}", key(&wire)));
            }
            lines.push(format!(
                "  /** @param {{any}} view */\n  initial(view) {{\n    return this.derive({{ {} }}, [], {{ view }});\n  }},",
                initial.join(", ")
            ));

            // options
            let mut options = Vec::new();
            for f in &form.fields {
                let fname = key(&self.case.name(&f.name));
                if let Some(list) = &f.options {
                    let items: Vec<String> = list
                        .iter()
                        .map(|o| {
                            format!(
                                "{{ value: {}, label: {} }}",
                                json_js(&o.value),
                                sq(&o.label)
                            )
                        })
                        .collect();
                    options.push(format!(
                        "    {fname}: (/** @type {{any}} */ _view) => [{}],",
                        items.join(", ")
                    ));
                } else if let Some(from) = &f.options_from {
                    let Some((js, t)) = self.view_path(screen, &from.path, "view") else {
                        self.refuse(&form.id, format!("Field \"{}\" of form \"{}\" takes its options from \"{}\", which is not a path into the view model of screen \"{}\".", f.name, form.id, from.path, screen.id));
                        continue;
                    };
                    let item = Self::list_item(&t).cloned();
                    let pair = match (&from.value, &from.label, item) {
                        (Some(v), Some(l), Some(item)) => {
                            let scope = UiScope {
                                spec: self.spec,
                                cases: &self.cases,
                                roots: vec![],
                            };
                            let slots = scope.struct_slots(&item).unwrap_or_default();
                            let wire_of = |n: &str| {
                                slots
                                    .iter()
                                    .find(|s| s.name == n)
                                    .map_or_else(|| self.case.name(n), |s| s.wire.clone())
                            };
                            format!(
                                "{{ value: o{}, label: String(o{}) }}",
                                member(&wire_of(v)),
                                member(&wire_of(l))
                            )
                        }
                        _ => "{ value: o, label: String(o) }".to_string(),
                    };
                    options.push(format!("    {fname}: (/** @type {{any}} */ view) => (({js}) ?? []).map((/** @type {{any}} */ o) => ({pair})),"));
                }
            }
            lines.push(format!("  options: {{\n{}\n  }},", options.join("\n")));

            // derive
            let form_root = self.form_root(form, "next");
            let mut derive_lines = Vec::new();
            for f in &form.fields {
                let Some(src) = &f.derive else { continue };
                let wire = self.case.name(&f.name);
                let mut droots = roots.clone();
                droots.push(form_root.clone());
                let what = format!(
                    "The derivation of field \"{}\" of form \"{}\"",
                    f.name, form.id
                );
                let Some(js) = self.compile(&form.id, &what, src, &droots, &[]) else {
                    continue;
                };
                let js = js.replace("scope.", "context.");
                let value = if f.transform.as_deref() == Some("slug") {
                    format!("slug(text({js}))")
                } else {
                    js
                };
                let guard = if f.until.as_deref() == Some("edited") {
                    format!("if (!edited.includes({})) ", sq(&wire))
                } else {
                    String::new()
                };
                derive_lines.push(format!("    {guard}next{} = {value};", member(&wire)));
            }
            lines.push(format!(
                "  /**\n   * @param {{Record<string, any>}} values\n   * @param {{string[]}} {}\n   * @param {{any}} {}\n   */\n  derive(values, {}, {}) {{\n    const next = {{ ...values }};\n{}    return next;\n  }},",
                if derive_lines.iter().any(|l| l.contains("edited")) { "edited" } else { "_edited" },
                if derive_lines.iter().any(|l| l.contains("context.")) { "context" } else { "_context" },
                if derive_lines.iter().any(|l| l.contains("edited")) { "edited" } else { "_edited" },
                if derive_lines.iter().any(|l| l.contains("context.")) { "context" } else { "_context" },
                derive_lines.iter().map(|l| format!("{l}\n")).collect::<String>()
            ));

            // validate
            let mut checks = Vec::new();
            for f in &form.fields {
                let Some(input) = inputs.iter().find(|i| i.name == f.input) else {
                    self.refuse(&form.id, format!("Field \"{}\" of form \"{}\" fills input \"{}\", which contract \"{}\" does not declare.", f.name, form.id, f.input, form.contract));
                    continue;
                };
                let wire = self.case.name(&f.name);
                for v in &self.spec.constructs.validations {
                    if v.contract.as_deref() != Some(form.contract.as_str())
                        || v.input.as_deref() != Some(f.input.as_str())
                    {
                        continue;
                    }
                    let message = sq(v
                        .message
                        .as_deref()
                        .unwrap_or("This value is not accepted."));
                    let value = format!("values{}", member(&wire));
                    let fails = match v.rule {
                        Rule::Required => Some(format!("empty({value})")),
                        Rule::Pattern => v.pattern.as_ref().map(|p| {
                            format!("!new RegExp({}).test(String({value}))", sq(&format!("^(?:{p})$")))
                        }),
                        Rule::MaxLength => v.max_length.as_ref().map(|n| match v.unit {
                            Some(Unit::Bytes) => format!("new TextEncoder().encode(String({value})).length > {n}"),
                            _ => format!("(Array.isArray({value}) ? {value}.length : [...String({value})].length) > {n}"),
                        }),
                        Rule::Range => {
                            let mut parts = Vec::new();
                            if let Some(min) = &v.min {
                                parts.push(format!("Number({value}) < {min}"));
                            }
                            if let Some(max) = &v.max {
                                parts.push(format!("Number({value}) > {max}"));
                            }
                            (!parts.is_empty()).then(|| parts.join(" || "))
                        }
                        Rule::OneOf => Some(format!(
                            "![{}].includes({value})",
                            v.values.iter().map(json_js).collect::<Vec<_>>().join(", ")
                        )),
                        Rule::Custom => {
                            let Some(src) = &v.expr else { continue };
                            // `value` is the validated input; `request.<input>` the other inputs,
                            // read from the fields that fill them.
                            let request: Vec<Slot> = form
                                .fields
                                .iter()
                                .filter_map(|g| {
                                    let i = inputs.iter().find(|i| i.name == g.input)?;
                                    Some(Slot {
                                        name: i.name.clone(),
                                        wire: self.case.name(&g.name),
                                        ty: i.ty.clone(),
                                        optional: i.optional,
                                    })
                                })
                                .collect();
                            let vroots = [
                                Root { name: "request", js: "values".into(), slots: request, direct: None },
                                Root { name: "value", js: value.clone(), slots: vec![], direct: Some(input.ty.clone()) },
                            ];
                            let what = format!("The expression of validation \"{}\"", v.id);
                            self.compile(&v.id, &what, src, &vroots, &[Ty::Bool]).map(|js| format!("!({js})"))
                        }
                    };
                    let Some(fails) = fails else { continue };
                    // A rule other than required does not apply to an optional input left empty.
                    let applies = if v.rule == Rule::Required || !input.optional {
                        String::new()
                    } else {
                        format!("!empty({value}) && ")
                    };
                    checks.push(format!(
                        "    if (errors{m} === undefined && {applies}({fails})) errors{m} = {message};",
                        m = member(&wire)
                    ));
                }
            }
            lines.push(format!(
                "  /** @param {{Record<string, any>}} {} */\n  validate({}) {{\n    /** @type {{Record<string, string>}} */\n    const errors = {{}};\n{}    return errors;\n  }},",
                if checks.is_empty() { "_values" } else { "values" },
                if checks.is_empty() { "_values" } else { "values" },
                checks.iter().map(|l| format!("{l}\n")).collect::<String>()
            ));

            // inputs
            let mut items = Vec::new();
            for f in &form.fields {
                items.push(format!(
                    "{}: values{}",
                    key(&input_case.name(&f.input)),
                    member(&self.case.name(&f.name))
                ));
            }
            let mut aroots = roots.clone();
            aroots.push(self.form_root(form, "values"));
            for (input, src) in &form.arguments {
                let Some(slot) = inputs.iter().find(|i| i.name == *input) else {
                    self.refuse(&form.id, format!("Form \"{}\" gives argument \"{input}\", which contract \"{}\" does not declare.", form.id, form.contract));
                    continue;
                };
                let what = format!("Argument \"{input}\" of form \"{}\"", form.id);
                let types = candidates(self.spec, &slot.ty);
                if let Some(js) = self.compile(&form.id, &what, src, &aroots, &types) {
                    items.push(format!("{}: {js}", key(&slot.wire)));
                }
            }
            let uses_scope = items.iter().any(|i| i.contains("scope."));
            lines.push(format!(
                "  /**\n   * @param {{Record<string, any>}} values\n   * @param {{any}} {}\n   */\n  inputs(values, {}) {{\n    return {{ {} }};\n  }},",
                if uses_scope { "scope" } else { "_scope" },
                if uses_scope { "scope" } else { "_scope" },
                items.join(", ")
            ));

            // succeeded
            roots.push(self.form_root(form, "scope.form"));
            roots.push(self.response_root(&form.contract));
            let outcome = self.outcome(&form.id, &format!("form \"{}\"", form.id), success, &roots);
            lines.push(outcome);

            code.push(format!(
                "/** Form `{}` of screen `{}`: calls `{}`. */\nconst {name} = {{\n{}\n}};\n",
                form.id,
                form.screen,
                form.contract,
                lines.join("\n")
            ));
            cases.push(format!("  {}: {name},", sq(&form.id)));
        }
        format!(
            r#"// @ts-check
// The forms: their fields, initial values, choices, derived fields, client-side validation,
// the inputs a submission sends and what a success does. Values are named by field, in the wire
// case; `edited` lists the derived fields the person has edited.

import {{ hrefFor }} from './routes.js';

/**
 * A field: the input it fills, its accessible name, its control, and whether it is derived.
 * @typedef {{{{ name: string, input: string, label: string, control: string, derived?: 'always' | 'until-edited' }}}} FormField
 * @typedef {{{{ screen: string, contract: string, submitLabel: string, show: string, navigates: boolean, fields: FormField[] }}}} FormInfo
 */

/** @type {{Record<string, FormInfo>}} */
export const FORMS = {{
{table}
}};

/**
 * A value as text: empty when absent.
 * @param {{unknown}} value
 * @returns {{string}}
 */
function text(value) {{
  return value == null ? '' : String(value);
}}

/**
 * True when a value is absent, an empty text or an empty list.
 * @param {{unknown}} value
 * @returns {{boolean}}
 */
function empty(value) {{
  return value == null || value === '' || (Array.isArray(value) && value.length === 0);
}}

/**
 * A text as an address segment: Unicode NFKD, combining marks removed, lower case, every run of
 * characters other than a-z and 0-9 replaced by `-`, `-` trimmed at both ends, cut to 80
 * characters, a trailing `-` trimmed again.
 * @param {{string}} value
 * @returns {{string}}
 */
export function slug(value) {{
  return value
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 80)
    .replace(/-+$/, '');
}}

/**
 * Where a success goes: `path` when it is a same-site absolute path, else the fallback.
 * @param {{unknown}} path
 * @param {{string}} fallback
 * @returns {{string}}
 */
function sameSite(path, fallback) {{
  return typeof path === 'string' && path.startsWith('/') && !path.startsWith('//') ? path : fallback;
}}

{code}
/** @type {{Record<string, any>}} */
const CODE = {{
{cases}
}};

/**
 * @param {{string}} form
 * @returns {{any}}
 */
function of(form) {{
  const code = CODE[form];
  if (!code) throw new Error(`there is no form ${{form}}`);
  return code;
}}

/**
 * The values of a new form on a screen with this view model: empty texts, unchecked boxes, the
 * first choice of a select or radio, and the derived fields derived.
 * @param {{string}} form
 * @param {{any}} view
 * @returns {{Record<string, any>}}
 */
export function initialValues(form, view) {{
  return of(form).initial(view);
}}

/**
 * The choices of a select or radio field.
 * @param {{string}} form
 * @param {{string}} field
 * @param {{any}} view
 * @returns {{{{ value: any, label: string }}[]}}
 */
export function options(form, field, view) {{
  const make = of(form).options[field];
  return make ? make(view) : [];
}}

/**
 * A change to a field: the new values, with the derived fields derived again, and the derived
 * fields the person has now edited.
 * @param {{string}} form
 * @param {{Record<string, any>}} values
 * @param {{string[]}} edited
 * @param {{string}} name
 * @param {{unknown}} value
 * @param {{any}} [context] The screen's scope, which derivations may read.
 * @returns {{{{ values: Record<string, any>, edited: string[] }}}}
 */
export function input(form, values, edited, name, value, context = {{}}) {{
  const field = FORMS[form]?.fields.find((f) => f.name === name);
  if (!field) return {{ values, edited }};
  const now = field.derived === 'until-edited' && !edited.includes(name) ? [...edited, name] : edited;
  return {{ values: of(form).derive({{ ...values, [name]: value }}, now, context), edited: now }};
}}

/**
 * The client-side checks of a form: field -> the message of its first failing validation.
 * @param {{string}} form
 * @param {{Record<string, any>}} values
 * @returns {{Record<string, string>}}
 */
export function validate(form, values) {{
  return of(form).validate(values);
}}

/**
 * The inputs a submission sends: the fields and the arguments.
 * @param {{string}} form
 * @param {{Record<string, any>}} values
 * @param {{any}} scope
 * @returns {{Record<string, any>}}
 */
export function inputs(form, values, scope) {{
  return of(form).inputs(values, scope);
}}

/**
 * What an accepted submission does: the address to open (or null) and the message to show (or
 * null). `scope.form` holds the submitted values and `scope.response` the outputs.
 * @param {{string}} form
 * @param {{any}} scope
 * @returns {{{{ navigate: string | null, toast: string | null }}}}
 */
export function succeeded(form, scope) {{
  return of(form).succeeded(scope);
}}
"#,
            table = table.join("\n"),
            code = code.join("\n"),
            cases = cases.join("\n"),
        )
    }

    /// The `succeeded(scope)` method of a form or an action.
    fn outcome(
        &mut self,
        entity: &str,
        what: &str,
        outcome: Option<&Outcome>,
        roots: &[Root],
    ) -> String {
        let mut navigate = "null".to_string();
        let mut toast = "null".to_string();
        if let Some(o) = outcome {
            if let Some(n) = &o.navigate {
                let mut params = Vec::new();
                let target = n.screen.as_deref().and_then(|s| self.screen(s));
                for (param, src) in &n.params {
                    let w = format!("Navigation parameter \"{param}\" of {what}");
                    if let Some(js) = self.compile(entity, &w, src, roots, &[]) {
                        params.push(format!("{}: {js}", key(param)));
                    }
                }
                let fallback = match target {
                    Some(t) => format!("hrefFor({}, {{ {} }})", sq(&t.id), params.join(", "))
                        .replace("{  }", "{}"),
                    None => "null".to_string(),
                };
                navigate = match &n.path {
                    Some(src) => {
                        let w = format!("The navigation address of {what}");
                        match self.compile(entity, &w, src, roots, &[]) {
                            Some(js) => format!("sameSite({js}, {fallback})"),
                            None => fallback,
                        }
                    }
                    None => fallback,
                };
            }
            if let Some(t) = &o.toast {
                if let Some(js) = self.template(entity, &format!("The toast of {what}"), t, roots) {
                    toast = js;
                }
            }
        }
        let uses = navigate.contains("scope") || toast.contains("scope");
        format!(
            "  /** @param {{any}} {s} */\n  succeeded({s}) {{\n    return {{ navigate: {navigate}, toast: {toast} }};\n  }},",
            s = if uses { "scope" } else { "_scope" }
        )
    }

    // -----------------------------------------------------------------------------------------
    // actions.js

    fn actions(&mut self) -> String {
        let mut table = Vec::new();
        let mut code = Vec::new();
        let mut cases = Vec::new();
        for a in &self.ui.actions {
            let Some(screen) = self.screen(&a.screen) else {
                self.refuse(
                    &a.id,
                    format!(
                        "Action \"{}\" names screen \"{}\", which is not declared.",
                        a.id, a.screen
                    ),
                );
                continue;
            };
            let Some(contract) = self.contract(&a.contract) else {
                self.refuse(
                    &a.id,
                    format!(
                        "Action \"{}\" names contract \"{}\", which is not declared.",
                        a.id, a.contract
                    ),
                );
                continue;
            };
            let inputs = self.slots(contract, false);
            let name = camel(&a.id);
            let mut roots = self.screen_roots(screen);
            let mut lines = Vec::new();

            // rows and item
            let mut item_slots = Vec::new();
            if let Some(path) = &a.for_each {
                match self.view_path(screen, path, "view") {
                    Some((js, t)) => {
                        let scope = UiScope {
                            spec: self.spec,
                            cases: &self.cases,
                            roots: vec![],
                        };
                        item_slots = Self::list_item(&t)
                            .and_then(|i| scope.struct_slots(i))
                            .unwrap_or_default();
                        lines.push(format!(
                            "  /** @param {{any}} view @returns {{any[]}} */\n  rows(view) {{\n    return ({js}) ?? [];\n  }},"
                        ));
                    }
                    None => {
                        self.refuse(&a.id, format!("Action \"{}\" repeats over \"{path}\", which is not a path into the view model of screen \"{}\".", a.id, screen.id));
                    }
                }
            } else {
                lines.push("  /** @param {any} _view @returns {any[]} */\n  rows(_view) {\n    return [];\n  },".to_string());
            }
            roots.push(Root {
                name: "item",
                js: "scope.item".into(),
                slots: item_slots.clone(),
                direct: None,
            });
            if let Some(form_id) = &a.form {
                match self.ui.forms.iter().find(|f| f.id == *form_id) {
                    Some(form) => roots.push(self.form_root(form, "scope.form")),
                    None => self.refuse(
                        &a.id,
                        format!(
                            "Action \"{}\" reads form \"{form_id}\", which is not declared.",
                            a.id
                        ),
                    ),
                }
            }

            // label and confirm
            let item_roots = vec![Root {
                name: "item",
                js: "item".into(),
                slots: item_slots.clone(),
                direct: None,
            }];
            let label = self
                .template(
                    &a.id,
                    &format!("The label of action \"{}\"", a.id),
                    &a.label,
                    &item_roots,
                )
                .unwrap_or_else(|| "''".into());
            let confirm = match &a.confirm {
                Some(c) => self
                    .template(
                        &a.id,
                        &format!("The confirmation of action \"{}\"", a.id),
                        c,
                        &item_roots,
                    )
                    .unwrap_or_else(|| "''".into()),
                None => "null".to_string(),
            };
            let item_param = |js: &str| if js.contains("item") { "item" } else { "_item" };
            lines.push(format!(
                "  /** @param {{any}} {p} */\n  label({p}) {{\n    return {label};\n  }},",
                p = item_param(&label)
            ));
            lines.push(format!(
                "  /** @param {{any}} {p} @returns {{string | null}} */\n  confirmText({p}) {{\n    return {confirm};\n  }},",
                p = item_param(&confirm)
            ));

            // args
            let mut items = Vec::new();
            for (input, src) in &a.arguments {
                let Some(slot) = inputs.iter().find(|i| i.name == *input) else {
                    self.refuse(&a.id, format!("Action \"{}\" gives argument \"{input}\", which contract \"{}\" does not declare.", a.id, a.contract));
                    continue;
                };
                let what = format!("Argument \"{input}\" of action \"{}\"", a.id);
                let types = candidates(self.spec, &slot.ty);
                if let Some(js) = self.compile(&a.id, &what, src, &roots, &types) {
                    items.push(format!("{}: {js}", key(&slot.wire)));
                }
            }
            let uses = items.iter().any(|i| i.contains("scope"));
            lines.push(format!(
                "  /** @param {{any}} {s} */\n  args({s}) {{\n    return {};\n  }},",
                if items.is_empty() {
                    "{}".to_string()
                } else {
                    format!("{{ {} }}", items.join(", "))
                },
                s = if uses { "scope" } else { "_scope" }
            ));

            // visible
            lines.push(self.visibility(a, contract, &inputs));

            // succeeded
            roots.push(self.response_root(&a.contract));
            let what = format!("action \"{}\"", a.id);
            lines.push(self.outcome(&a.id, &what, a.on_success.as_ref(), &roots));

            // result
            let place = match &a.result {
                Some(r) => {
                    let wire = contract
                        .outputs
                        .iter()
                        .find(|o| o.name == r.output)
                        .map(|o| self.contract_case(contract).name(&o.name));
                    match wire {
                        Some(w) => format!(
                            "{{ ...local, {}: outputs{} }}",
                            key(&r.into),
                            optional_member(&w)
                        ),
                        None => {
                            self.refuse(&a.id, format!("Action \"{}\" keeps output \"{}\", which contract \"{}\" does not declare.", a.id, r.output, a.contract));
                            "local".into()
                        }
                    }
                }
                None => "local".to_string(),
            };
            lines.push(format!(
                "  /**\n   * @param {{Record<string, any>}} local\n   * @param {{any}} {o}\n   */\n  placeResult(local, {o}) {{\n    return {place};\n  }},",
                o = if place.contains("outputs") { "outputs" } else { "_outputs" }
            ));

            let result = a.result.as_ref().map_or("null".to_string(), |r| {
                format!("{{ into: {}, output: {} }}", sq(&r.into), sq(&r.output))
            });
            table.push(format!(
                "  {}: {{ screen: {}, contract: {}, forEach: {}, form: {}, confirm: {}, optimistic: {}, result: {result}, show: {}, navigates: {} }},",
                sq(&a.id),
                sq(&a.screen),
                sq(&a.contract),
                a.for_each.as_deref().map_or("null".into(), sq),
                a.form.as_deref().map_or("null".into(), sq),
                a.confirm.is_some(),
                a.optimistic.unwrap_or(false),
                sq(a.on_error.as_ref().and_then(|e| e.show.as_deref()).unwrap_or("toast")),
                a.on_success.as_ref().and_then(|s| s.navigate.as_ref()).is_some(),
            ));
            code.push(format!(
                "/** Action `{}` of screen `{}`: calls `{}`. */\nconst {name} = {{\n{}\n}};\n",
                a.id,
                a.screen,
                a.contract,
                lines.join("\n")
            ));
            cases.push(format!("  {}: {name},", sq(&a.id)));
        }
        format!(
            r#"// @ts-check
// The actions: buttons that call a contract, per row of a list or once, with a confirmation, a
// visibility decided by the policies on the contract, and what a success does.

import {{ hrefFor }} from './routes.js';

/**
 * @typedef {{{{ screen: string, contract: string, forEach: string | null, form: string | null, confirm: boolean, optimistic: boolean, result: {{ into: string, output: string }} | null, show: string, navigates: boolean }}}} ActionInfo
 */

/** @type {{Record<string, ActionInfo>}} */
export const ACTIONS = {{
{table}
}};

/**
 * A value as text: empty when absent.
 * @param {{unknown}} value
 * @returns {{string}}
 */
function text(value) {{
  return value == null ? '' : String(value);
}}

/**
 * Where a success goes: `path` when it is a same-site absolute path, else the fallback.
 * @param {{unknown}} path
 * @param {{string}} fallback
 * @returns {{string}}
 */
function sameSite(path, fallback) {{
  return typeof path === 'string' && path.startsWith('/') && !path.startsWith('//') ? path : fallback;
}}

/**
 * True when the principal is of this kind and holds this role (either left out: any).
 * @param {{any}} principal
 * @param {{string | null}} kind
 * @param {{string | null}} role
 * @returns {{boolean}}
 */
function subject(principal, kind, role) {{
  return principal != null && (kind === null || principal.kind === kind) && (role === null || (principal.roles ?? []).includes(role));
}}

{code}
/** @type {{Record<string, any>}} */
const CODE = {{
{cases}
}};

/**
 * @param {{string}} action
 * @returns {{any}}
 */
function of(action) {{
  const code = CODE[action];
  if (!code) throw new Error(`there is no action ${{action}}`);
  return code;
}}

/**
 * The actions of a screen, in declaration order.
 * @param {{string}} screen
 * @returns {{string[]}}
 */
export function actionsOn(screen) {{
  return Object.keys(ACTIONS).filter((id) => ACTIONS[id].screen === screen);
}}

/**
 * The rows a per-row action is repeated for (none for another action).
 * @param {{string}} action
 * @param {{any}} view
 * @returns {{any[]}}
 */
export function rows(action, view) {{
  return of(action).rows(view);
}}

/**
 * The accessible name of an action's button, filled in with the row.
 * @param {{string}} action
 * @param {{any}} [item]
 * @returns {{string}}
 */
export function label(action, item) {{
  return of(action).label(item);
}}

/**
 * The confirmation an action asks for first, filled in with the row, or null.
 * @param {{string}} action
 * @param {{any}} [item]
 * @returns {{string | null}}
 */
export function confirmText(action, item) {{
  return of(action).confirmText(item);
}}

/**
 * The inputs of the call an action makes.
 * @param {{string}} action
 * @param {{any}} scope
 * @returns {{Record<string, any>}}
 */
export function args(action, scope) {{
  return of(action).args(scope);
}}

/**
 * Whether the policies on the action's contract may permit the principal: a matching deny
 * refuses, else a matching allow permits, else the contract's policyDefault (permit when it
 * states none). A condition that reads the record counts as matching.
 * @param {{string}} action
 * @param {{any}} scope
 * @param {{Record<string, any>}} inputs
 * @returns {{boolean}}
 */
export function visible(action, scope, inputs) {{
  return of(action).visible(scope, inputs);
}}

/**
 * What a success does: the address to open (or null) and the message to show (or null).
 * @param {{string}} action
 * @param {{any}} scope
 * @returns {{{{ navigate: string | null, toast: string | null }}}}
 */
export function succeeded(action, scope) {{
  return of(action).succeeded(scope);
}}

/**
 * The screen-local values after a success: the output an action keeps placed in its value.
 * @param {{string}} action
 * @param {{Record<string, any>}} local
 * @param {{any}} outputs
 * @returns {{Record<string, any>}}
 */
export function placeResult(action, local, outputs) {{
  return of(action).placeResult(local, outputs);
}}
"#,
            table = table.join("\n"),
            code = code.join("\n"),
            cases = cases.join("\n"),
        )
    }

    /// The `visible(scope, inputs)` method of an action: the policy decision of its contract.
    fn visibility(&mut self, a: &Action, contract: &Contract, inputs: &[Slot]) -> String {
        let policies: Vec<_> = self
            .spec
            .constructs
            .policies
            .iter()
            .filter(|p| p.contract.as_deref() == Some(contract.id.as_str()))
            .cloned()
            .collect();
        let roots = vec![
            Root {
                name: "request",
                js: "inputs".into(),
                slots: inputs.to_vec(),
                direct: None,
            },
            Root {
                name: "principal",
                js: "scope.principal".into(),
                slots: ["id", "tenant", "role"]
                    .iter()
                    .map(|n| Slot {
                        name: (*n).into(),
                        wire: (*n).into(),
                        ty: TypeRef::parse("string"),
                        optional: false,
                    })
                    .collect(),
                direct: None,
            },
        ];
        let mut lines = Vec::new();
        for verdict in [Verdict::Deny, Verdict::Allow] {
            for p in policies.iter().filter(|p| p.effect == Some(verdict)) {
                let condition = match &p.condition {
                    None => "true".to_string(),
                    Some(src) => {
                        let reads_record = expr::parse(src)
                            .map(|e| {
                                let mut refs = Vec::new();
                                references(&e, &mut refs);
                                refs.iter()
                                    .any(|r| r.first().map(String::as_str) == Some("record"))
                            })
                            .unwrap_or(false);
                        if reads_record {
                            "true".to_string()
                        } else {
                            let what = format!("The condition of policy \"{}\"", p.id);
                            self.compile(&a.id, &what, src, &roots, &[Ty::Bool])
                                .unwrap_or_else(|| "true".into())
                        }
                    }
                };
                let opt = |v: &Option<String>| v.as_deref().map_or("null".to_string(), sq);
                lines.push(format!(
                    "    // policy {}\n    if (subject(scope.principal, {}, {}) && ({condition})) return {};",
                    p.id,
                    opt(&p.principal),
                    opt(&p.role),
                    verdict == Verdict::Allow
                ));
            }
        }
        let default = !matches!(
            self.spec.constructs.policy_default(&contract.id),
            Some(Verdict::Deny)
        );
        let body = lines.join("\n");
        let s = if body.contains("scope") {
            "scope"
        } else {
            "_scope"
        };
        let i = if body.contains("inputs") {
            "inputs"
        } else {
            "_inputs"
        };
        format!(
            "  /**\n   * @param {{any}} {s}\n   * @param {{Record<string, any>}} {i}\n   * @returns {{boolean}}\n   */\n  visible({s}, {i}) {{\n{}    return {default};\n  }},",
            if body.is_empty() { String::new() } else { format!("{body}\n") }
        )
    }

    // -----------------------------------------------------------------------------------------
    // live.js

    fn live(&mut self) -> String {
        let mut table = Vec::new();
        let mut by_screen: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let mut order: Vec<&str> = Vec::new();
        for b in &self.ui.live_bindings {
            let Some(screen) = self.screen(&b.screen) else {
                self.refuse(
                    &b.id,
                    format!(
                        "Live binding \"{}\" names screen \"{}\", which is not declared.",
                        b.id, b.screen
                    ),
                );
                continue;
            };
            let Some(event) = self.spec.event(&b.event) else {
                self.refuse(
                    &b.id,
                    format!(
                        "Live binding \"{}\" names event \"{}\", which is not declared.",
                        b.id, b.event
                    ),
                );
                continue;
            };
            table.push(format!(
                "  {{ id: {}, screen: {}, event: {}, refresh: {} }},",
                sq(&b.id),
                sq(&b.screen),
                sq(&b.event),
                sq(&b.refresh)
            ));
            let case = wire::event_case(self.spec, event);
            let payload: Vec<Slot> = event
                .payload
                .iter()
                .map(|f| Slot {
                    name: f.name.clone(),
                    wire: case.name(&f.name),
                    ty: TypeRef::parse(f.type_name.as_deref().unwrap_or("json")),
                    optional: f.optional,
                })
                .collect();
            let mut roots = self.screen_roots(screen);
            for r in &mut roots {
                if r.name == "view" {
                    r.js = "view".into();
                }
            }
            roots.push(Root {
                name: "event",
                js: "payload".into(),
                slots: payload,
                direct: None,
            });
            let condition = match &b.when {
                None => "true".to_string(),
                Some(w) => self
                    .condition(&b.id, screen, w, &roots)
                    .unwrap_or_else(|| "false".into()),
            };
            if !order.contains(&b.screen.as_str()) {
                order.push(&b.screen);
            }
            by_screen.entry(&b.screen).or_default().push(format!(
                "        case {}:\n          return {condition};",
                sq(&b.event)
            ));
        }
        let mut cases = Vec::new();
        for s in order {
            cases.push(format!(
                "    case {}:\n      switch (event) {{\n{}\n        default:\n          return false;\n      }}",
                sq(s),
                by_screen[s].join("\n")
            ));
        }
        let body = cases.join("\n");
        format!(
            r"// @ts-check
// The live bindings: which events refresh which screen's view model, and when.

/**
 * @typedef {{{{ id: string, screen: string, event: string, refresh: string }}}} Binding
 */

/** @type {{Binding[]}} */
export const BINDINGS = [
{}
];

/**
 * Whether an occurrence of `event` with this payload refreshes the view model of `screen`.
 * `refresh` of the binding says what to refresh (`whole`, or a path into the view model).
 * @param {{string}} screen
 * @param {{string}} event
 * @param {{any}} {p}
 * @param {{any}} {v}
 * @returns {{boolean}}
 */
export function refreshes(screen, event, {p}, {v}) {{
  switch (screen) {{
{body}
    default:
      return false;
  }}
}}
",
            table.join("\n"),
            p = if body.contains("payload") {
                "payload"
            } else {
                "_payload"
            },
            v = if body.contains("view") {
                "view"
            } else {
                "_view"
            },
        )
    }

    /// A live binding's `when`: an expression, `{value, in}`, or `{any}` / `{all}`.
    fn condition(
        &mut self,
        entity: &str,
        screen: &Screen,
        w: &Value,
        roots: &[Root],
    ) -> Option<String> {
        match w {
            Value::String(src) => {
                let js = self.compile(
                    entity,
                    "A condition of the binding",
                    src,
                    roots,
                    &[Ty::Bool],
                )?;
                Some(format!("({js})"))
            }
            Value::Object(map) => {
                if let (Some(Value::String(value)), Some(Value::String(path))) =
                    (map.get("value"), map.get("in"))
                {
                    let v = self.compile(
                        entity,
                        "A membership value of the binding",
                        value,
                        roots,
                        &[],
                    )?;
                    let Some((list, _)) = self.view_path(screen, path, "view") else {
                        self.refuse(entity, format!("Live binding \"{entity}\" tests membership in \"{path}\", which is not a path into the view model of screen \"{}\".", screen.id));
                        return None;
                    };
                    return Some(format!("(({list}) ?? []).includes({v})"));
                }
                for (k, op) in [("any", " || "), ("all", " && ")] {
                    if let Some(Value::Array(items)) = map.get(k) {
                        let parts: Option<Vec<String>> = items
                            .iter()
                            .map(|i| self.condition(entity, screen, i, roots))
                            .collect();
                        return Some(format!("({})", parts?.join(op)));
                    }
                }
                self.refuse(entity, format!("Live binding \"{entity}\" has a condition that is not an expression, {{value, in}}, {{any}} or {{all}}."));
                None
            }
            _ => {
                self.refuse(entity, format!("Live binding \"{entity}\" has a condition that is not an expression, {{value, in}}, {{any}} or {{all}}."));
                None
            }
        }
    }
}

/// The parameter names of a route pattern.
fn route_params(route: &str) -> Vec<String> {
    route
        .split('/')
        .filter_map(|s| s.strip_prefix(':'))
        .map(str::to_string)
        .collect()
}

/// A JSON scalar as a JavaScript literal.
fn json_js(v: &Value) -> String {
    match v {
        Value::String(s) => sq(s),
        other => other.to_string(),
    }
}
