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

## The schema evolution model

`lean/Strata/Evolution/` is a second model, independent of the one above (its own namespace,
`Strata.Evolution`). It models what happens to the things that depend on a schema when the schema
changes.

- **Entities have stable ids, separate from names.** Object types and their fields, queries,
  policies and views are all referred to by id. A rename changes a label nobody resolves through,
  so no dependent needs a different read, and a name that is deleted and reused cannot make a
  dependent point at a different entity (no ABA).
- **A dependent is a read list and a function of the values read.** A query reading fields and a
  policy whose condition reads fields (the far end of one reference is one more cell) are readers;
  a view reads queries. A read is a cell, or a combination of two reads (which is what a rewrite
  leaves behind for a split field). Dependencies are stratified, so they are acyclic by
  construction. What the function does is not constrained.
- **Data** is a context of one record per object type, a map from field ids to values. A move
  along a reference reads the context as a record of the source object with the record of the
  target object it references.
- **Changes**: add a field, widen, rename, remove a field, narrow, retarget a reference, remove an
  object type, remove a dependent, move a field along a reference, split a field totally (one field
  becomes two whose combination gives the original), re-encode a field (with an inverse).
  `footprint` is the existing cells a change touches, `impact` the dependents that read them
  (through views too), `apply` the new schema, `migrate` the new data, `rewrite` the dependent as
  rewritten for the change, `project` the read-only view of new data in the old shape.
- **Classes** follow the specification: compatible (add a field, widen), adaptable (rename, move a
  field, total split) and breaking (everything else). See the reconciliation below.
- **Acceptance** (`accepted`): the local checks pass (targets exist, new ids are fresh, new
  references resolve, the type moves the right way, no reference is left to a deleted object) and,
  if the change is breaking, every dependent it reaches is pinned (`Schema.pins`) and the change
  has a left inverse (`hasLeftInverse`: the old shape can be rebuilt from the new data). With no
  pins that says the impact is empty. It does not say "the result is well-formed"; that is what
  closure proves.
- **Footprints for concurrency** (`ents`): the cells and field lists a change touches, labels
  included (a rename touches its field), together with its impact set. Two changes are disjoint
  (`Disjoint`) when these share nothing.

### Reconciliation of the change classes with the specification

The first version of the model called adding a field adaptable and a rename compatible. The
specification (D-8212) says the opposite: adding a field is compatible, a rename is adaptable.
The model now follows the specification (option (a)). Because ids are stable, a rename's rewrite
of a dependent is the identity, and the theorem of rewrite correctness holds for it trivially;
adding a field changes no existing cell, so it needs no rewrite and joins the compatible class.
The earlier theorems stay true. `compatibility` gains the hypothesis that the change passes its
local checks (`pre`) and that the dependents of the schema are valid, which adding a field needs
(a fresh id, and nothing read the new cell before); for a widening it is not used.

### Theorems

`lean/Strata/Evolution/` proves (each file's docstrings carry the requirement it verifies):

| Property | Theorem | Statement |
|---|---|---|
| Closure | `closure` | A well-formed schema stays well-formed under any accepted change, including a pinned breaking change and a move or a split |
| Locality | `locality` | For an accepted change on a well-formed schema, every dependent outside the change's impact (and not deleted by it) is still present, still valid, and evaluates to the same value on the migrated data (MIG-605) |
| Compatibility | `compatibility` | A compatible change (add a field, widen) keeps every valid dependent valid and its value unchanged on the migrated data (MIG-590) |
| The classification is necessary | `breaking_change_can_change_value`, `breaking_change_can_invalidate` | There is a well-formed schema, a breaking change refused by the acceptance rule, and a dependent in its impact, such that applying the change anyway changes the dependent's value (narrowing clamps a stored `-1`) or leaves it reading a field that no longer exists |
| 4. Rewrite correctness | `rewrite_correct` (`rewrite_correct_nb` for every non-breaking change) | For an accepted adaptable change on a well-formed schema and a dependent of it, the rewritten dependent is in the new schema, is valid, and `evalDep (apply c s) (migrate c data) (rewrite c d) = evalDep s data d` (MIG-591) |
| 5. Lens laws | `project_migrate`, `left_inverse_on_footprint`, `pinned_eval`, `pinned_eval_live`, `removeField_irreversible` (and `retargetRef_`, `removeObject_`, `narrow_nat_irreversible`) | For a change with a left inverse, `project c (migrate c data)` agrees with `data` on every cell of the schema (MIG-597), so a pinned dependent reading the projected data evaluates to what it did; removing a field has no left inverse on its cell, whatever function is tried (the irreversibility MIG-557 relies on). `pinned_breaking_change_accepted` shows the pinned route is not vacuous |
| 6. Commutation | `commutation` | For two accepted changes with disjoint footprints, `apply c₂ (apply c₁ s) = apply c₁ (apply c₂ s)` and `migrate c₂ (migrate c₁ data) = migrate c₁ (migrate c₂ data)`, exactly (MIG-602) |
| 7. Composition | `rewrite_compose`, `rewrite_chain` | A dependent rewritten by `c₁` then `c₂` (accepted, not breaking) is valid in the final schema and evaluates on `migrate c₂ (migrate c₁ data)` to what it evaluated to on `data`; the same over a chain of any length (MIG-606). `rewriteReader_compose` says the composed rewrite is one substitution of the reads |

`lake env lean Audit.lean` prints the axioms each depends on: only `propext`, `Classical.choice`
and `Quot.sound`.

### Assumptions and what is left out

1. The object of the theorems is this model, not the Rust source. The correspondence with the
   kernel is pending (WG-5330).
2. Dependents are read lists plus an arbitrary function. Expressions beyond their reads, and
   reads along more than one reference, are not modelled. A rewrite substitutes reads and never
   changes a dependent's function; a view is never rewritten.
3. Locality needs a well-formed starting schema; compatibility, rewrite correctness, the lens
   evaluation theorem and composition need the dependents of the schema to be valid. The two
   counterexamples do not.
4. A move along a reference is modelled inside one context (a record of the source with the record
   it references), copying the value; whether many sources agree is not modelled. A split is total
   by hypothesis (`jn (sp v) = v` is part of its local checks).
5. A pin is an id in `Schema.pins`. A pinned dependent stays valid because the only breaking
   changes that have a left inverse (a re-encoding) lose no cell. The end of a pin (the dependent
   migrates, or the deprecation window closes) and the projection of a chain of breaking changes
   are not modelled. A narrowing is not given a projection (it clamps); a projection on the data
   that happens to be lossless is not modelled.
6. Commutation is stated for changes accepted on the same base schema with disjoint `ents`. The
   equality is exact (schemas as lists, dependents as rewritten structures) and needs only each
   change's local checks. That each change is still accepted on the schema the other produced is
   what the revision guard checks; it is not proved here, and the rebase of overlapping changes
   is not modelled. The granularity is the cell, plus the object type for a change that edits its
   field list or deletes it, so two additions to one object are not disjoint.
7. Data conformance to field types is not modelled; `migrate` says what a changed cell becomes
   (defaulted, clamped, cleared, copied, re-encoded). Widening is the identity on values.
8. `classify` looks at the change alone, not at the schema.

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
