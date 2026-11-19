//! The end-to-end output: `specc e2e` turns the user journeys (`uiScenarios`) of a specification
//! into Playwright tests, one `<scenario-id>.spec.js` per journey, a runtime module
//! (`journey.js`) those tests share, and a Playwright configuration that runs them against the
//! stack the hand-written suite uses and writes a JUnit report Concordance can ingest.
//!
//! # Layout
//!
//! The files are meant to be generated three directories below the repository root
//! (`tests/e2e/generated`). Two paths inside them are written relative to that place: the log
//! files of `system.telemetry` (relative to the root) and the JUnit report
//! (`var/test-results/e2e-junit.xml`).
//!
//! # Linking tests to requirements
//!
//! Every requirement a journey `verifies` becomes one `// Verifies: <ref>` comment line directly
//! above its `test(` call. Concordance's TypeScript indexer reads the consecutive `//` lines
//! above a `test(...)` call as the test's doc and takes the ids after `Verifies:`; the JUnit
//! report names each case by its title, which is the test symbol's name, so results join.
//!
//! # Steps
//!
//! See [`lower_step`]. Elements are selected by role and accessible name only; form fields by
//! their label. Every step's input is checked as Exegra checks it (`src/core/ui-steps.js`), and
//! anything else is a diagnostic: nothing is dropped.
//!
//! # What the runtime observes
//!
//! - **Live refreshes.** `waitFor` passes only when, after the browser was last left, navigated or
//!   waited, a live notice arrived on the page's WebSocket (a frame of type `event`, `resync` or
//!   `session`, the notices of the live protocol) and then the open screen refetched its view
//!   (`GET /api/view?path=<the open path>`) with no navigation in between. The kernel's own
//!   navigation fetches are therefore never counted. The open screen must be one a live binding
//!   refreshes on the event: checked statically where the screen is known, and at run time from
//!   the address otherwise (a click, a key or a form may navigate).
//! - **Backend logs.** `expectLog` reads the lines written since the test began, either from the
//!   services' log files or, when `STRATA_E2E_LOG_CONTAINERS` is set, from `podman logs` of those
//!   containers. The matching line must carry the event's level, no member outside the record
//!   members and the event's allowlist, a correlation id that one of the journey's browsers (or
//!   one of its API calls) sent in `x-correlation-id`, and the same correlation must appear in
//!   another service's log: the browser request is linked to the backend event.
//! - **Another stack.** With `STRATA_E2E_BASE_URL`, addresses the journeys name are moved from
//!   the origin they were written against (`--origin`) to that stack.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::diagnostic::{dedup, Diagnostic, SPEC_ENTITY};
use crate::emit::{GeneratedFile, HEADER_MARKERS};
use crate::jsonc;

/// Code of a text that is not JSONC.
pub const SYNTAX: &str = "e2e.spec-syntax";
/// Code of a value with the wrong shape.
pub const SHAPE: &str = "e2e.spec-shape";
/// Code of a specification without journeys.
pub const NO_SCENARIOS: &str = "e2e.no-scenarios";
/// Code of a journey id that cannot name a file.
pub const ID_INVALID: &str = "e2e.id-invalid";
/// Code of an id used twice.
pub const DUPLICATE: &str = "e2e.duplicate-id";
/// Code of a step that is not exactly one known key with a value of its shape.
pub const STEP: &str = "e2e.step-shape";
/// Code of a role that is not a WAI-ARIA role.
pub const ROLE: &str = "e2e.role-invalid";
/// Code of a step that names a screen, form, field, action, event, browser or claim that does
/// not exist.
pub const REFERENCE: &str = "e2e.reference-unknown";
/// Code of a `goto` whose parameters do not fit the screen's route.
pub const GOTO_PARAMS: &str = "e2e.goto-params";
/// Code of a form field without a label, which a test cannot find.
pub const FIELD_LABEL: &str = "e2e.field-label-missing";
/// Code of a form without a submit label.
pub const SUBMIT_LABEL: &str = "e2e.submit-label-missing";
/// Code of a `fill` value the field's control cannot take.
pub const FILL_VALUE: &str = "e2e.fill-value";
/// Code of a `fill` by role with a role that is not a text control.
pub const FILL_ROLE: &str = "e2e.fill-role";
/// Code of a `fill` of a select or radio group with a value that is no option.
pub const FILL_OPTION: &str = "e2e.fill-option-unknown";
/// Code of an `expectStatus` with no navigation before it in its browser.
pub const STATUS_WITHOUT_GOTO: &str = "e2e.status-without-navigation";
/// Code of a `waitFor` whose event no live binding turns into something the page shows.
pub const WAIT_UNOBSERVABLE: &str = "e2e.wait-unobservable";
/// Code of a verified claim that is missing, not a requirement or has no `ref`.
pub const VERIFIES: &str = "e2e.verifies-invalid";
/// Code of an `expectLog` of an event that is not logged, by another service, or of a member
/// its line does not hold.
pub const LOG_EVENT: &str = "e2e.log-event";
/// Code of a `${name}` that no earlier step captured, or a value captured twice.
pub const CAPTURE: &str = "e2e.capture-undefined";
/// Code of an `expectNoReload` of a marker this browser did not set.
pub const MARK: &str = "e2e.mark-undefined";
/// Code of an `expectRequests` or `expectNoRequest` of a log this browser did not start.
pub const REQUEST_LOG: &str = "e2e.log-undefined";
/// Code of a `clickAction` whose `item` or `confirm` does not fit the action.
pub const ACTION_ITEM: &str = "e2e.action-item";
/// Code of an attribute a journey may not read.
pub const ATTRIBUTE: &str = "e2e.attribute";
/// Code of a host that is not a host name with an optional port.
pub const HOST: &str = "e2e.host";
/// Code of a path that is not an absolute path.
pub const PATH: &str = "e2e.path";
/// Code of a heading level on another role, or out of 1 to 6.
pub const LEVEL: &str = "e2e.level";
/// Code of a timeout that is not a positive integer.
pub const TIMEOUT: &str = "e2e.timeout";
/// Code of a touch or mobile emulation after the browser's first navigation (a browser context
/// cannot change them).
pub const EMULATE_LATE: &str = "e2e.emulate-after-navigation";
/// Code of a step that reads the services' logs when `system.telemetry` names no log file.
pub const NO_LOGS: &str = "e2e.no-logs";
/// Code of an `--origin` that is not `http(s)://host[:port]`.
pub const ORIGIN: &str = "e2e.origin-invalid";

/// WAI-ARIA 1.2 roles a step may select by (the list Exegra checks).
pub const ARIA_ROLES: &[&str] = &[
    "alert",
    "alertdialog",
    "application",
    "article",
    "banner",
    "blockquote",
    "button",
    "caption",
    "cell",
    "checkbox",
    "code",
    "columnheader",
    "combobox",
    "complementary",
    "contentinfo",
    "definition",
    "deletion",
    "dialog",
    "directory",
    "document",
    "emphasis",
    "feed",
    "figure",
    "form",
    "generic",
    "grid",
    "gridcell",
    "group",
    "heading",
    "img",
    "insertion",
    "link",
    "list",
    "listbox",
    "listitem",
    "log",
    "main",
    "marquee",
    "math",
    "menu",
    "menubar",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "meter",
    "navigation",
    "none",
    "note",
    "option",
    "paragraph",
    "presentation",
    "progressbar",
    "radio",
    "radiogroup",
    "region",
    "row",
    "rowgroup",
    "rowheader",
    "scrollbar",
    "search",
    "searchbox",
    "separator",
    "slider",
    "spinbutton",
    "status",
    "strong",
    "subscript",
    "superscript",
    "switch",
    "tab",
    "table",
    "tablist",
    "tabpanel",
    "term",
    "textbox",
    "time",
    "timer",
    "toolbar",
    "tooltip",
    "tree",
    "treegrid",
    "treeitem",
];

/// Roles a `fill` by role may type into.
pub const FILLABLE_ROLES: [&str; 4] = ["textbox", "searchbox", "combobox", "spinbutton"];

/// The keys of an element selector.
const TARGET: &[&str] = &["role", "name", "namePattern", "level", "within"];

/// Every step kind, with the keys its value may have (`None`: a scalar value). The same table
/// as Exegra's `STEP_KINDS`.
pub const STEP_KINDS: &[(&str, Option<&[&str]>)] = &[
    ("goto", Some(&["screen", "params", "host"])),
    ("gotoPath", Some(&["path", "host"])),
    ("reload", Some(&[])),
    ("back", Some(&[])),
    ("click", Some(TARGET)),
    ("focus", Some(TARGET)),
    ("clickAction", Some(&["action", "item", "confirm"])),
    (
        "fill",
        Some(&[
            "form",
            "field",
            "value",
            "role",
            "name",
            "namePattern",
            "level",
            "within",
        ]),
    ),
    ("submit", Some(&["form"])),
    ("press", None),
    (
        "expectText",
        Some(&["text", "pattern", "within", "timeoutMs"]),
    ),
    (
        "expectNoText",
        Some(&["text", "pattern", "within", "timeoutMs"]),
    ),
    ("expectTitle", Some(&["text", "pattern", "timeoutMs"])),
    ("expectScreen", Some(&["screen", "params", "timeoutMs"])),
    (
        "expectRole",
        Some(&[
            "role",
            "name",
            "namePattern",
            "level",
            "within",
            "state",
            "timeoutMs",
        ]),
    ),
    (
        "expectCount",
        Some(&[
            "role",
            "name",
            "namePattern",
            "level",
            "within",
            "count",
            "timeoutMs",
        ]),
    ),
    (
        "expectAttribute",
        Some(&[
            "role",
            "name",
            "namePattern",
            "level",
            "within",
            "attribute",
            "value",
            "pattern",
            "timeoutMs",
        ]),
    ),
    ("expectStatus", None),
    ("mark", None),
    ("expectNoReload", None),
    ("recordRequests", None),
    (
        "expectRequests",
        Some(&["log", "to", "only", "atLeast", "fromWorker"]),
    ),
    ("expectNoRequest", Some(&["log", "path"])),
    (
        "apiCall",
        Some(&[
            "method",
            "path",
            "host",
            "headers",
            "body",
            "as",
            "anonymous",
            "expectStatus",
            "expectJson",
            "expectBodyIncludes",
            "expectBodyExcludes",
            "capture",
        ]),
    ),
    ("expectCookie", Some(&["name", "absent", "attributes"])),
    ("setCookie", Some(&["name", "value", "host", "attributes"])),
    (
        "capture",
        Some(&["name", "from", "cookie", "header", "element", "attribute"]),
    ),
    (
        "emulate",
        Some(&["colorScheme", "reducedMotion", "touch", "mobile"]),
    ),
    ("expectNoOverflow", Some(&[])),
    ("expectBeside", Some(&["a", "b", "side"])),
    ("switchBrowser", None),
    ("goOffline", Some(&[])),
    ("goOnline", Some(&[])),
    ("setViewport", Some(&["width", "height"])),
    ("axeScan", Some(&[])),
    ("logScan", Some(&["forbid"])),
    (
        "expectLog",
        Some(&["service", "event", "fields", "timeoutMs"]),
    ),
    ("waitFor", Some(&["event", "timeoutMs"])),
    ("expectNoEvent", Some(&["event", "forMs"])),
];

/// Step kinds whose value may also be written as `true` (they take no parameters).
const FLAG_STEPS: &[&str] = &[
    "reload",
    "back",
    "goOffline",
    "goOnline",
    "axeScan",
    "expectNoOverflow",
];

/// A screen: its id, route and parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    /// The screen id.
    pub id: String,
    /// The route pattern.
    pub route: String,
    /// The route parameters in order.
    pub params: Vec<String>,
    /// The optional query parameters in declaration order.
    pub query: Vec<String>,
}

impl Screen {
    /// The catch-all screen (`*` or `*name`) has no address of its own.
    pub fn catch_all(&self) -> bool {
        self.route.starts_with('*')
    }
}

/// A form field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The field name.
    pub name: String,
    /// The accessible label.
    pub label: Option<String>,
    /// The control kind.
    pub control: String,
    /// `(value, label)` of a select's or radio group's options.
    pub options: Vec<(String, String)>,
    /// The options come from the view model (`optionsFrom`), so they are known only on the page.
    pub options_from: bool,
}

/// A form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// The form id.
    pub id: String,
    /// The fields.
    pub fields: Vec<Field>,
    /// The accessible name of the submit button.
    pub submit_label: Option<String>,
}

/// An action: a button on a screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// The action id.
    pub id: String,
    /// The button's label, `${item.<field>}` placeholders included.
    pub label: String,
    /// The accessible name of the confirmation button, if it asks for one.
    pub confirm: Option<String>,
    /// Repeated per row of a list (`forEach`).
    pub per_row: bool,
}

/// A user journey.
#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    /// The id, which names the file.
    pub id: String,
    /// The name, which titles the test.
    pub name: String,
    /// The `ref`s of the requirements it verifies.
    pub verifies: Vec<String>,
    /// The browser names.
    pub browsers: Vec<String>,
    /// The viewport, if stated.
    pub viewport: Option<(u64, u64)>,
    /// The steps as written.
    pub steps: Vec<Value>,
}

/// What the tests are made from.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Screens in declaration order.
    pub screens: Vec<Screen>,
    /// Forms in declaration order.
    pub forms: Vec<Form>,
    /// Actions in declaration order.
    pub actions: Vec<Action>,
    /// `(screen, event)` of every live binding.
    pub bindings: Vec<(String, String)>,
    /// Event ids.
    pub events: Vec<String>,
    /// The logged events: what an `expectLog` step reads.
    pub logged: Vec<LoggedEvent>,
    /// `(service, log file)` of every telemetry service with a log file.
    pub logs: Vec<(String, String)>,
    /// The journeys in declaration order.
    pub scenarios: Vec<Scenario>,
}

/// A logged event (`event.logging` with `logged: true`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedEvent {
    /// The event id.
    pub id: String,
    /// The service whose log holds it.
    pub service: String,
    /// The line's `level`.
    pub level: String,
    /// The line's `event` member.
    pub name: String,
    /// The members a test may expect: `tenant` when the event logs one, then the names its
    /// allowlisted fields are written as.
    pub members: Vec<String>,
}

/// How the files are generated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// The origin (`http://host:port`) the journeys' absolute hosts and header values were
    /// written against. With it, the tests move them to `STRATA_E2E_BASE_URL` when that is set;
    /// without it, they are used as written.
    pub origin: Option<String>,
    /// Journeys whose id starts with this are expected to fail (`test.fail()`): the fixtures
    /// that show each generated assertion can fail.
    pub expect_fail_prefix: Option<String>,
}

/// The generated files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eOutput {
    /// The files, sorted by path.
    pub files: Vec<GeneratedFile>,
}

/// Generates the Playwright files from a specification's text with the default options.
pub fn generate(spec_path: &str, text: &str) -> Result<E2eOutput, Vec<Diagnostic>> {
    generate_with(spec_path, text, &Options::default())
}

/// Generates the Playwright files from a specification's text, or refuses with every
/// diagnostic. `spec_path` is the path written into the headers.
pub fn generate_with(
    spec_path: &str,
    text: &str,
    options: &Options,
) -> Result<E2eOutput, Vec<Diagnostic>> {
    let spec_path = spec_path.replace('\\', "/");
    if let Some(origin) = &options.origin {
        if !valid_origin(origin) {
            return Err(vec![Diagnostic::new(
                ORIGIN,
                SPEC_ENTITY,
                format!("--origin \"{origin}\" is not http(s)://host[:port]"),
            )]);
        }
    }
    let plan = load(text)?;
    let mut files = Vec::new();
    let mut diagnostics = Vec::new();
    for scenario in &plan.scenarios {
        let expect_fail = options
            .expect_fail_prefix
            .as_deref()
            .is_some_and(|p| scenario.id.starts_with(p));
        match scenario_file(&plan, scenario, expect_fail) {
            Ok(body) => files.push(file(&format!("{}.spec.js", scenario.id), &spec_path, body)),
            Err(mut d) => diagnostics.append(&mut d),
        }
    }
    if !diagnostics.is_empty() {
        return Err(dedup(diagnostics));
    }
    files.push(file(
        "journey.js",
        &spec_path,
        runtime(&plan, options.origin.as_deref()),
    ));
    files.push(file("playwright.generated.config.js", &spec_path, config()));
    files.sort();
    Ok(E2eOutput { files })
}

fn file(path: &str, spec_path: &str, body: String) -> GeneratedFile {
    GeneratedFile {
        path: path.to_string(),
        contents: format!(
            "{} from {}. Do not edit; change the specification and regenerate.\n{body}",
            HEADER_MARKERS[0],
            crate::codetext::line_text(spec_path)
        ),
    }
}

fn valid_origin(origin: &str) -> bool {
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    valid_host(rest)
}

// ---- loading ------------------------------------------------------------------------------

fn string<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

/// A list of strings, or a diagnostic for anything else (absent is empty).
fn strings(
    obj: &Map<String, Value>,
    key: &str,
    entity: &str,
    d: &mut Vec<Diagnostic>,
) -> Vec<String> {
    match obj.get(key) {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            let mut out = Vec::new();
            for (i, item) in items.iter().enumerate() {
                match item.as_str() {
                    Some(s) => out.push(s.to_string()),
                    None => d.push(Diagnostic::new(
                        SHAPE,
                        entity,
                        format!("item {} of \"{key}\" is not a string", i + 1),
                    )),
                }
            }
            out
        }
        Some(_) => {
            d.push(Diagnostic::new(
                SHAPE,
                entity,
                format!("\"{key}\" is not an array of strings"),
            ));
            Vec::new()
        }
    }
}

fn array<'a>(
    top: &'a Map<String, Value>,
    key: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> &'a [Value] {
    match top.get(key) {
        None => &[],
        Some(Value::Array(items)) => items,
        Some(_) => {
            diagnostics.push(Diagnostic::new(
                SHAPE,
                SPEC_ENTITY,
                format!("\"{key}\" is not an array"),
            ));
            &[]
        }
    }
}

/// The objects of a section; anything else in it is a diagnostic.
fn objects<'a>(
    top: &'a Map<String, Value>,
    key: &str,
    d: &mut Vec<Diagnostic>,
) -> Vec<&'a Map<String, Value>> {
    let mut out = Vec::new();
    for (i, item) in array(top, key, d).iter().enumerate() {
        match item.as_object() {
            Some(o) => out.push(o),
            None => d.push(Diagnostic::new(
                SHAPE,
                key,
                format!("item {} of \"{key}\" is not an object", i + 1),
            )),
        }
    }
    out
}

/// The parameter names of a route pattern.
fn route_params(route: &str) -> Vec<String> {
    route
        .split('/')
        .filter_map(|s| s.strip_prefix(':'))
        .map(str::to_string)
        .collect()
}

fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Reads the plan out of the specification's text.
pub fn load(text: &str) -> Result<Plan, Vec<Diagnostic>> {
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
    let mut d = Vec::new();

    let mut screens = Vec::new();
    for o in objects(top, "screens", &mut d) {
        let (Some(id), Some(route)) = (string(o, "id"), string(o, "route")) else {
            d.push(Diagnostic::new(
                SHAPE,
                "screens",
                "a screen needs a string id and a string route",
            ));
            continue;
        };
        screens.push(Screen {
            id: id.to_string(),
            route: route.to_string(),
            params: route_params(route),
            query: strings(o, "query", id, &mut d),
        });
    }

    let mut forms = Vec::new();
    for o in objects(top, "forms", &mut d) {
        let Some(id) = string(o, "id") else {
            d.push(Diagnostic::new(SHAPE, "forms", "a form needs a string id"));
            continue;
        };
        let mut fields = Vec::new();
        let field_values: &[Value] = match o.get("fields") {
            None => &[],
            Some(Value::Array(a)) => a,
            Some(_) => {
                d.push(Diagnostic::new(SHAPE, id, "\"fields\" is not an array"));
                &[]
            }
        };
        for f in field_values {
            let Some(fo) = f.as_object() else {
                d.push(Diagnostic::new(SHAPE, id, "a field is not an object"));
                continue;
            };
            let Some(name) = string(fo, "name") else {
                d.push(Diagnostic::new(SHAPE, id, "a field needs a string name"));
                continue;
            };
            let mut options = Vec::new();
            match fo.get("options") {
                None => {}
                Some(Value::Array(opts)) => {
                    for opt in opts {
                        let pair = opt.as_object().and_then(|oo| {
                            Some((
                                scalar_text(oo.get("value")?)?,
                                string(oo, "label")?.to_string(),
                            ))
                        });
                        match pair {
                            Some(p) => options.push(p),
                            None => d.push(Diagnostic::new(
                                SHAPE,
                                id,
                                format!("an option of field \"{name}\" is not {{value, label}}"),
                            )),
                        }
                    }
                }
                Some(_) => d.push(Diagnostic::new(
                    SHAPE,
                    id,
                    format!("the options of field \"{name}\" are not an array"),
                )),
            }
            fields.push(Field {
                name: name.to_string(),
                label: string(fo, "label").map(str::to_string),
                control: string(fo, "control").unwrap_or("text").to_string(),
                options,
                options_from: fo.get("optionsFrom").is_some(),
            });
        }
        forms.push(Form {
            id: id.to_string(),
            fields,
            submit_label: string(o, "submitLabel").map(str::to_string),
        });
    }

    let mut actions = Vec::new();
    for o in objects(top, "actions", &mut d) {
        let (Some(id), Some(label)) = (string(o, "id"), string(o, "label")) else {
            d.push(Diagnostic::new(
                SHAPE,
                "actions",
                "an action needs a string id and a string label",
            ));
            continue;
        };
        actions.push(Action {
            id: id.to_string(),
            label: label.to_string(),
            confirm: string(o, "confirm").map(str::to_string),
            per_row: o.get("forEach").is_some(),
        });
    }

    let mut events = Vec::new();
    let mut logged = Vec::new();
    for o in objects(top, "events", &mut d) {
        let Some(id) = string(o, "id") else {
            d.push(Diagnostic::new(
                SHAPE,
                "events",
                "an event needs a string id",
            ));
            continue;
        };
        events.push(id.to_string());
        let Some(l) = o.get("logging").and_then(Value::as_object) else {
            continue;
        };
        if l.get("logged").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let mut members = Vec::new();
        if l.get("tenant").and_then(Value::as_str).is_some() {
            members.push("tenant".to_string());
        }
        for f in l
            .get("fields")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match f
                .as_object()
                .and_then(|fo| string(fo, "as").or_else(|| string(fo, "field")))
            {
                Some(name) => members.push(name.to_string()),
                None => d.push(Diagnostic::new(
                    SHAPE,
                    id,
                    "a logged field is not {field, as?}",
                )),
            }
        }
        logged.push(LoggedEvent {
            id: id.to_string(),
            service: string(l, "service").unwrap_or("").to_string(),
            level: string(l, "level").unwrap_or("info").to_string(),
            name: string(l, "name").unwrap_or(id).to_string(),
            members,
        });
    }
    let logs = top
        .get("system")
        .and_then(|s| s.get("telemetry"))
        .and_then(|t| t.get("services"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|s| Some((string(s, "id")?.to_string(), string(s, "log")?.to_string())))
        .collect();
    let mut bindings = Vec::new();
    for o in objects(top, "liveBindings", &mut d) {
        match (string(o, "screen"), string(o, "event")) {
            (Some(s), Some(e)) => bindings.push((s.to_string(), e.to_string())),
            _ => d.push(Diagnostic::new(
                SHAPE,
                string(o, "id").unwrap_or("liveBindings"),
                "a live binding needs a string screen and a string event",
            )),
        }
    }

    // Requirement id -> ref.
    let mut requirements: Vec<(String, Option<String>, bool)> = Vec::new();
    for o in objects(top, "claims", &mut d) {
        if let Some(id) = string(o, "id") {
            requirements.push((
                id.to_string(),
                string(o, "ref").map(str::to_string),
                string(o, "kind") == Some("requirement"),
            ));
        }
    }

    let mut scenarios = Vec::new();
    let mut seen = BTreeSet::new();
    for item in array(top, "uiScenarios", &mut d) {
        let Some(o) = item.as_object() else {
            d.push(Diagnostic::new(
                SHAPE,
                "uiScenarios",
                "a journey is not an object",
            ));
            continue;
        };
        let Some(id) = string(o, "id") else {
            d.push(Diagnostic::new(
                SHAPE,
                "uiScenarios",
                "a journey has no string id",
            ));
            continue;
        };
        if !valid_file_id(id) {
            d.push(Diagnostic::new(
                ID_INVALID,
                id,
                "a journey id names a file, so it must be letters, digits, '.', '_' and '-' and not start with '.'.",
            ));
        }
        if !seen.insert(id.to_string()) {
            d.push(Diagnostic::new(DUPLICATE, id, "two journeys have this id."));
            continue;
        }
        let Some(steps) = o.get("steps").and_then(Value::as_array) else {
            d.push(Diagnostic::new(SHAPE, id, "a journey needs a steps array."));
            continue;
        };
        if o.get("name").is_some_and(|n| !n.is_string()) {
            d.push(Diagnostic::new(
                SHAPE,
                id,
                "the name of a journey is a string.",
            ));
        }
        let mut verifies = Vec::new();
        for v in strings(o, "verifies", id, &mut d) {
            match requirements.iter().find(|(cid, _, _)| *cid == v) {
                None => d.push(Diagnostic::new(
                    VERIFIES,
                    id,
                    format!("\"{v}\" is not a claim of the specification."),
                )),
                Some((_, _, false)) => d.push(Diagnostic::new(
                    VERIFIES,
                    id,
                    format!("\"{v}\" is not a requirement."),
                )),
                Some((_, None, true)) => d.push(Diagnostic::new(
                    VERIFIES,
                    id,
                    format!(
                        "requirement \"{v}\" has no ref, so Concordance cannot link a test to it."
                    ),
                )),
                Some((_, Some(r), true)) => {
                    if !verifies.contains(r) {
                        verifies.push(r.clone());
                    }
                }
            }
        }
        let browsers = strings(o, "browsers", id, &mut d);
        for (i, b) in browsers.iter().enumerate() {
            if b.trim().is_empty() {
                d.push(Diagnostic::new(
                    SHAPE,
                    id,
                    format!("browser {} has no name.", i + 1),
                ));
            } else if browsers[..i].contains(b) {
                d.push(Diagnostic::new(
                    DUPLICATE,
                    id,
                    format!("the journey names browser \"{b}\" twice."),
                ));
            }
        }
        let viewport = match o.get("viewport") {
            None => None,
            Some(v) => {
                let dims = v
                    .as_object()
                    .and_then(|v| Some((v.get("width")?.as_u64()?, v.get("height")?.as_u64()?)));
                match dims.filter(|(w, h)| *w >= 1 && *h >= 1) {
                    Some(dims) => Some(dims),
                    None => {
                        d.push(Diagnostic::new(
                            SHAPE,
                            id,
                            "the viewport needs a positive integer width and height.",
                        ));
                        None
                    }
                }
            }
        };
        scenarios.push(Scenario {
            id: id.to_string(),
            name: string(o, "name").unwrap_or(id).to_string(),
            verifies,
            browsers,
            viewport,
            steps: steps.clone(),
        });
    }
    if scenarios.is_empty() && d.is_empty() {
        d.push(Diagnostic::new(
            NO_SCENARIOS,
            SPEC_ENTITY,
            "the specification has no uiScenarios, so there is nothing to test.",
        ));
    }
    if !d.is_empty() {
        return Err(dedup(d));
    }
    Ok(Plan {
        screens,
        forms,
        actions,
        bindings,
        events,
        logged,
        logs,
        scenarios,
    })
}

fn valid_file_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

// ---- small checks -------------------------------------------------------------------------

/// The `${name}` references of a text, as `(name, well formed)`.
fn refs(text: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        let name = &after[..end];
        out.push((name.to_string(), capture_name(name)));
        rest = &after[end + 1..];
    }
    out
}

fn capture_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A host name with an optional port (`${name}` references count as letters).
fn valid_host(host: &str) -> bool {
    let host = replace_refs(host, "x");
    let (name, port) = match host.rsplit_once(':') {
        Some((n, p)) => (n, Some(p)),
        None => (host.as_str(), None),
    };
    if port.is_some_and(|p| p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit())) {
        return false;
    }
    !name.is_empty()
        && name.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// An absolute path: `/...` without white space or a fragment.
fn valid_path(path: &str) -> bool {
    path.starts_with('/') && !path.contains(|c: char| c.is_whitespace() || c == '#')
}

fn replace_refs(text: &str, with: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        out.push_str(&rest[..start]);
        out.push_str(with);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// A path into a JSON reply: names and indexes joined by `.`.
fn valid_json_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('.').all(|seg| {
            !seg.is_empty()
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$' | b'-'))
        })
}

fn element_attribute(a: &str) -> bool {
    a.strip_prefix("aria-")
        .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_lowercase()))
        || data_attribute(a)
        || matches!(a, "value" | "href" | "lang" | "dir")
}

fn root_attribute(a: &str) -> bool {
    data_attribute(a) || matches!(a, "lang" | "dir")
}

fn data_attribute(a: &str) -> bool {
    a.strip_prefix("data-").is_some_and(|r| {
        r.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && r.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

// ---- lowering -----------------------------------------------------------------------------

/// A JavaScript string literal.
fn js(text: &str) -> String {
    crate::codetext::js::double_quoted(text)
}

/// A JavaScript string: a literal, or the text with its `${name}` captures filled in.
fn jstext(text: &str) -> String {
    if refs(text).is_empty() {
        js(text)
    } else {
        format!("j.sub({})", js(text))
    }
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Percent-encodes a value but keeps its `${name}` references, which the runtime fills in
/// encoded.
fn encode_keeping_refs(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        out.push_str(&percent_encode(&rest[..start]));
        out.push_str(&rest[start..start + 2 + end + 1]);
        rest = &after[end + 1..];
    }
    out.push_str(&percent_encode(rest));
    out
}

fn scalar(v: &Value) -> Option<String> {
    scalar_text(v)
}

/// The address a `goto` opens: the route with its parameters, then the query parameters that
/// were given, in the screen's declared order.
pub fn goto_path(screen: &Screen, params: &Map<String, Value>) -> String {
    let mut path = String::new();
    if screen.route == "/" {
        path.push('/');
    } else {
        for seg in screen.route.split('/').skip(1) {
            path.push('/');
            match seg.strip_prefix(':') {
                Some(name) => path.push_str(&encode_keeping_refs(
                    &params.get(name).and_then(scalar).unwrap_or_default(),
                )),
                None => path.push_str(seg),
            }
        }
    }
    let query: Vec<String> = screen
        .query
        .iter()
        .filter_map(|q| {
            params
                .get(q)
                .and_then(scalar)
                .map(|v| format!("{q}={}", encode_keeping_refs(&v)))
        })
        .collect();
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }
    path
}

fn regex_escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if "\\^$.|?*+()[]{}/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A JavaScript regular expression literal matching the path of a route. The catch-all route
/// matches any path; which screen answers it is decided against the other routes.
pub fn route_regex(route: &str) -> String {
    if route == "/" {
        return r"/^\/$/".to_string();
    }
    if route.starts_with('*') {
        return r"/^\/.*$/".to_string();
    }
    let mut out = String::from("/^");
    for seg in route.split('/').skip(1) {
        out.push_str(r"\/");
        if seg.starts_with(':') {
            out.push_str(r"[^\/]+");
        } else {
            out.push_str(&regex_escape(seg));
        }
    }
    out.push_str("$/");
    out
}

/// What the lowering of one journey tracks.
struct State {
    browsers: Vec<String>,
    current: usize,
    /// Per browser: has it navigated, and which screen is known to be open in it (None after a
    /// step that may navigate, until the next goto or expectScreen)?
    navigated: Vec<bool>,
    screen: Vec<Option<String>>,
    /// Per browser: the markers and request logs it set.
    marks: Vec<BTreeSet<String>>,
    logs: Vec<BTreeSet<String>>,
    /// The captured values, shared by every browser.
    captured: BTreeSet<String>,
}

impl State {
    fn new(browsers: &[String]) -> Self {
        let browsers = if browsers.is_empty() {
            vec![String::new()]
        } else {
            browsers.to_vec()
        };
        let n = browsers.len();
        Self {
            browsers,
            current: 0,
            navigated: vec![false; n],
            screen: vec![None; n],
            marks: vec![BTreeSet::new(); n],
            logs: vec![BTreeSet::new(); n],
            captured: BTreeSet::new(),
        }
    }
}

/// The context of one step's diagnostics.
struct Cx<'a> {
    scenario: &'a str,
    n: usize,
    kind: &'a str,
}

impl Cx<'_> {
    fn err(&self, code: &str, msg: impl AsRef<str>) -> Vec<Diagnostic> {
        vec![Diagnostic::new(
            code,
            self.scenario,
            format!("step {} ({}): {}.", self.n, self.kind, msg.as_ref()),
        )]
    }
}

type Lines = Result<Vec<String>, Vec<Diagnostic>>;

/// The options of a timed matcher, `{ timeout: n }` or nothing.
fn timeout_of(cx: &Cx, o: &Map<String, Value>, key: &str) -> Result<Option<u64>, Vec<Diagnostic>> {
    match o.get(key) {
        None => Ok(None),
        Some(v) => match v.as_u64().filter(|t| *t >= 1) {
            Some(t) => Ok(Some(t)),
            None => Err(cx.err(
                TIMEOUT,
                format!("\"{key}\" is a positive integer of milliseconds"),
            )),
        },
    }
}

fn timeout_arg(t: Option<u64>) -> String {
    t.map(|t| format!("{{ timeout: {t} }}")).unwrap_or_default()
}

fn timeout_param(t: Option<u64>) -> String {
    t.map(|t| t.to_string())
        .unwrap_or_else(|| "undefined".to_string())
}

#[derive(Clone, Copy)]
struct ElementOpts {
    name_required: bool,
    strict_keys: bool,
    include_hidden: bool,
}

const ELEMENT: ElementOpts = ElementOpts {
    name_required: true,
    strict_keys: true,
    include_hidden: false,
};

/// The locator of an element selector: role, accessible name (exact) or name pattern, heading
/// level, inside the element it is `within`.
fn element(cx: &Cx, t: &Value, scope: &str, opts: ElementOpts) -> Result<String, Vec<Diagnostic>> {
    let Some(o) = t.as_object() else {
        return Err(cx.err(
            STEP,
            "an element is {role, name or namePattern, level?, within?}",
        ));
    };
    if opts.strict_keys {
        let extra: Vec<&str> = o
            .keys()
            .map(String::as_str)
            .filter(|k| !TARGET.contains(k))
            .collect();
        if !extra.is_empty() {
            return Err(cx.err(
                STEP,
                format!(
                    "{} is not part of an element (role, name, namePattern, level, within)",
                    extra.join(", ")
                ),
            ));
        }
    }
    let Some(role) = string(o, "role") else {
        return Err(cx.err(STEP, "an element needs a string \"role\""));
    };
    if !ARIA_ROLES.contains(&role) {
        return Err(cx.err(ROLE, format!("\"{role}\" is not a WAI-ARIA role")));
    }
    let mut options = Vec::new();
    match (o.get("name"), o.get("namePattern")) {
        (Some(_), Some(_)) => {
            return Err(cx.err(
                STEP,
                "an element has a \"name\" or a \"namePattern\", not both",
            ))
        }
        (None, None) => {
            if opts.name_required {
                return Err(cx.err(
                    STEP,
                    "an element needs a \"name\" (its accessible name) or a \"namePattern\"",
                ));
            }
        }
        (Some(name), None) => {
            let Some(name) = name.as_str().filter(|n| !n.trim().is_empty()) else {
                return Err(cx.err(STEP, "the accessible name is a non-empty string"));
            };
            options.push(format!("name: {}", jstext(name)));
            options.push("exact: true".to_string());
        }
        (None, Some(p)) => {
            let Some(p) = p.as_str().filter(|p| !p.is_empty()) else {
                return Err(cx.err(STEP, "a name pattern is a non-empty regular expression"));
            };
            options.push(format!("name: j.re({})", js(p)));
        }
    }
    if let Some(level) = o.get("level") {
        match level.as_u64().filter(|l| (1..=6).contains(l)) {
            Some(l) if role == "heading" => options.push(format!("level: {l}")),
            _ => return Err(cx.err(LEVEL, "\"level\" applies to a heading and is 1 to 6")),
        }
    }
    if opts.include_hidden {
        options.push("includeHidden: true".to_string());
    }
    let scope = match o.get("within") {
        None => scope.to_string(),
        Some(w) => element(
            cx,
            w,
            scope,
            ElementOpts {
                name_required: false,
                strict_keys: true,
                include_hidden: false,
            },
        )?,
    };
    Ok(if options.is_empty() {
        format!("{scope}.getByRole({})", js(role))
    } else {
        format!(
            "{scope}.getByRole({}, {{ {} }})",
            js(role),
            options.join(", ")
        )
    })
}

/// The element part of a step's object: its keys other than `skip`.
fn target_part(o: &Map<String, Value>, skip: &[&str]) -> Value {
    Value::Object(
        o.iter()
            .filter(|(k, _)| !skip.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

/// `text` or `pattern`, exactly one, as a JavaScript matcher: a text is matched literally and
/// case-sensitively (`j.text`), a pattern as a regular expression (`j.re`).
fn text_matcher(cx: &Cx, o: &Map<String, Value>) -> Result<String, Vec<Diagnostic>> {
    match (o.get("text"), o.get("pattern")) {
        (Some(t), None) => match t.as_str().filter(|t| !t.trim().is_empty()) {
            Some(t) => Ok(format!("j.text({})", js(t))),
            None => Err(cx.err(STEP, "\"text\" is a non-empty string")),
        },
        (None, Some(p)) => match p.as_str().filter(|p| !p.is_empty()) {
            Some(p) => Ok(format!("j.re({})", js(p))),
            None => Err(cx.err(STEP, "\"pattern\" is a non-empty regular expression")),
        },
        _ => Err(cx.err(STEP, "it needs exactly one of \"text\" and \"pattern\"")),
    }
}

/// Every string inside a value, with its key path.
fn visit_strings<'v>(v: &'v Value, at: &mut Vec<String>, out: &mut Vec<(Vec<String>, &'v str)>) {
    match v {
        Value::String(s) => out.push((at.clone(), s)),
        Value::Array(items) => {
            for (i, x) in items.iter().enumerate() {
                at.push(i.to_string());
                visit_strings(x, at, out);
                at.pop();
            }
        }
        Value::Object(o) => {
            for (k, x) in o {
                at.push(k.clone());
                visit_strings(x, at, out);
                at.pop();
            }
        }
        _ => {}
    }
}

/// Declares a captured value.
fn define(cx: &Cx, st: &mut State, name: &Value) -> Result<String, Vec<Diagnostic>> {
    let Some(name) = name.as_str().filter(|n| capture_name(n)) else {
        return Err(cx.err(
            STEP,
            "a captured value is named by a letter or _, then letters, digits or _",
        ));
    };
    if !st.captured.insert(name.to_string()) {
        return Err(cx.err(
            CAPTURE,
            format!("value \"{name}\" is already captured by an earlier step"),
        ));
    }
    Ok(name.to_string())
}

fn host_of(cx: &Cx, o: &Map<String, Value>) -> Result<Option<String>, Vec<Diagnostic>> {
    match o.get("host") {
        None => Ok(None),
        Some(h) => match h.as_str().filter(|h| valid_host(h)) {
            Some(h) => Ok(Some(h.to_string())),
            None => Err(cx.err(
                HOST,
                format!("host {h} is not a host name with an optional port"),
            )),
        },
    }
}

fn check_path(cx: &Cx, p: Option<&Value>) -> Result<String, Vec<Diagnostic>> {
    match p.and_then(Value::as_str).filter(|p| valid_path(p)) {
        Some(p) => Ok(p.to_string()),
        None => Err(cx.err(
            PATH,
            "\"path\" is an absolute path (\"/...\" without spaces or a fragment)",
        )),
    }
}

/// The JavaScript lines of one step. The mapping, step by step:
///
/// | step | Playwright |
/// |---|---|
/// | `goto`, `gotoPath`, `reload`, `back` | navigate, then wait for the application's page marker (failing on timeout) |
/// | `click`, `focus` | `getByRole(role, { name, exact: true, level })` within its `within`, then `click`/`focus` |
/// | `clickAction` | the action's button by its label with `${item.<field>}` filled in, then the confirmation |
/// | `fill` by form field | `getByLabel(label, { exact: true })` and `fill`, `setChecked`, `selectOption` or a radio |
/// | `fill` by element | the element's `fill` |
/// | `submit` | click the button named by the form's `submitLabel` |
/// | `press` | `page.keyboard.press(key)` |
/// | `expectText`, `expectNoText` | a visible element matching the text (case-sensitive) or pattern, or none |
/// | `expectTitle` | `toHaveTitle` |
/// | `expectScreen` | the screen the address belongs to; with params, the address itself |
/// | `expectRole` | `toBeVisible`, `toBeEnabled`, ...; `hidden` needs the element to exist |
/// | `expectCount` | `toHaveCount` |
/// | `expectAttribute` | `toHaveAttribute` (`toHaveValue` for `value`) on the element or the document element |
/// | `expectStatus` | the status of the browser's last navigation |
/// | `mark`, `expectNoReload` | a marker on `window` that only survives without a reload |
/// | `recordRequests`, `expectRequests`, `expectNoRequest` | the browser's requests and WebSockets since the log started |
/// | `apiCall` | a request with the browser's cookies (or none), its status, reply and captures |
/// | `expectCookie`, `setCookie`, `capture` | the browser context's cookies, the last response, the page |
/// | `emulate`, `setViewport`, `goOffline`, `goOnline` | media, device, viewport and network emulation |
/// | `expectNoOverflow`, `expectBeside` | layout measured on the page |
/// | `switchBrowser` | another context and page, kept by name |
/// | `axeScan` | `AxeBuilder` with the suite's tags, no violations |
/// | `logScan` | no line written since the test began matches a pattern |
/// | `expectLog` | a line with the event's name and fields, level, allowlist and linked correlation |
/// | `waitFor`, `expectNoEvent` | a live notice followed by a view refetch; no live notice for a while |
fn lower_step(plan: &Plan, st: &mut State, scenario: &str, index: usize, step: &Value) -> Lines {
    let n = index + 1;
    let Some(obj) = step.as_object().filter(|o| o.len() == 1) else {
        return Err(vec![Diagnostic::new(
            STEP,
            scenario,
            format!("step {n}: a step is an object with exactly one key."),
        )]);
    };
    let (kind, raw) = obj.iter().next().expect("one key");
    let cx = Cx {
        scenario,
        n,
        kind: kind.as_str(),
    };
    let Some((_, allowed)) = STEP_KINDS.iter().find(|(k, _)| k == kind) else {
        return Err(cx.err(
            STEP,
            format!(
                "\"{kind}\" is not a step (elements are selected by role and accessible name only)"
            ),
        ));
    };

    // Uses of captured values come first: a step may use only what an earlier step captured.
    let mut texts = Vec::new();
    visit_strings(raw, &mut Vec::new(), &mut texts);
    for (at, s) in &texts {
        if kind == "capture" && at.len() == 1 && at[0] == "name" {
            continue;
        }
        for (name, ok) in refs(s) {
            if !ok || !st.captured.contains(&name) {
                return Err(cx.err(
                    CAPTURE,
                    format!("\"${{{name}}}\" uses a value no earlier step captured"),
                ));
            }
        }
    }

    let flag = Value::Object(Map::new());
    let v = if FLAG_STEPS.contains(&kind.as_str()) && raw == &Value::Bool(true) {
        &flag
    } else {
        raw
    };
    let screen_object;
    let v = if kind == "expectScreen" && v.is_string() {
        let mut m = Map::new();
        m.insert("screen".into(), v.clone());
        screen_object = Value::Object(m);
        &screen_object
    } else {
        v
    };
    let empty = Map::new();
    let o: &Map<String, Value> = match allowed {
        None => &empty,
        Some(keys) => {
            let Some(o) = v.as_object() else {
                return Err(cx.err(
                    STEP,
                    if keys.is_empty() {
                        "the value is an object with no keys (or true)".to_string()
                    } else {
                        format!("the value is an object with {}", keys.join(", "))
                    },
                ));
            };
            let extra: Vec<&str> = o
                .keys()
                .map(String::as_str)
                .filter(|k| !keys.contains(k))
                .collect();
            if !extra.is_empty() {
                return Err(cx.err(
                    STEP,
                    format!(
                        "{} is not part of this step (elements are selected by role and accessible name only)",
                        extra.join(", ")
                    ),
                ));
            }
            o
        }
    };
    let cur = st.current;
    let page = "j.page";

    match kind.as_str() {
        "goto" => {
            let host = host_of(&cx, o)?;
            let Some(sid) = string(o, "screen") else {
                return Err(cx.err(STEP, "it needs a \"screen\""));
            };
            let Some(screen) = plan.screens.iter().find(|s| s.id == sid) else {
                return Err(cx.err(REFERENCE, format!("\"{sid}\" is not a screen")));
            };
            if screen.catch_all() {
                return Err(cx.err(
                    GOTO_PARAMS,
                    format!("screen \"{sid}\" is the catch-all screen and has no address of its own; open an address no screen answers with gotoPath"),
                ));
            }
            let params = screen_params(&cx, screen, o.get("params"))?;
            st.navigated[cur] = true;
            st.screen[cur] = Some(sid.to_string());
            let path = goto_path(screen, &params);
            let path = if refs(&path).is_empty() {
                js(&path)
            } else {
                format!("j.subPath({})", js(&path))
            };
            Ok(vec![match host {
                None => format!("await j.goto({path});"),
                Some(h) => format!("await j.goto({path}, {});", jstext(&h)),
            }])
        }
        "gotoPath" => {
            let path = check_path(&cx, o.get("path"))?;
            let host = host_of(&cx, o)?;
            st.navigated[cur] = true;
            st.screen[cur] = None;
            Ok(vec![match host {
                None => format!("await j.goto({});", jstext(&path)),
                Some(h) => format!("await j.goto({}, {});", jstext(&path), jstext(&h)),
            }])
        }
        "reload" => {
            st.navigated[cur] = true;
            Ok(vec!["await j.reload();".to_string()])
        }
        "back" => {
            st.screen[cur] = None;
            Ok(vec!["await j.back();".to_string()])
        }
        "click" | "focus" => {
            let loc = element(&cx, v, page, ELEMENT)?;
            if kind == "click" {
                st.screen[cur] = None;
            }
            Ok(vec![format!("await {loc}.{kind}();")])
        }
        "clickAction" => lower_click_action(plan, &cx, st, o),
        "fill" => lower_fill(plan, &cx, o),
        "submit" => {
            let Some(fid) = string(o, "form") else {
                return Err(cx.err(STEP, "it needs a \"form\""));
            };
            let Some(form) = plan.forms.iter().find(|f| f.id == fid) else {
                return Err(cx.err(REFERENCE, format!("\"{fid}\" is not a form")));
            };
            let Some(label) = &form.submit_label else {
                return Err(cx.err(
                    SUBMIT_LABEL,
                    format!(
                        "form \"{fid}\" has no submitLabel, so its button has no accessible name"
                    ),
                ));
            };
            st.screen[cur] = None;
            Ok(vec![format!(
                "await {page}.getByRole(\"button\", {{ name: {}, exact: true }}).click();",
                js(label)
            )])
        }
        "press" => match v.as_str().filter(|s| !s.trim().is_empty()) {
            Some(key) => {
                st.screen[cur] = None;
                Ok(vec![format!("await {page}.keyboard.press({});", js(key))])
            }
            None => Err(cx.err(
                STEP,
                "the value is the key to press, such as Enter or Control+End",
            )),
        },
        "expectText" | "expectNoText" => {
            let matcher = text_matcher(&cx, o)?;
            let timeout = timeout_of(&cx, o, "timeoutMs")?;
            let scope = match o.get("within") {
                None => "null".to_string(),
                Some(w) => element(
                    &cx,
                    w,
                    page,
                    ElementOpts {
                        name_required: false,
                        strict_keys: true,
                        include_hidden: false,
                    },
                )?,
            };
            Ok(vec![format!(
                "await j.{kind}({scope}, {matcher}, {});",
                timeout_param(timeout)
            )])
        }
        "expectTitle" => {
            let timeout = timeout_of(&cx, o, "timeoutMs")?;
            let matcher = match (o.get("text"), o.get("pattern")) {
                (Some(t), None) => match t.as_str().filter(|t| !t.trim().is_empty()) {
                    Some(t) => jstext(t),
                    None => return Err(cx.err(STEP, "\"text\" is a non-empty string")),
                },
                (None, Some(p)) => match p.as_str().filter(|p| !p.is_empty()) {
                    Some(p) => format!("j.re({})", js(p)),
                    None => {
                        return Err(cx.err(STEP, "\"pattern\" is a non-empty regular expression"))
                    }
                },
                _ => return Err(cx.err(STEP, "it needs exactly one of \"text\" and \"pattern\"")),
            };
            let opts = timeout_arg(timeout);
            Ok(vec![if opts.is_empty() {
                format!("await expect({page}).toHaveTitle({matcher});")
            } else {
                format!("await expect({page}).toHaveTitle({matcher}, {opts});")
            }])
        }
        "expectScreen" => {
            let timeout = timeout_of(&cx, o, "timeoutMs")?;
            let Some(sid) = string(o, "screen") else {
                return Err(cx.err(
                    STEP,
                    "the value is a screen id, or {screen, params?, timeoutMs?}",
                ));
            };
            let Some(screen) = plan.screens.iter().find(|s| s.id == sid) else {
                return Err(cx.err(REFERENCE, format!("\"{sid}\" is not a screen")));
            };
            let mut lines = vec![format!(
                "await j.expectScreen({}, {});",
                js(sid),
                timeout_param(timeout)
            )];
            if o.contains_key("params") {
                let params = screen_params(&cx, screen, o.get("params"))?;
                let mut route = Map::new();
                for p in &screen.params {
                    if let Some(v) = params.get(p) {
                        route.insert(p.clone(), v.clone());
                    }
                }
                let route_path = goto_path(screen, &route);
                let query: Map<String, Value> = screen
                    .query
                    .iter()
                    .filter_map(|q| Some((q.clone(), Value::String(scalar(params.get(q)?)?))))
                    .collect();
                let declared: Vec<Value> =
                    screen.query.iter().cloned().map(Value::String).collect();
                lines.push(format!(
                    "await j.expectAddress({}, {}, {}, {});",
                    if refs(&route_path).is_empty() {
                        js(&route_path)
                    } else {
                        format!("j.subPath({})", js(&route_path))
                    },
                    Value::Object(query),
                    Value::Array(declared),
                    timeout_param(timeout)
                ));
            }
            st.screen[cur] = Some(sid.to_string());
            Ok(lines)
        }
        "expectRole" => {
            let timeout = timeout_of(&cx, o, "timeoutMs")?;
            let state = match o.get("state") {
                None => "visible",
                Some(s) => s.as_str().unwrap_or(""),
            };
            let hidden = state == "hidden";
            let loc = element(
                &cx,
                &target_part(o, &["state", "timeoutMs"]),
                page,
                ElementOpts {
                    include_hidden: hidden,
                    ..ELEMENT
                },
            )?;
            let opts = timeout_arg(timeout);
            let line = match state {
                "hidden" => format!("await j.expectHidden({loc}, {});", timeout_param(timeout)),
                "visible" => format!("await expect({loc}).toBeVisible({opts});"),
                "enabled" => format!("await expect({loc}).toBeEnabled({opts});"),
                "disabled" => format!("await expect({loc}).toBeDisabled({opts});"),
                "focused" => format!("await expect({loc}).toBeFocused({opts});"),
                "checked" => format!("await expect({loc}).toBeChecked({opts});"),
                "unchecked" => format!(
                    "await expect({loc}).toBeChecked({{ checked: false{} }});",
                    timeout.map(|t| format!(", timeout: {t}")).unwrap_or_default()
                ),
                other => {
                    return Err(cx.err(
                        STEP,
                        format!("state \"{other}\" is not visible, hidden, enabled, disabled, focused, checked or unchecked"),
                    ))
                }
            };
            Ok(vec![line])
        }
        "expectCount" => {
            let timeout = timeout_of(&cx, o, "timeoutMs")?;
            let Some(count) = o.get("count").and_then(Value::as_u64) else {
                return Err(cx.err(STEP, "\"count\" is an integer of at least 0"));
            };
            let loc = element(
                &cx,
                &target_part(o, &["count", "timeoutMs"]),
                page,
                ElementOpts {
                    name_required: false,
                    ..ELEMENT
                },
            )?;
            let opts = timeout_arg(timeout);
            Ok(vec![if opts.is_empty() {
                format!("await expect({loc}).toHaveCount({count});")
            } else {
                format!("await expect({loc}).toHaveCount({count}, {opts});")
            }])
        }
        "expectAttribute" => {
            let timeout = timeout_of(&cx, o, "timeoutMs")?;
            let target = target_part(o, &["attribute", "value", "pattern", "timeoutMs"]);
            let root = target.as_object().is_some_and(Map::is_empty);
            let Some(attribute) = string(o, "attribute") else {
                return Err(cx.err(ATTRIBUTE, "it needs the \"attribute\" to read"));
            };
            if root && !root_attribute(attribute) {
                return Err(cx.err(
                    ATTRIBUTE,
                    format!("attribute \"{attribute}\" of the document element is not a data-* attribute, lang or dir"),
                ));
            }
            if !root && !element_attribute(attribute) {
                return Err(cx.err(
                    ATTRIBUTE,
                    format!("attribute \"{attribute}\" is not an aria-* or data-* attribute, value, href, lang or dir"),
                ));
            }
            let expected = match (o.get("value"), o.get("pattern")) {
                (Some(v), None) => match v.as_str() {
                    Some(v) => jstext(v),
                    None => return Err(cx.err(STEP, "\"value\" is a string")),
                },
                (None, Some(p)) => match p.as_str().filter(|p| !p.is_empty()) {
                    Some(p) => format!("j.re({})", js(p)),
                    None => {
                        return Err(cx.err(STEP, "\"pattern\" is a non-empty regular expression"))
                    }
                },
                _ => return Err(cx.err(STEP, "it needs exactly one of \"value\" and \"pattern\"")),
            };
            let loc = if root {
                format!("{page}.locator(\"html\")")
            } else {
                element(&cx, &target, page, ELEMENT)?
            };
            let opts = timeout_arg(timeout);
            let sep = if opts.is_empty() { "" } else { ", " };
            Ok(vec![if attribute == "value" && !root {
                format!("await expect({loc}).toHaveValue({expected}{sep}{opts});")
            } else {
                format!(
                    "await expect({loc}).toHaveAttribute({}, {expected}{sep}{opts});",
                    js(attribute)
                )
            }])
        }
        "expectStatus" => {
            let Some(code) = v.as_u64().filter(|c| (100..=599).contains(c)) else {
                return Err(cx.err(STEP, "the value is an HTTP status from 100 to 599"));
            };
            if !st.navigated[cur] {
                return Err(cx.err(
                    STATUS_WITHOUT_GOTO,
                    "the status is that of a navigation, and this browser has not navigated yet",
                ));
            }
            Ok(vec![format!(
                "expect(j.response?.status(), \"the status of the last navigation\").toBe({code});"
            )])
        }
        "mark" | "recordRequests" => {
            let Some(name) = v.as_str().filter(|s| capture_name(s)) else {
                return Err(cx.err(
                    STEP,
                    "the value is a name: a letter or _, then letters, digits or _",
                ));
            };
            if kind == "mark" {
                st.marks[cur].insert(name.to_string());
                Ok(vec![format!("await j.mark({});", js(name))])
            } else {
                st.logs[cur].insert(name.to_string());
                Ok(vec![format!("j.recordRequests({});", js(name))])
            }
        }
        "expectNoReload" => {
            let Some(name) = v.as_str() else {
                return Err(cx.err(STEP, "the value is the name of a marker set by a mark step"));
            };
            if !st.marks[cur].contains(name) {
                return Err(cx.err(
                    MARK,
                    format!(
                        "marker \"{name}\" was not set by an earlier mark step in this browser"
                    ),
                ));
            }
            Ok(vec![format!("await j.expectNoReload({});", js(name))])
        }
        "expectRequests" | "expectNoRequest" => {
            let Some(log) = string(o, "log") else {
                return Err(cx.err(STEP, "it needs the \"log\" a recordRequests step started"));
            };
            if !st.logs[cur].contains(log) {
                return Err(cx.err(
                    REQUEST_LOG,
                    format!("request log \"{log}\" was not started by an earlier recordRequests step in this browser"),
                ));
            }
            let key = if kind == "expectRequests" {
                "to"
            } else {
                "path"
            };
            let Some(p) = string(o, key).filter(|p| !p.is_empty()) else {
                return Err(cx.err(STEP, format!("\"{key}\" is a non-empty regular expression")));
            };
            if kind == "expectNoRequest" {
                return Ok(vec![format!(
                    "j.expectNoRequest({}, j.urlRe({}));",
                    js(log),
                    js(p)
                )]);
            }
            let mut opts = Vec::new();
            for b in ["only", "fromWorker"] {
                match o.get(b) {
                    None => {}
                    Some(Value::Bool(x)) => opts.push(format!("{b}: {x}")),
                    Some(_) => return Err(cx.err(STEP, format!("\"{b}\" is a boolean"))),
                }
            }
            match o.get("atLeast") {
                None => {}
                Some(a) => match a.as_u64() {
                    Some(a) => opts.push(format!("atLeast: {a}")),
                    None => return Err(cx.err(STEP, "\"atLeast\" is an integer of at least 0")),
                },
            }
            Ok(vec![format!(
                "await j.expectRequests({}, j.urlRe({}), {{ {} }});",
                js(log),
                js(p),
                opts.join(", ")
            )])
        }
        "apiCall" => lower_api_call(&cx, st, o),
        "expectCookie" | "setCookie" => {
            if string(o, "name").is_none_or(|n| n.trim().is_empty()) {
                return Err(cx.err(STEP, "it needs the cookie \"name\""));
            }
            check_cookie_attributes(&cx, o.get("attributes"))?;
            if kind == "expectCookie" {
                match o.get("absent") {
                    None | Some(Value::Bool(false)) => {}
                    Some(Value::Bool(true)) => {
                        if o.contains_key("attributes") {
                            return Err(cx.err(STEP, "an absent cookie has no attributes"));
                        }
                    }
                    Some(_) => return Err(cx.err(STEP, "\"absent\" is a boolean")),
                }
            } else {
                if string(o, "value").is_none() {
                    return Err(cx.err(STEP, "it needs the cookie \"value\""));
                }
                if string(o, "host").is_none() {
                    return Err(cx.err(STEP, "it needs the \"host\" the cookie is set for"));
                }
                host_of(&cx, o)?;
            }
            Ok(vec![format!(
                "await j.{kind}({});",
                Value::Object(o.clone())
            )])
        }
        "capture" => {
            let from = string(o, "from").unwrap_or("");
            let need: &[&str] = match from {
                "cookie" => &["cookie"],
                "header" => &["header"],
                "text" => &["element"],
                "attribute" => &["element", "attribute"],
                _ => return Err(cx.err(STEP, "\"from\" is cookie, header, text or attribute")),
            };
            for x in ["cookie", "header", "element", "attribute"] {
                if o.contains_key(x) != need.contains(&x) {
                    return Err(cx.err(
                        STEP,
                        format!("a capture from {from} takes {}", need.join(" and ")),
                    ));
                }
            }
            let line = match from {
                "cookie" | "header" => {
                    let Some(source) = string(o, from).filter(|s| !s.is_empty()) else {
                        return Err(cx.err(STEP, format!("\"{from}\" is a non-empty name")));
                    };
                    let name = define(&cx, st, o.get("name").unwrap_or(&Value::Null))?;
                    let method = if from == "cookie" {
                        "captureCookie"
                    } else {
                        "captureHeader"
                    };
                    format!("await j.{method}({}, {});", js(&name), js(source))
                }
                _ => {
                    let loc =
                        element(&cx, o.get("element").unwrap_or(&Value::Null), page, ELEMENT)?;
                    let attribute = if from == "attribute" {
                        let a = string(o, "attribute").unwrap_or("");
                        if !element_attribute(a) {
                            return Err(cx.err(
                                ATTRIBUTE,
                                format!("attribute \"{a}\" is not an aria-* or data-* attribute, value, href, lang or dir"),
                            ));
                        }
                        Some(a)
                    } else {
                        None
                    };
                    let name = define(&cx, st, o.get("name").unwrap_or(&Value::Null))?;
                    match attribute {
                        None => format!("await j.captureText({}, {loc});", js(&name)),
                        Some(a) => {
                            format!("await j.captureAttribute({}, {loc}, {});", js(&name), js(a))
                        }
                    }
                }
            };
            Ok(vec![line])
        }
        "emulate" => {
            if o.is_empty() {
                return Err(cx.err(
                    STEP,
                    "it sets at least one of colorScheme, reducedMotion, touch and mobile",
                ));
            }
            let ok = |key: &str, values: &[&str]| match o.get(key) {
                None => true,
                Some(v) => v.as_str().is_some_and(|s| values.contains(&s)),
            };
            if !ok("colorScheme", &["light", "dark", "no-preference"]) {
                return Err(cx.err(STEP, "\"colorScheme\" is light, dark or no-preference"));
            }
            if !ok("reducedMotion", &["reduce", "no-preference"]) {
                return Err(cx.err(STEP, "\"reducedMotion\" is reduce or no-preference"));
            }
            for b in ["touch", "mobile"] {
                if o.get(b).is_some_and(|x| !x.is_boolean()) {
                    return Err(cx.err(STEP, format!("\"{b}\" is a boolean")));
                }
            }
            if (o.contains_key("touch") || o.contains_key("mobile")) && st.navigated[cur] {
                return Err(cx.err(
                    EMULATE_LATE,
                    "touch and mobile are properties of the browser, so they are emulated before its first navigation",
                ));
            }
            Ok(vec![format!(
                "await j.emulate({});",
                Value::Object(o.clone())
            )])
        }
        "expectNoOverflow" => Ok(vec!["await j.expectNoOverflow();".to_string()]),
        "expectBeside" => {
            let a = element(&cx, o.get("a").unwrap_or(&Value::Null), page, ELEMENT)?;
            let b = element(&cx, o.get("b").unwrap_or(&Value::Null), page, ELEMENT)?;
            let Some(side) =
                string(o, "side").filter(|s| ["left", "right", "above", "below"].contains(s))
            else {
                return Err(cx.err(
                    STEP,
                    "\"side\" is left, right, above or below: where a is relative to b",
                ));
            };
            Ok(vec![format!(
                "await j.expectBeside({a}, {b}, {});",
                js(side)
            )])
        }
        "switchBrowser" => {
            let Some(name) = v.as_str() else {
                return Err(cx.err(STEP, "the value is the name of a browser"));
            };
            let Some(i) = st.browsers.iter().position(|b| b == name && !b.is_empty()) else {
                return Err(cx.err(
                    REFERENCE,
                    format!("\"{name}\" is not one of the journey's browsers"),
                ));
            };
            st.current = i;
            Ok(vec![format!("j.use({});", js(name))])
        }
        "goOffline" | "goOnline" => Ok(vec![format!(
            "await j.context.setOffline({});",
            kind == "goOffline"
        )]),
        "setViewport" => {
            let dims = o
                .get("width")
                .and_then(Value::as_u64)
                .zip(o.get("height").and_then(Value::as_u64));
            match dims.filter(|(w, h)| *w >= 1 && *h >= 1) {
                Some((w, h)) => Ok(vec![format!(
                    "await {page}.setViewportSize({{ width: {w}, height: {h} }});"
                )]),
                None => Err(cx.err(STEP, "a viewport needs a positive integer width and height")),
            }
        }
        "axeScan" => Ok(vec![format!("await j.axe({});", js(&format!("step {n}")))]),
        "logScan" => {
            let Some(patterns) = o
                .get("forbid")
                .and_then(Value::as_array)
                .filter(|a| !a.is_empty())
            else {
                return Err(cx.err(STEP, "it needs a non-empty \"forbid\" list of patterns"));
            };
            let mut list = Vec::new();
            for p in patterns {
                match p.as_str().filter(|p| !p.is_empty()) {
                    Some(p) => list.push(format!("j.re({})", js(p))),
                    None => {
                        return Err(cx.err(
                            STEP,
                            "a forbidden pattern is a non-empty regular expression",
                        ))
                    }
                }
            }
            if plan.logs.is_empty() {
                return Err(cx.err(NO_LOGS, "system.telemetry names no service with a log file"));
            }
            Ok(vec![format!("await j.logScan([{}]);", list.join(", "))])
        }
        "expectLog" => {
            let timeout = timeout_of(&cx, o, "timeoutMs")?;
            let (Some(service), Some(event)) = (string(o, "service"), string(o, "event")) else {
                return Err(cx.err(
                    STEP,
                    "it needs the \"service\" whose log is read and the \"event\"",
                ));
            };
            if !plan.events.iter().any(|e| e == event) {
                return Err(cx.err(REFERENCE, format!("\"{event}\" is not an event")));
            }
            let Some(logged) = plan.logged.iter().find(|l| l.id == event) else {
                return Err(cx.err(LOG_EVENT, format!("event \"{event}\" is not logged")));
            };
            if logged.service != service {
                return Err(cx.err(
                    LOG_EVENT,
                    format!(
                        "event \"{event}\" is logged by service \"{}\", not \"{service}\"",
                        logged.service
                    ),
                ));
            }
            if !plan.logs.iter().any(|(s, _)| s == service) {
                return Err(cx.err(
                    LOG_EVENT,
                    format!("service \"{service}\" names no log file (system.telemetry)"),
                ));
            }
            let mut expected = Map::new();
            if let Some(fields) = o.get("fields") {
                let Some(fields) = fields.as_object() else {
                    return Err(cx.err(STEP, "\"fields\" maps members of the line to values"));
                };
                for (k, value) in fields {
                    if !logged.members.contains(k) {
                        return Err(cx.err(
                            LOG_EVENT,
                            format!("the line of event \"{event}\" holds no member \"{k}\""),
                        ));
                    }
                    if value.is_array() || value.is_object() {
                        return Err(cx.err(
                            STEP,
                            format!("the expected value of \"{k}\" is a string, a number, a boolean or null"),
                        ));
                    }
                    expected.insert(k.clone(), value.clone());
                }
            }
            let members: Vec<Value> = logged.members.iter().cloned().map(Value::String).collect();
            Ok(vec![format!(
                "await j.expectLog({}, {}, {{ level: {}, fields: {}, members: {}, timeout: {} }});",
                js(service),
                js(&logged.name),
                js(&logged.level),
                Value::Object(expected),
                Value::Array(members),
                timeout.unwrap_or(5_000)
            )])
        }
        "waitFor" | "expectNoEvent" => {
            let Some(event) = string(o, "event") else {
                return Err(cx.err(STEP, "it needs an \"event\""));
            };
            if !plan.events.iter().any(|e| e == event) {
                return Err(cx.err(REFERENCE, format!("\"{event}\" is not an event")));
            }
            if kind == "expectNoEvent" {
                let Some(for_ms) = timeout_of(&cx, o, "forMs")? else {
                    return Err(cx.err(
                        STEP,
                        "it needs \"forMs\", how long no such event may arrive",
                    ));
                };
                return Ok(vec![format!("await j.expectNoEvent({for_ms});")]);
            }
            let timeout = timeout_of(&cx, o, "timeoutMs")?;
            let bound: Vec<&str> = plan
                .bindings
                .iter()
                .filter(|(_, e)| e == event)
                .map(|(s, _)| s.as_str())
                .collect();
            let observable = match &st.screen[cur] {
                Some(s) => bound.contains(&s.as_str()),
                None => !bound.is_empty(),
            };
            if !observable {
                return Err(cx.err(
                    WAIT_UNOBSERVABLE,
                    format!(
                        "no live binding refreshes {} on event \"{event}\", so the page shows nothing a test can wait for",
                        match &st.screen[cur] {
                            Some(s) => format!("screen \"{s}\""),
                            None => "any screen".to_string(),
                        }
                    ),
                ));
            }
            let list: Vec<Value> = bound
                .iter()
                .map(|s| Value::String((*s).to_string()))
                .collect();
            Ok(vec![format!(
                "await j.waitFor({}, {});",
                Value::Array(list),
                timeout_param(timeout)
            )])
        }
        other => Err(cx.err(STEP, format!("\"{other}\" is not a step"))),
    }
}

/// The parameters of a screen's address: exactly its route parameters, and any of its query
/// parameters, each a string or a number.
fn screen_params(
    cx: &Cx,
    screen: &Screen,
    params: Option<&Value>,
) -> Result<Map<String, Value>, Vec<Diagnostic>> {
    let empty = Map::new();
    let given = match params {
        None => &empty,
        Some(Value::Object(p)) => p,
        Some(_) => {
            return Err(cx.err(
                GOTO_PARAMS,
                format!("the params of screen \"{}\" are an object", screen.id),
            ))
        }
    };
    let missing: Vec<&str> = screen
        .params
        .iter()
        .filter(|p| given.get(*p).and_then(scalar).is_none())
        .map(String::as_str)
        .collect();
    let extra: Vec<&str> = given
        .iter()
        .filter(|(k, v)| {
            (!screen.params.contains(k) && !screen.query.contains(k)) || scalar(v).is_none()
        })
        .map(|(k, _)| k.as_str())
        .collect();
    if !missing.is_empty() || !extra.is_empty() {
        return Err(cx.err(
            GOTO_PARAMS,
            format!(
                "screen \"{}\" ({}) needs its route parameters as strings or numbers; missing [{}], not parameters or not scalars [{}]",
                screen.id,
                screen.route,
                missing.join(", "),
                extra.join(", ")
            ),
        ));
    }
    Ok(given.clone())
}

fn check_cookie_attributes(cx: &Cx, a: Option<&Value>) -> Result<(), Vec<Diagnostic>> {
    let Some(a) = a else { return Ok(()) };
    let keys = ["secure", "httpOnly", "sameSite", "path", "hostPrefix"];
    let Some(a) = a
        .as_object()
        .filter(|a| a.keys().all(|k| keys.contains(&k.as_str())))
    else {
        return Err(cx.err(
            STEP,
            "\"attributes\" is an object of secure, httpOnly, sameSite, path, hostPrefix",
        ));
    };
    for b in ["secure", "httpOnly", "hostPrefix"] {
        if a.get(b).is_some_and(|x| !x.is_boolean()) {
            return Err(cx.err(STEP, format!("\"{b}\" is a boolean")));
        }
    }
    if a.get("sameSite").is_some_and(|s| {
        !s.as_str()
            .is_some_and(|s| ["Strict", "Lax", "None"].contains(&s))
    }) {
        return Err(cx.err(STEP, "\"sameSite\" is Strict, Lax or None"));
    }
    if a.get("path")
        .is_some_and(|p| !p.as_str().is_some_and(valid_path))
    {
        return Err(cx.err(PATH, "the cookie path is an absolute path"));
    }
    Ok(())
}

fn lower_api_call(cx: &Cx, st: &mut State, o: &Map<String, Value>) -> Lines {
    if !string(o, "method").is_some_and(|m| ["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&m))
    {
        return Err(cx.err(STEP, "\"method\" is GET, POST, PUT, PATCH or DELETE"));
    }
    check_path(cx, o.get("path"))?;
    host_of(cx, o)?;
    if let Some(h) = o.get("headers") {
        if !h
            .as_object()
            .is_some_and(|h| h.values().all(Value::is_string))
        {
            return Err(cx.err(STEP, "\"headers\" is an object of strings"));
        }
    }
    match o.get("anonymous") {
        None | Some(Value::Bool(_)) => {}
        Some(_) => return Err(cx.err(STEP, "\"anonymous\" is a boolean")),
    }
    if let Some(as_) = o.get("as") {
        if o.get("anonymous") == Some(&Value::Bool(true)) {
            return Err(cx.err(
                STEP,
                "a call is made as a browser's session or anonymously, not both",
            ));
        }
        if !as_
            .as_str()
            .is_some_and(|b| !b.is_empty() && st.browsers.iter().any(|x| x == b))
        {
            return Err(cx.err(
                REFERENCE,
                format!("{as_} is not one of the journey's browsers"),
            ));
        }
    }
    if let Some(s) = o.get("expectStatus") {
        if !s.as_u64().is_some_and(|c| (100..=599).contains(&c)) {
            return Err(cx.err(STEP, format!("{s} is not an HTTP status from 100 to 599")));
        }
    }
    if let Some(ej) = o.get("expectJson") {
        let Some(ej) = ej.as_object() else {
            return Err(cx.err(
                STEP,
                "\"expectJson\" maps a path in the reply to its expected value",
            ));
        };
        if let Some(bad) = ej.keys().find(|p| !valid_json_path(p)) {
            return Err(cx.err(STEP, format!("\"{bad}\" is not a path into a JSON reply")));
        }
    }
    for b in ["expectBodyIncludes", "expectBodyExcludes"] {
        if let Some(list) = o.get(b) {
            if !list.as_array().is_some_and(|l| {
                l.iter()
                    .all(|s| s.as_str().is_some_and(|s| !s.trim().is_empty()))
            }) {
                return Err(cx.err(STEP, format!("\"{b}\" is a list of non-empty strings")));
            }
        }
    }
    if let Some(c) = o.get("capture") {
        let Some(c) = c.as_object() else {
            return Err(cx.err(STEP, "\"capture\" maps a value name to a path in the reply"));
        };
        for (name, p) in c {
            if !p.as_str().is_some_and(valid_json_path) {
                return Err(cx.err(STEP, format!("{p} is not a path into a JSON reply")));
            }
            define(cx, st, &Value::String(name.clone()))?;
        }
    }
    Ok(vec![format!(
        "await j.apiCall({});",
        Value::Object(o.clone())
    )])
}

fn lower_click_action(plan: &Plan, cx: &Cx, st: &mut State, o: &Map<String, Value>) -> Lines {
    let Some(aid) = string(o, "action") else {
        return Err(cx.err(STEP, "it needs an \"action\""));
    };
    let Some(action) = plan.actions.iter().find(|a| a.id == aid) else {
        return Err(cx.err(REFERENCE, format!("\"{aid}\" is not an action")));
    };
    let confirm = match o.get("confirm") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(cx.err(STEP, "\"confirm\" is a boolean")),
    };
    if confirm && action.confirm.is_none() {
        return Err(cx.err(
            ACTION_ITEM,
            format!("action \"{aid}\" asks for no confirmation, so \"confirm\" does not apply"),
        ));
    }
    let empty = Map::new();
    let item = match o.get("item") {
        None => &empty,
        Some(Value::Object(i)) => i,
        Some(_) => return Err(cx.err(ACTION_ITEM, "\"item\" maps the row's fields to values")),
    };
    if !action.per_row && o.contains_key("item") {
        return Err(cx.err(
            ACTION_ITEM,
            format!("action \"{aid}\" is not repeated per row (no \"forEach\"), so it takes no \"item\""),
        ));
    }
    let resolve = |text: &str| -> Result<String, Vec<Diagnostic>> {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("${item.") {
            let after = &rest[start + 7..];
            let Some(end) = after.find('}') else { break };
            let field = &after[..end];
            let Some(value) = item.get(field).and_then(scalar) else {
                return Err(cx.err(
                    ACTION_ITEM,
                    format!("action \"{aid}\" is repeated per row; \"item\" gives the row's {field}, which its label or confirmation reads"),
                ));
            };
            out.push_str(&rest[..start]);
            out.push_str(&value);
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    };
    let label = resolve(&action.label)?;
    let mut lines = vec![format!(
        "await j.page.getByRole(\"button\", {{ name: {}, exact: true }}).click();",
        jstext(&label)
    )];
    if confirm {
        let text = resolve(action.confirm.as_deref().unwrap_or_default())?;
        lines.push(format!(
            "await j.page.getByRole(\"button\", {{ name: {}, exact: true }}).click();",
            jstext(&text)
        ));
    }
    st.screen[st.current] = None;
    Ok(lines)
}

fn lower_fill(plan: &Plan, cx: &Cx, o: &Map<String, Value>) -> Lines {
    let Some(value) = o
        .get("value")
        .filter(|v| v.is_string() || v.is_number() || v.is_boolean())
    else {
        return Err(cx.err(
            STEP,
            "it needs a \"value\": a string, a number or a boolean",
        ));
    };
    if o.contains_key("form") || o.contains_key("field") {
        if TARGET.iter().any(|k| o.contains_key(*k)) {
            return Err(cx.err(
                STEP,
                "it names its control by form and field, or by role and name, not both",
            ));
        }
        let (Some(fid), Some(field)) = (string(o, "form"), string(o, "field")) else {
            return Err(cx.err(STEP, "it needs both \"form\" and \"field\""));
        };
        let Some(form) = plan.forms.iter().find(|f| f.id == fid) else {
            return Err(cx.err(REFERENCE, format!("\"{fid}\" is not a form")));
        };
        let Some(f) = form.fields.iter().find(|f| f.name == field) else {
            return Err(cx.err(
                REFERENCE,
                format!("form \"{fid}\" has no field \"{field}\""),
            ));
        };
        let Some(label) = &f.label else {
            return Err(cx.err(
                FIELD_LABEL,
                format!("field \"{field}\" of form \"{fid}\" has no label, so its control has no accessible name"),
            ));
        };
        let target = format!("j.page.getByLabel({}, {{ exact: true }})", js(label));
        let wrong = |what: &str| {
            Err(cx.err(
                FILL_VALUE,
                format!("a {} control of field \"{field}\" takes {what}", f.control),
            ))
        };
        return match f.control.as_str() {
            "checkbox" => match value.as_bool() {
                Some(b) => Ok(vec![format!("await {target}.setChecked({b});")]),
                None => wrong("a boolean"),
            },
            "number" => match value {
                Value::Number(num) => Ok(vec![format!(
                    "await {target}.fill({});",
                    js(&num.to_string())
                )]),
                _ => wrong("a number"),
            },
            "date" => match scalar(value) {
                Some(s) => Ok(vec![format!("await {target}.fill({});", jstext(&s))]),
                None => wrong("a string or a number"),
            },
            "select" | "radio" => {
                let Some(s) = scalar(value) else {
                    return wrong("an option value or label");
                };
                let radio = f.control == "radio";
                let group = format!(
                    "j.page.getByRole(\"radiogroup\", {{ name: {}, exact: true }})",
                    js(label)
                );
                if f.options_from {
                    // The options come from the view model: the page knows them, so the value
                    // is matched there, as a value or a label.
                    return Ok(vec![if radio {
                        format!("await j.chooseRadio({group}, {});", jstext(&s))
                    } else {
                        format!("await {target}.selectOption({});", jstext(&s))
                    }]);
                }
                let by = if f.options.iter().any(|(v, _)| *v == s) {
                    "value"
                } else if f.options.iter().any(|(_, l)| *l == s) {
                    "label"
                } else {
                    return Err(cx.err(
                        FILL_OPTION,
                        format!("\"{s}\" is neither the value nor the label of an option of field \"{field}\""),
                    ));
                };
                Ok(vec![if radio {
                    let name = if by == "value" {
                        f.options
                            .iter()
                            .find(|(v, _)| *v == s)
                            .map(|(_, l)| l.clone())
                            .unwrap_or_default()
                    } else {
                        s
                    };
                    format!(
                        "await {group}.getByRole(\"radio\", {{ name: {}, exact: true }}).check();",
                        js(&name)
                    )
                } else {
                    format!("await {target}.selectOption({{ {by}: {} }});", js(&s))
                }])
            }
            _ => match value.as_str() {
                Some(s) => Ok(vec![format!("await {target}.fill({});", jstext(s))]),
                None => wrong("a string"),
            },
        };
    }
    let loc = element(cx, &target_part(o, &["value"]), "j.page", ELEMENT)?;
    let role = string(o, "role").unwrap_or("");
    if !FILLABLE_ROLES.contains(&role) {
        return Err(cx.err(
            FILL_ROLE,
            format!(
                "a control filled by role is one of {}",
                FILLABLE_ROLES.join(", ")
            ),
        ));
    }
    match scalar(value) {
        Some(s) => Ok(vec![format!("await {loc}.fill({});", jstext(&s))]),
        None => Err(cx.err(FILL_VALUE, "a text control takes a string or a number")),
    }
}

// ---- files --------------------------------------------------------------------------------

fn reads_logs(steps: &[Value]) -> bool {
    steps.iter().any(|s| {
        s.as_object()
            .is_some_and(|o| o.contains_key("expectLog") || o.contains_key("logScan"))
    })
}

fn scenario_file(plan: &Plan, sc: &Scenario, expect_fail: bool) -> Result<String, Vec<Diagnostic>> {
    let mut st = State::new(&sc.browsers);
    let mut diagnostics = Vec::new();
    let mut body: Vec<String> = Vec::new();
    for (i, step) in sc.steps.iter().enumerate() {
        match lower_step(plan, &mut st, &sc.id, i, step) {
            Ok(lines) => body.extend(lines),
            Err(mut d) => diagnostics.append(&mut d),
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let mut out = String::new();
    out.push_str("import { expect, Journey, test } from './journey.js';\n\n");
    if let Some((w, h)) = sc.viewport {
        out.push_str(&format!(
            "test.use({{ viewport: {{ width: {w}, height: {h} }} }});\n\n"
        ));
    }
    for r in &sc.verifies {
        out.push_str(&crate::codetext::js::line_comments(
            "",
            &format!("Verifies: {r}"),
        ));
    }
    out.push_str(&format!(
        "test({}, async ({{ browser, baseURL, page, context }}) => {{\n",
        js(&sc.name)
    ));
    if expect_fail {
        out.push_str("  // A fixture: the journey violates its last step, so the test must fail.\n  test.fail();\n");
    }
    let browsers: Vec<Value> = sc.browsers.iter().cloned().map(Value::String).collect();
    out.push_str(&format!(
        "  const j = await Journey.start({{ browser, baseURL, page, context }}, {}, {{ logs: {} }});\n  try {{\n",
        Value::Array(browsers),
        reads_logs(&sc.steps)
    ));
    for line in &body {
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("  } finally {\n    await j.close();\n  }\n});\n");
    Ok(out)
}

/// The shared runtime: the spec's screens and log files, then [`RUNTIME`].
fn runtime(plan: &Plan, origin: Option<&str>) -> String {
    let mut out = String::from(RUNTIME_IMPORTS);
    out.push_str("\n// From the specification.\n");
    out.push_str(&format!(
        "const WRITTEN_ORIGIN = {};\n",
        origin.map(js).unwrap_or_else(|| "null".to_string())
    ));
    let files: Map<String, Value> = plan
        .logs
        .iter()
        .map(|(s, f)| (s.clone(), Value::String(f.clone())))
        .collect();
    out.push_str(&format!("const LOG_FILES = {};\n", Value::Object(files)));
    out.push_str("const SCREENS = [\n");
    for s in &plan.screens {
        let literal = s
            .route
            .split('/')
            .filter(|seg| !seg.is_empty() && !seg.starts_with(':'))
            .count();
        out.push_str(&format!(
            "  {{ id: {}, route: {}, literal: {literal}, catchAll: {} }},\n",
            js(&s.id),
            route_regex(&s.route),
            s.catch_all()
        ));
    }
    out.push_str("];\n");
    out.push_str(RUNTIME);
    out
}

const RUNTIME_IMPORTS: &str = "\
// The runtime of the generated journeys: one Journey per test holds its browsers, the values it
// captured, the requests and live notices each page saw, and the log lines written since it began.
import AxeBuilder from '@axe-core/playwright';
import { expect, request as apiRequest, test } from '@playwright/test';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

export { expect, test };
";

const RUNTIME: &str = r##"
// The stack under test. Without STRATA_E2E_BASE_URL it is the one the Playwright configuration
// starts. With it, the addresses the journeys name are moved from WRITTEN_ORIGIN to that stack,
// another port of the written host to STRATA_E2E_BACKEND_URL when that is set, and the service logs
// are read from the containers named in STRATA_E2E_LOG_CONTAINERS (podman logs).
const EXTERNAL = process.env.STRATA_E2E_BASE_URL ? new URL(process.env.STRATA_E2E_BASE_URL) : null;
const BACKEND = process.env.STRATA_E2E_BACKEND_URL ? new URL(process.env.STRATA_E2E_BACKEND_URL) : null;
const CONTAINERS = (process.env.STRATA_E2E_LOG_CONTAINERS ?? '').split(',').map((name) => name.trim()).filter(Boolean);
const WRITTEN = WRITTEN_ORIGIN ? new URL(WRITTEN_ORIGIN) : null;
const ROOT = new URL('../../../', import.meta.url);
const READY = 'main[data-page]:not([data-page="loading"])';
const READY_TIMEOUT = 15_000;
// The members every log line has; any other member must be one the event's allowlist names.
const RECORD_MEMBERS = ['ts', 'level', 'service', 'event', 'correlation', 'tenant'];
// The frames of the live protocol that carry a notice (docs/protocol.md): an event, a resync, the
// end of the session. Answers to the page's own requests and heartbeats are not notices.
const NOTICE_TYPES = ['event', 'resync', 'session'];
// Requests a script makes, as opposed to documents, styles, fonts and images.
const SCRIPT_KINDS = ['fetch', 'xhr', 'websocket', 'eventsource'];
const CAPTURE_REF = /\$\{([A-Za-z_][A-Za-z0-9_]*)\}/g;

const escapeRe = (text) => text.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&');
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function hostParts(host) {
  const m = /^(.*?)(?::(\d+))?$/.exec(host);
  return { name: m[1], port: m[2] ?? '' };
}

/** The host a journey names, on the stack under test. */
function hostOf(host) {
  if (!EXTERNAL || !WRITTEN) return host;
  const { name, port } = hostParts(host);
  if (port === WRITTEN.port) return `${name === WRITTEN.hostname ? EXTERNAL.hostname : name}${EXTERNAL.port ? `:${EXTERNAL.port}` : ''}`;
  if (BACKEND && name === WRITTEN.hostname) return BACKEND.host;
  return host;
}

/** A host the external stack does not publish: only the site is reachable there. */
const unpublished = (host) => Boolean(EXTERNAL && WRITTEN && !BACKEND && hostParts(host).port !== WRITTEN.port);

/** A text (a header value) with the written origin moved to the stack under test. */
const onSite = (text) => (EXTERNAL && WRITTEN ? text.split(WRITTEN.origin).join(EXTERNAL.origin) : text);

/** A pattern over addresses with the written host moved to the stack under test. */
const onSitePattern = (source) => (EXTERNAL && WRITTEN ? source.split(escapeRe(WRITTEN.host)).join(escapeRe(EXTERNAL.host)) : source);

/** The screen an address belongs to: the matching route with the most literal segments, else the catch-all. */
function screenOf(pathname) {
  const hits = SCREENS.filter((s) => !s.catchAll && s.route.test(pathname)).sort((a, b) => b.literal - a.literal);
  return hits[0]?.id ?? SCREENS.find((s) => s.catchAll)?.id ?? null;
}

/** A value inside a JSON reply by a path of names and indexes joined by '.'. */
function at(json, path) {
  let v = json;
  for (const key of path.split('.')) {
    if (v === null || typeof v !== 'object') return undefined;
    v = Array.isArray(v) ? v[Number(key)] : v[key];
  }
  return v;
}

// ---- the services' logs ------------------------------------------------------------------------

function readLogFile(file) {
  try {
    return readFileSync(fileURLToPath(new URL(file, ROOT)), 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

/** Every log source's whole text by key (null: a log file that does not exist yet). */
function readLogs() {
  const texts = new Map();
  if (CONTAINERS.length) {
    for (const name of CONTAINERS) {
      const done = spawnSync('podman', ['logs', name], { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 });
      if (done.error || done.status !== 0) throw new Error(`podman logs ${name} failed: ${done.error?.message ?? done.stderr}`);
      texts.set(`${name}:stdout`, done.stdout);
      texts.set(`${name}:stderr`, done.stderr);
    }
    return texts;
  }
  if (EXTERNAL) throw new Error('STRATA_E2E_BASE_URL is set but STRATA_E2E_LOG_CONTAINERS is not: name the containers whose output holds the service logs');
  for (const file of new Set(Object.values(LOG_FILES))) texts.set(file, readLogFile(file));
  return texts;
}

/** Waits until no log source grows for a moment, so lines still being written are there. */
async function settled() {
  let last = '';
  for (let i = 0; i < 20; i += 1) {
    const now = [...readLogs()].map(([key, text]) => `${key}:${text?.length ?? -1}`).join(',');
    if (now === last) return;
    last = now;
    await sleep(150);
  }
}

// Installed in every document: notes what the page's own thread fetches or connects to, so a
// request a worker made can be told from one the main thread made.
function hooks() {
  if (window.__journeyCalls) return;
  const calls = (window.__journeyCalls = []);
  const note = (target) => {
    const url = typeof target === 'string' || target instanceof URL ? String(target) : target?.url;
    try {
      calls.push(new URL(url, location.href).href);
    } catch {
      calls.push(String(url));
    }
  };
  const fetch0 = window.fetch;
  window.fetch = function (input, init) {
    note(input);
    return fetch0.call(this, input, init);
  };
  const open0 = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    note(url);
    return open0.call(this, method, url, ...rest);
  };
  const Socket = window.WebSocket;
  window.WebSocket = class extends Socket {
    constructor(url, protocols) {
      note(url);
      super(url, protocols);
    }
  };
  const Source = window.EventSource;
  if (Source) {
    window.EventSource = class extends Source {
      constructor(url, init) {
        note(url);
        super(url, init);
      }
    };
  }
  if (navigator.sendBeacon) {
    const beacon = navigator.sendBeacon.bind(navigator);
    navigator.sendBeacon = (url, data) => {
      note(url);
      return beacon(url, data);
    };
  }
}

export class Journey {
  constructor({ browser, baseURL }) {
    this.browser = browser;
    this.baseURL = baseURL;
    this.sessions = new Map();
    this.owned = [];
    this.disposables = [];
    this.captures = new Map();
    // Every x-correlation-id the journey's browsers and API calls sent.
    this.correlations = new Set();
    this.claimed = new Set();
    this.logFrom = new Map();
    this.lastResponse = null;
    this.seq = 0;
    this.calls = 0;
  }

  /** A journey over the named browsers (the first is the test's own page). */
  static async start(fixtures, names, { logs = false } = {}) {
    const j = new Journey(fixtures);
    if (logs) for (const [key, text] of readLogs()) j.logFrom.set(key, text?.length ?? 0);
    const list = names.length ? names : [''];
    for (const [i, name] of list.entries()) {
      let session = { context: fixtures.context, page: fixtures.page };
      if (i > 0) {
        const context = await fixtures.browser.newContext({ baseURL: fixtures.baseURL, viewport: fixtures.page.viewportSize() });
        j.owned.push(context);
        session = { context, page: await context.newPage() };
      }
      j.sessions.set(name, await j.instrument({ ...session, name, response: null, navigated: false, navs: 0, mark: 0, notices: [], views: [], logs: new Map() }));
    }
    j.current = j.sessions.get(list[0]);
    return j;
  }

  async instrument(t) {
    const page = t.page;
    await page.addInitScript(hooks);
    const views = new WeakMap();
    page.on('framenavigated', (frame) => {
      if (frame === page.mainFrame()) t.navs += 1;
    });
    page.on('request', (request) => {
      const correlation = request.headers()['x-correlation-id'];
      if (correlation) this.correlations.add(correlation);
      const entry = { url: request.url(), kind: request.resourceType(), seq: (this.seq += 1) };
      for (const log of t.logs.values()) log.push(entry);
      const url = new URL(request.url());
      if (url.pathname === '/api/view') {
        const view = { seq: entry.seq, nav: t.navs, path: url.searchParams.get('path'), done: false };
        t.views.push(view);
        views.set(request, view);
      }
    });
    page.on('requestfinished', async (request) => {
      const view = views.get(request);
      // A refetch that finds nothing (the record was removed) is a refresh all the same.
      if (view) view.done = (await request.response()) !== null;
    });
    page.on('websocket', (socket) => {
      const entry = { url: socket.url(), kind: 'websocket', seq: (this.seq += 1) };
      for (const log of t.logs.values()) log.push(entry);
      socket.on('framereceived', ({ payload }) => {
        let message;
        try {
          message = JSON.parse(String(payload));
        } catch {
          return;
        }
        // A session frame saying the session is still active changes nothing.
        if (NOTICE_TYPES.includes(message?.type) && !(message.type === 'session' && message.state === 'active')) t.notices.push({ seq: (this.seq += 1), nav: t.navs, path: new URL(page.url()).pathname });
      });
    });
    return t;
  }

  get page() {
    return this.current.page;
  }

  get context() {
    return this.current.context;
  }

  get response() {
    return this.current.response;
  }

  async close() {
    for (const d of this.disposables) await d.dispose().catch(() => {});
    for (const c of this.owned) await c.close().catch(() => {});
  }

  /** Switches browser; what the left one sees from now on counts for its next waitFor. */
  use(name) {
    this.current.mark = this.seq;
    this.current = this.sessions.get(name);
  }

  // ---- captured values ----

  sub(text) {
    return text.replace(CAPTURE_REF, (_, name) => {
      if (!this.captures.has(name)) throw new Error(`no value \${${name}} was captured`);
      return this.captures.get(name);
    });
  }

  subPath(text) {
    return text.replace(CAPTURE_REF, (whole) => encodeURIComponent(this.sub(whole)));
  }

  subDeep(value) {
    if (typeof value === 'string') return this.sub(value);
    if (Array.isArray(value)) return value.map((v) => this.subDeep(v));
    if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, this.subDeep(v)]));
    return value;
  }

  /** A text matched literally and case-sensitively. */
  text(text) {
    return new RegExp(escapeRe(this.sub(text)), 'u');
  }

  /** A pattern; a captured value in it stands for its literal text. */
  re(pattern) {
    return new RegExp(pattern.replace(CAPTURE_REF, (whole) => escapeRe(this.sub(whole))), 'u');
  }

  urlRe(pattern) {
    return new RegExp(onSitePattern(pattern.replace(CAPTURE_REF, (whole) => escapeRe(this.sub(whole)))), 'u');
  }

  scheme() {
    return new URL(this.baseURL).protocol;
  }

  // ---- navigation ----

  async ready() {
    await this.page.locator(READY).first().waitFor({ timeout: READY_TIMEOUT });
  }

  async goto(path, host) {
    const t = this.current;
    const url = host ? `${this.scheme()}//${hostOf(host)}${path}` : path;
    t.response = await t.page.goto(url);
    this.lastResponse = t.response;
    t.navigated = true;
    await this.ready();
    t.mark = this.seq;
  }

  async reload() {
    const t = this.current;
    t.response = await t.page.reload();
    this.lastResponse = t.response;
    await this.ready();
    t.mark = this.seq;
  }

  async back() {
    await this.page.goBack();
    await this.ready();
  }

  screen() {
    return screenOf(new URL(this.page.url()).pathname);
  }

  async expectScreen(id, timeout) {
    await expect.poll(() => this.screen(), { message: `the screen open at ${this.page.url()}`, timeout }).toBe(id);
  }

  /** The address: the path, and every declared query parameter given or absent. */
  async expectAddress(path, query, declared, timeout) {
    const address = () => {
      const url = new URL(this.page.url());
      return { path: decodeURIComponent(url.pathname), query: Object.fromEntries(declared.filter((k) => url.searchParams.has(k)).map((k) => [k, url.searchParams.get(k)])) };
    };
    await expect.poll(address, { message: 'the address', timeout }).toEqual({ path: decodeURIComponent(path), query });
  }

  // ---- what the page shows ----

  async expectText(scope, matcher, timeout) {
    const hits = scope ? scope.filter({ hasText: matcher, visible: true }) : this.page.getByText(matcher).filter({ visible: true });
    await expect(hits.first(), `a visible element with text ${matcher}`).toBeVisible({ timeout });
  }

  async expectNoText(scope, matcher, timeout) {
    const hits = scope ? scope.filter({ hasText: matcher, visible: true }) : this.page.getByText(matcher).filter({ visible: true });
    await expect(hits, `no visible element with text ${matcher}`).toHaveCount(0, { timeout });
  }

  /** The element exists (a hidden one included) and none of its matches is visible. */
  async expectHidden(locator, timeout) {
    await expect(locator, 'the element exists').not.toHaveCount(0, { timeout });
    await expect(locator.filter({ visible: true }), 'and is hidden').toHaveCount(0, { timeout });
  }

  async chooseRadio(group, value) {
    const radios = group.getByRole('radio');
    const count = await radios.count();
    for (let i = 0; i < count; i += 1) {
      const radio = radios.nth(i);
      if ((await radio.getAttribute('value')) === value || (await radio.getAttribute('aria-label')) === value || (await radio.evaluate((e) => e.labels?.[0]?.textContent?.trim() ?? '')) === value) {
        await radio.check();
        return;
      }
    }
    throw new Error(`no radio button of the group has the value or label ${value}`);
  }

  async mark(name) {
    await this.page.evaluate((n) => {
      window.__journeyMarks = { ...window.__journeyMarks, [n]: true };
    }, name);
  }

  async expectNoReload(name) {
    expect(await this.page.evaluate((n) => window.__journeyMarks?.[n] === true, name), `the document marked ${name} was not reloaded`).toBe(true);
  }

  async expectNoOverflow() {
    const wide = await this.page.evaluate(() => {
      const width = document.documentElement.clientWidth;
      if (document.documentElement.scrollWidth <= width) return [];
      const over = [...document.querySelectorAll('body *')].filter((e) => e.getBoundingClientRect().right > width + 0.5);
      return over.length ? over.slice(0, 5).map((e) => `${e.tagName.toLowerCase()}${e.id ? `#${e.id}` : ''}${[...e.classList].map((c) => `.${c}`).join('')}`) : ['the document'];
    });
    expect(wide, 'elements wider than the viewport').toEqual([]);
  }

  async expectBeside(a, b, side) {
    await expect(a).toBeVisible();
    await expect(b).toBeVisible();
    const [x, y] = [await a.boundingBox(), await b.boundingBox()];
    const holds = {
      left: x.x + x.width <= y.x + 1,
      right: x.x >= y.x + y.width - 1,
      above: x.y + x.height <= y.y + 1,
      below: x.y >= y.y + y.height - 1,
    }[side];
    expect(holds, `a is ${side} of b: ${JSON.stringify(x)} against ${JSON.stringify(y)}`).toBe(true);
  }

  async emulate({ colorScheme, reducedMotion, touch, mobile }) {
    const t = this.current;
    if (touch !== undefined || mobile !== undefined) {
      // A browser context fixes touch and mobile, so the browser gets a new one before it navigates.
      const context = await this.browser.newContext({ baseURL: this.baseURL, viewport: t.page.viewportSize(), hasTouch: touch ?? false, isMobile: mobile ?? false });
      this.owned.push(context);
      Object.assign(t, { context, page: await context.newPage() });
      await this.instrument(t);
    }
    const media = { ...(colorScheme ? { colorScheme } : {}), ...(reducedMotion ? { reducedMotion } : {}) };
    if (Object.keys(media).length) await t.page.emulateMedia(media);
  }

  async axe(name) {
    const results = await new AxeBuilder({ page: this.page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa']).analyze();
    const problems = results.violations.map((v) => `${v.id}: ${v.help} (${v.nodes.length}) ${v.nodes[0]?.target}`);
    expect(problems, `accessibility problems at ${name}`).toEqual([]);
  }

  // ---- requests ----

  recordRequests(name) {
    this.current.logs.set(name, []);
  }

  async expectRequests(name, to, { only = false, atLeast = 1, fromWorker = false } = {}) {
    const log = this.current.logs.get(name);
    const matching = () => log.filter((e) => to.test(e.url));
    await expect.poll(() => matching().length, { message: `requests in log ${name} to ${to}` }).toBeGreaterThanOrEqual(atLeast);
    if (only) expect(log.filter((e) => !to.test(e.url)).map((e) => e.url), `requests in log ${name} not to ${to}`).toEqual([]);
    if (fromWorker) {
      expect(this.page.workers().length, 'workers of the page').toBeGreaterThan(0);
      const main = new Set(await this.page.evaluate(() => window.__journeyCalls ?? []));
      expect(matching().filter((e) => !SCRIPT_KINDS.includes(e.kind) || main.has(e.url)).map((e) => `${e.kind} ${e.url}`), 'requests the page itself made rather than a worker').toEqual([]);
    }
  }

  expectNoRequest(name, path) {
    const log = this.current.logs.get(name);
    expect(log.filter((e) => path.test(new URL(e.url).pathname)).map((e) => e.url), `requests in log ${name} to ${path}`).toEqual([]);
  }

  async apiCall(call) {
    const { method, path, host, headers = {}, body, as, anonymous, expectStatus, expectJson = {}, expectBodyIncludes = [], expectBodyExcludes = [], capture = {} } = call;
    const target = host ? `${this.scheme()}//${hostOf(host)}${this.sub(path)}` : this.sub(path);
    const sent = Object.fromEntries(Object.entries(headers).map(([k, v]) => [k, onSite(this.sub(v))]));
    if (!Object.keys(sent).some((k) => k.toLowerCase() === 'x-correlation-id')) {
      this.calls += 1;
      sent['x-correlation-id'] = `t-${Date.now().toString(36)}-${this.calls}`;
    }
    for (const [k, v] of Object.entries(sent)) if (k.toLowerCase() === 'x-correlation-id') this.correlations.add(v);
    let api;
    if (anonymous) {
      api = await apiRequest.newContext({ baseURL: this.baseURL });
      this.disposables.push(api);
    } else {
      api = (as === undefined ? this.current : this.sessions.get(as)).context.request;
    }
    const options = { method, headers: sent, failOnStatusCode: false, maxRedirects: 0, ...(body === undefined ? {} : { data: this.subDeep(body) }) };
    const what = `${method} ${path}`;
    if (host && unpublished(host)) {
      // The external stack publishes only the site: nothing may answer at another port.
      expect(expectStatus === undefined || expectStatus >= 400, `${what} expects ${expectStatus} from a host the stack under test does not publish`).toBe(true);
      await expect(api.fetch(target, { ...options, timeout: 5_000 }), `${what} at ${target} is unreachable`).rejects.toThrow();
      return;
    }
    const response = await api.fetch(target, options);
    this.lastResponse = response;
    const text = await response.text();
    if (expectStatus !== undefined) expect(response.status(), `${what}: the status (reply ${text.slice(0, 300)})`).toBe(expectStatus);
    for (const s of expectBodyIncludes) expect(text.includes(this.sub(s)), `${what}: the reply holds ${s}`).toBe(true);
    for (const s of expectBodyExcludes) expect(text.includes(this.sub(s)), `${what}: the reply does not hold ${s}`).toBe(false);
    if (Object.keys(expectJson).length || Object.keys(capture).length) {
      let json;
      try {
        json = JSON.parse(text);
      } catch {
        throw new Error(`${what}: the reply is not JSON: ${text.slice(0, 300)}`);
      }
      for (const [p, v] of Object.entries(expectJson)) expect(at(json, p), `${what}: ${p}`).toEqual(this.subDeep(v));
      for (const [name, p] of Object.entries(capture)) {
        const v = at(json, p);
        expect(v !== undefined && v !== null && typeof v !== 'object', `${what}: ${p} is a value to capture`).toBe(true);
        this.captures.set(name, String(v));
      }
    }
  }

  // ---- cookies and captures ----

  async cookie(name) {
    const host = new URL(this.page.url()).hostname;
    return (await this.context.cookies()).find((c) => c.name === name && (c.domain === host || (c.domain.startsWith('.') && `.${host}`.endsWith(c.domain))));
  }

  async expectCookie({ name, absent = false, attributes = {} }) {
    if (absent) {
      await expect.poll(async () => (await this.cookie(name)) === undefined, { message: `no cookie ${name}` }).toBe(true);
      return;
    }
    await expect.poll(async () => (await this.cookie(name)) !== undefined, { message: `a cookie ${name}` }).toBe(true);
    const c = await this.cookie(name);
    const got = {};
    for (const k of ['secure', 'httpOnly', 'sameSite', 'path']) if (attributes[k] !== undefined) got[k] = c[k];
    if (attributes.hostPrefix !== undefined) got.hostPrefix = name.startsWith('__Host-') && c.secure && c.path === '/' && !c.domain.startsWith('.');
    expect(got, `the attributes of cookie ${name}`).toEqual(attributes);
  }

  async setCookie({ name, value, host, attributes = {} }) {
    await this.context.addCookies([{
      name,
      value: this.sub(value),
      url: `${this.scheme()}//${hostOf(host)}${attributes.path ?? '/'}`,
      secure: attributes.secure ?? false,
      httpOnly: attributes.httpOnly ?? false,
      ...(attributes.sameSite ? { sameSite: attributes.sameSite } : {}),
    }]);
  }

  async captureCookie(name, cookie) {
    const c = await this.cookie(cookie);
    expect(c?.value, `cookie ${cookie} to capture`).toBeTruthy();
    this.captures.set(name, c.value);
  }

  async captureHeader(name, header) {
    const value = this.lastResponse?.headers()[header.toLowerCase()];
    expect(value, `header ${header} of the last response to capture`).toBeTruthy();
    this.captures.set(name, value);
  }

  async captureText(name, locator) {
    await expect(locator).toBeVisible();
    const text = ((await locator.textContent()) ?? '').replace(/\s+/g, ' ').trim();
    expect(text, 'the text to capture').not.toBe('');
    this.captures.set(name, text);
  }

  async captureAttribute(name, locator, attribute) {
    await expect(locator).toBeVisible();
    const value = attribute === 'value' ? await locator.inputValue() : await locator.getAttribute(attribute);
    expect(value, `attribute ${attribute} to capture`).not.toBeNull();
    this.captures.set(name, value);
  }

  // ---- live notices ----

  /** A live notice arrived and then the open screen refetched its view, with no navigation between. */
  async waitFor(bound, timeout) {
    const t = this.current;
    const screen = this.screen();
    expect(bound, `the open screen ${screen} (at ${t.page.url()}) is refreshed by the event`).toContain(screen);
    const refreshed = () => {
      const path = new URL(t.page.url()).pathname;
      return t.notices.some((n) => n.seq > t.mark && n.nav === t.navs && n.path === path && t.views.some((v) => v.seq > n.seq && v.nav === n.nav && v.path === path && v.done));
    };
    await expect.poll(refreshed, { message: 'a live notice, then the open screen refetching its view', timeout }).toBe(true);
    t.mark = this.seq;
  }

  /** No live notice reaches this browser for a while (counted from when it was last left). */
  async expectNoEvent(forMs) {
    const t = this.current;
    await t.page.waitForTimeout(forMs);
    expect(t.notices.filter((n) => n.seq > t.mark).length, `live notices in the last ${forMs} ms and since this browser was left`).toBe(0);
  }

  // ---- logs ----

  /** The lines written since the journey began, with a stable key each. */
  logLines(texts = readLogs()) {
    const out = [];
    for (const [key, text] of texts) {
      if (text === null) continue;
      let from = this.logFrom.get(key) ?? 0;
      if (from > text.length) from = 0;
      let pos = from;
      for (const raw of text.slice(from).split('\n')) {
        const start = pos;
        pos += raw.length + 1;
        const line = raw.trim();
        if (!line) continue;
        let json = null;
        try {
          json = JSON.parse(line.slice(Math.max(0, line.indexOf('{'))));
        } catch {
          json = null;
        }
        out.push({ key: `${key}@${start}`, text: raw, json: json && typeof json === 'object' ? json : null });
      }
    }
    return out;
  }

  async logScan(patterns) {
    await settled();
    const texts = readLogs();
    for (const [key, text] of texts) expect(text, `the log ${key}`).not.toBeNull();
    const lines = this.logLines(texts);
    expect(lines.length, 'log lines written during the journey').toBeGreaterThan(0);
    for (const pattern of patterns) {
      // The positions only: a matching line holds what must not be printed.
      expect(lines.filter((l) => pattern.test(l.text)).map((l) => l.key), `log lines matching a forbidden pattern (${pattern.source.length} characters)`).toEqual([]);
    }
  }

  async expectLog(service, event, { level, fields, members, timeout }) {
    let found;
    const find = () => {
      found = this.logLines().find((l) => l.json && !this.claimed.has(l.key) && l.json.service === service && l.json.event === event && Object.entries(fields).every(([k, v]) => l.json[k] === v));
      return found !== undefined;
    };
    await expect.poll(find, { message: `a ${service} log line ${event} with ${JSON.stringify(fields)}`, timeout }).toBe(true);
    this.claimed.add(found.key);
    const line = found.json;
    expect(line.level, `the level of ${event}`).toBe(level);
    expect(typeof line.ts, `the time of ${event}`).toBe('number');
    expect(Object.keys(line).filter((k) => !RECORD_MEMBERS.includes(k) && !members.includes(k)), `members of ${event} its allowlist does not name`).toEqual([]);
    expect(typeof line.correlation === 'string' && this.correlations.has(line.correlation), `the correlation ${line.correlation} of ${event} is one this journey's browsers sent`).toBe(true);
    const services = new Set(Object.keys(LOG_FILES));
    if (services.size > 1) {
      await expect.poll(() => this.logLines().some((l) => l.json && l.json.service !== service && l.json.correlation === line.correlation), {
        message: `another service logged the request ${line.correlation} that caused ${event}`,
        timeout,
      }).toBe(true);
    }
  }
}
"##;

fn config() -> String {
    "\
// Runs the generated journeys against the stack of the hand-written suite and writes a JUnit report
// for Concordance. Expected at tests/e2e/generated/playwright.generated.config.js. With
// STRATA_E2E_BASE_URL it starts nothing and runs against that stack instead.
import { defineConfig } from '@playwright/test';
import base from '../playwright.config.js';

const report = new URL('../../../var/test-results/e2e-junit.xml', import.meta.url).pathname.replace(/^\\/([A-Za-z]:)/, '$1');
const external = process.env.STRATA_E2E_BASE_URL;

export default defineConfig({
  ...base,
  testDir: '.',
  testMatch: '*.spec.js',
  outputDir: '../test-results',
  reporter: [['list'], ['junit', { outputFile: report }]],
  use: { ...base.use, ...(external ? { baseURL: external } : {}) },
  projects: [{ name: 'generated' }],
  webServer: external ? [] : base.webServer.slice(0, 1),
});
"
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn screen() -> Screen {
        Screen {
            id: "post".into(),
            route: "/posts/:slug".into(),
            params: vec!["slug".into()],
            query: vec!["q".into(), "page".into()],
        }
    }

    fn plan() -> Plan {
        Plan {
            screens: vec![
                screen(),
                Screen {
                    id: "home".into(),
                    route: "/".into(),
                    params: vec![],
                    query: vec![],
                },
                Screen {
                    id: "lost".into(),
                    route: "*missing".into(),
                    params: vec![],
                    query: vec![],
                },
            ],
            forms: vec![Form {
                id: "login".into(),
                fields: vec![
                    Field {
                        name: "name".into(),
                        label: Some("Name".into()),
                        control: "text".into(),
                        options: vec![],
                        options_from: false,
                    },
                    Field {
                        name: "remember".into(),
                        label: Some("Remember me".into()),
                        control: "checkbox".into(),
                        options: vec![],
                        options_from: false,
                    },
                    Field {
                        name: "topic".into(),
                        label: Some("Topic".into()),
                        control: "select".into(),
                        options: vec![("d".into(), "Design".into())],
                        options_from: false,
                    },
                    Field {
                        name: "accent".into(),
                        label: Some("Accent".into()),
                        control: "radio".into(),
                        options: vec![],
                        options_from: true,
                    },
                    Field {
                        name: "shade".into(),
                        label: Some("Shade".into()),
                        control: "radio".into(),
                        options: vec![("dk".into(), "Dark".into())],
                        options_from: false,
                    },
                    Field {
                        name: "bare".into(),
                        label: None,
                        control: "text".into(),
                        options: vec![],
                        options_from: false,
                    },
                ],
                submit_label: Some("Sign in".into()),
            }],
            actions: vec![
                Action {
                    id: "del".into(),
                    label: "Delete ${item.title}".into(),
                    confirm: Some("Confirm deleting ${item.title}".into()),
                    per_row: true,
                },
                Action {
                    id: "out".into(),
                    label: "Sign out".into(),
                    confirm: None,
                    per_row: false,
                },
            ],
            bindings: vec![("home".into(), "ev".into())],
            events: vec!["ev".into(), "other".into(), "done".into()],
            logged: vec![LoggedEvent {
                id: "done".into(),
                service: "backend".into(),
                level: "info".into(),
                name: "thing.done".into(),
                members: vec!["tenant".into(), "object".into()],
            }],
            logs: vec![
                ("backend".into(), "var/logs/backend.jsonl".into()),
                ("bff".into(), "var/logs/bff.jsonl".into()),
            ],
            scenarios: vec![],
        }
    }

    fn state() -> State {
        State::new(&["a".to_string(), "b".to_string()])
    }

    fn lower(step: Value) -> Result<Vec<String>, Vec<Diagnostic>> {
        lower_step(&plan(), &mut state(), "j", 0, &step)
    }

    /// The lines of the last of the steps, lowered in order.
    fn lower_all(steps: &[Value]) -> Result<Vec<String>, Vec<Diagnostic>> {
        let mut st = state();
        let mut last = vec![];
        for (i, s) in steps.iter().enumerate() {
            last = lower_step(&plan(), &mut st, "j", i, s)?;
        }
        Ok(last)
    }

    fn one(step: Value) -> String {
        let lines = lower(step).unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        lines[0].clone()
    }

    fn code(step: Value) -> String {
        lower(step).unwrap_err()[0].code.clone()
    }

    fn codes(steps: &[Value]) -> String {
        lower_all(steps).unwrap_err()[0].code.clone()
    }

    #[test]
    fn every_step_kind_of_exegra_is_known() {
        let kinds: Vec<&str> = STEP_KINDS.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds.len(), 39);
        for k in ["goto", "expectLog", "expectNoEvent", "apiCall", "capture"] {
            assert!(kinds.contains(&k));
        }
    }

    #[test]
    fn goto_fills_the_route_orders_the_query_and_keeps_captures() {
        assert_eq!(
            one(
                json!({"goto": {"screen": "post", "params": {"page": 2, "slug": "a b", "q": "kiln"}}})
            ),
            r#"await j.goto("/posts/a%20b?q=kiln&page=2");"#
        );
        assert_eq!(
            one(json!({"goto": {"screen": "home", "host": "tide.localhost:4200"}})),
            r#"await j.goto("/", "tide.localhost:4200");"#
        );
        assert_eq!(code(json!({"goto": {"screen": "post"}})), GOTO_PARAMS);
        assert_eq!(
            code(json!({"goto": {"screen": "home", "params": {"x": 1}}})),
            GOTO_PARAMS
        );
        assert_eq!(
            code(json!({"goto": {"screen": "post", "params": {"slug": [1]}}})),
            GOTO_PARAMS
        );
        assert_eq!(code(json!({"goto": {"screen": "nope"}})), REFERENCE);
        assert_eq!(code(json!({"goto": {"screen": "lost"}})), GOTO_PARAMS);
        assert_eq!(
            code(json!({"goto": {"screen": "home", "host": "a b"}})),
            HOST
        );
        assert_eq!(
            lower_all(&[
                json!({"capture": {"name": "s", "from": "cookie", "cookie": "sid"}}),
                json!({"goto": {"screen": "post", "params": {"slug": "x ${s}"}}}),
            ])
            .unwrap(),
            [r#"await j.goto(j.subPath("/posts/x%20${s}"));"#]
        );
    }

    #[test]
    fn goto_path_reload_and_back() {
        assert_eq!(
            one(json!({"gotoPath": {"path": "/no/such"}})),
            r#"await j.goto("/no/such");"#
        );
        assert_eq!(code(json!({"gotoPath": {"path": "no"}})), PATH);
        assert_eq!(one(json!({"reload": {}})), "await j.reload();");
        assert_eq!(one(json!({"reload": true})), "await j.reload();");
        assert_eq!(one(json!({"back": {}})), "await j.back();");
        assert_eq!(code(json!({"back": {"x": 1}})), STEP);
    }

    #[test]
    fn elements_select_by_role_name_pattern_level_and_within() {
        assert_eq!(
            one(json!({"click": {"role": "button", "name": "Save"}})),
            r#"await j.page.getByRole("button", { name: "Save", exact: true }).click();"#
        );
        assert_eq!(
            one(json!({"focus": {"role": "textbox", "name": "Text"}})),
            r#"await j.page.getByRole("textbox", { name: "Text", exact: true }).focus();"#
        );
        assert_eq!(
            one(
                json!({"click": {"role": "link", "name": "Topics", "within": {"role": "navigation", "name": "Sections"}}})
            ),
            r#"await j.page.getByRole("navigation", { name: "Sections", exact: true }).getByRole("link", { name: "Topics", exact: true }).click();"#
        );
        assert_eq!(
            one(json!({"expectRole": {"role": "heading", "level": 1, "namePattern": "Post"}})),
            r#"await expect(j.page.getByRole("heading", { name: j.re("Post"), level: 1 })).toBeVisible();"#
        );
        assert_eq!(
            code(json!({"click": {"role": "buttn", "name": "Save"}})),
            ROLE
        );
        assert_eq!(code(json!({"click": {"role": "button"}})), STEP);
        assert_eq!(
            code(json!({"click": {"role": "button", "name": " "}})),
            STEP
        );
        assert_eq!(
            code(json!({"click": {"role": "button", "name": "a", "namePattern": "b"}})),
            STEP
        );
        assert_eq!(
            code(json!({"click": {"role": "button", "name": "a", "level": 2}})),
            LEVEL
        );
        assert_eq!(
            code(json!({"click": {"role": "heading", "name": "a", "level": 7}})),
            LEVEL
        );
        assert_eq!(
            code(json!({"click": {"role": "button", "name": "a", "css": "#x"}})),
            STEP
        );
        assert_eq!(
            code(
                json!({"click": {"role": "button", "name": "a", "within": {"role": "row", "css": "x"}}})
            ),
            STEP
        );
    }

    #[test]
    fn click_action_fills_the_label_and_confirms() {
        assert_eq!(
            lower(json!({"clickAction": {"action": "del", "item": {"title": "On Sharpening"}, "confirm": true}})).unwrap(),
            [
                r#"await j.page.getByRole("button", { name: "Delete On Sharpening", exact: true }).click();"#,
                r#"await j.page.getByRole("button", { name: "Confirm deleting On Sharpening", exact: true }).click();"#
            ]
        );
        assert_eq!(
            one(json!({"clickAction": {"action": "out"}})),
            r#"await j.page.getByRole("button", { name: "Sign out", exact: true }).click();"#
        );
        assert_eq!(code(json!({"clickAction": {"action": "del"}})), ACTION_ITEM);
        assert_eq!(
            code(json!({"clickAction": {"action": "out", "item": {"a": 1}}})),
            ACTION_ITEM
        );
        assert_eq!(
            code(json!({"clickAction": {"action": "out", "confirm": true}})),
            ACTION_ITEM
        );
        assert_eq!(code(json!({"clickAction": {"action": "nope"}})), REFERENCE);
    }

    #[test]
    fn fill_resolves_labels_controls_and_options() {
        assert_eq!(
            one(json!({"fill": {"form": "login", "field": "name", "value": "ines"}})),
            r#"await j.page.getByLabel("Name", { exact: true }).fill("ines");"#
        );
        assert_eq!(
            one(json!({"fill": {"form": "login", "field": "remember", "value": true}})),
            r#"await j.page.getByLabel("Remember me", { exact: true }).setChecked(true);"#
        );
        assert_eq!(
            one(json!({"fill": {"form": "login", "field": "topic", "value": "Design"}})),
            r#"await j.page.getByLabel("Topic", { exact: true }).selectOption({ label: "Design" });"#
        );
        assert_eq!(
            one(json!({"fill": {"form": "login", "field": "accent", "value": "plum"}})),
            r#"await j.chooseRadio(j.page.getByRole("radiogroup", { name: "Accent", exact: true }), "plum");"#
        );
        assert_eq!(
            one(json!({"fill": {"form": "login", "field": "shade", "value": "dk"}})),
            r#"await j.page.getByRole("radiogroup", { name: "Shade", exact: true }).getByRole("radio", { name: "Dark", exact: true }).check();"#
        );
        assert_eq!(
            one(json!({"fill": {"role": "textbox", "name": "Proposed change", "value": "x"}})),
            r#"await j.page.getByRole("textbox", { name: "Proposed change", exact: true }).fill("x");"#
        );
        assert_eq!(
            code(json!({"fill": {"form": "login", "field": "topic", "value": "x"}})),
            FILL_OPTION
        );
        assert_eq!(
            code(json!({"fill": {"form": "login", "field": "remember", "value": "yes"}})),
            FILL_VALUE
        );
        assert_eq!(
            code(json!({"fill": {"form": "login", "field": "bare", "value": "x"}})),
            FIELD_LABEL
        );
        assert_eq!(
            code(json!({"fill": {"role": "button", "name": "x", "value": "x"}})),
            FILL_ROLE
        );
        assert_eq!(
            code(json!({"fill": {"form": "login", "field": "name"}})),
            STEP
        );
        assert_eq!(
            code(
                json!({"fill": {"form": "login", "field": "name", "role": "textbox", "value": "x"}})
            ),
            STEP
        );
    }

    #[test]
    fn submit_and_press() {
        assert_eq!(
            one(json!({"submit": {"form": "login"}})),
            r#"await j.page.getByRole("button", { name: "Sign in", exact: true }).click();"#
        );
        assert_eq!(
            one(json!({"press": "Control+End"})),
            r#"await j.page.keyboard.press("Control+End");"#
        );
        assert_eq!(code(json!({"press": ""})), STEP);
        assert_eq!(code(json!({"submit": {"form": "nope"}})), REFERENCE);
    }

    #[test]
    fn text_steps_match_case_sensitively_within_a_scope() {
        assert_eq!(
            one(json!({"expectText": {"text": "Hi", "within": {"role": "region", "name": "Top"}}})),
            r#"await j.expectText(j.page.getByRole("region", { name: "Top", exact: true }), j.text("Hi"), undefined);"#
        );
        assert_eq!(
            one(json!({"expectText": {"pattern": "\\d+ of 60", "timeoutMs": 60000}})),
            r#"await j.expectText(null, j.re("\\d+ of 60"), 60000);"#
        );
        assert_eq!(
            one(json!({"expectText": {"text": "x", "within": {"role": "blockquote"}}})),
            r#"await j.expectText(j.page.getByRole("blockquote"), j.text("x"), undefined);"#
        );
        assert_eq!(
            one(json!({"expectNoText": {"text": "Forged"}})),
            r#"await j.expectNoText(null, j.text("Forged"), undefined);"#
        );
        assert_eq!(
            code(json!({"expectText": {"text": "a", "pattern": "b"}})),
            STEP
        );
        assert_eq!(
            code(json!({"expectText": {"text": "a", "timeoutMs": 0}})),
            TIMEOUT
        );
    }

    #[test]
    fn title_screen_and_address() {
        assert_eq!(
            one(json!({"expectTitle": {"text": "Fieldnotes"}})),
            r#"await expect(j.page).toHaveTitle("Fieldnotes");"#
        );
        assert_eq!(
            one(json!({"expectTitle": {"pattern": "^F", "timeoutMs": 9}})),
            r#"await expect(j.page).toHaveTitle(j.re("^F"), { timeout: 9 });"#
        );
        assert_eq!(
            one(json!({"expectScreen": "home"})),
            r#"await j.expectScreen("home", undefined);"#
        );
        assert_eq!(
            lower(
                json!({"expectScreen": {"screen": "post", "params": {"slug": "a b", "q": "kiln"}}})
            )
            .unwrap(),
            [
                r#"await j.expectScreen("post", undefined);"#,
                r#"await j.expectAddress("/posts/a%20b", {"q":"kiln"}, ["q","page"], undefined);"#
            ]
        );
        assert_eq!(
            one(json!({"expectScreen": "lost"})),
            r#"await j.expectScreen("lost", undefined);"#
        );
        assert_eq!(
            code(json!({"expectScreen": {"screen": "post", "params": {}}})),
            GOTO_PARAMS
        );
        assert_eq!(code(json!({"expectScreen": 3})), STEP);
    }

    #[test]
    fn role_states_and_hidden_needs_the_element() {
        let base =
            |state: &str| json!({"expectRole": {"role": "button", "name": "Save", "state": state}});
        assert_eq!(
            one(base("disabled")),
            r#"await expect(j.page.getByRole("button", { name: "Save", exact: true })).toBeDisabled();"#
        );
        assert_eq!(
            one(base("enabled")),
            r#"await expect(j.page.getByRole("button", { name: "Save", exact: true })).toBeEnabled();"#
        );
        assert_eq!(
            one(base("focused")),
            r#"await expect(j.page.getByRole("button", { name: "Save", exact: true })).toBeFocused();"#
        );
        assert_eq!(
            one(base("checked")),
            r#"await expect(j.page.getByRole("button", { name: "Save", exact: true })).toBeChecked();"#
        );
        assert_eq!(
            one(base("unchecked")),
            r#"await expect(j.page.getByRole("button", { name: "Save", exact: true })).toBeChecked({ checked: false });"#
        );
        assert_eq!(
            one(base("hidden")),
            r#"await j.expectHidden(j.page.getByRole("button", { name: "Save", exact: true, includeHidden: true }), undefined);"#
        );
        assert_eq!(code(base("gone")), STEP);
    }

    #[test]
    fn count_and_attribute() {
        assert_eq!(
            one(
                json!({"expectCount": {"role": "article", "within": {"role": "region", "name": "Earlier"}, "count": 5}})
            ),
            r#"await expect(j.page.getByRole("region", { name: "Earlier", exact: true }).getByRole("article")).toHaveCount(5);"#
        );
        assert_eq!(
            code(json!({"expectCount": {"role": "article", "count": -1}})),
            STEP
        );
        assert_eq!(
            one(json!({"expectAttribute": {"attribute": "data-accent", "value": "moss"}})),
            r#"await expect(j.page.locator("html")).toHaveAttribute("data-accent", "moss");"#
        );
        assert_eq!(
            one(
                json!({"expectAttribute": {"role": "textbox", "name": "Text", "attribute": "value", "pattern": "x$", "timeoutMs": 50}})
            ),
            r#"await expect(j.page.getByRole("textbox", { name: "Text", exact: true })).toHaveValue(j.re("x$"), { timeout: 50 });"#
        );
        assert_eq!(
            one(
                json!({"expectAttribute": {"role": "button", "name": "Menu", "attribute": "aria-expanded", "value": "false"}})
            ),
            r#"await expect(j.page.getByRole("button", { name: "Menu", exact: true })).toHaveAttribute("aria-expanded", "false");"#
        );
        assert_eq!(
            code(json!({"expectAttribute": {"attribute": "class", "value": "x"}})),
            ATTRIBUTE
        );
        assert_eq!(
            code(json!({"expectAttribute": {"attribute": "aria-label", "value": "x"}})),
            ATTRIBUTE
        );
        assert_eq!(
            code(json!({"expectAttribute": {"attribute": "lang", "value": "x", "pattern": "y"}})),
            STEP
        );
    }

    #[test]
    fn status_needs_a_navigation_in_the_same_browser() {
        assert_eq!(code(json!({"expectStatus": 200})), STATUS_WITHOUT_GOTO);
        assert_eq!(
            lower_all(&[
                json!({"gotoPath": {"path": "/x"}}),
                json!({"expectStatus": 404})
            ])
            .unwrap(),
            [r#"expect(j.response?.status(), "the status of the last navigation").toBe(404);"#]
        );
        assert_eq!(
            codes(&[
                json!({"gotoPath": {"path": "/x"}}),
                json!({"switchBrowser": "b"}),
                json!({"expectStatus": 404})
            ]),
            STATUS_WITHOUT_GOTO
        );
        assert_eq!(code(json!({"expectStatus": 99})), STEP);
    }

    #[test]
    fn marks_and_request_logs_belong_to_their_browser() {
        assert_eq!(one(json!({"mark": "loaded"})), r#"await j.mark("loaded");"#);
        assert_eq!(
            lower_all(&[
                json!({"mark": "loaded"}),
                json!({"expectNoReload": "loaded"})
            ])
            .unwrap(),
            [r#"await j.expectNoReload("loaded");"#]
        );
        assert_eq!(
            codes(&[
                json!({"mark": "loaded"}),
                json!({"switchBrowser": "b"}),
                json!({"expectNoReload": "loaded"})
            ]),
            MARK
        );
        assert_eq!(
            one(json!({"recordRequests": "app"})),
            r#"j.recordRequests("app");"#
        );
        assert_eq!(
            lower_all(&[json!({"recordRequests": "app"}), json!({"expectRequests": {"log": "app", "to": "^ws://", "atLeast": 1, "fromWorker": true}})]).unwrap(),
            [r#"await j.expectRequests("app", j.urlRe("^ws://"), { fromWorker: true, atLeast: 1 });"#]
        );
        assert_eq!(
            lower_all(&[
                json!({"recordRequests": "v"}),
                json!({"expectNoRequest": {"log": "v", "path": "/api/view"}})
            ])
            .unwrap(),
            [r#"j.expectNoRequest("v", j.urlRe("/api/view"));"#]
        );
        assert_eq!(
            code(json!({"expectNoRequest": {"log": "v", "path": "/api/view"}})),
            REQUEST_LOG
        );
        assert_eq!(code(json!({"mark": "a b"})), STEP);
    }

    #[test]
    fn api_calls_capture_and_later_steps_use_the_captures() {
        let call = json!({"apiCall": {"method": "GET", "path": "/api/session", "expectStatus": 200, "capture": {"csrf": "session.csrf_token"}}});
        assert_eq!(
            lower(call.clone()).unwrap(),
            [
                r#"await j.apiCall({"capture":{"csrf":"session.csrf_token"},"expectStatus":200,"method":"GET","path":"/api/session"});"#
            ]
        );
        let use_it = json!({"apiCall": {"method": "POST", "path": "/api/records", "headers": {"x-csrf-token": "${csrf}"}, "expectJson": {"failed.0.rule": "p"}}});
        assert!(lower_all(&[call.clone(), use_it.clone()]).is_ok());
        assert_eq!(code(use_it), CAPTURE);
        assert_eq!(codes(&[call.clone(), call]), CAPTURE);
        assert_eq!(
            code(json!({"apiCall": {"method": "GET", "path": "/x", "as": "c"}})),
            REFERENCE
        );
        assert_eq!(
            code(json!({"apiCall": {"method": "GET", "path": "/x", "as": "a", "anonymous": true}})),
            STEP
        );
        assert_eq!(
            code(json!({"apiCall": {"method": "TRACE", "path": "/x"}})),
            STEP
        );
        assert_eq!(
            code(json!({"apiCall": {"method": "GET", "path": "/x", "expectJson": {"a..b": 1}}})),
            STEP
        );
        assert_eq!(
            code(json!({"apiCall": {"method": "GET", "path": "/x", "headers": {"a": 1}}})),
            STEP
        );
    }

    #[test]
    fn cookies_captures_and_emulation() {
        assert_eq!(
            one(json!({"expectCookie": {"name": "sid", "attributes": {"httpOnly": true}}})),
            r#"await j.expectCookie({"attributes":{"httpOnly":true},"name":"sid"});"#
        );
        assert_eq!(
            code(json!({"expectCookie": {"name": "sid", "absent": true, "attributes": {}}})),
            STEP
        );
        assert_eq!(
            code(json!({"expectCookie": {"name": "sid", "attributes": {"sameSite": "Loose"}}})),
            STEP
        );
        assert_eq!(
            one(json!({"setCookie": {"name": "sid", "value": "v", "host": "t.localhost:4200"}})),
            r#"await j.setCookie({"host":"t.localhost:4200","name":"sid","value":"v"});"#
        );
        assert_eq!(
            code(json!({"setCookie": {"name": "sid", "value": "v"}})),
            STEP
        );
        assert_eq!(
            one(
                json!({"capture": {"name": "snap", "from": "text", "element": {"role": "region", "name": "Snapshot"}}})
            ),
            r#"await j.captureText("snap", j.page.getByRole("region", { name: "Snapshot", exact: true }));"#
        );
        assert_eq!(
            one(
                json!({"capture": {"name": "v", "from": "attribute", "element": {"role": "textbox", "name": "A"}, "attribute": "value"}})
            ),
            r#"await j.captureAttribute("v", j.page.getByRole("textbox", { name: "A", exact: true }), "value");"#
        );
        assert_eq!(
            one(json!({"capture": {"name": "h", "from": "header", "header": "etag"}})),
            r#"await j.captureHeader("h", "etag");"#
        );
        assert_eq!(
            code(json!({"capture": {"name": "h", "from": "header", "cookie": "x"}})),
            STEP
        );
        assert_eq!(
            one(json!({"emulate": {"colorScheme": "dark"}})),
            r#"await j.emulate({"colorScheme":"dark"});"#
        );
        assert_eq!(code(json!({"emulate": {}})), STEP);
        assert_eq!(
            codes(&[
                json!({"gotoPath": {"path": "/"}}),
                json!({"emulate": {"touch": true}})
            ]),
            EMULATE_LATE
        );
    }

    #[test]
    fn layout_offline_viewport_and_axe() {
        assert_eq!(
            one(json!({"expectNoOverflow": {}})),
            "await j.expectNoOverflow();"
        );
        assert_eq!(
            one(
                json!({"expectBeside": {"a": {"role": "complementary", "name": "A"}, "b": {"role": "region", "name": "B"}, "side": "left"}})
            ),
            r#"await j.expectBeside(j.page.getByRole("complementary", { name: "A", exact: true }), j.page.getByRole("region", { name: "B", exact: true }), "left");"#
        );
        assert_eq!(
            code(
                json!({"expectBeside": {"a": {"role": "complementary", "name": "A"}, "b": {"role": "region", "name": "B"}, "side": "under"}})
            ),
            STEP
        );
        assert_eq!(
            one(json!({"goOffline": {}})),
            "await j.context.setOffline(true);"
        );
        assert_eq!(
            one(json!({"goOnline": true})),
            "await j.context.setOffline(false);"
        );
        assert_eq!(
            one(json!({"setViewport": {"width": 390, "height": 844}})),
            "await j.page.setViewportSize({ width: 390, height: 844 });"
        );
        assert_eq!(
            code(json!({"setViewport": {"width": 0, "height": 844}})),
            STEP
        );
        assert_eq!(one(json!({"axeScan": {}})), r#"await j.axe("step 1");"#);
        assert_eq!(one(json!({"switchBrowser": "b"})), r#"j.use("b");"#);
        assert_eq!(code(json!({"switchBrowser": "c"})), REFERENCE);
    }

    #[test]
    fn logs_are_read_with_level_allowlist_and_correlation() {
        assert_eq!(
            one(
                json!({"expectLog": {"service": "backend", "event": "done", "fields": {"tenant": "t", "object": "post"}}})
            ),
            r#"await j.expectLog("backend", "thing.done", { level: "info", fields: {"object":"post","tenant":"t"}, members: ["tenant","object"], timeout: 5000 });"#
        );
        assert_eq!(
            code(json!({"expectLog": {"service": "bff", "event": "done"}})),
            LOG_EVENT
        );
        assert_eq!(
            code(json!({"expectLog": {"service": "backend", "event": "ev"}})),
            LOG_EVENT
        );
        assert_eq!(
            code(
                json!({"expectLog": {"service": "backend", "event": "done", "fields": {"body": "x"}}})
            ),
            LOG_EVENT
        );
        assert_eq!(
            code(json!({"expectLog": {"service": "backend", "event": "nope"}})),
            REFERENCE
        );
        assert_eq!(
            lower_all(&[
                json!({"capture": {"name": "sid", "from": "cookie", "cookie": "s"}}),
                json!({"logScan": {"forbid": ["${sid}", "strata-dev-"]}}),
            ])
            .unwrap(),
            [r#"await j.logScan([j.re("${sid}"), j.re("strata-dev-")]);"#]
        );
        assert_eq!(code(json!({"logScan": {"forbid": []}})), STEP);
        assert_eq!(code(json!({"logScan": {"forbid": ["${sid}"]}})), CAPTURE);
    }

    #[test]
    fn wait_for_needs_a_live_binding_and_follows_navigation() {
        assert_eq!(
            one(json!({"waitFor": {"event": "ev"}})),
            r#"await j.waitFor(["home"], undefined);"#
        );
        assert_eq!(
            code(json!({"waitFor": {"event": "other"}})),
            WAIT_UNOBSERVABLE
        );
        assert_eq!(code(json!({"waitFor": {"event": "none"}})), REFERENCE);
        // A screen is open that the event does not refresh.
        assert_eq!(
            codes(&[
                json!({"goto": {"screen": "post", "params": {"slug": "a"}}}),
                json!({"waitFor": {"event": "ev"}})
            ]),
            WAIT_UNOBSERVABLE
        );
        // After a click the open screen is decided at run time.
        assert!(lower_all(&[
            json!({"goto": {"screen": "post", "params": {"slug": "a"}}}),
            json!({"click": {"role": "link", "name": "Home"}}),
            json!({"waitFor": {"event": "ev"}}),
        ])
        .is_ok());
        assert_eq!(
            one(json!({"expectNoEvent": {"event": "ev", "forMs": 1200}})),
            "await j.expectNoEvent(1200);"
        );
        assert_eq!(code(json!({"expectNoEvent": {"event": "ev"}})), STEP);
    }

    #[test]
    fn steps_have_exactly_one_known_key() {
        assert_eq!(
            code(json!({"click": {"role": "button", "name": "x"}, "press": "A"})),
            STEP
        );
        assert_eq!(code(json!({"css": "#x"})), STEP);
        assert_eq!(code(json!("goto")), STEP);
    }

    #[test]
    fn routes_become_anchored_regular_expressions() {
        assert_eq!(route_regex("/"), r"/^\/$/");
        assert_eq!(route_regex("/posts/:slug"), r"/^\/posts\/[^\/]+$/");
        assert_eq!(route_regex("/a.b/c~d"), r"/^\/a\.b\/c~d$/");
        assert_eq!(route_regex("*missing"), r"/^\/.*$/");
        assert_eq!(route_regex("*"), r"/^\/.*$/");
    }

    #[test]
    fn invalid_journey_input_is_a_diagnostic() {
        let refused = |text: &str| {
            load(text)
                .unwrap_err()
                .into_iter()
                .map(|d| d.code)
                .collect::<Vec<_>>()
        };
        let journey =
            |extra: &str| format!(r#"{{"uiScenarios": [{{"id": "a", "steps": [] {extra}}}]}}"#);
        assert_eq!(
            refused(&journey(r#", "browsers": ["x", "x"]"#)),
            [DUPLICATE]
        );
        assert_eq!(refused(&journey(r#", "browsers": ["x", 3]"#)), [SHAPE]);
        assert_eq!(
            refused(&journey(r#", "viewport": {"width": 0, "height": 3}"#)),
            [SHAPE]
        );
        assert_eq!(refused(&journey(r#", "verifies": [1]"#)), [SHAPE]);
        assert_eq!(refused(r#"{"screens": [1], "uiScenarios": []}"#), [SHAPE]);
        assert_eq!(refused(r#"{"forms": ["f"], "uiScenarios": []}"#), [SHAPE]);
        assert_eq!(
            refused(r#"{"liveBindings": [{"screen": 1}], "uiScenarios": []}"#),
            [SHAPE]
        );
        assert!(generate_with(
            "s",
            &journey(""),
            &Options {
                origin: Some("127.0.0.1:4200".into()),
                expect_fail_prefix: None
            }
        )
        .is_err());
    }
}
