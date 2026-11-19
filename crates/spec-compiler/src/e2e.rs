//! The end-to-end output: `specc e2e` turns the user journeys (`uiScenarios`) of a specification
//! into Playwright tests, one `<scenario-id>.spec.js` per journey, plus a Playwright
//! configuration that runs them against the stack the hand-written suite uses and writes a
//! JUnit report Concordance can ingest.
//!
//! # Layout
//!
//! The files are meant to be generated into `tests/e2e/generated`. Two paths inside them are
//! written relative to that place: the logs the `logScan` step reads (`var/logs`, three
//! directories up) and the JUnit report (`var/test-results/e2e-junit.xml`).
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
//! See [`lower_step`]. Elements are selected by role and accessible name only
//! (`getByRole(role, { name, exact: true })`); form fields by their label. The one step with no
//! direct browser equivalent is `waitFor`: an event is not visible to the page, but a live
//! binding makes the open screen refetch its view, so the step waits for a view request
//! (`/api/view?path=<the open screen's path>`) that was not seen before the session's last
//! navigation, the last switch away from it, or the last `waitFor`. It is refused when no live
//! binding refreshes the screen on that event, since then the page does nothing observable.

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
/// Code of a step that names a screen, form, field, event, browser or claim that does not exist.
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
/// Code of a `fill` of a select with a value that is no option.
pub const FILL_OPTION: &str = "e2e.fill-option-unknown";
/// Code of an `expectStatus` with no navigation before it in its browser.
pub const STATUS_WITHOUT_GOTO: &str = "e2e.status-without-navigation";
/// Code of a `waitFor` whose event no live binding turns into something the page shows.
pub const WAIT_UNOBSERVABLE: &str = "e2e.wait-unobservable";
/// Code of a verified claim that is missing, not a requirement or has no `ref`.
pub const VERIFIES: &str = "e2e.verifies-invalid";

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

/// A form field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The field name.
    pub name: String,
    /// The accessible label.
    pub label: Option<String>,
    /// The control kind.
    pub control: String,
    /// `(value, label)` of a select's options.
    pub options: Vec<(String, String)>,
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
    /// `(screen, event)` of every live binding.
    pub bindings: Vec<(String, String)>,
    /// Event ids.
    pub events: Vec<String>,
    /// The journeys in declaration order.
    pub scenarios: Vec<Scenario>,
}

/// The generated files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eOutput {
    /// The files, sorted by path.
    pub files: Vec<GeneratedFile>,
}

/// Generates the Playwright files from a specification's text, or refuses with every
/// diagnostic. `spec_path` is the path written into the headers.
pub fn generate(spec_path: &str, text: &str) -> Result<E2eOutput, Vec<Diagnostic>> {
    let spec_path = spec_path.replace('\\', "/");
    let plan = load(text)?;
    let mut files = Vec::new();
    let mut diagnostics = Vec::new();
    for scenario in &plan.scenarios {
        match scenario_file(&plan, scenario) {
            Ok(body) => files.push(file(&format!("{}.spec.js", scenario.id), &spec_path, body)),
            Err(mut d) => diagnostics.append(&mut d),
        }
    }
    if !diagnostics.is_empty() {
        return Err(dedup(diagnostics));
    }
    files.push(file("playwright.generated.config.js", &spec_path, config()));
    files.sort();
    Ok(E2eOutput { files })
}

fn file(path: &str, spec_path: &str, body: String) -> GeneratedFile {
    GeneratedFile {
        path: path.to_string(),
        contents: format!(
            "{} from {spec_path}. Do not edit; change the specification and regenerate.\n{body}",
            HEADER_MARKERS[0]
        ),
    }
}

// ---- loading ------------------------------------------------------------------------------

fn string<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

fn strings(obj: &Map<String, Value>, key: &str) -> Vec<String> {
    obj.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
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

/// The parameter names of a route pattern.
fn route_params(route: &str) -> Vec<String> {
    route
        .split('/')
        .filter_map(|s| s.strip_prefix(':'))
        .map(str::to_string)
        .collect()
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
    for item in array(top, "screens", &mut d) {
        let Some(o) = item.as_object() else { continue };
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
            query: strings(o, "query"),
        });
    }

    let mut forms = Vec::new();
    for item in array(top, "forms", &mut d) {
        let Some(o) = item.as_object() else { continue };
        let Some(id) = string(o, "id") else {
            d.push(Diagnostic::new(SHAPE, "forms", "a form needs a string id"));
            continue;
        };
        let mut fields = Vec::new();
        for f in o
            .get("fields")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let Some(fo) = f.as_object() else { continue };
            let Some(name) = string(fo, "name") else {
                d.push(Diagnostic::new(SHAPE, id, "a field needs a string name"));
                continue;
            };
            let options = fo
                .get("options")
                .and_then(Value::as_array)
                .map(|opts| {
                    opts.iter()
                        .filter_map(Value::as_object)
                        .map(|oo| {
                            (
                                string(oo, "value").unwrap_or("").to_string(),
                                string(oo, "label").unwrap_or("").to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            fields.push(Field {
                name: name.to_string(),
                label: string(fo, "label").map(str::to_string),
                control: string(fo, "control").unwrap_or("text").to_string(),
                options,
            });
        }
        forms.push(Form {
            id: id.to_string(),
            fields,
            submit_label: string(o, "submitLabel").map(str::to_string),
        });
    }

    let mut events = Vec::new();
    for item in array(top, "events", &mut d) {
        if let Some(id) = item.as_object().and_then(|o| string(o, "id")) {
            events.push(id.to_string());
        }
    }
    let mut bindings = Vec::new();
    for item in array(top, "liveBindings", &mut d) {
        if let Some(o) = item.as_object() {
            if let (Some(s), Some(e)) = (string(o, "screen"), string(o, "event")) {
                bindings.push((s.to_string(), e.to_string()));
            }
        }
    }

    // Requirement id -> ref.
    let mut requirements: Vec<(String, Option<String>, bool)> = Vec::new();
    for item in array(top, "claims", &mut d) {
        if let Some(o) = item.as_object() {
            if let Some(id) = string(o, "id") {
                requirements.push((
                    id.to_string(),
                    string(o, "ref").map(str::to_string),
                    string(o, "kind") == Some("requirement"),
                ));
            }
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
        let mut verifies = Vec::new();
        for v in strings(o, "verifies") {
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
        let viewport = o
            .get("viewport")
            .and_then(Value::as_object)
            .and_then(|v| Some((v.get("width")?.as_u64()?, v.get("height")?.as_u64()?)));
        scenarios.push(Scenario {
            id: id.to_string(),
            name: string(o, "name").unwrap_or(id).to_string(),
            verifies,
            browsers: strings(o, "browsers"),
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
        bindings,
        events,
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

// ---- lowering -----------------------------------------------------------------------------

/// A JavaScript string literal.
fn js(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
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

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
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
                Some(name) => path.push_str(&percent_encode(
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
                .map(|v| format!("{q}={}", percent_encode(&v)))
        })
        .collect();
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }
    path
}

/// A JavaScript regular expression literal matching the path of a route.
pub fn route_regex(route: &str) -> String {
    if route == "/" {
        return r"/^\/$/".to_string();
    }
    let mut out = String::from("/^");
    for seg in route.split('/').skip(1) {
        out.push_str(r"\/");
        if seg.starts_with(':') {
            out.push_str(r"[^\/]+");
        } else {
            out.push_str(&seg.replace('.', r"\."));
        }
    }
    out.push_str("$/");
    out
}

fn locator(role: &str, name: &str) -> String {
    format!(
        "page.getByRole({}, {{ name: {}, exact: true }})",
        js(role),
        js(name)
    )
}

/// What the lowering of one journey tracks.
struct State {
    browsers: Vec<String>,
    multi: bool,
    current: usize,
    /// Per browser: has it navigated, and which screen is open in it?
    navigated: Vec<bool>,
    screen: Vec<Option<String>>,
    uses: Uses,
}

#[derive(Default, Clone, Copy)]
struct Uses {
    axe: bool,
    logs: bool,
    views: bool,
}

/// The JavaScript lines of one step. The mapping, step by step:
///
/// | step | Playwright |
/// |---|---|
/// | `goto` | `response = await page.goto(path)`, then wait for the application's page marker |
/// | `click` | `getByRole(role, { name, exact: true }).click()` |
/// | `fill` by form field | `getByLabel(label, { exact: true })` and `fill`, `setChecked` or `selectOption` by control |
/// | `fill` by role | `getByRole(role, { name, exact: true }).fill(value)` |
/// | `submit` | click the button named by the form's `submitLabel` |
/// | `press` | `page.keyboard.press(key)` |
/// | `expectText` | `expect(getByText(text) [within getByRole]).first()).toBeVisible()` |
/// | `expectScreen` | `expect.poll(pathname).toMatch(route regex)` |
/// | `expectRole` | `toBeVisible`, `toBeHidden`, `toBeDisabled` or `toBeFocused` |
/// | `expectStatus` | `expect(response.status()).toBe(n)` on the last navigation |
/// | `switchBrowser` | another context and page, kept by name |
/// | `goOffline`, `goOnline` | `context.setOffline(true / false)` |
/// | `setViewport` | `page.setViewportSize` |
/// | `axeScan` | `AxeBuilder` with the suite's tags, no violations |
/// | `logScan` | no line of `var/logs/*.jsonl` matches a pattern |
/// | `waitFor` | a refetch of the open screen's view, see the module documentation |
fn lower_step(
    plan: &Plan,
    st: &mut State,
    scenario: &str,
    index: usize,
    step: &Value,
) -> Result<Vec<String>, Vec<Diagnostic>> {
    let n = index + 1;
    let fail = |code: &str, msg: String| {
        Err(vec![Diagnostic::new(
            code,
            scenario,
            format!("step {n}: {msg}"),
        )])
    };
    let Some(obj) = step.as_object().filter(|o| o.len() == 1) else {
        return fail(STEP, "a step is an object with exactly one key.".into());
    };
    let (kind, v) = obj.iter().next().expect("one key");
    let o = v.as_object();
    let text = |key: &str| o.and_then(|o| string(o, key));
    let role_name = |o: &Map<String, Value>| -> Result<(String, String), Vec<Diagnostic>> {
        let (Some(role), Some(name)) = (string(o, "role"), string(o, "name")) else {
            return Err(vec![Diagnostic::new(
                STEP,
                scenario,
                format!("step {n} ({kind}): it needs a string role and a string name."),
            )]);
        };
        if !ARIA_ROLES.contains(&role) {
            return Err(vec![Diagnostic::new(
                ROLE,
                scenario,
                format!("step {n} ({kind}): \"{role}\" is not a WAI-ARIA role."),
            )]);
        }
        if name.trim().is_empty() {
            return Err(vec![Diagnostic::new(
                STEP,
                scenario,
                format!("step {n} ({kind}): the accessible name is empty."),
            )]);
        }
        Ok((role.to_string(), name.to_string()))
    };
    let cur = st.current;
    match kind.as_str() {
        "goto" => {
            let Some(o) = o else {
                return fail(STEP, "goto is {screen, params?}.".into());
            };
            let Some(sid) = string(o, "screen") else {
                return fail(STEP, "goto needs a screen.".into());
            };
            let Some(screen) = plan.screens.iter().find(|s| s.id == sid) else {
                return fail(REFERENCE, format!("\"{sid}\" is not a screen."));
            };
            let empty = Map::new();
            let params = o.get("params").and_then(Value::as_object).unwrap_or(&empty);
            let missing: Vec<&str> = screen
                .params
                .iter()
                .filter(|p| params.get(*p).and_then(scalar).is_none())
                .map(String::as_str)
                .collect();
            let extra: Vec<&str> = params
                .keys()
                .filter(|k| !screen.params.contains(k) && !screen.query.contains(k))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() || !extra.is_empty() {
                return fail(
                    GOTO_PARAMS,
                    format!(
                        "screen \"{sid}\" ({}) needs its route parameters as strings or numbers; missing [{}], not parameters [{}].",
                        screen.route,
                        missing.join(", "),
                        extra.join(", ")
                    ),
                );
            }
            st.navigated[cur] = true;
            st.screen[cur] = Some(sid.to_string());
            let mut lines = vec![
                format!("response = await page.goto({});", js(&goto_path(screen, params))),
                "await ready(page);".to_string(),
            ];
            if st.uses.views {
                lines.push("marks.set(page, refreshes(page));".to_string());
            }
            Ok(lines)
        }
        "click" => {
            let Some(o) = o else {
                return fail(STEP, "click is {role, name}.".into());
            };
            let (r, nm) = role_name(o)?;
            Ok(vec![format!("await {}.click();", locator(&r, &nm))])
        }
        "fill" => lower_fill(plan, scenario, n, o),
        "submit" => {
            let Some(fid) = text("form") else {
                return fail(STEP, "submit is {form}.".into());
            };
            let Some(form) = plan.forms.iter().find(|f| f.id == fid) else {
                return fail(REFERENCE, format!("\"{fid}\" is not a form."));
            };
            let Some(label) = &form.submit_label else {
                return fail(
                    SUBMIT_LABEL,
                    format!("form \"{fid}\" has no submitLabel, so its button has no accessible name."),
                );
            };
            Ok(vec![format!("await {}.click();", locator("button", label))])
        }
        "press" => match v.as_str().filter(|s| !s.trim().is_empty()) {
            Some(key) => Ok(vec![format!("await page.keyboard.press({});", js(key))]),
            None => fail(STEP, "press is a key such as Enter.".into()),
        },
        "expectText" => {
            let Some(t) = text("text").filter(|t| !t.trim().is_empty()) else {
                return fail(STEP, "expectText needs a non-empty text.".into());
            };
            let scope = match o.and_then(|o| o.get("within")) {
                None => "page".to_string(),
                Some(w) => match w.as_object() {
                    Some(w) => {
                        let (r, nm) = role_name(w)?;
                        locator(&r, &nm)
                    }
                    None => return fail(STEP, "within is {role, name}.".into()),
                },
            };
            Ok(vec![format!(
                "await expect({scope}.getByText({}).first()).toBeVisible();",
                js(t)
            )])
        }
        "expectScreen" => {
            let Some(sid) = v.as_str() else {
                return fail(STEP, "expectScreen is a screen id.".into());
            };
            let Some(screen) = plan.screens.iter().find(|s| s.id == sid) else {
                return fail(REFERENCE, format!("\"{sid}\" is not a screen."));
            };
            st.screen[cur] = Some(sid.to_string());
            Ok(vec![format!(
                "await expect.poll(() => new URL(page.url()).pathname).toMatch({});",
                route_regex(&screen.route)
            )])
        }
        "expectRole" => {
            let Some(o) = o else {
                return fail(STEP, "expectRole is {role, name, state?}.".into());
            };
            let (r, nm) = role_name(o)?;
            let matcher = match string(o, "state").unwrap_or("visible") {
                "visible" => "toBeVisible",
                "hidden" => "toBeHidden",
                "disabled" => "toBeDisabled",
                "focused" => "toBeFocused",
                other => {
                    return fail(
                        STEP,
                        format!("state \"{other}\" is not visible, hidden, disabled or focused."),
                    )
                }
            };
            Ok(vec![format!("await expect({}).{matcher}();", locator(&r, &nm))])
        }
        "expectStatus" => {
            let Some(code) = v.as_u64().filter(|c| (100..=599).contains(c)) else {
                return fail(STEP, "expectStatus is an HTTP status from 100 to 599.".into());
            };
            if !st.navigated[cur] {
                return fail(
                    STATUS_WITHOUT_GOTO,
                    "the status is that of a navigation, and this browser has not navigated yet.".into(),
                );
            }
            Ok(vec![format!("expect(response.status()).toBe({code});")])
        }
        "switchBrowser" => {
            let Some(name) = v.as_str() else {
                return fail(STEP, "switchBrowser is a browser name.".into());
            };
            let Some(i) = st.browsers.iter().position(|b| b == name) else {
                return fail(
                    REFERENCE,
                    format!("\"{name}\" is not one of the journey's browsers."),
                );
            };
            if !st.multi {
                st.current = i;
                return Ok(vec![]);
            }
            let mut lines = Vec::new();
            if st.uses.views {
                lines.push("marks.set(page, refreshes(page));".to_string());
            }
            lines.push("current.response = response;".to_string());
            lines.push(format!("current = sessions[{}];", js(name)));
            lines.push("({ context, page, response } = current);".to_string());
            st.current = i;
            Ok(lines)
        }
        "goOffline" | "goOnline" => Ok(vec![format!(
            "await context.setOffline({});",
            kind == "goOffline"
        )]),
        "setViewport" => {
            let dims = o.and_then(|o| Some((o.get("width")?.as_u64()?, o.get("height")?.as_u64()?)));
            match dims.filter(|(w, h)| *w >= 1 && *h >= 1) {
                Some((w, h)) => Ok(vec![format!(
                    "await page.setViewportSize({{ width: {w}, height: {h} }});"
                )]),
                None => fail(STEP, "setViewport is {width, height}, positive integers.".into()),
            }
        }
        "axeScan" => Ok(vec![format!("await axe(page, {});", js(&format!("step {n}")))]),
        "logScan" => {
            let patterns: Vec<&str> = o
                .and_then(|o| o.get("forbid"))
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if patterns.is_empty() {
                return fail(STEP, "logScan needs a non-empty forbid list.".into());
            }
            let list: Vec<String> = patterns
                .iter()
                .map(|p| format!("new RegExp({}, 'u')", js(p)))
                .collect();
            Ok(vec![format!("noForbiddenLogLines([{}]);", list.join(", "))])
        }
        "waitFor" => {
            let Some(event) = text("event") else {
                return fail(STEP, "waitFor is {event}.".into());
            };
            if !plan.events.iter().any(|e| e == event) {
                return fail(REFERENCE, format!("\"{event}\" is not an event."));
            }
            let observable = match &st.screen[cur] {
                Some(s) => plan.bindings.iter().any(|(bs, be)| bs == s && be == event),
                None => plan.bindings.iter().any(|(_, be)| be == event),
            };
            if !observable {
                return fail(
                    WAIT_UNOBSERVABLE,
                    format!(
                        "no live binding refreshes {} on event \"{event}\", so the page shows nothing a test can wait for.",
                        match &st.screen[cur] {
                            Some(s) => format!("screen \"{s}\""),
                            None => "any screen".to_string(),
                        }
                    ),
                );
            }
            Ok(vec![
                "await expect.poll(() => refreshes(page), { message: 'the open screen refetches its view' }).toBeGreaterThan(marks.get(page) ?? 0);".to_string(),
                "marks.set(page, refreshes(page));".to_string(),
            ])
        }
        other => fail(
            STEP,
            format!("\"{other}\" is not a step (elements are selected by role and accessible name only)."),
        ),
    }
}

fn lower_fill(
    plan: &Plan,
    scenario: &str,
    n: usize,
    o: Option<&Map<String, Value>>,
) -> Result<Vec<String>, Vec<Diagnostic>> {
    let fail = |code: &str, msg: String| {
        Err(vec![Diagnostic::new(
            code,
            scenario,
            format!("step {n} (fill): {msg}"),
        )])
    };
    let Some(o) = o else {
        return fail(STEP, "fill is an object with a value.".into());
    };
    let Some(value) = o.get("value") else {
        return fail(STEP, "fill needs a value.".into());
    };
    if let (Some(fid), Some(field)) = (string(o, "form"), string(o, "field")) {
        let Some(form) = plan.forms.iter().find(|f| f.id == fid) else {
            return fail(REFERENCE, format!("\"{fid}\" is not a form."));
        };
        let Some(f) = form.fields.iter().find(|f| f.name == field) else {
            return fail(
                REFERENCE,
                format!("form \"{fid}\" has no field \"{field}\"."),
            );
        };
        let Some(label) = &f.label else {
            return fail(
                FIELD_LABEL,
                format!("field \"{field}\" of form \"{fid}\" has no label, so its control has no accessible name."),
            );
        };
        let target = format!("page.getByLabel({}, {{ exact: true }})", js(label));
        let wrong = |what: &str| {
            fail(
                FILL_VALUE,
                format!("a {} control of field \"{field}\" takes {what}.", f.control),
            )
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
                Some(s) => Ok(vec![format!("await {target}.fill({});", js(&s))]),
                None => wrong("a string or a number"),
            },
            "select" => {
                let Some(s) = scalar(value) else {
                    return wrong("an option value or label");
                };
                if f.options.iter().any(|(v, _)| *v == s) {
                    Ok(vec![format!(
                        "await {target}.selectOption({{ value: {} }});",
                        js(&s)
                    )])
                } else if f.options.iter().any(|(_, l)| *l == s) {
                    Ok(vec![format!(
                        "await {target}.selectOption({{ label: {} }});",
                        js(&s)
                    )])
                } else {
                    fail(
                        FILL_OPTION,
                        format!("\"{s}\" is neither the value nor the label of an option of field \"{field}\"."),
                    )
                }
            }
            _ => match value.as_str() {
                Some(s) => Ok(vec![format!("await {target}.fill({});", js(s))]),
                None => wrong("a string"),
            },
        };
    }
    let (Some(role), Some(name)) = (string(o, "role"), string(o, "name")) else {
        return fail(
            STEP,
            "fill names its control by form and field, or by role and name.".into(),
        );
    };
    if !ARIA_ROLES.contains(&role) {
        return fail(ROLE, format!("\"{role}\" is not a WAI-ARIA role."));
    }
    if !FILLABLE_ROLES.contains(&role) {
        return fail(
            FILL_ROLE,
            format!(
                "a control filled by role is one of {}.",
                FILLABLE_ROLES.join(", ")
            ),
        );
    }
    match scalar(value) {
        Some(s) => Ok(vec![format!(
            "await {}.fill({});",
            locator(role, name),
            js(&s)
        )]),
        None => fail(
            FILL_VALUE,
            "a text control takes a string or a number.".into(),
        ),
    }
}

// ---- files --------------------------------------------------------------------------------

fn uses_of(steps: &[Value]) -> Uses {
    let has = |k: &str| {
        steps
            .iter()
            .any(|s| s.as_object().is_some_and(|o| o.contains_key(k)))
    };
    Uses {
        axe: has("axeScan"),
        logs: has("logScan"),
        views: has("waitFor"),
    }
}

fn scenario_file(plan: &Plan, sc: &Scenario) -> Result<String, Vec<Diagnostic>> {
    let uses = uses_of(&sc.steps);
    let multi = sc.browsers.len() >= 2;
    let browsers = if sc.browsers.is_empty() {
        vec![String::new()]
    } else {
        sc.browsers.clone()
    };
    let mut st = State {
        navigated: vec![false; browsers.len()],
        screen: vec![None; browsers.len()],
        browsers,
        multi,
        current: 0,
        uses,
    };
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
    out.push_str("import { expect, test } from '@playwright/test';\n");
    if uses.axe {
        out.push_str("import AxeBuilder from '@axe-core/playwright';\n");
    }
    if uses.logs {
        out.push_str("import { readFileSync } from 'node:fs';\n");
    }
    out.push('\n');
    out.push_str(READY);
    if uses.axe {
        out.push_str(AXE);
    }
    if uses.logs {
        out.push_str(LOGS);
    }
    if uses.views {
        out.push_str(VIEWS);
    }
    if multi {
        out.push_str(SESSION);
    }
    if let Some((w, h)) = sc.viewport {
        out.push_str(&format!(
            "test.use({{ viewport: {{ width: {w}, height: {h} }} }});\n\n"
        ));
    }
    for r in &sc.verifies {
        out.push_str(&format!("// Verifies: {r}\n"));
    }
    let indent = if multi { "    " } else { "  " };
    if multi {
        out.push_str(&format!(
            "test({}, async ({{ browser, baseURL, page: firstPage, context: firstContext }}) => {{\n",
            js(&sc.name)
        ));
        out.push_str("  const sessions = {};\n");
        for (i, b) in sc.browsers.iter().enumerate() {
            if i == 0 {
                out.push_str(&format!(
                    "  sessions[{}] = {{ context: firstContext, page: firstPage, response: null }};\n",
                    js(b)
                ));
            } else {
                out.push_str(&format!(
                    "  sessions[{}] = await newSession(browser, baseURL, firstPage.viewportSize());\n",
                    js(b)
                ));
            }
        }
        if uses.views {
            out.push_str("  for (const s of Object.values(sessions)) watch(s.page);\n");
        }
        out.push_str(&format!(
            "  let current = sessions[{}];\n  let {{ context, page, response }} = current;\n  try {{\n",
            js(&sc.browsers[0])
        ));
    } else {
        out.push_str(&format!(
            "test({}, async ({{ page, context }}) => {{\n  let response = null;\n",
            js(&sc.name)
        ));
        if uses.views {
            out.push_str("  watch(page);\n");
        }
    }
    for line in &body {
        out.push_str(indent);
        out.push_str(line);
        out.push('\n');
    }
    if multi {
        out.push_str("  } finally {\n    for (const s of Object.values(sessions)) {\n      if (s.context !== firstContext) await s.context.close();\n    }\n  }\n");
    }
    out.push_str("});\n");
    Ok(out)
}

const READY: &str = "\
/** Wait until the application has drawn a real page; the assertions after it wait on their own, so a page without the marker (an error page) is not an error here. */
async function ready(page) {
  await page.locator('main[data-page]:not([data-page=\"loading\"])').waitFor({ timeout: 5_000 }).catch(() => {});
}

";

const AXE: &str = "\
async function axe(page, name) {
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa']).analyze();
  const problems = results.violations.map((v) => `${v.id}: ${v.help} (${v.nodes.length}) ${v.nodes[0]?.target}`);
  expect(problems, `accessibility problems at ${name}`).toEqual([]);
}

";

const LOGS: &str = "\
const LOGS = new URL('../../../var/logs/', import.meta.url).pathname.replace(/^\\/([A-Za-z]:)/, '$1');

function noForbiddenLogLines(patterns) {
  const lines = ['backend.jsonl', 'bff.jsonl'].map((name) => readFileSync(`${LOGS}${name}`, 'utf8')).join('\\n').split('\\n');
  for (const pattern of patterns) {
    expect(lines.filter((line) => pattern.test(line)), `log lines matching ${pattern}`).toEqual([]);
  }
}

";

const VIEWS: &str = "\
// A live binding makes the open screen refetch its view (GET /api/view?path=<screen path>).
const views = [];
const marks = new Map();
const watch = (page) => page.on('response', (r) => {
  if (r.url().includes('/api/view')) views.push({ page, path: new URL(r.url()).searchParams.get('path') });
});
const refreshes = (page) => views.filter((v) => v.page === page && v.path === new URL(page.url()).pathname).length;

";

const SESSION: &str = "\
async function newSession(browser, baseURL, viewport) {
  const context = await browser.newContext({ baseURL, viewport });
  return { context, page: await context.newPage(), response: null };
}

";

fn config() -> String {
    "\
// Runs the generated journeys against the stack of the hand-written suite and writes a JUnit report
// for Concordance. Expected at tests/e2e/generated/playwright.generated.config.js.
import { defineConfig } from '@playwright/test';
import base from '../playwright.config.js';

const report = new URL('../../../var/test-results/e2e-junit.xml', import.meta.url).pathname.replace(/^\\/([A-Za-z]:)/, '$1');

export default defineConfig({
  ...base,
  testDir: '.',
  testMatch: '*.spec.js',
  outputDir: '../test-results',
  reporter: [['list'], ['junit', { outputFile: report }]],
  projects: [{ name: 'generated' }],
  webServer: base.webServer.slice(0, 1),
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
            ],
            forms: vec![Form {
                id: "login".into(),
                fields: vec![
                    Field {
                        name: "name".into(),
                        label: Some("Name".into()),
                        control: "text".into(),
                        options: vec![],
                    },
                    Field {
                        name: "remember".into(),
                        label: Some("Remember me".into()),
                        control: "checkbox".into(),
                        options: vec![],
                    },
                    Field {
                        name: "topic".into(),
                        label: Some("Topic".into()),
                        control: "select".into(),
                        options: vec![("d".into(), "Design".into())],
                    },
                    Field {
                        name: "bare".into(),
                        label: None,
                        control: "text".into(),
                        options: vec![],
                    },
                ],
                submit_label: Some("Sign in".into()),
            }],
            bindings: vec![("home".into(), "ev".into())],
            events: vec!["ev".into(), "other".into()],
            scenarios: vec![],
        }
    }

    fn state() -> State {
        State {
            browsers: vec![String::new()],
            multi: false,
            current: 0,
            navigated: vec![false],
            screen: vec![None],
            uses: Uses::default(),
        }
    }

    fn lower(step: Value) -> Result<Vec<String>, Vec<Diagnostic>> {
        lower_step(&plan(), &mut state(), "j", 0, &step)
    }

    fn code(step: Value) -> String {
        lower(step).unwrap_err()[0].code.clone()
    }

    #[test]
    fn goto_fills_the_route_and_orders_the_query() {
        let lines = lower(
            json!({"goto": {"screen": "post", "params": {"page": 2, "slug": "a b", "q": "kiln"}}}),
        )
        .unwrap();
        assert_eq!(
            lines[0],
            r#"response = await page.goto("/posts/a%20b?q=kiln&page=2");"#
        );
        assert_eq!(code(json!({"goto": {"screen": "post"}})), GOTO_PARAMS);
        assert_eq!(
            code(json!({"goto": {"screen": "home", "params": {"x": 1}}})),
            GOTO_PARAMS
        );
        assert_eq!(code(json!({"goto": {"screen": "nope"}})), REFERENCE);
    }

    #[test]
    fn role_steps_select_by_exact_accessible_name() {
        assert_eq!(
            lower(json!({"click": {"role": "button", "name": "Save"}})).unwrap(),
            [r#"await page.getByRole("button", { name: "Save", exact: true }).click();"#]
        );
        assert_eq!(
            code(json!({"click": {"role": "buttn", "name": "Save"}})),
            ROLE
        );
        assert_eq!(
            lower(json!({"expectRole": {"role": "button", "name": "Save", "state": "disabled"}}))
                .unwrap(),
            [
                r#"await expect(page.getByRole("button", { name: "Save", exact: true })).toBeDisabled();"#
            ]
        );
        assert_eq!(
            lower(
                json!({"expectText": {"text": "Hi", "within": {"role": "region", "name": "Top"}}})
            )
            .unwrap(),
            [
                r#"await expect(page.getByRole("region", { name: "Top", exact: true }).getByText("Hi").first()).toBeVisible();"#
            ]
        );
    }

    #[test]
    fn fill_resolves_labels_and_controls() {
        assert_eq!(
            lower(json!({"fill": {"form": "login", "field": "name", "value": "ines"}})).unwrap(),
            [r#"await page.getByLabel("Name", { exact: true }).fill("ines");"#]
        );
        assert_eq!(
            lower(json!({"fill": {"form": "login", "field": "remember", "value": true}})).unwrap(),
            [r#"await page.getByLabel("Remember me", { exact: true }).setChecked(true);"#]
        );
        assert_eq!(
            lower(json!({"fill": {"form": "login", "field": "topic", "value": "Design"}})).unwrap(),
            [
                r#"await page.getByLabel("Topic", { exact: true }).selectOption({ label: "Design" });"#
            ]
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
    }

    #[test]
    fn submit_clicks_the_submit_label() {
        assert_eq!(
            lower(json!({"submit": {"form": "login"}})).unwrap(),
            [r#"await page.getByRole("button", { name: "Sign in", exact: true }).click();"#]
        );
    }

    #[test]
    fn expect_screen_matches_the_route_shape() {
        assert_eq!(route_regex("/"), r"/^\/$/");
        assert_eq!(route_regex("/posts/:slug"), r"/^\/posts\/[^\/]+$/");
        assert_eq!(route_regex("/a.b/c"), r"/^\/a\.b\/c$/");
    }

    #[test]
    fn status_needs_a_navigation_in_the_same_browser() {
        let mut st = state();
        assert_eq!(
            lower_step(&plan(), &mut st, "j", 0, &json!({"expectStatus": 200})).unwrap_err()[0]
                .code,
            STATUS_WITHOUT_GOTO
        );
        lower_step(
            &plan(),
            &mut st,
            "j",
            0,
            &json!({"goto": {"screen": "home"}}),
        )
        .unwrap();
        assert_eq!(
            lower_step(&plan(), &mut st, "j", 1, &json!({"expectStatus": 404})).unwrap(),
            ["expect(response.status()).toBe(404);"]
        );
    }

    #[test]
    fn wait_for_needs_a_live_binding() {
        let mut st = state();
        st.uses.views = true;
        let lines = lower_step(
            &plan(),
            &mut st,
            "j",
            0,
            &json!({"waitFor": {"event": "ev"}}),
        )
        .unwrap();
        assert!(lines[0].contains("refreshes(page)"));
        assert_eq!(
            code(json!({"waitFor": {"event": "other"}})),
            WAIT_UNOBSERVABLE
        );
        assert_eq!(code(json!({"waitFor": {"event": "none"}})), REFERENCE);
        // A screen is open that the event does not refresh.
        let mut st = state();
        lower_step(
            &plan(),
            &mut st,
            "j",
            0,
            &json!({"goto": {"screen": "post", "params": {"slug": "a"}}}),
        )
        .unwrap();
        assert_eq!(
            lower_step(
                &plan(),
                &mut st,
                "j",
                1,
                &json!({"waitFor": {"event": "ev"}})
            )
            .unwrap_err()[0]
                .code,
            WAIT_UNOBSERVABLE
        );
    }

    #[test]
    fn steps_have_exactly_one_known_key() {
        assert_eq!(
            code(json!({"click": {"role": "button", "name": "x"}, "press": "A"})),
            STEP
        );
        assert_eq!(code(json!({"css": "#x"})), STEP);
        assert_eq!(code(json!({"expectStatus": 99})), STEP);
    }

    #[test]
    fn offline_and_viewport() {
        assert_eq!(
            lower(json!({"goOffline": {}})).unwrap(),
            ["await context.setOffline(true);"]
        );
        assert_eq!(
            lower(json!({"goOnline": true})).unwrap(),
            ["await context.setOffline(false);"]
        );
        assert_eq!(
            lower(json!({"setViewport": {"width": 390, "height": 844}})).unwrap(),
            ["await page.setViewportSize({ width: 390, height: 844 });"]
        );
    }
}
