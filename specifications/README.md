# The Strata specification

`spec.jsonc` is the specification. It is one file and it is the authority for what the platform
is to do. Two tools read it:

- **Exegra** validates it, draws it, runs its behaviour models, and evaluates the claims it can
  decide from the model alone (ids starting `m-`): structure, the data rules for records and
  copies, and behaviour under injected faults. Its `apply` command edits the file through a
  validated transaction.
- **Concordance** renders its documents, lints every requirement, and reconciles the
  requirements with the code and with recorded test results.

No Markdown copy of the specification is kept. Concordance renders the documents under the
paths the file names (`specifications/00-vision/whitepaper.md`,
`specifications/03-identity/identity.md` and so on), which is why findings cite those paths.

The specification describes the whole intended platform. The prototype in `crates/` and `lean/`
answers milestone 1 only. Milestone numbers are in `milestones`; which milestones the gate holds
the code to is in `[gate] started_milestones` of `concordance.toml`.

## What the file holds

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
asks Concordance to lint the requirements and scan them. It passes only when every check passes.

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
2. Every requirement of a started milestone has a passing proof in Concordance's results file,
   and no Concordance finding on a started milestone is open. Concordance runs with
   `--fail-on unverified`, so a started requirement with no accepted proof fails the scan itself. The count of such requirements is
   taken from the specification. Currently that is 37 of the 738 requirements.
3. Every `m-` claim is verified by Exegra, with no violated claim. The count is taken from the
   specification, and every `models` link names a requirement that exists.
4. Exegra validates the file with no errors, warnings, gaps or Concordance warnings.
5. Concordance lints the requirements under the strict rules, and no finding anywhere is violated.
6. The number of requirements of started milestones and the number of `m-` claims have not gone
   down since the last successful gate run, unless `--accept-fewer` is given.

A passing gate does not establish these:

- **The planned requirements.** Requirements of milestones that are not started are reported as
  planned and are not checked against the code. They cannot fail the gate. At present that is
  701 of 738 requirements. The gate holds the code to 37 of 738 (5.0%).
- **That a proof tests the requirement.** A `Verifies:` line is written by hand, and nothing checks
  that the test asserts the statement it is tagged with. A passing test that carries the line is
  evidence that the test passes, not that the
  statement holds.
- **Lean theorems as proofs of the Rust kernel.** Concordance counts a theorem as passing when the
  build log shows its own module built without `sorry`. It reports a theorem that states `True` or
  rests on a declared axiom, and otherwise does not read the theorem. The theorems are about the
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
  tests, the gate runs `concordance record`, which binds each results file to the content of every
  indexed source file and to the text of every requirement. A later change to a source file makes
  every result stale, and a change to a requirement's text makes that requirement stale. What stays
  trusted is that the record was written straight after the run; the gate checks that no source
  changed while the tests ran.
- **Proofs that inspect source text.** A Rust proof that calls no indexed code is refused unless
  its doc comment says `Inspects: <path>`. KRN-012 is proved this way. Nothing checks that the test
  really reads that path.
- **Anything the specification says that no requirement covers**, and the correctness of Exegra
  and Concordance themselves.

## How a requirement gets verified

A requirement is verified when a Rust test or a Lean theorem carries a doc-comment line
`Verifies: <ID>` naming it, and the recorded result of that test is a pass. For Rust, the result
comes from `cargo test`. For Lean, Concordance reads the output of `lake build`: a theorem passes
when its module built.

The line goes in the doc comment of the test or theorem, for example a Rust `#[test]` function
with `/// Verifies: KRN-003` above it, or a Lean theorem whose doc comment contains
`Verifies: KRN-013`. Read the test before writing the line; the line is a claim that the test
checks the requirement.

A requirement whose milestone is not in `[gate] started_milestones` of `concordance.toml` is
reported as planned. Starting a milestone means adding its number there. From then on the gate
requires every requirement of that milestone to have a passing proof, and fails on any that does
not.

## Changing it

Change the specification first, then the code. Edit `spec.jsonc` through Exegra (`apply`), or by
hand, and then run the gate. The rules the lint enforces are described in Concordance's
documentation of its specification lint. The ones that come up most:

- one SHALL per requirement, stated at a boundary, with no implementation detail
- "for every", "never" and "always" need a property, a proof or a conformance table
- a number with a unit needs the method `budget` and a row in the budget measurement table
- every goal and every constraint is traced by at least one requirement
- a behaviour claim declares a run that violates it, or Exegra reports it as inconclusive
