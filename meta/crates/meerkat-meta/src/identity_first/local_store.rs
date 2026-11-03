//! Bundled SQLite-backed ContinuityStore for `persistent_state(path)` usage.
//!
//! Implements CONTRACT-06. Designed for single-process, local-disk persistence.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use async_trait::async_trait;
use meerkat_core::SessionStoreError;
use meerkat_core::session_store::{
    SessionHead, SessionHeadCas, StrandLayout, TranscriptStrandId, reconstruct_rewrite_record,
    session_head_cas_token, strand_layout_for_history, strand_layout_for_retained_history,
    validate_commit_rewrite_transition, validate_save_head_transition,
};
use meerkat_core::types::Message;
use meerkat_core::{Session, TranscriptRewriteCommit, TranscriptRewriteRecord};
use rusqlite::{Connection, OptionalExtension, Transaction};

use super::contracts::{
    ContinuityIncrementalSessions, ContinuityStore, ContinuityWriteCursor,
    SessionSnapshotMatchCandidate,
};
use super::types::{
    AgentIdentity, AgentRuntimeId, CheckpointVersion, ContinuityGeneration, ContinuityRecord,
    ContinuityResolveState, ContinuityStoreError, FencingToken, SessionSnapshot,
};

const READ_POOL_SIZE: usize = 4;

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS continuity_records (
        identity       TEXT PRIMARY KEY,
        agent_runtime_id TEXT NOT NULL,
        session_id     TEXT NOT NULL,
        generation     INTEGER NOT NULL,
        checkpoint_version INTEGER NOT NULL,
        fencing_token  INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS session_snapshots (
        session_id     TEXT PRIMARY KEY,
        identity       TEXT NOT NULL,
        generation     INTEGER NOT NULL,
        checkpoint_version INTEGER NOT NULL,
        fencing_token  INTEGER NOT NULL,
        data           BLOB NOT NULL
    );";

/// Head-canonical session representation (M4b): the durable trio meerkat's
/// incremental persistence contract needs, plus the `(identity, generation)`
/// stamps every meta continuity row carries so reset rollback and identity
/// deletion can scope their deletes — including strand rows written in the
/// crash window between `append_messages` and the head write that adopts
/// them.
const SCHEMA_HEAD_CANONICAL: &str = "CREATE TABLE IF NOT EXISTS continuity_session_heads (
        session_id     TEXT PRIMARY KEY,
        identity       TEXT NOT NULL,
        generation     INTEGER NOT NULL,
        checkpoint_version INTEGER NOT NULL,
        fencing_token  INTEGER NOT NULL,
        head_revision  TEXT NOT NULL,
        message_count  INTEGER NOT NULL,
        rewrite_count  INTEGER NOT NULL,
        head_json      BLOB NOT NULL,
        cas_token      TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS continuity_strand_messages (
        session_id     TEXT NOT NULL,
        strand         TEXT NOT NULL,
        seq            INTEGER NOT NULL,
        message_json   BLOB NOT NULL,
        identity       TEXT NOT NULL,
        generation     INTEGER NOT NULL,
        created_at_ms  INTEGER NOT NULL,
        PRIMARY KEY (session_id, strand, seq)
    );
    CREATE TABLE IF NOT EXISTS continuity_session_rewrites (
        session_id     TEXT NOT NULL,
        rewrite_idx    INTEGER NOT NULL,
        parent_strand  TEXT NOT NULL,
        parent_len     INTEGER NOT NULL,
        strand         TEXT NOT NULL,
        strand_len     INTEGER NOT NULL,
        commit_json    BLOB NOT NULL,
        identity       TEXT NOT NULL,
        generation     INTEGER NOT NULL,
        created_at_ms  INTEGER NOT NULL,
        PRIMARY KEY (session_id, rewrite_idx)
    );
    CREATE INDEX IF NOT EXISTS continuity_records_session_idx
        ON continuity_records(session_id);
    CREATE INDEX IF NOT EXISTS continuity_heads_identity_gen_idx
        ON continuity_session_heads(identity, generation);
    CREATE INDEX IF NOT EXISTS continuity_strands_identity_gen_idx
        ON continuity_strand_messages(identity, generation);
    CREATE INDEX IF NOT EXISTS continuity_rewrites_identity_gen_idx
        ON continuity_session_rewrites(identity, generation);";

/// Ledger version that records "this file carries the head-canonical
/// channel". A binary whose `meta-continuity` domain tops out below this
/// refuses the file typed at open — correctly, because it would keep writing
/// `session_snapshots.data` while head rows are the byte authority. The
/// lockout is therefore only allowed to exist once head rows can exist:
/// see [`LocalContinuityStore::open`].
pub const HEAD_CANONICAL_SCHEMA_VERSION: i64 = 2;

/// Explicit identity-history opt-in also refuses older binaries that cannot
/// preserve complete bindings through later disabled-extension intervals.
const IDENTITY_HISTORY_SCHEMA_VERSION: i64 = 3;

/// The continuity store's schema domain in the per-file migration ledger.
/// Migration 0001 is the historical two-table DDL (all `CREATE ... IF NOT
/// EXISTS`, so a pre-ledger file converges without its rows being touched);
/// migration 0002 adds the head-canonical trio (DDL-only, additive, zero row
/// rewrites). Migration 0003 retains exact identity bindings and native birth
/// coverage; only explicit extension activation applies it.
///
/// **This domain is NEVER applied by a plain [`LocalContinuityStore::open`].**
/// The head-only routes target frozen v2, which locks every `<= 0.8.5` binary
/// out of the file (`SqliteStoreError::SchemaFromTheFuture`). That lockout is load-bearing
/// only once a head row exists, so it is committed at exactly two moments:
/// by a delta write that actually creates head state (armed inside that
/// write's own transaction, so a REFUSED write leaves the file at v1), and
/// by explicit operator action (`storage-migrate --apply`) — which stamps
/// only AFTER [`LocalContinuityStore::backfill_head_canonical_sessions_at`]
/// has given every legacy blob a head row. Until 0.8.16 that second moment
/// stamped unconditionally while converting nothing, so an operator paid the
/// whole one-way cost and received an unconverted corpus; the stamp now
/// rides on complete conversion and a partial crossing stays at v1. Merely
/// launching a new gateway leaves rollback to the previous release intact.
/// Explicit history activation instead stamps v3 atomically with its retained
/// bindings. Older writers must then refuse: missing bindings during a
/// downgrade interval would invalidate the persisted native-birth coverage.
pub(crate) const META_CONTINUITY_DOMAIN: meerkat_sqlite::SchemaDomain =
    meerkat_sqlite::SchemaDomain {
        name: "meta-continuity",
        migrations: &[
            meerkat_sqlite::Migration {
                version: 1,
                name: "base-schema",
                apply: migration_0001_continuity_schema,
            },
            meerkat_sqlite::Migration {
                version: 2,
                name: "head-canonical-sessions",
                apply: migration_0002_head_canonical_sessions,
            },
            meerkat_sqlite::Migration {
                version: IDENTITY_HISTORY_SCHEMA_VERSION,
                name: "retained-identity-bindings",
                apply: migration_0003_identity_history,
            },
        ],
        initialize_current: initialize_current_continuity_schema,
        allowed_existing_versions: &[1, 2, 3],
        // Unledgered meta files are refused at open (below the 0.8.8 ledger
        // floor) and meta never runs the offline bridge, so no source
        // version is inferable.
        bridge_recoverable_versions: &[],
        released_predecessors: &[
            meerkat_sqlite::SchemaPredecessor {
                version: 1,
                verify: verify_released_v1_continuity_schema,
            },
            meerkat_sqlite::SchemaPredecessor {
                version: 2,
                verify: verify_released_v2_continuity_schema,
            },
        ],
        owned_objects: CONTINUITY_OWNED_OBJECTS,
        retired_objects: &[],
    };

const RELEASED_V1_CONTINUITY_OBJECTS: &[meerkat_sqlite::SchemaObject] = &[
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "continuity_records",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "session_snapshots",
    },
];

const RELEASED_V2_CONTINUITY_OBJECTS: &[meerkat_sqlite::SchemaObject] = &[
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "continuity_records",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "session_snapshots",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "continuity_session_heads",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "continuity_strand_messages",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "continuity_session_rewrites",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "continuity_records_session_idx",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "continuity_heads_identity_gen_idx",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "continuity_strands_identity_gen_idx",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "continuity_rewrites_identity_gen_idx",
    },
];

const CONTINUITY_OWNED_OBJECTS: &[meerkat_sqlite::SchemaObject] = &[
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "continuity_records",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "session_snapshots",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "continuity_session_heads",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "continuity_strand_messages",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "continuity_session_rewrites",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "continuity_records_session_idx",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "continuity_heads_identity_gen_idx",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "continuity_strands_identity_gen_idx",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "continuity_rewrites_identity_gen_idx",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "continuity_identity_history",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "continuity_identity_history_identity_idx",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "continuity_identity_coverage",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Trigger,
        name: "continuity_identity_history_immutable",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Trigger,
        name: "continuity_identity_history_insert",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Trigger,
        name: "continuity_identity_history_update",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Trigger,
        name: "continuity_identity_snapshot_insert",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Trigger,
        name: "continuity_identity_snapshot_update",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Trigger,
        name: "continuity_identity_head_insert",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Trigger,
        name: "continuity_identity_head_update",
    },
];

fn initialize_current_continuity_schema(
    tx: &rusqlite::Transaction<'_>,
) -> Result<(), rusqlite::Error> {
    initialize_released_v2_continuity_schema(tx)?;
    migration_0003_identity_history(tx)
}

/// Frozen initializer for both released v2 and v1 with deferred head DDL.
fn initialize_released_v2_continuity_schema(
    tx: &rusqlite::Transaction<'_>,
) -> Result<(), rusqlite::Error> {
    migration_0001_continuity_schema(tx)?;
    migration_0002_head_canonical_sessions(tx)
}

fn verify_released_v2_continuity_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    meerkat_sqlite::verify_released_schema_fingerprint(
        conn,
        &META_CONTINUITY_DOMAIN,
        RELEASED_V2_CONTINUITY_OBJECTS,
        initialize_released_v2_continuity_schema,
    )
}

/// Frozen v1 verifier honoring the deferred-stamp design: a delta write may
/// commit the head-canonical DDL inside its own transaction and leave the
/// ledger at v1 until head state actually exists, so a v1 file legally
/// carries either the plain two-table v1 catalog or the frozen v2 DDL.
fn verify_released_v1_continuity_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    meerkat_sqlite::verify_released_schema_fingerprint(
        conn,
        &META_CONTINUITY_DOMAIN,
        RELEASED_V1_CONTINUITY_OBJECTS,
        migration_0001_continuity_schema,
    )
    .or_else(|plain| {
        meerkat_sqlite::verify_released_schema_fingerprint(
            conn,
            &META_CONTINUITY_DOMAIN,
            RELEASED_V2_CONTINUITY_OBJECTS,
            initialize_released_v2_continuity_schema,
        )
        .map_err(|full| {
            format!("v1 catalog: {plain}; v1 + deferred head-canonical DDL catalog: {full}")
        })
    })
}

/// The open-time domain: migration 0001 only. Opening a fresh file converges
/// it to v1 exactly as every previous release did, and an already-v2 file is
/// left alone (the version check below runs against the FULL domain, so a v2
/// file is not "from the future").
const META_CONTINUITY_BASELINE_DOMAIN: meerkat_sqlite::SchemaDomain =
    meerkat_sqlite::SchemaDomain {
        name: "meta-continuity",
        migrations: &[meerkat_sqlite::Migration {
            version: 1,
            name: "base-schema",
            apply: migration_0001_continuity_schema,
        }],
        initialize_current: migration_0001_continuity_schema,
        allowed_existing_versions: &[1],
        // Unledgered meta files are refused at open (below the 0.8.8 ledger
        // floor) and meta never runs the offline bridge, so no source
        // version is inferable.
        bridge_recoverable_versions: &[],
        released_predecessors: &[],
        owned_objects: RELEASED_V1_CONTINUITY_OBJECTS,
        retired_objects: &[],
    };

/// Head-only operator migration. Its released v2 target never installs
/// optional identity history or pays the v3 downgrade barrier.
const META_CONTINUITY_HEAD_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "meta-continuity",
    migrations: &[
        meerkat_sqlite::Migration {
            version: 1,
            name: "base-schema",
            apply: migration_0001_continuity_schema,
        },
        meerkat_sqlite::Migration {
            version: 2,
            name: "head-canonical-sessions",
            apply: migration_0002_head_canonical_sessions,
        },
    ],
    initialize_current: initialize_released_v2_continuity_schema,
    allowed_existing_versions: &[1, 2],
    bridge_recoverable_versions: &[],
    released_predecessors: &[meerkat_sqlite::SchemaPredecessor {
        version: 1,
        verify: verify_released_v1_continuity_schema,
    }],
    owned_objects: RELEASED_V2_CONTINUITY_OBJECTS,
    retired_objects: &[],
};

fn migration_0001_continuity_schema(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(SCHEMA)
}

fn migration_0002_head_canonical_sessions(
    tx: &rusqlite::Transaction<'_>,
) -> Result<(), rusqlite::Error> {
    tx.execute_batch(SCHEMA_HEAD_CANONICAL)
}

/// The history is owned by the continuity domain: every trigger target
/// exists in its empty-database oracle. Backfill and the v3 compatibility
/// barrier commit together, or both roll back on conflicting exact bindings.
/// An outer UPSERT can override a trigger's OR IGNORE conflict policy. Skip
/// only an existing exact pair before inserting instead; a different owner
/// still reaches the immutable-binding trigger and aborts the outer write.
fn migration_0003_identity_history(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(
        "CREATE TABLE continuity_identity_history(session_id TEXT PRIMARY KEY, identity TEXT NOT NULL);
         CREATE INDEX continuity_identity_history_identity_idx ON continuity_identity_history(identity);
         CREATE TABLE continuity_identity_coverage(mob_id TEXT PRIMARY KEY, after_cursor INTEGER NOT NULL);
         CREATE TRIGGER continuity_identity_history_immutable BEFORE INSERT ON continuity_identity_history
         WHEN EXISTS(SELECT 1 FROM continuity_identity_history WHERE session_id=NEW.session_id AND identity<>NEW.identity)
         BEGIN SELECT RAISE(ABORT, 'conflicting historical identity binding'); END;
         CREATE TRIGGER continuity_identity_history_insert AFTER INSERT ON continuity_records
         BEGIN
             INSERT INTO continuity_identity_history(session_id,identity)
             SELECT NEW.session_id,NEW.identity
             WHERE NOT EXISTS(SELECT 1 FROM continuity_identity_history WHERE session_id=NEW.session_id AND identity=NEW.identity);
         END;
         CREATE TRIGGER continuity_identity_history_update AFTER UPDATE OF session_id,identity ON continuity_records
         BEGIN
             INSERT INTO continuity_identity_history(session_id,identity)
             SELECT NEW.session_id,NEW.identity
             WHERE NOT EXISTS(SELECT 1 FROM continuity_identity_history WHERE session_id=NEW.session_id AND identity=NEW.identity);
         END;
         CREATE TRIGGER continuity_identity_snapshot_insert AFTER INSERT ON session_snapshots
         BEGIN
             INSERT INTO continuity_identity_history(session_id,identity)
             SELECT NEW.session_id,NEW.identity
             WHERE NOT EXISTS(SELECT 1 FROM continuity_identity_history WHERE session_id=NEW.session_id AND identity=NEW.identity);
         END;
         CREATE TRIGGER continuity_identity_snapshot_update AFTER UPDATE OF session_id,identity ON session_snapshots
         BEGIN
             INSERT INTO continuity_identity_history(session_id,identity)
             SELECT NEW.session_id,NEW.identity
             WHERE NOT EXISTS(SELECT 1 FROM continuity_identity_history WHERE session_id=NEW.session_id AND identity=NEW.identity);
         END;
         CREATE TRIGGER continuity_identity_head_insert AFTER INSERT ON continuity_session_heads
         BEGIN
             INSERT INTO continuity_identity_history(session_id,identity)
             SELECT NEW.session_id,NEW.identity
             WHERE NOT EXISTS(SELECT 1 FROM continuity_identity_history WHERE session_id=NEW.session_id AND identity=NEW.identity);
         END;
         CREATE TRIGGER continuity_identity_head_update AFTER UPDATE OF session_id,identity ON continuity_session_heads
         BEGIN
             INSERT INTO continuity_identity_history(session_id,identity)
             SELECT NEW.session_id,NEW.identity
             WHERE NOT EXISTS(SELECT 1 FROM continuity_identity_history WHERE session_id=NEW.session_id AND identity=NEW.identity);
         END;
         INSERT OR IGNORE INTO continuity_identity_history(session_id,identity) SELECT session_id,identity FROM continuity_records;
         INSERT OR IGNORE INTO continuity_identity_history(session_id,identity) SELECT session_id,identity FROM session_snapshots;
         INSERT OR IGNORE INTO continuity_identity_history(session_id,identity) SELECT session_id,identity FROM continuity_session_heads;",
    )
}

/// Refuse a file whose `meta-continuity` ledger is ahead of this binary.
///
/// Local replacement for the retired `meerkat_sqlite::refuse_future_schema`:
/// deliberately ONLY a future-version check, because the deferred-stamp
/// design legally leaves a v1 ledger over committed head-canonical DDL, a
/// shape exact per-version catalog eligibility would refuse at every open.
fn refuse_future_continuity_schema(conn: &Connection) -> Result<(), ContinuityStoreError> {
    let supported = META_CONTINUITY_DOMAIN.supported_version();
    let found = meerkat_sqlite::domain_version(conn, META_CONTINUITY_DOMAIN.name)
        .map_err(|e| mechanics_err("continuity schema preflight", e))?;
    match found {
        Some(found) if found > supported => Err(mechanics_err(
            "continuity schema preflight",
            meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture {
                domain: META_CONTINUITY_DOMAIN.name.to_string(),
                found,
                supported,
            },
        )),
        _ => Ok(()),
    }
}

/// Open-time schema convergence that never commits the one-way v2 bump.
///
/// Returns whether the file already carries the head-canonical channel.
///
/// - future version (> the full domain) => refused typed, nothing mutated;
/// - already v2 => nothing applied, delta channel ready;
/// - already v1 => nothing applied (the v2 bump waits for a delta write);
/// - no ledger row (fresh or pre-ledger file) => baseline v1 applied, exactly
///   as before this release.
fn converge_schema_at_open(conn: &mut Connection) -> Result<bool, ContinuityStoreError> {
    refuse_future_continuity_schema(conn)?;
    let version = meerkat_sqlite::domain_version(conn, META_CONTINUITY_DOMAIN.name)
        .map_err(|e| mechanics_err("read continuity ledger", e))?;
    match version {
        Some(version) if version >= IDENTITY_HISTORY_SCHEMA_VERSION => {
            meerkat_sqlite::preflight_schema_eligibility(conn, &META_CONTINUITY_DOMAIN)
                .map_err(|e| mechanics_err("identity history schema preflight", e))?;
            Ok(true)
        }
        Some(version) if version >= HEAD_CANONICAL_SCHEMA_VERSION => Ok(true),
        Some(_) => Ok(false),
        None => {
            meerkat_sqlite::apply_domain_migrations(conn, &META_CONTINUITY_BASELINE_DOMAIN)
                .map_err(|e| mechanics_err("apply schema", e))?;
            Ok(false)
        }
    }
}

/// Bring the head-canonical tables into existence INSIDE the caller's
/// transaction, WITHOUT recording the ledger bump.
///
/// This is the half of the v1 -> v2 upgrade that is safe to speculate on:
/// the DDL is additive and `IF NOT EXISTS`, it applies the domain's own
/// migration bodies (never a second copy of the schema), and — because
/// SQLite runs DDL transactionally — a rollback removes the tables again.
/// The ledger stamp, which is the part that locks older binaries out, is
/// [`stamp_head_canonical_ledger_in_txn`] and is written only after the
/// enclosing write has actually created head state.
///
/// Refuses a file whose ledger is ahead of this binary, exactly as
/// `apply_domain_migrations` would, and refuses a file that has lost its
/// continuity ledger row (every opener converges one; its absence under a
/// write transaction is corruption, not a fresh file).
fn converge_head_canonical_schema_in_txn(tx: &Transaction<'_>) -> Result<(), ContinuityStoreError> {
    refuse_future_continuity_schema(tx)?;
    let current = meerkat_sqlite::domain_version(tx, META_CONTINUITY_DOMAIN.name)
        .map_err(|e| mechanics_err("read continuity ledger", e))?
        .ok_or_else(|| {
            ContinuityStoreError::Corruption(
                "continuity ledger has no meta-continuity row; the file's migration ledger was \
                 removed after it was opened"
                    .to_string(),
            )
        })?;
    for migration in META_CONTINUITY_DOMAIN
        .migrations
        .iter()
        .filter(|migration| {
            migration.version > current && migration.version <= HEAD_CANONICAL_SCHEMA_VERSION
        })
    {
        (migration.apply)(tx).map_err(|e| sqlite_err("apply head-canonical schema", e))?;
    }
    Ok(())
}

/// Whether the file's `meta-continuity` ledger row already carries the
/// one-way v2 lockout, read INSIDE the caller's transaction.
///
/// The authority for "is the lockout committed" is this row and nothing
/// else. In particular it is NOT
/// [`LocalContinuityStoreInner::schema_is_head_canonical`], which answers
/// the different question "are the head tables queryable" and latches
/// `true` the moment the tables are observed. Those two facts diverge on
/// purpose: a delta write that creates strand rows but no head row commits
/// the DDL and leaves the ledger at v1 (see
/// [`LocalContinuityStore::delta_write`]), so the tables can exist on a
/// file that is still rollback-safe. Deciding the stamp from the table
/// probe would then skip the bump forever once that state exists — head
/// rows with no lockout, the exact hazard the lockout is for.
fn head_canonical_ledger_stamped_in_txn(
    tx: &Transaction<'_>,
) -> Result<bool, ContinuityStoreError> {
    let version = meerkat_sqlite::domain_version(tx, META_CONTINUITY_DOMAIN.name)
        .map_err(|e| mechanics_err("read continuity ledger", e))?;
    Ok(version.is_some_and(|version| version >= HEAD_CANONICAL_SCHEMA_VERSION))
}

/// Does this session have a persisted head row right now?
///
/// The predicate that earns the one-way ledger bump. A head row is what
/// makes head+rows a session's sole byte authority; strand rows that no
/// head adopts are not part of any document (every read path gates on the
/// head row), so a file carrying only those is still correctly served by an
/// older binary from its blob.
fn session_head_exists_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
) -> Result<bool, ContinuityStoreError> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM continuity_session_heads WHERE session_id = ?1)",
        rusqlite::params![id.to_string()],
        |row| row.get::<_, bool>(0),
    )
    .map_err(|e| sqlite_err("probe session head row", e))
}

/// Record the one-way `meta-continuity` v2 bump in the caller's
/// transaction — the moment binaries older than this release start being
/// refused the file.
///
/// Written last, so it commits atomically with the head state that earns
/// it. It must never downgrade an explicitly enabled v3 history database.
fn stamp_head_canonical_ledger_in_txn(tx: &Transaction<'_>) -> Result<(), ContinuityStoreError> {
    tx.execute(
        "INSERT INTO main.meerkat_schema (domain, version) VALUES (?1, ?2)
         ON CONFLICT(domain) DO UPDATE SET version = MAX(meerkat_schema.version, excluded.version)",
        rusqlite::params![META_CONTINUITY_DOMAIN.name, HEAD_CANONICAL_SCHEMA_VERSION],
    )
    .map_err(|e| sqlite_err("stamp head-canonical ledger", e))?;
    Ok(())
}

/// Commit the head-canonical schema (ledger `meta-continuity` v1 -> v2).
///
/// Called from `storage-migrate --apply` under the exclusive maintenance
/// fence — the explicit-operator route. (The implicit route, the first
/// delta write that creates head state, arms the same bump inside its own
/// write transaction; see [`LocalContinuityStore::delta_write`].) The DDL
/// is additive and `IF NOT EXISTS`; the ledger stamp is what makes the
/// upgrade one-way.
pub(crate) fn apply_head_canonical_schema(
    conn: &mut Connection,
) -> Result<meerkat_sqlite::LedgerReport, meerkat_sqlite::SqliteStoreError> {
    let version = meerkat_sqlite::domain_version(conn, META_CONTINUITY_DOMAIN.name)?;
    let domain = if version.is_some_and(|version| version >= IDENTITY_HISTORY_SCHEMA_VERSION) {
        &META_CONTINUITY_DOMAIN
    } else {
        &META_CONTINUITY_HEAD_DOMAIN
    };
    meerkat_sqlite::apply_domain_migrations(conn, domain)
}

/// Classify a raw SQLite failure at the store boundary: busy/locked is
/// transient, corruption is corrupt, everything else keeps the historical
/// `Io` shape. Staleness (fencing-token / checkpoint CAS conflicts) is a
/// store-contract concept decided above this layer, never here.
fn sqlite_err(context: &str, error: rusqlite::Error) -> ContinuityStoreError {
    match meerkat_sqlite::classify_sqlite_error(&error) {
        meerkat_sqlite::SqliteErrorClass::Transient => {
            ContinuityStoreError::Transient(format!("{context}: {error}"))
        }
        meerkat_sqlite::SqliteErrorClass::Corrupt => {
            ContinuityStoreError::Corruption(format!("{context}: {error}"))
        }
        meerkat_sqlite::SqliteErrorClass::Other => {
            ContinuityStoreError::Io(format!("{context}: {error}"))
        }
    }
}

/// Map a shared-mechanics error into the typed store error, routing the
/// wrapped raw SQLite failures through [`sqlite_err`]'s classification. A
/// held maintenance fence is transient by nature (storage is under offline
/// maintenance; the operation may be retried once it lifts).
fn mechanics_err(context: &str, error: meerkat_sqlite::SqliteStoreError) -> ContinuityStoreError {
    match error {
        meerkat_sqlite::SqliteStoreError::Sqlite(sql) => sqlite_err(context, sql),
        meerkat_sqlite::SqliteStoreError::MaintenanceFenceHeld { .. } => {
            ContinuityStoreError::Transient(format!("{context}: {error}"))
        }
        other => ContinuityStoreError::Io(format!("{context}: {other}")),
    }
}

struct ReadConnectionPool {
    available: Mutex<Vec<Connection>>,
    ready: Condvar,
}

impl ReadConnectionPool {
    fn new(connections: Vec<Connection>) -> Self {
        debug_assert!(!connections.is_empty());
        Self {
            available: Mutex::new(connections),
            ready: Condvar::new(),
        }
    }

    fn acquire(&self) -> Result<ReadConnectionGuard<'_>, ContinuityStoreError> {
        let mut available = self
            .available
            .lock()
            .map_err(|e| ContinuityStoreError::Io(format!("read pool lock: {e}")))?;
        loop {
            if let Some(connection) = available.pop() {
                return Ok(ReadConnectionGuard {
                    pool: self,
                    connection: Some(connection),
                });
            }
            available = self
                .ready
                .wait(available)
                .map_err(|e| ContinuityStoreError::Io(format!("read pool wait: {e}")))?;
        }
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, ContinuityStoreError>,
    ) -> Result<T, ContinuityStoreError> {
        let connection = self.acquire()?;
        operation(connection.connection()?)
    }
}

struct ReadConnectionGuard<'a> {
    pool: &'a ReadConnectionPool,
    connection: Option<Connection>,
}

impl ReadConnectionGuard<'_> {
    fn connection(&self) -> Result<&Connection, ContinuityStoreError> {
        self.connection
            .as_ref()
            .ok_or_else(|| ContinuityStoreError::Io("read pool guard lost its connection".into()))
    }
}

impl Drop for ReadConnectionGuard<'_> {
    fn drop(&mut self) {
        let Some(connection) = self.connection.take() else {
            return;
        };
        let mut available = self
            .pool
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        available.push(connection);
        self.pool.ready.notify_one();
    }
}

enum ReadConnections {
    /// `Connection::open_in_memory()` is private to one connection. Reads share
    /// the writer rather than accidentally observing independent databases.
    Writer,
    Pool(ReadConnectionPool),
}

struct LocalContinuityStoreInner {
    /// Database file path; `:memory:` for in-memory stores (where the
    /// per-operation fence guard degrades to a no-op).
    db_path: PathBuf,
    writer: Mutex<Connection>,
    readers: ReadConnections,
    /// Whether the head-canonical TABLES are queryable on this file.
    /// Latched, never cleared: schema evolution is one-way. Read paths that
    /// observe the tables appearing under them (another handle on the same
    /// file committed the DDL) latch it too, so a stale `false` can never
    /// make this handle serve the frozen blob archive as authority.
    ///
    /// This is deliberately NOT "the ledger carries the v2 lockout" — see
    /// [`Self::ledger_is_head_canonical`]. Tables can exist on a file whose
    /// ledger is still v1.
    head_canonical_schema: AtomicBool,
    /// Whether the file's `meta-continuity` ledger row already carries the
    /// committed one-way v2 lockout. Latched from a fact that is itself
    /// one-way; a `false` here only means "not known to be stamped", and the
    /// write path re-reads the ledger row inside its own transaction before
    /// acting on it.
    head_canonical_ledger: AtomicBool,
}

impl LocalContinuityStoreInner {
    fn schema_is_head_canonical(&self) -> bool {
        self.head_canonical_schema.load(Ordering::Acquire)
    }

    /// Cached "the one-way lockout is already committed on this file".
    /// Only ever used to SKIP work; never to decide that a bump is owed.
    fn ledger_is_head_canonical(&self) -> bool {
        self.head_canonical_ledger.load(Ordering::Acquire)
    }

    /// Whether the head-canonical tables are queryable on this connection.
    /// Cheap after the first `true` (one relaxed atomic load).
    fn head_tables_available(&self, conn: &Connection) -> Result<bool, ContinuityStoreError> {
        if self.schema_is_head_canonical() {
            return Ok(true);
        }
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' \
                 AND name = 'continuity_session_heads')",
                [],
                |row| row.get(0),
            )
            .map_err(|e| sqlite_err("probe head-canonical tables", e))?;
        if exists {
            self.head_canonical_schema.store(true, Ordering::Release);
        }
        Ok(exists)
    }

    /// Per-operation maintenance-fence guard: the writer and reader pool
    /// hold their connections for the store's lifetime, so the fence
    /// cannot ride the open — every operation takes its own shared guard,
    /// and offline maintenance drains in-flight guards before touching the
    /// file.
    fn operation_fence(&self) -> Result<meerkat_sqlite::OperationGuard, ContinuityStoreError> {
        meerkat_sqlite::OperationGuard::for_database(&self.db_path)
            .map_err(|e| mechanics_err("operation fence", e))
    }

    fn with_reader<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, ContinuityStoreError>,
    ) -> Result<T, ContinuityStoreError> {
        let _fence = self.operation_fence()?;
        match &self.readers {
            ReadConnections::Writer => {
                let connection = self
                    .writer
                    .lock()
                    .map_err(|e| ContinuityStoreError::Io(format!("writer lock: {e}")))?;
                operation(&connection)
            }
            ReadConnections::Pool(pool) => pool.with_connection(operation),
        }
    }

    fn with_writer<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, ContinuityStoreError>,
    ) -> Result<T, ContinuityStoreError> {
        let _fence = self.operation_fence()?;
        let mut connection = self
            .writer
            .lock()
            .map_err(|e| ContinuityStoreError::Io(format!("writer lock: {e}")))?;
        operation(&mut connection)
    }
}

/// SQLite-backed ContinuityStore for the bundled `persistent_state(path)` path.
///
/// Stores ContinuityRecords and SessionSnapshots in a single SQLite database.
/// Enforces compare-and-set on (fencing_token, checkpoint_version). File-backed
/// stores use one serialized writer plus a bounded WAL read pool; async trait
/// operations execute SQLite work on Tokio's blocking workers.
#[derive(Clone)]
pub struct LocalContinuityStore {
    inner: Arc<LocalContinuityStoreInner>,
}

impl LocalContinuityStore {
    /// Open (or create) a local continuity store at the given path.
    ///
    /// # Errors
    ///
    /// Returns `ContinuityStoreError::Io` if the database cannot be opened or
    /// the schema cannot be initialized.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ContinuityStoreError> {
        let path = path.as_ref();
        if path == Path::new(":memory:") {
            return Self::in_memory();
        }

        let mut writer = meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::PRIMARY)
            .map_err(|e| mechanics_err("open writer", e))?;
        // Deliberately NOT `apply_domain_migrations(.., &META_CONTINUITY_DOMAIN)`:
        // opening a state directory with a new binary must not commit the
        // one-way ledger v2 bump that locks the previous release out of the
        // file. See `converge_schema_at_open`.
        let head_canonical = converge_schema_at_open(&mut writer)?;

        let mut readers = Vec::with_capacity(READ_POOL_SIZE);
        for index in 0..READ_POOL_SIZE {
            // `ReadOnly` (SQLITE_OPEN_READ_ONLY) is the honest form of the
            // historical `PRAGMA query_only=ON` reader configuration: the
            // connection itself cannot write, and a reader never converts
            // the file's journal mode.
            let reader = meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::ReadOnly)
                .map_err(|e| mechanics_err(&format!("open reader {index}"), e))?;
            readers.push(reader);
        }

        Ok(Self {
            inner: Arc::new(LocalContinuityStoreInner {
                db_path: path.to_path_buf(),
                writer: Mutex::new(writer),
                readers: ReadConnections::Pool(ReadConnectionPool::new(readers)),
                // A stamped ledger implies the tables; the reverse does not
                // hold, so the table latch is seeded from the same fact and
                // widened later by `head_tables_available`.
                head_canonical_schema: AtomicBool::new(head_canonical),
                head_canonical_ledger: AtomicBool::new(head_canonical),
            }),
        })
    }

    /// Open the store and read its fencing-token floor without blocking a
    /// Tokio worker. Async builders and gateways must use this seam because
    /// SQLite open/schema/WAL setup can wait on the filesystem or a database
    /// lock for the configured busy timeout.
    pub async fn open_with_fencing_floor(
        path: impl Into<PathBuf>,
    ) -> Result<(Self, u64), ContinuityStoreError> {
        let path = path.into();
        tokio::task::spawn_blocking(move || {
            let store = Self::open(path)?;
            let fencing_floor = store.max_fencing_token()?;
            Ok((store, fencing_floor))
        })
        .await
        .map_err(|error| {
            ContinuityStoreError::Io(format!(
                "open_with_fencing_floor blocking worker failed: {error}"
            ))
        })?
    }

    /// Commit the head-canonical schema (ledger `meta-continuity` v1 -> v2)
    /// into an existing database file as an explicit operator action.
    ///
    /// This is the `storage-migrate --apply` route. It is separate from
    /// [`Self::open`] on purpose: the bump locks binaries older than this
    /// release out of the file (`SchemaFromTheFuture`), so launching a new
    /// gateway must never commit it as a side effect. Runs under whatever
    /// exclusive maintenance fence the caller already holds.
    ///
    /// # Errors
    ///
    /// Returns `ContinuityStoreError::Io` when the file cannot be opened or
    /// the migration cannot be applied.
    pub fn apply_head_canonical_schema_at(
        path: impl AsRef<Path>,
    ) -> Result<bool, ContinuityStoreError> {
        let mut conn =
            meerkat_sqlite::open(path.as_ref(), meerkat_sqlite::ConnectionProfile::PRIMARY)
                .map_err(|e| mechanics_err("open writer for head-canonical migration", e))?;
        let report = apply_head_canonical_schema(&mut conn)
            .map_err(|e| mechanics_err("apply head-canonical schema", e))?;
        Ok(report.migrated())
    }

    /// Offline head-canonical backfill for a legacy v1 corpus.
    ///
    /// The lazy path mints a head row only inside a delta write
    /// (`ensure_head_canonical_for_write_in_txn`), so a corpus whose
    /// documents are large enough to make that write expensive can never
    /// leave the whole-document branch under its own steam: the conversion
    /// is gated behind the very write it makes slow. This is the operator
    /// path `META_CONTINUITY_DOMAIN`'s stamp contract has always named
    /// and never had.
    ///
    /// Contract, in the order it matters:
    /// - **Resumable.** ONE transaction per session. An interrupted run
    ///   leaves every already-converted session converted; re-running
    ///   resumes on the remainder, because the pending set is derived from
    ///   the absence of a head row rather than from a cursor.
    /// - **The blob is retained.** `migrate_legacy_blob_in_txn` leaves the
    ///   `session_snapshots` row untouched as a frozen archive.
    /// - **The ledger stamps only on complete conversion.** A partial run
    ///   leaves the file at v1, so CONTINUITY DOES NOT BECOME THE DOMAIN THAT
    ///   BLOCKS an older binary. This is why the stamp is not folded into the
    ///   per-session transaction.
    ///
    ///   Deliberately NOT "rollback stays available": a real state directory
    ///   carries several ledgered domains (runtime-store, schedule-store,
    ///   workgraph, console, metadata, continuity), and any one of them being
    ///   ahead of the target binary refuses the open on its own. Measured on
    ///   a production clone, `rpc_gateway` 0.8.5 refused at
    ///   `runtime-store` (file v2, binary ceiling v1) and never reached
    ///   continuity at all. So holding this domain at v1 is necessary for
    ///   rollback and nowhere near sufficient; whether rollback is actually
    ///   available is a per-domain question about the WHOLE state dir and is
    ///   not a claim this function is entitled to make.
    /// - **Dry-run mutates nothing**, including the DDL: a caller inspecting
    ///   a v1 file gets a count and no schema change.
    ///
    /// The caller is responsible for the exclusive maintenance fence; this
    /// function does not take one, exactly as the other `*_at` maintenance
    /// entry points do not.
    ///
    /// # Errors
    ///
    /// Returns [`ContinuityStoreError`] if the file cannot be opened or the
    /// pending set cannot be read. Per-session conversion failures are
    /// collected into the report rather than aborting the run, so one
    /// unconvertible session does not strand the rest.
    pub fn backfill_head_canonical_sessions_at(
        path: impl AsRef<Path>,
        apply: bool,
        acknowledged_rows: &std::collections::BTreeSet<String>,
    ) -> Result<HeadCanonicalBackfillReport, ContinuityStoreError> {
        // A dry run opens READ-ONLY, and that is a correctness requirement
        // rather than tidiness. The writer profile sets `journal_mode=WAL`,
        // which is a durable change to the file: a caller inspecting a
        // DELETE-mode corpus would find it converted to WAL (and `-wal` /
        // `-shm` siblings created) purely by asking what a migration WOULD do.
        // "Dry run mutates nothing" has to include the pragmas, or an
        // operator cannot use it to inspect a file they are not ready to
        // change.
        let mut conn = if apply {
            meerkat_sqlite::open(path.as_ref(), meerkat_sqlite::ConnectionProfile::PRIMARY)
                .map_err(|e| mechanics_err("open writer for head-canonical backfill", e))?
        } else {
            Connection::open_with_flags(
                path.as_ref(),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )
            .map_err(|e| sqlite_err("open read-only for head-canonical dry run", e))?
        };

        let (pending, unparseable) = pending_head_canonical_sessions(&conn)?;
        let mut report = HeadCanonicalBackfillReport {
            examined: pending.len(),
            applied: apply,
            skipped_unparseable: unparseable,
            ..HeadCanonicalBackfillReport::default()
        };
        if !apply {
            return Ok(report);
        }

        // The DDL half, once, before any conversion. Additive and
        // `IF NOT EXISTS`; still no ledger bump.
        {
            let tx = conn
                .transaction()
                .map_err(|e| sqlite_err("begin head-canonical schema convergence", e))?;
            converge_head_canonical_schema_in_txn(&tx)?;
            tx.commit()
                .map_err(|e| sqlite_err("commit head-canonical schema convergence", e))?;
        }

        for candidate in pending {
            match backfill_one_session(&mut conn, &candidate) {
                Ok(BackfillOutcome::Converted) => {
                    report.converted.push(candidate.session_id.clone());
                }
                // A conversion the deployment had already outgrown, replaced.
                // Reported separately: an operator re-running after a failed
                // crossing needs to see that prior work was redone, not just
                // that the run "succeeded".
                Ok(BackfillOutcome::Reconverted) => {
                    report.reconverted.push(candidate.session_id.clone());
                }
                Ok(BackfillOutcome::AlreadyCurrent) => {}
                // The blob vanished between census and conversion. Not a
                // failure, but not a conversion either — record it so a
                // complete-conversion claim cannot be made on a corpus that
                // changed under the fence.
                Ok(BackfillOutcome::Vanished) => {
                    report.vanished.push(candidate.session_id.clone());
                }
                Err(error) => report
                    .failures
                    .push((candidate.session_id.clone(), error.to_string())),
            }
        }

        // Stamp ONLY when the corpus is wholly across — which includes a
        // corpus that had nothing to convert. The stamp means "no legacy
        // blob is left unconverted", not "this run did work"; withholding it
        // from an already-clean corpus would strand such a file at v1
        // forever. Any failure, or any session that disappeared mid-run,
        // leaves the file at v1, so continuity does not become the domain
        // that blocks an older binary. Whether rollback is available at all
        // depends on every other ledgered domain in the state dir, which this
        // verb neither inspects nor controls.
        // Malformed rows BLOCK by default. A row this classifier calls
        // malformed may be a corrupted durable session, or a real session the
        // classifier is simply too strict about — a widened identity grammar,
        // a shape written by an older binary. Those are indistinguishable
        // here, and the distinction only matters at the one step that cannot
        // be undone. So the operator acknowledges them explicitly or the
        // ledger does not advance; nobody is stranded, but nothing crosses
        // the one-way door without a human having seen what is left behind.
        // Acknowledgement is by STABLE ROW IDENTITY, never by count. An
        // operator who read a list of three rows must not silently authorise
        // a different three on a later run, so every skipped row has to be
        // named. Unnamed rows are reported individually rather than as a
        // total, because the operator has to be able to act on them.
        let unacknowledged: Vec<&String> = report
            .skipped_unparseable
            .iter()
            .filter(|row| !acknowledged_rows.contains(*row))
            .collect();
        if !unacknowledged.is_empty() {
            report.failures.push((
                String::new(),
                format!(
                    "refusing ledger stamp: {} blob row(s) could not be parsed as sessions and \
                     were not acknowledged: {}",
                    unacknowledged.len(),
                    unacknowledged
                        .iter()
                        .map(|row| row.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        if report.failures.is_empty() && report.vanished.is_empty() {
            // A failure HERE must not discard the report. By this point
            // sessions have been converted and committed in their own
            // transactions; returning Err would hand the operator an error
            // with no record of the work that already happened, and they
            // would have no way to know whether to expect it on a re-run.
            // Record it as a failure and return the report instead.
            let mut remaining = match blob_rows_without_head(&conn) {
                Ok(rows) => rows,
                Err(error) => {
                    report.failures.push((
                        String::new(),
                        format!("refusing ledger stamp: final verification failed: {error}"),
                    ));
                    return Ok(report);
                }
            };
            // A row the operator explicitly acknowledged as unparseable will
            // never have a head — that is what acknowledging it meant. It is
            // not evidence of unfinished work, so it must not block the stamp
            // the acknowledgement was given to permit.
            remaining.retain(|row| !acknowledged_rows.contains(row));
            if remaining.is_empty() {
                match conn.transaction() {
                    Ok(tx) => match stamp_head_canonical_ledger_in_txn(&tx).and_then(|()| {
                        tx.commit()
                            .map_err(|e| sqlite_err("commit head-canonical ledger stamp", e))
                    }) {
                        Ok(()) => report.ledger_stamped = true,
                        // Conversions stand; only the stamp failed. The file
                        // stays at v1, which is the safe outcome, and the
                        // operator keeps the record of what converted.
                        Err(error) => report.failures.push((
                            String::new(),
                            format!("conversions committed but ledger stamp failed: {error}"),
                        )),
                    },
                    Err(error) => report.failures.push((
                        String::new(),
                        format!("conversions committed but ledger stamp could not begin: {error}"),
                    )),
                }
            } else {
                // Re-census disagreed with the per-session results. Refuse to
                // stamp rather than trust the optimistic count.
                report.failures.push((
                    String::new(),
                    format!(
                        "refusing ledger stamp: {} session(s) still lack a head row after conversion",
                        remaining.len()
                    ),
                ));
            }
        }
        Ok(report)
    }

    /// Open an in-memory store (for testing).
    ///
    /// # Errors
    ///
    /// Returns `ContinuityStoreError::Io` if initialization fails.
    pub fn in_memory() -> Result<Self, ContinuityStoreError> {
        let mut writer =
            Connection::open_in_memory().map_err(|e| sqlite_err("in-memory open", e))?;
        // Same staged convergence as the file path, so the lazy v2 bump is
        // exercised identically in tests and in production.
        let head_canonical = converge_schema_at_open(&mut writer)?;
        Ok(Self {
            inner: Arc::new(LocalContinuityStoreInner {
                db_path: PathBuf::from(":memory:"),
                writer: Mutex::new(writer),
                readers: ReadConnections::Writer,
                head_canonical_schema: AtomicBool::new(head_canonical),
                head_canonical_ledger: AtomicBool::new(head_canonical),
            }),
        })
    }

    /// The highest fencing token ever committed to this store, across BOTH
    /// `continuity_records` and `session_snapshots` (0 if the store is empty).
    ///
    /// The bundled [`LocalLeaseProvider`](super::local_lease::LocalLeaseProvider)
    /// seeds its monotonic counter from this on startup so fencing tokens keep
    /// advancing across process restarts. Without it the provider's in-memory
    /// counter resets to 1 and restore presents a stale token that this store's
    /// compare-and-set rejects — the v0.7.8 "stale fencing token: presented 1,
    /// current N" restart abort.
    ///
    /// # Errors
    ///
    /// Returns `ContinuityStoreError::Io` on a query failure.
    pub fn max_fencing_token(&self) -> Result<u64, ContinuityStoreError> {
        self.inner.with_reader(|connection| {
            // The head-canonical arm only exists once the file carries the
            // channel; on a v1 file the union of the two historical tables
            // IS the whole high-water.
            let sql = if self.inner.head_tables_available(connection)? {
                "SELECT COALESCE(MAX(t), 0) FROM (
                        SELECT MAX(fencing_token) AS t FROM continuity_records
                        UNION ALL
                        SELECT MAX(fencing_token) AS t FROM session_snapshots
                        UNION ALL
                        SELECT MAX(fencing_token) AS t FROM continuity_session_heads
                    )"
            } else {
                "SELECT COALESCE(MAX(t), 0) FROM (
                        SELECT MAX(fencing_token) AS t FROM continuity_records
                        UNION ALL
                        SELECT MAX(fencing_token) AS t FROM session_snapshots
                    )"
            };
            connection
                .query_row(sql, [], |row| row.get::<_, u64>(0))
                .map_err(|e| sqlite_err("max_fencing_token", e))
        })
    }

    async fn run_blocking<T>(
        &self,
        operation_name: &'static str,
        operation: impl FnOnce(Arc<LocalContinuityStoreInner>) -> Result<T, ContinuityStoreError>
        + Send
        + 'static,
    ) -> Result<T, ContinuityStoreError>
    where
        T: Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || operation(inner))
            .await
            .map_err(|e| {
                ContinuityStoreError::Io(format!("{operation_name} blocking worker failed: {e}"))
            })?
    }
}

// ---------------------------------------------------------------------------
// Head-canonical session representation (M4b)
//
// Canonical-representation rule, per session: a `continuity_session_heads`
// row exists => head+rows are the SOLE durable authority for that session,
// and its `session_snapshots` row (if any) is a frozen archive that is never
// read or written again. No head row => the legacy blob behavior is
// byte-for-byte unchanged. This mirrors meerkat-store's own rule for
// `session_heads` vs `sessions` and is what makes the delta channel a
// REPLACEMENT of the byte authority rather than a second one beside it.
//
// Guard semantics are meerkat's published validators verbatim
// (`validate_save_head_transition`, `validate_commit_rewrite_transition`,
// `strand_layout_for_history`, `reconstruct_rewrite_record`), so this store
// mirror can never accept or reject something the meerkat service would not.
// ---------------------------------------------------------------------------

/// Map a continuity-store failure onto the session-store error surface the
/// incremental verbs speak, exactly as the whole-blob adapter save does.
fn session_err(context: &str, error: ContinuityStoreError) -> SessionStoreError {
    SessionStoreError::Internal(format!("{context}: {error}"))
}

fn sqlite_session_err(context: &str, error: rusqlite::Error) -> SessionStoreError {
    session_err(context, sqlite_err(context, error))
}

fn now_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default(),
    )
    .unwrap_or(i64::MAX)
}

/// The stored head row plus its CAS token.
type StoredHead = (SessionHead, String);

fn head_row_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
) -> Result<Option<StoredHead>, SessionStoreError> {
    let row = tx
        .query_row(
            "SELECT head_json, cas_token FROM continuity_session_heads WHERE session_id = ?1",
            rusqlite::params![id.to_string()],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|e| sqlite_session_err("query session head", e))?;
    let Some((head_json, cas_token)) = row else {
        return Ok(None);
    };
    let head: SessionHead =
        serde_json::from_slice(&head_json).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    Ok(Some((head, cas_token)))
}

/// The `(identity, generation)` a head row belongs to, when one exists.
fn head_owner_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
) -> Result<Option<(String, u64)>, ContinuityStoreError> {
    tx.query_row(
        "SELECT identity, generation FROM continuity_session_heads WHERE session_id = ?1",
        rusqlite::params![id.to_string()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
    )
    .optional()
    .map_err(|e| sqlite_err("query head owner", e))
}

fn write_head_row_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
    identity: &AgentIdentity,
    generation: ContinuityGeneration,
    version: CheckpointVersion,
    fencing_token: FencingToken,
) -> Result<String, SessionStoreError> {
    let head_json = serde_json::to_vec(head).map_err(SessionStoreError::from)?;
    let cas_token = session_head_cas_token(head)?;
    let message_count = i64::try_from(head.message_count)
        .map_err(|_| SessionStoreError::Corrupted(head.id.clone()))?;
    let rewrite_count = i64::try_from(head.rewrite_count)
        .map_err(|_| SessionStoreError::Corrupted(head.id.clone()))?;
    tx.execute(
        "INSERT INTO continuity_session_heads (
            session_id, identity, generation, checkpoint_version, fencing_token,
            head_revision, message_count, rewrite_count, head_json, cas_token
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(session_id) DO UPDATE SET
            identity = excluded.identity,
            generation = excluded.generation,
            checkpoint_version = excluded.checkpoint_version,
            fencing_token = excluded.fencing_token,
            head_revision = excluded.head_revision,
            message_count = excluded.message_count,
            rewrite_count = excluded.rewrite_count,
            head_json = excluded.head_json,
            cas_token = excluded.cas_token",
        rusqlite::params![
            head.id.to_string(),
            identity.as_str(),
            generation.get(),
            version.get(),
            fencing_token.get(),
            head.head_revision,
            message_count,
            rewrite_count,
            head_json,
            cas_token,
        ],
    )
    .map_err(|e| sqlite_session_err("upsert session head", e))?;
    Ok(cas_token)
}

fn strand_row_count_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
    strand: &TranscriptStrandId,
) -> Result<u64, SessionStoreError> {
    let count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM continuity_strand_messages \
             WHERE session_id = ?1 AND strand = ?2",
            rusqlite::params![id.to_string(), strand.as_str()],
            |row| row.get(0),
        )
        .map_err(|e| sqlite_session_err("count strand rows", e))?;
    u64::try_from(count).map_err(|_| SessionStoreError::Corrupted(id.clone()))
}

fn strand_row_bytes_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
    strand: &TranscriptStrandId,
    range: std::ops::Range<u64>,
) -> Result<Vec<Vec<u8>>, SessionStoreError> {
    if range.start >= range.end {
        return Ok(Vec::new());
    }
    let start = i64::try_from(range.start).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let end = i64::try_from(range.end).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let mut statement = tx
        .prepare_cached(
            "SELECT message_json FROM continuity_strand_messages
             WHERE session_id = ?1 AND strand = ?2 AND seq >= ?3 AND seq < ?4
             ORDER BY seq ASC",
        )
        .map_err(|e| sqlite_session_err("prepare strand read", e))?;
    let rows = statement
        .query_map(
            rusqlite::params![id.to_string(), strand.as_str(), start, end],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .map_err(|e| sqlite_session_err("read strand rows", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| sqlite_session_err("read strand rows", e))?;
    if rows.len() as u64 != range.end - range.start {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(rows)
}

fn strand_messages_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
    strand: &TranscriptStrandId,
    range: std::ops::Range<u64>,
) -> Result<Vec<Message>, SessionStoreError> {
    strand_row_bytes_in_txn(tx, id, strand, range)?
        .into_iter()
        .map(|bytes| {
            serde_json::from_slice::<Message>(&bytes)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))
        })
        .collect()
}

/// Append rows under the trait's contiguity / idempotency / immutability
/// contract: `base_seq` may not exceed the current row count; overlapping
/// rows must be byte-identical; shrink is structurally inexpressible.
fn insert_strand_rows_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
    strand: &TranscriptStrandId,
    base_seq: u64,
    messages: &[Message],
    identity: &AgentIdentity,
    generation: ContinuityGeneration,
) -> Result<(), SessionStoreError> {
    let existing = strand_row_count_in_txn(tx, id, strand)?;
    if base_seq > existing {
        return Err(SessionStoreError::TranscriptContinuityViolation {
            id: id.clone(),
            previous_revision: format!("strand-rows:{existing}"),
            incoming_revision: format!("append-base-seq:{base_seq}"),
            reason: format!(
                "append at base_seq {base_seq} would leave a gap in strand {strand} with \
                 {existing} rows"
            ),
        });
    }
    let serialized: Vec<Vec<u8>> = messages
        .iter()
        .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
        .collect::<Result<_, _>>()?;
    let overlap_end = existing.min(base_seq + serialized.len() as u64);
    if overlap_end > base_seq {
        let stored = strand_row_bytes_in_txn(tx, id, strand, base_seq..overlap_end)?;
        for (offset, stored_bytes) in stored.iter().enumerate() {
            if stored_bytes != &serialized[offset] {
                return Err(SessionStoreError::TranscriptContinuityViolation {
                    id: id.clone(),
                    previous_revision: format!("strand:{strand} seq:{}", base_seq + offset as u64),
                    incoming_revision: "divergent-bytes".to_string(),
                    reason: format!(
                        "append would overwrite immutable row (strand {strand}, seq {}) with \
                         different bytes",
                        base_seq + offset as u64
                    ),
                });
            }
        }
    }
    let created_at_ms = now_millis();
    for (offset, bytes) in serialized.iter().enumerate() {
        let seq = base_seq + offset as u64;
        if seq < existing {
            continue;
        }
        let seq_i64 = i64::try_from(seq).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        tx.execute(
            "INSERT INTO continuity_strand_messages
                (session_id, strand, seq, message_json, identity, generation, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                id.to_string(),
                strand.as_str(),
                seq_i64,
                bytes,
                identity.as_str(),
                generation.get(),
                created_at_ms,
            ],
        )
        .map_err(|e| sqlite_session_err("insert strand row", e))?;
    }
    Ok(())
}

struct RewriteRow {
    commit: TranscriptRewriteCommit,
    parent_strand: TranscriptStrandId,
    parent_len: u64,
    strand: TranscriptStrandId,
    strand_len: u64,
}

fn rewrite_row_count_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
) -> Result<u64, SessionStoreError> {
    let count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM continuity_session_rewrites WHERE session_id = ?1",
            rusqlite::params![id.to_string()],
            |row| row.get(0),
        )
        .map_err(|e| sqlite_session_err("count rewrite rows", e))?;
    u64::try_from(count).map_err(|_| SessionStoreError::Corrupted(id.clone()))
}

/// The adopted rewrite records of a head-canonical session, reconstructed
/// from the persisted rows in the caller's transaction. One place, so every
/// caller reconstructs history identically.
fn rewrite_records_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
    max_idx_exclusive: u64,
) -> Result<Vec<TranscriptRewriteRecord>, SessionStoreError> {
    rewrite_rows_in_txn(tx, id, max_idx_exclusive)?
        .into_iter()
        .map(|row| {
            let parent_messages =
                strand_messages_in_txn(tx, id, &row.parent_strand, 0..row.parent_len)?;
            let revision_messages = strand_messages_in_txn(tx, id, &row.strand, 0..row.strand_len)?;
            reconstruct_rewrite_record(id, row.commit, parent_messages, revision_messages)
        })
        .collect()
}

fn rewrite_rows_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
    max_idx_exclusive: u64,
) -> Result<Vec<RewriteRow>, SessionStoreError> {
    let limit =
        i64::try_from(max_idx_exclusive).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let mut statement = tx
        .prepare_cached(
            "SELECT commit_json, parent_strand, parent_len, strand, strand_len
             FROM continuity_session_rewrites
             WHERE session_id = ?1 AND rewrite_idx < ?2
             ORDER BY rewrite_idx ASC",
        )
        .map_err(|e| sqlite_session_err("prepare rewrite read", e))?;
    let rows = statement
        .query_map(rusqlite::params![id.to_string(), limit], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| sqlite_session_err("read rewrite rows", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| sqlite_session_err("read rewrite rows", e))?;
    rows.into_iter()
        .map(
            |(commit_json, parent_strand, parent_len, strand, strand_len)| {
                let commit: TranscriptRewriteCommit = serde_json::from_slice(&commit_json)
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
                Ok(RewriteRow {
                    commit,
                    parent_strand: TranscriptStrandId::from_persisted(parent_strand),
                    parent_len: u64::try_from(parent_len)
                        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                    strand: TranscriptStrandId::from_persisted(strand),
                    strand_len: u64::try_from(strand_len)
                        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                })
            },
        )
        .collect()
}

fn insert_rewrite_row_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
    rewrite_idx: u64,
    row: &RewriteRow,
    identity: &AgentIdentity,
    generation: ContinuityGeneration,
) -> Result<(), SessionStoreError> {
    let commit_json = serde_json::to_vec(&row.commit).map_err(SessionStoreError::from)?;
    let idx = i64::try_from(rewrite_idx).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let parent_len =
        i64::try_from(row.parent_len).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let strand_len =
        i64::try_from(row.strand_len).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    tx.execute(
        "INSERT OR REPLACE INTO continuity_session_rewrites
            (session_id, rewrite_idx, parent_strand, parent_len, strand, strand_len,
             commit_json, identity, generation, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            id.to_string(),
            idx,
            row.parent_strand.as_str(),
            parent_len,
            row.strand.as_str(),
            strand_len,
            commit_json,
            identity.as_str(),
            generation.get(),
            now_millis(),
        ],
    )
    .map_err(|e| sqlite_session_err("insert rewrite row", e))?;
    Ok(())
}

fn blob_session_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
) -> Result<Option<Session>, SessionStoreError> {
    let data = tx
        .query_row(
            "SELECT data FROM session_snapshots WHERE session_id = ?1",
            rusqlite::params![id.to_string()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(|e| sqlite_session_err("read archived snapshot", e))?;
    let Some(data) = data else {
        return Ok(None);
    };
    match serde_json::from_slice::<Session>(&data) {
        Ok(session) => Ok(Some(session)),
        Err(decode_error) => import_released_blob_in_txn(tx, id, &data, &decode_error).map(Some),
    }
}

/// Same-transaction one-time import of a released 0.8.10 blob row.
///
/// Per the banked 0.8.11 import contract: the public core importer is the
/// sole boundary allowed to interpret released evidence, the non-Clone
/// receipt is consumed by the adoption, the source blob SHA is re-proved
/// against the exact bytes read, nothing mints the retired vocabulary, and
/// every proof failure fails closed. The durable adoption rewrites the
/// payload bytes INSIDE the caller's transaction; the row's cursor custody
/// columns stay exactly as observed. A read-only transaction (the read-pool
/// fallbacks) serves the imported document without adoption - the first
/// write-path decode (head-canonical conversion, delta writes) adopts.
fn import_released_blob_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
    source: &[u8],
    decode_error: &serde_json::Error,
) -> Result<Session, SessionStoreError> {
    use sha2::Digest as _;

    let imported = meerkat_core::import_released_0810_session(source).map_err(|import| {
        SessionStoreError::Serialization(format!(
            "continuity blob {id} decodes neither as a current document ({decode_error}) nor as              a released 0.8.10 envelope ({import})"
        ))
    })?;
    let (session, receipt) = imported.into_parts();
    let observed_sha256: [u8; 32] = sha2::Sha256::digest(source).into();
    if receipt.source_document_sha256() != &observed_sha256 {
        return Err(SessionStoreError::Serialization(format!(
            "continuity blob {id} changed during exact released-0.8.10 import"
        )));
    }
    if receipt.session_id() != id {
        return Err(SessionStoreError::Serialization(format!(
            "continuity blob key {id} contains released session {}",
            receipt.session_id()
        )));
    }
    let current = session
        .to_persisted_bytes()
        .map_err(|e| SessionStoreError::Serialization(e.to_string()))?;
    // The receipt is consumed by this durable adoption inside the caller's
    // transaction.
    drop(receipt);
    let changed = match tx.execute(
        "UPDATE session_snapshots SET data = ?2 WHERE session_id = ?1",
        rusqlite::params![id.to_string(), current],
    ) {
        Ok(changed) => changed,
        Err(rusqlite::Error::SqliteFailure(failure, _))
            if failure.code == rusqlite::ErrorCode::ReadOnly =>
        {
            // Read-pool fallback: serve the imported document; the first
            // write-path decode (head-canonical conversion, delta writes)
            // performs the durable adoption.
            tracing::info!(
                session_id = %id,
                "released 0.8.10 blob imported on a read-only connection; durable adoption \
                 follows the first write-path decode"
            );
            return Ok(session);
        }
        Err(e) => return Err(sqlite_session_err("adopt imported released snapshot", e)),
    };
    if changed != 1 {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    tracing::info!(
        session_id = %id,
        source_bytes = source.len(),
        current_bytes = current.len(),
        "released 0.8.10 blob imported and durably adopted in-transaction"
    );
    Ok(session)
}

/// Full-vector projection of the 0.8.11 splice-based [`StrandLayout`].
///
/// The continuity schema stores every strand as its complete message vector
/// (there is no strand-link table), so the append-only suffix/splice layout
/// is materialized back into full per-strand rows. The walk mirrors the
/// lineage validation in meerkat's own blob conversion: parent-transition
/// splice, parent-suffix extension, successor replacement splice, tail.
struct MaterializedBlobLayout {
    /// Full rows per strand, in first-appearance order. A strand id extended
    /// across rewrites (exact-append parents) holds its final, longest vector.
    strands: Vec<(TranscriptStrandId, Vec<Message>)>,
    rewrites: Vec<RewriteRow>,
    head_strand: TranscriptStrandId,
}

impl MaterializedBlobLayout {
    fn from_layout(
        id: &meerkat_core::types::SessionId,
        layout: &StrandLayout,
    ) -> Result<Self, SessionStoreError> {
        fn decode_rows(
            id: &meerkat_core::types::SessionId,
            rows: &[Vec<u8>],
        ) -> Result<Vec<Message>, SessionStoreError> {
            rows.iter()
                .map(|bytes| {
                    serde_json::from_slice::<Message>(bytes).map_err(|error| {
                        SessionStoreError::InvalidTranscriptRewrite {
                            id: id.clone(),
                            reason: format!("layout strand row does not decode: {error}"),
                        }
                    })
                })
                .collect()
        }
        fn splice_rows(
            id: &meerkat_core::types::SessionId,
            source: &[Message],
            start: u64,
            end: u64,
            replacement: &[Message],
        ) -> Result<Vec<Message>, SessionStoreError> {
            let start =
                usize::try_from(start).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            let end = usize::try_from(end).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            if start > end || end > source.len() {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            let mut rows = Vec::with_capacity(source.len() - (end - start) + replacement.len());
            rows.extend_from_slice(&source[..start]);
            rows.extend_from_slice(replacement);
            rows.extend_from_slice(&source[end..]);
            Ok(rows)
        }
        fn upsert(
            strands: &mut Vec<(TranscriptStrandId, Vec<Message>)>,
            strand: &TranscriptStrandId,
            rows: Vec<Message>,
        ) {
            if let Some(entry) = strands.iter_mut().find(|(sid, _)| sid == strand) {
                entry.1 = rows;
            } else {
                strands.push((strand.clone(), rows));
            }
        }

        let mut strands: Vec<(TranscriptStrandId, Vec<Message>)> = Vec::new();
        let mut current = decode_rows(id, &layout.serialized_anchor)?;
        let mut current_strand = layout.anchor_strand.clone();
        upsert(&mut strands, &current_strand, current.clone());
        let mut rewrites = Vec::with_capacity(layout.rewrites.len());
        for rewrite in &layout.rewrites {
            match &rewrite.parent_transition {
                meerkat_core::session_store::PreparedHeadCanonicalParentTransition::ExactAppend => {
                    if rewrite.parent_strand != current_strand {
                        return Err(SessionStoreError::Corrupted(id.clone()));
                    }
                }
                meerkat_core::session_store::PreparedHeadCanonicalParentTransition::ExactSplice(
                    parent_splice,
                ) => {
                    let link = parent_splice.link_splice();
                    let replacement = decode_rows(id, parent_splice.serialized_replacement())?;
                    current = splice_rows(
                        id,
                        &current,
                        link.splice_start,
                        link.splice_end,
                        &replacement,
                    )?;
                    current_strand = rewrite.parent_strand.clone();
                }
            }
            if u64::try_from(current.len()).map_err(|_| SessionStoreError::Corrupted(id.clone()))?
                != rewrite.parent_base_seq
            {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            current.extend(decode_rows(id, &rewrite.serialized_parent_suffix)?);
            let parent_len = u64::try_from(current.len())
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            upsert(&mut strands, &current_strand, current.clone());
            let replacement = decode_rows(id, &rewrite.serialized_replacement)?;
            current = splice_rows(
                id,
                &current,
                rewrite.link_splice.splice_start,
                rewrite.link_splice.successor_end,
                &replacement,
            )?;
            if u64::try_from(current.len()).map_err(|_| SessionStoreError::Corrupted(id.clone()))?
                != rewrite.link_splice.strand_len
            {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            current_strand = rewrite.strand.clone();
            upsert(&mut strands, &current_strand, current.clone());
            rewrites.push(RewriteRow {
                commit: rewrite.commit.clone(),
                parent_strand: rewrite.parent_strand.clone(),
                parent_len,
                strand: rewrite.strand.clone(),
                strand_len: rewrite.link_splice.strand_len,
            });
        }
        if layout.head_strand != current_strand {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        current.extend(decode_rows(id, &layout.serialized_tail)?);
        if u64::try_from(current.len()).map_err(|_| SessionStoreError::Corrupted(id.clone()))?
            != layout.head_len
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        upsert(&mut strands, &current_strand, current);
        Ok(Self {
            strands,
            rewrites,
            head_strand: layout.head_strand.clone(),
        })
    }
}

/// The whole-graph blob layout a head-canonical write persists, and its head.
///
/// The continuity schema keeps every rewrite row of a head-canonical session
/// and has no row for a retired base. A graph that retired its oldest
/// rewrite occurrences after compaction (a re-anchored graph) therefore has
/// no faithful conversion here and refuses typed, through Meerkat's
/// whole-graph layout. Reads lay such a blob out with
/// [`retained_layout_for_blob_session`].
fn layout_for_blob_session(
    session: &Session,
) -> Result<(MaterializedBlobLayout, SessionHead), SessionStoreError> {
    let history = session
        .validated_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("stored transcript history state is malformed: {err}"),
        })?;
    let layout = strand_layout_for_history(session, history.as_ref())?;
    let materialized = MaterializedBlobLayout::from_layout(session.id(), &layout)?;
    let head = SessionHead::from_session(
        session,
        materialized.head_strand.clone(),
        materialized.rewrites.len() as u64,
    )?;
    Ok((materialized, head))
}

/// The blob layout reads use, and the head they synthesize, through
/// Meerkat's retained layout: a re-anchored graph lays out from its
/// re-anchored anchor through its retained occurrences, as Meerkat's own
/// SQLite session store lays out the same blob. The head counts every
/// rewrite generation, the retired ones included, as the session's rewrite
/// prefix does. A whole graph lays out exactly as
/// [`layout_for_blob_session`] lays it out.
fn retained_layout_for_blob_session(
    session: &Session,
) -> Result<(MaterializedBlobLayout, SessionHead), SessionStoreError> {
    let history = session
        .validated_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("stored transcript history state is malformed: {err}"),
        })?;
    let retained = strand_layout_for_retained_history(session, history.as_ref())?;
    let materialized = MaterializedBlobLayout::from_layout(session.id(), &retained.layout)?;
    let rewrite_count = retained
        .retired_count()
        .checked_add(materialized.rewrites.len())
        .and_then(|count| u64::try_from(count).ok())
        .ok_or_else(|| SessionStoreError::Corrupted(session.id().clone()))?;
    let head = SessionHead::from_session(session, materialized.head_strand.clone(), rewrite_count)?;
    Ok((materialized, head))
}

/// One-time per-session migration inside the caller's transaction: lay the
/// legacy blob out as strands + rewrite rows + a head row. The blob row is
/// left untouched as a frozen archive and is never read again once the head
/// row exists. Reads never migrate.
fn migrate_legacy_blob_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
    identity: &AgentIdentity,
    generation: ContinuityGeneration,
    version: CheckpointVersion,
    fencing_token: FencingToken,
) -> Result<Option<StoredHead>, SessionStoreError> {
    let Some(session) = blob_session_in_txn(tx, id)? else {
        return Ok(None);
    };
    // IDENTITY EQUALITY, BEFORE ANY WRITE.
    //
    // The row key and the blob's own `Session.id` must agree. If they do not,
    // every write below would be misattributed: the orphan delete clears rows
    // for the ROW key while the strand/rewrite/head writes are laid out from
    // the DECODED session, so a row keyed A carrying session B can write — or
    // overwrite — B's head row. A later re-census refuses the ledger stamp,
    // but by then the damage is durable: a stamp guard is not a write guard.
    //
    // So this is checked here, at the top of the shared primitive, rather
    // than in any one caller: the lazy delta-write path reaches it too, and a
    // mismatch is equally a defect there. Refuse the row, write nothing, and
    // let the caller report it by key.
    if session.id() != id {
        tracing::error!(
            row_session_id = %id,
            blob_session_id = %session.id(),
            identity = %identity,
            "refusing legacy conversion: stored blob decodes to a DIFFERENT session than its row key"
        );
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    // Clear any ORPHAN rows first — strand/rewrite rows for this session
    // that no head row adopts.
    //
    // They exist because an append that creates no head state commits its
    // rows and leaves the ledger at v1 (the rollback-safety rule in
    // `delta_write`), so an interrupted creation window, or a rollback to a
    // previous release followed by a re-upgrade, can leave rows behind that
    // disagree with the blob this migration is about to lay out. Without
    // this, `insert_strand_rows_in_txn` would refuse the divergence as an
    // immutability violation and the session would be permanently
    // unwritable.
    //
    // Safe unconditionally: this function is reached only with NO head row
    // for `id`, and every read path gates on the head row, so nothing can
    // observe these rows. The blob is the authority being migrated.
    delete_orphan_head_canonical_rows_in_txn(tx, id)?;
    // This one-time conversion is the ONE phase of a boot guaranteed to be
    // slow (minutes of CPU on a large legacy document) and it previously
    // emitted nothing — a supervised deploy read the silence as a stalled
    // candidate and aborted its activation. Say what is happening, at entry
    // and completion, so a long migration is visibly a long migration.
    let started = std::time::Instant::now();
    tracing::info!(
        session_id = %id,
        identity = %identity,
        messages = session.messages().len(),
        "head-canonical conversion of a legacy blob starting"
    );
    let (layout, head) = layout_for_blob_session(&session)?;
    for (strand, rows) in &layout.strands {
        insert_strand_rows_in_txn(tx, id, strand, 0, rows, identity, generation)?;
    }
    for (idx, rewrite) in layout.rewrites.iter().enumerate() {
        insert_rewrite_row_in_txn(tx, id, idx as u64, rewrite, identity, generation)?;
    }
    let token = write_head_row_in_txn(tx, &head, identity, generation, version, fencing_token)?;
    tracing::info!(
        session_id = %id,
        identity = %identity,
        strands = layout.strands.len(),
        rewrite_rows = layout.rewrites.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "head-canonical conversion of a legacy blob complete"
    );
    Ok(Some((head, token)))
}

/// Head row if present, otherwise migrate a legacy blob in this transaction.
/// The FIRST delta write migrates; reads synthesize without writing.
fn ensure_head_canonical_for_write_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
    identity: &AgentIdentity,
    generation: ContinuityGeneration,
    version: CheckpointVersion,
    fencing_token: FencingToken,
) -> Result<Option<StoredHead>, SessionStoreError> {
    if let Some(existing) = head_row_in_txn(tx, id)? {
        return Ok(Some(existing));
    }
    migrate_legacy_blob_in_txn(tx, id, identity, generation, version, fencing_token)
}

fn materialize_slim_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
    head: &SessionHead,
) -> Result<Session, SessionStoreError> {
    let messages = strand_messages_in_txn(tx, id, &head.strand, 0..head.message_count)?;
    match head.clone().into_session(messages) {
        Ok(session) => Ok(session),
        // Released 0.8.10 HEAD ROW (session envelope v2): interpretable only
        // through the explicit one-time importer — the head-row lane of the
        // same contract `import_released_blob_in_txn` implements for whole
        // blobs. Every 0.8.10-written head refuses current materialization
        // (`Session::from_head_parts` fails typed on the envelope version),
        // so without this lane an entire released head-canonical fleet is
        // unreadable at resume (downstream app binding, 17/17 identities:
        // "failed to restore session from head row: ... expected current 3,
        // got 2").
        Err(restore_error)
            if head.version == super::contracts::RELEASED_0810_SESSION_ENVELOPE_VERSION =>
        {
            import_released_head_in_txn(
                tx,
                id,
                head,
                &format!("failed current materialization ({restore_error})"),
            )
        }
        Err(err) => Err(err),
    }
}

/// Serialized-verbatim released envelope, reassembled from the exact durable
/// parts a released 0.8.10 head row commits to. `messages` embeds the exact
/// strand row bytes (`RawValue`), never a re-serialization, so the importer
/// interprets precisely what the released writer persisted.
#[derive(serde::Serialize)]
struct ReleasedHeadEnvelope0810<'a> {
    version: u32,
    id: &'a meerkat_core::types::SessionId,
    messages: &'a [Box<serde_json::value::RawValue>],
    created_at: std::time::SystemTime,
    updated_at: std::time::SystemTime,
    metadata: &'a serde_json::Map<String, serde_json::Value>,
    usage: &'a meerkat_core::Usage,
}

/// One-time released-0.8.10 import for a HEAD-CANONICAL continuity document.
///
/// Same banked import contract as [`import_released_blob_in_txn`], adapted to
/// the head representation: the public core importer is the sole boundary
/// allowed to interpret released evidence, and every proof failure fails
/// closed with the original refusal surfaced (never a healed reading).
///
/// Proof chain, in order:
/// 1. The exact durable strand rows must be the rows the released head
///    committed to: `released_0810_transcript_serialized_rows_digest` (the
///    byte-faithful recomputation of the released transcript digest) must
///    equal `head.head_revision`.
/// 2. The released envelope is reassembled from those exact bytes plus the
///    head's own envelope facts (a released head inlines its full metadata
///    map — `metadata_identity` is a 0.8.11 concept), and handed to
///    `import_released_0810_session`, which re-validates the envelope
///    version and every released metadata shape.
/// 3. The receipt's source digest is re-proved against the exact bytes
///    interpreted, and its session id against the row key.
///
/// This runs on the read pool, so like the blob lane's read-only fallback it
/// serves the imported document WITHOUT durable adoption: the first
/// write-path decode observes the released head, fails its prefix-digest
/// probe against the current algorithm, and rebases the strand under a
/// current-format head — the durable adoption every later read observes.
fn import_released_head_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
    head: &SessionHead,
    refusal_context: &str,
) -> Result<Session, SessionStoreError> {
    use sha2::Digest as _;

    let raw_rows = strand_row_bytes_in_txn(tx, id, &head.strand, 0..head.message_count)?;
    let released_digest = meerkat_core::released_0810_transcript_serialized_rows_digest(&raw_rows)
        .map_err(|digest_error| {
            SessionStoreError::Serialization(format!(
                "continuity head row {id} {refusal_context} and \
                 its strand rows do not admit the released 0.8.10 digest ({digest_error})"
            ))
        })?;
    if released_digest != head.head_revision {
        return Err(SessionStoreError::Serialization(format!(
            "continuity head row {id} {refusal_context} and its \
             strand rows do not match the released head commitment (released digest \
             {released_digest}, head revision {})",
            head.head_revision
        )));
    }
    let messages = raw_rows
        .into_iter()
        .map(|bytes| {
            String::from_utf8(bytes)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))
                .and_then(|row| {
                    serde_json::value::RawValue::from_string(row)
                        .map_err(|_| SessionStoreError::Corrupted(id.clone()))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let envelope = serde_json::to_vec(&ReleasedHeadEnvelope0810 {
        version: head.version,
        id,
        messages: &messages,
        created_at: head.created_at,
        updated_at: head.updated_at,
        metadata: &head.metadata,
        usage: &head.usage,
    })
    .map_err(|e| SessionStoreError::Serialization(e.to_string()))?;
    let imported = meerkat_core::import_released_0810_session(&envelope).map_err(|import| {
        SessionStoreError::Serialization(format!(
            "continuity head row {id} {refusal_context} and does not interpret as a \
             released 0.8.10 head-canonical document either ({import})"
        ))
    })?;
    let (session, receipt) = imported.into_parts();
    let observed_sha256: [u8; 32] = sha2::Sha256::digest(&envelope).into();
    if receipt.source_document_sha256() != &observed_sha256 {
        return Err(SessionStoreError::Serialization(format!(
            "continuity head row {id} changed during exact released-0.8.10 import"
        )));
    }
    if receipt.session_id() != id {
        return Err(SessionStoreError::Serialization(format!(
            "continuity head key {id} contains released session {}",
            receipt.session_id()
        )));
    }
    drop(receipt);
    tracing::info!(
        session_id = %id,
        released_rows = head.message_count,
        "released 0.8.10 head-canonical document imported on load; durable adoption follows \
         the first write-path decode"
    );
    Ok(session)
}

/// One-time durable adoption of a released 0.8.10 head-canonical document,
/// inside the caller's WRITE transaction (see
/// `ContinuityIncrementalSessions::adopt_released_head_document`).
///
/// A released head with retained rewrites cannot authorize a current
/// mutation - its rewrite-generation authority predates the compact
/// graph/rewrite-prefix carriers, so `session_head_cas_token` refuses it
/// typed and every ordinary write arm is unreachable. Authorization here is
/// the import proof: the stored released document is re-proved through
/// [`import_released_head_in_txn`] (byte proof against the released head
/// commitment + the sanctioned importer + receipt re-proof), `incoming` must
/// be a legal successor of that imported reading (equal or append-extension;
/// the boundary guard refuses genuine divergence typed), and only then is the
/// released representation replaced wholesale with the current-format layout
/// of `incoming` - the same strand/rewrite/head writer the legacy-blob
/// migration uses, so rewrite-carrying documents lay out identically to a
/// converted blob.
fn adopt_released_head_in_txn(
    tx: &Transaction<'_>,
    incoming: &Session,
    identity: &AgentIdentity,
    generation: ContinuityGeneration,
    version: CheckpointVersion,
    fencing_token: FencingToken,
) -> Result<(), SessionStoreError> {
    let id = incoming.id();
    let Some((stored, _token)) = head_row_in_txn(tx, id)? else {
        return Err(SessionStoreError::Internal(format!(
            "released head adoption for session {id} found no durable head row; the adoption \
             lane is only reachable from a stored released head"
        )));
    };
    if stored.version != super::contracts::RELEASED_0810_SESSION_ENVELOPE_VERSION {
        return Err(SessionStoreError::Internal(format!(
            "released head adoption for session {id} found a current head (envelope version \
             {}); refusing to re-adopt a document the ordinary write arms already own",
            stored.version
        )));
    }
    let imported =
        import_released_head_in_txn(tx, id, &stored, "is being adopted on the write path")?;
    meerkat_core::session_store::append_only_save_guard(incoming, Some(&imported))?;
    // The released rows are being REPLACED wholesale inside this transaction;
    // nothing can observe the intermediate state, and the imported reading
    // above is the receipt-proved successor source. The head row itself is
    // upserted by `write_head_row_in_txn`.
    delete_orphan_head_canonical_rows_in_txn(tx, id)?;
    let started = std::time::Instant::now();
    let (layout, head) = layout_for_blob_session(incoming)?;
    for (strand, rows) in &layout.strands {
        insert_strand_rows_in_txn(tx, id, strand, 0, rows, identity, generation)?;
    }
    for (idx, rewrite) in layout.rewrites.iter().enumerate() {
        insert_rewrite_row_in_txn(tx, id, idx as u64, rewrite, identity, generation)?;
    }
    write_head_row_in_txn(tx, &head, identity, generation, version, fencing_token)?;
    tracing::info!(
        session_id = %id,
        released_rows = stored.message_count,
        released_rewrite_count = stored.rewrite_count,
        adopted_rows = head.message_count,
        adopted_rewrite_count = head.rewrite_count,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "released 0.8.10 head-canonical document durably adopted on the write path"
    );
    Ok(())
}

/// Head-canonical compat write for a WHOLE-document verb: delta-append when
/// the incoming transcript extends the persisted head strand, otherwise a
/// `rebase:` strand switch. The archived blob row is never touched.
fn write_head_canonical_session_in_txn(
    tx: &Transaction<'_>,
    session: &Session,
    head: &SessionHead,
    identity: &AgentIdentity,
    generation: ContinuityGeneration,
    version: CheckpointVersion,
    fencing_token: FencingToken,
) -> Result<(), SessionStoreError> {
    let id = session.id();
    let live = session.messages();
    let prev_count = usize::try_from(head.message_count)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let plain_append = live.len() >= prev_count
        && meerkat_core::transcript_messages_digest(&live[..prev_count])
            .map_err(SessionStoreError::from)?
            == head.head_revision;
    let new_head = if plain_append {
        if live.len() > prev_count {
            insert_strand_rows_in_txn(
                tx,
                id,
                &head.strand,
                head.message_count,
                &live[prev_count..],
                identity,
                generation,
            )?;
        }
        // The successor head must commit to the EXACT durable row bytes.
        // Rows 0..prev_count keep the serialization they were written with,
        // which need not equal re-encoding the same typed Messages today, so
        // the stored commitment is EXTENDED by only the appended rows' bytes
        // - mirrors meerkat-core
        // `SessionHead::from_session_with_proved_inline_storage_authority`, the
        // published seam for retained boundaries whose exact row bytes may
        // use an older representation. Re-minting via `from_session` breaks
        // `SessionHead::into_session`'s byte-exact prefix verification on
        // the next cold materialization.
        match head.message_row_prefix.clone() {
            Some(prefix) => {
                let appended_serialized = live[prev_count..]
                    .iter()
                    .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
                    .collect::<Result<Vec<_>, _>>()?;
                let proved = prefix.extend_serialized_rows(&appended_serialized)?;
                SessionHead::from_session_with_proved_inline_storage_authority(
                    session,
                    head.strand.clone(),
                    head.rewrite_prefix.clone(),
                    proved,
                )?
            }
            None => {
                // A pre-0.8.11 head whose row identity was never proved
                // stays unproved rather than inventing a commitment the
                // stored rows may not satisfy.
                let mut unproved =
                    SessionHead::from_session(session, head.strand.clone(), head.rewrite_count)?;
                unproved.message_row_prefix = None;
                unproved.row_lineage_anchor = None;
                unproved
            }
        }
    } else {
        let live_digest =
            meerkat_core::transcript_messages_digest(live).map_err(SessionStoreError::from)?;
        let rebased = TranscriptStrandId::rebase(&live_digest);
        insert_strand_rows_in_txn(tx, id, &rebased, 0, live, identity, generation)?;
        // A fresh strand: every row was just written from these exact
        // instances, so the minted commitment matches the durable bytes.
        SessionHead::from_session(session, rebased, head.rewrite_count)?
    };
    write_head_row_in_txn(tx, &new_head, identity, generation, version, fencing_token)?;
    Ok(())
}

/// The continuity write discipline, enforced inside the same transaction as
/// the rows it authorizes. Byte-for-byte the checks
/// `save_session_snapshot_owned` runs for a whole-blob save, so a delta
/// mutation can never be accepted where a whole-document save would be
/// refused (or the reverse). Returns whether a continuity record exists for
/// the identity — the caller advances it in the same transaction.
fn enforce_continuity_cursor_in_txn(
    tx: &Transaction<'_>,
    identity: &AgentIdentity,
    session_id: &meerkat_core::types::SessionId,
    generation: ContinuityGeneration,
    version: CheckpointVersion,
    fencing_token: FencingToken,
) -> Result<bool, ContinuityStoreError> {
    let existing = tx
        .query_row(
            "SELECT session_id, generation, fencing_token, checkpoint_version
             FROM continuity_records WHERE identity = ?1",
            rusqlite::params![identity.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|e| sqlite_err("query continuity record", e))?;

    let record_was_present = existing.is_some();
    if let Some((current_session_id, current_generation, current_token, current_version)) = existing
    {
        if current_session_id != session_id.to_string() || current_generation != generation.get() {
            return Err(ContinuityStoreError::NotFound {
                identity: identity.clone(),
            });
        }
        if fencing_token.get() < current_token {
            return Err(ContinuityStoreError::StaleFencingToken {
                identity: identity.clone(),
                presented: fencing_token,
                current: FencingToken::new(current_token),
            });
        }
        if version.get() <= current_version {
            return Err(ContinuityStoreError::StaleCheckpointVersion {
                identity: identity.clone(),
                presented: version,
                current: CheckpointVersion::new(current_version),
            });
        }
    }
    Ok(record_was_present)
}

fn advance_continuity_record_in_txn(
    tx: &Transaction<'_>,
    identity: &AgentIdentity,
    session_id: &meerkat_core::types::SessionId,
    generation: ContinuityGeneration,
    version: CheckpointVersion,
    fencing_token: FencingToken,
    record_was_present: bool,
) -> Result<(), ContinuityStoreError> {
    tx.execute(
        "UPDATE continuity_records
         SET checkpoint_version = ?1, fencing_token = ?2
         WHERE identity = ?3 AND session_id = ?4 AND generation = ?5",
        rusqlite::params![
            version.get(),
            fencing_token.get(),
            identity.as_str(),
            session_id.to_string(),
            generation.get(),
        ],
    )
    .map_err(|e| sqlite_err("advance continuity record", e))?;
    if record_was_present && tx.changes() == 0 {
        return Err(ContinuityStoreError::NotFound {
            identity: identity.clone(),
        });
    }
    Ok(())
}

/// The blob row's ownership check: a session's `session_snapshots` row may
/// never be written — nor superseded by head rows — under a different
/// identity or a different generation than the one that owns it.
///
/// ONE function, called by BOTH write paths (`save_session_snapshot_owned`
/// and every `delta_write`), because two write paths onto the same durable
/// session must not have different accept/reject boundaries. A generation
/// bump always mints a fresh session id, so a mismatch here is genuine
/// cross-owner corruption, never an ordinary lifecycle transition.
fn ensure_snapshot_owner_in_txn(
    tx: &Transaction<'_>,
    session_id: &meerkat_core::types::SessionId,
    identity: &AgentIdentity,
    generation: ContinuityGeneration,
) -> Result<(), ContinuityStoreError> {
    let existing_snapshot_owner = tx
        .query_row(
            "SELECT identity, generation FROM session_snapshots WHERE session_id = ?1",
            rusqlite::params![session_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
        )
        .optional()
        .map_err(|e| sqlite_err("query snapshot owner", e))?;
    if let Some((snapshot_identity, snapshot_generation)) = existing_snapshot_owner
        && (snapshot_identity != identity.as_str() || snapshot_generation != generation.get())
    {
        return Err(ContinuityStoreError::Corruption(format!(
            "session snapshot {session_id} is owned by {snapshot_identity}/generation \
             {snapshot_generation}, not {identity}/generation {generation}"
        )));
    }
    Ok(())
}

/// The head row's ownership check, mirroring the whole-blob path's
/// snapshot-owner corruption check: a session's durable representation may
/// never be written by a different identity or a different generation.
fn ensure_head_owner_in_txn(
    tx: &Transaction<'_>,
    session_id: &meerkat_core::types::SessionId,
    identity: &AgentIdentity,
    generation: ContinuityGeneration,
) -> Result<(), ContinuityStoreError> {
    if let Some((head_identity, head_generation)) = head_owner_in_txn(tx, session_id)?
        && (head_identity != identity.as_str() || head_generation != generation.get())
    {
        return Err(ContinuityStoreError::Corruption(format!(
            "session head {session_id} is owned by {head_identity}/generation \
             {head_generation}, not {identity}/generation {generation}"
        )));
    }
    Ok(())
}

/// Drop the strand and rewrite rows of a session that has NO head row.
///
/// Callers must have established that (`migrate_legacy_blob_in_txn` is the
/// only one, and it runs only when `head_row_in_txn` returned `None`). The
/// head table is deliberately untouched: this clears orphans, it is not a
/// session delete.
fn delete_orphan_head_canonical_rows_in_txn(
    tx: &Transaction<'_>,
    id: &meerkat_core::types::SessionId,
) -> Result<(), SessionStoreError> {
    for table in ["continuity_strand_messages", "continuity_session_rewrites"] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE session_id = ?1"),
            rusqlite::params![id.to_string()],
        )
        .map_err(|e| sqlite_session_err("delete orphan head-canonical rows", e))?;
    }
    Ok(())
}

fn delete_head_canonical_rows_in_txn(
    tx: &Transaction<'_>,
    predicate: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<(), ContinuityStoreError> {
    for table in [
        "continuity_session_heads",
        "continuity_strand_messages",
        "continuity_session_rewrites",
    ] {
        tx.execute(&format!("DELETE FROM {table} WHERE {predicate}"), params)
            .map_err(|e| sqlite_err("delete head-canonical rows", e))?;
    }
    Ok(())
}

#[async_trait]
impl ContinuityStore for LocalContinuityStore {
    #[cfg(feature = "extension-state")]
    async fn establish_identity_history_coverage(
        &self,
        mob: &str,
        cursor: u64,
    ) -> Result<(), ContinuityStoreError> {
        let mob = mob.to_string();
        self.run_blocking("identity history coverage", move |inner| inner.with_writer(|connection| {
            connection.execute("INSERT OR IGNORE INTO continuity_identity_coverage(mob_id,after_cursor) VALUES(?1,?2)", rusqlite::params![mob, cursor]).map_err(|e| sqlite_err("coverage anchor", e))?;
            Ok(())
        })).await
    }

    #[cfg(feature = "extension-state")]
    async fn identity_history_covers_birth(
        &self,
        mob: &str,
        birth_cursor: u64,
    ) -> Result<bool, ContinuityStoreError> {
        let mob = mob.to_string();
        self.run_blocking("identity history covers birth", move |inner| {
            inner.with_reader(|connection| {
                let anchor: Option<u64> = connection
                    .query_row(
                        "SELECT after_cursor FROM continuity_identity_coverage WHERE mob_id=?1",
                        [mob],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| sqlite_err("coverage read", e))?;
                Ok(anchor.is_some_and(|cursor| birth_cursor > cursor))
            })
        })
        .await
    }
    #[cfg(feature = "extension-state")]
    async fn enable_identity_binding_history(&self) -> Result<(), ContinuityStoreError> {
        self.run_blocking("identity binding history", |inner| {
            inner.with_writer(|connection| {
                meerkat_sqlite::apply_domain_migrations(connection, &META_CONTINUITY_DOMAIN)
                    .map_err(|e| mechanics_err("identity history schema", e))?;
                inner.head_canonical_schema.store(true, Ordering::Release);
                inner.head_canonical_ledger.store(true, Ordering::Release);
                Ok(())
            })
        })
        .await
    }

    #[cfg(feature = "extension-state")]
    async fn has_historical_identity(
        &self,
        identity: &AgentIdentity,
    ) -> Result<bool, ContinuityStoreError> {
        let identity = identity.to_string();
        self.run_blocking("historical identity intent", move |inner| inner.with_reader(|connection| {
            connection.query_row("SELECT EXISTS(SELECT 1 FROM continuity_identity_history WHERE identity=?1)", [identity], |row| row.get(0))
                .map_err(|e| sqlite_err("identity intent history", e))
        })).await
    }

    #[cfg(feature = "extension-state")]
    async fn historical_identity_binding(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<Option<AgentIdentity>, ContinuityStoreError> {
        let id = session_id.to_string();
        self.run_blocking("historical identity binding", move |inner| {
            inner.with_reader(|connection| {
                let identity: Option<String> = connection
                    .query_row(
                        "SELECT identity FROM continuity_identity_history WHERE session_id=?1",
                        [id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| sqlite_err("identity history read", e))?;
                identity
                    .map(|identity| {
                        AgentIdentity::parse(&identity)
                            .map_err(|e| ContinuityStoreError::Corruption(e.to_string()))
                    })
                    .transpose()
            })
        })
        .await
    }
    async fn resolve_many(
        &self,
        identities: &[AgentIdentity],
    ) -> Result<BTreeMap<AgentIdentity, ContinuityResolveState>, ContinuityStoreError> {
        let identities = identities.to_vec();
        self.run_blocking("resolve_many", move |inner| {
            inner.with_reader(|connection| {
                let mut map = BTreeMap::new();
                for id in &identities {
                    let mut stmt = connection
                        .prepare_cached(
                            "SELECT agent_runtime_id, session_id, generation, checkpoint_version
                             FROM continuity_records WHERE identity = ?1",
                        )
                        .map_err(|e| sqlite_err("prepare", e))?;
                    let row = stmt
                        .query_row(rusqlite::params![id.as_str()], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, u64>(2)?,
                                row.get::<_, u64>(3)?,
                            ))
                        })
                        .optional()
                        .map_err(|e| sqlite_err("query", e))?;
                    match row {
                        Some((runtime_id, session_id_str, generation, cpv)) => {
                            let record = ContinuityRecord {
                                identity: id.clone(),
                                agent_runtime_id: AgentRuntimeId::parse(&runtime_id).map_err(
                                    |e| {
                                        ContinuityStoreError::Corruption(format!(
                                            "invalid runtime_id in store: {e}"
                                        ))
                                    },
                                )?,
                                session_id: meerkat_core::types::SessionId::parse(&session_id_str)
                                    .map_err(|e| {
                                        ContinuityStoreError::Corruption(format!(
                                            "invalid session_id in store: {e}"
                                        ))
                                    })?,
                                generation: ContinuityGeneration::new(generation),
                                checkpoint_version: CheckpointVersion::new(cpv),
                            };
                            map.insert(id.clone(), ContinuityResolveState::Ready { record });
                        }
                        None => {
                            map.insert(id.clone(), ContinuityResolveState::Uninitialized);
                        }
                    }
                }
                Ok(map)
            })
        })
        .await
    }

    async fn resolve_record_by_session(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<(ContinuityRecord, FencingToken, CheckpointVersion)>, ContinuityStoreError>
    {
        let session_id = session_id.clone();
        self.run_blocking("resolve_record_by_session", move |inner| {
            inner.with_reader(|connection| {
                let mut stmt = connection
                    .prepare_cached(
                        "SELECT identity, agent_runtime_id, generation, checkpoint_version, \
                         fencing_token FROM continuity_records WHERE session_id = ?1",
                    )
                    .map_err(|e| sqlite_err("prepare", e))?;
                let row = stmt
                    .query_row(rusqlite::params![session_id.to_string()], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, u64>(2)?,
                            row.get::<_, u64>(3)?,
                            row.get::<_, u64>(4)?,
                        ))
                    })
                    .optional()
                    .map_err(|e| sqlite_err("query", e))?;
                let Some((identity, runtime_id, generation, cpv, token)) = row else {
                    return Ok(None);
                };
                // The substrate's CURRENT checkpoint version for the session:
                // the fence the next write cursor must advance past. The
                // record's own stamp trails it whenever writes landed after
                // the last checkpoint. The head-canonical arm exists only once
                // the file carries the channel (the v2 DDL waits for the first
                // delta write), so a v1 file's fence is its snapshots alone:
                // reading the absent table failed the lookup for every member
                // that had not run a turn before its last shutdown.
                let sql = if inner.head_tables_available(connection)? {
                    "SELECT MAX(v) FROM (\
                         SELECT COALESCE(MAX(checkpoint_version), 0) AS v \
                             FROM session_snapshots WHERE session_id = ?1 \
                         UNION ALL \
                         SELECT COALESCE(MAX(checkpoint_version), 0) AS v \
                             FROM continuity_session_heads WHERE session_id = ?1)"
                } else {
                    "SELECT COALESCE(MAX(checkpoint_version), 0) \
                         FROM session_snapshots WHERE session_id = ?1"
                };
                let fence_current: u64 = connection
                    .query_row(sql, rusqlite::params![session_id.to_string()], |row| {
                        row.get(0)
                    })
                    .map_err(|e| sqlite_err("fence query", e))?;
                let fence_current = fence_current.max(cpv);
                let record = ContinuityRecord {
                    identity: AgentIdentity::parse(&identity).map_err(|e| {
                        ContinuityStoreError::Corruption(format!("invalid identity in store: {e}"))
                    })?,
                    agent_runtime_id: AgentRuntimeId::parse(&runtime_id).map_err(|e| {
                        ContinuityStoreError::Corruption(format!(
                            "invalid runtime_id in store: {e}"
                        ))
                    })?,
                    session_id: session_id.clone(),
                    generation: ContinuityGeneration::new(generation),
                    checkpoint_version: CheckpointVersion::new(cpv),
                };
                Ok(Some((
                    record,
                    FencingToken::new(token),
                    CheckpointVersion::new(fence_current),
                )))
            })
        })
        .await
    }

    async fn session_owner(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<AgentIdentity>, ContinuityStoreError> {
        let session_id = session_id.clone();
        self.run_blocking("session_owner", move |inner| {
            inner.with_reader(|connection| {
                // Rotation changes the current binding, but retained snapshot and
                // head rows keep the original session owner. Read all available
                // owner facts in one statement and refuse inconsistent storage.
                let sql = if inner.head_tables_available(connection)? {
                    "SELECT identity FROM session_snapshots WHERE session_id = ?1 \
                     UNION ALL SELECT identity FROM continuity_session_heads WHERE session_id = ?1 \
                     UNION ALL SELECT identity FROM continuity_records WHERE session_id = ?1"
                } else {
                    "SELECT identity FROM session_snapshots WHERE session_id = ?1 \
                     UNION ALL SELECT identity FROM continuity_records WHERE session_id = ?1"
                };
                let mut statement = connection
                    .prepare_cached(sql)
                    .map_err(|e| sqlite_err("prepare session owner", e))?;
                let rows = statement
                    .query_map(rusqlite::params![session_id.to_string()], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(|e| sqlite_err("query session owner", e))?;
                let mut owner = None;
                for row in rows {
                    let identity = row.map_err(|e| sqlite_err("read session owner", e))?;
                    let identity = AgentIdentity::parse(&identity).map_err(|e| {
                        ContinuityStoreError::Corruption(format!(
                            "invalid session owner in store: {e}"
                        ))
                    })?;
                    if owner.as_ref().is_some_and(|existing| existing != &identity) {
                        return Err(ContinuityStoreError::Corruption(format!(
                            "conflicting owners for session {session_id}"
                        )));
                    }
                    owner = Some(identity);
                }
                Ok(owner)
            })
        })
        .await
    }

    async fn load_session_snapshot(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<SessionSnapshot>, ContinuityStoreError> {
        let session_id = session_id.clone();
        self.run_blocking("load_session_snapshot", move |inner| {
            inner.with_reader(|connection| {
                // Head-canonical sessions serve the slim materialization of
                // head+rows; their `session_snapshots` row (if any) is a
                // frozen archive and is never read again.
                if inner.head_tables_available(connection)? {
                    let tx = connection
                        .unchecked_transaction()
                        .map_err(|e| sqlite_err("begin read tx", e))?;
                    if let Some((head, _token)) = head_row_in_txn(&tx, &session_id)
                        .map_err(|e| ContinuityStoreError::Io(e.to_string()))?
                    {
                        let session = materialize_slim_in_txn(&tx, &session_id, &head)
                            .map_err(|e| ContinuityStoreError::Io(e.to_string()))?;
                        let data = serde_json::to_vec(&session).map_err(|e| {
                            ContinuityStoreError::Io(format!(
                                "serialize head-canonical session snapshot: {e}"
                            ))
                        })?;
                        return Ok(Some(SessionSnapshot { data }));
                    }
                }
                let mut stmt = connection
                    .prepare_cached("SELECT data FROM session_snapshots WHERE session_id = ?1")
                    .map_err(|e| sqlite_err("prepare", e))?;
                let row = stmt
                    .query_row(rusqlite::params![session_id.to_string()], |row| {
                        row.get::<_, Vec<u8>>(0)
                    })
                    .optional()
                    .map_err(|e| sqlite_err("query", e))?;
                Ok(row.map(|data| SessionSnapshot { data }))
            })
        })
        .await
    }

    async fn session_snapshot_matches_current(
        &self,
        candidate: SessionSnapshotMatchCandidate,
    ) -> Result<bool, ContinuityStoreError> {
        self.run_blocking("session_snapshot_matches_current", move |inner| {
            inner.with_reader(|connection| {
                // The whole-blob byte-equality probe is a blob-authority
                // concept. On a head-canonical session there is no candidate
                // blob to compare against, so the conservative trait default
                // applies and the caller takes its ordinary guard path.
                if inner.head_tables_available(connection)?
                    && head_owner_in_txn(
                        &connection
                            .unchecked_transaction()
                            .map_err(|e| sqlite_err("begin read tx", e))?,
                        &candidate.session_id,
                    )?
                    .is_some()
                {
                    return Ok(false);
                }
                connection
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1
                            FROM session_snapshots AS snapshot
                            JOIN continuity_records AS continuity
                              ON continuity.identity = snapshot.identity
                             AND continuity.session_id = snapshot.session_id
                             AND continuity.generation = snapshot.generation
                             AND continuity.checkpoint_version = snapshot.checkpoint_version
                            WHERE snapshot.session_id = ?1
                              AND snapshot.identity = ?2
                              AND snapshot.generation = ?3
                              AND snapshot.checkpoint_version = ?4
                              AND continuity.fencing_token = ?5
                              AND snapshot.fencing_token <= ?5
                              AND snapshot.data = ?6
                        )",
                        rusqlite::params![
                            candidate.session_id.to_string(),
                            candidate.identity.as_str(),
                            candidate.generation.get(),
                            candidate.checkpoint_version.get(),
                            candidate.fencing_token.get(),
                            &candidate.snapshot.data,
                        ],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(|e| sqlite_err("match session snapshot", e))
            })
        })
        .await
    }

    async fn delete_session_snapshot_if_current_revision(
        &self,
        session_id: &meerkat_core::types::SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, ContinuityStoreError> {
        let session_id = session_id.clone();
        let expected_current_revision = expected_current_revision.to_string();
        self.run_blocking(
            "delete_session_snapshot_if_current_revision",
            move |inner| {
                inner.with_writer(|connection| {
                    let head_tables = inner.head_tables_available(connection)?;
                    let tx = connection
                        .transaction()
                        .map_err(|e| sqlite_err("begin tx", e))?;

                    // Head-canonical sessions derive the CAS token from the
                    // slim materialization of head+rows, then drop head,
                    // strands, rewrites AND the frozen archive in one tx.
                    let head = if head_tables {
                        head_row_in_txn(&tx, &session_id)
                            .map_err(|e| ContinuityStoreError::Io(e.to_string()))?
                    } else {
                        None
                    };
                    let session = match head.as_ref() {
                        Some((head, _token)) => Some(
                            materialize_slim_in_txn(&tx, &session_id, head)
                                .map_err(|e| ContinuityStoreError::Io(e.to_string()))?,
                        ),
                        None => {
                            let data = tx
                                .query_row(
                                    "SELECT data FROM session_snapshots WHERE session_id = ?1",
                                    rusqlite::params![session_id.to_string()],
                                    |row| row.get::<_, Vec<u8>>(0),
                                )
                                .optional()
                                .map_err(|e| sqlite_err("query snapshot", e))?;
                            match data {
                                Some(data) => {
                                    Some(serde_json::from_slice::<Session>(&data).map_err(|e| {
                                        ContinuityStoreError::Io(format!(
                                            "deserialize session snapshot for revision check: {e}"
                                        ))
                                    })?)
                                }
                                None => None,
                            }
                        }
                    };

                    let Some(session) = session else {
                        return Ok(false);
                    };
                    let current_revision =
                        meerkat_core::session_store::session_projection_cas_token(&session)
                            .map_err(|e| ContinuityStoreError::Io(e.to_string()))?;
                    if current_revision != expected_current_revision {
                        return Ok(false);
                    }

                    let deleted = tx
                        .execute(
                            "DELETE FROM session_snapshots WHERE session_id = ?1",
                            rusqlite::params![session_id.to_string()],
                        )
                        .map_err(|e| sqlite_err("delete snapshot", e))?;
                    let head_deleted = if head_tables {
                        delete_head_canonical_rows_in_txn(
                            &tx,
                            "session_id = ?1",
                            rusqlite::params![session_id.to_string()],
                        )?;
                        head.is_some()
                    } else {
                        false
                    };
                    tx.commit()
                        .map_err(|e| sqlite_err("commit snapshot delete", e))?;
                    Ok(deleted > 0 || head_deleted)
                })
            },
        )
        .await
    }

    async fn save_session_snapshot(
        &self,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        generation: ContinuityGeneration,
        version: CheckpointVersion,
        fencing_token: FencingToken,
        snapshot: &SessionSnapshot,
    ) -> Result<(), ContinuityStoreError> {
        self.save_session_snapshot_owned(
            identity.clone(),
            session_id.clone(),
            generation,
            version,
            fencing_token,
            snapshot.clone(),
        )
        .await
    }

    async fn save_session_snapshot_owned(
        &self,
        identity: AgentIdentity,
        session_id: meerkat_core::types::SessionId,
        generation: ContinuityGeneration,
        version: CheckpointVersion,
        fencing_token: FencingToken,
        snapshot: SessionSnapshot,
    ) -> Result<(), ContinuityStoreError> {
        self.run_blocking("save_session_snapshot", move |inner| {
            inner.with_writer(|connection| {
                let head_tables = inner.head_tables_available(connection)?;
                // Keep the check, snapshot write, and record version/fence
                // advance in one writer transaction.
                let tx = connection
                    .unchecked_transaction()
                    .map_err(|e| sqlite_err("begin tx", e))?;

                let record_was_present = enforce_continuity_cursor_in_txn(
                    &tx,
                    &identity,
                    &session_id,
                    generation,
                    version,
                    fencing_token,
                )?;

                ensure_snapshot_owner_in_txn(&tx, &session_id, &identity, generation)?;
                if head_tables {
                    ensure_head_owner_in_txn(&tx, &session_id, &identity, generation)?;
                }

                // Representation-aware write. A head row means head+rows are
                // this session's byte authority: convert the incoming
                // document into delta rows + a small head instead of
                // upserting the blob, and leave the frozen archive row
                // untouched. Without a head row the legacy blob semantics
                // are byte-for-byte unchanged — an ordinary whole-document
                // save never migrates a session and never stamps ledger v2.
                let head = if head_tables {
                    head_row_in_txn(&tx, &session_id)
                        .map_err(|e| ContinuityStoreError::Io(e.to_string()))?
                } else {
                    None
                };
                match head {
                    Some((head, _token)) => {
                        // Once head+rows are a session's byte authority, a
                        // whole-document write has to be expressible as rows.
                        // Falling back to the blob row here would be the
                        // two-write-authorities failure the representation
                        // rule exists to prevent (the blob would be silently
                        // never read again), so this refuses instead.
                        let session: Session = serde_json::from_slice(&snapshot.data)
                            .map_err(|e| {
                                ContinuityStoreError::Io(format!(
                                    "session {session_id} is head-canonical: a whole-document \
                                     save must carry a serialized session document, not opaque \
                                     bytes ({e})"
                                ))
                            })?;
                        if session.id() != &session_id {
                            return Err(ContinuityStoreError::Corruption(format!(
                                "session snapshot for {session_id} carries session {}",
                                session.id()
                            )));
                        }
                        write_head_canonical_session_in_txn(
                            &tx,
                            &session,
                            &head,
                            &identity,
                            generation,
                            version,
                            fencing_token,
                        )
                        .map_err(|e| ContinuityStoreError::Io(e.to_string()))?;
                    }
                    None => {
                        tx.execute(
                            "INSERT INTO session_snapshots (session_id, identity, generation, checkpoint_version, fencing_token, data)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                             ON CONFLICT(session_id) DO UPDATE SET
                                identity = excluded.identity,
                                generation = excluded.generation,
                                checkpoint_version = excluded.checkpoint_version,
                                fencing_token = excluded.fencing_token,
                                data = excluded.data",
                            rusqlite::params![
                                session_id.to_string(),
                                identity.as_str(),
                                generation.get(),
                                version.get(),
                                fencing_token.get(),
                                &snapshot.data,
                            ],
                        )
                        .map_err(|e| sqlite_err("upsert snapshot", e))?;
                    }
                }

                advance_continuity_record_in_txn(
                    &tx,
                    &identity,
                    &session_id,
                    generation,
                    version,
                    fencing_token,
                    record_was_present,
                )?;

                tx.commit()
                    .map_err(|e| sqlite_err("commit tx", e))?;
                Ok(())
            })
        })
        .await
    }

    async fn upsert_continuity_record(
        &self,
        record: &ContinuityRecord,
        fencing_token: FencingToken,
    ) -> Result<(), ContinuityStoreError> {
        let record = record.clone();
        self.run_blocking("upsert_continuity_record", move |inner| {
            inner.with_writer(|connection| {
                let mut stmt = connection
                    .prepare_cached(
                        "SELECT fencing_token, generation FROM continuity_records WHERE identity = ?1",
                    )
                    .map_err(|e| sqlite_err("prepare", e))?;
                let existing = stmt
                    .query_row(rusqlite::params![record.identity.as_str()], |row| {
                        Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?))
                    })
                    .optional()
                    .map_err(|e| sqlite_err("query", e))?;
                drop(stmt);

                if let Some((current_token, current_generation)) = existing {
                    if fencing_token.get() < current_token {
                        return Err(ContinuityStoreError::StaleFencingToken {
                            identity: record.identity.clone(),
                            presented: fencing_token,
                            current: FencingToken::new(current_token),
                        });
                    }
                    if record.generation.get() < current_generation {
                        return Err(ContinuityStoreError::StaleContinuityGeneration {
                            identity: record.identity.clone(),
                            presented: record.generation,
                            current: ContinuityGeneration::new(current_generation),
                        });
                    }
                }

                connection
                    .execute(
                        "INSERT INTO continuity_records (identity, agent_runtime_id, session_id, generation, checkpoint_version, fencing_token)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(identity) DO UPDATE SET
                            agent_runtime_id = excluded.agent_runtime_id,
                            session_id = excluded.session_id,
                            generation = excluded.generation,
                            checkpoint_version = CASE
                                WHEN continuity_records.generation = excluded.generation
                                THEN MAX(continuity_records.checkpoint_version, excluded.checkpoint_version)
                                ELSE excluded.checkpoint_version
                            END,
                            fencing_token = excluded.fencing_token",
                        rusqlite::params![
                            record.identity.as_str(),
                            record.agent_runtime_id.as_str(),
                            record.session_id.to_string(),
                            record.generation.get(),
                            record.checkpoint_version.get(),
                            fencing_token.get(),
                        ],
                    )
                    .map_err(|e| sqlite_err("upsert record", e))?;
                Ok(())
            })
        })
        .await
    }

    async fn rollback_continuity_record(
        &self,
        expected_attempt: &ContinuityRecord,
        previous: Option<&ContinuityRecord>,
        fencing_token: FencingToken,
    ) -> Result<(), ContinuityStoreError> {
        let expected_attempt = expected_attempt.clone();
        let previous = previous.cloned();
        self.run_blocking("rollback_continuity_record", move |inner| {
            inner.with_writer(|connection| {
                let head_tables = inner.head_tables_available(connection)?;
                if previous
                    .as_ref()
                    .is_some_and(|record| record.identity != expected_attempt.identity)
                {
                    return Err(ContinuityStoreError::Corruption(format!(
                        "reset rollback identity mismatch: attempted {}, previous {}",
                        expected_attempt.identity,
                        previous
                            .as_ref()
                            .map(|record| record.identity.as_str())
                            .unwrap_or_default(),
                    )));
                }

                let tx = connection
                    .unchecked_transaction()
                    .map_err(|e| sqlite_err("begin tx", e))?;
                let current = tx
                    .query_row(
                        "SELECT agent_runtime_id, session_id, generation, fencing_token
                         FROM continuity_records WHERE identity = ?1",
                        rusqlite::params![expected_attempt.identity.as_str()],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, u64>(2)?,
                                row.get::<_, u64>(3)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|e| sqlite_err("query", e))?
                    .ok_or_else(|| ContinuityStoreError::NotFound {
                        identity: expected_attempt.identity.clone(),
                    })?;

                let (current_runtime_id, current_session_id, current_generation, current_token) =
                    current;
                if current_token != fencing_token.get() {
                    return Err(ContinuityStoreError::StaleFencingToken {
                        identity: expected_attempt.identity.clone(),
                        presented: fencing_token,
                        current: FencingToken::new(current_token),
                    });
                }
                if current_runtime_id != expected_attempt.agent_runtime_id.as_str()
                    || current_session_id != expected_attempt.session_id.to_string()
                    || current_generation != expected_attempt.generation.get()
                {
                    return Err(ContinuityStoreError::StaleContinuityGeneration {
                        identity: expected_attempt.identity.clone(),
                        presented: expected_attempt.generation,
                        current: ContinuityGeneration::new(current_generation),
                    });
                }

                // Only the provisional reset generation is abandoned. Older
                // snapshots — blob rows AND head-canonical head/strand/
                // rewrite rows alike — remain the rollback authority for the
                // restored row, while a concurrently advanced generation is
                // protected by the exact CAS above.
                tx.execute(
                    "DELETE FROM session_snapshots WHERE identity = ?1 AND generation = ?2",
                    rusqlite::params![
                        expected_attempt.identity.as_str(),
                        expected_attempt.generation.get(),
                    ],
                )
                .map_err(|e| sqlite_err("delete attempted snapshots", e))?;
                if head_tables {
                    delete_head_canonical_rows_in_txn(
                        &tx,
                        "identity = ?1 AND generation = ?2",
                        rusqlite::params![
                            expected_attempt.identity.as_str(),
                            expected_attempt.generation.get(),
                        ],
                    )?;
                }

                if let Some(previous) = previous {
                    tx.execute(
                        "UPDATE continuity_records
                         SET agent_runtime_id = ?1,
                             session_id = ?2,
                             generation = ?3,
                             checkpoint_version = ?4,
                             fencing_token = ?5
                         WHERE identity = ?6",
                        rusqlite::params![
                            previous.agent_runtime_id.as_str(),
                            previous.session_id.to_string(),
                            previous.generation.get(),
                            previous.checkpoint_version.get(),
                            fencing_token.get(),
                            expected_attempt.identity.as_str(),
                        ],
                    )
                    .map_err(|e| sqlite_err("restore previous record", e))?;
                } else {
                    tx.execute(
                        "DELETE FROM continuity_records WHERE identity = ?1",
                        rusqlite::params![expected_attempt.identity.as_str()],
                    )
                    .map_err(|e| sqlite_err("delete attempted record", e))?;
                }

                tx.commit().map_err(|e| sqlite_err("commit tx", e))?;
                Ok(())
            })
        })
        .await
    }

    /// M4b landed: the bundled store ships the session-delta channel because
    /// head+rows are now its canonical durable session representation, not a
    /// second authority beside `session_snapshots.data`.
    ///
    /// The deferral note this replaces named the exact hazard — a delta
    /// channel bolted beside the blob would create two write authorities
    /// over one session with no reconciliation rule. The rule now exists and
    /// every whole-snapshot verb honors it: a
    /// `continuity_session_heads` row means head+rows are the sole byte
    /// authority for that session, its blob row is a frozen archive that is
    /// never read or written again, whole-document saves convert into delta
    /// rows + a head, the exact-match probe declines, CAS tokens derive from
    /// the slim materialization, and delete/rollback scope all four tables.
    ///
    /// Advertising the capability does NOT mutate the file: the head-canonical
    /// ledger bump is committed by a delta write that actually creates head
    /// state, inside that write's own transaction (see [`Self::open`] and
    /// `LocalContinuityStore::delta_write`).
    fn as_incremental_sessions(&self) -> Option<Arc<dyn ContinuityIncrementalSessions>> {
        Some(Arc::new(self.clone()))
    }

    async fn delete_continuity_record(
        &self,
        identity: &AgentIdentity,
        fencing_token: FencingToken,
    ) -> Result<(), ContinuityStoreError> {
        let identity = identity.clone();
        self.run_blocking("delete_continuity_record", move |inner| {
            inner.with_writer(|connection| {
                let head_tables = inner.head_tables_available(connection)?;
                // Keep the fence check and both deletes in one transaction so
                // failures cannot leave a half-deleted identity.
                let tx = connection
                    .unchecked_transaction()
                    .map_err(|e| sqlite_err("begin tx", e))?;

                let mut stmt = tx
                    .prepare_cached(
                        "SELECT fencing_token FROM continuity_records WHERE identity = ?1",
                    )
                    .map_err(|e| sqlite_err("prepare", e))?;
                let existing_token = stmt
                    .query_row(rusqlite::params![identity.as_str()], |row| {
                        row.get::<_, u64>(0)
                    })
                    .optional()
                    .map_err(|e| sqlite_err("query", e))?;
                drop(stmt);

                if let Some(current) = existing_token
                    && fencing_token.get() < current
                {
                    return Err(ContinuityStoreError::StaleFencingToken {
                        identity: identity.clone(),
                        presented: fencing_token,
                        current: FencingToken::new(current),
                    });
                }

                tx.execute(
                    "DELETE FROM session_snapshots WHERE identity = ?1",
                    rusqlite::params![identity.as_str()],
                )
                .map_err(|e| sqlite_err("delete snapshots", e))?;
                if head_tables {
                    delete_head_canonical_rows_in_txn(
                        &tx,
                        "identity = ?1",
                        rusqlite::params![identity.as_str()],
                    )?;
                }
                tx.execute(
                    "DELETE FROM continuity_records WHERE identity = ?1",
                    rusqlite::params![identity.as_str()],
                )
                .map_err(|e| sqlite_err("delete record", e))?;
                tx.commit().map_err(|e| sqlite_err("commit tx", e))?;
                Ok(())
            })
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// The session-delta channel (M4b)
// ---------------------------------------------------------------------------

impl LocalContinuityStore {
    /// Read-only enumeration of every durable identity → session binding.
    ///
    /// Operator-maintenance surface: an offline reader walks the fleet's
    /// bindings without knowing identities up front; each binding then goes
    /// through the ordinary per-session
    /// [`ContinuityStore::resolve_record_by_session`] path, so this adds no
    /// new write or trust surface.
    pub async fn list_session_bindings(
        &self,
    ) -> Result<Vec<(AgentIdentity, meerkat_core::types::SessionId)>, ContinuityStoreError> {
        self.run_blocking("list_session_bindings", move |inner| {
            inner.with_reader(|connection| {
                let mut stmt = connection
                    .prepare_cached(
                        "SELECT identity, session_id FROM continuity_records ORDER BY identity",
                    )
                    .map_err(|e| sqlite_err("prepare", e))?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|e| sqlite_err("query", e))?;
                let mut bindings = Vec::new();
                for row in rows {
                    let (identity, session_id) = row.map_err(|e| sqlite_err("row", e))?;
                    bindings.push((
                        AgentIdentity::parse(&identity).map_err(|e| {
                            ContinuityStoreError::Corruption(format!(
                                "invalid identity in store: {e}"
                            ))
                        })?,
                        meerkat_core::types::SessionId::parse(&session_id).map_err(|e| {
                            ContinuityStoreError::Corruption(format!(
                                "invalid session id in store: {e}"
                            ))
                        })?,
                    ));
                }
                Ok(bindings)
            })
        })
        .await
    }

    /// Run one delta mutation inside ONE writer transaction that enforces
    /// and advances the continuity cursor — and, on a file that does not
    /// carry the head-canonical channel yet, converges the schema and arms
    /// the one-way ledger bump in that same transaction.
    ///
    /// Crash consistency: the head-canonical DDL, the rows, the head row,
    /// the `continuity_records` advance and the ledger v2 stamp commit
    /// together or not at all. So the durable cursor can never point past
    /// durable data, a partial append can never leave a torn document, and
    /// — the property this ordering exists for — **a write that does not
    /// leave a head row behind never arms the v1-writer lockout**.
    ///
    /// Two ways a write fails to earn the bump, and both leave the file
    /// rollback-safe at v1:
    ///
    /// - it is REFUSED (a guard rejection or a typed operation refusal):
    ///   the DDL and the stamp roll back with everything else, and the file
    ///   keeps zero head rows;
    /// - it is ACCEPTED but creates no head state. `append_messages` on a
    ///   session with neither a head row nor a blob to migrate is the real
    ///   case: the service appends and then adopts under two separate
    ///   locks, so the append commits alone. Strand rows that no head
    ///   adopts are not part of any document — every read path gates on the
    ///   head row — so an older binary still correctly serves that session
    ///   from its blob, and locking it out of the whole file would be a
    ///   brick bought for nothing. The rows and the (additive, `IF NOT
    ///   EXISTS`) DDL commit; the stamp waits for the adopting head write.
    ///
    /// The converse — a head row with no stamp — remains impossible, so an
    /// older binary can never mistake a frozen blob archive for authority.
    /// That is why the "is the lockout owed" question is answered from the
    /// ledger row inside this transaction
    /// ([`head_canonical_ledger_stamped_in_txn`]) and never from the
    /// table-existence latch, which this method can now leave `true` on a
    /// v1 file.
    async fn delta_write<T, F>(
        &self,
        operation_name: &'static str,
        cursor: &ContinuityWriteCursor,
        session_id: meerkat_core::types::SessionId,
        operation: F,
    ) -> Result<T, SessionStoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>, &ContinuityWriteCursor) -> Result<T, SessionStoreError>
            + Send
            + 'static,
    {
        let cursor = cursor.clone();
        // The typed session-store error the guards produce rides OUT as a
        // value, never laundered into `Internal`: the accept/reject boundary
        // this store mirrors must be indistinguishable from the meerkat
        // service's, conflict codes included. A carried `Err` skips the
        // commit, so the transaction rolls back on drop.
        let outcome: Result<Result<T, SessionStoreError>, ContinuityStoreError> = self
            .run_blocking(operation_name, move |inner| {
                inner.with_writer(|connection| {
                    let tx = connection
                        .unchecked_transaction()
                        .map_err(|e| sqlite_err("begin tx", e))?;
                    // Is the one-way lockout still owed? Answered from the
                    // ledger row itself, inside this transaction — the
                    // cached flag may only SKIP the read, never decide that
                    // no bump is owed.
                    let lockout_owed = !inner.ledger_is_head_canonical()
                        && !head_canonical_ledger_stamped_in_txn(&tx)?;
                    // Converge the head-canonical schema INSIDE this
                    // transaction (SQLite DDL is transactional): the tables
                    // have to exist before the operation can write a row,
                    // but they vanish again with a rollback. The ledger
                    // stamp that makes the bump one-way is deliberately NOT
                    // written here — it is the last thing before commit,
                    // and only once the write has actually left a head row
                    // behind.
                    if lockout_owed {
                        converge_head_canonical_schema_in_txn(&tx)?;
                    }
                    let record_was_present = enforce_continuity_cursor_in_txn(
                        &tx,
                        &cursor.identity,
                        &session_id,
                        cursor.generation,
                        cursor.checkpoint_version,
                        cursor.fencing_token,
                    )?;
                    // Both owner guards, in the same order and with the same
                    // meaning the whole-document verb applies. A delta
                    // mutation must not be accepted where a whole-document
                    // save onto the same durable session would be refused.
                    ensure_snapshot_owner_in_txn(
                        &tx,
                        &session_id,
                        &cursor.identity,
                        cursor.generation,
                    )?;
                    ensure_head_owner_in_txn(
                        &tx,
                        &session_id,
                        &cursor.identity,
                        cursor.generation,
                    )?;
                    let value = match operation(&tx, &cursor) {
                        Ok(value) => value,
                        Err(typed) => return Ok(Err(typed)),
                    };
                    advance_continuity_record_in_txn(
                        &tx,
                        &cursor.identity,
                        &session_id,
                        cursor.generation,
                        cursor.checkpoint_version,
                        cursor.fencing_token,
                        record_was_present,
                    )?;
                    // Earned only by head state that will be durable when
                    // this transaction commits. An accepted append that
                    // adopts nothing leaves the file at v1.
                    let stamp_lockout =
                        lockout_owed && session_head_exists_in_txn(&tx, &session_id)?;
                    if stamp_lockout {
                        stamp_head_canonical_ledger_in_txn(&tx)?;
                    }
                    tx.commit().map_err(|e| sqlite_err("commit tx", e))?;
                    // Latch only after the commit that made each fact true,
                    // and keep the two facts apart: the DDL committed
                    // whenever the bump was owed, so the tables are
                    // queryable either way, but the lockout latch tracks
                    // the ledger row alone.
                    if lockout_owed {
                        inner.head_canonical_schema.store(true, Ordering::Release);
                    }
                    if stamp_lockout || !lockout_owed {
                        inner.head_canonical_ledger.store(true, Ordering::Release);
                    }
                    Ok(Ok(value))
                })
            })
            .await;
        match outcome {
            Ok(typed) => typed,
            Err(error) => Err(session_err(operation_name, error)),
        }
    }

    /// Read one delta view. Head-canonical sessions read their rows; a
    /// blob-only session is served from the deterministic read-only strand
    /// layout of its archived document (never a write), so the CAS token a
    /// caller derives before migration matches the one the first migrating
    /// write persists.
    async fn delta_read<T, F>(
        &self,
        operation_name: &'static str,
        operation: F,
    ) -> Result<T, SessionStoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>, bool) -> Result<T, SessionStoreError> + Send + 'static,
    {
        let outcome: Result<Result<T, SessionStoreError>, ContinuityStoreError> = self
            .run_blocking(operation_name, move |inner| {
                inner.with_reader(|connection| {
                    let head_tables = inner.head_tables_available(connection)?;
                    let tx = connection
                        .unchecked_transaction()
                        .map_err(|e| sqlite_err("begin read tx", e))?;
                    Ok(operation(&tx, head_tables))
                })
            })
            .await;
        match outcome {
            Ok(typed) => typed,
            Err(error) => Err(session_err(operation_name, error)),
        }
    }
}

#[async_trait]
impl ContinuityIncrementalSessions for LocalContinuityStore {
    async fn append_messages(
        &self,
        cursor: &ContinuityWriteCursor,
        id: &meerkat_core::types::SessionId,
        strand: &TranscriptStrandId,
        base_seq: u64,
        messages: &[Message],
    ) -> Result<(), SessionStoreError> {
        let session_id = id.clone();
        let strand = strand.clone();
        let messages = messages.to_vec();
        let migrate_id = session_id.clone();
        self.delta_write(
            "continuity append_messages",
            cursor,
            session_id,
            move |tx, cursor| {
                // First delta write on a blob-only session migrates it inside
                // this transaction; the blob stays as a frozen archive.
                ensure_head_canonical_for_write_in_txn(
                    tx,
                    &migrate_id,
                    &cursor.identity,
                    cursor.generation,
                    cursor.checkpoint_version,
                    cursor.fencing_token,
                )?;
                insert_strand_rows_in_txn(
                    tx,
                    &migrate_id,
                    &strand,
                    base_seq,
                    &messages,
                    &cursor.identity,
                    cursor.generation,
                )
            },
        )
        .await
    }

    async fn commit_rewrite(
        &self,
        cursor: &ContinuityWriteCursor,
        id: &meerkat_core::types::SessionId,
        record: &TranscriptRewriteRecord,
        expected: SessionHeadCas,
    ) -> Result<SessionHead, SessionStoreError> {
        let session_id = id.clone();
        let record = record.clone();
        let target = session_id.clone();
        self.delta_write(
            "continuity commit_rewrite",
            cursor,
            session_id,
            move |tx, cursor| {
                let stored = ensure_head_canonical_for_write_in_txn(
                    tx,
                    &target,
                    &cursor.identity,
                    cursor.generation,
                    cursor.checkpoint_version,
                    cursor.fencing_token,
                )?
                .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                    id: target.clone(),
                    reason: "rewrite target has no persisted session head".to_string(),
                })?;
                let (stored_head, stored_token) = &stored;
                // CAS races and stale parents surface as
                // TranscriptRevisionConflict BEFORE the parent strand read,
                // which would otherwise fail on an unrelated shape.
                match &expected {
                    SessionHeadCas::Create => {
                        return Err(SessionStoreError::TranscriptRevisionConflict {
                            id: target.clone(),
                            expected: "<create>".to_string(),
                            actual: stored_token.clone(),
                        });
                    }
                    SessionHeadCas::IfToken(expected_token) => {
                        if expected_token != stored_token {
                            return Err(SessionStoreError::TranscriptRevisionConflict {
                                id: target.clone(),
                                expected: expected_token.clone(),
                                actual: stored_token.clone(),
                            });
                        }
                    }
                }
                if record.commit.parent_revision != stored_head.head_revision {
                    return Err(SessionStoreError::TranscriptRevisionConflict {
                        id: target,
                        expected: record.commit.parent_revision,
                        actual: stored_head.head_revision.clone(),
                    });
                }
                let before = record.commit.messages_before as u64;
                if before > strand_row_count_in_txn(tx, &target, &stored_head.strand)? {
                    return Err(SessionStoreError::InvalidTranscriptRewrite {
                        id: target,
                        reason: format!(
                            "commit messages_before {before} exceeds persisted rows of strand {}",
                            stored_head.strand
                        ),
                    });
                }
                let parent_rows =
                    strand_messages_in_txn(tx, &target, &stored_head.strand, 0..before)?;
                let parent_digest = meerkat_core::transcript_messages_digest(&parent_rows)
                    .map_err(SessionStoreError::from)?;
                let next = validate_commit_rewrite_transition(
                    &target,
                    &record,
                    stored_head,
                    stored_token,
                    &expected,
                    &parent_digest,
                )?;
                insert_rewrite_row_in_txn(
                    tx,
                    &target,
                    stored_head.rewrite_count,
                    &RewriteRow {
                        commit: record.commit.clone(),
                        parent_strand: stored_head.strand.clone(),
                        parent_len: before,
                        strand: next.strand.clone(),
                        strand_len: record.commit.messages_after as u64,
                    },
                    &cursor.identity,
                    cursor.generation,
                )?;
                insert_strand_rows_in_txn(
                    tx,
                    &target,
                    &next.strand,
                    0,
                    &record.revision_body.messages,
                    &cursor.identity,
                    cursor.generation,
                )?;
                Ok(next)
            },
        )
        .await
    }

    async fn save_head(
        &self,
        cursor: &ContinuityWriteCursor,
        head: &SessionHead,
        expected: SessionHeadCas,
    ) -> Result<(), SessionStoreError> {
        let session_id = head.id.clone();
        let head = head.clone();
        self.delta_write(
            "continuity save_head",
            cursor,
            session_id,
            move |tx, cursor| {
                let stored = ensure_head_canonical_for_write_in_txn(
                    tx,
                    &head.id,
                    &cursor.identity,
                    cursor.generation,
                    cursor.checkpoint_version,
                    cursor.fencing_token,
                )?;
                let strand_len = strand_row_count_in_txn(tx, &head.id, &head.strand)?;
                let recorded = rewrite_row_count_in_txn(tx, &head.id)?;
                validate_save_head_transition(
                    &head,
                    stored.as_ref().map(|(h, t)| (h, t.as_str())),
                    &expected,
                    strand_len,
                    recorded,
                )?;
                write_head_row_in_txn(
                    tx,
                    &head,
                    &cursor.identity,
                    cursor.generation,
                    cursor.checkpoint_version,
                    cursor.fencing_token,
                )?;
                Ok(())
            },
        )
        .await
    }

    async fn adopt_released_head_document(
        &self,
        cursor: &ContinuityWriteCursor,
        session: &meerkat_core::Session,
    ) -> Result<(), SessionStoreError> {
        let session = session.clone();
        self.delta_write(
            "continuity adopt_released_head_document",
            cursor,
            session.id().clone(),
            move |tx, cursor| {
                adopt_released_head_in_txn(
                    tx,
                    &session,
                    &cursor.identity,
                    cursor.generation,
                    cursor.checkpoint_version,
                    cursor.fencing_token,
                )
            },
        )
        .await
    }

    async fn session_head_matches_current(
        &self,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        generation: ContinuityGeneration,
        fencing_token: FencingToken,
        head: &SessionHead,
    ) -> Result<bool, SessionStoreError> {
        let identity = identity.clone();
        let session_id = session_id.clone();
        let head = head.clone();
        self.delta_read(
            "continuity session_head_matches_current",
            move |tx, head_tables| {
                if !head_tables {
                    return Ok(false);
                }
                let Some((stored, _token)) = head_row_in_txn(tx, &session_id)? else {
                    return Ok(false);
                };
                if stored != head {
                    return Ok(false);
                }
                // Fence currency, same shape `enforce_continuity_cursor_in_txn`
                // validates on the mutating verbs: the identity's CURRENT
                // record must bind this session and generation, and its fence
                // must EQUAL the presented one. An advanced durable fence
                // makes this probe false so the caller's fencing write verb
                // surfaces the ordinary stale-fence refusal.
                let record = tx
                    .query_row(
                        "SELECT session_id, generation, fencing_token
                         FROM continuity_records WHERE identity = ?1",
                        rusqlite::params![identity.as_str()],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, u64>(1)?,
                                row.get::<_, u64>(2)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|e| sqlite_session_err("query continuity record", e))?;
                let Some((record_session, record_generation, record_token)) = record else {
                    return Ok(false);
                };
                Ok(record_session == session_id.to_string()
                    && record_generation == generation.get()
                    && record_token == fencing_token.get())
            },
        )
        .await
    }

    async fn load_head(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<SessionHead>, SessionStoreError> {
        let id = id.clone();
        self.delta_read("continuity load_head", move |tx, head_tables| {
            if head_tables && let Some((head, _token)) = head_row_in_txn(tx, &id)? {
                return Ok(Some(head));
            }
            let Some(session) = blob_session_in_txn(tx, &id)? else {
                return Ok(None);
            };
            let (_layout, head) = retained_layout_for_blob_session(&session)?;
            Ok(Some(head))
        })
        .await
    }

    async fn load_canonical_head(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<SessionHead>, SessionStoreError> {
        let id = id.clone();
        self.delta_read("continuity load_canonical_head", move |tx, head_tables| {
            if !head_tables {
                return Ok(None);
            }
            Ok(head_row_in_txn(tx, &id)?.map(|(head, _token)| head))
        })
        .await
    }

    async fn load_canonical_session(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<Session>, SessionStoreError> {
        let id = id.clone();
        self.delta_read(
            "continuity load_canonical_session",
            move |tx, head_tables| {
                if !head_tables {
                    return Ok(None);
                }
                // ONE snapshot over head + rows. `materialize_slim_in_txn`
                // re-derives the transcript digest against `head_revision`, so a
                // torn pair would surface as `Corrupted` rather than silently —
                // but under a single transaction the pair cannot tear at all.
                let Some((head, _token)) = head_row_in_txn(tx, &id)? else {
                    return Ok(None);
                };
                materialize_slim_in_txn(tx, &id, &head).map(Some)
            },
        )
        .await
    }

    async fn load_canonical_previous(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<(Session, Vec<meerkat_core::TranscriptRewriteCommit>)>, SessionStoreError>
    {
        let id = id.clone();
        self.delta_read(
            "continuity load_canonical_previous",
            move |tx, head_tables| {
                if !head_tables {
                    return Ok(None);
                }
                let Some((head, _token)) = head_row_in_txn(tx, &id)? else {
                    return Ok(None);
                };
                let rewrite_count = head.rewrite_count;
                let session = materialize_slim_in_txn(tx, &id, &head)?;
                // The commits alone — deliberately not `load_rewrites`,
                // which reconstructs both message bodies of every rewrite
                // and would make a guard read O(rewrites x transcript).
                let adopted = rewrite_rows_in_txn(tx, &id, rewrite_count)?
                    .into_iter()
                    .map(|row| row.commit)
                    .collect();
                Ok(Some((session, adopted)))
            },
        )
        .await
    }

    async fn load_messages(
        &self,
        id: &meerkat_core::types::SessionId,
        strand: &TranscriptStrandId,
        range: std::ops::Range<u64>,
    ) -> Result<Vec<Message>, SessionStoreError> {
        let id = id.clone();
        let strand = strand.clone();
        self.delta_read("continuity load_messages", move |tx, head_tables| {
            if head_tables && head_row_in_txn(tx, &id)?.is_some() {
                return strand_messages_in_txn(tx, &id, &strand, range);
            }
            let Some(session) = blob_session_in_txn(tx, &id)? else {
                return Err(SessionStoreError::NotFound(id));
            };
            let (layout, _head) = retained_layout_for_blob_session(&session)?;
            let rows = layout
                .strands
                .iter()
                .find(|(sid, _)| *sid == strand)
                .map(|(_, rows)| rows.as_slice())
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            let start = usize::try_from(range.start)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            let end =
                usize::try_from(range.end).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            if start > end || end > rows.len() {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            Ok(rows[start..end].to_vec())
        })
        .await
    }

    async fn load_rewrites(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Vec<TranscriptRewriteRecord>, SessionStoreError> {
        let id = id.clone();
        self.delta_read("continuity load_rewrites", move |tx, head_tables| {
            if head_tables && let Some((head, _token)) = head_row_in_txn(tx, &id)? {
                return rewrite_records_in_txn(tx, &id, head.rewrite_count);
            }
            let Some(session) = blob_session_in_txn(tx, &id)? else {
                return Ok(Vec::new());
            };
            let (layout, _head) = retained_layout_for_blob_session(&session)?;
            layout
                .rewrites
                .iter()
                .map(|rewrite| {
                    let parent_len = usize::try_from(rewrite.parent_len)
                        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
                    let strand_len = usize::try_from(rewrite.strand_len)
                        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
                    let parent_messages = layout
                        .strands
                        .iter()
                        .find(|(sid, _)| *sid == rewrite.parent_strand)
                        .map(|(_, rows)| rows[..parent_len].to_vec())
                        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
                    let revision_messages = layout
                        .strands
                        .iter()
                        .find(|(sid, _)| *sid == rewrite.strand)
                        .map(|(_, rows)| rows[..strand_len].to_vec())
                        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
                    reconstruct_rewrite_record(
                        &id,
                        rewrite.commit.clone(),
                        parent_messages,
                        revision_messages,
                    )
                })
                .collect()
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn extension_history_schema_is_self_contained_and_refuses_old_writers() {
        let mut connection = Connection::open_in_memory().unwrap();
        meerkat_sqlite::apply_domain_migrations(&mut connection, &META_CONTINUITY_DOMAIN).unwrap();
        assert_eq!(
            meerkat_sqlite::domain_version(&connection, META_CONTINUITY_DOMAIN.name).unwrap(),
            Some(IDENTITY_HISTORY_SCHEMA_VERSION)
        );
        assert!(matches!(
            meerkat_sqlite::preflight_schema_eligibility(&connection, &META_CONTINUITY_HEAD_DOMAIN),
            Err(meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture {
                found: 3,
                supported: 2,
                ..
            })
        ));
        let tx = connection.transaction().unwrap();
        stamp_head_canonical_ledger_in_txn(&tx).unwrap();
        tx.commit().unwrap();
        assert!(
            !apply_head_canonical_schema(&mut connection)
                .unwrap()
                .migrated()
        );
        assert_eq!(
            meerkat_sqlite::domain_version(&connection, META_CONTINUITY_DOMAIN.name).unwrap(),
            Some(IDENTITY_HISTORY_SCHEMA_VERSION)
        );
    }

    #[tokio::test]
    async fn extension_history_opt_in_preserves_released_schema_boundaries() {
        // The middle case is v1 with legitimately deferred v2 DDL.
        for (version, deferred_heads) in [(1, false), (1, true), (2, false)] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("continuity.sqlite3");
            let store = LocalContinuityStore::open(&path).unwrap();
            let identity = AgentIdentity::parse("existing-owner").unwrap();
            let session = meerkat_core::SessionId::new();
            store
                .upsert_continuity_record(&record(&identity, &session), FencingToken::new(1))
                .await
                .unwrap();
            let mut connection = Connection::open(&path).unwrap();
            if version == 2 {
                apply_head_canonical_schema(&mut connection).unwrap();
            } else if deferred_heads {
                let tx = connection.transaction().unwrap();
                converge_head_canonical_schema_in_txn(&tx).unwrap();
                tx.commit().unwrap();
            }
            drop(store);
            let _reopened = LocalContinuityStore::open(&path).unwrap();
            assert_eq!(continuity_domain_version(&path), Some(version));
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_schema WHERE name LIKE 'continuity_identity_%'",
                        [],
                        |row| row.get::<_, usize>(0),
                    )
                    .unwrap(),
                0,
                "opening or head-only migration must not opt into history"
            );
            meerkat_sqlite::apply_domain_migrations(&mut connection, &META_CONTINUITY_DOMAIN)
                .unwrap();
            assert_eq!(continuity_domain_version(&path), Some(3));
            assert_eq!(
                connection
                    .query_row(
                        "SELECT identity FROM continuity_identity_history WHERE session_id=?1",
                        [session.to_string()],
                        |row| row.get::<_, String>(0),
                    )
                    .unwrap(),
                identity.as_str()
            );
        }
    }

    #[tokio::test]
    async fn extension_history_conflicting_backfill_rolls_back_ddl_and_stamp() {
        for version in [1, 2] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("continuity.sqlite3");
            let store = LocalContinuityStore::open(&path).unwrap();
            let session = meerkat_core::SessionId::new();
            for name in ["first-owner", "conflicting-owner"] {
                let identity = AgentIdentity::parse(name).unwrap();
                store
                    .upsert_continuity_record(&record(&identity, &session), FencingToken::new(1))
                    .await
                    .unwrap();
            }
            let mut connection = Connection::open(&path).unwrap();
            if version == 2 {
                apply_head_canonical_schema(&mut connection).unwrap();
            }
            assert!(
                meerkat_sqlite::apply_domain_migrations(&mut connection, &META_CONTINUITY_DOMAIN)
                    .is_err()
            );
            assert_eq!(continuity_domain_version(&path), Some(version));
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_schema WHERE name LIKE 'continuity_identity_%'",
                        [],
                        |row| row.get::<_, usize>(0),
                    )
                    .unwrap(),
                0
            );
            if version == 1 {
                verify_released_v1_continuity_schema(&connection).unwrap();
            } else {
                verify_released_v2_continuity_schema(&connection).unwrap();
            }
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM continuity_records", [], |row| {
                        row.get::<_, usize>(0)
                    })
                    .unwrap(),
                2
            );
        }
    }

    // Unconditional test: feature-off binaries also understand the opted-in
    // schema and preserve history without constructing extension factories.
    #[tokio::test]
    async fn extension_history_retains_disabled_writes_and_rollback_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("continuity.sqlite3");
        let store = LocalContinuityStore::open(&path).unwrap();
        let identity = AgentIdentity::parse("retained-owner").unwrap();
        let original = meerkat_core::SessionId::new();
        let attempted = meerkat_core::SessionId::new();
        let snapshot_only = meerkat_core::SessionId::new();
        let previous = record(&identity, &original);
        store
            .upsert_continuity_record(&previous, FencingToken::new(1))
            .await
            .unwrap();
        {
            let mut connection = Connection::open(&path).unwrap();
            meerkat_sqlite::apply_domain_migrations(&mut connection, &META_CONTINUITY_DOMAIN)
                .unwrap();
        }
        // This writer was opened before another connection opted in.
        let mut provisional = record(&identity, &attempted);
        provisional.generation = ContinuityGeneration::new(1);
        store
            .upsert_continuity_record(&provisional, FencingToken::new(2))
            .await
            .unwrap();
        store
            .rollback_continuity_record(&provisional, Some(&previous), FencingToken::new(2))
            .await
            .unwrap();
        store
            .delete_continuity_record(&identity, FencingToken::new(2))
            .await
            .unwrap();
        drop(store);
        let reopened = LocalContinuityStore::open(&path).unwrap();
        reopened
            .save_session_snapshot(
                &identity,
                &snapshot_only,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(1),
                FencingToken::new(3),
                &SessionSnapshot { data: vec![1] },
            )
            .await
            .unwrap();
        let head_session = Session::new();
        let head = SessionHead::from_session(&head_session, TranscriptStrandId::root(), 0).unwrap();
        reopened
            .as_incremental_sessions()
            .unwrap()
            .save_head(&cursor(&identity, 0, 1, 3), &head, SessionHeadCas::Create)
            .await
            .unwrap();
        reopened
            .delete_continuity_record(&identity, FencingToken::new(3))
            .await
            .unwrap();
        let connection = Connection::open(&path).unwrap();
        for session in [&original, &attempted, &snapshot_only, head_session.id()] {
            assert_eq!(
                connection
                    .query_row(
                        "SELECT identity FROM continuity_identity_history WHERE session_id=?1",
                        [session.to_string()],
                        |row| row.get::<_, String>(0),
                    )
                    .unwrap(),
                identity.as_str()
            );
        }
        assert_eq!(continuity_domain_version(&path), Some(3));
    }

    #[test]
    fn extension_history_reopen_refuses_missing_retention_trigger() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("continuity.sqlite3");
        let mut connection = Connection::open(&path).unwrap();
        meerkat_sqlite::apply_domain_migrations(&mut connection, &META_CONTINUITY_DOMAIN).unwrap();
        connection
            .execute_batch("DROP TRIGGER continuity_identity_history_insert")
            .unwrap();
        assert!(LocalContinuityStore::open(&path).is_err());
    }

    #[test]
    fn extension_history_trigger_retries_and_conflicts_under_outer_upsert() {
        // Exercise both INSERT and UPDATE triggers on each producer. In
        // particular, an outer DO UPDATE overrides an inner OR IGNORE, so
        // duplicate history must be skipped without invoking a constraint.
        for (table, upsert) in [
            (
                "continuity_records",
                "INSERT INTO continuity_records(session_id,identity,agent_runtime_id,generation,checkpoint_version,fencing_token)
                 VALUES(?1,?2,'runtime',0,1,1)
                 ON CONFLICT(identity) DO UPDATE SET session_id=excluded.session_id,identity=excluded.identity,checkpoint_version=excluded.checkpoint_version",
            ),
            (
                "session_snapshots",
                "INSERT INTO session_snapshots(session_id,identity,generation,checkpoint_version,fencing_token,data)
                 VALUES(?1,?2,0,1,1,X'00')
                 ON CONFLICT(session_id) DO UPDATE SET session_id=excluded.session_id,identity=excluded.identity,checkpoint_version=excluded.checkpoint_version",
            ),
            (
                "continuity_session_heads",
                "INSERT INTO continuity_session_heads(session_id,identity,generation,checkpoint_version,fencing_token,head_revision,message_count,rewrite_count,head_json,cas_token)
                 VALUES(?1,?2,0,1,1,'revision',0,0,X'00','token')
                 ON CONFLICT(session_id) DO UPDATE SET session_id=excluded.session_id,identity=excluded.identity,checkpoint_version=excluded.checkpoint_version",
            ),
        ] {
            let mut connection = Connection::open_in_memory().unwrap();
            meerkat_sqlite::apply_domain_migrations(&mut connection, &META_CONTINUITY_DOMAIN)
                .unwrap();
            connection
                .execute(
                    "INSERT INTO continuity_identity_history(session_id,identity) VALUES('first','owner'),('reserved','reserved-owner')",
                    [],
                )
                .unwrap();
            // Identical retained binding, first through AFTER INSERT and
            // then through AFTER UPDATE, must be idempotent.
            for _ in 0..2 {
                connection.execute(upsert, ["first", "owner"]).unwrap();
            }
            let read_producer = || {
                connection
                    .prepare(&format!(
                        "SELECT session_id,identity,checkpoint_version FROM {table} ORDER BY session_id"
                    ))
                    .unwrap()
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, u64>(2)?,
                        ))
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            };
            let expected = vec![("first".to_owned(), "owner".to_owned(), 1)];
            assert_eq!(read_producer(), expected);
            let conflicting_update = if table == "continuity_records" {
                ["reserved", "owner"]
            } else {
                ["first", "update-impostor"]
            };
            for binding in [["reserved", "insert-impostor"], conflicting_update] {
                let error = connection.execute(upsert, binding).unwrap_err();
                assert_eq!(
                    error.sqlite_error().unwrap().extended_code,
                    rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER,
                    "{table}: {error}"
                );
                assert_eq!(read_producer(), expected, "{table} rolled back");
            }
            assert_eq!(
                connection
                    .prepare("SELECT session_id,identity FROM continuity_identity_history ORDER BY session_id")
                    .unwrap()
                    .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap(),
                vec![("first".into(), "owner".into()), ("reserved".into(), "reserved-owner".into())]
            );
        }
    }

    #[tokio::test]
    async fn extension_history_record_retry_and_session_revisit_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("continuity.sqlite3");
        let store = LocalContinuityStore::open(&path).unwrap();
        let identity = AgentIdentity::parse("retry-owner").unwrap();
        let original = meerkat_core::SessionId::new();
        let successor = meerkat_core::SessionId::new();
        let token = FencingToken::new(1);
        {
            let mut connection = Connection::open(&path).unwrap();
            meerkat_sqlite::apply_domain_migrations(&mut connection, &META_CONTINUITY_DOMAIN)
                .unwrap();
        }
        for session in [&original, &original, &successor, &successor, &original] {
            store
                .upsert_continuity_record(&record(&identity, session), token)
                .await
                .unwrap();
        }
        drop(store);
        let reopened = LocalContinuityStore::open(&path).unwrap();
        reopened
            .upsert_continuity_record(&record(&identity, &original), token)
            .await
            .unwrap();
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM continuity_identity_history",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
            2
        );
    }

    #[cfg(feature = "extension-state")]
    #[tokio::test]
    async fn extension_history_survives_rotation_deletion_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("continuity.sqlite3");
        let store = LocalContinuityStore::open(&path).unwrap();
        let identity = AgentIdentity::parse("writer").unwrap();
        let original = meerkat_core::SessionId::new();
        let successor = meerkat_core::SessionId::new();
        let token = FencingToken::new(1);
        store
            .upsert_continuity_record(&record(&identity, &original), token)
            .await
            .unwrap();
        store.enable_identity_binding_history().await.unwrap();
        assert_eq!(
            store.historical_identity_binding(&original).await.unwrap(),
            Some(identity.clone())
        );
        store
            .establish_identity_history_coverage("mob", 10)
            .await
            .unwrap();
        assert!(
            !store
                .identity_history_covers_birth("mob", 10)
                .await
                .unwrap()
        );
        assert!(
            store
                .identity_history_covers_birth("mob", 11)
                .await
                .unwrap()
        );
        assert!(
            !store
                .identity_history_covers_birth("unknown-mob", 11)
                .await
                .unwrap()
        );
        store
            .upsert_continuity_record(&record(&identity, &successor), token)
            .await
            .unwrap();
        store
            .delete_continuity_record(&identity, token)
            .await
            .unwrap();
        drop(store);
        let reopened = LocalContinuityStore::open(&path).unwrap();
        reopened.enable_identity_binding_history().await.unwrap();
        assert!(reopened.has_historical_identity(&identity).await.unwrap());
        assert!(
            !reopened
                .has_historical_identity(&AgentIdentity::parse("never-declared").unwrap())
                .await
                .unwrap()
        );
        reopened
            .establish_identity_history_coverage("mob", 99)
            .await
            .unwrap();
        for session in [original, successor] {
            assert_eq!(
                reopened
                    .historical_identity_binding(&session)
                    .await
                    .unwrap(),
                Some(identity.clone())
            );
        }
        assert!(
            reopened
                .identity_history_covers_birth("mob", 11)
                .await
                .unwrap()
        );
    }

    #[cfg(feature = "extension-state")]
    #[tokio::test]
    async fn extension_history_refuses_exact_session_identity_borrowing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("continuity.sqlite3");
        let store = LocalContinuityStore::open(&path).unwrap();
        store.enable_identity_binding_history().await.unwrap();
        let original = AgentIdentity::parse("original").unwrap();
        let impostor = AgentIdentity::parse("impostor").unwrap();
        let session = meerkat_core::SessionId::new();
        let token = FencingToken::new(1);
        store
            .upsert_continuity_record(&record(&original, &session), token)
            .await
            .unwrap();
        store
            .delete_continuity_record(&original, token)
            .await
            .unwrap();
        assert!(
            store
                .upsert_continuity_record(&record(&impostor, &session), token)
                .await
                .is_err()
        );
        assert_eq!(
            store.historical_identity_binding(&session).await.unwrap(),
            Some(original)
        );
        assert!(matches!(
            store
                .resolve_many(std::slice::from_ref(&impostor))
                .await
                .unwrap()
                .get(&impostor),
            Some(ContinuityResolveState::Uninitialized)
        ));
    }

    fn record(
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
    ) -> ContinuityRecord {
        ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt-001").unwrap(),
            session_id: session_id.clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        }
    }

    #[tokio::test]
    async fn fresh_store_stamps_meta_continuity_domain() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let store = LocalContinuityStore::open(&path).expect("open");
        // The store must stay usable while we inspect the ledger.
        assert_eq!(store.max_fencing_token().expect("floor"), 0);
        let probe = Connection::open(&path).expect("probe");
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-continuity").expect("ledger"),
            Some(1)
        );
    }

    /// A pre-ledger file (historical two-table DDL, no meerkat_schema table)
    /// is refused typed at open with its rows left untouched and no ledger
    /// stamped: pre-ledger corpora are below the mobkit 0.8.8 floor, and the
    /// 0.8.11 reset retired silent pre-floor convergence (this test pinned
    /// that convergence until then).
    #[tokio::test]
    async fn legacy_file_is_refused_with_rows_preserved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let session_id = meerkat_core::types::SessionId::new();
        {
            let conn = Connection::open(&path).expect("legacy create");
            conn.execute_batch(SCHEMA).expect("legacy ddl");
            conn.execute(
                "INSERT INTO continuity_records (identity, agent_runtime_id, session_id, \
                 generation, checkpoint_version, fencing_token) VALUES (?1, 'rt-001', ?2, 3, 5, 7)",
                rusqlite::params![identity.as_str(), session_id.to_string()],
            )
            .expect("legacy record");
            conn.execute(
                "INSERT INTO session_snapshots (session_id, identity, generation, \
                 checkpoint_version, fencing_token, data) VALUES (?1, ?2, 3, 5, 9, X'010203')",
                rusqlite::params![session_id.to_string(), identity.as_str()],
            )
            .expect("legacy snapshot");
        }

        assert!(
            LocalContinuityStore::open(&path).is_err(),
            "opening a pre-ledger continuity database must refuse typed: unledgered owned \
             tables are below the mobkit 0.8.8 floor and must never be silently converged"
        );
        let probe = Connection::open(&path).expect("probe");
        let (generation, checkpoint): (i64, i64) = probe
            .query_row(
                "SELECT generation, checkpoint_version FROM continuity_records \
                 WHERE identity = ?1",
                rusqlite::params![identity.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("legacy record preserved");
        assert_eq!((generation, checkpoint), (3, 5));
        let snapshots: i64 = probe
            .query_row("SELECT COUNT(*) FROM session_snapshots", [], |row| {
                row.get(0)
            })
            .expect("legacy snapshots preserved");
        assert_eq!(snapshots, 1, "the refusal must leave legacy rows untouched");
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-continuity").expect("ledger"),
            None,
            "a refused open must not stamp the ledger"
        );
    }

    /// A member that never ran a turn leaves a v1 file: the head-canonical
    /// tables wait for the first delta write. Resolving its record by session
    /// at the next boot must read the fence from the snapshots alone, not
    /// fail on the absent table (which skipped the member's owner
    /// pre-registration, so the restart never materialized it).
    #[tokio::test]
    async fn resolve_record_by_session_reads_the_fence_on_a_v1_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("domain:research").unwrap();
        let session_id = meerkat_core::types::SessionId::new();
        {
            let store = LocalContinuityStore::open(&path).expect("open");
            store
                .upsert_continuity_record(&record(&identity, &session_id), FencingToken::new(1))
                .await
                .unwrap();
        }
        let probe = Connection::open(&path).expect("probe");
        let heads: bool = probe
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' \
                 AND name = 'continuity_session_heads')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!heads, "no delta write ran, so the file is still v1");

        let reopened = LocalContinuityStore::open(&path).expect("reopen");
        let (resolved, token, fence) = reopened
            .resolve_record_by_session(&session_id)
            .await
            .expect("the fence lookup succeeds on a v1 file")
            .expect("the record is found");
        assert_eq!(resolved.identity, identity);
        assert_eq!(token, FencingToken::new(1));
        assert_eq!(fence, CheckpointVersion::new(0));
    }

    #[tokio::test]
    async fn delete_continuity_record_removes_record_and_snapshots_atomically() {
        // Regression: the record + its session snapshots must be deleted as one
        // transaction so a crash between the two DELETEs cannot leave a
        // half-deleted store. Functionally: after a successful delete BOTH the
        // continuity record and the snapshot must be gone.
        let store = LocalContinuityStore::in_memory().expect("in-memory store");
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let session_id = meerkat_core::types::SessionId::new();

        store
            .upsert_continuity_record(&record(&identity, &session_id), FencingToken::new(1))
            .await
            .unwrap();
        store
            .save_session_snapshot(
                &identity,
                &session_id,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(1),
                FencingToken::new(1),
                &SessionSnapshot {
                    data: vec![1, 2, 3],
                },
            )
            .await
            .unwrap();

        // Both present before delete.
        assert!(
            store
                .load_session_snapshot(&session_id)
                .await
                .unwrap()
                .is_some()
        );

        store
            .delete_continuity_record(&identity, FencingToken::new(2))
            .await
            .unwrap();

        // Record gone: resolve returns Uninitialized.
        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .unwrap();
        assert!(matches!(
            resolved.get(&identity),
            Some(ContinuityResolveState::Uninitialized)
        ));
        // Snapshot gone too (same transaction).
        assert!(
            store
                .load_session_snapshot(&session_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn delete_continuity_record_rejects_stale_fencing_token() {
        let store = LocalContinuityStore::in_memory().expect("in-memory store");
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let session_id = meerkat_core::types::SessionId::new();
        store
            .upsert_continuity_record(&record(&identity, &session_id), FencingToken::new(5))
            .await
            .unwrap();

        let err = store
            .delete_continuity_record(&identity, FencingToken::new(2))
            .await
            .expect_err("stale fencing token must be rejected");
        assert!(matches!(
            err,
            ContinuityStoreError::StaleFencingToken { .. }
        ));

        // The record must survive a rejected delete.
        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .unwrap();
        assert!(!matches!(
            resolved.get(&identity),
            Some(ContinuityResolveState::Uninitialized)
        ));
    }

    #[tokio::test]
    async fn continuity_upsert_rejects_generation_regression_even_with_newer_fence() {
        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let session_id = meerkat_core::types::SessionId::new();
        let mut current = record(&identity, &session_id);
        current.generation = ContinuityGeneration::new(1);
        store
            .upsert_continuity_record(&current, FencingToken::new(2))
            .await
            .unwrap();

        let mut stale = current.clone();
        stale.generation = ContinuityGeneration::new(0);
        let error = store
            .upsert_continuity_record(&stale, FencingToken::new(3))
            .await
            .expect_err("a newer fence must not authorize generation rollback");
        assert!(matches!(
            error,
            ContinuityStoreError::StaleContinuityGeneration { .. }
        ));
        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .unwrap();
        let ContinuityResolveState::Ready { record } = &resolved[&identity] else {
            panic!("continuity should remain ready");
        };
        assert_eq!(record.generation, ContinuityGeneration::new(1));
    }

    #[tokio::test]
    async fn same_generation_session_rebind_preserves_durable_checkpoint_head() {
        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let old_session_id = meerkat_core::types::SessionId::new();
        let mut old = record(&identity, &old_session_id);
        old.checkpoint_version = CheckpointVersion::new(10);
        store
            .upsert_continuity_record(&old, FencingToken::new(1))
            .await
            .unwrap();
        store
            .save_session_snapshot(
                &identity,
                &old_session_id,
                old.generation,
                CheckpointVersion::new(11),
                FencingToken::new(2),
                &SessionSnapshot { data: vec![11] },
            )
            .await
            .unwrap();

        let new_session_id = meerkat_core::types::SessionId::new();
        let mut stale_rebind = old;
        stale_rebind.session_id = new_session_id.clone();
        store
            .upsert_continuity_record(&stale_rebind, FencingToken::new(3))
            .await
            .unwrap();

        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .unwrap();
        let ContinuityResolveState::Ready { record } = &resolved[&identity] else {
            panic!("continuity should remain ready");
        };
        assert_eq!(record.session_id, new_session_id);
        assert_eq!(record.checkpoint_version, CheckpointVersion::new(11));
        assert!(
            store
                .resolve_record_by_session(&old_session_id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.session_owner(&old_session_id).await.unwrap(),
            Some(identity.clone())
        );
        assert_eq!(
            store.session_owner(&new_session_id).await.unwrap(),
            Some(identity)
        );
    }

    #[tokio::test]
    async fn session_owner_reads_retained_heads_and_rejects_conflicting_owners() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("continuity.sqlite3");
        let store = LocalContinuityStore::open(&path).unwrap();
        let session_id = meerkat_core::types::SessionId::new();
        assert_eq!(store.session_owner(&session_id).await.unwrap(), None);
        let owner = AgentIdentity::parse("triage:main").unwrap();
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(SCHEMA_HEAD_CANONICAL).unwrap();
        connection
            .execute(
                "INSERT INTO continuity_session_heads \
             (session_id, identity, generation, checkpoint_version, fencing_token, \
              head_revision, message_count, rewrite_count, head_json, cas_token) \
             VALUES (?1, ?2, 0, 1, 1, 'revision', 0, 0, X'7B7D', 'cas')",
                rusqlite::params![session_id.to_string(), owner.as_str()],
            )
            .unwrap();
        assert_eq!(
            store.session_owner(&session_id).await.unwrap(),
            Some(owner.clone())
        );
        connection
            .execute(
                "INSERT INTO session_snapshots \
             (session_id, identity, generation, checkpoint_version, fencing_token, data) \
             VALUES (?1, 'different:owner', 0, 1, 1, X'7B7D')",
                rusqlite::params![session_id.to_string()],
            )
            .unwrap();
        assert!(matches!(
            store.session_owner(&session_id).await,
            Err(ContinuityStoreError::Corruption(_))
        ));
        connection
            .execute(
                "UPDATE session_snapshots SET identity = ?1 WHERE session_id = ?2",
                rusqlite::params![owner.as_str(), session_id.to_string()],
            )
            .unwrap();
        assert_eq!(store.session_owner(&session_id).await.unwrap(), Some(owner));
        connection
            .execute(
                "INSERT INTO continuity_records (identity, agent_runtime_id, session_id, \
             generation, checkpoint_version, fencing_token) \
             VALUES ('different:owner', 'rt-001', ?1, 0, 1, 1)",
                rusqlite::params![session_id.to_string()],
            )
            .unwrap();
        assert!(matches!(
            store.session_owner(&session_id).await,
            Err(ContinuityStoreError::Corruption(_))
        ));
    }

    #[tokio::test]
    async fn reset_rollback_cas_restores_previous_row_and_only_removes_attempt_snapshots() {
        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let previous_session = meerkat_core::types::SessionId::new();
        let mut previous = record(&identity, &previous_session);
        store
            .upsert_continuity_record(&previous, FencingToken::new(1))
            .await
            .unwrap();
        store
            .save_session_snapshot(
                &identity,
                &previous_session,
                previous.generation,
                CheckpointVersion::new(1),
                FencingToken::new(1),
                &SessionSnapshot { data: vec![10] },
            )
            .await
            .unwrap();
        previous.checkpoint_version = CheckpointVersion::new(1);

        let attempted_session = meerkat_core::types::SessionId::new();
        let mut attempted = record(&identity, &attempted_session);
        attempted.agent_runtime_id = AgentRuntimeId::parse("rt:triage:main:1").unwrap();
        attempted.generation = ContinuityGeneration::new(1);
        store
            .upsert_continuity_record(&attempted, FencingToken::new(2))
            .await
            .unwrap();
        store
            .save_session_snapshot(
                &identity,
                &attempted_session,
                attempted.generation,
                CheckpointVersion::new(1),
                FencingToken::new(2),
                &SessionSnapshot { data: vec![20] },
            )
            .await
            .unwrap();

        // The session service may have advanced the attempted checkpoint
        // after reset captured the provisional record. Runtime/session/
        // generation/fence identify the attempt; its checkpoint is not part
        // of the rollback CAS.
        store
            .rollback_continuity_record(&attempted, Some(&previous), FencingToken::new(2))
            .await
            .unwrap();

        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .unwrap();
        assert_eq!(
            resolved.get(&identity),
            Some(&ContinuityResolveState::Ready {
                record: previous.clone(),
            })
        );
        assert_eq!(
            store
                .load_session_snapshot(&previous_session)
                .await
                .unwrap(),
            Some(SessionSnapshot { data: vec![10] })
        );
        assert_eq!(
            store
                .load_session_snapshot(&attempted_session)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn reset_rollback_cas_deletes_uninitialized_attempt_and_its_snapshots() {
        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:new").unwrap();
        let attempted_session = meerkat_core::types::SessionId::new();
        let mut attempted = record(&identity, &attempted_session);
        attempted.agent_runtime_id = AgentRuntimeId::parse("rt:triage:new:1").unwrap();
        attempted.generation = ContinuityGeneration::new(1);
        store
            .upsert_continuity_record(&attempted, FencingToken::new(1))
            .await
            .unwrap();
        store
            .save_session_snapshot(
                &identity,
                &attempted_session,
                attempted.generation,
                CheckpointVersion::new(1),
                FencingToken::new(1),
                &SessionSnapshot { data: vec![30] },
            )
            .await
            .unwrap();

        store
            .rollback_continuity_record(&attempted, None, FencingToken::new(1))
            .await
            .unwrap();

        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .unwrap();
        assert_eq!(
            resolved.get(&identity),
            Some(&ContinuityResolveState::Uninitialized)
        );
        assert!(
            store
                .load_session_snapshot(&attempted_session)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn reset_rollback_cas_cannot_clobber_a_newer_attempt() {
        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let previous_session = meerkat_core::types::SessionId::new();
        let previous = record(&identity, &previous_session);
        store
            .upsert_continuity_record(&previous, FencingToken::new(1))
            .await
            .unwrap();

        let attempted_session = meerkat_core::types::SessionId::new();
        let mut attempted = record(&identity, &attempted_session);
        attempted.agent_runtime_id = AgentRuntimeId::parse("rt:triage:main:1").unwrap();
        attempted.generation = ContinuityGeneration::new(1);
        store
            .upsert_continuity_record(&attempted, FencingToken::new(2))
            .await
            .unwrap();
        store
            .save_session_snapshot(
                &identity,
                &attempted_session,
                attempted.generation,
                CheckpointVersion::new(1),
                FencingToken::new(2),
                &SessionSnapshot { data: vec![40] },
            )
            .await
            .unwrap();

        let newer_session = meerkat_core::types::SessionId::new();
        let mut newer = record(&identity, &newer_session);
        newer.agent_runtime_id = AgentRuntimeId::parse("rt:triage:main:2").unwrap();
        newer.generation = ContinuityGeneration::new(2);
        store
            .upsert_continuity_record(&newer, FencingToken::new(3))
            .await
            .unwrap();

        let error = store
            .rollback_continuity_record(&attempted, Some(&previous), FencingToken::new(2))
            .await
            .expect_err("a stale reset attempt must not overwrite a newer generation");
        assert!(matches!(
            error,
            ContinuityStoreError::StaleFencingToken { .. }
        ));
        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .unwrap();
        assert_eq!(
            resolved.get(&identity),
            Some(&ContinuityResolveState::Ready { record: newer })
        );
        assert_eq!(
            store
                .load_session_snapshot(&attempted_session)
                .await
                .unwrap(),
            Some(SessionSnapshot { data: vec![40] })
        );
    }

    #[tokio::test]
    async fn max_fencing_token_recovers_high_water_across_tables_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("continuity.db");
        let identity = AgentIdentity::parse("identity:lead-1").unwrap();
        let session_id = meerkat_core::types::SessionId::new();
        {
            let store = LocalContinuityStore::open(&path).unwrap();
            assert_eq!(store.max_fencing_token().unwrap(), 0, "empty store -> 0");
            // First boot: continuity record + session snapshot both at token 1.
            store
                .upsert_continuity_record(&record(&identity, &session_id), FencingToken::new(1))
                .await
                .unwrap();
            store
                .save_session_snapshot(
                    &identity,
                    &session_id,
                    ContinuityGeneration::new(0),
                    CheckpointVersion::new(1),
                    FencingToken::new(1),
                    &SessionSnapshot {
                        data: vec![1, 2, 3],
                    },
                )
                .await
                .unwrap();
            // Reconcile re-bumps the continuity record to 15; the snapshot stays
            // at 1 — the two-table divergence from the field report.
            store
                .upsert_continuity_record(&record(&identity, &session_id), FencingToken::new(15))
                .await
                .unwrap();
            assert_eq!(
                store.max_fencing_token().unwrap(),
                15,
                "high-water = MAX over continuity_records (15) and session_snapshots (1)"
            );
        }
        // Restart: the high-water must survive re-opening the same db file.
        let store = LocalContinuityStore::open(&path).unwrap();
        assert_eq!(
            store.max_fencing_token().unwrap(),
            15,
            "high-water must persist across reopen"
        );

        // The session_snapshots arm of the union must actually count: a snapshot
        // whose token exceeds the continuity record (the crash-window case the
        // MAX-over-both-tables query is for) becomes the high-water.
        let snap_only = LocalContinuityStore::in_memory().unwrap();
        let sid = meerkat_core::types::SessionId::new();
        snap_only
            .save_session_snapshot(
                &identity,
                &sid,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(1),
                FencingToken::new(7),
                &SessionSnapshot { data: vec![9] },
            )
            .await
            .unwrap();
        assert_eq!(
            snap_only.max_fencing_token().unwrap(),
            7,
            "high-water must come from session_snapshots when no continuity record is present"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_open_keeps_tokio_worker_responsive_while_sqlite_is_locked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("async-open.db");
        let lock = Connection::open(&path).unwrap();
        lock.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; BEGIN IMMEDIATE;")
            .unwrap();

        let open = tokio::spawn({
            let path = path.clone();
            async move { LocalContinuityStore::open_with_fencing_floor(path).await }
        });
        tokio::time::timeout(
            std::time::Duration::from_millis(250),
            tokio::time::sleep(std::time::Duration::from_millis(25)),
        )
        .await
        .expect("the current-thread Tokio worker must remain responsive");
        assert!(
            !open.is_finished(),
            "schema initialization should still be waiting on the held SQLite writer lock"
        );

        lock.execute_batch("ROLLBACK;").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), open)
            .await
            .expect("async open should finish after releasing the SQLite lock")
            .expect("open task should not panic")
            .expect("store should open and read its fencing floor");
    }

    /// The end-to-end restart regression: a lease provider seeded from the
    /// persisted high-water issues a token that the store accepts on restore,
    /// while a provider that reset to 1 (the v0.7.8 bug) is rejected as stale.
    #[tokio::test]
    async fn lease_fencing_resumes_above_high_water_on_restart() {
        use super::super::contracts::LeaseProvider;
        use super::super::local_lease::LocalLeaseProvider;
        use super::super::types::LeaseAcquireResult;

        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("identity:lead-1").unwrap();
        let session_id = meerkat_core::types::SessionId::new();
        // Pre-restart history: reconcile bumped the continuity record to 15.
        store
            .upsert_continuity_record(&record(&identity, &session_id), FencingToken::new(15))
            .await
            .unwrap();

        // Restart: seed a fresh lease provider from the persisted high-water.
        let high_water = store.max_fencing_token().unwrap();
        assert_eq!(high_water, 15);
        let provider = LocalLeaseProvider::with_floor(high_water);
        let acquired = provider
            .acquire_leases(std::slice::from_ref(&identity), "rt-restart")
            .await
            .unwrap();
        let token = match acquired.get(&identity) {
            Some(LeaseAcquireResult::Acquired(grant)) => grant.fencing_token,
            _ => panic!("expected an acquired lease"),
        };
        assert!(
            token.get() > high_water,
            "resumed token {} must exceed the high-water {high_water}",
            token.get()
        );
        // The restore upsert with the resumed token SUCCEEDS (not stale).
        store
            .upsert_continuity_record(&record(&identity, &session_id), token)
            .await
            .expect("a token resumed above the high-water must be accepted");

        // Prove the bug this fixes: a provider that reset to 1 IS rejected.
        let reset_provider = LocalLeaseProvider::with_floor(0);
        let reset_acquired = reset_provider
            .acquire_leases(std::slice::from_ref(&identity), "rt-reset")
            .await
            .unwrap();
        let reset_token = match reset_acquired.get(&identity) {
            Some(LeaseAcquireResult::Acquired(grant)) => grant.fencing_token,
            _ => panic!("expected an acquired lease"),
        };
        let err = store
            .upsert_continuity_record(&record(&identity, &session_id), reset_token)
            .await
            .expect_err("a reset-to-1 token must be rejected as stale");
        assert!(matches!(
            err,
            ContinuityStoreError::StaleFencingToken { .. }
        ));
    }

    #[tokio::test]
    async fn exact_snapshot_match_requires_the_current_continuity_head() {
        let store = LocalContinuityStore::in_memory().expect("in-memory store");
        let identity = AgentIdentity::parse("agent:exact-match").unwrap();
        let session_id = meerkat_core::types::SessionId::new();
        let snapshot = SessionSnapshot {
            data: vec![1, 3, 3, 7],
        };
        let continuity_record = record(&identity, &session_id);
        store
            .upsert_continuity_record(&continuity_record, FencingToken::new(1))
            .await
            .unwrap();
        store
            .save_session_snapshot(
                &identity,
                &session_id,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(1),
                FencingToken::new(1),
                &snapshot,
            )
            .await
            .unwrap();

        let candidate = SessionSnapshotMatchCandidate {
            identity: identity.clone(),
            session_id: session_id.clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(1),
            fencing_token: FencingToken::new(1),
            snapshot: Arc::new(snapshot),
        };
        assert!(
            store
                .session_snapshot_matches_current(candidate.clone())
                .await
                .unwrap(),
            "the complete durable provenance tuple and bytes should match"
        );

        store
            .upsert_continuity_record(&continuity_record, FencingToken::new(2))
            .await
            .unwrap();
        assert!(
            !store
                .session_snapshot_matches_current(candidate.clone())
                .await
                .unwrap(),
            "a stale presented write fence must not match a newer continuity head"
        );
        assert!(
            store
                .session_snapshot_matches_current(SessionSnapshotMatchCandidate {
                    fencing_token: FencingToken::new(2),
                    ..candidate
                })
                .await
                .unwrap(),
            "the historical row fence is provenance and may precede current write authority"
        );
    }

    #[tokio::test]
    async fn snapshot_save_rejects_another_identity_owning_the_session_id() {
        let store = LocalContinuityStore::in_memory().expect("in-memory store");
        let first = AgentIdentity::parse("agent:first-owner").unwrap();
        let second = AgentIdentity::parse("agent:second-owner").unwrap();
        let session_id = meerkat_core::types::SessionId::new();
        store
            .upsert_continuity_record(&record(&first, &session_id), FencingToken::new(1))
            .await
            .unwrap();
        store
            .save_session_snapshot(
                &first,
                &session_id,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(1),
                FencingToken::new(1),
                &SessionSnapshot { data: vec![1] },
            )
            .await
            .unwrap();
        store
            .upsert_continuity_record(&record(&second, &session_id), FencingToken::new(2))
            .await
            .unwrap();

        let error = store
            .save_session_snapshot(
                &second,
                &session_id,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(1),
                FencingToken::new(2),
                &SessionSnapshot { data: vec![2] },
            )
            .await
            .expect_err("a different identity must not overwrite the session row");
        assert!(matches!(error, ContinuityStoreError::Corruption(_)));
        assert_eq!(
            store.load_session_snapshot(&session_id).await.unwrap(),
            Some(SessionSnapshot { data: vec![1] })
        );
    }

    #[tokio::test]
    async fn snapshot_save_rejects_same_identity_from_another_generation_atomically() {
        let store = LocalContinuityStore::in_memory().expect("in-memory store");
        let identity = AgentIdentity::parse("agent:generation-owner").unwrap();
        let session_id = meerkat_core::types::SessionId::new();
        store
            .upsert_continuity_record(&record(&identity, &session_id), FencingToken::new(1))
            .await
            .unwrap();
        store
            .save_session_snapshot(
                &identity,
                &session_id,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(1),
                FencingToken::new(1),
                &SessionSnapshot { data: vec![1] },
            )
            .await
            .unwrap();

        let identity_for_update = identity.clone();
        store
            .run_blocking("advance-test-generation", move |inner| {
                inner.with_writer(|connection| {
                    connection
                        .execute(
                            "UPDATE continuity_records
                             SET generation = 1, checkpoint_version = 0, fencing_token = 2
                             WHERE identity = ?1",
                            rusqlite::params![identity_for_update.as_str()],
                        )
                        .map_err(|error| {
                            ContinuityStoreError::Io(format!(
                                "advance test continuity generation: {error}"
                            ))
                        })?;
                    Ok(())
                })
            })
            .await
            .unwrap();

        let error = store
            .save_session_snapshot(
                &identity,
                &session_id,
                ContinuityGeneration::new(1),
                CheckpointVersion::new(1),
                FencingToken::new(2),
                &SessionSnapshot { data: vec![2] },
            )
            .await
            .expect_err("a new generation must not overwrite the prior session row");
        assert!(matches!(error, ContinuityStoreError::Corruption(_)));
        assert_eq!(
            store.load_session_snapshot(&session_id).await.unwrap(),
            Some(SessionSnapshot { data: vec![1] }),
            "failed cross-generation save must leave snapshot bytes unchanged"
        );
        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .unwrap();
        let ContinuityResolveState::Ready { record } = resolved.get(&identity).unwrap() else {
            panic!("expected ready continuity head");
        };
        assert_eq!(record.generation, ContinuityGeneration::new(1));
        assert_eq!(record.checkpoint_version, CheckpointVersion::new(0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_worker_keeps_the_async_executor_responsive() {
        let store = LocalContinuityStore::in_memory().expect("in-memory store");
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let work = tokio::spawn(async move {
            store
                .run_blocking("blocking-worker-test", move |_| {
                    let _ = started_tx.send(());
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    Ok(())
                })
                .await
        });

        started_rx.await.expect("blocking worker started");
        assert!(!work.is_finished(), "worker should still be sleeping");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(
            !work.is_finished(),
            "the current-thread executor should run while SQLite work is blocking"
        );
        work.await.expect("worker task joined").unwrap();
    }

    #[tokio::test]
    async fn file_backed_store_serves_reads_from_a_bounded_pool() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalContinuityStore::open(dir.path().join("read-pool.db")).unwrap();
        let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let max_active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut tasks = Vec::with_capacity(READ_POOL_SIZE);

        for _ in 0..READ_POOL_SIZE {
            let store = store.clone();
            let active = active.clone();
            let max_active = max_active.clone();
            let release = release.clone();
            tasks.push(tokio::spawn(async move {
                store
                    .run_blocking("read-pool-test", move |inner| {
                        inner.with_reader(|_| {
                            let now = active.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            max_active.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                            while !release.load(std::sync::atomic::Ordering::SeqCst) {
                                std::thread::sleep(std::time::Duration::from_millis(1));
                            }
                            active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                            Ok(())
                        })
                    })
                    .await
            }));
        }

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        while max_active.load(std::sync::atomic::Ordering::SeqCst) < READ_POOL_SIZE
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        release.store(true, std::sync::atomic::Ordering::SeqCst);
        for task in tasks {
            task.await.expect("read task joined").unwrap();
        }
        assert_eq!(
            max_active.load(std::sync::atomic::Ordering::SeqCst),
            READ_POOL_SIZE,
            "all bounded reader connections should be independently usable"
        );
    }

    #[tokio::test]
    async fn cloned_in_memory_store_shares_one_database() {
        let store = LocalContinuityStore::in_memory().expect("in-memory store");
        let reader = store.clone();
        let identity = AgentIdentity::parse("agent:shared-memory").unwrap();
        let session_id = meerkat_core::types::SessionId::new();
        store
            .upsert_continuity_record(&record(&identity, &session_id), FencingToken::new(1))
            .await
            .unwrap();

        let resolved = reader
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .unwrap();
        assert!(matches!(
            resolved.get(&identity),
            Some(ContinuityResolveState::Ready { .. })
        ));
    }

    // -----------------------------------------------------------------
    // M4b: the head-canonical session-delta channel
    // -----------------------------------------------------------------

    fn session_with(texts: &[&str]) -> Session {
        let mut session = Session::new();
        for text in texts {
            session.push(meerkat_core::Message::User(
                meerkat_core::UserMessage::text((*text).to_string()),
            ));
        }
        session
    }

    /// A ledgered-v1 corpus in the shape a real deployment actually has:
    /// the store's own open converges the DDL, the deferred stamp leaves the
    /// ledger at v1, and a blob row exists with no head row adopting it.
    fn plant_ledgered_v1_blob(path: &Path, identity: &AgentIdentity, session: &Session) {
        drop(LocalContinuityStore::open(path).expect("open converges schema"));
        let conn = Connection::open(path).expect("plant");
        conn.execute(
            "INSERT INTO session_snapshots (session_id, identity, generation, \
             checkpoint_version, fencing_token, data) VALUES (?1, ?2, 3, 5, 9, ?3)",
            rusqlite::params![
                session.id().to_string(),
                identity.as_str(),
                serde_json::to_vec(session).expect("encode session")
            ],
        )
        .expect("plant blob row");
    }

    fn continuity_domain_version(path: &Path) -> Option<i64> {
        let conn = Connection::open(path).expect("probe");
        meerkat_sqlite::domain_version(&conn, META_CONTINUITY_DOMAIN.name).expect("domain version")
    }

    /// Head rows, tolerating the table's absence — a real v1 corpus has NO
    /// head-canonical tables at all (they are created inside the delta
    /// write's transaction, not at open), so "missing table" is zero rows
    /// rather than an error.
    fn head_row_count(path: &Path) -> i64 {
        let conn = Connection::open(path).expect("probe");
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name='continuity_session_heads'",
                [],
                |row| row.get(0),
            )
            .expect("probe head table");
        if exists == 0 {
            return 0;
        }
        conn.query_row("SELECT COUNT(*) FROM continuity_session_heads", [], |row| {
            row.get(0)
        })
        .expect("count head rows")
    }

    #[test]
    fn backfill_converts_a_ledgered_v1_corpus_and_stamps_only_on_complete_conversion() {
        use std::collections::BTreeSet;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:main").expect("identity");
        let session = session_with(&["hello", "world"]);
        plant_ledgered_v1_blob(&path, &identity, &session);

        // Fixture self-check: the deferred stamp means a converged file is
        // still v1. If this ever fails the fixture is not the shape under
        // test and every assertion below is meaningless.
        assert_eq!(
            continuity_domain_version(&path),
            Some(1),
            "fixture must be a LEDGERED v1 corpus, not a stamped one"
        );
        assert_eq!(head_row_count(&path), 0, "fixture must have no head row");

        // ---- dry run mutates nothing, including the ledger ----
        let dry = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            false,
            &BTreeSet::new(),
        )
        .expect("dry run");
        assert_eq!(dry.examined, 1);
        assert!(!dry.applied);
        assert!(!dry.ledger_stamped);
        assert!(dry.converted.is_empty());
        assert_eq!(continuity_domain_version(&path), Some(1), "dry run stamped");
        assert_eq!(head_row_count(&path), 0, "dry run converted");

        // ---- apply converts, retains the blob, and stamps ----
        let applied = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &BTreeSet::new(),
        )
        .expect("apply");
        assert_eq!(applied.converted.len(), 1);
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert!(applied.complete());
        assert!(applied.ledger_stamped, "complete conversion must stamp");
        assert_eq!(head_row_count(&path), 1, "head row not created");
        assert_eq!(continuity_domain_version(&path), Some(2), "not stamped v2");

        // The blob is a frozen archive, never deleted.
        let conn = Connection::open(&path).expect("probe");
        let blobs: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_snapshots", [], |row| {
                row.get(0)
            })
            .expect("count blobs");
        assert_eq!(blobs, 1, "the legacy blob must be retained as an archive");

        // ---- re-running is idempotent ----
        // The stamp asserts "no legacy blob is left unconverted", not "this
        // run did work", so a second run re-affirms it as a no-op rather than
        // withholding it. What must NOT happen is re-converting a session or
        // duplicating its head row.
        let again = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &BTreeSet::new(),
        )
        .expect("second apply");
        // Examined counts EVERY blob row, so an already-converted session is
        // still examined — it just produces no work.
        assert_eq!(again.examined, 1);
        assert!(
            again.converted.is_empty(),
            "re-converted an already-converted session"
        );
        assert!(again.failures.is_empty(), "{:?}", again.failures);
        assert_eq!(head_row_count(&path), 1, "second run duplicated head rows");
        assert_eq!(continuity_domain_version(&path), Some(2));
    }

    #[test]
    fn backfill_leaves_the_ledger_at_v1_when_a_session_cannot_convert() {
        use std::collections::BTreeSet;
        // The whole point of deferring the stamp: a corpus that did not fully
        // cross keeps rollback to a pre-head-canonical release available.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:main").expect("identity");
        let session = session_with(&["ok"]);
        plant_ledgered_v1_blob(&path, &identity, &session);
        {
            // A second blob row that cannot decode into a Session.
            let conn = Connection::open(&path).expect("plant");
            conn.execute(
                "INSERT INTO session_snapshots (session_id, identity, generation, \
                 checkpoint_version, fencing_token, data) VALUES (?1, ?2, 3, 5, 9, X'6E6F7065')",
                rusqlite::params![
                    meerkat_core::types::SessionId::new().to_string(),
                    identity.as_str()
                ],
            )
            .expect("plant undecodable blob");
        }

        let applied = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &BTreeSet::new(),
        )
        .expect("apply");
        assert_eq!(applied.examined, 2);
        assert!(!applied.failures.is_empty(), "undecodable blob must fail");
        assert!(!applied.complete());
        assert!(
            !applied.ledger_stamped,
            "a partial conversion must NOT stamp — rollback stays available"
        );
        assert_eq!(
            continuity_domain_version(&path),
            Some(1),
            "partial run must leave the file at v1"
        );
    }

    #[test]
    fn a_post_failure_file_resumes_on_the_remainder_without_reconverting() {
        use std::collections::BTreeSet;
        // Idempotence on a CLEAN file is the easy input. The interesting one
        // is the file a failed crossing leaves behind: A already has a head
        // row, so the census must skip it and retry only B. If that is wrong,
        // resumability is broken exactly where an operator needs it.
        // (Raised by the deployment owner reviewing the rollback spec.)
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:main").expect("identity");
        let good = session_with(&["keep me"]);
        plant_ledgered_v1_blob(&path, &identity, &good);
        // Session ids are UUIDv7, so one created later sorts later: B is
        // attempted after A has already committed its own transaction.
        // B is a REAL session whose stored BYTES are corrupt, so the row's
        // session_id and the blob's own id agree. (A blob whose internal id
        // disagrees with its row is a different defect, and the re-census
        // correctly refuses to stamp on it — which is how this fixture was
        // caught being wrong.)
        let doomed_session = session_with(&["recovered"]);
        let doomed = doomed_session.id().clone();
        let doomed_bytes = serde_json::to_vec(&doomed_session).expect("encode");
        {
            let conn = Connection::open(&path).expect("plant");
            conn.execute(
                "INSERT INTO session_snapshots (session_id, identity, generation, \
                 checkpoint_version, fencing_token, data) VALUES (?1, ?2, 3, 5, 9, X'6E6F7065')",
                rusqlite::params![doomed.to_string(), identity.as_str()],
            )
            .expect("plant doomed blob");
        }

        // ---- the failed crossing ----
        let failed = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &BTreeSet::new(),
        )
        .expect("first apply");
        assert_eq!(failed.examined, 2);
        assert_eq!(failed.converted, vec![good.id().to_string()]);
        assert_eq!(failed.failures.len(), 1);
        assert!(!failed.ledger_stamped);
        assert_eq!(continuity_domain_version(&path), Some(1));
        assert_eq!(head_row_count(&path), 1);

        // ---- re-run on the POST-FAILURE file: resume, do not re-convert ----
        let resumed = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &BTreeSet::new(),
        )
        .expect("resume apply");
        assert_eq!(
            resumed.examined, 2,
            "both rows are examined; A produces no work, B is retried"
        );
        assert!(
            resumed.reconverted.is_empty(),
            "A did not change, so it must not be reconverted"
        );
        assert!(resumed.converted.is_empty(), "B still cannot convert");
        assert_eq!(head_row_count(&path), 1, "A was re-converted or duplicated");
        assert_eq!(continuity_domain_version(&path), Some(1));

        // ---- repair B, re-run: the corpus completes and only then stamps ----
        {
            let conn = Connection::open(&path).expect("repair");
            conn.execute(
                "UPDATE session_snapshots SET data = ?2 WHERE session_id = ?1",
                rusqlite::params![doomed.to_string(), doomed_bytes],
            )
            .expect("repair blob");
        }
        let finished = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &BTreeSet::new(),
        )
        .expect("final apply");
        assert_eq!(finished.examined, 2);
        assert_eq!(finished.converted.len(), 1);
        assert!(finished.ledger_stamped, "completed corpus must stamp");
        assert_eq!(head_row_count(&path), 2);
        assert_eq!(continuity_domain_version(&path), Some(2));
    }

    #[test]
    fn a_blob_that_changed_after_conversion_is_reconverted_before_the_stamp() {
        use std::collections::BTreeSet;
        // The live-deployment sequence, which is the DEFAULT path rather than
        // an edge case: a crossing fails partway, the ledger stays at v1, and
        // AT v1 THE WHOLE-DOCUMENT PATH IS STILL THE ACTIVE WRITER. So the
        // deployment keeps appending to blobs that were already converted.
        // On retry, a census keyed on "has a head row" would skip them and
        // stamp, burying every message written in the gap.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:main").expect("identity");
        let mut session = session_with(&["first"]);
        plant_ledgered_v1_blob(&path, &identity, &session);
        // A second row that cannot convert, so the first run fails partway.
        let doomed = meerkat_core::types::SessionId::new();
        {
            let conn = Connection::open(&path).expect("plant");
            conn.execute(
                "INSERT INTO session_snapshots (session_id, identity, generation, \
                 checkpoint_version, fencing_token, data) VALUES (?1, ?2, 3, 5, 9, X'6E6F7065')",
                rusqlite::params![doomed.to_string(), identity.as_str()],
            )
            .expect("plant doomed");
        }
        let failed = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &BTreeSet::new(),
        )
        .expect("first apply");
        assert_eq!(failed.converted, vec![session.id().to_string()]);
        assert!(!failed.ledger_stamped);

        // ---- the deployment keeps running: a message lands in the LEGACY blob ----
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("written after the failed crossing".to_string()),
        ));
        {
            let conn = Connection::open(&path).expect("append");
            conn.execute(
                "UPDATE session_snapshots SET data = ?2 WHERE session_id = ?1",
                rusqlite::params![
                    session.id().to_string(),
                    serde_json::to_vec(&session).expect("encode")
                ],
            )
            .expect("update blob");
            // ...and the unconvertible row is removed so the retry completes.
            conn.execute(
                "DELETE FROM session_snapshots WHERE session_id = ?1",
                rusqlite::params![doomed.to_string()],
            )
            .expect("drop doomed");
        }

        // ---- retry ----
        let retry = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &BTreeSet::new(),
        )
        .expect("retry apply");
        assert_eq!(
            retry.reconverted,
            vec![session.id().to_string()],
            "a blob that changed after conversion must be RECONVERTED, not skipped"
        );
        assert!(retry.ledger_stamped, "the completed corpus must stamp");

        // The head now reflects BOTH messages. Without the fix it would carry
        // one, and the second would exist only in a blob nothing reads.
        let conn = Connection::open(&path).expect("probe");
        let count: i64 = conn
            .query_row(
                "SELECT message_count FROM continuity_session_heads WHERE session_id = ?1",
                rusqlite::params![session.id().to_string()],
                |row| row.get(0),
            )
            .expect("head row");
        assert_eq!(
            count, 2,
            "the head is stale: the message written after the failed crossing was buried"
        );
    }

    /// Harness for the ROLLBACK PROOF, not a unit test — builds a corpus in
    /// the exact post-failure state the proof requires and leaves it on disk
    /// for an older binary to open.
    ///
    ///   META_ROLLBACK_CORPUS=/path/to/dir \
    ///     cargo test -p meerkat-meta --lib --all-features \
    ///     build_mid_run_failed_corpus_for_rollback_proof -- --ignored --nocapture
    ///
    /// Ordering is what makes the failure MID-RUN rather than pre-flight:
    /// session ids are UUIDv7, so the healthy session created first sorts
    /// first, converts, and COMMITS its own transaction before the poison row
    /// is attempted. A pre-flight failure would leave no head rows at all and
    /// would prove nothing.
    #[test]
    #[ignore = "harness: writes a corpus for the external rollback proof"]
    fn build_mid_run_failed_corpus_for_rollback_proof() {
        use std::collections::BTreeSet;
        let dir = std::env::var("META_ROLLBACK_CORPUS")
            .expect("set META_ROLLBACK_CORPUS to an existing directory");
        let path = std::path::Path::new(&dir).join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:main").expect("identity");
        let healthy = session_with(&["message one", "message two"]);
        plant_ledgered_v1_blob(&path, &identity, &healthy);
        let doomed = meerkat_core::types::SessionId::new();
        {
            let conn = Connection::open(&path).expect("plant");
            conn.execute(
                "INSERT INTO session_snapshots (session_id, identity, generation, \
                 checkpoint_version, fencing_token, data) VALUES (?1, ?2, 3, 5, 9, X'6E6F7065')",
                rusqlite::params![doomed.to_string(), identity.as_str()],
            )
            .expect("plant doomed");
        }
        let report = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &BTreeSet::new(),
        )
        .expect("apply");

        // The three conditions that make this a MID-RUN failure. If any fails,
        // the corpus is not the input the proof is about.
        assert_eq!(
            report.converted,
            vec![healthy.id().to_string()],
            "A must convert"
        );
        assert!(!report.failures.is_empty(), "B must fail");
        assert!(!report.ledger_stamped, "the ledger must NOT have advanced");
        assert_eq!(
            head_row_count(&path),
            1,
            "exactly one head row (A) must exist"
        );
        assert_eq!(
            continuity_domain_version(&path),
            Some(1),
            "file must remain v1"
        );
        let conn = Connection::open(&path).expect("probe");
        let blobs: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_snapshots", [], |r| r.get(0))
            .expect("count blobs");
        assert_eq!(blobs, 2, "both blobs must be retained");

        println!("ROLLBACK_CORPUS_PATH={}", path.display());
        println!("ROLLBACK_HEALTHY_SESSION={}", healthy.id());
        println!("ROLLBACK_DOOMED_SESSION={doomed}");
        println!("ROLLBACK_LEDGER_VERSION=1");
        println!("ROLLBACK_HEAD_ROWS=1");
        println!("ROLLBACK_BLOB_ROWS=2");
    }

    #[test]
    fn a_late_failure_preserves_the_record_of_work_already_done() {
        use std::collections::BTreeSet;
        // P1: a failure at the re-census or stamp used to return Err and
        // discard the whole report, handing the operator an error with no
        // record of the sessions that had already converted and committed.
        // They would have no way to know what to expect on a re-run.
        //
        // Provoked here through the ordinary blocking path: an unacknowledged
        // malformed row makes the run refuse the stamp AFTER a healthy
        // session has converted. The refusal must arrive as a report, not as
        // a discarded one.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:main").expect("identity");
        let healthy = session_with(&["kept"]);
        plant_ledgered_v1_blob(&path, &identity, &healthy);
        {
            let conn = Connection::open(&path).expect("plant");
            conn.execute(
                "INSERT INTO session_snapshots (session_id, identity, generation, \
                 checkpoint_version, fencing_token, data) VALUES ('not-a-uuid', ?1, 3, 5, 9, X'00')",
                rusqlite::params![identity.as_str()],
            )
            .expect("plant malformed");
        }

        let report = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &BTreeSet::new(),
        )
        .expect("a blocked stamp must still return a report, not an Err");
        assert_eq!(
            report.converted,
            vec![healthy.id().to_string()],
            "the record of the converted session must survive the refusal"
        );
        assert!(!report.failures.is_empty(), "the refusal must be recorded");
        assert!(!report.ledger_stamped);
        assert_eq!(head_row_count(&path), 1, "the conversion itself must stand");
    }

    #[test]
    fn a_dry_run_does_not_change_the_journal_mode_or_create_sidecars() {
        use std::collections::BTreeSet;
        // "Dry run mutates nothing" has to include PRAGMAs. The writer
        // profile sets journal_mode=WAL, so inspecting a DELETE-mode corpus
        // with a dry run used to convert it to WAL and leave -wal/-shm
        // siblings behind -- a durable change to a file the operator had
        // explicitly declined to modify.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:main").expect("identity");
        let session = session_with(&["hello"]);
        plant_ledgered_v1_blob(&path, &identity, &session);

        // Force the file to DELETE mode and close every connection.
        {
            let conn = Connection::open(&path).expect("open");
            let mode: String = conn
                .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
                .expect("set delete mode");
            assert_eq!(
                mode.to_lowercase(),
                "delete",
                "fixture must start in DELETE"
            );
        }
        let before_bytes = std::fs::read(&path).expect("read before");

        let dry = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            false,
            &BTreeSet::new(),
        )
        .expect("dry run");
        assert!(!dry.applied);
        assert_eq!(dry.examined, 1);

        let conn = Connection::open(&path).expect("probe");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("read mode");
        assert_eq!(
            mode.to_lowercase(),
            "delete",
            "a dry run converted the journal mode to WAL"
        );
        drop(conn);
        assert_eq!(
            std::fs::read(&path).expect("read after"),
            before_bytes,
            "a dry run changed the database bytes"
        );
        assert!(
            !path.with_extension("sqlite3-wal").exists(),
            "a dry run created a -wal sidecar"
        );
    }

    #[test]
    fn an_identity_mismatch_writes_nothing_at_all() {
        use std::collections::BTreeSet;
        // A blob that decodes PERFECTLY but into a DIFFERENT session than its
        // row key. Every write in the conversion is laid out from the decoded
        // session while the orphan delete targets the row key, so without a
        // guard this misattributes head/strand rows to the blob's session —
        // durably, before any re-census can refuse the ledger.
        //
        // The assertion is therefore ZERO ROWS WRITTEN, not "the ledger
        // stayed at v1". A stamp guard is not a write guard.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:main").expect("identity");
        let anchor_session = session_with(&["anchor"]);
        plant_ledgered_v1_blob(&path, &identity, &anchor_session);

        // Row key R, blob containing a different session V.
        let row_key = meerkat_core::types::SessionId::new();
        let victim = session_with(&["victim"]);
        {
            let conn = Connection::open(&path).expect("plant");
            conn.execute(
                "INSERT INTO session_snapshots (session_id, identity, generation, \
                 checkpoint_version, fencing_token, data) VALUES (?1, ?2, 3, 5, 9, ?3)",
                rusqlite::params![
                    row_key.to_string(),
                    identity.as_str(),
                    serde_json::to_vec(&victim).expect("encode")
                ],
            )
            .expect("plant mismatched blob");
        }

        let report = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &BTreeSet::new(),
        )
        .expect("apply");
        assert!(
            report
                .failures
                .iter()
                .any(|(row, _)| row == &row_key.to_string()),
            "the mismatched row must be reported as a failure: {:?}",
            report.failures
        );
        assert!(!report.ledger_stamped);

        let conn = Connection::open(&path).expect("probe");
        // NOTHING was written under the blob's id.
        for table in ["continuity_session_heads", "continuity_strand_messages"] {
            let n: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE session_id = ?1"),
                    rusqlite::params![victim.id().to_string()],
                    |row| row.get(0),
                )
                .expect("count victim rows");
            assert_eq!(n, 0, "{table} was written under the BLOB's session id");
        }
        // ...nor under the row key.
        let under_key: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM continuity_session_heads WHERE session_id = ?1",
                rusqlite::params![row_key.to_string()],
                |row| row.get(0),
            )
            .expect("count row-key heads");
        assert_eq!(
            under_key, 0,
            "a head row was written for a refused conversion"
        );
        // The healthy session still converted; one bad row does not strand it.
        assert_eq!(report.converted, vec![anchor_session.id().to_string()]);
    }

    #[test]
    fn malformed_blob_rows_block_the_stamp_until_acknowledged() {
        use std::collections::BTreeSet;
        // A row this classifier calls malformed may be a corrupted session OR
        // a real one the classifier is too strict about. Those are the same
        // from here, so the irreversible step waits for a human.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:main").expect("identity");
        let session = session_with(&["ok"]);
        plant_ledgered_v1_blob(&path, &identity, &session);
        {
            let conn = Connection::open(&path).expect("plant");
            conn.execute(
                "INSERT INTO session_snapshots (session_id, identity, generation, \
                 checkpoint_version, fencing_token, data) VALUES ('not-a-uuid', ?1, 3, 5, 9, X'00')",
                rusqlite::params![identity.as_str()],
            )
            .expect("plant malformed row");
        }

        // Unacknowledged: the convertible session still converts, but the
        // ledger refuses to advance.
        let blocked = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &BTreeSet::new(),
        )
        .expect("apply");
        assert_eq!(
            blocked.converted.len(),
            1,
            "convertible session must convert"
        );
        assert_eq!(blocked.skipped_unparseable, vec!["not-a-uuid".to_string()]);
        assert!(!blocked.failures.is_empty(), "must record the refusal");
        assert!(
            !blocked.ledger_stamped,
            "malformed row must block the stamp"
        );
        assert_eq!(continuity_domain_version(&path), Some(1));

        // Acknowledged BY NAME: acknowledging a different id must not work,
        // so the set carries the exact row the operator read.
        let wrong: BTreeSet<String> = ["some-other-row".to_string()].into_iter().collect();
        let still_blocked =
            LocalContinuityStore::backfill_head_canonical_sessions_at(&path, true, &wrong)
                .expect("apply with wrong ack");
        assert!(
            !still_blocked.ledger_stamped,
            "acknowledging a DIFFERENT row must not authorise the stamp"
        );
        let acknowledge_all: BTreeSet<String> = ["not-a-uuid".to_string()].into_iter().collect();
        let allowed = LocalContinuityStore::backfill_head_canonical_sessions_at(
            &path,
            true,
            &acknowledge_all,
        )
        .expect("apply acknowledged");
        assert_eq!(allowed.skipped_unparseable, vec!["not-a-uuid".to_string()]);
        assert!(
            allowed.ledger_stamped,
            "acknowledgement must permit the stamp"
        );
        assert_eq!(continuity_domain_version(&path), Some(2));
    }

    fn cursor(
        identity: &AgentIdentity,
        generation: u64,
        version: u64,
        token: u64,
    ) -> ContinuityWriteCursor {
        ContinuityWriteCursor {
            identity: identity.clone(),
            generation: ContinuityGeneration::new(generation),
            checkpoint_version: CheckpointVersion::new(version),
            fencing_token: FencingToken::new(token),
        }
    }

    fn ledger_version(path: &Path) -> Option<i64> {
        let probe = Connection::open(path).expect("probe");
        meerkat_sqlite::domain_version(&probe, "meta-continuity").expect("ledger")
    }

    async fn seed_record(
        store: &LocalContinuityStore,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        token: u64,
    ) {
        store
            .upsert_continuity_record(&record(identity, session_id), FencingToken::new(token))
            .await
            .expect("seed continuity record");
    }

    /// BLOCKER PIN: opening a state directory with this binary must NOT
    /// commit the one-way ledger bump. Rollback to the previous release stays
    /// possible until a delta write actually creates a head row.
    #[tokio::test]
    async fn open_never_stamps_the_head_canonical_ledger_bump() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        {
            let store = LocalContinuityStore::open(&path).expect("open");
            assert_eq!(store.max_fencing_token().expect("floor"), 0);
        }
        assert_eq!(
            ledger_version(&path),
            Some(1),
            "a plain open must leave the file at the rollback-safe baseline"
        );

        // Reopening, resolving, whole-blob saving: still no bump.
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let session_id = meerkat_core::types::SessionId::new();
        {
            let store = LocalContinuityStore::open(&path).expect("reopen");
            seed_record(&store, &identity, &session_id, 1).await;
            store
                .save_session_snapshot(
                    &identity,
                    &session_id,
                    ContinuityGeneration::new(0),
                    CheckpointVersion::new(1),
                    FencingToken::new(1),
                    &SessionSnapshot {
                        data: serde_json::to_vec(&session_with(&["blob turn"])).unwrap(),
                    },
                )
                .await
                .expect("whole-blob save");
        }
        assert_eq!(
            ledger_version(&path),
            Some(1),
            "ordinary whole-document saves must not commit the head-canonical bump"
        );

        // The delta write that CREATES HEAD STATE is where the v1-writer
        // lockout becomes load-bearing, and only there is the bump
        // committed. An append that adopts nothing does not earn it — see
        // `an_accepted_delta_write_that_creates_no_head_state_stays_at_v1`.
        {
            let store = LocalContinuityStore::open(&path).expect("reopen");
            let head_session = session_with(&["delta turn"]);
            let delta_session_id = head_session.id().clone();
            let delta_identity = AgentIdentity::parse("triage:delta").unwrap();
            seed_record(&store, &delta_identity, &delta_session_id, 2).await;
            let root = TranscriptStrandId::root();
            store
                .append_messages(
                    &cursor(&delta_identity, 0, 1, 2),
                    &delta_session_id,
                    &root,
                    0,
                    head_session.messages(),
                )
                .await
                .expect("first delta write");
            assert_eq!(
                ledger_version(&path),
                Some(1),
                "an append that no head adopts creates no authority an older binary \
                 could misread, so it must not commit the lockout"
            );
            let head = SessionHead::from_session(&head_session, root, 0).expect("head");
            store
                .save_head(
                    &cursor(&delta_identity, 0, 2, 2),
                    &head,
                    SessionHeadCas::Create,
                )
                .await
                .expect("adopting head write");
        }
        assert_eq!(
            ledger_version(&path),
            Some(HEAD_CANONICAL_SCHEMA_VERSION),
            "the delta write that creates head state commits the head-canonical bump"
        );
    }

    /// ROLLBACK-SAFETY PIN (N1): the lockout must be earned by HEAD STATE,
    /// not merely by an accepted write.
    ///
    /// `append_messages` on a session with neither a head row nor a blob to
    /// migrate — the real creation-window shape, because the service appends
    /// and adopts under two separate locks — commits rows and no head. Rows
    /// no head adopts are not part of any document (every read path gates on
    /// the head row), so an older binary still correctly serves that session
    /// from its blob. Arming the one-way lockout there buys a brick for
    /// nothing.
    ///
    /// The second half is the trap the first half sets: the DDL DOES commit
    /// with those rows, so the file now has head-canonical tables at ledger
    /// v1. A binary that decided "is the bump owed?" from a table probe
    /// would latch "already head-canonical" on the next open and never stamp
    /// again — head rows with no lockout, which is the very state the
    /// lockout exists to prevent. The reopen below is what catches that.
    #[tokio::test]
    async fn an_accepted_delta_write_that_creates_no_head_state_stays_at_v1() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:append-only").unwrap();
        let document = session_with(&["creation-window turn"]);
        let session_id = document.id().clone();
        let root = TranscriptStrandId::root();

        {
            let store = LocalContinuityStore::open(&path).expect("open");
            seed_record(&store, &identity, &session_id, 1).await;
            store
                .append_messages(
                    &cursor(&identity, 0, 1, 1),
                    &session_id,
                    &root,
                    0,
                    document.messages(),
                )
                .await
                .expect("an append with no adopting head is a legitimate accepted write");
        }

        assert_eq!(
            ledger_version(&path),
            Some(1),
            "an ACCEPTED write that creates zero head state must leave the file \
             rollback-safe at v1"
        );
        assert!(
            head_tables_exist(&path),
            "the rows are durable, so their (additive, IF NOT EXISTS) DDL committed with them"
        );
        // The v1-shaped reader the lockout protects: it must still open.
        {
            let probe = Connection::open(&path).expect("probe");
            refuse_future_schema_model(&probe, &V0_8_5_CONTINUITY_DOMAIN).expect(
                "a previous release must still open a file whose only head-canonical \
                 content is rows no head adopts",
            );
        }
        {
            let probe = Connection::open(&path).expect("probe");
            let heads: i64 = probe
                .query_row("SELECT COUNT(*) FROM continuity_session_heads", [], |row| {
                    row.get(0)
                })
                .expect("count heads");
            assert_eq!(heads, 0, "no head row was created");
            let rows: i64 = probe
                .query_row(
                    "SELECT COUNT(*) FROM continuity_strand_messages",
                    [],
                    |row| row.get(0),
                )
                .expect("count rows");
            assert_eq!(rows, 1, "the appended row is durable");
        }

        // Reopen (a fresh handle, ledger v1, tables present) and land the
        // adopting head. This MUST still stamp.
        {
            let store = LocalContinuityStore::open(&path).expect("reopen");
            // A restore reads before it writes, and the read observes the
            // tables. That is precisely what latches the "head tables are
            // queryable" flag — which is NOT the same fact as "the lockout
            // is committed". A binary that conflated them would now be
            // convinced the bump is already done and never stamp again.
            assert!(
                store
                    .load_canonical_head(&session_id)
                    .await
                    .expect("canonical head probe")
                    .is_none(),
                "rows no head adopts are not a document"
            );
            let head = SessionHead::from_session(&document, root.clone(), 0).expect("head");
            store
                .save_head(&cursor(&identity, 0, 2, 1), &head, SessionHeadCas::Create)
                .await
                .expect("the adopting head write");
        }
        assert_eq!(
            ledger_version(&path),
            Some(HEAD_CANONICAL_SCHEMA_VERSION),
            "the write that finally creates head state must commit the lockout, even \
             though the tables already existed when the handle opened"
        );
        {
            let probe = Connection::open(&path).expect("probe");
            match refuse_future_schema_model(&probe, &V0_8_5_CONTINUITY_DOMAIN) {
                Err(meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture { domain, .. }) => {
                    assert_eq!(domain, "meta-continuity");
                }
                other => panic!(
                    "once a head row exists the file must be closed to binaries that would keep \
                     writing the frozen blob archive as authority, got {other:?}"
                ),
            }
        }
    }

    fn head_tables_exist(path: &Path) -> bool {
        let probe = Connection::open(path).expect("probe");
        probe
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' \
                 AND name = 'continuity_session_heads')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("probe head tables")
    }

    /// ROLLBACK-SAFETY PIN: the one-way v1-writer lockout must be EARNED by
    /// a write that actually creates head state. A delta write refused by a
    /// guard, or refused by the operation's own CAS, leaves the file exactly
    /// as it found it — ledger v1, no head-canonical tables — so rolling
    /// back to the previous release stays possible.
    #[tokio::test]
    async fn a_refused_delta_write_never_arms_the_v1_writer_lockout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:refused").unwrap();
        let document = session_with(&["refused turn"]);
        let session_id = document.id().clone();
        let root = TranscriptStrandId::root();
        let head = SessionHead::from_session(&document, root.clone(), 0).expect("head");

        {
            let store = LocalContinuityStore::open(&path).expect("open");
            seed_record(&store, &identity, &session_id, 5).await;

            // (1) refused by a GUARD, before the operation runs at all.
            let stale = store
                .append_messages(
                    &cursor(&identity, 0, 1, 4),
                    &session_id,
                    &root,
                    0,
                    document.messages(),
                )
                .await
                .expect_err("a stale fencing token must refuse the delta write");
            assert!(
                stale.to_string().contains("stale fencing token"),
                "unexpected guard refusal: {stale}"
            );

            // (2) refused by the OPERATION's own CAS, after the guards pass.
            let conflict = store
                .save_head(
                    &cursor(&identity, 0, 1, 5),
                    &head,
                    SessionHeadCas::IfToken("row-sha256:nothing-like-this".to_string()),
                )
                .await
                .expect_err("a head CAS that cannot match must refuse the delta write");
            assert!(
                matches!(
                    conflict,
                    SessionStoreError::TranscriptRevisionConflict { .. }
                ),
                "unexpected operation refusal: {conflict}"
            );
        }
        assert_eq!(
            ledger_version(&path),
            Some(1),
            "a REFUSED delta write must not arm the one-way v1-writer lockout"
        );
        assert!(
            !head_tables_exist(&path),
            "a refused delta write must roll its speculative head-canonical DDL back"
        );

        // The same file still upgrades on a write that DOES create head
        // state. The append alone does not: it adopts nothing, so it commits
        // its rows and its DDL and leaves the file rollback-safe (pinned by
        // `an_accepted_delta_write_that_creates_no_head_state_stays_at_v1`).
        // The head write is what earns the bump.
        {
            let store = LocalContinuityStore::open(&path).expect("reopen");
            store
                .append_messages(
                    &cursor(&identity, 0, 1, 5),
                    &session_id,
                    &root,
                    0,
                    document.messages(),
                )
                .await
                .expect("an accepted delta write");
            assert_eq!(
                ledger_version(&path),
                Some(1),
                "an accepted append that no head adopts is still rollback-safe"
            );
            store
                .save_head(&cursor(&identity, 0, 2, 5), &head, SessionHeadCas::Create)
                .await
                .expect("the adopting head write");
        }
        assert_eq!(
            ledger_version(&path),
            Some(HEAD_CANONICAL_SCHEMA_VERSION),
            "a write that creates head state arms the lockout in the same transaction"
        );
        assert!(head_tables_exist(&path));
    }

    /// ALIGNMENT PIN: the delta channel and the whole-document verb are two
    /// write paths onto ONE durable session, so their accept/reject boundary
    /// must be identical. A session whose `session_snapshots` row is owned by
    /// another `(identity, generation)` is refused by BOTH — the intruder
    /// here holds a perfectly valid continuity cursor of its own, so nothing
    /// but the shared ownership guard can reject it.
    #[tokio::test]
    async fn delta_writes_refuse_the_foreign_snapshot_owner_the_blob_path_refuses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let owner = AgentIdentity::parse("triage:owner").unwrap();
        let intruder = AgentIdentity::parse("triage:intruder").unwrap();
        let document = session_with(&["owned turn"]);
        let session_id = document.id().clone();
        let snapshot = SessionSnapshot {
            data: serde_json::to_vec(&document).unwrap(),
        };

        let store = LocalContinuityStore::open(&path).expect("open");
        seed_record(&store, &owner, &session_id, 1).await;
        store
            .save_session_snapshot(
                &owner,
                &session_id,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(1),
                FencingToken::new(1),
                &snapshot,
            )
            .await
            .expect("the owner's whole-document save");

        // The intruder's own continuity record points at the same session id
        // and is current, so the cursor guard passes for it.
        seed_record(&store, &intruder, &session_id, 2).await;

        let blob_refusal = store
            .save_session_snapshot(
                &intruder,
                &session_id,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(1),
                FencingToken::new(2),
                &snapshot,
            )
            .await
            .expect_err("the whole-document verb refuses a foreign snapshot owner");
        assert!(
            matches!(blob_refusal, ContinuityStoreError::Corruption(_)),
            "unexpected whole-document refusal: {blob_refusal}"
        );

        let delta_refusal = store
            .append_messages(
                &cursor(&intruder, 0, 1, 2),
                &session_id,
                &TranscriptStrandId::root(),
                0,
                document.messages(),
            )
            .await
            .expect_err("the delta channel must refuse exactly what the blob path refuses");
        assert!(
            delta_refusal.to_string().contains("is owned by")
                && delta_refusal.to_string().contains("triage:owner"),
            "the delta refusal must be the same ownership corruption: {delta_refusal}"
        );

        // Refused means refused: no rows, and no earned lockout either.
        drop(store);
        assert!(
            !head_tables_exist(&path),
            "a refused delta write must leave no head-canonical rows behind"
        );
        assert_eq!(ledger_version(&path), Some(1));
    }

    /// A v2 file reopens cleanly (never re-applied, never refused) and keeps
    /// serving head-canonical sessions.
    #[tokio::test]
    async fn head_canonical_file_reopens_and_keeps_serving_head_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let document = session_with(&["one", "two"]);
        let session_id = document.id().clone();
        {
            let store = LocalContinuityStore::open(&path).expect("open");
            seed_record(&store, &identity, &session_id, 1).await;
            let root = TranscriptStrandId::root();
            store
                .append_messages(
                    &cursor(&identity, 0, 1, 1),
                    &session_id,
                    &root,
                    0,
                    document.messages(),
                )
                .await
                .expect("append");
            let head = SessionHead::from_session(&document, root, 0).expect("head");
            store
                .save_head(&cursor(&identity, 0, 2, 1), &head, SessionHeadCas::Create)
                .await
                .expect("save head");
        }
        assert_eq!(ledger_version(&path), Some(HEAD_CANONICAL_SCHEMA_VERSION));

        let store = LocalContinuityStore::open(&path).expect("reopen a head-canonical file");
        assert_eq!(ledger_version(&path), Some(HEAD_CANONICAL_SCHEMA_VERSION));
        let head = store
            .load_canonical_head(&session_id)
            .await
            .expect("load canonical head")
            .expect("head row survives reopen");
        assert_eq!(head.message_count, 2);
        let snapshot = store
            .load_session_snapshot(&session_id)
            .await
            .expect("snapshot")
            .expect("head-canonical sessions serve a synthesized snapshot");
        let loaded: Session = serde_json::from_slice(&snapshot.data).expect("decode");
        assert_eq!(loaded.messages(), document.messages());
    }

    /// A document compacted `cycles` times the way a long-lived member is:
    /// two turns, a compaction of everything but the newest message, the
    /// default transcript retention, then a live tail.
    fn compacted_session(cycles: usize) -> Session {
        let user =
            |text: String| meerkat_core::Message::User(meerkat_core::UserMessage::text(text));
        let mut document = session_with(&["seed"]);
        for cycle in 0..cycles {
            for turn in 0..2 {
                document.push(user(format!("cycle {cycle} turn {turn}")));
            }
            let end = document.messages().len() - 1;
            document
                .commit_transcript_rewrite(
                    meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end },
                    vec![user(format!("summary {cycle}"))],
                    meerkat_core::TranscriptRewriteReason::new("compaction"),
                    Some("unit-test".to_string()),
                    None,
                )
                .expect("commit the compaction rewrite");
            document
                .retire_transcript_history(meerkat_core::TranscriptHistoryRetention::default())
                .expect("retire past the default retention");
            document.push(user(format!("cycle {cycle} live tail")));
        }
        document
    }

    /// A graph that retired nothing lays out for reads exactly as the
    /// head-canonical conversion lays it out.
    #[test]
    fn whole_graph_blob_reads_through_the_conversion_layout() {
        let document = compacted_session(4);
        let history = document
            .validated_transcript_history_state()
            .expect("history decodes")
            .expect("compacted history");
        assert_eq!(
            history.state().retired_count(),
            0,
            "four rewrites retire none"
        );
        let (read_layout, read_head) =
            retained_layout_for_blob_session(&document).expect("read layout");
        let (write_layout, write_head) = layout_for_blob_session(&document).expect("write layout");
        assert_eq!(
            serde_json::to_value(&read_head).unwrap(),
            serde_json::to_value(&write_head).unwrap()
        );
        assert_eq!(read_layout.strands, write_layout.strands);
        assert_eq!(read_layout.head_strand, write_layout.head_strand);
        assert_eq!(read_layout.rewrites.len(), 4);
        assert_eq!(write_layout.rewrites.len(), 4);
    }

    /// A member compacted more often than the transcript retention keeps (4
    /// rewrites by default) has a re-anchored graph in its blob: the oldest
    /// rewrite occurrences are retired and their bodies are gone. Reads lay
    /// the retained part out as Meerkat's own SQLite session store does for
    /// the same blob: the same transcript head, head rows and retained
    /// rewrites. The head-canonical conversion still refuses the blob typed,
    /// because the continuity schema keeps every rewrite row and has none for
    /// a retired base, and it writes nothing.
    #[tokio::test]
    async fn re_anchored_blob_reads_as_meerkats_store_lays_it_out_and_does_not_convert() {
        let document = compacted_session(7);
        let history = document
            .validated_transcript_history_state()
            .expect("history decodes")
            .expect("compacted history");
        assert_eq!(
            (
                history.state().commit_count(),
                history.state().retired_count()
            ),
            (7, 3),
            "seven compactions under the default retention retire three"
        );
        match layout_for_blob_session(&document) {
            Ok(_) => panic!("the conversion layout laid out a re-anchored graph"),
            Err(error) => assert!(
                error.to_string().contains("needs the whole graph"),
                "{error}"
            ),
        }
        let session_id = document.id().clone();

        // Meerkat's own store, holding the same document as a whole blob. A
        // plain first save may not seed rewrite history, so the blob enters
        // through the offline create path a storage migration uses.
        let core_dir = tempfile::tempdir().expect("tempdir");
        let core: Arc<dyn meerkat::SessionStore> = Arc::new(
            meerkat_store::SqliteSessionStore::open(core_dir.path().join("sessions.sqlite3"))
                .expect("open Meerkat's session store"),
        );
        core.save_authoritative_projection_if_current_revision(&document, None)
            .await
            .expect("whole-blob seed");
        let core_rows = Arc::clone(&core)
            .as_incremental()
            .expect("Meerkat's SQLite store has an incremental channel");
        let core_head = core_rows
            .load_head(&session_id)
            .await
            .expect("Meerkat lays the re-anchored blob out")
            .expect("synthesized head");

        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:main").unwrap();
        seed_record(&store, &identity, &session_id, 1).await;
        store
            .save_session_snapshot(
                &identity,
                &session_id,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(1),
                FencingToken::new(1),
                &SessionSnapshot {
                    data: serde_json::to_vec(&document).unwrap(),
                },
            )
            .await
            .unwrap();
        let head = store
            .load_head(&session_id)
            .await
            .expect("reads lay the retained graph out")
            .expect("synthesized head");

        // The continuity store derives its row prefix from full per-strand
        // rows; every other transcript fact of the head is Meerkat's.
        let transcript_facts = |head: &SessionHead| {
            let value = serde_json::to_value(head).unwrap();
            serde_json::json!({
                "strand": value["strand"],
                "head_revision": value["head_revision"],
                "message_count": value["message_count"],
                "rewrite_count": value["rewrite_count"],
                "rewrite_prefix": value["rewrite_prefix"],
                "graph_prefix": value["graph_prefix"],
            })
        };
        assert_eq!(transcript_facts(&head), transcript_facts(&core_head));
        assert_eq!(
            head.rewrite_count, 7,
            "the head counts the retired generations"
        );
        assert_eq!(
            store
                .load_messages(&session_id, &head.strand, 0..head.message_count)
                .await
                .expect("head rows"),
            document.messages()
        );
        let retained: Vec<TranscriptRewriteCommit> = store
            .load_rewrites(&session_id)
            .await
            .expect("retained rewrites")
            .into_iter()
            .map(|record| record.commit)
            .collect();
        assert_eq!(retained.len(), 4, "only the retained occurrences have rows");
        assert_eq!(
            serde_json::to_value(&retained).unwrap(),
            serde_json::to_value(
                core_rows
                    .load_rewrite_commits(&session_id)
                    .await
                    .expect("Meerkat's retained commits")
            )
            .unwrap()
        );

        // The conversion has no row for the retired base: the first delta
        // write refuses typed and leaves no head row.
        let refused = store
            .append_messages(
                &cursor(&identity, 0, 2, 1),
                &session_id,
                &head.strand,
                head.message_count,
                &[meerkat_core::Message::User(
                    meerkat_core::UserMessage::text("after the cut".to_string()),
                )],
            )
            .await
            .expect_err("a re-anchored blob does not convert");
        assert!(
            refused.to_string().contains("needs the whole graph"),
            "{refused}"
        );
        assert!(
            store
                .load_canonical_head(&session_id)
                .await
                .unwrap()
                .is_none(),
            "the refused conversion writes no head row"
        );
    }

    /// Lazy per-session migration: the first delta write on a blob-only
    /// session converts it in the caller's transaction, the document loads
    /// back identically, and the blob row survives untouched as a frozen
    /// archive.
    #[tokio::test]
    async fn first_delta_write_migrates_the_blob_and_leaves_it_as_a_frozen_archive() {
        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let document = session_with(&["one", "two"]);
        let session_id = document.id().clone();
        let blob = serde_json::to_vec(&document).unwrap();
        seed_record(&store, &identity, &session_id, 1).await;
        store
            .save_session_snapshot(
                &identity,
                &session_id,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(1),
                FencingToken::new(1),
                &SessionSnapshot { data: blob.clone() },
            )
            .await
            .unwrap();
        let before = store
            .load_session_snapshot(&session_id)
            .await
            .unwrap()
            .unwrap();

        // Read-only head synthesis must NOT migrate.
        let synthesized = store.load_head(&session_id).await.unwrap().unwrap();
        assert!(
            store
                .load_canonical_head(&session_id)
                .await
                .unwrap()
                .is_none(),
            "reads never migrate a blob-only session"
        );

        let mut extended = document.clone();
        extended.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("three".to_string()),
        ));
        store
            .append_messages(
                &cursor(&identity, 0, 2, 1),
                &session_id,
                &synthesized.strand,
                synthesized.message_count,
                &extended.messages()[2..],
            )
            .await
            .expect("first delta write migrates");
        let migrated = store
            .load_canonical_head(&session_id)
            .await
            .unwrap()
            .expect("head row exists after the first delta write");
        assert_eq!(migrated.head_revision, synthesized.head_revision);
        assert_eq!(
            session_head_cas_token(&migrated).unwrap(),
            session_head_cas_token(&synthesized).unwrap(),
            "the deterministic layout makes the pre-migration token match the persisted one"
        );

        // The head still covers the pre-append prefix: the document is
        // unchanged until a head write adopts the appended rows.
        let after = store
            .load_session_snapshot(&session_id)
            .await
            .unwrap()
            .unwrap();
        let after_doc: Session = serde_json::from_slice(&after.data).unwrap();
        let before_doc: Session = serde_json::from_slice(&before.data).unwrap();
        assert_eq!(
            after_doc.messages(),
            before_doc.messages(),
            "unadopted tail rows are invisible to loads (the crash-window contract)"
        );

        // The archived blob row is byte-identical and never read again.
        let archived = store
            .run_blocking("read-archive", {
                let session_id = session_id.clone();
                move |inner| {
                    inner.with_reader(|connection| {
                        connection
                            .query_row(
                                "SELECT data FROM session_snapshots WHERE session_id = ?1",
                                rusqlite::params![session_id.to_string()],
                                |row| row.get::<_, Vec<u8>>(0),
                            )
                            .map_err(|e| sqlite_err("read archive", e))
                    })
                }
            })
            .await
            .unwrap();
        assert_eq!(archived, blob, "the archived blob must stay byte-identical");
    }

    /// A whole-document save on a head-canonical session converts into delta
    /// rows + a head and must NOT rewrite the frozen archive (the
    /// two-write-authorities tripwire).
    #[tokio::test]
    async fn whole_document_save_on_a_head_canonical_session_leaves_the_archive_untouched() {
        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let document = session_with(&["one"]);
        let session_id = document.id().clone();
        let blob = serde_json::to_vec(&document).unwrap();
        seed_record(&store, &identity, &session_id, 1).await;
        store
            .save_session_snapshot(
                &identity,
                &session_id,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(1),
                FencingToken::new(1),
                &SessionSnapshot { data: blob.clone() },
            )
            .await
            .unwrap();
        // Migrate + adopt through the delta channel. The service's own flow:
        // `load_head` synthesizes deterministically from the blob, so the
        // token it derives is the one the migrating write persists and the
        // `IfToken` CAS matches.
        let head = store.load_head(&session_id).await.unwrap().unwrap();
        store
            .save_head(
                &cursor(&identity, 0, 2, 1),
                &head,
                SessionHeadCas::IfToken(session_head_cas_token(&head).unwrap()),
            )
            .await
            .expect("the pre-migration token matches the migrating write");
        store
            .save_head(&cursor(&identity, 0, 3, 1), &head, SessionHeadCas::Create)
            .await
            .expect_err("Create must conflict once the head row exists");

        let mut extended = document.clone();
        extended.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("two".to_string()),
        ));
        store
            .save_session_snapshot(
                &identity,
                &session_id,
                ContinuityGeneration::new(0),
                CheckpointVersion::new(4),
                FencingToken::new(1),
                &SessionSnapshot {
                    data: serde_json::to_vec(&extended).unwrap(),
                },
            )
            .await
            .expect("whole-document save converts on a head-canonical session");

        let served = store
            .load_session_snapshot(&session_id)
            .await
            .unwrap()
            .unwrap();
        let served_doc: Session = serde_json::from_slice(&served.data).unwrap();
        assert_eq!(served_doc.messages(), extended.messages());
        let archived = store
            .run_blocking("read-archive", {
                let session_id = session_id.clone();
                move |inner| {
                    inner.with_reader(|connection| {
                        connection
                            .query_row(
                                "SELECT data FROM session_snapshots WHERE session_id = ?1",
                                rusqlite::params![session_id.to_string()],
                                |row| row.get::<_, Vec<u8>>(0),
                            )
                            .map_err(|e| sqlite_err("read archive", e))
                    })
                }
            })
            .await
            .unwrap();
        assert_eq!(
            archived, blob,
            "a head-canonical write must never touch the frozen blob archive"
        );
    }

    /// Per-mutation continuity discipline: the delta verbs apply exactly the
    /// fence / version / binding CAS the whole-blob verb applies, and a
    /// refused mutation commits nothing.
    #[tokio::test]
    async fn delta_writes_enforce_fence_and_version_cas_per_mutation() {
        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let document = session_with(&["one"]);
        let session_id = document.id().clone();
        let root = TranscriptStrandId::root();
        store
            .upsert_continuity_record(&record(&identity, &session_id), FencingToken::new(5))
            .await
            .unwrap();

        let stale_fence = store
            .append_messages(
                &cursor(&identity, 0, 1, 2),
                &session_id,
                &root,
                0,
                document.messages(),
            )
            .await
            .expect_err("a stale fencing token must be refused per append");
        assert!(
            stale_fence.to_string().contains("stale fencing token"),
            "unexpected error: {stale_fence}"
        );
        assert!(
            store
                .load_canonical_head(&session_id)
                .await
                .unwrap()
                .is_none(),
            "a refused delta write commits nothing"
        );

        store
            .append_messages(
                &cursor(&identity, 0, 1, 5),
                &session_id,
                &root,
                0,
                document.messages(),
            )
            .await
            .expect("a current fence is admitted");
        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .unwrap();
        let ContinuityResolveState::Ready { record: advanced } = &resolved[&identity] else {
            panic!("record must stay ready");
        };
        assert_eq!(
            advanced.checkpoint_version,
            CheckpointVersion::new(1),
            "the durable cursor advances atomically with the rows"
        );

        let stale_version = store
            .append_messages(
                &cursor(&identity, 0, 1, 5),
                &session_id,
                &root,
                1,
                document.messages(),
            )
            .await
            .expect_err("a non-advancing checkpoint version must be refused per append");
        assert!(
            stale_version
                .to_string()
                .contains("stale checkpoint version"),
            "unexpected error: {stale_version}"
        );

        let foreign = AgentIdentity::parse("triage:other").unwrap();
        let other_session = meerkat_core::types::SessionId::new();
        store
            .upsert_continuity_record(&record(&foreign, &other_session), FencingToken::new(6))
            .await
            .unwrap();
        let cross = store
            .append_messages(
                &cursor(&foreign, 0, 9, 6),
                &session_id,
                &root,
                1,
                document.messages(),
            )
            .await
            .expect_err("a foreign identity must not write another session's rows");
        assert!(
            cross.to_string().contains("not found") || cross.to_string().contains("owned by"),
            "unexpected error: {cross}"
        );
    }

    /// CAS delete over a head-canonical session: the token derives from the
    /// slim materialization and the delete scrubs head + strands + rewrites
    /// + the archive in one transaction.
    #[tokio::test]
    async fn cas_delete_over_head_canonical_rows_removes_every_table() {
        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let document = session_with(&["one", "two"]);
        let session_id = document.id().clone();
        let root = TranscriptStrandId::root();
        seed_record(&store, &identity, &session_id, 1).await;
        store
            .append_messages(
                &cursor(&identity, 0, 1, 1),
                &session_id,
                &root,
                0,
                document.messages(),
            )
            .await
            .unwrap();
        let head = SessionHead::from_session(&document, root, 0).unwrap();
        store
            .save_head(&cursor(&identity, 0, 2, 1), &head, SessionHeadCas::Create)
            .await
            .unwrap();

        let snapshot = store
            .load_session_snapshot(&session_id)
            .await
            .unwrap()
            .unwrap();
        let served: Session = serde_json::from_slice(&snapshot.data).unwrap();
        let token = meerkat_core::session_store::session_projection_cas_token(&served).unwrap();
        assert!(
            !store
                .delete_session_snapshot_if_current_revision(&session_id, "row-sha256:stale")
                .await
                .unwrap(),
            "a stale token must decline"
        );
        assert!(
            store
                .delete_session_snapshot_if_current_revision(&session_id, &token)
                .await
                .unwrap(),
            "the token derived from head+rows must be accepted"
        );
        assert!(
            store
                .load_session_snapshot(&session_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .load_canonical_head(&session_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// Reset rollback keeps the PRIOR generation's head+rows as the rollback
    /// authority and deletes only the attempted generation's.
    #[tokio::test]
    async fn rollback_scopes_head_canonical_rows_to_the_attempted_generation() {
        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let previous_doc = session_with(&["previous"]);
        let previous_session = previous_doc.id().clone();
        let root = TranscriptStrandId::root();
        let mut previous = record(&identity, &previous_session);
        store
            .upsert_continuity_record(&previous, FencingToken::new(1))
            .await
            .unwrap();
        store
            .append_messages(
                &cursor(&identity, 0, 1, 1),
                &previous_session,
                &root,
                0,
                previous_doc.messages(),
            )
            .await
            .unwrap();
        let previous_head = SessionHead::from_session(&previous_doc, root.clone(), 0).unwrap();
        store
            .save_head(
                &cursor(&identity, 0, 2, 1),
                &previous_head,
                SessionHeadCas::Create,
            )
            .await
            .unwrap();
        previous.checkpoint_version = CheckpointVersion::new(2);

        let attempted_doc = session_with(&["attempted"]);
        let attempted_session = attempted_doc.id().clone();
        let mut attempted = record(&identity, &attempted_session);
        attempted.agent_runtime_id = AgentRuntimeId::parse("rt:triage:main:1").unwrap();
        attempted.generation = ContinuityGeneration::new(1);
        store
            .upsert_continuity_record(&attempted, FencingToken::new(2))
            .await
            .unwrap();
        store
            .append_messages(
                &cursor(&identity, 1, 1, 2),
                &attempted_session,
                &root,
                0,
                attempted_doc.messages(),
            )
            .await
            .unwrap();
        let attempted_head = SessionHead::from_session(&attempted_doc, root, 0).unwrap();
        store
            .save_head(
                &cursor(&identity, 1, 2, 2),
                &attempted_head,
                SessionHeadCas::Create,
            )
            .await
            .unwrap();

        store
            .rollback_continuity_record(&attempted, Some(&previous), FencingToken::new(2))
            .await
            .expect("rollback");

        assert!(
            store
                .load_canonical_head(&attempted_session)
                .await
                .unwrap()
                .is_none(),
            "the attempted generation's head+rows are abandoned"
        );
        let restored = store
            .load_canonical_head(&previous_session)
            .await
            .unwrap()
            .expect("the prior generation stays the rollback authority");
        assert_eq!(restored.head_revision, previous_head.head_revision);
        let served = store
            .load_session_snapshot(&previous_session)
            .await
            .unwrap()
            .expect("the restored session still loads");
        let doc: Session = serde_json::from_slice(&served.data).unwrap();
        assert_eq!(doc.messages(), previous_doc.messages());
    }

    /// Identity deletion scrubs all four tables atomically, and the fencing
    /// floor spans the head table so the lease provider never regresses.
    #[tokio::test]
    async fn identity_delete_scrubs_head_rows_and_the_floor_spans_the_head_table() {
        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let document = session_with(&["one"]);
        let session_id = document.id().clone();
        let root = TranscriptStrandId::root();
        seed_record(&store, &identity, &session_id, 1).await;
        store
            .append_messages(
                &cursor(&identity, 0, 1, 9),
                &session_id,
                &root,
                0,
                document.messages(),
            )
            .await
            .unwrap();
        let head = SessionHead::from_session(&document, root, 0).unwrap();
        store
            .save_head(&cursor(&identity, 0, 2, 9), &head, SessionHeadCas::Create)
            .await
            .unwrap();
        assert_eq!(
            store.max_fencing_token().unwrap(),
            9,
            "the head table participates in the fencing floor"
        );

        store
            .delete_continuity_record(&identity, FencingToken::new(10))
            .await
            .unwrap();
        assert!(
            store
                .load_canonical_head(&session_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .load_session_snapshot(&session_id)
                .await
                .unwrap()
                .is_none()
        );
        let remaining_rows = store
            .run_blocking("count-strands", move |inner| {
                inner.with_reader(|connection| {
                    connection
                        .query_row(
                            "SELECT COUNT(*) FROM continuity_strand_messages",
                            [],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(|e| sqlite_err("count strands", e))
                })
            })
            .await
            .unwrap();
        assert_eq!(
            remaining_rows, 0,
            "strand rows are scrubbed with the identity"
        );
    }

    /// The exact-bytes no-op probe is a blob-authority concept: on a
    /// head-canonical session it declines so the caller takes its ordinary
    /// guard path.
    #[tokio::test]
    async fn exact_snapshot_probe_declines_for_head_canonical_sessions() {
        let store = LocalContinuityStore::in_memory().unwrap();
        let identity = AgentIdentity::parse("triage:main").unwrap();
        let document = session_with(&["one"]);
        let session_id = document.id().clone();
        let root = TranscriptStrandId::root();
        seed_record(&store, &identity, &session_id, 1).await;
        store
            .append_messages(
                &cursor(&identity, 0, 1, 1),
                &session_id,
                &root,
                0,
                document.messages(),
            )
            .await
            .unwrap();
        let head = SessionHead::from_session(&document, root, 0).unwrap();
        store
            .save_head(&cursor(&identity, 0, 2, 1), &head, SessionHeadCas::Create)
            .await
            .unwrap();

        let snapshot = store
            .load_session_snapshot(&session_id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !store
                .session_snapshot_matches_current(SessionSnapshotMatchCandidate {
                    identity,
                    session_id,
                    generation: ContinuityGeneration::new(0),
                    checkpoint_version: CheckpointVersion::new(2),
                    fencing_token: FencingToken::new(1),
                    snapshot: Arc::new(snapshot),
                })
                .await
                .unwrap(),
            "head-canonical sessions must decline the whole-blob byte probe"
        );
    }

    // ----------------------------------------------------------------
    // Previous-release model: what a v1-shaped binary sees in the file.
    // ----------------------------------------------------------------

    /// The v0.8.5 continuity schema domain, declared LOCALLY on purpose.
    ///
    /// Not `META_CONTINUITY_BASELINE_DOMAIN`: that constant is this
    /// binary's internal staging device and is free to grow. What these
    /// tests must model is the previous RELEASE's version ceiling, which is
    /// frozen at 1 forever. Declaring it here means a future migration
    /// cannot quietly raise the bar the "old binary" is held to and turn
    /// these tests green for the wrong reason.
    const V0_8_5_CONTINUITY_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
        name: "meta-continuity",
        migrations: &[meerkat_sqlite::Migration {
            version: 1,
            name: "base-schema",
            apply: migration_0001_continuity_schema,
        }],
        initialize_current: migration_0001_continuity_schema,
        allowed_existing_versions: &[1],
        // Unledgered meta files are refused at open (below the 0.8.8 ledger
        // floor) and meta never runs the offline bridge, so no source
        // version is inferable.
        bridge_recoverable_versions: &[],
        released_predecessors: &[],
        owned_objects: RELEASED_V1_CONTINUITY_OBJECTS,
        retired_objects: &[],
    };

    /// Local model of the retired `meerkat_sqlite::refuse_future_schema`
    /// check the previous release ran at open: read the ledger row, refuse
    /// when it exceeds the modeled version ceiling.
    fn refuse_future_schema_model(
        conn: &Connection,
        domain: &meerkat_sqlite::SchemaDomain,
    ) -> Result<(), meerkat_sqlite::SqliteStoreError> {
        let supported = domain.supported_version();
        match meerkat_sqlite::domain_version(conn, domain.name)? {
            Some(found) if found > supported => {
                Err(meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture {
                    domain: domain.name.to_string(),
                    found,
                    supported,
                })
            }
            _ => Ok(()),
        }
    }

    /// A stand-in for a release that predates the head-canonical channel:
    /// it supports `meta-continuity` up to v1 and reads sessions ONLY from
    /// `session_snapshots.data`.
    ///
    /// `refuse_future_schema` against that ceiling is exactly the check such
    /// a binary runs at open (the head-canonical migration simply does not
    /// exist in it), so this reproduces the real `SchemaFromTheFuture`
    /// lockout without needing the old binary on disk.
    fn v1_shaped_binary_opens(path: &Path) -> Result<(), meerkat_sqlite::SqliteStoreError> {
        let conn = Connection::open(path).expect("probe");
        refuse_future_schema_model(&conn, &V0_8_5_CONTINUITY_DOMAIN)
    }

    /// N1-INTERACTION PIN: keeping the file at v1 for an append that adopts
    /// nothing is what makes rollback possible — and it is also what lets
    /// ORPHAN strand rows outlive a rollback.
    ///
    /// Sequence: an append lands, the adopting head write never does (crash,
    /// or the operator rolls back mid-creation-window). The previous release
    /// can now open the file — that is the whole point — and it writes
    /// whole-document blobs, including ones that DIVERGE from the orphan
    /// rows (a compaction, a rewind). Re-upgrading then migrates that blob
    /// into head+rows over the orphans.
    ///
    /// Before this was handled, `insert_strand_rows_in_txn` refused the
    /// divergence as an immutability violation and the session became
    /// permanently unwritable. The migration clears orphans first: they are
    /// unreachable by construction (every read path gates on the head row),
    /// and the blob is the authority being migrated.
    #[tokio::test]
    async fn re_upgrading_over_orphan_rows_that_diverge_from_the_blob_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let identity = AgentIdentity::parse("triage:orphans").unwrap();
        let root = TranscriptStrandId::root();
        let interrupted = session_with(&["draft turn that was never adopted"]);
        let session_id = interrupted.id().clone();

        // 1. An append with no adopting head: rows land, ledger stays v1.
        {
            let store = LocalContinuityStore::open(&path).expect("open");
            seed_record(&store, &identity, &session_id, 1).await;
            store
                .append_messages(
                    &cursor(&identity, 0, 1, 1),
                    &session_id,
                    &root,
                    0,
                    interrupted.messages(),
                )
                .await
                .expect("append");
        }
        assert_eq!(ledger_version(&path), Some(1));
        v1_shaped_binary_opens(&path).expect("rollback is possible — that is the point");

        // 2. The previous release writes a whole-document blob that diverges
        //    from the orphan rows at seq 0.
        let divergent = {
            let store = LocalContinuityStore::open(&path).expect("reopen");
            // The same session id carrying a transcript that differs from
            // the orphan rows at seq 0 — a compaction, say.
            let rebuilt = rebuild_with_messages(
                &interrupted,
                vec![meerkat_core::Message::User(
                    meerkat_core::UserMessage::text("post-rollback compaction".to_string()),
                )],
            );
            store
                .save_session_snapshot(
                    &identity,
                    &session_id,
                    ContinuityGeneration::new(0),
                    CheckpointVersion::new(2),
                    FencingToken::new(1),
                    &SessionSnapshot {
                        data: serde_json::to_vec(&rebuilt).unwrap(),
                    },
                )
                .await
                .expect("post-rollback whole-blob save");
            rebuilt
        };

        // 3. Re-upgrade: the first delta write migrates the divergent blob.
        {
            let store = LocalContinuityStore::open(&path).expect("reopen");
            let mut next = divergent.clone();
            next.push(meerkat_core::Message::User(
                meerkat_core::UserMessage::text("turn after re-upgrade".to_string()),
            ));
            let base = divergent.messages().len() as u64;
            store
                .append_messages(
                    &cursor(&identity, 0, 3, 1),
                    &session_id,
                    &root,
                    base,
                    &next.messages()[base as usize..],
                )
                .await
                .expect(
                    "the migrating append must clear orphan rows the blob diverges from, \
                     not refuse the session forever",
                );
            let migrated = store
                .load_canonical_head(&session_id)
                .await
                .expect("head")
                .expect("the blob migrated into head+rows");
            assert_eq!(
                migrated.message_count,
                divergent.messages().len() as u64,
                "the migrated head describes the BLOB, which is the authority"
            );
            let rows = store
                .load_messages(&session_id, &migrated.strand, 0..migrated.message_count)
                .await
                .expect("rows");
            assert_eq!(
                rows,
                divergent.messages(),
                "the orphan rows must be gone, replaced by the blob's transcript"
            );
        }
    }

    /// Rebuild a session document on the SAME id with a different transcript.
    fn rebuild_with_messages(source: &Session, messages: Vec<meerkat_core::Message>) -> Session {
        let mut head =
            SessionHead::from_session(source, TranscriptStrandId::root(), 0).expect("head");
        head.message_count = messages.len() as u64;
        head.head_revision = meerkat_core::transcript_messages_digest(&messages).expect("digest");
        // meerkat 0.8.11: `SessionHead::into_session` verifies the byte-exact
        // row-prefix commitment against the rows it materializes (mirrors
        // meerkat-core `SessionHead::into_session_with_serialized_rows`), so
        // the fabricated head must commit to the REPLACEMENT rows. The
        // source-derived lineage anchor cannot describe them (its fields are
        // store-private), so it is cleared - `None` is the accepted
        // unactivated shape at materialization.
        let serialized = messages
            .iter()
            .map(|message| serde_json::to_vec(message).expect("serialize replacement row"))
            .collect::<Vec<_>>();
        head.message_row_prefix = Some(
            meerkat_core::session_store::SessionMessageRowPrefixAccumulator::empty()
                .extend_serialized_rows(&serialized)
                .expect("recommit row prefix"),
        );
        head.row_lineage_anchor = None;
        head.into_session(messages).expect("rebuild")
    }
}

/// One legacy blob session awaiting head-canonical conversion.
///
/// Every value the conversion needs already lives in the blob row, so the
/// offline driver needs no external state and no running runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingLegacySession {
    pub(crate) session_id: String,
    pub(crate) identity: String,
    pub(crate) generation: i64,
    pub(crate) checkpoint_version: i64,
    pub(crate) fencing_token: i64,
}

/// Outcome of an offline head-canonical backfill.
///
/// `converted.len() == examined` with empty `failures`/`vanished` is
/// the only shape that stamps the ledger; every other shape leaves the file
/// at v1 and rollback available.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HeadCanonicalBackfillReport {
    /// Legacy blob rows the run examined. EVERY blob row, converted or not:
    /// a row that already has a head row is still examined, because its blob
    /// may have changed since that conversion. Do not read this as "work to
    /// do" — `converted` + `reconverted` is the work that happened.
    pub examined: usize,
    /// Session ids that now carry a head row because of this run.
    pub converted: Vec<String>,
    /// Sessions whose blob disappeared between census and conversion.
    pub vanished: Vec<String>,
    /// Sessions that HAD been converted but whose legacy blob changed
    /// afterwards, so their head-canonical rows were replaced. Non-empty here
    /// means the deployment kept writing between a failed crossing and this
    /// retry — which is normal, and exactly the case that would otherwise
    /// have buried those messages.
    pub reconverted: Vec<String>,
    /// `(session_id, error)`; an empty session id is a run-level refusal.
    pub failures: Vec<(String, String)>,
    /// Blob rows whose session id or identity is not well formed. These
    /// could not be parsed as sessions and were therefore not converted.
    ///
    /// These BLOCK the ledger stamp until acknowledged by id. An earlier
    /// design skipped them silently on the reasoning that a row which was
    /// never a session can never be converted — true, but it makes a parser
    /// bug indistinguishable from genuine garbage at the one step that
    /// cannot be undone, so a real session classified too strictly would be
    /// dropped and the ledger advanced over it.
    pub skipped_unparseable: Vec<String>,
    /// True only when the whole corpus crossed in this run.
    pub ledger_stamped: bool,
    /// False for a dry run, which mutates nothing including the DDL.
    pub applied: bool,
}

impl HeadCanonicalBackfillReport {
    /// True when the corpus is wholly head-canonical after this run.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.failures.is_empty()
            && self.vanished.is_empty()
            && self.converted.len() == self.examined
    }
}

/// Sessions with a legacy blob row and no head row.
///
/// Returns every blob session when the head table does not exist yet, which
/// is the ordinary v1 shape — a dry run must be able to report the pending
/// count without applying the DDL first.
fn pending_head_canonical_sessions(
    conn: &Connection,
) -> Result<(Vec<PendingLegacySession>, Vec<String>), ContinuityStoreError> {
    let heads_exist: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='continuity_session_heads'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| sqlite_err("probe head-canonical table", e))?
        > 0;
    // EVERY blob row, not only those lacking a head.
    //
    // "has a head row" is not "converted and current". A partial crossing
    // leaves the ledger at v1, and AT v1 THE WHOLE-DOCUMENT PATH IS STILL THE
    // ACTIVE WRITER — so between a failed run and its retry the deployment
    // keeps appending to exactly the blobs already converted. Filtering those
    // out here would skip them on the retry, stamp, and bury every message
    // written in the gap inside a retained blob nothing reads afterwards.
    // Silent, and discovered only when an agent appears to have forgotten a
    // conversation. So the census returns them and the driver compares
    // CONTENT.
    let sql = if heads_exist {
        "SELECT s.session_id, s.identity, s.generation, s.checkpoint_version, s.fencing_token \
         FROM session_snapshots s ORDER BY s.session_id"
    } else {
        "SELECT session_id, identity, generation, checkpoint_version, fencing_token \
         FROM session_snapshots ORDER BY session_id"
    };
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| sqlite_err("prepare pending legacy session census", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(PendingLegacySession {
                session_id: row.get(0)?,
                identity: row.get(1)?,
                generation: row.get(2)?,
                checkpoint_version: row.get(3)?,
                fencing_token: row.get(4)?,
            })
        })
        .map_err(|e| sqlite_err("query pending legacy sessions", e))?;
    let mut pending = Vec::new();
    let mut unparseable = Vec::new();
    for row in rows {
        let row = row.map_err(|e| sqlite_err("read pending legacy session", e))?;
        // A row whose session id or identity is not well formed was never a
        // meta session and can never be converted by anything. Treating it
        // as a conversion FAILURE would block the ledger stamp permanently
        // and strand an otherwise-healthy corpus over one piece of garbage,
        // so it is surfaced separately and excluded from the pending set.
        let parseable = meerkat_core::types::SessionId::parse(&row.session_id).is_ok()
            && AgentIdentity::parse(&row.identity).is_ok();
        if parseable {
            pending.push(row);
        } else {
            unparseable.push(row.session_id);
        }
    }
    Ok((pending, unparseable))
}

/// Convert one legacy blob session in its own transaction.
///
/// `Ok(false)` means the blob was gone by the time the transaction opened —
/// reported rather than silently counted, so a complete-conversion claim
/// cannot be made about a corpus that changed under the fence.
/// Blob rows with no head row, for the final pre-stamp verification.
///
/// The census deliberately returns EVERY blob row so the driver can compare
/// content, so it can no longer double as "is anything unconverted?". This
/// asks that narrower question in SQL, with no decode: it catches a row that
/// appeared during the run, or a write that did not land, without trusting
/// the loop's own optimistic account of itself.
fn blob_rows_without_head(conn: &Connection) -> Result<Vec<String>, ContinuityStoreError> {
    let heads_exist: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='continuity_session_heads'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| sqlite_err("probe head-canonical table", e))?
        > 0;
    if !heads_exist {
        return Ok(vec!["<no head-canonical tables>".to_string()]);
    }
    let mut stmt = conn
        .prepare(
            "SELECT s.session_id FROM session_snapshots s \
             LEFT JOIN continuity_session_heads h ON h.session_id = s.session_id \
             WHERE h.session_id IS NULL ORDER BY s.session_id",
        )
        .map_err(|e| sqlite_err("prepare unconverted census", e))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| sqlite_err("query unconverted rows", e))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| sqlite_err("read unconverted row", e))?);
    }
    Ok(out)
}

fn backfill_one_session(
    conn: &mut Connection,
    candidate: &PendingLegacySession,
) -> Result<BackfillOutcome, ContinuityStoreError> {
    let session_id = meerkat_core::types::SessionId::parse(&candidate.session_id)
        .map_err(|e| ContinuityStoreError::Io(format!("malformed session id in blob row: {e}")))?;
    let identity = AgentIdentity::parse(&candidate.identity)
        .map_err(|e| ContinuityStoreError::Io(format!("malformed identity in blob row: {e}")))?;
    let tx = conn
        .transaction()
        .map_err(|e| sqlite_err("begin legacy session backfill", e))?;
    // SQLite hands these back as i64. A negative stamp is a corrupt row, not
    // a value to wrap around silently — refuse it and let the report name the
    // session rather than converting it against a fabricated stamp.
    let stamp = |label: &str, value: i64| -> Result<u64, ContinuityStoreError> {
        u64::try_from(value).map_err(|_| {
            ContinuityStoreError::Io(format!(
                "negative {label} ({value}) in blob row for session {}",
                candidate.session_id
            ))
        })
    };
    let generation = stamp("generation", candidate.generation)?;
    let checkpoint_version = stamp("checkpoint_version", candidate.checkpoint_version)?;
    let fencing_token = stamp("fencing_token", candidate.fencing_token)?;
    // STALENESS, BY CONTENT.
    //
    // A head row means this session was converted at SOME point, not that the
    // conversion still reflects the blob. Compare the blob's own head
    // revision against the stored one and reconvert on divergence.
    //
    // The comparison is a content digest, never a message count or a
    // timestamp: a message added and then removed between runs leaves a count
    // unchanged while the content differs, and a timestamp says nothing about
    // what the bytes hold.
    let existing = head_row_in_txn(&tx, &session_id)
        .map_err(|e| ContinuityStoreError::Io(format!("read existing head row: {e}")))?;
    let mut replaced = false;
    if let Some((stored, _token)) = existing.as_ref() {
        let session = blob_session_in_txn(&tx, &session_id)
            .map_err(|e| ContinuityStoreError::Io(format!("read blob for staleness check: {e}")))?;
        match session {
            Some(session) => {
                if session.id() != &session_id {
                    // The identity guard below refuses this, but do not touch
                    // the existing head on the way there.
                    return Err(ContinuityStoreError::Io(format!(
                        "blob for {session_id} decodes to a different session; refusing"
                    )));
                }
                let (_layout, expected) = layout_for_blob_session(&session).map_err(|e| {
                    ContinuityStoreError::Io(format!("recompute head for staleness check: {e}"))
                })?;
                if expected.head_revision == stored.head_revision {
                    // Converted and still current — nothing to do.
                    tx.commit()
                        .map_err(|e| sqlite_err("commit no-op backfill", e))?;
                    return Ok(BackfillOutcome::AlreadyCurrent);
                }
                tracing::info!(
                    session_id = %session_id,
                    stored_revision = %stored.head_revision,
                    blob_revision = %expected.head_revision,
                    "legacy blob changed since its conversion; replacing the head-canonical rows"
                );
                delete_head_canonical_rows_in_txn(
                    &tx,
                    "session_id = ?1",
                    &[&session_id.to_string()],
                )?;
                replaced = true;
            }
            None => {
                tx.commit()
                    .map_err(|e| sqlite_err("commit vanished backfill", e))?;
                return Ok(BackfillOutcome::Vanished);
            }
        }
    }
    let migrated = migrate_legacy_blob_in_txn(
        &tx,
        &session_id,
        &identity,
        ContinuityGeneration::new(generation),
        CheckpointVersion::new(checkpoint_version),
        FencingToken::new(fencing_token),
    )
    .map_err(|e| ContinuityStoreError::Io(format!("head-canonical conversion failed: {e}")))?;
    tx.commit()
        .map_err(|e| sqlite_err("commit legacy session backfill", e))?;
    Ok(match (migrated.is_some(), replaced) {
        (false, _) => BackfillOutcome::Vanished,
        (true, true) => BackfillOutcome::Reconverted,
        (true, false) => BackfillOutcome::Converted,
    })
}

/// What one row did, so the report can distinguish a first conversion from a
/// replacement of a conversion the deployment had already outgrown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackfillOutcome {
    Converted,
    /// The blob changed after an earlier conversion; head rows were replaced.
    Reconverted,
    /// Already converted and still current.
    AlreadyCurrent,
    /// The blob was gone by the time the transaction opened.
    Vanished,
}
