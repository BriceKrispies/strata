//! The generated JavaScript, run with `node`, against Exegra's own evaluator
//! (`exegra/src/core/expr.js`). Skipped with a message when either is absent.

mod common;

use common::{scope, TestScope};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use strata_spec_expr::{parse, to_js, Ty};

fn env() -> TestScope {
    scope(&[
        ("a", Ty::U32),
        ("b", Ty::U32),
        ("n", Ty::I64),
        ("signed", Ty::I32),
        ("f", Ty::F64),
        ("g", Ty::F64),
        ("p", Ty::Bool),
        ("q", Ty::Bool),
        ("s", Ty::String),
        ("t", Ty::String),
        ("o", Ty::Option(Box::new(Ty::U32))),
        ("event.token", Ty::U32),
    ])
}

#[derive(Clone, Copy, PartialEq)]
enum Expect {
    /// Same value, or both raise an error.
    Same,
    /// The JavaScript raises the overflow error; Exegra's unbounded double does not.
    JsOverflow,
}

use Expect::{JsOverflow, Same};

/// (expression, variables as a JSON object keyed by reference name, expectation)
fn cases() -> Vec<(&'static str, &'static str, Expect)> {
    vec![
        ("a + b * 2", r#"{"a":3,"b":4}"#, Same),
        ("a - b", r#"{"a":5,"b":2}"#, Same),
        ("(a + b) * 2 == 14", r#"{"a":3,"b":4}"#, Same),
        ("a % 3", r#"{"a":10}"#, Same),
        ("a % b", r#"{"a":10,"b":0}"#, Same),
        ("signed % 3", r#"{"signed":-7}"#, Same),
        ("-signed", r#"{"signed":5}"#, Same),
        ("-signed", r#"{"signed":0}"#, Same),
        ("n * 3 - 1", r#"{"n":-4}"#, Same),
        ("f / g", r#"{"f":7,"g":2}"#, Same),
        ("f / g", r#"{"f":7,"g":0}"#, Same),
        ("f % g", r#"{"f":7.5,"g":2}"#, Same),
        ("f % g", r#"{"f":7.5,"g":0}"#, Same),
        ("f * g - 1.5", r#"{"f":2,"g":0.25}"#, Same),
        ("-f", r#"{"f":2.5}"#, Same),
        ("f - -1.5", r#"{"f":2.5}"#, Same),
        ("-(-f)", r#"{"f":2.5}"#, Same),
        ("f - (g - 1)", r#"{"f":10,"g":4}"#, Same),
        ("p && q", r#"{"p":true,"q":false}"#, Same),
        ("p || q", r#"{"p":false,"q":true}"#, Same),
        ("p => q", r#"{"p":true,"q":false}"#, Same),
        ("p => q", r#"{"p":false,"q":false}"#, Same),
        ("p => q => p", r#"{"p":true,"q":true}"#, Same),
        ("(p => q) => p", r#"{"p":false,"q":false}"#, Same),
        ("!p", r#"{"p":true}"#, Same),
        ("!(a == b)", r#"{"a":1,"b":2}"#, Same),
        ("p && a % b == 0", r#"{"p":false,"a":1,"b":0}"#, Same),
        ("q || a % b == 0", r#"{"q":true,"a":1,"b":0}"#, Same),
        ("p => a % b == 0", r#"{"p":false,"a":1,"b":0}"#, Same),
        ("p => a % b == 0", r#"{"p":true,"a":1,"b":0}"#, Same),
        ("s + t", r#"{"s":"ab","t":"cd"}"#, Same),
        ("s + t == 'abcd'", r#"{"s":"ab","t":"cd"}"#, Same),
        ("s == 'a\"b'", r#"{"s":"a\"b"}"#, Same),
        ("s == t", r#"{"s":"x","t":"y"}"#, Same),
        ("o == null", r#"{"o":null}"#, Same),
        ("o == null", r#"{"o":5}"#, Same),
        ("o != null", r#"{"o":5}"#, Same),
        ("a == b", r#"{"a":2,"b":2}"#, Same),
        ("a < b", r#"{"a":1,"b":2}"#, Same),
        ("a <= b", r#"{"a":3,"b":2}"#, Same),
        ("a > b", r#"{"a":3,"b":2}"#, Same),
        ("a >= b", r#"{"a":2,"b":2}"#, Same),
        ("event.token + 1 == 8", r#"{"event.token":7}"#, Same),
        ("a + b", r#"{"a":4294967295,"b":1}"#, JsOverflow),
        ("a - b", r#"{"a":0,"b":1}"#, JsOverflow),
        ("a * b", r#"{"a":65536,"b":65536}"#, JsOverflow),
        ("-signed", r#"{"signed":-2147483648}"#, JsOverflow),
        ("signed % -1", r#"{"signed":-2147483648}"#, JsOverflow),
        ("n + 1", r#"{"n":9007199254740991}"#, JsOverflow),
        ("n - 1", r#"{"n":-9007199254740991}"#, JsOverflow),
    ]
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn ident(name: &str) -> String {
    name.replace(['.', '-'], "_")
}

#[test]
fn generated_javascript_agrees_with_exegra_evaluator() {
    let exegra = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../exegra/src/core/expr.js");
    if !exegra.exists() {
        eprintln!("skipped: {} is absent", exegra.display());
        return;
    }
    if !node_available() {
        eprintln!("skipped: node is not on PATH");
        return;
    }
    let exegra = exegra.canonicalize().unwrap();
    let e = env();
    let table = cases();

    let mut script = String::new();
    script.push_str("import { pathToFileURL } from 'node:url';\n");
    script.push_str("const { evaluate } = await import(pathToFileURL(process.argv[2]).href);\n");
    script.push_str("class Overflow extends Error {}\n");
    script.push_str("function __ov() { throw new Overflow('overflow'); }\n");
    script.push_str("const cases = [];\n");
    for (src, vars, _) in &table {
        let ast = parse(src).unwrap();
        let code =
            to_js(&ast, &e).unwrap_or_else(|err| panic!("`{src}` does not translate: {err}"));
        let vars_json: Value = serde_json::from_str(vars).unwrap();
        let mut binds = String::new();
        for name in vars_json.as_object().unwrap().keys() {
            binds.push_str(&format!(
                "const {} = v[{}]; ",
                ident(name),
                serde_json::to_string(name).unwrap()
            ));
        }
        script.push_str(&format!(
            "cases.push({{ src: {}, vars: {}, f: (v) => {{ {}return ({}); }} }});\n",
            serde_json::to_string(src).unwrap(),
            vars,
            binds,
            code
        ));
    }
    script.push_str(
        "const run = (g) => { try { return { v: g() }; } catch (e) { return { err: e instanceof Overflow ? 'overflow' : 'error' }; } };\n\
         const out = cases.map((c) => ({\n\
           js: run(() => c.f(c.vars)),\n\
           ex: run(() => evaluate(c.src, (path) => c.vars[path.join('.')])),\n\
         }));\n\
         console.log(JSON.stringify(out));\n",
    );

    let dir = std::env::temp_dir().join(format!("strata-spec-expr-js-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("cases.mjs");
    std::fs::write(&file, script).unwrap();
    let output = Command::new("node")
        .arg(&file)
        .arg(&exegra)
        .output()
        .expect("node runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "node failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let results: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(results.len(), table.len());
    for ((src, vars, expect), r) in table.iter().zip(&results) {
        let (js, ex) = (&r["js"], &r["ex"]);
        match expect {
            Expect::Same => {
                let both_err = js.get("err").is_some() && ex.get("err").is_some();
                assert!(
                    both_err || js == ex,
                    "`{src}` with {vars}: js {js}, exegra {ex}"
                );
            }
            Expect::JsOverflow => assert_eq!(
                js["err"], "overflow",
                "`{src}` with {vars}: js {js}, exegra {ex}"
            ),
        }
    }
}
