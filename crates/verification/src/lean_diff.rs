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

use crate::lean::{build_model, kernel_output, run_model};
use std::path::Path;

/// The intent kinds the generator produces; each is the word after the principal in an `op` line.
pub const GENERATED_INTENTS: [&str; 18] = [
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
    match rng.below(8) {
        0 => rng.pick(&[0u64, 7]),
        1..=3 => rng.pick(&[1u64, 2, 3]),
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

/// The field of a `require`: half the time one of the reference fields, sometimes a list.
fn require_field(rng: &mut Lcg) -> u64 {
    match rng.below(8) {
        0..=3 => 4 + rng.below(2),
        4 => rng.pick(&[0u64, 7, 8]),
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
        // A `require` or `unique` names a field the object type declares. The kernel defines a
        // field the type lacks when a change names it, and the model's intents name a field the
        // type has (see the slice 4 review note); `part` declares fields 1 to 4 only.
        let kind = match (object, kind) {
            (2, OpKind::Require(f, strict)) => OpKind::Require(1 + f % 4, strict),
            (2, OpKind::Unique(f, strict)) => OpKind::Unique(1 + f % 4, strict),
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
    ops
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
    }
}

fn count_outcomes(scenarios: &[Scenario], lean: &[Vec<String>]) -> Outcomes {
    let mut counts = Outcomes::new();
    for (ops, out) in scenarios.iter().zip(lean) {
        // Line 0 is `scenario <name>`; then one line per operation.
        for (op, line) in ops.iter().zip(out.iter().skip(1)) {
            *counts.entry(line.clone()).or_default() += 1;
            *counts
                .entry(format!("{} {line}", intent_name(op)))
                .or_default() += 1;
        }
    }
    counts
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
        | OpKind::Redeem(..) => None,
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
        .map(|i| generate(case_seed(base_seed, i)))
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
