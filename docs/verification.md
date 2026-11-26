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
per tenant with typed fields that may be required, records with an owner and a version, roles and policies as data (slice 6), and four intents: create (filling constant defaults), update (a patch),
delete (both naming an expected version or not), and make a field required (strictly or not).
Fields may be references to records of the same type, which a delete either refuses to break or
clears (slice 1, below), and lists of scalars or of references, whose elements a delete removes
(slice 2). Written values and records are held to the kernel's size limits, as an abstract bound
(slice 2). `step` is a total, computable function. Slice 3 adds unique fields and composite unique constraints, checked on a write and when a
constraint is defined over existing data. Slice 4 adds several object types per tenant: a write
may name any of them, a reference may name a record of another type, and an intent naming a type
the tenant lacks is refused. Slice 5 adds expressions, constraints and expression defaults. Slice 6 replaces the fixed tiers by
roles and policies as data (the owner role, role permissions, policies with conditions, capabilities,
the system and the anonymous principal). Slice 7 adds members, teams and invitations, and slice 8 adds
workflows: a workflow over a string field of an object type, with its initial states, its transitions
and their guards and assignments, decided by the same `step`. The model has no queries, events or
scopes: the assumptions below list each
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
| Expressions (slice 5) | `and_null_false`, `and_null_true`, `and_null_null`, `and_true_null`, `and_true_true`, `and_true_false`, `or_null_true`, `or_null_false`, `or_false_null`, `or_false_true`, `not_null`, `isNull_null`, `cmp_null_operand`, `cmp_int_lt`, `cmp_str_ge`, `cmp_ref_eq` | The truth tables of the three-valued connectives, comparisons and `not`, decided by evaluation on closed terms: `null` is contagious and the operands of `and`/`or` are Kleene |
| | `and_false_shortcircuit`, `or_true_shortcircuit`, `and_false_left`, `or_true_left`, `and_null_left_nonfalse`, `and_error_left` | A decisive operand decides before the other is looked at, so an evaluation error there is not seen (`false and x` is `false` whatever `x` is); an error in an operand that is looked at is an error of the connective |
| | `and_null_error`, `and_int_error`, `or_false_error`, `cmp_kind_error`, `cmp_bool_order_error`, `not_int` | A comparison of two kinds, an ordering of booleans, and a non-boolean connective operand are evaluation errors |
| Constraints (slice 5) | `constraint_violated_refused`, `constraint_evaluation_refused`, `null_constraint_passes`, `constraint_first_failure` | The first constraint that is `false` gives `constraintViolated`, the first that cannot be evaluated gives `evaluation`; `true` and `null` pass; the first failure decides |
| | `create_constraint_refused`, `update_constraint_refused` | A write whose structural checks pass and whose constraint fails on the record it writes is refused with that error, and no state follows |
| Defaults as expressions (slice 5) | `create_unauthorized_regardless_of_defaults`, `create_default_error_when_permitted`, `unauthorized_regardless_of_defaults` | A caller whom nothing could allow to create on the type is refused `unauthorized` whatever the defaults are; a caller who could create gets `evaluation` from a failing default, before the decision on the record: the permission comes before the defaults (WG-5665) |
| | `create_checks_defaulted_record`, `default_refused_by_constraint`, `named_value_accepted` | An accepted create's record, with its defaults in it, satisfies the constraints; a default that violates a constraint refuses the create (a concrete tenant) |
| | `create_fills_default` (restated for a constant default `.lit v`), `defaultsOf_mem`, `newRecord_parts`, `defaultsOf_nodup`, `newRecord_nodup` | An accepted create writes the constant default of each unnamed field; the defaults add no duplicate name |
| Constraint invariant (slice 5) | `stepTenant_preserves_satisfied`, `step_preserves_satisfied` | For constraints that are state-only (`Tenant.StateConstraints`), an accepted create, update or schema change leaves every stored record satisfying every constraint of its type; it needs the state before to satisfy them (`Tenant.Satisfied`) |
| | `evalExpr_congr`, `constraintProblem_congr`, `satisfied_of_putType` | A state-only constraint has the same value on a stored record as on the record a write checks (the old record and the principal do not enter it), and replacing a type's schema keeps its records' constraints |
| | `brk_delete_refused`, `clearBreaks_constraints` | A delete that would leave a rewritten referrer violating a constraint is refused with `constraintViolated` (a concrete refusal, and the lemma the invariant proof uses), so every accepted delete keeps the invariant |
| Codec (slice 5) | `lawful_expr`, `decExprN_encExpr`, `decExprN_canon`, `lawful_objType`, `lawful_fieldDef`, `lawful_err` | The expression encoding round-trips for any fuel that covers its nodes, and every decoding is canonical; the object type and the field definition carry their constraints and default expressions; the error tags are fourteen |
| Roles and policies (slice 6) | `allows_iff`, `default_deny`, `default_deny_create`, `default_deny_update`, `default_deny_delete` | The Bool decision holds exactly when the principal is permitted the action (`Tenant.Permits`: the owner role, a grant of a held role naming the action on the type, or a policy of a held role naming the action on the resource whose condition is `true`). A principal no role of which permits the action is refused `unauthorized` at the step, and no state follows |
| | `create_ok_permitted`, `update_ok_permitted`, `delete_ok_permitted`, `setRequired_ok_permitted` | An accepted step, of a record intent or a schema change, is permitted for the record it writes: its principal holds a role that permits the action |
| | `system_never_permitted`, `system_refused_create`, `system_refused_update`, `system_refused_delete`, `system_refused_schema`, `rolesOf_system` | The system holds no role, so nothing permits it: every record intent and schema change it makes is refused `unauthorized` |
| | `rolesOf_anonymous`, `anonymous_not_owner`, `anonymous_permitted_only_public` | An anonymous caller holds `public` and nothing else; what it is permitted is only what a grant or a policy of `public` gives it |
| | `owner_holds_every_capability`, `capability_held_by_grant` | The owner role holds every capability; any other holding of a capability is a grant of a role the principal holds |
| | `unknown_type_owner`, `unknown_type_other`, `unknown_type_hidden`, `unknown_type_schema_refused` | The slice 4 refusals, restated over the decision: a principal the owner role makes able to act on the type gets `unknownObject` for an unknown type, anyone else `unauthorized`, and a schema change needs the schema permission |
| | `fixture_owner_permits`, `fixture_editor_permits`, `fixture_stranger_refused_item`, `fixture_anonymous_refused_item`, `fixture_anonymous_creates_part`, `fixture_stranger_refused_part`, `fixture_editor_creates_item`, `fixture_writer_updates_own`, `fixture_writer_refused_other`, `fixture_editor_updates_other`, `fixture_writer_missing_record_hidden`, `fixture_editor_missing_record`, `fixture_system_refused_item` | The fixture's roles decide as the tiers of slices 0 to 5 did (owner, editor, writers who may change only the records they created), and the anonymous caller creates only parts; checked by evaluation on tenant `1` and on a tenant of two items |
| | `step_ok_authorized`, `unauthorized_cannot_update`, `unauthorized_cannot_delete`, `missing_record_hidden_on_update`, `missing_record_hidden_on_delete`, `version_conflict_on_update`, `version_conflict_on_delete`, `delete_refused_while_referenced`, `update_duplicate_refused`, `create_duplicate_refused` | Restated over the generalised decision: the permission is that of the record as it would be written (for an update, the patched record), and a record-free check for a missing record asks the type and the action; the statements keep their meaning (no statement weakened) |
| Membership, teams and invitations (slice 7, `lean/Strata/MembersProofs.lean`) | `effective_roles`, `system_holds_no_role`, `anonymous_holds_public`, `system_cannot_manage_members` | A user holds `public`, its direct roles and the roles of its teams, and nothing else; the system holds no role; an anonymous caller holds `public` |
| | `hasOwner_iff`, `genesis_hasOwner`, `changeRole_owner`, `defineTeam_owner`, `teamRole_owner`, `teamMember_owner`, `redeemInvitation_owner`, `stepMember_owner_kept`, `owner_kept_world` | An accepted membership step keeps an owner in every tenant that had one, a revocation or removal only when one remains; a created tenant has one (given distinct team ids, `stepMember_teamsNodup`, `step_nodup`), whatever the intent: a record or schema intent keeps the membership (`stepTenant_data_mt`, `step_data_mt`) |
| | `assign_only_conferrable`, `teamRole_only_conferrable`, `teamMember_only_conferrable`, `redeem_only_conferrable` | An accepted assignment, team role grant, team membership or invitation redemption gives no principal a role it did not hold that the conferrer (the issuer, for a redemption) could not confer in the state before |
| | `step_member_form`, `stepMember_manage`, `member_authorized`, `stepTenant_member_eq`, `step_cases` | The membership intents are decided by `step`: an accepted one changes the addressed tenant by a membership step and no other, only a manager makes one (a redemption only a person), and an accepted step is a decision of `stepTenant` or a creation |
| | `reserved_refused`, `reserved_caller_refused`, `create_genesis`, `create_blank_refused`, `create_world_form`, `genesisTenant_wf` | A reserved id is refused first, whatever the intent; only the system creates a tenant, once, with a name, and the tenant it makes is well-formed and has an owner |
| | `create_invitation_expiry` | An accepted invitation is recorded for its issuer, expiring fourteen days after the intent's time |
| | `redeem_sets_redeemed`, `redeem_twice_refused`, `redeem_not_expired`, `redeem_not_closed`, `redeem_issuer_authority` | An accepted redemption is of an invitation unredeemed, unrevoked and unexpired, while the issuer may still manage and confer the roles, and afterwards the invitation is redeemed, so a second redemption is refused for any caller |
| | `revoked_not_redeemable` | A revoked invitation is refused every redemption |
| | `reserved_refused`, `assign_reserved_refused`, `add_member_reserved_refused`, `create_reserved_refused` | A step that names a reserved principal, or whose caller is reserved, is refused `invalidIntent` before the tenant is looked at |
| | `create_genesis`, `create_existing_refused`, `create_not_system_refused`, `genesis_owner_and_nodup` | A tenant is created only when it does not exist, by the system, with a name that is not blank; the genesis tenant has the owner, the built-in roles, no teams and no invitations |
| | `directOf_setDirect`, `rolesOf_mono_setDirect`, `changeRole_nodup`, `stepMember_teamsNodup` | The member and team tables keep their meaning under the updates, and team ids stay distinct under every accepted step |
| Workflows (slice 8, `lean/Strata/Workflows.lean`; the list lemmas in `lean/Strata/WorkflowLemmas.lean`) | `create_ok_initials`, `create_workflow_initial` | An accepted create writes a record whose workflow field holds one of the initial states of every workflow of its type |
| | `create_non_initial_refused` | A permitted create whose record does not start in an initial state is refused with `workflowViolation`, and no state follows |
| | `fireOne_declared` | A workflow that changes the state of its field takes a declared transition from the old state to the new one whose guard is `true` on the environment the step reads; its assignments give their fields the values of their expressions, when the assigned fields are distinct |
| | `fireOne_keeps`, `applySets_get_mem`, `applySets_get_other`, `evalSets_keys`, `fireAll_nodup`, `fireAll_version` | An assignment changes only the fields it names, to the values they are given; the workflows keep the field names distinct and the version |
| | `fireAll_fires` | Under `WfIndependent` (no transition assigns a workflow field), every workflow of the write whose field the patch changed took such a transition, and its guard held on the candidate the step read |
| | `update_changed_fired` | An accepted update that changes a workflow field by its patch has, for that workflow, a declared transition from the stored state to the patched one whose guard held (the `WfIndependent` hypothesis) |
| | `update_undeclared_refused` | Under `WfIndependent`, an update that changes a workflow field with no declared transition from the stored state to the patched one is never accepted |
| | `update_writes_fired` | The record an accepted update writes is the patched record as the workflows leave it; a record the workflows changed is authorised again (`canUpdate` on the record written) |
| | `stepTenant_preserves_states`, `step_preserves_states`, `clearRef_statesOk`, `str_of_clearFields`, `putType_statesOk` | Every accepted step keeps each stored record's workflow field one of its states, when it holds a string (`Tenant.StatesOk`); a delete's clearing of references keeps it so (the distinct field names of the records are assumed, `Tenant.Distinct`) |
| | `update_raises_version` | Unchanged in statement; the version of the written record is the stored one plus one for every tenant, since the workflows change fields and never the version |
| | `update_writes_patched`, `update_keeps_unnamed`, `update_ok_parts`, `update_refs_resolve`, `update_refused_dangling_across_types`, `update_within_limits` | Restated with the hypothesis `Tenant.Plain` (no transition assigns a field): these describe the patched record as the record written, which holds only when no assignment changes it (see the slice 8 review note) |
| | `update_duplicate_refused`, `create_duplicate_refused`, `create_refused_over_record_bound`, `create_constraint_refused`, `update_constraint_refused` | Restated with the workflow checks an otherwise valid write must pass: `initialsOk` on a create, and for an update that the workflows leave the patched record as it is and its states hold |

`lake build` checks them. `lake env lean Audit.lean` prints the axioms each depends on: only
Lean's standard `propext`, `Classical.choice` and `Quot.sound`. There is no `sorry`, no `admit`
and no added axiom; a test fails if the sources contain one or the audit reports `sorryAx`.

### Assumptions

1. The object of every theorem is the model, not the Rust source.
2. The model has no queries or events. Its workflows (slice 8) are over a string field of an object type, with transitions whose guards and assignments are expressions of the slice 5 fragment; the kernel's static rules for them (the field is a string, the transitions name states, the expressions check) are the schema validity of slice 11 and are not part of the model. Its constraints are of the state-only kind
   (slice 5: they read neither the old record nor the principal). Its unique fields and composite
   constraints are checked on written values only; the kernel's schema rules for them (a list
   field cannot be unique, a composite names two to four distinct fields of the object) are the
   model's `invalidSchema` refusals and nothing more. Its references are
   references, single or in lists, to records of the tenant's object types, the target of a field
   being the type it names (slice 4). Object types are fixed by the fixture; creating or removing
   one is not modelled (slice 11).
3. Its authorization is roles and policies as data (slice 6): principals hold roles directly, roles
   grant actions on object types and capabilities, and policies grant actions under conditions of the
   slice 5 expressions. Scopes, role and policy definitions by intent, and automation principals
   are not modelled (slices 11 and 15, and the gap analysis); a team gives its members its roles (slice 7). Policy ids are left out: they name a policy in
   a refusal and decide nothing.
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
| `Record { owner, version, fields }` | `Record { created_by, version, fields, .. }`; `owner` is none when the system or an anonymous caller created the record |
| `Limits`, `Limits.kernel` | the constants of `limits.rs` |
| `Tenant.uniques` | `ObjectTypeDef.unique`, the composite constraints of the object type (`UniqueDef`, by its fields) |
| `Tenant.members`, `grants`, `policies` | `members` (each user with its roles; the owner holds the owner role); `grants` (a role's permissions and capabilities); `policies` (a role, a resource, actions and an optional condition: the kernel's policy without its id) |
| `Principal` | `Principal` (`User`, `System`, `Anonymous`) |
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
- Slice 6's additions to the scenario format: the principal of an `op` line is a user number, `system`
  (which holds no role) or `anon` (an anonymous caller, which holds `public` only). A record with no
  creator prints `owner=-`.
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
| A caller, new owner, assigned member or added team member whose id starts with `team:`, or is reserved for an automation | `InvalidIntent` | covered in the membership layer (`reserved_refused`); the id prefix and the automation ids are one predicate, `reserved` | 7 (spec layer) |
| `CreateTenant` when the tenant exists; by anyone but the system; with a blank name | `TenantExists`, `Unauthorized`, `InvalidIntent` | covered in the membership layer (`create_existing_refused`, `create_not_system_refused`, `create_genesis`); the agent refusal `AgentAdminPower` is folded into the reserved predicate | 7 (spec layer) |
| Accepted `CreateTenant`: the genesis schema, the named owner holding `owner`, record counter 1 | | covered in the membership layer (`create_genesis`, `genesis_owner_and_nodup`); the genesis schema has no object types | 7 (spec layer) |
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
| Create fills absent fields from their defaults; a default that fails to evaluate | `Evaluation` | covered: constant and expression defaults (`withDefaults`, `defaultsOf`, a field named as `null` gets none); a failing default is refused before the permission | 2 (constant), 5 (expression) |
| A new record's workflow field is not an initial state | `WorkflowViolation` | covered (`create_non_initial_refused`, and `create_workflow_initial` for the stored record; the check comes after the permission and the defaults, before the other checks of the record) | 8 |
| An update that changes a workflow field needs a declared transition whose guard is true; its assignments apply; the update is authorized again if they changed the record | `WorkflowViolation`, `Evaluation`, `Unauthorized` | covered (`fireOne`, `fireAll`, `update_changed_fired`, `update_undeclared_refused`, `update_writes_fired`, `update_reauthorized`), for a workflow field changed by the patch under `WfIndependent` (no transition assigns a workflow field); a guard that is not `true` or cannot be evaluated is not a transition, and an assignment whose value cannot be evaluated is `Evaluation` (in the model; no fixture reaches it, see the slice 8 review note) | 8 |
| Validation; the first failure names the refusal, in this order: | | | |
| 1. the record is larger than `MAX_RECORD_BYTES` | `LimitExceeded` | partly: over an abstract size that bounds the encoding below (slice 2's note) | 2 |
| 2. a field the type does not declare | `UnknownField` | covered (`invalid`) | 0 |
| 3. per field in id order: required and absent; a value over the string or list limits; a unique value over `MAX_UNIQUE_BYTES`; the wrong type | `MissingRequired`, `LimitExceeded`, `TypeMismatch` | covered for required, type (lists elementwise) and the string and list limits; partly for the unique-value limit (abstract size, not the encoding; field `10` is unique in the fixtures) | 0, 2, 3 |
| 4. per field in id order, after its type: a unique value another record holds | `UniqueViolated` | covered (`valueProblem`, before the reference check of the same field) | 3 |
| 5. per field in id order, after its type: a reference to a record that does not exist | `DanglingReference` | covered for single references and list elements, across types too | 1, 2, 4 |
| 6. composite unique constraints whose fields all have values, held by another record | `UniqueViolated` | covered (`compositeProblem`, after every field) | 3 |
| 7. a workflow field holding a value that is not one of its states | `WorkflowViolation` | covered (`statesOk`, checked after the other structural checks and before the constraints; `step_preserves_states` is the invariant); an absent workflow field is not a state the check refuses (the kernel checks string values only) | 8 |
| 8. only if nothing above failed: constraints, failing on `false`; `Evaluation` when one cannot be evaluated | `ConstraintViolated`, `Evaluation` | covered for the expression fragment of slice 5 (`constraintProblem`, after every other check); the kernel's static check of a schema is not modelled (slice 11) | 5 |
| Accepted create: the record under the next id, `created_by` the caller (none for the system), counter + 1 | | covered (owner = caller) | 0 |
| Delete: another record of any type referring through a `restrict` field, or through a required single reference, blocks it; a record referring to itself does not | `StillReferenced` | covered for references and lists, across types too (a required list never blocks) | 1, 2, 4 |
| A delete clears a referrer's reference and would leave the referrer violating a constraint | | covered: the delete is refused with `constraintViolated` (`brk_delete_refused`), so the invariant is kept (WG-5664) | 5 |
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
| `Reject` and a record violates the new schema, constraints included | `MigrationRejected` | covered for making a field required (`strict`) and for the constraints of the touched types (slice 5); not for a unique field, which slice 3 covers | 0, 3, 5, 11 |
| `Grandfather`: violating records stay, counted in the event | | covered for making a field required (`lax`); the count not | 0, 9 |
| An object type removed while it has records and the change does not delete them; a role removed while held | `MigrationRejected`, `InvalidSchema` | not covered | 11 |
| Accepted: the new schema at version + 1, rewritten and deleted records, `SchemaChanged` | | partly: the required flag | 11 |

### Authorization (`rules::authorize`, `roles_of`, `may_confer`)

| Decision | Model | Slice |
|---|---|---|
| The `owner` role may do everything in its tenant | covered (`ownerRole`, held through `members`) | 0, 6 |
| A role permission granting an action on an object type allows it | covered (`Grant.object`; never for a manage action) | 6 |
| A policy of a held role, for the action and resource (`AnyObject` matches every type), allows it when it has no condition or its condition is exactly `true`; `false`, `null` and an evaluation error do not | covered (`Policy`, `condHolds`, `Resource.covers`; conditions of the slice 5 fragment, `$created_by` included) | 6 |
| Default deny | covered (`default_deny`, `Tenant.Permits`) | 6 |
| Roles: the system holds none, the anonymous caller `public`, a person its direct roles and `public` | covered (`rolesOf`); a person's team roles are covered in the membership layer (`effective_roles`) | 6 (teams 7) |
| An automation principal holds only its run-as roles | not covered | 15 |
| Conferring a role: only an owner confers `owner`; a role that grants nothing is free; otherwise the conferrer must hold every permission and policy power of it | covered in the membership layer (`mayConfer`, `assign_only_conferrable`, `teamRole_only_conferrable`, `teamMember_only_conferrable`, `redeem_only_conferrable`); not wired into `Intent` or the executable kernel | 7 (spec layer) |
| Capabilities: the owner holds every capability | covered as data (`HoldsCapability`); no modelled intent is gated by one (the gated changes are slices 11 to 13) | 6 |
| Scoped roles (D-8403): a role held through a scope gives its powers where the scope holds | not covered | unassigned |
| Defining roles, permissions and policies by a schema change (`DefineRole`, `DefinePolicy`, `DefineRolePermissions`), and refusing a change that adds a power its definer lacks | not covered | 11 |

### Membership, teams, invitations

From `transition::change_member`, `define_team`, `team_role`, `team_member`,
`create_invitation`, `revoke_invitation`, `redeem_invitation`. Covered as decisions of the membership
layer (`lean/Strata/Members.lean`, slice 7, see its review note), as `Intent` constructors decided
by `step`, run by both executable kernels and compared with the Rust kernel by the differential run
and the `h-` scenarios: the refusals below, the accepted effects, and the owner rule. Refusals in check
order:

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
who started it, past the deadline) and are not part of the workflow decision: slice 15 adds the automation steps they belong to.

### Expressions and constraints (slice 5)

From `expr::eval`, `rules::validate_record` (the constraint loop), `transition::create_record` (the
defaults) and `schema::problems` (the static check of expressions). The fragment is in
`lean/Strata/Model.lean` ("Expressions").

| Decision | Refusal | Model | Slice |
|---|---|---|---|
| The clock: `now`, and a time value in a comparison | `Evaluation` (a kind mismatch) | not covered: the model has no clock and no time value | 10 |
| Roles: `hasRole` | none (`false`) | not covered: the fragment has no `hasRole` (the roles a principal holds are modelled, slice 6) | 6 |
| Arithmetic (`+`, `-`, `*`, `ms_between`, `days`), checked within 2^53 - 1 | `Evaluation` (`integer-overflow`) | not covered | 10 |
| Text functions (`concat`, `len`, `lower`, `graphemes`, slugs, emoji), the 4,096-byte bound | `Evaluation` (`string-too-long`) | not covered | 10 |
| `in`, `contains`, `starts_with` | `Evaluation` on a kind mismatch | not covered | 10 |
| Following a reference (`through`) and `ancestor_of`: a target the snapshot lacks | `InsufficientState` (not a refusal), `Evaluation` for a cycle or a non-reference | not covered: the model reads the whole tenant (see "Snapshot coverage") | 10 |
| Parameters (`param`) | `Evaluation` when unbound | not covered: no parameters in constraints or defaults | 10 |
| System attributes (`$version`, `$created_at`, ...) | `Evaluation` for an unknown name | not covered: the model's records have an owner and a version, not the attributes | 10 |
| A constraint that is not a truth value, or compares two shapes, at definition (`expr::check`) | `InvalidSchema` | not covered: the schema validity predicate is slice 11; the fixtures define only checked constraints | 11 |
| Defining a constraint by a schema change | `InvalidSchema` (names, shapes), `MigrationRejected` (existing records) | not covered: `ChangeSchema` is slice 11; the fixtures define the constraints in the initial tenant | 11 |
| A constraint that reads the old record, or the principal, and so is not state-only | none | not covered: the model admits only state-only constraints (`Expr.stateOnly`) | 11 |

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
| **5 (done)** | Expressions: literals, field reads of the record, the old record and the new record, the principal, comparisons, the three-valued connectives, `isNull`, evaluation errors; constraints (checked last, state-only); expression defaults | 2, 4 |
| **6 (done)** | Roles and policies as data, replacing the fixed tiers: the owner role, role permissions, policies with conditions, capabilities, the system and anonymous principals | 5 |
| **7 (done)** | Tenants, members, teams, invitations, conferring, the owner rule, reserved principal ids. The decisions are in `Strata/Members.lean`, the theorems in `Strata/MembersProofs.lean`, and the intents are in `Intent` and decided by `step`; the executable kernels, the working set, the codec of tenants (Lean and Rust) and the differential harness, scenarios and generator cover them (see the slice 7 review note) | 6 |
| **8 (done)** | Workflows: the workflow of an object type (field, initial states, transitions with guards and assignments), the create and update decisions, the state check and the invariant (`lean/Strata/Workflows.lean`); the executable kernels, the codec (Lean and Rust), the harness with the ticket type (id 4), the h- scenarios and the generator cover them (see the slice 8 review note). Approvals are not in this slice: they decide automation steps, which slice 15 adds | 5, 6 |
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

### Slice 5 review note: what was abstracted

- **The expression language is a fragment of the kernel's.** `Expr` has literals, field reads of the
  record, the old record and the new record, the principal, comparisons (`=`, `!=`, `<`, `<=`, `>`,
  `>=`), `and`, `or`, `not` and `isNull`. It is the set the fixtures and the model's properties use.
  Not modelled: `now` (the model has no clock), `hasRole` (roles are slice 6), arithmetic and the
  text functions, `in`, `through` (following a reference, which needs the other record in the
  snapshot), `ancestor_of`, parameters, system attributes such as `$version`, and the timestamp and
  list kinds. The kernel's static check (`expr::check`) runs on every expression of a schema; the
  model's schema validity is not yet defined, so it takes no part in a write (slice 11).
- **The principal is a string in both.** The kernel names a principal `p<n>`, and its `principal`
  reads that string. The model's principal is a number, and `Expr.principal` reads it as `"p" ++
  toString n`. That is the fixtures' naming (`p100` is principal `100`), and a comparison of the
  principal with a field is a comparison of strings. Any other naming would need its own rendering.
- **Three-valued semantics, as the kernel's `kleene` and `equality`.** A comparison with `null` on
  either side is `null`; a comparison of two different kinds is an error; an ordering is defined for
  integers and strings only. `and` and `or` are Kleene: the first operand that decides (`false` for
  `and`, `true` for `or`) decides the result without its right operand being looked at, so an error
  there is not seen; otherwise `null` is contagious, and an operand that is neither a boolean nor
  `null` is an error. The truth tables are the closed lemmas `and_null_false`, `or_null_true`,
  `and_false_shortcircuit`, and so on (`Strata/Constraints.lean`).
- **Constraints are checked last and the first failure decides.** A write's structural checks
  (`problem`) run first; the constraints run only when they pass, in the order the object type lists
  them (the kernel's `constraints` by id). The first constraint that is `false` gives
  `constraintViolated`, the first that cannot be evaluated gives `evaluation`, and `true` and `null`
  pass. This is the kernel's `validate_record`, whose constraint loop runs only when no earlier check
  failed.
- **The constraints the model admits read only the record.** `Tenant.StateConstraints` says a
  constraint reads neither the old record nor the principal (`Expr.stateOnly`). The kernel's
  constraints may read the old record, which its own migration check does not give; the model's
  invariant needs the restriction, and the fixtures respect it. With it, a constraint's value on a
  stored record is the value a write checked (`constraintProblem_congr`).
- **Defaults are expressions, evaluated before the permission is asked.** A field's default is an
  `Expr` (`.lit v` for a constant), evaluated with no record and the caller as principal. The
  kernel's `create_record` asks `may_attempt(Create)` before it evaluates the defaults (WG-5665), so an
  unauthorized caller is refused `unauthorized` whether or not a default fails. A caller who could create
  and whose default fails gets `evaluation`, as before. The order used to leak: a caller who may not
  create learns that an existing type has a failing default, where an unknown type gives
  `unauthorized`. The model follows the kernel, and `unknown_type_hidden` now takes as a premise
  that the defaults of the type evaluate.
- **Constraints are re-checked by a strict schema change.** The kernel's `plan_migration` runs
  `validate_record` on the stored records of the types it touches, constraints included, so a strict
  change is refused when a stored record violates a constraint. The model's `tableViolated` now says
  so. A lax change keeps the records, and a constraint's value does not depend on the schema, so it
  cannot break one.
- **A delete keeps the constraints by refusal.** The kernel's delete clears a referrer's set-null
  reference and writes the referrer with its version raised. When the referrer would then break one of
  its type's constraints, the delete is refused with `constraintViolated` (`Tenant.clearBreaks`), as a
  restrict refuses it with `stillReferenced`. The invariant `Tenant.Satisfied` is therefore preserved by
  every accepted step, deletes included (`stepTenant_preserves_satisfied`). The rule is chosen (WG-5664)
  and the decisions log does not fix another. `brk_delete_refused` shows the refusal on a concrete tenant.
- **Static checks make an evaluation error unreachable from a schema the kernel accepts.** The
  kernel's `expr::check` refuses at definition any constraint that is not a truth value and any
  comparison of two different shapes (in a constraint or a default). The model's evaluator can
  still fail on those, so `evaluation` is reachable in the model only on schemas the kernel refuses.
  The fixtures therefore contain no such expression, and the h- scenario that needed one was
  removed. The model keeps the error so that its evaluator is total and the schema validity predicate
  (slice 11) has a case to exclude.
- **Changed statements.** `create_fills_default` is about a constant default, `.lit v`.
  `create_refs_resolve`, `write_refused_dangling_across_types`, `create_within_limits` and
  `create_refused_over_record_bound` take the record that the defaults give (`newRecord = .ok r`),
  since the defaults can now fail. `unknown_type_hidden` has the premise above. The `WorkingSet` mirror
  (`decideTenant`, `decideTenant_faithful`) follows the new order, and its `TypeHead` carries the
  constraints.
- **Codec.** `Err` has `constraintViolated` (tag `12`) and `evaluation` (`13`); `FieldDef.default` is
  an expression; `ObjType` lists its constraints after its records; an expression is encoded in
  prefix order (`encExpr`). The Lean decoder takes its fuel from the input length, so it accepts any
  nesting that the input holds. The Rust codec stops at `MAX_EXPR_DEPTH` (256), a deviation the module
  documents: an input nested deeper is refused by Rust and accepted by Lean.
- **Not modelled.** The schema validity predicate (a constraint must be a truth value, and its
  shapes must agree); defining a constraint by a schema change (the fixtures define them in the
  initial tenant; `ChangeSchema` is slice 11); the clock; roles; the string and arithmetic functions;
  the reference-following and ancestor expressions; and the kernel's `Expr::Or`/`And` over lists, which
  the model's binary connectives represent exactly.

### Slice 7 review note: what was abstracted

Slice 7 is the membership layer: the decisions are in `lean/Strata/Members.lean`, the theorems in
`lean/Strata/MembersProofs.lean`, and the intents are constructors of the model's `Intent`, decided by
the model's one `step` (`lean/Strata/Model.lean`). The state is in `lean/Strata/ModelBase.lean`.

- **State.** A tenant holds, besides what it held before, the roles its schema defines (`Tenant.roles`,
  with `owner` and `public` built in), its teams (`Tenant.teams`) and its invitations
  (`Tenant.invitations`). The membership intents see only `MTenant`, the part of a tenant they read and
  write: its members (who hold the direct roles), its grants and policies, its roles, teams and
  invitations (`Tenant.mt`; `Tenant.withM` writes one back and keeps the object types, the records, the
  counter and the limits). A user's effective roles are `public`, its direct roles and the roles of
  each team it belongs to (`MTenant.rolesOf`); `Tenant.rolesOf` is that, so a team's role gives its
  members permissions on records too. The system holds none; an anonymous caller holds `public`.
- **Integration (one `step`).** `Intent` has `createTenant owner name` and `member j` (a
  `MemberIntent`: assign and revoke a role, define a team, give a team a role and take one away, add a
  team member and remove one, create, revoke and redeem an invitation). `stepTenant` decides them
  (`createTenant` against a tenant that exists is `tenantExists`; `member j` is `stepMemberT`, which
  decides on `Tenant.mt` and writes back). `step` first asks `reservedRefused` of the caller and the
  intent, then makes a tenant that does not exist from a system `createTenant` (`createdTenant`) and
  otherwise says `noTenant`, and an intent against a tenant that exists is `stepTenant`'s. The executable
  kernels (`stepI`, `stepX`), the working set (`decideOn`, whose delta is `Delta.members` or
  `Delta.genesis`) and the differential harness all run this `step`.
- **Statements that changed shape.** The meaning of every earlier theorem is kept. `Tenant.rolesOf` now
  includes team roles, so `rolesOf_user` states them; `rolesOf_user_no_teams` is the earlier statement,
  for a tenant with no teams. A refusal theorem about a record or schema intent at the world level
  (`unauthorized_cannot_update`, `unauthorized_cannot_delete`, `missing_record_hidden_on_update`,
  `missing_record_hidden_on_delete`, `version_conflict_on_update`, `version_conflict_on_delete`,
  `create_duplicate_refused`, `update_duplicate_refused`, `define_unique_refused_over_duplicates`,
  `set_unique_refused_over_duplicates`) gains the hypothesis that the caller's id is not reserved
  (`callerReserved c.principal = false`), because `step` refuses a reserved caller before anything else,
  as the kernel does. The world theorems (`step_preserves_wf`, `step_preserves_refs`,
  `step_preserves_satisfied`, `step_isolated`, `step_local`, `step_ok_authorized`) have a case for the
  creation of a tenant (the tenant it makes has no object types, `genesisTenant_wf`); the tenant-level
  ones have a case for the membership intents, which write back `Tenant.withM` and so keep the object
  types, the records and the counter (`stepTenant_member_eq`). `Err` gained `invalidIntent`,
  `unknownRole`, `lastOwner` and `tenantExists`. The fixture's editor holds a policy `manage` on the
  members resource and the fixture's roles are defined (`initialTenant.roles`), so a manager who is not
  the owner is in the fixture.
- **Intents carry the request's time and address.** An invitation's creation and redemption read the
  time of the request, and a redemption the verified address of the caller. The model's `Ctx` is the
  tenant and the principal, so `createInvitation` carries `now` and `redeemInvitation` carries `now` and
  the verified address: the scenario format writes them (`@<now>`, `verified=<address>`) and the
  harness puts them in the kernel's context.
- **Tenants.** A `CreateTenant` is refused `tenantExists` when the tenant exists, and otherwise made by
  the system only (`unauthorized`), with a name that is not blank (`invalidIntent`). The genesis tenant
  has the owner holding `owner`, the two built-in roles, no object types, no teams, no invitations,
  and record counter 1. The name is checked and not stored.
- **Reserved ids.** Principal ids are numbers; the kernel's `team:` prefix and its automation ids are
  one predicate, `reservedId` (ids from `reservedFrom` = 1,000,000 up; the harness writes such a user
  `team:p<n>`). A reserved named principal (of `CreateTenant`, `AssignRole`, `AddTeamMember`) or a
  reserved caller is refused `invalidIntent` before the tenant is looked at, for every intent. Agents,
  which the kernel refuses as tenant owners with its own kind (`AgentAdminPower`), are not generated.
- **Check order.** Each decision follows `transition.rs`: the permission to manage members, then the
  team or invitation, then the role (`unknownRole`, then `public`), then conferring, then the owner rule
  (asked of the state after the change, for removals only). `manageMembers` is the owner, or a policy on
  the `members` resource with `manage` whose condition holds; no grant gives it, as in the kernel.
- **Conferring** (`mayConfer`) is the kernel's `may_confer`: only an owner confers `owner`; a role that
  grants nothing and has no policy power is free; otherwise the conferrer holds every grant of the role
  and every policy power of it (`powerHeld`, the kernel's `holds_policy_power`).
- **The owner rule** asks whether some user holds `owner` directly or through a team after the change,
  as the kernel's `effective_owners` does; a tenant with no owner before is not held to it.
- **Invitations.** The address is trimmed and lowercased and must contain `@`. Expiry is the intent's
  time plus fourteen days in milliseconds. Redemption asks the verified address of the caller (the
  kernel's `verified_address`), that the invitation is unredeemed, unrevoked and unexpired, and that the
  issuer still manages members and confers every role offered; the redeemer gains the roles.
- **The executable kernels** decide a membership intent with the same decision as the model on the
  tenant's membership (`ITenant.mt`, `ITenant.withM`), which is not indexed by record, so their
  indexes are untouched (`member_good`); the refinement theorems (`stepTenantI_refines`,
  `stepTenantX_refines`, `stepI_refines`, `stepX_refines`, `stepI_inv`, `stepX_ok`) cover every intent.
  A creation makes the genesis tenant with empty slots and empty indexes (`createdTenantI_abs`,
  `createdTenantX_abs`, `genesis_ok`).
- **The working set** carries the membership in its header (`Header.roles`, `teams`, `invitations`), decides
  a membership intent from the header alone, and commits it as `Delta.members`; a creation is
  `Delta.genesis`. `touchedIn` invalidates the header for both (and the counter and the tables for a
  creation), and `touchedIn_sound` covers them (`sound_members`, `sound_genesis`).
- **The codec** carries a tenant's roles, teams and invitations after its limits, the intents `createTenant`
  (tag 6) and `member` (tag 7, with its own tag), and four more refusals (tags 14 to 17). The Rust
  mirror (`crates/lean-kernel-ffi/src/codec.rs`) reads and writes the same bytes; the fixtures
  `fix_tenant`, `fix_tenant_members`, `fix_policy_members`, `fix_intent_*` and `fix_err_*` are checked on
  both sides.
- **The harness.** `lean/Main.lean` reads `create-tenant`, `assign`, `revoke`, `team`, `team-assign`,
  `team-revoke`, `team-add`, `team-remove`, `invite`, `invite-revoke` and `redeem` lines;
  `crates/verification/src/lean.rs` maps each to the kernel's intent (role numbers 0 to 3 are
  `owner`, `public`, `editor`, `writer`, any other number an undefined role; teams are `team<n>`,
  invitations `inv<n>`), and a tenant's state ends with its membership lines when it is not the
  fixture's (always for tenant 3, which is printed only when it exists). The generator
  (`lean_diff::generate`) mixes random membership operations with scripted lifecycles, and writes the
  roles of an invitation in the kernel's order (by role id text), so the model's order-given rule
  names the same bad role. Eighteen `h-` scenarios cover each decision.
- **Abstractions.** Whitespace is the ASCII space, tab, line feed and carriage return (`trimWs`,
  `isBlank`), where the kernel trims Unicode whitespace. A role list is taken in the order given; the
  kernel's set is ordered, so the generator writes roles in that order. The tenant's name is checked and
  not stored. A removal's sessions are not modelled. Separations of duties, scoped roles (D-8403),
  staging roles and products are not modelled. An invitation's expiry is a number rather than an
  optional timestamp, so a legacy invitation with no expiry is not represented.
- **What was checked.** The model builds with no `sorry`; the axiom audit lists only `propext`,
  `Classical.choice` and `Quot.sound`; the existing fixtures print the same text from the specification
  (`strata_model`), the executable kernel (`--impl`) and the indexed kernel (`--indexed`), and that text
  is `lean/fixtures/expected.txt`. The Rust sides (codec mirror, scenario mapping, generator) were
  written by reading and are compared with the model by `cargo test -p strata-verification` and the
  differential run.

### Slice 8 review note: what was abstracted

Slice 8 is workflows. The state is in `lean/Strata/ModelBase.lean` (`Workflow`, `WfTransition`, `ObjType.workflows`,
`fireOne`, `fireAll`, `initialsOk`, `statesOk`), the decisions in `Strata/Model.lean` (the create and update branches of
`stepTenant`, which follow `transition::create_record` and `update_record`), the theorems in `Strata/Workflows.lean`,
their list lemmas in `Strata/WorkflowLemmas.lean`. The executable kernels, the working set, the codec and the harness
cover the decisions.

- **State.** An object type holds its workflows, in the kernel's order. A workflow names a field, its initial states (a
  list; the kernel keeps a set) and its transitions. A transition has a source and a target state, an optional guard and
  a list of assignments with distinct fields. The kernel's workflow id and transition names are not modelled (they name
  events, slice 9). The states of a workflow are its initial states and the ends of its transitions (`Workflow.states`).
- **Create.** After the permission and the defaults, and before the other checks of the record, every workflow's field
  must hold one of its initial states (`initialsOk`). Any other value, an absent field included, is `workflowViolation`.
- **Update.** The order is the kernel's: the permission on the patched record, the expected version, then the workflows in
  order. A workflow whose field keeps its state does nothing. Otherwise the first transition from the old state to the new
  one whose guard is `true` is taken (`findTransition`); a guard that is `false`, `null` or cannot be evaluated is not a
  match. Its assignments are all evaluated against the stored record (as `record` and `old`) and the candidate (as `new`);
  the first evaluation error is `evaluation`, otherwise they are written in order, and a `null` removes the field. No match
  is `workflowViolation`. When the workflows changed the record, the permission is asked again for the record as it now
  is. Then come the structural checks, the state check (a string in a workflow field must be a state, else
  `workflowViolation`) and the constraints.
- **Abstractions.** Guards and assignments are the slice 5 fragment, evaluated over the record only (no clock, parameters,
  lookups or `through`). Approvals are not in this slice: `approvals::decide` decides a waiting automation step, a separate
  feature that slice 15 adds. Computed fields, events and the kernel's refresh of computed referrers are not modelled. The
  kernel's static checks of a workflow (its field is a string, its transitions name states, its expressions check) are the
  schema validity of slice 11 and are not part of the model; the fixture's workflow satisfies them. Schema changes do not
  define or remove workflows (slice 11).
- **Findings.** Three things the kernel does, which the model reproduces and the theorems state.
  1. A transition may assign the workflow field itself, or another workflow's field, so a field can change without that
     workflow's transition: the kernel checks that the field holds a state after the assignments, not that a transition
     reached it. `WfIndependent` (no transition assigns a workflow field) is the hypothesis of the theorems that need the
     transition to be the cause (`update_changed_fired`, `update_undeclared_refused`).
  2. A transition may assign `null` to the workflow field. The kernel's state check looks only at string values, so the
     stored record can then have no state. `statesOk` is stated the same way, so the invariant is that a string state is
     one of the states. Whether a record without a state is wanted is for the schema's owner to decide.
  3. In a statically checked workflow, an assignment cannot raise an evaluation error: the kernel refuses at definition an
     assignment that compares a string with an integer, and the fragment has no other runtime error. The model keeps the
     `evaluation` refusal (`evalSets`); the fixture, which must satisfy the kernel's checks, has no scenario that reaches it.
- **Statements that changed shape.** Each change is a hypothesis or a stronger refusal; the meaning of each earlier
  theorem is kept for the write it describes. These theorems describe the patched record as the record an update writes
  (`update_writes_patched`, `update_keeps_unnamed`, `update_ok_parts`, `update_refs_resolve`,
  `update_refused_dangling_across_types`, `update_within_limits`). They gain the hypothesis `Tenant.Plain` (no transition
  assigns a field): an assignment makes the written record differ from the patch, so the statement is false without it.
  The general statements use the record the workflows leave (`update_ok_parts_gen`). The refusal theorems of an otherwise
  valid write gain the workflow checks the write must pass: `create_refused_over_record_bound`, `create_duplicate_refused`
  and `create_constraint_refused` take `initialsOk`; `update_duplicate_refused` and `update_constraint_refused` take that
  the workflows leave the patched record as it is and its states hold. `update_raises_version` keeps its statement for every
  tenant. `Err` gains `workflowViolation` (codec tag 18).
- **Invariant.** `Tenant.StatesOk` is separate from `Tenant.WF`, so `WF` and its theorems keep their statements.
  `step_preserves_states` takes the distinct field names of the stored records (`Tenant.Distinct`, part of `WF`), which the
  clearing of references in a delete needs (`get_clearFields_eq`).
- **Executable kernels.** `stepTenantI` and `stepTenantX` decide the same checks with the spec's functions on the records
  (`fireAll`, `initialsOk`, `statesOk`). The workflows ride in the object type of the implementation (`IObj`), and no index
  covers a workflow field, so the indexes are untouched. `stepParts` in `XStep` and the working set's `decideTenant` take
  the same decision.
- **Harness.** The fixture gains a ticket type, id `4` (`3` names an unknown type in an existing scenario). Its fields are a
  state (`1`, default `open`), a vote count (`2`) and a note (`3`). Its workflow is in `lean/Strata/Fixture.lean` and in
  `crates/verification/src/lean.rs`; its policies let a writer change only its own tickets with at most five votes.
  `strata_model` prints the type's block only while it holds a record, so the existing scenarios print as before. The
  generator (`lean_diff::generate`) emits ticket operations in one iteration of six. Thirteen h- scenarios cover the
  decisions, from `h-a-new-ticket-starts-open` to `h-deleting-a-ticket-needs-no-transition`. `expected.txt` is not edited.
- **Checks.** `lake build` builds. `lake env lean Audit.lean` prints only `propext`, `Classical.choice` and `Quot.sound` for
  the new theorems and finds no `sorryAx`. `strata_model`, `--impl` and `--indexed` print the same text on
  `fixtures/scenarios.txt`, and the scenarios that existed before print the lines of `expected.txt`. The Rust sides (codec,
  fixtures, harness, generator) were written by reading and are not compiled or run here.
- **Not covered.** Approvals (slice 15); events and transition names (slice 9); the schema changes that define or remove
  workflows, and the static validity of a workflow (slice 11); computed fields (slice 10). A later assignment overwriting an
  earlier workflow's assignment is the record the step writes, stated by `update_writes_fired`, not as a theorem about
  each field.

### Slice 6 review note: what was abstracted

- **Principals are a sum type.** `Principal` is a user (its id), the system or an anonymous caller.
  The system holds no role; an anonymous caller holds `public` and nothing else; a user holds its
  direct roles and `public` (`Tenant.rolesOf`, the kernel's `roles_of` without teams and automation).
  `Expr.principal` reads a user as `p<n>` and reads null for the system and for an anonymous caller,
  as the kernel does. A record's creator is `Record.owner`, which is none when no user created the
  record; `Expr.createdBy` reads it as `p<n>` or null, the kernel's `$created_by`.
- **Roles are numbers, two of them built in.** `ownerRole` (0) may do everything; `publicRole` (1)
  is held by every user and by the anonymous caller. The fixture's editor is role 2 and its writers
  are role 3. A role has no definition in the model: it is named by the members, grants and policies
  that name it. Defining a role by a schema change is slice 11.
- **One decision, without scopes.** `Tenant.allows` is the kernel's `authorize` with the scoped roles
  (D-8403) and the clock and the lookups removed. In order: the owner role; a grant of a role the
  principal holds on the object type (never for `manage`); a policy of such a role whose action and
  resource match and whose condition is exactly `true`. Default deny. `Tenant.Permits` is the same
  decision as a proposition, and `allows_iff` says they agree.
- **The environment of a decision is the kernel's subject.** A create's condition reads the candidate
  record. An update's reads the stored record as `record` and `old` and the patched record as `new`.
  A delete's reads the stored record in all three. A decision with no record (a missing record, an
  unknown type, a schema change) reads none, so a condition on a field of it is null and does not
  hold. A condition that is null or an error does not permit.
- **The order.** A create asks the permission to create on the type before it evaluates the defaults,
  as the kernel does since WG-5665 (`create_unauthorized_regardless_of_defaults`); the unknown type and the missing record are
  decided on the action and the type, without a record.
- **Capabilities are data.** A role holds a capability through a grant (`Grant.capability`); the owner
  holds every capability (`HoldsCapability`). No intent of the model is gated by a capability: the
  kernel's capability checks are on definitions that slices 11 to 13 add (automations, settings, holds,
  separations, the ledger, products) and on the role changes of slice 11.
- **The schema permission** is `manage` on the schema (`canManage`). Only the owner holds it in the
  fixture, so a schema change by anyone else is still refused `unauthorized`.
- **The fixture's roles** are the Rust fixture's: `editors_all`, `writers_create`, `writers_own` and
  their `part_` counterparts, with the same conditions. The one new policy is the public role's: it
  creates a part when `isNull(principal)`, which only an anonymous caller satisfies. Existing scenario
  outputs are unchanged. The expected output of the whole file is regenerated by `bless_fixtures`, not
  here.
- **The writer's rule is checked on concrete records.** The general statement that a writer may update
  a record exactly when its creator is that writer needs `Nat.repr` to be injective, and the library
  names no such lemma. The fixture theorems `fixture_writer_updates_own` and
  `fixture_writer_refused_other` decide the rule on two records instead.
- **Statement changes.** `canEdit`, `canEditAny` and `canCreate` are replaced by `canUpdate`,
  `canDelete`, `canCreate` (over the candidate record), `canWithoutRecord` and `canManage`. `unknownType`
  and `missingRecord` take the type and the action. `step_ok_authorized`, `unauthorized_cannot_update`,
  `missing_record_hidden_on_update` and the other authorization theorems keep their statements in
  substance, restated over these decisions; `unknown_type_hidden` takes the permission for each action
  and, for a create, for every candidate record. No statement was weakened.
- **The executable kernel** takes the header (members, grants, policies) and calls the model's
  decisions on it, so its permission tests are the model's, and its refinement theorems are as before.
- **Not modelled.** Scoped
  roles (D-8403, unassigned); automation principals and their run-as roles (slice 15); defining roles,
  permissions and policies by a schema change (slice 11); `hasRole`, the clock, a condition that reads
  through a reference, and the capability-gated changes (slices 11 to 13); reads, whose visibility uses
  the same decision (slice 10).

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
