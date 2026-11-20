//! A screen added to the specification reaches the client's navigation, guards and entry hooks
//! with no hand-written change (WG-5039): `screens.js` and `screens.rs` are generated from what the
//! screens and row actions state, and a specification that leaves it out is refused.
//!
//! The fixture is the adversarial specification (`tests/fixtures/adversarial/spec.jsonc`) with
//! screens added to its text. The comparison with the page needs `node` (see `support`).

mod support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use strata_spec_compiler::{output, web, Diagnostic, GeneratedFile};

const SPEC: &str = "crates/spec-compiler/tests/fixtures/adversarial/spec.jsonc";
const COMPOSE: &str =
    r#"{ "id": "screen-compose", "name": "Compose", "route": "/compose", "auth": "none" }"#;
const HOME: &str = r#"{ "id": "screen-home", "name": "Home */  ", "route": "/", "kind": "pages.home", "auth": "none" }"#;
const ACTION: &str = r#"{ "id": "default", "screen": "screen-compose","#;

fn fixture() -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/adversarial/spec.jsonc"),
    )
    .unwrap()
}

/// The fixture with screens added after its compose screen, and its home screen replaced.
fn with_screens(home: &str, added: &[&str]) -> String {
    let mut text = fixture().replace(HOME, home);
    let more: String = added.iter().map(|s| format!(",\n    {s}")).collect();
    text = text.replace(COMPOSE, &format!("{COMPOSE}{more}"));
    text
}

fn generated(text: &str) -> Vec<GeneratedFile> {
    match web::generate(SPEC, text) {
        Ok(files) => files,
        Err(d) => panic!("the fixture was refused:\n{}", lines(&d)),
    }
}

fn refused(text: &str) -> Vec<Diagnostic> {
    match web::generate(SPEC, text) {
        Ok(_) => panic!("the specification was not refused"),
        Err(d) => d,
    }
}

fn lines(d: &[Diagnostic]) -> String {
    d.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn file<'a>(files: &'a [GeneratedFile], path: &str) -> &'a str {
    &files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("no generated file {path}"))
        .contents
}

const SIGN_IN_HOME: &str = r#"{ "id": "screen-home", "name": "Home", "route": "/", "kind": "pages.home", "auth": "none", "signIn": true, "viewName": "home", "title": "Home" }"#;
const GUIDE: &str = r#"{ "id": "screen-guide", "name": "Guide", "route": "/guide", "kind": "pages.guide", "auth": "none", "viewName": "guide", "title": "The guide: ${data.guide.fields.name}", "navLabel": "Guide", "navOrder": 5, "navArea": "menu", "navVisibleTo": "anyone", "navCurrent": ["/guide*"] }"#;
const PRIVATE: &str = r#"{ "id": "screen-private", "name": "Private", "route": "/private", "kind": "pages.private", "auth": "session", "viewName": "private", "title": "Private", "navLabel": "Private", "navOrder": 9, "navArea": "tools", "navVisibleTo": "session", "onEnter": ["sessions", "lab"] }"#;

#[test]
fn a_screen_added_to_the_specification_appears_in_the_generated_navigation() {
    let files = generated(&with_screens(SIGN_IN_HOME, &[GUIDE, PRIVATE]));
    let js = file(&files, "screens.js");
    assert!(
        js.contains("  { screen: 'screen-guide', href: '/guide', label: 'Guide', order: 5, area: 'menu', visibleTo: 'anyone', current: ['/guide*'] },\n"),
        "{js}"
    );
    // A guarded screen's entry is for a session only; its sections load on entry.
    assert!(
        js.contains("  { screen: 'screen-private', href: '/private', label: 'Private', order: 9, area: 'tools', visibleTo: 'session', current: ['/private'] },\n"),
        "{js}"
    );
    assert!(
        js.contains("view: 'private', kind: 'private', route: '/private', auth: 'session', title: ['Private'], onEnter: ['sessions', 'lab'], signIn: false"),
        "{js}"
    );
    assert!(js.contains("export const SIGN_IN_PATH = '/';"), "{js}");
    let rs = file(&files, "screens.rs");
    assert!(rs.contains("view: \"guide\""), "{rs}");
    assert!(
        rs.contains("TitlePart::Path(&[\"data\", \"guide\", \"fields\", \"name\"])"),
        "{rs}"
    );
}

#[test]
fn the_generated_navigation_guards_titles_and_entry_hooks_answer_as_stated() {
    if !support::node_required("the run of the generated screens module") {
        return;
    }
    let files = generated(&with_screens(SIGN_IN_HOME, &[GUIDE, PRIVATE]));
    let dir = std::env::temp_dir().join(format!("specc-screens-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    output::write(&dir, &files).unwrap();
    fs::write(dir.join("package.json"), "{ \"type\": \"module\" }\n").unwrap();
    let script = r#"
import assert from 'node:assert/strict';
import { navigation, guardedView, pageTitle, enterSections, screenOfView, SIGN_IN_SCREEN } from './screens.js';
const anon = navigation('/guide/intro', false);
assert.deepEqual(anon.map((n) => [n.label, n.current]), [['Guide', true]]);
const person = navigation('/private', true);
assert.deepEqual(person.map((n) => [n.label, n.area, n.current]), [['Guide', 'menu', false], ['Private', 'tools', true]]);
assert.equal(guardedView('private'), true);
assert.equal(guardedView('guide'), false);
assert.equal(guardedView('no-such-view'), false);
assert.equal(pageTitle('guide', { data: { guide: { fields: { name: 'Start' } } } }), 'The guide: Start');
assert.equal(pageTitle('guide', {}), 'The guide: ');
assert.equal(pageTitle('screen-compose', {}), null);
assert.deepEqual(enterSections('private'), ['sessions', 'lab']);
assert.deepEqual(enterSections('guide'), []);
assert.equal(screenOfView('home').screen, 'screen-home');
assert.equal(SIGN_IN_SCREEN, 'screen-home');
"#;
    fs::write(dir.join("check.mjs"), script).unwrap();
    let run = Command::new("node")
        .arg(dir.join("check.mjs"))
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "the generated screens module misbehaves:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn a_guarded_screen_that_states_no_view_name_title_or_sign_in_screen_is_refused() {
    let bare =
        r#"{ "id": "screen-private", "name": "Private", "route": "/private", "auth": "session" }"#;
    let d = refused(&with_screens(HOME, &[bare]));
    let about_it: Vec<&str> = d
        .iter()
        .filter(|d| d.entity == "screen-private")
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        about_it.iter().any(|m| m.contains("viewName")),
        "{}",
        lines(&d)
    );
    assert!(
        about_it.iter().any(|m| m.contains("no title")),
        "{}",
        lines(&d)
    );
    assert!(
        about_it.iter().any(|m| m.contains("signIn")),
        "{}",
        lines(&d)
    );
    // Stated, it is accepted.
    generated(&with_screens(SIGN_IN_HOME, &[PRIVATE]));
}

#[test]
fn page_data_that_cannot_be_generated_from_is_refused() {
    let cases: [(&str, &str, &str); 8] = [
        (
            r#"{ "id": "screen-x", "name": "X", "route": "/x/:id", "auth": "none", "navLabel": "X", "navOrder": 1, "navArea": "menu", "navVisibleTo": "anyone" }"#,
            "screen-x",
            "no address of its own",
        ),
        (
            r#"{ "id": "screen-x", "name": "X", "route": "/x", "auth": "none", "navLabel": "X" }"#,
            "screen-x",
            "part of a navigation entry",
        ),
        (
            r#"{ "id": "screen-x", "name": "X", "route": "/x", "auth": "session", "viewName": "x", "title": "X", "navLabel": "X", "navOrder": 1, "navArea": "menu", "navVisibleTo": "anyone" }"#,
            "screen-x",
            "visible to session",
        ),
        (
            r#"{ "id": "screen-x", "name": "X", "route": "/x", "auth": "none", "viewName": "guide" }"#,
            "screen-x",
            "which another screen shows too",
        ),
        (
            r#"{ "id": "screen-x", "name": "X", "route": "/x", "auth": "none", "title": "${open" }"#,
            "screen-x",
            "never closed",
        ),
        (
            r#"{ "id": "screen-x", "name": "X", "route": "/x", "auth": "none", "viewOutputs": ["rows posts"] }"#,
            "screen-x",
            "is not valid",
        ),
        (
            r#"{ "id": "screen-x", "name": "X", "route": "/x", "auth": "none", "signIn": true, "viewName": "x" }"#,
            "screen-x",
            "second sign-in screen",
        ),
        (
            r#"{ "id": "screen-x", "name": "X", "route": "/x", "auth": "none", "onEnter": ["a", "a"] }"#,
            "screen-x",
            "repeated section",
        ),
    ];
    for (screen, entity, message) in cases {
        let d = refused(&with_screens(SIGN_IN_HOME, &[GUIDE, screen]));
        assert!(
            d.iter()
                .any(|d| d.entity == entity && d.message.contains(message)),
            "{message}: {}",
            lines(&d)
        );
    }
}

#[test]
fn a_row_button_must_be_complete_and_its_confirmation_named() {
    let partial = fixture().replace(ACTION, &format!("{ACTION} \"uiName\": \"row.run\","));
    let d = refused(&partial);
    assert!(
        d.iter()
            .any(|d| d.entity == "default" && d.message.contains("part of a row button")),
        "{}",
        lines(&d)
    );
    let whole = fixture().replace(
        ACTION,
        &format!(
            "{ACTION} \"forEach\": \"rows\", \"confirm\": \"Sure?\", \"uiName\": \"row.run\", \"uiMarker\": \"done\", \"uiRowKey\": \"id\", \"uiLabelKey\": \"run\","
        ),
    );
    let d = refused(&whole);
    assert!(
        d.iter()
            .any(|d| d.entity == "default" && d.message.contains("no uiAsk")),
        "{}",
        lines(&d)
    );
}
