//! Compiled Lean called from Rust: the bytes come back as the Lean function computes them, from
//! one thread and from many tokio worker threads at once.

use strata_lean_kernel_ffi::codec::{
    decode_ctx, decode_field_def, decode_intent, decode_member_intent, decode_policy,
    decode_record, decode_refusal, decode_step_result, decode_tenant, decode_ty, decode_val,
    encode_ctx, encode_field_def, encode_intent, encode_member_intent, encode_policy,
    encode_record, encode_refusal, encode_step_result, encode_tenant, encode_ty, encode_val,
    Action, CmpOp, Ctx, Elem, Expr, FieldDef, Grant, Intent, InvitationDef, Limits, MemberIntent,
    ObjType, OnDelete, Policy, Principal, Record, Refusal, Resource, Scope, StepResult, TeamDef,
    Tenant, Ty, Val, MAX_EXPR_DEPTH, MAX_TY_DEPTH,
};
use strata_lean_kernel_ffi::{step, step_reference};

#[test]
fn the_lean_step_returns_the_bytes_lean_computes() {
    assert_eq!(step(&[]), vec![0]);
    assert_eq!(step(&[1, 2, 3]), vec![3, 2, 1, 6]);
    assert_eq!(step(&[200, 100]), vec![100, 200, 44]);
    let big: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
    assert_eq!(step(&big), step_reference(&big));
}

#[test]
fn many_tokio_worker_threads_call_lean_at_once() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .build()
        .unwrap();
    rt.block_on(async {
        let mut tasks = Vec::new();
        for t in 0..64u32 {
            tasks.push(tokio::task::spawn_blocking(move || {
                for i in 0..200u32 {
                    let input: Vec<u8> = (0..(t * 7 + i) % 513).map(|k| (k ^ t) as u8).collect();
                    assert_eq!(step(&input), step_reference(&input), "thread {t}, call {i}");
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
    });
}

// ----------------------------------------------------------------------------------------------
// The boundary codec. The bytes below are the ones `lean/Strata/Codec/Fixtures.lean` proves the
// Lean encoder produces for the same values.
// ----------------------------------------------------------------------------------------------

/// The fixture tenant of `lean/Strata/Codec/Fixtures.lean` (`fxTenant`): one object type, `1`,
/// with one string field, no constraints and no records.
fn fixture_tenant() -> Tenant {
    Tenant {
        types: vec![(
            1,
            ObjType {
                schema: vec![(
                    1,
                    FieldDef {
                        ty: Ty::Str,
                        required: true,
                        unique: false,
                        default: Expr::Lit(Val::Null),
                    },
                )],
                uniques: vec![],
                records: vec![],
                constraints: vec![],
            },
        )],
        members: vec![(7, vec![0])],
        grants: vec![],
        policies: vec![],
        next_id: 1,
        limits: Limits::KERNEL,
        roles: vec![0, 1],
        teams: vec![],
        invitations: vec![],
    }
}

const FIXTURE_TENANT: &[u8] = &[
    1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1, 7, 1, 0, 0, 0, 1, 128, 128, 4, 232, 7, 200, 1, 128, 128,
    16, 2, 0, 1, 0, 0,
];

/// The fixture tenant with a role `2`, a team and an invitation (`fxTenantM` in
/// `lean/Strata/Codec/Fixtures.lean`): team `5` labelled `ab` with member 7 holding role 2, and
/// invitation `9` to `a@b` for role 2, issued by 7, expiring at 300 and revoked by 7.
fn fixture_tenant_members() -> Tenant {
    Tenant {
        roles: vec![0, 1, 2],
        teams: vec![(
            5,
            TeamDef {
                label: "ab".into(),
                members: vec![7],
                roles: vec![2],
            },
        )],
        invitations: vec![(
            9,
            InvitationDef {
                address: "a@b".into(),
                roles: vec![2],
                created_by: 7,
                expires_at: 300,
                redeemed_by: None,
                revoked_by: Some(7),
            },
        )],
        ..fixture_tenant()
    }
}

const FIXTURE_TENANT_MEMBERS: &[u8] = &[
    1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1, 7, 1, 0, 0, 0, 1, 128, 128, 4, 232, 7, 200, 1, 128, 128,
    16, 3, 0, 1, 2, 1, 5, 2, 97, 98, 1, 7, 1, 2, 1, 9, 3, 97, 64, 98, 1, 2, 7, 172, 2, 0, 1, 7,
];

#[test]
fn codec_fixtures_match_the_lean_encoder() {
    assert_eq!(encode_val(&Val::Null), vec![0]);
    assert_eq!(encode_val(&Val::Int(-1)), vec![1, 1, 0]);
    assert_eq!(encode_val(&Val::Str("ab".into())), vec![2, 2, 97, 98]);
    assert_eq!(encode_val(&Val::Ref(300)), vec![4, 172, 2]);
    assert_eq!(
        encode_val(&Val::List(vec![Elem::Int(5), Elem::Null])),
        vec![5, 2, 1, 0, 5, 0]
    );
    assert_eq!(
        encode_ty(&Ty::List(Box::new(Ty::Ref(0, OnDelete::SetNull)))),
        vec![5, 4, 0]
    );
    assert_eq!(
        encode_ty(&Ty::Ref(300, OnDelete::Restrict)),
        vec![3, 172, 2]
    );
    let field = FieldDef {
        ty: Ty::Int,
        required: false,
        unique: true,
        default: Expr::Lit(Val::Int(-2)),
    };
    assert_eq!(encode_field_def(&field), vec![0, 0, 1, 0, 1, 1, 1]);
    let record = Record {
        owner: Some(4),
        version: 1,
        fields: vec![(2, Val::Bool(true))],
    };
    assert_eq!(encode_record(&record), vec![1, 4, 1, 1, 2, 3, 1]);
    let anonymous_record = Record {
        owner: None,
        version: 1,
        fields: vec![(2, Val::Bool(true))],
    };
    assert_eq!(encode_record(&anonymous_record), vec![0, 1, 1, 2, 3, 1]);
    assert_eq!(encode_tenant(&fixture_tenant()), FIXTURE_TENANT);
    assert_eq!(
        encode_tenant(&fixture_tenant_members()),
        FIXTURE_TENANT_MEMBERS
    );
    // The membership intents and refusals of slice 7 (`fix_intent_*` and `fix_err_*` in
    // `Fixtures.lean`).
    assert_eq!(
        encode_intent(&Intent::CreateTenant {
            owner: 7,
            name: "ab".into()
        }),
        vec![6, 7, 2, 97, 98]
    );
    assert_eq!(
        encode_intent(&Intent::Member(MemberIntent::AssignRole {
            user: 9,
            role: 2
        })),
        vec![7, 0, 9, 2]
    );
    assert_eq!(
        encode_intent(&Intent::Member(MemberIntent::DefineTeam {
            team: 5,
            label: "ab".into()
        })),
        vec![7, 2, 5, 2, 97, 98]
    );
    assert_eq!(
        encode_intent(&Intent::Member(MemberIntent::CreateInvitation {
            invitation: 3,
            address: "a@b".into(),
            roles: vec![2],
            now: 300
        })),
        vec![7, 7, 3, 3, 97, 64, 98, 1, 2, 172, 2]
    );
    assert_eq!(
        encode_member_intent(&MemberIntent::RevokeInvitation { invitation: 3 }),
        vec![8, 3]
    );
    assert_eq!(
        encode_intent(&Intent::Member(MemberIntent::RedeemInvitation {
            invitation: 3,
            now: 300,
            verified: Some("a@b".into())
        })),
        vec![7, 9, 3, 172, 2, 1, 3, 97, 64, 98]
    );
    assert_eq!(
        encode_intent(&Intent::Member(MemberIntent::RedeemInvitation {
            invitation: 3,
            now: 300,
            verified: None
        })),
        vec![7, 9, 3, 172, 2, 0]
    );
    assert_eq!(encode_refusal(&Refusal::InvalidIntent), vec![14]);
    assert_eq!(encode_refusal(&Refusal::UnknownRole), vec![15]);
    assert_eq!(encode_refusal(&Refusal::LastOwner), vec![16]);
    assert_eq!(encode_refusal(&Refusal::TenantExists), vec![17]);
    assert_eq!(
        decode_tenant(FIXTURE_TENANT_MEMBERS),
        Some(fixture_tenant_members())
    );
    assert_eq!(
        decode_member_intent(&[9, 3, 172, 2, 0]),
        Some(MemberIntent::RedeemInvitation {
            invitation: 3,
            now: 300,
            verified: None
        })
    );
    assert_eq!(
        decode_intent(&[6, 7, 2, 97, 98]),
        Some(Intent::CreateTenant {
            owner: 7,
            name: "ab".into()
        })
    );
    // A policy naming the tenant's membership: role 1, resource tag 3, one action (manage, tag 4),
    // no condition (`fix_policy_members` in `Fixtures.lean`).
    let members_policy = Policy {
        role: 1,
        resource: Resource::Members,
        actions: vec![Action::Manage],
        condition: None,
    };
    assert_eq!(encode_policy(&members_policy), vec![1, 3, 1, 4, 0]);
    assert_eq!(decode_policy(&[1, 3, 1, 4, 0]), Some(members_policy));
    assert_eq!(
        encode_ctx(&Ctx {
            tenant: 3,
            principal: Principal::User(9)
        }),
        vec![3, 0, 9]
    );
    assert_eq!(
        encode_ctx(&Ctx {
            tenant: 3,
            principal: Principal::System
        }),
        vec![3, 1]
    );
    assert_eq!(
        encode_ctx(&Ctx {
            tenant: 3,
            principal: Principal::Anonymous
        }),
        vec![3, 2]
    );
    assert_eq!(
        encode_intent(&Intent::SetRequired {
            ty: 1,
            field: 2,
            strict: false
        }),
        vec![3, 1, 2, 0]
    );
    let update = Intent::Update {
        ty: 1,
        id: 5,
        fields: vec![(1, Val::Str("x".into()))],
        expected: None,
    };
    assert_eq!(encode_intent(&update), vec![1, 1, 5, 1, 1, 2, 1, 120, 0]);
    assert_eq!(
        encode_intent(&Intent::Create {
            ty: 1,
            fields: vec![]
        }),
        vec![0, 1, 0]
    );
    assert_eq!(encode_refusal(&Refusal::LimitExceeded), vec![8]);
    assert_eq!(encode_refusal(&Refusal::UnknownObject), vec![11]);
    assert_eq!(encode_refusal(&Refusal::ConstraintViolated), vec![12]);
    assert_eq!(encode_refusal(&Refusal::Evaluation), vec![13]);
    assert_eq!(
        encode_step_result(&StepResult::Refused(Refusal::Unauthorized)),
        vec![0, 1]
    );
    let accepted = [vec![1u8], FIXTURE_TENANT.to_vec()].concat();
    assert_eq!(
        encode_step_result(&StepResult::Accepted(fixture_tenant())),
        accepted
    );

    // The same bytes decode back to the same values.
    assert_eq!(decode_val(&[4, 172, 2]), Some(Val::Ref(300)));
    assert_eq!(
        decode_val(&[5, 2, 1, 0, 5, 0]),
        Some(Val::List(vec![Elem::Int(5), Elem::Null]))
    );
    assert_eq!(decode_field_def(&[0, 0, 1, 0, 1, 1, 1]), Some(field));
    assert_eq!(decode_record(&[1, 4, 1, 1, 2, 3, 1]), Some(record));
    assert_eq!(decode_tenant(FIXTURE_TENANT), Some(fixture_tenant()));
    assert_eq!(decode_intent(&[1, 1, 5, 1, 1, 2, 1, 120, 0]), Some(update));
    assert_eq!(
        decode_ty(&[5, 4, 0]),
        Some(Ty::List(Box::new(Ty::Ref(0, OnDelete::SetNull))))
    );
    assert_eq!(
        decode_ty(&[3, 172, 2]),
        Some(Ty::Ref(300, OnDelete::Restrict))
    );
    assert_eq!(
        decode_ctx(&[3, 0, 9]),
        Some(Ctx {
            tenant: 3,
            principal: Principal::User(9)
        })
    );
    assert_eq!(
        decode_step_result(&accepted),
        Some(StepResult::Accepted(fixture_tenant()))
    );
}

/// A small seeded generator (xorshift64): the same values on every run.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    fn coin(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

fn gen_string(rng: &mut Rng) -> String {
    const CHARS: [char; 7] = ['a', 'z', ' ', '\u{e9}', '\u{2713}', '\u{1F600}', '0'];
    let len = rng.below(6);
    let mut s = String::new();
    for _ in 0..len {
        let c = CHARS[rng.below(CHARS.len() as u64) as usize];
        s.push(c);
    }
    s
}

fn gen_int(rng: &mut Rng) -> i64 {
    match rng.below(6) {
        0 => 0,
        1 => i64::MIN,
        2 => i64::MAX,
        3 => -(rng.below(1000) as i64),
        4 => rng.below(1000) as i64,
        _ => rng.next_u64() as i64,
    }
}

fn gen_nat(rng: &mut Rng) -> u64 {
    match rng.below(5) {
        0 => 0,
        1 => rng.below(200),
        2 => rng.below(1 << 21),
        3 => u64::MAX - rng.below(4),
        _ => rng.next_u64(),
    }
}

fn gen_elem(rng: &mut Rng) -> Elem {
    match rng.below(5) {
        0 => Elem::Null,
        1 => Elem::Int(gen_int(rng)),
        2 => Elem::Str(gen_string(rng)),
        3 => Elem::Bool(rng.coin()),
        _ => Elem::Ref(gen_nat(rng)),
    }
}

fn gen_val(rng: &mut Rng) -> Val {
    match rng.below(6) {
        0 => Val::Null,
        1 => Val::Int(gen_int(rng)),
        2 => Val::Str(gen_string(rng)),
        3 => Val::Bool(rng.coin()),
        4 => Val::Ref(gen_nat(rng)),
        _ => {
            let n = rng.below(4);
            let mut items = Vec::new();
            for _ in 0..n {
                items.push(gen_elem(rng));
            }
            Val::List(items)
        }
    }
}

fn gen_ty(rng: &mut Rng, depth: u32) -> Ty {
    if depth < 2 && rng.below(4) == 0 {
        return Ty::List(Box::new(gen_ty(rng, depth + 1)));
    }
    match rng.below(5) {
        0 => Ty::Int,
        1 => Ty::Str,
        2 => Ty::Bool,
        3 => Ty::Ref(gen_nat(rng), OnDelete::Restrict),
        _ => Ty::Ref(gen_nat(rng), OnDelete::SetNull),
    }
}

fn gen_scope(rng: &mut Rng) -> Scope {
    match rng.below(3) {
        0 => Scope::Record,
        1 => Scope::Old,
        _ => Scope::New,
    }
}

fn gen_cmp_op(rng: &mut Rng) -> CmpOp {
    match rng.below(6) {
        0 => CmpOp::Eq,
        1 => CmpOp::Ne,
        2 => CmpOp::Lt,
        3 => CmpOp::Le,
        4 => CmpOp::Gt,
        _ => CmpOp::Ge,
    }
}

/// An expression of bounded nesting: literals, reads, the principal, comparisons, the connectives.
fn gen_expr(rng: &mut Rng, depth: usize) -> Expr {
    if depth >= 3 {
        return Expr::Lit(gen_val(rng));
    }
    match rng.below(10) {
        0 => Expr::Lit(gen_val(rng)),
        1 => Expr::Field(gen_scope(rng), gen_nat(rng)),
        2 => Expr::Principal,
        3 => Expr::Cmp(
            gen_cmp_op(rng),
            Box::new(gen_expr(rng, depth + 1)),
            Box::new(gen_expr(rng, depth + 1)),
        ),
        4 => Expr::And(
            Box::new(gen_expr(rng, depth + 1)),
            Box::new(gen_expr(rng, depth + 1)),
        ),
        5 => Expr::Or(
            Box::new(gen_expr(rng, depth + 1)),
            Box::new(gen_expr(rng, depth + 1)),
        ),
        6 => Expr::Not(Box::new(gen_expr(rng, depth + 1))),
        7 => Expr::IsNull(Box::new(gen_expr(rng, depth + 1))),
        8 => Expr::CreatedBy(gen_scope(rng)),
        _ => Expr::Lit(Val::Null),
    }
}

fn gen_field_def(rng: &mut Rng) -> FieldDef {
    let ty = gen_ty(rng, 0);
    let required = rng.coin();
    let unique = rng.coin();
    let default = gen_expr(rng, 0);
    FieldDef {
        ty,
        required,
        unique,
        default,
    }
}

fn gen_fields(rng: &mut Rng) -> Vec<(u64, Val)> {
    let n = rng.below(3);
    let mut fields = Vec::new();
    for _ in 0..n {
        let id = gen_nat(rng);
        let v = gen_val(rng);
        fields.push((id, v));
    }
    fields
}

fn gen_record(rng: &mut Rng) -> Record {
    let owner = gen_opt(rng);
    let version = gen_nat(rng);
    let fields = gen_fields(rng);
    Record {
        owner,
        version,
        fields,
    }
}

fn gen_ids(rng: &mut Rng, max_len: u64) -> Vec<u64> {
    let n = rng.below(max_len);
    let mut ids = Vec::new();
    for _ in 0..n {
        ids.push(gen_nat(rng));
    }
    ids
}

fn gen_obj_type(rng: &mut Rng) -> ObjType {
    let mut schema = Vec::new();
    for _ in 0..rng.below(3) {
        let id = gen_nat(rng);
        let fd = gen_field_def(rng);
        schema.push((id, fd));
    }
    let mut uniques = Vec::new();
    for _ in 0..rng.below(3) {
        uniques.push(gen_ids(rng, 4));
    }
    let mut records = Vec::new();
    for _ in 0..rng.below(3) {
        let id = gen_nat(rng);
        let rec = gen_record(rng);
        records.push((id, rec));
    }
    let mut constraints = Vec::new();
    for _ in 0..rng.below(3) {
        constraints.push(gen_expr(rng, 0));
    }
    ObjType {
        schema,
        uniques,
        records,
        constraints,
    }
}

fn gen_tenant(rng: &mut Rng) -> Tenant {
    let mut types = Vec::new();
    for _ in 0..rng.below(3) {
        let id = gen_nat(rng);
        let obj = gen_obj_type(rng);
        types.push((id, obj));
    }
    let mut members = Vec::new();
    for _ in 0..rng.below(3) {
        let id = gen_nat(rng);
        let roles = gen_ids(rng, 3);
        members.push((id, roles));
    }
    let mut grants = Vec::new();
    for _ in 0..rng.below(3) {
        let role = gen_nat(rng);
        let grant = gen_grant(rng);
        grants.push((role, grant));
    }
    let mut policies = Vec::new();
    for _ in 0..rng.below(3) {
        policies.push(gen_policy(rng));
    }
    let next_id = gen_nat(rng);
    let limits = Limits {
        max_string: gen_nat(rng),
        max_list: gen_nat(rng),
        max_unique: gen_nat(rng),
        max_record: gen_nat(rng),
    };
    let roles = gen_ids(rng, 4);
    let mut teams = Vec::new();
    for _ in 0..rng.below(3) {
        let id = gen_nat(rng);
        teams.push((
            id,
            TeamDef {
                label: gen_string(rng),
                members: gen_ids(rng, 3),
                roles: gen_ids(rng, 3),
            },
        ));
    }
    let mut invitations = Vec::new();
    for _ in 0..rng.below(3) {
        let id = gen_nat(rng);
        invitations.push((
            id,
            InvitationDef {
                address: gen_string(rng),
                roles: gen_ids(rng, 3),
                created_by: gen_nat(rng),
                expires_at: gen_nat(rng),
                redeemed_by: gen_opt(rng),
                revoked_by: gen_opt(rng),
            },
        ));
    }
    Tenant {
        types,
        members,
        grants,
        policies,
        next_id,
        limits,
        roles,
        teams,
        invitations,
    }
}

fn gen_principal(rng: &mut Rng) -> Principal {
    match rng.below(3) {
        0 => Principal::User(gen_nat(rng)),
        1 => Principal::System,
        _ => Principal::Anonymous,
    }
}

fn gen_action(rng: &mut Rng) -> Action {
    match rng.below(5) {
        0 => Action::Read,
        1 => Action::Create,
        2 => Action::Update,
        3 => Action::Delete,
        _ => Action::Manage,
    }
}

fn gen_resource(rng: &mut Rng) -> Resource {
    match rng.below(4) {
        0 => Resource::Object(gen_nat(rng)),
        1 => Resource::AnyObject,
        2 => Resource::Schema,
        _ => Resource::Members,
    }
}

fn gen_grant(rng: &mut Rng) -> Grant {
    if rng.coin() {
        Grant::Capability(gen_nat(rng))
    } else {
        let ty = gen_nat(rng);
        Grant::Object(ty, gen_action(rng))
    }
}

fn gen_policy(rng: &mut Rng) -> Policy {
    let role = gen_nat(rng);
    let resource = gen_resource(rng);
    let mut actions = Vec::new();
    for _ in 0..rng.below(4) {
        actions.push(gen_action(rng));
    }
    let condition = if rng.coin() {
        Some(gen_expr(rng, 0))
    } else {
        None
    };
    Policy {
        role,
        resource,
        actions,
        condition,
    }
}

fn gen_opt(rng: &mut Rng) -> Option<u64> {
    if rng.coin() {
        Some(gen_nat(rng))
    } else {
        None
    }
}

fn gen_member_intent(rng: &mut Rng) -> MemberIntent {
    match rng.below(10) {
        0 => MemberIntent::AssignRole {
            user: gen_nat(rng),
            role: gen_nat(rng),
        },
        1 => MemberIntent::RevokeRole {
            user: gen_nat(rng),
            role: gen_nat(rng),
        },
        2 => MemberIntent::DefineTeam {
            team: gen_nat(rng),
            label: gen_string(rng),
        },
        3 => MemberIntent::AssignTeamRole {
            team: gen_nat(rng),
            role: gen_nat(rng),
        },
        4 => MemberIntent::RevokeTeamRole {
            team: gen_nat(rng),
            role: gen_nat(rng),
        },
        5 => MemberIntent::AddTeamMember {
            team: gen_nat(rng),
            user: gen_nat(rng),
        },
        6 => MemberIntent::RemoveTeamMember {
            team: gen_nat(rng),
            user: gen_nat(rng),
        },
        7 => MemberIntent::CreateInvitation {
            invitation: gen_nat(rng),
            address: gen_string(rng),
            roles: gen_ids(rng, 3),
            now: gen_nat(rng),
        },
        8 => MemberIntent::RevokeInvitation {
            invitation: gen_nat(rng),
        },
        _ => MemberIntent::RedeemInvitation {
            invitation: gen_nat(rng),
            now: gen_nat(rng),
            verified: if rng.coin() {
                Some(gen_string(rng))
            } else {
                None
            },
        },
    }
}

fn gen_intent(rng: &mut Rng) -> Intent {
    match rng.below(8) {
        6 => Intent::CreateTenant {
            owner: gen_nat(rng),
            name: gen_string(rng),
        },
        7 => Intent::Member(gen_member_intent(rng)),
        0 => Intent::Create {
            ty: gen_nat(rng),
            fields: gen_fields(rng),
        },
        1 => {
            let ty = gen_nat(rng);
            let id = gen_nat(rng);
            let fields = gen_fields(rng);
            let expected = gen_opt(rng);
            Intent::Update {
                ty,
                id,
                fields,
                expected,
            }
        }
        2 => {
            let ty = gen_nat(rng);
            let id = gen_nat(rng);
            let expected = gen_opt(rng);
            Intent::Delete { ty, id, expected }
        }
        3 => {
            let ty = gen_nat(rng);
            let field = gen_nat(rng);
            let strict = rng.coin();
            Intent::SetRequired { ty, field, strict }
        }
        4 => {
            let ty = gen_nat(rng);
            let field = gen_nat(rng);
            let strict = rng.coin();
            Intent::SetUnique { ty, field, strict }
        }
        _ => {
            let ty = gen_nat(rng);
            Intent::DefineUnique {
                ty,
                fields: gen_ids(rng, 4),
            }
        }
    }
}

fn gen_refusal(rng: &mut Rng) -> Refusal {
    match rng.below(18) {
        0 => Refusal::NoTenant,
        1 => Refusal::Unauthorized,
        2 => Refusal::NotFound,
        3 => Refusal::Invalid,
        4 => Refusal::MigrationRejected,
        5 => Refusal::DanglingReference,
        6 => Refusal::StillReferenced,
        7 => Refusal::VersionConflict,
        8 => Refusal::LimitExceeded,
        9 => Refusal::UniqueViolated,
        10 => Refusal::InvalidSchema,
        11 => Refusal::UnknownObject,
        12 => Refusal::ConstraintViolated,
        13 => Refusal::Evaluation,
        14 => Refusal::InvalidIntent,
        15 => Refusal::UnknownRole,
        16 => Refusal::LastOwner,
        _ => Refusal::TenantExists,
    }
}

/// Encodes `value`, decodes the bytes back to an equal value, and encodes that again to the same
/// bytes: one encoding per value.
fn check_round_trip<T: PartialEq + std::fmt::Debug>(
    value: &T,
    encode: fn(&T) -> Vec<u8>,
    decode: fn(&[u8]) -> Option<T>,
) {
    let bytes = encode(value);
    let back = decode(&bytes).expect("the canonical bytes of a value decode");
    assert_eq!(&back, value);
    assert_eq!(encode(&back), bytes, "re-encoding changed the bytes");
}

/// No proper prefix of a canonical encoding decodes, and neither does the encoding with a byte
/// appended.
fn check_refuses_damage<T: PartialEq + std::fmt::Debug>(
    bytes: &[u8],
    decode: fn(&[u8]) -> Option<T>,
) {
    for cut in 0..bytes.len() {
        assert!(
            decode(&bytes[..cut]).is_none(),
            "a prefix of length {cut} decoded"
        );
    }
    let mut longer = bytes.to_vec();
    longer.push(0);
    assert!(decode(&longer).is_none(), "a trailing byte was accepted");
}

#[test]
fn generated_values_round_trip_and_decode_only_canonically() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..300 {
        let v = gen_val(&mut rng);
        check_round_trip(&v, encode_val, decode_val);
        let t = gen_ty(&mut rng, 0);
        check_round_trip(&t, encode_ty, decode_ty);
        let fd = gen_field_def(&mut rng);
        check_round_trip(&fd, encode_field_def, decode_field_def);
        let r = gen_record(&mut rng);
        check_round_trip(&r, encode_record, decode_record);
        let c = Ctx {
            tenant: gen_nat(&mut rng),
            principal: gen_principal(&mut rng),
        };
        check_round_trip(&c, encode_ctx, decode_ctx);
        let i = gen_intent(&mut rng);
        check_round_trip(&i, encode_intent, decode_intent);
        let e = gen_refusal(&mut rng);
        check_round_trip(&e, encode_refusal, decode_refusal);
    }
    for _ in 0..40 {
        let t = gen_tenant(&mut rng);
        check_round_trip(&t, encode_tenant, decode_tenant);
        let s = if rng.coin() {
            StepResult::Accepted(t)
        } else {
            StepResult::Refused(gen_refusal(&mut rng))
        };
        check_round_trip(&s, encode_step_result, decode_step_result);
    }
}

#[test]
fn damaged_encodings_do_not_decode() {
    check_refuses_damage(
        &encode_val(&Val::List(vec![Elem::Int(-7), Elem::Str("é".into())])),
        decode_val,
    );
    check_refuses_damage(
        &encode_intent(&Intent::Update {
            ty: 1,
            id: 5,
            fields: vec![(1, Val::Str("x".into()))],
            expected: Some(300),
        }),
        decode_intent,
    );
    check_refuses_damage(FIXTURE_TENANT, decode_tenant);
    check_refuses_damage(FIXTURE_TENANT_MEMBERS, decode_tenant);
    check_refuses_damage(
        &encode_intent(&Intent::Member(MemberIntent::CreateInvitation {
            invitation: 3,
            address: "a@b".into(),
            roles: vec![2],
            now: 300,
        })),
        decode_intent,
    );
}

#[test]
fn non_canonical_and_out_of_range_bytes_are_refused() {
    // A natural with a zero last group (the minimal form of 0 is one byte).
    assert_eq!(decode_val(&[4, 0x80, 0x00]), None);
    // A negative integer whose natural has a zero last group.
    assert_eq!(decode_val(&[1, 1, 0x80, 0x00]), None);
    // Invalid UTF-8 in a string.
    assert_eq!(decode_val(&[2, 1, 0xFF]), None);
    // A boolean that is neither 0 nor 1; an unknown tag; an integer sign tag of 3.
    assert_eq!(decode_val(&[3, 2]), None);
    assert_eq!(decode_val(&[6]), None);
    assert_eq!(decode_val(&[1, 3, 5]), None);
    // A list whose count is longer than the bytes that follow.
    assert_eq!(decode_val(&[5, 9]), None);
    // A natural that does not fit in 64 bits.
    assert_eq!(
        decode_val(&[4, 255, 255, 255, 255, 255, 255, 255, 255, 255, 127]),
        None
    );
    // The largest natural, and its one encoding, decodes.
    assert_eq!(
        decode_val(&[4, 255, 255, 255, 255, 255, 255, 255, 255, 255, 1]),
        Some(Val::Ref(u64::MAX))
    );
    assert_eq!(
        encode_val(&Val::Ref(u64::MAX)),
        vec![4, 255, 255, 255, 255, 255, 255, 255, 255, 255, 1]
    );
    // Nesting: `MAX_TY_DEPTH` levels of list decode, one more does not.
    let mut deep = vec![5u8; MAX_TY_DEPTH];
    deep.push(0);
    assert!(decode_ty(&deep).is_some());
    let mut too_deep = vec![5u8; MAX_TY_DEPTH + 1];
    too_deep.push(0);
    assert_eq!(decode_ty(&too_deep), None);
    assert_eq!(MAX_TY_DEPTH, 64);
}

/// An expression is a tag and its parts, in prefix order: `not` (tag `6`) nested `MAX_EXPR_DEPTH`
/// levels, around the literal `null`, decodes; one level more does not (the codec's bound).
#[test]
fn codec_expression_nesting_is_bounded() {
    let mut deep = vec![6u8; MAX_EXPR_DEPTH];
    deep.extend([0, 0]);
    let decoded = decode_expr_bytes(&deep);
    assert!(decoded.is_some());
    let mut too_deep = vec![6u8; MAX_EXPR_DEPTH + 1];
    too_deep.extend([0, 0]);
    assert_eq!(decode_expr_bytes(&too_deep), None);
    assert_eq!(
        encode_field_def(&FieldDef {
            ty: Ty::Bool,
            required: false,
            unique: false,
            default: Expr::Principal,
        }),
        vec![2, 0, 0, 2]
    );
}

/// Decodes an expression by decoding a field definition whose default is it, after the type `bool`
/// (tag `2`) and two flags.
fn decode_expr_bytes(expr: &[u8]) -> Option<FieldDef> {
    let mut bytes = vec![2u8, 0, 0];
    bytes.extend_from_slice(expr);
    decode_field_def(&bytes)
}
