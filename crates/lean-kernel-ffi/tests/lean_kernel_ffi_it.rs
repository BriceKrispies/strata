//! Compiled Lean called from Rust: the bytes come back as the Lean function computes them, from
//! one thread and from many tokio worker threads at once.

use strata_lean_kernel_ffi::codec::{
    decode_ctx, decode_field_def, decode_intent, decode_record, decode_refusal, decode_step_result,
    decode_tenant, decode_ty, decode_val, encode_ctx, encode_field_def, encode_intent,
    encode_record, encode_refusal, encode_step_result, encode_tenant, encode_ty, encode_val, Ctx,
    Elem, FieldDef, Intent, Limits, OnDelete, Record, Refusal, StepResult, Tenant, Ty, Val,
    MAX_TY_DEPTH,
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

/// The fixture tenant of `lean/Strata/Codec/Fixtures.lean` (`fxTenant`).
fn fixture_tenant() -> Tenant {
    Tenant {
        schema: vec![(
            1,
            FieldDef {
                ty: Ty::Str,
                required: true,
                unique: false,
                default: Val::Null,
            },
        )],
        owner: 7,
        editors: vec![],
        writers: vec![],
        records: vec![],
        next_id: 1,
        limits: Limits::KERNEL,
        uniques: vec![],
    }
}

const FIXTURE_TENANT: &[u8] = &[
    1, 1, 1, 1, 0, 0, 7, 0, 0, 0, 1, 128, 128, 4, 232, 7, 200, 1, 128, 128, 16, 0,
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
        encode_ty(&Ty::List(Box::new(Ty::Ref(OnDelete::SetNull)))),
        vec![5, 4]
    );
    let field = FieldDef {
        ty: Ty::Int,
        required: false,
        unique: true,
        default: Val::Int(-2),
    };
    assert_eq!(encode_field_def(&field), vec![0, 0, 1, 1, 1, 1]);
    let record = Record {
        owner: 4,
        version: 1,
        fields: vec![(2, Val::Bool(true))],
    };
    assert_eq!(encode_record(&record), vec![4, 1, 1, 2, 3, 1]);
    assert_eq!(encode_tenant(&fixture_tenant()), FIXTURE_TENANT);
    assert_eq!(
        encode_ctx(&Ctx {
            tenant: 3,
            principal: 9
        }),
        vec![3, 9]
    );
    assert_eq!(
        encode_intent(&Intent::SetRequired {
            field: 2,
            strict: false
        }),
        vec![3, 2, 0]
    );
    let update = Intent::Update {
        id: 5,
        fields: vec![(1, Val::Str("x".into()))],
        expected: None,
    };
    assert_eq!(encode_intent(&update), vec![1, 5, 1, 1, 2, 1, 120, 0]);
    assert_eq!(encode_intent(&Intent::Create(vec![])), vec![0, 0]);
    assert_eq!(encode_refusal(&Refusal::LimitExceeded), vec![8]);
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
    assert_eq!(decode_field_def(&[0, 0, 1, 1, 1, 1]), Some(field));
    assert_eq!(decode_record(&[4, 1, 1, 2, 3, 1]), Some(record));
    assert_eq!(decode_tenant(FIXTURE_TENANT), Some(fixture_tenant()));
    assert_eq!(decode_intent(&[1, 5, 1, 1, 2, 1, 120, 0]), Some(update));
    assert_eq!(
        decode_ty(&[5, 4]),
        Some(Ty::List(Box::new(Ty::Ref(OnDelete::SetNull))))
    );
    assert_eq!(
        decode_ctx(&[3, 9]),
        Some(Ctx {
            tenant: 3,
            principal: 9
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
        3 => Ty::Ref(OnDelete::Restrict),
        _ => Ty::Ref(OnDelete::SetNull),
    }
}

fn gen_field_def(rng: &mut Rng) -> FieldDef {
    let ty = gen_ty(rng, 0);
    let required = rng.coin();
    let unique = rng.coin();
    let default = gen_val(rng);
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
    let owner = gen_nat(rng);
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

fn gen_tenant(rng: &mut Rng) -> Tenant {
    let mut schema = Vec::new();
    for _ in 0..rng.below(3) {
        let id = gen_nat(rng);
        let fd = gen_field_def(rng);
        schema.push((id, fd));
    }
    let owner = gen_nat(rng);
    let editors = gen_ids(rng, 3);
    let writers = gen_ids(rng, 3);
    let mut records = Vec::new();
    for _ in 0..rng.below(3) {
        let id = gen_nat(rng);
        let rec = gen_record(rng);
        records.push((id, rec));
    }
    let next_id = gen_nat(rng);
    let limits = Limits {
        max_string: gen_nat(rng),
        max_list: gen_nat(rng),
        max_unique: gen_nat(rng),
        max_record: gen_nat(rng),
    };
    let mut uniques = Vec::new();
    for _ in 0..rng.below(3) {
        uniques.push(gen_ids(rng, 4));
    }
    Tenant {
        schema,
        owner,
        editors,
        writers,
        records,
        next_id,
        limits,
        uniques,
    }
}

fn gen_opt(rng: &mut Rng) -> Option<u64> {
    if rng.coin() {
        Some(gen_nat(rng))
    } else {
        None
    }
}

fn gen_intent(rng: &mut Rng) -> Intent {
    match rng.below(6) {
        0 => Intent::Create(gen_fields(rng)),
        1 => {
            let id = gen_nat(rng);
            let fields = gen_fields(rng);
            let expected = gen_opt(rng);
            Intent::Update {
                id,
                fields,
                expected,
            }
        }
        2 => {
            let id = gen_nat(rng);
            let expected = gen_opt(rng);
            Intent::Delete { id, expected }
        }
        3 => {
            let field = gen_nat(rng);
            let strict = rng.coin();
            Intent::SetRequired { field, strict }
        }
        4 => {
            let field = gen_nat(rng);
            let strict = rng.coin();
            Intent::SetUnique { field, strict }
        }
        _ => Intent::DefineUnique(gen_ids(rng, 4)),
    }
}

fn gen_refusal(rng: &mut Rng) -> Refusal {
    match rng.below(11) {
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
        _ => Refusal::InvalidSchema,
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
            principal: gen_nat(&mut rng),
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
            id: 5,
            fields: vec![(1, Val::Str("x".into()))],
            expected: Some(300),
        }),
        decode_intent,
    );
    check_refuses_damage(FIXTURE_TENANT, decode_tenant);
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
