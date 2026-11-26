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
/// `and` (`4`), `or` (`5`), `not` (`6`), `isNull` (`7`) and `$created_by` (`8`, the scope).
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

/// Everything one tenant owns, as the boundary carries it: its object types by id, its members
/// (each user with its roles), what each role grants, its policies, the next record id, the size
/// bounds, (slice 7) the roles its schema defines, its teams and its invitations, and (slice 9) its
/// revision, its event sequence and its schema version. The owner is a member holding role `0`.
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
}

/// The acting context of a step: which tenant, and who is acting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ctx {
    pub tenant: u64,
    pub principal: Principal,
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

fn put_ctx(out: &mut Vec<u8>, c: &Ctx) {
    put_nat(out, c.tenant);
    put_principal(out, &c.principal);
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
    })
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

fn get_ctx(r: &mut Reader<'_>) -> Option<Ctx> {
    let tenant = get_nat(r)?;
    let principal = get_principal(r)?;
    Some(Ctx { tenant, principal })
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
