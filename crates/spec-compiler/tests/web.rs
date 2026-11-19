//! The web backend: client machines as JavaScript modules.
//!
//! - the golden files `tests/fixtures/machines-web/web/` are what `specc web` generates from the
//!   machines fixture (`SPECC_BLESS=1` rewrites them);
//! - every generated file passes `node --check`;
//! - the generated `login-form` steps exactly as Exegra runs scenario `login-run`, driven by
//!   `tests/fixtures/machines-web/web-differential.mjs`;
//! - the generated payload checks refuse what Rust's serde refuses.
//!
//! The tests that need `node` or Exegra fail without them (see `support`); only an explicit
//! `SPECC_SKIP_NODE_TESTS=1` or `SPECC_SKIP_EXEGRA_TESTS=1` skips them, with a banner.
//! - a client machine is not generated as Rust, and a specification without one gives no files.

mod support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Value};
use strata_spec_compiler::{
    generate, jsonc, output, web, GeneratedFile, Options, RuntimeDependency,
};

const SPEC: &str = "crates/spec-compiler/tests/fixtures/machines-web/spec.jsonc";
const MACHINE: &str = "login-form";
const SCENARIO: &str = "login-run";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn machines_text() -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures/machines-web/spec.jsonc")).unwrap()
}

fn web_files() -> Vec<GeneratedFile> {
    match web::generate(SPEC, &machines_text()) {
        Ok(files) => files,
        Err(diagnostics) => panic!(
            "the fixture was refused:\n{}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

/// A fresh, empty directory under the system temporary directory.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("specc-web-{name}-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The generated files written to a directory that node reads as ES modules.
fn written(name: &str) -> PathBuf {
    let dir = scratch_dir(name);
    output::write(&dir, &web_files()).unwrap();
    fs::write(dir.join("package.json"), "{ \"type\": \"module\" }\n").unwrap();
    dir
}

#[test]
fn golden_output_matches_the_fixture_directory() {
    let dir = manifest_dir().join("tests/fixtures/machines-web/web");
    let files = web_files();
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
    assert!(
        differences.is_empty(),
        "tests/fixtures/machines-web/web is not what specc web generates from the machines fixture: {differences:?}. \
         Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}

#[test]
fn generated_files_are_valid_javascript() {
    if !support::node_required("the node --check of the generated files") {
        return;
    }
    let dir = written("check");
    let files = web_files();
    assert!(files.iter().any(|f| f.path == "login-form.js"));
    assert!(files.iter().any(|f| f.path == web::RUNTIME_FILE));
    for file in &files {
        let output = Command::new("node")
            .arg("--check")
            .arg(dir.join(&file.path))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "node --check {}:\n{}",
            file.path,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn a_client_machine_is_not_generated_as_rust() {
    let options = Options {
        crate_name: "out".to_string(),
        runtime: RuntimeDependency::Workspace,
    };
    let files = generate(SPEC, &machines_text(), &options).unwrap();
    // The fixture's only machine runs in the browser, so the crate has no machines module.
    assert!(!files.iter().any(|f| f.path == "src/machines.rs"));
    assert!(!files.iter().any(|f| f.contents.contains("impl Machine")));
}

#[test]
fn an_omitted_option_payload_field_is_null() {
    let files = web_files();
    let js = &files
        .iter()
        .find(|f| f.path == "login-form.js")
        .unwrap()
        .contents;
    // `credentials-sent` has an `option<string>` field `hint` that the machine never gives.
    assert!(js.contains("attempt: next.attempts, hint: null }"), "{js}");
    assert!(js.contains("hint?: string | null"), "{js}");
}

#[test]
fn payload_members_of_an_event_that_crosses_a_contract_follow_its_wire_case() {
    // credentials-sent becomes the request of a contract in camelCase, with a field hint-text.
    let text = machines_text()
        .replace(
            r#"{ "id": "credentials-sent", "name": "Credentials sent", "delivery": "at-least-once","#,
            r#"{ "id": "credentials-sent", "name": "Credentials sent", "delivery": "at-least-once", "contract": "send-credentials", "role": "request","#,
        )
        .replace(
            r#"{ "name": "hint", "type": "option<string>" } ] },"#,
            r#"{ "name": "hint-text", "type": "option<string>" } ] },"#,
        )
        .replace(
            r#"  "events": ["#,
            r#"  "contracts": [ { "id": "send-credentials", "name": "Send credentials", "binding": { "case": "camelCase" },
      "inputs": [ { "name": "name", "type": "string" }, { "name": "attempt", "type": "u32" }, { "name": "hint-text", "type": "option<string>" } ] } ],
  "events": ["#,
        );
    let files = web::generate(SPEC, &text).unwrap();
    let js = &files
        .iter()
        .find(|f| f.path == "login-form.js")
        .unwrap()
        .contents;
    assert!(
        js.contains("attempt: next.attempts, hintText: null }"),
        "{js}"
    );
    assert!(js.contains("hintText?: string | null"), "{js}");
    assert!(!js.contains("hint-text"), "{js}");
}

#[test]
fn a_specification_without_client_machines_gives_no_files() {
    let text = fs::read_to_string(manifest_dir().join("tests/fixtures/full/spec.jsonc")).unwrap();
    assert_eq!(web::generate("spec.jsonc", &text), Ok(Vec::new()));
}

#[test]
fn a_client_machine_refuses_what_it_cannot_express() {
    let text = machines_text().replace(
        r#"{ "id": "accept", "from": "sending", "to": "signed-in", "on": "login-accepted", "otherwise": true,
          "effects": [ { "cancel": "timeout" } ] }"#,
        r#"{ "id": "accept", "from": "sending", "to": "signed-in", "on": "login-accepted", "otherwise": true,
          "effects": [ { "acquire": "slot" } ] }"#,
    );
    let diagnostics = web::generate(SPEC, &text).unwrap_err();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, "specc.resource-effect");
    assert_eq!(diagnostics[0].entity, MACHINE);
}

#[test]
fn the_generated_login_form_steps_as_exegra_runs_it() {
    let what = "the comparison of the generated login form with Exegra";
    let Some(cli) = support::exegra_cli(what) else {
        return;
    };
    if !support::node_required(what) {
        return;
    }
    let specs = manifest_dir().join("tests").join("fixtures");
    let output = Command::new("node")
        .arg(&cli)
        .args(["trace", SCENARIO, "--project", "machines-web", "--specs"])
        .arg(&specs)
        .output()
        .unwrap();
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "the Exegra CLI did not print JSON ({e}):\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(
        envelope["ok"],
        json!(true),
        "Exegra refused the fixture: {envelope:#}"
    );

    let dir = written("differential");
    let trace = dir.join("trace.json");
    let spec = dir.join("spec.json");
    fs::write(&trace, envelope["result"].to_string()).unwrap();
    fs::write(&spec, jsonc::parse(&machines_text()).unwrap().to_string()).unwrap();
    let driver = manifest_dir().join("tests/fixtures/machines-web/web-differential.mjs");
    let run = Command::new("node")
        .arg(&driver)
        .arg(&dir)
        .arg(MACHINE)
        .arg(&trace)
        .arg(&spec)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "the generated login form disagrees with Exegra:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// The Strata specification, from the repository root.
fn strata_spec() -> (String, PathBuf) {
    let root = manifest_dir().join("../..");
    let text = fs::read_to_string(root.join("specifications/spec.jsonc")).unwrap();
    (text, root)
}

#[test]
fn the_web_application_holds_what_specc_web_generates_from_the_specification() {
    let (text, root) = strata_spec();
    let files = web::generate("specifications/spec.jsonc", &text).unwrap_or_else(|d| {
        panic!(
            "the specification was refused:\n{}",
            d.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    });
    for name in [
        "routes.js",
        "contracts.js",
        "forms.js",
        "actions.js",
        "live.js",
        "site-path.js",
        "errors.js",
    ] {
        assert!(
            files.iter().any(|f| f.path == name),
            "{name} is not generated"
        );
    }
    let differences = output::compare(&root.join("web/src/generated"), &files).unwrap();
    assert!(
        differences.is_empty(),
        "web/src/generated is not what specc web generates from specifications/spec.jsonc: {differences:?}. \
         Run specc web --spec specifications/spec.jsonc --out web/src/generated and commit."
    );
}

#[test]
fn the_generated_kernel_modules_route_derive_validate_and_refresh() {
    if !support::node_required("the run of the generated kernel modules") {
        return;
    }
    let (_, root) = strata_spec();
    let generated = root.join("web/src/generated");
    let url = |name: &str| {
        format!(
            "file:///{}",
            generated
                .join(name)
                .display()
                .to_string()
                .replace('\\', "/")
                .trim_start_matches('/')
        )
    };
    let script = format!(
        r#"
import assert from 'node:assert/strict';
const routes = await import({routes:?});
const forms = await import({forms:?});
const contracts = await import({contracts:?});
const live = await import({live:?});
const actions = await import({actions:?});
const errors = await import({errors:?});
assert.deepEqual(routes.matchRoute('/posts/slow-software', ''), {{ screen: 'screen-article', params: {{ slug: 'slow-software' }}, query: {{}} }});
assert.deepEqual(routes.matchRoute('/desk/new', ''), {{ screen: 'screen-desk-new', params: {{}}, query: {{}} }});
assert.deepEqual(routes.matchRoute('/search', '?q=wood+kiln&x=1'), {{ screen: 'screen-search', params: {{}}, query: {{ q: 'wood kiln' }} }});
assert.deepEqual(routes.matchRoute('/no/such/route', ''), {{ screen: 'screen-not-found', params: {{ missing: '/no/such/route' }}, query: {{}} }});
// An empty segment is a segment, and the catch-all's parameter is decoded.
assert.equal(routes.matchRoute('/posts//x', '').screen, 'screen-not-found');
assert.equal(routes.matchRoute('//posts/x', '').screen, 'screen-not-found');
assert.equal(routes.matchRoute('/posts/', '').screen, 'screen-not-found');
assert.deepEqual(routes.matchRoute('/no/such%20route/%C3%A9', '').params, {{ missing: '/no/such route/é' }});
assert.deepEqual(routes.matchRoute('/no/%zz', '').params, {{ missing: '/no/%zz' }});
// Only a path on this site is a same-site address; the URL parser decides.
const site = await import({site:?});
for (const bad of ['\\evil.com', '/\\evil.com', '/\t/evil.com', '/\n/evil.com', '//evil.com', 'https://evil.com', 'evil', '', null])
  assert.equal(site.sameSite(bad, '/home'), '/home', String(bad));
assert.equal(site.sameSite('/desk?x=1#y', '/home'), '/desk?x=1#y');
assert.equal(site.sitePath('/\\evil.com'), null);
assert.equal(routes.hrefFor('screen-search', {{ q: 'kiln' }}), '/search?q=kiln');
assert.equal(routes.hrefFor('screen-search', {{ q: '' }}), '/search');
assert.equal(routes.hrefFor('screen-desk-edit', {{ slug: 'on-sharpening' }}), '/desk/edit/on-sharpening');
let {{ values, edited }} = forms.input('form-new-article-save', forms.initialValues('form-new-article-save', {{ view: {{ topics: [{{ value: '', label: 'No topic' }}], authors: [] }} }}), [], 'title', 'Inès & the Café');
assert.equal(values.slug, 'ines-the-cafe');
({{ values, edited }} = forms.input('form-new-article-save', values, edited, 'slug', 'mine'));
({{ values, edited }} = forms.input('form-new-article-save', values, edited, 'title', 'Other'));
assert.equal(values.slug, 'mine');
assert.deepEqual(forms.validate('form-sign-in', {{ login: '  ', password: 'x' }}), {{ login: 'Enter your name and password.' }});
assert.deepEqual(forms.validate('form-sign-in', {{ login: 'ines', password: 'x' }}), {{}});
assert.equal(forms.succeeded('form-sign-in', {{ query: {{ next: '//evil' }}, response: {{ session: {{ display_name: 'Inès' }} }} }}).navigate, '/desk');
assert.deepEqual(contracts.request('read-front-page', {{}}, '/'), {{ method: 'GET', path: '/api/view?path=%2F', write: false }});
assert.deepEqual(contracts.request('delete-article', {{ article: 9, expected_version: 2 }}).body, {{ op: 'delete', object: 'post', id: 9, expected_version: 2 }});
// An output that repeats an input is read from the reply, never from the request.
assert.deepEqual(contracts.decode('create-article', {{ id: 4, version: 1, fields: {{ slug: 'saved' }} }}, {{ slug: 'asked' }}), {{ article_id: 4, version: 1, slug: 'saved' }});
const view = {{ depends_on: ['post'] }};
assert.equal(live.refreshes('screen-front-page', 'ui-notice-received', {{ kind: 'upsert', object: 'post' }}, view), true);
assert.equal(live.refreshes('screen-front-page', 'ui-notice-received', {{ kind: 'upsert', object: 'invoice' }}, view), false);
assert.equal(actions.label('act-desk-delete', {{ title: 'On Sharpening' }}), 'Delete On Sharpening');
assert.equal(actions.confirmText('act-desk-delete', {{ title: 'On Sharpening' }}), 'Confirm deleting On Sharpening');
// The records contracts encode their inputs: trimmed, comma lists, numbers, empty as null (an
// update) or left out (a create); an output repeating an input repeats it as sent.
assert.deepEqual(contracts.request('update-article', {{ article: 3, expected_version: 2, title: ' T ', slug: 's', excerpt: '  ', body: 'b', category: '', author: '7', tags: 'a, ,b', cover: '', cover_alt: ' x ', featured: false }}).body.fields,
  {{ title: 'T', slug: 's', excerpt: null, body: 'b', category: null, author: 7, tags: ['a', 'b'], cover: null, cover_alt: 'x', featured: null }});
assert.deepEqual(contracts.request('create-article', {{ title: 'T', slug: 's', excerpt: '', body: 'b', category: '', author: '7', tags: '', cover: '', cover_alt: '', featured: false }}).body.fields,
  {{ title: 'T', slug: 's', body: 'b', author: 7 }});
assert.equal(contracts.decode('update-article', {{ id: 4, version: 2, fields: {{ slug: 'x' }} }}, {{ slug: ' y ' }}).slug, 'x');
// Publishing a new article is the second call of its form; the outcome reads both replies.
assert.deepEqual(forms.followUp('form-new-article-publish', {{ response: {{ article_id: 4, version: 1, slug: 'x' }} }}), {{ contract: 'update-article', inputs: {{ article: 4, expected_version: 1, status: 'published' }} }});
assert.equal(forms.followUp('form-new-article-save', {{ response: {{}} }}), null);
assert.deepEqual(forms.bothResponses({{ article_id: 4, version: 1, slug: 'x' }}, {{ article_id: 4, version: 2, slug: undefined }}), {{ article_id: 4, version: 2, slug: 'x' }});
// Initial values from the view model, and the principal's own author.
const post = {{ fields: {{ title: 'T', slug: 's', body: 'b', author: 7, category: 2, tags: ['a', 'b'], featured: true }} }};
const edit = forms.initialValues('form-edit-article-save-draft', {{ view: {{ data: {{ post }}, topics: [{{ value: '', label: 'No topic' }}], authors: [{{ value: '7', label: 'A' }}] }} }});
assert.deepEqual([edit.title, edit.category, edit.author, edit.tags, edit.featured, edit.excerpt], ['T', '2', '7', 'a, b', true, '']);
const authors = [{{ id: 5, fields: {{ principal: 'p-1' }} }}, {{ id: 9, fields: {{ principal: 'p-2' }} }}];
const choices = [{{ value: '5', label: 'X' }}, {{ value: '9', label: 'Y' }}];
assert.equal(forms.initialValues('form-new-article-save', {{ view: {{ data: {{ authors }}, authors: choices, topics: [] }}, principal: {{ id: 'p-2' }} }}).author, '9');
assert.equal(forms.initialValues('form-new-article-save', {{ view: {{ data: {{ authors }}, authors: choices, topics: [] }}, principal: {{ id: 'p-3' }} }}).author, '5');
assert.deepEqual(forms.held('form-settings', {{ publication_version: 3, name: 'x' }}), {{ publication_version: 3 }});
assert.deepEqual(forms.succeeded('form-settings', {{ response: {{ version: 4 }} }}).view, {{ publication_version: 4 }});
// The search is trimmed; a sign-in keeps its texts as typed.
assert.deepEqual(forms.submitted('form-search', {{ q: '  kiln ' }}), {{ q: 'kiln' }});
assert.deepEqual(forms.submitted('form-sign-in', {{ login: ' ines ' }}), {{ login: ' ines ', password: '' }});
// Sign-out is offered on every screen; the delete confirmation is announced.
assert.ok(actions.actionsOn('screen-lab').includes('act-sign-out-desk'));
assert.ok(actions.actionsOn('screen-about').includes('act-sign-out-front-page'));
assert.equal(actions.ACTIONS['act-desk-delete'].confirmAnnounce, 'Press Confirm delete to remove the article for good.');
// The wording of refusals.
const failed = {{ code: 'invalid', message: 'm', failed: [{{ rule: 'constraint:post.slug_format', detail: 'A slug uses lower-case letters [slug_format]' }}, {{ rule: 'required:post.title', detail: 'x' }}] }};
assert.deepEqual(errors.refusal('form-edit-article-save-draft', failed), {{ errors: {{ slug: 'A slug uses lower-case letters.', title: 'This is required.' }}, formError: '', announce: 'Some fields need attention.', text: 'A slug uses lower-case letters.' }});
assert.equal(errors.refusal('form-edit-article-save-draft', {{ code: 'conflict', message: 'm' }}).formError, 'This was changed somewhere else since you opened it. Reload to see the latest version.');
assert.equal(errors.refusal('form-settings', {{ code: 'unavailable', message: '' }}).formError, 'That did not work. Try again.');
assert.equal(errors.refusal('form-sign-in', {{ code: 'unauthenticated', message: 'The name or password is not right.' }}).formError, 'The name or password is not right.');
assert.equal(errors.refusal('form-sign-in', {{ code: 'unavailable', message: 'The site could not be reached.' }}).formError, 'Signing in did not work. Try again.');
assert.equal(errors.refusal('act-desk-delete', {{ code: 'forbidden', message: 'No.', failed: [{{ rule: 'policy:authors_own_posts', detail: 'x' }}] }}).text, 'Not done. You are not allowed to do this.');
assert.equal(errors.refusal('act-sign-out-desk', {{ code: 'unauthenticated', message: 'x' }}).text, 'Signing out did not work. Try again.');
// The impact lab refreshes on a notice of a schema change.
assert.equal(live.refreshes('screen-lab', 'ui-notice-received', {{ kind: 'schema', object: '' }}, {{}}), true);
"#,
        routes = url("routes.js"),
        forms = url("forms.js"),
        contracts = url("contracts.js"),
        live = url("live.js"),
        actions = url("actions.js"),
        errors = url("errors.js"),
        site = url("site-path.js"),
    );
    let dir = scratch_dir("kernel");
    let path = dir.join("kernel.mjs");
    fs::write(&path, script).unwrap();
    let run = Command::new("node").arg(&path).output().unwrap();
    assert!(
        run.status.success(),
        "the generated kernel modules misbehave:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn the_generated_machines_refuse_payloads_that_rust_refuses() {
    if !support::node_required("the run of the generated payload checks") {
        return;
    }
    let (_, root) = strata_spec();
    let generated = root.join("web/src/generated");
    let url = |name: &str| {
        format!(
            "file:///{}",
            generated
                .join(name)
                .display()
                .to_string()
                .replace('\\', "/")
                .trim_start_matches('/')
        )
    };
    let script = format!(
        r#"
import assert from 'node:assert/strict';
const live = await import({live:?});
const submit = await import({submit:?});
const runtime = await import({runtime:?});
const {{ state, vars }} = live.initial();
const welcome = (payload) => () => live.step(state, vars, {{ event: 'ui-live-welcome', payload }}, 0);
const refused = (name, fn) => assert.throws(fn, (e) => e instanceof runtime.StepError && e.kind === 'invalid-event', name);
// Accepted: what serde accepts for a u64 and a u32 (within the integers JavaScript holds exactly).
assert.doesNotThrow(welcome({{ head: 10, 'heartbeat-ms': 10000 }}));
assert.doesNotThrow(welcome({{ head: Number.MAX_SAFE_INTEGER, 'heartbeat-ms': 4294967295 }}));
// Refused, as serde refuses them: a fraction or a string for a number, a negative for an unsigned
// integer, a value past the type's range, a missing member, an object that is not one.
refused('fraction for u64', welcome({{ head: 1.5, 'heartbeat-ms': 10000 }}));
refused('string for u64', welcome({{ head: '10', 'heartbeat-ms': 10000 }}));
refused('negative for u64', welcome({{ head: -1, 'heartbeat-ms': 10000 }}));
refused('past u32', welcome({{ head: 1, 'heartbeat-ms': 4294967296 }}));
refused('null for u64', welcome({{ head: null, 'heartbeat-ms': 10000 }}));
refused('missing member', welcome({{ head: 1 }}));
refused('not an object', welcome('x'));
// Refused by the +-(2^53 - 1) bound: Rust accepts these, JavaScript cannot hold them.
refused('past 2^53 - 1', welcome({{ head: Number.MAX_SAFE_INTEGER + 1, 'heartbeat-ms': 10000 }}));
// An event the machine does not receive is refused, as the Rust machine cannot decode it.
refused('unknown event', () => live.step(state, vars, {{ event: 'ml-requested', payload: {{}} }}, 0));
// A u8 member.
const form = submit.initial();
const submitted = (payload) => () => submit.step(form.state, form.vars, {{ event: 'ui-form-submitted', payload }}, 0);
assert.doesNotThrow(submitted({{ complete: 1 }}));
refused('past u8', submitted({{ complete: 256 }}));
refused('boolean for u8', submitted({{ complete: true }}));
"#,
        live = url("client-live-flow-impl.js"),
        submit = url("client-submit-flow-impl.js"),
        runtime = url("machine-runtime.js"),
    );
    let dir = scratch_dir("payload-checks");
    let path = dir.join("checks.mjs");
    fs::write(&path, script).unwrap();
    let run = Command::new("node").arg(&path).output().unwrap();
    assert!(
        run.status.success(),
        "the generated payload checks misbehave:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}
