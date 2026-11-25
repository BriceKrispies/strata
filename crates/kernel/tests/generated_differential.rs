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
//!   3. Not exercised: a query order key over a list-valued attribute. The generated order
//!      does not order a list element by element, as `Value`'s derived order does.
//!      The corpus has no list-valued order key (it has a list-valued field, `tags`, which no
//!      query orders by).
//!   4. Not exercised: the intents of invitations and of the roles and removals of teams, and
//!      schema changes other than `DefineField`, `DefineRelationship`, `DefineConstraint`,
//!      `DefineQuery`, `DefineRolePermissions`, `RemoveField`, the definitions of triggers,
//!      automations, tenant operations and composite unique constraints (and their removals), of
//!      settings and of capabilities, which the generator below adds to the seven intents of
//!      `properties.rs`, with the writes of settings, `DefineTeam` and `AddTeamMember`. The functions of the other intents are mirrored all the same (the conferral
//!      of a role, the owner that remains, the reserved `team:` prefix, the rewrite of records
//!      by a schema change), each with examples in the specification.
//!   7. Not exercised, and not mirrored: the bounds the kernel checks on a definition that the
//!      specification does not state yet (specc compiles durations and structural recursion
//!      since WG-5132 and WG-5133, but the functions are not written): retry delays, attempt
//!      timeouts and waits of an automation step, and the two hundred
//!      nodes of an operation condition. The generator draws no definition that breaks one, nor
//!      a hundred and one triggers.
//!   8. Not mirrored: the problems of the aggregates a named query declares, of its declared
//!      fields and of a view section that names an aggregate (`strata_kernel::aggregates`,
//!      AGG-001 to AGG-023): the generated `schema-problems` states none of them, and the
//!      aggregate itself is not a read of the generated `kernel-read`. The corpus defines no
//!      query with an aggregate.
//!   9. Not mirrored, in the trigger decisions of `strata_kernel::triggers` (the rest of the module
//!      is mirrored by `after-decision`): (a) the check that a condition or a run key reads nothing
//!      the automation principal may not read (`unreadable`, which declines with `condition-failed`)
//!      and the `InsufficientState` of a record a condition follows to (`expr::missing`): both walk
//!      the expression tree, and the generated `decide-run` evaluates a trigger condition without
//!      the snapshot's lookup; (b) the run key of a started run: the kernel writes the SHA-256 of the
//!      canonical JSON of the value, and the function language has no hash, so the generated
//!      `trigger-start` writes the canonical JSON itself (`value-canonical-json`) and the test
//!      hashes it with `canon::hash_bytes` before comparing, which compares the hash of the same
//!      text; a run key that is a list holding a list is written one level deep only, and the
//!      corpus has no such key; (c) the parameters of a run started by a trigger of the event
//!      ledger-posted, which starts a run for the entry a ledger posting writes, with the
//!      parameters owner, unit, amount, balance, balance_before, entry_sequence and reason: the
//!      generated `event-params` does not, and the generator declares no ledger unit and posts
//!      nothing (difference 11). Guarded: `assert_no_input_of_a_known_difference` fails on a trigger
//!      condition or an automation run key that follows a reference or reads a field some object
//!      type protects with a read condition.
//!  10. Not exercised, and not mirrored: the invocation of a tenant operation (`InvokeOperation`,
//!      `crates/kernel/src/operations.rs`, WG-5100). The generated `intent-decision` refuses it
//!      with `operation:decided_by_the_kernel` and the generated `needs` plans nothing for it,
//!      because the function language does not state the decision yet. The operation generator
//!      draws no invocation, and `compare_transition` fails on one.
//!  11. Not mirrored: the posting of the tenant ledger from the reading of an account on
//!      (`strata_kernel::ledger::post`, the `PostLedger` intent, a transfer as two postings
//!      committed together; WG-5083) and everything that reads an account. Mirrored, and compared
//!      on the postings `OpKind::PostLedger` draws: the decision of `PostLedger` up to the account
//!      (`ledger-intent-decision`, `ledger-posting-precheck`; WG-5170): the capability
//!      `ledger-post`, the count of postings, and the checks of the first posting before its
//!      account is read (a declared unit, an owner that exists, the amount, the key, the reason
//!      and the scope); the generated `intent-decision` refuses a first posting that passes them
//!      with `ledger:decided_by_the_ledger_module`. An account is named by a UUID derived from the
//!      SHA-256 of its tenant, owner, unit and scope (`account_id`), and the function language
//!      has neither a hash nor a way to build a uuid; and an example argument or result of a
//!      generated function may not contain a uuid, which every account header and entry holds,
//!      so no example of a decision over a snapshot with accounts can be written. The generated
//!      `intent-decision` refuses such a `PostLedger` (`ledger:decided_by_the_ledger_module`),
//!      `apply-delta` leaves a ledger operation out, the plan of a posting (`refine_needs`) is not
//!      stated, and the branch of `ledger-removal-check` that finds an account naming a unit or a
//!      pool has no example. Mirrored: the declarations (`declare-ledger-unit`, `-pool`,
//!      `set-ledger-limit`, `remove-ledger-definition` in `apply-change`), their problems
//!      (`ledger-problems`), the capability `ledger-configure`, the needs of a removal, the
//!      references of a trigger, an automation step and an operation step to a unit, and, compared
//!      directly (`the_ledger_periods_and_limits_agree_with_the_generated_functions`), the
//!      calendar periods and the limits of an account (`ledger-period-start`, `ledger-limits-of`).
//!      Also not mirrored, for the same reason (WG-5168, WG-5169, WG-5172): a policy over the
//!      ledger resource (the generated `policy-problems` checks its condition against no fields
//!      and has no arm for the fields of an account), the buckets of credit of an account header
//!      and the expiry of a unit, and the intent `ExpireLedger`, which the generated
//!      `intent-decision` refuses with `ledger:decided_by_the_ledger_module`. Guarded:
//!      `assert_no_input_of_a_known_difference` fails on a snapshot whose state holds a ledger
//!      account or entry and on a policy that names the ledger, and `compare_transition`
//!      on an expiry and on a posting that the kernel decides after reading its account.
//!      Mirrored, and compared on the schema changes the generator draws: the limits of single
//!      accounts, which live in the ledger's state and not in the schema (WG-5172, LDG-1550,
//!      D-6554): the checks of a change (`ledger-limit-checks`: the need, the ceiling, the
//!      removal of a limit that is not set, the evidence `ledger:account-limits`), the line and
//!      the refusal of the limit of an account in `apply-change`, and the operations that put or
//!      remove it (`ledger-limit-ops`). A snapshot that holds only limits is not a difference.
//!  12. Not exercised, and not mirrored: the checks of the declarations of an object type, of the
//!      templates, tokens and product records of the tenant schema that the hand-written
//!      `schema.problems` makes through `object_meta::problems`, `templates::problems` and
//!      `products::problems` (retention from a timestamp field, the states of a publication, the
//!      type of a language field, the one-line subject of a template, the expression of a token,
//!      the owner of an entity, the searchable fields of an object type), and the removal of an
//!      object type with the deletion of its records. Also the blocks of a template body
//!      (`blocks::analyse`: the block schema, the tokens and typed values it places, the allowed
//!      link schemes and the nesting bound, D-7900) and the refusal of a schema change from
//!      outside a product to an entity it owns (`products::foreign_change`, with the capability
//!      bundle-install of a change that carries a product record, D-6582, INT-1582): the generated
//!      `schema-problems` and `change-schema` do not state them. The generator draws no such
//!      declaration, template, token or product record.
//!  13. Not exercised, and not mirrored: planned named queries (`strata_kernel::query_plan`,
//!      KRN-2520 to KRN-2524, KRN-2920 to KRN-2923). The hand-written kernel serves a query whose
//!      filter has an indexable condition from `Need::IndexPage` candidates, and an equality or a
//!      list of values on one field ordered by another attribute (a pair plan,
//!      `PlanOrder::By`) from `Need::PairPage` candidates, pages them with a `PageCursor::At` or
//!      `After` cursor, plans `read_needs` as index and pair pages, evaluates every query filter
//!      and order on the record as the reader receives it, and adds a `query-plan:<query>`
//!      warning, with the one change that would plan the query (`plan_hint`), to the evidence of
//!      a schema change that defines a scan or turns a planned query into one. The generated
//!      `kernel-read`, `read-needs` and `change-schema` still scan every filtered query and refuse
//!      an `at` cursor; the spec functions are wired afterwards. The rows of a planned read equal
//!      the scan's (`tests/query_plan.rs`), so only the cursor, the needs and the evidence differ.
//!      `query_plan::read_missing`, which names every record the rows of a read lack at once, is
//!      a shell-side helper with no generated counterpart and takes no part in a decision.
//!      Guarded: `compare_read` fails on a read of a query that has an index plan (a pair plan is
//!      one), and `compare_transition` fails on a schema change whose evidence would hold a scan
//!      warning.
//!  15. Not mirrored: the class and the impact set of a schema change (D-8212, MIG-589,
//!      MIG-592) and the refusal of a breaking change that reaches a dependent
//!      (`strata_kernel::dependencies`). The hand-written kernel writes a `schema:class` check for
//!      each change, which is dropped before the comparison (`drop_evolution_evidence`); the
//!      refusal by impact is not drawn by the generator, and the comparison fails on one. The three changes of D-8212..D-8217 (`rename-field`, `move-field`, `retype-field`) are decided by the hand-written kernel alone: the generated `apply-change` ends in a wildcard arm that refuses them as not a change of a declaration, and the generator draws none (the evidence `schema:rewrite`, `schema:pin` and `schema:projection` they write is dropped with `schema:class`).
//!  14. Not exercised, and not mirrored: the erasure of a data subject (`EraseSubject`,
//!      `crates/kernel/src/erasure.rs`, D-7460). The generated `intent-decision` refuses it with
//!      `erasure:decided_by_the_kernel` and the generated `needs` plans nothing for it. The
//!      operation generator draws no erasure, and `compare_transition` fails on one. The same
//!      holds for the deletion of records past their retention (`ExpireRecords`,
//!      `crates/kernel/src/retention.rs`, PRV-357): refused by the generated `intent-decision`
//!      with `retention:decided_by_the_kernel`, nothing planned by the generated `needs`.
//!  15. Not mirrored: the checks of an invoke-operation step
//!      (`definitions::invoke_operation_problems`, WG-5154): the operation exists, the run-as
//!      roles may invoke it, and its inputs are declared, present when required and of the shape
//!      of their type. The generated `automation-step-problems` checks only the expressions of its
//!      inputs. Guarded: `assert_no_input_of_a_known_difference` fails on an automation that holds
//!      such a step.
//!  16. Not mirrored: a rule of a transition (a policy condition, a workflow guard or assignment)
//!      that follows the covered one of two different targets of one reference field. The
//!      generated lookup (`rule-lookup`) is keyed by field: it holds a field only when the snapshot
//!      covers the field's targets in both the stored record and the candidate, because Exegra's
//!      lookup tells a target the snapshot covers but does not hold (null) from a field it does
//!      not cover (an error), and has no way to say that one target of a field it holds is not
//!      covered. The kernel's `Lookup` refuses per record. So when an update changes a reference
//!      and the snapshot covers only one of the two targets, a rule that follows the covered one
//!      fails to evaluate where the kernel follows it. A read has one record as old and new and
//!      cannot reach this. Guarded: an update of the corpus never changes a reference, and
//!      `compare_attributed` fails on an intent that does.
//!
//! Every difference that is not compared through an adjustment is guarded, so that a corpus that
//! grows into one is told to mirror the rule or to count the difference here:
//! `assert_no_input_of_a_known_difference` runs on every snapshot the test decides or reads and
//! fails on an input of differences 7, 8, 9, 11, 12 or 15; `compare_transition` fails on an
//! intent of differences 10, 11 or 14 and on a schema change that warns of a scan (difference
//! 13); `compare_read` fails on a read of a planned query (difference 13); `compare_attributed`
//! fails on an update that changes a reference (difference 16). Difference 1 is compared through
//! an adjustment (the detail's prefix) and the run key of difference 9 through another
//! (`hash_run_keys`). Differences 3 and 4 are guarded by the operation generator drawing nothing
//! that reaches them. (Numbers 2, 5 and 6 are free: their differences are mirrored now.)
//!
//! Mirrored, so no longer differences: the reads, query filters, field read conditions and
//! workflow guards that follow a reference, through the snapshot's lookup, with the
//! `InsufficientState` naming the record they need when the snapshot does not cover it
//! (`visible-in`, `redact`, `named-read`, `workflow-step` with `rule-lookup` and `rule-missing`;
//! WG-5096), compared by `compare_following_reads` on a Read policy, a filter and a field read
//! condition that read `record.author.name`, over the complete snapshot and over one that covers
//! only what the read plans to load, and by `compare_attributed` on a workflow guard that reads
//! `old.author.name`; the limits and sizes of a stored value at any depth
//! (`value-limit-problem`, `value-json-bytes`, restated by structural recursion; WG-5133),
//! compared on lists nested two and three deep (`BigShape::DeepString`, `DeepList`, `DeepFine`);
//! a policy on every object whose condition reads a field (`mentions-field` in
//! `policy-problems`, the kernel's `mentions_field`; WG-5133), compared on the policies of
//! `SchemaOp::AnyObjectPolicy`; a policy condition of a transition that follows a
//! reference, through the lookup the snapshot gives, and the `InsufficientState` naming the record
//! it needs when the snapshot does not cover it (`authorize-needing`, `require`; WG-5132,
//! WG-5133), compared by `compare_attributed` on a policy that reads `record.author.name`, over
//! the complete snapshot and over one that covers only what the intent plans to load; the
//! automation principal (`automation:<id>` holds the run-as roles of its automation and no other,
//! and the prefix is reserved: `roles-of`, `reserved-principal`, `automation-roles`; WG-5160), the
//! trigger decisions at commit (the effects `StartAutomation` and `DeclineAutomation`, the
//! `transitions` of a record-updated event, the definer and time stamped on a trigger or an
//! automation, the suspension of an automation whose definer lost authority: `after-decision`,
//! `stamp-definitions`; WG-5136), layered settings and product capabilities (the write of an
//! override, the resolution, the effects of a change of definitions, the checks of definitions and
//! of the grants of declared capabilities: `write-setting`, `resolve-setting`,
//! `setting-change-effects`, `setting-problems`, `capability-problems`,
//! `capability-grant-problem`; WG-5140), the declarations of the tenant ledger (units, pools and
//! limits: `apply-change`, `ledger-problems`, `ledger-removal-check`, `ledger-change-needs`, the
//! capability `ledger-configure`; WG-5170), the size limits of `limits.rs` (the bytes of a record,
//! the length of a string or a list, the encoded size of a value of a unique field:
//! `record-limit-check`, `limit-step`; WG-5092), the field-level read conditions (`redact`, in
//! `kernel-read` and `named-read`; WG-5093), the deletion of a record that two fields of one
//! record refer to (one rewrite per record; WG-5094), the problems of a schema that
//! `schema-problems` did not state: counts and limits, field read conditions, the permissions of
//! roles, teams and invitations (WG-5095, WG-5077); and in an earlier pass: authorisation before
//! the lookup of an object type or a record (`object-def`, `stored`), the conferral of a role
//! counting policies (`may-confer`, `holds-policy-power`), a revocation asking for the authority
//! to confer, the owner that remains counting teams (`owner-lost`), the second authorisation of
//! an update after workflow assignments, the reserved `team:` prefix (`reserved-principal`), the
//! records a schema change rewrites (version one higher, an event each), the limit of 200 changes
//! in one schema change, the permissions of a role granting before any policy (`permission:role`),
//! the plan of a schema change (`ref-needs`) and a query filter that cannot be evaluated on a
//! record leaving the record out.

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model_core::functions::{self as spec, Failure};
use strata_model_core::types::Rejection;

/// How many random operation sequences run from each starting world, and how long each is.
const SEQUENCES: usize = 96;
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
    /// A `PostLedger` intent refused before any account is read (WG-5170): 0 a unit no tenant
    /// declares, 1 an amount of 0, 2 an owner who is not a member, 3 a pool no tenant declares,
    /// 4 an amount past 2^53-1, 5 an empty idempotency key, 6 a key with the expiry prefix, 7 a
    /// reason of 257 bytes, 8 no posting, 9 nine postings.
    PostLedger {
        kind: u8,
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
    /// A string of 65537 bytes three lists deep in `tags`: a limit `limits.rs` finds at any depth
    /// (WG-5133), and a type mismatch, since a field admits a list of scalars only.
    DeepString,
    /// A list of 1001 items two lists deep in `tags`.
    DeepList,
    /// Lists nested three deep within every limit: only the type mismatch.
    DeepFine,
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
    /// One of the changes of the ledger declarations `ledger_change` lists, and a trigger on a
    /// posting of the unit `credits`.
    Ledger(u8),
    DefineLedgerTrigger,
    /// A policy on every object (WG-5133): 0 whose condition reads only `$created_by`, 1 that
    /// reads a field in a branch that never runs, 2 that reads no field and names a role the
    /// tenant does not define, 3 that follows a reference.
    AnyObjectPolicy(u8),
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
        5 => (0u8..13).prop_map(SchemaOp::Ledger),
        1 => Just(SchemaOp::DefineLedgerTrigger),
        2 => (0u8..4).prop_map(SchemaOp::AnyObjectPolicy),
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
        Just(BigShape::DeepString),
        Just(BigShape::DeepList),
        Just(BigShape::DeepFine),
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
        2 => (0u8..10).prop_map(|kind| OpKind::PostLedger { kind }),
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
                BigShape::DeepString => {
                    let deep = Value::List(vec![Value::List(vec![long("a", 65_537)])]);
                    fields.insert(field("tags"), Value::List(vec![deep]));
                }
                BigShape::DeepList => {
                    let deep = Value::List(vec![Value::List(vec![text("x"); 1001])]);
                    fields.insert(field("tags"), Value::List(vec![deep]));
                }
                BigShape::DeepFine => {
                    let deep = Value::List(vec![Value::List(vec![text("x")])]);
                    fields.insert(field("tags"), Value::List(vec![deep, text("y")]));
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
                SchemaOp::Ledger(kind) => ledger_change(*kind),
                SchemaOp::AnyObjectPolicy(kind) => {
                    SchemaChange::DefinePolicy(any_object_policy(*kind))
                }
                SchemaOp::DefineLedgerTrigger => SchemaChange::DefineTrigger(TriggerDef {
                    id: TriggerId::new("on_post").unwrap(),
                    on: TriggerEvent::LedgerPosted {
                        unit: LedgerUnit::new("credits").unwrap(),
                    },
                    condition: None,
                    automation: WorkflowId::new("award").unwrap(),
                    paused: None,
                    defined_by: None,
                    defined_at: None,
                }),
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
        OpKind::PostLedger { kind } => {
            let tenant = WORLD_TENANTS[op.tenant];
            let member = LedgerOwner::Principal {
                principal: principal_id(&format!("{tenant}:owner")),
            };
            let request =
                |unit: &str, owner: LedgerOwner, amount: i64, key: &str, reason: String| {
                    LedgerPostingRequest {
                        owner,
                        unit: LedgerUnit::new(unit).unwrap(),
                        amount: LedgerAmount(amount),
                        idempotency_key: key.into(),
                        reason,
                        corrects: None,
                        scope: None,
                    }
                };
            let postings = match kind {
                0 => vec![request("nonesuch", member, 5, "k1", "r".into())],
                1 => vec![request("credits", member, 0, "k1", "r".into())],
                2 => vec![request(
                    "credits",
                    LedgerOwner::Principal {
                        principal: principal_id(&format!("{tenant}:ghost")),
                    },
                    5,
                    "k1",
                    "r".into(),
                )],
                3 => vec![request(
                    "credits",
                    LedgerOwner::Pool {
                        pool: LedgerPoolName::new("nopool").unwrap(),
                    },
                    1,
                    "k1",
                    "r".into(),
                )],
                4 => vec![request(
                    "credits",
                    member,
                    9_007_199_254_740_992,
                    "k1",
                    "r".into(),
                )],
                5 => vec![request("credits", member, 5, "", "r".into())],
                6 => vec![request("credits", member, 5, "expiry:x", "r".into())],
                7 => vec![request("credits", member, 5, "k1", "r".repeat(257))],
                8 => Vec::new(),
                _ => (0..9)
                    .map(|i| request("credits", member.clone(), 0, &format!("k{i}"), "r".into()))
                    .collect(),
            };
            Intent::PostLedger { postings }
        }
    }
}

/// The rules of a posting's checks made before its account is read (`ledger-posting-precheck`,
/// `ledger-intent-decision`; WG-5170): a `PostLedger` that the kernel refuses with one of them,
/// or with a refusal that is not the ledger's, is mirrored; any other is difference 11.
const LEDGER_PRECHECK_RULES: [&str; 8] = [
    "permission:capability:ledger-post",
    "ledger:postings",
    "ledger:unknown-unit",
    "ledger:unknown-owner",
    "ledger:invalid-amount",
    "ledger:invalid-key",
    "ledger:invalid-reason",
    "ledger:invalid-scope",
];

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
        state: None,
    }
}

/// An automation that only ends, run as `writer`, with a run key that is the sequence of the event:
/// a run it starts carries the hash of that value (difference 9 b). The powers of `writer` are
/// given by policies, so defining it checks the conferral of a role whose powers come from
/// policies (`may-confer`, `holds-policy-power`; WG-5102).
fn keyed() -> AutomationDef {
    let done = StepName::new("done").unwrap();
    AutomationDef {
        state: None,
        id: WorkflowId::new("keyed").unwrap(),
        label: "Keyed".into(),
        run_as: RunAs {
            roles: BTreeSet::from([role("writer")]),
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

/// A limit of a ledger unit (no owner) or of one account of it.
fn ledger_limit(
    unit: &str,
    owner: Option<&str>,
    scope: Option<&str>,
    amounts: (Option<i64>, Option<i64>),
    period: Option<(u64, LedgerPeriod)>,
) -> LedgerLimit {
    LedgerLimit {
        unit: LedgerUnit::new(unit).unwrap(),
        owner: owner.map(|principal| LedgerOwner::Principal {
            principal: principal_id(principal),
        }),
        floor: amounts.0.map(LedgerAmount),
        cap: amounts.1.map(LedgerAmount),
        period_cap: period.map(|(cap, _)| cap),
        period: period.map(|(_, period)| period),
        scope: scope.map(|text| LedgerScope(text.to_string())),
    }
}

/// A change of the ledger declarations: 0 and 1 the units `credits` and `points`, 2 the pool
/// `promo`, 3 a limit of `credits` (a floor, a cap and a cap per day), 4 the limit of one account
/// of `credits` in a scope, 5 to 7 limits that are problems (a unit that is not declared, a floor
/// above the cap, a limit that sets nothing), 8 to 10 the removal of the unit, the pool and the
/// limit of `credits`, 11 a period without its cap, 12 a limit of a unit that names a scope.
fn ledger_change(kind: u8) -> SchemaChange {
    let credits = || LedgerUnit::new("credits").unwrap();
    match kind {
        0 | 1 => {
            let unit = if kind == 0 { "credits" } else { "points" };
            SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
                expiry: None,
                unit: LedgerUnit::new(unit).unwrap(),
                description: format!("The unit {unit}"),
            })
        }
        2 => SchemaChange::DeclareLedgerPool(LedgerPoolDecl {
            pool: LedgerPoolName::new("promo").unwrap(),
            description: "A promotion budget".into(),
        }),
        3 => SchemaChange::SetLedgerLimit(ledger_limit(
            "credits",
            None,
            None,
            (Some(0), Some(100)),
            Some((10, LedgerPeriod::Day)),
        )),
        4 => SchemaChange::SetLedgerLimit(ledger_limit(
            "credits",
            Some("alpha:editor"),
            Some("promo"),
            (None, Some(5)),
            None,
        )),
        5 => SchemaChange::SetLedgerLimit(ledger_limit("ghost", None, None, (None, Some(1)), None)),
        6 => SchemaChange::SetLedgerLimit(ledger_limit(
            "credits",
            None,
            None,
            (Some(5), Some(2)),
            None,
        )),
        7 => SchemaChange::SetLedgerLimit(ledger_limit("credits", None, None, (None, None), None)),
        8 => SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Unit { unit: credits() }),
        9 => SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Pool {
            pool: LedgerPoolName::new("promo").unwrap(),
        }),
        10 => SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Limit {
            unit: credits(),
            owner: None,
            scope: None,
        }),
        11 => {
            let mut limit = ledger_limit("credits", None, None, (None, Some(9)), None);
            limit.period = Some(LedgerPeriod::Week);
            SchemaChange::SetLedgerLimit(limit)
        }
        _ => SchemaChange::SetLedgerLimit(ledger_limit(
            "credits",
            None,
            Some("promo"),
            (None, Some(3)),
            None,
        )),
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
        area: None,
        description: None,
        allowed: None,
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

/// Whether an expression follows a reference (`through`): what the generated trigger decisions
/// do not follow (difference 9).
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

/// Differences 7, 8, 9, 11, 12 and 15 of the header: fail when a snapshot holds an input of a
/// rule the generated functions do not mirror, so the corpus cannot start to depend on one
/// silently.
fn assert_no_input_of_a_known_difference(state: &State) {
    let Some(tenant) = &state.tenant else {
        return;
    };
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
        tenant
            .ledger
            .as_ref()
            .is_none_or(|ledger| ledger.accounts.is_empty() && ledger.entries.is_empty()),
        "{} holds ledger accounts or entries; the generated functions do not mirror them (difference 11)",
        tenant.id
    );
    assert!(
        tenant
            .schema
            .policies
            .values()
            .all(|policy| !matches!(policy.resource, Resource::Ledger { .. })),
        "{} has a policy over the ledger; the generated functions do not mirror it (difference 11)",
        tenant.id
    );
    assert!(
        tenant.schema.mail_templates.is_none()
            && tenant.schema.notice_templates.is_none()
            && tenant.schema.tokens.is_none()
            && tenant.schema.products.is_none(),
        "the schema holds a template, a token or a product record; the generated functions do not state their checks (difference 12)"
    );
    for (object_id, def) in &tenant.schema.objects {
        assert!(
            def.retention.is_none()
                && def.publication.is_none()
                && def.language_field.is_none()
                && def.translation_group.is_none()
                && def.searchable.is_none(),
            "{object_id} declares retention, publication, a language field, a translation group or searchable fields; the generated functions do not state their checks (difference 12)"
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
            "trigger {} has a condition that follows a reference or reads a protected field; the generated decide-run does not check what the automation may read (difference 9)",
            trigger.id
        );
    }
    for automation in tenant.schema.automations.iter().flat_map(BTreeMap::values) {
        assert!(
            !automation.run_key.as_ref().is_some_and(unreadable),
            "{} has a run key that follows a reference or reads a protected field; the generated decide-run does not check what the automation may read (difference 9)",
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
            assert!(
                !matches!(step.action, AutomationStep::InvokeOperation { .. }),
                "{}.{name} invokes an operation; the generated functions do not check the operation, its invokers or its inputs (difference 15)",
                automation.id
            );
        }
    }
}

/// Difference 9 (b) of the header: the generated `trigger-start` writes the canonical JSON of a
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

/// Difference 13 of the header: a read of a query with an index plan is not compared, because the
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
            "query {query} has an index plan; the generated kernel-read scans it (difference 13)"
        );
    }
}

/// Difference 15 of the header: the class and the impact of each schema change (D-8212) are
/// written to the evidence of the hand-written kernel as `schema:class` checks, and a breaking
/// change that reaches a dependent is refused with `schema:impact` checks. The generated
/// `change-schema` states neither yet, so the class checks are dropped before the comparison, and
/// a refusal by impact fails the test: the generator draws no such change.
fn drop_evolution_evidence(label: &str, hand: &mut Result<Transition, DomainError>) {
    let evidence = match hand {
        Ok(transition) => &mut transition.evidence,
        Err(error) => &mut error.evidence,
    };
    assert!(
        !evidence.iter().any(|check| check.rule == "schema:impact"),
        "{label}: refused for a breaking change that reaches a dependent; the generated change-schema accepts it (difference 15)"
    );
    evidence.retain(|check| {
        !matches!(
            check.rule.as_str(),
            "schema:class" | "schema:rewrite" | "schema:pin" | "schema:projection"
        )
    });
}

/// Difference 13 of the header: a schema change whose evidence holds a scan warning is not
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
                "the schema change warns of a scan; the generated change-schema does not (difference 13)"
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
    /// Updates compared under `writers_edit_attributed` (`compare_attributed`).
    attributed: usize,
    /// Of those on a partial snapshot, the ones the kernel answered with `InsufficientState`.
    attributed_insufficient: usize,
    /// Reads compared under rules that follow a reference (`compare_following_reads`).
    following: usize,
    /// Of those on a partial snapshot, the ones the kernel answered with `InsufficientState`.
    following_insufficient: usize,
}

/// The policies on every object of `SchemaOp::AnyObjectPolicy` (WG-5133): only the first is
/// accepted; the second and fourth read a field (in a branch that never runs, and through a
/// reference), which `mentions_field` and the generated `mentions-field` find; the third reads
/// no field and fails its check on a role the tenant does not define.
fn any_object_policy(kind: u8) -> PolicyDef {
    let status = || {
        eq(
            var(Scope::Record, "status"),
            Expr::Lit(Value::Str("x".into())),
        )
    };
    let (id, condition) = match kind {
        0 => (
            "any_own",
            eq(var(Scope::Record, "$created_by"), Expr::Principal),
        ),
        1 => (
            "any_branch",
            Expr::If(
                Box::new(Expr::Lit(Value::Bool(true))),
                Box::new(Expr::Lit(Value::Bool(true))),
                Box::new(status()),
            ),
        ),
        2 => ("any_ghost", Expr::HasRole(role("nobody_has_this"))),
        _ => (
            "any_through",
            Expr::Not(Box::new(Expr::IsNull(Box::new(Expr::Through(
                Scope::Record,
                "author".into(),
                "name".into(),
            ))))),
        ),
    };
    PolicyDef {
        id: PolicyId::new(id).unwrap(),
        role: role("writer"),
        resource: Resource::AnyObject,
        actions: BTreeSet::from([Action::Delete]),
        condition: Some(condition),
    }
}

/// A writer may update a note whose author record has a name: a policy that follows a
/// reference (`record.author.name`), which the generated `require` decides through
/// `authorize-needing` with the snapshot's lookup (WG-5132, WG-5133). It applies to Update only;
/// the reads that follow a reference are compared by `compare_following_reads`. Its id sorts before
/// `writers_edit_own`, so it is tried first.
fn writers_edit_attributed() -> PolicyDef {
    PolicyDef {
        id: PolicyId::new("writers_edit_attributed").unwrap(),
        role: role("writer"),
        resource: Resource::Object(object("note")),
        actions: BTreeSet::from([Action::Update]),
        condition: Some(Expr::Not(Box::new(Expr::IsNull(Box::new(Expr::Through(
            Scope::Record,
            "author".into(),
            "name".into(),
        )))))),
    }
}

/// The policy condition of a transition that follows a reference, compared on an update of a
/// note: the snapshot with `writers_edit_attributed` added, once complete and once covering only
/// what the intent plans to load (`needs`), which leaves out the person the note refers to. On
/// the second the kernel answers `InsufficientState` naming that person when no other policy
/// grants, and the generated `require` must answer the same.
fn compare_attributed(state: &State, intent: &Intent, context: &Context, tally: &mut Tally) {
    let (
        Some(tenant),
        Intent::UpdateRecord {
            object: o, fields, ..
        },
    ) = (&state.tenant, intent)
    else {
        return;
    };
    let note = object("note");
    if o != &note
        || !tenant
            .schema
            .objects
            .get(&note)
            .is_some_and(|def| def.fields.contains_key(&field("author")))
    {
        return;
    }
    // Difference 16: the generated lookup leaves out a field whose old and new targets differ in
    // coverage, so the comparison holds for an update that keeps every reference.
    let holds_reference = |value: &Value| match value {
        Value::Ref(_) => true,
        Value::List(items) => items.iter().any(|item| matches!(item, Value::Ref(_))),
        _ => false,
    };
    assert!(
        !fields.values().any(holds_reference),
        "an update of the corpus changes a reference; the generated lookup does not follow the covered one of two targets (difference 16)"
    );
    let mut schema = TenantSchema::clone(&tenant.schema);
    let policy = writers_edit_attributed();
    schema.policies.insert(policy.id.clone(), policy);
    // A workflow guard that follows a reference when the guard it had does not hold (WG-5096):
    // `<guard> or not(is_null(old.author.name))`.
    for workflow in schema.workflows.values_mut() {
        if workflow.object != note {
            continue;
        }
        for step in &mut workflow.transitions {
            if let Some(guard) = step.guard.take() {
                step.guard = Some(Expr::Or(vec![
                    guard,
                    Expr::Not(Box::new(Expr::IsNull(Box::new(Expr::Through(
                        Scope::Old,
                        "author".into(),
                        "name".into(),
                    ))))),
                ]));
            }
        }
    }
    let mut attributed = TenantState::clone(tenant);
    attributed.schema = std::sync::Arc::new(schema);
    let planned: BTreeSet<Need> = needs(Some(attributed.schema.as_ref()), intent)
        .into_iter()
        .collect();
    let complete = State {
        tenant_id: state.tenant_id.clone(),
        tenant: Some(std::sync::Arc::new(attributed)),
        coverage: Coverage::Complete,
    };
    compare_transition(&complete, intent, context, tally);
    let partial = State {
        coverage: Coverage::Partial(planned),
        ..complete
    };
    compare_transition(&partial, intent, context, tally);
    if matches!(
        transition(&partial, intent, context),
        Err(e) if e.kind == ErrorKind::InsufficientState
    ) {
        tally.attributed_insufficient += 1;
    }
    tally.attributed += 1;
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
        "{label}: the generated functions do not decide a tenant operation (difference 10)"
    );
    assert!(
        !matches!(intent, Intent::ExpireLedger { .. }),
        "{label}: the generated functions do not decide an expiry (difference 11)"
    );
    assert!(
        !matches!(intent, Intent::EraseSubject { .. }),
        "{label}: the generated functions do not decide an erasure (difference 14)"
    );
    assert!(
        !matches!(intent, Intent::ExpireRecords { .. }),
        "{label}: the generated functions do not decide a retention deletion (difference 14)"
    );
    let mut hand = transition(state, intent, context);
    if matches!(intent, Intent::PostLedger { .. }) {
        let before_the_account = match &hand {
            Ok(_) => false,
            Err(error) => {
                error.kind != ErrorKind::InsufficientState
                    && error.evidence.last().is_none_or(|check| {
                        !check.rule.starts_with("ledger:")
                            || LEDGER_PRECHECK_RULES.contains(&check.rule.as_str())
                    })
            }
        };
        assert!(
            before_the_account,
            "{label}: a posting decided after its account is read is not mirrored (difference 11); the kernel gave {hand:?}"
        );
    }
    assert_no_scan_warning(state, intent, &hand);
    drop_evolution_evidence(&label, &mut hand);
    let mut generated = match spec::kernel_transition(state, intent, context) {
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
    // The generated needs can fault (checked arithmetic, plan-intent-needs); the hand-written
    // planner has no such outcome, so a fault is a difference.
    let generated_needs = spec::needs(&schema, intent)
        .unwrap_or_else(|fault| panic!("{label}: the generated needs faulted: {fault:?}"));
    assert_eq!(
        needs(schema.as_ref(), intent),
        generated_needs,
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
    let mut generated = match spec::kernel_read(state, request, context) {
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

/// The tenant with three rules that follow a reference (WG-5096): a Read policy of the public role
/// on notes, `not(is_null(record.author.name))`; the same as the filter of a query
/// `attributed_notes`; and a read condition on a note's title, `has_role(owner) or` the same. None
/// when the tenant's notes have no author field.
fn following_reads(tenant: &TenantState) -> Option<TenantState> {
    let note = object("note");
    let named = || {
        Expr::Not(Box::new(Expr::IsNull(Box::new(Expr::Through(
            Scope::Record,
            "author".into(),
            "name".into(),
        )))))
    };
    let mut schema = TenantSchema::clone(&tenant.schema);
    let def = schema.objects.get_mut(&note)?;
    if !def.fields.contains_key(&field("author")) {
        return None;
    }
    if let Some(title) = def.fields.get_mut(&field("title")) {
        title.read = Some(Expr::Or(vec![Expr::HasRole(role("owner")), named()]));
    }
    let policy = PolicyDef {
        id: PolicyId::new("public_reads_attributed").unwrap(),
        role: role("public"),
        resource: Resource::Object(note.clone()),
        actions: BTreeSet::from([Action::Read]),
        condition: Some(named()),
    };
    schema.policies.insert(policy.id.clone(), policy);
    let query = QueryDef {
        id: QueryId::new("attributed_notes").unwrap(),
        object: note,
        filter: Some(named()),
        params: BTreeSet::new(),
        order: vec![],
        limit: None,
        aggregates: None,
        declared_fields: None,
    };
    schema.queries.insert(query.id.clone(), query);
    let mut out = TenantState::clone(tenant);
    out.schema = std::sync::Arc::new(schema);
    Some(out)
}

/// Reads under rules that follow a reference (`following_reads`; WG-5096), as every kind of
/// principal of every tenant: the scanned query `attributed_notes` and the notes by id, over the
/// complete snapshot and over one that covers only what the read plans to load (`read_needs`),
/// which leaves out the people the notes refer to. On the second the kernel answers
/// `InsufficientState` naming a person a rule needs, and the generated `kernel-read` must answer
/// the same.
fn compare_following_reads(world: &World, now: i64, tally: &mut Tally) {
    let requests = [
        ReadRequest::Named {
            query: QueryId::new("attributed_notes").unwrap(),
            params: BTreeMap::new(),
            page_size: None,
            cursor: None,
        },
        ReadRequest::Records {
            object: object("note"),
            ids: (1..9).map(RecordId).collect(),
        },
    ];
    for tenant in WORLD_TENANTS {
        let state = world.state(&tenant_id(tenant));
        let Some(following) = state.tenant.as_deref().and_then(following_reads) else {
            continue;
        };
        let complete = State {
            tenant_id: state.tenant_id.clone(),
            tenant: Some(std::sync::Arc::new(following)),
            coverage: Coverage::Complete,
        };
        for actor_tenant in 0..WORLD_TENANTS.len() {
            for actor in 0..8 {
                let context = ctx(tenant, principal_of(actor_tenant, actor), now);
                for request in &requests {
                    compare_read(&complete, request, &context, tally);
                    let planned: BTreeSet<Need> = complete
                        .tenant
                        .as_deref()
                        .map(|t| read_needs(&t.schema, request))
                        .unwrap_or_default()
                        .into_iter()
                        .collect();
                    let partial = State {
                        tenant_id: complete.tenant_id.clone(),
                        tenant: complete.tenant.clone(),
                        coverage: Coverage::Partial(planned),
                    };
                    compare_read(&partial, request, &context, tally);
                    if matches!(
                        read(&partial, request, &context),
                        Err(e) if e.kind == ErrorKind::InsufficientState
                    ) {
                        tally.following_insufficient += 1;
                    }
                    tally.following += 1;
                }
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
                    domain_error(Failure::Error(failure)).unwrap_or_else(|fault| {
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
                compare_attributed(&state, &intent, &context, &mut tally);
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
                compare_following_reads(&world, now, &mut tally);
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
    // A policy that follows a reference is decided on complete and partial snapshots, and the
    // partial ones reach the InsufficientState that names the record it needs.
    assert!(
        tally.attributed > 50,
        "only {} updates under a policy that follows a reference",
        tally.attributed
    );
    assert!(
        tally.attributed_insufficient > 0,
        "no update under a policy that follows a reference asked for the record it needs"
    );
    // The reads under rules that follow a reference are compared on complete and partial
    // snapshots, and the partial ones reach the InsufficientState that names the record needed.
    assert!(
        tally.following > 100,
        "only {} reads under rules that follow a reference",
        tally.following
    );
    assert!(
        tally.following_insufficient > 0,
        "no read under a rule that follows a reference asked for the record it needs"
    );
    assert!(
        tally.settings > 500,
        "only {} setting resolutions",
        tally.settings
    );
    eprintln!(
        "compared {} decisions ({} accepted, {} rejected), {} reads, {} visibility checks, {} plans; \
         {} policy-condition details matched under difference 1; {} updates under a policy that \
         follows a reference, {} of them InsufficientState on a partial snapshot; {} reads under \
         rules that follow a reference, {} of them InsufficientState",
        tally.compared,
        tally.accepted,
        tally.rejected,
        tally.reads,
        tally.visibility,
        tally.needs,
        tally.evaluation_failed_details,
        tally.attributed,
        tally.attributed_insufficient,
        tally.following,
        tally.following_insufficient
    );
}

/// The calendar periods and the limits of an account (`ledger::period_start`, `ledger::limits_of`)
/// against the generated `ledger-period-start` and `ledger-limits-of`, on fixed inputs: the
/// posting that uses them is not mirrored (difference 11), these two are.
#[test]
fn the_ledger_periods_and_limits_agree_with_the_generated_functions() {
    let mut instants: Vec<i64> = vec![
        0,
        5_000,
        3_599_999,
        3_600_000,
        86_399_999,
        86_400_000,
        1_000_000_000_000,
        1_735_689_599_999,
        1_735_689_600_000,
        1_709_164_800_000,
        -1,
        -3_600_001,
        -86_400_001,
        -604_800_000,
        -2_678_400_000,
    ];
    instants.extend((-40..40).map(|i| i * 123_456_789_123));
    for period in [
        LedgerPeriod::Hour,
        LedgerPeriod::Day,
        LedgerPeriod::Week,
        LedgerPeriod::Month,
    ] {
        for at in &instants {
            let hand = strata_kernel::ledger::period_start(period, Timestamp(*at)).0;
            let generated = spec::ledger_period_start(&period, *at)
                .unwrap_or_else(|fault| panic!("ledger-period-start faulted: {fault}"));
            assert_eq!(
                hand, generated,
                "the {period:?} that holds {at} starts differently"
            );
        }
    }
    let day = Some((10, LedgerPeriod::Day));
    let week = Some((3, LedgerPeriod::Week));
    let unit = ledger_limit("credits", None, None, (Some(0), Some(100)), day);
    let account = ledger_limit(
        "credits",
        Some("alpha:editor"),
        Some("promo"),
        (None, Some(5)),
        None,
    );
    let account_floor = ledger_limit(
        "credits",
        Some("alpha:editor"),
        None,
        (Some(-3), None),
        week,
    );
    let account_period = ledger_limit("credits", Some("alpha:editor"), None, (None, None), week);
    let mut half_period = ledger_limit("credits", Some("alpha:editor"), None, (None, None), None);
    half_period.period = Some(LedgerPeriod::Week);
    let other_unit = ledger_limit("points", None, None, (Some(1), None), None);
    let sets: Vec<Vec<LedgerLimit>> = vec![
        vec![],
        vec![unit.clone()],
        vec![unit.clone(), account.clone()],
        vec![account.clone()],
        vec![unit.clone(), account_floor.clone(), account.clone()],
        vec![unit.clone(), account_period],
        vec![unit.clone(), half_period],
        vec![other_unit, unit],
    ];
    for limits in sets {
        let mut schema = TenantSchema::genesis();
        if !limits.is_empty() {
            schema.ledger = Some(LedgerDeclarations {
                units: BTreeMap::from([(
                    LedgerUnit::new("credits").unwrap(),
                    LedgerUnitDecl {
                        expiry: None,
                        unit: LedgerUnit::new("credits").unwrap(),
                        description: "Store credit".into(),
                    },
                )]),
                pools: BTreeMap::new(),
                limits: limits.clone(),
            });
        }
        for owner in ["alpha:editor", "alpha:owner"] {
            for scope in ["", "promo", "other"] {
                let owner = LedgerOwner::Principal {
                    principal: principal_id(owner),
                };
                let credits = LedgerUnit::new("credits").unwrap();
                // The kernel keeps account limits outside the schema (WG-5172) and is handed the
                // account's own limit; the spec function still finds it among the schema's limits.
                let of_account = limits.iter().find(|l| {
                    l.unit == credits
                        && l.owner.as_ref() == Some(&owner)
                        && l.scope.as_ref().map_or("", |s| s.0.as_str()) == scope
                });
                let hand = strata_kernel::ledger::limits_of(&schema, of_account, &credits);
                let generated =
                    spec::ledger_limits_of(&schema, &owner, &credits, &scope.to_string());
                assert_eq!(hand.floor, generated.floor.map(|amount| amount.0));
                assert_eq!(hand.cap, generated.cap.map(|amount| amount.0));
                assert_eq!(
                    hand.period_cap,
                    generated.period_cap.map(|cap| (cap.cap, cap.period)),
                    "the limits of {owner:?} in {scope:?} differ under {limits:?}"
                );
            }
        }
    }
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
    let mut other = match spec::kernel_transition(&state, &intent, &context) {
        Ok(result) => result,
        Err(_) => panic!("the generated kernel refused what the hand kernel accepted"),
    };
    assert_eq!(hand, other);
    other.evidence.pop();
    assert_ne!(hand, other, "dropping a check must make the results differ");
}
