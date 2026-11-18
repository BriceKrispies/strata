//! The bridge to the Lean project: correspondence fixtures, the model executable, the axiom
//! audit, and the report that puts them together.
//!
//! # Correspondence
//!
//! The Lean model and the Rust kernel are different programs. To check that they agree where
//! they are meant to, both are run on the same generated scenarios and must print the same
//! text: one line per operation (`ok` or `err <kind>`) and the final state of both tenants.
//!
//! The mapping, which is also `docs/verification.md`:
//!
//! | Lean model | Rust kernel in these fixtures |
//! |---|---|
//! | tenant `n` | tenant `t<n>` with one object type `item` |
//! | fields 1, 2, 3 | `f1: string, required`, `f2: int`, `f3: bool` |
//! | owner, editors, writers | the `owner` role; an `editor` role allowed everything on `item`; a `writer` role allowed to create, and to update or delete where `record.$created_by == principal` |
//! | `create fs` | `CreateRecord` |
//! | `update id fs` | `UpdateRecord` naming all three fields, absent ones as `null` |
//! | `delete id` | `DeleteRecord` |
//! | `setRequired f strict` | `ChangeSchema [DefineField f required]` with `Reject` or `Grandfather` |
//! | `Err.invalid` | `TypeMismatch`, `MissingRequired`, `UnknownField` |
//!
//! Agreement on these scenarios is evidence of correspondence on the modelled fragment. It is
//! not a proof, and it says nothing about what the model leaves out: constraints, workflows,
//! references, uniqueness, queries, events.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use strata_kernel::*;

/// A linear congruential generator: enough to make varied scenarios, and trivially repeatable.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

fn value_token(rng: &mut Lcg, field: u64) -> String {
    // Mostly the right type for the field, sometimes the wrong one, sometimes absent.
    let kind = match (rng.below(10), field) {
        (0, _) => 3,
        (1, _) => rng.below(3),
        (_, 1) => 0,
        (_, 2) => 1,
        _ => 2,
    };
    match kind {
        0 => format!("s:{}", rng.pick(&["oak", "ash", "elm", "fir", "yew"])),
        1 => format!("i:{}", rng.below(40) as i64 - 5),
        2 => format!("b:{}", rng.below(2)),
        _ => "n".to_string(),
    }
}

fn field_tokens(rng: &mut Lcg) -> String {
    let mut tokens = Vec::new();
    for field in [1u64, 2, 3] {
        // Field 1 is required, so it is usually given.
        let include = if field == 1 { rng.below(8) != 0 } else { rng.below(2) == 0 };
        if include {
            tokens.push(format!("{field}={}", value_token(rng, field)));
        }
    }
    if rng.below(25) == 0 {
        tokens.push("9=i:1".to_string());
    }
    tokens.join(" ")
}

/// The scenario file both sides run. Deterministic in `seed`.
pub fn scenarios(seed: u64, count: usize) -> String {
    let mut rng = Lcg(seed);
    let mut out = String::new();
    for n in 0..count {
        out.push_str(&format!("scenario s{n}\n"));
        for _ in 0..(10 + rng.below(16)) {
            let tenant = match rng.below(12) {
                0 => 3,
                1 | 2 => 2,
                _ => 1,
            };
            let home = if tenant == 2 { 200 } else { 100 };
            let principal = match rng.below(14) {
                0 => 999,
                1 => if home == 100 { 201 } else { 101 },
                2 | 3 => home,
                4 | 5 => home + 1,
                6..=9 => home + 2,
                _ => home + 3,
            };
            let op = match rng.below(20) {
                0..=7 => format!("create {}", field_tokens(&mut rng)),
                8..=13 => format!("update {} {}", 1 + rng.below(6), field_tokens(&mut rng)),
                14..=16 => format!("delete {}", 1 + rng.below(6)),
                17 => format!("require {} strict", 1 + rng.below(3)),
                _ => format!("require {} lax", 1 + rng.below(3)),
            };
            // Schema changes are mostly attempted by the owner, so that both strict outcomes occur.
            let principal = if op.starts_with("require") && rng.below(4) != 0 { home } else { principal };
            out.push_str(format!("op {tenant} {principal} {op}").trim_end());
            out.push('\n');
        }
        out.push_str("end\n");
    }
    out
}

fn id<T: TryFrom<String>>(text: String) -> T
where
    T::Error: std::fmt::Debug,
{
    T::try_from(text).expect("fixture identifiers are valid")
}

fn item() -> ObjectTypeId {
    id("item".to_string())
}

fn field_id(n: u64) -> FieldId {
    id(format!("f{n}"))
}

fn field_type(n: u64) -> FieldType {
    match n {
        1 => FieldType::Str,
        2 => FieldType::Int,
        _ => FieldType::Bool,
    }
}

fn field_def(n: u64, required: bool) -> FieldDef {
    FieldDef { id: field_id(n), label: format!("f{n}"), ty: field_type(n), required, unique: false, default: None }
}

fn principal(n: u64) -> Principal {
    Principal::User(id(format!("p{n}")))
}

/// The kernel world that corresponds to the model's `initialWorld`.
fn initial_world() -> World {
    let mut world = World::default();
    let mut at = 0;
    for n in [1u64, 2] {
        let tenant: TenantId = id(format!("t{n}"));
        let owner = n * 100;
        let mut run = |world: &World, who: Principal, intent: Intent| {
            at += 1;
            let ctx = Context { tenant: tenant.clone(), principal: who, session: None, now: Timestamp(at), correlation: "fixture".into() };
            world.step(&intent, &ctx).expect("the fixture world installs").0
        };
        world = run(&world, Principal::System, Intent::CreateTenant { name: format!("t{n}"), owner: id(format!("p{owner}")) });
        let role = |name: &str| RoleDef { id: id(name.to_string()), label: name.to_string() };
        let policy = |name: &str, role: &str, actions: &[Action], condition: Option<Expr>| {
            SchemaChange::DefinePolicy(PolicyDef {
                id: id(name.to_string()),
                role: id(role.to_string()),
                resource: Resource::Object(item()),
                actions: actions.iter().copied().collect(),
                condition,
            })
        };
        let own = Expr::Cmp(CmpOp::Eq, Box::new(Expr::Var(Scope::Record, "$created_by".into())), Box::new(Expr::Principal));
        let changes = vec![
            SchemaChange::DefineRole(role("editor")),
            SchemaChange::DefineRole(role("writer")),
            SchemaChange::DefineObject { id: item(), label: "Item".into() },
            SchemaChange::DefineField { object: item(), field: field_def(1, true) },
            SchemaChange::DefineField { object: item(), field: field_def(2, false) },
            SchemaChange::DefineField { object: item(), field: field_def(3, false) },
            policy("editors_all", "editor", &[Action::Read, Action::Create, Action::Update, Action::Delete], None),
            policy("writers_create", "writer", &[Action::Read, Action::Create], None),
            policy("writers_own", "writer", &[Action::Update, Action::Delete], Some(own)),
        ];
        world = run(&world, principal(owner), Intent::ChangeSchema { changes, expected_version: None, existing: ExistingData::Reject });
        for (member, role_name) in [(owner + 1, "editor"), (owner + 2, "writer"), (owner + 3, "writer")] {
            world = run(&world, principal(owner), Intent::AssignRole { principal: id(format!("p{member}")), role: id(role_name.to_string()) });
        }
    }
    world
}

fn parse_value(token: &str) -> Value {
    match token.split_once(':') {
        Some(("i", n)) => Value::Int(n.parse().expect("fixture integer")),
        Some(("s", s)) => Value::Str(s.to_string()),
        Some(("b", b)) => Value::Bool(b == "1"),
        _ => Value::Null,
    }
}

fn parse_fields(tokens: &[&str]) -> BTreeMap<FieldId, Value> {
    tokens
        .iter()
        .filter_map(|token| token.split_once('='))
        .map(|(field, value)| (field_id(field.parse().expect("fixture field")), parse_value(value)))
        .collect()
}

fn show_value(value: &Value) -> String {
    match value {
        Value::Int(i) => format!("i:{i}"),
        Value::Str(s) => format!("s:{s}"),
        Value::Bool(b) => format!("b:{}", u8::from(*b)),
        _ => "n".to_string(),
    }
}

fn show_error(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::TenantNotFound => "no_tenant",
        ErrorKind::Unauthorized => "unauthorized",
        ErrorKind::UnknownRecord => "not_found",
        ErrorKind::TypeMismatch | ErrorKind::MissingRequired | ErrorKind::UnknownField => "invalid",
        ErrorKind::MigrationRejected => "migration_rejected",
        _ => "unmodelled",
    }
}

fn show_world(world: &World) -> Vec<String> {
    let mut out = Vec::new();
    for n in [1u64, 2] {
        let Some(tenant) = world.tenants.get(&id::<TenantId>(format!("t{n}"))) else {
            out.push(format!("tenant {n} absent"));
            continue;
        };
        let required: Vec<String> = tenant.schema.objects[&item()]
            .fields
            .values()
            .filter(|f| f.required)
            .map(|f| f.id.as_str()[1..].to_string())
            .collect();
        out.push(format!("tenant {n} next={} required={}", tenant.next_record, required.join(",")));
        for record in tenant.records_of(&item()) {
            let owner = record.created_by.as_ref().map_or("0", |p| &p.as_str()[1..]).to_string();
            let fields: Vec<String> =
                record.fields.iter().map(|(field, value)| format!("{}={}", &field.as_str()[1..], show_value(value))).collect();
            out.push(format!("record {} owner={owner} {}", record.id, fields.join(" ")).trim_end().to_string());
        }
    }
    out
}

/// Run a scenario file through the Rust kernel and print what the Lean program prints.
pub fn kernel_output(scenarios: &str) -> String {
    let initial = initial_world();
    let mut world = initial.clone();
    let mut out = Vec::new();
    let mut at = 1_000;
    for line in scenarios.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        match tokens.as_slice() {
            [] => {}
            ["scenario", name @ ..] => {
                world = initial.clone();
                out.push(format!("scenario {}", name.join(" ")));
            }
            ["end"] => {
                out.push("state".to_string());
                out.extend(show_world(&world));
                out.push("end".to_string());
                world = initial.clone();
            }
            ["op", tenant, who, rest @ ..] => {
                at += 1;
                let ctx = Context {
                    tenant: id(format!("t{tenant}")),
                    principal: principal(who.parse().expect("fixture principal")),
                    session: None,
                    now: Timestamp(at),
                    correlation: "fixture".into(),
                };
                let intent = match rest {
                    ["create", fields @ ..] => Intent::CreateRecord { object: item(), fields: parse_fields(fields) },
                    ["update", record, fields @ ..] => {
                        // The model replaces a record; a patch that names every field does too.
                        let mut all: BTreeMap<FieldId, Value> = (1..=3).map(|n| (field_id(n), Value::Null)).collect();
                        all.extend(parse_fields(fields));
                        Intent::UpdateRecord { object: item(), id: RecordId(record.parse().expect("id")), fields: all, expected_version: None }
                    }
                    ["delete", record] => {
                        Intent::DeleteRecord { object: item(), id: RecordId(record.parse().expect("id")), expected_version: None }
                    }
                    ["require", field, mode] => {
                        let n: u64 = field.parse().expect("field");
                        Intent::ChangeSchema {
                            changes: vec![SchemaChange::DefineField { object: item(), field: field_def(n, true) }],
                            expected_version: None,
                            existing: if *mode == "strict" { ExistingData::Reject } else { ExistingData::Grandfather },
                        }
                    }
                    _ => {
                        out.push(format!("unparsed {line}"));
                        continue;
                    }
                };
                match world.step(&intent, &ctx) {
                    Ok((next, _)) => {
                        world = next;
                        out.push("ok".to_string());
                    }
                    Err(error) => out.push(format!("err {}", show_error(error.kind))),
                }
            }
            _ => out.push(format!("unparsed {line}")),
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

fn lake(lean_dir: &Path, args: &[&str]) -> Result<(bool, String), String> {
    let output = Command::new("lake")
        .args(args)
        .current_dir(lean_dir)
        .output()
        .map_err(|e| format!("could not run lake (is Lean installed and on PATH?): {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    Ok((output.status.success(), text.replace("\r\n", "\n")))
}

/// Run the Lean model on a scenario file inside `lean_dir` and return what it prints.
pub fn model_output(lean_dir: &Path, relative_fixture: &str) -> Result<String, String> {
    let (ok, text) = lake(lean_dir, &["exe", "strata_model", relative_fixture])?;
    if ok {
        Ok(text)
    } else {
        Err(format!("the Lean model failed to run:\n{text}"))
    }
}

/// One theorem as the audit found it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    pub theorem: String,
    pub axioms: Vec<String>,
    /// Checked by Lean with no `sorry` and no axiom beyond Lean's standard three.
    pub clean: bool,
}

const STANDARD_AXIOMS: [&str; 3] = ["propext", "Classical.choice", "Quot.sound"];

/// Parse the output of `lake env lean Audit.lean`.
pub fn parse_audit(output: &str) -> Vec<Checked> {
    output
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix('\'')?;
            let (name, tail) = rest.split_once('\'')?;
            let axioms: Vec<String> = match tail.split_once('[') {
                Some((_, list)) => list.trim_end_matches(']').split(',').map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect(),
                None => Vec::new(),
            };
            let clean = axioms.iter().all(|axiom| STANDARD_AXIOMS.contains(&axiom.as_str()));
            Some(Checked { theorem: name.to_string(), axioms, clean })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Correspondence {
    pub scenarios: usize,
    pub operations: usize,
    pub agree: bool,
    /// The first line on which the two outputs differ: `(line, lean, rust)`.
    pub first_difference: Option<(usize, String, String)>,
}

/// Compare two outputs line by line.
pub fn compare_outputs(scenarios: &str, lean: &str, rust: &str) -> Correspondence {
    let (a, b): (Vec<&str>, Vec<&str>) = (lean.lines().collect(), rust.lines().collect());
    let first_difference = (0..a.len().max(b.len()))
        .find(|i| a.get(*i) != b.get(*i))
        .map(|i| (i + 1, a.get(i).unwrap_or(&"<missing>").to_string(), b.get(i).unwrap_or(&"<missing>").to_string()));
    Correspondence {
        scenarios: scenarios.lines().filter(|l| l.starts_with("scenario")).count(),
        operations: scenarios.lines().filter(|l| l.starts_with("op ")).count(),
        agree: first_difference.is_none(),
        first_difference,
    }
}

/// The verification report: what Lean checked, with which axioms, and whether the model and the
/// kernel agree on the fixtures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub checked_at: i64,
    pub lean_version: String,
    pub build_ok: bool,
    pub build_log_tail: String,
    pub theorems: Vec<Checked>,
    /// Theorems the obligations table names that the audit did not find.
    pub missing: Vec<String>,
    pub correspondence: Option<Correspondence>,
    pub obligations: Vec<crate::obligations::Obligation>,
    pub assumptions: Vec<String>,
    /// Everything built, every obligation's theorem is present and clean, and the fixtures agree.
    pub all_checked: bool,
    pub problem: Option<String>,
}

/// The seed and size of the committed fixture file `lean/fixtures/scenarios.txt`.
pub const FIXTURE_SEED: u64 = 20_261_118;
pub const FIXTURE_COUNT: usize = 60;

/// Build the Lean project, audit its theorems and run the correspondence fixtures.
///
/// Slow (it runs the Lean toolchain) and never on a request path. `checked_at` is supplied by
/// the caller; this crate does not read the clock.
pub fn verify(lean_dir: &Path, checked_at: i64) -> Report {
    let obligations = crate::obligations::all();
    let assumptions = crate::obligations::assumptions().into_iter().map(str::to_string).collect();
    let mut report = Report {
        checked_at,
        lean_version: String::new(),
        build_ok: false,
        build_log_tail: String::new(),
        theorems: Vec::new(),
        missing: Vec::new(),
        correspondence: None,
        obligations,
        assumptions,
        all_checked: false,
        problem: None,
    };
    let run = (|| -> Result<(), String> {
        let (_, version) = lake(lean_dir, &["env", "lean", "--version"])?;
        report.lean_version = version.lines().next().unwrap_or("").trim().to_string();
        let (built, log) = lake(lean_dir, &["build"])?;
        report.build_ok = built;
        let lines: Vec<&str> = log.lines().collect();
        report.build_log_tail = lines[lines.len().saturating_sub(12)..].join("\n");
        if !built {
            return Err("the Lean project does not build".into());
        }
        let (audited, audit) = lake(lean_dir, &["env", "lean", "Audit.lean"])?;
        if !audited {
            return Err(format!("the axiom audit failed:\n{audit}"));
        }
        report.theorems = parse_audit(&audit);
        let found: BTreeSet<&str> = report.theorems.iter().map(|t| t.theorem.as_str()).collect();
        report.missing = report
            .obligations
            .iter()
            .flat_map(|o| o.theorems.iter())
            .filter(|name| !found.contains(name.as_str()))
            .cloned()
            .collect();
        let text = scenarios(FIXTURE_SEED, FIXTURE_COUNT);
        let lean = model_output(lean_dir, "fixtures/scenarios.txt")?;
        report.correspondence = Some(compare_outputs(&text, &lean, &kernel_output(&text)));
        Ok(())
    })();
    report.problem = run.err();
    report.all_checked = report.problem.is_none()
        && report.build_ok
        && report.missing.is_empty()
        && !report.theorems.is_empty()
        && report.theorems.iter().all(|t| t.clean)
        && report.correspondence.as_ref().is_some_and(|c| c.agree);
    report
}
