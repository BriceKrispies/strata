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
//!   4. Not exercised: the intents of invitations and of the roles and removals of teams, and
//!      schema changes other than `DefineField`, `DefineRelationship`, `DefineConstraint`,
//!      `DefineQuery`, `DefineRolePermissions`, `RemoveField`, the definitions of triggers,
//!      automations, tenant operations and composite unique constraints (and their removals), of
//!      settings and of capabilities, which the generator below adds to the seven intents of
//!      `properties.rs`, with the writes of settings, `DefineTeam` and `AddTeamMember`. `texts_for` reads defaults and
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
//!      a hundred and one triggers.
//!   8. Not mirrored: the problems of the aggregates a named query declares, of its declared
//!      fields and of a view section that names an aggregate (`strata_kernel::aggregates`,
//!      AGG-001 to AGG-023): the generated `schema-problems` states none of them, and the
//!      aggregate itself is not a read of the generated `kernel-read`. The corpus defines no
//!      query with an aggregate.
//!  10. Not mirrored, in the trigger decisions of `strata_kernel::triggers` (the rest of the module
//!      is mirrored by `after-decision`): (a) the check that a condition or a run key reads nothing
//!      the automation principal may not read (`unreadable`, which declines with `condition-failed`)
//!      and the `InsufficientState` of a record a condition follows to (`expr::missing`): both walk
//!      the expression tree, a structural recursion that specc refuses, and following a reference
//!      is difference 2; (b) the run key of a started run: the kernel writes the SHA-256 of the
//!      canonical JSON of the value, and the function language has no hash, so the generated
//!      `trigger-start` writes the canonical JSON itself (`value-canonical-json`) and the test
//!      hashes it with `canon::hash_bytes` before comparing, which compares the hash of the same
//!      text; a run key that is a list holding a list is written one level deep only, and the
//!      corpus has no such key. Guarded: `assert_no_input_of_a_known_difference` fails on a trigger
//!      condition or an automation run key that follows a reference or reads a field some object
//!      type protects with a read condition.
//!  12. Not exercised, and not mirrored: the invocation of a tenant operation (`InvokeOperation`,
//!      `crates/kernel/src/operations.rs`, WG-5100). The generated `intent-decision` refuses it
//!      with `operation:decided_by_the_kernel` and the generated `needs` plans nothing for it,
//!      because the function language does not state the decision yet. The operation generator
//!      draws no invocation, and `compare_transition` fails on one.
//!  13. Not mirrored: the tenant ledger (`strata_kernel::ledger`, WG-5083). The generated
//!      `intent-decision` refuses `PostLedger`, `apply-change` refuses the ledger schema changes,
//!      `needs` plans no ledger need, `apply-delta` leaves a ledger operation out and
//!      `schema-problems` does not check ledger declarations; the functions that rebuild a tenant
//!      schema or state carry its `ledger` member unchanged. The generator draws no ledger intent
//!      or change: `assert_no_input_of_a_known_difference` fails on a snapshot that holds a
//!      ledger, and `compare_transition` on a posting.
//!  14. Not exercised, and not mirrored: the checks of the declarations of an object type, of the
//!      templates, tokens and product records of the tenant schema that the hand-written `schema.problems` makes through
//!      `object_meta::problems`, `templates::problems` and `products::problems` (retention from a
//!      timestamp field, the states of a publication, the type of a language field, the one-line
//!      subject of a template, the expression of a token, the owner of an entity), and the
//!      removal of an object type with the deletion of its records. The generated
//!      `schema-problems` does not state them. The generator draws no such declaration, template,
//!      token or product record.
//!  15. Not exercised, and not mirrored: planned named queries (`strata_kernel::query_plan`,
//!      KRN-2520 to KRN-2524). The hand-written kernel serves a query whose filter has an
//!      indexable condition from `Need::IndexPage` candidates, pages them with a
//!      `PageCursor::At` or `After` cursor, plans `read_needs` as index pages, evaluates every
//!      query filter and order on the record as the reader receives it, and adds a
//!      `query-plan:<query>` warning to the evidence of a schema change that defines a scan or
//!      turns a planned query into one. The generated `kernel-read`, `read-needs` and
//!      `change-schema` still scan every filtered query and refuse an `at` cursor; the spec
//!      functions are wired afterwards. The rows of a planned read equal the scan's
//!      (`tests/query_plan.rs`), so only the cursor, the needs and the evidence differ. Guarded:
//!      `compare_read` fails on a read of a query that has an index plan, and
//!      `compare_transition` fails on a schema change whose evidence would hold a scan warning.
//!  16. Not exercised, and not mirrored: the erasure of a data subject (`EraseSubject`,
//!      `crates/kernel/src/erasure.rs`, D-7460). The generated `intent-decision` refuses it with
//!      `erasure:decided_by_the_kernel` and the generated `needs` plans nothing for it. The
//!      operation generator draws no erasure, and `compare_transition` fails on one.
//!
//! Every difference that is not compared through an adjustment is guarded, so that a corpus that
//! grows into one is told to mirror the rule or to count the difference here:
//! `assert_no_input_of_a_known_difference` runs on every snapshot the test decides or reads and
//! fails on an input of differences 2, 7, 8, 10, 13 or 14; `compare_transition` fails on an
//! intent of differences 12, 13 or 16 and on a schema change that warns of a scan (difference
//! 15); `compare_read` fails on a read of a planned query (difference 15). Difference 1 is
//! compared through an adjustment (the detail's prefix) and the run key of difference 10 through
//! another (`hash_run_keys`). Differences 3 to 6 are guarded by the operation generator drawing
//! nothing that reaches them.
//!
//! Mirrored, so no longer differences: the automation principal (`automation:<id>` holds the
//! run-as roles of its automation and no other, and the prefix is reserved: `roles-of`,
//! `reserved-principal`, `automation-roles`; WG-5160), the trigger decisions at commit (the
//! effects `StartAutomation` and `DeclineAutomation`, the `transitions` of a record-updated event,
//! the definer and time stamped on a trigger or an automation, the suspension of an automation
//! whose definer lost authority: `after-decision`, `stamp-definitions`; WG-5136), layered settings
//! and product capabilities (the write of an override, the resolution, the effects of a change of
//! definitions, the checks of definitions and of the grants of declared capabilities:
//! `write-setting`, `resolve-setting`, `setting-change-effects`, `setting-problems`,
//! `capability-problems`, `capability-grant-problem`; WG-5140), the size limits of `limits.rs` (the bytes of a record, the
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
    /// 0..=3 the members of the tenant, 4 a stranger, 5 an anonymous visitor, 6 the system, 7 the
    /// automation `award` acting as its principal without a session, 8 that principal with a
    /// session, which the kernel refuses.
    actor: usize,
    kind: OpKind,
    /// Whether the change carries a hop count past the limit of the triggers (the hop of the
    /// automation worker): one op in eight.
    past_hop_limit: bool,
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
    /// The write of an override of a setting (or its removal), at a level, with a revision the
    /// caller claims to have read.
    WriteSetting {
        /// 0 `theme`, 1 `mode`, 2 a setting that is not defined.
        setting: u8,
        /// 0 tenant, 1 team, 2 person, 3 platform.
        level: u8,
        /// 0 none, 1 the team `desk`, 2 a team that does not exist.
        team: u8,
        /// 0 and 1 a string, 2 an integer, 3 the null, 4 no value (a removal), 5 a bool.
        value: u8,
        expected: u64,
    },
    /// The team `desk`, so that the team level of a setting has someone to resolve for.
    DefineTeam,
    AddToTeam {
        who: usize,
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
    /// An automation whose run key is a parameter of the event, so a started run carries a hash.
    DefineKeyedAutomation,
    BrokenAutomation,
    RemoveAutomation,
    DefineTrigger,
    /// A trigger on the creation of a note that starts the keyed automation.
    DefineCreateTrigger,
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
    /// A setting `theme` (0 a string that every level may override, 1 a bool, which changes the
    /// type of the first, 2 a string a lock of the tenant closes below the tenant level) and the
    /// setting `mode`, a string that only the person level overrides (3).
    DefineSetting(u8),
    RemoveSetting(u8),
    /// A capability a product declares, one with an id of no product form, and a grant of a
    /// capability of the product form to a role.
    DefineCapability,
    DefineBadCapability,
    GrantProductCapability(bool),
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
        1 => Just(SchemaOp::DefineKeyedAutomation),
        1 => Just(SchemaOp::BrokenAutomation),
        1 => Just(SchemaOp::RemoveAutomation),
        2 => Just(SchemaOp::DefineTrigger),
        1 => Just(SchemaOp::DefineCreateTrigger),
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
        3 => (0u8..4).prop_map(SchemaOp::DefineSetting),
        1 => (0u8..2).prop_map(SchemaOp::RemoveSetting),
        1 => Just(SchemaOp::DefineCapability),
        1 => Just(SchemaOp::DefineBadCapability),
        1 => any::<bool>().prop_map(SchemaOp::GrantProductCapability),
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
        4 => (0u8..3, 0u8..4, 0u8..3, 0u8..6, 0u64..4).prop_map(|(setting, level, team, value, expected)| {
            OpKind::WriteSetting { setting, level, team, value, expected }
        }),
        1 => Just(OpKind::DefineTeam),
        1 => (0usize..5).prop_map(|who| OpKind::AddToTeam { who }),
    ];
    (0usize..3, 0usize..3, 0usize..9, kind, 0u8..10, 0u8..8).prop_map(
        |(tenant, other, actor, kind, cross, hop)| Op {
            tenant,
            actor_tenant: if cross == 0 { other } else { tenant },
            actor,
            kind,
            past_hop_limit: hop == 0,
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
                SchemaOp::DefineKeyedAutomation => SchemaChange::DefineAutomation(keyed()),
                SchemaOp::BrokenAutomation => SchemaChange::DefineAutomation(award(true)),
                SchemaOp::RemoveAutomation => {
                    SchemaChange::RemoveAutomation(WorkflowId::new("award").unwrap())
                }
                SchemaOp::DefineTrigger => SchemaChange::DefineTrigger(on_publish()),
                SchemaOp::DefineCreateTrigger => SchemaChange::DefineTrigger(on_create()),
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
                    aggregates: None,
                    declared_fields: None,
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
                SchemaOp::DefineSetting(kind) => SchemaChange::DefineSetting(setting_def(*kind)),
                SchemaOp::RemoveSetting(kind) => SchemaChange::RemoveSetting(
                    SettingId::new(if *kind == 0 { "theme" } else { "mode" }).unwrap(),
                ),
                SchemaOp::DefineCapability => SchemaChange::DefineCapability(CapabilityDef {
                    id: CapabilityName("member_tiers__promote".into()),
                    name: "Promote".into(),
                    description: "Promote a member of a tier".into(),
                }),
                SchemaOp::DefineBadCapability => SchemaChange::DefineCapability(CapabilityDef {
                    id: CapabilityName("Promote".into()),
                    name: " ".into(),
                    description: "Neither a namespace nor a name".into(),
                }),
                SchemaOp::GrantProductCapability(declared) => SchemaChange::DefineRolePermissions {
                    role: role("editor"),
                    permissions: vec![Permission::Capability(
                        if *declared {
                            "member_tiers__promote"
                        } else {
                            "other_product__thing"
                        }
                        .into(),
                    )],
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
        OpKind::WriteSetting {
            setting,
            level,
            team,
            value,
            expected,
        } => Intent::WriteSetting {
            setting: SettingId::new(["theme", "mode", "nothing"][*setting as usize]).unwrap(),
            level: [
                SettingLevel::Tenant,
                SettingLevel::Team,
                SettingLevel::Person,
                SettingLevel::Platform,
            ][*level as usize],
            team: [None, Some("desk"), Some("ghost")][*team as usize]
                .map(|name| TeamId::new(name).unwrap()),
            value: match value {
                0 => Some(text("dark")),
                1 => Some(text("light")),
                2 => Some(Value::Int(1)),
                3 => Some(Value::Null),
                4 => None,
                _ => Some(Value::Bool(true)),
            },
            expected_revision: *expected,
        },
        OpKind::DefineTeam => Intent::DefineTeam {
            team: TeamId::new("desk").unwrap(),
            label: "Desk".into(),
        },
        OpKind::AddToTeam { who } => Intent::AddTeamMember {
            team: TeamId::new("desk").unwrap(),
            principal: principal_id(&format!("{}:{}", WORLD_TENANTS[op.tenant], MEMBERS[*who])),
        },
    }
}

/// An automation that creates a note, looks at it and ends; `broken`, it names a step it does
/// not declare and leaves one unreachable. It acts as `editor`, so its definition is checked for the
/// conferral of that role, its principal reads what an editor reads and it is suspended when its
/// definer loses the authority to confer the role (WFL-1528, WFL-1529).
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
            roles: BTreeSet::from([role("editor")]),
        },
        run_key: None,
        start: name("make"),
        steps,
        defined_by: None,
        defined_at: None,
        suspended_for: None,
    }
}

/// An automation that only ends, run as `editor`, with a run key that is the sequence of the event:
/// a run it starts carries the hash of that value (difference 10 b).
fn keyed() -> AutomationDef {
    let done = StepName::new("done").unwrap();
    AutomationDef {
        id: WorkflowId::new("keyed").unwrap(),
        label: "Keyed".into(),
        run_as: RunAs {
            roles: BTreeSet::from([role("editor")]),
        },
        run_key: Some(Expr::Param("seq".into())),
        start: done.clone(),
        steps: BTreeMap::from([(
            done,
            AutomationStepDef {
                action: AutomationStep::End,
                next: None,
                retry: None,
                timeout: None,
                compensate: None,
            },
        )]),
        defined_by: None,
        defined_at: None,
        suspended_for: None,
    }
}

/// A trigger on the creation of a note that starts `keyed`, with no condition.
fn on_create() -> TriggerDef {
    TriggerDef {
        id: TriggerId::new("on_create").unwrap(),
        on: TriggerEvent::RecordCreated {
            object: object("note"),
        },
        condition: None,
        automation: WorkflowId::new("keyed").unwrap(),
        paused: None,
        defined_by: None,
        defined_at: None,
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

/// A definition of a setting: 0 `theme`, a string every level may override; 1 `theme` as a bool (a
/// change of its type); 2 `theme` as a string whose lock of the tenant closes the team and person
/// levels; 3 `mode`, a string only the person level overrides.
fn setting_def(kind: u8) -> SettingDef {
    let (id, ty, default, levels, locked_by) = match kind {
        0 => (
            "theme",
            "string",
            text("light"),
            vec![
                SettingLevel::Tenant,
                SettingLevel::Team,
                SettingLevel::Person,
            ],
            None,
        ),
        1 => (
            "theme",
            "bool",
            Value::Bool(false),
            vec![SettingLevel::Tenant, SettingLevel::Person],
            None,
        ),
        2 => (
            "theme",
            "string",
            text("light"),
            vec![
                SettingLevel::Tenant,
                SettingLevel::Team,
                SettingLevel::Person,
            ],
            Some(SettingLevel::Tenant),
        ),
        _ => (
            "mode",
            "string",
            text("plain"),
            vec![SettingLevel::Person],
            None,
        ),
    };
    SettingDef {
        id: SettingId::new(id).unwrap(),
        label: id.into(),
        r#type: SettingType::new(ty).unwrap(),
        default,
        levels: levels.into_iter().collect(),
        locked_by,
    }
}

fn principal_of(tenant: usize, actor: usize) -> Principal {
    match actor {
        6 => Principal::System,
        7 | 8 => Principal::User(strata_kernel::automation::automation_principal(
            &WorkflowId::new("award").unwrap(),
        )),
        _ => principals(WORLD_TENANTS[tenant])[actor].clone(),
    }
}

fn context_of(op: &Op, now: i64) -> Context {
    let mut context = ctx(
        WORLD_TENANTS[op.tenant],
        principal_of(op.actor_tenant, op.actor),
        now,
    );
    if op.actor == 8 {
        context.session = Some(SessionId::new("s-1").unwrap());
    }
    if op.past_hop_limit {
        context.hop = Some(9);
    }
    context
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

/// Whether an expression reads a field of a record scope whose name satisfies `named`.
fn reads_a_field(expr: &Expr, named: &dyn Fn(&str) -> bool) -> bool {
    match expr {
        Expr::Var(_, name) => named(name),
        Expr::Through(..)
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
        | Expr::Text(a) => reads_a_field(a, named),
        Expr::And(items) | Expr::Or(items) => items.iter().any(|item| reads_a_field(item, named)),
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => reads_a_field(a, named) || reads_a_field(b, named),
        Expr::If(a, b, c) => {
            reads_a_field(a, named) || reads_a_field(b, named) || reads_a_field(c, named)
        }
    }
}

/// Differences 2, 7, 8, 10, 13 and 14 of the header: fail when a snapshot holds an input of a
/// rule the generated functions do not mirror, so the corpus cannot start to depend on one
/// silently.
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
    let protected: BTreeSet<&str> = tenant
        .schema
        .objects
        .values()
        .flat_map(|def| def.fields.values())
        .filter(|field| field.read.is_some())
        .map(|field| field.id.as_str())
        .collect();
    let unreadable = |expr: &Expr| {
        mentions_through(expr) || reads_a_field(expr, &|name| protected.contains(name))
    };
    for trigger in tenant.schema.triggers.iter().flat_map(BTreeMap::values) {
        assert!(
            !trigger.condition.as_ref().is_some_and(unreadable),
            "trigger {} has a condition that follows a reference or reads a protected field; the generated decide-run does not check what the automation may read (difference 10)",
            trigger.id
        );
    }
    for automation in tenant.schema.automations.iter().flat_map(BTreeMap::values) {
        assert!(
            !automation.run_key.as_ref().is_some_and(unreadable),
            "{} has a run key that follows a reference or reads a protected field; the generated decide-run does not check what the automation may read (difference 10)",
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

/// Difference 10 (b) of the header: the generated `trigger-start` writes the canonical JSON of a
/// run key where the kernel writes its SHA-256 in hexadecimal; hash it, so both sides carry the
/// hash of the same text.
fn hash_run_keys(result: &mut Result<Transition, DomainError>) {
    let Ok(outcome) = result else {
        return;
    };
    for effect in &mut outcome.effects {
        if let Effect::StartAutomation(start) = effect {
            start.run_key = start
                .run_key
                .take()
                .map(|text| strata_kernel::canon::hash_bytes(text.as_bytes()));
        }
    }
}

/// Difference 15 of the header: a read of a query with an index plan is not compared, because the
/// generated `kernel-read` scans it and issues another cursor.
fn assert_not_planned(state: &State, request: &ReadRequest) {
    let (Some(tenant), ReadRequest::Named { query, .. }) = (&state.tenant, request) else {
        return;
    };
    if let Some(def) = tenant.schema.queries.get(query) {
        assert!(
            !matches!(
                strata_kernel::query_plan::plan_query(&tenant.schema, def),
                strata_kernel::query_plan::QueryPlan::Index(_)
            ),
            "query {query} has an index plan; the generated kernel-read scans it (difference 15)"
        );
    }
}

/// Difference 15 of the header: a schema change whose evidence holds a scan warning is not
/// compared, because the generated `change-schema` writes no such warning.
fn assert_no_scan_warning(state: &State, intent: &Intent, hand: &Result<Transition, DomainError>) {
    let (Intent::ChangeSchema { changes, .. }, Ok(accepted), Some(tenant)) =
        (intent, hand, &state.tenant)
    else {
        return;
    };
    for op in &accepted.delta.ops {
        if let DeltaOp::PutSchema(after) = op {
            assert!(
                strata_kernel::query_plan::scan_warnings(&tenant.schema, after, changes).is_empty(),
                "the schema change warns of a scan; the generated change-schema does not (difference 15)"
            );
        }
    }
}

#[derive(Default)]
struct Tally {
    compared: usize,
    accepted: usize,
    rejected: usize,
    reads: usize,
    visibility: usize,
    settings: usize,
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
    assert!(
        !matches!(intent, Intent::PostLedger { .. }),
        "{label}: the generated functions do not decide a posting (difference 13)"
    );
    assert!(
        !matches!(intent, Intent::EraseSubject { .. }),
        "{label}: the generated functions do not decide an erasure (difference 16)"
    );
    let mut hand = transition(state, intent, context);
    assert_no_scan_warning(state, intent, &hand);
    let texts = texts_for(state, intent);
    let mut generated = match spec::kernel_transition(state, intent, context, &texts) {
        Ok(result) => Ok(result),
        Err(failure) => Err(domain_error(failure).unwrap_or_else(|fault| {
            panic!(
                "{label}: the generated kernel failed with {fault}; the hand kernel gave {hand:?}"
            )
        })),
    };
    hash_run_keys(&mut generated);
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
    assert_not_planned(state, request);
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
            for actor in 0..8 {
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

/// The resolution of every setting for every principal of every tenant (`settings::resolve` and the
/// generated `resolve-setting`): a defined setting, one that is not, and a person in a team.
fn compare_settings(world: &World, tally: &mut Tally) {
    let settings: Vec<SettingId> = ["theme", "mode", "nothing"]
        .into_iter()
        .map(|name| SettingId::new(name).unwrap())
        .collect();
    for tenant in world.tenants.values() {
        let mut principals: Vec<Principal> = (0..WORLD_TENANTS.len())
            .flat_map(|index| (0..6).map(move |actor| principal_of(index, actor)))
            .collect();
        principals.push(Principal::System);
        for principal in principals {
            for setting in &settings {
                let hand = strata_kernel::settings::resolve(tenant, &principal, setting);
                let generated = spec::resolve_setting(tenant, &principal, setting);
                let generated = generated.map_err(|failure| {
                    domain_error(failure).unwrap_or_else(|fault| {
                        panic!("resolve-setting of {setting} faulted: {fault}")
                    })
                });
                assert_eq!(
                    hand, generated,
                    "the resolution of {setting} for {principal:?} in {} differs",
                    tenant.id
                );
                tally.settings += 1;
            }
        }
    }
}

/// `visible` on every stored record, for every principal.
fn compare_visibility(world: &World, now: i64, tally: &mut Tally) {
    for tenant in world.tenants.values() {
        for actor_tenant in 0..WORLD_TENANTS.len() {
            for actor in 0..8 {
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
                compare_settings(&world, &mut tally);
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
    assert!(
        tally.settings > 500,
        "only {} setting resolutions",
        tally.settings
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
