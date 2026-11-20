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
    FieldDef, FieldId, FieldType, ObjectTypeDef, ObjectTypeId, OnDelete, Principal, PrincipalId,
    Record as KRecord, RecordId, RoleId, Timestamp, Value as KValue,
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

fn refusal(text: &str) -> Vec<String> {
    match strata_spec_compiler::generate_core(LANGUAGE_SPEC, text, LANGUAGE_CRATE) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics.iter().map(ToString::to_string).collect(),
    }
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

#[test]
fn a_language_specc_cannot_generate_is_refused() {
    let text = language_text();
    // An operator naming a primitive outside the catalogue.
    let unknown = text.replace("\"primitive\": \"lower\"", "\"primitive\": \"upper\"");
    assert!(
        refusal(&unknown)
            .iter()
            .any(|d| d.contains("specc.language-invalid") && d.contains("lower")),
        "{:?}",
        refusal(&unknown)
    );
    // An AST variant without an operator.
    let uncovered = text.replace(
        "{ \"name\": \"is-slug\", \"variant\": \"IsSlug\"",
        "{ \"name\": \"is-slug\", \"variant\": \"Nowhere\"",
    );
    assert!(
        refusal(&uncovered)
            .iter()
            .any(|d| d.contains("IsSlug has no operator")),
        "{:?}",
        refusal(&uncovered)
    );
    // A language without generation of the values it needs: no bool literal.
    let no_bool = text.replace(
        "{ \"name\": \"lit-bool\", \"variant\": \"Lit\", \"case\": { \"field\": \"value\", \"value\": \"Bool\" }, \"syntax\": { \"form\": \"literal\", \"token\": \"bool\" }, \"operands\": [], \"result\": \"bool\", \"nulls\": \"total\", \"primitive\": \"literal\" },",
        "",
    );
    assert_ne!(no_bool, text);
    assert!(
        refusal(&no_bool)
            .iter()
            .any(|d| d.contains("specc.language")),
        "{:?}",
        refusal(&no_bool)
    );
    // The fixture itself generates.
    assert_eq!(refusal(&text), Vec::<String>::new());
}

// ---------------------------------------------------------------------------------------------
// The publication template's expressions
// ---------------------------------------------------------------------------------------------

/// One expression of the template with its definition site.
struct Site {
    source: String,
    object: String,
    params: Vec<String>,
}

fn template() -> Json {
    let path = manifest_dir().join("../../tenants/_templates/publication.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn strings(v: &Json) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(str::to_string)
        .collect()
}

fn template_expressions(t: &Json) -> Vec<Site> {
    let mut out = Vec::new();
    let mut push = |source: Option<&str>, object: &str, params: Vec<String>| {
        if let Some(source) = source {
            out.push(Site {
                source: source.to_string(),
                object: object.to_string(),
                params,
            });
        }
    };
    let list = |v: &Json, k: &str| -> Vec<Json> {
        v.get(k)
            .and_then(Json::as_array)
            .cloned()
            .unwrap_or_default()
    };
    for o in list(t, "objects") {
        let id = o["id"].as_str().unwrap();
        for f in list(&o, "fields") {
            push(f.get("default").and_then(Json::as_str), id, Vec::new());
        }
        for c in list(&o, "constraints") {
            push(c.get("check").and_then(Json::as_str), id, Vec::new());
        }
    }
    for w in list(t, "workflows") {
        let object = w["object"].as_str().unwrap();
        for tr in list(&w, "transitions") {
            push(tr.get("guard").and_then(Json::as_str), object, Vec::new());
            if let Some(set) = tr.get("set").and_then(Json::as_object) {
                for value in set.values() {
                    push(value.as_str(), object, Vec::new());
                }
            }
        }
    }
    for p in list(t, "policies") {
        push(
            p.get("when").and_then(Json::as_str),
            p["on"].as_str().unwrap(),
            Vec::new(),
        );
    }
    for q in list(t, "queries") {
        push(
            q.get("where").and_then(Json::as_str),
            q["object"].as_str().unwrap(),
            strings(&q["params"]),
        );
    }
    out
}

#[test]
fn the_generated_printer_prints_the_template_as_the_kernel_does() {
    let sites = template_expressions(&template());
    assert!(
        sites.len() >= 30,
        "the template was read: {} expressions",
        sites.len()
    );
    let mut failures = Vec::new();
    for site in &sites {
        let source = site.source.as_str();
        let kernel = kexpr::parse(source)
            .unwrap_or_else(|e| panic!("the kernel refuses {source}: {e}"))
            .to_string();
        let generated = match lang::parse(source) {
            Ok(expr) => expr.to_string(),
            Err(e) => {
                failures.push(format!("{source}: the generated parser refuses it: {e}"));
                continue;
            }
        };
        if generated != kernel {
            failures.push(format!(
                "{source}:\n  kernel    {kernel}\n  generated {generated}"
            ));
        }
        // What is printed reads back to the same expression.
        match lang::parse(&generated) {
            Ok(again) if again.to_string() == generated => {}
            other => failures.push(format!(
                "{source}: the printed form does not read back: {other:?}"
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
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
];

const BAD: &[&str] = &[
    "record.title ==",
    "drop_table(record.x)",
    "'open",
    "record.a == 1 2",
    "[record.a]",
    "record.a == 1 == 2",
    "ref(-1)",
    "time(x)",
    "len(1, 2)",
    "new.",
    "record.a @ 1",
    "'bad \\q escape'",
    "99999999999999999999",
    "(true",
    "has_role(",
];

#[test]
fn parse_errors_are_the_kernels() {
    let mut failures = Vec::new();
    for source in BAD {
        let kernel = kexpr::parse(source).map(|e| e.to_string());
        let generated = lang::parse(source).map(|e| e.to_string());
        if kernel != generated {
            failures.push(format!(
                "{source}:\n  kernel    {kernel:?}\n  generated {generated:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------------------------
// Evaluation against the kernel
// ---------------------------------------------------------------------------------------------

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
        .map(|s| s.source)
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

// ---------------------------------------------------------------------------------------------
// The static check against the kernel
// ---------------------------------------------------------------------------------------------

/// The template's object types as kernel definitions.
fn objects(t: &Json) -> BTreeMap<ObjectTypeId, ObjectTypeDef> {
    let mut out = BTreeMap::new();
    for o in t["objects"].as_array().unwrap() {
        let id = ObjectTypeId::new(o["id"].as_str().unwrap()).unwrap();
        let mut fields = BTreeMap::new();
        for f in o["fields"].as_array().unwrap() {
            let ty_text = f["type"].as_str().unwrap();
            let ty = |t: &str| match t {
                "string" => FieldType::Str,
                "int" => FieldType::Int,
                "bool" => FieldType::Bool,
                "timestamp" => FieldType::Time,
                other => match other.strip_prefix("ref:") {
                    Some(target) => FieldType::Ref {
                        target: ObjectTypeId::new(target).unwrap(),
                        on_delete: OnDelete::Restrict,
                    },
                    None => panic!("a template field type {other}"),
                },
            };
            let ty = match ty_text.strip_prefix("list:") {
                Some(inner) => FieldType::List(Box::new(ty(inner))),
                None => ty(ty_text),
            };
            let fid = field(f["id"].as_str().unwrap());
            fields.insert(
                fid.clone(),
                FieldDef {
                    id: fid,
                    label: String::new(),
                    ty,
                    required: false,
                    unique: false,
                    default: None,
                    read: None,
                    personal: None,
                },
            );
        }
        out.insert(
            id.clone(),
            ObjectTypeDef {
                id,
                label: String::new(),
                fields,
                constraints: BTreeMap::new(),
                retention: None,
                publication: None,
                language_field: None,
                translation_group: None,
            },
        );
    }
    out
}

fn shape_of(ty: &FieldType) -> lang::Shape {
    match ty {
        FieldType::Str => lang::Shape::Str,
        FieldType::Int => lang::Shape::Int,
        FieldType::Bool => lang::Shape::Bool,
        FieldType::Time => lang::Shape::Time,
        FieldType::Ref { .. } => lang::Shape::Ref,
        FieldType::File { .. } | FieldType::Language => lang::Shape::Str,
        FieldType::List(_) => lang::Shape::List,
    }
}

/// The generated check's definition site over the kernel's definitions.
struct Definitions<'a> {
    object: &'a ObjectTypeDef,
    objects: &'a BTreeMap<ObjectTypeId, ObjectTypeDef>,
    params: &'a BTreeSet<String>,
    roles: &'a BTreeSet<RoleId>,
}

impl Definitions<'_> {
    fn def(&self, field: &str) -> Result<&FieldDef, String> {
        let id = FieldId::new(field).map_err(|e| e.to_string())?;
        self.object
            .fields
            .get(&id)
            .ok_or_else(|| format!("unknown field {field}"))
    }
}

impl lang::TypeEnv for Definitions<'_> {
    fn field_shape(&self, through: Option<&str>, name: &str) -> Result<lang::Shape, String> {
        let fields = match through {
            None => &self.object.fields,
            Some(reference) => {
                let Some((object, _, false)) = self.def(reference)?.ty.ref_target() else {
                    return Err(format!("{reference} is not a single reference"));
                };
                &self
                    .objects
                    .get(object)
                    .ok_or_else(|| format!("unknown object type {object}"))?
                    .fields
            }
        };
        let id = FieldId::new(name).map_err(|e| e.to_string())?;
        fields
            .get(&id)
            .map(|def| shape_of(&def.ty))
            .ok_or_else(|| format!("unknown field {name}"))
    }

    fn follow(&self, field: &str) -> Result<(), String> {
        let Some((object, _, false)) = self.def(field)?.ty.ref_target() else {
            return Err(format!("{field} is not a single reference"));
        };
        self.objects
            .get(object)
            .map(|_| ())
            .ok_or_else(|| format!("unknown object type {object}"))
    }

    fn has_param(&self, name: &str) -> bool {
        self.params.contains(name)
    }

    fn has_role(&self, role: &str) -> bool {
        RoleId::new(role).is_ok_and(|r| self.roles.contains(&r))
    }
}

#[test]
fn the_static_check_agrees_with_the_kernel() {
    let t = template();
    let objects = objects(&t);
    let roles: BTreeSet<RoleId> = t["roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| RoleId::new(r["id"].as_str().unwrap()).unwrap())
        .chain(["owner", "public"].map(|r| RoleId::new(r).unwrap()))
        .collect();
    let mut sites = template_expressions(&t);
    // The extra expressions, checked at the post with the parameter q.
    sites.extend(MORE.iter().map(|s| Site {
        source: s.to_string(),
        object: "post".to_string(),
        params: vec!["q".to_string()],
    }));
    let mut failures = Vec::new();
    for site in &sites {
        let object = &objects[&ObjectTypeId::new(&site.object).unwrap()];
        let params: BTreeSet<String> = site.params.iter().cloned().collect();
        // Check every expression at its own object, and the template's at the post too.
        for at in [object, &objects[&ObjectTypeId::new("post").unwrap()]] {
            let kenv = kexpr::TypeEnv {
                fields: &at.fields,
                params: &params,
                roles: &roles,
                objects: &objects,
            };
            let genv = Definitions {
                object: at,
                objects: &objects,
                params: &params,
                roles: &roles,
            };
            let source = site.source.as_str();
            let kernel =
                kexpr::check(&kexpr::parse(source).unwrap(), &kenv).map(|s| format!("{s:?}"));
            let generated =
                lang::check(&lang::parse(source).unwrap(), &genv).map(|s| format!("{s:?}"));
            if kernel != generated {
                failures.push(format!(
                    "{source} at {}:\n  kernel    {kernel:?}\n  generated {generated:?}",
                    at.id
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
