//! `screens.js` and `screens.rs`: what the page frame and the backend-for-frontend derive from the
//! screens and the per-row actions, so that a screen added to the specification needs no
//! hand-written change in the client or the backend-for-frontend (WG-5039).
//!
//! A screen states, besides its route: `viewName` (the view the tenant schema gives it, default the
//! screen id), `title` (the page title: text with `${path}` parts read from the view model),
//! `navLabel`/`navOrder`/`navArea`/`navVisibleTo`/`navCurrent` (its navigation entry), `onEnter`
//! (the sections the client loads on entry), `signIn` (the sign-in screen) and `viewOutputs` (the
//! outputs of the view model the backend-for-frontend derives). A per-row action states
//! `uiName`, `uiAsk`, `uiMarker`, `uiRowKey` and `uiLabelKey`. These are vocabulary properties of
//! the specification.
//!
//! The modules:
//!
//! - `screens.js`: `SCREENS`, `NAVIGATION`, `ROW_ACTIONS`, `SIGN_IN_SCREEN`, `SIGN_IN_PATH`, and
//!   the functions `screenOfView`, `guardedView`, `navigation(path, signedIn)`,
//!   `pageTitle(view, model)`, `enterSections(view)`, `rowActionsOn(view)`, `rowAction(name)` and
//!   `rowActionAsked(name)`.
//! - `screens.rs`: `VIEWS`, one `ViewInfo` per view with its title parts and view outputs, which
//!   the backend-for-frontend includes (it names no view itself).
//!
//! A screen guarded by a session must state its view name and its title; a specification with such
//! a screen must have a sign-in screen. Everything else that is stated is checked: unique view
//! names, navigation at an address, entries of a guarded screen visible to a session only, the
//! syntax of titles and view outputs, and complete, unique row-button names.

use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Value;

use crate::codetext;
use crate::diagnostic::{Diagnostic, SPEC_ENTITY};
use crate::web::sq;

/// Code of a screen or action whose stated page data cannot be generated from.
pub const CODE: &str = "specc.web-screens";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Doc {
    #[serde(default)]
    screens: Vec<Screen>,
    #[serde(default)]
    actions: Vec<Action>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Screen {
    id: String,
    route: String,
    kind: Option<String>,
    auth: Option<String>,
    title: Option<String>,
    view_name: Option<String>,
    nav_label: Option<String>,
    nav_order: Option<f64>,
    nav_area: Option<String>,
    nav_visible_to: Option<String>,
    #[serde(default)]
    nav_current: Vec<String>,
    #[serde(default)]
    on_enter: Vec<String>,
    #[serde(default)]
    sign_in: bool,
    #[serde(default)]
    view_outputs: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Action {
    id: String,
    screen: String,
    for_each: Option<String>,
    confirm: Option<String>,
    ui_name: Option<String>,
    ui_ask: Option<String>,
    ui_marker: Option<String>,
    ui_row_key: Option<String>,
    ui_label_key: Option<String>,
}

/// A piece of a page title: literal text, or a path into the view model.
#[derive(Debug, PartialEq)]
pub enum Part {
    /// Literal text.
    Text(String),
    /// A dotted path read from the view model.
    Path(Vec<String>),
}

/// One derived output of a view model: `<output> <kind> <from>` and its `name=value` arguments.
#[derive(Debug, PartialEq)]
pub struct Output {
    /// The output's name.
    pub output: String,
    /// `rows`, `choices`, `copy` or `theme-names`.
    pub kind: String,
    /// The section, path or theme list it is derived from.
    pub from: String,
    /// The `name=value` arguments, in order.
    pub args: Vec<(String, String)>,
}

fn is_path_word(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A page title: text with `${a.b.c}` parts.
pub fn parse_title(text: &str) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("${") {
        if at > 0 {
            parts.push(Part::Text(rest[..at].to_string()));
        }
        let after = &rest[at + 2..];
        let Some(end) = after.find('}') else {
            return Err("a `${` is never closed".to_string());
        };
        let path: Vec<String> = after[..end].split('.').map(str::to_string).collect();
        if !path.iter().all(|word| is_path_word(word)) {
            return Err(format!("`{}` is not a dotted path", &after[..end]));
        }
        parts.push(Part::Path(path));
        rest = &after[end + 1..];
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest.to_string()));
    }
    Ok(parts)
}

/// A view output: `<output> <kind> <from>` then `;`-separated `name=value` arguments.
pub fn parse_output(text: &str) -> Result<Output, String> {
    let mut segments = text.split(';');
    let head: Vec<&str> = segments.next().unwrap_or("").split_whitespace().collect();
    let [output, kind, from] = head.as_slice() else {
        return Err(format!(
            "\"{text}\" is not \"<output> <kind> <from>\" with optional \";\"-separated arguments"
        ));
    };
    if !output
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(format!(
            "output \"{output}\" uses characters other than letters, digits and _"
        ));
    }
    if !from.split('.').all(is_path_word) {
        return Err(format!("\"{from}\" is not a dotted path"));
    }
    let mut args = Vec::new();
    for segment in segments {
        let Some((name, value)) = segment.split_once('=') else {
            return Err(format!("argument \"{}\" is not name=value", segment.trim()));
        };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || value.is_empty() {
            return Err(format!("argument \"{}\" is not name=value", segment.trim()));
        }
        args.push((name.to_string(), value.to_string()));
    }
    match *kind {
        "rows" => {
            if args.is_empty() {
                return Err(format!("rows output \"{output}\" names no fields"));
            }
        }
        "choices" => {
            if args.iter().any(|(name, _)| name != "leading-label") {
                return Err(format!(
                    "choices output \"{output}\" takes only leading-label"
                ));
            }
        }
        "copy" => {
            if !args.is_empty() || !from.contains('.') {
                return Err(format!(
                    "copy output \"{output}\" takes a path of a section and a field, and no arguments"
                ));
            }
        }
        "theme-names" => {
            if !args.is_empty() || !matches!(*from, "accents" | "papers") {
                return Err(format!(
                    "theme-names output \"{output}\" takes accents or papers, and no arguments"
                ));
            }
        }
        other => {
            return Err(format!(
                "output \"{output}\" has kind \"{other}\", not rows, choices, copy or theme-names"
            ));
        }
    }
    Ok(Output {
        output: (*output).to_string(),
        kind: (*kind).to_string(),
        from: (*from).to_string(),
        args,
    })
}

fn refuse(out: &mut Vec<Diagnostic>, entity: &str, message: String) {
    out.push(Diagnostic::new(CODE, entity, message));
}

/// Whether a route has an address of its own: no parameters, and not the catch-all.
fn addressable(route: &str) -> bool {
    !route.starts_with('*') && !route.split('/').any(|p| p.starts_with(':'))
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn js_list(items: &[String]) -> String {
    format!(
        "[{}]",
        items.iter().map(|i| sq(i)).collect::<Vec<_>>().join(", ")
    )
}

fn js_parts(parts: &[Part]) -> String {
    format!(
        "[{}]",
        parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => sq(t),
                Part::Path(path) => format!("{{ path: {} }}", js_list(path)),
            })
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn rs(text: &str) -> String {
    codetext::rust::string(text)
}

fn rs_list(items: &[String]) -> String {
    format!(
        "&[{}]",
        items.iter().map(|i| rs(i)).collect::<Vec<_>>().join(", ")
    )
}

fn rows(lines: &[String]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

struct Info<'a> {
    screen: &'a Screen,
    view: String,
    guarded: bool,
    title: Option<Vec<Part>>,
    outputs: Vec<Output>,
}

struct Nav {
    screen: String,
    href: String,
    label: String,
    order: f64,
    area: String,
    visible_to: String,
    current: Vec<String>,
}

struct Row<'a> {
    action: &'a Action,
    name: String,
    ask: Option<String>,
    marker: String,
    row_key: String,
    label_key: String,
}

/// The two modules, `(screens.js, screens.rs)` without their headers, from the parsed
/// specification, or every refusal.
pub fn generate(value: &Value) -> Result<(String, String), Vec<Diagnostic>> {
    let doc = Doc::deserialize(value).map_err(|e| {
        vec![Diagnostic::new(
            CODE,
            SPEC_ENTITY,
            format!("the screens' page data cannot be read: {e}"),
        )]
    })?;
    let mut out = Vec::new();

    let mut infos: Vec<Info> = Vec::new();
    let mut views = BTreeSet::new();
    for s in &doc.screens {
        let guarded = s.auth.as_deref().unwrap_or("none") != "none";
        let view = s.view_name.clone().unwrap_or_else(|| s.id.clone());
        if view.is_empty() {
            refuse(
                &mut out,
                &s.id,
                format!("Screen \"{}\" has an empty viewName.", s.id),
            );
        } else if !views.insert(view.clone()) {
            refuse(
                &mut out,
                &s.id,
                format!(
                    "Screen \"{}\" shows view \"{view}\", which another screen shows too; set viewName so each screen has its own.",
                    s.id
                ),
            );
        }
        if guarded && s.view_name.is_none() {
            refuse(
                &mut out,
                &s.id,
                format!(
                    "Screen \"{}\" is guarded by a session and states no viewName, so the client cannot tell its view from the others.",
                    s.id
                ),
            );
        }
        if guarded && s.title.is_none() {
            refuse(
                &mut out,
                &s.id,
                format!(
                    "Screen \"{}\" is guarded by a session and states no title, so its page has no title.",
                    s.id
                ),
            );
        }
        let title = match s.title.as_deref().map(parse_title) {
            None => None,
            Some(Ok(parts)) => Some(parts),
            Some(Err(why)) => {
                refuse(
                    &mut out,
                    &s.id,
                    format!("The title of screen \"{}\" is not valid: {why}.", s.id),
                );
                None
            }
        };
        let mut outputs = Vec::new();
        for text in &s.view_outputs {
            match parse_output(text) {
                Ok(o) => {
                    if outputs.iter().any(|p: &Output| p.output == o.output) {
                        refuse(
                            &mut out,
                            &s.id,
                            format!(
                                "Screen \"{}\" derives the view output \"{}\" twice.",
                                s.id, o.output
                            ),
                        );
                    } else {
                        outputs.push(o);
                    }
                }
                Err(why) => refuse(
                    &mut out,
                    &s.id,
                    format!("A view output of screen \"{}\" is not valid: {why}.", s.id),
                ),
            }
        }
        let mut seen = BTreeSet::new();
        for name in &s.on_enter {
            if name.trim().is_empty() || !seen.insert(name.as_str()) {
                refuse(
                    &mut out,
                    &s.id,
                    format!(
                        "Screen \"{}\" names an empty or repeated section in onEnter.",
                        s.id
                    ),
                );
            }
        }
        infos.push(Info {
            screen: s,
            view,
            guarded,
            title,
            outputs,
        });
    }

    // Navigation.
    let mut navs: Vec<Nav> = Vec::new();
    for info in &infos {
        let s = info.screen;
        let stated = s.nav_label.is_some()
            || s.nav_order.is_some()
            || s.nav_area.is_some()
            || s.nav_visible_to.is_some()
            || !s.nav_current.is_empty();
        if !stated {
            continue;
        }
        let (Some(label), Some(order), Some(area), Some(visible_to)) = (
            s.nav_label.as_deref(),
            s.nav_order,
            s.nav_area.as_deref(),
            s.nav_visible_to.as_deref(),
        ) else {
            refuse(
                &mut out,
                &s.id,
                format!(
                    "Screen \"{}\" states part of a navigation entry; navLabel, navOrder, navArea and navVisibleTo are all required.",
                    s.id
                ),
            );
            continue;
        };
        if label.trim().is_empty() {
            refuse(
                &mut out,
                &s.id,
                format!("The navigation label of screen \"{}\" is empty.", s.id),
            );
        }
        if !matches!(area, "menu" | "tools") {
            refuse(
                &mut out,
                &s.id,
                format!(
                    "The navigation area of screen \"{}\" is \"{area}\", not menu or tools.",
                    s.id
                ),
            );
        }
        if !matches!(visible_to, "anyone" | "session") {
            refuse(
                &mut out,
                &s.id,
                format!(
                    "The navigation of screen \"{}\" is visible to \"{visible_to}\", not anyone or session.",
                    s.id
                ),
            );
        }
        if !addressable(&s.route) {
            refuse(
                &mut out,
                &s.id,
                format!(
                    "Screen \"{}\" is in the navigation but its route \"{}\" has no address of its own.",
                    s.id, s.route
                ),
            );
        }
        if info.guarded && visible_to != "session" {
            refuse(
                &mut out,
                &s.id,
                format!(
                    "Screen \"{}\" is guarded by a session, so its navigation entry must be visible to session, not \"{visible_to}\".",
                    s.id
                ),
            );
        }
        if let Some(bad) = s
            .nav_current
            .iter()
            .find(|p| !p.starts_with('/') || p.as_str() == "*")
        {
            refuse(
                &mut out,
                &s.id,
                format!(
                    "The navigation of screen \"{}\" is current on \"{bad}\", which is not a path starting with /.",
                    s.id
                ),
            );
        }
        let current = if s.nav_current.is_empty() {
            vec![s.route.clone()]
        } else {
            s.nav_current.clone()
        };
        navs.push(Nav {
            screen: s.id.clone(),
            href: s.route.clone(),
            label: label.to_string(),
            order,
            area: area.to_string(),
            visible_to: visible_to.to_string(),
            current,
        });
    }
    navs.sort_by(|a, b| a.order.total_cmp(&b.order));

    // The sign-in screen.
    let sign_ins: Vec<&Info> = infos.iter().filter(|i| i.screen.sign_in).collect();
    for extra in sign_ins.iter().skip(1) {
        refuse(
            &mut out,
            &extra.screen.id,
            format!(
                "Screen \"{}\" is a second sign-in screen; only one screen may be.",
                extra.screen.id
            ),
        );
    }
    for info in &sign_ins {
        if !addressable(&info.screen.route) || info.guarded {
            refuse(
                &mut out,
                &info.screen.id,
                format!(
                    "The sign-in screen \"{}\" must have a route without parameters and no session guard.",
                    info.screen.id
                ),
            );
        }
    }
    if sign_ins.is_empty() {
        if let Some(guarded) = infos.iter().find(|i| i.guarded) {
            refuse(
                &mut out,
                &guarded.screen.id,
                format!(
                    "Screen \"{}\" is guarded by a session but no screen has signIn, so a visitor has nowhere to sign in.",
                    guarded.screen.id
                ),
            );
        }
    }

    // Row buttons.
    let mut rows_out: Vec<Row> = Vec::new();
    let mut names = BTreeSet::new();
    for a in &doc.actions {
        let stated = a.ui_name.is_some()
            || a.ui_ask.is_some()
            || a.ui_marker.is_some()
            || a.ui_row_key.is_some()
            || a.ui_label_key.is_some();
        if !stated {
            continue;
        }
        let (Some(name), Some(marker), Some(row_key), Some(label_key)) = (
            a.ui_name.as_deref(),
            a.ui_marker.as_deref(),
            a.ui_row_key.as_deref(),
            a.ui_label_key.as_deref(),
        ) else {
            refuse(
                &mut out,
                &a.id,
                format!(
                    "Action \"{}\" states part of a row button; uiName, uiMarker, uiRowKey and uiLabelKey are all required.",
                    a.id
                ),
            );
            continue;
        };
        if a.for_each.is_none() {
            refuse(
                &mut out,
                &a.id,
                format!("Action \"{}\" is a row button but has no forEach.", a.id),
            );
        }
        if !doc.screens.iter().any(|s| s.id == a.screen) {
            refuse(
                &mut out,
                &a.id,
                format!(
                    "Action \"{}\" names screen \"{}\", which is not declared.",
                    a.id, a.screen
                ),
            );
        }
        match (&a.confirm, &a.ui_ask) {
            (Some(_), None) => refuse(
                &mut out,
                &a.id,
                format!(
                    "Action \"{}\" asks for a second press (confirm) but states no uiAsk, the name of the first.",
                    a.id
                ),
            ),
            (None, Some(_)) => refuse(
                &mut out,
                &a.id,
                format!("Action \"{}\" states uiAsk but has no confirm.", a.id),
            ),
            _ => {}
        }
        for n in std::iter::once(name).chain(a.ui_ask.as_deref()) {
            if n.is_empty() || !names.insert(n.to_string()) {
                refuse(
                    &mut out,
                    &a.id,
                    format!(
                        "Action \"{}\" gives its button the name \"{n}\", which is empty or another button's.",
                        a.id
                    ),
                );
            }
        }
        rows_out.push(Row {
            action: a,
            name: name.to_string(),
            ask: a.ui_ask.clone(),
            marker: marker.to_string(),
            row_key: row_key.to_string(),
            label_key: label_key.to_string(),
        });
    }

    if !out.is_empty() {
        return Err(out);
    }

    // screens.js
    let screen_lines: Vec<String> = infos
        .iter()
        .map(|i| {
            let s = i.screen;
            format!(
                "  {{ screen: {}, view: {}, kind: {}, route: {}, auth: {}, title: {}, onEnter: {}, signIn: {} }},",
                sq(&s.id),
                sq(&i.view),
                s.kind
                    .as_deref()
                    .map_or("null".to_string(), |k| sq(k.rsplit('.').next().unwrap_or(k))),
                sq(&s.route),
                sq(s.auth.as_deref().unwrap_or("none")),
                i.title.as_deref().map_or("null".to_string(), js_parts),
                js_list(&s.on_enter),
                s.sign_in,
            )
        })
        .collect();
    let nav_lines: Vec<String> = navs
        .iter()
        .map(|n| {
            format!(
                "  {{ screen: {}, href: {}, label: {}, order: {}, area: {}, visibleTo: {}, current: {} }},",
                sq(&n.screen),
                sq(&n.href),
                sq(&n.label),
                number(n.order),
                sq(&n.area),
                sq(&n.visible_to),
                js_list(&n.current),
            )
        })
        .collect();
    let row_lines: Vec<String> = rows_out
        .iter()
        .map(|r| {
            format!(
                "  {{ action: {}, screen: {}, name: {}, ask: {}, marker: {}, rowKey: {}, labelKey: {}, confirm: {} }},",
                sq(&r.action.id),
                sq(&r.action.screen),
                sq(&r.name),
                r.ask.as_deref().map_or("null".to_string(), sq),
                sq(&r.marker),
                sq(&r.row_key),
                sq(&r.label_key),
                r.action.confirm.is_some(),
            )
        })
        .collect();
    let sign_in = sign_ins.first();
    let js = JS
        .replace("@@SCREENS@@", &rows(&screen_lines))
        .replace("@@NAVIGATION@@", &rows(&nav_lines))
        .replace("@@ROWS@@", &rows(&row_lines))
        .replace(
            "@@SIGN_IN_SCREEN@@",
            &sign_in.map_or("null".to_string(), |i| sq(&i.screen.id)),
        )
        .replace(
            "@@SIGN_IN_PATH@@",
            &sign_in.map_or("null".to_string(), |i| sq(&i.screen.route)),
        );

    // screens.rs
    let mut views_rs = String::new();
    for i in &infos {
        let title = i.title.as_deref().map_or("None".to_string(), |parts| {
            format!(
                "Some(&[{}][..])",
                parts
                    .iter()
                    .map(|p| match p {
                        Part::Text(t) => format!("TitlePart::Text({})", rs(t)),
                        Part::Path(path) => format!("TitlePart::Path({})", rs_list(path)),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        let outputs: String = i
            .outputs
            .iter()
            .map(|o| {
                let args = o
                    .args
                    .iter()
                    .map(|(n, v)| format!("({}, {})", rs(n), rs(v)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "            ViewOutput {{ output: {}, kind: {}, from: {}, args: &[{args}] }},\n",
                    rs(&o.output),
                    rs(&o.kind),
                    rs(&o.from),
                )
            })
            .collect();
        views_rs.push_str(&format!(
            "    ViewInfo {{\n        view: {},\n        title: {title},\n        outputs: &[\n{outputs}        ],\n    }},\n",
            rs(&i.view),
        ));
    }
    Ok((js, RS.replace("@@VIEWS@@", &views_rs)))
}

const JS: &str = r#"// @ts-check
// What the page frame derives from the screens: the view each one shows, who may open it, its
// navigation entry, what is loaded on entry, its title, and the names of its row buttons.

/** @typedef {string | { path: string[] }} TitlePart */

/**
 * A screen. `view` is the name of the view it shows (`model.view`); `kind` the name of its
 * renderer; `title` the parts of its page title (null when it states none).
 * @typedef {{ screen: string, view: string, kind: string | null, route: string, auth: string, title: TitlePart[] | null, onEnter: string[], signIn: boolean }} ScreenInfo
 */

/**
 * A navigation entry. `current` lists the paths it is current on; a path ending in `*` matches by
 * prefix.
 * @typedef {{ screen: string, href: string, label: string, order: number, area: string, visibleTo: string, current: string[] }} NavEntry
 */

/**
 * A per-row action's button: `name` is what the page presses, `ask` the first press of a
 * confirmation, `marker` the status the row shows at once, `rowKey` the output of the row that
 * holds the record id, `labelKey` the key the renderer receives the label under.
 * @typedef {{ action: string, screen: string, name: string, ask: string | null, marker: string, rowKey: string, labelKey: string, confirm: boolean }} RowAction
 */

/** Every screen, in declaration order. @type {ScreenInfo[]} */
export const SCREENS = [
@@SCREENS@@];

/** The navigation entries, in the order they are shown. @type {NavEntry[]} */
export const NAVIGATION = [
@@NAVIGATION@@];

/** The per-row action buttons. @type {RowAction[]} */
export const ROW_ACTIONS = [
@@ROWS@@];

/** The screen a visitor signs in on. @type {string | null} */
export const SIGN_IN_SCREEN = @@SIGN_IN_SCREEN@@;

/** The path of the sign-in screen. @type {string | null} */
export const SIGN_IN_PATH = @@SIGN_IN_PATH@@;

const BY_VIEW = new Map(SCREENS.map((s) => [s.view, s]));
const BY_NAME = new Map(ROW_ACTIONS.map((a) => [a.name, a]));
const BY_ASK = new Map(ROW_ACTIONS.filter((a) => a.ask !== null).map((a) => [a.ask, a]));

/**
 * The screen that shows a view.
 * @param {string | null | undefined} view
 * @returns {ScreenInfo | null}
 */
export function screenOfView(view) {
  return BY_VIEW.get(view ?? '') ?? null;
}

/**
 * Whether a view's screen needs a signed-in person.
 * @param {string | null | undefined} view
 * @returns {boolean}
 */
export function guardedView(view) {
  const info = screenOfView(view);
  return info !== null && info.auth !== 'none';
}

/**
 * The navigation for a path: the entries a visitor may see, in order, each with whether it is the
 * current page.
 * @param {string} path
 * @param {boolean} signedIn
 * @returns {{ screen: string, href: string, label: string, area: string, current: boolean }[]}
 */
export function navigation(path, signedIn) {
  return NAVIGATION.filter((n) => n.visibleTo === 'anyone' || signedIn).map((n) => ({
    screen: n.screen,
    href: n.href,
    label: n.label,
    area: n.area,
    current: n.current.some((p) => (p.endsWith('*') ? path.startsWith(p.slice(0, -1)) : path === p)),
  }));
}

/** @param {any} value @param {string[]} path */
function valueAt(value, path) {
  let at = value;
  for (const key of path) {
    if (at === null || typeof at !== 'object') return '';
    at = at[key];
  }
  return typeof at === 'string' || typeof at === 'number' ? String(at) : '';
}

/**
 * The page title a view's screen states, its `${path}` parts read from the view model; null when
 * the screen states none.
 * @param {string | null | undefined} view
 * @param {any} model
 * @returns {string | null}
 */
export function pageTitle(view, model) {
  const info = screenOfView(view);
  if (!info || info.title === null) return null;
  return info.title.map((part) => (typeof part === 'string' ? part : valueAt(model, part.path))).join('');
}

/**
 * The names of the sections the client loads when a view's screen is shown.
 * @param {string | null | undefined} view
 * @returns {string[]}
 */
export function enterSections(view) {
  return screenOfView(view)?.onEnter ?? [];
}

/**
 * The row buttons of a view's screen.
 * @param {string | null | undefined} view
 * @returns {RowAction[]}
 */
export function rowActionsOn(view) {
  const info = screenOfView(view);
  return info ? ROW_ACTIONS.filter((a) => a.screen === info.screen) : [];
}

/**
 * The row button a press name belongs to.
 * @param {string} name
 * @returns {RowAction | null}
 */
export function rowAction(name) {
  return BY_NAME.get(name) ?? null;
}

/**
 * The row button whose first press (the one that asks for the second) has this name.
 * @param {string} name
 * @returns {RowAction | null}
 */
export function rowActionAsked(name) {
  return BY_ASK.get(name) ?? null;
}
"#;

const RS: &str = r#"// What the backend-for-frontend knows of the screens' views: the title of each and the outputs of
// its view model it derives. Included by crates/bff (the backend-for-frontend names no view).

/// A piece of a page title: literal text, or a dotted path read from the view model.
pub enum TitlePart {
    /// Literal text.
    Text(&'static str),
    /// A dotted path of the view model.
    Path(&'static [&'static str]),
}

/// One output of a view model the backend-for-frontend derives: rows, choices, copy or theme-names.
pub struct ViewOutput {
    /// The output's name.
    pub output: &'static str,
    /// rows, choices, copy or theme-names.
    pub kind: &'static str,
    /// The section, path or theme list it is derived from.
    pub from: &'static str,
    /// The kind's name=value arguments.
    pub args: &'static [(&'static str, &'static str)],
}

/// A view: its title and the outputs the backend-for-frontend adds.
pub struct ViewInfo {
    /// The view's name.
    pub view: &'static str,
    /// The parts of its page title; none when the screen states no title.
    pub title: Option<&'static [TitlePart]>,
    /// The outputs to derive, in order.
    pub outputs: &'static [ViewOutput],
}

/// Every view, in the order of the screens.
pub const VIEWS: &[ViewInfo] = &[
@@VIEWS@@];

/// The view of this name.
pub fn view_info(view: &str) -> Option<&'static ViewInfo> {
    VIEWS.iter().find(|v| v.view == view)
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_text_and_paths() {
        assert_eq!(parse_title("").unwrap(), vec![]);
        assert_eq!(
            parse_title("Editing: ${data.post.fields.title}!").unwrap(),
            vec![
                Part::Text("Editing: ".into()),
                Part::Path(vec![
                    "data".into(),
                    "post".into(),
                    "fields".into(),
                    "title".into()
                ]),
                Part::Text("!".into()),
            ]
        );
        assert!(parse_title("${open").is_err());
        assert!(parse_title("${a..b}").is_err());
    }

    #[test]
    fn outputs_are_checked_by_kind() {
        let o = parse_output("articles rows posts; a=id; b=fields.title").unwrap();
        assert_eq!(
            o.args,
            vec![
                ("a".into(), "id".into()),
                ("b".into(), "fields.title".into())
            ]
        );
        assert!(parse_output("articles rows posts").is_err());
        assert!(parse_output("t choices categories; leading-label=No topic").is_ok());
        assert!(parse_output("t choices categories; other=x").is_err());
        assert!(parse_output("i copy post").is_err());
        assert!(parse_output("i copy post.id").is_ok());
        assert!(parse_output("a theme-names colours").is_err());
        assert!(parse_output("a unknown x").is_err());
    }
}
