# Verification: what is claimed, and on what evidence

Every claim Strata makes about its own behaviour carries one of five labels. There are no
confidence scores.

| Label | Means | Does not mean |
|---|---|---|
| **Proven** | Machine-checked in Lean 4 for the abstract model, under the stated assumptions | That the Rust kernel has the property |
| **Reproduced** | The production kernel was run on stated inputs and did this; running it again gives the same hash | Anything about other inputs |
| **Sound approximation** | Everything that can happen is included; some things that cannot may be | That each listed thing will happen |
| **Estimated** | Counted over a generated or recorded sample | A forecast |
| **Unknown** | Not modelled | Safe |

## The Lean model

`lean/Strata/Model.lean` defines a small model: tenants as a function from ids, one object type
per tenant with typed fields that may be required, records with an owner, an owner, editors and
writers, and four intents: create, update, delete, and make a field required (strictly or not).
`step` is a total, computable function. It has no constraints, workflows, references, uniqueness,
queries, events or policy conditions: the assumptions below list each omission. The theorems are
about this model. The Rust kernel is compared with it on generated scenarios, which is evidence and
not a proof.

The specification's `Verifies:` lines on these theorems count them as passing when `lake build`
builds their module, without reading the statements (`specifications/README.md`).

`lean/Strata/Proofs.lean` proves:

| Property | Theorems | Statement |
|---|---|---|
| Accepted transitions preserve the invariant | `step_preserves_wf`, `stepTenant_preserves_wf` | If every record conforms to its schema and has an id below the counter, the same holds after any accepted strict step |
| | `lax_change_can_break_wf` | The exclusion of the non-strict schema change is necessary: there is a well-formed state it breaks |
| An unauthorized principal cannot mutate | `step_ok_authorized`, `unauthorized_cannot_mutate`, `unauthorized_cannot_update`, `unauthorized_cannot_delete` | A step is accepted only if its principal is authorized for it; otherwise no new state exists |
| | `missing_record_hidden_on_update`, `missing_record_hidden_on_delete` | The permission is asked before the record is looked up: one who could edit no record it did not create is refused a missing record with `unauthorized`, as for an existing one, and cannot probe which ids exist |
| Tenant isolation | `step_isolated`, `step_local` | An accepted step leaves every other tenant unchanged, and its result for its own tenant does not depend on any other tenant |
| Determinism | `step_deterministic` | Equal arguments give equal results |

`lake build` checks them. `lake env lean Audit.lean` prints the axioms each depends on: only
Lean's standard `propext`, `Classical.choice` and `Quot.sound`. There is no `sorry`, no `admit`
and no added axiom; a test fails if the sources contain one or the audit reports `sorryAx`.

### Assumptions

1. The object of every theorem is the model, not the Rust source.
2. The model has no constraints, workflows, references, uniqueness, queries or events.
3. Its authorization is owner, editors, and writers on their own records. The kernel's policies
   are data with arbitrary conditions.
4. Its update replaces a record's fields; the kernel's is a patch.
5. `step_deterministic` is true of any Lean function. The substance of determinism is on the Rust
   side, where it is tested, not proved.
6. Lean's kernel and its three standard axioms are trusted.

## Correspondence between the model and the kernel

| Lean | Rust kernel |
|---|---|
| `World := TenantId → Option Tenant` | `World { tenants: BTreeMap<TenantId, Arc<TenantState>> }` |
| `Tenant.schema` | one `ObjectTypeDef` of `TenantSchema.objects` |
| `FieldDef { ty, required }` | `FieldDef { ty, required, .. }` |
| `Val` | `Value` restricted to `Null`, `Int`, `Str`, `Bool` |
| `Record { owner, fields }` | `Record { created_by, fields, .. }` |
| `Tenant.owner`, `editors`, `writers` | members holding `owner`; a role with an unconditional policy; a role with a policy conditioned on `record.$created_by == principal` |
| `Ctx { tenant, principal }` | `Context { tenant, principal, .. }` |
| `Intent.create / update / delete` | `Intent::CreateRecord / UpdateRecord / DeleteRecord` |
| `Intent.setRequired f strict` | `Intent::ChangeSchema` with `Reject` or `Grandfather` |
| `step` | `World::step`, that is `apply_delta ∘ transition` |
| `conforms` | `rules::validate_record`, restricted to type and required checks |
| `Tenant.WF` | the `records_conform` and `records_placed` invariants of `verification::invariants` |
| `Err.invalid` | `TypeMismatch`, `MissingRequired`, `UnknownField` |

The two are different programs, so the correspondence is exercised rather than assumed.
`strata_verification::lean::scenarios` generates 60 scenarios (over a thousand operations: valid
and invalid writes, wrong types, missing records, strangers, members of the other tenant, strict
and non-strict schema changes). The Lean executable (`lake exe strata_model`) and the Rust kernel
each run them and print every outcome and the final state of both tenants. The outputs must be
identical, line for line. The scenario file and the expected output are committed, so the Rust
side runs everywhere; the Lean side runs wherever `lake` is installed.

This is evidence that the kernel behaves like the model on the modelled fragment. It is not a
proof, and a passing run says nothing about the parts of the kernel the model leaves out.

## What covers the rest

| Kernel behaviour | Evidence |
|---|---|
| Constraints, uniqueness, references, workflow states preserved | Property test over random operation sequences, checking every invariant after every accepted step |
| Determinism and canonical hashing | Property test; simulation trace hashes; journal replay |
| Purity | Source scan of kernel and substrate for I/O, clocks, randomness, interior mutability, `unsafe`, unordered maps; dependency allow-list |
| Isolation beyond the kernel | Storage conformance; backend and BFF integration tests; browser tests on two tenants |
| Partial snapshots never change a decision | Property test comparing complete and planned snapshots |
| Event visibility | Unit tests of `subscribe::notice`; backend socket test; browser tests |

## Simulation

The simulator runs `World::step`. A projection runs a schedule twice, once as it is and once with
a proposed schema change injected, and compares the executions structurally. Its report labels:

- **Reproduced**: which operations end differently and why (the failed checks), which records and
  query results differ, which invariants hold, that other tenants are identical in both runs, and
  the smallest schedule found that still shows the first divergence.
- **Sound approximation**: existing records that do not conform to the proposed schema, from the
  kernel's own migration planning. Each will be refused on its next write unless that write fixes
  it; records never written again are never affected.
- **Estimated**: how many of the scheduled operations are accepted in each run.
- **Unknown**: everything in the list below.

Replaying the journal of committed transitions from nothing reproduces every recorded delta and
event hash and arrives at the live state. That shows the kernel is deterministic on real inputs
and that live data is what the journalled decisions produce. It does not re-enact rejected
requests (they are not journalled), concurrent arrival order beyond commit order, sessions, or
event delivery.

### Not modelled

- Wall-clock behaviour: CPU, database latency, contention, network delay.
- Concurrency: operations run one at a time in schedule order.
- The shell: sessions, cookies, CSRF, the WebSocket relay, the browser.
- External effects beyond delivered, dropped or retried.
- Operations nobody generated or recorded.
- Parameterised queries, other than through the views that bind their arguments.

A calibrated resource model would be a second pass over finished traces (see the note in
`crates/simulation/src/lib.rs`). None exists, and no performance claim is made.
