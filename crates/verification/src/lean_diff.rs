//! Differential random testing of the Rust kernel against the Lean model, in the pattern Cedar
//! uses between its Lean model and its Rust implementation.
//!
//! A seeded generator produces operation sequences ([`Scenario`]) over the two-tenant world the
//! Lean model starts from, using only the intents `lean/Main.lean` executes. Each sequence is
//! rendered to the scenario text both sides read (protocol in [`crate::lean`]). All sequences of a
//! run go into one scenario file, so one Lean process runs the whole batch. Every outcome line and
//! the final state of each scenario are compared; a difference not explained by an entry of
//! [`BY_DESIGN`] fails, with the seed and a sequence shrunk by delta reduction.
//!
//! Shrinking removes whole operations and single field assignments. Each round runs every
//! candidate in one Lean process and keeps the first that still differs, until none does.
//!
//! [`lean_intents`] reads the intent kinds `Main.lean` dispatches so a test can fail when the
//! model gains an intent that [`GENERATED_INTENTS`] does not cover.
//!
//! Slice 7 adds the membership intents: the creation of tenant 3, roles, teams and invitations. The
//! generator mixes single random operations of them with scripted sequences that reach the
//! decisions that need several operations to line up (an invitation redeemed, revoked, expired, its
//! issuer losing the right to manage members; a team that keeps the last owner; the last owner).
//!
//! The generator is aimed at the decisions of slice 2: patches that remove fields, expected
//! versions right and wrong, a default relied on and suppressed, lists of scalars and of
//! references (dangling elements, set-null and restricting lists, a required list), and values
//! one below, at and one above each size bound the kernel states. A record near the record
//! bound is only ever built so that the model's size and the kernel's encoding agree on which
//! side of it it falls (see `straddle`).

use crate::lean::{build_model, kernel_output, read_shapes, run_model};
use std::path::Path;

/// The intent kinds the generator produces; each is the word after the principal in an `op` line.
pub const GENERATED_INTENTS: [&str; 20] = [
    "create",
    "update",
    "patch",
    "delete",
    "require",
    "unique",
    "composite",
    "create-tenant",
    "assign",
    "revoke",
    "team",
    "team-assign",
    "team-revoke",
    "team-add",
    "team-remove",
    "invite",
    "invite-revoke",
    "redeem",
    "schema",
    "setting",
];

/// Known differences between the model and the kernel that are intended, each with its reason.
/// A differing pair of lines is accepted only if an entry explains it. Empty means the two agree
/// everywhere the generator reaches.
pub const BY_DESIGN: &[ByDesign] = &[];

/// One accepted difference.
pub struct ByDesign {
    pub name: &'static str,
    pub reason: &'static str,
    /// Whether this entry explains why `lean` and `rust` differ for the same position of `ops`.
    pub explains: fn(ops: &[Op], lean: &str, rust: &str) -> bool,
}

/// Who makes a generated operation: a user by its number, the system (which holds no role), or an
/// anonymous caller (which holds `public` only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    User(u64),
    System,
    Anonymous,
}

impl Caller {
    /// The principal as an `op` line names it.
    fn token(self) -> String {
        match self {
            Caller::User(n) => n.to_string(),
            Caller::System => "system".to_string(),
            Caller::Anonymous => "anon".to_string(),
        }
    }
}

/// One generated operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Op {
    pub tenant: u64,
    pub principal: Caller,
    /// The object type the intent names: `1` is `item`, `2` is `part`, and any other number names
    /// a type the tenant does not have. Rendered as `type <n>` unless it is `1`.
    pub object: u64,
    pub kind: OpKind,
}

/// The intents, with fields as `(field number, value token)` (`s:oak`, `i:3`, `b:1`, `n`,
/// `l:r:1,r:2`, `s*65537:x`, ...) and expected versions as `v=<n>` when present.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpKind {
    Create(Vec<(u64, String)>),
    /// A replacement: the model reads it as a patch that names every field of the fixture.
    Update(u64, Option<u64>, Vec<(u64, String)>),
    /// A patch: only the named fields change, `n` removes one.
    Patch(u64, Option<u64>, Vec<(u64, String)>),
    Delete(u64, Option<u64>),
    Require(u64, bool),
    /// A unique field: the field number, and whether the change is strict.
    Unique(u64, bool),
    /// A composite unique constraint over these fields.
    Composite(Vec<u64>),
    /// Create a tenant: its owner and its name token.
    CreateTenant(u64, String),
    /// Give a user a role, or take one away.
    Assign(u64, u64),
    Revoke(u64, u64),
    /// Define a team: its number and its label token.
    Team(u64, String),
    /// Give a team a role, take one away; add a user to a team, remove one.
    TeamAssign(u64, u64),
    TeamRevoke(u64, u64),
    TeamAdd(u64, u64),
    TeamRemove(u64, u64),
    /// Issue an invitation: its number, the address token, the roles (in the kernel's order) and the
    /// time of the request.
    Invite(u64, String, Vec<u64>, u64),
    InviteRevoke(u64),
    /// Redeem an invitation at a time, with a verified address token if the caller has one.
    Redeem(u64, u64, Option<String>),
    /// A schema change in general (slice 11): the expected version, whether the change is strict (`lax` otherwise),
    /// and its changes, each as the tokens of one change (`define-field <type> <field> <ty> [required] [unique]
    /// [default=<v>]`, `remove-field <type> <field>`, `define-type <type>`, `remove-type <type> [delete]`).
    Schema(Option<u64>, bool, Vec<String>),
    /// Read records (slice 10): the object type is the op's `object`, and the ids are tokens, `n` or `a-b`.
    ReadRecords(Vec<String>),
    /// Read a named query: its name and its arguments (`k=v`, `page=<n>`, `cursor=<c>`).
    ReadQuery(String, Vec<String>),
    /// Search (slice 10, second half): the object type (`*` or a number) and the words of the query, then its
    /// arguments (`page=<n>`).
    ReadSearch(String, Vec<String>, Vec<String>),
    /// An aggregate of a named query: the query, the aggregate, and its arguments (`filter.<n>=<v>`, `k=v`).
    ReadAggregate(String, String, Vec<String>),
    /// A setting write (slice 12): the setting's name, its level token, the team it names (when any), the
    /// expected revision (`v=<n>`, omitted when zero) and the value token, or `None` for a removal.
    SettingWrite(String, String, Option<u64>, u64, Option<String>),
    /// A setting read (slice 12): the setting's name.
    SettingRead(String),
}

pub type Scenario = Vec<Op>;

fn fields_text(fields: &[(u64, String)]) -> String {
    fields
        .iter()
        .map(|(f, v)| format!(" {f}={v}"))
        .collect::<String>()
}

fn expected_text(expected: Option<u64>) -> String {
    expected.map_or(String::new(), |v| format!(" v={v}"))
}

impl Op {
    pub fn render(&self) -> String {
        let head = if self.object == 1 {
            format!("op {} {}", self.tenant, self.principal.token())
        } else {
            format!(
                "op {} {} type {}",
                self.tenant,
                self.principal.token(),
                self.object
            )
        };
        match &self.kind {
            OpKind::Create(f) => format!("{head} create{}", fields_text(f)),
            OpKind::Update(id, v, f) => {
                format!("{head} update {id}{}{}", expected_text(*v), fields_text(f))
            }
            OpKind::Patch(id, v, f) => {
                format!("{head} patch {id}{}{}", expected_text(*v), fields_text(f))
            }
            OpKind::Delete(id, v) => format!("{head} delete {id}{}", expected_text(*v)),
            OpKind::Require(f, strict) => {
                format!(
                    "{head} require {f} {}",
                    if *strict { "strict" } else { "lax" }
                )
            }
            OpKind::Unique(f, strict) => {
                format!(
                    "{head} unique {f} {}",
                    if *strict { "strict" } else { "lax" }
                )
            }
            OpKind::Composite(fs) => format!(
                "{head} composite{}",
                fs.iter().map(|f| format!(" {f}")).collect::<String>()
            ),
            OpKind::Schema(expected, strict, changes) => format!(
                "{head} schema{} {}{}",
                expected_text(*expected),
                if *strict { "strict" } else { "lax" },
                if changes.is_empty() {
                    String::new()
                } else {
                    format!(" {}", changes.join(" ; "))
                }
            ),
            OpKind::CreateTenant(owner, name) => format!("{head} create-tenant {owner} {name}"),
            OpKind::Assign(user, role) => format!("{head} assign {user} {role}"),
            OpKind::Revoke(user, role) => format!("{head} revoke {user} {role}"),
            OpKind::Team(team, label) => format!("{head} team {team} {label}"),
            OpKind::TeamAssign(team, role) => format!("{head} team-assign {team} {role}"),
            OpKind::TeamRevoke(team, role) => format!("{head} team-revoke {team} {role}"),
            OpKind::TeamAdd(team, user) => format!("{head} team-add {team} {user}"),
            OpKind::TeamRemove(team, user) => format!("{head} team-remove {team} {user}"),
            OpKind::Invite(inv, address, roles, now) => {
                let roles = if roles.is_empty() {
                    "-".to_string()
                } else {
                    roles
                        .iter()
                        .map(u64::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                };
                format!("{head} invite {inv} {address} {roles} @{now}")
            }
            OpKind::InviteRevoke(inv) => format!("{head} invite-revoke {inv}"),
            OpKind::Redeem(inv, now, verified) => format!(
                "{head} redeem {inv} @{now}{}",
                verified
                    .as_ref()
                    .map_or(String::new(), |v| format!(" verified={v}"))
            ),
            OpKind::ReadRecords(ids) => format!(
                "read {} {} records {}{}",
                self.tenant,
                self.principal.token(),
                self.object,
                ids.iter().map(|id| format!(" {id}")).collect::<String>()
            ),
            OpKind::ReadQuery(name, args) => format!(
                "read {} {} query {name}{}",
                self.tenant,
                self.principal.token(),
                args.iter().map(|arg| format!(" {arg}")).collect::<String>()
            ),
            OpKind::ReadSearch(ty, words, args) => format!(
                "read {} {} search {ty}{}{}",
                self.tenant,
                self.principal.token(),
                words
                    .iter()
                    .map(|word| format!(" {word}"))
                    .collect::<String>(),
                args.iter().map(|arg| format!(" {arg}")).collect::<String>()
            ),
            OpKind::ReadAggregate(query, aggregate, args) => format!(
                "read {} {} aggregate {query} {aggregate}{}",
                self.tenant,
                self.principal.token(),
                args.iter().map(|arg| format!(" {arg}")).collect::<String>()
            ),
            OpKind::SettingWrite(name, level, team, expected, value) => format!(
                "{head} setting {name} {level}{}{}{}",
                team.map_or(String::new(), |t| format!(" team={t}")),
                if *expected == 0 {
                    String::new()
                } else {
                    format!(" v={expected}")
                },
                value
                    .as_ref()
                    .map_or(" remove".to_string(), |v| format!(" {v}"))
            ),
            OpKind::SettingRead(name) => format!(
                "read {} {} setting {name}",
                self.tenant,
                self.principal.token()
            ),
        }
    }
}

/// Render scenarios named `<prefix>0`, `<prefix>1`, ... into one scenario file.
pub fn render(prefix: &str, scenarios: &[Scenario]) -> String {
    let mut out = String::new();
    for (n, ops) in scenarios.iter().enumerate() {
        out.push_str(&format!("scenario {prefix}{n}\n"));
        for op in ops {
            out.push_str(&op.render());
            out.push('\n');
        }
        out.push_str("end\n");
    }
    out
}

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

/// The kernel's string bound, in bytes (`limits::MAX_STRING_BYTES`): a string of this length is
/// accepted, one byte more is refused.
const STRING_LIMIT: u64 = 65_536;
/// The kernel's list bound (`limits::MAX_LIST_LEN`): a list of this many items is accepted, one
/// more is refused.
const LIST_LIMIT: u64 = 1_000;
// The kernel's record bound is 262,144 bytes of the record's JSON encoding
// (`limits::MAX_RECORD_BYTES`). The model measures a lower bound of that encoding, so a record
// whose model size is above it is refused by both, and one whose model size is below it by more
// than the encoding's overhead (field names, ids, timestamps, a few bytes per value; at most a
// few hundred bytes for the handful of values these records hold) is accepted by both. The
// generator keeps every record it builds out of the band between the two: the sets in
// `straddle` have model sizes of 196,608, 245,536 and 257,536 (below) or 265,536 and more
// (above).

/// A word for a short string.
fn word(rng: &mut Lcg) -> &'static str {
    rng.pick(&["oak", "ash", "elm", "fir", "yew"])
}

/// A reference to one of the first few records (which exist, or were deleted, so chains form and
/// both delete behaviours fire), sometimes to an id never issued.
fn loose_ref(rng: &mut Lcg) -> String {
    if rng.below(8) == 0 {
        format!("r:{}", 90 + rng.below(5))
    } else {
        format!("r:{}", 1 + rng.below(6))
    }
}

/// An element that is not a reference: of a type no list field of the fixture holds, or `n`.
fn wrong_elem(rng: &mut Lcg, want_string: bool) -> String {
    match rng.below(3) {
        0 => "n".to_string(),
        1 if want_string => format!("i:{}", rng.below(9)),
        1 => format!("s:{}", word(rng)),
        _ => "b:1".to_string(),
    }
}

/// A list value for `field` (0 and 8 hold references, 7 strings): mostly well formed and short,
/// now and then empty, with a repeated element, or with an element of the wrong type.
fn list_token(rng: &mut Lcg, field: u64, elem: &mut dyn FnMut(&mut Lcg) -> String) -> String {
    let n = match rng.below(8) {
        0 => 0,
        1 => 1,
        2 | 3 => 2,
        4 => 3,
        _ => 1 + rng.below(4),
    };
    let mut items: Vec<String> = (0..n).map(|_| elem(rng)).collect();
    if n >= 2 && rng.below(6) == 0 {
        items[1] = items[0].clone();
    }
    if n >= 1 && rng.below(12) == 0 {
        let at = rng.below(n) as usize;
        items[at] = wrong_elem(rng, field == 7);
    }
    format!("l:{}", items.join(","))
}

fn value_token(rng: &mut Lcg, field: u64) -> String {
    // Mostly the right type for the field, sometimes the wrong one, sometimes null.
    let kind = match (rng.below(10), field) {
        (0, _) => 3,
        (1, _) => rng.below(3),
        (_, 0) | (_, 8) => 5,
        (_, 1) => 0,
        (_, 2) | (_, 6) => 1,
        (_, 3) => 2,
        (_, 7) => 6,
        (_, 10) => 0,
        // Field 9 is the caller's principal by default, a string (slice 5).
        (_, 9) => 0,
        _ => 4,
    };
    // Field 1 is refused by a constraint when it is `forbidden` (slice 5), so that is sometimes
    // written.
    let forbidden = field == 1 && rng.below(12) == 0;
    match kind {
        0 if forbidden => "s:forbidden".to_string(),
        0 => format!("s:{}", word(rng)),
        1 => format!("i:{}", rng.below(40) as i64 - 5),
        2 => format!("b:{}", rng.below(2)),
        4 => loose_ref(rng),
        5 => list_token(rng, field, &mut |rng| loose_ref(rng)),
        6 => list_token(rng, field, &mut |rng| format!("s:{}", word(rng))),
        _ => "n".to_string(),
    }
}

fn field_list(rng: &mut Lcg) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    for field in [1u64, 2, 3] {
        // Field 1 is required, so it is usually given.
        let include = if field == 1 {
            rng.below(8) != 0
        } else {
            rng.below(2) == 0
        };
        if include {
            out.push((field, value_token(rng, field)));
        }
    }
    for (field, odds) in [(0u64, 4), (4, 3), (5, 4), (7, 4), (8, 4), (10, 3), (9, 4)] {
        if rng.below(odds) == 0 {
            out.push((field, value_token(rng, field)));
        }
    }
    // Field 6 has a default: usually left out, so a create gets it; sometimes named, with a value,
    // a wrong type or `n` (which suppresses the default).
    if rng.below(5) == 0 {
        out.push((6, value_token(rng, 6)));
    }
    // Field 13 is declared by no type: an unknown field, refused as invalid. (Field 9 played this
    // part until slice 5 declared it.)
    if rng.below(25) == 0 {
        out.push((UNDECLARED_FIELD, "i:1".to_string()));
    }
    out.sort_by_key(|(f, _)| *f);
    out
}

/// A field no fixture type declares.
const UNDECLARED_FIELD: u64 = 13;

/// A patch: one to three fields, each often removed (`n`).
fn patch_fields(rng: &mut Lcg) -> Vec<(u64, String)> {
    let mut out: Vec<(u64, String)> = Vec::new();
    for _ in 0..(1 + rng.below(3)) {
        let field = rng.below(9);
        if out.iter().any(|(f, _)| *f == field) {
            continue;
        }
        let token = if rng.below(4) == 0 {
            "n".to_string()
        } else {
            value_token(rng, field)
        };
        out.push((field, token));
    }
    if rng.below(40) == 0 {
        out.push((UNDECLARED_FIELD, "i:1".to_string()));
    }
    out.sort_by_key(|(f, _)| *f);
    out
}

/// The field of a `unique`: a scalar, sometimes a list (refused), sometimes a field whose
/// default makes every record without a value agree.
fn unique_field(rng: &mut Lcg) -> u64 {
    match rng.below(10) {
        0 => rng.pick(&[0u64, 7]),
        1..=3 => rng.pick(&[1u64, 2, 3]),
        4 => UNDECLARED_FIELD + rng.below(4),
        _ => rng.pick(&[10u64, 4, 6]),
    }
}

/// The fields of a composite constraint: usually two or three declared scalars, sometimes a list,
/// a repeated field, an undeclared one, or too few.
fn composite_fields(rng: &mut Lcg) -> Vec<u64> {
    match rng.below(10) {
        0 => vec![2, 0],
        1 => vec![2, 2],
        2 => vec![1, 9],
        3 => vec![2],
        4 | 5 => vec![2, 3],
        6 => vec![1, 10],
        7 => vec![2, 3, 10],
        _ => vec![3, 10, 4, 6],
    }
}

/// The field of a `require`: half the time one of the reference fields, sometimes a list, sometimes a field the type
/// does not declare (the change then defines it, as the kernel does).
fn require_field(rng: &mut Lcg) -> u64 {
    match rng.below(10) {
        0..=3 => 4 + rng.below(2),
        4 => rng.pick(&[0u64, 7, 8]),
        5 => UNDECLARED_FIELD + rng.below(4),
        _ => 1 + rng.below(3),
    }
}

/// A reference to a record the sequence made (so far), now and then to one it has not made yet
/// or an id never issued.
fn aimed_target(rng: &mut Lcg, created: u64) -> u64 {
    match rng.below(8) {
        0 => 90 + rng.below(5),
        1 => created + 1 + rng.below(3),
        _ => 1 + rng.below(created.max(1)),
    }
}

/// Set reference fields (4, 5 and the lists 0 and 8) on `fields` to records the sequence made (so
/// far), now and then to one it has not made yet or an id never issued, and remember what is
/// referred to.
fn aimed_refs(
    rng: &mut Lcg,
    fields: &mut Vec<(u64, String)>,
    created: u64,
    referred: &mut Vec<u64>,
) {
    fields.retain(|(f, _)| !matches!(f, 0 | 4 | 5 | 8));
    for field in [4u64, 5] {
        if rng.below(3) == 0 {
            continue;
        }
        let target = aimed_target(rng, created);
        referred.push(target);
        fields.push((field, format!("r:{target}")));
    }
    for field in [0u64, 8] {
        if rng.below(3) == 0 {
            continue;
        }
        let mut items = Vec::new();
        for _ in 0..(1 + rng.below(3)) {
            let target = aimed_target(rng, created);
            referred.push(target);
            items.push(format!("r:{target}"));
        }
        fields.push((field, format!("l:{}", items.join(","))));
    }
    fields.sort_by_key(|(f, _)| *f);
}

/// How the generator built a bound-straddling field set.
struct Straddle {
    fields: Vec<(u64, String)>,
    /// The record is large enough (model size above 100,000) that a later patch could land it in
    /// the band where the model and the kernel measure it differently.
    big: bool,
}

/// Fields that sit on a size bound: a string, a list or a record one below, at, or one above
/// what the kernel accepts (the string and list bounds are exact on both sides), or a record
/// whose size is far enough from the record bound on either side that the model's lower bound
/// and the kernel's encoding agree. `whole_record` allows the record-size sets, which are only
/// sound when the fields named are the whole record (a create, or an update, which replaces).
fn straddle(rng: &mut Lcg, created: u64, referred: &mut Vec<u64>, whole_record: bool) -> Straddle {
    let around = |rng: &mut Lcg, limit: u64| limit - 1 + rng.below(3);
    let mut fields = vec![(1u64, "s:oak".to_string())];
    let mut big = false;
    // The record-size sets are costly (a quarter megabyte each, and they make the tenant's later
    // patches small), so they are a quarter of the whole-record choices.
    let which = if whole_record && rng.below(4) == 0 {
        match rng.below(4) {
            0 => 5,
            1 | 2 => 6,
            _ => 8,
        }
    } else {
        rng.below(5)
    };
    match which {
        // The string bound on the required string.
        0 | 1 => {
            fields = vec![(1, format!("s*{}:x", around(rng, STRING_LIMIT)))];
        }
        // The list bound on a list of strings.
        2 => fields.push((7, format!("l*{}:s:a", around(rng, LIST_LIMIT)))),
        // The string bound on an element of a list of strings.
        3 => {
            let len = around(rng, STRING_LIMIT);
            fields.push((7, format!("l:s:oak,s*{len}:y")));
        }
        // The list bound on the two lists of references. The target is a record that probably
        // exists, so the length is what decides.
        4 => {
            let target = 1 + rng.below(created.max(1));
            referred.push(target);
            let field = if rng.below(2) == 0 { 0 } else { 8 };
            fields.push((field, format!("l*{}:r:{target}", around(rng, LIST_LIMIT))));
        }
        // The record bound, far above: a string and strings in a list, or the list alone.
        5 => {
            big = true;
            if rng.below(3) == 0 {
                // Five strings of at least 60,000 bytes: model size 300,000 or more.
                let len = rng.pick(&[60_000u64, 65_536]);
                fields = vec![(7, format!("l*5:s*{len}:x"))];
            } else {
                // The required string at its bound and four or five of at least 50,000: model
                // size 265,536 or more.
                let n = 4 + rng.below(2);
                let len = rng.pick(&[50_000u64, 60_000, 65_536]);
                fields = vec![
                    (1, format!("s*{STRING_LIMIT}:x")),
                    (7, format!("l*{n}:s*{len}:x")),
                ];
            }
        }
        // The record bound, far below: model size 196,608, 245,536 or 257,536.
        6 => {
            big = true;
            let (k, len) = rng.pick(&[(2u64, 65_536u64), (3, 60_000), (3, 64_000)]);
            fields = vec![
                (1, format!("s*{STRING_LIMIT}:x")),
                (7, format!("l*{k}:s*{len}:x")),
            ];
        }
        // The record bound with several big strings and the lists of references together.
        _ => {
            big = true;
            fields = vec![
                (1, format!("s*{STRING_LIMIT}:x")),
                (7, format!("l*4:s*{}:x", 50_000 + 1_000 * rng.below(15))),
                (0, "l*500:r:1".to_string()),
            ];
            referred.push(1);
        }
    }
    fields.sort_by_key(|(f, _)| *f);
    Straddle { fields, big }
}

/// The version the sequence guesses a record is at, as an expected version: usually the right
/// one, otherwise one off or far off.
fn expected_version(rng: &mut Lcg, guess: u64) -> Option<u64> {
    match rng.below(6) {
        0 | 1 => Some(guess),
        2 => Some(guess + 1),
        3 => Some(guess.saturating_sub(1)),
        4 => Some(1 + rng.below(8)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// The membership intents (slice 7)
// ---------------------------------------------------------------------------------------------

/// The user numbers from this one up are the kernel's reserved principal ids (`lean::RESERVED_FROM`,
/// the model's `reservedFrom`).
const RESERVED_USER: u64 = 1_000_000;

/// Fourteen days in milliseconds: how long an invitation lasts (ADM-1082).
const FORTNIGHT_MS: u64 = 14 * 24 * 60 * 60 * 1000;

/// The owner of a tenant: the fixture's for `1` and `2`, and the owner the generator gives tenant `3`.
fn home_of(tenant: u64) -> u64 {
    match tenant {
        2 => 200,
        3 => 300,
        _ => 100,
    }
}

/// What a sequence has probably done to the membership of the tenants, so that aimed operations name
/// teams and invitations that exist (an approximation, as for records: a refused operation still counts).
#[derive(Clone, Default)]
struct Members {
    /// Tenant 3 was (probably) created.
    created3: bool,
    /// How many teams `team1`.. the sequence defined, per tenant.
    teams: [u64; 4],
    /// The invitations made, per tenant: the address token and the time of the request.
    invites: [Vec<(String, u64)>; 4],
}

/// The kernel orders a set of roles by the role id's text: `editor` (2), `owner` (0), `public` (1),
/// `role<n>`, `writer` (3). The model takes a role list in the order given, so the generator writes the
/// roles of an invitation in the kernel's order, and a refusal for two bad roles is the same on both
/// sides.
fn role_order(role: u64) -> String {
    match role {
        0 => "owner".to_string(),
        1 => "public".to_string(),
        2 => "editor".to_string(),
        3 => "writer".to_string(),
        n => format!("role{n}"),
    }
}

fn sorted_roles(mut roles: Vec<u64>) -> Vec<u64> {
    roles.sort_by_key(|r| role_order(*r));
    roles.dedup();
    roles
}

/// A user an assignment or a team names: the tenant's own users, strangers, now and then a reserved id.
fn member_user(rng: &mut Lcg, home: u64) -> u64 {
    match rng.below(14) {
        0..=2 => home,
        3..=5 => home + 1,
        6 | 7 => home + 2,
        8 => home + 3,
        9 | 10 => 150,
        11 => 160,
        12 => home + 4,
        _ => RESERVED_USER + rng.below(2),
    }
}

/// A role number: owner, editor and writer mostly, now and then `public` or one the tenant does not define.
fn member_role(rng: &mut Lcg) -> u64 {
    match rng.below(14) {
        0..=2 => 0,
        3 => 1,
        4..=7 => 2,
        8..=11 => 3,
        _ => 9,
    }
}

/// The caller of a membership operation: mostly the tenant's owner or editor, who may manage members,
/// otherwise a writer, a stranger, the system, an anonymous caller, another tenant's owner or a
/// reserved id.
fn member_caller(rng: &mut Lcg, home: u64) -> Caller {
    match rng.below(22) {
        0..=8 => Caller::User(home),
        9..=13 => Caller::User(home + 1),
        14 | 15 => Caller::User(home + 2),
        16 => Caller::User(999),
        17 => Caller::System,
        18 => Caller::Anonymous,
        19 => Caller::User(RESERVED_USER + 1),
        _ => Caller::User(if home == 100 { 201 } else { 101 }),
    }
}

/// The address an invitation names: with capitals or spaces to be normalised, or with no `@`.
fn invite_address(rng: &mut Lcg) -> String {
    match rng.below(7) {
        0 => "s:ann@x.org".to_string(),
        1 => "s:Bob@X.com".to_string(),
        2 => "s:_cy@x.org_".to_string(),
        3 => "s:dave".to_string(),
        4 => "s:EVE@x.org".to_string(),
        5 => "s:_@_".to_string(),
        _ => "s:fay@x.org".to_string(),
    }
}

/// An address that is the same once normalised (trimmed and lowercased), written differently, or another.
fn verified_address(rng: &mut Lcg, named: &str) -> Option<String> {
    match rng.below(10) {
        0 => None,
        1 | 2 => Some("s:zed@x.org".to_string()),
        3 | 4 => Some(named.to_uppercase().replacen("S:", "s:", 1)),
        5 => Some(format!("s:_{}_", &named[2..])),
        _ => Some(named.to_string()),
    }
}

/// The roles an invitation offers.
fn invite_roles(rng: &mut Lcg) -> Vec<u64> {
    sorted_roles(match rng.below(10) {
        0 => vec![],
        1 => vec![0],
        2 => vec![1],
        3 => vec![9],
        4 | 5 => vec![2],
        6 | 7 => vec![3],
        8 => vec![2, 3],
        _ => vec![1, 9],
    })
}

/// The time of a redemption, around the expiry of an invitation issued at `at`.
fn redeem_time(rng: &mut Lcg, at: u64) -> u64 {
    match rng.below(8) {
        0 => at + FORTNIGHT_MS - 1,
        1 => at + FORTNIGHT_MS,
        2 => at + FORTNIGHT_MS + 1,
        3 => at.saturating_sub(1),
        _ => at + 1 + rng.below(1000),
    }
}

/// One iteration of the generator that is about membership: a creation of tenant 3, a scripted
/// sequence that builds a lifecycle (an invitation made, redeemed, revoked, expired; an owner team;
/// the last owner), or one random operation.
fn member_ops(rng: &mut Lcg, ms: &mut Members) -> Vec<Op> {
    let tenant = match rng.below(10) {
        0..=2 => 3,
        3 | 4 => 2,
        _ => 1,
    };
    let t = tenant as usize;
    let home = home_of(tenant);
    let mut ops = Vec::new();
    let mk = |principal: Caller, kind: OpKind| Op {
        tenant,
        principal,
        object: 1,
        kind,
    };
    // Tenant 3 does not exist until the system creates it. Creations are mostly right; some name a
    // blank name, a reserved owner or a tenant that exists, or are made by someone else.
    if tenant == 3 && (!ms.created3 || rng.below(10) == 0) {
        let owner = match rng.below(10) {
            0 => RESERVED_USER,
            1 => 301,
            _ => 300,
        };
        let name = if rng.below(8) == 0 { "s:_" } else { "s:acme" };
        let (at, who) = match rng.below(12) {
            0 => (3, Caller::User(300)),
            1 => (3, Caller::Anonymous),
            2 => (rng.pick(&[1u64, 2]), Caller::System),
            3 => (3, Caller::User(RESERVED_USER + 1)),
            _ => (3, Caller::System),
        };
        ms.created3 |= at == 3 && who == Caller::System && owner < RESERVED_USER && name != "s:_";
        ops.push(Op {
            tenant: at,
            principal: who,
            object: 1,
            kind: OpKind::CreateTenant(owner, name.to_string()),
        });
        return ops;
    }
    // Scripted sequences.
    if rng.below(8) == 0 {
        let inv = 1 + ms.invites[t].len() as u64;
        let address = ["s:ann@x.org", "s:Bob@X.com", "s:_cy@x.org_"][rng.below(3) as usize];
        let at = 1_000 + 1_000 * rng.below(5);
        let issuer = if rng.below(3) == 0 { home + 1 } else { home };
        let roles = sorted_roles(match rng.below(4) {
            0 => vec![2],
            1 => vec![3],
            2 => vec![2, 3],
            _ => vec![0],
        });
        let invite = |i: u64, who: u64, at: u64| {
            mk(
                Caller::User(who),
                OpKind::Invite(i, address.to_string(), roles.clone(), at),
            )
        };
        let redeem = |i: u64, who: u64, at: u64, verified: Option<&str>| {
            mk(
                Caller::User(who),
                OpKind::Redeem(i, at, verified.map(str::to_string)),
            )
        };
        match rng.below(7) {
            // The invitation is made, redeemed, redeemed again and revoked too late.
            0 => {
                ops.push(invite(inv, issuer, at));
                ops.push(redeem(inv, 150, at + 5, Some(address)));
                ops.push(redeem(inv, 160, at + 6, Some(address)));
                ops.push(mk(Caller::User(home), OpKind::InviteRevoke(inv)));
            }
            // The invitation is revoked, then redeemed.
            1 => {
                ops.push(invite(inv, issuer, at));
                ops.push(mk(Caller::User(issuer), OpKind::InviteRevoke(inv)));
                ops.push(mk(Caller::User(issuer), OpKind::InviteRevoke(inv)));
                ops.push(redeem(inv, 150, at + 5, Some(address)));
            }
            // The expiry: the last millisecond and the first one after.
            2 => {
                ops.push(invite(inv, issuer, at));
                ops.push(redeem(inv, 150, at + FORTNIGHT_MS, Some(address)));
                ops.push(redeem(inv, 150, at + FORTNIGHT_MS - 1, Some(address)));
            }
            // The issuer loses the right to manage members before the invitation is redeemed.
            3 => {
                ops.push(invite(inv, home + 1, at));
                ops.push(mk(Caller::User(home), OpKind::Revoke(home + 1, 2)));
                ops.push(redeem(inv, 150, at + 5, Some(address)));
            }
            // The wrong address, no verified address, then the right one.
            4 => {
                ops.push(invite(inv, issuer, at));
                ops.push(redeem(inv, 150, at + 5, Some("s:zed@x.org")));
                ops.push(redeem(inv, 150, at + 5, None));
                ops.push(redeem(inv, 150, at + 5, Some(address)));
            }
            // An owner team: the owner can drop its own role while the team keeps an owner.
            5 => {
                let team = 1 + ms.teams[t].min(2);
                ms.teams[t] = ms.teams[t].max(team);
                ops.push(mk(
                    Caller::User(home),
                    OpKind::Team(team, "s:ops".to_string()),
                ));
                ops.push(mk(Caller::User(home), OpKind::TeamAssign(team, 0)));
                ops.push(mk(Caller::User(home), OpKind::TeamAdd(team, 150)));
                ops.push(mk(Caller::User(home), OpKind::Revoke(home, 0)));
                ops.push(mk(Caller::User(150), OpKind::TeamRemove(team, 150)));
                ops.push(mk(Caller::User(home), OpKind::TeamRemove(team, 150)));
            }
            // The last owner: the owner takes its role away, with and without a second owner.
            _ => {
                ops.push(mk(Caller::User(home), OpKind::Revoke(home, 0)));
                ops.push(mk(Caller::User(home), OpKind::Assign(home + 1, 0)));
                ops.push(mk(Caller::User(home), OpKind::Revoke(home, 0)));
                ops.push(mk(Caller::User(home + 1), OpKind::Revoke(home + 1, 0)));
            }
        }
        if ops.iter().any(|op| matches!(op.kind, OpKind::Invite(..))) {
            ms.invites[t].push((address.to_string(), at));
        }
        return ops;
    }
    // One random operation.
    let caller = member_caller(rng, home);
    let team = |rng: &mut Lcg, ms: &Members| {
        if rng.below(10) == 0 {
            7
        } else {
            1 + rng.below(ms.teams[t].clamp(1, 3))
        }
    };
    let kind = match rng.below(26) {
        0..=3 => OpKind::Assign(member_user(rng, home), member_role(rng)),
        4..=6 => OpKind::Revoke(member_user(rng, home), member_role(rng)),
        7 | 8 => {
            let n = 1 + rng.below(3);
            ms.teams[t] = ms.teams[t].max(n);
            let label = if rng.below(5) == 0 { "s:_" } else { "s:ops" };
            OpKind::Team(n, label.to_string())
        }
        9 | 10 => OpKind::TeamAssign(team(rng, ms), member_role(rng)),
        11 => OpKind::TeamRevoke(team(rng, ms), member_role(rng)),
        12..=14 => OpKind::TeamAdd(team(rng, ms), member_user(rng, home)),
        15 => OpKind::TeamRemove(team(rng, ms), member_user(rng, home)),
        16..=19 => {
            let inv = if rng.below(6) == 0 {
                1 + rng.below(3)
            } else {
                1 + ms.invites[t].len() as u64
            };
            let address = invite_address(rng);
            let at = 1_000 + 1_000 * rng.below(5);
            if inv as usize == ms.invites[t].len() + 1 {
                ms.invites[t].push((address.clone(), at));
            }
            OpKind::Invite(inv, address, invite_roles(rng), at)
        }
        20 => OpKind::InviteRevoke(1 + rng.below(3)),
        _ => {
            let made = ms.invites[t].len() as u64;
            if made == 0 || rng.below(8) == 0 {
                OpKind::Redeem(
                    1 + rng.below(3),
                    1_000 + rng.below(5_000),
                    Some("s:ann@x.org".to_string()),
                )
            } else {
                let inv = 1 + rng.below(made);
                let (named, at) = ms.invites[t][inv as usize - 1].clone();
                let verified = verified_address(rng, &named);
                OpKind::Redeem(inv, redeem_time(rng, at), verified)
            }
        }
    };
    // A redemption is made by whoever redeems, not by a manager: a user who may have no role.
    let caller = if matches!(kind, OpKind::Redeem(..)) {
        match rng.below(10) {
            0 => Caller::System,
            1 => Caller::Anonymous,
            2 => Caller::User(home),
            3 => Caller::User(RESERVED_USER + 1),
            4 | 5 => Caller::User(160),
            _ => Caller::User(150),
        }
    } else {
        caller
    };
    ops.push(mk(caller, kind));
    ops
}

/// One sequence, deterministic in `seed`.
/// One ticket operation (slice 8, the model's type `4`): a create, a patch of the state or the votes, a
/// replacement, or a delete, by an owner, an editor or a writer of tenant 1 or 2. The states are the
/// workflow's (`open`, `closed`, `archived`, `escalated`), with a state no transition names, a state
/// spelt in capitals, and `null`; the votes are small numbers, `0`, `null` and absent.
fn ticket_op(rng: &mut Lcg) -> Op {
    let tenant = if rng.below(4) == 0 { 2 } else { 1 };
    let home = home_of(tenant);
    let principal = match rng.below(16) {
        0..=7 => home,
        8..=10 => home + 1,
        11..=13 => home + 2,
        14 => home + 3,
        _ => 999,
    };
    let state = |rng: &mut Lcg| -> String {
        match rng.below(10) {
            0..=2 => "s:open".to_string(),
            3 | 4 => "s:closed".to_string(),
            5 => "s:archived".to_string(),
            6 => "s:escalated".to_string(),
            7 => "s:bogus".to_string(),
            8 => "s:OPEN".to_string(),
            _ => "n".to_string(),
        }
    };
    let votes = |rng: &mut Lcg| -> String {
        match rng.below(8) {
            0 => "n".to_string(),
            1 => "i:0".to_string(),
            2 => "i:3".to_string(),
            3 => "i:7".to_string(),
            _ => format!("i:{}", rng.below(3) + 1),
        }
    };
    let kind = match rng.below(10) {
        0..=2 => {
            let mut fields = Vec::new();
            if rng.below(8) != 0 {
                fields.push((1, state(rng)));
            }
            if rng.below(2) == 0 {
                fields.push((2, votes(rng)));
            }
            if rng.below(5) == 0 {
                fields.push((3, "s:note".to_string()));
            }
            OpKind::Create(fields)
        }
        3..=6 => {
            let mut fields = Vec::new();
            if rng.below(3) != 0 {
                fields.push((1, state(rng)));
            }
            if rng.below(2) == 0 || fields.is_empty() {
                fields.push((2, votes(rng)));
            }
            OpKind::Patch(1 + rng.below(3), expected_version(rng, 1), fields)
        }
        7 => OpKind::Update(
            1 + rng.below(3),
            expected_version(rng, 1),
            vec![(1, state(rng)), (2, votes(rng))],
        ),
        _ => OpKind::Delete(1 + rng.below(3), expected_version(rng, 1)),
    };
    Op {
        tenant,
        principal: Caller::User(principal),
        object: 4,
        kind,
    }
}

pub fn generate(seed: u64) -> Scenario {
    let mut rng = Lcg(seed ^ 0x9e37_79b9_7f4a_7c15);
    // Warm the generator: nearby seeds would otherwise start alike.
    for _ in 0..4 {
        rng.below(2);
    }
    let mut ops = Vec::new();
    // What the sequence has probably built so far, per tenant (an approximation: a refused
    // create still counts), so that aimed operations target records that exist and the ones
    // other records refer to. `versions` guesses each record's version, for expected versions.
    let mut created = [0u64; 4];
    let mut referred: [Vec<u64>; 4] = Default::default();
    let mut versions: [Vec<u64>; 4] = Default::default();
    // Whether the tenant may hold a record near the record bound: patches there stay small.
    let mut big = [false; 4];
    // What the sequence has probably done to the tenants' membership (slice 7).
    let mut members = Members::default();
    for _ in 0..(8 + rng.below(20)) {
        // One iteration in four is about membership: a creation of tenant 3, a scripted sequence, or
        // one random operation.
        if rng.below(4) == 0 {
            ops.extend(member_ops(&mut rng, &mut members));
            continue;
        }
        // One iteration in six is a ticket operation (slice 8): the tickets' workflow decides it.
        if rng.below(6) == 0 {
            ops.push(ticket_op(&mut rng));
            continue;
        }
        let tenant = match rng.below(12) {
            0 => 3,
            1 | 2 => 2,
            _ => 1,
        };
        let home = home_of(tenant);
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
        let t = tenant as usize;
        // One iteration in ten is a short aimed sequence by the owner, so that the outcomes that
        // need two or three operations to line up (a delete refused because a record still refers
        // to its target; an over-limit write to a record that exists) do not depend on the
        // random operations happening to line up.
        if tenant != 3 && rng.below(10) == 0 {
            let first = created[t] + 1;
            let owner = |kind| Op {
                tenant,
                principal: Caller::User(home),
                object: 1,
                kind,
            };
            ops.push(owner(OpKind::Create(vec![(
                1,
                format!("s:{}", word(&mut rng)),
            )])));
            created[t] += 1;
            versions[t].push(1);
            if rng.below(2) == 0 {
                // A record refers to the first, then the first is deleted. Mostly the reference
                // restricts the delete (field 5, or the list 0); otherwise it is cleared (4, 8).
                let (field, token) = match rng.below(8) {
                    0..=2 => (5, format!("r:{first}")),
                    3..=5 => (0, format!("l:r:{first}")),
                    6 => (4, format!("r:{first}")),
                    _ => (8, format!("l:r:{first}")),
                };
                referred[t].push(first);
                ops.push(owner(OpKind::Create(vec![
                    (1, format!("s:{}", word(&mut rng))),
                    (field, token),
                ])));
                created[t] += 1;
                versions[t].push(1);
                let expected = (rng.below(4) == 0).then_some(1);
                ops.push(owner(OpKind::Delete(first, expected)));
            } else {
                // A write that is over a limit by one, to the record just made: a replacement or a
                // patch. It is refused whatever else the record holds, so it never depends on
                // how near the record bound the record is.
                let over = match rng.below(3) {
                    0 => (1, format!("s*{}:x", STRING_LIMIT + 1)),
                    1 => (7, format!("l*{}:s:a", LIST_LIMIT + 1)),
                    _ => (7, format!("l:s:oak,s*{}:y", STRING_LIMIT + 1)),
                };
                let kind = if rng.below(2) == 0 {
                    OpKind::Update(first, None, vec![(1, "s:ash".to_string()), over])
                } else {
                    OpKind::Patch(first, (rng.below(3) == 0).then_some(1), vec![over])
                };
                ops.push(owner(kind));
                versions[t][first as usize - 1] += 1;
            }
            continue;
        }
        // Half the operations on a real tenant are aimed: made by someone who may write, to
        // build references, to delete what is referred to, to refer to what is gone.
        let aimed = tenant != 3 && rng.below(2) == 0;
        let mut principal = principal;
        // An aimed update, patch or delete names a record the sequence probably made.
        let kind = match rng.below(22) {
            0..=6 => {
                let mut fields = field_list(&mut rng);
                if aimed {
                    principal = home + rng.below(2);
                    aimed_refs(&mut rng, &mut fields, created[t], &mut referred[t]);
                }
                if aimed && rng.below(8) == 0 {
                    principal = home + rng.below(2);
                    let s = straddle(&mut rng, created[t], &mut referred[t], true);
                    fields = s.fields;
                    big[t] |= s.big;
                }
                if tenant != 3 {
                    created[t] += 1;
                    versions[t].push(1);
                }
                OpKind::Create(fields)
            }
            7..=10 => {
                let mut id = 1 + rng.below(6);
                let mut fields = field_list(&mut rng);
                let mut expected = None;
                if aimed && created[t] > 0 {
                    principal = home + rng.below(2);
                    id = 1 + rng.below(created[t]);
                    aimed_refs(&mut rng, &mut fields, created[t], &mut referred[t]);
                    if rng.below(8) == 0 {
                        let s = straddle(&mut rng, created[t], &mut referred[t], true);
                        fields = s.fields;
                        big[t] |= s.big;
                    }
                    let guess = versions[t][id as usize - 1];
                    if rng.below(2) == 0 {
                        expected = expected_version(&mut rng, guess);
                    }
                    versions[t][id as usize - 1] += 1;
                } else if rng.below(6) == 0 {
                    expected = Some(1 + rng.below(4));
                }
                OpKind::Update(id, expected, fields)
            }
            11..=14 => {
                let mut id = 1 + rng.below(6);
                let mut fields = patch_fields(&mut rng);
                let mut expected = None;
                if aimed && created[t] > 0 {
                    principal = home + rng.below(2);
                    id = 1 + rng.below(created[t]);
                    if rng.below(2) == 0 {
                        aimed_refs(&mut rng, &mut fields, created[t], &mut referred[t]);
                        // Keep the fields patch_fields chose among the references some of the time.
                        fields.truncate(1 + rng.below(3) as usize);
                    }
                    if rng.below(6) == 0 && !big[t] {
                        let s = straddle(&mut rng, created[t], &mut referred[t], false);
                        fields = s.fields;
                    }
                    let guess = versions[t][id as usize - 1];
                    if rng.below(2) == 0 {
                        expected = expected_version(&mut rng, guess);
                    }
                    versions[t][id as usize - 1] += 1;
                } else if rng.below(6) == 0 {
                    expected = Some(1 + rng.below(4));
                }
                if big[t] {
                    // A record near the record bound could be pushed over it by the encoding's
                    // overhead alone: only patches that shrink or barely grow it.
                    fields = vec![if rng.below(2) == 0 {
                        (2, "i:5".to_string())
                    } else {
                        (3, "n".to_string())
                    }];
                }
                OpKind::Patch(id, expected, fields)
            }
            15..=17 => {
                let mut id = 1 + rng.below(6);
                let mut expected = None;
                if aimed {
                    principal = home + rng.below(2);
                    if !referred[t].is_empty() {
                        id = referred[t][rng.below(referred[t].len() as u64) as usize];
                    }
                    if id >= 1 && (id as usize) <= versions[t].len() && rng.below(2) == 0 {
                        expected = expected_version(&mut rng, versions[t][id as usize - 1]);
                    }
                } else if rng.below(6) == 0 {
                    expected = Some(1 + rng.below(4));
                }
                OpKind::Delete(id, expected)
            }
            18 => OpKind::Require(require_field(&mut rng), true),
            19 => OpKind::Unique(unique_field(&mut rng), rng.below(3) != 0),
            20 => OpKind::Composite(composite_fields(&mut rng)),
            _ => OpKind::Require(require_field(&mut rng), false),
        };
        // Schema changes are mostly attempted by the owner, so both strict outcomes occur.
        let principal = if matches!(
            kind,
            OpKind::Require(..) | OpKind::Unique(..) | OpKind::Composite(..)
        ) && rng.below(4) != 0
        {
            home
        } else {
            principal
        };
        // Most intents name `item`; one in four names `part` (whose fields are its own), and one
        // in twelve names a type the tenant does not have, so the unknown-type refusals occur.
        let object = match rng.below(12) {
            0 => 9,
            1..=3 => 2,
            _ => 1,
        };
        // A `require` or `unique` names any field: one the object type declares, or one it lacks, which the change
        // defines as an optional boolean field, in the model (`ensureField`) as in the kernel (slice 11). Half the
        // time a `require` or `unique` on a part names one of its own fields (1 to 4), as before.
        let kind = match (object, kind) {
            (2, OpKind::Require(f, strict)) if rng.below(2) == 0 => {
                OpKind::Require(1 + f % 4, strict)
            }
            (2, OpKind::Unique(f, strict)) if rng.below(2) == 0 => {
                OpKind::Unique(1 + f % 4, strict)
            }
            (_, kind) => kind,
        };
        // One operation in twenty-four is the system's, one in twelve an anonymous caller's, so the
        // system and the `public` role are exercised on every type.
        let caller = match rng.below(24) {
            0 => Caller::System,
            1 | 2 => Caller::Anonymous,
            _ => Caller::User(principal),
        };
        ops.push(Op {
            tenant,
            principal: caller,
            object,
            kind,
        });
    }
    with_reads(seed, with_schema(seed, ops))
}

// ---------------------------------------------------------------------------------------------
// The schema change in general (slice 11)
// ---------------------------------------------------------------------------------------------

/// The object types a schema change defines and removes. They are single digits so that the kernel's order of the
/// types (by name, `type5` to `type8`, after `item`, `part` and `ticket`) is the model's (by number). The fixture's
/// own types are never removed: a policy, a role permission and the fixture's queries name them.
const NEW_TYPES: [u64; 4] = [5, 6, 7, 8];

/// The fields the fixture's item declares that no constraint, workflow, policy, query, field read rule or search
/// reads. The kernel refuses a change that removes or retypes a field one of these dependents reads
/// (`dependencies::impact`, and the schema check), and the model holds the dependents that live in the tenant
/// (constraints, workflows, composite unique constraints, policies) but not the queries, read rules and search
/// declarations of the fixture, which the model keeps beside the tenant (`ReadSpec`). A generated change removes or
/// redefines with another type only fields no such dependent reads, so that the two agree on every change the
/// generator makes. Reachable again when the model holds the read declarations (see the slice 11 review note).
const FREE_ITEM_FIELDS: [u64; 7] = [0, 4, 5, 6, 8, 11, 12];

/// The part's field that no dependent reads (the part's constraint reads fields 1, 2, 4 and 5, its query field 4).
const FREE_PART_FIELD: u64 = 3;

/// Fields a schema change adds to a type (the fixture declares none of them), and may later redefine or remove.
const ADDED_FIELDS: [u64; 4] = [13, 14, 15, 16];

/// A field type token of a `schema` line (the model's `parseTy`): mostly a scalar, a reference to an item, a part or
/// one of the types a change defines, or a list of one.
fn type_token(rng: &mut Lcg, scalar_only: bool) -> String {
    let span = if scalar_only { 8 } else { 10 };
    match rng.below(span) {
        0 | 1 => "int".to_string(),
        2 | 3 => "str".to_string(),
        4 => "bool".to_string(),
        5 | 6 => format!(
            "ref:{}:{}",
            rng.pick(&[1u64, 1, 2, 5, 6, 9]),
            rng.pick(&["r", "s"])
        ),
        7 => "ref:1:s".to_string(),
        8 => format!("list:{}", type_token(rng, true)),
        _ => "list:int".to_string(),
    }
}

/// A default of a field of type `ty` as a value token: usually one of its type, sometimes of another.
fn default_token(rng: &mut Lcg, ty: &str) -> String {
    if rng.below(6) == 0 {
        return "s:oak".to_string();
    }
    if ty == "int" {
        format!("i:{}", rng.below(9))
    } else if ty == "str" {
        format!("s:{}", word(rng))
    } else if ty == "bool" {
        format!("b:{}", rng.below(2))
    } else if ty.starts_with("ref:") {
        format!("r:{}", 1 + rng.below(3))
    } else {
        "l:".to_string()
    }
}

/// The options after a field's type: `required`, `unique`, `default=<v>`, each now and then.
fn field_options(rng: &mut Lcg, ty: &str) -> String {
    let mut out = String::new();
    if rng.below(4) == 0 {
        out.push_str(" required");
    }
    if rng.below(4) == 0 {
        out.push_str(" unique");
    }
    if rng.below(4) == 0 {
        out.push_str(&format!(" default={}", default_token(rng, ty)));
    }
    out
}

/// A type and a field a generated change may define or remove: a field the change adds, a free field of the fixture
/// (see [`FREE_ITEM_FIELDS`]), or a field of a type a change defines.
fn change_target(rng: &mut Lcg) -> (u64, u64) {
    match rng.below(10) {
        0..=2 => (1, rng.pick(&ADDED_FIELDS)),
        3 => (1, rng.pick(&FREE_ITEM_FIELDS)),
        4 => (2, rng.pick(&ADDED_FIELDS)),
        5 => (2, FREE_PART_FIELD),
        6 => (4, rng.pick(&ADDED_FIELDS)),
        _ => (rng.pick(&NEW_TYPES), 1 + rng.below(4)),
    }
}

/// A list of changes for one `schema` line, one to three of them. A type the list removes is the only type it names
/// in that change and no other change of the list names it: the kernel rewrites and deletes the records of a type the
/// tenant has when it is removed, and defined again or changed in the same list the two disagree on the events (the
/// model holds that a type removed and defined again is refused only when a field of a type names it).
fn change_list(rng: &mut Lcg) -> Vec<String> {
    let mut changes: Vec<String> = Vec::new();
    let mut removed: Vec<u64> = Vec::new();
    let mut mentioned: Vec<u64> = Vec::new();
    for _ in 0..(1 + rng.below(3)) {
        let (change, named, is_removal) = match rng.below(10) {
            0..=4 => {
                let (ty, field) = change_target(rng);
                let ty_text = type_token(rng, false);
                let options = field_options(rng, &ty_text);
                (
                    format!("define-field {ty} {field} {ty_text}{options}"),
                    ty,
                    false,
                )
            }
            5..=6 => {
                let (ty, field) = change_target(rng);
                (format!("remove-field {ty} {field}"), ty, false)
            }
            7 => {
                let ty = rng.pick(&NEW_TYPES);
                (format!("define-type {ty}"), ty, false)
            }
            _ => {
                let ty = rng.pick(&NEW_TYPES);
                let delete = if rng.below(2) == 0 { " delete" } else { "" };
                (format!("remove-type {ty}{delete}"), ty, true)
            }
        };
        if removed.contains(&named) || (is_removal && mentioned.contains(&named)) {
            continue;
        }
        if is_removal {
            removed.push(named);
        }
        mentioned.push(named);
        changes.push(change);
    }
    changes
}

/// A schema change op of `tenant`.
fn schema_op(
    tenant: u64,
    principal: Caller,
    expected: Option<u64>,
    strict: bool,
    changes: Vec<String>,
) -> Op {
    Op {
        tenant,
        principal,
        object: 1,
        kind: OpKind::Schema(expected, strict, changes),
    }
}

/// How many schema-changing operations the owner of `tenant` made in `before`, so that a guess of the schema version
/// is often right (a refused operation still counts, so it is only a guess).
fn schema_guess(before: &[Op], tenant: u64) -> u64 {
    let owner = Caller::User(home_of(tenant));
    let made = before
        .iter()
        .filter(|op| {
            op.tenant == tenant
                && op.principal == owner
                && matches!(
                    op.kind,
                    OpKind::Require(..)
                        | OpKind::Unique(..)
                        | OpKind::Composite(..)
                        | OpKind::Schema(..)
                )
        })
        .count() as u64;
    if tenant == 3 {
        1 + made
    } else {
        3 + made
    }
}

/// One random schema change of slice 11: mostly the owner's, in one of the tenants, with an expected version now and
/// then, strict or lax, and a short list of changes.
fn random_schema_op(rng: &mut Lcg, before: &[Op]) -> Op {
    let tenant = match rng.below(10) {
        0 | 1 => 3,
        2 | 3 => 2,
        _ => 1,
    };
    let home = home_of(tenant);
    let principal = match rng.below(12) {
        0 => Caller::System,
        1 => Caller::Anonymous,
        2 => Caller::User(home + 2),
        3 => Caller::User(home + 1),
        4 => Caller::User(999),
        _ => Caller::User(home),
    };
    let expected = match rng.below(8) {
        0 => Some(schema_guess(before, tenant)),
        1 => Some(1 + rng.below(8)),
        _ => None,
    };
    let strict = rng.below(5) < 3;
    let changes = match rng.below(40) {
        0 => Vec::new(),
        1 => vec!["define-type 5".to_string(); 201],
        _ => change_list(rng),
    };
    schema_op(tenant, principal, expected, strict, changes)
}

/// A short scripted sequence by the owner of a tenant that reaches the decisions a single random change seldom does:
/// the records of a type a change removes or rewrites, a required or unique field defined over records, a field
/// retyped under a composite constraint, the expected version, a reference to a type a change removes, the refusals
/// of a list that is empty, over the limit, not the owner's or not valid.
fn schema_script(rng: &mut Lcg) -> Vec<Op> {
    let tenant = if rng.below(4) == 0 { 2 } else { 1 };
    let owner = Caller::User(home_of(tenant));
    let ty = rng.pick(&NEW_TYPES);
    let on = |changes: Vec<String>, strict: bool| schema_op(tenant, owner, None, strict, changes);
    let create_in = |object: u64, fields: Vec<(u64, String)>| Op {
        tenant,
        principal: owner,
        object,
        kind: OpKind::Create(fields),
    };
    let item = |fields: Vec<(u64, String)>| create_in(1, fields);
    let patch_item = |id: u64, fields: Vec<(u64, String)>| Op {
        tenant,
        principal: owner,
        object: 1,
        kind: OpKind::Patch(id, None, fields),
    };
    match rng.below(7) {
        // A type with records, then a change that rewrites or removes them.
        0 => {
            let unique = if rng.below(2) == 0 { " unique" } else { "" };
            let mut ops = vec![on(
                vec![
                    format!("define-type {ty}"),
                    format!("define-field {ty} 1 str required{unique}"),
                    format!("define-field {ty} 2 int"),
                ],
                true,
            )];
            for _ in 0..(1 + rng.below(3)) {
                ops.push(create_in(
                    ty,
                    vec![
                        (1, format!("s:{}", word(rng))),
                        (2, format!("i:{}", rng.below(5))),
                    ],
                ));
            }
            match rng.below(5) {
                0 => ops.push(on(vec![format!("remove-field {ty} 2")], rng.below(2) == 0)),
                1 => {
                    ops.push(on(vec![format!("remove-type {ty}")], rng.below(2) == 0));
                    ops.push(on(vec![format!("remove-type {ty} delete")], true));
                }
                2 => ops.push(on(
                    vec![format!("remove-type {ty} delete")],
                    rng.below(2) == 0,
                )),
                3 => {
                    ops.push(on(vec![format!("define-field {ty} 3 int required")], true));
                    ops.push(on(vec![format!("define-field {ty} 3 int required")], false));
                }
                _ => ops.push(on(
                    vec![
                        format!("define-field {ty} 2 str"),
                        format!("remove-field {ty} 1"),
                    ],
                    rng.below(2) == 0,
                )),
            }
            ops
        }
        // A required field, then a retyped one, over the item's records.
        1 => {
            let mut ops = vec![item(vec![(1, format!("s:{}", word(rng)))])];
            if rng.below(2) == 0 {
                ops.push(item(vec![(1, format!("s:{}", word(rng)))]));
            }
            ops.push(on(vec!["define-field 1 13 int required".to_string()], true));
            ops.push(on(
                vec!["define-field 1 13 int required".to_string()],
                false,
            ));
            ops.push(patch_item(1, vec![(13, "i:3".to_string())]));
            ops.push(on(vec!["define-field 1 13 str".to_string()], true));
            ops.push(on(vec!["define-field 1 13 str".to_string()], false));
            ops.push(on(vec!["remove-field 1 13".to_string()], rng.below(2) == 0));
            ops
        }
        // A composite constraint over added fields, then a change that retypes or removes one of them.
        2 => vec![
            on(
                vec![
                    "define-field 1 13 int".to_string(),
                    "define-field 1 14 str".to_string(),
                ],
                true,
            ),
            Op {
                tenant,
                principal: owner,
                object: 1,
                kind: OpKind::Composite(vec![13, 14]),
            },
            on(vec!["define-field 1 13 str".to_string()], rng.below(2) == 0),
            on(vec!["remove-field 1 14".to_string()], rng.below(2) == 0),
            on(vec!["define-field 1 13 int required".to_string()], true),
            on(
                vec![
                    "remove-field 1 13".to_string(),
                    "remove-field 1 14".to_string(),
                ],
                true,
            ),
        ],
        // The expected version: the schema of a fixture tenant starts at version 3.
        3 => {
            let expect = |v: u64, name: String| {
                schema_op(
                    tenant,
                    owner,
                    Some(v),
                    true,
                    vec![format!("define-type {name}")],
                )
            };
            vec![
                expect(3, "5".to_string()),
                expect(3, "6".to_string()),
                expect(4, "6".to_string()),
                expect(9, "7".to_string()),
            ]
        }
        // A reference to a type, then the removal of the type.
        4 => vec![
            on(
                vec![
                    format!("define-type {ty}"),
                    format!("define-field {ty} 1 str"),
                    format!("define-field 1 13 ref:{ty}:s"),
                ],
                true,
            ),
            on(vec![format!("remove-type {ty}")], true),
            on(
                vec![format!("remove-field 1 13"), format!("remove-type {ty}")],
                rng.below(2) == 0,
            ),
        ],
        // Duplicates, then a unique field.
        5 => vec![
            on(vec!["define-field 1 13 str".to_string()], true),
            item(vec![(1, "s:a".to_string()), (13, "s:dup".to_string())]),
            item(vec![(1, "s:b".to_string()), (13, "s:dup".to_string())]),
            on(vec!["define-field 1 13 str unique".to_string()], true),
            on(vec!["define-field 1 13 str unique".to_string()], false),
            on(
                vec!["define-field 1 13 str unique required".to_string()],
                true,
            ),
        ],
        // The refusals of a list.
        _ => {
            let who = match rng.below(4) {
                0 => Caller::System,
                1 => Caller::Anonymous,
                2 => Caller::User(home_of(tenant) + 2),
                _ => Caller::User(999),
            };
            let invalid = match rng.below(6) {
                0 => vec!["define-field 1 13 ref:9:r".to_string()],
                1 => vec!["define-field 1 13 list:int unique".to_string()],
                2 => vec!["define-field 1 13 list:list:int".to_string()],
                3 => vec!["remove-field 1 15".to_string()],
                4 => vec!["define-field 99 1 int".to_string()],
                _ => vec!["remove-type 7".to_string()],
            };
            vec![
                schema_op(tenant, who, None, true, vec![format!("define-type {ty}")]),
                on(Vec::new(), true),
                on(vec!["define-type 5".to_string(); 201], true),
                on(invalid, rng.below(2) == 0),
                schema_op(
                    tenant,
                    owner,
                    Some(1 + rng.below(2)),
                    true,
                    vec!["define-type 5".to_string()],
                ),
            ]
        }
    }
}

/// The scenario with schema changes of slice 11 placed among its operations: a random one now and then, and the
/// scripted sequences of [`schema_script`], from a stream of their own so the operations the rest of the generator
/// draws are the same with or without them.
fn with_schema(seed: u64, mut ops: Scenario) -> Scenario {
    let mut rng = Lcg(seed ^ 0x7363_6865_6d61_3131);
    for _ in 0..4 {
        rng.below(2);
    }
    let rounds = match rng.below(6) {
        0 | 1 => 0,
        2 | 3 => 1,
        4 => 2,
        _ => 3,
    };
    for _ in 0..rounds {
        let from = ops.len() / 4;
        let at = from + rng.below((ops.len() - from) as u64 + 1) as usize;
        let batch = if rng.below(3) == 0 {
            vec![random_schema_op(&mut rng, &ops[..at])]
        } else {
            schema_script(&mut rng)
        };
        for (k, op) in batch.into_iter().enumerate() {
            ops.insert(at + k, op);
        }
    }
    ops
}

/// A read of slice 10, drawn from the stream `rng`. The reads of a scenario come from a stream of their
/// own, so the operations the rest of the generator draws are the same with or without them. Mostly the
/// tenants and principals of the fixture, sometimes tenant `3` (which may not exist yet), the system and an
/// anonymous caller. A records read names a few ids (missing, invisible and visible ones occur together:
/// the ids of other types and of records a principal may not read are among them), now and then 200
/// ids (the bound) or 201 (over it), and now and then a type the tenant lacks. A query read names one of
/// the fixture's queries, or one it lacks, with a parameter it declares or one it does not, a page size
/// (none, 1 to 3, 0, 201) and a cursor (none, an offset, a position after a record).
fn read_op(rng: &mut Lcg) -> Op {
    let tenant = match rng.below(12) {
        0 => 3,
        1 | 2 => 2,
        _ => 1,
    };
    let home = home_of(tenant);
    let principal = match rng.below(20) {
        0 => Caller::System,
        1 | 2 => Caller::Anonymous,
        3 => Caller::User(999),
        4 => Caller::User(if home == 100 { 201 } else { 101 }),
        5..=7 => Caller::User(home),
        8..=10 => Caller::User(home + 1),
        11..=15 => Caller::User(home + 2),
        _ => Caller::User(home + 3),
    };
    if rng.below(5) < 2 {
        let object = match rng.below(10) {
            0 => 9,
            1..=4 => 1,
            5 | 6 => 2,
            _ => 4,
        };
        let ids = match rng.below(14) {
            0 => vec!["1-201".to_string()],
            1 => vec!["1-200".to_string()],
            _ => (0..1 + rng.below(5))
                .map(|_| (1 + rng.below(12)).to_string())
                .collect(),
        };
        return Op {
            tenant,
            principal,
            object,
            kind: OpKind::ReadRecords(ids),
        };
    }
    let name = match rng.below(14) {
        0 => "nope",
        1..=4 => "items_by_rank",
        5..=8 => "items_not_elm",
        9 | 10 => "parts_ranked",
        _ => "tickets_open",
    };
    let mut args: Vec<String> = Vec::new();
    // `tag` is declared by `items_not_elm` only; any other name is refused.
    match rng.below(8) {
        0 => args.push("tag=s:x".to_string()),
        1 => args.push("bogus=i:1".to_string()),
        _ => {}
    }
    match rng.below(10) {
        0 => args.push("page=0".to_string()),
        1 => args.push("page=201".to_string()),
        2..=5 => args.push(format!("page={}", 1 + rng.below(3))),
        _ => {}
    }
    match rng.below(10) {
        0..=2 => args.push(format!("cursor=o{}", rng.below(5))),
        3 => args.push(format!("cursor=a{}", 1 + rng.below(6))),
        _ => {}
    }
    Op {
        tenant,
        principal,
        object: 1,
        kind: OpKind::ReadQuery(name.to_string(), args),
    }
}

/// The operations with a few reads placed among them, mostly in the later part of the sequence where
/// there are records to read.
fn with_reads(seed: u64, mut ops: Scenario) -> Scenario {
    let mut rng = Lcg(seed ^ 0x7265_6164_735f_3130);
    for _ in 0..4 {
        rng.below(2);
    }
    for _ in 0..rng.below(7) {
        let from = ops.len() / 3;
        let at = from + rng.below((ops.len() - from) as u64 + 1) as usize;
        let read = read_op(&mut rng);
        ops.insert(at, read);
    }
    ops
}

/// The principal of a read of a tenant whose home is `home`: the system, an anonymous caller, a user the
/// tenant lacks, or one of its members.
fn read_caller(rng: &mut Lcg, home: u64) -> Caller {
    match rng.below(20) {
        0 => Caller::System,
        1 | 2 => Caller::Anonymous,
        3 => Caller::User(999),
        4 => Caller::User(if home == 100 { 201 } else { 101 }),
        5..=7 => Caller::User(home),
        8..=10 => Caller::User(home + 1),
        11..=15 => Caller::User(home + 2),
        _ => Caller::User(home + 3),
    }
}

/// A search (slice 10, second half): an object type or all of them, a query of known words, none, too many
/// or too long, a page size (none, 0, 201, or 1 to 3).
fn search_op(rng: &mut Lcg) -> Op {
    let tenant = match rng.below(12) {
        0 => 3,
        1 | 2 => 2,
        _ => 1,
    };
    let principal = read_caller(rng, home_of(tenant));
    let object = match rng.below(8) {
        0 => "*",
        1 => "9",
        2 => "2",
        3 => "4",
        _ => "1",
    };
    let words: Vec<String> = match rng.below(12) {
        0 => Vec::new(),
        1 => ["a", "b", "c", "d", "e", "f", "g", "h", "i"]
            .iter()
            .map(|w| w.to_string())
            .collect(),
        2 => vec!["a".repeat(201)],
        _ => (0..1 + rng.below(2))
            .map(|_| word(rng).to_string())
            .collect(),
    };
    let mut args: Vec<String> = Vec::new();
    match rng.below(10) {
        0 => args.push("page=0".to_string()),
        1 => args.push("page=201".to_string()),
        2..=5 => args.push(format!("page={}", 1 + rng.below(3))),
        _ => {}
    }
    Op {
        tenant,
        principal,
        object: 1,
        kind: OpKind::ReadSearch(object.to_string(), words, args),
    }
}

/// An aggregate (slice 10, second half): a declared aggregate of one of the fixture's queries, or an
/// undeclared one or query, with a filter or a parameter the query declares or does not.
fn aggregate_op(rng: &mut Lcg) -> Op {
    let tenant = match rng.below(12) {
        0 => 3,
        1 | 2 => 2,
        _ => 1,
    };
    let principal = read_caller(rng, home_of(tenant));
    let (query, aggregate) = match rng.below(14) {
        0 => ("nope", "count"),
        1 => ("items_by_rank", "nope"),
        2 | 3 => ("items_by_rank", "bounded_count"),
        4 | 5 => ("items_by_rank", "count_by_flag"),
        6 => ("items_by_rank", "top_rank"),
        7 => ("items_by_rank", "sum_rank"),
        8 => ("items_by_rank", "min_rank"),
        9 => ("items_by_rank", "max_rank"),
        10 => ("items_not_elm", "count"),
        11 => ("tickets_open", "max_note"),
        12 => ("parts_ranked", "max"),
        _ => ("tickets_open", "sum_votes"),
    };
    let mut args: Vec<String> = Vec::new();
    match rng.below(8) {
        0 => args.push("tag=s:x".to_string()),
        1 => args.push("bogus=i:1".to_string()),
        2 => args.push("filter.3=b:1".to_string()),
        3 => args.push("filter.1=s:oak".to_string()),
        _ => {}
    }
    Op {
        tenant,
        principal,
        object: 1,
        kind: OpKind::ReadAggregate(query.to_string(), aggregate.to_string(), args),
    }
}

/// The searches and aggregates placed among the operations of a scenario, from a stream of their own.
fn with_queries(seed: u64, mut ops: Scenario) -> Scenario {
    let mut rng = Lcg(seed ^ 0x7175_6572_795f_3130);
    for _ in 0..4 {
        rng.below(2);
    }
    for _ in 0..3 + rng.below(6) {
        let from = ops.len() / 3;
        let at = from + rng.below((ops.len() - from) as u64 + 1) as usize;
        let read = if rng.below(2) == 0 {
            search_op(&mut rng)
        } else {
            aggregate_op(&mut rng)
        };
        ops.insert(at, read);
    }
    ops
}

/// The setting name a setting operation picks (slice 12): one of the fixture's five, the platform's
/// `member_names`, or a name no tenant defines (`unknown_object`).
fn setting_name(rng: &mut Lcg) -> &'static str {
    match rng.below(14) {
        0 => "nope",
        1 | 2 => "notify",
        3 | 4 => "density",
        5 | 6 => "label",
        7 => "banner",
        8 | 9 => "mode",
        _ => "member_names",
    }
}

/// The level a setting write names (slice 12): the platform (always refused for a user), the tenant, a team
/// or the person.
fn setting_level(rng: &mut Lcg) -> &'static str {
    match rng.below(10) {
        0 => "platform",
        1..=3 => "tenant",
        4..=6 => "team",
        _ => "person",
    }
}

/// The value token a setting write carries, or `None` for a removal (slice 12). Mostly a value of the setting's
/// type and within its allowed set or range, sometimes out of them, of the wrong type, or null.
fn setting_value(rng: &mut Lcg, name: &str) -> Option<String> {
    if rng.below(5) == 0 {
        return None;
    }
    if rng.below(12) == 0 {
        return Some("n".to_string());
    }
    if rng.below(10) == 0 {
        let wrong = match name {
            "notify" => "s:x",
            "density" => "s:3",
            _ => "i:1",
        };
        return Some(wrong.to_string());
    }
    let values: &[&str] = match name {
        "notify" => &["b:0", "b:1"],
        "density" => &["i:0", "i:3", "i:9", "i:10", "i:-1"],
        "label" => &["s:x", "s:a_b"],
        "banner" => &["s:hi", "s:none"],
        "mode" => &["s:plain", "s:bold", "s:italic"],
        "member_names" => &["s:members", "s:everyone", "s:nobody", "s:anyone"],
        _ => &["s:x", "i:1", "b:1"],
    };
    Some(rng.pick(values).to_string())
}

/// The subject an override of a setting names, as a key of the guess (slice 12).
fn setting_subject(level: &str, team: Option<u64>, principal: Caller) -> String {
    match level {
        "tenant" => "tenant".to_string(),
        "team" => format!("team{}", team.unwrap_or(0)),
        "person" => format!("p{}", principal.token()),
        _ => "platform".to_string(),
    }
}

/// What the settings generator expects of the overrides it has made so far (slice 12), as if every write it
/// makes were accepted: the revision of each override held (by tenant, setting and subject) and the writes
/// made to each setting of each tenant (the setting's revision counts them). A guess: an expected revision is
/// the guessed one most of the time and another one sometimes, so accepted and refused writes both occur.
#[derive(Default)]
struct SettingGuess {
    held: std::collections::BTreeMap<String, u64>,
    writes: std::collections::BTreeMap<String, u64>,
}

/// A setting write (slice 12): a tenant, a caller (mostly the owner and the editor,
/// with the system, an anonymous caller and a user of another tenant as well), a setting name, a level, a team
/// (named at the team level, sometimes missing, and now and then at another level), an expected revision
/// (usually the guessed one) and a value or a removal.
fn setting_write_op(rng: &mut Lcg, guess: &mut SettingGuess) -> Op {
    let tenant = match rng.below(12) {
        0 => 3,
        1 | 2 => 2,
        _ => 1,
    };
    let home = home_of(tenant);
    let principal = match rng.below(20) {
        0 => Caller::System,
        1 | 2 => Caller::Anonymous,
        3 => Caller::User(999),
        4 => Caller::User(if home == 100 { 201 } else { 101 }),
        5..=9 => Caller::User(home),
        10..=13 => Caller::User(home + 1),
        14..=16 => Caller::User(home + 2),
        _ => Caller::User(home + 3),
    };
    let name = setting_name(rng);
    let level = setting_level(rng);
    // Teams 1 to 3: a team may not exist, which is a refusal.
    let team = if level == "team" {
        (rng.below(10) != 0).then(|| 1 + rng.below(3))
    } else {
        (rng.below(8) == 0).then(|| 1 + rng.below(3))
    };
    let key = format!("{tenant}/{name}/{}", setting_subject(level, team, principal));
    let expected = if rng.below(6) == 0 {
        rng.below(4)
    } else {
        guess.held.get(&key).copied().unwrap_or(0)
    };
    let value = setting_value(rng, name);
    let revision = {
        let writes = guess.writes.entry(format!("{tenant}/{name}")).or_insert(0);
        *writes += 1;
        *writes
    };
    if value.is_some() {
        guess.held.insert(key, revision);
    } else {
        guess.held.remove(&key);
    }
    Op {
        tenant,
        principal,
        object: 1,
        kind: OpKind::SettingWrite(
            name.to_string(),
            level.to_string(),
            team,
            expected,
            value,
        ),
    }
}

/// A setting read (slice 12): a tenant, a caller (as for a record read) and a setting name.
fn setting_read_op(rng: &mut Lcg) -> Op {
    let tenant = match rng.below(12) {
        0 => 3,
        1 | 2 => 2,
        _ => 1,
    };
    let principal = read_caller(rng, home_of(tenant));
    Op {
        tenant,
        principal,
        object: 1,
        kind: OpKind::SettingRead(setting_name(rng).to_string()),
    }
}

/// A team defined by its owner and given a member, so that a team's override can resolve for that member
/// (slice 12). Teams 1 and 2 only, in tenant 1 or, now and then, tenant 2.
fn setting_team_setup(rng: &mut Lcg) -> Vec<Op> {
    let tenant = if rng.below(4) == 0 { 2 } else { 1 };
    let home = home_of(tenant);
    let team = 1 + rng.below(2);
    let member = home + rng.below(4);
    vec![
        Op {
            tenant,
            principal: Caller::User(home),
            object: 1,
            kind: OpKind::Team(team, "s:ops".to_string()),
        },
        Op {
            tenant,
            principal: Caller::User(home),
            object: 1,
            kind: OpKind::TeamAdd(team, member),
        },
    ]
}

/// The settings operations placed among the operations of a scenario, from a stream of their own (slice 12):
/// writes and reads of the settings, and a team set up now and then. The team comes early, so that the
/// writes and reads after it can see its member.
fn with_settings(seed: u64, mut ops: Scenario) -> Scenario {
    let mut rng = Lcg(seed ^ 0x7365_7474_696e_6773);
    for _ in 0..4 {
        rng.below(2);
    }
    if rng.below(2) == 0 {
        let at = rng.below(ops.len() as u64 / 3 + 1) as usize;
        for (k, op) in setting_team_setup(&mut rng).into_iter().enumerate() {
            ops.insert(at + k, op);
        }
    }
    let mut guess = SettingGuess::default();
    for _ in 0..4 + rng.below(8) {
        let from = ops.len() / 3;
        let at = from + rng.below((ops.len() - from) as u64 + 1) as usize;
        let op = if rng.below(4) == 0 {
            setting_read_op(&mut rng)
        } else {
            setting_write_op(&mut rng, &mut guess)
        };
        ops.insert(at, op);
    }
    ops
}

/// The scenario of case `seed` with its searches, aggregates and settings placed in (see `with_queries` and
/// `with_settings`). `generate` alone is what the fixture scenarios are made of, so the fixture does not change.
pub fn generate_reads(seed: u64) -> Scenario {
    with_settings(seed, with_queries(seed, generate(seed)))
}

/// The seed of case `index` in a run with base seed `base`.
pub fn case_seed(base: u64, index: usize) -> u64 {
    base.wrapping_add((index as u64).wrapping_mul(0x2545_f491_4f6c_dd1d))
}

/// The intent kinds `Main.lean`'s `parseOp` dispatches: the first word of each arm of the
/// `match rest with` that follows the `op <tenant> <principal>` prefix. Errors when an arm cannot
/// be read, so a new arm in an unfamiliar form cannot slip past unexercised.
pub fn lean_intents(main_source: &str) -> Result<Vec<String>, String> {
    let start = main_source
        .find("def parseOp")
        .ok_or("Main.lean has no parseOp")?;
    let body = &main_source[start..];
    let body = &body[body
        .find("match rest with")
        .ok_or("parseOp has no `match rest with`")?..];
    let mut kinds: Vec<String> = Vec::new();
    for line in body.lines().skip(1) {
        let line = line.trim();
        if line.starts_with("def ") {
            break;
        }
        let Some(arm) = line.strip_prefix('|') else {
            continue;
        };
        let arm = arm.trim_start();
        if arm.starts_with('_') {
            break;
        }
        let word = arm
            .strip_prefix('[')
            .unwrap_or(arm)
            .strip_prefix('"')
            .and_then(|rest| rest.split_once('"'))
            .map(|(word, _)| word.to_string())
            .ok_or_else(|| format!("cannot read the intent of this parseOp arm: {line}"))?;
        if !kinds.contains(&word) {
            kinds.push(word);
        }
    }
    if kinds.is_empty() {
        return Err("no intent arms found in parseOp".into());
    }
    Ok(kinds)
}

/// One scenario's output per entry, from its `scenario` line to its `end` line.
fn chunks(output: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    for line in output.lines() {
        if line.starts_with("scenario ") {
            out.push(Vec::new());
        }
        if let Some(last) = out.last_mut() {
            last.push(line.to_string());
        }
    }
    out
}

/// A difference that no `BY_DESIGN` entry explains.
#[derive(Clone, Debug)]
pub struct Difference {
    /// Index into the scenario list the batch was run on.
    pub scenario: usize,
    /// First differing line: `(line within the scenario's output, lean, rust)`.
    pub line: (usize, String, String),
    pub lean: Vec<String>,
    pub rust: Vec<String>,
}

fn unexplained(ops: &[Op], lean: &[String], rust: &[String]) -> Option<(usize, String, String)> {
    for i in 0..lean.len().max(rust.len()) {
        let (a, b) = (
            lean.get(i).map_or("<missing>", String::as_str),
            rust.get(i).map_or("<missing>", String::as_str),
        );
        if a != b && !BY_DESIGN.iter().any(|d| (d.explains)(ops, a, b)) {
            return Some((i, a.to_string(), b.to_string()));
        }
    }
    None
}

/// Run every scenario through Lean (one process) and the kernel; return the unexplained
/// differences, one per differing scenario.
pub fn diff_batch(lean_dir: &Path, scenarios: &[Scenario]) -> Result<Vec<Difference>, String> {
    diff_batch_counted(lean_dir, scenarios).map(|(diffs, _)| diffs)
}

/// How often each outcome occurred, by name: `ok`, `err <kind>`, and the same per intent
/// (`create ok`, `delete err still_referenced`, `require-strict ok`, ...), counted in the
/// output of the Lean model.
pub type Outcomes = std::collections::BTreeMap<String, usize>;

fn intent_name(op: &Op) -> &'static str {
    match op.kind {
        OpKind::Create(_) => "create",
        OpKind::Update(..) => "update",
        OpKind::Patch(..) => "patch",
        OpKind::Delete(..) => "delete",
        OpKind::Require(_, true) => "require-strict",
        OpKind::Require(_, false) => "require-lax",
        OpKind::Unique(_, true) => "unique-strict",
        OpKind::Unique(_, false) => "unique-lax",
        OpKind::Composite(_) => "composite",
        OpKind::Schema(..) => "schema",
        OpKind::CreateTenant(..) => "create-tenant",
        OpKind::Assign(..) => "assign",
        OpKind::Revoke(..) => "revoke",
        OpKind::Team(..) => "team",
        OpKind::TeamAssign(..) => "team-assign",
        OpKind::TeamRevoke(..) => "team-revoke",
        OpKind::TeamAdd(..) => "team-add",
        OpKind::TeamRemove(..) => "team-remove",
        OpKind::Invite(..) => "invite",
        OpKind::InviteRevoke(..) => "invite-revoke",
        OpKind::Redeem(..) => "redeem",
        OpKind::ReadRecords(_) => "read-records",
        OpKind::ReadQuery(..) => "read-query",
        OpKind::ReadSearch(..) => "read-search",
        OpKind::ReadAggregate(..) => "read-aggregate",
        OpKind::SettingWrite(_, _, _, _, Some(_)) => "setting",
        OpKind::SettingWrite(_, _, _, _, None) => "setting-remove",
        OpKind::SettingRead(_) => "read-setting",
    }
}

fn count_outcomes(scenarios: &[Scenario], lean: &[Vec<String>]) -> Outcomes {
    let mut counts = Outcomes::new();
    for (ops, out) in scenarios.iter().zip(lean) {
        // Line 0 is `scenario <name>`; then the lines of each operation, the first of which is its outcome.
        for (op, group) in ops.iter().zip(op_groups(out)) {
            let line = &group[0];
            *counts.entry(line.clone()).or_default() += 1;
            *counts
                .entry(format!("{} {line}", intent_name(op)))
                .or_default() += 1;
            // The shapes of a read's answer, besides its outcome.
            let mut shapes = read_shapes(&op.render(), group);
            shapes.extend(setting_shapes(&op.render(), group));
            for shape in shapes {
                if !shape.ends_with(" ok") {
                    *counts.entry(shape).or_default() += 1;
                }
            }
        }
    }
    counts
}

/// The outcome groups of an operation's output, cut as `lean::op_outcomes` cuts them, and in addition with
/// the lines of a setting read (`source`, `team`, `value` and `tenant-rev`, slice 12) in the group of their
/// read. `op_outcomes` does not know those lines, so it would start a group at each of them and the groups
/// would fall out of step with the operations. Its continuation prefixes are repeated here for that reason.
pub fn op_groups<S: AsRef<str>>(output: &[S]) -> Vec<&[S]> {
    const CONTINUATION: [&str; 14] = [
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
        "source ",
        "team ",
        "value ",
        "tenant-rev ",
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

/// The shapes of a setting read's answer as the coverage counts them (slice 12): for an accepted read, the
/// source its value came from (`read-setting ok source <source>`). Its refusals are counted by their own
/// outcome, as for every intent.
pub fn setting_shapes<S: AsRef<str>>(rendered: &str, group: &[S]) -> Vec<String> {
    let mut words = rendered.split_whitespace();
    if words.next() != Some("read") || words.nth(2) != Some("setting") {
        return Vec::new();
    }
    if group.first().map(|line| line.as_ref()) != Some("ok") {
        return Vec::new();
    }
    group
        .iter()
        .find_map(|line| line.as_ref().strip_prefix("source "))
        .map(|source| vec![format!("read-setting ok source {source}")])
        .unwrap_or_default()
}

/// [`diff_batch`] that also reports the outcomes counted in the Lean output.
pub fn diff_batch_counted(
    lean_dir: &Path,
    scenarios: &[Scenario],
) -> Result<(Vec<Difference>, Outcomes), String> {
    let text = render("c", scenarios);
    let path = std::env::temp_dir().join(format!(
        "strata-lean-diff-{}-{}.txt",
        std::process::id(),
        text.len()
    ));
    std::fs::write(&path, &text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let lean = run_model(lean_dir, &path.to_string_lossy());
    let _ = std::fs::remove_file(&path);
    let lean = chunks(&lean?);
    let rust = chunks(&kernel_output(&text));
    if lean.len() != scenarios.len() || rust.len() != scenarios.len() {
        return Err(format!(
            "expected {} scenarios, Lean printed {} and the kernel {}",
            scenarios.len(),
            lean.len(),
            rust.len()
        ));
    }
    let counts = count_outcomes(scenarios, &lean);
    let diffs = scenarios
        .iter()
        .enumerate()
        .filter_map(|(n, ops)| {
            unexplained(ops, &lean[n], &rust[n]).map(|line| Difference {
                scenario: n,
                line,
                lean: lean[n].clone(),
                rust: rust[n].clone(),
            })
        })
        .collect();
    Ok((diffs, counts))
}

/// The named fields of an operation, when it has any.
fn fields_mut(kind: &mut OpKind) -> Option<&mut Vec<(u64, String)>> {
    match kind {
        OpKind::Create(f) | OpKind::Update(_, _, f) | OpKind::Patch(_, _, f) => Some(f),
        OpKind::Delete(..)
        | OpKind::Require(..)
        | OpKind::Unique(..)
        | OpKind::Composite(..)
        | OpKind::Schema(..)
        | OpKind::CreateTenant(..)
        | OpKind::Assign(..)
        | OpKind::Revoke(..)
        | OpKind::Team(..)
        | OpKind::TeamAssign(..)
        | OpKind::TeamRevoke(..)
        | OpKind::TeamAdd(..)
        | OpKind::TeamRemove(..)
        | OpKind::Invite(..)
        | OpKind::InviteRevoke(..)
        | OpKind::Redeem(..)
        | OpKind::ReadRecords(_)
        | OpKind::ReadQuery(..)
        | OpKind::ReadSearch(..)
        | OpKind::ReadAggregate(..)
        | OpKind::SettingWrite(..)
        | OpKind::SettingRead(_) => None,
    }
}

/// Every sequence one step smaller than `ops`: one operation removed, one field removed, or one
/// expected version dropped.
fn reductions(ops: &Scenario) -> Vec<Scenario> {
    let mut out = Vec::new();
    for i in 0..ops.len() {
        let mut fewer = ops.clone();
        fewer.remove(i);
        out.push(fewer);
    }
    for i in 0..ops.len() {
        let mut probe = ops[i].kind.clone();
        let count = fields_mut(&mut probe).map_or(0, |f| f.len());
        for j in 0..count {
            let mut smaller = ops.clone();
            if let Some(f) = fields_mut(&mut smaller[i].kind) {
                f.remove(j);
            }
            out.push(smaller);
        }
        if let OpKind::Schema(_, _, changes) = &ops[i].kind {
            if changes.len() > 1 {
                for j in 0..changes.len() {
                    let mut smaller = ops.clone();
                    if let OpKind::Schema(_, _, c) = &mut smaller[i].kind {
                        c.remove(j);
                    }
                    out.push(smaller);
                }
            }
            if let OpKind::Schema(Some(_), strict, changes) = &ops[i].kind {
                let mut unversioned = ops.clone();
                unversioned[i].kind = OpKind::Schema(None, *strict, changes.clone());
                out.push(unversioned);
            }
        }
        let versioned = match &ops[i].kind {
            OpKind::Update(_, v, _) | OpKind::Patch(_, v, _) | OpKind::Delete(_, v) => v.is_some(),
            _ => false,
        };
        if versioned {
            let mut unversioned = ops.clone();
            if let OpKind::Update(_, v, _) | OpKind::Patch(_, v, _) | OpKind::Delete(_, v) =
                &mut unversioned[i].kind
            {
                *v = None;
            }
            out.push(unversioned);
        }
    }
    out
}

/// Delta-reduce a failing sequence: one Lean process per round, keep the first candidate that
/// still differs, stop when no candidate does.
pub fn shrink(lean_dir: &Path, failing: Scenario) -> Result<Scenario, String> {
    let mut current = failing;
    for _ in 0..500 {
        let candidates = reductions(&current);
        if candidates.is_empty() {
            break;
        }
        let diffs = diff_batch(lean_dir, &candidates)?;
        match diffs.first() {
            Some(d) => current = candidates[d.scenario].clone(),
            None => break,
        }
    }
    Ok(current)
}

/// Runs of at least this many cases must exercise every outcome kind (see [`check_coverage`]).
pub const COVERAGE_FROM_CASES: usize = 1000;

/// The fewest times each outcome kind must occur in such a run.
pub const MIN_OUTCOME_COUNT: usize = 20;

/// The errors the model can print, from the `showErr` function of `Main.lean`: the name after
/// each `=>`.
pub fn lean_error_names(main_source: &str) -> Result<Vec<String>, String> {
    let start = main_source
        .find("def showErr")
        .ok_or("Main.lean has no showErr")?;
    let names: Vec<String> = main_source[start..]
        .lines()
        .skip(1)
        .take_while(|l| l.trim_start().starts_with('|'))
        .filter_map(|l| {
            let (_, rest) = l.split_once("=>")?;
            Some(rest.trim().trim_matches('"').to_string())
        })
        .collect();
    if names.is_empty() {
        return Err("no error names found in showErr".into());
    }
    Ok(names)
}

/// Outcomes that must also be reached through a particular intent, because reaching the error
/// some other way would not exercise that intent's own rule.
pub const REQUIRED_PER_INTENT: &[&str] = &[
    "create ok",
    "update ok",
    "delete ok",
    "require-strict ok",
    "require-lax ok",
    "create err invalid",
    "update err invalid",
    "create err dangling_reference",
    "update err dangling_reference",
    "patch ok",
    "patch err invalid",
    "patch err dangling_reference",
    "patch err not_found",
    "patch err version_conflict",
    "update err version_conflict",
    "delete err version_conflict",
    "create err limit_exceeded",
    "update err limit_exceeded",
    "patch err limit_exceeded",
    "delete err still_referenced",
    "delete err not_found",
    "update err not_found",
    "require-strict err migration_rejected",
    "require-strict err unauthorized",
    "schema ok",
    "schema err unauthorized",
    "schema err schema_conflict",
    "schema err invalid_intent",
    "schema err limit_exceeded",
    "schema err invalid_schema",
    "schema err migration_rejected",
    "schema err no_tenant",
    "delete err unauthorized",
    "create-tenant ok",
    "create-tenant err unauthorized",
    "create-tenant err invalid_intent",
    "create-tenant err tenant_exists",
    "assign ok",
    "assign err unauthorized",
    "assign err unknown_role",
    "assign err invalid_intent",
    "revoke ok",
    "revoke err unauthorized",
    "revoke err last_owner",
    "team ok",
    "team err invalid_intent",
    "team-assign ok",
    "team-assign err invalid_intent",
    "team-assign err unknown_role",
    "team-revoke ok",
    "team-add ok",
    "team-add err invalid_intent",
    "team-remove ok",
    "invite ok",
    "invite err unauthorized",
    "invite err invalid_intent",
    "invite err unknown_role",
    "invite-revoke ok",
    "invite-revoke err invalid_intent",
    "redeem ok",
    "redeem err unauthorized",
    "redeem err invalid_intent",
    "read-records ok",
    "read-records ok rows>0",
    "read-records ok rows=0",
    "read-records err limit_exceeded",
    "read-records err unknown_object",
    "read-query ok",
    "read-query ok rows>0",
    "read-query ok rows=0",
    "read-query ok next",
    "read-query ok cursor",
    "read-query err unknown_query",
    "read-query err invalid_intent",
    "read-query err limit_exceeded",
    // Settings (slice 12). The platform source is not required: the kernel's `resolve` holds no platform
    // values, so no read reaches it.
    "setting ok",
    "setting-remove ok",
    "setting err unauthorized",
    "setting err invalid_intent",
    "setting err unknown_object",
    "setting err invalid",
    "setting err limit_exceeded",
    "setting err version_conflict",
    "setting-remove err invalid_intent",
    "read-setting err unknown_object",
    "read-setting err no_tenant",
    "read-setting ok source person",
    "read-setting ok source team",
    "read-setting ok source tenant",
    "read-setting ok source default",
];

/// Fail when the batch did not reach every outcome kind often enough: `ok`, every error the model
/// can print, and each of [`REQUIRED_PER_INTENT`] occur at least [`MIN_OUTCOME_COUNT`] times, and
/// nothing was unparsed. A generator that stops reaching a rule must fail here, not pass quietly.
pub fn check_coverage(outcomes: &Outcomes, main_source: &str) -> Result<(), String> {
    let mut wanted: Vec<String> = vec!["ok".to_string()];
    wanted.extend(
        lean_error_names(main_source)?
            .into_iter()
            .map(|n| format!("err {n}")),
    );
    wanted.extend(REQUIRED_PER_INTENT.iter().map(|s| s.to_string()));
    let seen = |k: &str| outcomes.get(k).copied().unwrap_or(0);
    let thin: Vec<String> = wanted
        .iter()
        .filter(|k| seen(k) < MIN_OUTCOME_COUNT)
        .map(|k| format!("`{k}` {} times", seen(k)))
        .collect();
    let unparsed = outcomes
        .keys()
        .filter(|k| k.starts_with("unparsed"))
        .count();
    if !thin.is_empty() || unparsed > 0 {
        return Err(format!(
            "the generator does not reach every outcome (need {MIN_OUTCOME_COUNT} each): {}; \
             unparsed lines: {unparsed}. Counts: {outcomes:?}",
            thin.join(", ")
        ));
    }
    Ok(())
}

/// Run `cases` generated sequences from `base_seed`. `Err` carries the full failure report: the
/// seeds, the shrunk sequence and both sides' output.
pub fn run(lean_dir: &Path, base_seed: u64, cases: usize) -> Result<(), String> {
    build_model(lean_dir)?;
    let scenarios: Vec<Scenario> = (0..cases)
        .map(|i| generate_reads(case_seed(base_seed, i)))
        .collect();
    let (diffs, outcomes) = diff_batch_counted(lean_dir, &scenarios)?;
    let Some(first) = diffs.first() else {
        if cases >= COVERAGE_FROM_CASES {
            let main = std::fs::read_to_string(lean_dir.join("Main.lean"))
                .map_err(|e| format!("cannot read Main.lean: {e}"))?;
            check_coverage(&outcomes, &main)?;
        }
        return Ok(());
    };
    let seed = case_seed(base_seed, first.scenario);
    let small = shrink(lean_dir, scenarios[first.scenario].clone())?;
    let after = diff_batch(lean_dir, std::slice::from_ref(&small))?;
    let shown = after.first().unwrap_or(first);
    Err(format!(
        "the Lean model and the kernel differ in {} of {cases} cases (base seed {base_seed}).\n\
         First failing case {} has seed {seed}; reproduce with STRATA_LEAN_DIFF_SEED={base_seed}.\n\
         Shrunk from {} to {} operations:\n{}\n\
         First difference at output line {}: lean `{}`, kernel `{}`\n\
         Lean:\n{}\nKernel:\n{}\n\
         If the difference is intended, add it to BY_DESIGN in crates/verification/src/lean_diff.rs with its reason.",
        diffs.len(),
        first.scenario,
        scenarios[first.scenario].len(),
        small.len(),
        render("shrunk", std::slice::from_ref(&small)),
        shown.line.0 + 1,
        shown.line.1,
        shown.line.2,
        shown.lean.join("\n"),
        shown.rust.join("\n"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generator places reads of both forms among the operations of a scenario, and a rendered read
    /// is a line `Main.lean` and the kernel harness both read (`read <tenant> <principal> records|query ..`).
    #[test]
    fn the_generator_places_reads_of_both_forms() {
        let (mut records, mut queries) = (0, 0);
        for i in 0..300 {
            for op in generate(case_seed(20_261_121, i)) {
                match &op.kind {
                    OpKind::ReadRecords(_) => {
                        records += 1;
                        assert!(op.render().contains(" records "), "{}", op.render());
                    }
                    OpKind::ReadQuery(..) => {
                        queries += 1;
                        assert!(op.render().contains(" query "), "{}", op.render());
                    }
                    _ => {}
                }
                if matches!(op.kind, OpKind::ReadRecords(_) | OpKind::ReadQuery(..)) {
                    assert!(op.render().starts_with("read "), "{}", op.render());
                }
            }
        }
        assert!(records > 100 && queries > 100, "{records} {queries}");
    }

    /// The generator places schema changes of slice 11 among the operations, and the kernel's harness reads every one
    /// of them (no `unparsed` line), without the lake: the changes the generator writes are changes the kernel runs.
    #[test]
    fn the_generator_places_schema_changes_the_kernel_reads() {
        let mut changes = 0;
        let scenarios: Vec<Scenario> = (0..200)
            .map(|i| generate(case_seed(20_261_125, i)))
            .collect();
        for ops in &scenarios {
            for op in ops {
                if matches!(op.kind, OpKind::Schema(..)) {
                    changes += 1;
                    assert!(op.render().contains(" schema "), "{}", op.render());
                }
            }
        }
        assert!(changes > 100, "{changes}");
        let out = kernel_output(&render("k", &scenarios));
        assert!(
            !out.lines().any(|l| l.starts_with("unparsed")),
            "{}",
            out.lines()
                .find(|l| l.starts_with("unparsed"))
                .unwrap_or("")
        );
        assert!(out.contains("schema-changed"));
    }

    /// The full generator builds the chain the refused delete needs, without the lake: within 2000
    /// scenarios, one makes a record as the first thing done to its tenant, then a record that refers to
    /// it through field 5 or the list field 0, then deletes the first, all as the tenant's owner (who may
    /// delete). The first record exists and has a name, the second names it, so the delete is refused
    /// `still_referenced` by the model.
    #[test]
    fn the_generator_refuses_a_delete_of_a_record_a_restricting_field_refers_to() {
        let mut found = 0;
        for i in 0..2000 {
            let ops = generate(case_seed(20_261_121, i));
            for (at, window) in ops.windows(3).enumerate() {
                let [first, second, delete] = window else {
                    continue;
                };
                let tenant = first.tenant;
                let owner = Caller::User(home_of(tenant));
                if !(1..=2).contains(&tenant) || first.principal != owner {
                    continue;
                }
                // Nothing before touched this tenant, so the first record has id 1.
                if ops[..at].iter().any(|op| op.tenant == tenant) {
                    continue;
                }
                let OpKind::Create(a) = &first.kind else {
                    continue;
                };
                let OpKind::Create(b) = &second.kind else {
                    continue;
                };
                let OpKind::Delete(id, _) = &delete.kind else {
                    continue;
                };
                let has = |fields: &Vec<(u64, String)>, f: u64| fields.iter().any(|(g, _)| *g == f);
                let names_first = b
                    .iter()
                    .any(|(f, v)| (*f == 5 && v == "r:1") || (*f == 0 && v == "l:r:1"));
                let same = |op: &Op| op.tenant == tenant && op.principal == owner && op.object == 1;
                if same(first)
                    && same(second)
                    && same(delete)
                    && has(a, 1)
                    && !has(a, 4)
                    && !has(a, 0)
                    && !has(a, 5)
                    && names_first
                    && *id == 1
                {
                    found += 1;
                }
            }
        }
        assert!(
            found > 0,
            "no generated scenario deletes a record a restricting field of a live record refers to"
        );
    }
}
