# Strata invariants

Each invariant names where it is enforced and what checks it.

## Kernel

| # | Invariant | Enforced by | Checked by |
|---|---|---|---|
| K1 | **Purity.** `transition`, `read`, `needs`, `apply_delta` depend only on their arguments | No I/O-capable imports; `forbid(unsafe_code)` | `verification/tests/purity.rs` source scan |
| K2 | **Determinism.** Equal `(state, intent, context)` give equal results, byte for byte in canonical form | Ordered maps only, no clock, no randomness, counters for identifiers | Kernel property tests; simulation trace hashes; Lean `step_deterministic` |
| K3 | **Tenant scope.** A transition only produces a delta for `ctx.tenant`, and only from a snapshot of that tenant | `State` holds one tenant; `transition` rejects a mismatched snapshot | Kernel property test over a multi-tenant `World`; Lean `step_isolated` |
| K4 | **Default deny.** A mutation or read is allowed only by the `owner` role or by a matching policy whose condition evaluates to `true` | `authz::authorize`; evaluation errors and `null` deny | Property tests; Lean `unauthorized_cannot_mutate` |
| K5 | **Record conformance.** Every accepted create or update leaves the record conforming to the schema: known fields, types, required, constraints, uniqueness, references, workflow state | `validate::validate_record` on the whole candidate | State-machine property test with `verification::invariants`; Lean `step_preserves_wf` |
| K6 | **Migration honesty.** A schema change under `Reject` is accepted only if every existing record conforms to the new schema. Under `Grandfather` it is accepted and the non-conforming records are listed in the evidence | `transition::change_schema` | Kernel tests; Impact Lab scenario |
| K7 | **Coverage soundness.** A transition is never accepted on a snapshot that lacks a record the decision depends on | `needs` recomputed inside `transition` | Kernel tests with deliberately thin snapshots |
| K8 | **Monotonic counters.** Tenant revision rises by exactly one per accepted mutating transition; event sequence numbers are gapless per tenant; record ids are never reused | Delta construction | Invariant checker, storage conformance |
| K9 | **Explanations.** Every acceptance and rejection carries the list of checks that were evaluated | `Check` evidence in `Transition` and `DomainError` | Kernel tests |

## Storage

| # | Invariant | Checked by |
|---|---|---|
| S1 | A commit applies all of delta, events, journal entry and counters, or none | Conformance: atomicity |
| S2 | A commit whose base revision is not the stored revision fails with `Conflict` and changes nothing | Conformance: conflict |
| S3 | Reads and writes name a tenant; a record of one tenant is never returned for another | Conformance: isolation |
| S4 | Events are durable in commit order and stay in the outbox until acknowledged | Conformance: outbox |
| S5 | A snapshot restores to a state with the same canonical hash | Conformance: snapshot |
| S6 | Committed data survives closing and reopening the store | SQLite recovery test |

## Runtime

| # | Invariant | Checked by |
|---|---|---|
| R1 | The browser reaches only the BFF; the backend listens on loopback and requires the service token | Backend integration tests, e2e network assertions |
| R2 | Tenant and principal come from the server-side session and the BFF's site configuration, never from client input | Backend tests, e2e cross-tenant test |
| R3 | No event is published before its transaction commits | Outbox design; backend test |
| R4 | A subscriber receives an event only when the kernel grants read access to the record | Backend visibility test, e2e draft-leak test |
| R5 | A reconnecting client either receives every missed visible event or is told to resynchronise | e2e reconnect test |
| R6 | Passwords, session tokens and CSRF tokens never appear in logs or URLs | Log scan test |

## Simulation

| # | Invariant | Checked by |
|---|---|---|
| X1 | Equal scenario inputs give equal traces and equal trace hashes | Simulation tests |
| X2 | A branch with no change is indistinguishable from its baseline | Simulation tests |
| X3 | Simulation never writes to the live store | Lab runs operate on a loaded snapshot; e2e asserts live revision unchanged |
