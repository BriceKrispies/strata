//! The contract conformance suite (WG-5182): `specc conformance` writes `contracts.rs`, one Rust
//! test per contract that is `deployment: shipped` and names a `bffRoute`, which drives the
//! contract through the running BFF and backend with the hand-written harness of
//! `crates/bff/tests/conformance/harness.rs` and compares what comes back with the declaration.
//!
//! Everything a test checks follows mechanically from the contract; nothing needs invented data:
//!
//! - **No session** (or no credential): a `session` contract answers `unauthenticated`, an
//!   `anonymous` one answers without an auth refusal.
//! - **Origin and request token** (`auth.origin`, `auth.csrf`): a call without our origin, with
//!   another site's origin, without the request token or with a wrong one is refused `forbidden`,
//!   and refused before the body is read: the body sent is deliberately not JSON.
//! - **Capabilities and roles** (`auth.requires`, every one, and `auth.roles`, any one): for each
//!   required capability a person of the site who lacks it (by the specification's roles) is
//!   refused `forbidden`; so is one who holds none of the roles.
//! - **Tenant**: a session of the other site presented at this site's host is refused, and an
//!   input of type `tenant-id` naming the other site is refused `forbidden`.
//! - **Step-up** (`auth.stepUp`): a session whose authentication was aged on the development stack
//!   is refused with the declared error `step-up-required`.
//! - **Read-only** (`readOnly`): another, state-changing method is refused.
//! - **Quota**: the `limit + 1`th call in the window is `throttled`, for a quota whose limit is at
//!   most [`QUOTA_CALLS`]; a larger one is skipped with its reason.
//! - **Success**: with the contract's example inputs (the vocabulary property
//!   [`EXAMPLE_PROPERTY`], a JSON object as text), or with none when it has no required input, a
//!   person who holds what it asks for gets a reply that is exactly the declared outputs with their
//!   types.
//! - **Every reply**: a refusal is the `errorBody` type with a `code` that is the `kind` of one of
//!   the contract's declared errors and the status `system.generation.errorKinds` gives it; a
//!   success is the declared outputs.
//!
//! A check that applies but cannot be made (no example, a quota too large to exhaust, a scheme the
//! suite holds no credential of) is listed in `SKIPPED` with its reason, which the gate counts and
//! prints; the harness prints the ones it can only find at run time (no person of the site lacks
//! a capability). None is silent.
//!
//! How a request is built from the `bffRoute` follows `crate::web_kernel`: no shape sends the
//! inputs as a JSON body (`POST`) or as the query string (`GET`); `view` is the view endpoint,
//! whose query is `path` (the address of the screen whose `viewModel` is the contract) and the
//! other inputs; `records <op> <object>` is the records endpoint, `{op, object, id, fields,
//! expected_version}`, whose reply is `{id, version, fields}`. A route parameter (`:name`, `*name`)
//! takes the input of that name. A `GET` whose outputs include an `event-stream` is a socket.
//!
//! Contracts whose id starts with `--expect-fail-prefix` are generated as tests that must fail
//! with the rule their id names (`violates-<rule>` must fail with `[<rule>]`): the fixtures of
//! `crates/bff/tests/conformance/fixtures` that show each check can fail.
//!
//! The specification is read as JSON, as the catalogue reads it; the command checks what it
//! reads (routes, types, error kinds, examples) and refuses with every problem.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde_json::{Map, Value};

use crate::codetext::{line_text, rust, Claims};
use crate::diagnostic::{Diagnostic, SPEC_ENTITY};
use crate::emit::{self, FileError, GeneratedFile, HEADER_MARKERS};
use crate::ir::{variant_wire_name, VariantCase};
use crate::jsonc;
use crate::resolve::{Builtin, TypeRef};
use crate::wire::WireCase;

/// Code of a text that is not JSONC.
pub const SYNTAX: &str = "conformance.spec-syntax";
/// Code of a specification that is not a JSON object, or a section with the wrong shape.
pub const SHAPE: &str = "conformance.spec-shape";
/// Code of a `bffRoute` the suite cannot build a request from.
pub const ROUTE: &str = "conformance.route-invalid";
/// Code of a type name that resolves to nothing.
pub const TYPE: &str = "conformance.type-unknown";
/// Code of an example that is not a JSON object of the contract's inputs with values of their
/// types, or that leaves out a required input.
pub const EXAMPLE: &str = "conformance.example-invalid";
/// Code of an error kind that `system.generation.errorKinds` does not declare.
pub const ERROR_KIND: &str = "conformance.error-kind-unknown";

/// The path of the generated file, relative to the output directory.
pub const PATH: &str = "contracts.rs";
/// The vocabulary property of a contract that holds its example inputs.
pub const EXAMPLE_PROPERTY: &str = "conformanceExample";
/// The largest quota limit the suite exhausts; a quota above it is skipped with its reason.
pub const QUOTA_CALLS: u64 = 100;
/// The type of an input that names a tenant.
pub const TENANT_TYPE: &str = "tenant-id";
/// The output type of a contract served as a socket.
pub const SOCKET_TYPE: &str = "event-stream";
/// The name of the declared error a refusal for a stale authentication is.
pub const STEP_UP_ERROR: &str = "step-up-required";
/// The value a route parameter takes when no example gives one.
pub const PLACEHOLDER: &str = "conformance-probe";

/// Options of a generation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Contracts whose id starts with this are generated as tests that must fail.
    pub expect_fail_prefix: Option<String>,
}

/// A check the suite cannot make for a contract.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Skip {
    /// The contract id.
    pub contract: String,
    /// The check (`success`, `quota`, `capability`, ...).
    pub check: String,
    /// Why it is not made.
    pub reason: String,
}

/// The generated file and what it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceOutput {
    /// The files, sorted by path (one).
    pub files: Vec<GeneratedFile>,
    /// The ids of the contracts with a test, in declaration order.
    pub covered: Vec<String>,
    /// The checks that are not made, in declaration order of their contracts.
    pub skipped: Vec<Skip>,
}

// ---- the plan of one contract ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Shape {
    Json,
    View,
    Records { op: String, object: String },
    Socket,
}

#[derive(Debug, Clone)]
struct Input {
    name: String,
    wire: String,
    ty: String,
    optional: bool,
}

#[derive(Debug, Clone, Default)]
struct Request {
    path: String,
    query: Vec<(String, String)>,
    body: Option<String>,
}

#[derive(Debug, Clone)]
enum RecordFrom {
    Id,
    Version,
    Field(String),
}

#[derive(Debug, Clone)]
enum Reply {
    Members(Vec<(String, String, bool)>),
    Records(Vec<(RecordFrom, String, bool)>),
    Socket,
}

#[derive(Debug, Clone)]
struct Plan {
    id: String,
    reference: String,
    route: String,
    method: String,
    shape: Shape,
    scheme: String,
    requires: Vec<String>,
    roles: Vec<String>,
    origin: bool,
    csrf: bool,
    step_up: bool,
    read_only: bool,
    tenant_input: Option<String>,
    probe: Request,
    success: Option<Request>,
    reply: Reply,
    errors: Vec<String>,
    step_up_error: Option<String>,
    quota: Option<u64>,
    expect_fail: Option<String>,
}

/// What the generation reads besides the contracts.
struct World<'a> {
    top: &'a Map<String, Value>,
    case: WireCase,
    types: BTreeMap<&'a str, &'a Value>,
    type_cases: BTreeMap<String, WireCase>,
    error_kinds: Vec<(String, u16)>,
    error_body: Option<String>,
}

// ---- reading --------------------------------------------------------------------------------

fn items<'a>(top: &'a Map<String, Value>, key: &str) -> &'a [Value] {
    match top.get(key) {
        Some(Value::Array(list)) => list.as_slice(),
        _ => &[],
    }
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn texts(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool) == Some(true)
}

/// The wire case of a contract: its `binding.case`, else the system's.
fn contract_case(c: &Value, system: WireCase) -> WireCase {
    c.get("binding")
        .and_then(|b| text(b, "case"))
        .and_then(WireCase::parse)
        .unwrap_or(system)
}

/// The struct and union types a contract payload reaches, each with the case of the first
/// payload that reaches it: the walk of `crate::wire`, over the specification's JSON.
fn type_cases(
    top: &Map<String, Value>,
    types: &BTreeMap<&str, &Value>,
    system: WireCase,
) -> BTreeMap<String, WireCase> {
    let mut roots: Vec<(String, WireCase)> = Vec::new();
    let contracts = items(top, "contracts");
    for c in contracts {
        let case = contract_case(c, system);
        for key in ["inputs", "outputs", "errors"] {
            for f in c.get(key).and_then(Value::as_array).into_iter().flatten() {
                if let Some(t) = text(f, "type") {
                    roots.push((t.to_string(), case));
                }
            }
        }
    }
    for e in items(top, "events") {
        let Some(contract) = text(e, "contract") else {
            continue;
        };
        let case = contracts
            .iter()
            .find(|c| text(c, "id") == Some(contract))
            .map_or(WireCase::Kebab, |c| contract_case(c, system));
        for f in e
            .get("payload")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(t) = text(f, "type") {
                roots.push((t.to_string(), case));
            }
        }
    }
    let mut cases: BTreeMap<String, WireCase> = BTreeMap::new();
    for (root, case) in roots {
        let mut stack = vec![TypeRef::parse(&root)];
        while let Some(t) = stack.pop() {
            match t {
                TypeRef::List(inner)
                | TypeRef::Set(inner)
                | TypeRef::Option(inner)
                | TypeRef::Shared(inner) => stack.push(*inner),
                TypeRef::Map(key, value) => {
                    stack.push(*key);
                    stack.push(*value);
                }
                TypeRef::Builtin(_) => {}
                TypeRef::Declared(id) => {
                    let Some(decl) = types.get(id.as_str()) else {
                        continue;
                    };
                    let fields: Vec<&Value> = match text(decl, "kind") {
                        Some("struct") => decl
                            .get("fields")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().collect())
                            .unwrap_or_default(),
                        Some("union") => decl
                            .get("variants")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .flat_map(|v| {
                                v.get("fields")
                                    .and_then(Value::as_array)
                                    .into_iter()
                                    .flatten()
                            })
                            .collect(),
                        _ => continue,
                    };
                    if cases.contains_key(&id) {
                        continue;
                    }
                    cases.insert(id.clone(), case);
                    stack.extend(
                        fields
                            .iter()
                            .filter_map(|f| text(f, "type"))
                            .map(TypeRef::parse),
                    );
                }
            }
        }
    }
    cases
}

/// Generates the suite from a specification's text, or refuses with every diagnostic.
/// `spec_path` is the path written into the header.
pub fn generate(
    spec_path: &str,
    text_in: &str,
    options: &Options,
) -> Result<ConformanceOutput, Vec<Diagnostic>> {
    let spec_path = spec_path.replace('\\', "/");
    let value = jsonc::parse(text_in).map_err(|e| {
        vec![Diagnostic::new(
            e.code(SYNTAX),
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
    let generation = top.get("system").and_then(|s| s.get("generation"));
    let case = generation
        .and_then(|g| text(g, "wireCase"))
        .and_then(WireCase::parse)
        .unwrap_or_default();
    let types: BTreeMap<&str, &Value> = items(top, "types")
        .iter()
        .filter_map(|t| text(t, "id").map(|id| (id, t)))
        .collect();
    let error_kinds: Vec<(String, u16)> = generation
        .and_then(|g| g.get("errorKinds"))
        .and_then(Value::as_object)
        .map(|kinds| {
            kinds
                .iter()
                .filter_map(|(k, s)| {
                    s.as_u64()
                        .and_then(|s| u16::try_from(s).ok())
                        .map(|s| (k.clone(), s))
                })
                .collect()
        })
        .unwrap_or_default();
    let error_body = generation
        .and_then(|g| g.get("errorBody"))
        .and_then(|b| text(b, "type"))
        .map(str::to_string);
    let world = World {
        top,
        case,
        type_cases: type_cases(top, &types, case),
        types,
        error_kinds,
        error_body,
    };
    let mut diagnostics = Vec::new();
    let mut plans = Vec::new();
    for c in items(top, "contracts") {
        if text(c, "deployment") != Some("shipped") {
            continue;
        }
        let Some(route) = text(c, "bffRoute") else {
            continue;
        };
        match plan(&world, c, route, options) {
            Ok(p) => plans.push(p),
            Err(mut d) => diagnostics.append(&mut d),
        }
    }
    // The types the replies and the error body reach, each with its wire names.
    let mut wanted: Vec<String> = Vec::new();
    for p in &plans {
        match &p.reply {
            Reply::Members(fields) => wanted.extend(fields.iter().map(|(_, t, _)| t.clone())),
            Reply::Records(fields) => wanted.extend(fields.iter().map(|(_, t, _)| t.clone())),
            Reply::Socket => {}
        }
    }
    if let Some(body) = &world.error_body {
        wanted.push(body.clone());
    }
    let decls = match closure(&world, &wanted) {
        Ok(decls) => decls,
        Err(mut d) => {
            diagnostics.append(&mut d);
            BTreeMap::new()
        }
    };
    if !diagnostics.is_empty() {
        diagnostics.sort();
        diagnostics.dedup();
        return Err(diagnostics);
    }
    let skipped = skips(&plans);
    let body = render(&world, &plans, &decls, &skipped)?;
    let header = format!(
        "{} from {}. Do not edit; change the specification and regenerate.",
        HEADER_MARKERS[0],
        line_text(&spec_path)
    );
    let contents = match emit::rustfmt(&format!("{header}\n{body}")) {
        Ok(text) => text,
        Err(FileError::RustfmtMissing(e)) => {
            return Err(vec![Diagnostic::new(
                emit::RUSTFMT_MISSING,
                SPEC_ENTITY,
                format!("rustfmt could not be started ({e}); specc formats its output with the rustfmt on PATH (rustup component add rustfmt) and does not write unformatted code."),
            )])
        }
        Err(FileError::Rustfmt(e)) => {
            return Err(vec![Diagnostic::new(
                emit::INTERNAL,
                SPEC_ENTITY,
                format!("rustfmt refused the conformance suite specc produced ({e}); this is a defect in specc."),
            )])
        }
        Err(FileError::Syntax(e)) => {
            return Err(vec![Diagnostic::new(
                emit::INTERNAL,
                SPEC_ENTITY,
                format!("specc produced a conformance suite that is not valid Rust ({e}); this is a defect in specc."),
            )])
        }
    };
    Ok(ConformanceOutput {
        files: vec![GeneratedFile {
            path: PATH.to_string(),
            contents,
        }],
        covered: plans.iter().map(|p| p.id.clone()).collect(),
        skipped,
    })
}

/// The route's method, path and shape, or why there is none.
fn parse_route(route: &str) -> Result<(String, String, Shape), String> {
    let words: Vec<&str> = route.split_whitespace().collect();
    let (method, path, rest) = match words.as_slice() {
        [method, path, rest @ ..] => (*method, *path, rest),
        _ => return Err("it is not \"<METHOD> <path> [shape]\"".to_string()),
    };
    if !matches!(method, "GET" | "POST") {
        return Err(format!("the method \"{method}\" is not GET or POST"));
    }
    if !path.starts_with('/') {
        return Err(format!("the path \"{path}\" does not start with /"));
    }
    let shape = match rest {
        [] => Shape::Json,
        ["view"] if method == "GET" => Shape::View,
        // The generic records endpoint (mutate-record): its inputs are the body.
        ["records"] if method == "POST" => Shape::Json,
        ["records", op, object] if method == "POST" => Shape::Records {
            op: (*op).to_string(),
            object: (*object).to_string(),
        },
        _ => {
            return Err(format!(
                "the shape \"{}\" is not view (GET) or records <op> <object> (POST)",
                rest.join(" ")
            ))
        }
    };
    Ok((method.to_string(), path.to_string(), shape))
}

/// The names of the route parameters of a path (`:name` and `*name` segments).
fn route_params(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|s| s.strip_prefix(':').or_else(|| s.strip_prefix('*')))
        .map(str::to_string)
        .collect()
}

/// The path with each parameter replaced by its value, percent-encoded, or by [`PLACEHOLDER`].
fn fill_path(path: &str, values: &BTreeMap<String, String>) -> String {
    path.split('/')
        .map(
            |s| match s.strip_prefix(':').or_else(|| s.strip_prefix('*')) {
                Some(name) => percent_encode(values.get(name).map_or(PLACEHOLDER, String::as_str)),
                None => s.to_string(),
            },
        )
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// A value as a route parameter or a query value: a string as it is, anything else as JSON.
fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn plan(
    world: &World<'_>,
    c: &Value,
    route: &str,
    options: &Options,
) -> Result<Plan, Vec<Diagnostic>> {
    let id = text(c, "id").unwrap_or_default().to_string();
    let mut out = Vec::new();
    let (method, path, mut shape) = parse_route(route).map_err(|why| {
        vec![Diagnostic::new(
            ROUTE,
            &id,
            format!("Contract \"{id}\" names the bffRoute \"{route}\", from which no request can be built: {why}."),
        )]
    })?;
    let case = contract_case(c, world.case);
    let field_list = |key: &str| -> Vec<Input> {
        c.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|f| {
                let name = text(f, "name").unwrap_or_default().to_string();
                Input {
                    wire: case.name(&name),
                    ty: text(f, "type").unwrap_or("json").to_string(),
                    optional: flag(f, "optional"),
                    name,
                }
            })
            .collect()
    };
    let inputs = field_list("inputs");
    let outputs = field_list("outputs");
    if method == "GET" && outputs.iter().any(|o| o.ty == SOCKET_TYPE) {
        shape = Shape::Socket;
    }
    for param in route_params(&path) {
        if !inputs.iter().any(|i| i.name == param) {
            out.push(Diagnostic::new(
                ROUTE,
                &id,
                format!("Contract \"{id}\" has the route parameter \"{param}\" in its bffRoute \"{route}\" and no input of that name."),
            ));
        }
    }
    // The view's address: the route of the screen whose view model this contract is.
    let screen_route = if shape == Shape::View {
        items(world.top, "screens")
            .iter()
            .find(|s| text(s, "viewModel") == Some(id.as_str()))
            .and_then(|s| text(s, "route"))
            .map(str::to_string)
    } else {
        None
    };
    let screen_params: Vec<String> = match &screen_route {
        Some(r) if r.starts_with('*') => vec![r.trim_start_matches('*').to_string()],
        Some(r) => route_params(r),
        None => Vec::new(),
    };
    let auth = c.get("auth").cloned().unwrap_or(Value::Null);
    let scheme = text(&auth, "scheme").unwrap_or("anonymous").to_string();
    let tenant_input = if matches!(shape, Shape::Records { .. } | Shape::Socket) {
        None
    } else {
        inputs
            .iter()
            .find(|i| i.ty == TENANT_TYPE)
            .map(|i| i.wire.clone())
    };
    // The example, checked against the inputs.
    let example: Option<Map<String, Value>> = match c.get(EXAMPLE_PROPERTY) {
        None => None,
        Some(Value::String(json)) => match serde_json::from_str::<Value>(json) {
            Ok(Value::Object(map)) => Some(map),
            _ => {
                out.push(Diagnostic::new(
                    EXAMPLE,
                    &id,
                    format!("Contract \"{id}\" has a {EXAMPLE_PROPERTY} that is not a JSON object written as text."),
                ));
                None
            }
        },
        Some(_) => {
            out.push(Diagnostic::new(
                EXAMPLE,
                &id,
                format!("Contract \"{id}\" has a {EXAMPLE_PROPERTY} that is not a string holding a JSON object."),
            ));
            None
        }
    };
    if let Some(example) = &example {
        for (name, v) in example {
            match inputs.iter().find(|i| i.name == *name) {
                None => out.push(Diagnostic::new(
                    EXAMPLE,
                    &id,
                    format!("The {EXAMPLE_PROPERTY} of contract \"{id}\" gives \"{name}\", which is not one of its inputs."),
                )),
                Some(input) => {
                    if let Err(why) = check_value(world, &TypeRef::parse(&input.ty), v, name) {
                        out.push(Diagnostic::new(
                            EXAMPLE,
                            &id,
                            format!("The {EXAMPLE_PROPERTY} of contract \"{id}\" gives \"{name}\" a value that is not a {}: {why}.", input.ty),
                        ));
                    }
                }
            }
        }
        for input in inputs.iter().filter(|i| !i.optional) {
            if shape != Shape::Socket && !example.contains_key(&input.name) {
                out.push(Diagnostic::new(
                    EXAMPLE,
                    &id,
                    format!("The {EXAMPLE_PROPERTY} of contract \"{id}\" leaves out the required input \"{}\".", input.name),
                ));
            }
        }
    }
    // Errors: the kinds the contract declares, and the step-up refusal by name.
    let mut errors: Vec<String> = Vec::new();
    let mut step_up_error = None;
    for e in c
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(kind) = text(e, "kind") else {
            continue;
        };
        if !world.error_kinds.iter().any(|(k, _)| k == kind) {
            out.push(Diagnostic::new(
                ERROR_KIND,
                &id,
                format!("Contract \"{id}\" declares an error of kind \"{kind}\", which system.generation.errorKinds does not list."),
            ));
        }
        if !errors.iter().any(|k| k == kind) {
            errors.push(kind.to_string());
        }
        if text(e, "name") == Some(STEP_UP_ERROR) {
            step_up_error = Some(case.name(STEP_UP_ERROR));
        }
    }
    for i in inputs.iter().chain(&outputs) {
        if let Err(name) = resolves(world, &TypeRef::parse(&i.ty)) {
            out.push(Diagnostic::new(
                TYPE,
                &id,
                format!("Contract \"{id}\" names the type \"{name}\" (in \"{}\"), which is neither built in nor declared.", i.ty),
            ));
        }
    }
    if !out.is_empty() {
        return Err(out);
    }
    let path_bound: BTreeSet<String> = route_params(&path).into_iter().collect();
    let request = |values: Option<&Map<String, Value>>| -> Request {
        let given: BTreeMap<String, String> = values
            .map(|m| m.iter().map(|(k, v)| (k.clone(), plain(v))).collect())
            .unwrap_or_default();
        let mut query: Vec<(String, String)> = Vec::new();
        let mut body = None;
        let path_text = match &shape {
            Shape::View => {
                let address = match &screen_route {
                    Some(r) if r.starts_with('*') => format!(
                        "/{}",
                        percent_encode(
                            given
                                .get(r.trim_start_matches('*'))
                                .map_or(PLACEHOLDER, String::as_str)
                        )
                    ),
                    Some(r) => fill_path(r, &given),
                    None => "/".to_string(),
                };
                query.push(("path".to_string(), address));
                for i in &inputs {
                    if screen_params.contains(&i.name) {
                        continue;
                    }
                    if let Some(v) = given.get(&i.name) {
                        query.push((i.wire.clone(), v.clone()));
                    }
                }
                fill_path(&path, &given)
            }
            Shape::Socket => fill_path(&path, &given),
            Shape::Json if method == "GET" => {
                for i in &inputs {
                    if path_bound.contains(i.name.as_str()) {
                        continue;
                    }
                    if let Some(v) = given.get(&i.name) {
                        query.push((i.wire.clone(), v.clone()));
                    }
                }
                fill_path(&path, &given)
            }
            Shape::Json => {
                if let Some(values) = values {
                    let mut members = Map::new();
                    for i in &inputs {
                        if path_bound.contains(i.name.as_str()) {
                            continue;
                        }
                        if let Some(v) = values.get(&i.name) {
                            members.insert(i.wire.clone(), wire_value(world, &i.ty, v));
                        }
                    }
                    body = Some(Value::Object(members).to_string());
                }
                fill_path(&path, &given)
            }
            Shape::Records { op, object } => {
                if let Some(values) = values {
                    let id_input = inputs
                        .iter()
                        .find(|i| i.ty == "record-id" && i.name != "expected-version");
                    let mut members = Map::new();
                    members.insert("op".to_string(), Value::String(op.clone()));
                    members.insert("object".to_string(), Value::String(object.clone()));
                    if let Some(i) = id_input {
                        if let Some(v) = values.get(&i.name) {
                            members.insert("id".to_string(), v.clone());
                        }
                    }
                    if op != "delete" {
                        let mut fields = Map::new();
                        for i in &inputs {
                            if Some(i.name.as_str()) == id_input.map(|i| i.name.as_str())
                                || i.name == "expected-version"
                            {
                                continue;
                            }
                            if let Some(v) = values.get(&i.name) {
                                fields.insert(i.wire.clone(), wire_value(world, &i.ty, v));
                            }
                        }
                        members.insert("fields".to_string(), Value::Object(fields));
                    }
                    if let Some(v) = values.get("expected-version") {
                        members.insert("expected_version".to_string(), v.clone());
                    }
                    body = Some(Value::Object(members).to_string());
                }
                fill_path(&path, &given)
            }
        };
        Request {
            path: path_text,
            query,
            body,
        }
    };
    let probe = request(example.as_ref());
    // A success call needs every required input: from the example, or none required (a tenant
    // input is the site's own, which the harness fills; a socket's inputs travel after the
    // upgrade).
    let success = if example.is_some() {
        Some(probe.clone())
    } else if shape == Shape::Socket || inputs.iter().all(|i| i.optional || i.ty == TENANT_TYPE) {
        Some(request(None))
    } else {
        None
    };
    let reply = match &shape {
        Shape::Socket => Reply::Socket,
        Shape::Records { .. } => Reply::Records(
            outputs
                .iter()
                .map(|o| {
                    let from = if o.ty == "record-id" {
                        RecordFrom::Id
                    } else if o.name == "version" {
                        RecordFrom::Version
                    } else {
                        RecordFrom::Field(o.wire.clone())
                    };
                    (from, o.ty.clone(), o.optional)
                })
                .collect(),
        ),
        _ => Reply::Members(
            outputs
                .iter()
                .map(|o| (o.wire.clone(), o.ty.clone(), o.optional))
                .collect(),
        ),
    };
    let quota = items(world.top, "quotas")
        .iter()
        .filter(|q| text(q, "contract") == Some(id.as_str()))
        .filter(|q| text(q, "onExceed").unwrap_or("refuse") == "refuse")
        .filter_map(|q| q.get("limit").and_then(Value::as_u64))
        .min();
    let expect_fail = options
        .expect_fail_prefix
        .as_deref()
        .and_then(|prefix| id.strip_prefix(prefix))
        .map(|rest| rest.to_string());
    Ok(Plan {
        reference: text(c, "ref").unwrap_or_default().to_string(),
        route: route.to_string(),
        method,
        shape,
        scheme,
        requires: texts(&auth, "requires"),
        roles: texts(&auth, "roles"),
        origin: flag(&auth, "origin"),
        csrf: flag(&auth, "csrf"),
        step_up: text(&auth, "stepUp").is_some(),
        read_only: flag(c, "readOnly"),
        tenant_input,
        probe,
        success,
        reply,
        errors,
        step_up_error,
        quota,
        expect_fail,
        id,
    })
}

/// `Ok` when every name in the type resolves, else the first that does not.
fn resolves(world: &World<'_>, t: &TypeRef) -> Result<(), String> {
    match t {
        TypeRef::Builtin(_) => Ok(()),
        TypeRef::List(inner)
        | TypeRef::Set(inner)
        | TypeRef::Option(inner)
        | TypeRef::Shared(inner) => resolves(world, inner),
        TypeRef::Map(key, value) => {
            resolves(world, key)?;
            resolves(world, value)
        }
        TypeRef::Declared(id) if world.types.contains_key(id.as_str()) => Ok(()),
        TypeRef::Declared(id) => Err(id.clone()),
    }
}

/// The wire case a declared struct or union is written in.
fn decl_case(world: &World<'_>, id: &str) -> WireCase {
    world.type_cases.get(id).copied().unwrap_or_default()
}

/// A value of the type with the members of every struct in it renamed to their wire names, as
/// `crate::wire::WireValues` does: an example is written with the specification's names.
fn wire_value(world: &World<'_>, ty: &str, v: &Value) -> Value {
    wire_value_of(world, &TypeRef::parse(ty), v)
}

fn wire_value_of(world: &World<'_>, t: &TypeRef, v: &Value) -> Value {
    match (t, v) {
        (TypeRef::Option(inner) | TypeRef::Shared(inner), v) => wire_value_of(world, inner, v),
        (TypeRef::List(inner) | TypeRef::Set(inner), Value::Array(items)) => Value::Array(
            items
                .iter()
                .map(|i| wire_value_of(world, inner, i))
                .collect(),
        ),
        (TypeRef::Map(_, value), Value::Object(map)) => Value::Object(
            map.iter()
                .map(|(k, m)| (k.clone(), wire_value_of(world, value, m)))
                .collect(),
        ),
        (TypeRef::Declared(id), Value::Object(map)) => {
            let Some(decl) = world.types.get(id.as_str()) else {
                return v.clone();
            };
            if text(decl, "kind") != Some("struct") {
                return v.clone();
            }
            let case = decl_case(world, id);
            let fields = decl.get("fields").and_then(Value::as_array);
            let mut out = Map::new();
            for (k, member) in map {
                match fields
                    .into_iter()
                    .flatten()
                    .find(|f| text(f, "name") == Some(k.as_str()))
                {
                    Some(f) => out.insert(
                        case.name(k),
                        wire_value(world, text(f, "type").unwrap_or("json"), member),
                    ),
                    None => out.insert(k.clone(), member.clone()),
                };
            }
            Value::Object(out)
        }
        _ => v.clone(),
    }
}

/// Whether an example value (with the specification's names) is a value of the type.
fn check_value(world: &World<'_>, t: &TypeRef, v: &Value, at: &str) -> Result<(), String> {
    let wrong = |what: &str| Err(format!("{at} is not {what}"));
    match t {
        TypeRef::Shared(inner) => check_value(world, inner, v, at),
        TypeRef::Option(inner) => {
            if v.is_null() {
                Ok(())
            } else {
                check_value(world, inner, v, at)
            }
        }
        TypeRef::List(inner) | TypeRef::Set(inner) => match v {
            Value::Array(list) => {
                for (i, item) in list.iter().enumerate() {
                    check_value(world, inner, item, &format!("{at}[{i}]"))?;
                }
                Ok(())
            }
            _ => wrong("a list"),
        },
        TypeRef::Map(_, value) => match v {
            Value::Object(map) => {
                for (k, item) in map {
                    check_value(world, value, item, &format!("{at}.{k}"))?;
                }
                Ok(())
            }
            _ => wrong("an object"),
        },
        TypeRef::Builtin(b) => check_builtin(*b, v, at),
        TypeRef::Declared(id) => {
            let Some(decl) = world.types.get(id.as_str()) else {
                return Err(format!("{at} has the undeclared type {id}"));
            };
            match text(decl, "kind") {
                Some("id" | "scalar") => {
                    let repr = text(decl, "repr").unwrap_or("string");
                    match Builtin::parse(repr) {
                        Some(b) => check_builtin(b, v, at),
                        None => Err(format!(
                            "{at} has the type {id}, whose repr {repr} is not built in"
                        )),
                    }
                }
                Some("enum") => {
                    let values = texts(decl, "values");
                    match v.as_str() {
                        Some(s) if values.iter().any(|x| x == s) => Ok(()),
                        _ => wrong(&format!("one of {}", values.join(", "))),
                    }
                }
                Some("struct") => {
                    let Value::Object(map) = v else {
                        return wrong("an object");
                    };
                    let fields: Vec<&Value> = decl
                        .get("fields")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().collect())
                        .unwrap_or_default();
                    for f in &fields {
                        let name = text(f, "name").unwrap_or_default();
                        match map.get(name) {
                            Some(member) => check_value(
                                world,
                                &TypeRef::parse(text(f, "type").unwrap_or("json")),
                                member,
                                &format!("{at}.{name}"),
                            )?,
                            None if flag(f, "optional") => {}
                            None => return Err(format!("{at} has no {name}")),
                        }
                    }
                    for k in map.keys() {
                        if !fields.iter().any(|f| text(f, "name") == Some(k.as_str())) {
                            return Err(format!("{at} has {k}, which {id} does not declare"));
                        }
                    }
                    Ok(())
                }
                // A union is checked by the server; an example of one is passed on as written.
                _ => Ok(()),
            }
        }
    }
}

fn check_builtin(b: Builtin, v: &Value, at: &str) -> Result<(), String> {
    let ok = match b {
        Builtin::Bool => v.is_boolean(),
        Builtin::F64 => v.is_number(),
        Builtin::String | Builtin::Uuid | Builtin::Bytes => v.is_string(),
        Builtin::Json => true,
        _ => {
            let (min, max) = b.integer_range().unwrap_or((0, 0));
            let n = v
                .as_i64()
                .map(i128::from)
                .or_else(|| v.as_u64().map(i128::from));
            n.is_some_and(|n| n >= min && n <= max)
        }
    };
    if ok {
        Ok(())
    } else {
        Err(format!("{at} is not a {}", b.as_str()))
    }
}

// ---- the declared types the suite checks replies against -------------------------------------

/// The declared types `roots` reach, by id, each as the Rust expression of its `Decl`.
fn closure(
    world: &World<'_>,
    roots: &[String],
) -> Result<BTreeMap<String, String>, Vec<Diagnostic>> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut problems = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for root in roots {
        names_in(&TypeRef::parse(root), &mut pending);
    }
    while let Some(id) = pending.pop() {
        if out.contains_key(&id) {
            continue;
        }
        let Some(decl) = world.types.get(id.as_str()) else {
            problems.push(Diagnostic::new(
                TYPE,
                &id,
                format!("The type \"{id}\" is used by a contract and not declared."),
            ));
            continue;
        };
        let mut reached = Vec::new();
        match decl_expr(world, &id, decl, &mut reached) {
            Ok(expr) => {
                out.insert(id.clone(), expr);
            }
            Err(why) => problems.push(Diagnostic::new(TYPE, &id, why)),
        }
        pending.extend(reached);
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}

fn names_in(t: &TypeRef, out: &mut Vec<String>) {
    match t {
        TypeRef::Builtin(_) => {}
        TypeRef::List(inner)
        | TypeRef::Set(inner)
        | TypeRef::Option(inner)
        | TypeRef::Shared(inner) => {
            names_in(inner, out);
        }
        TypeRef::Map(key, value) => {
            names_in(key, out);
            names_in(value, out);
        }
        TypeRef::Declared(id) => out.push(id.clone()),
    }
}

/// The Rust expression of a `Ty` for a type name.
fn ty_expr(t: &TypeRef, reached: &mut Vec<String>) -> String {
    match t {
        TypeRef::Builtin(b) => builtin_expr(*b),
        TypeRef::List(inner) | TypeRef::Set(inner) => {
            format!("Ty::List(&{})", ty_expr(inner, reached))
        }
        TypeRef::Option(inner) => format!("Ty::Option(&{})", ty_expr(inner, reached)),
        TypeRef::Shared(inner) => ty_expr(inner, reached),
        TypeRef::Map(_, value) => format!("Ty::Map(&{})", ty_expr(value, reached)),
        TypeRef::Declared(id) => {
            reached.push(id.clone());
            format!("Ty::Named({})", rust::string(id))
        }
    }
}

fn builtin_expr(b: Builtin) -> String {
    match b {
        Builtin::Bool => "Ty::Bool".to_string(),
        Builtin::F64 => "Ty::Float".to_string(),
        Builtin::String | Builtin::Uuid => "Ty::Text".to_string(),
        Builtin::Bytes => "Ty::Bytes".to_string(),
        Builtin::Json => "Ty::Any".to_string(),
        _ => {
            let (min, max) = b.integer_range().unwrap_or((0, 0));
            format!("Ty::Int({min}, {max})")
        }
    }
}

fn field_exprs(case: WireCase, fields: &[Value], reached: &mut Vec<String>) -> String {
    let items: Vec<String> = fields
        .iter()
        .map(|f| {
            let name = text(f, "name").unwrap_or_default();
            format!(
                "Field {{ wire: {}, ty: {}, optional: {} }}",
                rust::string(&case.name(name)),
                ty_expr(&TypeRef::parse(text(f, "type").unwrap_or("json")), reached),
                flag(f, "optional")
            )
        })
        .collect();
    format!("&[{}]", items.join(", "))
}

fn decl_expr(
    world: &World<'_>,
    id: &str,
    decl: &Value,
    reached: &mut Vec<String>,
) -> Result<String, String> {
    let list = |key: &str| -> Vec<Value> {
        decl.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    match text(decl, "kind") {
        Some("id" | "scalar") => {
            let repr = text(decl, "repr").unwrap_or("string");
            match Builtin::parse(repr) {
                Some(b) => Ok(format!("Decl::Alias({})", builtin_expr(b))),
                None => Err(format!(
                    "The type \"{id}\" has the representation \"{repr}\", which is not built in."
                )),
            }
        }
        Some("enum") => Ok(format!(
            "Decl::Enum(&[{}])",
            texts(decl, "values")
                .iter()
                .map(|v| rust::string(v.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Some("struct") => Ok(format!(
            "Decl::Struct({})",
            field_exprs(decl_case(world, id), &list("fields"), reached)
        )),
        Some("union") => {
            let encoding = decl.get("encoding").cloned().unwrap_or(Value::Null);
            let variant_case = text(&encoding, "variantCase").and_then(VariantCase::parse);
            let tag = match text(&encoding, "tag").unwrap_or("external") {
                "internal" => format!(
                    "Tag::Internal({})",
                    rust::string(text(&encoding, "tagField").unwrap_or("type"))
                ),
                "adjacent" => format!(
                    "Tag::Adjacent({}, {})",
                    rust::string(text(&encoding, "tagField").unwrap_or("type")),
                    rust::string(text(&encoding, "contentField").unwrap_or("content"))
                ),
                _ => "Tag::External".to_string(),
            };
            let case = decl_case(world, id);
            let variants: Vec<String> = list("variants")
                .iter()
                .map(|v| {
                    let name = text(v, "name").unwrap_or_default();
                    let fields = v
                        .get("fields")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let positional = |f: &Value, reached: &mut Vec<String>| {
                        let t = TypeRef::parse(text(f, "type").unwrap_or("json"));
                        if flag(f, "optional") {
                            ty_expr(&TypeRef::Option(Box::new(t)), reached)
                        } else {
                            ty_expr(&t, reached)
                        }
                    };
                    let content = if fields.is_empty() {
                        "Content::Unit".to_string()
                    } else if flag(v, "newtype") {
                        format!("Content::Newtype({})", positional(&fields[0], reached))
                    } else if flag(v, "tuple") {
                        format!(
                            "Content::Tuple(&[{}])",
                            fields
                                .iter()
                                .map(|f| positional(f, reached))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    } else {
                        format!("Content::Fields({})", field_exprs(case, &fields, reached))
                    };
                    format!(
                        "Variant {{ wire: {}, content: {content} }}",
                        rust::string(&variant_wire_name(name, variant_case))
                    )
                })
                .collect();
            Ok(format!("Decl::Union({tag}, &[{}])", variants.join(", ")))
        }
        other => Err(format!(
            "The type \"{id}\" has the kind \"{}\", which the suite cannot check a value of.",
            other.unwrap_or("?")
        )),
    }
}

// ---- what is not checked --------------------------------------------------------------------

fn skips(plans: &[Plan]) -> Vec<Skip> {
    let mut out = Vec::new();
    let mut skip = |p: &Plan, check: &str, reason: String| {
        out.push(Skip {
            contract: p.id.clone(),
            check: check.to_string(),
            reason,
        });
    };
    for p in plans {
        let session = p.scheme == "session";
        if !session && p.scheme != "anonymous" {
            skip(
                p,
                "success",
                format!(
                    "the scheme {} needs a credential the suite does not hold",
                    p.scheme
                ),
            );
            if !p.requires.is_empty() || !p.roles.is_empty() {
                skip(
                    p,
                    "capability",
                    format!(
                        "the scheme {} has no session whose holdings the suite can vary",
                        p.scheme
                    ),
                );
            }
        } else if p.success.is_none() {
            skip(
                p,
                "success",
                format!("it has required inputs and no {EXAMPLE_PROPERTY}"),
            );
        }
        if let Some(limit) = p.quota {
            if limit > QUOTA_CALLS {
                skip(
                    p,
                    "quota",
                    format!("its limit {limit} is above the {QUOTA_CALLS} calls the suite makes"),
                );
            } else if p.success.is_none() || (!session && p.scheme != "anonymous") {
                skip(
                    p,
                    "quota",
                    "it has no call that succeeds to repeat".to_string(),
                );
            }
        }
        if matches!(p.reply, Reply::Socket) {
            skip(
                p,
                "reply",
                "a socket answers an upgrade, whose events the suite does not read".to_string(),
            );
        }
    }
    out
}

// ---- writing --------------------------------------------------------------------------------

/// The reserved names of the generated file.
const RESERVED: &[&str] = &[
    "SPEC",
    "ROLES",
    "ERROR_STATUS",
    "TYPES",
    "COVERED",
    "SKIPPED",
];

fn const_name(id: &str) -> String {
    rust::ident(&id.replace('-', "_").to_uppercase())
}

fn test_name(id: &str) -> String {
    rust::ident(&id.replace('-', "_").to_lowercase())
}

fn strings(list: &[String]) -> String {
    format!(
        "&[{}]",
        list.iter()
            .map(|s| rust::string(s.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn request_expr(r: &Request) -> String {
    format!(
        "Request {{ path: {}, query: &[{}], body: {} }}",
        rust::string(&r.path),
        r.query
            .iter()
            .map(|(k, v)| format!("({}, {})", rust::string(k), rust::string(v)))
            .collect::<Vec<_>>()
            .join(", "),
        match &r.body {
            Some(b) => format!("Some({})", rust::string(b)),
            None => "None".to_string(),
        }
    )
}

fn option_str(v: Option<&str>) -> String {
    match v {
        Some(s) => format!("Some({})", rust::string(s)),
        None => "None".to_string(),
    }
}

fn render(
    world: &World<'_>,
    plans: &[Plan],
    decls: &BTreeMap<String, String>,
    skipped: &[Skip],
) -> Result<String, Vec<Diagnostic>> {
    let mut claims = Claims::new(PATH, RESERVED);
    // `conform` is the harness's own function, which every test calls.
    let mut tests = Claims::new(PATH, &["conform"]);
    let mut problems = Vec::new();
    for p in plans {
        if let Err(d) = claims.claim_unique(&p.id, &const_name(&p.id)) {
            problems.push(d);
        }
        if let Err(d) = tests.claim_unique(&p.id, &test_name(&p.id)) {
            problems.push(d);
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let system = world
        .top
        .get("system")
        .and_then(|s| text(s, "name"))
        .unwrap_or("the system");
    let mut out = String::new();
    for line in [
        format!("The contract conformance suite of {}: one test per contract that is shipped and names a bffRoute, each of which drives the contract through the running BFF and backend (`crate::harness::conform`) and compares what comes back with its declaration.", line_text(system)),
        String::new(),
        "Generated by `specc conformance` (WG-5182); the checks are described in `crates/spec-compiler/src/conformance.rs`.".to_string(),
    ] {
        if line.is_empty() {
            out.push_str("//!\n");
        } else {
            out.push_str(&format!("//! {line}\n"));
        }
    }
    out.push('\n');
    out.push_str("use crate::harness::*;\n\n");
    out.push_str(
        "/// What the harness reads of the specification besides the contract it checks.\n",
    );
    out.push_str(&format!(
        "pub const SPEC: Spec = Spec {{ roles: ROLES, types: TYPES, error_status: ERROR_STATUS, error_body: {}, skipped: SKIPPED }};\n\n",
        option_str(world.error_body.as_deref())
    ));
    out.push_str("/// The capabilities each role of the specification confers.\n");
    let roles: Vec<String> = items(world.top, "roles")
        .iter()
        .filter_map(|r| {
            text(r, "id").map(|id| {
                format!(
                    "({}, {})",
                    rust::string(id),
                    strings(&texts(r, "capabilities"))
                )
            })
        })
        .collect();
    out.push_str(&format!(
        "pub const ROLES: &[(&str, &[&str])] = &[{}];\n\n",
        roles.join(", ")
    ));
    out.push_str("/// The HTTP status of each kind of error (`system.generation.errorKinds`).\n");
    out.push_str(&format!(
        "pub const ERROR_STATUS: &[(&str, u16)] = &[{}];\n\n",
        world
            .error_kinds
            .iter()
            .map(|(k, s)| format!("({}, {s})", rust::string(k)))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str("/// The declared types the replies reach, with their wire names.\n");
    out.push_str(&format!(
        "pub const TYPES: &[TypeDecl] = &[{}];\n\n",
        decls
            .iter()
            .map(|(id, decl)| format!("TypeDecl {{ id: {}, decl: {decl} }}", rust::string(id)))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str("/// Every contract with a test here.\n");
    out.push_str(&format!(
        "pub const COVERED: &[&str] = {};\n\n",
        strings(&plans.iter().map(|p| p.id.clone()).collect::<Vec<_>>())
    ));
    out.push_str("/// The checks that are not made: (contract, check, reason).\n");
    out.push_str(&format!(
        "pub const SKIPPED: &[(&str, &str, &str)] = &[{}];\n\n",
        skipped
            .iter()
            .map(|s| format!(
                "({}, {}, {})",
                rust::string(&s.contract),
                rust::string(&s.check),
                rust::string(&s.reason)
            ))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    for p in plans {
        let name = const_name(&p.id);
        let mut reached = Vec::new();
        let shape = match &p.shape {
            Shape::Json => "Shape::Json",
            Shape::View => "Shape::View",
            Shape::Records { .. } => "Shape::Records",
            Shape::Socket => "Shape::Socket",
        };
        let scheme = match p.scheme.as_str() {
            "anonymous" => "Scheme::Anonymous".to_string(),
            "session" => "Scheme::Session".to_string(),
            other => format!("Scheme::Other({})", rust::string(other)),
        };
        let reply = match &p.reply {
            Reply::Socket => "Reply::Socket".to_string(),
            Reply::Members(fields) => format!(
                "Reply::Members(&[{}])",
                fields
                    .iter()
                    .map(|(wire, ty, optional)| format!(
                        "Field {{ wire: {}, ty: {}, optional: {optional} }}",
                        rust::string(wire),
                        ty_expr(&TypeRef::parse(ty), &mut reached)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Reply::Records(fields) => format!(
                "Reply::Records(&[{}])",
                fields
                    .iter()
                    .map(|(from, ty, optional)| format!(
                        "RecordOutput {{ from: {}, ty: {}, optional: {optional} }}",
                        match from {
                            RecordFrom::Id => "RecordFrom::Id".to_string(),
                            RecordFrom::Version => "RecordFrom::Version".to_string(),
                            RecordFrom::Field(wire) =>
                                format!("RecordFrom::Field({})", rust::string(wire)),
                        },
                        ty_expr(&TypeRef::parse(ty), &mut reached)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        out.push_str(&rust::doc_comment(
            "",
            &format!(
                "Contract `{}` ({}): {}.",
                p.id,
                if p.reference.is_empty() {
                    "no ref"
                } else {
                    p.reference.as_str()
                },
                p.route
            ),
        ));
        let _ = writeln!(
            out,
            "pub const {name}: Contract = Contract {{ id: {}, method: {}, shape: {shape}, scheme: {scheme}, requires: {}, roles: {}, origin: {}, csrf: {}, step_up: {}, read_only: {}, tenant_input: {}, probe: {}, success: {}, reply: {reply}, errors: {}, step_up_error: {}, quota: {} }};\n",
            rust::string(&p.id),
            rust::string(&p.method),
            strings(&p.requires),
            strings(&p.roles),
            p.origin,
            p.csrf,
            p.step_up,
            p.read_only,
            option_str(p.tenant_input.as_deref()),
            request_expr(&p.probe),
            match &p.success {
                Some(r) => format!("Some({})", request_expr(r)),
                None => "None".to_string(),
            },
            strings(&p.errors),
            option_str(p.step_up_error.as_deref()),
            // Only a quota the suite can exhaust with a call that succeeds; the others are in
            // SKIPPED.
            match p.quota {
                Some(limit)
                    if limit <= QUOTA_CALLS
                        && p.success.is_some()
                        && matches!(p.scheme.as_str(), "session" | "anonymous") =>
                {
                    format!("Some({limit})")
                }
                _ => "None".to_string(),
            },
        );
        out.push_str("#[tokio::test]\n");
        if let Some(rule) = &p.expect_fail {
            out.push_str(&format!(
                "#[should_panic(expected = {})]\n",
                rust::string(&format!("[{rule}]"))
            ));
        }
        let _ = writeln!(
            out,
            "async fn {}() {{\n    conform(&SPEC, &{name}).await;\n}}\n",
            test_name(&p.id)
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(contracts: &str) -> String {
        format!(
            r#"{{
  "system": {{ "id": "s", "name": "S", "generation": {{ "wireCase": "snake_case", "errorKinds": {{ "unauthenticated": 401, "forbidden": 403, "invalid": 422, "throttled": 429 }}, "errorBody": {{ "type": "api-error" }} }} }},
  "types": [
    {{ "id": "error-code", "kind": "enum", "values": ["unauthenticated", "forbidden", "invalid", "throttled"] }},
    {{ "id": "api-error", "kind": "struct", "fields": [{{ "name": "code", "type": "error-code" }}, {{ "name": "message", "type": "string" }}, {{ "name": "kind", "type": "string", "optional": true }}, {{ "name": "correlation", "type": "string" }}] }},
    {{ "id": "tenant-id", "kind": "id", "repr": "string" }},
    {{ "id": "record-id", "kind": "id", "repr": "u64" }},
    {{ "id": "note", "kind": "struct", "fields": [{{ "name": "note-title", "type": "string" }}, {{ "name": "words", "type": "u32", "optional": true }}] }}
  ],
  "roles": [{{ "id": "owner", "capabilities": ["notes-write"] }}],
  "screens": [{{ "id": "screen-note", "route": "/notes/:slug", "viewModel": "read-note" }}],
  "quotas": [{{ "id": "q", "contract": "write-note", "limit": 3, "window": "1 min", "onExceed": "refuse" }}],
  "contracts": [{contracts}]
}}"#
        )
    }

    const WRITE: &str = r#"{ "id": "write-note", "ref": "CT-1", "deployment": "shipped", "bffRoute": "POST /api/notes/:slug",
      "auth": { "scheme": "session", "requires": ["notes-write"], "csrf": true, "origin": true, "stepUp": "15 min" },
      "inputs": [{ "name": "slug", "type": "string" }, { "name": "tenant", "type": "tenant-id" }, { "name": "note", "type": "note" }],
      "outputs": [{ "name": "note-id", "type": "record-id" }],
      "errors": [{ "name": "unauthenticated", "kind": "unauthenticated" }, { "name": "step-up-required", "kind": "forbidden" }, { "name": "forbidden", "kind": "forbidden" }],
      "conformanceExample": "{\"slug\": \"a b\", \"tenant\": \"alpha\", \"note\": {\"note-title\": \"T\"}}" }"#;

    fn run(contracts: &str, prefix: Option<&str>) -> Result<ConformanceOutput, Vec<Diagnostic>> {
        generate(
            "fixture.jsonc",
            &spec(contracts),
            &Options {
                expect_fail_prefix: prefix.map(str::to_string),
            },
        )
    }

    #[test]
    fn routes_are_parsed_by_their_shape() {
        assert_eq!(parse_route("GET /api/view view").unwrap().2, Shape::View);
        assert_eq!(
            parse_route("POST /api/records records").unwrap().2,
            Shape::Json
        );
        assert_eq!(
            parse_route("POST /api/records records create post")
                .unwrap()
                .2,
            Shape::Records {
                op: "create".into(),
                object: "post".into()
            }
        );
        assert!(parse_route("PUT /api/x").is_err());
        assert!(parse_route("GET api/x").is_err());
        assert!(parse_route("GET /api/x records a b").is_err());
        assert_eq!(route_params("/a/:b/c/*d"), vec!["b", "d"]);
        assert_eq!(
            fill_path("/a/:b", &BTreeMap::new()),
            format!("/a/{PLACEHOLDER}")
        );
    }

    #[test]
    fn a_shipped_routed_contract_gets_a_test_with_its_example_in_wire_names() {
        let out = run(WRITE, None).unwrap();
        assert_eq!(out.covered, vec!["write-note"]);
        let text = &out.files[0].contents;
        assert!(text.starts_with("// @generated by specc from fixture.jsonc."));
        assert!(text.contains("async fn write_note()"));
        // The route parameter is filled and encoded; the body has the wire names of the struct.
        assert!(text.contains("/api/notes/a%20b"), "{text}");
        assert!(text.contains(r#"\"note_title\":\"T\""#), "{text}");
        assert!(
            !text.contains(r#"\"slug\""#),
            "a route parameter is not in the body: {text}"
        );
        assert!(text.contains(r#"tenant_input: Some("tenant")"#));
        assert!(text.contains(r#"step_up_error: Some("step_up_required")"#));
        assert!(text.contains("quota: Some(3)"));
        assert!(text.contains("TypeDecl {"));
        assert!(text.contains(r#"wire: "note_title""#) || text.contains("note_title"));
    }

    #[test]
    fn a_contract_that_is_not_shipped_or_has_no_route_gets_no_test() {
        let target = WRITE.replace("\"deployment\": \"shipped\"", "\"deployment\": \"target\"");
        assert!(run(&target, None).unwrap().covered.is_empty());
        let unrouted = WRITE.replace("\"bffRoute\": \"POST /api/notes/:slug\",", "");
        assert!(run(&unrouted, None).unwrap().covered.is_empty());
    }

    #[test]
    fn an_example_must_name_inputs_with_values_of_their_types() {
        let unknown = WRITE.replace(
            r#"\"tenant\": \"alpha\""#,
            r#"\"tenant\": \"alpha\", \"extra\": 1"#,
        );
        let codes: Vec<String> = run(&unknown, None)
            .unwrap_err()
            .iter()
            .map(|d| d.code.clone())
            .collect();
        assert_eq!(codes, vec![EXAMPLE]);
        let wrong = WRITE.replace(r#"\"note-title\": \"T\""#, r#"\"note-title\": 7"#);
        assert_eq!(run(&wrong, None).unwrap_err()[0].code, EXAMPLE);
        let missing = WRITE.replace(r#"\"tenant\": \"alpha\", "#, "");
        assert_eq!(run(&missing, None).unwrap_err()[0].code, EXAMPLE);
        let not_json = WRITE.replace(r#""{\"slug\""#, r#""{slug\""#);
        assert_eq!(run(&not_json, None).unwrap_err()[0].code, EXAMPLE);
    }

    #[test]
    fn unknown_types_kinds_and_parameters_are_refused() {
        let ty = WRITE.replace(r#""type": "record-id" }]"#, r#""type": "nothing" }]"#);
        assert!(run(&ty, None).unwrap_err().iter().any(|d| d.code == TYPE));
        let kind = WRITE.replace(r#""kind": "unauthenticated""#, r#""kind": "gone""#);
        assert!(run(&kind, None)
            .unwrap_err()
            .iter()
            .any(|d| d.code == ERROR_KIND));
        let param = WRITE.replace("/api/notes/:slug", "/api/notes/:other");
        assert!(run(&param, None)
            .unwrap_err()
            .iter()
            .any(|d| d.code == ROUTE));
        let route = WRITE.replace("POST /api/notes/:slug", "PATCH /api/notes");
        assert!(run(&route, None)
            .unwrap_err()
            .iter()
            .any(|d| d.code == ROUTE));
    }

    #[test]
    fn checks_that_cannot_be_made_are_listed_with_a_reason() {
        let bare = WRITE.replace(
            r#""conformanceExample": "{\"slug\": \"a b\", \"tenant\": \"alpha\", \"note\": {\"note-title\": \"T\"}}" "#,
            "\"readOnly\": true ",
        );
        let out = run(&bare, None).unwrap();
        let checks: Vec<&str> = out.skipped.iter().map(|s| s.check.as_str()).collect();
        assert_eq!(checks, vec!["success", "quota"], "{:?}", out.skipped);
        assert!(out.files[0].contents.contains("success: None"));
        // Without an example the route parameter is the placeholder.
        assert!(out.files[0].contents.contains(PLACEHOLDER));
    }

    #[test]
    fn a_view_contract_asks_for_its_screen_and_a_fixture_is_expected_to_fail_with_its_rule() {
        let view = r#"{ "id": "read-note", "deployment": "shipped", "bffRoute": "GET /api/view view",
          "auth": { "scheme": "anonymous" }, "inputs": [{ "name": "slug", "type": "string" }],
          "outputs": [{ "name": "note", "type": "note" }], "errors": [],
          "conformanceExample": "{\"slug\": \"first\"}" }"#;
        // The screen whose view model is `read-note` gives the address, with the slug in it.
        let text = run(view, Some("violates-")).unwrap().files[0]
            .contents
            .clone();
        assert!(text.contains(r#"("path", "/notes/first")"#), "{text}");
        assert!(!text.contains("should_panic"));
        // A view no screen names asks for the front page; a violating fixture must fail with its rule.
        let fixture = view.replace("read-note", "violates-reply-shape");
        let text = run(&fixture, Some("violates-")).unwrap().files[0]
            .contents
            .clone();
        assert!(text.contains(r#"("path", "/")"#), "{text}");
        assert!(
            text.contains(r#"#[should_panic(expected = "[reply-shape]")]"#),
            "{text}"
        );
    }

    #[test]
    fn two_contracts_that_become_one_name_are_refused() {
        let twins = format!("{WRITE}, {}", WRITE.replace("write-note", "write_note"));
        assert!(run(&twins, None)
            .unwrap_err()
            .iter()
            .any(|d| d.code == crate::codetext::IDENTIFIER_COLLISION));
    }
}
