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
//! | tenant `n` | tenant `t<n>` with two object types: `item` (the model's type `1`, the default) and `part` (type `2`) |
//! | fields 0 to 8, 10, 11, 12 of type 1 | `f0: list of ref item, restrict`, `f1: string, required`, `f2: int`, `f3: bool`, `f4: ref item, set-null`, `f5: ref item, restrict`, `f6: int, default 7`, `f7: list of string`, `f8: list of ref item, set-null`, `f10: string, unique`, `f11: ref part, restrict`, `f12: ref part, set-null` |
//! | fields 1 to 4 of type 2 | `f1: ref item, restrict`, `f2: ref item, set-null`, `f3: list of ref item, set-null`, `f4: int` |
//! | `type <n>` before the intent | the object type of the intent; any number but `1` and `2` names a type the tenant does not have |
//! | owner, editors, writers | the `owner` role; an `editor` role allowed everything on `item`; a `writer` role allowed to create, and to update or delete where `record.$created_by == principal` |
//! | `create fs` | `CreateRecord` |
//! | `update id [v=n] fs` | `UpdateRecord` naming fields `f0` to `f8` and `f10`, absent ones as `null` |
//! | `patch id [v=n] fs` | `UpdateRecord` naming only the fields given, `n` removing one |
//! | `delete id [v=n]` | `DeleteRecord` |
//! | `setRequired f strict` | `ChangeSchema [DefineField f required]` with `Reject` or `Grandfather` |
//! | `setUnique f strict` | `ChangeSchema [DefineField f unique]`, the field's other attributes kept, with `Reject` or `Grandfather` |
//! | `defineUnique fs` | `ChangeSchema [DefineUnique]`, a composite constraint named `u<fields>` |
//! | `Err.danglingReference`, `Err.stillReferenced` | `DanglingReference`, `StillReferenced` |
//! | `Err.invalid` | `TypeMismatch`, `MissingRequired`, `UnknownField` |
//! | `Err.uniqueViolated`, `Err.invalidSchema` | `UniqueViolated`, `InvalidSchema` |
//! | `Err.unknownObject` | `UnknownObject` (the owner's refusal of a type the tenant does not have) |
//! | `Err.versionConflict`, `Err.limitExceeded` | `VersionConflict`, `LimitExceeded` |
//!
//! # The Main.lean protocol
//!
//! `lean/Main.lean` takes one argument, the path of a scenario file, and prints one result text
//! for all of it. A file holds any number of scenarios, so one process runs a whole batch:
//!
//! ```text
//! scenario <name>              starts a scenario from the initial world
//! op <tenant> <principal> [type <n>] <intent> [args]
//!                              one intent, as `create f=v ..`, `update <id> [v=n] f=v ..`,
//!                              `patch <id> [v=n] f=v ..`, `delete <id> [v=n]`,
//!                              `require <field> strict|lax`
//!                              `unique <field> strict|lax`, `composite <field> ..`
//! end                          prints the final state and returns to the initial world
//! ```
//!
//! A value is `n`, `i:<int>`, `s:<text>`, `s*<count>:<text>` (the text repeated), `b:1`, `b:0`,
//! `r:<id>`, or a list: `l:<element>,<element>,..` (`l:` is empty) or `l*<count>:<element>`.
//!
//! Output: `scenario <name>`, then `ok` or `err <kind>` per op (`unparsed <line>` for a line it
//! cannot read), then `state`, the state lines and `end`. A new intent is a new `op` arm in
//! `parseOp`; `crate::lean_diff` generates every such intent (and a test fails if one is missing).
//! The differential harness is `crate::lean_diff`.
//!
//! Agreement on these scenarios is evidence of correspondence on the modelled fragment. It is
//! not a proof, and it says nothing about what the model leaves out: workflows, queries, events,
//! roles and policies, the clock, and the expression functions beyond the slice 5 fragment.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use strata_kernel::*;

/// A linear congruential generator: enough to make varied scenarios, and trivially repeatable.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
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
        let include = if field == 1 {
            rng.below(8) != 0
        } else {
            rng.below(2) == 0
        };
        if include {
            tokens.push(format!("{field}={}", value_token(rng, field)));
        }
    }
    if rng.below(25) == 0 {
        tokens.push("9=s:oak".to_string());
    }
    tokens.join(" ")
}

/// One scenario, named `s<n>`, written from the generator's random stream.
fn write_scenario(rng: &mut Lcg, n: usize, out: &mut String) {
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
            1 => {
                if home == 100 {
                    201
                } else {
                    101
                }
            }
            2 | 3 => home,
            4 | 5 => home + 1,
            6..=9 => home + 2,
            _ => home + 3,
        };
        let op = match rng.below(20) {
            0..=7 => format!("create {}", field_tokens(rng)),
            8..=13 => format!("update {} {}", 1 + rng.below(6), field_tokens(rng)),
            14..=16 => format!("delete {}", 1 + rng.below(6)),
            17 => format!("require {} strict", 1 + rng.below(3)),
            _ => format!("require {} lax", 1 + rng.below(3)),
        };
        // Schema changes are mostly attempted by the owner, so that both strict outcomes occur.
        let principal = if op.starts_with("require") && rng.below(4) != 0 {
            home
        } else {
            principal
        };
        out.push_str(format!("op {tenant} {principal} {op}").trim_end());
        out.push('\n');
    }
    out.push_str("end\n");
}

/// The generator behind [`scenarios`], read a batch at a time. Each batch continues the random
/// stream and numbers its scenarios on from the last, so the batches of one stream, concatenated,
/// are exactly what [`scenarios`] writes for the same seed and total count.
pub struct ScenarioStream {
    rng: Lcg,
    next: usize,
    seed: u64,
    generator: Generator,
}

/// Which generator a [`ScenarioStream`] draws from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generator {
    /// The generator of the committed fixtures ([`scenarios`]). It covers the create, full
    /// replacement, delete and schema-change intents, the first slice of the model only.
    Fixture,
    /// The generator of `lean_diff`: every intent of `Main.lean` (patch and expected versions
    /// included), references, lists, and the string, list and record bounds, one below, at and
    /// above. Scenario `n` is `lean_diff::generate` of the case seed `n` of the base seed.
    Full,
}

impl ScenarioStream {
    /// The stream [`scenarios`] draws from.
    pub fn new(seed: u64) -> Self {
        Self::with_generator(seed, Generator::Fixture)
    }

    /// A stream of the full generator (see [`Generator::Full`]).
    pub fn full(seed: u64) -> Self {
        Self::with_generator(seed, Generator::Full)
    }

    fn with_generator(seed: u64, generator: Generator) -> Self {
        Self {
            rng: Lcg(seed),
            next: 0,
            seed,
            generator,
        }
    }

    /// The next `count` scenarios, as a scenario file.
    pub fn batch(&mut self, count: usize) -> String {
        let mut out = String::new();
        for n in self.next..self.next + count {
            match self.generator {
                Generator::Fixture => write_scenario(&mut self.rng, n, &mut out),
                Generator::Full => write_full_scenario(self.seed, n, &mut out),
            }
        }
        self.next += count;
        out
    }
}

/// One scenario of the full generator, named `f<n>`.
fn write_full_scenario(seed: u64, n: usize, out: &mut String) {
    out.push_str(&format!("scenario f{n}\n"));
    for op in crate::lean_diff::generate(crate::lean_diff::case_seed(seed, n)) {
        out.push_str(&op.render());
        out.push('\n');
    }
    out.push_str("end\n");
}

/// The scenario file both sides run. Deterministic in `seed`.
pub fn scenarios(seed: u64, count: usize) -> String {
    ScenarioStream::new(seed).batch(count)
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

/// The second object type of the fixture (the model's type `2`). Its references point into items
/// and back.
fn part() -> ObjectTypeId {
    id("part".to_string())
}

/// The object type an intent names: `1` is `item`, `2` is `part`, and any other number is a type
/// the tenant does not have.
fn object_of(ty: u64) -> ObjectTypeId {
    match ty {
        1 => item(),
        2 => part(),
        n => id(format!("type{n}")),
    }
}

/// The fields an `update` line names for an object type, as the model's `fieldsOf` lists them.
fn fixture_fields(object: &ObjectTypeId) -> Vec<u64> {
    if *object == item() {
        vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
    } else if *object == part() {
        vec![1, 2, 3, 4, 5]
    } else {
        Vec::new()
    }
}

/// Field `n` of the fixture. The kernel checks a record's fields in the order of their ids, which
/// are strings; the model checks them by number. Two-digit ids (`f04`, `f10`) make the two orders
/// the same, so the fixture does not invent a check-order difference the kernel does not have.
fn field_id(n: u64) -> FieldId {
    id(format!("f{n:02}"))
}

/// The number of a fixture field (`f10` is 10).
fn field_number(field: &FieldId) -> u64 {
    field.as_str()[1..]
        .parse()
        .expect("fixture fields are f<number>")
}

fn field_type(n: u64) -> FieldType {
    let reference = |on_delete| FieldType::Ref {
        target: item(),
        on_delete,
    };
    let part_reference = |on_delete| FieldType::Ref {
        target: part(),
        on_delete,
    };
    match n {
        // Fields 11 and 12 of an item refer to a part; 11 restricts the part's delete, 12 clears.
        11 => part_reference(OnDelete::Restrict),
        12 => part_reference(OnDelete::SetNull),
        0 => FieldType::List(Box::new(reference(OnDelete::Restrict))),
        1 => FieldType::Str,
        2 | 6 => FieldType::Int,
        3 => FieldType::Bool,
        // Field 4 clears itself when its target is deleted; field 5 refuses the delete.
        4 => reference(OnDelete::SetNull),
        5 => reference(OnDelete::Restrict),
        7 => FieldType::List(Box::new(FieldType::Str)),
        // Field 8 loses an element when that element's record is deleted.
        8 => FieldType::List(Box::new(reference(OnDelete::SetNull))),
        // Field 9 is the caller's principal, as a string (the kernel names a principal `p<n>`).
        9 => FieldType::Str,
        10 => FieldType::Str,
        _ => FieldType::Bool,
    }
}

/// The default of a field of an item, as the model's `Main.lean` gives it: field `6` the constant
/// `7`, field `9` the principal (an expression).
fn item_default(n: u64) -> Option<Expr> {
    match n {
        6 => Some(Expr::Lit(Value::Int(7))),
        9 => Some(Expr::Principal),
        _ => None,
    }
}

/// The constraints of the item type, in id order (the model's `itemConstraints`): field `2` is absent
/// or not negative, and field `1` is not the name `forbidden`.
fn item_constraints() -> Vec<SchemaChange> {
    let field = |n: u64| Box::new(Expr::Var(Scope::Record, format!("f{n:02}")));
    let absent_or_not_negative = Expr::Or(vec![
        Expr::IsNull(field(2)),
        Expr::Cmp(CmpOp::Ge, field(2), Box::new(Expr::Lit(Value::Int(0)))),
    ]);
    let not_forbidden = Expr::Cmp(
        CmpOp::Ne,
        field(1),
        Box::new(Expr::Lit(Value::Str("forbidden".into()))),
    );
    [
        ("c1", absent_or_not_negative, "a value is negative"),
        ("c2", not_forbidden, "the name is forbidden"),
    ]
    .into_iter()
    .map(|(name, expr, message)| SchemaChange::DefineConstraint {
        object: item(),
        constraint: ConstraintDef {
            id: id(name.to_string()),
            expr,
            message: message.to_string(),
        },
    })
    .collect()
}

fn field_def(n: u64, required: bool) -> FieldDef {
    FieldDef {
        id: field_id(n),
        label: format!("f{n}"),
        ty: field_type(n),
        required,
        // Field 10 is unique: no two records hold the same value in it.
        unique: n == 10,
        default: item_default(n),
        read: None,
        personal: None,
        normalise: None,
        computed: None,
    }
}

/// Field `n` of a part: `1` an item reference that restricts the delete, `2` an item reference
/// that clears (set-null), `3` a list of item references that clear, `4` an integer.
fn part_field_type(n: u64) -> FieldType {
    let item_reference = |on_delete| FieldType::Ref {
        target: item(),
        on_delete,
    };
    match n {
        1 => item_reference(OnDelete::Restrict),
        2 => item_reference(OnDelete::SetNull),
        3 => FieldType::List(Box::new(item_reference(OnDelete::SetNull))),
        5 => FieldType::Bool,
        _ => FieldType::Int,
    }
}

/// The default of a part's field: field `5` is whether the caller is `p100` (the model's
/// `partSchema`).
fn part_default(n: u64) -> Option<Expr> {
    (n == 5).then(|| {
        Expr::Cmp(
            CmpOp::Eq,
            Box::new(Expr::Principal),
            Box::new(Expr::Lit(Value::Str("p100".into()))),
        )
    })
}

fn part_field_def(n: u64) -> FieldDef {
    FieldDef {
        id: field_id(n),
        label: format!("f{n}"),
        ty: part_field_type(n),
        required: false,
        unique: false,
        default: part_default(n),
        read: None,
        personal: None,
        normalise: None,
        computed: None,
    }
}

/// The definition a `require` or `unique` line falls back to when the tenant does not have the
/// field yet, as `field_def` and `part_field_def` define the fixture's fields.
fn fallback_field(object: &ObjectTypeId, n: u64) -> FieldDef {
    if *object == part() {
        part_field_def(n)
    } else {
        field_def(n, false)
    }
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
            let ctx = Context {
                tenant: tenant.clone(),
                principal: who,
                session: None,
                now: Timestamp(at),
                correlation: "fixture".into(),
                verified_address: None,
                idempotency_key: None,
                language: None,
                hop: None,
                agent: None,
            };
            world
                .step(&intent, &ctx)
                .expect("the fixture world installs")
                .0
        };
        world = run(
            &world,
            Principal::System,
            Intent::CreateTenant {
                name: format!("t{n}"),
                owner: id(format!("p{owner}")),
            },
        );
        let role = |name: &str| RoleDef {
            id: id(name.to_string()),
            label: name.to_string(),
        };
        let policy = |object: ObjectTypeId,
                      name: &str,
                      role: &str,
                      actions: &[Action],
                      condition: Option<Expr>| {
            SchemaChange::DefinePolicy(PolicyDef {
                id: id(name.to_string()),
                role: id(role.to_string()),
                resource: Resource::Object(object),
                actions: actions.iter().copied().collect(),
                condition,
            })
        };
        let own = Expr::Cmp(
            CmpOp::Eq,
            Box::new(Expr::Var(Scope::Record, "$created_by".into())),
            Box::new(Expr::Principal),
        );
        let mut changes = vec![
            SchemaChange::DefineRole(role("editor")),
            SchemaChange::DefineRole(role("writer")),
            SchemaChange::DefineObject {
                id: item(),
                label: "Item".into(),
            },
            SchemaChange::DefineField {
                object: item(),
                field: field_def(1, true),
            },
        ];
        changes.extend(
            [0u64, 2, 3, 4, 5, 6, 7, 8, 9, 10].map(|n| SchemaChange::DefineField {
                object: item(),
                field: field_def(n, false),
            }),
        );
        // The part type comes after the item type, since its references point into items.
        changes.push(SchemaChange::DefineObject {
            id: part(),
            label: "Part".into(),
        });
        changes.extend([1u64, 2, 3, 4, 5].map(|n| SchemaChange::DefineField {
            object: part(),
            field: part_field_def(n),
        }));
        // The item's constraints read its fields, so they follow the fields of both types.
        changes.extend(item_constraints());
        changes.extend([
            policy(
                item(),
                "editors_all",
                "editor",
                &[Action::Read, Action::Create, Action::Update, Action::Delete],
                None,
            ),
            policy(
                item(),
                "writers_create",
                "writer",
                &[Action::Read, Action::Create],
                None,
            ),
            policy(
                item(),
                "writers_own",
                "writer",
                &[Action::Update, Action::Delete],
                Some(own.clone()),
            ),
            policy(
                part(),
                "part_editors_all",
                "editor",
                &[Action::Read, Action::Create, Action::Update, Action::Delete],
                None,
            ),
            policy(
                part(),
                "part_writers_create",
                "writer",
                &[Action::Read, Action::Create],
                None,
            ),
            policy(
                part(),
                "part_writers_own",
                "writer",
                &[Action::Update, Action::Delete],
                Some(own),
            ),
        ]);
        world = run(
            &world,
            principal(owner),
            Intent::ChangeSchema {
                changes,
                expected_version: None,
                existing: ExistingData::Reject,
            },
        );
        // Fields 11 and 12 of an item refer to parts, so they follow the part type.
        world = run(
            &world,
            principal(owner),
            Intent::ChangeSchema {
                changes: [11u64, 12]
                    .map(|n| SchemaChange::DefineField {
                        object: item(),
                        field: field_def(n, false),
                    })
                    .to_vec(),
                expected_version: None,
                existing: ExistingData::Reject,
            },
        );
        for (member, role_name) in [
            (owner + 1, "editor"),
            (owner + 2, "writer"),
            (owner + 3, "writer"),
        ] {
            world = run(
                &world,
                principal(owner),
                Intent::AssignRole {
                    principal: id(format!("p{member}")),
                    role: id(role_name.to_string()),
                },
            );
        }
    }
    world
}

/// A scalar, reference or `n`: the element of a list, or a whole value. Garbage reads as `null`,
/// which the generator never writes.
fn parse_elem(token: &str) -> Value {
    if let Some(rest) = token.strip_prefix("s*") {
        if let Some((count, text)) = rest.split_once(':') {
            return Value::Str(text.repeat(count.parse().expect("fixture count")));
        }
    }
    match token.split_once(':') {
        Some(("i", n)) => Value::Int(n.parse().expect("fixture integer")),
        Some(("s", s)) => Value::Str(s.to_string()),
        Some(("b", b)) => Value::Bool(b == "1"),
        Some(("r", r)) => Value::Ref(RecordId(r.parse().expect("fixture record id"))),
        _ => Value::Null,
    }
}

/// A value: an element, or a list: `l:` and elements separated by commas (`l:` is the empty
/// list), or `l*<count>:<element>`.
fn parse_value(token: &str) -> Value {
    if let Some(rest) = token.strip_prefix("l*") {
        if let Some((count, element)) = rest.split_once(':') {
            let n: usize = count.parse().expect("fixture count");
            return Value::List(vec![parse_elem(element); n]);
        }
    }
    if let Some(body) = token.strip_prefix("l:") {
        return Value::List(if body.is_empty() {
            Vec::new()
        } else {
            body.split(',').map(parse_elem).collect()
        });
    }
    parse_elem(token)
}

fn parse_fields(tokens: &[&str]) -> BTreeMap<FieldId, Value> {
    tokens
        .iter()
        .filter_map(|token| token.split_once('='))
        .map(|(field, value)| {
            (
                field_id(field.parse().expect("fixture field")),
                parse_value(value),
            )
        })
        .collect()
}

/// The optional `v=<n>` at the head of the tokens after a record id, and the tokens after it.
fn split_expected_version<'a, 'b>(tokens: &'b [&'a str]) -> (Option<u64>, &'b [&'a str]) {
    match tokens.split_first() {
        Some((first, rest)) => match first.strip_prefix("v=") {
            Some(n) => (Some(n.parse().expect("fixture version")), rest),
            None => (None, tokens),
        },
        None => (None, tokens),
    }
}

/// A string of at least 100 characters that repeats one character prints as `s*<count>:<c>`.
fn show_str(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) if s.chars().count() >= 100 && chars.all(|c| c == first) => {
            format!("s*{}:{first}", s.chars().count())
        }
        _ => format!("s:{s}"),
    }
}

fn show_value(value: &Value) -> String {
    match value {
        Value::Int(i) => format!("i:{i}"),
        Value::Str(s) => show_str(s),
        Value::Bool(b) => format!("b:{}", u8::from(*b)),
        Value::Ref(r) => format!("r:{}", r.0),
        // At least 100 equal elements print as `l*<count>:<element>`.
        Value::List(items) => match items.split_first() {
            Some((first, rest)) if items.len() >= 100 && rest.iter().all(|v| v == first) => {
                format!("l*{}:{}", items.len(), show_value(first))
            }
            _ => format!(
                "l:{}",
                items.iter().map(show_value).collect::<Vec<_>>().join(",")
            ),
        },
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
        ErrorKind::DanglingReference => "dangling_reference",
        ErrorKind::StillReferenced => "still_referenced",
        ErrorKind::VersionConflict => "version_conflict",
        ErrorKind::LimitExceeded => "limit_exceeded",
        ErrorKind::UniqueViolated => "unique_violated",
        ErrorKind::InvalidSchema => "invalid_schema",
        ErrorKind::UnknownObject => "unknown_object",
        ErrorKind::ConstraintViolated => "constraint_violated",
        ErrorKind::Evaluation => "evaluation",
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
        // The required fields of an object type, by number, as the model's `requiredOf` prints
        // them. Fields print in the model's order, by number (`f10` after `f6`), not by name.
        let required_text = |object: &ObjectTypeId| -> String {
            let mut required: Vec<u64> = tenant.schema.objects[object]
                .fields
                .values()
                .filter(|f| f.required)
                .map(|f| field_number(&f.id))
                .collect();
            required.sort_unstable();
            let required: Vec<String> = required.iter().map(u64::to_string).collect();
            required.join(",")
        };
        out.push(format!(
            "tenant {n} next={} required={}",
            tenant.next_record,
            required_text(&item())
        ));
        // The item's records, then the part type's (the model's type `2`), with its own header.
        for (object, pre) in [(item(), ""), (part(), "type 2 ")] {
            if !pre.is_empty() {
                out.push(format!("type 2 required={}", required_text(&object)));
            }
            for record in tenant.records_of(&object) {
                let owner = record
                    .created_by
                    .as_ref()
                    .map_or("0", |p| &p.as_str()[1..])
                    .to_string();
                let mut numbered: Vec<(u64, &Value)> = record
                    .fields
                    .iter()
                    .map(|(field, value)| (field_number(field), value))
                    .collect();
                numbered.sort_by_key(|(n, _)| *n);
                let fields: Vec<String> = numbered
                    .iter()
                    .map(|(n, value)| format!("{n}={}", show_value(value)))
                    .collect();
                out.push(
                    format!(
                        "{pre}record {} owner={owner} v={} {}",
                        record.id,
                        record.version,
                        fields.join(" ")
                    )
                    .trim_end()
                    .to_string(),
                );
            }
        }
    }
    out
}

/// The definition of field `n` as the tenant's schema holds it now. The kernel's `DefineField`
/// replaces the whole definition, so a change to one attribute starts from this one.
fn current_field(
    world: &World,
    tenant: &TenantId,
    object: &ObjectTypeId,
    n: u64,
) -> Option<FieldDef> {
    let state = world.tenants.get(tenant)?;
    state
        .schema
        .objects
        .get(object)?
        .fields
        .get(&field_id(n))
        .cloned()
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
            ["op", tenant, who, after_who @ ..] => {
                let (object, rest): (ObjectTypeId, &[&str]) = match after_who {
                    ["type", ty, tail @ ..] => (object_of(ty.parse().expect("object type")), tail),
                    _ => (item(), after_who),
                };
                at += 1;
                let ctx = Context {
                    tenant: id(format!("t{tenant}")),
                    principal: principal(who.parse().expect("fixture principal")),
                    session: None,
                    now: Timestamp(at),
                    correlation: "fixture".into(),
                    verified_address: None,
                    idempotency_key: None,
                    language: None,
                    hop: None,
                    agent: None,
                };
                let intent = match rest {
                    ["create", fields @ ..] => Intent::CreateRecord {
                        object: object.clone(),
                        fields: parse_fields(fields),
                    },
                    ["update", record, after_id @ ..] => {
                        // The model replaces a record; a patch that names every field does too.
                        let (expected_version, fields) = split_expected_version(after_id);
                        let mut all: BTreeMap<FieldId, Value> = fixture_fields(&object)
                            .into_iter()
                            .map(|n| (field_id(n), Value::Null))
                            .collect();
                        all.extend(parse_fields(fields));
                        Intent::UpdateRecord {
                            object: object.clone(),
                            id: RecordId(record.parse().expect("id")),
                            fields: all,
                            expected_version,
                        }
                    }
                    ["patch", record, after_id @ ..] => {
                        let (expected_version, fields) = split_expected_version(after_id);
                        Intent::UpdateRecord {
                            object: object.clone(),
                            id: RecordId(record.parse().expect("id")),
                            fields: parse_fields(fields),
                            expected_version,
                        }
                    }
                    ["delete", record, after_id @ ..] => {
                        let (expected_version, more) = split_expected_version(after_id);
                        if !more.is_empty() {
                            out.push(format!("unparsed {line}"));
                            continue;
                        }
                        Intent::DeleteRecord {
                            object: object.clone(),
                            id: RecordId(record.parse().expect("id")),
                            expected_version,
                        }
                    }
                    ["require", field, mode] => {
                        let n: u64 = field.parse().expect("field");
                        // A field the tenant does not have (or a tenant that does not exist) is
                        // defined afresh, as before uniqueness; the kernel decides what follows.
                        let mut def = current_field(&world, &ctx.tenant, &object, n)
                            .unwrap_or_else(|| fallback_field(&object, n));
                        def.required = true;
                        Intent::ChangeSchema {
                            changes: vec![SchemaChange::DefineField {
                                object: object.clone(),
                                field: def,
                            }],
                            expected_version: None,
                            existing: if *mode == "strict" {
                                ExistingData::Reject
                            } else {
                                ExistingData::Grandfather
                            },
                        }
                    }
                    ["unique", field, mode] => {
                        let n: u64 = field.parse().expect("field");
                        // A field the tenant does not have (or a tenant that does not exist) is
                        // defined afresh, as before uniqueness; the kernel decides what follows.
                        let mut def = current_field(&world, &ctx.tenant, &object, n)
                            .unwrap_or_else(|| fallback_field(&object, n));
                        def.unique = true;
                        Intent::ChangeSchema {
                            changes: vec![SchemaChange::DefineField {
                                object: object.clone(),
                                field: def,
                            }],
                            expected_version: None,
                            existing: if *mode == "strict" {
                                ExistingData::Reject
                            } else {
                                ExistingData::Grandfather
                            },
                        }
                    }
                    ["composite", fields @ ..] => {
                        let Some(ids) = fields
                            .iter()
                            .map(|f| f.parse::<u64>().ok().map(field_id))
                            .collect::<Option<Vec<FieldId>>>()
                        else {
                            out.push(format!("unparsed {line}"));
                            continue;
                        };
                        let unique = UniqueDef {
                            id: id(format!("u{}", fields.join("_"))),
                            fields: ids,
                            message: "unique".to_string(),
                        };
                        Intent::ChangeSchema {
                            changes: vec![SchemaChange::DefineUnique {
                                object: object.clone(),
                                unique,
                            }],
                            expected_version: None,
                            existing: ExistingData::Reject,
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
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok((output.status.success(), text.replace("\r\n", "\n")))
}

/// Build the Lean model executable on its own (see `model_output` for why).
pub fn build_model(lean_dir: &Path) -> Result<(), String> {
    let (built, log) = lake(lean_dir, &["build", "strata_model"])?;
    if built {
        Ok(())
    } else {
        Err(format!(
            "the Lean model failed to build:
{log}"
        ))
    }
}

/// Run the already built model on a scenario file (a path relative to `lean_dir`, or absolute)
/// and return what it prints.
pub fn run_model(lean_dir: &Path, fixture: &str) -> Result<String, String> {
    // Run the built executable itself, not `lake exe`: Lake replays the build's warnings onto
    // stdout before the program's output, and those lines would be compared as model output.
    let exe = lean_dir
        .join(".lake")
        .join("build")
        .join("bin")
        .join(format!("strata_model{}", std::env::consts::EXE_SUFFIX));
    let output = Command::new(&exe)
        .arg(fixture)
        .current_dir(lean_dir)
        .output()
        .map_err(|e| {
            format!(
                "could not run {} (build it with `lake build strata_model`): {e}",
                exe.display()
            )
        })?;
    let ok = output.status.success();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .replace("\r\n", "\n");
    if ok {
        Ok(text)
    } else {
        Err(format!(
            "the Lean model failed to run:
{text}"
        ))
    }
}

/// Run the Lean model on a scenario file inside `lean_dir` and return what it prints.
pub fn model_output(lean_dir: &Path, relative_fixture: &str) -> Result<String, String> {
    // Build first, on its own: on a cold build `lake exe` prints Lake's progress lines
    // ("Built Strata.Model") into the same output as the program, and those lines would
    // be compared with the kernel's output as if the model had written them.
    build_model(lean_dir)?;
    run_model(lean_dir, relative_fixture)
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
                Some((_, list)) => list
                    .trim_end_matches(']')
                    .split(',')
                    .map(|a| a.trim().to_string())
                    .filter(|a| !a.is_empty())
                    .collect(),
                None => Vec::new(),
            };
            let clean = axioms
                .iter()
                .all(|axiom| STANDARD_AXIOMS.contains(&axiom.as_str()));
            Some(Checked {
                theorem: name.to_string(),
                axioms,
                clean,
            })
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
        .map(|i| {
            (
                i + 1,
                a.get(i).unwrap_or(&"<missing>").to_string(),
                b.get(i).unwrap_or(&"<missing>").to_string(),
            )
        });
    Correspondence {
        scenarios: scenarios
            .lines()
            .filter(|l| l.starts_with("scenario"))
            .count(),
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

/// The hand-written scenarios of a committed fixture file: everything from its first scenario
/// named `h-...`. The generator writes the scenarios before them.
pub fn hand_written(file: &str) -> &str {
    if file.starts_with("scenario h-") {
        return file;
    }
    file.find("\nscenario h-").map_or("", |at| &file[at + 1..])
}

/// The seed and size of the generated part of the committed fixture file `lean/fixtures/scenarios.txt`.
pub const FIXTURE_SEED: u64 = 20_261_118;
pub const FIXTURE_COUNT: usize = 60;

/// Build the Lean project, audit its theorems and run the correspondence fixtures.
///
/// Slow (it runs the Lean toolchain) and never on a request path. `checked_at` is supplied by
/// the caller; this crate does not read the clock.
pub fn verify(lean_dir: &Path, checked_at: i64) -> Report {
    let obligations = crate::obligations::all();
    let assumptions = crate::obligations::assumptions()
        .into_iter()
        .map(str::to_string)
        .collect();
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
        let text = std::fs::read_to_string(lean_dir.join("fixtures/scenarios.txt"))
            .map_err(|e| format!("cannot read fixtures/scenarios.txt: {e}"))?
            .replace("\r\n", "\n");
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
