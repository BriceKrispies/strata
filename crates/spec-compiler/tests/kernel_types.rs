//! The specification's kernel types equal the serde form of the kernel types they describe.
//!
//! Sample values of every kernel type (every variant of every enum) are serialised with serde and
//! checked against the type the specification declares for it (`specifications/spec.jsonc`,
//! section `types`), by a small checker that reads the specification as JSON:
//!
//! - a struct is an object with exactly its fields, under their wire names (kebab-case to
//!   snake_case, as `system.generation.wireCase` says); an `option<T>` field is present and may
//!   be null; a field marked optional may be absent;
//! - a union is written in its `encoding` (serde's external tagging, variant names in the
//!   declared `variantCase`): a unit variant is the bare name, a newtype variant `{Name: value}`,
//!   a tuple variant `{Name: [values]}` and a struct variant `{Name: {fields}}`;
//! - `list<T>`, `set<T>` (an array of distinct items), `map<K, V>` (also a map of maps) and the
//!   built-in representations are checked by shape and range, id and scalar types by their
//!   representation and their `constraints` (`pattern`, `maxLength`);
//! - an enum is one of its values.
//!
//! The checker rejects a sample the specification does not describe: an unknown variant, a
//! missing or extra field, a value of the wrong shape. Each type also has to be reached by at
//! least one sample, so a kernel type added to the list below without a sample fails.
//!
//! The shape check alone would pass a specification that is looser than the kernel. So every
//! sample is also decoded into the type specc generates from the specification
//! (`strata_model::types`, re-exported from `strata-model-core`) and encoded again, and the JSON
//! must come back unchanged: the generated `Expr`, with its tuple variants, the generated
//! `TenantState`, with its map of record maps and its role sets, and every other kernel type
//! read and write exactly what the kernel's serde does.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use regex::Regex;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value as Json;
use strata_kernel::testkit::{ctx, field, object, principal_id, role, starting_worlds, tenant_id};
use strata_kernel::*;
use strata_model::types as generated;
use strata_spec_compiler::jsonc;

struct Types(BTreeMap<String, Json>);

impl Types {
    /// The types of the specification, parsed once for every test in this binary.
    fn load() -> Types {
        static TYPES: std::sync::OnceLock<BTreeMap<String, Json>> = std::sync::OnceLock::new();
        Types(
            TYPES
                .get_or_init(|| {
                    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../specifications/spec.jsonc");
                    let text = fs::read_to_string(path).expect("read the specification");
                    let spec = jsonc::parse(&text).expect("parse the specification");
                    let types = spec["types"].as_array().expect("types").iter();
                    types
                        .map(|t| (t["id"].as_str().unwrap().to_string(), t.clone()))
                        .collect()
                })
                .clone(),
        )
    }
}

fn snake(name: &str) -> String {
    name.replace('-', "_")
}

fn pascal(name: &str) -> String {
    name.split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

/// Splits `K, V` of `map<K, V>` at the top-level comma.
fn split_pair(inner: &str) -> (&str, &str) {
    let mut depth = 0;
    for (i, c) in inner.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => return (inner[..i].trim(), inner[i + 1..].trim()),
            _ => {}
        }
    }
    panic!("not a map pair: {inner}");
}

fn builtin(name: &str, value: &Json) -> Option<Result<(), String>> {
    let integer = |lo: i128, hi: i128| match value
        .as_i64()
        .map(i128::from)
        .or(value.as_u64().map(i128::from))
    {
        Some(n) if (lo..=hi).contains(&n) => Ok(()),
        _ => Err(format!("{value} is not an integer in {lo}..={hi}")),
    };
    Some(match name {
        "bool" => value
            .is_boolean()
            .then_some(())
            .ok_or_else(|| format!("{value} is not a bool")),
        "string" | "uuid" => value
            .is_string()
            .then_some(())
            .ok_or_else(|| format!("{value} is not a string")),
        "u8" => integer(0, u8::MAX.into()),
        "u16" => integer(0, u16::MAX.into()),
        "u32" => integer(0, u32::MAX.into()),
        "u64" => integer(0, u64::MAX.into()),
        "i32" => integer(i32::MIN.into(), i32::MAX.into()),
        "i64" | "timestamp" | "duration" => integer(i64::MIN.into(), i64::MAX.into()),
        "f64" => value
            .is_number()
            .then_some(())
            .ok_or_else(|| format!("{value} is not a number")),
        "json" => Ok(()),
        "bytes" => value
            .is_array()
            .then_some(())
            .ok_or_else(|| format!("{value} is not a byte array")),
        _ => return None,
    })
}

fn check(types: &Types, used: &mut BTreeSet<String>, ty: &str, value: &Json) -> Result<(), String> {
    let ty = ty.trim();
    if let Some(done) = builtin(ty, value) {
        return done;
    }
    // A shared value (`Arc<T>` in Rust) is exactly a T on the wire.
    if let Some(inner) = ty.strip_prefix("shared<").and_then(|t| t.strip_suffix('>')) {
        return check(types, used, inner, value);
    }
    if let Some(inner) = ty.strip_prefix("option<").and_then(|t| t.strip_suffix('>')) {
        return if value.is_null() {
            Ok(())
        } else {
            check(types, used, inner, value)
        };
    }
    if let Some(inner) = ty.strip_prefix("list<").and_then(|t| t.strip_suffix('>')) {
        let items = value
            .as_array()
            .ok_or_else(|| format!("{value} is not a list"))?;
        return items.iter().enumerate().try_for_each(|(i, item)| {
            check(types, used, inner, item).map_err(|e| format!("[{i}]: {e}"))
        });
    }
    if let Some(inner) = ty.strip_prefix("set<").and_then(|t| t.strip_suffix('>')) {
        let items = value
            .as_array()
            .ok_or_else(|| format!("{value} is not a set"))?;
        for (i, item) in items.iter().enumerate() {
            if items[..i].contains(item) {
                return Err(format!("[{i}]: {item} repeats an earlier item of a set"));
            }
            check(types, used, inner, item).map_err(|e| format!("[{i}]: {e}"))?;
        }
        return Ok(());
    }
    if let Some(inner) = ty.strip_prefix("map<").and_then(|t| t.strip_suffix('>')) {
        let (key, val) = split_pair(inner);
        let entries = value
            .as_object()
            .ok_or_else(|| format!("{value} is not a map"))?;
        for (k, v) in entries {
            // JSON object keys are strings; an integer key is written as its decimal text.
            let key_json = match k.parse::<u64>() {
                Ok(n) if !matches!(key_repr(types, key).as_str(), "string") => Json::from(n),
                _ => Json::String(k.clone()),
            };
            check(types, used, key, &key_json).map_err(|e| format!("key {k}: {e}"))?;
            check(types, used, val, v).map_err(|e| format!("{k}: {e}"))?;
        }
        return Ok(());
    }
    let decl = types
        .0
        .get(ty)
        .ok_or_else(|| format!("type {ty} is not declared"))?;
    used.insert(ty.to_string());
    let kind = decl["kind"].as_str().unwrap();
    match kind {
        "id" | "scalar" => {
            let repr = decl["repr"].as_str().unwrap();
            builtin(repr, value).ok_or_else(|| format!("{ty}: repr {repr} is not built in"))??;
            if let (Some(text), Some(c)) = (value.as_str(), decl.get("constraints")) {
                if let Some(max) = c.get("maxLength").and_then(Json::as_u64) {
                    if text.len() as u64 > max {
                        return Err(format!("{ty}: {text:?} is longer than {max}"));
                    }
                }
                if let Some(pattern) = c.get("pattern").and_then(Json::as_str) {
                    if !Regex::new(&strata_spec_compiler::rules::anchored(pattern))
                        .unwrap()
                        .is_match(text)
                    {
                        return Err(format!("{ty}: {text:?} does not match {pattern}"));
                    }
                }
            }
            Ok(())
        }
        "enum" => {
            let text = value
                .as_str()
                .ok_or_else(|| format!("{ty}: {value} is not a string"))?;
            let values = decl["values"].as_array().unwrap();
            values
                .iter()
                .any(|v| v == text)
                .then_some(())
                .ok_or_else(|| format!("{ty}: {text:?} is not a value"))
        }
        "struct" => {
            let object = value
                .as_object()
                .ok_or_else(|| format!("{ty}: {value} is not an object"))?;
            check_fields(types, used, ty, decl["fields"].as_array().unwrap(), object)
        }
        "union" => check_union(types, used, ty, decl, value),
        other => Err(format!("{ty}: unknown kind {other}")),
    }
}

fn key_repr(types: &Types, ty: &str) -> String {
    match types.0.get(ty) {
        Some(decl) => decl["repr"].as_str().unwrap_or("string").to_string(),
        None => ty.to_string(),
    }
}

fn check_fields(
    types: &Types,
    used: &mut BTreeSet<String>,
    ty: &str,
    fields: &[Json],
    object: &serde_json::Map<String, Json>,
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for f in fields {
        let wire = snake(f["name"].as_str().unwrap());
        let field_ty = f["type"].as_str().unwrap();
        match object.get(&wire) {
            Some(v) => check(types, used, field_ty, v).map_err(|e| format!("{ty}.{wire}: {e}"))?,
            None if f["optional"] == true => {}
            None => return Err(format!("{ty}: field {wire} is missing")),
        }
        seen.insert(wire);
    }
    match object.keys().find(|k| !seen.contains(*k)) {
        Some(extra) => Err(format!("{ty}: the specification has no field {extra}")),
        None => Ok(()),
    }
}

fn check_union(
    types: &Types,
    used: &mut BTreeSet<String>,
    ty: &str,
    decl: &Json,
    value: &Json,
) -> Result<(), String> {
    let encoding = &decl["encoding"];
    assert!(
        encoding.get("tag").is_none_or(|t| t == "external"),
        "{ty}: this checker reads externally tagged unions only"
    );
    let wire = |variant: &Json| {
        let name = variant["name"].as_str().unwrap();
        match encoding["variantCase"].as_str() {
            Some("PascalCase") => pascal(name),
            Some("snake_case") => snake(name),
            _ => name.to_string(),
        }
    };
    let variants = decl["variants"].as_array().unwrap();
    let unit = |v: &Json| {
        v.get("fields")
            .is_none_or(|f| f.as_array().unwrap().is_empty())
    };
    match value {
        Json::String(name) => {
            let v = variants
                .iter()
                .find(|v| &wire(v) == name)
                .ok_or_else(|| format!("{ty}: no variant {name}"))?;
            unit(v)
                .then_some(())
                .ok_or_else(|| format!("{ty}: variant {name} is not a unit variant"))
        }
        Json::Object(map) if map.len() == 1 => {
            let (name, content) = map.iter().next().unwrap();
            let v = variants
                .iter()
                .find(|v| &wire(v) == name)
                .ok_or_else(|| format!("{ty}: no variant {name}"))?;
            if unit(v) {
                return Err(format!(
                    "{ty}: variant {name} is a unit variant, written as an object"
                ));
            }
            let fields = v["fields"].as_array().unwrap();
            if v["newtype"] == true {
                check(types, used, fields[0]["type"].as_str().unwrap(), content)
                    .map_err(|e| format!("{ty}::{name}: {e}"))
            } else if v["tuple"] == true {
                let items = content
                    .as_array()
                    .filter(|items| items.len() == fields.len())
                    .ok_or_else(|| {
                        format!(
                            "{ty}::{name}: {content} is not an array of {} values",
                            fields.len()
                        )
                    })?;
                fields
                    .iter()
                    .zip(items)
                    .enumerate()
                    .try_for_each(|(i, (f, item))| {
                        check(types, used, f["type"].as_str().unwrap(), item)
                            .map_err(|e| format!("{ty}::{name}.{i}: {e}"))
                    })
            } else {
                let object = content
                    .as_object()
                    .ok_or_else(|| format!("{ty}::{name}: {content} is not an object"))?;
                check_fields(types, used, &format!("{ty}::{name}"), fields, object)
            }
        }
        _ => Err(format!("{ty}: {value} is not a variant")),
    }
}

/// `value` decodes into `T` and encodes back to exactly `value`.
fn exact<T: Serialize + DeserializeOwned>(value: &Json) -> Result<(), String> {
    let decoded: T = serde_json::from_value(value.clone()).map_err(|e| format!("decode: {e}"))?;
    let back = serde_json::to_value(&decoded).unwrap();
    (back == *value)
        .then_some(())
        .ok_or_else(|| format!("encoded back as {back}"))
}

/// The exact check of a sample of the kernel type the specification names `ty`, through the type
/// specc generates for it; `None` for a type with no generated counterpart listed here.
fn exact_for(ty: &str, value: &Json) -> Option<Result<(), String>> {
    use generated as g;
    Some(match ty {
        "value" => exact::<g::Value>(value),
        "field-type" => exact::<g::FieldType>(value),
        "expr" => exact::<g::Expr>(value),
        "intent" => exact::<g::Intent>(value),
        "schema-change" => exact::<g::SchemaChange>(value),
        "delta-op" => exact::<g::DeltaOp>(value),
        "state-delta" => exact::<g::StateDelta>(value),
        "event-body" => exact::<g::EventBody>(value),
        "domain-event" => exact::<g::DomainEvent>(value),
        "effect" => exact::<g::Effect>(value),
        "output" => exact::<g::Output>(value),
        "transition-result" => exact::<g::TransitionResult>(value),
        "check" => exact::<g::Check>(value),
        "error-kind" => exact::<g::ErrorKind>(value),
        "domain-error" => exact::<g::DomainError>(value),
        "need" => exact::<g::Need>(value),
        "page-cursor" => exact::<g::PageCursor>(value),
        "coverage" => exact::<g::Coverage>(value),
        "principal" => exact::<g::Principal>(value),
        "action" => exact::<g::Action>(value),
        "resource" => exact::<g::Resource>(value),
        "existing-data" => exact::<g::ExistingData>(value),
        "record" => exact::<g::Record>(value),
        "tenant-state" => exact::<g::TenantState>(value),
        "state" => exact::<g::State>(value),
        "tenant-schema" => exact::<g::TenantSchema>(value),
        "read-request" => exact::<g::ReadRequest>(value),
        "read-result" => exact::<g::ReadResult>(value),
        "context" => exact::<g::Context>(value),
        _ => return None,
    })
}

// ---- sample values of the kernel types ------------------------------------------------------

fn json<T: Serialize>(value: &T) -> Json {
    serde_json::to_value(value).unwrap()
}

fn lit_text(s: &str) -> Expr {
    Expr::Lit(Value::Str(s.into()))
}

fn b(e: Expr) -> Box<Expr> {
    Box::new(e)
}

fn exprs() -> Vec<Expr> {
    vec![
        Expr::Lit(Value::Null),
        Expr::Var(Scope::Record, "title".into()),
        Expr::Through(Scope::Old, "author".into(), "name".into()),
        Expr::Param("slug".into()),
        Expr::Now,
        Expr::Principal,
        Expr::HasRole(role("editor")),
        Expr::Not(b(Expr::Lit(Value::Bool(true)))),
        Expr::And(vec![lit_text("a"), lit_text("b")]),
        Expr::Or(vec![]),
        Expr::Cmp(CmpOp::Eq, b(lit_text("a")), b(lit_text("b"))),
        Expr::In(
            b(lit_text("a")),
            b(Expr::Lit(Value::List(vec![Value::Str("a".into())]))),
        ),
        Expr::IsNull(b(Expr::Var(Scope::New, "rank".into()))),
        Expr::Len(b(lit_text("abc"))),
        Expr::Lower(b(lit_text("ABC"))),
        Expr::Contains(b(lit_text("abc")), b(lit_text("b"))),
        Expr::StartsWith(b(lit_text("abc")), b(lit_text("a"))),
        Expr::IsSlug(b(lit_text("a-b"))),
        Expr::If(
            b(Expr::Lit(Value::Bool(true))),
            b(lit_text("x")),
            b(lit_text("y")),
        ),
        Expr::Arith(ArithOp::Add, b(Expr::Now), b(Expr::Lit(Value::Int(1)))),
        Expr::Arith(
            ArithOp::Mul,
            b(Expr::Lit(Value::Int(2))),
            b(Expr::Lit(Value::Int(3))),
        ),
        Expr::MsBetween(b(Expr::Now), b(Expr::Var(Scope::Record, "ends_at".into()))),
        Expr::Duration(DurationUnit::Hours, b(Expr::Lit(Value::Int(2)))),
        Expr::Concat(b(lit_text("a")), b(Expr::Principal)),
        Expr::Text(b(Expr::Var(Scope::Record, "$id".into()))),
    ]
}

fn values() -> Vec<Value> {
    vec![
        Value::Null,
        Value::Bool(false),
        Value::Int(-7),
        Value::Str("text".into()),
        Value::Time(1_000),
        Value::Ref(RecordId(3)),
        Value::List(vec![Value::Int(1), Value::List(vec![])]),
    ]
}

fn field_types() -> Vec<FieldType> {
    vec![
        FieldType::Str,
        FieldType::Int,
        FieldType::Bool,
        FieldType::Time,
        FieldType::Ref {
            target: object("person"),
            on_delete: OnDelete::SetNull,
        },
        FieldType::Ref {
            target: object("person"),
            on_delete: OnDelete::Restrict,
        },
        FieldType::List(Box::new(FieldType::Str)),
    ]
}

fn field_def(id: &str, ty: FieldType, default: Option<Expr>) -> FieldDef {
    FieldDef {
        id: field(id),
        label: id.into(),
        ty,
        required: true,
        unique: false,
        default,
        read: None,
        personal: None,
    }
}

fn schema_changes() -> Vec<SchemaChange> {
    let constraint = ConstraintDef {
        id: ConstraintId::new("short").unwrap(),
        expr: Expr::Lit(Value::Bool(true)),
        message: "short".into(),
    };
    let policy = PolicyDef {
        id: PolicyId::new("read_all").unwrap(),
        role: role("public"),
        resource: Resource::Object(object("note")),
        actions: [Action::Read, Action::Update].into_iter().collect(),
        condition: Some(Expr::Now),
    };
    let workflow = WorkflowDef {
        id: WorkflowId::new("status").unwrap(),
        object: object("note"),
        field: field("status"),
        initial: ["draft".to_string()].into_iter().collect(),
        transitions: vec![WorkflowTransition {
            name: "publish".into(),
            from: "draft".into(),
            to: "published".into(),
            guard: Some(Expr::HasRole(role("editor"))),
            set: [(field("title"), lit_text("x"))].into_iter().collect(),
        }],
    };
    let query = QueryDef {
        id: QueryId::new("recent").unwrap(),
        object: object("note"),
        filter: Some(Expr::Lit(Value::Bool(true))),
        params: ["slug".to_string()].into_iter().collect(),
        order: vec![OrderKey {
            field: "created_at".into(),
            descending: true,
        }],
        limit: Some(10),
        aggregates: None,
        declared_fields: None,
    };
    let view = ViewDef {
        id: ViewId::new("home").unwrap(),
        route: "/".into(),
        title: "Home".into(),
        sections: [(
            "notes".to_string(),
            ViewSection {
                query: QueryId::new("recent").unwrap(),
                args: [
                    ("a".to_string(), ArgSource::Route("slug".into())),
                    ("b".to_string(), ArgSource::Request("q".into())),
                    ("c".to_string(), ArgSource::Const(Value::Int(1))),
                    (
                        "d".to_string(),
                        ArgSource::Section {
                            section: "x".into(),
                            attribute: "$id".into(),
                        },
                    ),
                ]
                .into_iter()
                .collect(),
                include: vec![field("title")],
                markdown: vec![],
                single: false,
                aggregate: None,
            },
        )]
        .into_iter()
        .collect(),
    };
    vec![
        SchemaChange::DefineObject {
            id: object("note"),
            label: "Note".into(),
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: field_def("title", FieldType::Str, Some(lit_text("t"))),
        },
        SchemaChange::DefineRelationship {
            object: object("note"),
            field: field("author"),
            label: "Author".into(),
            target: object("person"),
            on_delete: OnDelete::SetNull,
            required: false,
        },
        SchemaChange::RemoveField {
            object: object("note"),
            field: field("title"),
        },
        SchemaChange::DefineConstraint {
            object: object("note"),
            constraint,
        },
        SchemaChange::RemoveConstraint {
            object: object("note"),
            constraint: ConstraintId::new("short").unwrap(),
        },
        SchemaChange::DefineRole(RoleDef {
            id: role("editor"),
            label: "Editor".into(),
        }),
        SchemaChange::DefinePolicy(policy),
        SchemaChange::RemovePolicy(PolicyId::new("read_all").unwrap()),
        SchemaChange::DefineWorkflow(workflow),
        SchemaChange::RemoveWorkflow(WorkflowId::new("status").unwrap()),
        SchemaChange::DefineQuery(query),
        SchemaChange::RemoveQuery(QueryId::new("recent").unwrap()),
        SchemaChange::DefineView(view),
        SchemaChange::RemoveView(ViewId::new("home").unwrap()),
        SchemaChange::DefineTrigger(TriggerDef {
            id: TriggerId::new("on_new").unwrap(),
            on: TriggerEvent::RecordUpdated {
                object: object("note"),
                transition: Some("publish".into()),
            },
            condition: Some(Expr::Param("seq".into())),
            automation: WorkflowId::new("award").unwrap(),
            paused: Some(false),
            defined_by: None,
            defined_at: None,
        }),
        SchemaChange::RemoveTrigger(TriggerId::new("on_new").unwrap()),
        SchemaChange::DefineAutomation(AutomationDef {
            id: WorkflowId::new("award").unwrap(),
            label: "Award".into(),
            run_as: RunAs {
                roles: [role("editor")].into_iter().collect(),
            },
            run_key: None,
            start: StepName::new("done").unwrap(),
            steps: [(
                StepName::new("done").unwrap(),
                AutomationStepDef {
                    action: AutomationStep::End,
                    next: None,
                    retry: Some(StepRetry {
                        attempts: 3,
                        first_delay: None,
                        max_delay: None,
                    }),
                    timeout: None,
                    compensate: None,
                },
            )]
            .into_iter()
            .collect(),
            defined_by: None,
            defined_at: None,
            suspended_for: None,
            state: None,
        }),
        SchemaChange::RemoveAutomation(WorkflowId::new("award").unwrap()),
        SchemaChange::DefineOperation(TenantOperationDef {
            id: TenantOperationId::new("redeem").unwrap(),
            label: "Redeem".into(),
            inputs: vec![TenantOperationInput {
                name: "note".into(),
                ty: FieldType::Int,
                required: true,
                label: None,
            }],
            invokers: TenantOperationInvokers {
                signed_in: true,
                roles: BTreeSet::new(),
            },
            subject: None,
            conditions: vec![],
            steps: vec![TenantOperationStep::DeleteRecord {
                id: "drop".into(),
                object: object("note"),
                record: Expr::Param("note".into()),
            }],
        }),
        SchemaChange::RemoveOperation(TenantOperationId::new("redeem").unwrap()),
        SchemaChange::DefineUnique {
            object: object("note"),
            unique: UniqueDef {
                id: ConstraintId::new("title_rank").unwrap(),
                fields: vec![field("title"), field("rank")],
                message: "One note per title and rank".into(),
            },
        },
        SchemaChange::RemoveUnique {
            object: object("note"),
            unique: ConstraintId::new("title_rank").unwrap(),
        },
        SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
            unit: points(),
            description: "Points".into(),
            expiry: None,
        }),
        SchemaChange::DeclareLedgerPool(LedgerPoolDecl {
            pool: LedgerPoolName::new("budget").unwrap(),
            description: "Budget".into(),
        }),
        SchemaChange::SetLedgerLimit(LedgerLimit {
            unit: points(),
            owner: Some(writer_owner()),
            floor: Some(LedgerAmount(0)),
            cap: Some(LedgerAmount(100)),
            period_cap: Some(10),
            period: Some(LedgerPeriod::Day),
            scope: Some(LedgerScope::new("42").unwrap()),
        }),
        SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Limit {
            unit: points(),
            owner: None,
            scope: None,
        }),
        SchemaChange::SetRetention {
            object: object("note"),
            retention: RetentionRule {
                days: 30,
                from: RetentionStart::Field {
                    field: field("published_at"),
                },
            },
        },
        SchemaChange::ClearRetention {
            object: object("note"),
        },
        SchemaChange::SetPublication {
            object: object("note"),
            publication: PublicationDef {
                state_field: field("status"),
                draft: "draft".into(),
                published: "published".into(),
                withdrawn: "withdrawn".into(),
                in_review: Some("in_review".into()),
                approved: Some("approved".into()),
                time_field: Some(field("published_at")),
                review_workflow: Some(WorkflowId::new("review").unwrap()),
                review_run_field: Some(field("review_run")),
            },
        },
        SchemaChange::ClearPublication {
            object: object("note"),
        },
        SchemaChange::SetLanguageField {
            object: object("note"),
            field: field("lang"),
        },
        SchemaChange::ClearLanguageField {
            object: object("note"),
        },
        SchemaChange::SetTranslationGroup {
            object: object("note"),
            field: field("group"),
        },
        SchemaChange::ClearTranslationGroup {
            object: object("note"),
        },
        SchemaChange::SetSearchable {
            object: object("note"),
            fields: vec![field("title"), field("body")],
        },
        SchemaChange::ClearSearchable {
            object: object("note"),
        },
        SchemaChange::RemoveObject {
            id: object("note"),
            delete_records: true,
        },
        SchemaChange::RemoveRole(role("guest")),
        SchemaChange::DefineMailTemplate(MailTemplateDecl {
            template: TemplateId("welcome".into()),
            subject: "Welcome".into(),
            body: StructuredBlocks("[]".into()),
        }),
        SchemaChange::RemoveMailTemplate(TemplateId("welcome".into())),
        SchemaChange::DefineNoticeTemplate(NoticeTemplateDecl {
            template: TemplateId("greeting".into()),
            title: "Hello".into(),
            body: StructuredBlocks("[]".into()),
        }),
        SchemaChange::RemoveNoticeTemplate(TemplateId("greeting".into())),
        SchemaChange::DefineToken(TokenDecl {
            name: "first_name".into(),
            expression: Expression("record.title".into()),
            fallback: "friend".into(),
        }),
        SchemaChange::RemoveToken("first_name".into()),
        SchemaChange::RecordProduct(InstalledProduct {
            product: ProductId::new("badges").unwrap(),
            version: "1.0.0".into(),
            platform: Some(PlatformCompat {
                min: "0.2".into(),
                below: None,
            }),
            owns: vec![EntityRef {
                kind: EntityKind::Object,
                id: "badges__award".into(),
            }],
            schema_version: Some(SchemaVersion(3)),
        }),
        SchemaChange::RemoveProduct(ProductId::new("badges").unwrap()),
    ]
}

fn points() -> LedgerUnit {
    LedgerUnit::new("points").unwrap()
}

fn writer_owner() -> LedgerOwner {
    LedgerOwner::Principal {
        principal: principal_id("alpha:writer1"),
    }
}

fn sample_account() -> LedgerAccountId {
    ledger::account_id(&tenant_id("alpha"), &writer_owner(), &points(), "")
}

fn sample_entry() -> LedgerEntry {
    LedgerEntry {
        account: sample_account(),
        sequence: 1,
        amount: LedgerAmount(5),
        idempotency_key: "k-1".into(),
        reason: "welcome".into(),
        caused_by: Some(principal_id("alpha:owner")),
        at: Timestamp(12),
        corrects: None,
    }
}

fn sample_header() -> LedgerAccount {
    LedgerAccount {
        account: sample_account(),
        owner: Some(writer_owner()),
        unit: points(),
        scope: LedgerScope::new("").unwrap(),
        balance: LedgerAmount(5),
        last_sequence: 1,
        opened_at: Timestamp(12),
        periods: vec![LedgerPeriodTotal {
            period: LedgerPeriod::Day,
            start: Timestamp(0),
            credits: 5,
        }],
        buckets: None,
    }
}

fn record() -> Record {
    Record {
        id: RecordId(5),
        object: object("note"),
        version: 2,
        created_by: Some(principal_id("alpha:owner")),
        created_at: Timestamp(10),
        updated_at: Timestamp(11),
        fields: [
            (field("title"), Value::Str("t".into())),
            (field("rank"), Value::Null),
        ]
        .into_iter()
        .collect(),
    }
}

fn error_kinds() -> Vec<ErrorKind> {
    use ErrorKind::*;
    vec![
        TenantMismatch,
        TenantExists,
        TenantNotFound,
        Unauthorized,
        UnknownObject,
        UnknownField,
        UnknownRecord,
        UnknownRole,
        UnknownQuery,
        TypeMismatch,
        MissingRequired,
        ConstraintViolated,
        UniqueViolated,
        DanglingReference,
        StillReferenced,
        WorkflowViolation,
        VersionConflict,
        SchemaConflict,
        InvalidSchema,
        MigrationRejected,
        InsufficientState,
        Evaluation,
        InvalidIntent,
    ]
}

fn needs() -> Vec<Need> {
    vec![
        Need::Record {
            object: object("note"),
            id: RecordId(1),
        },
        Need::ByField {
            object: object("note"),
            field: field("slug"),
            value: Value::Str("a".into()),
        },
        Need::AllOf(object("note")),
        Need::PageOf {
            object: object("note"),
            after: Some(RecordId(1)),
            limit: 50,
        },
        Need::ByFields {
            object: object("note"),
            fields: [
                (field("title"), Value::Str("a".into())),
                (field("rank"), Value::Int(1)),
            ]
            .into_iter()
            .collect(),
        },
        Need::Ledger(LedgerNeed::Account {
            account: sample_account(),
        }),
        Need::Ledger(LedgerNeed::Ranked {
            unit: points(),
            scope: LedgerScope::new("").unwrap(),
            order: LedgerOrder::GreatestFirst,
            offset: 0,
            limit: 21,
        }),
    ]
}

fn samples() -> Vec<(&'static str, Json)> {
    let mut s: Vec<(&'static str, Json)> = Vec::new();
    // A starting world whose alpha tenant holds records, so the record maps are sampled too.
    let alpha = tenant_id("alpha");
    let world = starting_worlds()
        .into_iter()
        .map(|(_, world)| world)
        .find(|world| {
            world
                .tenants
                .get(&alpha)
                .is_some_and(|t| t.records.values().any(|by_id| !by_id.is_empty()))
        })
        .expect("a starting world with records");
    let tenant = world.tenants.get(&alpha).expect("alpha").clone();
    let owner = Principal::User(principal_id("alpha:owner"));

    for e in exprs() {
        s.push(("expr", json(&e)));
    }
    for v in values() {
        s.push(("value", json(&v)));
    }
    for t in field_types() {
        s.push(("field-type", json(&t)));
    }
    for c in schema_changes() {
        s.push(("schema-change", json(&c)));
    }
    for k in error_kinds() {
        s.push(("error-kind", json(&k)));
    }
    for c in [PageCursor::After(RecordId(7)), PageCursor::Offset(40)] {
        s.push(("page-cursor", json(&c)));
    }
    for n in needs() {
        s.push(("need", json(&n)));
        s.push((
            "domain-error",
            json(&DomainError {
                kind: ErrorKind::InsufficientState,
                message: "m".into(),
                evidence: vec![Check::fail("r", "d")],
                missing: Some(n),
            }),
        ));
    }
    s.push((
        "domain-error",
        json(&DomainError::new(ErrorKind::Unauthorized, "no")),
    ));
    s.push(("check", json(&Check::pass("rule", "detail"))));
    for c in [
        Coverage::Complete,
        Coverage::Partial(needs().into_iter().collect()),
    ] {
        s.push(("coverage", json(&c)));
    }
    for p in [Principal::System, Principal::Anonymous, owner.clone()] {
        s.push(("principal", json(&p)));
    }
    for a in [
        Action::Read,
        Action::Create,
        Action::Update,
        Action::Delete,
        Action::Manage,
    ] {
        s.push(("action", json(&a)));
    }
    for r in [
        Resource::Object(object("note")),
        Resource::AnyObject,
        Resource::Schema,
        Resource::Members,
        Resource::Ledger { unit: None },
        Resource::Ledger {
            unit: Some(points()),
        },
    ] {
        s.push(("resource", json(&r)));
    }
    for e in [ExistingData::Reject, ExistingData::Grandfather] {
        s.push(("existing-data", json(&e)));
    }
    s.push(("record", json(&record())));
    s.push(("tenant-state", json(&*tenant)));
    s.push(("tenant-schema", json(&*tenant.schema)));
    s.push(("tenant-schema", json(&TenantSchema::genesis())));
    s.push(("state", json(&State::complete(tenant.clone()))));
    s.push(("state", json(&State::absent(alpha.clone()))));
    s.push((
        "state",
        json(&State {
            tenant_id: alpha.clone(),
            tenant: Some(tenant.clone()),
            coverage: Coverage::Partial(needs().into_iter().collect()),
        }),
    ));
    let context = Context {
        tenant: alpha.clone(),
        principal: owner.clone(),
        session: Some(SessionId::new("sess:1").unwrap()),
        now: Timestamp(9),
        correlation: "c-1".into(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    };
    s.push(("context", json(&context)));
    s.push(("context", json(&ctx("alpha", Principal::Anonymous, 5))));

    let intents = vec![
        Intent::CreateTenant {
            name: "Gamma".into(),
            owner: principal_id("gamma:owner"),
        },
        Intent::ChangeSchema {
            changes: schema_changes(),
            expected_version: Some(SchemaVersion(1)),
            existing: ExistingData::Grandfather,
        },
        Intent::ChangeSchema {
            changes: vec![],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        Intent::AssignRole {
            principal: principal_id("alpha:writer1"),
            role: role("writer"),
        },
        Intent::RevokeRole {
            principal: principal_id("alpha:writer1"),
            role: role("writer"),
        },
        Intent::CreateRecord {
            object: object("note"),
            fields: [(field("title"), Value::Str("t".into()))]
                .into_iter()
                .collect(),
        },
        Intent::UpdateRecord {
            object: object("note"),
            id: RecordId(1),
            fields: BTreeMap::new(),
            expected_version: Some(1),
        },
        Intent::UpdateRecord {
            object: object("note"),
            id: RecordId(1),
            fields: BTreeMap::new(),
            expected_version: None,
        },
        Intent::DeleteRecord {
            object: object("note"),
            id: RecordId(1),
            expected_version: None,
        },
        Intent::PostLedger {
            postings: vec![LedgerPostingRequest {
                owner: writer_owner(),
                unit: points(),
                amount: LedgerAmount(-3),
                idempotency_key: "k-2".into(),
                reason: "redeem".into(),
                corrects: Some(1),
                scope: None,
            }],
        },
    ];
    for i in &intents {
        s.push(("intent", json(i)));
    }

    let rec = Arc::new(record());
    let delta_ops = vec![
        DeltaOp::CreateTenant {
            name: "Gamma".into(),
        },
        DeltaOp::PutSchema(tenant.schema.clone()),
        DeltaOp::PutMember {
            principal: principal_id("alpha:owner"),
            roles: [role("owner"), role("editor")].into_iter().collect(),
        },
        DeltaOp::PutRecord(rec.clone()),
        DeltaOp::DeleteRecord {
            object: object("note"),
            id: RecordId(5),
        },
        DeltaOp::Ledger(LedgerDelta::PutAccount(sample_header())),
        DeltaOp::Ledger(LedgerDelta::AppendEntry(sample_entry())),
    ];
    for o in &delta_ops {
        s.push(("delta-op", json(o)));
    }
    s.push((
        "state-delta",
        json(&StateDelta {
            tenant: alpha.clone(),
            base_revision: Some(StateRevision(4)),
            new_revision: StateRevision(5),
            next_record: 7,
            event_seq: 9,
            ops: delta_ops,
        }),
    ));
    let bodies = vec![
        EventBody::TenantCreated {
            name: "Gamma".into(),
        },
        EventBody::SchemaChanged {
            version: SchemaVersion(2),
            summary: vec!["define note".into()],
            grandfathered: 0,
        },
        EventBody::MemberChanged {
            principal: principal_id("alpha:owner"),
            roles: [role("owner")].into_iter().collect(),
        },
        EventBody::RecordCreated {
            record: rec.clone(),
        },
        EventBody::RecordUpdated {
            before: rec.clone(),
            after: rec.clone(),
            transitions: None,
        },
        EventBody::RecordUpdated {
            before: rec.clone(),
            after: rec.clone(),
            transitions: Some(BTreeMap::from([(
                WorkflowId::new("status").unwrap(),
                "publish".to_string(),
            )])),
        },
        EventBody::RecordDeleted {
            before: rec.clone(),
        },
        EventBody::AutomationSuspended {
            automation: WorkflowId::new("award").unwrap(),
            definer: principal_id("alpha:editor"),
            role: role("editor"),
        },
        EventBody::LedgerEntryPosted {
            account: sample_account(),
            owner: writer_owner(),
            unit: points(),
            scope: LedgerScope::new("").unwrap(),
            amount: LedgerAmount(5),
            balance: LedgerAmount(5),
            balance_before: LedgerAmount(0),
            entry_sequence: 1,
            reason: "welcome".into(),
        },
    ];
    for body in bodies {
        s.push(("event-body", json(&body)));
        s.push((
            "domain-event",
            json(&DomainEvent {
                tenant: alpha.clone(),
                seq: 3,
                revision: StateRevision(5),
                at: Timestamp(12),
                principal: owner.clone(),
                correlation: "c".into(),
                body,
            }),
        ));
    }
    for e in [
        Effect::Publish { seq: 3 },
        Effect::RevokeSessions {
            principal: principal_id("alpha:owner"),
        },
        Effect::StartAutomation(generated::AutomationStart {
            trigger: TriggerId::new("on_new").unwrap(),
            automation: WorkflowId::new("award").unwrap(),
            event_seq: 3,
            hop: 1,
            caused_by: principal_id("automation:award"),
            on_behalf_of: Some(principal_id("alpha:owner")),
            definition_version: SchemaVersion(2),
            run_key: Some("ab".repeat(32)),
            params: BTreeMap::from([("seq".to_string(), Value::Int(3))]),
        }),
        Effect::DeclineAutomation(generated::AutomationDecline {
            trigger: TriggerId::new("on_new").unwrap(),
            automation: WorkflowId::new("award").unwrap(),
            event_seq: 3,
            hop: 9,
            reason: generated::AutomationExceptionReason::HopLimit,
            detail: "hop 9 is past the hop limit of 8".into(),
        }),
    ] {
        s.push(("effect", json(&e)));
    }
    for o in [
        Output::Tenant,
        Output::Schema {
            version: SchemaVersion(2),
        },
        Output::Member,
        Output::Record {
            object: object("note"),
            id: RecordId(5),
            version: 1,
        },
        Output::Deleted {
            object: object("note"),
            id: RecordId(5),
        },
        Output::LedgerPosted {
            results: vec![LedgerPostingResult {
                entry: sample_entry(),
                balance: LedgerBalance {
                    account: sample_account(),
                    unit: points(),
                    balance: LedgerAmount(5),
                    last_sequence: 1,
                    scope: None,
                },
                replayed: false,
            }],
        },
    ] {
        s.push(("output", json(&o)));
    }
    for r in [
        ReadRequest::Named {
            query: QueryId::new("recent").unwrap(),
            params: [("slug".to_string(), Value::Str("a".into()))]
                .into_iter()
                .collect(),
            page_size: Some(20),
            cursor: Some(PageCursor::After(RecordId(7))),
        },
        ReadRequest::Records {
            object: object("note"),
            ids: vec![RecordId(1), RecordId(2)],
        },
    ] {
        s.push(("read-request", json(&r)));
    }
    s.push((
        "read-result",
        json(&ReadResult {
            rows: vec![rec],
            revision: StateRevision(5),
            schema_version: SchemaVersion(2),
            next: Some(PageCursor::Offset(40)),
        }),
    ));

    // Transitions the kernel really produces: found a tenant, change its schema, write a record.
    let mut world = World::default();
    let steps = vec![
        (
            Principal::System,
            Intent::CreateTenant {
                name: "Delta".into(),
                owner: principal_id("delta:owner"),
            },
        ),
        (
            Principal::User(principal_id("delta:owner")),
            Intent::ChangeSchema {
                changes: schema_changes()[..3].to_vec(),
                expected_version: None,
                existing: ExistingData::Reject,
            },
        ),
    ];
    for (i, (who, intent)) in steps.into_iter().enumerate() {
        if let Ok((next, done)) = world.step(&intent, &ctx("delta", who, 100 + i as i64)) {
            s.push(("transition-result", json(&done)));
            world = next;
        }
    }
    s
}

#[test]
fn every_kernel_sample_decodes_exactly_into_the_generated_types() {
    let mut failures = Vec::new();
    let mut checked = BTreeSet::new();
    for (ty, value) in samples() {
        match exact_for(ty, &value) {
            Some(Ok(())) => {
                checked.insert(ty);
            }
            Some(Err(e)) => failures.push(format!("{ty}: {e}\n    sample: {value}")),
            None => failures.push(format!("{ty}: no generated type is listed for it")),
        }
    }
    assert!(
        failures.is_empty(),
        "kernel values the generated types do not read and write exactly:\n{}",
        failures.join("\n")
    );
    // The expression language and the stored records, which WG-4927 could state only by shape.
    for ty in [
        "expr",
        "record",
        "tenant-state",
        "coverage",
        "delta-op",
        "event-body",
    ] {
        assert!(
            checked.contains(ty),
            "no sample of {ty} was checked exactly"
        );
    }
    let tenant = samples()
        .into_iter()
        .find(|(ty, _)| *ty == "tenant-state")
        .map(|(_, v)| v)
        .unwrap();
    let decoded: generated::TenantState = serde_json::from_value(tenant).unwrap();
    assert!(
        decoded.records.values().any(|by_id| !by_id.is_empty()),
        "the tenant sample holds records, so the map of record maps was decoded"
    );
}
