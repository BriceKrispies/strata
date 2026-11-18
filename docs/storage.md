# Persistence

The rest of Strata knows one thing about storage: the `Store` trait in
`crates/storage-api/src/lib.rs`. It is written in Strata's terms and mentions no SQL.

## What the contract requires

| Guarantee | Meaning |
|---|---|
| Atomic commit | `commit` applies the state delta, the events, the journal entry, the counters and any session revocations together, or nothing |
| Conflict detection | A commit applies only if the tenant is at the delta's base revision (or absent, for a creation). Otherwise `Conflict`, and nothing changes |
| Serialisable commits per tenant | Each commit sees the state the previous one left. A reader never sees part of a commit |
| Tenant scope | Every operation names a tenant; data of one is never returned for or changed through another |
| Ordered events | A gapless per-tenant sequence, and a global outbox position that follows commit order |
| Outbox | Events stay pending until `mark_published` |
| Snapshot | `create_snapshot` then `restore_snapshot` yields a state with the same canonical hash |
| Durability | What `capabilities().durable` says |

A snapshot load takes a list of `Need`s (one record, records by field value, all of an object
type) and returns at least the records that answer them, with coverage stating exactly those
needs. The tenant's schema, members and counters always come along.

Databases differ, and the contract does not pretend otherwise. `Capabilities` reports the
adapter's name, whether it is durable, how it orders concurrent commits and what a reader can
observe. The diagnostics endpoint shows it.

## The two adapters

**Memory** (`storage-api/src/memory.rs`): one lock, plain maps. The contract in its simplest form.
Not durable. Used by tests and by `--store memory`.

**SQLite** (`storage-sqlite/src/lib.rs`): a file, one connection behind a mutex, `BEGIN
IMMEDIATE` write transactions, WAL journal, `synchronous = FULL`, foreign keys on, a five second
busy timeout.

| Table | Holds |
|---|---|
| `tenants` | one row per tenant: revision, counters, schema and members as JSON |
| `schema_history` | every schema version with the revision that introduced it |
| `records` | one row per record: `(tenant, object, id)`, version, the record as canonical JSON |
| `record_index` | one row per scalar field value and per list element, for lookups by value |
| `events` | the event log; `published = 0` rows are the outbox |
| `journal` | every accepted transition: intent, context, delta hash, events hash |
| `sessions`, `accounts` | server-side sessions (token hash only) and development logins |
| `snapshots` | complete tenant states with their hash |
| `storage_migrations` | which steps of the layout history have been applied, and when |

Tenant structure is data, so there are no per-tenant tables and no DDL when a tenant changes its
schema. The layout itself changes only by appending to `MIGRATIONS`.

## Adding a database

1. Create a crate that depends on `strata-storage-api` and `strata-kernel`.
2. Implement `Store`. `check_event_sequence`, `partial_state` and `field_matches` are provided.
3. Run the conformance suite against it:

   ```rust
   #[test]
   fn conforms() {
       strata_storage_api::conformance::run(&|| Box::new(MyStore::fresh()) as Box<dyn Store>);
   }
   ```

   with `strata-storage-api = { features = ["conformance"] }` in dev-dependencies. The suite has
   thirteen checks: stored state equals kernel state by hash, stale commits conflict, malformed
   commits are refused whole, partial loads, list containment, isolation, outbox order, journal
   replay, sessions, accounts, snapshots, reset.
4. Add the adapter to the `match` in `crates/backend/src/main.rs`.

Nothing else changes: not the kernel, not tenant metadata, not the BFF, not the browser.

### Notes for PostgreSQL

- Give each tenant's commit a row lock on its `tenants` row (`SELECT ... FOR UPDATE`) and compare
  the revision under it; that is the conflict check and the per-tenant serialisation in one.
- The outbox position must follow commit order, not sequence allocation order. A plain sequence
  can hand out a lower number to a transaction that commits later. Either serialise the position
  assignment or publish by `(commit timestamp, id)` with a safe horizon.
- `record_index` can become a GIN index on a `jsonb` column.
- State what isolation level the adapter runs at in `Capabilities`; the conformance suite checks
  behaviour, not the label.

## Limits

- Named queries load every record of their object type and filter in the kernel. Correct, and
  fine at this size; pushing filters down to the store is future work and needs a way to prove
  the pushed-down filter equal to the kernel's.
- One connection means reads wait for writes inside the backend process.
- Snapshots are whole copies.
