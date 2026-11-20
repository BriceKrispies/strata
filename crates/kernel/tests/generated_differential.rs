//! The hand-written kernel (`strata_kernel::transition`, `read`, `visible`, `needs`,
//! `read_needs`) against the functions specc generates from the specification
//! (`strata_model_core::functions::kernel_transition`, `kernel_read`, `visible`, `needs`,
//! `read_needs`). The backend runs the hand-written kernel; the generated functions are what
//! contracts CT-001 to CT-005 are derived from. Nothing else compares the two, so this test
//! runs both on the same inputs and fails on any difference in outcome.
//!
//! Inputs: the five starting worlds of `strata_kernel::testkit` (the corpus `properties.rs` and
//! the Lean fixtures start from), driven by random operation sequences drawn from the same
//! operation generator `properties.rs` uses (copied here, because a test binary cannot import
//! another), with a fixed seed (`TestRunner::deterministic`). After the operations, reads and
//! visibility checks run over every tenant, principal and record.
//!
//! The kernel crate is the one place that already depends on both sides (the hand-written code
//! and `strata-model-core`), so the test lives here and adds no dependency.
//!
//! The two sides are given the same types: the kernel re-exports the generated types. The
//! adapters are small:
//!   - the generated functions take `texts` (`IntentTexts`), the printed forms of expressions the
//!     function language cannot print (WG-4957); `texts_for` derives them from the snapshot's
//!     schema and the intent, with `Expr`'s `Display`, which is what the hand-written kernel
//!     prints;
//!   - the generated functions fail with `Failure<Rejection>`; a `Rejection::Rejected` payload is
//!     the hand-written kernel's `DomainError`, and a `Failure::Fault` (integer overflow,
//!     division by zero) fails the test, because the hand-written kernel has no such outcome;
//!   - the generated `needs` takes `&Option<TenantSchema>`, the hand-written one
//!     `Option<&TenantSchema>`.
//!
//! Differences known by design, each listed with its reason. A difference that matches none of
//! them fails the test.
//!   1. `EVALUATION_FAILED_DETAIL`: for a policy condition that fails to evaluate, the
//!      hand-written kernel writes the check detail `<condition>: <evaluation error>`; the
//!      generated `policy-check-text` writes `<condition>: evaluation failed`, because
//!      `policy-check` does not carry the error (a gap of WG-4938). Compared as: the generated
//!      detail minus its suffix is a prefix of the hand-written detail.
//!   2. Not exercised, so not compared: a policy condition, a query filter or a field read
//!      condition that follows a reference (`through`), and the `InsufficientState` the
//!      hand-written kernel answers when the snapshot lacks the record it would follow
//!      (`expr::missing`, `authorize_needing`, WG-5096). Two things are missing in the toolchain,
//!      neither in Exegra: specc compiles `eval` of three arguments only (the lookup argument of
//!      the fourth, which Exegra evaluates, has no generated counterpart: the generated
//!      `follow` of the bridge always fails), and the need a condition names is found by
//!      walking the expression tree, a structural recursion that specc refuses (Exegra checks
//!      and evaluates it). The generated `authorize`, filters and `redact` therefore fail such
//!      a condition. The corpus has no such condition, and `assert_no_input_of_a_known_difference`
//!      fails on a field read condition that follows a reference.
//!   3. Not exercised: a query order key over a list-valued attribute. The generated order
//!      does not order a list element by element, as `Value`'s derived order does.
//!      The corpus has no list-valued order key (it has a list-valued field, `tags`, which no
//!      query orders by).
//!   4. Not exercised: the intents of teams and invitations, and schema changes other than
//!      `DefineField`, `DefineRelationship`, `DefineConstraint`, `DefineQuery`,
//!      `DefineRolePermissions`, `RemoveField` and the definitions of triggers, automations, tenant
//!      operations and composite unique constraints (and their removals), which the generator
//!      below adds to the seven intents of `properties.rs`. `texts_for` reads defaults and
//!      constraints only from `DefineField` and `DefineConstraint`; the definitions print no
//!      expression. The functions of the other intents are mirrored all the same (the conferral
//!      of a role, the owner that remains, the reserved `team:` prefix, the rewrite of records
//!      by a schema change), each with examples in the specification.
//!   5. Not exercised: a list nested three deep in a stored value. The generated limit and
//!      size functions (`value-limit-problem`, `value-json-bytes`) look two levels into a list
//!      where `limits.rs` looks as deep as there are lists, because specc compiles no
//!      recursion; a field admits a list of scalars only, so such a value is a type mismatch
//!      whose only other effect is the order of two refusals on a record over the size limit.
//!      The corpus draws no nested list.
//!   6. Not exercised: a policy on every object whose condition reads a field is found by
//!      `policy-problems` as a condition that does not check against no fields, where the
//!      hand-written `mentions_field` walks the expression (a recursion specc refuses). The two
//!      differ for a condition that fails to check for another reason, or that reads a field in
//!      a branch that never runs. The corpus defines no policy on every object.
//!   7. Not exercised, and not mirrored: the bounds the kernel checks on a definition that the
//!      function language cannot state, because it compiles neither durations nor recursion:
//!      retry delays, attempt timeouts and waits of an automation step, and the two hundred
//!      nodes of an operation condition. The generator draws no definition that breaks one, nor
//!      a hundred and one triggers. Its automation acts with no run-as role, so the conferral of
//!      a run-as role is compared only through the specification's examples.
//!   8. Not mirrored: the problems of the aggregates a named query declares, of its declared
//!      fields and of a view section that names an aggregate (`strata_kernel::aggregates`,
//!      AGG-001 to AGG-023): the generated `schema-problems` states none of them, and the
//!      aggregate itself is not a read of the generated `kernel-read`. The corpus defines no
//!      query with an aggregate.
//!   9. Not exercised, and not mirrored until the generated functions are wired: layered settings
//!      and product capabilities (`settings.rs`, `capabilities.rs`). The generated `apply-change`,
//!      `intent-decision`, `needs` and `apply-delta` have no arm for the schema changes
//!      define-setting, remove-setting, define-capability and remove-capability, the intent
//!      write-setting and the delta operations on overrides, and the generated `schema-problems`
//!      has not the checks of setting definitions, of the grants of declared capabilities and of
//!      the references of a trigger to a setting. The corpus holds no setting, no capability and
//!      no override; `assert_no_input_of_a_known_difference` fails when a snapshot holds one.
//!  10. Not mirrored: what `strata_kernel::triggers` adds once the events of a decision are final
//!      (D-7200 to D-7204, WG-5097, WG-5099, WG-5079): the `StartAutomation` and
//!      `DeclineAutomation` effects of the triggers that match an event, the `transitions` a
//!      record-updated event names, the `defined-by` and `defined-at` the kernel writes on a
//!      trigger or an automation it defines, the suspension of an automation whose definer lost
//!      authority, the roles of an automation principal and its reserved `automation:` prefix.
//!      The generated `finish`, `update-record` and `apply-change` do not state them yet. Compared
//!      as: both results without those effects, transitions, definers and times
//!      (`without_kernel_only`). The suspension is guarded: the corpus's automation acts with no
//!      run-as role, so it is never suspended, and `assert_no_input_of_a_known_difference` fails
//!      on one that has a role. No principal of the corpus starts with `automation:`.
//!  11. Not exercised, and not mirrored: the automation principal (`strata_kernel::automation`,
//!      WFL-1527). The hand-written `roles_of` gives a principal `automation:<id>` exactly the
//!      run-as roles of its automation, and `refuse_reserved_principals` refuses the prefix for
//!      a named person and for a caller with a session; the generated `roles-of` and
//!      `reserved-principal` do not yet. Guarded: `assert_no_automation_principal` fails on a
//!      context whose principal has the prefix.
//!  12. Not exercised, and not mirrored: the invocation of a tenant operation (`InvokeOperation`,
//!      `crates/kernel/src/operations.rs`, WG-5100). The generated `intent-decision` refuses it
//!      with `operation:decided_by_the_kernel` and the generated `needs` plans nothing for it,
//!      because the function language does not state the decision yet. The operation generator
//!      draws no invocation, and `compare_transition` fails on one.
//!  13. Not mirrored: the tenant ledger (`strata_kernel::ledger`, WG-5083). The generated
//!      `intent-decision` refuses `PostLedger`, `apply-change` refuses the ledger schema changes,
//!      `needs` plans no ledger need, `apply-delta` ignores a ledger operation, `schema-problems`
//!      does not check ledger declarations, and the functions that rebuild a tenant schema or
//!      state do not carry its `ledger` member. The generator draws no ledger intent or change.
//!  14. Not exercised, and not mirrored: the checks of the declarations of an object type, of the
//!      templates, tokens and product records of the tenant schema that the hand-written `schema.problems` makes through
//!      `object_meta::problems`, `templates::problems` and `products::problems` (retention from a
//!      timestamp field, the states of a publication, the type of a language field, the one-line
//!      subject of a template, the expression of a token, the owner of an entity), and the
//!      removal of an object type with the deletion of its records. The generated
//!      `schema-problems` does not state them. The generator draws no such declaration, template,
//!      token or product record.
//!
//! Difference 7 is guarded: `assert_no_input_of_a_known_difference` runs on every snapshot the
//! test decides or reads, and fails when a snapshot has an automation step with a retry, a
//! timeout or a wait, or a field read condition that follows a reference (difference 2), so that
//! a corpus that grows into them is told to mirror the rule or to count the difference here. The
//! others are guarded by the operation generator drawing nothing that reaches them.
//!
//! Mirrored, so no longer differences: the size limits of `limits.rs` (the bytes of a record, the
//! length of a string or a list, the encoded size of a value of a unique field: `record-limit-check`,
//! `limit-step`; WG-5092), the field-level read conditions (`redact`, in `kernel-read` and
//! `named-read`; WG-5093), the deletion of a record that two fields of one record refer to (one
//! rewrite per record; WG-5094), the problems of a schema that `schema-problems` did not state:
//! counts and limits, field read conditions, the permissions of roles, teams and invitations
//! (WG-5095, WG-5077); and in an earlier pass: authorisation before the lookup of an object
//! type or a record (`object-def`, `stored`), the conferral of a role counting policies
//! (`may-confer`, `holds-policy-power`), a revocation asking for the authority to confer, the
//! owner that remains counting teams (`owner-lost`), the second authorisation of an update after
//! workflow assignments, the reserved `team:` prefix (`reserved-principal`), the records a
//! schema change rewrites (version one higher, an event each), the limit of 200 changes in one
//! schema change, the permissions of a role granting before any policy (`permission:role`), the
//! plan of a schema change (`ref-needs`) and a query filter that cannot be evaluated on a record
//! leaving the record out.

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model_core::functions::{self as spec, Failure};
use strata_model_core::types::{IntentTexts, Rejection};

/// How many random operation sequences run from each starting world, and how long each is.
const SEQUENCES: usize = 24;
const OPS_PER_SEQUENCE: usize = 20;

/// The reason of difference 1 above.
const EVALUATION_FAILED_DETAIL: &str = ": evaluation failed";

// ---------------------------------------------------------------------------------------
// The operation generator (a copy of `properties.rs`).
// ---------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Op {
    tenant: usize,
    actor_tenant: usize,
    /// 0..=3 the members of the tenant, 4 a stranger, 5 an anonymous visitor, 6 the system.
    actor: usize,
    kind: OpKind,
}

#[derive(Clone, Debug)]
enum OpKind {
    CreateTenant {
        name: String,
        owner: usize,
    },
    Create {
        title: String,
        slug: u8,
        rank: Option<i64>,
        author: Option<u64>,
        /// A second reference field to a person (once `SchemaOp::SecondReference` defines it).
        editor: Option<u64>,
        /// A value in `secret`, a field with a read condition (once `SchemaOp::DefineSecret`
        /// defines it).
        secret: bool,
    },
    /// A note, created or updated, with a value at or over one of the size limits.
    Big {
        id: Option<u64>,
        shape: BigShape,
    },
    Person {
        name: String,
    },
    Update {
        id: u64,
        title: Option<String>,
        status: Option<&'static str>,
        rank: Option<i64>,
    },
    Delete {
        object: &'static str,
        id: u64,
    },
    ChangeSchema {
        change: SchemaOp,
        grandfather: bool,
    },
    Assign {
        who: usize,
        role: usize,
    },
    Revoke {
        who: usize,
        role: usize,
    },
}

/// What a `Big` operation writes, at or just over a limit of `limits.rs`.
#[derive(Clone, Copy, Debug)]
enum BigShape {
    /// A title of 65537 bytes: one over the longest string.
    LongTitle,
    /// A title of 32769 characters of two bytes: 65538 bytes, which only a count of bytes finds.
    WideTitle,
    /// A title of 65536 bytes: the longest string.
    LimitTitle,
    /// A slug of 191 characters: 201 bytes encoded, one over a unique value.
    LongSlug,
    /// A slug of 190 characters: 200 bytes encoded, the longest unique value.
    LimitSlug,
    /// Five strings of 65536 bytes (the last three in fields `SchemaOp::DefineExtra` defines):
    /// no string is too long and the record is over 262144 bytes.
    FatRecord,
    /// A list of 1001 items in `tags` (once `SchemaOp::DefineTags` defines it).
    LongList,
    /// A list of 1000 items.
    LimitList,
}

#[derive(Clone, Copy, Debug)]
enum SchemaOp {
    RequireRank,
    TightenTitle(i64),
    Nonsense,
    DefineAutomation,
    BrokenAutomation,
    RemoveAutomation,
    DefineTrigger,
    RemoveTrigger,
    DefineOperation,
    RemoveOperation,
    DefineUnique,
    RemoveUnique,
    /// A field with a read condition: only an editor reads `secret`.
    DefineSecret,
    /// A second reference field to person, so a note can refer to one person twice.
    SecondReference,
    /// A list field, the only one of the corpus.
    DefineTags,
    /// One of three more string fields, so that a record can be over its size limit.
    DefineExtra(u8),
    /// A field whose read condition is an integer, not a truth value.
    BadReadInt,
    /// A field whose read condition names a field the type does not have.
    BadReadUnknown,
    /// A query whose limit is 0 or over 200.
    BadQueryLimit(u32),
    /// Permissions with a blank capability, an unknown object type and no action.
    BadPermissions,
    /// A hundred more fields, which is over the most an object type may have.
    ManyFields,
}

const MEMBERS: [&str; 5] = ["owner", "editor", "writer1", "writer2", "stranger"];
const ROLES: [&str; 5] = ["owner", "editor", "writer", "ghost", "public"];

fn op_strategy() -> impl Strategy<Value = Op> {
    let status = prop_oneof![Just("draft"), Just("published"), Just("archived")];
    let schema_op = prop_oneof![
        3 => Just(SchemaOp::RequireRank),
        3 => (3i64..30).prop_map(SchemaOp::TightenTitle),
        1 => Just(SchemaOp::Nonsense),
        2 => Just(SchemaOp::DefineAutomation),
        1 => Just(SchemaOp::BrokenAutomation),
        1 => Just(SchemaOp::RemoveAutomation),
        2 => Just(SchemaOp::DefineTrigger),
        1 => Just(SchemaOp::RemoveTrigger),
        2 => Just(SchemaOp::DefineOperation),
        1 => Just(SchemaOp::RemoveOperation),
        2 => Just(SchemaOp::DefineUnique),
        1 => Just(SchemaOp::RemoveUnique),
        2 => Just(SchemaOp::DefineSecret),
        2 => Just(SchemaOp::SecondReference),
        1 => Just(SchemaOp::DefineTags),
        1 => (1u8..4).prop_map(SchemaOp::DefineExtra),
        1 => Just(SchemaOp::BadReadInt),
        1 => Just(SchemaOp::BadReadUnknown),
        1 => prop_oneof![Just(0u32), Just(201u32)].prop_map(SchemaOp::BadQueryLimit),
        1 => Just(SchemaOp::BadPermissions),
        1 => Just(SchemaOp::ManyFields),
    ];
    let big = prop_oneof![
        Just(BigShape::LongTitle),
        Just(BigShape::WideTitle),
        Just(BigShape::LimitTitle),
        Just(BigShape::LongSlug),
        Just(BigShape::LimitSlug),
        Just(BigShape::FatRecord),
        Just(BigShape::LongList),
        Just(BigShape::LimitList),
    ];
    let kind = prop_oneof![
        1 => ("[a-z ]{0,4}", 0usize..2).prop_map(|(name, owner)| OpKind::CreateTenant { name, owner }),
        4 => ("[a-zA-Z ]{0,30}", 0u8..6, proptest::option::of(0i64..5), proptest::option::of(1u64..6), proptest::option::of(1u64..6), any::<bool>())
            .prop_map(|(title, slug, rank, author, editor, secret)| OpKind::Create { title, slug, rank, author, editor, secret }),
        2 => (proptest::option::of(1u64..8), big).prop_map(|(id, shape)| OpKind::Big { id, shape }),
        1 => "[a-z]{1,6}".prop_map(|name| OpKind::Person { name }),
        4 => (1u64..8, proptest::option::of("[a-z]{0,30}"), proptest::option::of(status), proptest::option::of(0i64..5))
            .prop_map(|(id, title, status, rank)| OpKind::Update { id, title, status, rank }),
        2 => (prop_oneof![Just("note"), Just("person")], 1u64..8).prop_map(|(object, id)| OpKind::Delete { object, id }),
        2 => (schema_op, any::<bool>()).prop_map(|(change, grandfather)| OpKind::ChangeSchema { change, grandfather }),
        2 => (0usize..5, 0usize..5).prop_map(|(who, role)| OpKind::Assign { who, role }),
        2 => (0usize..5, 0usize..5).prop_map(|(who, role)| OpKind::Revoke { who, role }),
    ];
    (0usize..3, 0usize..3, 0usize..7, kind, 0u8..10).prop_map(
        |(tenant, other, actor, kind, cross)| Op {
            tenant,
            actor_tenant: if cross == 0 { other } else { tenant },
            actor,
            kind,
        },
    )
}

fn intent_of(op: &Op) -> Intent {
    match &op.kind {
        OpKind::CreateTenant { name, owner } => Intent::CreateTenant {
            name: name.clone(),
            owner: principal_id(&format!("{}:{}", WORLD_TENANTS[op.tenant], MEMBERS[*owner])),
        },
        OpKind::Create {
            title,
            slug,
            rank,
            author,
            editor,
            secret,
        } => {
            let mut fields = BTreeMap::from([
                (field("title"), text(title)),
                (field("slug"), text(&format!("s{slug}"))),
            ]);
            if let Some(editor) = editor {
                fields.insert(field("editor"), Value::Ref(RecordId(*editor)));
            }
            if *secret {
                fields.insert(field("secret"), text("for editors"));
            }
            if let Some(rank) = rank {
                fields.insert(field("rank"), Value::Int(*rank));
            }
            if let Some(author) = author {
                fields.insert(field("author"), Value::Ref(RecordId(*author)));
            }
            Intent::CreateRecord {
                object: object("note"),
                fields,
            }
        }
        OpKind::Big { id, shape } => {
            let long = |unit: &str, count: usize| text(&unit.repeat(count));
            let mut fields = BTreeMap::new();
            match shape {
                BigShape::LongTitle => {
                    fields.insert(field("title"), long("a", 65_537));
                }
                BigShape::WideTitle => {
                    fields.insert(field("title"), long("\u{e9}", 32_769));
                }
                BigShape::LimitTitle => {
                    fields.insert(field("title"), long("a", 65_536));
                }
                BigShape::LongSlug => {
                    fields.insert(field("slug"), long("a", 191));
                }
                BigShape::LimitSlug => {
                    fields.insert(field("slug"), long("a", 190));
                }
                BigShape::FatRecord => {
                    for name in ["title", "body", "e1", "e2", "e3"] {
                        fields.insert(field(name), long("a", 65_536));
                    }
                }
                BigShape::LongList => {
                    fields.insert(field("tags"), Value::List(vec![text("x"); 1001]));
                }
                BigShape::LimitList => {
                    fields.insert(field("tags"), Value::List(vec![text("x"); 1000]));
                }
            }
            match id {
                Some(id) => Intent::UpdateRecord {
                    object: object("note"),
                    id: RecordId(*id),
                    fields,
                    expected_version: None,
                },
                None => {
                    fields.entry(field("title")).or_insert_with(|| text("Big"));
                    fields.entry(field("slug")).or_insert_with(|| text("big"));
                    Intent::CreateRecord {
                        object: object("note"),
                        fields,
                    }
                }
            }
        }
        OpKind::Person { name } => Intent::CreateRecord {
            object: object("person"),
            fields: BTreeMap::from([(field("name"), text(name))]),
        },
        OpKind::Update {
            id,
            title,
            status,
            rank,
        } => {
            let mut fields = BTreeMap::new();
            if let Some(title) = title {
                fields.insert(field("title"), text(title));
            }
            if let Some(status) = status {
                fields.insert(field("status"), text(status));
            }
            if let Some(rank) = rank {
                fields.insert(field("rank"), Value::Int(*rank));
            }
            Intent::UpdateRecord {
                object: object("note"),
                id: RecordId(*id),
                fields,
                expected_version: None,
            }
        }
        OpKind::Delete { object: name, id } => Intent::DeleteRecord {
            object: object(name),
            id: RecordId(*id),
            expected_version: None,
        },
        OpKind::ChangeSchema {
            change,
            grandfather,
        } => {
            let single = match change {
                SchemaOp::RequireRank => SchemaChange::DefineField {
                    object: object("note"),
                    field: field_def("rank", FieldType::Int, true),
                },
                SchemaOp::TightenTitle(limit) => SchemaChange::DefineConstraint {
                    object: object("note"),
                    constraint: ConstraintDef {
                        id: ConstraintId::new("title_length").unwrap(),
                        expr: Expr::Cmp(
                            CmpOp::Le,
                            Box::new(Expr::Len(Box::new(var(Scope::New, "title")))),
                            Box::new(Expr::Lit(Value::Int(*limit))),
                        ),
                        message: "shorter".into(),
                    },
                },
                SchemaOp::Nonsense => SchemaChange::RemoveField {
                    object: object("nothing"),
                    field: field("nothing"),
                },
                SchemaOp::DefineAutomation => SchemaChange::DefineAutomation(award(false)),
                SchemaOp::BrokenAutomation => SchemaChange::DefineAutomation(award(true)),
                SchemaOp::RemoveAutomation => {
                    SchemaChange::RemoveAutomation(WorkflowId::new("award").unwrap())
                }
                SchemaOp::DefineTrigger => SchemaChange::DefineTrigger(on_publish()),
                SchemaOp::RemoveTrigger => {
                    SchemaChange::RemoveTrigger(TriggerId::new("on_publish").unwrap())
                }
                SchemaOp::DefineOperation => SchemaChange::DefineOperation(redeem()),
                SchemaOp::RemoveOperation => {
                    SchemaChange::RemoveOperation(TenantOperationId::new("redeem").unwrap())
                }
                SchemaOp::DefineUnique => SchemaChange::DefineUnique {
                    object: object("note"),
                    unique: UniqueDef {
                        id: ConstraintId::new("title_rank").unwrap(),
                        fields: vec![field("title"), field("rank")],
                        message: "One note per title and rank".into(),
                    },
                },
                SchemaOp::RemoveUnique => SchemaChange::RemoveUnique {
                    object: object("note"),
                    unique: ConstraintId::new("title_rank").unwrap(),
                },
                SchemaOp::DefineSecret => SchemaChange::DefineField {
                    object: object("note"),
                    field: FieldDef {
                        read: Some(Expr::HasRole(role("editor"))),
                        ..field_def("secret", FieldType::Str, false)
                    },
                },
                SchemaOp::SecondReference => SchemaChange::DefineRelationship {
                    object: object("note"),
                    field: field("editor"),
                    label: "Editor".into(),
                    target: object("person"),
                    on_delete: OnDelete::SetNull,
                    required: false,
                },
                SchemaOp::DefineTags => SchemaChange::DefineField {
                    object: object("note"),
                    field: field_def("tags", FieldType::List(Box::new(FieldType::Str)), false),
                },
                SchemaOp::DefineExtra(n) => SchemaChange::DefineField {
                    object: object("note"),
                    field: field_def(&format!("e{n}"), FieldType::Str, false),
                },
                SchemaOp::BadReadInt => SchemaChange::DefineField {
                    object: object("note"),
                    field: FieldDef {
                        read: Some(Expr::Len(Box::new(var(Scope::Record, "title")))),
                        ..field_def("peek", FieldType::Str, false)
                    },
                },
                SchemaOp::BadReadUnknown => SchemaChange::DefineField {
                    object: object("note"),
                    field: FieldDef {
                        read: Some(var(Scope::Record, "nope")),
                        ..field_def("peek", FieldType::Str, false)
                    },
                },
                SchemaOp::BadQueryLimit(limit) => SchemaChange::DefineQuery(QueryDef {
                    id: QueryId::new("bad_limit").unwrap(),
                    object: object("note"),
                    filter: None,
                    params: BTreeSet::new(),
                    order: vec![],
                    limit: Some(*limit),
                }),
                SchemaOp::BadPermissions => SchemaChange::DefineRolePermissions {
                    role: role("editor"),
                    permissions: vec![
                        Permission::Capability("  ".into()),
                        Permission::Object {
                            object: object("nowhere"),
                            actions: BTreeSet::from([ObjectAction::Read]),
                        },
                        Permission::Object {
                            object: object("note"),
                            actions: BTreeSet::new(),
                        },
                    ],
                },
                // Placeholder: `ManyFields` is a hundred changes, built below.
                SchemaOp::ManyFields => SchemaChange::RemoveField {
                    object: object("nothing"),
                    field: field("nothing"),
                },
            };
            let changes = if matches!(change, SchemaOp::ManyFields) {
                (0..100)
                    .map(|i| SchemaChange::DefineField {
                        object: object("note"),
                        field: field_def(&format!("x{i:03}"), FieldType::Str, false),
                    })
                    .collect()
            } else {
                vec![single]
            };
            Intent::ChangeSchema {
                changes,
                expected_version: None,
                existing: if *grandfather {
                    ExistingData::Grandfather
                } else {
                    ExistingData::Reject
                },
            }
        }
        OpKind::Assign { who, role: r } => Intent::AssignRole {
            principal: principal_id(&format!("{}:{}", WORLD_TENANTS[op.tenant], MEMBERS[*who])),
            role: role(ROLES[*r]),
        },
        OpKind::Revoke { who, role: r } => Intent::RevokeRole {
            principal: principal_id(&format!("{}:{}", WORLD_TENANTS[op.tenant], MEMBERS[*who])),
            role: role(ROLES[*r]),
        },
    }
}

/// An automation that creates a note, looks at it and ends; `broken`, it names a step it does
/// not declare and leaves one unreachable. It acts with no role, so the comparison is of the
/// definitions and not of the conferral of a run-as role (difference 7).
fn award(broken: bool) -> AutomationDef {
    let name = |n: &str| StepName::new(n).unwrap();
    let step = |action: AutomationStep, next: Option<&str>| AutomationStepDef {
        action,
        next: next.map(|n| StepName::new(n).unwrap()),
        retry: None,
        timeout: None,
        compensate: None,
    };
    let mut steps = BTreeMap::from([
        (
            name("make"),
            step(
                AutomationStep::CreateRecord {
                    object: object("note"),
                    fields: BTreeMap::from([
                        (field("title"), Expr::Lit(text("Silver"))),
                        (field("slug"), Expr::Param("owner".into())),
                    ]),
                },
                Some("check"),
            ),
        ),
        (
            name("check"),
            step(
                AutomationStep::Branch {
                    condition: Expr::Cmp(
                        CmpOp::Eq,
                        Box::new(var(Scope::Record, "status")),
                        Box::new(Expr::Lit(text("draft"))),
                    ),
                    object: Some(object("note")),
                    record: Some(Expr::Param("make_record".into())),
                    then: name("done"),
                    r#else: name("done"),
                },
                None,
            ),
        ),
        (name("done"), step(AutomationStep::End, None)),
    ]);
    if broken {
        steps.insert(name("lost"), step(AutomationStep::End, Some("gone")));
    }
    AutomationDef {
        id: WorkflowId::new("award").unwrap(),
        label: "Award".into(),
        run_as: RunAs {
            roles: BTreeSet::new(),
        },
        run_key: Some(Expr::Param("owner".into())),
        start: name("make"),
        steps,
        defined_by: None,
        defined_at: None,
        suspended_for: None,
    }
}

/// A trigger on the publish transition of a note that starts `award`.
fn on_publish() -> TriggerDef {
    TriggerDef {
        id: TriggerId::new("on_publish").unwrap(),
        on: TriggerEvent::RecordUpdated {
            object: object("note"),
            transition: Some("publish".into()),
        },
        condition: Some(Expr::Cmp(
            CmpOp::Eq,
            Box::new(var(Scope::New, "status")),
            Box::new(Expr::Lit(text("published"))),
        )),
        automation: WorkflowId::new("award").unwrap(),
        paused: None,
        defined_by: None,
        defined_at: None,
    }
}

/// A tenant operation that copies a draft note and ranks the copy.
fn redeem() -> TenantOperationDef {
    TenantOperationDef {
        id: TenantOperationId::new("redeem").unwrap(),
        label: "Redeem".into(),
        inputs: vec![TenantOperationInput {
            name: "note".into(),
            ty: FieldType::Ref {
                target: object("note"),
                on_delete: OnDelete::Restrict,
            },
            required: true,
            label: None,
        }],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: Some(TenantOperationSubject {
            object: object("note"),
            input: "note".into(),
        }),
        conditions: vec![TenantOperationCondition {
            id: "drafted".into(),
            expr: Expr::Cmp(
                CmpOp::Eq,
                Box::new(var(Scope::Record, "status")),
                Box::new(Expr::Lit(text("draft"))),
            ),
            error: "not-draft".into(),
            message: "Only a draft is redeemed.".into(),
        }],
        steps: vec![
            TenantOperationStep::CreateRecord {
                id: "copy".into(),
                object: object("note"),
                fields: BTreeMap::from([
                    (field("title"), var(Scope::Record, "title")),
                    (field("slug"), Expr::Lit(text("copy"))),
                ]),
            },
            TenantOperationStep::UpdateRecord {
                id: "touch".into(),
                object: object("note"),
                record: Expr::Param("copy".into()),
                fields: BTreeMap::from([(field("rank"), Expr::Lit(Value::Int(1)))]),
            },
        ],
    }
}

fn principal_of(tenant: usize, actor: usize) -> Principal {
    if actor == 6 {
        Principal::System
    } else {
        principals(WORLD_TENANTS[tenant])[actor].clone()
    }
}

fn context_of(op: &Op, now: i64) -> Context {
    ctx(
        WORLD_TENANTS[op.tenant],
        principal_of(op.actor_tenant, op.actor),
        now,
    )
}

// ---------------------------------------------------------------------------------------
// The adapters.
// ---------------------------------------------------------------------------------------

/// The `texts` the generated functions take, derived as the caller (the shell) would: the
/// printed defaults, constraints, policy conditions and query filters of the snapshot's schema,
/// those a `ChangeSchema` intent defines, and whether a `CreateTenant` name is blank.
fn texts_for(state: &State, intent: &Intent) -> IntentTexts {
    let mut texts = IntentTexts {
        defaults: BTreeMap::new(),
        constraints: BTreeMap::new(),
        conditions: BTreeMap::new(),
        blank_name: false,
    };
    if let Some(tenant) = &state.tenant {
        for (object_id, def) in &tenant.schema.objects {
            for (field_id, field) in &def.fields {
                if let Some(default) = &field.default {
                    texts
                        .defaults
                        .entry(object_id.clone())
                        .or_default()
                        .insert(field_id.clone(), default.to_string());
                }
            }
            for (constraint_id, constraint) in &def.constraints {
                texts
                    .constraints
                    .entry(object_id.clone())
                    .or_default()
                    .insert(constraint_id.clone(), constraint.expr.to_string());
            }
        }
        for (policy_id, policy) in &tenant.schema.policies {
            if let Some(condition) = &policy.condition {
                texts
                    .conditions
                    .insert(format!("policy:{}", policy_id.0), condition.to_string());
            }
        }
        for (query_id, query) in &tenant.schema.queries {
            if let Some(filter) = &query.filter {
                texts
                    .conditions
                    .insert(format!("query:{}", query_id.0), filter.to_string());
            }
        }
    }
    match intent {
        Intent::ChangeSchema { changes, .. } => {
            for change in changes {
                match change {
                    SchemaChange::DefineField { object, field } => {
                        if let Some(default) = &field.default {
                            texts
                                .defaults
                                .entry(object.clone())
                                .or_default()
                                .insert(field.id.clone(), default.to_string());
                        }
                    }
                    SchemaChange::DefineConstraint { object, constraint } => {
                        texts
                            .constraints
                            .entry(object.clone())
                            .or_default()
                            .insert(constraint.id.clone(), constraint.expr.to_string());
                    }
                    _ => {}
                }
            }
        }
        Intent::CreateTenant { name, .. } => texts.blank_name = name.trim().is_empty(),
        _ => {}
    }
    texts
}

/// A generated function's failure as the hand-written kernel's error type. A fault has no
/// counterpart there, so it is reported as a difference by the caller.
fn domain_error(failure: Failure<Rejection>) -> Result<DomainError, String> {
    match failure {
        Failure::Error(Rejection::Rejected(error)) => Ok(error),
        Failure::Fault(fault) => Err(format!("fault: {fault}")),
    }
}

/// Difference 1: make the generated detail of a failed policy condition comparable with the
/// hand-written one when, and only when, the only difference is the evaluation error text.
/// Returns how many details it matched.
fn reconcile_evaluation_failed(hand: &[Check], generated: &mut [Check]) -> usize {
    let mut matched = 0;
    for (h, g) in hand.iter().zip(generated.iter_mut()) {
        if h.rule == g.rule
            && h.rule.starts_with("policy:")
            && !h.passed
            && !g.passed
            && g.detail.ends_with(EVALUATION_FAILED_DETAIL)
        {
            let condition = &g.detail[..g.detail.len() - EVALUATION_FAILED_DETAIL.len()];
            if h.detail.starts_with(&format!("{condition}: ")) {
                g.detail = h.detail.clone();
                matched += 1;
            }
        }
    }
    matched
}

/// Whether an expression follows a reference (`through`): what the generated functions cannot
/// (difference 2).
fn mentions_through(expr: &Expr) -> bool {
    match expr {
        Expr::Through(..) => true,
        Expr::Var(..)
        | Expr::Lit(_)
        | Expr::Param(_)
        | Expr::Now
        | Expr::Principal
        | Expr::HasRole(_) => false,
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => mentions_through(a),
        Expr::And(items) | Expr::Or(items) => items.iter().any(mentions_through),
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => mentions_through(a) || mentions_through(b),
        Expr::If(a, b, c) => mentions_through(a) || mentions_through(b) || mentions_through(c),
    }
}

/// Differences 2 and 7 of the header: fail when a snapshot holds an input of a rule the generated
/// functions do not mirror, so the corpus cannot start to depend on one silently.
fn assert_no_input_of_a_known_difference(state: &State) {
    let Some(tenant) = &state.tenant else {
        return;
    };
    for (object_id, def) in &tenant.schema.objects {
        for (field_id, field) in &def.fields {
            if let Some(read) = &field.read {
                assert!(
                    !mentions_through(read),
                    "{object_id}.{field_id} has a read condition that follows a reference; the generated redact has no lookup (difference 2)"
                );
            }
        }
    }
    for query in tenant.schema.queries.values() {
        assert!(
            query.aggregates.is_none() && query.declared_fields.is_none(),
            "{} declares aggregates or fields; the generated schema-problems does not check them (difference 8)",
            query.id
        );
    }
    for view in tenant.schema.views.values() {
        assert!(
            view.sections.values().all(|section| section.aggregate.is_none()),
            "{} has a section that names an aggregate; the generated schema-problems does not check it (difference 8)",
            view.id
        );
    }
    assert!(
        tenant.schema.settings.is_none()
            && tenant.schema.capabilities.is_none()
            && tenant.setting_values.is_none(),
        "the snapshot holds a setting, a capability or an override; the generated functions do not mirror them (difference 9)"
    );
    assert!(
        tenant.ledger.is_none() && tenant.schema.ledger.is_none(),
        "{} holds a ledger; the generated functions do not mirror it (difference 13)",
        tenant.id
    );
    assert!(
        tenant.schema.mail_templates.is_none()
            && tenant.schema.notice_templates.is_none()
            && tenant.schema.tokens.is_none()
            && tenant.schema.products.is_none(),
        "the schema holds a template, a token or a product record; the generated functions do not state their checks (difference 14)"
    );
    for (object_id, def) in &tenant.schema.objects {
        assert!(
            def.retention.is_none()
                && def.publication.is_none()
                && def.language_field.is_none()
                && def.translation_group.is_none(),
            "{object_id} declares retention, publication, a language field or a translation group; the generated functions do not state their checks (difference 14)"
        );
    }
    for automation in tenant.schema.automations.iter().flat_map(BTreeMap::values) {
        assert!(
            automation.run_as.roles.is_empty(),
            "{} has a run-as role; the generated functions do not suspend an automation whose definer lost authority (difference 10)",
            automation.id
        );
        for (name, step) in &automation.steps {
            assert!(
                step.retry.is_none()
                    && step.timeout.is_none()
                    && !matches!(
                        step.action,
                        AutomationStep::WaitFor { .. } | AutomationStep::RequestApproval { .. }
                    ),
                "{}.{name} has a retry, a timeout or a wait; the generated functions do not bound durations (difference 7)",
                automation.id
            );
        }
    }
}

/// Difference 10: a result without what `strata_kernel::triggers` adds and the generated
/// functions do not state: the effects of triggers, the transitions a record-updated event names,
/// and the definer and time written on a trigger or an automation.
fn without_kernel_only(result: &mut Result<Transition, DomainError>) {
    let Ok(outcome) = result else {
        return;
    };
    outcome.effects.retain(|effect| {
        !matches!(
            effect,
            Effect::StartAutomation(_) | Effect::DeclineAutomation(_)
        )
    });
    for event in &mut outcome.events {
        if let EventBody::RecordUpdated { transitions, .. } = &mut event.body {
            *transitions = None;
        }
    }
    for op in &mut outcome.delta.ops {
        if let DeltaOp::PutSchema(schema) = op {
            let mut plain = TenantSchema::clone(schema);
            for def in plain
                .automations
                .iter_mut()
                .flat_map(|all| all.values_mut())
            {
                def.defined_by = None;
                def.defined_at = None;
            }
            for def in plain.triggers.iter_mut().flat_map(|all| all.values_mut()) {
                def.defined_by = None;
                def.defined_at = None;
            }
            *schema = std::sync::Arc::new(plain);
        }
    }
}

/// Difference 11 of the header: fail when a decision or read is made as an automation principal,
/// whose roles the generated functions do not mirror.
fn assert_no_automation_principal(context: &Context) {
    if let Principal::User(id) = &context.principal {
        assert!(
            strata_kernel::automation::automation_named(id).is_none(),
            "{id} is an automation principal; the generated roles-of and reserved-principal do not mirror it (difference 11)"
        );
    }
}

#[derive(Default)]
struct Tally {
    compared: usize,
    accepted: usize,
    rejected: usize,
    reads: usize,
    visibility: usize,
    needs: usize,
    evaluation_failed_details: usize,
}

fn compare_transition(state: &State, intent: &Intent, context: &Context, tally: &mut Tally) {
    let label = format!(
        "{} as {:?} in {} (snapshot of {})",
        intent.name(),
        context.principal,
        context.tenant,
        state.tenant_id
    );
    assert_no_input_of_a_known_difference(state);
    assert!(
        !matches!(intent, Intent::InvokeOperation { .. }),
        "{label}: the generated functions do not decide a tenant operation (difference 12)"
    );
    assert_no_automation_principal(context);
    let mut hand = transition(state, intent, context);
    without_kernel_only(&mut hand);
    let texts = texts_for(state, intent);
    let mut generated = match spec::kernel_transition(state, intent, context, &texts) {
        Ok(result) => Ok(result),
        Err(failure) => Err(domain_error(failure).unwrap_or_else(|fault| {
            panic!(
                "{label}: the generated kernel failed with {fault}; the hand kernel gave {hand:?}"
            )
        })),
    };
    without_kernel_only(&mut generated);
    match (&hand, &mut generated) {
        (Ok(h), Ok(g)) => {
            tally.evaluation_failed_details +=
                reconcile_evaluation_failed(&h.evidence, &mut g.evidence);
            tally.accepted += 1;
        }
        (Err(h), Err(g)) => {
            tally.evaluation_failed_details +=
                reconcile_evaluation_failed(&h.evidence, &mut g.evidence);
            tally.rejected += 1;
        }
        _ => {}
    }
    assert_eq!(
        hand, generated,
        "kernel and generated kernel-transition differ on {label}"
    );
    tally.compared += 1;

    // The state each side plans to load before deciding.
    let schema = state.tenant.as_ref().map(|t| (*t.schema).clone());
    assert_eq!(
        needs(schema.as_ref(), intent),
        spec::needs(&schema, intent),
        "needs differ on {label}"
    );
    tally.needs += 1;
}

fn compare_read(state: &State, request: &ReadRequest, context: &Context, tally: &mut Tally) {
    let label = format!(
        "{request:?} as {:?} in {} (snapshot of {})",
        context.principal, context.tenant, state.tenant_id
    );
    assert_no_input_of_a_known_difference(state);
    assert_no_automation_principal(context);
    let hand = read(state, request, context);
    // Only the query filters are printed by a read; the intent is none.
    let texts = texts_for(
        state,
        &Intent::AssignRole {
            principal: principal_id("x"),
            role: role("public"),
        },
    );
    let mut generated = match spec::kernel_read(state, request, context, &texts) {
        Ok(result) => Ok(result),
        Err(failure) => Err(domain_error(failure).unwrap_or_else(|fault| {
            panic!("{label}: the generated read failed with {fault}; the hand kernel gave {hand:?}")
        })),
    };
    if let (Err(h), Err(g)) = (&hand, &mut generated) {
        tally.evaluation_failed_details +=
            reconcile_evaluation_failed(&h.evidence, &mut g.evidence);
    }
    assert_eq!(
        hand, generated,
        "kernel and generated kernel-read differ on {label}"
    );
    tally.reads += 1;

    if let Some(tenant) = &state.tenant {
        assert_eq!(
            read_needs(&tenant.schema, request),
            spec::read_needs(&tenant.schema, request),
            "read-needs differ on {label}"
        );
        tally.needs += 1;
    }
}

/// Every read the corpus asks of a snapshot, as every kind of principal of every tenant.
fn compare_reads_of(world: &World, now: i64, tally: &mut Tally) {
    let ids: Vec<RecordId> = (1..9).map(RecordId).collect();
    let requests = [
        ReadRequest::Named {
            query: QueryId::new("notes_by_rank").unwrap(),
            params: BTreeMap::new(),
            page_size: None,
            cursor: None,
        },
        ReadRequest::Named {
            query: QueryId::new("no_such_query").unwrap(),
            params: BTreeMap::new(),
            page_size: None,
            cursor: None,
        },
        ReadRequest::Records {
            object: object("note"),
            ids: ids.clone(),
        },
        ReadRequest::Records {
            object: object("person"),
            ids,
        },
        ReadRequest::Records {
            object: object("nothing"),
            ids: vec![RecordId(1)],
        },
    ];
    for tenant in WORLD_TENANTS {
        for actor_tenant in 0..WORLD_TENANTS.len() {
            for actor in 0..7 {
                let context = ctx(tenant, principal_of(actor_tenant, actor), now);
                let state = world.state(&tenant_id(tenant));
                for request in &requests {
                    compare_read(&state, request, &context, tally);
                }
                // A snapshot of another tenant than the context names.
                let elsewhere = world.state(&tenant_id(WORLD_TENANTS[(actor_tenant + 1) % 3]));
                compare_read(&elsewhere, &requests[0], &context, tally);
            }
        }
    }
}

/// `visible` on every stored record, for every principal.
fn compare_visibility(world: &World, now: i64, tally: &mut Tally) {
    for tenant in world.tenants.values() {
        for actor_tenant in 0..WORLD_TENANTS.len() {
            for actor in 0..7 {
                let principal = principal_of(actor_tenant, actor);
                for record in tenant.records.values().flat_map(|by_id| by_id.values()) {
                    let hand = visible(tenant, &principal, record, Timestamp(now));
                    let generated = spec::visible(tenant, &principal, record, Timestamp(now))
                        .unwrap_or_else(|fault| {
                            panic!(
                                "visible of {} {} for {principal:?} faulted: {fault}",
                                record.object, record.id.0
                            )
                        });
                    assert_eq!(
                        hand, generated,
                        "visible differs for {principal:?} on {} {} of {}",
                        record.object, record.id.0, tenant.id
                    );
                    tally.visibility += 1;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// The tests.
// ---------------------------------------------------------------------------------------

/// The hand-written kernel and the generated one decide, read and plan alike over the corpus.
#[test]
fn the_hand_written_kernel_agrees_with_the_generated_functions() {
    let mut runner = TestRunner::deterministic();
    let mut tally = Tally::default();
    let strategy = op_strategy();
    for (index, (_, start)) in starting_worlds().into_iter().enumerate() {
        for sequence in 0..SEQUENCES {
            let mut world = start.clone();
            for step in 0..OPS_PER_SEQUENCE {
                let op = strategy.new_tree(&mut runner).unwrap().current();
                let (intent, context) = (
                    intent_of(&op),
                    context_of(&op, 10_000 + (sequence * OPS_PER_SEQUENCE + step) as i64),
                );
                let state = world.state(&context.tenant);
                compare_transition(&state, &intent, &context, &mut tally);
                // A snapshot of another tenant than the context names.
                let elsewhere = world.state(&tenant_id(WORLD_TENANTS[(op.tenant + 1) % 3]));
                compare_transition(&elsewhere, &intent, &context, &mut tally);
                if let Ok((next, _)) = world.step(&intent, &context) {
                    world = next;
                }
            }
            if sequence % 4 == 0 {
                let now = 99_000 + (index * SEQUENCES + sequence) as i64;
                compare_reads_of(&world, now, &mut tally);
                compare_visibility(&world, now, &mut tally);
            }
        }
    }
    // The corpus reaches both outcomes, so agreement is not agreement on one answer.
    assert!(
        tally.accepted > 200,
        "only {} accepted decisions",
        tally.accepted
    );
    assert!(
        tally.rejected > 200,
        "only {} rejected decisions",
        tally.rejected
    );
    assert!(tally.reads > 1000, "only {} reads", tally.reads);
    assert!(
        tally.visibility > 500,
        "only {} visibility checks",
        tally.visibility
    );
    eprintln!(
        "compared {} decisions ({} accepted, {} rejected), {} reads, {} visibility checks, {} plans; \
         {} policy-condition details matched under difference 1",
        tally.compared,
        tally.accepted,
        tally.rejected,
        tally.reads,
        tally.visibility,
        tally.needs,
        tally.evaluation_failed_details
    );
}

/// The comparison can fail: the same inputs with a hand-written answer that has been altered are
/// not equal to the generated one. Guards against `assert_eq!` on two values that are always
/// equal by construction.
#[test]
fn the_comparison_notices_a_difference() {
    let world = notes_world();
    let context = ctx("alpha", user("alpha:owner"), 10_000);
    let intent = new_note("A title", "a-slug");
    let state = world.state(&context.tenant);
    let hand = transition(&state, &intent, &context).expect("the owner may create a note");
    let texts = texts_for(&state, &intent);
    let mut other = match spec::kernel_transition(&state, &intent, &context, &texts) {
        Ok(result) => result,
        Err(_) => panic!("the generated kernel refused what the hand kernel accepted"),
    };
    assert_eq!(hand, other);
    other.evidence.pop();
    assert_ne!(hand, other, "dropping a check must make the results differ");
}
