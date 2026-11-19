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
//!   declared `variantCase`): a unit variant is the bare name, a newtype variant `{Name: value}`
//!   and a struct variant `{Name: {fields}}`;
//! - `list<T>`, `map<K, V>` and the built-in representations are checked by shape and range, id
//!   and scalar types by their representation and their `constraints` (`pattern`, `maxLength`);
//! - an enum is one of its values.
//!
//! The checker rejects a sample the specification does not describe: an unknown variant, a
//! missing or extra field, a value of the wrong shape. Each type also has to be reached by at
//! least one sample, so a kernel type added to the list below without a sample fails.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use regex::Regex;
use serde::Serialize;
use serde_json::Value as Json;
use strata_kernel::testkit::{ctx, field, notes_world, object, principal_id, role, tenant_id};
use strata_kernel::*;
use strata_spec_compiler::jsonc;

struct Types(BTreeMap<String, Json>);

impl Types {
    fn load() -> Types {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specifications/spec.jsonc");
        let text = fs::read_to_string(path).expect("read the specification");
        let spec = jsonc::parse(&text).expect("parse the specification");
        let types = spec["types"].as_array().expect("types").iter();
        Types(
            types
                .map(|t| (t["id"].as_str().unwrap().to_string(), t.clone()))
                .collect(),
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
                    if !Regex::new(pattern).unwrap().is_match(text) {
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
        encoding.get("tag").map_or(true, |t| t == "external"),
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
            .map_or(true, |f| f.as_array().unwrap().is_empty())
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
    ]
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
    ]
}

fn samples() -> Vec<(&'static str, Json)> {
    let mut s: Vec<(&'static str, Json)> = Vec::new();
    let world = notes_world();
    let alpha = tenant_id("alpha");
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
        },
        EventBody::RecordDeleted {
            before: rec.clone(),
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
    ] {
        s.push(("output", json(&o)));
    }
    for r in [
        ReadRequest::Named {
            query: QueryId::new("recent").unwrap(),
            params: [("slug".to_string(), Value::Str("a".into()))]
                .into_iter()
                .collect(),
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
fn the_kernel_types_are_the_serde_form_of_the_specification_types() {
    let types = Types::load();
    let mut used = BTreeSet::new();
    let mut failures = Vec::new();
    for (ty, value) in samples() {
        if let Err(e) = check(&types, &mut used, ty, &value) {
            failures.push(format!("{ty}: {e}\n    sample: {value}"));
        }
    }
    assert!(
        failures.is_empty(),
        "samples the specification does not describe:\n{}",
        failures.join("\n")
    );

    // Every type of the list must have been reached, directly or through another type.
    let expected = [
        "value",
        "field-type",
        "expr",
        "intent",
        "schema-change",
        "delta-op",
        "state-delta",
        "event-body",
        "domain-event",
        "effect",
        "output",
        "transition-result",
        "check",
        "error-kind",
        "domain-error",
        "need",
        "coverage",
        "principal",
        "record",
        "tenant-state",
        "state",
        "tenant-schema",
        "object-type-def",
        "field-def",
        "policy-def",
        "constraint-def",
        "workflow-def",
        "role-def",
        "query-def",
        "view-def",
        "read-request",
        "read-result",
        "context",
    ];
    let missed: Vec<_> = expected.iter().filter(|t| !used.contains(**t)).collect();
    assert!(missed.is_empty(), "no sample reaches {missed:?}");
}

#[test]
fn the_checker_refuses_what_the_specification_does_not_describe() {
    let types = Types::load();
    let mut used = BTreeSet::new();
    let bad = [
        ("value", serde_json::json!({"Float": 1.5})),
        ("value", serde_json::json!("Int")),
        (
            "intent",
            serde_json::json!({"DeleteRecord": {"object": "note", "id": 1}}),
        ),
        ("output", serde_json::json!({"Tenant": {}})),
        ("record", serde_json::json!({"id": 1})),
        ("field-id", serde_json::json!("Title")),
    ];
    for (ty, value) in bad {
        assert!(
            check(&types, &mut used, ty, &value).is_err(),
            "{ty} accepted {value}"
        );
    }
}
