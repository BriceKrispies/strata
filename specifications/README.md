# The Strata specification

`spec.jsonc` is the specification. It is one file and it is the authority for what the platform
is to do. Two tools read it:

- **Exegra** validates it, draws it, runs its behaviour models, and evaluates the claims it can
  decide from the model alone (ids starting `m-`): structure, the data rules for records and
  copies, and behaviour under injected faults.
- **Concordance** renders its documents, lints every requirement, and reconciles the
  requirements with the code and with recorded test results.

No Markdown copy of the specification is kept. Concordance renders the documents under the
paths the file names (`specifications/00-vision/whitepaper.md`,
`specifications/03-identity/identity.md` and so on), which is why findings cite those paths.

## What the file holds

| Section | Holds |
|---|---|
| `goals`, `nonGoals`, `boundaries`, `decisions`, `constraints`, `openQuestions` | The white paper: G-n, NG-n, B-n, D-n, K-n, Q-n |
| `releaseStages`, `milestones` | The roadmap, in dependency order |
| `components`, `relationships` | The architecture (C-n) |
| `records`, `copies` | The data model: who owns each record, and every copy kept of it, with its key, freshness bound and erasure point |
| `claims` with `"kind": "requirement"` | The requirements, one SHALL each, with boundary, traces, method and milestone |
| `claims` with ids `m-...` | Claims Exegra evaluates itself. `models` names the requirement each one is a model of |
| `events`, `behaviors`, `faults`, `scenarios` | The behaviour models |
| `environments`, `failureModes`, `faultModels`, `assumptions` | What the design is meant to survive, and what it takes for granted |
| `documents`, `sections`, `caseTables`, `terms` | How the above is laid out for reading |

## Checking it

```sh
npm run spec                 # run the tests, then every check
npm run spec -- --no-test    # reuse the recorded test results
```

The gate runs `cargo test` and `lake build` and records their output under `var/test-results`,
then asks Exegra to validate the file and evaluate its model claims, then asks Concordance to
lint the requirements and reconcile them with the code. It fails when any of those fails, or
when a requirement of a started milestone has no proof with a passing result.

Exegra finds this file through the linked project `specs/strata/project.jsonc` in the Exegra
checkout (`EXEGRA_HOME`, default `../exegra`):

```jsonc
{ "spec": "../../../strata/specifications/spec.jsonc" }
```

## How a requirement gets verified

A requirement is verified by a test or theorem that names it, with a recorded passing result
newer than the sources:

```rust
/// Verifies: KRN-003
#[test]
fn outsiders_cannot_mutate() { /* ... */ }
```

```lean
/-- An accepted step leaves every other tenant as it was.

    Verifies: KRN-013 -/
theorem step_isolated /- ... -/
```

`scripts/tag-proofs.mjs` adds such lines from a list of pairs. A requirement whose milestone is
not in `[gate] started_milestones` of `concordance.toml` is reported as planned. Starting a
milestone means adding its number there, after which the gate holds the code to it.

## Changing it

Change the specification first, then the code. Edit `spec.jsonc` through Exegra (`apply`), or
by hand and then run the gate. The rules the lint enforces are described in Concordance's
documentation of its specification lint. The ones that come up most:

- one SHALL per requirement, stated at a boundary, with no implementation detail
- "for every", "never" and "always" need a property, a proof or a conformance table
- a number with a unit needs the method `budget` and a row in the budget measurement table
- every goal and every constraint is traced by at least one requirement
- a behaviour claim declares a run that violates it, or Exegra reports it as inconclusive
