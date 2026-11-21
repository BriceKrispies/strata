//! The layout: the control schema's steps and a tenant schema's steps, numbered and named as the
//! SQLite adapter numbers and names its control database's and its tenant files' steps, so that
//! the two adapters report the same history. Each schema keeps its own ledger table of the steps
//! applied to it. Steps are only ever appended.
//!
//! Types: every integer column is `BIGINT` (timestamps are milliseconds, the values SQLite
//! stores); text that is looked up, compared or ordered is `TEXT COLLATE "C"`, so that it orders
//! by its bytes as SQLite's `BINARY` collation and Rust's `str` do, whatever the database's
//! default collation; JSON documents the statements read into (jobs, runs, exception records) are
//! `JSONB`, every other document is `TEXT` holding the canonical JSON the SQLite adapter holds.
//!
//! The counters SQLite keeps with `AUTOINCREMENT` are:
//! - `events.position`, the outbox position: assigned from the tenant row's `outbox_position`
//!   counter under the tenant row lock of the commit, never from a sequence, so that positions
//!   follow commit order;
//! - `segment_events.position` and `audit_log.seq`: the next number after the highest, assigned
//!   under the tenant's writer lock (their rows are never deleted, and the audit chain's numbers
//!   increase by one);
//! - `jobs.seq`, `automation_runs.seq`, `automation_exceptions.seq`: identity columns, which never
//!   hand out a number twice (their rows are purged, so the highest is not a safe base).

use crate::db;
use postgres::Transaction;
use strata_storage_api::{StoreError, StoreResult};

/// One step of a layout.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// The ledger table of the control schema's steps.
pub(crate) const CONTROL_LEDGER: &str = "control_migrations";
/// The ledger table of a tenant schema's steps.
pub(crate) const TENANT_LEDGER: &str = "tenant_migrations";

/// Apply the steps of `steps` above the highest one recorded in `ledger` (created if missing), in
/// the caller's transaction, recording each with `now`. The caller holds the schema's writer lock
/// and has set `search_path` to the schema. A ledger that records a step this binary does not
/// know is refused, as the SQLite runner refuses it.
pub(crate) fn migrate(
    t: &mut Transaction<'_>,
    ledger: &str,
    steps: &[Migration],
    now: i64,
) -> StoreResult<()> {
    // One literal statement per ledger table, so that each table is visible in the source.
    let create = match ledger {
        CONTROL_LEDGER => {
            "CREATE TABLE IF NOT EXISTS control_migrations (
                version    BIGINT PRIMARY KEY,
                name       TEXT NOT NULL,
                applied_at BIGINT NOT NULL
            )"
        }
        TENANT_LEDGER => {
            "CREATE TABLE IF NOT EXISTS tenant_migrations (
                version    BIGINT PRIMARY KEY,
                name       TEXT NOT NULL,
                applied_at BIGINT NOT NULL
            )"
        }
        _ => return Err(StoreError::Invalid(format!("bad ledger name {ledger:?}"))),
    };
    t.batch_execute(create).map_err(db)?;
    let row = t
        .query_one(
            &*format!("SELECT COALESCE(MAX(version), 0)::bigint FROM {ledger}"),
            &[],
        )
        .map_err(db)?;
    let applied: i64 = crate::get(&row, 0)?;
    let known = steps
        .iter()
        .map(|m| i64::from(m.version))
        .max()
        .unwrap_or(0);
    if applied > known {
        return Err(StoreError::Unavailable(format!(
            "the ledger {ledger} records step {applied}, and this binary knows steps up to {known}"
        )));
    }
    for step in steps.iter().filter(|m| i64::from(m.version) > applied) {
        t.batch_execute(step.sql).map_err(db)?;
        t.execute(
            &*format!("INSERT INTO {ledger} (version, name, applied_at) VALUES ($1, $2, $3)"),
            &[&i64::from(step.version), &step.name, &now],
        )
        .map_err(db)?;
    }
    Ok(())
}

/// The control schema's layout.
pub(crate) const CONTROL_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "tenant directory and session directory",
        // `schema_name` is where the tenant's tables are: the schema the placement rule named
        // when the tenant was created. The database is the placement rule's, so that no
        // connection string (and no password) is stored.
        sql: r#"
    CREATE TABLE tenant_directory (
        id          TEXT COLLATE "C" PRIMARY KEY,
        state       TEXT NOT NULL,
        schema_name TEXT COLLATE "C" NOT NULL,
        created_at  BIGINT NOT NULL,
        CONSTRAINT tenant_directory_state_check CHECK (state IN ('active', 'deleting'))
    );
    CREATE TABLE session_directory (
        id         TEXT COLLATE "C" PRIMARY KEY,
        token_hash TEXT COLLATE "C" NOT NULL UNIQUE,
        tenant     TEXT COLLATE "C" NOT NULL REFERENCES tenant_directory(id) ON DELETE CASCADE
    );
    CREATE INDEX session_directory_tenant ON session_directory(tenant);
    "#,
    },
    Migration {
        version: 2,
        name: "directory state provisioning",
        sql: "
    ALTER TABLE tenant_directory DROP CONSTRAINT tenant_directory_state_check;
    ALTER TABLE tenant_directory ADD CONSTRAINT tenant_directory_state_check
        CHECK (state IN ('active', 'provisioning', 'deleting'));
    ",
    },
    Migration {
        version: 3,
        name: "job wake index",
        sql: r#"
    CREATE TABLE job_wake (
        tenant  TEXT COLLATE "C" PRIMARY KEY REFERENCES tenant_directory(id) ON DELETE CASCADE,
        due_at  BIGINT NOT NULL,
        touched BIGINT NOT NULL
    );
    CREATE INDEX job_wake_due ON job_wake(due_at);
    "#,
    },
    Migration {
        version: 4,
        name: "tenant plan",
        sql: "ALTER TABLE tenant_directory ADD COLUMN plan TEXT NOT NULL DEFAULT 'standard';",
    },
    Migration {
        version: 5,
        name: "platform level of settings",
        sql: r#"
    CREATE TABLE platform_settings (
        setting    TEXT COLLATE "C" PRIMARY KEY,
        value      TEXT,
        revision   BIGINT NOT NULL,
        updated_at BIGINT NOT NULL
    );
    "#,
    },
];

/// A tenant schema's layout.
pub(crate) const TENANT_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "tenant file layout",
        // `tenant.outbox_position` is the last outbox position handed out; the commit raises it
        // under the row lock it holds on this row.
        sql: r#"
    CREATE TABLE tenant (
        id              TEXT COLLATE "C" PRIMARY KEY,
        name            TEXT NOT NULL,
        revision        BIGINT NOT NULL,
        next_record     BIGINT NOT NULL,
        event_seq       BIGINT NOT NULL,
        schema          TEXT NOT NULL,
        members         TEXT NOT NULL,
        outbox_position BIGINT NOT NULL DEFAULT 0
    );
    CREATE TABLE schema_history (
        version  BIGINT PRIMARY KEY,
        revision BIGINT NOT NULL,
        schema   TEXT NOT NULL
    );
    CREATE TABLE records (
        object  TEXT COLLATE "C" NOT NULL,
        id      BIGINT NOT NULL,
        version BIGINT NOT NULL,
        data    TEXT NOT NULL,
        PRIMARY KEY (object, id)
    );
    CREATE TABLE record_index (
        object TEXT COLLATE "C" NOT NULL,
        field  TEXT COLLATE "C" NOT NULL,
        value  TEXT COLLATE "C" NOT NULL,
        id     BIGINT NOT NULL,
        PRIMARY KEY (object, field, value, id),
        FOREIGN KEY (object, id) REFERENCES records(object, id) ON DELETE CASCADE
    );
    CREATE INDEX record_index_rows ON record_index(object, id);
    CREATE TABLE events (
        position  BIGINT PRIMARY KEY,
        seq       BIGINT NOT NULL UNIQUE,
        revision  BIGINT NOT NULL,
        data      TEXT NOT NULL,
        published BIGINT NOT NULL DEFAULT 0
    );
    CREATE INDEX events_outbox ON events(position) WHERE published = 0;
    CREATE TABLE journal (
        revision BIGINT PRIMARY KEY,
        data     TEXT NOT NULL
    );
    CREATE TABLE sessions (
        id           TEXT COLLATE "C" PRIMARY KEY,
        token_hash   TEXT COLLATE "C" NOT NULL UNIQUE,
        csrf_token   TEXT NOT NULL,
        principal    TEXT COLLATE "C" NOT NULL,
        display_name TEXT NOT NULL,
        created_at   BIGINT NOT NULL,
        last_seen_at BIGINT NOT NULL,
        expires_at   BIGINT NOT NULL,
        revoked_at   BIGINT
    );
    CREATE INDEX sessions_principal ON sessions(principal);
    CREATE TABLE accounts (
        login         TEXT COLLATE "C" PRIMARY KEY,
        principal     TEXT COLLATE "C" NOT NULL,
        display_name  TEXT NOT NULL,
        password_hash TEXT NOT NULL
    );
    CREATE TABLE snapshots (
        id         BIGINT PRIMARY KEY,
        label      TEXT NOT NULL,
        revision   BIGINT NOT NULL,
        hash       TEXT NOT NULL,
        created_at BIGINT NOT NULL,
        data       TEXT NOT NULL
    );
    "#,
    },
    Migration {
        version: 2,
        name: "login throttle",
        sql: r#"
    CREATE TABLE login_throttle (
        key          TEXT COLLATE "C" PRIMARY KEY,
        failures     BIGINT NOT NULL,
        window_start BIGINT NOT NULL,
        locked_until BIGINT NOT NULL,
        expires_at   BIGINT NOT NULL
    );
    CREATE INDEX login_throttle_expiry ON login_throttle(expires_at);
    "#,
    },
    Migration {
        version: 3,
        name: "passkeys",
        sql: r#"
    CREATE TABLE passkeys (
        id           TEXT COLLATE "C" PRIMARY KEY,
        principal    TEXT COLLATE "C" NOT NULL,
        rp_id        TEXT NOT NULL,
        label        TEXT NOT NULL,
        credential   TEXT NOT NULL,
        created_at   BIGINT NOT NULL,
        last_used_at BIGINT
    );
    CREATE INDEX passkeys_principal ON passkeys(principal, created_at);
    CREATE TABLE passkey_challenges (
        id_hash    TEXT COLLATE "C" PRIMARY KEY,
        ceremony   TEXT NOT NULL,
        session    TEXT,
        principal  TEXT COLLATE "C",
        origin     TEXT NOT NULL,
        state      TEXT NOT NULL,
        expires_at BIGINT NOT NULL
    );
    CREATE INDEX passkey_challenges_expiry ON passkey_challenges(expires_at);
    CREATE INDEX accounts_principal ON accounts(principal);
    "#,
    },
    Migration {
        version: 4,
        name: "sign-in links",
        sql: r#"
    CREATE TABLE sign_in_links (
        id_hash      TEXT COLLATE "C" PRIMARY KEY,
        principal    TEXT COLLATE "C" NOT NULL,
        token_hash   TEXT NOT NULL,
        binding_hash TEXT NOT NULL,
        origin       TEXT NOT NULL,
        expires_at   BIGINT NOT NULL
    );
    CREATE INDEX sign_in_links_expiry ON sign_in_links(expires_at);
    "#,
    },
    Migration {
        version: 5,
        name: "identity-provider sign-ins and federated identities",
        sql: r#"
    CREATE TABLE oidc_flows (
        id_hash      TEXT COLLATE "C" PRIMARY KEY,
        provider     TEXT NOT NULL,
        nonce        TEXT NOT NULL,
        verifier     TEXT NOT NULL,
        binding_hash TEXT NOT NULL,
        origin       TEXT NOT NULL,
        principal    TEXT COLLATE "C",
        expires_at   BIGINT NOT NULL
    );
    CREATE INDEX oidc_flows_expiry ON oidc_flows(expires_at);
    CREATE TABLE federated_identities (
        provider   TEXT COLLATE "C" NOT NULL,
        subject    TEXT COLLATE "C" NOT NULL,
        principal  TEXT COLLATE "C" NOT NULL,
        created_at BIGINT NOT NULL,
        PRIMARY KEY (provider, subject)
    );
    CREATE INDEX federated_identities_principal ON federated_identities(principal);
    "#,
    },
    Migration {
        version: 6,
        name: "second factors, recovery codes, pending sign-ins, recovery requests and session authentication",
        sql: r#"
    CREATE TABLE totp_factors (
        principal  TEXT COLLATE "C" PRIMARY KEY,
        secret     TEXT NOT NULL,
        confirmed  BIGINT NOT NULL,
        last_step  BIGINT NOT NULL,
        created_at BIGINT NOT NULL
    );
    CREATE TABLE recovery_codes (
        principal  TEXT COLLATE "C" NOT NULL,
        code_hash  TEXT COLLATE "C" NOT NULL,
        created_at BIGINT NOT NULL,
        PRIMARY KEY (principal, code_hash)
    );
    CREATE TABLE pending_sign_ins (
        id_hash    TEXT COLLATE "C" PRIMARY KEY,
        principal  TEXT COLLATE "C" NOT NULL,
        method     TEXT NOT NULL,
        factors    BIGINT NOT NULL,
        attempts   BIGINT NOT NULL,
        expires_at BIGINT NOT NULL
    );
    CREATE INDEX pending_sign_ins_expiry ON pending_sign_ins(expires_at);
    CREATE TABLE recovery_requests (
        id_hash      TEXT COLLATE "C" PRIMARY KEY,
        principal    TEXT COLLATE "C" NOT NULL,
        login        TEXT NOT NULL,
        token_hash   TEXT NOT NULL,
        cancel_hash  TEXT NOT NULL,
        requested_at BIGINT NOT NULL,
        completes_at BIGINT NOT NULL,
        expires_at   BIGINT NOT NULL
    );
    CREATE INDEX recovery_requests_principal ON recovery_requests(principal);
    CREATE INDEX recovery_requests_expiry ON recovery_requests(expires_at);
    ALTER TABLE sessions ADD COLUMN authenticated_at BIGINT NOT NULL DEFAULT 0;
    ALTER TABLE sessions ADD COLUMN network TEXT;
    UPDATE sessions SET authenticated_at = created_at;
    "#,
    },
    Migration {
        version: 7,
        name: "SAML sign-ins and accepted assertions",
        sql: r#"
    CREATE TABLE saml_requests (
        id_hash      TEXT COLLATE "C" PRIMARY KEY,
        provider     TEXT NOT NULL,
        request_id   TEXT NOT NULL,
        binding_hash TEXT NOT NULL,
        origin       TEXT NOT NULL,
        principal    TEXT COLLATE "C",
        expires_at   BIGINT NOT NULL
    );
    CREATE INDEX saml_requests_expiry ON saml_requests(expires_at);
    CREATE TABLE saml_assertions (
        provider   TEXT COLLATE "C" NOT NULL,
        id_hash    TEXT COLLATE "C" NOT NULL,
        expires_at BIGINT NOT NULL,
        PRIMARY KEY (provider, id_hash)
    );
    CREATE INDEX saml_assertions_expiry ON saml_assertions(expires_at);
    "#,
    },
    Migration {
        version: 8,
        name: "identity service documents",
        sql: r#"
    CREATE TABLE identity_docs (
        kind  TEXT COLLATE "C" NOT NULL,
        key   TEXT COLLATE "C" NOT NULL,
        value TEXT NOT NULL,
        PRIMARY KEY (kind, key)
    );
    "#,
    },
    Migration {
        version: 9,
        name: "verified e-mail address of an account",
        sql: "ALTER TABLE accounts ADD COLUMN verified_email TEXT;",
    },
    Migration {
        version: 10,
        name: "idempotency keys",
        sql: r#"
    CREATE TABLE idempotency (
        principal     TEXT COLLATE "C" NOT NULL,
        key           TEXT COLLATE "C" NOT NULL,
        request_hash  TEXT NOT NULL,
        recorded_at   BIGINT NOT NULL,
        revision      BIGINT NOT NULL,
        last_position BIGINT,
        result        TEXT NOT NULL,
        PRIMARY KEY (principal, key)
    );
    CREATE INDEX idempotency_recorded ON idempotency(recorded_at);
    "#,
    },
    Migration {
        version: 11,
        name: "security audit trail",
        sql: r#"
    CREATE TABLE audit_log (
        seq         BIGINT PRIMARY KEY,
        at          BIGINT NOT NULL,
        kind        TEXT COLLATE "C" NOT NULL,
        event       TEXT NOT NULL,
        correlation TEXT NOT NULL,
        data        TEXT NOT NULL
    );
    CREATE INDEX audit_log_kind ON audit_log(kind, seq);
    "#,
    },
    Migration {
        version: 12,
        name: "setting overrides and revisions",
        sql: r#"
    CREATE TABLE setting_overrides (
        setting  TEXT COLLATE "C" NOT NULL,
        level    TEXT COLLATE "C" NOT NULL,
        subject  TEXT COLLATE "C" NOT NULL,
        revision BIGINT NOT NULL,
        data     TEXT NOT NULL,
        PRIMARY KEY (setting, level, subject)
    );
    CREATE TABLE setting_revisions (
        setting  TEXT COLLATE "C" PRIMARY KEY,
        revision BIGINT NOT NULL
    );
    "#,
    },
    Migration {
        version: 13,
        name: "automation worker jobs, runs, steps and exceptions",
        sql: r#"
    CREATE TABLE jobs (
        seq    BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        id     TEXT COLLATE "C" NOT NULL UNIQUE,
        key    TEXT COLLATE "C" NOT NULL UNIQUE,
        state  TEXT COLLATE "C" NOT NULL,
        due_at BIGINT NOT NULL,
        data   JSONB NOT NULL
    );
    CREATE INDEX jobs_due ON jobs(due_at) WHERE state IN ('waiting', 'leased');
    CREATE TABLE automation_runs (
        seq        BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        run        TEXT COLLATE "C" NOT NULL UNIQUE,
        automation TEXT COLLATE "C" NOT NULL,
        state      TEXT COLLATE "C" NOT NULL,
        data       JSONB NOT NULL
    );
    CREATE TABLE automation_steps (
        run      TEXT COLLATE "C" NOT NULL,
        sequence BIGINT NOT NULL,
        data     TEXT NOT NULL,
        PRIMARY KEY (run, sequence)
    );
    CREATE TABLE automation_exceptions (
        seq     BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        id      TEXT COLLATE "C" NOT NULL UNIQUE,
        kind    TEXT COLLATE "C" NOT NULL,
        settled BIGINT NOT NULL,
        data    JSONB NOT NULL
    );
    "#,
    },
    crate::ledger::MIGRATION,
    Migration {
        version: 15,
        name: "segments: definitions, memberships, wake rows, batches, erased persons and events",
        sql: r#"
    CREATE TABLE segments (
        id           TEXT COLLATE "C" PRIMARY KEY,
        name         TEXT COLLATE "C" NOT NULL UNIQUE,
        version      BIGINT NOT NULL,
        member_count BIGINT NOT NULL DEFAULT 0,
        data         TEXT NOT NULL
    );
    CREATE TABLE segment_memberships (
        segment   TEXT COLLATE "C" NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
        principal TEXT COLLATE "C" NOT NULL,
        version   BIGINT NOT NULL,
        since     BIGINT NOT NULL,
        PRIMARY KEY (segment, principal)
    );
    CREATE INDEX segment_memberships_principal ON segment_memberships(principal);
    CREATE TABLE segment_wakes (
        segment   TEXT COLLATE "C" NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
        principal TEXT COLLATE "C" NOT NULL,
        wake_at   BIGINT NOT NULL,
        PRIMARY KEY (segment, principal)
    );
    CREATE INDEX segment_wake_due ON segment_wakes(segment, wake_at);
    CREATE INDEX segment_wakes_principal ON segment_wakes(principal);
    CREATE TABLE segment_batches (
        segment TEXT COLLATE "C" NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
        scope   TEXT COLLATE "C" NOT NULL,
        data    TEXT NOT NULL,
        PRIMARY KEY (segment, scope)
    );
    CREATE TABLE segment_erased (
        principal TEXT COLLATE "C" PRIMARY KEY
    );
    CREATE TABLE segment_events (
        position  BIGINT PRIMARY KEY,
        data      TEXT NOT NULL,
        published BIGINT NOT NULL DEFAULT 0
    );
    "#,
    },
    Migration {
        version: 16,
        name: "order keys of the record index",
        sql: crate::index_page::ORDER_KEYS_SQL,
    },
    Migration {
        version: 17,
        name: "pair index of named queries",
        sql: crate::pair_index::PAIR_INDEX_SQL,
    },
    Migration {
        version: 18,
        name: "search entries",
        sql: crate::search_index::SEARCH_ENTRIES_SQL,
    },
    crate::ledger::ACCOUNT_LIMITS_MIGRATION,
    crate::ledger::EXPIRY_MIGRATION,
    Migration {
        version: 21,
        name: "change counters of the object types",
        sql: r#"
    CREATE TABLE object_revisions (
        object   TEXT COLLATE "C" PRIMARY KEY,
        revision BIGINT NOT NULL
    );
    "#,
    },
    Migration {
        version: 22,
        name: "schedules, run keys and per-entry idempotency retention",
        sql: r#"
    ALTER TABLE idempotency ADD COLUMN expires_at BIGINT;
    UPDATE idempotency SET expires_at = recorded_at + 86400000;
    CREATE INDEX idempotency_expires ON idempotency(expires_at);
    CREATE TABLE schedules (
        id   TEXT COLLATE "C" PRIMARY KEY,
        data TEXT NOT NULL
    );
    CREATE TABLE automation_run_keys (
        automation TEXT COLLATE "C" NOT NULL,
        key        TEXT COLLATE "C" NOT NULL,
        run        TEXT NOT NULL,
        PRIMARY KEY (automation, key)
    );
    "#,
    },
    Migration {
        version: 23,
        name: "audit chain values, and the schema history without teams and invitations",
        sql: "
    ALTER TABLE audit_log ADD COLUMN chain TEXT;
    UPDATE schema_history SET schema = ((schema::jsonb) - 'teams' - 'invitations')::text;
    ",
    },
];
