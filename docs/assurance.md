# Assurance and generation

How Strata is specified, generated, implemented and checked, and why it changes. This document
states direction and principles; the work itself is tracked in Workgraph under WG-5372 and its
children, not here.

## What was measured

Measured on the specification and toolchain of 2026-11-21:

- **The function layer is a second kernel, transcribed from the first.** 283 of the 325
  specification functions name, in their description, the Rust file they mirror; 129 examples cite
  Rust constants; two functions (`error-kind-debug`, `error-kind-snake`) specify how Rust prints an
  error name. The bodies total 160,000 characters (`apply-change` alone 25,630).
- **The examples restate the code.** 1,706 examples, written to give every branch of a function
  body an example (Exegra's readiness gap `ready.function-branch-uncovered`). Generated as Rust
  tests they were 195,000 of the 212,158 lines of `crates/model-core/src/functions.rs`, and their
  binary overflowed its stack before reporting a result.
- **Generated code dominates the build.** Compiling the generated model crates is the peak of the
  workspace build: single `rustc` processes of 2 to 3.4 GB, on a 16 GB machine shared with other
  builds.
- **Most requirements are held to nothing.** 58 of the 1,715 requirements belong to a started
  milestone and are held to the code, each by a hand-written `Verifies:` line that nothing checks
  against what the test asserts. Generation reads a requirement's id, ref, kind and method; its
  statement only for budget requirements and five named ones.
- **Checks failed silently.** The requirement lint read no requirement for some time (an 8 MiB
  file cap); 415 wording findings and a broken document layout accumulated unnoticed.

What holds: types, records, contracts and errors generate the boundary code, the conformance suite
and the deployment files, and the gate's audit checks reconcile contracts with handlers, records
with tables, environment variables and routes. Exegra's 507 model claims are checked with injected
faults. These stay.

## Principles

1. **A specification is smaller and more abstract than the code it constrains.** A model as large
   as its implementation is a second implementation with the same blind spots. The reference model
   of the kernel states what must hold, not how the kernel computes it.
2. **Every element has a consumer that can fail.** A specification entry that no check reads is
   documentation and is kept apart from what the gate counts.
3. **Tests come from the specification, never from reading the implementation.** The agent that
   changes the specification, the agent that implements it and the source of a test's expected
   values are kept apart. An expected value comes from the requirement, a worked case or a known
   edge, not from running the code.
4. **Tests are judged by whether they fail when the code is wrong.** Mutation testing is the
   measure of a test suite; branch coverage of examples is not.
5. **Generate the boundary, write the rest.** `specc` generates types, serialisation, routes,
   contract stubs, conformance tests and deployment files. Behaviour is written (by people or
   agents) inside those boundaries and checked against the specification.
6. **A finding becomes a rule.** A review finding is fixed and also becomes a mechanical check that
   refuses its recurrence, with a negative fixture.

## Target architecture

### The kernel: one reference model, checked against the implementation

The kernel is pure (no clock, no I/O, deterministic) and everything depends on it. It is held to a
single reference model, the Lean model in `lean/`, by three checks:

- **Proofs** about the model (`lean/Strata/Proofs.lean`, extended as the model grows).
- **Differential testing**: the Rust kernel and the Lean model as a program (`lean/Main.lean`,
  driven by `crates/verification/src/lean.rs`) run on the same generated operation sequences, and
  any difference in outcome fails. This is the pattern Cedar uses between its Lean model and its
  Rust implementation.
- **Mutation testing** of the kernel: a mutant that no test, property or differential run kills is
  a gap.

The Lean model grows to cover the kernel's decision surface (constraints, references, uniqueness,
policies with conditions, workflows, queries, events), feature by feature, each feature landing with
its differential coverage. It stays abstract: it states outcomes, not algorithms.

The specification functions stop being a reference model. Once the differential test against Lean
covers what `generated_differential.rs` covers today, the transcribed functions are retired; the
small number of functions that are genuinely specification (pure rules a contract or a tenant
expression relies on) are kept, rewritten from their requirement, with a few chosen examples.

### The specification

- **Structural model** (types, records, contracts, errors, routes, capabilities): kept, generating
  the boundary.
- **Behaviour models** (Exegra's behaviours, faults, scenarios and model claims): kept.
- **Requirements**: each names the mechanical check that holds it (a contract and its conformance
  test, a property, a differential, a model claim, a budget measurement) or is demoted to
  documentation. Examples are few and chosen for meaning.
- **Documentation** (decisions, rationale, white paper): kept for people, outside the gate's counts.

### `specc`

Generates the boundary only. It stops generating per-example tests, and stops compiling functions
that are no longer specification. Its lint enforces the authoring rules above as they become
mechanical.

### The simulator

The simulator (`crates/simulation`) is kept and made as accurate as it can be: it runs exactly what
the deployed system runs, and nothing it reports is a property of the simulator rather than of
Strata. Concretely:

- it drives the same shell path the backend drives (store contract, jobs, outbox, automation,
  effects), not only the kernel;
- its fault injection covers the failure modes the specification lists, as they happen in a
  deployment (a lost or repeated message, a stopped process, a store conflict, a replication lag);
- its traces are checked against production: the journal replay (`journal.rs`) must reproduce
  committed state hashes, and a simulated run of a recorded workload must agree with the recorded
  outcome;
- a resource model, when added, is calibrated from measurements on a real deployment and consumes
  traces without feeding back into behaviour (as `lib.rs` already describes).

### Implementation by agents

Agents implement behaviour inside the generated boundary, one module per task, with testable
acceptance criteria, landing through Workgraph stacks with tests run once per stack. The roles in
principle 3 are separate agents. Architecture rules (crate dependencies, purity) are mechanical
checks, so drift is refused rather than reviewed.

## Order of work

By dependency:

1. **Kernel reference model** (Lean as the single reference) and **kernel checking**
   (differential testing against Lean at scale, mutation testing) proceed together.
2. **Authoring rules and requirements** (principles 2 to 4 applied to the requirement and function
   layers, piloted on one area first) and **narrowing `specc`** (no per-example tests; boundary
   generation only) proceed together, alongside 1.
3. **Simulator accuracy** proceeds alongside both.
4. **Retiring the transcribed functions** needs 1 (the Lean differential covers what the generated
   reference covered) and 2.
5. **Agents implementing against the boundary** needs 1 and 2.
