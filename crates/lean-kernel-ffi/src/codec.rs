//! The canonical byte codec, Rust side: the encoding of the boundary types between this backend
//! and the compiled Lean kernel.
//!
//! The format is the one `lean/Strata/Codec/Core.lean` and `lean/Strata/Codec/Boundary.lean`
//! define, and the Lean encoder produces the same bytes for the fixtures of
//! `lean/Strata/Codec/Fixtures.lean`, which the tests of this crate carry too:
//!
//! - a tag is one byte, the constructor index;
//! - a natural is minimal LEB128: seven bits per byte, low group first, the high bit set on every
//!   byte but the last; a multi-byte natural never ends in a zero byte;
//! - an integer is a sign tag (`0` non-negative, `1` negative) and the natural `i`, or `!i` when
//!   negative (that is `-i - 1`);
//! - a boolean is `0` or `1`; a string is the natural length of its UTF-8 bytes and the bytes, and
//!   decoding requires valid UTF-8;
//! - a list is the natural count and then the elements; an option is `0`, or `1` and the value;
//! - a structure is its fields in declaration order.
//!
//! Three shapes carry the object types of a tenant (slice 4 of the model): a reference type is its
//! tag (`3` restricts, `4` clears) and the natural id of the object type it names; a tenant holds
//! its object types as a list of entries, each an id and then the type's schema, composite
//! constraints and records; an intent is its tag and then the natural id of the object type it acts
//! on, before its other fields. The kernel's refusal `UnknownObject` is tag `11`, `ConstraintViolated`
//! `12` and `Evaluation` `13`.
//!
//! An expression (slice 5) is its tag and then its parts, in prefix order: a literal is `0` and its
//! value, a field read `1` and its scope (`0` record, `1` old, `2` new) and its field id, the
//! principal `2`, a comparison `3` and its operator (`0` eq, `1` ne, `2` lt, `3` le, `4` gt, `5` ge)
//! and two operands, `and` `4` and `or` `5` with two operands each, `not` `6` and `isNull` `7` with
//! one. A field's default is an expression, and an object type lists its constraints after its
//! records. The nesting of an expression is bounded by [`MAX_EXPR_DEPTH`].
//!
//! Slice 7 adds the membership of a tenant: after its limits a tenant carries its roles (a list of
//! naturals), its teams (a list of entries, each the id and then the label, the members and the roles)
//! and its invitations (each the id, then the address, the roles, the issuer, the expiry, the redeemer
//! and the revoker, the last two optional). The kernel's refusals `InvalidIntent`, `UnknownRole`,
//! `LastOwner` and `TenantExists` are tags `14` to `17`, `WorkflowViolation` `18` and `SchemaConflict`
//! (slice 11) `19`. An intent is tag `6` for a creation of a tenant (its owner, then its
//! name) or `7` for a membership intent: a second tag (`0` assign a role, `1` revoke one, `2` define
//! a team, `3` and `4` give a team a role and take one away, `5` and `6` add a team member and remove
//! one, `7` create an invitation, `8` revoke one, `9` redeem one) and its fields in declaration order.
//!
//! Slice 9 adds the kernel's names and the revision of a tenant: a workflow is its id (a string) before
//! its field, a transition is its name (a string) before its source, and a tenant ends with its revision,
//! its event sequence and its schema version (three naturals, in that order, after its invitations).
//!
//! Slice 12 adds the settings of a tenant. A write of a setting is intent tag `9`: its name (a string),
//! its level (one byte: platform `0`, tenant `1`, team `2`, person `3`), an option of the team's number,
//! an option of the value (none removes the override) and the expected revision (a natural). A tenant
//! ends, after its schema version, with three lists: its setting definitions, its overrides and its
//! setting revisions, each entry as its encoder describes.
//!
//! Slice 15 adds the triggers and automations of a tenant. After its operations a tenant ends with two lists:
//! its triggers, each entry the id and then the trigger (its event, an optional condition, its automation, a
//! paused flag and an optional definer), and its automations, each entry the id and then its run-as roles, an
//! optional run key, an optional definer, an optional suspension role and its state (`0` active, `1` draft).
//! A trigger's event is tag `0` to `3` (created, updated, deleted, ledger posted). A context ends with its hop,
//! a natural, zero when no automation caused the intent. A principal has no automation form here: Lean's tag
//! `3` (an automation principal) is refused by the decoder.
//!
//! Every value has one encoding and the decoders accept nothing else: [`decode_all`] requires the
//! input to be consumed exactly.
//!
//! The types mirror the Lean boundary types restricted to the modelled fragment: [`Val`] has null,
//! integer, string, boolean, reference and list only, and ids and counts are `u64`. The Lean
//! codec is unbounded, so two inputs that Lean decodes are refused here: an integer outside `i64`
//! or a natural outside `u64` (values the Rust types cannot hold), and a type nested deeper than
//! [`MAX_TY_DEPTH`]. Every value the Rust types can hold encodes to the bytes Lean encodes.
//!
//! This module is written against the Lean definitions by reading; the tests in
//! `tests/lean_kernel_ffi_it.rs` check it against the fixtures. It contains no `unsafe` code.

/// The deepest nesting of list types the decoder accepts. The kernel's schema admits one level of
/// list; the codec allows more so that any type the Rust types hold round-trips.
pub const MAX_TY_DEPTH: usize = 64;

/// The deepest nesting of expressions the decoder accepts (constraints and defaults, slice 5). The
/// Lean decoder takes its fuel from the input length, so any depth it decodes is bounded by the
/// input; the codec here stops at this depth, so that a crafted input cannot exhaust the stack.
pub const MAX_EXPR_DEPTH: usize = 256;

/// The kernel's `on_delete` of a reference: refuse the delete, or clear the referring field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnDelete {
    Restrict,
    SetNull,
}

/// A field type, as the schema states it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    Int,
    Str,
    Bool,
    /// A reference to the object type of this id, and what its delete does.
    Ref(u64, OnDelete),
    List(Box<Ty>),
}

/// An element of a list value: a scalar or a reference; a list never holds a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Elem {
    Null,
    Int(i64),
    Str(String),
    Bool(bool),
    Ref(u64),
}

/// A value of a field. `Null` is the absence of a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Val {
    Null,
    Int(i64),
    Str(String),
    Bool(bool),
    Ref(u64),
    List(Vec<Elem>),
}

/// Which record an expression reads a field from: the record being checked (`0`), the stored record
/// an update replaces (`1`), or the new record (`2`). Lean's `Scope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Record,
    Old,
    New,
}

/// A comparison operator: `0` eq, `1` ne, `2` lt, `3` le, `4` gt, `5` ge. Lean's `CmpOp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// An acting principal: a user by its id, the system, or an anonymous caller. Lean's `Principal`:
/// a user is tag `0` and its id, the system `1` and an anonymous caller `2`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Principal {
    User(u64),
    System,
    Anonymous,
}

/// An action a policy names: read `0`, create `1`, update `2`, delete `3`, manage `4`. Lean's
/// `Action`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Read,
    Create,
    Update,
    Delete,
    Manage,
}

/// What a policy names: an object type (`0`, its id), every object type (`1`), the schema (`2`) or
/// the tenant's membership (`3`). Lean's `Resource`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Object(u64),
    AnyObject,
    Schema,
    Members,
}

/// What a role grants: a capability (`0`, its id), or an action on the records of an object type
/// (`1`, the object type and the action). Lean's `Grant`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grant {
    Capability(u64),
    Object(u64, Action),
}

/// A policy: the role it belongs to, the resource, the actions it names and its condition, if any
/// (Lean's `Policy`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub role: u64,
    pub resource: Resource,
    pub actions: Vec<Action>,
    pub condition: Option<Expr>,
}

/// An expression of a constraint or of a field default (slice 5). Lean's `Expr`: a literal (tag
/// `0`), a field read (`1`, the scope and the field id), the principal (`2`), a comparison (`3`),
/// `and` (`4`), `or` (`5`), `not` (`6`), `isNull` (`7`) and `$created_by` (`8`, the scope) and `param` (`9`, an input of an operation by name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Lit(Val),
    Field(Scope, u64),
    Principal,
    Cmp(CmpOp, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    IsNull(Box<Expr>),
    CreatedBy(Scope),
    /// An input of the running operation, by name (tag `9`; slice 14). Unbound outside an operation.
    Param(String),
}

/// The definition of a field of the schema. `default` is the expression a create fills the field
/// with when the intent does not name it; `Expr::Lit(Val::Null)` is no default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDef {
    pub ty: Ty,
    pub required: bool,
    pub unique: bool,
    pub default: Expr,
}

/// The size bounds of a tenant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_string: u64,
    pub max_list: u64,
    pub max_unique: u64,
    pub max_record: u64,
}

impl Limits {
    /// The kernel's numbers (`MAX_STRING_BYTES`, `MAX_LIST_LEN`, `MAX_UNIQUE_BYTES`,
    /// `MAX_RECORD_BYTES` in `limits.rs`).
    pub const KERNEL: Limits = Limits {
        max_string: 65_536,
        max_list: 1_000,
        max_unique: 200,
        max_record: 262_144,
    };
}

/// A stored record: the user who created it (none when the system or an anonymous caller did), its
/// version, and its fields by id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub owner: Option<u64>,
    pub version: u64,
    pub fields: Vec<(u64, Val)>,
}

/// A transition of a workflow (slice 8): its name (slice 9), its source and target states, its guard
/// (`None` when it always holds), and the fields it assigns with the expressions that give their values.
/// Lean's `WfTransition`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WfTransition {
    pub name: String,
    pub source: String,
    pub target: String,
    pub guard: Option<Expr>,
    pub set: Vec<(u64, Expr)>,
}

/// A workflow over one field of an object type (slice 8): its id (slice 9), its field, its initial states
/// and its transitions. Lean's `Workflow`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workflow {
    pub id: String,
    pub field: u64,
    pub initial: Vec<String>,
    pub transitions: Vec<WfTransition>,
}

/// An object type of a tenant: its schema, its composite unique constraints, its records, its
/// constraints (slice 5) and its workflows (slice 8), in the kernel's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjType {
    pub schema: Vec<(u64, FieldDef)>,
    pub uniques: Vec<Vec<u64>>,
    pub records: Vec<(u64, Record)>,
    pub constraints: Vec<Expr>,
    pub workflows: Vec<Workflow>,
}

/// A team (slice 7): its label, its members and the roles those members hold through it. Lean's
/// `TeamDef`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeamDef {
    pub label: String,
    pub members: Vec<u64>,
    pub roles: Vec<u64>,
}

/// An invitation (slice 7): the address it names, the roles it offers, who issued it, when it expires
/// (a number of milliseconds), and who redeemed it or revoked it, if anyone. Lean's `InvitationDef`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvitationDef {
    pub address: String,
    pub roles: Vec<u64>,
    pub created_by: u64,
    pub expires_at: u64,
    pub redeemed_by: Option<u64>,
    pub revoked_by: Option<u64>,
}

/// A level at which a setting is overridden (slice 12; the Lean `SettingLevel`): platform `0`, tenant `1`,
/// team `2`, person `3`, one byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingLevel {
    Platform,
    Tenant,
    Team,
    Person,
}

/// A setting's type (slice 12; the Lean `SettingType`): a flag `0`, a whole number `1`, text `2`, one byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingType {
    Bool,
    Int,
    Str,
}

/// The subject of an override (slice 12; the Lean `SettingSubject`): the tenant (`0`), a team by its number
/// (`1`) or a person by its number (`2`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingSubject {
    Tenant,
    Team(u64),
    Person(u64),
}

/// What a setting allows (slice 12; the Lean `SettingAllowed`): a closed set of values (`0`), or a range of
/// whole numbers, low then high (`1`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingAllowed {
    Closed(Vec<Val>),
    Range(i64, i64),
}

/// A setting's definition (slice 12; the Lean `SettingDef`): its name, its type, the levels that may hold an
/// override, its lock (`None` when unlocked), its default and what it allows (`None` when anything).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingDef {
    pub name: String,
    pub ty: SettingType,
    pub levels: Vec<SettingLevel>,
    pub locked: Option<SettingLevel>,
    pub default: Val,
    pub allowed: Option<SettingAllowed>,
}

/// An override of a setting (slice 12; the Lean `SettingOverride`): its setting, its subject (which names its
/// level), its value and its revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingOverride {
    pub name: String,
    pub subject: SettingSubject,
    pub value: Val,
    pub revision: u64,
}

/// A calendar period of the ledger (slice 13; the Lean `LedgerPeriod`): hour `0`, day `1`, week `2`, month `3`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerPeriod {
    Hour,
    Day,
    Week,
    Month,
}

/// The owner of a ledger account (slice 13; the Lean `LedgerOwner`): a person by number (`0`) or a pool by
/// number (`1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerOwner {
    Person(u64),
    Pool(u64),
}

/// The expiry a unit declares (slice 13; the Lean `LedgerExpiry`): a bucket lives for `periods` periods.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerExpiry {
    pub period: LedgerPeriod,
    pub periods: u64,
}

/// A unit the schema declares, with its expiry when it has one (slice 13; the Lean `LedgerUnitDecl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerUnitDecl {
    pub unit: u64,
    pub expiry: Option<LedgerExpiry>,
}

/// A limit (slice 13; the Lean `LedgerLimit`): the unit, the owner (none for the unit's own), the scope, the
/// floor, the cap, the cap per period with its period, and whether that cap counts only additions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerLimit {
    pub unit: u64,
    pub owner: Option<LedgerOwner>,
    pub scope: String,
    pub floor: Option<i64>,
    pub cap: Option<i64>,
    pub period_cap: Option<(u64, LedgerPeriod)>,
    pub additions_only: Option<bool>,
}

/// The declarations of a ledger (slice 13; the Lean `LedgerDecls`): its units, its pools and the limits of
/// units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerDecls {
    pub units: Vec<LedgerUnitDecl>,
    pub pools: Vec<u64>,
    pub limits: Vec<LedgerLimit>,
}

/// A bucket of expiring credit (slice 13; the Lean `LedgerBucket`): the start of its period and the credit left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerBucket {
    pub period_start: i64,
    pub remaining: u64,
}

/// The credits an account posted in one calendar period (slice 13; the Lean `LedgerPeriodTotal`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerPeriodTotal {
    pub period: LedgerPeriod,
    pub start: i64,
    pub credits: u64,
}

/// An account and its header (slice 13; the Lean `LedgerAccount`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerAccount {
    pub owner: LedgerOwner,
    pub unit: u64,
    pub scope: String,
    pub balance: i64,
    pub last_seq: u64,
    pub periods: Vec<LedgerPeriodTotal>,
    pub buckets: Vec<LedgerBucket>,
}

/// One entry of the journal (slice 13; the Lean `LedgerEntry`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerEntry {
    pub owner: LedgerOwner,
    pub unit: u64,
    pub scope: String,
    pub sequence: u64,
    pub amount: i64,
    pub key: String,
    pub reason: String,
    pub caused_by: Option<u64>,
    pub time: i64,
    pub corrects: Option<u64>,
    /// The transaction the entry is a leg of, as the text that names it (the Lean `LedgerEntry.txn`).
    pub transaction: Option<String>,
}

/// The ledger state of a tenant (slice 13; the Lean `LedgerState`): its accounts, its journal (oldest first)
/// and the limits of accounts.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LedgerState {
    pub accounts: Vec<LedgerAccount>,
    pub journal: Vec<LedgerEntry>,
    pub limits: Vec<LedgerLimit>,
}

/// One atom of a ledger policy's condition (slice 13; the Lean `LedgerAtom`): the reader is the owner (`0`), the
/// unit is a number (`1`), or the scope is a text (`2`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerAtom {
    OwnerIsCaller,
    UnitIs(u64),
    ScopeIs(String),
}

/// A policy over the ledger (slice 13; the Lean `LedgerPolicy`): its role, its unit (none for every unit) and its
/// conjunction of atoms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerPolicy {
    pub role: u64,
    pub unit: Option<u64>,
    pub conds: Vec<LedgerAtom>,
}

/// A posting of a `PostLedger` intent (slice 13; the Lean `LedgerPosting`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerPosting {
    pub owner: LedgerOwner,
    pub unit: u64,
    pub amount: i64,
    pub key: String,
    pub reason: String,
    pub corrects: Option<u64>,
    pub scope: String,
}

/// One leg of a transaction (slice 13; the Lean `LedgerLeg`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerLeg {
    pub owner: LedgerOwner,
    pub unit: u64,
    pub amount: i64,
    pub scope: String,
}

/// A transaction of a `PostLedgerTransaction` intent (slice 13; the Lean `LedgerTx`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerTx {
    pub operation: String,
    pub key: String,
    pub reason: String,
    pub legs: Vec<LedgerLeg>,
}

/// One input of an operation (slice 14; the Lean `OpInput`): its name, its type and whether it must be given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpInput {
    pub name: String,
    pub ty: Ty,
    pub required: bool,
}

/// Whose account a posting step of an operation posts to (slice 14; the Lean `OpOwner`): tag `0` the invoker, tag
/// `1` a pool (its natural).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpOwner {
    Invoker,
    Pool(u64),
}

/// One step of an operation (slice 14; the Lean `OpStep`): `0` create, `1` update, `2` delete, `3` post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpStep {
    Create {
        id: String,
        ty: u64,
        fields: Vec<(u64, Expr)>,
    },
    Update {
        id: String,
        ty: u64,
        record: Expr,
        fields: Vec<(u64, Expr)>,
    },
    Delete {
        id: String,
        ty: u64,
        record: Expr,
    },
    Post {
        id: String,
        owner: OpOwner,
        unit: u64,
        amount: Expr,
        reason: String,
    },
}

/// A condition of an operation (slice 14; the Lean `OpCond`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpCond {
    pub id: String,
    pub expr: Expr,
}

/// An operation of a schema (slice 14; the Lean `OpDef`): its id, inputs, whether every signed-in person may invoke
/// it, the roles that may, its optional subject (an object type and the input that names the record), its conditions
/// and its steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpDef {
    pub id: String,
    pub inputs: Vec<OpInput>,
    pub signed_in: bool,
    pub roles: Vec<u64>,
    pub subject: Option<(u64, String)>,
    pub conditions: Vec<OpCond>,
    pub steps: Vec<OpStep>,
}

/// Everything one tenant owns, as the boundary carries it: its object types by id, its members
/// (each user with its roles), what each role grants, its policies, the next record id, the size
/// bounds, (slice 7) the roles its schema defines, its teams and its invitations, (slice 9) its
/// revision, its event sequence and its schema version, (slice 12) its setting definitions, its
/// overrides and the revision of each setting, and (slice 13) its ledger declarations, its ledger
/// state and its ledger policies. The owner is a member holding role `0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tenant {
    pub types: Vec<(u64, ObjType)>,
    pub members: Vec<(u64, Vec<u64>)>,
    pub grants: Vec<(u64, Grant)>,
    pub policies: Vec<Policy>,
    pub next_id: u64,
    pub limits: Limits,
    pub roles: Vec<u64>,
    pub teams: Vec<(u64, TeamDef)>,
    pub invitations: Vec<(u64, InvitationDef)>,
    pub revision: u64,
    pub event_seq: u64,
    pub schema_version: u64,
    pub setting_defs: Vec<SettingDef>,
    pub overrides: Vec<SettingOverride>,
    pub setting_revs: Vec<(String, u64)>,
    pub ledger_decls: Option<LedgerDecls>,
    pub ledger: LedgerState,
    pub ledger_policies: Vec<LedgerPolicy>,
    /// The operations of the schema (slice 14), each with its name.
    pub operations: Vec<(String, OpDef)>,
    /// The triggers, by id (slice 15).
    pub triggers: Vec<(u64, TriggerDef)>,
    /// The automations, by id (slice 15).
    pub automations: Vec<(u64, AutomationDef)>,
}

/// The event a trigger listens for (slice 15), Lean's `TriggerOn`: a record of an object type created (`0`),
/// updated (`1`, with the transition it names, if any), or deleted (`2`), or a ledger unit posted (`3`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriggerOn {
    RecordCreated(u64),
    RecordUpdated(u64, Option<String>),
    RecordDeleted(u64),
    LedgerPosted(u64),
}

/// A trigger (slice 15), Lean's `TriggerDef`: its id, its event, its condition (none for every event), the
/// automation it starts, whether it is paused, and its definer (none for the system). The Rust harness maps
/// trigger `n` to `TriggerId` `t<n>`; the codec carries only the number `n`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerDef {
    pub id: u64,
    pub on: TriggerOn,
    pub condition: Option<Expr>,
    pub automation: u64,
    pub paused: bool,
    pub defined_by: Option<u64>,
}

/// The state of an automation (slice 15), Lean's `AutoState`: active (`0`) or draft (`1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoState {
    Active,
    Draft,
}

/// An automation (slice 15), Lean's `AutomationDef` without its steps: its id, its run-as roles, its run key
/// (none for one run per event), its definer (none for the system), the role it is suspended for, and its
/// state. The Rust harness maps automation `n` to `WorkflowId` `w<n>`; the codec carries only the number `n`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutomationDef {
    pub id: u64,
    pub run_as: Vec<u64>,
    pub run_key: Option<Expr>,
    pub defined_by: Option<u64>,
    pub suspended_for: Option<u64>,
    pub state: AutoState,
}

/// The acting context of a step: which tenant, who is acting, and the hop of the event that caused the intent
/// (zero for an intent no automation caused). Lean's `Ctx`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ctx {
    pub tenant: u64,
    pub principal: Principal,
    pub hop: u64,
}

/// Why a step was refused: the kernel's refusals of the model, `Err` in Lean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    NoTenant,
    Unauthorized,
    NotFound,
    Invalid,
    MigrationRejected,
    DanglingReference,
    StillReferenced,
    VersionConflict,
    LimitExceeded,
    UniqueViolated,
    InvalidSchema,
    UnknownObject,
    ConstraintViolated,
    Evaluation,
    InvalidIntent,
    UnknownRole,
    LastOwner,
    TenantExists,
    WorkflowViolation,
    SchemaConflict,
    /// An adjustment or a reversal of one's own account (slice 13; the Lean `selfAction`, tag `20`).
    SelfAction,
}

/// A membership intent (slice 7). Lean's `MemberIntent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberIntent {
    AssignRole {
        user: u64,
        role: u64,
    },
    RevokeRole {
        user: u64,
        role: u64,
    },
    DefineTeam {
        team: u64,
        label: String,
    },
    AssignTeamRole {
        team: u64,
        role: u64,
    },
    RevokeTeamRole {
        team: u64,
        role: u64,
    },
    AddTeamMember {
        team: u64,
        user: u64,
    },
    RemoveTeamMember {
        team: u64,
        user: u64,
    },
    CreateInvitation {
        invitation: u64,
        address: String,
        roles: Vec<u64>,
        now: u64,
    },
    RevokeInvitation {
        invitation: u64,
    },
    RedeemInvitation {
        invitation: u64,
        now: u64,
        verified: Option<String>,
    },
}

/// One change of a schema change in general (slice 11; the Lean `Change`): define a field of an object type (a new
/// one, or the redefinition of one it has), remove a field, define an object type, or remove one (deleting its
/// records or not).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    DefineField { ty: u64, field: u64, def: FieldDef },
    RemoveField { ty: u64, field: u64 },
    DefineType { ty: u64 },
    RemoveType { ty: u64, delete_records: bool },
}

/// What a caller asks the kernel to do. Each intent names the object type it acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    Create {
        ty: u64,
        fields: Vec<(u64, Val)>,
    },
    Update {
        ty: u64,
        id: u64,
        fields: Vec<(u64, Val)>,
        expected: Option<u64>,
    },
    Delete {
        ty: u64,
        id: u64,
        expected: Option<u64>,
    },
    SetRequired {
        ty: u64,
        field: u64,
        strict: bool,
    },
    SetUnique {
        ty: u64,
        field: u64,
        strict: bool,
    },
    DefineUnique {
        ty: u64,
        fields: Vec<u64>,
    },
    /// Create a tenant owned by `owner` (slice 7).
    CreateTenant {
        owner: u64,
        name: String,
    },
    /// A membership intent of an existing tenant (slice 7).
    Member(MemberIntent),
    /// A schema change in general (slice 11): the schema version the caller expects, whether a record that no
    /// longer conforms refuses the change (`strict`), and the changes, applied in order.
    ChangeSchema {
        expected: Option<u64>,
        strict: bool,
        changes: Vec<Change>,
    },
    /// A write of a setting (slice 12; the Lean `writeSetting`): the setting's name, its level, the team when the
    /// level is a team's, the value (`None` removes the override) and the revision the caller read.
    WriteSetting {
        setting: String,
        level: SettingLevel,
        team: Option<u64>,
        value: Option<Val>,
        expected_revision: u64,
    },
    /// A `PostLedger` intent (slice 13; tag `10`): the acting context's time, then one to eight postings, each
    /// decided on the state the earlier ones leave.
    PostLedger {
        now: i64,
        postings: Vec<LedgerPosting>,
    },
    /// An `InvokeOperation` intent (slice 14; tag `13`): the time, the operation's name, its inputs by name and the
    /// idempotency key.
    InvokeOperation {
        now: i64,
        operation: String,
        inputs: Vec<(String, Val)>,
        key: String,
    },
    /// An `ExpireLedger` intent (slice 13; tag `11`): the time, the unit and the cutoff.
    ExpireLedger {
        now: i64,
        unit: u64,
        cutoff: i64,
    },
    /// A `PostLedgerTransaction` intent (slice 13; tag `12`): the time, then the transaction.
    PostLedgerTx {
        now: i64,
        tx: LedgerTx,
    },
}

/// The result of a step: the refusal, or the tenant after the change (Lean's `Except Err Tenant`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepResult {
    Refused(Refusal),
    Accepted(Tenant),
}

// ----------------------------------------------------------------------------------------------
// Encoding
// ----------------------------------------------------------------------------------------------

/// A natural, minimal LEB128 (`encNat`).
fn put_nat(out: &mut Vec<u8>, mut n: u64) {
    while n >= 128 {
        out.push((n % 128) as u8 + 128);
        n /= 128;
    }
    out.push(n as u8);
}

/// An integer: the sign tag, then the natural magnitude (`encInt`).
fn put_int(out: &mut Vec<u8>, i: i64) {
    if i >= 0 {
        out.push(0);
        put_nat(out, i as u64);
    } else {
        out.push(1);
        put_nat(out, (!i) as u64);
    }
}

/// A boolean, one byte (`encBool`).
fn put_bool(out: &mut Vec<u8>, b: bool) {
    out.push(u8::from(b));
}

/// A string: the length of its UTF-8 bytes, then the bytes (`encStr`).
fn put_str(out: &mut Vec<u8>, s: &str) {
    put_nat(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

/// An option of a natural (`encOpt encNat`).
fn put_opt_nat(out: &mut Vec<u8>, v: Option<u64>) {
    match v {
        None => out.push(0),
        Some(n) => {
            out.push(1);
            put_nat(out, n);
        }
    }
}

/// A list: the count, then the elements (`encList`).
fn put_list<T>(out: &mut Vec<u8>, items: &[T], put: fn(&mut Vec<u8>, &T)) {
    put_nat(out, items.len() as u64);
    for item in items {
        put(out, item);
    }
}

fn put_u64(out: &mut Vec<u8>, n: &u64) {
    put_nat(out, *n);
}

fn put_u64_list(out: &mut Vec<u8>, ids: &Vec<u64>) {
    put_list(out, ids.as_slice(), put_u64);
}

fn put_elem(out: &mut Vec<u8>, e: &Elem) {
    match e {
        Elem::Null => out.push(0),
        Elem::Int(i) => {
            out.push(1);
            put_int(out, *i);
        }
        Elem::Str(s) => {
            out.push(2);
            put_str(out, s);
        }
        Elem::Bool(b) => {
            out.push(3);
            put_bool(out, *b);
        }
        Elem::Ref(id) => {
            out.push(4);
            put_nat(out, *id);
        }
    }
}

fn put_val(out: &mut Vec<u8>, v: &Val) {
    match v {
        Val::Null => out.push(0),
        Val::Int(i) => {
            out.push(1);
            put_int(out, *i);
        }
        Val::Str(s) => {
            out.push(2);
            put_str(out, s);
        }
        Val::Bool(b) => {
            out.push(3);
            put_bool(out, *b);
        }
        Val::Ref(id) => {
            out.push(4);
            put_nat(out, *id);
        }
        Val::List(items) => {
            out.push(5);
            put_list(out, items.as_slice(), put_elem);
        }
    }
}

/// A field of a record or an intent: the field id, then its value (`encFieldVal`).
fn put_field(out: &mut Vec<u8>, f: &(u64, Val)) {
    put_nat(out, f.0);
    put_val(out, &f.1);
}

fn put_ty(out: &mut Vec<u8>, t: &Ty) {
    match t {
        Ty::Int => out.push(0),
        Ty::Str => out.push(1),
        Ty::Bool => out.push(2),
        Ty::Ref(target, OnDelete::Restrict) => {
            out.push(3);
            put_nat(out, *target);
        }
        Ty::Ref(target, OnDelete::SetNull) => {
            out.push(4);
            put_nat(out, *target);
        }
        Ty::List(elem) => {
            out.push(5);
            put_ty(out, elem);
        }
    }
}

fn put_scope(out: &mut Vec<u8>, s: &Scope) {
    out.push(match s {
        Scope::Record => 0,
        Scope::Old => 1,
        Scope::New => 2,
    });
}

fn put_cmp_op(out: &mut Vec<u8>, op: &CmpOp) {
    out.push(match op {
        CmpOp::Eq => 0,
        CmpOp::Ne => 1,
        CmpOp::Lt => 2,
        CmpOp::Le => 3,
        CmpOp::Gt => 4,
        CmpOp::Ge => 5,
    });
}

/// An expression, in prefix order (`encExpr`).
fn put_expr(out: &mut Vec<u8>, e: &Expr) {
    match e {
        Expr::Lit(v) => {
            out.push(0);
            put_val(out, v);
        }
        Expr::Field(s, f) => {
            out.push(1);
            put_scope(out, s);
            put_nat(out, *f);
        }
        Expr::Principal => out.push(2),
        Expr::Cmp(op, a, b) => {
            out.push(3);
            put_cmp_op(out, op);
            put_expr(out, a);
            put_expr(out, b);
        }
        Expr::And(a, b) => {
            out.push(4);
            put_expr(out, a);
            put_expr(out, b);
        }
        Expr::Or(a, b) => {
            out.push(5);
            put_expr(out, a);
            put_expr(out, b);
        }
        Expr::Not(a) => {
            out.push(6);
            put_expr(out, a);
        }
        Expr::IsNull(a) => {
            out.push(7);
            put_expr(out, a);
        }
        Expr::CreatedBy(s) => {
            out.push(8);
            put_scope(out, s);
        }
        Expr::Param(n) => {
            out.push(9);
            put_str(out, n);
        }
    }
}

fn put_field_def(out: &mut Vec<u8>, fd: &FieldDef) {
    put_ty(out, &fd.ty);
    put_bool(out, fd.required);
    put_bool(out, fd.unique);
    put_expr(out, &fd.default);
}

fn put_schema_entry(out: &mut Vec<u8>, e: &(u64, FieldDef)) {
    put_nat(out, e.0);
    put_field_def(out, &e.1);
}

fn put_limits(out: &mut Vec<u8>, l: &Limits) {
    put_nat(out, l.max_string);
    put_nat(out, l.max_list);
    put_nat(out, l.max_unique);
    put_nat(out, l.max_record);
}

fn put_record(out: &mut Vec<u8>, r: &Record) {
    put_opt_nat(out, r.owner);
    put_nat(out, r.version);
    put_list(out, r.fields.as_slice(), put_field);
}

/// A principal: a user is tag `0` and its id, the system `1`, an anonymous caller `2` (`encPrincipal`).
fn put_principal(out: &mut Vec<u8>, p: &Principal) {
    match p {
        Principal::User(id) => {
            out.push(0);
            put_nat(out, *id);
        }
        Principal::System => out.push(1),
        Principal::Anonymous => out.push(2),
    }
}

/// An action: read `0`, create `1`, update `2`, delete `3`, manage `4` (`encAction`).
fn put_action(out: &mut Vec<u8>, a: &Action) {
    out.push(match a {
        Action::Read => 0,
        Action::Create => 1,
        Action::Update => 2,
        Action::Delete => 3,
        Action::Manage => 4,
    });
}

/// A resource: an object type `0` and its id, every object type `1`, the schema `2`, the membership
/// `3` (`encResource`).
fn put_resource(out: &mut Vec<u8>, r: &Resource) {
    match r {
        Resource::Object(ty) => {
            out.push(0);
            put_nat(out, *ty);
        }
        Resource::AnyObject => out.push(1),
        Resource::Schema => out.push(2),
        Resource::Members => out.push(3),
    }
}

/// A grant: a capability `0` and its id, or an object type `1` and an action (`encGrant`).
fn put_grant(out: &mut Vec<u8>, g: &Grant) {
    match g {
        Grant::Capability(c) => {
            out.push(0);
            put_nat(out, *c);
        }
        Grant::Object(ty, a) => {
            out.push(1);
            put_nat(out, *ty);
            put_action(out, a);
        }
    }
}

fn put_action_item(out: &mut Vec<u8>, a: &Action) {
    put_action(out, a);
}

/// A policy: its role, its resource, its actions, then its condition as an option (`encPolicy`).
fn put_policy(out: &mut Vec<u8>, p: &Policy) {
    put_nat(out, p.role);
    put_resource(out, &p.resource);
    put_list(out, p.actions.as_slice(), put_action_item);
    match &p.condition {
        None => out.push(0),
        Some(c) => {
            out.push(1);
            put_expr(out, c);
        }
    }
}

/// A member: a user, then its roles (`encMember`).
fn put_member(out: &mut Vec<u8>, m: &(u64, Vec<u64>)) {
    put_nat(out, m.0);
    put_u64_list(out, &m.1);
}

/// A grant entry: a role, then what it grants (`encGrantEntry`).
fn put_grant_entry(out: &mut Vec<u8>, e: &(u64, Grant)) {
    put_nat(out, e.0);
    put_grant(out, &e.1);
}

fn put_record_entry(out: &mut Vec<u8>, e: &(u64, Record)) {
    put_nat(out, e.0);
    put_record(out, &e.1);
}

fn put_obj_type(out: &mut Vec<u8>, o: &ObjType) {
    put_list(out, o.schema.as_slice(), put_schema_entry);
    put_list(out, o.uniques.as_slice(), put_u64_list);
    put_list(out, o.records.as_slice(), put_record_entry);
    put_list(out, o.constraints.as_slice(), put_expr_root);
    put_list(out, o.workflows.as_slice(), put_workflow);
}

/// A string as a list element (`encList encStr`).
fn put_string(out: &mut Vec<u8>, s: &String) {
    put_str(out, s);
}

/// An optional expression (`encOpt encExpr`): a tag, then the expression when there is one.
fn put_opt_expr(out: &mut Vec<u8>, e: &Option<Expr>) {
    match e {
        None => out.push(0),
        Some(e) => {
            out.push(1);
            put_expr(out, e);
        }
    }
}

/// An assignment of a transition: the field id, then the expression that gives its value
/// (`encFieldExpr`).
fn put_field_expr(out: &mut Vec<u8>, fe: &(u64, Expr)) {
    put_nat(out, fe.0);
    put_expr(out, &fe.1);
}

/// A transition of a workflow: its name, its source, its target, its guard, then its assignments.
fn put_wf_transition(out: &mut Vec<u8>, t: &WfTransition) {
    put_str(out, &t.name);
    put_str(out, &t.source);
    put_str(out, &t.target);
    put_opt_expr(out, &t.guard);
    put_list(out, t.set.as_slice(), put_field_expr);
}

/// A workflow: its id, its field, its initial states, then its transitions.
fn put_workflow(out: &mut Vec<u8>, w: &Workflow) {
    put_str(out, &w.id);
    put_nat(out, w.field);
    put_list(out, w.initial.as_slice(), put_string);
    put_list(out, w.transitions.as_slice(), put_wf_transition);
}

/// An expression as a list element: the encoder of a constraint in a list (`encList encExpr`).
fn put_expr_root(out: &mut Vec<u8>, e: &Expr) {
    put_expr(out, e);
}

fn put_type_entry(out: &mut Vec<u8>, e: &(u64, ObjType)) {
    put_nat(out, e.0);
    put_obj_type(out, &e.1);
}

/// A team: its label, its members, then its roles (`encTeamDef`).
fn put_team_def(out: &mut Vec<u8>, d: &TeamDef) {
    put_str(out, &d.label);
    put_u64_list(out, &d.members);
    put_u64_list(out, &d.roles);
}

fn put_team_entry(out: &mut Vec<u8>, e: &(u64, TeamDef)) {
    put_nat(out, e.0);
    put_team_def(out, &e.1);
}

/// An invitation: the address, the roles, the issuer, the expiry, the redeemer, the revoker
/// (`encInvitationDef`).
fn put_invitation_def(out: &mut Vec<u8>, d: &InvitationDef) {
    put_str(out, &d.address);
    put_u64_list(out, &d.roles);
    put_nat(out, d.created_by);
    put_nat(out, d.expires_at);
    put_opt_nat(out, d.redeemed_by);
    put_opt_nat(out, d.revoked_by);
}

fn put_invitation_entry(out: &mut Vec<u8>, e: &(u64, InvitationDef)) {
    put_nat(out, e.0);
    put_invitation_def(out, &e.1);
}

fn put_tenant(out: &mut Vec<u8>, t: &Tenant) {
    put_list(out, t.types.as_slice(), put_type_entry);
    put_list(out, t.members.as_slice(), put_member);
    put_list(out, t.grants.as_slice(), put_grant_entry);
    put_list(out, t.policies.as_slice(), put_policy);
    put_nat(out, t.next_id);
    put_limits(out, &t.limits);
    put_u64_list(out, &t.roles);
    put_list(out, t.teams.as_slice(), put_team_entry);
    put_list(out, t.invitations.as_slice(), put_invitation_entry);
    put_nat(out, t.revision);
    put_nat(out, t.event_seq);
    put_nat(out, t.schema_version);
    put_list(out, t.setting_defs.as_slice(), put_setting_def);
    put_list(out, t.overrides.as_slice(), put_setting_override);
    put_list(out, t.setting_revs.as_slice(), put_setting_rev);
    put_opt(out, &t.ledger_decls, put_ledger_decls);
    put_ledger_state(out, &t.ledger);
    put_list(out, t.ledger_policies.as_slice(), put_ledger_policy);
    put_list(out, t.operations.as_slice(), put_op_entry);
    put_list(out, t.triggers.as_slice(), put_trigger_entry);
    put_list(out, t.automations.as_slice(), put_automation_entry);
}

/// A trigger's event (`encTriggerOn`): tag `0` created, `1` updated (its type, then its transition if any),
/// `2` deleted, `3` ledger posted; each names a natural (an object type, or a ledger unit).
fn put_trigger_on(out: &mut Vec<u8>, t: &TriggerOn) {
    match t {
        TriggerOn::RecordCreated(ty) => {
            out.push(0);
            put_nat(out, *ty);
        }
        TriggerOn::RecordUpdated(ty, transition) => {
            out.push(1);
            put_nat(out, *ty);
            put_opt(out, transition, put_string);
        }
        TriggerOn::RecordDeleted(ty) => {
            out.push(2);
            put_nat(out, *ty);
        }
        TriggerOn::LedgerPosted(unit) => {
            out.push(3);
            put_nat(out, *unit);
        }
    }
}

/// An automation state (`encAutoState`): active `0`, draft `1`.
fn put_auto_state(out: &mut Vec<u8>, s: &AutoState) {
    out.push(match s {
        AutoState::Active => 0,
        AutoState::Draft => 1,
    });
}

/// A trigger (`encTriggerDef`): its id, its event, its optional condition, its automation, its paused flag
/// and its optional definer.
fn put_trigger_def(out: &mut Vec<u8>, t: &TriggerDef) {
    put_nat(out, t.id);
    put_trigger_on(out, &t.on);
    put_opt_expr(out, &t.condition);
    put_nat(out, t.automation);
    put_bool(out, t.paused);
    put_opt(out, &t.defined_by, put_u64);
}

/// A trigger entry (`encTriggerEntry`): its id, then the trigger.
fn put_trigger_entry(out: &mut Vec<u8>, e: &(u64, TriggerDef)) {
    put_nat(out, e.0);
    put_trigger_def(out, &e.1);
}

/// An automation (`encAutomationDef`): its id, its run-as roles, its optional run key, its optional definer,
/// its optional suspension role and its state.
fn put_automation_def(out: &mut Vec<u8>, a: &AutomationDef) {
    put_nat(out, a.id);
    put_u64_list(out, &a.run_as);
    put_opt_expr(out, &a.run_key);
    put_opt(out, &a.defined_by, put_u64);
    put_opt(out, &a.suspended_for, put_u64);
    put_auto_state(out, &a.state);
}

/// An automation entry (`encAutomationEntry`): its id, then the automation.
fn put_automation_entry(out: &mut Vec<u8>, e: &(u64, AutomationDef)) {
    put_nat(out, e.0);
    put_automation_def(out, &e.1);
}

/// An input of an operation (`encOpInput`).
fn put_op_input(out: &mut Vec<u8>, i: &OpInput) {
    put_str(out, &i.name);
    put_ty(out, &i.ty);
    put_bool(out, i.required);
}

/// The owner of a posting step (`encOpOwner`).
fn put_op_owner(out: &mut Vec<u8>, o: &OpOwner) {
    match o {
        OpOwner::Invoker => out.push(0),
        OpOwner::Pool(n) => {
            out.push(1);
            put_nat(out, *n);
        }
    }
}

/// A step of an operation (`encOpStep`).
fn put_op_step(out: &mut Vec<u8>, s: &OpStep) {
    match s {
        OpStep::Create { id, ty, fields } => {
            out.push(0);
            put_str(out, id);
            put_nat(out, *ty);
            put_list(out, fields.as_slice(), put_field_expr);
        }
        OpStep::Update {
            id,
            ty,
            record,
            fields,
        } => {
            out.push(1);
            put_str(out, id);
            put_nat(out, *ty);
            put_expr(out, record);
            put_list(out, fields.as_slice(), put_field_expr);
        }
        OpStep::Delete { id, ty, record } => {
            out.push(2);
            put_str(out, id);
            put_nat(out, *ty);
            put_expr(out, record);
        }
        OpStep::Post {
            id,
            owner,
            unit,
            amount,
            reason,
        } => {
            out.push(3);
            put_str(out, id);
            put_op_owner(out, owner);
            put_nat(out, *unit);
            put_expr(out, amount);
            put_str(out, reason);
        }
    }
}

/// A condition of an operation (`encOpCond`).
fn put_op_cond(out: &mut Vec<u8>, c: &OpCond) {
    put_str(out, &c.id);
    put_expr(out, &c.expr);
}

/// The subject of an operation: an object type and the name of the input (`encPair encNat encStr`).
fn put_op_subject(out: &mut Vec<u8>, s: &(u64, String)) {
    put_nat(out, s.0);
    put_str(out, &s.1);
}

/// An operation (`encOpDef`): its id, inputs, signed-in flag, roles, subject, conditions and steps.
fn put_op_def(out: &mut Vec<u8>, d: &OpDef) {
    put_str(out, &d.id);
    put_list(out, d.inputs.as_slice(), put_op_input);
    put_bool(out, d.signed_in);
    put_u64_list(out, &d.roles);
    put_opt(out, &d.subject, put_op_subject);
    put_list(out, d.conditions.as_slice(), put_op_cond);
    put_list(out, d.steps.as_slice(), put_op_step);
}

/// An operation with its name, as the tenant holds it (`encOpEntry`).
fn put_op_entry(out: &mut Vec<u8>, e: &(String, OpDef)) {
    put_str(out, &e.0);
    put_op_def(out, &e.1);
}

/// An input of an invocation: its name, then its value (`encPair encStr encVal`).
fn put_invoke_input(out: &mut Vec<u8>, e: &(String, Val)) {
    put_str(out, &e.0);
    put_val(out, &e.1);
}

/// A membership intent: its tag, then its fields (`encMemberIntent`).
fn put_member_intent(out: &mut Vec<u8>, j: &MemberIntent) {
    match j {
        MemberIntent::AssignRole { user, role } => {
            out.push(0);
            put_nat(out, *user);
            put_nat(out, *role);
        }
        MemberIntent::RevokeRole { user, role } => {
            out.push(1);
            put_nat(out, *user);
            put_nat(out, *role);
        }
        MemberIntent::DefineTeam { team, label } => {
            out.push(2);
            put_nat(out, *team);
            put_str(out, label);
        }
        MemberIntent::AssignTeamRole { team, role } => {
            out.push(3);
            put_nat(out, *team);
            put_nat(out, *role);
        }
        MemberIntent::RevokeTeamRole { team, role } => {
            out.push(4);
            put_nat(out, *team);
            put_nat(out, *role);
        }
        MemberIntent::AddTeamMember { team, user } => {
            out.push(5);
            put_nat(out, *team);
            put_nat(out, *user);
        }
        MemberIntent::RemoveTeamMember { team, user } => {
            out.push(6);
            put_nat(out, *team);
            put_nat(out, *user);
        }
        MemberIntent::CreateInvitation {
            invitation,
            address,
            roles,
            now,
        } => {
            out.push(7);
            put_nat(out, *invitation);
            put_str(out, address);
            put_u64_list(out, roles);
            put_nat(out, *now);
        }
        MemberIntent::RevokeInvitation { invitation } => {
            out.push(8);
            put_nat(out, *invitation);
        }
        MemberIntent::RedeemInvitation {
            invitation,
            now,
            verified,
        } => {
            out.push(9);
            put_nat(out, *invitation);
            put_nat(out, *now);
            match verified {
                None => out.push(0),
                Some(address) => {
                    out.push(1);
                    put_str(out, address);
                }
            }
        }
    }
}

/// A change of a schema change in general: tag `0` defines a field, `1` removes one, `2` defines an object type, `3`
/// removes one (`encChange`).
fn put_change(out: &mut Vec<u8>, c: &Change) {
    match c {
        Change::DefineField { ty, field, def } => {
            out.push(0);
            put_nat(out, *ty);
            put_nat(out, *field);
            put_field_def(out, def);
        }
        Change::RemoveField { ty, field } => {
            out.push(1);
            put_nat(out, *ty);
            put_nat(out, *field);
        }
        Change::DefineType { ty } => {
            out.push(2);
            put_nat(out, *ty);
        }
        Change::RemoveType { ty, delete_records } => {
            out.push(3);
            put_nat(out, *ty);
            put_bool(out, *delete_records);
        }
    }
}

/// An option (`encOpt`): a tag, then the value when there is one.
fn put_opt<T>(out: &mut Vec<u8>, v: &Option<T>, put: fn(&mut Vec<u8>, &T)) {
    match v {
        None => out.push(0),
        Some(x) => {
            out.push(1);
            put(out, x);
        }
    }
}

/// A setting level: one byte, platform `0`, tenant `1`, team `2`, person `3` (`encLevel`).
fn put_setting_level(out: &mut Vec<u8>, l: &SettingLevel) {
    out.push(match l {
        SettingLevel::Platform => 0,
        SettingLevel::Tenant => 1,
        SettingLevel::Team => 2,
        SettingLevel::Person => 3,
    });
}

/// A setting's type, one byte: flag `0`, whole number `1`, text `2` (`encSettingType`).
fn put_setting_type(out: &mut Vec<u8>, t: &SettingType) {
    out.push(match t {
        SettingType::Bool => 0,
        SettingType::Int => 1,
        SettingType::Str => 2,
    });
}

/// A subject: the tenant `0`, a team `1` and its number, a person `2` and its number (`encSubject`).
fn put_setting_subject(out: &mut Vec<u8>, s: &SettingSubject) {
    match s {
        SettingSubject::Tenant => out.push(0),
        SettingSubject::Team(t) => {
            out.push(1);
            put_nat(out, *t);
        }
        SettingSubject::Person(p) => {
            out.push(2);
            put_nat(out, *p);
        }
    }
}

/// What a setting allows: a closed list of values `0`, or a range of integers `1`, low then high (`encAllowed`).
fn put_allowed(out: &mut Vec<u8>, a: &SettingAllowed) {
    match a {
        SettingAllowed::Closed(values) => {
            out.push(0);
            put_list(out, values.as_slice(), put_val);
        }
        SettingAllowed::Range(lo, hi) => {
            out.push(1);
            put_int(out, *lo);
            put_int(out, *hi);
        }
    }
}

/// A setting definition: its name, its type, its levels, its lock as an option, its default and its allowed
/// values as an option (`encSettingDef`).
fn put_setting_def(out: &mut Vec<u8>, d: &SettingDef) {
    put_str(out, &d.name);
    put_setting_type(out, &d.ty);
    put_list(out, d.levels.as_slice(), put_setting_level);
    put_opt(out, &d.locked, put_setting_level);
    put_val(out, &d.default);
    put_opt(out, &d.allowed, put_allowed);
}

/// An override: its setting, its subject, its value and its revision (`encOverride`).
fn put_setting_override(out: &mut Vec<u8>, o: &SettingOverride) {
    put_str(out, &o.name);
    put_setting_subject(out, &o.subject);
    put_val(out, &o.value);
    put_nat(out, o.revision);
}

/// A setting's revision at the tenant: its name, then its revision (`encSettingRev`).
fn put_setting_rev(out: &mut Vec<u8>, e: &(String, u64)) {
    put_str(out, &e.0);
    put_nat(out, e.1);
}

/// An integer by reference, for the option and list combinators (`encInt`).
fn put_i64(out: &mut Vec<u8>, i: &i64) {
    put_int(out, *i);
}

/// A boolean by reference, for the option combinator (`encBool`).
fn put_flag(out: &mut Vec<u8>, b: &bool) {
    put_bool(out, *b);
}

/// A calendar period, one byte: hour `0`, day `1`, week `2`, month `3` (`encLedgerPeriod`).
fn put_ledger_period(out: &mut Vec<u8>, p: &LedgerPeriod) {
    out.push(match p {
        LedgerPeriod::Hour => 0,
        LedgerPeriod::Day => 1,
        LedgerPeriod::Week => 2,
        LedgerPeriod::Month => 3,
    });
}

/// An owner: a person `0` and its number, or a pool `1` and its number (`encLedgerOwner`).
fn put_ledger_owner(out: &mut Vec<u8>, o: &LedgerOwner) {
    match o {
        LedgerOwner::Person(p) => {
            out.push(0);
            put_nat(out, *p);
        }
        LedgerOwner::Pool(n) => {
            out.push(1);
            put_nat(out, *n);
        }
    }
}

/// An expiry: its period, then its number of periods (`encLedgerExpiry`).
fn put_ledger_expiry(out: &mut Vec<u8>, e: &LedgerExpiry) {
    put_ledger_period(out, &e.period);
    put_nat(out, e.periods);
}

/// A unit declaration: the unit, then its expiry as an option (`encLedgerUnitDecl`).
fn put_ledger_unit_decl(out: &mut Vec<u8>, u: &LedgerUnitDecl) {
    put_nat(out, u.unit);
    put_opt(out, &u.expiry, put_ledger_expiry);
}

/// A cap per period: its number, then its period (`encLedgerPeriodCap`).
fn put_period_cap(out: &mut Vec<u8>, c: &(u64, LedgerPeriod)) {
    put_nat(out, c.0);
    put_ledger_period(out, &c.1);
}

/// A limit: the unit, its owner, its scope, its floor, its cap, its cap per period and its additions flag, each
/// option as its encoder gives (`encLedgerLimit`).
fn put_ledger_limit(out: &mut Vec<u8>, l: &LedgerLimit) {
    put_nat(out, l.unit);
    put_opt(out, &l.owner, put_ledger_owner);
    put_str(out, &l.scope);
    put_opt(out, &l.floor, put_i64);
    put_opt(out, &l.cap, put_i64);
    put_opt(out, &l.period_cap, put_period_cap);
    put_opt(out, &l.additions_only, put_flag);
}

/// The declarations: the units, the pools and the limits of units (`encLedgerDecls`).
fn put_ledger_decls(out: &mut Vec<u8>, d: &LedgerDecls) {
    put_list(out, d.units.as_slice(), put_ledger_unit_decl);
    put_list(out, d.pools.as_slice(), put_u64);
    put_list(out, d.limits.as_slice(), put_ledger_limit);
}

/// A bucket: the start of its period, then the credit left (`encLedgerBucket`).
fn put_ledger_bucket(out: &mut Vec<u8>, b: &LedgerBucket) {
    put_int(out, b.period_start);
    put_nat(out, b.remaining);
}

/// A period total: its period, its start and its credits (`encLedgerPeriodTotal`).
fn put_ledger_period_total(out: &mut Vec<u8>, t: &LedgerPeriodTotal) {
    put_ledger_period(out, &t.period);
    put_int(out, t.start);
    put_nat(out, t.credits);
}

/// An account header: owner, unit, scope, balance, last sequence number, periods, buckets (`encLedgerAccount`).
fn put_ledger_account(out: &mut Vec<u8>, a: &LedgerAccount) {
    put_ledger_owner(out, &a.owner);
    put_nat(out, a.unit);
    put_str(out, &a.scope);
    put_int(out, a.balance);
    put_nat(out, a.last_seq);
    put_list(out, a.periods.as_slice(), put_ledger_period_total);
    put_list(out, a.buckets.as_slice(), put_ledger_bucket);
}

/// An entry: owner, unit, scope, sequence, amount, key, reason, the caller as an option, time, and the
/// corrected sequence as an option (`encLedgerEntry`).
fn put_ledger_entry(out: &mut Vec<u8>, e: &LedgerEntry) {
    put_ledger_owner(out, &e.owner);
    put_nat(out, e.unit);
    put_str(out, &e.scope);
    put_nat(out, e.sequence);
    put_int(out, e.amount);
    put_str(out, &e.key);
    put_str(out, &e.reason);
    put_opt_nat(out, e.caused_by);
    put_int(out, e.time);
    put_opt_nat(out, e.corrects);
    put_opt(out, &e.transaction, put_string);
}

/// The ledger state: its accounts, its journal and its limits (`encLedgerState`).
fn put_ledger_state(out: &mut Vec<u8>, s: &LedgerState) {
    put_list(out, s.accounts.as_slice(), put_ledger_account);
    put_list(out, s.journal.as_slice(), put_ledger_entry);
    put_list(out, s.limits.as_slice(), put_ledger_limit);
}

/// An atom: the owner-is-caller tag `0`, the unit tag `1` and its number, the scope tag `2` and its text
/// (`encLedgerAtom`).
fn put_ledger_atom(out: &mut Vec<u8>, a: &LedgerAtom) {
    match a {
        LedgerAtom::OwnerIsCaller => out.push(0),
        LedgerAtom::UnitIs(n) => {
            out.push(1);
            put_nat(out, *n);
        }
        LedgerAtom::ScopeIs(s) => {
            out.push(2);
            put_str(out, s);
        }
    }
}

/// A ledger policy: its role, its unit as an option, and its atoms (`encLedgerPolicy`).
fn put_ledger_policy(out: &mut Vec<u8>, p: &LedgerPolicy) {
    put_nat(out, p.role);
    put_opt(out, &p.unit, put_u64);
    put_list(out, p.conds.as_slice(), put_ledger_atom);
}

/// A posting of a `PostLedger` intent: owner, unit, amount, key, reason, the corrected sequence as an option,
/// and the scope (`encLedgerPosting`).
fn put_ledger_posting(out: &mut Vec<u8>, q: &LedgerPosting) {
    put_ledger_owner(out, &q.owner);
    put_nat(out, q.unit);
    put_int(out, q.amount);
    put_str(out, &q.key);
    put_str(out, &q.reason);
    put_opt_nat(out, q.corrects);
    put_str(out, &q.scope);
}

/// A leg of a transaction: owner, unit, amount and scope (`encLedgerLeg`).
fn put_ledger_leg(out: &mut Vec<u8>, l: &LedgerLeg) {
    put_ledger_owner(out, &l.owner);
    put_nat(out, l.unit);
    put_int(out, l.amount);
    put_str(out, &l.scope);
}

/// A transaction: operation, key, reason and legs (`encLedgerTx`).
fn put_ledger_tx(out: &mut Vec<u8>, t: &LedgerTx) {
    put_str(out, &t.operation);
    put_str(out, &t.key);
    put_str(out, &t.reason);
    put_list(out, t.legs.as_slice(), put_ledger_leg);
}

fn put_ctx(out: &mut Vec<u8>, c: &Ctx) {
    put_nat(out, c.tenant);
    put_principal(out, &c.principal);
    put_nat(out, c.hop);
}

fn put_intent(out: &mut Vec<u8>, i: &Intent) {
    match i {
        Intent::Create { ty, fields } => {
            out.push(0);
            put_nat(out, *ty);
            put_list(out, fields.as_slice(), put_field);
        }
        Intent::Update {
            ty,
            id,
            fields,
            expected,
        } => {
            out.push(1);
            put_nat(out, *ty);
            put_nat(out, *id);
            put_list(out, fields.as_slice(), put_field);
            put_opt_nat(out, *expected);
        }
        Intent::Delete { ty, id, expected } => {
            out.push(2);
            put_nat(out, *ty);
            put_nat(out, *id);
            put_opt_nat(out, *expected);
        }
        Intent::SetRequired { ty, field, strict } => {
            out.push(3);
            put_nat(out, *ty);
            put_nat(out, *field);
            put_bool(out, *strict);
        }
        Intent::SetUnique { ty, field, strict } => {
            out.push(4);
            put_nat(out, *ty);
            put_nat(out, *field);
            put_bool(out, *strict);
        }
        Intent::DefineUnique { ty, fields } => {
            out.push(5);
            put_nat(out, *ty);
            put_list(out, fields.as_slice(), put_u64);
        }
        Intent::CreateTenant { owner, name } => {
            out.push(6);
            put_nat(out, *owner);
            put_str(out, name);
        }
        Intent::Member(j) => {
            out.push(7);
            put_member_intent(out, j);
        }
        Intent::ChangeSchema {
            expected,
            strict,
            changes,
        } => {
            out.push(8);
            put_opt_nat(out, *expected);
            put_bool(out, *strict);
            put_list(out, changes.as_slice(), put_change);
        }
        Intent::WriteSetting {
            setting,
            level,
            team,
            value,
            expected_revision,
        } => {
            out.push(9);
            put_str(out, setting);
            put_setting_level(out, level);
            put_opt(out, team, put_u64);
            put_opt(out, value, put_val);
            put_nat(out, *expected_revision);
        }
        Intent::PostLedger { now, postings } => {
            out.push(10);
            put_int(out, *now);
            put_list(out, postings.as_slice(), put_ledger_posting);
        }
        Intent::InvokeOperation {
            now,
            operation,
            inputs,
            key,
        } => {
            out.push(13);
            put_int(out, *now);
            put_str(out, operation);
            put_list(out, inputs.as_slice(), put_invoke_input);
            put_str(out, key);
        }
        Intent::ExpireLedger { now, unit, cutoff } => {
            out.push(11);
            put_int(out, *now);
            put_nat(out, *unit);
            put_int(out, *cutoff);
        }
        Intent::PostLedgerTx { now, tx } => {
            out.push(12);
            put_int(out, *now);
            put_ledger_tx(out, tx);
        }
    }
}

fn put_refusal(out: &mut Vec<u8>, e: &Refusal) {
    out.push(match e {
        Refusal::NoTenant => 0,
        Refusal::Unauthorized => 1,
        Refusal::NotFound => 2,
        Refusal::Invalid => 3,
        Refusal::MigrationRejected => 4,
        Refusal::DanglingReference => 5,
        Refusal::StillReferenced => 6,
        Refusal::VersionConflict => 7,
        Refusal::LimitExceeded => 8,
        Refusal::UniqueViolated => 9,
        Refusal::InvalidSchema => 10,
        Refusal::UnknownObject => 11,
        Refusal::ConstraintViolated => 12,
        Refusal::Evaluation => 13,
        Refusal::InvalidIntent => 14,
        Refusal::UnknownRole => 15,
        Refusal::LastOwner => 16,
        Refusal::TenantExists => 17,
        Refusal::WorkflowViolation => 18,
        Refusal::SchemaConflict => 19,
        Refusal::SelfAction => 20,
    });
}

fn put_step_result(out: &mut Vec<u8>, s: &StepResult) {
    match s {
        StepResult::Refused(e) => {
            out.push(0);
            put_refusal(out, e);
        }
        StepResult::Accepted(t) => {
            out.push(1);
            put_tenant(out, t);
        }
    }
}

/// The canonical bytes of a value of the boundary: one encoding per value.
pub fn encode_val(v: &Val) -> Vec<u8> {
    let mut out = Vec::new();
    put_val(&mut out, v);
    out
}

/// The canonical bytes of a field type.
pub fn encode_ty(t: &Ty) -> Vec<u8> {
    let mut out = Vec::new();
    put_ty(&mut out, t);
    out
}

/// The canonical bytes of a field definition.
pub fn encode_field_def(fd: &FieldDef) -> Vec<u8> {
    let mut out = Vec::new();
    put_field_def(&mut out, fd);
    out
}

/// The canonical bytes of a record.
pub fn encode_record(r: &Record) -> Vec<u8> {
    let mut out = Vec::new();
    put_record(&mut out, r);
    out
}

/// The canonical bytes of a tenant.
pub fn encode_tenant(t: &Tenant) -> Vec<u8> {
    let mut out = Vec::new();
    put_tenant(&mut out, t);
    out
}

/// The canonical bytes of a policy.
pub fn encode_policy(p: &Policy) -> Vec<u8> {
    let mut out = Vec::new();
    put_policy(&mut out, p);
    out
}

/// The canonical bytes of a membership intent.
pub fn encode_member_intent(j: &MemberIntent) -> Vec<u8> {
    let mut out = Vec::new();
    put_member_intent(&mut out, j);
    out
}

/// The canonical bytes of a context.
pub fn encode_ctx(c: &Ctx) -> Vec<u8> {
    let mut out = Vec::new();
    put_ctx(&mut out, c);
    out
}

/// The canonical bytes of an intent.
pub fn encode_intent(i: &Intent) -> Vec<u8> {
    let mut out = Vec::new();
    put_intent(&mut out, i);
    out
}

/// The canonical bytes of a refusal.
pub fn encode_refusal(e: &Refusal) -> Vec<u8> {
    let mut out = Vec::new();
    put_refusal(&mut out, e);
    out
}

/// The canonical bytes of a step's result.
pub fn encode_step_result(s: &StepResult) -> Vec<u8> {
    let mut out = Vec::new();
    put_step_result(&mut out, s);
    out
}

// ----------------------------------------------------------------------------------------------
// Decoding
// ----------------------------------------------------------------------------------------------

/// The unread part of the input.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.remaining() {
            return None;
        }
        let buf: &'a [u8] = self.buf;
        let slice = &buf[self.pos..self.pos + n];
        self.pos += n;
        Some(slice)
    }
}

/// Decodes `bytes` with `get`, and refuses unless the whole input is consumed.
fn decode_all<T>(bytes: &[u8], get: fn(&mut Reader<'_>) -> Option<T>) -> Option<T> {
    let mut r = Reader { buf: bytes, pos: 0 };
    let value = get(&mut r)?;
    if r.remaining() == 0 {
        Some(value)
    } else {
        None
    }
}

/// A natural. Refuses a non-minimal encoding (a multi-byte natural ending in a zero byte) and one
/// that does not fit in `u64`.
fn get_nat(r: &mut Reader<'_>) -> Option<u64> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    let mut count: usize = 0;
    loop {
        let b = r.byte()?;
        count += 1;
        let group = u64::from(b & 0x7f);
        if shift >= 64 {
            return None;
        }
        let shifted = group << shift;
        if shifted >> shift != group {
            return None;
        }
        value |= shifted;
        if b & 0x80 == 0 {
            if count > 1 && b == 0 {
                return None;
            }
            return Some(value);
        }
        shift += 7;
    }
}

fn get_len(r: &mut Reader<'_>) -> Option<usize> {
    let n = get_nat(r)?;
    usize::try_from(n).ok()
}

fn get_int(r: &mut Reader<'_>) -> Option<i64> {
    let tag = r.byte()?;
    let m = get_nat(r)?;
    if m > i64::MAX as u64 {
        return None;
    }
    match tag {
        0 => Some(m as i64),
        1 => Some(!(m as i64)),
        _ => None,
    }
}

fn get_str(r: &mut Reader<'_>) -> Option<String> {
    let n = get_len(r)?;
    let bytes = r.take(n)?;
    String::from_utf8(bytes.to_vec()).ok()
}

fn get_bool(r: &mut Reader<'_>) -> Option<bool> {
    match r.byte()? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn get_opt_nat(r: &mut Reader<'_>) -> Option<Option<u64>> {
    match r.byte()? {
        0 => Some(None),
        1 => Some(Some(get_nat(r)?)),
        _ => None,
    }
}

/// A list: the count, then the elements. A count larger than the bytes left is refused before
/// anything is allocated: every element takes at least one byte.
fn get_list<T>(r: &mut Reader<'_>, get: fn(&mut Reader<'_>) -> Option<T>) -> Option<Vec<T>> {
    let n = get_len(r)?;
    if n > r.remaining() {
        return None;
    }
    let mut items = Vec::with_capacity(n);
    for _ in 0..n {
        items.push(get(r)?);
    }
    Some(items)
}

fn get_u64(r: &mut Reader<'_>) -> Option<u64> {
    get_nat(r)
}

fn get_u64_list(r: &mut Reader<'_>) -> Option<Vec<u64>> {
    get_list(r, get_u64)
}

fn get_elem(r: &mut Reader<'_>) -> Option<Elem> {
    match r.byte()? {
        0 => Some(Elem::Null),
        1 => Some(Elem::Int(get_int(r)?)),
        2 => Some(Elem::Str(get_str(r)?)),
        3 => Some(Elem::Bool(get_bool(r)?)),
        4 => Some(Elem::Ref(get_nat(r)?)),
        _ => None,
    }
}

fn get_val(r: &mut Reader<'_>) -> Option<Val> {
    match r.byte()? {
        0 => Some(Val::Null),
        1 => Some(Val::Int(get_int(r)?)),
        2 => Some(Val::Str(get_str(r)?)),
        3 => Some(Val::Bool(get_bool(r)?)),
        4 => Some(Val::Ref(get_nat(r)?)),
        5 => Some(Val::List(get_list(r, get_elem)?)),
        _ => None,
    }
}

fn get_field(r: &mut Reader<'_>) -> Option<(u64, Val)> {
    let id = get_nat(r)?;
    let v = get_val(r)?;
    Some((id, v))
}

fn get_ty(r: &mut Reader<'_>, depth: usize) -> Option<Ty> {
    if depth > MAX_TY_DEPTH {
        return None;
    }
    match r.byte()? {
        0 => Some(Ty::Int),
        1 => Some(Ty::Str),
        2 => Some(Ty::Bool),
        3 => Some(Ty::Ref(get_nat(r)?, OnDelete::Restrict)),
        4 => Some(Ty::Ref(get_nat(r)?, OnDelete::SetNull)),
        5 => Some(Ty::List(Box::new(get_ty(r, depth + 1)?))),
        _ => None,
    }
}

fn get_scope(r: &mut Reader<'_>) -> Option<Scope> {
    match r.byte()? {
        0 => Some(Scope::Record),
        1 => Some(Scope::Old),
        2 => Some(Scope::New),
        _ => None,
    }
}

fn get_cmp_op(r: &mut Reader<'_>) -> Option<CmpOp> {
    match r.byte()? {
        0 => Some(CmpOp::Eq),
        1 => Some(CmpOp::Ne),
        2 => Some(CmpOp::Lt),
        3 => Some(CmpOp::Le),
        4 => Some(CmpOp::Gt),
        5 => Some(CmpOp::Ge),
        _ => None,
    }
}

/// An expression. The depth of its nesting is bounded by [`MAX_EXPR_DEPTH`].
fn get_expr(r: &mut Reader<'_>, depth: usize) -> Option<Expr> {
    if depth > MAX_EXPR_DEPTH {
        return None;
    }
    match r.byte()? {
        0 => Some(Expr::Lit(get_val(r)?)),
        1 => {
            let s = get_scope(r)?;
            Some(Expr::Field(s, get_nat(r)?))
        }
        2 => Some(Expr::Principal),
        3 => {
            let op = get_cmp_op(r)?;
            let a = get_expr(r, depth + 1)?;
            let b = get_expr(r, depth + 1)?;
            Some(Expr::Cmp(op, Box::new(a), Box::new(b)))
        }
        4 => {
            let a = get_expr(r, depth + 1)?;
            let b = get_expr(r, depth + 1)?;
            Some(Expr::And(Box::new(a), Box::new(b)))
        }
        5 => {
            let a = get_expr(r, depth + 1)?;
            let b = get_expr(r, depth + 1)?;
            Some(Expr::Or(Box::new(a), Box::new(b)))
        }
        6 => Some(Expr::Not(Box::new(get_expr(r, depth + 1)?))),
        7 => Some(Expr::IsNull(Box::new(get_expr(r, depth + 1)?))),
        8 => Some(Expr::CreatedBy(get_scope(r)?)),
        9 => Some(Expr::Param(get_str(r)?)),
        _ => None,
    }
}

/// An expression as a list element, from the root of the nesting.
fn get_expr_root(r: &mut Reader<'_>) -> Option<Expr> {
    get_expr(r, 0)
}

fn get_field_def(r: &mut Reader<'_>) -> Option<FieldDef> {
    let ty = get_ty(r, 0)?;
    let required = get_bool(r)?;
    let unique = get_bool(r)?;
    let default = get_expr(r, 0)?;
    Some(FieldDef {
        ty,
        required,
        unique,
        default,
    })
}

fn get_schema_entry(r: &mut Reader<'_>) -> Option<(u64, FieldDef)> {
    let id = get_nat(r)?;
    let fd = get_field_def(r)?;
    Some((id, fd))
}

fn get_limits(r: &mut Reader<'_>) -> Option<Limits> {
    let max_string = get_nat(r)?;
    let max_list = get_nat(r)?;
    let max_unique = get_nat(r)?;
    let max_record = get_nat(r)?;
    Some(Limits {
        max_string,
        max_list,
        max_unique,
        max_record,
    })
}

fn get_principal(r: &mut Reader<'_>) -> Option<Principal> {
    match r.byte()? {
        0 => Some(Principal::User(get_nat(r)?)),
        1 => Some(Principal::System),
        2 => Some(Principal::Anonymous),
        _ => None,
    }
}

fn get_action(r: &mut Reader<'_>) -> Option<Action> {
    match r.byte()? {
        0 => Some(Action::Read),
        1 => Some(Action::Create),
        2 => Some(Action::Update),
        3 => Some(Action::Delete),
        4 => Some(Action::Manage),
        _ => None,
    }
}

fn get_resource(r: &mut Reader<'_>) -> Option<Resource> {
    match r.byte()? {
        0 => Some(Resource::Object(get_nat(r)?)),
        1 => Some(Resource::AnyObject),
        2 => Some(Resource::Schema),
        3 => Some(Resource::Members),
        _ => None,
    }
}

fn get_grant(r: &mut Reader<'_>) -> Option<Grant> {
    match r.byte()? {
        0 => Some(Grant::Capability(get_nat(r)?)),
        1 => {
            let ty = get_nat(r)?;
            Some(Grant::Object(ty, get_action(r)?))
        }
        _ => None,
    }
}

fn get_action_item(r: &mut Reader<'_>) -> Option<Action> {
    get_action(r)
}

fn get_policy(r: &mut Reader<'_>) -> Option<Policy> {
    let role = get_nat(r)?;
    let resource = get_resource(r)?;
    let actions = get_list(r, get_action_item)?;
    let condition = match r.byte()? {
        0 => None,
        1 => Some(get_expr(r, 0)?),
        _ => return None,
    };
    Some(Policy {
        role,
        resource,
        actions,
        condition,
    })
}

fn get_member(r: &mut Reader<'_>) -> Option<(u64, Vec<u64>)> {
    let id = get_nat(r)?;
    let roles = get_u64_list(r)?;
    Some((id, roles))
}

fn get_grant_entry(r: &mut Reader<'_>) -> Option<(u64, Grant)> {
    let role = get_nat(r)?;
    let grant = get_grant(r)?;
    Some((role, grant))
}

fn get_record(r: &mut Reader<'_>) -> Option<Record> {
    let owner = get_opt_nat(r)?;
    let version = get_nat(r)?;
    let fields = get_list(r, get_field)?;
    Some(Record {
        owner,
        version,
        fields,
    })
}

fn get_record_entry(r: &mut Reader<'_>) -> Option<(u64, Record)> {
    let id = get_nat(r)?;
    let rec = get_record(r)?;
    Some((id, rec))
}

fn get_obj_type(r: &mut Reader<'_>) -> Option<ObjType> {
    let schema = get_list(r, get_schema_entry)?;
    let uniques = get_list(r, get_u64_list)?;
    let records = get_list(r, get_record_entry)?;
    let constraints = get_list(r, get_expr_root)?;
    let workflows = get_list(r, get_workflow)?;
    Some(ObjType {
        schema,
        uniques,
        records,
        constraints,
        workflows,
    })
}

/// An optional expression: a tag (`0` none, `1` some), then the expression.
fn get_opt_expr(r: &mut Reader<'_>) -> Option<Option<Expr>> {
    match r.byte()? {
        0 => Some(None),
        1 => Some(Some(get_expr_root(r)?)),
        _ => None,
    }
}

fn get_field_expr(r: &mut Reader<'_>) -> Option<(u64, Expr)> {
    let f = get_nat(r)?;
    let e = get_expr_root(r)?;
    Some((f, e))
}

fn get_wf_transition(r: &mut Reader<'_>) -> Option<WfTransition> {
    let name = get_str(r)?;
    let source = get_str(r)?;
    let target = get_str(r)?;
    let guard = get_opt_expr(r)?;
    let set = get_list(r, get_field_expr)?;
    Some(WfTransition {
        name,
        source,
        target,
        guard,
        set,
    })
}

fn get_workflow(r: &mut Reader<'_>) -> Option<Workflow> {
    let id = get_str(r)?;
    let field = get_nat(r)?;
    let initial = get_list(r, get_str)?;
    let transitions = get_list(r, get_wf_transition)?;
    Some(Workflow {
        id,
        field,
        initial,
        transitions,
    })
}

fn get_type_entry(r: &mut Reader<'_>) -> Option<(u64, ObjType)> {
    let id = get_nat(r)?;
    let obj = get_obj_type(r)?;
    Some((id, obj))
}

fn get_team_def(r: &mut Reader<'_>) -> Option<TeamDef> {
    let label = get_str(r)?;
    let members = get_u64_list(r)?;
    let roles = get_u64_list(r)?;
    Some(TeamDef {
        label,
        members,
        roles,
    })
}

fn get_team_entry(r: &mut Reader<'_>) -> Option<(u64, TeamDef)> {
    let id = get_nat(r)?;
    Some((id, get_team_def(r)?))
}

fn get_invitation_def(r: &mut Reader<'_>) -> Option<InvitationDef> {
    let address = get_str(r)?;
    let roles = get_u64_list(r)?;
    let created_by = get_nat(r)?;
    let expires_at = get_nat(r)?;
    let redeemed_by = get_opt_nat(r)?;
    let revoked_by = get_opt_nat(r)?;
    Some(InvitationDef {
        address,
        roles,
        created_by,
        expires_at,
        redeemed_by,
        revoked_by,
    })
}

fn get_invitation_entry(r: &mut Reader<'_>) -> Option<(u64, InvitationDef)> {
    let id = get_nat(r)?;
    Some((id, get_invitation_def(r)?))
}

fn get_tenant(r: &mut Reader<'_>) -> Option<Tenant> {
    let types = get_list(r, get_type_entry)?;
    let members = get_list(r, get_member)?;
    let grants = get_list(r, get_grant_entry)?;
    let policies = get_list(r, get_policy)?;
    let next_id = get_nat(r)?;
    let limits = get_limits(r)?;
    let roles = get_u64_list(r)?;
    let teams = get_list(r, get_team_entry)?;
    let invitations = get_list(r, get_invitation_entry)?;
    let revision = get_nat(r)?;
    let event_seq = get_nat(r)?;
    let schema_version = get_nat(r)?;
    let setting_defs = get_list(r, get_setting_def)?;
    let overrides = get_list(r, get_setting_override)?;
    let setting_revs = get_list(r, get_setting_rev)?;
    let ledger_decls = get_opt(r, get_ledger_decls)?;
    let ledger = get_ledger_state(r)?;
    let ledger_policies = get_list(r, get_ledger_policy)?;
    let operations = get_list(r, get_op_entry)?;
    let triggers = get_list(r, get_trigger_entry)?;
    let automations = get_list(r, get_automation_entry)?;
    Some(Tenant {
        types,
        members,
        grants,
        policies,
        next_id,
        limits,
        roles,
        teams,
        invitations,
        revision,
        event_seq,
        schema_version,
        setting_defs,
        overrides,
        setting_revs,
        ledger_decls,
        ledger,
        ledger_policies,
        operations,
        triggers,
        automations,
    })
}

/// A trigger's event (`decTriggerOn`): tags `0` to `3`, each with its natural (and the transition for `1`).
fn get_trigger_on(r: &mut Reader<'_>) -> Option<TriggerOn> {
    match r.byte()? {
        0 => Some(TriggerOn::RecordCreated(get_nat(r)?)),
        1 => {
            let ty = get_nat(r)?;
            let transition = get_opt(r, get_str)?;
            Some(TriggerOn::RecordUpdated(ty, transition))
        }
        2 => Some(TriggerOn::RecordDeleted(get_nat(r)?)),
        3 => Some(TriggerOn::LedgerPosted(get_nat(r)?)),
        _ => None,
    }
}

/// An automation state (`decAutoState`): active `0`, draft `1`.
fn get_auto_state(r: &mut Reader<'_>) -> Option<AutoState> {
    match r.byte()? {
        0 => Some(AutoState::Active),
        1 => Some(AutoState::Draft),
        _ => None,
    }
}

/// A trigger (`decTriggerDef`).
fn get_trigger_def(r: &mut Reader<'_>) -> Option<TriggerDef> {
    let id = get_nat(r)?;
    let on = get_trigger_on(r)?;
    let condition = get_opt_expr(r)?;
    let automation = get_nat(r)?;
    let paused = get_bool(r)?;
    let defined_by = get_opt(r, get_u64)?;
    Some(TriggerDef {
        id,
        on,
        condition,
        automation,
        paused,
        defined_by,
    })
}

/// A trigger entry (`decTriggerEntry`).
fn get_trigger_entry(r: &mut Reader<'_>) -> Option<(u64, TriggerDef)> {
    let id = get_nat(r)?;
    let def = get_trigger_def(r)?;
    Some((id, def))
}

/// An automation (`decAutomationDef`).
fn get_automation_def(r: &mut Reader<'_>) -> Option<AutomationDef> {
    let id = get_nat(r)?;
    let run_as = get_u64_list(r)?;
    let run_key = get_opt_expr(r)?;
    let defined_by = get_opt(r, get_u64)?;
    let suspended_for = get_opt(r, get_u64)?;
    let state = get_auto_state(r)?;
    Some(AutomationDef {
        id,
        run_as,
        run_key,
        defined_by,
        suspended_for,
        state,
    })
}

/// An automation entry (`decAutomationEntry`).
fn get_automation_entry(r: &mut Reader<'_>) -> Option<(u64, AutomationDef)> {
    let id = get_nat(r)?;
    let def = get_automation_def(r)?;
    Some((id, def))
}

fn get_op_input(r: &mut Reader<'_>) -> Option<OpInput> {
    let name = get_str(r)?;
    let ty = get_ty(r, 0)?;
    let required = get_bool(r)?;
    Some(OpInput { name, ty, required })
}

fn get_op_owner(r: &mut Reader<'_>) -> Option<OpOwner> {
    match r.byte()? {
        0 => Some(OpOwner::Invoker),
        1 => Some(OpOwner::Pool(get_nat(r)?)),
        _ => None,
    }
}

fn get_op_step(r: &mut Reader<'_>) -> Option<OpStep> {
    match r.byte()? {
        0 => {
            let id = get_str(r)?;
            let ty = get_nat(r)?;
            let fields = get_list(r, get_field_expr)?;
            Some(OpStep::Create { id, ty, fields })
        }
        1 => {
            let id = get_str(r)?;
            let ty = get_nat(r)?;
            let record = get_expr_root(r)?;
            let fields = get_list(r, get_field_expr)?;
            Some(OpStep::Update {
                id,
                ty,
                record,
                fields,
            })
        }
        2 => {
            let id = get_str(r)?;
            let ty = get_nat(r)?;
            let record = get_expr_root(r)?;
            Some(OpStep::Delete { id, ty, record })
        }
        3 => {
            let id = get_str(r)?;
            let owner = get_op_owner(r)?;
            let unit = get_nat(r)?;
            let amount = get_expr_root(r)?;
            let reason = get_str(r)?;
            Some(OpStep::Post {
                id,
                owner,
                unit,
                amount,
                reason,
            })
        }
        _ => None,
    }
}

fn get_op_cond(r: &mut Reader<'_>) -> Option<OpCond> {
    let id = get_str(r)?;
    let expr = get_expr_root(r)?;
    Some(OpCond { id, expr })
}

fn get_op_subject(r: &mut Reader<'_>) -> Option<(u64, String)> {
    let ty = get_nat(r)?;
    let input = get_str(r)?;
    Some((ty, input))
}

fn get_op_def(r: &mut Reader<'_>) -> Option<OpDef> {
    let id = get_str(r)?;
    let inputs = get_list(r, get_op_input)?;
    let signed_in = get_bool(r)?;
    let roles = get_u64_list(r)?;
    let subject = get_opt(r, get_op_subject)?;
    let conditions = get_list(r, get_op_cond)?;
    let steps = get_list(r, get_op_step)?;
    Some(OpDef {
        id,
        inputs,
        signed_in,
        roles,
        subject,
        conditions,
        steps,
    })
}

fn get_op_entry(r: &mut Reader<'_>) -> Option<(String, OpDef)> {
    let name = get_str(r)?;
    let def = get_op_def(r)?;
    Some((name, def))
}

fn get_invoke_input(r: &mut Reader<'_>) -> Option<(String, Val)> {
    let name = get_str(r)?;
    let val = get_val(r)?;
    Some((name, val))
}

fn get_member_intent(r: &mut Reader<'_>) -> Option<MemberIntent> {
    match r.byte()? {
        0 => {
            let user = get_nat(r)?;
            let role = get_nat(r)?;
            Some(MemberIntent::AssignRole { user, role })
        }
        1 => {
            let user = get_nat(r)?;
            let role = get_nat(r)?;
            Some(MemberIntent::RevokeRole { user, role })
        }
        2 => {
            let team = get_nat(r)?;
            let label = get_str(r)?;
            Some(MemberIntent::DefineTeam { team, label })
        }
        3 => {
            let team = get_nat(r)?;
            let role = get_nat(r)?;
            Some(MemberIntent::AssignTeamRole { team, role })
        }
        4 => {
            let team = get_nat(r)?;
            let role = get_nat(r)?;
            Some(MemberIntent::RevokeTeamRole { team, role })
        }
        5 => {
            let team = get_nat(r)?;
            let user = get_nat(r)?;
            Some(MemberIntent::AddTeamMember { team, user })
        }
        6 => {
            let team = get_nat(r)?;
            let user = get_nat(r)?;
            Some(MemberIntent::RemoveTeamMember { team, user })
        }
        7 => {
            let invitation = get_nat(r)?;
            let address = get_str(r)?;
            let roles = get_u64_list(r)?;
            let now = get_nat(r)?;
            Some(MemberIntent::CreateInvitation {
                invitation,
                address,
                roles,
                now,
            })
        }
        8 => {
            let invitation = get_nat(r)?;
            Some(MemberIntent::RevokeInvitation { invitation })
        }
        9 => {
            let invitation = get_nat(r)?;
            let now = get_nat(r)?;
            let verified = match r.byte()? {
                0 => None,
                1 => Some(get_str(r)?),
                _ => return None,
            };
            Some(MemberIntent::RedeemInvitation {
                invitation,
                now,
                verified,
            })
        }
        _ => None,
    }
}

fn get_change(r: &mut Reader<'_>) -> Option<Change> {
    match r.byte()? {
        0 => {
            let ty = get_nat(r)?;
            let field = get_nat(r)?;
            let def = get_field_def(r)?;
            Some(Change::DefineField { ty, field, def })
        }
        1 => {
            let ty = get_nat(r)?;
            let field = get_nat(r)?;
            Some(Change::RemoveField { ty, field })
        }
        2 => Some(Change::DefineType { ty: get_nat(r)? }),
        3 => {
            let ty = get_nat(r)?;
            let delete_records = get_bool(r)?;
            Some(Change::RemoveType { ty, delete_records })
        }
        _ => None,
    }
}

/// An option: a tag (`0` none, `1` some), then the value.
fn get_opt<T>(r: &mut Reader<'_>, get: fn(&mut Reader<'_>) -> Option<T>) -> Option<Option<T>> {
    match r.byte()? {
        0 => Some(None),
        1 => Some(Some(get(r)?)),
        _ => None,
    }
}

fn get_setting_level(r: &mut Reader<'_>) -> Option<SettingLevel> {
    match r.byte()? {
        0 => Some(SettingLevel::Platform),
        1 => Some(SettingLevel::Tenant),
        2 => Some(SettingLevel::Team),
        3 => Some(SettingLevel::Person),
        _ => None,
    }
}

fn get_setting_type(r: &mut Reader<'_>) -> Option<SettingType> {
    match r.byte()? {
        0 => Some(SettingType::Bool),
        1 => Some(SettingType::Int),
        2 => Some(SettingType::Str),
        _ => None,
    }
}

fn get_setting_subject(r: &mut Reader<'_>) -> Option<SettingSubject> {
    match r.byte()? {
        0 => Some(SettingSubject::Tenant),
        1 => Some(SettingSubject::Team(get_nat(r)?)),
        2 => Some(SettingSubject::Person(get_nat(r)?)),
        _ => None,
    }
}

fn get_allowed(r: &mut Reader<'_>) -> Option<SettingAllowed> {
    match r.byte()? {
        0 => Some(SettingAllowed::Closed(get_list(r, get_val)?)),
        1 => {
            let lo = get_int(r)?;
            let hi = get_int(r)?;
            Some(SettingAllowed::Range(lo, hi))
        }
        _ => None,
    }
}

fn get_setting_def(r: &mut Reader<'_>) -> Option<SettingDef> {
    let name = get_str(r)?;
    let ty = get_setting_type(r)?;
    let levels = get_list(r, get_setting_level)?;
    let locked = get_opt(r, get_setting_level)?;
    let default = get_val(r)?;
    let allowed = get_opt(r, get_allowed)?;
    Some(SettingDef {
        name,
        ty,
        levels,
        locked,
        default,
        allowed,
    })
}

fn get_setting_override(r: &mut Reader<'_>) -> Option<SettingOverride> {
    let name = get_str(r)?;
    let subject = get_setting_subject(r)?;
    let value = get_val(r)?;
    let revision = get_nat(r)?;
    Some(SettingOverride {
        name,
        subject,
        value,
        revision,
    })
}

fn get_setting_rev(r: &mut Reader<'_>) -> Option<(String, u64)> {
    let name = get_str(r)?;
    let revision = get_nat(r)?;
    Some((name, revision))
}

fn get_ledger_period(r: &mut Reader<'_>) -> Option<LedgerPeriod> {
    match r.byte()? {
        0 => Some(LedgerPeriod::Hour),
        1 => Some(LedgerPeriod::Day),
        2 => Some(LedgerPeriod::Week),
        3 => Some(LedgerPeriod::Month),
        _ => None,
    }
}

fn get_ledger_owner(r: &mut Reader<'_>) -> Option<LedgerOwner> {
    match r.byte()? {
        0 => Some(LedgerOwner::Person(get_nat(r)?)),
        1 => Some(LedgerOwner::Pool(get_nat(r)?)),
        _ => None,
    }
}

fn get_ledger_expiry(r: &mut Reader<'_>) -> Option<LedgerExpiry> {
    let period = get_ledger_period(r)?;
    let periods = get_nat(r)?;
    Some(LedgerExpiry { period, periods })
}

fn get_ledger_unit_decl(r: &mut Reader<'_>) -> Option<LedgerUnitDecl> {
    let unit = get_nat(r)?;
    let expiry = get_opt(r, get_ledger_expiry)?;
    Some(LedgerUnitDecl { unit, expiry })
}

fn get_period_cap(r: &mut Reader<'_>) -> Option<(u64, LedgerPeriod)> {
    let n = get_nat(r)?;
    let period = get_ledger_period(r)?;
    Some((n, period))
}

fn get_ledger_limit(r: &mut Reader<'_>) -> Option<LedgerLimit> {
    let unit = get_nat(r)?;
    let owner = get_opt(r, get_ledger_owner)?;
    let scope = get_str(r)?;
    let floor = get_opt(r, get_int)?;
    let cap = get_opt(r, get_int)?;
    let period_cap = get_opt(r, get_period_cap)?;
    let additions_only = get_opt(r, get_bool)?;
    Some(LedgerLimit {
        unit,
        owner,
        scope,
        floor,
        cap,
        period_cap,
        additions_only,
    })
}

fn get_ledger_decls(r: &mut Reader<'_>) -> Option<LedgerDecls> {
    let units = get_list(r, get_ledger_unit_decl)?;
    let pools = get_list(r, get_u64)?;
    let limits = get_list(r, get_ledger_limit)?;
    Some(LedgerDecls {
        units,
        pools,
        limits,
    })
}

fn get_ledger_bucket(r: &mut Reader<'_>) -> Option<LedgerBucket> {
    let period_start = get_int(r)?;
    let remaining = get_nat(r)?;
    Some(LedgerBucket {
        period_start,
        remaining,
    })
}

fn get_ledger_period_total(r: &mut Reader<'_>) -> Option<LedgerPeriodTotal> {
    let period = get_ledger_period(r)?;
    let start = get_int(r)?;
    let credits = get_nat(r)?;
    Some(LedgerPeriodTotal {
        period,
        start,
        credits,
    })
}

fn get_ledger_account(r: &mut Reader<'_>) -> Option<LedgerAccount> {
    let owner = get_ledger_owner(r)?;
    let unit = get_nat(r)?;
    let scope = get_str(r)?;
    let balance = get_int(r)?;
    let last_seq = get_nat(r)?;
    let periods = get_list(r, get_ledger_period_total)?;
    let buckets = get_list(r, get_ledger_bucket)?;
    Some(LedgerAccount {
        owner,
        unit,
        scope,
        balance,
        last_seq,
        periods,
        buckets,
    })
}

fn get_ledger_entry(r: &mut Reader<'_>) -> Option<LedgerEntry> {
    let owner = get_ledger_owner(r)?;
    let unit = get_nat(r)?;
    let scope = get_str(r)?;
    let sequence = get_nat(r)?;
    let amount = get_int(r)?;
    let key = get_str(r)?;
    let reason = get_str(r)?;
    let caused_by = get_opt_nat(r)?;
    let time = get_int(r)?;
    let corrects = get_opt_nat(r)?;
    let transaction = get_opt(r, get_str)?;
    Some(LedgerEntry {
        owner,
        unit,
        scope,
        sequence,
        amount,
        key,
        reason,
        caused_by,
        time,
        corrects,
        transaction,
    })
}

fn get_ledger_state(r: &mut Reader<'_>) -> Option<LedgerState> {
    let accounts = get_list(r, get_ledger_account)?;
    let journal = get_list(r, get_ledger_entry)?;
    let limits = get_list(r, get_ledger_limit)?;
    Some(LedgerState {
        accounts,
        journal,
        limits,
    })
}

fn get_ledger_atom(r: &mut Reader<'_>) -> Option<LedgerAtom> {
    match r.byte()? {
        0 => Some(LedgerAtom::OwnerIsCaller),
        1 => Some(LedgerAtom::UnitIs(get_nat(r)?)),
        2 => Some(LedgerAtom::ScopeIs(get_str(r)?)),
        _ => None,
    }
}

fn get_ledger_policy(r: &mut Reader<'_>) -> Option<LedgerPolicy> {
    let role = get_nat(r)?;
    let unit = get_opt(r, get_u64)?;
    let conds = get_list(r, get_ledger_atom)?;
    Some(LedgerPolicy { role, unit, conds })
}

fn get_ledger_posting(r: &mut Reader<'_>) -> Option<LedgerPosting> {
    let owner = get_ledger_owner(r)?;
    let unit = get_nat(r)?;
    let amount = get_int(r)?;
    let key = get_str(r)?;
    let reason = get_str(r)?;
    let corrects = get_opt_nat(r)?;
    let scope = get_str(r)?;
    Some(LedgerPosting {
        owner,
        unit,
        amount,
        key,
        reason,
        corrects,
        scope,
    })
}

fn get_ledger_leg(r: &mut Reader<'_>) -> Option<LedgerLeg> {
    let owner = get_ledger_owner(r)?;
    let unit = get_nat(r)?;
    let amount = get_int(r)?;
    let scope = get_str(r)?;
    Some(LedgerLeg {
        owner,
        unit,
        amount,
        scope,
    })
}

fn get_ledger_tx(r: &mut Reader<'_>) -> Option<LedgerTx> {
    let operation = get_str(r)?;
    let key = get_str(r)?;
    let reason = get_str(r)?;
    let legs = get_list(r, get_ledger_leg)?;
    Some(LedgerTx {
        operation,
        key,
        reason,
        legs,
    })
}

fn get_ctx(r: &mut Reader<'_>) -> Option<Ctx> {
    let tenant = get_nat(r)?;
    let principal = get_principal(r)?;
    let hop = get_nat(r)?;
    Some(Ctx {
        tenant,
        principal,
        hop,
    })
}

fn get_intent(r: &mut Reader<'_>) -> Option<Intent> {
    match r.byte()? {
        0 => {
            let ty = get_nat(r)?;
            let fields = get_list(r, get_field)?;
            Some(Intent::Create { ty, fields })
        }
        1 => {
            let ty = get_nat(r)?;
            let id = get_nat(r)?;
            let fields = get_list(r, get_field)?;
            let expected = get_opt_nat(r)?;
            Some(Intent::Update {
                ty,
                id,
                fields,
                expected,
            })
        }
        2 => {
            let ty = get_nat(r)?;
            let id = get_nat(r)?;
            let expected = get_opt_nat(r)?;
            Some(Intent::Delete { ty, id, expected })
        }
        3 => {
            let ty = get_nat(r)?;
            let field = get_nat(r)?;
            let strict = get_bool(r)?;
            Some(Intent::SetRequired { ty, field, strict })
        }
        4 => {
            let ty = get_nat(r)?;
            let field = get_nat(r)?;
            let strict = get_bool(r)?;
            Some(Intent::SetUnique { ty, field, strict })
        }
        5 => {
            let ty = get_nat(r)?;
            let fields = get_u64_list(r)?;
            Some(Intent::DefineUnique { ty, fields })
        }
        6 => {
            let owner = get_nat(r)?;
            let name = get_str(r)?;
            Some(Intent::CreateTenant { owner, name })
        }
        7 => Some(Intent::Member(get_member_intent(r)?)),
        8 => {
            let expected = get_opt_nat(r)?;
            let strict = get_bool(r)?;
            let changes = get_list(r, get_change)?;
            Some(Intent::ChangeSchema {
                expected,
                strict,
                changes,
            })
        }
        9 => {
            let setting = get_str(r)?;
            let level = get_setting_level(r)?;
            let team = get_opt(r, get_u64)?;
            let value = get_opt(r, get_val)?;
            let expected_revision = get_nat(r)?;
            Some(Intent::WriteSetting {
                setting,
                level,
                team,
                value,
                expected_revision,
            })
        }
        10 => {
            let now = get_int(r)?;
            let postings = get_list(r, get_ledger_posting)?;
            Some(Intent::PostLedger { now, postings })
        }
        11 => {
            let now = get_int(r)?;
            let unit = get_nat(r)?;
            let cutoff = get_int(r)?;
            Some(Intent::ExpireLedger { now, unit, cutoff })
        }
        12 => {
            let now = get_int(r)?;
            let tx = get_ledger_tx(r)?;
            Some(Intent::PostLedgerTx { now, tx })
        }
        13 => {
            let now = get_int(r)?;
            let operation = get_str(r)?;
            let inputs = get_list(r, get_invoke_input)?;
            let key = get_str(r)?;
            Some(Intent::InvokeOperation {
                now,
                operation,
                inputs,
                key,
            })
        }
        _ => None,
    }
}

fn get_refusal(r: &mut Reader<'_>) -> Option<Refusal> {
    match r.byte()? {
        0 => Some(Refusal::NoTenant),
        1 => Some(Refusal::Unauthorized),
        2 => Some(Refusal::NotFound),
        3 => Some(Refusal::Invalid),
        4 => Some(Refusal::MigrationRejected),
        5 => Some(Refusal::DanglingReference),
        6 => Some(Refusal::StillReferenced),
        7 => Some(Refusal::VersionConflict),
        8 => Some(Refusal::LimitExceeded),
        9 => Some(Refusal::UniqueViolated),
        10 => Some(Refusal::InvalidSchema),
        11 => Some(Refusal::UnknownObject),
        12 => Some(Refusal::ConstraintViolated),
        13 => Some(Refusal::Evaluation),
        14 => Some(Refusal::InvalidIntent),
        15 => Some(Refusal::UnknownRole),
        16 => Some(Refusal::LastOwner),
        17 => Some(Refusal::TenantExists),
        18 => Some(Refusal::WorkflowViolation),
        19 => Some(Refusal::SchemaConflict),
        20 => Some(Refusal::SelfAction),
        _ => None,
    }
}

fn get_step_result(r: &mut Reader<'_>) -> Option<StepResult> {
    match r.byte()? {
        0 => Some(StepResult::Refused(get_refusal(r)?)),
        1 => Some(StepResult::Accepted(get_tenant(r)?)),
        _ => None,
    }
}

/// The value of `bytes`, if they are exactly the canonical encoding of a value.
pub fn decode_val(bytes: &[u8]) -> Option<Val> {
    decode_all(bytes, get_val)
}

fn get_ty_root(r: &mut Reader<'_>) -> Option<Ty> {
    get_ty(r, 0)
}

/// The field type of `bytes`, if they are exactly its canonical encoding.
pub fn decode_ty(bytes: &[u8]) -> Option<Ty> {
    decode_all(bytes, get_ty_root)
}

/// The field definition of `bytes`, if they are exactly its canonical encoding.
pub fn decode_field_def(bytes: &[u8]) -> Option<FieldDef> {
    decode_all(bytes, get_field_def)
}

/// The record of `bytes`, if they are exactly its canonical encoding.
pub fn decode_record(bytes: &[u8]) -> Option<Record> {
    decode_all(bytes, get_record)
}

/// The tenant of `bytes`, if they are exactly its canonical encoding.
pub fn decode_tenant(bytes: &[u8]) -> Option<Tenant> {
    decode_all(bytes, get_tenant)
}

/// The policy of `bytes`, if they are exactly its canonical encoding.
pub fn decode_policy(bytes: &[u8]) -> Option<Policy> {
    decode_all(bytes, get_policy)
}

/// The membership intent of `bytes`, if they are exactly its canonical encoding.
pub fn decode_member_intent(bytes: &[u8]) -> Option<MemberIntent> {
    decode_all(bytes, get_member_intent)
}

/// The context of `bytes`, if they are exactly its canonical encoding.
pub fn decode_ctx(bytes: &[u8]) -> Option<Ctx> {
    decode_all(bytes, get_ctx)
}

/// The intent of `bytes`, if they are exactly its canonical encoding.
pub fn decode_intent(bytes: &[u8]) -> Option<Intent> {
    decode_all(bytes, get_intent)
}

/// The refusal of `bytes`, if they are exactly its canonical encoding.
pub fn decode_refusal(bytes: &[u8]) -> Option<Refusal> {
    decode_all(bytes, get_refusal)
}

/// The step result of `bytes`, if they are exactly its canonical encoding.
pub fn decode_step_result(bytes: &[u8]) -> Option<StepResult> {
    decode_all(bytes, get_step_result)
}
