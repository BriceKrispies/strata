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

/// What a policy names: an object type (`0`, its id), every object type (`1`) or the schema (`2`).
/// Lean's `Resource`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Object(u64),
    AnyObject,
    Schema,
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

/// An object type of a tenant: its schema, its composite unique constraints, its records, and its
/// constraints (slice 5), in the kernel's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjType {
    pub schema: Vec<(u64, FieldDef)>,
    pub uniques: Vec<Vec<u64>>,
    pub records: Vec<(u64, Record)>,
    pub constraints: Vec<Expr>,
}

/// Everything one tenant owns, as the boundary carries it: its object types by id, its members
/// (each user with its roles), what each role grants, its policies, the next record id, and the size
/// bounds. The owner is a member holding role `0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tenant {
    pub types: Vec<(u64, ObjType)>,
    pub members: Vec<(u64, Vec<u64>)>,
    pub grants: Vec<(u64, Grant)>,
    pub policies: Vec<Policy>,
    pub next_id: u64,
    pub limits: Limits,
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

/// A resource: an object type `0` and its id, every object type `1`, the schema `2` (`encResource`).
fn put_resource(out: &mut Vec<u8>, r: &Resource) {
    match r {
        Resource::Object(ty) => {
            out.push(0);
            put_nat(out, *ty);
        }
        Resource::AnyObject => out.push(1),
        Resource::Schema => out.push(2),
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
}

/// An expression as a list element: the encoder of a constraint in a list (`encList encExpr`).
fn put_expr_root(out: &mut Vec<u8>, e: &Expr) {
    put_expr(out, e);
}

fn put_type_entry(out: &mut Vec<u8>, e: &(u64, ObjType)) {
    put_nat(out, e.0);
    put_obj_type(out, &e.1);
}

fn put_tenant(out: &mut Vec<u8>, t: &Tenant) {
    put_list(out, t.types.as_slice(), put_type_entry);
    put_list(out, t.members.as_slice(), put_member);
    put_list(out, t.grants.as_slice(), put_grant_entry);
    put_list(out, t.policies.as_slice(), put_policy);
    put_nat(out, t.next_id);
    put_limits(out, &t.limits);
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
    Some(ObjType {
        schema,
        uniques,
        records,
        constraints,
    })
}

fn get_type_entry(r: &mut Reader<'_>) -> Option<(u64, ObjType)> {
    let id = get_nat(r)?;
    let obj = get_obj_type(r)?;
    Some((id, obj))
}

fn get_tenant(r: &mut Reader<'_>) -> Option<Tenant> {
    let types = get_list(r, get_type_entry)?;
    let members = get_list(r, get_member)?;
    let grants = get_list(r, get_grant_entry)?;
    let policies = get_list(r, get_policy)?;
    let next_id = get_nat(r)?;
    let limits = get_limits(r)?;
    Some(Tenant {
        types,
        members,
        grants,
        policies,
        next_id,
        limits,
    })
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
