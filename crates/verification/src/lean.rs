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
//! | fields 1 to 3 of type 4 (the tickets, slice 8) | `f01: string, default open`, `f02: int`, `f03: string`; the workflow `ticket_flow` over `f01` (`open` to `closed` when `f02` is at least one and the note `f03` becomes `shipped`; `closed` to `open`; `open` to `archived` when `f02` is absent, setting `f02` to `0`; `closed` to `archived`; `open` to `escalated` setting `f02` to nine); an editor does everything to them, a writer reads and creates them and changes or deletes the ones it created whose `f02` is at most five |
//! | `type <n>` before the intent | the object type of the intent; any number but `1` and `2` names a type the tenant does not have |
//! | members, roles and policies | the `owner` role; an `editor` role with policies allowing every action on each object type; a `writer` role with policies allowing read and create, and update or delete where `record.$created_by == principal`; the `public` role with a policy allowing create of `part` when no user acts (`isNull(principal)`), so an anonymous caller may create a part; the system holds no role |
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
//! | tenant `3` | a tenant that does not exist until a system `create-tenant` makes it (no object types) |
//! | role `n` | `0` is `owner`, `1` `public`, `2` `editor`, `3` `writer`, any other number the unknown role `role<n>` |
//! | user `n`, `n` below `1000000` | principal `p<n>`; from `1000000` up the reserved principal `team:p<n>` (the model's `reservedId`) |
//! | team `n`, invitation `n` | `team<n>`, `inv<n>` |
//! | `create-tenant <owner> <name>` | `CreateTenant` (the context's principal is the caller) |
//! | `assign <user> <role>`, `revoke <user> <role>` | `AssignRole`, `RevokeRole` |
//! | `team <team> <label>` | `DefineTeam` |
//! | `team-assign`, `team-revoke <team> <role>` | `AssignTeamRole`, `RevokeTeamRole` |
//! | `team-add`, `team-remove <team> <user>` | `AddTeamMember`, `RemoveTeamMember` |
//! | `invite <inv> <address> <roles> @<now>` | `CreateInvitation`, in a context whose `now` is `<now>` |
//! | `invite-revoke <inv>` | `RevokeInvitation` |
//! | `redeem <inv> @<now> [verified=<address>]` | `RedeemInvitation`, in a context whose `now` is `<now>` and whose `verified_address` is the address |
//! | `Err.invalidIntent`, `unknownRole`, `lastOwner`, `tenantExists`, `workflowViolation` | `InvalidIntent`, `UnknownRole`, `LastOwner`, `TenantExists`, `WorkflowViolation` |
//! | the editor role's `manage` power | the policy `editors_manage_members` (`manage` on `Resource::Members`, no condition) |
//! | `fixtureReadRules` (slice 10) | the field read rules of `t1` and `t2`: `f10` of an item is shown when `f03 == true`; `f04` of a part only to `p100`; the note `f03` of a ticket when `f02 >= 1` |
//! | `fixtureQueries` | the named queries `items_by_rank`, `items_not_elm` (a declared parameter `tag` its filter does not read), `parts_ranked` and `tickets_open`, each planned by the kernel as a scan; tenant `3` declares none |
//! | `read <t> <p> records <type> <ids..>` | `ReadRequest::Records`, the ids `n` or `a-b` |
//! | `read <t> <p> query <name> [k=v ..] [page=<n>] [cursor=<c>]` | `ReadRequest::Named`; a cursor is `o<n>` (`PageCursor::Offset`) or `a<id>` (`After`) |
//! | `Err.unknownQuery` (read only) | `UnknownQuery` |
//!
//! # The Main.lean protocol
//!
//! `lean/Main.lean` takes one argument, the path of a scenario file, and prints one result text
//! for all of it. A file holds any number of scenarios, so one process runs a whole batch:
//!
//! ```text
//! scenario <name>              starts a scenario from the initial world
//! op <tenant> <principal> [type <n>] <intent> [args]
//!                              `<principal>` is a user number, `system` (no role) or `anon` (the `public` role only);
//!                              one intent, as `create f=v ..`, `update <id> [v=n] f=v ..`,
//!                              `patch <id> [v=n] f=v ..`, `delete <id> [v=n]`,
//!                              `require <field> strict|lax`
//!                              `unique <field> strict|lax`, `composite <field> ..`, or a membership intent
//!                              (slice 7): `create-tenant <owner> <name>`, `assign <user> <role>`,
//!                              `revoke <user> <role>`, `team <team> <label>`, `team-assign <team> <role>`,
//!                              `team-revoke <team> <role>`, `team-add <team> <user>`,
//!                              `team-remove <team> <user>`, `invite <inv> <address> <roles> @<now>`,
//!                              `invite-revoke <inv>`, `redeem <inv> @<now> [verified=<address>]`.
//!                              A text (name, label, address) is `s:<text>` with `_` for a space; roles
//!                              are numbers separated by commas, or `-` for none; `@<now>` is the time of
//!                              the request in milliseconds
//! end                          prints the final state and returns to the initial world
//! ```
//!
//! A value is `n`, `i:<int>`, `s:<text>`, `s*<count>:<text>` (the text repeated), `b:1`, `b:0`,
//! `r:<id>`, or a list: `l:<element>,<element>,..` (`l:` is empty) or `l*<count>:<element>`.
//!
//! Output: `scenario <name>`, then per op either `err <kind>` or an accepted step: `ok`, `revision <n>`
//! (the tenant's revision after it), one `event <seq> rev=<rev> by=<principal> <body>` line per event,
//! then one `effect publish <seq>` per event (`unparsed <line>` for a line it cannot read), then
//! `state`, the state lines and `end`. `outcome_lines` drops the revision, event and effect lines.
//! A read line (`read <tenant> <principal> records <type> <ids..>` or
//! `read <tenant> <principal> query <name> [k=v ..] [page=<n>] [cursor=<c>]`; an id is `n` or `a-b`) changes
//! nothing and prints `err <kind>` (`no_tenant`, `unknown_object`, `unknown_query`, `invalid_intent`,
//! `limit_exceeded`), or `ok`, `next <cursor>` (`-` for none) and one `row <id> <record>` line per row, in order.
//! `op_outcomes` cuts an output into the lines of each operation, so that the coverage counts one outcome each.
//! A tenant's state lines end with its membership (`roles`, `member`, `team` and `invitation` lines, sorted) when that is not the one the
//! fixture starts it with, and always for tenant 3, which is printed only when it exists. A new intent is a new `op` arm in
//! `parseOp`; `crate::lean_diff` generates every such intent (and a test fails if one is missing).
//! The differential harness is `crate::lean_diff`.
//!
//! Agreement on these scenarios is evidence of correspondence on the modelled fragment. It is
//! not a proof, and it says nothing about what the model leaves out: workflows, queries, events,
//! teams, scopes, role and policy definitions by intent, the clock, and the expression functions beyond
//! the slice 5 fragment.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use strata_kernel::search::{SearchRequest, SearchResult};
use strata_kernel::types::{PageCursor, QueryId};
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
        4 => ticket(),
        n => id(format!("type{n}")),
    }
}

/// The fields an `update` line names for an object type, as the model's `fieldsOf` lists them.
fn fixture_fields(object: &ObjectTypeId) -> Vec<u64> {
    if *object == item() {
        vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
    } else if *object == part() {
        vec![1, 2, 3, 4, 5]
    } else if *object == ticket() {
        vec![1, 2, 3]
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

/// The constraint of the part type (the model's `partConstraints`, WG-5664): a part names an item
/// (field `1`, or field `2`, a set-null reference, which a delete clears), has a number (field `4`),
/// or was made by `p100` (field `5`, whose default is whether the caller is `p100`). A delete that
/// clears the only reference of a part that has none of the others is refused.
fn part_constraints() -> Vec<SchemaChange> {
    let field = |n: u64| Box::new(Expr::Var(Scope::Record, format!("f{n:02}")));
    let named = |n: u64| Expr::Not(Box::new(Expr::IsNull(field(n))));
    let made_by_p100 = Expr::Cmp(CmpOp::Eq, field(5), Box::new(Expr::Lit(Value::Bool(true))));
    vec![SchemaChange::DefineConstraint {
        object: part(),
        constraint: ConstraintDef {
            id: id("c1".to_string()),
            expr: Expr::Or(vec![named(1), named(2), named(4), made_by_p100]),
            message: "a part names no item, has no number and was not made by p100".to_string(),
        },
    }]
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
        read: item_read(n),
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
        read: part_read(n),
        personal: None,
        normalise: None,
        computed: None,
    }
}

/// The read rule of field `n` of an item (the model's `fixtureReadRules`): field `10` is shown only
/// when the boolean field `3` is `true`.
fn item_read(n: u64) -> Option<Expr> {
    (n == 10).then(|| {
        Expr::Cmp(
            CmpOp::Eq,
            Box::new(Expr::Var(Scope::Record, "f03".into())),
            Box::new(Expr::Lit(Value::Bool(true))),
        )
    })
}

/// The read rule of field `n` of a part: field `4` is shown only to `p100`.
fn part_read(n: u64) -> Option<Expr> {
    (n == 4).then(|| {
        Expr::Cmp(
            CmpOp::Eq,
            Box::new(Expr::Principal),
            Box::new(Expr::Lit(Value::Str("p100".into()))),
        )
    })
}

/// The read rule of field `n` of a ticket: the note (field `3`) is shown only once the ticket has a
/// vote (field `2` at least one).
fn ticket_read(n: u64) -> Option<Expr> {
    (n == 3).then(|| {
        Expr::Cmp(
            CmpOp::Ge,
            Box::new(Expr::Var(Scope::Record, "f02".into())),
            Box::new(Expr::Lit(Value::Int(1))),
        )
    })
}

/// The named queries of the fixture (the model's `fixtureQueries`), declared in tenants `t1` and `t2`.
/// Each is one the kernel plans as a scan: it orders, and either has no filter or orders by more than
/// one key. `items_not_elm` declares a parameter `tag` that its filter does not read.
fn fixture_queries() -> Vec<SchemaChange> {
    let key = |field: &str, descending: bool| OrderKey {
        field: field.to_string(),
        descending,
    };
    let text = |op: CmpOp, field: &str, value: &str| {
        Expr::Cmp(
            op,
            Box::new(Expr::Var(Scope::Record, field.into())),
            Box::new(Expr::Lit(Value::Str(value.into()))),
        )
    };
    let query = |name: &str,
                 object: ObjectTypeId,
                 filter: Option<Expr>,
                 params: &[&str],
                 order: Vec<OrderKey>,
                 limit: Option<u32>| {
        SchemaChange::DefineQuery(QueryDef {
            id: id::<QueryId>(name.to_string()),
            object,
            filter,
            params: params.iter().map(|p| p.to_string()).collect(),
            order,
            limit,
            aggregates: None,
            declared_fields: None,
            tree: None,
        })
    };
    let changes = vec![
        query(
            "items_by_rank",
            item(),
            None,
            &[],
            vec![key("f02", true)],
            None,
        ),
        query(
            "items_not_elm",
            item(),
            Some(text(CmpOp::Ne, "f01", "elm")),
            &["tag"],
            vec![key("f03", false), key("f02", true)],
            Some(3),
        ),
        query(
            "parts_ranked",
            part(),
            None,
            &[],
            vec![key("f04", false)],
            None,
        ),
        query(
            "tickets_open",
            ticket(),
            Some(text(CmpOp::Eq, "f01", "open")),
            &[],
            vec![key("f02", false), key("f03", false)],
            Some(2),
        ),
    ];
    changes.into_iter().map(with_aggregates).collect()
}

/// The aggregates and the declared fields of a fixture query (the model's `fixtureQueries`, slice 10, second
/// half): what a request may filter or aggregate on, and the aggregates it declares by name. The queries
/// with none are left as they are.
fn with_aggregates(change: SchemaChange) -> SchemaChange {
    let SchemaChange::DefineQuery(mut def) = change else {
        return change;
    };
    let agg = |name: &str,
               kind: AggregateKind,
               field: Option<u64>,
               group_by: Option<u64>,
               n: Option<u32>,
               descending: bool,
               scan_bound: u32| AggregateDef {
        id: id::<AggregateId>(name.to_string()),
        kind,
        field: field.map(field_id),
        group_by: group_by.map(field_id),
        group_by_second: None,
        n,
        descending,
        scan_bound,
    };
    let (declared, aggregates): (Vec<u64>, Vec<AggregateDef>) = match def.id.0.as_str() {
        "items_by_rank" => (
            vec![2, 3],
            vec![
                agg(
                    "count",
                    AggregateKind::Count,
                    None,
                    None,
                    None,
                    false,
                    10_000,
                ),
                agg(
                    "bounded_count",
                    AggregateKind::Count,
                    None,
                    None,
                    None,
                    false,
                    2,
                ),
                agg(
                    "sum_rank",
                    AggregateKind::Sum,
                    Some(2),
                    None,
                    None,
                    false,
                    10_000,
                ),
                agg(
                    "min_rank",
                    AggregateKind::Minimum,
                    Some(2),
                    None,
                    None,
                    false,
                    10_000,
                ),
                agg(
                    "max_rank",
                    AggregateKind::Maximum,
                    Some(2),
                    None,
                    None,
                    false,
                    10_000,
                ),
                agg(
                    "top_rank",
                    AggregateKind::Top,
                    Some(2),
                    None,
                    Some(2),
                    true,
                    10_000,
                ),
                agg(
                    "count_by_flag",
                    AggregateKind::Count,
                    None,
                    Some(3),
                    None,
                    false,
                    10_000,
                ),
            ],
        ),
        "items_not_elm" => (
            vec![2, 3],
            vec![
                agg(
                    "count",
                    AggregateKind::Count,
                    None,
                    None,
                    None,
                    false,
                    10_000,
                ),
                agg(
                    "sum_rank",
                    AggregateKind::Sum,
                    Some(2),
                    None,
                    None,
                    false,
                    10_000,
                ),
            ],
        ),
        "parts_ranked" => (
            vec![4],
            vec![
                agg(
                    "count",
                    AggregateKind::Count,
                    None,
                    None,
                    None,
                    false,
                    10_000,
                ),
                agg(
                    "sum",
                    AggregateKind::Sum,
                    Some(4),
                    None,
                    None,
                    false,
                    10_000,
                ),
                agg(
                    "max",
                    AggregateKind::Maximum,
                    Some(4),
                    None,
                    None,
                    false,
                    10_000,
                ),
            ],
        ),
        "tickets_open" => (
            vec![2, 3],
            vec![
                agg(
                    "count",
                    AggregateKind::Count,
                    None,
                    None,
                    None,
                    false,
                    10_000,
                ),
                agg(
                    "sum_votes",
                    AggregateKind::Sum,
                    Some(2),
                    None,
                    None,
                    false,
                    10_000,
                ),
                agg(
                    "max_note",
                    AggregateKind::Maximum,
                    Some(3),
                    None,
                    None,
                    false,
                    10_000,
                ),
            ],
        ),
        _ => (Vec::new(), Vec::new()),
    };
    if !declared.is_empty() {
        def.declared_fields = Some(declared.iter().map(|n| field_id(*n)).collect());
    }
    if !aggregates.is_empty() {
        def.aggregates = Some(aggregates);
    }
    SchemaChange::DefineQuery(def)
}

/// The object type of the tickets, the model's type `4` (slice 8): a state (`1`), a vote count (`2`)
/// and a note (`3`), with the workflow of `ticket_workflow`.
fn ticket() -> ObjectTypeId {
    id("ticket".to_string())
}

/// The type of a ticket's field: the vote count is an integer, the state and the note are strings.
fn ticket_field_type(n: u64) -> FieldType {
    if n == 2 {
        FieldType::Int
    } else {
        FieldType::Str
    }
}

/// The default of a ticket's field: a new ticket starts `open` (field `1`).
fn ticket_default(n: u64) -> Option<Expr> {
    (n == 1).then(|| Expr::Lit(Value::Str("open".into())))
}

fn ticket_field_def(n: u64) -> FieldDef {
    FieldDef {
        id: field_id(n),
        label: format!("f{n}"),
        ty: ticket_field_type(n),
        required: false,
        unique: false,
        default: ticket_default(n),
        read: ticket_read(n),
        personal: None,
        normalise: None,
        computed: None,
    }
}

/// The workflow of the tickets (the model's `ticketWorkflow`, slice 8), over the state field `1`: a
/// ticket with a vote ships (`closed`, note `shipped`); a closed one reopens; an open one without votes
/// is archived with its votes set to `0`; a closed one is archived with its note set;
/// an open one is escalated to nine votes.
fn ticket_workflow() -> WorkflowDef {
    let votes = |scope: Scope| Box::new(Expr::Var(scope, "f02".to_string()));
    WorkflowDef {
        id: id("ticket_flow".to_string()),
        object: ticket(),
        field: field_id(1),
        initial: BTreeSet::from(["open".to_string()]),
        transitions: vec![
            WorkflowTransition {
                name: "ship".into(),
                from: "open".into(),
                to: "closed".into(),
                guard: Some(Expr::Cmp(
                    CmpOp::Ge,
                    votes(Scope::New),
                    Box::new(Expr::Lit(Value::Int(1))),
                )),
                set: BTreeMap::from([(field_id(3), Expr::Lit(Value::Str("shipped".into())))]),
            },
            WorkflowTransition {
                name: "reopen".into(),
                from: "closed".into(),
                to: "open".into(),
                guard: None,
                set: BTreeMap::new(),
            },
            WorkflowTransition {
                name: "archive".into(),
                from: "open".into(),
                to: "archived".into(),
                guard: Some(Expr::IsNull(votes(Scope::Old))),
                set: BTreeMap::from([(field_id(2), Expr::Lit(Value::Int(0)))]),
            },
            WorkflowTransition {
                name: "archive_closed".into(),
                from: "closed".into(),
                to: "archived".into(),
                guard: None,
                set: BTreeMap::from([(field_id(3), Expr::Lit(Value::Str("archived".into())))]),
            },
            WorkflowTransition {
                name: "escalate".into(),
                from: "open".into(),
                to: "escalated".into(),
                guard: None,
                set: BTreeMap::from([(field_id(2), Expr::Lit(Value::Int(9)))]),
            },
        ],
    }
}

/// The definition a `require` or `unique` line falls back to when the tenant does not have the
/// field yet, as `field_def`, `part_field_def` and `ticket_field_def` define the fixture's fields.
fn fallback_field(object: &ObjectTypeId, n: u64) -> FieldDef {
    if *object == part() {
        part_field_def(n)
    } else if *object == ticket() {
        ticket_field_def(n)
    } else {
        field_def(n, false)
    }
}

/// The user numbers from this one up are the kernel's reserved principal ids (the model's
/// `reservedFrom`): the harness writes such a user `team:p<n>`, which `refuse_reserved_principals`
/// refuses as a caller and as a named user.
const RESERVED_FROM: u64 = 1_000_000;

/// The principal id of user `n`.
fn principal_id(n: u64) -> PrincipalId {
    if n >= RESERVED_FROM {
        id(format!("team:p{n}"))
    } else {
        id(format!("p{n}"))
    }
}

fn principal(n: u64) -> Principal {
    Principal::User(principal_id(n))
}

/// The role numbered `n` in the scenario format: `0` owner, `1` public, `2` editor, `3` writer, and
/// any other number a role the tenant does not define.
fn role_id(n: u64) -> RoleId {
    id(match n {
        0 => "owner".to_string(),
        1 => "public".to_string(),
        2 => "editor".to_string(),
        3 => "writer".to_string(),
        n => format!("role{n}"),
    })
}

/// The number of a role: the inverse of [`role_id`].
fn role_number(role: &RoleId) -> u64 {
    match role.as_str() {
        "owner" => 0,
        "public" => 1,
        "editor" => 2,
        "writer" => 3,
        other => other["role".len()..]
            .parse()
            .expect("fixture roles are role<number>"),
    }
}

/// The number of a user: the digits after the `p`.
fn user_number(user: &PrincipalId) -> u64 {
    user.as_str()
        .trim_start_matches("team:")
        .trim_start_matches('p')
        .parse()
        .expect("fixture users are p<number>")
}

/// A text token of the scenario format: `s:<text>`, `_` for a space.
fn parse_text(token: &str) -> Option<String> {
    token.strip_prefix("s:").map(|text| text.replace('_', " "))
}

/// A text as the state lines print it.
fn show_text(text: &str) -> String {
    format!("s:{}", text.replace(' ', "_"))
}

/// A set of numbers, sorted, separated by commas; `-` for none.
fn show_numbers(mut numbers: Vec<u64>) -> String {
    numbers.sort_unstable();
    if numbers.is_empty() {
        "-".to_string()
    } else {
        numbers
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// The roles of an invitation as the scenario format writes them: numbers separated by commas, or
/// `-` for none.
fn parse_roles(token: &str) -> Option<BTreeSet<RoleId>> {
    if token == "-" {
        return Some(BTreeSet::new());
    }
    token
        .split(',')
        .map(|n| n.parse::<u64>().ok().map(role_id))
        .collect()
}

/// The time of a request: `@<milliseconds>`.
fn parse_now(token: &str) -> Option<Timestamp> {
    token
        .strip_prefix('@')
        .and_then(|n| n.parse::<i64>().ok())
        .map(Timestamp)
}

/// A membership or creation line of the scenario format (slice 7), or `None` for a line it cannot
/// read. The two that carry the time and the verified address of the request set them in `ctx`.
fn parse_member_op(rest: &[&str], ctx: &mut Context) -> Option<Intent> {
    let number = |token: &str| token.parse::<u64>().ok();
    Some(match rest {
        ["create-tenant", owner, name] => Intent::CreateTenant {
            name: parse_text(name)?,
            owner: principal_id(number(owner)?),
        },
        ["assign", user, role] => Intent::AssignRole {
            principal: principal_id(number(user)?),
            role: role_id(number(role)?),
        },
        ["revoke", user, role] => Intent::RevokeRole {
            principal: principal_id(number(user)?),
            role: role_id(number(role)?),
        },
        ["team", team, label] => Intent::DefineTeam {
            team: id(format!("team{}", number(team)?)),
            label: parse_text(label)?,
        },
        ["team-assign", team, role] => Intent::AssignTeamRole {
            team: id(format!("team{}", number(team)?)),
            role: role_id(number(role)?),
        },
        ["team-revoke", team, role] => Intent::RevokeTeamRole {
            team: id(format!("team{}", number(team)?)),
            role: role_id(number(role)?),
        },
        ["team-add", team, user] => Intent::AddTeamMember {
            team: id(format!("team{}", number(team)?)),
            principal: principal_id(number(user)?),
        },
        ["team-remove", team, user] => Intent::RemoveTeamMember {
            team: id(format!("team{}", number(team)?)),
            principal: principal_id(number(user)?),
        },
        ["invite", invitation, address, roles, now] => {
            ctx.now = parse_now(now)?;
            Intent::CreateInvitation {
                invitation: id(format!("inv{}", number(invitation)?)),
                address: parse_text(address)?,
                roles: parse_roles(roles)?,
            }
        }
        ["invite-revoke", invitation] => Intent::RevokeInvitation {
            invitation: id(format!("inv{}", number(invitation)?)),
        },
        ["redeem", invitation, now] => {
            ctx.now = parse_now(now)?;
            Intent::RedeemInvitation {
                invitation: id(format!("inv{}", number(invitation)?)),
            }
        }
        ["redeem", invitation, now, verified] => {
            ctx.now = parse_now(now)?;
            ctx.verified_address = Some(parse_text(verified.strip_prefix("verified=")?)?);
            Intent::RedeemInvitation {
                invitation: id(format!("inv{}", number(invitation)?)),
            }
        }
        _ => return None,
    })
}

/// A principal token of the scenario format: a number is that user, `system` the system and `anon` an
/// anonymous caller.
fn parse_principal(token: &str) -> Principal {
    match token {
        "system" => Principal::System,
        "anon" => Principal::Anonymous,
        number => principal(number.parse().expect("fixture principal")),
    }
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
        // The constraints read fields, so they follow the fields of both types.
        changes.extend(item_constraints());
        changes.extend(part_constraints());
        // The tickets (the model's type `4`, slice 8): their fields, then their workflow, which reads them.
        changes.push(SchemaChange::DefineObject {
            id: ticket(),
            label: "Ticket".into(),
        });
        changes.extend([1u64, 2, 3].map(|n| SchemaChange::DefineField {
            object: ticket(),
            field: ticket_field_def(n),
        }));
        changes.push(SchemaChange::DefineWorkflow(ticket_workflow()));
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
            // The tickets: an editor does everything; a writer reads and creates them, and changes or
            // deletes the ones it created whose votes are at most five (the model's `ticketWorkflow`
            // and `fixturePolicies`).
            policy(
                ticket(),
                "tickets_editors_all",
                "editor",
                &[Action::Read, Action::Create, Action::Update, Action::Delete],
                None,
            ),
            policy(
                ticket(),
                "tickets_writers_create",
                "writer",
                &[Action::Read, Action::Create],
                None,
            ),
            policy(
                ticket(),
                "tickets_writers_own",
                "writer",
                &[Action::Update, Action::Delete],
                Some(Expr::And(vec![
                    own.clone(),
                    Expr::Cmp(
                        CmpOp::Le,
                        Box::new(Expr::Var(Scope::New, "f02".into())),
                        Box::new(Expr::Lit(Value::Int(5))),
                    ),
                ])),
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
            // Anyone who holds `public` may create a part when no user is acting: an anonymous caller.
            policy(
                part(),
                "public_creates_parts_anonymously",
                "public",
                &[Action::Create],
                Some(Expr::IsNull(Box::new(Expr::Principal))),
            ),
            // An editor may manage the tenant's members (slice 7), so a manager who is not the owner
            // is in the fixture: it may confer the roles whose powers it holds.
            SchemaChange::DefinePolicy(PolicyDef {
                id: id("editors_manage_members".to_string()),
                role: id("editor".to_string()),
                resource: Resource::Members,
                actions: BTreeSet::from([Action::Manage]),
                condition: None,
            }),
        ]);
        // The named queries read the fields above and the policies decide who may read them.
        changes.extend(fixture_queries());
        // The searchable fields (slice 10, second half): an item's text fields (`1`, `7`, `9` and `10`) and a
        // ticket's state and note (`1` and `3`). A part has no text field.
        changes.push(SchemaChange::SetSearchable {
            object: item(),
            fields: vec![field_id(1), field_id(7), field_id(9), field_id(10)],
        });
        changes.push(SchemaChange::SetSearchable {
            object: ticket(),
            fields: vec![field_id(1), field_id(3)],
        });
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
        ErrorKind::InvalidIntent => "invalid_intent",
        ErrorKind::UnknownRole => "unknown_role",
        ErrorKind::LastOwner => "last_owner",
        ErrorKind::TenantExists => "tenant_exists",
        ErrorKind::WorkflowViolation => "workflow_violation",
        ErrorKind::UnknownQuery => "unknown_query",
        _ => "unmodelled",
    }
}

/// The membership of a tenant, one line each: its roles, its members, its teams and its invitations,
/// sorted by number, lists sorted (the model's `membershipLines`).
fn membership_lines(n: u64, tenant: &TenantState) -> Vec<String> {
    let mut out = vec![format!(
        "roles {n} {}",
        show_numbers(tenant.schema.roles.keys().map(role_number).collect())
    )];
    let mut members: Vec<(u64, String)> = tenant
        .members
        .iter()
        .map(|(user, roles)| {
            (
                user_number(user),
                show_numbers(roles.iter().map(role_number).collect()),
            )
        })
        .collect();
    members.sort();
    out.extend(
        members
            .iter()
            .map(|(user, roles)| format!("member {n} {user} {roles}")),
    );
    let mut teams: Vec<(u64, String)> = tenant
        .schema
        .all_teams()
        .map(|team| {
            (
                team.id.as_str()["team".len()..]
                    .parse()
                    .expect("fixture teams are team<number>"),
                format!(
                    "{} {} {}",
                    show_text(&team.label),
                    show_numbers(team.members.iter().map(user_number).collect()),
                    show_numbers(team.roles.iter().map(role_number).collect())
                ),
            )
        })
        .collect();
    teams.sort();
    out.extend(
        teams
            .iter()
            .map(|(team, rest)| format!("team {n} {team} {rest}")),
    );
    let user_or_dash = |user: &Option<PrincipalId>| {
        user.as_ref()
            .map_or_else(|| "-".to_string(), |u| user_number(u).to_string())
    };
    let mut invitations: Vec<(u64, String)> = tenant
        .schema
        .all_invitations()
        .map(|invitation| {
            (
                invitation.id.as_str()["inv".len()..]
                    .parse()
                    .expect("fixture invitations are inv<number>"),
                format!(
                    "{} {} by={} expires={} redeemed={} revoked={}",
                    show_text(&invitation.address),
                    show_numbers(invitation.roles.iter().map(role_number).collect()),
                    user_number(&invitation.created_by),
                    invitation
                        .expires_at
                        .map_or_else(|| "-".to_string(), |at| at.0.to_string()),
                    user_or_dash(&invitation.redeemed_by),
                    user_or_dash(&invitation.revoked_by)
                ),
            )
        })
        .collect();
    invitations.sort();
    out.extend(
        invitations
            .iter()
            .map(|(invitation, rest)| format!("invitation {n} {invitation} {rest}")),
    );
    out
}

/// The membership lines of the fixture's own tenants (`1` and `2`), which a tenant's state prints its
/// membership against.
fn initial_membership() -> &'static [Vec<String>; 2] {
    static INITIAL: std::sync::OnceLock<[Vec<String>; 2]> = std::sync::OnceLock::new();
    INITIAL.get_or_init(|| {
        let world = initial_world();
        [1u64, 2].map(|n| {
            let tenant = &world.tenants[&id::<TenantId>(format!("t{n}"))];
            membership_lines(n, tenant)
        })
    })
}

fn show_world(world: &World) -> Vec<String> {
    let mut out = Vec::new();
    // Tenants 1 and 2 are always printed, `absent` when they are; tenant 3 only when a creation made it.
    for n in [1u64, 2, 3] {
        let Some(tenant) = world.tenants.get(&id::<TenantId>(format!("t{n}"))) else {
            if n != 3 {
                out.push(format!("tenant {n} absent"));
            }
            continue;
        };
        // The required fields of an object type, by number, as the model's `requiredOf` prints
        // them. Fields print in the model's order, by number (`f10` after `f6`), not by name.
        let required_text = |object: &ObjectTypeId| -> String {
            let Some(definition) = tenant.schema.objects.get(object) else {
                return String::new();
            };
            let mut required: Vec<u64> = definition
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
        for (object, pre) in [(item(), ""), (part(), "type 2 "), (ticket(), "type 4 ")] {
            if !pre.is_empty() {
                if !tenant.schema.objects.contains_key(&object) {
                    continue;
                }
                // The tickets print only while they hold a record, as the model's `showTenant` does.
                if pre == "type 4 " && tenant.records_of(&object).next().is_none() {
                    continue;
                }
                out.push(format!("{pre}required={}", required_text(&object)));
            }
            for record in tenant.records_of(&object) {
                out.push(
                    format!("{pre}record {} {}", record.id, record_text(record))
                        .trim_end()
                        .to_string(),
                );
            }
        }
        // The membership, when it is not the one the fixture starts the tenant with.
        let lines = membership_lines(n, tenant);
        if n == 3 || lines != initial_membership()[n as usize - 1] {
            out.extend(lines);
        }
    }
    out
}

/// The object type as the scenario format numbers it: `item` is `1`, `part` is `2`, `ticket` is `4`,
/// and `type<n>` is `n`.
fn object_number(object: &ObjectTypeId) -> u64 {
    match object.as_str() {
        "item" => 1,
        "part" => 2,
        "ticket" => 4,
        other => other
            .strip_prefix("type")
            .and_then(|n| n.parse().ok())
            .expect("fixture object types are item, part, ticket or type<number>"),
    }
}

/// A record as the model's `recordBody` prints it: its creator (a user number, or `-`), its version and
/// its fields by number, with the values of `show_value`. Used by the state lines and by the events.
fn record_text(record: &Record) -> String {
    // The creator as the model prints it: its user id, or `-` when no user created the record.
    let owner = record
        .created_by
        .as_ref()
        .map_or_else(|| "-".to_string(), |p| p.as_str()[1..].to_string());
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
    format!("owner={owner} v={} {}", record.version, fields.join(" "))
        .trim_end()
        .to_string()
}

/// A principal as the event lines print it: a user's number, `system` or `anon`.
fn principal_text(principal: &Principal) -> String {
    match principal {
        Principal::User(user) => user_number(user).to_string(),
        Principal::System => "system".to_string(),
        Principal::Anonymous => "anon".to_string(),
    }
}

/// The member of a `member-changed` event: a user's number, or `team:<team number>` for the change of a
/// team's own roles (the kernel names a team's principal `team:<team id>`, and its id is `team<n>`).
fn member_text(principal: &PrincipalId) -> String {
    match principal.as_str().strip_prefix("team:team") {
        Some(number) => format!("team:{number}"),
        None => user_number(principal).to_string(),
    }
}

/// The summary of a `schema-changed` line, from the change list of the intent: each field definition as
/// `field:<object>.<field>` and each composite unique as `unique:<object>:<fields>`, in list order; `-` for
/// none. Other kinds of change are not in the model and are left out.
fn schema_summary(changes: &[SchemaChange]) -> String {
    let items: Vec<String> = changes
        .iter()
        .filter_map(|change| match change {
            SchemaChange::DefineField { object, field } => Some(format!(
                "field:{}.{}",
                object_number(object),
                field_number(&field.id)
            )),
            SchemaChange::DefineUnique { object, unique } => {
                let fields: Vec<String> = unique
                    .fields
                    .iter()
                    .map(|field| field_number(field).to_string())
                    .collect();
                Some(format!(
                    "unique:{}:{}",
                    object_number(object),
                    fields.join(",")
                ))
            }
            _ => None,
        })
        .collect();
    if items.is_empty() {
        "-".to_string()
    } else {
        items.join(";")
    }
}

/// The workflow transitions an update fired, sorted by workflow id, as `<workflow>:<transition>` entries
/// joined by commas; `-` for none.
fn fired_text(fired: Option<&BTreeMap<WorkflowId, String>>) -> String {
    match fired {
        Some(map) if !map.is_empty() => map
            .iter()
            .map(|(workflow, transition)| format!("{workflow}:{transition}"))
            .collect::<Vec<String>>()
            .join(","),
        _ => "-".to_string(),
    }
}

/// One domain event as the model's `showEvent` prints it: the sequence number, the revision, the
/// principal and the body. `changes` is the change list of the intent that emitted the event (empty for
/// any other intent); a `schema-changed` line takes its summary from it.
fn event_text(event: &DomainEvent, changes: &[SchemaChange]) -> String {
    let body = match &event.body {
        EventBody::TenantCreated { name } => format!("tenant-created name={name}"),
        EventBody::SchemaChanged {
            version,
            grandfathered,
            ..
        } => format!(
            "schema-changed version={} grandfathered={grandfathered} summary={}",
            version.0,
            schema_summary(changes)
        ),
        EventBody::RecordCreated { record } => format!(
            "record-created type={} id={} {}",
            object_number(&record.object),
            record.id,
            record_text(record)
        ),
        EventBody::RecordUpdated {
            before,
            after,
            transitions,
        } => format!(
            "record-updated type={} id={} before=[{}] after=[{}] fired={}",
            object_number(&before.object),
            before.id,
            record_text(before),
            record_text(after),
            fired_text(transitions.as_ref())
        ),
        EventBody::RecordDeleted { before } => format!(
            "record-deleted type={} id={} before=[{}]",
            object_number(&before.object),
            before.id,
            record_text(before)
        ),
        EventBody::MemberChanged { principal, roles } => format!(
            "member-changed who={} roles={}",
            member_text(principal),
            show_numbers(roles.iter().map(role_number).collect())
        ),
        // The model has no such event. The line does not start with `event `, so a kernel that emits one
        // makes the comparison fail instead of being hidden by the filter of `outcome_lines`.
        _ => format!("unmodelled event seq={}", event.seq),
    };
    format!(
        "event {} rev={} by={} {body}",
        event.seq,
        event.revision.0,
        principal_text(&event.principal)
    )
}

/// The lines of an output that are not the revision, event and effect lines of an accepted step. The
/// committed `expected.txt` holds only these outcome lines.
pub fn outcome_lines(output: &str) -> String {
    let kept: Vec<&str> = output
        .lines()
        .filter(|line| {
            !(line.starts_with("revision ")
                || line.starts_with("event ")
                || line.starts_with("effect "))
        })
        .collect();
    let mut text = kept.join("\n");
    text.push('\n');
    text
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

/// A token of record ids: `n`, or `a-b` for every id from `a` to `b` (the model's `parseIds`).
fn parse_ids(token: &str) -> Option<Vec<RecordId>> {
    match token.split_once('-') {
        None => Some(vec![RecordId(token.parse().ok()?)]),
        Some((first, last)) => {
            let (first, last): (u64, u64) = (first.parse().ok()?, last.parse().ok()?);
            Some((first..=last).map(RecordId).collect())
        }
    }
}

/// A cursor token: `o<n>` is an offset, `a<id>` a position after a record (the model's `parseCursor`).
fn parse_cursor(token: &str) -> Option<PageCursor> {
    if let Some(offset) = token.strip_prefix('o') {
        Some(PageCursor::Offset(offset.parse().ok()?))
    } else {
        let after = token.strip_prefix('a')?;
        Some(PageCursor::After(RecordId(after.parse().ok()?)))
    }
}

/// The request a read line asks, after its tenant and principal: `records <type> <ids..>` or
/// `query <name> [k=v ..] [page=<n>] [cursor=<c>]` (the model's `parseRead`).
fn parse_read(tokens: &[&str]) -> Option<ReadRequest> {
    match tokens {
        ["records", ty, ids @ ..] => {
            let mut all = Vec::new();
            for token in ids {
                all.extend(parse_ids(token)?);
            }
            Some(ReadRequest::Records {
                object: object_of(ty.parse().ok()?),
                ids: all,
            })
        }
        ["query", name, args @ ..] => {
            let mut params = BTreeMap::new();
            let mut page_size = None;
            let mut cursor = None;
            for arg in args {
                let (key, value) = arg.split_once('=')?;
                match key {
                    "page" => page_size = Some(value.parse().ok()?),
                    "cursor" => cursor = Some(parse_cursor(value)?),
                    _ => {
                        params.insert(key.to_string(), parse_value(value));
                    }
                }
            }
            Some(ReadRequest::Named {
                query: id::<QueryId>(name.to_string()),
                params,
                page_size,
                cursor,
            })
        }
        _ => None,
    }
}

/// The lines a read prints (the model's `showRead`): `err <kind>`, or `ok`, `next <cursor>` (`-` for
/// none) and one `row <id> <record>` line per row.
fn read_lines(result: &Result<ReadResult, DomainError>) -> Vec<String> {
    match result {
        Err(error) => vec![format!("err {}", show_error(error.kind))],
        Ok(read) => {
            let next = match &read.next {
                None => "-".to_string(),
                Some(PageCursor::Offset(n)) => format!("o{n}"),
                Some(PageCursor::After(after)) => format!("a{}", after.0),
                Some(PageCursor::At(_)) => "at".to_string(),
            };
            let mut lines = vec!["ok".to_string(), format!("next {next}")];
            lines.extend(
                read.rows
                    .iter()
                    .map(|row| format!("row {} {}", row.id.0, record_text(row))),
            );
            lines
        }
    }
}

/// The request a search line asks, after its tenant and principal: `<type|*> <words..> [page=<n>]` (the
/// model's `parseSearch`). The words are the tokens before a last `page=` token, joined by spaces.
fn parse_search(tokens: &[&str]) -> Option<SearchRequest> {
    let (ty, rest) = tokens.split_first()?;
    let objects = match *ty {
        "*" => Vec::new(),
        number => vec![object_of(number.parse().ok()?)],
    };
    let (words, page_size): (&[&str], Option<u32>) = match rest.last() {
        Some(last) if last.starts_with("page=") => (
            &rest[..rest.len() - 1],
            Some(last["page=".len()..].parse().ok()?),
        ),
        _ => (rest, None),
    };
    Some(SearchRequest {
        query: words.join(" "),
        objects,
        page_size,
        cursor: None,
    })
}

/// The request an aggregate line asks, after its tenant and principal: `<query> <aggregate> [filter.<n>=<v>
/// ..] [<param>=<v> ..]` (the model's `parseAggregate`). `filter.<n>` filters on field `n`; any other `k=v` is
/// a parameter.
fn parse_aggregate(tokens: &[&str]) -> Option<AggregateRequest> {
    let [query, aggregate, args @ ..] = tokens else {
        return None;
    };
    let mut params = BTreeMap::new();
    let mut filter = BTreeMap::new();
    for arg in args {
        let (key, value) = arg.split_once('=')?;
        match key.strip_prefix("filter.") {
            Some(number) => {
                filter.insert(field_id(number.parse().ok()?), parse_value(value));
            }
            None => {
                params.insert(key.to_string(), parse_value(value));
            }
        }
    }
    Some(AggregateRequest {
        query: id::<QueryId>(query.to_string()),
        aggregate: id::<AggregateId>(aggregate.to_string()),
        params,
        filter,
    })
}

/// The number the model gives an object type of the fixture (`item` is `1`, `part` `2`, `ticket` `4`).
fn type_number(object: &ObjectTypeId) -> u64 {
    match object.to_string().as_str() {
        "item" => 1,
        "part" => 2,
        "ticket" => 4,
        other => other
            .strip_prefix("type")
            .and_then(|number| number.parse().ok())
            .unwrap_or(0),
    }
}

/// The lines a search prints (the model's `showSearch`): `err <kind>`, or `ok`, `examined <n>`, `next <type>
/// <id>` (`-` for none) and one `hit <type> <id> matched=<fields> <record>` line per hit, in order.
fn search_lines(result: &Result<SearchResult, DomainError>) -> Vec<String> {
    match result {
        Err(error) => vec![format!("err {}", show_error(error.kind))],
        Ok(found) => {
            let next = match &found.next {
                None => "-".to_string(),
                Some(cursor) => format!("{} {}", type_number(&cursor.object), cursor.id.0),
            };
            let mut lines = vec![
                "ok".to_string(),
                format!("examined {}", found.examined),
                format!("next {next}"),
            ];
            for hit in &found.hits {
                let matched: Vec<String> = hit
                    .matched
                    .iter()
                    .map(|field| field_number(field).to_string())
                    .collect();
                lines.push(format!(
                    "hit {} {} matched={} {}",
                    type_number(&hit.object),
                    hit.record.id.0,
                    matched.join(","),
                    record_text(&hit.record)
                ));
            }
            lines
        }
    }
}

/// The error line of an aggregate: a limit names its rule (`scan-bound-exceeded`, `sum-overflow` or
/// `group-bound-exceeded`), and an unknown field is named as such (not as the write's `invalid`).
fn aggregate_error_line(error: &DomainError) -> String {
    match error.kind {
        ErrorKind::LimitExceeded => match error.evidence.first() {
            Some(check) => format!("err limit_exceeded {}", check.rule),
            None => "err limit_exceeded".to_string(),
        },
        ErrorKind::UnknownField => "err unknown_field".to_string(),
        kind => format!("err {}", show_error(kind)),
    }
}

/// A top row's id and value, as the model prints them: the value as `show_value` writes a value.
fn top_row_text(row: &ValueMap) -> Option<(u64, String)> {
    let json: serde_json::Value = serde_json::from_str(row.as_str()).ok()?;
    let id = json.get("id")?.as_u64()?;
    let value = match json.get("value")? {
        serde_json::Value::Null => "n".to_string(),
        serde_json::Value::Bool(flag) => show_value(&Value::Bool(*flag)),
        serde_json::Value::Number(number) => show_value(&Value::Int(number.as_i64()?)),
        serde_json::Value::String(text) => show_value(&Value::Str(text.clone())),
        _ => return None,
    };
    Some((id, value))
}

/// The lines an aggregate prints (the model's `showAgg`): `err <kind>`, or `ok`, `scanned <n>`, and for each
/// group `group <key|-> <value|->` followed by its `top <id> <value>` rows.
fn aggregate_lines(result: &Result<AggregateResult, DomainError>) -> Vec<String> {
    match result {
        Err(error) => vec![aggregate_error_line(error)],
        Ok(found) => {
            let shown = |value: Option<&Value>| value.map_or_else(|| "-".to_string(), show_value);
            let mut lines = vec!["ok".to_string(), format!("scanned {}", found.scanned)];
            for group in &found.groups {
                lines.push(format!(
                    "group {} {}",
                    shown(group.key.as_ref()),
                    shown(group.value.as_ref())
                ));
                for row in &group.rows {
                    if let Some((id, value)) = top_row_text(row) {
                        lines.push(format!("top {id} {value}"));
                    }
                }
            }
            lines
        }
    }
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
                let mut ctx = Context {
                    tenant: id(format!("t{tenant}")),
                    principal: parse_principal(who),
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
                    _ => match parse_member_op(rest, &mut ctx) {
                        Some(intent) => intent,
                        None => {
                            out.push(format!("unparsed {line}"));
                            continue;
                        }
                    },
                };
                match world.step(&intent, &ctx) {
                    Ok((next, transition)) => {
                        world = next;
                        // As the model prints an accepted step: `ok`, the tenant's revision after it, each
                        // event, then one `effect publish` per event. The kernel's other effects are not
                        // modelled and are not printed.
                        out.push("ok".to_string());
                        let revision = world.tenants.get(&ctx.tenant).map_or(0, |t| t.revision.0);
                        out.push(format!("revision {revision}"));
                        let changes: Vec<SchemaChange> = match &intent {
                            Intent::ChangeSchema { changes, .. } => changes.clone(),
                            _ => Vec::new(),
                        };
                        out.extend(
                            transition
                                .events
                                .iter()
                                .map(|event| event_text(event, &changes)),
                        );
                        out.extend(transition.effects.iter().filter_map(|effect| match effect {
                            Effect::Publish { seq } => Some(format!("effect publish {seq}")),
                            _ => None,
                        }));
                    }
                    Err(error) => out.push(format!("err {}", show_error(error.kind))),
                }
            }
            ["read", tenant, who, rest @ ..] => {
                at += 1;
                let ctx = Context {
                    tenant: id(format!("t{tenant}")),
                    principal: parse_principal(who),
                    session: None,
                    now: Timestamp(at),
                    correlation: "fixture".into(),
                    verified_address: None,
                    idempotency_key: None,
                    language: None,
                    hop: None,
                    agent: None,
                };
                match rest {
                    ["search", request @ ..] => match parse_search(request) {
                        Some(request) => out.extend(search_lines(&world.search(&request, &ctx))),
                        None => out.push(format!("unparsed {line}")),
                    },
                    ["aggregate", request @ ..] => match parse_aggregate(request) {
                        Some(request) => {
                            out.extend(aggregate_lines(&world.aggregate(&request, &ctx)))
                        }
                        None => out.push(format!("unparsed {line}")),
                    },
                    _ => match parse_read(rest) {
                        Some(request) => out.extend(read_lines(&world.read(&request, &ctx))),
                        None => out.push(format!("unparsed {line}")),
                    },
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
    let mut command = Command::new(&exe);
    // STRATA_LEAN_KERNEL selects which Lean kernel prints: `spec` (the default, the model),
    // `impl` (the executable kernel, stage 1) or `indexed` (stage 2, the one meant to run).
    match std::env::var("STRATA_LEAN_KERNEL").as_deref() {
        Ok("impl") => {
            command.arg("--impl");
        }
        Ok("indexed") => {
            command.arg("--indexed");
        }
        Ok("spec") | Err(_) => {}
        Ok(other) => {
            return Err(format!(
                "STRATA_LEAN_KERNEL={other}: expected spec, impl or indexed"
            ))
        }
    }
    let output = command
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

/// The lines of each operation in one scenario's output (from its `scenario` line, as `chunks` and
/// `outputs_by_scenario` cut it). An operation prints a first line, `ok`, `err <kind>` or `unparsed ..`,
/// and an accepted one may go on with `revision`, `event`, `effect`, `next` and `row` lines. A group is
/// a first line with the lines that follow it up to the next first line; the state lines at the end
/// are not an operation's. The `k`th group is the outcome of the `k`th operation.
pub fn op_outcomes<S: AsRef<str>>(output: &[S]) -> Vec<&[S]> {
    const CONTINUATION: [&str; 10] = [
        "revision ",
        "event ",
        "effect ",
        "next ",
        "row ",
        "examined ",
        "hit ",
        "scanned ",
        "group ",
        "top ",
    ];
    let end = output
        .iter()
        .position(|line| line.as_ref() == "state")
        .unwrap_or(output.len());
    let mut groups: Vec<&[S]> = Vec::new();
    let mut start: Option<usize> = None;
    for (at, line) in output.iter().enumerate().take(end).skip(1) {
        if CONTINUATION
            .iter()
            .any(|prefix| line.as_ref().starts_with(prefix))
        {
            continue;
        }
        if let Some(first) = start {
            groups.push(&output[first..at]);
        }
        start = Some(at);
    }
    if let Some(first) = start {
        groups.push(&output[first..end]);
    }
    groups
}

/// The kind of an operation's outcome as the coverage counts it: its first line, and for a read the
/// shape of an answer as well. `rendered` is the operation's own line, `group` its outcome lines.
/// A read is `read-records` or `read-query` (by its fourth word); an answer is counted with rows or
/// without, with a next cursor, and with a cursor asked for.
pub fn read_shapes<S: AsRef<str>>(rendered: &str, group: &[S]) -> Vec<String> {
    let mut words = rendered.split_whitespace();
    if words.next() != Some("read") {
        return Vec::new();
    }
    let name = match words.nth(2) {
        Some("search") => return search_shapes(group),
        Some("aggregate") => return aggregate_shapes(group),
        Some("records") => "read-records",
        Some("query") => "read-query",
        _ => return Vec::new(),
    };
    let first = group.first().map_or("", |line| line.as_ref());
    let mut shapes = vec![format!("{name} {first}")];
    if first == "ok" {
        let has = |prefix: &str, except: &str| {
            group
                .iter()
                .any(|line| line.as_ref().starts_with(prefix) && line.as_ref() != except)
        };
        shapes.push(format!(
            "{name} ok rows{}",
            if has("row ", "") { ">0" } else { "=0" }
        ));
        if has("next ", "next -") {
            shapes.push(format!("{name} ok next"));
        }
        if rendered.contains(" cursor=") {
            shapes.push(format!("{name} ok cursor"));
        }
    }
    shapes
}

/// The shapes of a search answer as the coverage counts them: its first line, and for an accepted search
/// whether it holds hits and whether it names a next cursor.
fn search_shapes<S: AsRef<str>>(group: &[S]) -> Vec<String> {
    let first = group.first().map_or("", |line| line.as_ref());
    let mut shapes = vec![format!("read-search {first}")];
    if first == "ok" {
        let hits = group.iter().any(|line| line.as_ref().starts_with("hit "));
        shapes.push(format!(
            "read-search ok hits{}",
            if hits { ">0" } else { "=0" }
        ));
        if group
            .iter()
            .any(|line| line.as_ref().starts_with("next ") && line.as_ref() != "next -")
        {
            shapes.push("read-search ok next".to_string());
        }
    }
    shapes
}

/// The shapes of an aggregate answer as the coverage counts them: its first line, and for an accepted one
/// whether it is grouped (its groups have keys) and whether it lists top rows.
fn aggregate_shapes<S: AsRef<str>>(group: &[S]) -> Vec<String> {
    let first = group.first().map_or("", |line| line.as_ref());
    let mut shapes = vec![format!("read-aggregate {first}")];
    if first == "ok" {
        let grouped = group.iter().any(|line| {
            let line = line.as_ref();
            line.starts_with("group ") && !line.starts_with("group - ")
        });
        shapes.push(format!(
            "read-aggregate ok {}",
            if grouped { "grouped" } else { "ungrouped" }
        ));
        if group.iter().any(|line| line.as_ref().starts_with("top ")) {
            shapes.push("read-aggregate ok top".to_string());
        }
    }
    shapes
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
        operations: scenarios
            .lines()
            .filter(|l| l.starts_with("op ") || l.starts_with("read "))
            .count(),
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
