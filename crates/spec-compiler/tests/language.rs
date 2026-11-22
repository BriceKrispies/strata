//! The generated language (WG-4931) against the kernel it is pinned to.
//!
//! - The golden fixture: generating the pure crate from `tests/fixtures/language/spec.jsonc` gives
//!   exactly `crates/spec-compiler-language-fixture` (`SPECC_BLESS=1` rewrites it).
//! - Printer equality: every expression of `tenants/_templates/publication.json` (field defaults,
//!   constraints, workflow guards and assignments, policy conditions, query filters), parsed with
//!   the fixture's generated parser and printed with its generated printer, is byte for byte what
//!   the kernel prints for its own reading of it. Evidence strings embed that text
//!   (crates/kernel/src/rules.rs).
//! - Parse errors, evaluation and the static check agree with `strata_kernel::expr::parse`,
//!   `strata_kernel::expr::eval` and `strata_kernel::expr::check` on the same expressions.
//!
//! Since WG-4932 the kernel's side is itself generated: the specification's
//! `tenant-rule-language` in `strata_model_core`, read through the kernel's adapters (its `Env`,
//! `TypeEnv` and records). These tests were first run against the hand-written kernel
//! (WG-4931), so they now pin the generated kernel language to the fixture that matched it.
//! - A language specc cannot generate is refused with its diagnostics.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde_json::Value as Json;
use strata_kernel::expr as kexpr;
use strata_kernel::ext::*;
use strata_kernel::{
    FieldId, ObjectTypeId, Principal, PrincipalId, Record as KRecord, RecordId, RoleId, Timestamp,
    Value as KValue,
};
use strata_spec_compiler::output;
use strata_spec_compiler_language_fixture::rule_language as lang;
use strata_spec_compiler_language_fixture::types::{Scope, Value};

const LANGUAGE_SPEC: &str = "crates/spec-compiler/tests/fixtures/language/spec.jsonc";
const LANGUAGE_CRATE: &str = "strata-spec-compiler-language-fixture";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn language_text() -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures/language/spec.jsonc")).unwrap()
}

#[test]
fn language_golden_output_matches_its_fixture_crate() {
    let dir = manifest_dir().join("../spec-compiler-language-fixture");
    let files = match strata_spec_compiler::generate_core(
        LANGUAGE_SPEC,
        &language_text(),
        LANGUAGE_CRATE,
    ) {
        Ok(files) => files,
        Err(diagnostics) => panic!(
            "the language fixture was refused:\n{}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    };
    assert!(files.iter().any(|f| f.path == "src/rule_language.rs"));
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
    assert!(
        differences.is_empty(),
        "crates/spec-compiler-language-fixture is not what specc generates from the language fixture: {differences:?}. Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}

// ---------------------------------------------------------------------------------------------
// The publication template's expressions
// ---------------------------------------------------------------------------------------------

fn template() -> Json {
    let path = manifest_dir().join("../../tenants/_templates/publication.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn template_expressions(t: &Json) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |source: Option<&str>| {
        if let Some(source) = source {
            out.push(source.to_string());
        }
    };
    let list = |v: &Json, k: &str| -> Vec<Json> {
        v.get(k)
            .and_then(Json::as_array)
            .cloned()
            .unwrap_or_default()
    };
    for o in list(t, "objects") {
        for f in list(&o, "fields") {
            push(f.get("default").and_then(Json::as_str));
        }
        for c in list(&o, "constraints") {
            push(c.get("check").and_then(Json::as_str));
        }
    }
    for w in list(t, "workflows") {
        for tr in list(&w, "transitions") {
            push(tr.get("guard").and_then(Json::as_str));
            if let Some(set) = tr.get("set").and_then(Json::as_object) {
                for value in set.values() {
                    push(value.as_str());
                }
            }
        }
    }
    for p in list(t, "policies") {
        push(p.get("when").and_then(Json::as_str));
    }
    for q in list(t, "queries") {
        push(q.get("where").and_then(Json::as_str));
    }
    out
}

/// Expressions beyond the template, for the parser, the evaluator and the check.
const MORE: &[&str] = &[
    "len(new.title) <= 120 and not is_null(new.slug) or has_role(editor)",
    "new.kind in ['a', 'b\\'c', 'd\\\\e']",
    "if(is_null(old.rank), 0, old.rank) < -3",
    "new.when == time(1700000000000) and new.who != ref(7) and new.flag == true and is_null(null)",
    "not not record.status == 'draft'",
    "[1, 'a', [true, null], time(3), ref(4)]",
    "len(new.tags) >= 1 and 'x' in new.tags",
    "contains(new.tags, 1) or contains(new.title, 'He')",
    "lower(new.title) == 'hello' and starts_with(new.title, 'He')",
    "new.title < 'z' and new.published_at > time(0) and new.$version >= 1",
    "true and 1",
    "false and 1",
    "1 or true",
    "not 1",
    "len(3)",
    "lower(1)",
    "contains(1, 2)",
    "contains(new.title, 1)",
    "starts_with(1, 'a')",
    "is_slug(1)",
    "if(1, 2, 3)",
    "1 in 'abc'",
    "1 < 'a'",
    "ref(1) < ref(2)",
    "true < false",
    "[1] < [2]",
    "null == null",
    "new.author.name",
    "new.category.name",
    "new.title.name",
    "new.Bad",
    "new.Bad.name",
    "old.title",
    "param.q",
    "param.missing",
    "principal",
    "now",
    "if(true, 1, 'a')",
    "if(new.featured, new.title, null)",
    // Checked arithmetic, time arithmetic and text (D-6540 to D-6543, WG-5064).
    "new.rank + 1 > 4 and new.rank * 2 - 3 == 5",
    "new.rank-1",
    "new.rank - -1",
    "new.published_at + days(1) > now and now - hours(2) < new.published_at",
    "ms_between(new.published_at, now) >= minutes(0)",
    "9007199254740991 + 1",
    "-9007199254740991 - 1",
    "4503599627370496 * 2",
    "9007199254740991 * 9007199254740991",
    "days(104249992)",
    "ms_between(time(-9007199254740991), time(1))",
    "concat(new.title, concat(':', text(new.rank)))",
    "text(new.author) == '7' and text(new.featured) == 'true' and text(new.published_at) == '500'",
    "text(new.tags)",
    "text(null)",
    "new.title + 1",
    "1 + new.published_at",
    "new.published_at - new.published_at",
    "new.published_at * 2",
    "minutes(new.title)",
    "ms_between(1, now)",
    "concat(new.title, 1)",
    "concat(new.title, null)",
    "param.q + 1",
];

/// The generated language's record, reading a kernel record as the kernel reads it.
struct Rec<'a>(&'a KRecord);

impl lang::Record for Rec<'_> {
    fn system_attribute(&self, name: &str) -> Value {
        to_generated(&kexpr::attribute(Some(self.0), name).expect("a system attribute"))
    }

    fn field(&self, name: &str) -> Result<Value, String> {
        FieldId::new(name)
            .map(|id| to_generated(self.0.get(&id)))
            .map_err(|e| e.to_string())
    }
}

/// The generated language's environment over the same data as a kernel `Env` without a lookup.
struct World<'a> {
    record: Option<Rec<'a>>,
    old: Option<Rec<'a>>,
    new: Option<Rec<'a>>,
    params: &'a BTreeMap<String, KValue>,
    now: i64,
    principal: &'a Principal,
    roles: &'a BTreeSet<RoleId>,
}

impl lang::Env for World<'_> {
    fn record(&self, scope: &Scope) -> Option<&dyn lang::Record> {
        let record = match scope {
            Scope::Record => &self.record,
            Scope::Old => &self.old,
            Scope::New => &self.new,
        };
        record.as_ref().map(|r| r as &dyn lang::Record)
    }

    fn follow(
        &self,
        _scope: &Scope,
        field: &str,
        _target: &u64,
    ) -> Result<Option<Box<dyn lang::Record + '_>>, lang::EvalError> {
        // The kernel's Env here has no lookup.
        Err(lang::EvalError(format!("{field} cannot be followed here")))
    }

    fn param(&self, name: &str) -> Option<Value> {
        self.params.get(name).map(to_generated)
    }

    fn now(&self) -> i64 {
        self.now
    }

    fn principal(&self) -> Option<String> {
        match self.principal {
            Principal::User(id) => Some(id.as_str().to_string()),
            _ => None,
        }
    }

    fn has_role(&self, role: &str) -> bool {
        RoleId::new(role).is_ok_and(|r| self.roles.contains(&r))
    }
}

fn field(name: &str) -> FieldId {
    FieldId::new(name).unwrap()
}

fn record(id: u64, fields: &[(&str, KValue)]) -> KRecord {
    KRecord {
        id: RecordId(id),
        object: ObjectTypeId::new("post").unwrap(),
        version: 3,
        created_by: Some(PrincipalId::new("u1").unwrap()),
        created_at: Timestamp(10),
        updated_at: Timestamp(20),
        fields: fields.iter().map(|(k, v)| (field(k), v.clone())).collect(),
    }
}

fn text(s: &str) -> KValue {
    KValue::Str(s.to_string())
}

fn posts() -> (KRecord, KRecord) {
    let new = record(
        1,
        &[
            ("title", text("Hello")),
            ("slug", text("hello-world")),
            ("body", text("Some text")),
            ("excerpt", text("Short")),
            ("author", KValue::Ref(RecordId(7))),
            ("tags", KValue::List(vec![text("a"), text("b")])),
            ("status", text("published")),
            ("featured", KValue::Bool(true)),
            ("published_at", KValue::Time(500)),
            ("cover", text("/media/a.png")),
            ("cover_alt", text("A picture")),
            ("key", text("main")),
            ("name", text("Strata")),
            ("accent", text("moss")),
            ("paper", text("linen")),
            ("tagline", text("Notes")),
            ("rank", KValue::Int(4)),
        ],
    );
    let old = record(
        1,
        &[
            ("title", text("Hello")),
            ("status", text("draft")),
            ("author", KValue::Ref(RecordId(8))),
            ("featured", KValue::Bool(false)),
            ("rank", KValue::Int(-5)),
        ],
    );
    (new, old)
}

#[test]
fn evaluation_agrees_with_the_kernel() {
    let (new, old) = posts();
    let params: BTreeMap<String, KValue> = [
        ("q".to_string(), text("ell")),
        ("slug".to_string(), text("hello-world")),
        ("category".to_string(), KValue::Null),
        ("author".to_string(), KValue::Ref(RecordId(7))),
    ]
    .into_iter()
    .collect();
    let user = Principal::User(PrincipalId::new("u1").unwrap());
    let principals = [user, Principal::Anonymous, Principal::System];
    let role_sets: Vec<BTreeSet<RoleId>> = vec![
        BTreeSet::new(),
        ["editor"]
            .iter()
            .map(|r| RoleId::new(*r).unwrap())
            .collect(),
        ["author", "owner"]
            .iter()
            .map(|r| RoleId::new(*r).unwrap())
            .collect(),
    ];
    let sources: Vec<String> = template_expressions(&template())
        .into_iter()
        .chain(MORE.iter().map(|s| s.to_string()))
        .collect();
    let mut failures = Vec::new();
    let mut compared = 0;
    for with_old in [false, true] {
        for principal in &principals {
            for roles in &role_sets {
                let kenv = kexpr::Env {
                    record: Some(&new),
                    old: with_old.then_some(&old),
                    new: Some(&new),
                    params: &params,
                    now: Timestamp(500),
                    principal,
                    roles,
                    lookup: None,
                };
                let genv = World {
                    record: Some(Rec(&new)),
                    old: with_old.then_some(Rec(&old)),
                    new: Some(Rec(&new)),
                    params: &params,
                    now: 500,
                    principal,
                    roles,
                };
                for source in &sources {
                    let kexpr_ = kexpr::parse(source).unwrap();
                    let gexpr = lang::parse(source).unwrap();
                    let kernel = kexpr::eval(&kexpr_, &kenv)
                        .map(|v| to_generated(&v))
                        .map_err(|e| e.0);
                    let generated = lang::eval(&gexpr, &genv).map_err(|e| e.0);
                    if kernel != generated {
                        failures.push(format!(
                            "{source} (old: {with_old}, {principal:?}, {roles:?}):\n  kernel    {kernel:?}\n  generated {generated:?}"
                        ));
                    }
                    let kernel_holds = kexpr::holds(&kexpr_, &kenv).map_err(|e| e.0);
                    let generated_holds = lang::holds(&gexpr, &genv).map_err(|e| e.0);
                    if kernel_holds != generated_holds {
                        failures.push(format!("{source}: holds differs"));
                    }
                    compared += 1;
                }
            }
        }
    }
    assert!(compared > 100);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn to_generated(v: &KValue) -> Value {
    match v {
        KValue::Null => Value::Null,
        KValue::Bool(b) => Value::Bool(*b),
        KValue::Int(i) => Value::Int(*i),
        KValue::Str(s) => Value::Str(s.clone()),
        KValue::Time(t) => Value::Time(*t),
        KValue::Ref(id) => Value::Ref(id.0),
        KValue::List(items) => Value::List(items.iter().map(to_generated).collect()),
    }
}
