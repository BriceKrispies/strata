# Persistence

The rest of Strata knows one thing about storage: the `Store` trait in
`crates/storage-api/src/lib.rs`. It is written in Strata's terms and mentions no SQL.

The prototype stores every tenant in one SQLite file. The specification sets a different target:
one PostgreSQL database per tenant (`tenant-database`, `postgres-adapter`, requirement `ten-014`),
with a control database and backups outside it. The `Store` contract is expected to carry over,
but the layout, the tables below and the single-connection design are the prototype's only.

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

## The adapters

**Memory** (`storage-api/src/memory.rs`): one lock, plain maps. The contract in its simplest form.
Not durable. Used by tests and by `--store memory`.

**SQLite** (`storage-sqlite/src/lib.rs`): a file, one connection behind a mutex, `BEGIN
IMMEDIATE` write transactions, WAL journal, `synchronous = FULL`, foreign keys on, a five second
busy timeout.

There are two kinds of database file. The control database holds `tenant_directory` and
`session_directory`, and its ledger `control_migrations`. Each tenant has a file of its own, with
the ledger `tenant_migrations`. A tenant file holds, among others:

| Table | Holds |
|---|---|
| `tenant` | the tenant's row: revision, counters, schema and members as JSON |
| `schema_history` | every schema version with the revision that introduced it |
| `records` | one row per record: `(object, id)`, version, the record as canonical JSON |
| `record_index` | one row per scalar field value and per list element, for lookups by value |
| `events` | the event log; `published = 0` rows are the outbox |
| `journal` | every accepted transition: intent, context, delta hash, events hash |
| `sessions`, `accounts` | server-side sessions (token hash only) and logins |
| `snapshots` | complete tenant states with their hash |

The sign-in methods keep further tables (throttle counters, passkeys, emailed links, identity-provider
and SAML flows, second factors, recovery codes). The complete layout is the `CONTROL_MIGRATIONS` and
`TENANT_MIGRATIONS` lists in `storage-sqlite/src/lib.rs`; this table is a summary and the lists win.

Tenant structure is data, so there are no per-tenant tables and no DDL when a tenant changes its
schema. The layout itself changes only by appending to `MIGRATIONS`.

**PostgreSQL** (`storage-postgres/src/lib.rs`): the same tables as SQLite, on a PostgreSQL server,
through the synchronous `postgres` crate (no TLS yet), with a small pool of connections per
database.

- **Connecting.** `PostgresStore::connect(control_url, now)` takes a connection string such as
  `postgres://strata:secret@127.0.0.1:5432/strata`. `PostgresStore::connect_with(control_url,
  placement, now)` takes a placement rule as well.
- **Placement.** The control connection's database holds, in the schema `strata_control`, what
  SQLite keeps in its control database: the tenant directory, the session directory, the job wake
  index, the platform level of settings, and the ledger `control_migrations`. Where a tenant's
  tables live is a `Placement`: a rule that maps a tenant to a connection string and a schema. The
  default rule, `SchemaPerTenant`, puts every tenant in the control database, each in a schema
  `t_<tenant id>` (`tenant_schema`; an id that would not fit 63 bytes is shortened and given a
  hash). A rule may put tenants in other databases, one each or a few shared; the database must
  exist, and the adapter creates the schema in it. The directory records each tenant's schema;
  the connection string is always the rule's, so none is stored. Each tenant schema holds the
  tables of a SQLite tenant file and its own ledger `tenant_migrations`. The control and tenant
  steps have SQLite's numbers and names, so `migrations()` reports the same list.
- **Isolation.** Commits run at READ COMMITTED. Every write of a tenant first takes a
  transaction-level advisory lock on the tenant's schema, so the writers of one tenant go one at a
  time; a commit then locks the tenant's row (`SELECT ... FOR UPDATE`), compares the revision
  under that lock, and gives its events outbox positions from a counter on that row, so positions
  follow commit order and never come from a sequence. Loads run in REPEATABLE READ, READ ONLY
  transactions and see whole commits only. `Capabilities` states this.
- **Types.** Timestamps are `BIGINT` milliseconds, as SQLite stores them. Text that is compared or
  ordered is `COLLATE "C"`, so it orders by bytes whatever the database's collation. The JSON the
  statements read into (jobs, runs, exception records) is `JSONB`; every other document is the
  canonical JSON text SQLite holds. A broken constraint (SQLSTATE class 23, the ledger's
  append-only trigger included) is `Invalid`; any other database error, a serialisation failure
  included, is `Unavailable`.
- **Conformance.** `crates/storage-postgres/tests/postgres.rs` runs every conformance entry point
  the SQLite test runs against the server named by `STRATA_PG_URL` (for example
  `postgres://strata:strata@127.0.0.1:55432/strata`; the role must be allowed to create
  databases). Each case gets a fresh database `strata_conf_*`; the ones a previous run left are
  dropped first. The test is `#[ignore]`d, so the gate reports it as ignored without a server. To
  run it: `STRATA_PG_URL=... cargo test -p strata-storage-postgres --test storage_postgres_slow -- --ignored`.
  Run with `--ignored` and no variable, it fails and names the variable.

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

   with `strata-storage-api = { features = ["conformance"] }` in dev-dependencies. The suite
   (`storage-api/src/conformance.rs`) checks: stored state equals kernel state by hash, stale commits conflict, malformed
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
