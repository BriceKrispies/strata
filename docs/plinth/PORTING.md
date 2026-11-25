# Porting a Plinth module to a Strata product

This is the recipe for taking one Plinth module from the inventory to a Strata product that
compiles with `specc bundle`, installs into a test tenant and has its screens. The generator does
the mechanical part. The steps below say what is left, and in what order.

Inputs: `docs/plinth/inventory/<group>.json` (every module item with `strata.status`,
`strata.how`, `strata.fundamentals`), `docs/plinth/fundamentals.json` (the canonical fundamentals and
their aliases), and the AUTHZ matrix in `docs/plinth/inventory/g4-governance.json`.

Terms used below:

- **live**: a declaration the generator emits as a real product entry.
- **PLANNED F-x**: a commented declaration blocked on fundamental F-x. Look up F-x in
  `fundamentals.json` (its `title`, `neededBy`, `existingTask`).
- **MANUAL**: a commented declaration that needs a human decision. The reason is on the line.
- **HAND-FINISHED**: a product file whose first line says so. The generator keeps such a file as it
  is (see step 5).

## Steps

1. **Pick the module and run the generator.** `node docs/plinth/gen-product.mjs <ID>` (for example
   `M10`, `AUTHZ`, `REPORTING`). It writes `docs/plinth/products/<id lower-case>.jsonc` and prints
   the live, planned, manual and out-of-scope counts with the fundamentals that block the planned
   items. Check: the file is new or identical to what was there (a second run changes nothing).
   Rank the modules with `node docs/plinth/gen-product.mjs --all`, which prints the table and
   regenerates every module.

2. **Check the file starts with the generated header**, not `HAND-FINISHED`. If it is hand-finished,
   the generator will not overwrite it; any later generator change then has to be applied by hand.
   Prefer step 5's generator route to hand edits.

3. **Compile.** Build the compiler once: `CARGO_BUILD_JOBS=2 cargo build -q -p strata-spec-compiler --bin specc`.
   Then `target/debug/specc.exe bundle --spec specifications/spec.jsonc --product docs/plinth/products/<id>.jsonc --out <scratch dir>`.
   Expected output: `wrote 1 files to ...`. Any `error[...]` line is a generator defect or a
   hand-finish gap: fix the generator, regenerate, and compile again. Do not hand-patch around it.
   Common errors seen so far and their generator fix are listed under "Generator history" below.

4. **Resolve each PLANNED block only after its fundamental has landed.** Check the fundamental in
   `fundamentals.json` and its Workgraph task (`existingTask`, or the `task` field) for accepted work.
   When it has landed, the item's `strata.status` in `docs/plinth/inventory/<group>.json` changes to
   `expressible` or `partly` (no fundamental), the inventory is merged as usual, and the generator
   regenerates the live declaration. Do not uncomment a PLANNED block by hand: that makes a
   hand-finished file (step 5).

5. **Hand-finish only what the generator cannot yet produce.** If a live declaration is still
   needed and the generator cannot make it, write it into the product file, put `HAND-FINISHED: <why>`
   on the first line, and list the change in the module's entry under "Proof log" below. A
   hand-finish that recurs across modules becomes a generator change (step 5 is then removed for
   that case).

6. **Resolve MANUAL blocks.** Each one is a decision the generator will not take:
   - **Preconditions** (prose): write `conditions` entries over `param.*`, `record.*`, `principal`,
     `now`, with an `error` code and a `message`. Check each with `specc bundle`.
   - **Effects** (prose): steps are emitted only for "insert/create/add <entity>" effects, and only
     when every required field of that entity is an input. Write the other steps by hand:
     `UpdateRecord` (needs the record expression), `DeleteRecord`, `PostLedger` (needs the unit, pool and
     owner, from the ledger design), `WriteSetting`. Ledger postings use the ledger units declared in
     the product (see `docs/plinth/products/*` for the shape, and `promotions.jsonc` in
     `crates/spec-compiler/tests/fixtures/products/` for a worked example).
   - **Personal fields**: declare `personal` with a kind and a subject, or record why not.
   - **Approximate types** (`jsonb`, `bytea`, `set<...>`, exact decimals): choose the Strata type and
     the scale; decimals need an integer unit.
   - **Enums on nullable fields**: no constraint is generated; write one that allows null if needed.
   - **Events and invariants** (the `CHECKS` block at the end of the file): see step 9.
   - **Settings** that are not plain quoted strings: declare the type and the default by hand.
   - **Queries** with a filter, sort or aggregates: write the `filter` (tenant rule language),
     `order` and `aggregates` declarations.
   - **Views and screens**: a widget becomes a section only when its `data` names a live query.
     Other widgets stay MANUAL; create the query, or drop the widget.
   - **Lifecycle**: check the `status` field the generator added, if it added one (the MANUAL
     note says so), and check the transition names.
   - **Approvals** (F-held-operations): a change that needs a second person's decision is a hold
     rule in the section `holds`: `{ "id": "<product>__hold_x", "label": "...", "target":
     { "Operation": { "operation": "<product>__x" } }, "when": "param.amount > 100", "approver":
     "<capability>" }`. The target is an operation, `{ "Role": { "role": ... } }` (every way to
     confer the role) or `{ "Setting": { "setting": ... } }`; `when` is optional and reads the
     inputs as `param.<name>` (`principal`, `team`, `role` and `action` for a role, `setting`,
     `level`, `team` and `value` for a setting); the approver is a capability a role of the product
     grants. The held change is decided through `decide-hold` (`/internal/holds/decide`), never by
     a policy. See `crates/spec-compiler/tests/fixtures/products/hold-rule.jsonc`.

7. **Roles and policies.** Roles come from the AUTHZ matrix (agent roles are left out). Check the
   `holders` and `capabilities`. Each product declares its own roles: a product may not name another
   product's roles (cross-product references are planned on F-cross-product-refs). Policies are
   derived per role and object and are planned on F-visibility: the row scope is not derivable.
   Write them by hand with the row conditions, and check each one against the inventory's
   `rowPredicate` for the permission it serves. This is a security review, not a mechanical step.

8. **Presentations.** The generator emits a presentation per live object whose name-like field
   exists (`name`, `title`, `label`, `subject`). Fill the `fields` map: how each field displays
   (`display`, `props` for states). Objects without a name-like field are MANUAL.

9. **Invariants are tests.** Each invariant id in the `CHECKS` block must be covered by a test, or
   recorded as a backlog task (Workgraph, tag `plinth`). Use the invariant `statement` for the test
   name. Invariants marked `security` need a test before the module is installed.

10. **Install into a test tenant.** The corpus loader is `crates/corpus`. Put the product file in
    the corpus directory the loader reads (`--corpus <dir>`; the generated products are in `docs/plinth/products`).
    One module, into a fresh tenant of a store that outlives the process:
    `cargo run -p strata-corpus -- load --store sqlite:var/plinth-proof.db --tenant <id> --corpus docs/plinth/products --product <file stem>`
    (the tenant id is letters, digits and hyphens without a leading hyphen, as the loader checks it).
    Without `--product`, `load` installs every product of the directory. Expected: the product loads, and the
    verification line reports the owned entities present with 0 missing. `strata-corpus check --product <product id>`
    (for example `plinth-store`) installs the product alone on a fresh in-memory tenant and lists every kernel problem. Check the installed
    tenant's bundle hash (`strata-corpus hash --store <store> --tenant <id>`) changes when the product changes.

11. **Check the screens.** Each live screen needs: a `title` and a `route`, a landmark (one `main`
    region per screen), a heading in order, a label on each form field and an accessible name for
    each control. The a11y checks are the `ui.a11y.*` codes: `landmark`, `heading-order`,
    `field-label`, `error-association`, `unnamed-control`, `live-region`, `image-alt`, `contrast`,
    `target-size`, `reduced-motion`, `gesture-keyboard`, `gesture-pointer`. Run the accessibility
    gate when it lands (F-accessibility-gate). Until then, check each screen against these codes by
    reading the design-system spec (`specifications/design-system.jsonc`).

12. **Record the module in the proof log** below, with what was hand-finished and why, the counts
    from step 1, and the compile result from step 3.

## Generator history

Compile failures found during the port, and the generator change that fixed each:

- A reference, a create or a query naming a planned or manual entity left the name undeclared
  (`layer.reference-unknown`, seen on `m1` and `m18`). Only live entities now name each other
  (`entityIds` in `emitModule`, `createTarget`, `findQueryObject`).
- A role and an entity from the same AUTHZ module shared an id (`plinth_authz__role_member`), so
  `layer.redefines-entity` on `authz`. Roles now use `role__<name>`.

Readability change, not a compile failure: invariants and events are in a `CHECKS` comment block at
the end of each product file, not among the operations.

Rules added with the M4 proof (each one recurs across modules; the rule and the defect it addresses):

- A `PRIMARY KEY (a, b)` pseudo-field was emitted as a column. It is now a `unique` key over the named
  fields (`primaryKeyOf`), so `cart_items` and `wishlist_items` keep their key.
- A `workspace_id` column is not declared: the tenant is the workspace. Its references to `workspaces`
  were MANUAL and held as text on every table of every module that has one (`fieldDecls`, `buildOperation`).
- A uuid input named `<noun>_id` that names a live entity is a `Ref` to it (`entityNamed`). An
  `Update <noun> ...` effect becomes an `UpdateRecord` on that reference input, with the fields the input
  of the same name sets; a `<name>_at` timestamp is set to `now`; a `<name>_by` or principal field to
  `principal`. A field the rules cannot set keeps the effect unmapped (`updateTarget`).
- An effect that names an event (`review.created`, `order.status_changed ...`) is a notice, not a step; the
  events section declares it, so it no longer makes the operation manual (`EVENT_EFFECT`).
- A precondition of the form `<input> <op> <input or number> (CODE)` becomes a condition with the code as
  its error (`simpleCondition`). Other preconditions stay prose, as before.
- An enum input admits only its listed values: a condition `param.<name> in [...]` (`buildOperation`).
- Runner: `strata-corpus load --product <file stem>` installs one product of a directory, so one module can
  be proven in a fresh tenant without the other products (`crates/corpus/src/main.rs`).

## What remains manual, by class

These are what the generator does not decide, and what steps 6 to 9 cover:

- operation conditions from prose preconditions (step 6);
- operation steps other than create-record and ledger postings (step 6);
- row scope on policies and on operations with a row predicate or owner-only (step 7);
- presentations' display choices (step 8);
- invariant tests (step 9);
- personal-data kinds and subjects (step 6);
- approximate types (step 6);
- events and notices, as triggers or notice templates (step 6);
- lifecycle field and transition names (step 6).

## Proof log

The ranking is `node docs/plinth/gen-product.mjs --all`. Its score is live objects plus live operations
plus live screens; planned items do not add to the score, and they break ties (fewest planned first).
The table prints both counts (objects and operations) and the planned, manual and out-of-scope totals.
The first rows, as generated after the rules below were added:

| rank | module | score | objects | operations | screens | planned | manual |
|---:|---|---:|---:|---:|---:|---:|---:|
| 1 | M4 | 9 | 8 | 1 | 0 | 48 | 10 |
| 2 | M1 | 4 | 4 | 0 | 0 | 20 | 12 |
| 3 | M2 | 3 | 3 | 0 | 0 | 19 | 23 |
| 4 | M7 | 3 | 3 | 0 | 0 | 37 | 36 |
| 5 | AUTHZ | 3 | 3 | 0 | 0 | 83 | 33 |

M4 is the module proven (below). Its operation count is the generator's view: one operation is generated
live, and three more are hand-finished, so the proof has four live operations in all. MCP, the other
candidate, has no live object or operation after the rules, so it cannot meet the three-and-three bar.

- **M4** (`docs/plinth/products/m4.jsonc`, first line `HAND-FINISHED`). Live: 8 objects (generated),
  4 operations (1 generated, 3 hand-finished), 3 ledger declarations (hand-finished), 4 roles.
  Compile: `specc bundle --product docs/plinth/products/m4.jsonc` writes one file, as generated and after the
  hand-finish.
  Install: `CARGO_BUILD_JOBS=2 cargo run -q -p strata-corpus --bin strata-corpus -- load --store sqlite:var/plinth-proof.db --tenant plinth01 --corpus docs/plinth/products --product m4`.
  Output: `installed 1 products into tenant plinth01: 23 entities owned, schema version 2, 2 commits`, then
  `plinth-store v0.1.0 23 entities`, then `verified 23 owned entities present (0 missing, 1 not checkable)`.
  Tenant ids cannot contain a hyphen, so the tenant is `plinth01`.
  Hand-finishes, and whether each became a rule:
  1. `wishlist.add` and `wishlist.remove` take the product as a reference input (`product`, a Ref to
     `plinth_store__product_inventory`), not the slug. The find step looks up the member's wishlist row; `add`
     creates it in the unmatched branch, `remove` deletes it in the matched branch. Not a rule: changing a
     slug into a record reference changes the operation's contract with its callers, which is a decision.
     The same shape recurs in `cart.set_item` and `store.review_create`, so the decision is owed once for all.
  2. The "product exists" precondition of `wishlist.add` is dropped: a Ref input already names an existing
     record, so there is nothing to check.
  3. `inventory.adjust` takes the product as a reference input and posts the stock change to a ledger unit
     (`plinth_store__stock`, pool `plinth_store__stock_pool`, floor 0). The first version wrote
     `on_hand = on_hand + delta` on the record, and specc refused it with `layer.balance-outside-ledger`: a
     count that moves up and down is a ledger unit. Not a rule yet: choosing the unit, the pool and the
     floor is a ledger design decision (PORTING step 6, `PostLedger`), and it recurs wherever a table holds
     a count (`product_inventory.on_hand`, `inventory_movements`). The generator should propose it once the
     decision is made for one module.
  4. The `inventory_movements` row of `inventory.adjust` is not emitted: that table is `partly`, blocked on a
     fundamental, so it stays PLANNED in the file's comments.
  5. Generator defect found while porting, now a rule (below): the `PRIMARY KEY (...)` pseudo-field of
     `cart_items` and `wishlist_items` was emitted as a column named `primary_key`.

Caveat: the M4 product installs into a fresh tenant and its objects and operations are in the tenant. The
product has no screens and no policies yet (policies are planned on F-visibility), so the screen and row-scope
parts of step 11 and step 7 are not proven here.

## Where the generator stands

Twenty-three modules are generated as before. M4 is hand-finished (its file is kept by the generator, as the
recipe says). An operation is live when the inventory marks it expressible without a fundamental, it has no
row scope, and every one of its effects and preconditions maps to a Strata step or condition (see the rules
under "Generator history"). An object is live when the inventory does not block it; its approximate-type and
personal fields stay live with a MANUAL note beside them. So a module's live count is small by design.
