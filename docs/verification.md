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
per tenant with typed fields that may be required, records with an owner and a version, an owner,
editors and writers, and four intents: create (filling constant defaults), update (a patch),
delete (both naming an expected version or not), and make a field required (strictly or not).
Fields may be references to records of the same type, which a delete either refuses to break or
clears (slice 1, below), and lists of scalars or of references, whose elements a delete removes
(slice 2). Written values and records are held to the kernel's size limits, as an abstract bound
(slice 2). `step` is a total, computable function. Slice 3 adds unique fields and composite unique constraints, checked on a write and when a
constraint is defined over existing data. Slice 4 adds several object types per tenant: a write
may name any of them, a reference may name a record of another type, and an intent naming a type
the tenant lacks is refused. It has no constraints,
workflows, queries, events or policy conditions: the assumptions below list each
omission, and the gap analysis further down says which slice will add each. The theorems are
about this model. The Rust kernel is compared with it on generated scenarios, which is evidence and
not a proof.

The specification's `Verifies:` lines on these theorems count them as passing when `lake build`
builds their module, without reading the statements (`specifications/README.md`).

`lean/Strata/Proofs.lean`, `lean/Strata/References.lean` and `lean/Strata/Writes.lean` prove:

| Property | Theorems | Statement |
|---|---|---|
| Accepted transitions preserve the invariant | `step_preserves_wf`, `stepTenant_preserves_wf` | If every record conforms to its schema, has an id below the counter, refers only to existing records, and no two records agree on a unique field or composite constraint, the same holds after any accepted strict step |
| | `lax_change_can_break_wf` | The exclusion of the non-strict schema change is necessary: there is a well-formed state it breaks |
| An unauthorized principal cannot mutate | `step_ok_authorized`, `unauthorized_cannot_mutate`, `unauthorized_cannot_update`, `unauthorized_cannot_delete` | A step is accepted only if its principal is authorized for it; otherwise no new state exists |
| | `missing_record_hidden_on_update`, `missing_record_hidden_on_delete` | The permission is asked before the record is looked up: one who could edit no record it did not create is refused a missing record with `unauthorized`, as for an existing one, and cannot probe which ids exist |
| Tenant isolation | `step_isolated`, `step_local` | An accepted step leaves every other tenant unchanged, and its result for its own tenant does not depend on any other tenant |
| Determinism | `step_deterministic` | Equal arguments give equal results |
| Referential integrity (slices 1, 2) | `step_preserves_refs`, `stepTenant_preserves_refs` | If every reference held by a record, single or as a list element, names an existing record, the same holds after any accepted step, strict or not: it needs no conformance and no exclusion of the non-strict schema change |
| | `problem_none` | A record the write checks find nothing wrong with conforms to the schema (lists typed elementwise) and names only existing records |
| A delete does not break a reference it may not break (slices 1, 2) | `delete_refused_while_referenced` | A record that another record names through a restricting field (single or list) or a required single reference is refused deletion with `stillReferenced`, to one who may delete it and names no other version |
| | `delete_leaves_no_reference` | After an accepted delete the record is gone and no record names it, through a field or a list element, whatever the state was |
| | `conforms_afterDelete` | The clearing a delete does keeps a conforming record conforming (a required single reference is never cleared, because it blocks; a required list keeps its field, possibly empty) |
| | `required_list_never_blocks` | A reference held in a list forbids a delete only through a restricting definition, never because the list is required |
| Updates are patches (slice 2) | `update_writes_patched`, `update_keeps_unnamed`, `patch_keeps_unnamed`, `patch_last_wins` | The record an accepted update writes is the stored one patched; a field the patch does not name keeps its values; the value named last for a field is its value afterwards, `null` removing it |
| Versions (slice 2) | `update_raises_version`, `delete_raises_referrer_version` | An accepted update raises the record's version by exactly one; an accepted delete rewrites each other record whose reference it clears, at its version plus one |
| | `version_conflict_on_update`, `version_conflict_on_delete` | One who may act on a record and names a version it is not at is refused with `versionConflict`, whatever the patch, and no state follows; the version is asked after the permission (`unauthorized_cannot_update`, `unauthorized_cannot_delete` hold for every expected version) |
| Defaults (slice 2) | `create_fills_default` | An accepted create writes, at version 1 under the next id, the constant default of every field of the schema the intent does not name |
| Size limits (slice 2) | `create_refused_over_record_bound` | A record over the record bound is refused with `limitExceeded` before its fields are checked |
| | `create_within_limits`, `update_within_limits`, `problem_none_within` | Every record an accepted create or update writes is within the record bound and holds no value over the string or list bound |
| Uniqueness (slice 3) | `stepTenant_preserves_unique`, `step_preserves_unique` | An accepted strict step keeps every unique field and every composite constraint unique: no two distinct records agree on one (agreeing needs a present, non-list value in each field) |
| Several object types (slice 4) | `unknown_type_refused`, `unknown_type_owner`, `unknown_type_other`, `unknown_type_schema_refused`, `unknown_type_hidden` | A record intent naming an object type the tenant does not have is refused with `unknownObject` for the owner and `unauthorized` for anyone else; a schema change naming one is refused with `invalidSchema` for the owner and `unauthorized` for anyone else; a caller who may neither create nor edit gets `unauthorized` whether or not the type exists |
| References across types (slice 4) | `create_refs_resolve`, `update_refs_resolve`, `write_refused_dangling_across_types`, `update_refused_dangling_across_types`, `stepTenant_preserves_refs` | A write whose reference names no record of the type its field names is never accepted, for a create or an update; an accepted step keeps every reference of every type resolving |
| Deletes across types (slice 4) | `delete_refused_while_referenced`, `delete_leaves_no_reference` | Both quantify over every referring type: a record of any type naming the deleted one through a restricting or required field refuses the delete; after an accepted delete no record of any type names it |
| | `stepTenant_preserves_distinct` | An accepted step keeps the schema's names distinct and each record's field names distinct, so a delete's clearing of references keeps them so |
| | `create_duplicate_refused`, `update_duplicate_refused` | A write that is otherwise valid and duplicates another record's unique value, or agrees with another record on a composite constraint, is refused with `uniqueViolated`, and no state follows |
| | `define_unique_refused_over_duplicates`, `set_unique_refused_over_duplicates` | A composite constraint that existing records already violate is refused with `uniqueViolated`; a strict unique field over records that agree on it is refused with `migrationRejected`; no state follows |
| | `lax_unique_change_can_break_wf` | The exclusion of the non-strict unique change is necessary: a lax `setUnique` over two records that agree leaves a tenant that is not well formed |
| | `strict_schema_change_conforms`, `schemaViolated_false` | An accepted strict schema change (`setRequired`, `setUnique`, `defineUnique`, and any strict change refused by the same check) leaves the whole tenant conforming: every record conforms to the resulting schema, and `UniqueRecords` holds for it, including records and unique values a lax change left behind |
| | `setUnique_records`, `defineUnique_records`, `stepTenant_conform_ids` | A schema change leaves the records as they are, and every accepted step keeps them conforming and their ids below the counter |

`lake build` checks them. `lake env lean Audit.lean` prints the axioms each depends on: only
Lean's standard `propext`, `Classical.choice` and `Quot.sound`. There is no `sorry`, no `admit`
and no added axiom; a test fails if the sources contain one or the audit reports `sorryAx`.

### Assumptions

1. The object of every theorem is the model, not the Rust source.
2. The model has no constraints, workflows, queries or events. Its unique fields and composite
   constraints are checked on written values only; the kernel's schema rules for them (a list
   field cannot be unique, a composite names two to four distinct fields of the object) are the
   model's `invalidSchema` refusals and nothing more. Its references are
   references, single or in lists, to records of the tenant's object types, the target of a field
   being the type it names (slice 4). Object types are fixed by the fixture; creating or removing
   one is not modelled (slice 11).
3. Its authorization is owner, editors, and writers on their own records. The kernel's policies
   are data with arbitrary conditions.
4. Its size limits are the kernel's numbers over an abstract measure, not over the kernel's
   encodings (slice 2's review note): a string's bytes and a list's length are exact, a record's
   and a unique value's encoded size are bounded below.
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
| `FieldDef { ty, required, unique, default }` | `FieldDef { ty, required, unique, default, .. }`, `default` a constant |
| `Ty.ref onDelete` | `FieldType::Ref { target, on_delete }` with `target` one of the tenant's object types |
| `Ty.list elem` | `FieldType::List(elem)`, `elem` a scalar or reference type |
| `Val` | `Value` restricted to `Null`, `Int`, `Str`, `Bool`, `Ref` and `List` of those (`Elem`) |
| `Record { owner, version, fields }` | `Record { created_by, version, fields, .. }` |
| `Limits`, `Limits.kernel` | the constants of `limits.rs` |
| `Tenant.uniques` | `ObjectTypeDef.unique`, the composite constraints of the object type (`UniqueDef`, by its fields) |
| `Tenant.owner`, `editors`, `writers` | members holding `owner`; a role with an unconditional policy; a role with a policy conditioned on `record.$created_by == principal` |
| `Ctx { tenant, principal }` | `Context { tenant, principal, .. }` |
| `Intent.create / update / delete` | `Intent::CreateRecord / UpdateRecord / DeleteRecord`, with `expected_version` |
| `Intent.setRequired f strict` | `Intent::ChangeSchema` with `Reject` or `Grandfather` |
| `Intent.setUnique f strict` | `Intent::ChangeSchema` with `DefineField` of the field with `unique` set and the rest of its definition kept, `Reject` or `Grandfather` |
| `Intent.defineUnique fs` | `Intent::ChangeSchema` with `DefineUnique` over the same fields |
| `step` | `World::step`, that is `apply_delta ∘ transition` |
| `problem` | `rules::validate_record`, restricted to the record bound, unknown-field, required, value bound, unique-value bound, type, unique-value clash and reference checks per field, then the composite constraints; the first failure names the refusal |
| `conforms` | the type and required checks of `rules::validate_record` |
| `blocks`, `clearRef` | the referrer scan of `transition::delete_record` |
| `Tenant.WF` | the `records_conform` and `records_placed` invariants of `verification::invariants`, and referential integrity |
| `Err.invalid` | `TypeMismatch`, `MissingRequired`, `UnknownField` |
| `Err.danglingReference`, `Err.stillReferenced` | `DanglingReference`, `StillReferenced` |
| `Err.versionConflict`, `Err.limitExceeded` | `VersionConflict` (from `check_version`, not from `apply_delta`), `LimitExceeded` |
| `Err.uniqueViolated`, `Err.invalidSchema` | `UniqueViolated`, `InvalidSchema` (the schema refusals of a change, as the model names them) |

The two are different programs, so the correspondence is exercised rather than assumed.
`strata_verification::lean::scenarios` generates 60 scenarios (over a thousand operations: valid
and invalid writes, wrong types, missing records, strangers, members of the other tenant, strict
and non-strict schema changes). The Lean executable (`lake exe strata_model`) and the Rust kernel
each run them and print every outcome and the final state of both tenants. The outputs must be
identical, line for line. The scenario file and the expected output are committed, so the Rust
side runs everywhere; the Lean side runs wherever `lake` is installed.

This is evidence that the kernel behaves like the model on the modelled fragment. It is not a
proof, and a passing run says nothing about the parts of the kernel the model leaves out.

The fixture tenants of `lean/Main.lean` declare ten fields, in id order: `0` an optional list of
references that restricts the delete of an element's target, `1` a required string, `2` an
optional integer, `3` an optional boolean, `4` an optional reference to the tenant's own records
that a delete clears (`set-null`), `5` an optional reference that restricts the delete
(`restrict`), `6` an optional integer whose default is the constant `7`, `7` an optional list of
strings, `8` an optional list of references whose elements a delete removes (`set-null`), and `10` an
optional string that is unique (slice 3).
Field `9` is undeclared. A reference value is written `r:<id>` in the scenario format; the
refusals of slice 1 print as `err dangling_reference` and `err still_referenced`, those of slice 2
as `err version_conflict` and `err limit_exceeded`.

Slice 2's additions to the scenario format:

- A list value is `l:` followed by element tokens separated by commas (`7=l:s:oak,s:ash`,
  `8=l:r:1,r:2`, the empty list `7=l:`); `l*<n>:<element>` is a list of `n` copies of one
  element (`7=l*1001:s:a`). An element is any scalar token or `n`, never a list, and contains no
  comma.
- `s*<n>:<text>` is the string of `n` copies of `<text>` (`1=s*65537:x`), for the size limits.
- `update <id> [v=<n>] <fields>` replaces the record, as before: the model reads it as the patch
  that names every fixture field, those the line leaves out as `null`, so the Rust side names
  fields `0` to `8`, absent ones as `null`. `patch <id> [v=<n>] <fields>` is a patch: only the
  named fields change, `n` removing one. `delete <id> [v=<n>]` deletes. `v=<n>`, only directly
  after the id, is the expected version.
- Slice 4's additions: `type <n>` between the principal and the intent names the object type of the
  intent: `1` is `item` (the default when the line has no `type`), `2` is `part`, and any other
  number names a type the tenant does not have. The part type has fields `1` (a reference to an item
  that restricts its delete), `2` (a reference to an item that clears), `3` (a list of references to
  items that clear) and `4` (an int). Items gain `11` (a reference to a part that restricts) and
  `12` (a reference to a part that clears). The owner's refusal of a missing type prints as
  `err unknown_object`, anyone else's as `err unauthorized`. The state prints each further type as
  `type <n> required=<fields>` followed by its records as `type <n> record <id> ..`.
- Slice 3's additions: `unique <f> strict|lax` makes field `f` unique (`strict` refuses a change
  over existing records that already agree on `f`, `lax` leaves them as they are), and
  `composite <f> <f> ..` defines a composite unique constraint over the named fields. The
  refusals print as `err unique_violated`, `err invalid_schema`, and `err migration_rejected`
  for a strict change over duplicate data.
- A record prints its version after its owner: `record 1 owner=102 v=2 1=s:oak 6=i:7`. A string of
  at least 100 characters that repeats one character prints as `s*<n>:<c>`, a list of at least
  100 equal elements as `l*<n>:<element>`; other lists as `l:` and their elements joined by
  commas.

## Gap analysis: the kernel's decisions against the model

This is the plan for making `lean/` the single reference model of the kernel (WG-5373). It
lists the decisions `transition` and `read` make, by feature: the refusals with their
`ErrorKind`, and what an accepted intent changes. Each is marked **covered** (the model decides
it and the differential run compares it), **partly** (covered on a restricted fragment, the
restriction named) or **not covered**, with the slice that will add it. The decisions were read
from `crates/kernel/src` at the commit that adds this section; the source is the authority for
check order, and the model follows it where the order changes which refusal is given.

### What is outside the model by design

These are not decisions about the tenant's data, or are decided elsewhere and checked by other
evidence. No slice adds them.

- **Snapshot coverage.** `InsufficientState` (every `Lookup`, and every policy, constraint, guard
  or rule that follows a reference into a record the snapshot lacks) asks the shell to load more
  and retry; it never decides an outcome. The model reads the whole tenant. Evidence: the
  property test that a planned snapshot never changes a decision.
- **Explanations.** The evidence list of checks, the messages, and which clashing record a
  uniqueness refusal names (it names one only to a reader who may see it). The model gives the
  refusal kind and the resulting state, not the explanation.
- **Commit mechanics.** `apply_delta`'s base-revision check (`VersionConflict` when the tenant
  moved since the snapshot) is concurrency between transitions; the model runs one step at a
  time. `step` is `apply_delta ∘ transition` on an unmoved base.
- **Query planning.** Window, index and scan plans, candidate bounds that only change which plan
  serves a read, and cursor encodings. A read's outcome (which rows, in which order, which page)
  is modelled in slice 10; how it is computed is not.
- **Schema evolution of dependents.** Pins, structural versions, projections and rewrites are the
  separate model in `lean/Strata/Evolution/`; slice 11 connects its acceptance rule to `step`.

### Every intent: the preamble

| Decision | Refusal | Model | Slice |
|---|---|---|---|
| Snapshot is of another tenant than the context names | `TenantMismatch` | not covered: the model addresses the tenant through the context, so no snapshot can disagree | outside |
| A caller, new owner, assigned member or added team member whose id starts with `team:`, or is reserved for an automation | `InvalidIntent` | not covered | 7 |
| `CreateTenant` when the tenant exists; by anyone but the system; with a blank name | `TenantExists`, `Unauthorized`, `InvalidIntent` | not covered | 7 |
| Accepted `CreateTenant`: the genesis schema, the named owner holding `owner`, record counter 1 | | not covered | 7 |
| Any other intent on a tenant that does not exist | `TenantNotFound` | covered (`noTenant`) | 0 |

### Records: `CreateRecord`, `UpdateRecord`, `DeleteRecord`

From `transition::create_record`, `update_record`, `delete_record` and `rules::validate_record`.

| Decision | Refusal | Model | Slice |
|---|---|---|---|
| The object type does not exist, for a record intent: the refusal is `UnknownObject` for the owner, who may act on every type, and `Unauthorized` for anyone else, so a caller who may not act on a type cannot tell whether it exists | `UnknownObject` / `Unauthorized` | covered (`unknown_type_refused`, `unknown_type_hidden`) | 4 |
| The object type does not exist, for a schema change: the kernel requires the schema permission first, then finds no type when it applies the change | `Unauthorized` for anyone but the owner, `InvalidSchema` for the owner | covered (`unknown_type_schema_refused`) | 4 |
| Update or delete of a record that does not exist; `Unauthorized` unless the caller could act on some record of the type | `UnknownRecord` / `Unauthorized` | covered (`missingRecord`) | 0 |
| The caller may not create, update (judged on the old and the new record) or delete | `Unauthorized` | partly: owner, editors, writers on their own records | 6 |
| `expected_version` given and not the stored version, checked after authorization | `VersionConflict` | covered (`versionClash`) | 2 |
| Update is a patch: named fields replaced, `null` removes, others kept; version + 1 | | covered (`patched`) | 2 |
| Create fills absent fields from their defaults; a default that fails to evaluate | `Evaluation` | partly: constant defaults (`withDefaults`, a field named as `null` gets none); expressions not | 2 (constant), 5 (expression) |
| A new record's workflow field is not an initial state | `WorkflowViolation` | not covered | 8 |
| An update that changes a workflow field needs a declared transition whose guard is true; its assignments apply; the update is authorized again if they changed the record | `WorkflowViolation`, `Evaluation`, `Unauthorized` | not covered | 8 |
| Validation; the first failure names the refusal, in this order: | | | |
| 1. the record is larger than `MAX_RECORD_BYTES` | `LimitExceeded` | partly: over an abstract size that bounds the encoding below (slice 2's note) | 2 |
| 2. a field the type does not declare | `UnknownField` | covered (`invalid`) | 0 |
| 3. per field in id order: required and absent; a value over the string or list limits; a unique value over `MAX_UNIQUE_BYTES`; the wrong type | `MissingRequired`, `LimitExceeded`, `TypeMismatch` | covered for required, type (lists elementwise) and the string and list limits; partly for the unique-value limit (abstract size, not the encoding; field `10` is unique in the fixtures) | 0, 2, 3 |
| 4. per field in id order, after its type: a unique value another record holds | `UniqueViolated` | covered (`valueProblem`, before the reference check of the same field) | 3 |
| 5. per field in id order, after its type: a reference to a record that does not exist | `DanglingReference` | covered for single references and list elements, across types too | 1, 2, 4 |
| 6. composite unique constraints whose fields all have values, held by another record | `UniqueViolated` | covered (`compositeProblem`, after every field) | 3 |
| 7. a workflow field holding a value that is not one of its states | `WorkflowViolation` | not covered | 8 |
| 8. only if nothing above failed: constraints, failing on `false`; `Evaluation` when one cannot be evaluated | `ConstraintViolated`, `Evaluation` | not covered | 5 |
| Accepted create: the record under the next id, `created_by` the caller (none for the system), counter + 1 | | covered (owner = caller) | 0 |
| Delete: another record of any type referring through a `restrict` field, or through a required single reference, blocks it; a record referring to itself does not | `StillReferenced` | covered for references and lists, across types too (a required list never blocks) | 1, 2, 4 |
| Accepted delete: the record removed; every other reference to it cleared (a list loses the element); each cleared record's version + 1 | | covered within the one type | 1, 2 |
| Events: `RecordCreated`, `RecordUpdated` (naming the transitions fired), `RecordDeleted`, an update event per cleared referrer; revision + 1; one `Publish` effect per event | | not covered | 9 |

### Schema: `ChangeSchema`

From `transition::change_schema`, `plan_migration`, `apply_change` and `schema::problems`.

| Decision | Refusal | Model | Slice |
|---|---|---|---|
| The caller may not manage the schema | `Unauthorized` | covered for owner only (`setRequired`) | 0, 6 |
| `expected_version` given and not the schema version | `SchemaConflict` | not covered | 11 |
| No changes; more than 200 | `InvalidIntent`, `LimitExceeded` | not covered | 11 |
| Definition authority: trigger and automation changes need `automations-define` and conferrable run-as roles; settings `setting-define`; separations of duties `separations-define`; ledger `ledger-configure`; products `bundle-install`; an operation's definer must hold the powers its steps use | `Unauthorized` | not covered | 11 (with 12 to 15) |
| A change to an entity an installed product owns, outside its upgrade or removal | `InvalidSchema` | not covered | 11 |
| A change that does not apply: its object, field, constraint, role, policy, workflow, query, view, trigger, automation, operation, unique, setting, capability, template, token, product or UI element is missing; an automation still named by a trigger; a built-in role redefined | `InvalidSchema` | not covered | 11 |
| The resulting schema has a problem (`schema::problems`: counts, unknown references between definitions, expressions that do not check as truth values, retention for personal fields, and the workflow, query, view, trigger, automation, operation, unique, capability, template, product and UI rules) | `InvalidSchema` | partly: the unique rules (a list field unique, an unknown field, a composite of the wrong size or with a repeated field, a list field in a composite) are covered by `invalidSchema`; the others are not, and the model will take an abstract validity predicate | 3, 11 |
| A new unique constraint that existing records already violate | `UniqueViolated` (a composite constraint), `MigrationRejected` (a strict unique field) | covered for a composite constraint (`defineUnique`) and for a strict unique field (`setUnique`); a lax change is kept, its count not modelled | 3 |
| A breaking change that reaches dependents not all pinned at the version in force, or past their window, or that is not a retype; a retype whose stored values do not round-trip | `InvalidSchema` | not covered in `step`; modelled in `Strata.Evolution` (`accepted`) | 11 |
| A field change whose stored values cannot be carried | `MigrationRejected` | not covered | 11 |
| Settings effects, such as a setting retyped while overrides exist | `InvalidSchema` | not covered | 12 |
| `Reject` and a record violates the new schema | `MigrationRejected` | covered for making a field required (`strict`) | 0, 11 |
| `Grandfather`: violating records stay, counted in the event | | covered for making a field required (`lax`); the count not | 0, 9 |
| An object type removed while it has records and the change does not delete them; a role removed while held | `MigrationRejected`, `InvalidSchema` | not covered | 11 |
| Accepted: the new schema at version + 1, rewritten and deleted records, `SchemaChanged` | | partly: the required flag | 11 |

### Authorization (`rules::authorize`, `roles_of`, `may_confer`)

| Decision | Model | Slice |
|---|---|---|
| The `owner` role may do everything in its tenant | covered (`Tenant.owner`) | 0 |
| A role permission granting an action on an object type allows it | not covered | 6 |
| A policy of a held role, for the action and resource (`AnyObject` matches every type), allows it when it has no condition or its condition is exactly `true`; `false`, `null` and an evaluation error do not | partly: editors (unconditional) and writers (`record.$created_by == principal`) | 6 |
| Default deny | covered | 0 |
| Roles: the system holds none, the anonymous caller `public`, an automation principal only its run-as roles, a person its direct and team roles and `public` | not covered | 6, 7, 15 |
| Conferring a role: only an owner confers `owner`; a role that grants nothing is free; otherwise the conferrer must hold every permission and policy power of it | not covered | 7 |
| Capabilities: the owner holds every capability | not covered | 6 |

### Membership, teams, invitations

From `transition::change_member`, `define_team`, `team_role`, `team_member`,
`create_invitation`, `revoke_invitation`, `redeem_invitation`. All not covered; slice 7.
Refusals in check order:

- `AssignRole`, `RevokeRole`: may not manage members (`Unauthorized`); unknown role
  (`UnknownRole`); the role `public` (`InvalidIntent`); may not confer it, for a revoke too
  (`Unauthorized`); a revoke that leaves no effective owner (`InvalidIntent`). Accepted: the direct
  roles change; a member left with no roles is removed and its sessions revoked; `MemberChanged`.
- `DefineTeam`: may not manage (`Unauthorized`); blank label (`InvalidIntent`). `AssignTeamRole`,
  `RevokeTeamRole`: may not manage; unknown team (`InvalidIntent`); unknown role; `public`; may
  not confer; no owner remains on revoke. `AddTeamMember`, `RemoveTeamMember`: may not manage;
  unknown team; may not confer every role of the team; no owner remains on removal. Granting a
  held role or removing a non-member is accepted.
- `CreateInvitation`: may not manage (`Unauthorized`); not a person, an address without `@`, no
  roles, an id already used (`InvalidIntent`); unknown role (`UnknownRole`); `public`
  (`InvalidIntent`); may not confer (`Unauthorized`). Accepted: an invitation expiring 14 days
  after `now`; no event.
- `RevokeInvitation`: may not manage; not a person; unknown, redeemed or revoked invitation; may
  not confer its roles. Expiry is not checked.
- `RedeemInvitation`: not a person, no verified address or a different one (`Unauthorized`);
  unknown, redeemed, revoked or expired (`InvalidIntent`); the issuer may no longer manage members
  or confer a role (`Unauthorized`). Accepted: the redeemer gains the roles; `MemberChanged`.

### Reads (`read::read`, `query_plan`, `search`, `aggregates`)

All not covered; slice 10.

- Visibility of a record: the authorization above for `Read` on the stored record; a field read
  rule hides a present field unless it is exactly `true`. `redact` gives the system records
  unredacted, but `roles_of` gives the system no role, and every policy and permission is held
  through a role, so `visible_in` refuses the system every record. Slice 10 must state which
  outcome is intended for system reads (open question).
- `Records { object, ids }`: more than 200 ids (`LimitExceeded`, before the type check); unknown
  type (`UnknownObject`); missing and invisible records are left out silently, never
  `UnknownRecord`; rows in request order, redacted.
- Named queries: unknown query (`UnknownQuery`); an undeclared parameter, page size 0 or a cursor
  of the wrong kind (`InvalidIntent`); page size over 200, or a scan over 10,000 records of the
  type (`LimitExceeded`). Accepted: the visible records whose redacted copy makes the filter
  exactly `true`, ordered by the order keys then by id, paged.
- Search: a query over 200 bytes, with no word, or with more than 8 words, or a page size of 0 or
  over 200 (`InvalidIntent`); unknown type (`UnknownObject`); at most 1,000 candidates examined.
  Hits in type then id order, with only the matched fields the reader may read.
- Aggregates: unknown query or aggregate (`UnknownQuery`); an undeclared parameter or filter field
  (`InvalidIntent`); unknown type or field (`UnknownObject`, `UnknownField`); scan, selection,
  group and sum bounds (`LimitExceeded`). Count, sum, min, max and top over the visible, filtered
  records whose named fields the reader may read.

### Settings (`settings::write_setting`, `resolve`)

Not covered; slice 12. `WriteSetting`: the caller is not a person (`Unauthorized`); platform
level (`InvalidIntent`); tenant or team level without its capability (`Unauthorized`); undefined
setting (`UnknownObject`); the level is not open (`InvalidIntent`); a team missing, unknown, or
given where none is allowed (`InvalidIntent`); `null` or the wrong type (`TypeMismatch`); over
the value limits (`LimitExceeded`); `expected_revision` not the override's (`VersionConflict`);
removing an absent override (`InvalidIntent`); over 100,000 overrides (`LimitExceeded`).
Accepted: the override put or removed; `SettingChanged`. Resolution: the person, team and tenant
overrides, then the platform value, then the default, each only where its level is open.

### Ledger (`ledger::decide_intent`, `post_after`, `ledger_expiry::decide`)

Not covered; slice 13. `PostLedger`: lacks `ledger-post` (`Unauthorized`); no postings or more
than 8 (`InvalidIntent`); then each posting on the state the earlier ones leave, the first
refusal refusing all: undeclared unit, owner not a member, undeclared pool (`UnknownRecord`); a
zero or oversized amount, a bad idempotency key, an oversized reason or scope (`InvalidIntent`);
a key already used with the same posting is replayed with no change, with a different posting
refused (`VersionConflict`); a bad correction target or a balance out of range
(`InvalidIntent`); a floor, cap or period cap crossed while not moving toward it
(`ConstraintViolated`). Accepted: the account's balance, period totals and buckets, an appended
entry, `LedgerEntryPosted`. `ExpireLedger`: not the system and lacks `ledger-configure`
(`Unauthorized`); a unit without expiry, or a cutoff whose buckets are still alive
(`InvalidIntent`); accepted: up to 100 accounts' expired buckets debited. Ledger reads: reach by
`ledger-read`, ledger policies or ownership (`Unauthorized`, `UnknownRecord`, `LimitExceeded`).

### Operations, triggers, automations (`operations::invoke`, `triggers::after_decision`)

Not covered; slices 14 and 15. `InvokeOperation`: unknown operation (`InvalidIntent`); the caller
is not a person or not an invoker (`Unauthorized`); a bad idempotency key (`InvalidIntent`); an
undeclared, missing, oversized or mistyped input (`UnknownField`, `MissingRequired`,
`LimitExceeded`, `TypeMismatch`); a subject that is not a record the invoker can read
(`UnknownRecord`); a condition that is false (`ConstraintViolated`) or cannot be evaluated
(`Evaluation`); then the steps (create, update, delete, post) in order, with the operation's own
powers, as one atomic decision in which any step's refusal refuses all. Triggers run after every
accepted decision: an automation whose definer lost authority is suspended
(`AutomationSuspended`); each event matching a trigger yields a `StartAutomation` or a
`DeclineAutomation` effect (paused, removed, suspended, inactive, unreadable, condition false or
failed, hop limit, run key).

### Segments, erasure, retention, approvals

Not covered; slices 16 and 17. Segments: definition refusals (capability, name, predicate checks,
counts, refresh interval) and membership changes with entry and exit events. `EraseSubject`: not
an owner who is a person (`Unauthorized`); it would leave no owner (`InvalidIntent`); personal
fields cleared or held, memberships, teams and invitations stripped, sessions revoked.
`ExpireRecords`: not the system (`Unauthorized`); unknown type (`UnknownObject`); no retention,
or more than 100 ids (`InvalidIntent`); records past their period that no record refers to are
deleted, the others kept with a reason. Approvals (`approvals::decide`) are a pure decision on
the approver (not a person, without `workflow-approve`, not named by the step's policy, the one
who started it, past the deadline) and go with slice 8.

### Slices, in dependency order

Each slice extends the model abstractly (outcomes, not the kernel's algorithm), keeps `step`
total and computable, adds its intents to `lean/Main.lean` and its properties to the proofs.

| Slice | Adds | Needs |
|---|---|---|
| 0 (before WG-5373) | One object type, scalar fields, the type, required and unknown-field checks, owner, editors and writers, create, update as replacement, delete, making a field required strictly or not, the missing-record rule | |
| **1 (done)** | Single references within the one type: `DanglingReference` on write in field order, `StillReferenced` for restricting or required fields, clearing on delete; referential integrity as an invariant | 0 |
| **2 (done)** | Record writes in full: patch updates, versions and `expected_version`, constant defaults, lists (of scalars and of references: element removal on delete, a required list never blocks), the size limits as an abstract bound | 1 |
| **3 (done)** | Uniqueness: single-field and composite (only when every field has a value), on write and on defining a constraint over existing data | 2 |
| **4 (done)** | Several object types: `UnknownObject` with its hiding rule, references and referrers across types | 1 |
| 5 | Expressions as an abstract three-valued language over record, old, new, principal, roles and `now`, with evaluation errors; constraints (checked last); expression defaults | 2, 4 |
| 6 | Roles and policies as data, replacing the fixed tiers: the owner role, role permissions, policies with conditions, capabilities, the system and anonymous principals | 5 |
| 7 | Tenants, members, teams, invitations, reserved principal ids | 6 |
| 8 | Workflows: initial states, transitions with guards and assignments, re-authorization, state validity; approvals | 5, 6 |
| 9 | Events and revisions: event bodies, sequence numbers, the revision, `Publish` effects, the grandfathered count | 1 to 8, for their content |
| 10 | Reads: visibility, field read rules, record reads, named queries (filter, order, pages), search, aggregates | 5, 6 |
| 11 | Schema change in general: the change kinds, `SchemaConflict`, the abstract validity predicate, `Reject` and `Grandfather`, object and role removal, and the evolution model's acceptance as the rule for breaking changes | 3 to 8 |
| 12 | Settings | 6, 7 |
| 13 | Ledger | 6 |
| 14 | Operations | 1 to 8, 13 |
| 15 | Triggers and automations | 6, 9 |
| 16 | Segments | 5, 9, 15 |
| 17 | Erasure and retention | 1, 7 |

### Slice 1 review note: what was abstracted

- **One object type.** A reference names a record of the tenant's only type, so the referrers are
  the records of that type. References between types, and `UnknownObject`, wait for slice 4; the
  statements quantify over all records and do not depend on there being one type.
- **Outcomes, not the referrer scan.** The kernel walks the reference fields of every type and,
  for each, an index of the records holding the deleted id, stopping at the first that blocks.
  The model states the outcome: refuse if any other record names the id through a field that
  forbids it (`blocks`), else remove the record and clear every value naming it (`clearRef`). The
  order of the scan cannot change the outcome, so the model has none.
- **Single references only.** Lists of references (an element removed rather than the field
  cleared, and a required list never blocking) wait for slice 2, with lists in general.
- **Refusal order.** Which refusal a record with several faults gets is part of the outcome, so
  `problem` keeps the kernel's order: unknown fields first, then each field of the schema in
  schema order (the kernel's is id order, and the fixtures declare theirs so), missing, wrong
  type, then naming no record. Uniqueness, workflow and constraint checks are not yet in it.
- **What forbids a delete.** `forbidsDelete` asks whether any definition of the field is required
  or restricting. With one definition per field id, as in the kernel, that is the kernel's rule;
  it is stated over all definitions so that the proofs need no uniqueness of field ids.
- **No versions or events.** The kernel raises the version of each cleared record and emits an
  update event for it; versions are slice 2, events slice 9.
- **Integrity holds for every step.** Referential integrity is proved for every accepted step,
  the non-strict schema change included, and it is part of `Tenant.WF`. The kernel's strict
  migration check re-validates references too; the model's checks conformance only. The two agree
  on every state where integrity holds, which are the only states the model reaches.

### Slice 2 review note: what was abstracted

- **Outcomes, not the patch loop.** The kernel copies the stored record and inserts or removes each
  named field. The model applies the patch entry by entry (`Fields.patch`) and states the outcome:
  unnamed fields keep their values, the last value named wins, `null` removes. The kernel's patch
  is a map, so it never names a field twice; the model's statements hold either way.
- **Check order.** An update or delete asks, in the kernel's order: does the record exist (the
  missing-record rule), may the caller act on it, is it at the expected version, then (update) the
  validation of the patched record or (delete) whether anything blocks it. Workflow transitions,
  which the kernel runs between the version check and validation, are slice 8.
- **Versions.** A created record is at version 1; an accepted update writes version + 1; a delete
  raises the version of each record whose reference it clears, once however many of its fields
  named the deleted record (`clearRef` raises it only for a record that refers). The update events
  the kernel emits for those records are slice 9.
- **Constant defaults only.** A default is a value of the field's definition, `null` for none. The
  kernel evaluates an expression, which can fail with `Evaluation`; expressions are slice 5. The
  kernel fills a default only when the intent does not name the field at all: a field named as
  `null` gets no default, and so it is in the model. A default goes through validation like any
  other value, so a default of the wrong type is refused with `invalid` on create.
- **Lists hold scalars or references.** List elements are a type of their own (`Elem`), so a list
  never holds a list, matching the kernel's schema rule against nested lists. A list type checks
  when every element has the element type; a `null` element has no type, as in the kernel. The
  type is checked over the whole list before any element's reference is resolved, so a list with
  an element of the wrong type and a dangling element is `invalid`, as in the kernel.
- **What forbids a delete, for lists.** A restricting list blocks; a required list never does
  (the element is removed and the field stays, possibly as the empty list, which is present).
  `forbidsDelete` asks whether the value held is a list rather than whether the definition is a
  list type; on records of the declared types, the only ones the model reaches, the two agree,
  and the value form keeps the proofs free of an assumption that field ids are unique.
- **Size limits as an abstract bound.** The bounds are numbers of the tenant (`Limits`, the
  kernel's constants in the fixtures). A string's UTF-8 bytes and a list's length are measured
  exactly, so the string and list limits agree with the kernel everywhere. The record size and a
  unique value's size are measured by `Val.size` (a string's bytes, one for any other scalar, a
  list the sum of its elements), not by the kernel's JSON encoding, which also counts quotes,
  separators, field names, the id, the timestamps and so on. The encoding is never shorter than
  `Val.size`, so whenever the model refuses for size the kernel does too, at the same step of the
  validation order (the record bound first, the value bounds in each field's step, before its
  type). The kernel can refuse a record the model accepts when the encoding's overhead crosses the
  bound: scenarios that compare the record bound keep the model's size either above it or below
  it by more than the overhead (a few hundred bytes plus a few bytes per value). The unique-value
  bound waits for slice 3 to be driven, since a unique fixture field would bring uniqueness with it.
- **Statement changes.** `delete_refused_while_referenced` now takes the delete's expected version
  and a hypothesis that it does not clash; `delete_leaves_no_reference` states that no field
  `refersTo` the deleted record, which covers list elements and implies the slice-1 statement; the
  authorization theorems take any expected version. No statement was weakened.

### Slice 3 review note: what was abstracted

- **Unique values are keyed scalars.** A value clashes on a unique field or in a composite
  constraint only when it is present and not a list (`Val.keyed`). The kernel refuses a list field
  as unique, so no reachable state has a unique list. Because a list is never a key, a delete's
  clearing of references cannot make two values equal: a single reference becomes absent, and a
  list loses elements without ever becoming a key.
- **Composites are sets of field ids.** The kernel's `UniqueDef` also carries an id and a message.
  The id is the constraint's name; the model needs neither, and the message is not an outcome.
  `uniqueFieldsOk` refuses what the kernel's `unique_problems` refuses for the object's fields: a
  count outside two to four, an unknown field, a list field, a repeated field (`invalidSchema`).
  The kernel's check of the message, which is never empty in the fixtures, is not modelled. A
  composite defined again under the same id overwrites the kernel's entry; the model appends a
  second copy, which checks the same thing, so the outcomes agree.
- **The check order is the kernel's.** Per field in schema order: required, the value bound, the
  unique bound, the type, the unique clash, the reference; then the composite constraints. The
  clash is checked after the type, as in `validate_record`. `valueProblem` and `problem` take a flag
  `uniq` that switches the unique checks on, so the theorems can say what the refusal is when the
  write passes everything else (`problem_uniq_dichotomy`, `problem_uniq_refusal`).
- **Existing records are checked by one rule for every strict schema change.** The kernel's
  `Reject` validates every existing record against the resulting schema, and every unique
  constraint with it. The model has one shared refusal, `schemaChangeRefused`, that `setRequired`,
  `setUnique` and `defineUnique` all use: refused with `migrationRejected` when `schemaViolated`
  holds, which is when some record does not conform (a required field missing, a value of the wrong
  type, a reference to nothing), a unique field is held by two records or holds a value over the
  unique bound, or a composite constraint is held by two records. A violation a lax change left
  behind therefore refuses a later strict change whatever the change is about. A lax change never
  refuses. `defineUnique` checks its own new composite first, as the kernel's `plan_migration` does
  (`uniqueViolated`), and the shared check after it (`migrationRejected`). The rule that results is
  `strict_schema_change_conforms`.
- **Names stay distinct, and `normalise` keeps the last value.** The kernel's records are maps, so
  a name occurs once in a record and once in the schema. The model needed that as the invariant
  `Tenant.Distinct`, since the delete proof reads each record's value by name. `normalise` now keeps
  the last of a name given twice, as the kernel's map insertion does. Before this slice a create
  naming a field twice kept both entries; no scenario does that (the one that names a field twice
  is a `patch`, which already used `Fields.set`), so no existing output changed.
- **The unique bound is abstract, as in slice 2.** A unique value's encoded size is measured by
  `Val.size`, which is never more than the kernel's encoding. The fixture's unique field holds short
  strings, `s*201` is refused by both sides, and no scenario puts a unique value between 196 and 200
  bytes, where the two encodings disagree on the bound.
- **The working set reads the whole record list for a write with unique checks.** The decision
  for a create or update reads the clash from the record list it is given, which is the list a delete
  and a strict schema change already read. `decideTenant_faithful` still holds. Whether the runtime
  should answer the clash from an index instead is a question for the working-set coverage, not for
  the model; the model's decision is the kernel's.
- **Not modelled.** Workflow and constraint checks; the kernel's value normalisation
  (`normalise`, a field's `Normalisation`); the lookup index (`by_field`, `by_fields`), which the
  model answers by scanning the records; the explanation of a refusal, which names the clashing
  record only to a reader who may see it (the model gives the kind only); and the kernel's count of
  grandfathered records.

### Slice 4 review note: what was abstracted

- **Object types are a list of entries with global ids.** A tenant holds its types as a list of
  `(id, object type)` pairs; `putType` replaces the first entry of its id only. Record ids come from
  the tenant's one counter, as the kernel's do, so an id names a record of one type at a time
  within a tenant.
- **Roles are tenant-wide.** The owner, editors and writers act on every type of the tenant. The
  kernel's fixture gives the same roles a policy per type (`editors_all`, `writers_create`,
  `writers_own` and their `part_` counterparts), so the two agree on the fixture's types only.
- **The hiding rule is the owner's alone.** An unknown type is `unknownObject` for the owner and
  `unauthorized` for anyone else. The model has no notion of a caller who may act on some types
  and not this one, so a caller in that position learns the same refusal as any other non-owner.
- **Schema changes on an unknown type follow the kernel's ChangeSchema path, not the record path.**
  `change_schema` requires `Manage` on the schema first (`unauthorized` for anyone but the owner),
  then `apply_change` fails with `no object type` for a change naming a missing type, which
  `plan_migration` reports as `InvalidSchema`. So the owner gets `invalid_schema`, whatever the
  change (`require`, `unique`, `composite`). Record intents differ: `draft.require(action, object)`
  passes for the owner, who then gets `UnknownObject` (`object_def` in `transition.rs`).
- **Unique and required flags name fields the type declares.** The kernel's `DefineField` creates a
  field the type lacks, which the model's `setRequired` and `setUnique` (naming an existing field)
  cannot express without a field type in the intent. The model refuses `setUnique` on such a field
  with `invalidSchema`, and `setRequired` leaves the schema as it is. The differential generator
  therefore names only declared fields in `require` and `unique` lines on `part` (fields 1 to 4).
  The gap is open: a model extension that carries the field's type would close it.
- **Checked against the kernel for every schema intent** (`schema.rs` `problems`, `definitions.rs`
  `unique_problems`, `transition.rs` `change_schema`, `plan_migration`, `apply_change`): an unknown
  type (above); `setUnique` on a list field is refused by the kernel (`a list field cannot be
  unique`), as the model's `isList` check; a composite constraint (`defineUnique`) is refused for
  fewer than two or more than four fields, a repeated field, an undeclared field, a list field, or
  an empty message, as `uniqueFieldsOk`; a strict change over records that do not conform or agree
  is `MigrationRejected` in both, over the types the change touches (`schemaChangeRefused` on `scopedTypes`, `Reject`). The kernel's other checks
  (definition authority, last owner, products, ledger, pins) apply to change kinds the model does
  not have.
- **The check order is kept.** Object type first, then the record, the permission, the version, then
  the referrer scan or the validation. The referrer scan considers every type; the validation
  resolves a reference in the type its field names.
- **A strict schema change is checked over the types it touches.** The kernel re-validates the
  records of the object type a change names and, through `touched_objects` in `transition.rs`, the
  types a new reference field refers to (a `DefineField` or `DefineRelationship` naming a reference,
  and a `MoveField` target). A composite constraint touches only its own type. The model's
  `Intent.touched` gives that set (the named type, plus `fieldTargets` of a `setRequired` or
  `setUnique` field), and `schemaChangeRefused` is applied to `scopedTypes` of it, not to the whole
  tenant. A grandfathered record of an unrelated type therefore does not refuse a strict change, as
  the random tests found (`h-strict-change-scoped-to-the-touched-type`). A reference into a type
  with a grandfathered record does refuse it (`h-strict-change-revalidates-a-reference-target`).
  `strict_schema_change_scoped` states the touched types' conformance and the others' records as
  they were; `strict_schema_change_conforms` and `strict_schema_change_unique` derive the tenant's
  conformance and uniqueness from it, given that the tenant held them before.
- **Restated theorems.** Several slice-2 and slice-3 statements were stated over the one table.
  They are restated over the typed tables, with the same content: `setRequired_spec`,
  `setUnique_spec`, `defineUnique_spec` (the counter and the records of no type, and the one type's
  schema or constraints), `setRequired_records`, `setUnique_records`, `defineUnique_records`,
  `schemaViolated_false` (over every type), `mem_afterDelete` (over any clearing function, which
  `clearRef` is). Two statements have typed counterparts under new names rather than restatements:
  `has_afterDelete` is `env_afterDelete_has`, and `resolves_dropRef` is `resolves_dropRef_sub`.
- **Not modelled.** Object types are fixed by the fixture: creating or removing a type, a type's
  permissions, and the kernel's referrer index are not modelled. Fields `11` and `12` are the only
  item fields that name a part; a reference to a type the fixture does not define is refused by the
  kernel's own schema check, which the model does not state.

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
- External effects: with faults on, publication runs through the backend's own publisher over an in-memory store; what a subscriber does with an event is not modelled.
- Operations nobody generated or recorded.
- Parameterised queries, other than through the views that bind their arguments.

A calibrated resource model would be a second pass over finished traces (see the note in
`crates/simulation/src/lib.rs`). None exists, and no performance claim is made.
