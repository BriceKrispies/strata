# The Strata specification

`spec.jsonc` is the specification. It is a manifest: it lists the part files beside it, and the parts
together are the authority for what the platform is to do (see "Parts of the specification");
products are specified beside it (see "Product specifications"). Exegra, specc, Concordance and the
gate's own checks read it, each through the manifest:

- **Exegra** validates it, draws it, runs its behaviour models, and evaluates the claims it can
  decide from the model alone (ids starting `m-`): structure, the data rules for records and
  copies, and behaviour under injected faults. Its `apply` command edits the parts through a
  validated transaction, each changed entity in the part that holds it.
- **The gate's evidence check** (`scripts/gate-evidence.mjs`) reconciles the requirements with the
  code and with recorded test results: see "How a requirement gets verified".

No Markdown copy of the specification is kept. The documents the file names
(the white paper, the roadmap, the glossary and so on) are sections of this file, not files.

The specification describes the whole intended platform. The prototype in `crates/` and `lean/`
answers milestone 1 only. Milestone numbers are in `milestones`; which milestones the gate holds
the code to is in `startedMilestones` of `gate.json`.

## Product specifications

A product feature (badges, profiles, messaging and so on) is specified in a file of its own,
never merged into `spec.jsonc`: d-103 and ARC-113 make a product module tenant metadata, with no
change to the kernel. Product files go in `specifications/products/`. Each starts with a header
that names the platform file and the platform entities it uses:

```jsonc
{
  "product": {
    "id": "badges", "name": "Badges",
    "imports": { "spec": "../spec.jsonc", "entities": ["content-read", "content-write", "product-module"] }
  },
  "types": [ ... ], "roles": [ ... ]
}
```

A product may declare `records`, `types`, `screens`, `forms`, `actions`, `liveBindings`,
`errorMessages`, `behaviors`, `events`, `contracts`, `roles`, `scenarios`, `uiScenarios`,
`claims` and `evidence`, and nothing that redefines a platform entity. The interface sections
(`screens`, `forms`, `actions`, `liveBindings`, `errorMessages`) become the `ui` section of the
product's bundle (D-8250): a screen shows the product view of its route, and a form or action
submits to the product operation its `contract` names. specc checks every file
in `specifications/products/` when `npm run generate` and the gate run (`--product`), refuses a
bad one with an `error[layer.*]` line saying what to change, and generates nothing from it, so
the generated files and their semantic hash stay the platform's. Exegra reads a product file
when the project's `project.jsonc` lists it (`"products": ["<path>/specifications/products"]`),
validates it with the platform and reports the platform's hash unchanged. The rules, and an
accepted and a refused fixture of each, are in `crates/spec-compiler/src/layers.rs` and
`crates/spec-compiler/tests/fixtures/products`.

## Parts of the specification

`spec.jsonc` holds one member, `"specParts"`, the part files in order, and an optional `"specSections"`
table saying which part each section belongs to (Exegra's `apply` uses it to place a new section or
entity; the other readers ignore it). Every top-level section of the specification is in exactly one
part, except `claims`, which is in two:

| Part | Sections | Read by |
|---|---|---|
| `intent.jsonc` | `system`, `exegra`, `vocabulary`, `goals`, `nonGoals`, `boundaries`, `decisions`, `constraints`, `openQuestions`, `assumptions`, `releaseStages`, `milestones`, `terms`, `documents`, `sections`, `views`, `environments` | people, and Concordance rendering the documents |
| `boundary.jsonc` | `types`, `records`, `copies`, `contracts`, `capabilities`, `roles`, `quotas`, `retentions`, `plans`, `grants`, `delegations`, `tokenScopes`, `featureFlags`, `validations`, `languages`, `components`, `relationships`, `screens`, `forms`, `actions`, `liveBindings`, `errorMessages` | specc, which generates from them |
| `behaviour.jsonc` | `events`, `behaviors`, `faults`, `scenarios`, `uiScenarios`, `failureModes`, `faultModels`, and the `claims` that are not requirements (`m-...`) | Exegra, which runs them |
| `requirements.jsonc` | the `claims` with `"kind": "requirement"`, `caseTables` | the gate's evidence check |
| `functions.jsonc` | `functions`, `exampleFixtures` | specc (the transcribed function layer) |

Resolving the manifest gives the single document one file would hold, and every tool does it by the
same rule:

- array sections are concatenated in manifest order (`claims`: the `m-` claims, then the requirements);
- a section that is not an array (`system`, `vocabulary`, `exegra`) may be in one part only;
- an entity id that two parts give one section is refused, naming both files, as are a missing part,
  a part that is itself a manifest, and a manifest with sections of its own;
- the assembled text is canonical (one member per line, one array item per line, each item's text
  unchanged), so every tool produces the same bytes and the specification's text hash is the same
  in all of them.

The order of the entities within a section changes nothing the tools compute: the semantic hash is
the same as for the one-file specification, and specc's output is byte-identical. It does change the
order in which `claims` are listed (`exegra evaluate`).

`node scripts/spec-parts.mjs` is the reader for scripts (`readSpecValue`, `specSha256`);
`strata-spec-parts` is the Rust one (specc and the tests that read the specification). Do not read
`spec.jsonc` as a JSON document directly.

`node scripts/split-spec.mjs` splits a one-file `spec.jsonc` into the manifest and parts, the same way
each time (a second run changes nothing). Run it again on a one-file `spec.jsonc` that another branch has
edited; `node scripts/split-spec.mjs --check <monolithic file>` confirms the parts reassemble to it.
A section that is in no part makes it stop: add the section to `SECTION_PART` in the script and to the
table above.

## What the specification holds

| Section | Holds |
|---|---|
| `system`, `exegra`, `vocabulary` | The system's name and draft version, the Exegra format version, and the verification methods Exegra does not run itself |
| `goals`, `nonGoals`, `boundaries`, `decisions`, `constraints`, `openQuestions` | The white paper: G-n, NG-n, B-n, D-n, K-n, Q-n |
| `releaseStages`, `milestones` | The roadmap, in dependency order |
| `components`, `relationships` | The architecture (C-n): one tenant database per tenant, an identity service, a gateway, and so on |
| `records`, `copies` | The data model: who owns each record, and every copy kept of it, with its key, freshness bound and erasure point |
| `claims` with `"kind": "requirement"` | The requirements, one SHALL each, with boundary, traces, method and milestone |
| `claims` with ids `m-...` | Claims Exegra evaluates itself. `models` names the requirement each one is a model of (see below); a behaviour claim also carries `faultBasis` (see "Behaviour models") |
| `events`, `behaviors`, `faults`, `scenarios` | The behaviour models |
| `environments`, `failureModes`, `faultModels`, `assumptions` | What the design is meant to survive, and what it takes for granted |
| `views` | Six views for reading: architecture, trust zones, data, traceability, requirements and claims, behaviour |
| `documents`, `sections`, `caseTables`, `terms` | How the above is laid out for reading |

## Behaviour models

A behaviour claim is an `m-` claim that lists scenarios (`state-invariant`, `precedes`, `eventually`, `never`). Exegra runs each scenario with its faults and runs each declared negative: the same scenario with extra faults, which must violate the claim.

### Fault basis of a behaviour claim

Every behaviour claim carries the property `faultBasis` (a property of the specification's vocabulary, because `claim` has no `tags`):

- `fault`: a listed scenario injects a real fault (a message lost, delayed or repeated, a stop, a concurrent actor, an early request), the claim holds in that scenario, and a negative on that scenario takes away the protection and violates the claim. In most of them the fault is what exposes the defect: without the late approval, the defect that applies an unapproved change has nothing to apply. In a few the defect would show with or without the fault (the trigger chain, the audit chain, the self-approval): the claim is then only shown to hold under the fault, and the fault is not what the protection is for.
- `restatement`: the claim fails only under a defect built to negate it, usually a transition disabled or a check removed, and no real fault is modelled for it. These are liveness claims ("the operation commits", "the move completes") and their scenarios show the mechanism works, not that it survives a failure.

`node scripts/spec-gate.mjs` prints the two counts separately. The ids of the restatements are the `m-` claims whose `faultBasis` is `restatement`.

A failure mode (`failureModes`) carries `modelling`: `exercised` when a fault of a scenario realises it (`realises`) and a model claim lists that scenario, or `not-modelled` with a `modellingNote` that says what answers it instead. Exegra's result for a claim states how many failure modes of its fault model its scenarios inject.

### Ticks

A tick is logical time. For every bound that a requirement or a decision states in minutes, one tick is one minute: the emailed link lifetime of 15 minutes (D-9) is 15 ticks, and the access token lifetime of 5 minutes (D-537), which the revocation bound of the gateway's session copy follows, is 5 ticks. A gap of a few ticks between two messages fixes an order and is not a duration. The relation is stated once as the assumption `a-tick` and the term `term-tick`; the model claims that depend on a bound in ticks name `a-tick`. Where a model scales a number down (a threshold of 3 failures for the 100 of D-700) the claim says so.

### Structure claims over paths

The relationships are paths between components, not requests. A claim that every path from a client to the gateway passes through the traffic switch (`m-external-requests-pass-traffic-switch`) shows that no relationship lets a client bypass the switch. Which colour a request reaches is the behaviour of the colour switch model, and which listener takes a request class is a relationship per listener (the traffic switch for public requests, the agent traffic switch for agents). The relationship kind `controls` is an instruction that carries no request, record or tenant data, and the claims that follow data paths name the kinds they follow.

## Checking it

```sh
npm run spec                                           # the full gate: tests, then every check
node scripts/spec-gate.mjs --no-test --allow-recorded  # reuse the recorded test results
```

The full gate runs `cargo test --workspace`, `lake build`, the web kernel tests (`node --test`)
and `npm run check` (type check). It records their output under `var/test-results`, together with
a gate record there that holds the specification's hash and a content hash of every source file
as they were before the run. It then asks Exegra to validate the file and evaluate its claims, and
checks every requirement of a started milestone for a passing proof (`scripts/gate-evidence.mjs`).
It passes only when every check passes.

`--no-test` reuses the recorded results and says so on screen. Without `--allow-recorded` it
fails. With it, it still fails when the specification or any source file differs from the hashes
in the gate record, or when the recorded run did not pass. The browser tests (`npm run e2e`)
are not part of the gate.

Exegra finds this file through the linked project `specs/strata/project.jsonc` in the Exegra
checkout (`EXEGRA_HOME`, default `../exegra`). If that checkout is missing, the gate fails.

```jsonc
{ "spec": "../../../strata/specifications/spec.jsonc" }
```

### What the gate establishes, and what it does not

A passing gate establishes these, and only these:

1. The tests ran and passed: `cargo test --workspace` reports no failed test and at least one
   passing test; `lake build` completed with no `sorry`; the web kernel tests and the type check
   pass. Each tool's result is read from its own output or exit status. A tool that cannot start,
   crashes, or prints nothing parseable fails the gate.
2. Every requirement of a started milestone has a passing proof (see "How a requirement gets
   verified"), and no `Verifies:` line names a ref the specification does not have. The count of
   such requirements is taken from the specification. Currently that is 37 of the 739 requirements.
3. Every `m-` claim is verified by Exegra, with no violated claim. The count is taken from the
   specification, and every `models` link names a requirement that exists.
4. Exegra validates the file with no errors, warnings or gaps.
5. The number of requirements of started milestones and the number of `m-` claims have not gone
   down since the last successful gate run, unless `--accept-fewer` is given.

A passing gate does not establish these:

- **The planned requirements.** Requirements of milestones that are not started are reported as
  planned and are not checked against the code. They cannot fail the gate. At present that is
  702 of 739 requirements. The gate holds the code to 37 of 739 (5.0%).
- **That a proof tests the requirement.** A `Verifies:` line is written by hand, and nothing checks
  that the test asserts the statement it is tagged with. A passing test that carries the line is
  evidence that the test passes, not that the
  statement holds.
- **Lean theorems as proofs of the Rust kernel.** The evidence check counts a theorem as passing when the
  build log shows its own module built without `sorry`. It reports a theorem that states `True` and
  does not count it; it does not otherwise read the theorem or the axioms it uses (`lean/Audit.lean` lists them). The theorems are about the
  abstract model in `lean/Strata/`, not about the Rust code (see `docs/verification.md`).
- **The model claims as statements about the code.** Exegra evaluates each `m-` claim on the
  scenarios listed in the specification, under their faults. Exegra reads the `models` field: it
  must name a requirement, and the requirement lists the model claims that name it with their
  statuses. That is a link, not a proof: nothing checks that the model claim says what the
  requirement says.
- **Milestone honesty.** Nothing checks that a requirement's milestone is right. A requirement can
  be planned for a later milestone while the prototype already contradicts it, and the gate does
  not see the contradiction.
- **That the recorded results came from this code, beyond the gate's own run.** Right after the
  tests, the gate writes a gate record that holds the hash of the specification and of every source
  file, and of each result file. With `--no-test --allow-recorded` the results are refused when the
  specification or any source file, `gate.json` included, differs from the record, so a change to a
  requirement or to a source makes every recorded result stale. What stays trusted is that the record
  was written straight after the run; the gate checks that no source changed while the tests ran.
- **Proofs that call no code.** A test that asserts nothing about the code is a passing test all the
  same; nothing reads the test body. KRN-012 is proved by a test that reads source text, and nothing
  checks that it reads the right path.
- **Test names across binaries.** The recorded `cargo test` text names each test by its path and not by
  its binary, except where the headers it ends with match its `running` blocks one for one; when they
  do not, a result is matched to a test by name alone and the worst outcome of that name counts.
- **Anything the specification says that no requirement covers**, and the correctness of Exegra
  and of the gate's own scripts.

## How a requirement gets verified

A requirement is verified when a Rust test or a Lean theorem carries a doc-comment line
`Verifies: <ID>` naming it, and the recorded result of that test is a pass. For Rust, the result
comes from `cargo test`. For Lean, the check reads the output of `lake build`: a theorem passes
when its module built and the log shows no `sorry` or error inside the theorem. A requirement
realised by `construction` is verified instead by the Exegra verdicts of the `m-` claims whose
`models` names it: all of them `verified`. The gate writes those verdicts to
`var/exegra-verdicts.json`.

The line goes in the doc comment of the test or theorem (for a Rust test a plain `//` comment line
directly above it counts too), for example a Rust `#[test]` function
with `/// Verifies: KRN-003` above it, or a Lean theorem whose doc comment contains
`Verifies: KRN-013`. Read the test before writing the line; the line is a claim that the test
checks the requirement.

A `Verifies:` line that names a ref the specification does not define (a requirement's or a
contract's) fails the gate; one that names a contract's ref is accepted and proves nothing.

A requirement whose milestone is not in `startedMilestones` of `gate.json` is
reported as planned. Starting a milestone means adding its number there. From then on the gate
requires every requirement of that milestone to have a passing proof, and fails on any that does
not.

## What holds a requirement

A requirement of a started milestone names the mechanical check that holds it, or is documentation
(docs/assurance.md, principles 2 to 4). The declaration is two properties of the claim, declared in
the vocabulary of this specification so that Exegra accepts them:

```jsonc
{"id":"krn-1283","kind":"requirement", ... "evaluate":{"method":"example"},
 "check":"test",
 "checkedBy":["crates/kernel/tests/limits.rs::a_schema_change_of_200_changes_is_accepted_and_one_of_201_is_refused"]}
```

| `check` | `checkedBy` names | What the gate reads |
|---|---|---|
| `test` | Rust tests, each `<path from the repository root>::<test function>` | Each must exist and carry a `Verifies:` line for this requirement (`scripts/gate-evidence.mjs`); the recorded result of each must be a pass |
| `conformance` | a contract ref (`CT-004`) whose conformance suite holds it | The contract must exist |
| `differential` | the tests that run the kernel and the Lean model on the same operations, named like tests | As `test` |
| `model-claim` | `m-` claims whose `models` is this requirement | Each must exist, model this requirement, and be verified by Exegra |
| `budget` | the ref of its row in the budget measurement table | `lint.budget-row` |
| `documentation` | nothing | See below |

A check is only as good as the numbers its tests use. A test states the values of the requirement
(65536, 200, 10000) as literals, taken from its wording, and never reads them from the code it
checks: a test that reads `limits::MAX_READ_IDS` moves with a changed limit and passes unseen. For
a limit, a pair of cases at the boundary (the most that is accepted, and one more that is refused)
holds it from both sides; `crates/kernel/tests/limits.rs` is the pilot.

**Documentation.** A statement that no check can hold (a rationale, a stated direction, a claim
about people) is marked `"check": "documentation"`, with no `checkedBy`. It stays in the file for
readers, and it is outside the gate's counts: `scripts/spec-gate.mjs` does not count it among the
requirements, the evidence check (`scripts/gate-evidence.mjs`) does not require a proof of it, and
the realisation check (`scripts/gate-realisation.mjs`) does not label it. Marking a requirement
documentation lowers the requirement count, which the gate refuses without `--accept-fewer`.

**The lint.** `specc lint` enforces the declaration (`crates/spec-compiler/src/lint.rs`):

- `lint.no-check`: a requirement of a started milestone (`startedMilestones` of `gate.json`, which
  the lint reads from beside the specification, or from `--gate <file>`) declares no `check`. Only
  started milestones are held to it, so the requirements of milestones that are not started are not
  refused today. When a milestone is started, or an area is rewritten, its requirements are brought
  under the rule: each area of the specification is rewritten to the pilot's approach (state the
  requirement at its boundary, name the check, write the check from the requirement) as its own task.
- `lint.check-declaration`: a `check` that is not in the table, a `checkedBy` that does not fit
  its check or is given without one, or a `documentation` statement that names a check. This rule
  applies to every requirement.

Currently the requirements KRN-1280 to KRN-1288 (the kernel's limits and paging) declare their
checks; no started requirement is documentation yet.

## Changing it

Change the specification first, then the code. Edit the parts through Exegra (`apply`), or by
hand, and then run the gate. The rules for wording a requirement that come up most:

- one SHALL per requirement, stated at a boundary, with no implementation detail
- every requirement of a started milestone names the check that holds it, or is documentation (see "What holds a requirement")
- "for every", "never" and "always" need a property, a proof or a conformance table
- a performance bound (a percentile, a time an operation takes in seconds or less, a rate) needs the method `budget` and a row in the budget measurement table; an enforced limit, a retention period or an expiry is shown by an example at the bound
- every goal and every constraint is traced by at least one requirement
- a behaviour claim declares a run that violates it, or Exegra reports it as inconclusive
