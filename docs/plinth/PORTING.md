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
    the corpus directory the loader reads (`--corpus <dir>`), then run `cargo run -p strata-corpus -- check --product plinth-<slug>`
    (the product id is `plinth-<slug>`), and `cargo run -p strata-corpus -- load --store memory --tenant <id>`.
    Expected: the products load with no error. Check the installed tenant's bundle hash
    (`strata-corpus hash --tenant <id>`) changes when the product changes.

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

Two modules with the fewest planned items, from `node docs/plinth/gen-product.mjs --all` (the
table is printed in full there; the ranking is by planned items, then by module id):

| module | title | live | planned | manual | blocked by |
|---|---|---:|---:|---:|---|
| COND | Condition grammar: CondTree, facts and the pure evaluator | 0 | 0 | 9 | - |
| M17 | Builder catalogue | 0 | 1 | 4 | F-visibility 1 |

- **COND** (`docs/plinth/products/cond.jsonc`): compiles with `specc bundle` as generated. Hand-finished: nothing.
  Why: the module has no stored entity, operation, query or screen. Its two entities are read models
  (`kind: view`, MANUAL), and its seven invariants are MANUAL (tests, step 9). It declares no product
  content, so the bundle is empty.
- **M17** (`docs/plinth/products/m17.jsonc`): compiles with `specc bundle` as generated. Hand-finished: nothing.
  Why: the catalogue is a read-only description of the product's own schema (`kind: view`), which
  Strata answers with the platform's `describe` intent, not with a product declaration. Its one
  query has no object, so it stays MANUAL. The one PLANNED item is invariant M17-I2 on F-visibility
  (which sources a member may see).

Caveat: the planned-count ranking selects modules that declare nothing. Both proofs compile, but
neither exercises an object, an operation, a view or a screen, so they show that the generator's
output is accepted, not that a module installs and has its screens. The next proof should use the
lowest-ranked module with live operations (for example MCP, which has one live operation and
twelve planned items), once its MANUAL items are resolved in steps 6 to 9.

## Where the generator stands

All 24 modules compile with `specc bundle` as generated, with no hand-finish. An operation is live only when the
inventory marks it expressible without a fundamental, it has no prose precondition, no effect
beyond a create, and no row scope. An object is live when the inventory does not block it; its
approximate-type and personal fields stay live with a MANUAL note beside them. So a module's live
count is small by design.
