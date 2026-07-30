//! SQLite-backed session store.

use crate::error::into_session_store_error;
use crate::json_column::JsonColumnBytes;
use crate::{SessionFilter, SessionStore, SessionStoreError, StoreError};
use async_trait::async_trait;
use meerkat_core::session_store::{
    IncrementalSessionStore, PreparedHeadCanonicalMutation, PreparedHeadCanonicalParentSplice,
    PreparedHeadCanonicalParentTransition, PreparedHeadCanonicalRewriteMutation,
    SESSION_ROW_LINEAGE_REBASE_INTERVAL, SaveGuardWitness, SessionHead, SessionHeadCas,
    SessionMessageRowPrefixAccumulator, StrandLayout, StrandRewriteLayout, StrandSegment,
    StrandSplice, TranscriptStrandId, head_canonical_plain_save_guard_with_prefix_witness,
    reconstruct_rewrite_record, session_head_cas_token, strand_layout_for_history,
    validate_save_head_transition,
};
use meerkat_core::time_compat::SystemTime;
use meerkat_core::transcript_messages_digest;
use meerkat_core::types::Message;
use meerkat_core::{
    ComponentEventPrefixAuthority, PreparedComponentEventSuffix, SerializedComponentEvent, Session,
    SessionHeadMetadataCell, SessionHeadMetadataCellIdentity, SessionHeadMetadataIdentity,
    SessionHeadMetadataProjection, SessionHeadMetadataValueDigest, SessionId, SessionMeta,
    StoredComponentEventRow, TranscriptRevisionEdge, TranscriptRewriteCommit,
    TranscriptRewritePrefixAccumulator, TranscriptRewriteRecord, VerifiedComponentEventSequence,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use uuid::Uuid;

/// Per-store SQLite contention policy. The default tolerates the long WAL
/// writer holds produced by large durable snapshot commits while keeping the
/// wait bounded. Runtime/session stores may override it per instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqliteConnectionOptions {
    /// Maximum time SQLite's busy handler waits and retries a locked write.
    pub busy_timeout: Duration,
}

impl Default for SqliteConnectionOptions {
    fn default() -> Self {
        Self {
            busy_timeout: meerkat_sqlite::SHARED_BUSY_TIMEOUT,
        }
    }
}

const CREATE_SESSIONS_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    message_count INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL,
    metadata_json TEXT NOT NULL,
    session_json BLOB NOT NULL
)";

const CREATE_SESSIONS_UPDATED_INDEX_SQL: &str = r"
CREATE INDEX IF NOT EXISTS sessions_updated_idx
ON sessions(updated_at_ms DESC, session_id ASC)";

// Incremental session persistence (OB3 ask 11).
//
// Canonical-representation rule (per session): a `session_heads` row exists
// => the head representation is canonical and the blob row (if any) is a
// frozen migration archive, never read or written again; no head row => the
// legacy blob behavior stays byte-for-byte unchanged.
//
// Row-materialization rule (per session): a linked strand owns only its exact
// splice span plus any direct append tail beyond the link's immutable logical
// endpoint. Every other row resolves through the named successor. Current
// prepared rewrites therefore store the child delta over its parent; released
// 0.8.10 strands may retain the inverse parent-to-child supersession shape
// until their one-time verified activation. Both directions obey the same
// mechanical overlay invariant and avoid a fresh full transcript per rewrite
// (measured pre-fix: 98 rewrites of a 371-message transcript => 16,672 rows /
// 24 MB for one session).
const CREATE_SESSION_STRAND_MESSAGES_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS session_strand_messages (
    session_id TEXT NOT NULL,
    strand TEXT NOT NULL,
    seq INTEGER NOT NULL,
    message_json BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, strand, seq)
)";

// Strand overlay edges: `strand` owns exactly `[splice_start, splice_end)` and
// resolves its shared prefix/suffix through `successor`. A strand with no link
// owns every logical row directly.
const CREATE_SESSION_STRAND_LINKS_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS session_strand_links (
    session_id TEXT NOT NULL,
    strand TEXT NOT NULL,
    successor TEXT NOT NULL,
    strand_len INTEGER NOT NULL,
    splice_start INTEGER NOT NULL,
    splice_end INTEGER NOT NULL,
    successor_end INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, strand)
)";

const CREATE_SESSION_REWRITES_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS session_rewrites (
    session_id TEXT NOT NULL,
    rewrite_idx INTEGER NOT NULL,
    parent_strand TEXT NOT NULL,
    parent_len INTEGER NOT NULL,
    strand TEXT NOT NULL,
    strand_len INTEGER NOT NULL,
    commit_json BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, rewrite_idx)
)";

const CREATE_SESSION_HEADS_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS session_heads (
    session_id TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    strand TEXT NOT NULL,
    head_revision TEXT NOT NULL,
    message_count INTEGER NOT NULL,
    rewrite_count INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    metadata_json TEXT NOT NULL,
    head_json BLOB NOT NULL,
    cas_token TEXT NOT NULL
)";

const CREATE_SESSION_HEADS_UPDATED_INDEX_SQL: &str = r"
CREATE INDEX IF NOT EXISTS session_heads_updated_idx
ON session_heads(updated_at_ms DESC, session_id ASC)";

const ADD_SESSION_REWRITES_GRAPH_EDGE_COLUMN_SQL: &str =
    "ALTER TABLE session_rewrites ADD COLUMN graph_edge_json BLOB";

const CREATE_SESSION_COMPONENT_EVENTS_TABLE_SQL: &str = r"
CREATE TABLE session_component_events (
    session_id TEXT NOT NULL,
    component TEXT NOT NULL
        CHECK (component = 'realtime'),
    seq INTEGER NOT NULL CHECK (seq >= 0),
    event_json BLOB NOT NULL,
    event_digest TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, component, seq)
)";

// First published authenticated HeadCanonical sidecar shape (session schema
// migration v3). Released 0.8.10's v2 schema had neither the rewrite
// graph-edge column nor the component/metadata sidecar tables; no unreleased
// candidate shape is recognized or migrated here.
//
// Cells, states, deltas, and head lineage are immutable. `current` is the
// physical head's materialized key set; `refs` pins the exact physical and
// runtime metadata states. A ref's head token is the immutable token which
// created that state, not every later transcript-only head which reuses the
// same identity; that distinction is what makes an unchanged-metadata
// boundary a literal metadata-table no-write. The runtime validates every
// relationship inside the writing transaction. These tables deliberately
// carry no foreign keys: co-tenant migration and convergence pruning have
// different deletion orders, and neither may turn ordering into authority.
const CREATE_SESSION_HEAD_METADATA_CELLS_TABLE_SQL: &str = r"
CREATE TABLE session_head_metadata_cells (
    session_id TEXT NOT NULL,
    metadata_key TEXT NOT NULL,
    key_route BLOB NOT NULL CHECK (length(key_route) = 32),
    exact_value_digest TEXT NOT NULL,
    metadata_json BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, metadata_key, exact_value_digest)
)";

const CREATE_SESSION_HEAD_METADATA_CELLS_ROUTE_KEY_INDEX_SQL: &str = r"
CREATE INDEX session_head_metadata_cells_route_key_idx
ON session_head_metadata_cells(session_id, key_route, metadata_key)";

const CREATE_SESSION_HEAD_METADATA_CURRENT_TABLE_SQL: &str = r"
CREATE TABLE session_head_metadata_current (
    session_id TEXT NOT NULL,
    metadata_key TEXT NOT NULL,
    key_route BLOB NOT NULL CHECK (length(key_route) = 32),
    exact_value_digest TEXT NOT NULL,
    PRIMARY KEY (session_id, metadata_key)
)";

const CREATE_SESSION_HEAD_METADATA_STATES_TABLE_SQL: &str = r"
CREATE TABLE session_head_metadata_states (
    session_id TEXT NOT NULL,
    state_id TEXT NOT NULL,
    predecessor_state_id TEXT,
    identity_json BLOB NOT NULL,
    transition_id TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, state_id),
    UNIQUE (session_id, transition_id)
)";

const CREATE_SESSION_HEAD_METADATA_STATES_PREDECESSOR_INDEX_SQL: &str = r"
CREATE INDEX session_head_metadata_states_predecessor_idx
ON session_head_metadata_states(session_id, predecessor_state_id)";

const CREATE_SESSION_HEAD_METADATA_STATE_DELTAS_TABLE_SQL: &str = r"
CREATE TABLE session_head_metadata_state_deltas (
    session_id TEXT NOT NULL,
    state_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    metadata_key TEXT NOT NULL,
    key_route BLOB NOT NULL CHECK (length(key_route) = 32),
    predecessor_exact_value_digest TEXT,
    successor_exact_value_digest TEXT,
    PRIMARY KEY (session_id, state_id, ordinal),
    UNIQUE (session_id, state_id, metadata_key)
)";

const CREATE_SESSION_HEAD_METADATA_STATE_DELTAS_KEY_INDEX_SQL: &str = r"
CREATE INDEX session_head_metadata_state_deltas_key_idx
ON session_head_metadata_state_deltas(session_id, metadata_key, state_id)";

const CREATE_SESSION_HEAD_METADATA_REFS_TABLE_SQL: &str = r"
CREATE TABLE session_head_metadata_refs (
    session_id TEXT NOT NULL,
    owner TEXT NOT NULL
        CHECK (owner IN ('physical_head', 'runtime_boundary')),
    head_cas_token TEXT NOT NULL,
    state_id TEXT NOT NULL,
    PRIMARY KEY (session_id, owner)
)";

const CREATE_SESSION_HEAD_METADATA_HEAD_LINEAGE_TABLE_SQL: &str = r"
CREATE TABLE session_head_metadata_head_lineage (
    session_id TEXT NOT NULL,
    transition_id TEXT NOT NULL,
    predecessor_head_cas_token TEXT,
    successor_head_cas_token TEXT NOT NULL,
    predecessor_state_id TEXT,
    successor_state_id TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, transition_id),
    UNIQUE (session_id, successor_head_cas_token)
)";

const CREATE_SESSION_HEAD_METADATA_HEAD_LINEAGE_PREDECESSOR_INDEX_SQL: &str = r"
CREATE INDEX session_head_metadata_head_lineage_predecessor_idx
ON session_head_metadata_head_lineage(session_id, predecessor_head_cas_token)";

const CREATE_SESSION_HEAD_METADATA_HEAD_LINEAGE_SUCCESSOR_INDEX_SQL: &str = r"
CREATE INDEX session_head_metadata_head_lineage_successor_idx
ON session_head_metadata_head_lineage(session_id, successor_head_cas_token)";

fn system_time_millis(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

fn millis_to_system_time(value: i64) -> SystemTime {
    let millis = u64::try_from(value).unwrap_or_default();
    UNIX_EPOCH + Duration::from_millis(millis)
}

fn parse_session_id(raw: String) -> Result<SessionId, StoreError> {
    let uuid = Uuid::parse_str(&raw)
        .map_err(|err| StoreError::Internal(format!("invalid session_id '{raw}': {err}")))?;
    Ok(SessionId(uuid))
}

/// Open a session-store connection under the shared Primary profile (WAL,
/// `synchronous=FULL`, the shared busy timeout).
///
/// DDL-free since the storage unification: opening a connection no longer
/// plants the session tables, so co-tenant stores (schedule, runtime) stop
/// materializing empty session tables in their files — they open through
/// their own domain-preflighted openers. Callers apply the
/// [`SESSION_STORE_DOMAIN`] schema domain after opening; the same domain is
/// preflighted at open so an ineligible file is refused before the profile's
/// WAL conversion touches it.
pub fn open_connection(path: &Path) -> Result<Connection, StoreError> {
    open_connection_with_options(path, SqliteConnectionOptions::default())
}

/// [`open_connection`] with a per-store contention policy override.
pub fn open_connection_with_options(
    path: &Path,
    options: SqliteConnectionOptions,
) -> Result<Connection, StoreError> {
    meerkat_sqlite::open_with(
        path,
        meerkat_sqlite::ConnectionProfile::PRIMARY,
        meerkat_sqlite::OpenOptions {
            busy_timeout: Some(options.busy_timeout),
            // Schema-eligibility refusal must fire before the Primary
            // profile's journal-mode conversion mutates the file.
            schema_preflight: &[&SESSION_STORE_DOMAIN],
        },
    )
    .map_err(StoreError::from)
}

fn migration_0001_session_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(CREATE_SESSIONS_TABLE_SQL)?;
    tx.execute_batch(CREATE_SESSIONS_UPDATED_INDEX_SQL)?;
    tx.execute_batch(CREATE_SESSION_STRAND_MESSAGES_TABLE_SQL)?;
    tx.execute_batch(CREATE_SESSION_REWRITES_TABLE_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEADS_TABLE_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEADS_UPDATED_INDEX_SQL)?;
    Ok(())
}

fn migration_0002_strand_links(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(CREATE_SESSION_STRAND_LINKS_TABLE_SQL)
}

fn migration_0003_authenticated_head_sidecars(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(ADD_SESSION_REWRITES_GRAPH_EDGE_COLUMN_SQL)?;
    tx.execute_batch(CREATE_SESSION_COMPONENT_EVENTS_TABLE_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEAD_METADATA_CELLS_TABLE_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEAD_METADATA_CELLS_ROUTE_KEY_INDEX_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEAD_METADATA_CURRENT_TABLE_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEAD_METADATA_STATES_TABLE_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEAD_METADATA_STATES_PREDECESSOR_INDEX_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEAD_METADATA_STATE_DELTAS_TABLE_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEAD_METADATA_STATE_DELTAS_KEY_INDEX_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEAD_METADATA_REFS_TABLE_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEAD_METADATA_HEAD_LINEAGE_TABLE_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEAD_METADATA_HEAD_LINEAGE_PREDECESSOR_INDEX_SQL)?;
    tx.execute_batch(CREATE_SESSION_HEAD_METADATA_HEAD_LINEAGE_SUCCESSOR_INDEX_SQL)
}

fn initialize_current_session_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    migration_0001_session_schema(tx)?;
    migration_0002_strand_links(tx)?;
    migration_0003_authenticated_head_sidecars(tx)
}

fn build_released_0_8_10_session_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    migration_0001_session_schema(tx)?;
    migration_0002_strand_links(tx)
}

const RELEASED_0_8_10_SESSION_OBJECTS: &[meerkat_sqlite::SchemaObject] = &[
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "sessions",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "session_strand_messages",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "session_strand_links",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "session_rewrites",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "session_heads",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "sessions_updated_idx",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "session_heads_updated_idx",
    },
];

fn verify_released_0_8_10_session_schema(conn: &Connection) -> Result<(), String> {
    meerkat_sqlite::verify_released_schema_fingerprint(
        conn,
        &SESSION_STORE_DOMAIN,
        RELEASED_0_8_10_SESSION_OBJECTS,
        build_released_0_8_10_session_schema,
    )
}

/// The session store's schema domain in the per-file migration ledger.
pub const SESSION_STORE_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "session-store",
    migrations: &[
        meerkat_sqlite::Migration {
            version: 1,
            name: "base-schema",
            apply: migration_0001_session_schema,
        },
        meerkat_sqlite::Migration {
            version: 2,
            name: "strand-supersession-links",
            apply: migration_0002_strand_links,
        },
        meerkat_sqlite::Migration {
            version: 3,
            name: "authenticated-head-sidecars",
            apply: migration_0003_authenticated_head_sidecars,
        },
    ],
    initialize_current: initialize_current_session_schema,
    allowed_existing_versions: &[2, 3],
    released_predecessors: &[meerkat_sqlite::SchemaPredecessor {
        version: 2,
        verify: verify_released_0_8_10_session_schema,
    }],
    owned_objects: &[
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "sessions",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "sessions_updated_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "session_strand_messages",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "session_strand_links",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "session_rewrites",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "session_heads",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "session_heads_updated_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "session_component_events",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "session_head_metadata_cells",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "session_head_metadata_cells_route_key_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "session_head_metadata_current",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "session_head_metadata_states",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "session_head_metadata_states_predecessor_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "session_head_metadata_state_deltas",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "session_head_metadata_state_deltas_key_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "session_head_metadata_refs",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "session_head_metadata_head_lineage",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "session_head_metadata_head_lineage_predecessor_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "session_head_metadata_head_lineage_successor_idx",
        },
    ],
    retired_objects: &[],
};

#[cfg(test)]
mod schema_floor_tests {
    use super::*;

    fn released_v2() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open");
        let tx = conn.transaction().expect("tx");
        build_released_0_8_10_session_schema(&tx).expect("released schema");
        tx.commit().expect("commit");
        conn.execute_batch(
            "CREATE TABLE meerkat_schema (
                 domain TEXT PRIMARY KEY,
                 version INTEGER NOT NULL
             );
             INSERT INTO meerkat_schema VALUES ('session-store', 2);",
        )
        .expect("ledger");
        conn
    }

    #[test]
    fn exact_released_v2_upgrades_to_current() {
        let mut conn = released_v2();
        let report = meerkat_sqlite::apply_domain_migrations(&mut conn, &SESSION_STORE_DOMAIN)
            .expect("upgrade");
        assert_eq!(report.from_version, 2);
        assert_eq!(report.to_version, 3);
    }

    #[test]
    fn released_v2_final_column_table_and_index_collisions_are_refused_unmutated() {
        for collision in [
            "ALTER TABLE session_rewrites ADD COLUMN graph_edge_json BLOB",
            "CREATE TABLE session_component_events (candidate INTEGER)",
            "CREATE INDEX session_head_metadata_states_predecessor_idx
                 ON session_heads(session_id)",
        ] {
            let mut conn = released_v2();
            conn.execute_batch(collision).expect("collision");
            let err = meerkat_sqlite::apply_domain_migrations(&mut conn, &SESSION_STORE_DOMAIN)
                .expect_err("refuse collision");
            assert!(matches!(
                err,
                meerkat_sqlite::SqliteStoreError::SchemaFingerprintMismatch { version: 2, .. }
            ));
            assert_eq!(
                meerkat_sqlite::domain_version(&conn, SESSION_STORE_DOMAIN.name).expect("ledger"),
                Some(2)
            );
        }
    }
}

/// Open a connection and bring the session-store schema domain up to date.
fn open_session_connection(
    path: &Path,
    options: SqliteConnectionOptions,
) -> Result<Connection, StoreError> {
    let mut conn = open_connection_with_options(path, options)?;
    meerkat_sqlite::apply_domain_migrations(&mut conn, &SESSION_STORE_DOMAIN)?;
    Ok(conn)
}

pub fn begin_immediate_transaction(conn: &mut Connection) -> Result<Transaction<'_>, StoreError> {
    begin_immediate_transaction_with_options(conn, SqliteConnectionOptions::default())
}

pub fn begin_immediate_transaction_with_options(
    conn: &mut Connection,
    _options: SqliteConnectionOptions,
) -> Result<Transaction<'_>, StoreError> {
    // rusqlite's configured busy handler performs the bounded retry while
    // BEGIN IMMEDIATE waits for the WAL writer. Keeping it on the connection
    // makes the policy apply consistently to begin, statements, and commit.
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(StoreError::from)
}

/// Bring the session-store schema domain up to date on an already-open
/// connection.
///
/// Routes through the shared migration ledger ([`SESSION_STORE_DOMAIN`]):
/// the domain version is checked and stamped in the same transaction as the
/// DDL, and a file stamped by a newer binary is refused typed
/// ([`StoreError::SchemaFromTheFuture`]) before anything runs. There is no
/// unledgered DDL entry point.
pub fn ensure_schema(conn: &mut Connection) -> Result<(), StoreError> {
    meerkat_sqlite::apply_domain_migrations(conn, &SESSION_STORE_DOMAIN)?;
    Ok(())
}

pub fn write_session_snapshot_in_txn(
    tx: &Transaction<'_>,
    session: &Session,
) -> Result<(), StoreError> {
    let session_id = session.id().to_string();
    let metadata_json = serde_json::to_string(session.metadata())?;
    // Encode + midstate admission are one core-owned operation: in-process
    // reads of these exact bytes adopt the warm digest midstates instead of
    // reseeding them with O(document) canonical passes.
    let session_json = session.to_persisted_bytes()?;
    // Derived projection counters must round-trip through the durable i64
    // columns without loss. A count that exceeds i64::MAX is itself an
    // impossible state, so fail closed rather than silently clamping to a
    // fabricated maximum (terminal-truth store-metadata cluster).
    let message_count = i64::try_from(session.messages().len()).map_err(|_| {
        StoreError::Internal(format!(
            "session '{session_id}' message_count {} exceeds durable i64 range",
            session.messages().len()
        ))
    })?;
    let total_tokens = i64::try_from(session.total_tokens()).map_err(|_| {
        StoreError::Internal(format!(
            "session '{session_id}' total_tokens {} exceeds durable i64 range",
            session.total_tokens()
        ))
    })?;
    tx.execute(
        r"
        INSERT INTO sessions (
            session_id,
            created_at_ms,
            updated_at_ms,
            message_count,
            total_tokens,
            metadata_json,
            session_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(session_id) DO UPDATE SET
            created_at_ms = excluded.created_at_ms,
            updated_at_ms = excluded.updated_at_ms,
            message_count = excluded.message_count,
            total_tokens = excluded.total_tokens,
            metadata_json = excluded.metadata_json,
            session_json = excluded.session_json
        ",
        params![
            session_id,
            system_time_millis(session.created_at()),
            system_time_millis(session.updated_at()),
            message_count,
            total_tokens,
            metadata_json,
            session_json,
        ],
    )?;
    Ok(())
}

fn load_session_snapshot_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<Option<Session>, StoreError> {
    tx.query_row(
        "SELECT session_json FROM sessions WHERE session_id = ?1",
        params![id.to_string()],
        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
    )
    .optional()?
    .map(|bytes| Session::from_persisted_bytes(&bytes).map_err(StoreError::Serialization))
    .transpose()
}

// ---------------------------------------------------------------------------
// Incremental (head-canonical) helpers. All run inside an immediate
// transaction on the caller's connection.
// ---------------------------------------------------------------------------

fn now_millis() -> i64 {
    system_time_millis(SystemTime::now())
}

fn head_row_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<Option<(SessionHead, String)>, SessionStoreError> {
    let row = tx
        .query_row(
            "SELECT head_json, cas_token FROM session_heads WHERE session_id = ?1",
            params![id.to_string()],
            |row| {
                Ok((
                    row.get::<_, JsonColumnBytes>(0)?.into_bytes(),
                    row.get::<_, String>(1)?,
                ))
            },
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let Some((head_json, cas_token)) = row else {
        return Ok(None);
    };
    let head: SessionHead =
        serde_json::from_slice(&head_json).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    Ok(Some((head, cas_token)))
}

#[derive(Clone, Copy)]
enum HeadMetadataProjectionOwner {
    PhysicalHead,
    RuntimeBoundary,
}

impl HeadMetadataProjectionOwner {
    fn as_str(self) -> &'static str {
        match self {
            Self::PhysicalHead => "physical_head",
            Self::RuntimeBoundary => "runtime_boundary",
        }
    }
}

#[derive(Debug, Clone)]
struct HeadMetadataOwnerRef {
    /// Immutable head token that created `state_id`.
    ///
    /// Transcript-only successors with the same metadata identity deliberately
    /// do not churn this row.
    head_cas_token: String,
    state_id: String,
}

#[derive(Debug, Clone)]
struct HeadMetadataStateRow {
    predecessor_state_id: Option<String>,
    identity: SessionHeadMetadataIdentity,
    transition_id: String,
}

fn metadata_state_id(head_cas_token: &str) -> String {
    format!("head-metadata-state:{head_cas_token}")
}

fn metadata_owner_ref_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    owner: HeadMetadataProjectionOwner,
) -> Result<Option<HeadMetadataOwnerRef>, SessionStoreError> {
    tx.query_row(
        r"
        SELECT head_cas_token, state_id
        FROM session_head_metadata_refs
        WHERE session_id = ?1 AND owner = ?2
        ",
        params![id.to_string(), owner.as_str()],
        |row| {
            Ok(HeadMetadataOwnerRef {
                head_cas_token: row.get(0)?,
                state_id: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(StoreError::from)
    .map_err(into_session_store_error)
}

fn metadata_state_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    state_id: &str,
) -> Result<Option<HeadMetadataStateRow>, SessionStoreError> {
    let stored = tx
        .query_row(
            r"
            SELECT predecessor_state_id, identity_json, transition_id
            FROM session_head_metadata_states
            WHERE session_id = ?1 AND state_id = ?2
            ",
            params![id.to_string(), state_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    stored
        .map(|(predecessor_state_id, identity_json, transition_id)| {
            let identity: SessionHeadMetadataIdentity = serde_json::from_slice(&identity_json)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            if serde_json::to_vec(&identity).map_err(SessionStoreError::from)? != identity_json {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            Ok(HeadMetadataStateRow {
                predecessor_state_id,
                identity,
                transition_id,
            })
        })
        .transpose()
}

fn parse_metadata_cell(
    id: &SessionId,
    key: String,
    key_route: Vec<u8>,
    exact_value_digest: String,
    metadata_json: Vec<u8>,
) -> Result<Arc<SessionHeadMetadataCell>, SessionStoreError> {
    let route: [u8; 32] = key_route
        .try_into()
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let identity = SessionHeadMetadataCellIdentity::new(
        SessionHeadMetadataValueDigest::parse(exact_value_digest)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
    );
    let cell =
        SessionHeadMetadataCell::from_canonical_json(key, identity, Arc::from(metadata_json))
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    if cell.key_route() != &route {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(Arc::new(cell))
}

fn metadata_cell_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    key: &str,
    exact_value_digest: &str,
) -> Result<Option<Arc<SessionHeadMetadataCell>>, SessionStoreError> {
    let row = tx
        .query_row(
            r"
            SELECT key_route, metadata_json
            FROM session_head_metadata_cells
            WHERE session_id = ?1
              AND metadata_key = ?2
              AND exact_value_digest = ?3
            ",
            params![id.to_string(), key, exact_value_digest],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                ))
            },
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    row.map(|(key_route, metadata_json)| {
        parse_metadata_cell(
            id,
            key.to_string(),
            key_route,
            exact_value_digest.to_string(),
            metadata_json,
        )
    })
    .transpose()
}

fn persist_metadata_cell_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    cell: &SessionHeadMetadataCell,
) -> Result<(), SessionStoreError> {
    // The covering `(session_id, key_route, metadata_key)` index makes both
    // strict key ranges point lookups. Do not use `metadata_key <> ?` here:
    // SQLite can satisfy that only by walking every historical version of the
    // same hot key, turning a collision guard into O(history).
    let lower_collision = tx
        .query_row(
            r"
            SELECT metadata_key
            FROM session_head_metadata_cells
            WHERE session_id = ?1 AND key_route = ?2 AND metadata_key < ?3
            ORDER BY metadata_key DESC
            LIMIT 1
            ",
            params![id.to_string(), cell.key_route().as_slice(), cell.key()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let upper_collision = tx
        .query_row(
            r"
            SELECT metadata_key
            FROM session_head_metadata_cells
            WHERE session_id = ?1 AND key_route = ?2 AND metadata_key > ?3
            ORDER BY metadata_key ASC
            LIMIT 1
            ",
            params![id.to_string(), cell.key_route().as_slice(), cell.key()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    if lower_collision.is_some() || upper_collision.is_some() {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let inserted = tx
        .execute(
            r"
            INSERT OR IGNORE INTO session_head_metadata_cells (
                session_id, metadata_key, key_route, exact_value_digest,
                metadata_json, created_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ",
            params![
                id.to_string(),
                cell.key(),
                cell.key_route().as_slice(),
                cell.identity().exact_value_digest().as_str(),
                cell.canonical_json(),
                now_millis(),
            ],
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    if inserted == 0 {
        let stored = metadata_cell_in_txn(
            tx,
            id,
            cell.key(),
            cell.identity().exact_value_digest().as_str(),
        )?
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if stored.key_route() != cell.key_route()
            || stored.identity() != cell.identity()
            || stored.canonical_json() != cell.canonical_json()
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
    }
    Ok(())
}

fn current_metadata_cell_digest_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    key: &str,
) -> Result<Option<(Vec<u8>, String)>, SessionStoreError> {
    tx.query_row(
        r"
        SELECT key_route, exact_value_digest
        FROM session_head_metadata_current
        WHERE session_id = ?1 AND metadata_key = ?2
        ",
        params![id.to_string(), key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(StoreError::from)
    .map_err(into_session_store_error)
}

fn move_metadata_owner_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    owner: HeadMetadataProjectionOwner,
    head_cas_token: &str,
    state_id: &str,
) -> Result<(), SessionStoreError> {
    tx.execute(
        r"
        INSERT INTO session_head_metadata_refs (
            session_id, owner, head_cas_token, state_id
        ) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(session_id, owner) DO UPDATE SET
            head_cas_token = excluded.head_cas_token,
            state_id = excluded.state_id
        ",
        params![id.to_string(), owner.as_str(), head_cas_token, state_id],
    )
    .map_err(StoreError::from)
    .map_err(into_session_store_error)?;
    Ok(())
}

fn insert_metadata_state_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    state_id: &str,
    predecessor_state_id: Option<&str>,
    identity: &SessionHeadMetadataIdentity,
    transition_id: &str,
) -> Result<(), SessionStoreError> {
    let identity_json = serde_json::to_vec(identity).map_err(SessionStoreError::from)?;
    tx.execute(
        r"
        INSERT INTO session_head_metadata_states (
            session_id, state_id, predecessor_state_id, identity_json,
            transition_id, created_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ",
        params![
            id.to_string(),
            state_id,
            predecessor_state_id,
            identity_json,
            transition_id,
            now_millis(),
        ],
    )
    .map_err(StoreError::from)
    .map_err(into_session_store_error)?;
    Ok(())
}

fn insert_metadata_lineage_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    transition_id: &str,
    predecessor_head_cas_token: Option<&str>,
    successor_head_cas_token: &str,
    predecessor_state_id: Option<&str>,
    successor_state_id: &str,
) -> Result<(), SessionStoreError> {
    tx.execute(
        r"
        INSERT INTO session_head_metadata_head_lineage (
            session_id, transition_id, predecessor_head_cas_token,
            successor_head_cas_token, predecessor_state_id,
            successor_state_id, created_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ",
        params![
            id.to_string(),
            transition_id,
            predecessor_head_cas_token,
            successor_head_cas_token,
            predecessor_state_id,
            successor_state_id,
            now_millis(),
        ],
    )
    .map_err(StoreError::from)
    .map_err(into_session_store_error)?;
    Ok(())
}

fn persist_metadata_state_delta_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    state_id: &str,
    ordinal: usize,
    mutation: &meerkat_core::session::SessionHeadMetadataCellMutation,
) -> Result<(), SessionStoreError> {
    let ordinal = i64::try_from(ordinal).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    tx.execute(
        r"
        INSERT INTO session_head_metadata_state_deltas (
            session_id, state_id, ordinal, metadata_key, key_route,
            predecessor_exact_value_digest, successor_exact_value_digest
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ",
        params![
            id.to_string(),
            state_id,
            ordinal,
            mutation.key(),
            mutation.key_route().as_slice(),
            mutation
                .predecessor()
                .map(|identity| identity.exact_value_digest().as_str()),
            mutation
                .successor()
                .map(|cell| cell.identity().exact_value_digest().as_str()),
        ],
    )
    .map_err(StoreError::from)
    .map_err(into_session_store_error)?;
    Ok(())
}

fn apply_metadata_mutation_to_current_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    mutation: &meerkat_core::session::SessionHeadMetadataCellMutation,
) -> Result<(), SessionStoreError> {
    let observed = current_metadata_cell_digest_in_txn(tx, id, mutation.key())?;
    let expected = mutation
        .predecessor()
        .map(|identity| identity.exact_value_digest().as_str());
    match (observed.as_ref(), expected) {
        (None, None) => {}
        (Some((route, digest)), Some(expected))
            if route.as_slice() == mutation.key_route() && digest.as_str() == expected => {}
        _ => return Err(SessionStoreError::Corrupted(id.clone())),
    }
    match mutation.successor() {
        Some(cell) => {
            persist_metadata_cell_in_txn(tx, id, cell)?;
            tx.execute(
                r"
                INSERT INTO session_head_metadata_current (
                    session_id, metadata_key, key_route, exact_value_digest
                ) VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(session_id, metadata_key) DO UPDATE SET
                    key_route = excluded.key_route,
                    exact_value_digest = excluded.exact_value_digest
                ",
                params![
                    id.to_string(),
                    mutation.key(),
                    mutation.key_route().as_slice(),
                    cell.identity().exact_value_digest().as_str(),
                ],
            )
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?;
        }
        None => {
            let deleted = tx
                .execute(
                    r"
                    DELETE FROM session_head_metadata_current
                    WHERE session_id = ?1 AND metadata_key = ?2
                    ",
                    params![id.to_string(), mutation.key()],
                )
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            if deleted != 1 {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
        }
    }
    Ok(())
}

/// Reconcile one authenticated metadata state transition before publishing its
/// compact head row.
///
/// An unchanged identity is a literal metadata-table no-write path: the
/// physical state is point-checked and no cell/state/delta/ref/lineage row is
/// changed. A root or changed identity verifies the prepared sparse-Merkle
/// chain, writes only its changed cells plus compact lineage, and moves the
/// physical owner in this same transaction.
fn reconcile_head_metadata_transition_in_txn(
    tx: &Transaction<'_>,
    predecessor_head: Option<&SessionHead>,
    predecessor_head_token: Option<&str>,
    successor_head: &SessionHead,
    successor_head_token: &str,
) -> Result<(), SessionStoreError> {
    let id = &successor_head.id;
    if session_head_cas_token(successor_head)? != successor_head_token {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let successor_identity = successor_head
        .metadata_identity()
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    let projection = successor_head
        .metadata_projection()
        .filter(|projection| projection.identity() == successor_identity)
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    if projection
        .mutations()
        .iter()
        .any(|mutation| !mutation.verify())
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }

    let predecessor_identity = predecessor_head.and_then(SessionHead::metadata_identity);
    let physical_ref =
        metadata_owner_ref_in_txn(tx, id, HeadMetadataProjectionOwner::PhysicalHead)?;
    let predecessor_state_id = match predecessor_identity {
        Some(identity) => {
            if projection.predecessor_identity() != Some(identity) {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            let owner = physical_ref
                .as_ref()
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            let state = metadata_state_in_txn(tx, id, &owner.state_id)?
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            if &state.identity != identity {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            Some(owner.state_id.clone())
        }
        None => {
            if projection.predecessor_identity().is_some()
                || !projection.is_full_snapshot()
                || physical_ref.is_some()
            {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            let current_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM session_head_metadata_current WHERE session_id = ?1",
                    params![id.to_string()],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            if current_count != 0 {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            if projection.mutations().is_empty() {
                if !successor_identity.is_canonical_empty() {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
            } else if projection.mutations().len()
                != usize::try_from(successor_identity.entry_count())
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?
                || projection.mutations().first().map_or(true, |mutation| {
                    !mutation.predecessor_identity().is_canonical_empty()
                })
            {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            None
        }
    };

    if predecessor_identity == Some(successor_identity) {
        if !projection.mutations().is_empty() || predecessor_state_id.is_none() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        // Literal stable path: no metadata table is written and no metadata
        // value row is read. The ref's token remains the state-creating anchor.
        return Ok(());
    }

    if predecessor_identity.is_some() && projection.mutations().is_empty() {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let successor_state_id = metadata_state_id(successor_head_token);
    insert_metadata_state_in_txn(
        tx,
        id,
        &successor_state_id,
        predecessor_state_id.as_deref(),
        successor_identity,
        successor_head_token,
    )?;
    for (ordinal, mutation) in projection.mutations().iter().enumerate() {
        apply_metadata_mutation_to_current_in_txn(tx, id, mutation)?;
        persist_metadata_state_delta_in_txn(tx, id, &successor_state_id, ordinal, mutation)?;
    }
    move_metadata_owner_in_txn(
        tx,
        id,
        HeadMetadataProjectionOwner::PhysicalHead,
        successor_head_token,
        &successor_state_id,
    )?;
    insert_metadata_lineage_in_txn(
        tx,
        id,
        successor_head_token,
        predecessor_head_token,
        successor_head_token,
        predecessor_state_id.as_deref(),
        &successor_state_id,
    )?;
    // Standalone SessionStore users have no RuntimeStore owner-ref move to
    // trigger convergence. Bound them here as soon as the physical state
    // advances. A divergent runtime owner makes this a read-only no-op; its
    // later atomic owner move performs the same prune.
    prune_converged_metadata_history_in_txn(tx, id, None)
}

fn current_metadata_cells_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<BTreeMap<String, Arc<SessionHeadMetadataCell>>, SessionStoreError> {
    let mut statement = tx
        .prepare(
            r"
            SELECT current.metadata_key, current.key_route, cell.key_route,
                   current.exact_value_digest, cell.metadata_json
            FROM session_head_metadata_current AS current
            JOIN session_head_metadata_cells AS cell
              ON cell.session_id = current.session_id
             AND cell.metadata_key = current.metadata_key
             AND cell.exact_value_digest = current.exact_value_digest
            WHERE current.session_id = ?1
            ORDER BY current.metadata_key
            ",
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let rows = statement
        .query_map(params![id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, JsonColumnBytes>(4)?.into_bytes(),
            ))
        })
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let mut cells = BTreeMap::new();
    for (key, current_route, cell_route, exact, bytes) in rows {
        if current_route != cell_route {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let cell = parse_metadata_cell(id, key.clone(), current_route, exact, bytes)?;
        if cells.insert(key, cell).is_some() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
    }
    Ok(cells)
}

#[derive(Debug)]
struct StoredMetadataDelta {
    key: String,
    key_route: Vec<u8>,
    predecessor_exact_value_digest: Option<String>,
    successor_exact_value_digest: Option<String>,
}

fn metadata_state_deltas_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    state_id: &str,
) -> Result<Vec<StoredMetadataDelta>, SessionStoreError> {
    let mut statement = tx
        .prepare(
            r"
            SELECT metadata_key, key_route, predecessor_exact_value_digest,
                   successor_exact_value_digest
            FROM session_head_metadata_state_deltas
            WHERE session_id = ?1 AND state_id = ?2
            ORDER BY ordinal
            ",
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    statement
        .query_map(params![id.to_string(), state_id], |row| {
            Ok(StoredMetadataDelta {
                key: row.get(0)?,
                key_route: row.get(1)?,
                predecessor_exact_value_digest: row.get(2)?,
                successor_exact_value_digest: row.get(3)?,
            })
        })
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)
}

fn materialize_standalone_metadata_state_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    state_id: &str,
) -> Result<BTreeMap<String, Arc<SessionHeadMetadataCell>>, SessionStoreError> {
    let state = metadata_state_in_txn(tx, id, state_id)?
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    if state.predecessor_state_id.is_some() {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let mut cells = BTreeMap::new();
    for delta in metadata_state_deltas_in_txn(tx, id, state_id)? {
        if delta.predecessor_exact_value_digest.is_some() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let exact = delta
            .successor_exact_value_digest
            .as_deref()
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let cell = metadata_cell_in_txn(tx, id, &delta.key, exact)?
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if cell.key_route().as_slice() != delta.key_route.as_slice()
            || cells.insert(delta.key, cell).is_some()
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
    }
    Ok(cells)
}

fn metadata_state_for_head_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
    head_token: &str,
) -> Result<Option<String>, SessionStoreError> {
    let id = &head.id;
    if let Some(state_id) = tx
        .query_row(
            r"
            SELECT successor_state_id
            FROM session_head_metadata_head_lineage
            WHERE session_id = ?1 AND successor_head_cas_token = ?2
            ",
            params![id.to_string(), head_token],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
    {
        return Ok(Some(state_id));
    }
    if let Some(state_id) = tx
        .query_row(
            r"
            SELECT predecessor_state_id
            FROM session_head_metadata_head_lineage
            WHERE session_id = ?1 AND predecessor_head_cas_token = ?2
            ",
            params![id.to_string(), head_token],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .flatten()
    {
        return Ok(Some(state_id));
    }
    let expected = head
        .metadata_identity()
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    for owner in [
        HeadMetadataProjectionOwner::PhysicalHead,
        HeadMetadataProjectionOwner::RuntimeBoundary,
    ] {
        if let Some(reference) = metadata_owner_ref_in_txn(tx, id, owner)?
            && metadata_state_in_txn(tx, id, &reference.state_id)?
                .is_some_and(|state| &state.identity == expected)
        {
            return Ok(Some(reference.state_id));
        }
    }
    Ok(None)
}

fn attach_head_metadata_projection(
    tx: &Transaction<'_>,
    head: &mut SessionHead,
    owner: HeadMetadataProjectionOwner,
) -> Result<(), SessionStoreError> {
    let Some(expected_identity) = head.metadata_identity().cloned() else {
        return Ok(());
    };
    let reference = metadata_owner_ref_in_txn(tx, &head.id, owner)?
        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    // A transcript-only successor deliberately has no metadata lineage row:
    // the exact owner ref remains pinned to the immutable state-creating
    // token. The compact head's authenticated identity below proves that
    // reusing that state is exact without churning metadata WAL.
    let target_state_id = reference.state_id;
    let target_state = metadata_state_in_txn(tx, &head.id, &target_state_id)?
        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    if target_state.identity != expected_identity {
        return Err(SessionStoreError::Corrupted(head.id.clone()));
    }

    let physical =
        metadata_owner_ref_in_txn(tx, &head.id, HeadMetadataProjectionOwner::PhysicalHead)?
            .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    let mut state_id = physical.state_id;
    let mut cells = current_metadata_cells_in_txn(tx, &head.id)?;
    let mut visited = BTreeSet::new();
    while state_id != target_state_id {
        if !visited.insert(state_id.clone()) {
            return Err(SessionStoreError::Corrupted(head.id.clone()));
        }
        let state = metadata_state_in_txn(tx, &head.id, &state_id)?
            .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
        let Some(predecessor_state_id) = state.predecessor_state_id else {
            cells = materialize_standalone_metadata_state_in_txn(tx, &head.id, &target_state_id)?;
            state_id = target_state_id.clone();
            break;
        };
        for delta in metadata_state_deltas_in_txn(tx, &head.id, &state_id)? {
            match delta.predecessor_exact_value_digest.as_deref() {
                Some(exact) => {
                    let cell = metadata_cell_in_txn(tx, &head.id, &delta.key, exact)?
                        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
                    if cell.key_route().as_slice() != delta.key_route.as_slice() {
                        return Err(SessionStoreError::Corrupted(head.id.clone()));
                    }
                    cells.insert(delta.key, cell);
                }
                None => {
                    cells.remove(&delta.key);
                }
            }
        }
        state_id = predecessor_state_id;
    }
    let projection = Arc::new(
        SessionHeadMetadataProjection::from_snapshot(
            expected_identity,
            cells.into_values().collect(),
        )
        .map_err(|_| SessionStoreError::Corrupted(head.id.clone()))?,
    );
    head.attach_metadata_projection(projection)
}

fn persist_standalone_metadata_snapshot_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
    head_token: &str,
) -> Result<String, SessionStoreError> {
    let id = &head.id;
    let identity = head
        .metadata_identity()
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    let projection = head
        .metadata_projection()
        .filter(|projection| {
            projection.identity() == identity
                && projection.predecessor_identity().is_none()
                && projection.is_full_snapshot()
        })
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    if projection
        .mutations()
        .iter()
        .any(|mutation| !mutation.verify())
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    if projection.mutations().is_empty() {
        if !identity.is_canonical_empty() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
    } else if projection.mutations().len()
        != usize::try_from(identity.entry_count())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?
        || projection.mutations().first().map_or(true, |mutation| {
            !mutation.predecessor_identity().is_canonical_empty()
        })
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let state_id = metadata_state_id(head_token);
    if let Some(stored) = metadata_state_in_txn(tx, id, &state_id)? {
        if stored.identity != *identity
            || stored.predecessor_state_id.is_some()
            || stored.transition_id != head_token
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let stored_deltas = metadata_state_deltas_in_txn(tx, id, &state_id)?;
        if stored_deltas.len() != projection.mutations().len() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        for (delta, mutation) in stored_deltas.iter().zip(projection.mutations()) {
            let cell = mutation
                .successor()
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            if !mutation.verify()
                || mutation.predecessor().is_some()
                || delta.key != mutation.key()
                || delta.key_route.as_slice() != mutation.key_route()
                || delta.predecessor_exact_value_digest.is_some()
                || delta.successor_exact_value_digest.as_deref()
                    != Some(cell.identity().exact_value_digest().as_str())
            {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            let stored_cell = metadata_cell_in_txn(
                tx,
                id,
                mutation.key(),
                cell.identity().exact_value_digest().as_str(),
            )?
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            if stored_cell.identity() != cell.identity()
                || stored_cell.key_route() != cell.key_route()
                || stored_cell.canonical_json() != cell.canonical_json()
            {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
        }
        let lineage = tx
            .query_row(
                r"
                SELECT transition_id, predecessor_head_cas_token,
                       predecessor_state_id, successor_state_id
                FROM session_head_metadata_head_lineage
                WHERE session_id = ?1 AND successor_head_cas_token = ?2
                ",
                params![id.to_string(), head_token],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if lineage.0 != head_token
            || lineage.1.is_some()
            || lineage.2.is_some()
            || lineage.3 != state_id
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        return Ok(state_id);
    }
    insert_metadata_state_in_txn(tx, id, &state_id, None, identity, head_token)?;
    for (ordinal, mutation) in projection.mutations().iter().enumerate() {
        if mutation.predecessor().is_some() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let cell = mutation
            .successor()
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        persist_metadata_cell_in_txn(tx, id, cell)?;
        persist_metadata_state_delta_in_txn(tx, id, &state_id, ordinal, mutation)?;
    }
    insert_metadata_lineage_in_txn(tx, id, head_token, None, head_token, None, &state_id)?;
    Ok(state_id)
}

fn delete_unreferenced_metadata_cell_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    key: &str,
    exact_value_digest: &str,
) -> Result<(), SessionStoreError> {
    tx.execute(
        r"
        DELETE FROM session_head_metadata_cells
        WHERE session_id = ?1
          AND metadata_key = ?2
          AND exact_value_digest = ?3
          AND NOT EXISTS (
              SELECT 1 FROM session_head_metadata_current AS current
              WHERE current.session_id = ?1
                AND current.metadata_key = ?2
                AND current.exact_value_digest = ?3
          )
          AND NOT EXISTS (
              SELECT 1 FROM session_head_metadata_state_deltas AS delta
              WHERE delta.session_id = ?1
                AND delta.metadata_key = ?2
                AND (
                    delta.predecessor_exact_value_digest = ?3
                    OR delta.successor_exact_value_digest = ?3
                )
          )
        ",
        params![id.to_string(), key, exact_value_digest],
    )
    .map_err(StoreError::from)
    .map_err(into_session_store_error)?;
    Ok(())
}

/// Bound retained metadata history after RuntimeStore and the physical head
/// converge on the same exact state.
///
/// Keep the current state plus its direct predecessor/delta as the latest
/// exact-retry witness. Rebase that predecessor to a root, remove its now-false
/// historical lineage plus every older state/delta/lineage row, then
/// point-delete cell versions no longer named by current state or the retained
/// witness. Work is proportional to the since-boundary metadata chain, never
/// to the accumulated session lifetime.
fn prune_converged_metadata_history_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    retired_runtime_state: Option<&str>,
) -> Result<(), SessionStoreError> {
    let physical = metadata_owner_ref_in_txn(tx, id, HeadMetadataProjectionOwner::PhysicalHead)?
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    let runtime = metadata_owner_ref_in_txn(tx, id, HeadMetadataProjectionOwner::RuntimeBoundary)?;
    if runtime
        .as_ref()
        .is_some_and(|runtime| runtime.state_id != physical.state_id)
    {
        return Ok(());
    }

    let physical_state = metadata_state_in_txn(tx, id, &physical.state_id)?
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    let retained_predecessor = physical_state.predecessor_state_id.clone();
    let mut delete_states = Vec::new();
    let mut candidates = BTreeSet::new();
    let mut cursor = match retained_predecessor.as_deref() {
        Some(state_id) => {
            metadata_state_in_txn(tx, id, state_id)?
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?
                .predecessor_state_id
        }
        None => None,
    };
    let mut visited = BTreeSet::new();
    while let Some(state_id) = cursor {
        if !visited.insert(state_id.clone()) {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let state = metadata_state_in_txn(tx, id, &state_id)?
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        for delta in metadata_state_deltas_in_txn(tx, id, &state_id)? {
            if let Some(digest) = delta.predecessor_exact_value_digest {
                candidates.insert((delta.key.clone(), digest));
            }
            if let Some(digest) = delta.successor_exact_value_digest {
                candidates.insert((delta.key, digest));
            }
        }
        cursor = state.predecessor_state_id.clone();
        delete_states.push(state_id);
    }
    if let Some(state_id) = retired_runtime_state
        && state_id != physical.state_id
        && retained_predecessor.as_deref() != Some(state_id)
        && !delete_states.iter().any(|existing| existing == state_id)
    {
        let state = metadata_state_in_txn(tx, id, state_id)?
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if state.predecessor_state_id.is_some() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        for delta in metadata_state_deltas_in_txn(tx, id, state_id)? {
            if let Some(digest) = delta.predecessor_exact_value_digest {
                candidates.insert((delta.key.clone(), digest));
            }
            if let Some(digest) = delta.successor_exact_value_digest {
                candidates.insert((delta.key, digest));
            }
        }
        delete_states.push(state_id.to_string());
    }

    if let Some(predecessor_state_id) = retained_predecessor.as_deref() {
        for delta in metadata_state_deltas_in_txn(tx, id, predecessor_state_id)? {
            if let Some(digest) = delta.predecessor_exact_value_digest {
                candidates.insert((delta.key.clone(), digest));
            }
            if let Some(digest) = delta.successor_exact_value_digest {
                candidates.insert((delta.key, digest));
            }
        }
        tx.execute(
            r"
            DELETE FROM session_head_metadata_state_deltas
            WHERE session_id = ?1 AND state_id = ?2
            ",
            params![id.to_string(), predecessor_state_id],
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
        tx.execute(
            r"
            UPDATE session_head_metadata_states
            SET predecessor_state_id = NULL
            WHERE session_id = ?1 AND state_id = ?2
            ",
            params![id.to_string(), predecessor_state_id],
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
        tx.execute(
            r"
            DELETE FROM session_head_metadata_head_lineage
            WHERE session_id = ?1 AND successor_state_id = ?2
            ",
            params![id.to_string(), predecessor_state_id],
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    }
    for state_id in &delete_states {
        tx.execute(
            r"
            DELETE FROM session_head_metadata_state_deltas
            WHERE session_id = ?1 AND state_id = ?2
            ",
            params![id.to_string(), state_id],
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
        tx.execute(
            r"
            DELETE FROM session_head_metadata_head_lineage
            WHERE session_id = ?1 AND successor_state_id = ?2
            ",
            params![id.to_string(), state_id],
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
        tx.execute(
            r"
            DELETE FROM session_head_metadata_states
            WHERE session_id = ?1 AND state_id = ?2
            ",
            params![id.to_string(), state_id],
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    }
    for (key, digest) in candidates {
        delete_unreferenced_metadata_cell_in_txn(tx, id, &key, &digest)?;
    }
    Ok(())
}

/// Move the retained RuntimeStore boundary reference to `head`.
///
/// The runtime authority row and this reference update share one co-tenant
/// transaction. Ordinary equal-state boundaries are a literal no-write path.
/// The exceptional 0.8.10 activation may seed a second verified root snapshot
/// when the retained runtime boundary predates a newer physical projection.
#[doc(hidden)]
pub fn retain_runtime_boundary_head_metadata_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
) -> Result<(), SessionStoreError> {
    let Some(identity) = head.metadata_identity() else {
        return Ok(());
    };
    let head_token = session_head_cas_token(head)?;
    let current =
        metadata_owner_ref_in_txn(tx, &head.id, HeadMetadataProjectionOwner::RuntimeBoundary)?;
    let mut state_id = metadata_state_for_head_in_txn(tx, head, &head_token)?;
    if state_id.is_none() {
        state_id = Some(persist_standalone_metadata_snapshot_in_txn(
            tx,
            head,
            &head_token,
        )?);
    }
    let state_id = state_id.ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    let state = metadata_state_in_txn(tx, &head.id, &state_id)?
        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    if &state.identity != identity {
        return Err(SessionStoreError::Corrupted(head.id.clone()));
    }
    if current
        .as_ref()
        .is_some_and(|current| current.state_id == state_id)
    {
        return Ok(());
    }
    let retired_runtime_state = current.as_ref().map(|current| current.state_id.as_str());
    move_metadata_owner_in_txn(
        tx,
        &head.id,
        HeadMetadataProjectionOwner::RuntimeBoundary,
        &head_token,
        &state_id,
    )?;
    prune_converged_metadata_history_in_txn(tx, &head.id, retired_runtime_state)
}

/// Clear the retained runtime-boundary reference before deleting its
/// co-tenant authority row and prune history no longer needed by any owner.
#[doc(hidden)]
pub fn clear_runtime_boundary_head_metadata_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<(), SessionStoreError> {
    let retired = metadata_owner_ref_in_txn(tx, id, HeadMetadataProjectionOwner::RuntimeBoundary)?;
    tx.execute(
        "DELETE FROM session_head_metadata_refs WHERE session_id = ?1 AND owner = 'runtime_boundary'",
        params![id.to_string()],
    )
    .map_err(StoreError::from)
    .map_err(into_session_store_error)?;
    prune_converged_metadata_history_in_txn(
        tx,
        id,
        retired.as_ref().map(|retired| retired.state_id.as_str()),
    )
}

/// Write the head row and restore the row-materialization rule.
///
/// A strand-changing head write point-settles only the strand it just left as
/// a splice delta of the new head. Same-strand appends do no topology work;
/// whole-history reachability and orphan collection are explicit maintenance
/// concerns.
fn write_head_row_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
) -> Result<String, SessionStoreError> {
    if head.metadata_identity().is_some() {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: "authenticated HeadCanonical metadata may only advance through a sealed \
                     prepared mutation"
                .to_string(),
        });
    }
    let previous_strand = head_row_in_txn(tx, &head.id)?.map(|(stored, _)| stored.strand);
    let cas_token = write_head_row_only_in_txn(tx, head)?;
    settle_strand_topology_in_txn(tx, head, previous_strand.as_ref())?;
    Ok(cas_token)
}

fn write_head_row_only_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
) -> Result<String, SessionStoreError> {
    let cas_token = session_head_cas_token(head)?;
    // `SessionHead` skips the Arc carrier: runtime authority and ordinary CAS
    // rows remain bounded regardless of accumulated user/config metadata.
    let head_json = serde_json::to_vec(head).map_err(SessionStoreError::from)?;
    let metadata_json = serde_json::to_string(&head.metadata).map_err(SessionStoreError::from)?;
    let message_count = i64::try_from(head.message_count).map_err(|_| {
        SessionStoreError::Internal(format!(
            "session '{}' head message_count {} exceeds durable i64 range",
            head.id, head.message_count
        ))
    })?;
    let rewrite_count = i64::try_from(head.rewrite_count).map_err(|_| {
        SessionStoreError::Internal(format!(
            "session '{}' head rewrite_count {} exceeds durable i64 range",
            head.id, head.rewrite_count
        ))
    })?;
    let total_tokens = i64::try_from(head.usage.total_tokens()).map_err(|_| {
        SessionStoreError::Internal(format!(
            "session '{}' head total_tokens {} exceeds durable i64 range",
            head.id,
            head.usage.total_tokens()
        ))
    })?;
    tx.execute(
        r"
        INSERT INTO session_heads (
            session_id, version, strand, head_revision, message_count,
            rewrite_count, total_tokens, created_at_ms, updated_at_ms,
            metadata_json, head_json, cas_token
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        ON CONFLICT(session_id) DO UPDATE SET
            version = excluded.version,
            strand = excluded.strand,
            head_revision = excluded.head_revision,
            message_count = excluded.message_count,
            rewrite_count = excluded.rewrite_count,
            total_tokens = excluded.total_tokens,
            created_at_ms = excluded.created_at_ms,
            updated_at_ms = excluded.updated_at_ms,
            metadata_json = excluded.metadata_json,
            head_json = excluded.head_json,
            cas_token = excluded.cas_token
        ",
        params![
            head.id.to_string(),
            i64::from(head.version),
            head.strand.as_str(),
            head.head_revision,
            message_count,
            rewrite_count,
            total_tokens,
            system_time_millis(head.created_at),
            system_time_millis(head.updated_at),
            metadata_json,
            head_json,
            cas_token,
        ],
    )
    .map_err(StoreError::from)
    .map_err(into_session_store_error)?;
    Ok(cas_token)
}

/// Rows a strand physically owns. For a materialized strand this is its
/// logical length; for a superseded one it is only the splice span, so read
/// paths must use [`strand_logical_len_in_txn`] instead.
fn materialized_row_count_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
) -> Result<u64, SessionStoreError> {
    let count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM session_strand_messages WHERE session_id = ?1 AND strand = ?2",
            params![id.to_string(), strand.as_str()],
            |row| row.get(0),
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    u64::try_from(count).map_err(|_| SessionStoreError::Corrupted(id.clone()))
}

/// Whether an exact strand key owns any physical row.
///
/// Prepared rewrite occurrence ids must be empty before their immutable link
/// is installed. A LIMIT-1 probe keeps that conflict check independent of the
/// number of stray/corrupt rows at the key.
fn strand_has_physical_rows_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
) -> Result<bool, SessionStoreError> {
    tx.query_row(
        "SELECT 1 FROM session_strand_messages
         WHERE session_id = ?1 AND strand = ?2
         LIMIT 1",
        params![id.to_string(), strand.as_str()],
        |_row| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .map_err(StoreError::from)
    .map_err(into_session_store_error)
}

/// One past the highest physical row sequence on a strand.
///
/// Linked rewrite strands are sparse: they own the replacement span inside
/// the link's base length and may later own an ordinary appended tail beyond
/// that base. `COUNT(*)` cannot describe that logical extent.
fn physical_row_extent_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
) -> Result<u64, SessionStoreError> {
    let max_seq: Option<i64> = tx
        .query_row(
            "SELECT MAX(seq) FROM session_strand_messages WHERE session_id = ?1 AND strand = ?2",
            params![id.to_string(), strand.as_str()],
            |row| row.get(0),
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    match max_seq {
        Some(seq) => u64::try_from(seq)
            .ok()
            .and_then(|seq| seq.checked_add(1))
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone())),
        None => Ok(0),
    }
}

// ---------------------------------------------------------------------------
// Bounded active overlays and periodically settled materialized anchors.
// ---------------------------------------------------------------------------

/// One persisted supersession edge.
#[derive(Debug, Clone)]
struct StrandLinkRow {
    successor: TranscriptStrandId,
    splice: StrandSplice,
}

/// Supersession edges keyed by superseded strand.
///
/// Ordinary materialization populates this map only from bounded point reads
/// reachable from the store-issued anchor/head. Explicit audit and maintenance
/// may still load the complete historical map.
type StrandLinks = std::collections::HashMap<String, StrandLinkRow>;

/// Point-read one immutable strand descriptor.
///
/// Ordinary prepared rewrites name every descriptor they may create or
/// observe. The hot apply/retry path must therefore read those exact keys
/// instead of loading and validating the session's accumulated topology.
/// Full topology validation remains part of explicit activation, rewrite
/// audit, and doctor/maintenance surfaces.
fn strand_link_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
) -> Result<Option<StrandLinkRow>, SessionStoreError> {
    let row = tx
        .query_row(
            "SELECT successor, strand_len, splice_start, splice_end, successor_end
             FROM session_strand_links
             WHERE session_id = ?1 AND strand = ?2",
            params![id.to_string(), strand.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let Some((successor, strand_len, splice_start, splice_end, successor_end)) = row else {
        return Ok(None);
    };
    let corrupt = || SessionStoreError::Corrupted(id.clone());
    let splice = StrandSplice {
        strand_len: u64::try_from(strand_len).map_err(|_| corrupt())?,
        splice_start: u64::try_from(splice_start).map_err(|_| corrupt())?,
        splice_end: u64::try_from(splice_end).map_err(|_| corrupt())?,
        successor_end: u64::try_from(successor_end).map_err(|_| corrupt())?,
    };
    if !splice.is_well_formed() {
        return Err(corrupt());
    }
    Ok(Some(StrandLinkRow {
        successor: TranscriptStrandId::from_persisted(successor),
        splice,
    }))
}

/// Load only topology reachable from the named roots, refusing any path whose
/// active depth exceeds the store-issued budget.
///
/// This is the ordinary read primitive. Historical links not reachable from
/// the current anchor/head are deliberately invisible here; whole-graph
/// validation remains an explicit audit/maintenance operation.
fn bounded_strand_links_for_roots_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    roots: &[TranscriptStrandId],
    max_hops_per_root: usize,
) -> Result<StrandLinks, SessionStoreError> {
    let mut links = StrandLinks::new();
    let mut verified_to_terminal = std::collections::HashSet::new();
    for root in roots {
        let mut cursor = root.clone();
        let mut path = std::collections::HashSet::new();
        let mut hops = 0_usize;
        loop {
            if verified_to_terminal.contains(cursor.as_str()) {
                break;
            }
            if !path.insert(cursor.as_str().to_string()) {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            let link = match links.get(cursor.as_str()) {
                Some(link) => Some(link.clone()),
                None => strand_link_in_txn(tx, id, &cursor)?,
            };
            let Some(link) = link else {
                break;
            };
            if hops == max_hops_per_root {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            hops = hops
                .checked_add(1)
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            let successor = link.successor.clone();
            if let Some(existing) = links.insert(cursor.as_str().to_string(), link)
                && existing.successor != successor
            {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            cursor = successor;
        }
        verified_to_terminal.extend(path);
    }
    Ok(links)
}

fn ordinary_topology_hop_budget(
    head: &SessionHead,
    additional_rewrites: usize,
) -> Result<usize, SessionStoreError> {
    let anchor = head
        .row_lineage_anchor
        .as_ref()
        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    let post_anchor = head
        .rewrite_count
        .checked_sub(anchor.rewrite_count())
        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    usize::try_from(post_anchor)
        .ok()
        .and_then(|post_anchor| {
            usize::try_from(SESSION_ROW_LINEAGE_REBASE_INTERVAL)
                .ok()
                .and_then(|interval| interval.checked_add(post_anchor))
        })
        .and_then(|budget| budget.checked_add(additional_rewrites))
        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))
}

fn reconcile_exact_strand_link_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    successor: &TranscriptStrandId,
    splice: StrandSplice,
) -> Result<(), SessionStoreError> {
    if strand == successor || !splice.is_well_formed() {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    if let Some(existing) = strand_link_in_txn(tx, id, strand)? {
        if existing.successor == *successor && existing.splice == splice {
            return Ok(());
        }
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    if strand_has_physical_rows_in_txn(tx, id, strand)? {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    tx.execute(
        "INSERT INTO session_strand_links
             (session_id, strand, successor, strand_len, splice_start, splice_end,
              successor_end, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id.to_string(),
            strand.as_str(),
            successor.as_str(),
            i64::try_from(splice.strand_len)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            i64::try_from(splice.splice_start)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            i64::try_from(splice.splice_end)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            i64::try_from(splice.successor_end)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            now_millis(),
        ],
    )
    .map_err(StoreError::from)
    .map_err(into_session_store_error)?;
    Ok(())
}

/// Validate the session's functional strand graph once and return every
/// strand named as an edge target.
///
/// Specialized multi-rewrite catch-up carries this result across all pending
/// occurrences. Reloading or re-walking the full retained graph for each
/// occurrence would turn a `k`-rewrite recovery into O(k²) topology work.
fn validate_strand_links_acyclic(
    id: &SessionId,
    links: &StrandLinks,
) -> Result<std::collections::HashSet<String>, SessionStoreError> {
    let linked_successors = links
        .values()
        .map(|link| link.successor.as_str().to_string())
        .collect::<std::collections::HashSet<_>>();
    let mut settled = std::collections::HashSet::with_capacity(links.len());
    for start in links.keys() {
        if settled.contains(start) {
            continue;
        }
        let mut path = std::collections::HashSet::new();
        let mut cursor = start.as_str();
        while let Some(link) = links.get(cursor) {
            if settled.contains(cursor) {
                break;
            }
            if !path.insert(cursor.to_string()) {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            cursor = link.successor.as_str();
        }
        settled.extend(path);
    }
    Ok(linked_successors)
}

fn strand_links_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<StrandLinks, SessionStoreError> {
    let mut stmt = tx
        .prepare(
            "SELECT strand, successor, strand_len, splice_start, splice_end, successor_end
             FROM session_strand_links WHERE session_id = ?1",
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let rows = stmt
        .query_map(params![id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let mut links = StrandLinks::with_capacity(rows.len());
    for (strand, successor, strand_len, splice_start, splice_end, successor_end) in rows {
        let corrupt = || SessionStoreError::Corrupted(id.clone());
        let splice = StrandSplice {
            strand_len: u64::try_from(strand_len).map_err(|_| corrupt())?,
            splice_start: u64::try_from(splice_start).map_err(|_| corrupt())?,
            splice_end: u64::try_from(splice_end).map_err(|_| corrupt())?,
            successor_end: u64::try_from(successor_end).map_err(|_| corrupt())?,
        };
        // A malformed durable descriptor could silently serve the wrong
        // rows; refuse it instead (terminal-truth store-metadata cluster).
        if !splice.is_well_formed() {
            return Err(corrupt());
        }
        links.insert(
            strand,
            StrandLinkRow {
                successor: TranscriptStrandId::from_persisted(successor),
                splice,
            },
        );
    }
    Ok(links)
}

/// Logical row count of a strand: the link's recorded length for a
/// superseded strand, the physical count for a materialized one.
fn strand_logical_len_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    links: &StrandLinks,
) -> Result<u64, SessionStoreError> {
    match links.get(strand.as_str()) {
        Some(link) => Ok(link
            .splice
            .strand_len
            .max(physical_row_extent_in_txn(tx, id, strand)?)),
        None => materialized_row_count_in_txn(tx, id, strand),
    }
}

/// Serve a strand's logical rows, following supersession edges for the runs
/// the strand no longer owns.
fn resolve_strand_bytes_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    range: std::ops::Range<u64>,
    links: &StrandLinks,
) -> Result<Vec<Vec<u8>>, SessionStoreError> {
    // A chain cannot legally revisit a strand, so the edge count bounds the
    // hops; exceeding it means the durable edges cycle.
    resolve_strand_bytes_hops(tx, id, strand, range, links, links.len())
}

fn resolve_strand_bytes_hops(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    range: std::ops::Range<u64>,
    links: &StrandLinks,
    hops: usize,
) -> Result<Vec<Vec<u8>>, SessionStoreError> {
    let Some(link) = links.get(strand.as_str()) else {
        return strand_row_bytes_in_txn(tx, id, strand, range);
    };
    let logical_len = link
        .splice
        .strand_len
        .max(physical_row_extent_in_txn(tx, id, strand)?);
    if hops == 0 || range.start > range.end || range.end > logical_len {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let wanted = usize::try_from(range.end - range.start)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let mut rows = Vec::with_capacity(wanted);
    let linked_end = range.end.min(link.splice.strand_len);
    if range.start < linked_end {
        for segment in link.splice.segments(range.start..linked_end) {
            match segment {
                StrandSegment::Retained(span) => {
                    rows.extend(strand_row_bytes_in_txn(tx, id, strand, span)?);
                }
                StrandSegment::Successor(span) => {
                    rows.extend(resolve_strand_bytes_hops(
                        tx,
                        id,
                        &link.successor,
                        span,
                        links,
                        hops - 1,
                    )?);
                }
            }
        }
    }
    let tail_start = range.start.max(link.splice.strand_len);
    if tail_start < range.end {
        rows.extend(strand_row_bytes_in_txn(
            tx,
            id,
            strand,
            tail_start..range.end,
        )?);
    }
    if rows.len() != wanted {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(rows)
}

/// Settle one bounded active overlay into exact direct rows.
///
/// The caller must derive `links` either from a store-issued head/anchor
/// budget or from an explicit one-time migration audit. The full logical bytes
/// are resolved before the edge is retired, missing direct rows are reconciled
/// byte-for-byte, and the resulting strand is then a singular O(document)
/// cold-read authority. Historical inbound links remain valid because the
/// strand id and bytes do not change.
fn settle_resolved_strand_direct_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    logical_len: u64,
    links: &StrandLinks,
) -> Result<Vec<Vec<u8>>, SessionStoreError> {
    let serialized_rows = resolve_strand_bytes_in_txn(tx, id, strand, 0..logical_len, links)?;
    if physical_row_extent_in_txn(tx, id, strand)? > logical_len {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    reconcile_serialized_strand_rows_in_txn(tx, id, strand, 0, &serialized_rows)?;
    if links.contains_key(strand.as_str()) {
        let removed = tx
            .execute(
                "DELETE FROM session_strand_links WHERE session_id = ?1 AND strand = ?2",
                params![id.to_string(), strand.as_str()],
            )
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?;
        if removed != 1 {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
    }
    if materialized_row_count_in_txn(tx, id, strand)? != logical_len
        || physical_row_extent_in_txn(tx, id, strand)? != logical_len
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(serialized_rows)
}

/// Re-encode `strand` as the splice delta of `successor`, deleting the rows
/// the successor reproduces byte-for-byte.
///
/// The splice is derived by comparing the two persisted row vectors, so the
/// edge can never claim sharing that does not exist; a splice that shares
/// nothing (a full-transcript compaction) records the edge and keeps every
/// row. The refusals — already superseded, empty, self, or an edge that
/// would close a cycle — leave the strand materialized: correctness never
/// depends on supersession having run, only durable size does.
fn supersede_strand_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    successor: &TranscriptStrandId,
) -> Result<(), SessionStoreError> {
    if strand == successor {
        return Ok(());
    }
    if strand_link_in_txn(tx, id, strand)?.is_some() {
        return Ok(());
    }
    let hop_budget = usize::try_from(SESSION_ROW_LINEAGE_REBASE_INTERVAL)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let links =
        bounded_strand_links_for_roots_in_txn(tx, id, std::slice::from_ref(successor), hop_budget)?;
    // Refuse an edge whose successor already resolves back through `strand`:
    // a cycle would make both strands unreadable.
    let mut cursor: &str = successor.as_str();
    for _ in 0..=links.len() {
        if cursor == strand.as_str() {
            return Ok(());
        }
        let Some(next) = links.get(cursor) else {
            break;
        };
        cursor = next.successor.as_str();
    }

    let strand_len = materialized_row_count_in_txn(tx, id, strand)?;
    if strand_len == 0 {
        return Ok(());
    }
    let strand_rows = strand_row_bytes_in_txn(tx, id, strand, 0..strand_len)?;
    let successor_len = strand_logical_len_in_txn(tx, id, successor, &links)?;
    let successor_rows = resolve_strand_bytes_in_txn(tx, id, successor, 0..successor_len, &links)?;
    let splice = StrandSplice::between(&strand_rows, &successor_rows);
    // The edge is recorded even when nothing is shared (a full-transcript
    // compaction retains its whole parent by definition): the link is what
    // makes supersession idempotent, so later writes never recompute an
    // O(transcript) comparison that already settled.
    let to_i64 =
        |value: u64| i64::try_from(value).map_err(|_| SessionStoreError::Corrupted(id.clone()));
    let durable_strand_len = to_i64(splice.strand_len)?;
    let durable_splice_start = to_i64(splice.splice_start)?;
    let durable_splice_end = to_i64(splice.splice_end)?;
    let durable_successor_end = to_i64(splice.successor_end)?;
    tx.execute(
        "INSERT OR REPLACE INTO session_strand_links
             (session_id, strand, successor, strand_len, splice_start, splice_end,
              successor_end, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id.to_string(),
            strand.as_str(),
            successor.as_str(),
            durable_strand_len,
            durable_splice_start,
            durable_splice_end,
            durable_successor_end,
            now_millis(),
        ],
    )
    .map_err(StoreError::from)
    .map_err(into_session_store_error)?;
    tx.execute(
        "DELETE FROM session_strand_messages
         WHERE session_id = ?1 AND strand = ?2 AND (seq < ?3 OR seq >= ?4)",
        params![
            id.to_string(),
            strand.as_str(),
            durable_splice_start,
            durable_splice_end,
        ],
    )
    .map_err(StoreError::from)
    .map_err(into_session_store_error)?;
    Ok(())
}

/// Settle only the strand transition named by an ordinary head write.
///
/// Whole-history reachability and orphan collection are maintenance concerns;
/// neither belongs on the append/save path.
fn settle_strand_topology_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
    previous_strand: Option<&TranscriptStrandId>,
) -> Result<(), SessionStoreError> {
    let id = &head.id;
    if let Some(previous) = previous_strand
        && previous != &head.strand
    {
        // The head transition names the only newly inactive strand. Point
        // settle that exact edge; historical reachability collection belongs
        // to explicit maintenance and must not tax an ordinary head write.
        if head.rewrite_count == 0 {
            // With no rewrite rows, no historical authority can name the old
            // strand. Delete that exact key instead of retaining a useless
            // overlay edge or scanning an empty-history reachability graph.
            for sql in [
                "DELETE FROM session_strand_messages WHERE session_id = ?1 AND strand = ?2",
                "DELETE FROM session_strand_links WHERE session_id = ?1 AND strand = ?2",
            ] {
                tx.execute(sql, params![id.to_string(), previous.as_str()])
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
            }
        } else {
            supersede_strand_in_txn(tx, id, previous, &head.strand)?;
        }
    }
    Ok(())
}

fn strand_row_bytes_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    range: std::ops::Range<u64>,
) -> Result<Vec<Vec<u8>>, SessionStoreError> {
    if range.start >= range.end {
        return Ok(Vec::new());
    }
    let start = i64::try_from(range.start).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let end = i64::try_from(range.end).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let mut stmt = tx
        .prepare(
            "SELECT message_json FROM session_strand_messages
             WHERE session_id = ?1 AND strand = ?2 AND seq >= ?3 AND seq < ?4
             ORDER BY seq ASC",
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let rows = stmt
        .query_map(
            params![id.to_string(), strand.as_str(), start, end],
            |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let expected = range.end - range.start;
    if rows.len() as u64 != expected {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(rows)
}

/// A strand's logical messages, following exact overlay edges. Every read path
/// goes through here: a linked strand owns only its splice span and direct
/// append tail.
fn strand_messages_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    range: std::ops::Range<u64>,
) -> Result<Vec<Message>, SessionStoreError> {
    Ok(strand_messages_with_bounded_topology_in_txn(tx, id, strand, range)?.0)
}

fn strand_messages_with_bounded_topology_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    range: std::ops::Range<u64>,
) -> Result<(Vec<Message>, usize), SessionStoreError> {
    let (head, stored_token) =
        head_row_in_txn(tx, id)?.ok_or_else(|| SessionStoreError::NotFound(id.clone()))?;
    if session_head_cas_token(&head)? != stored_token {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let hop_budget = ordinary_topology_hop_budget(&head, 0)?;
    let links =
        bounded_strand_links_for_roots_in_txn(tx, id, std::slice::from_ref(strand), hop_budget)?;
    let loaded_link_rows = links.len();
    let messages = strand_messages_with_links_in_txn(tx, id, strand, range, &links)?;
    Ok((messages, loaded_link_rows))
}

fn strand_messages_with_links_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    range: std::ops::Range<u64>,
    links: &StrandLinks,
) -> Result<Vec<Message>, SessionStoreError> {
    resolve_strand_bytes_in_txn(tx, id, strand, range, links)?
        .into_iter()
        .map(|bytes| {
            serde_json::from_slice::<Message>(&bytes)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))
        })
        .collect()
}

/// Append rows with the trait's contiguity/idempotency contract: base_seq
/// must not exceed the current row count; overlapping rows must be
/// byte-identical; shrink is structurally inexpressible.
fn insert_strand_rows_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    base_seq: u64,
    messages: &[Message],
) -> Result<(), SessionStoreError> {
    let hop_budget = usize::try_from(SESSION_ROW_LINEAGE_REBASE_INTERVAL)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let links =
        bounded_strand_links_for_roots_in_txn(tx, id, std::slice::from_ref(strand), hop_budget)?;
    if let Some(link) = links.get(strand.as_str()) {
        // A superseded strand is history: its rows live as a splice of its
        // successor and are immutable. Re-writing the identical bytes stays
        // idempotent (crash-retry); extending or diverging is exactly the
        // continuity violation the append contract names.
        let serialized: Vec<Vec<u8>> = messages
            .iter()
            .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
            .collect::<Result<_, _>>()?;
        let end = base_seq.saturating_add(serialized.len() as u64);
        let divergence = |detail: String| SessionStoreError::TranscriptContinuityViolation {
            id: id.clone(),
            previous_revision: format!("superseded-strand:{strand}"),
            incoming_revision: format!("append-base-seq:{base_seq}"),
            reason: detail,
        };
        if end > link.splice.strand_len {
            return Err(divergence(format!(
                "append would extend strand {strand}, which was superseded by {} at \
                 {} rows",
                link.successor, link.splice.strand_len
            )));
        }
        let stored = resolve_strand_bytes_in_txn(tx, id, strand, base_seq..end, &links)?;
        if stored != serialized {
            return Err(divergence(format!(
                "append would overwrite superseded strand {strand} with different bytes"
            )));
        }
        return Ok(());
    }
    let existing = materialized_row_count_in_txn(tx, id, strand)?;
    if base_seq > existing {
        return Err(SessionStoreError::TranscriptContinuityViolation {
            id: id.clone(),
            previous_revision: format!("strand-rows:{existing}"),
            incoming_revision: format!("append-base-seq:{base_seq}"),
            reason: format!(
                "append at base_seq {base_seq} would leave a gap in strand {strand} with {existing} rows"
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
                        "append would overwrite immutable row (strand {strand}, seq {}) with different bytes",
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
            "INSERT INTO session_strand_messages (session_id, strand, seq, message_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id.to_string(), strand.as_str(), seq_i64, bytes, created_at_ms],
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    }
    Ok(())
}

fn rewrite_row_count_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<u64, SessionStoreError> {
    let count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM session_rewrites WHERE session_id = ?1",
            params![id.to_string()],
            |row| row.get(0),
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    u64::try_from(count).map_err(|_| SessionStoreError::Corrupted(id.clone()))
}

struct RewriteRow {
    commit: TranscriptRewriteCommit,
    parent_strand: TranscriptStrandId,
    parent_len: u64,
    strand: TranscriptStrandId,
    strand_len: u64,
    /// Strict current compact-edge wire. Released 0.8.10 rows predate this
    /// column and remain `None` before the head's explicit lineage anchor.
    graph_edge_json: Option<Vec<u8>>,
}

/// Adopted rewrite rows (`rewrite_idx < max_idx_exclusive`), oldest first.
fn rewrite_rows_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    max_idx_exclusive: u64,
) -> Result<Vec<RewriteRow>, SessionStoreError> {
    let limit =
        i64::try_from(max_idx_exclusive).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    rewrite_rows_up_to_in_txn(tx, id, Some(limit))
}

/// Exact persisted rewrite tail `[min_idx_inclusive, max_idx_exclusive)`.
///
/// Activation uses this one-time read to prove that the physical head's
/// rewrite authority is an exact descendant of the frozen runtime boundary.
/// Returning the stored indices prevents a corrupt/gapped table from being
/// mistaken for a contiguous accumulator tail.
fn indexed_rewrite_rows_range_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    min_idx_inclusive: u64,
    max_idx_exclusive: u64,
) -> Result<Vec<(u64, RewriteRow)>, SessionStoreError> {
    let min_idx =
        i64::try_from(min_idx_inclusive).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let max_idx =
        i64::try_from(max_idx_exclusive).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let mut stmt = tx
        .prepare(
            "SELECT rewrite_idx, commit_json, parent_strand, parent_len, strand, strand_len,
                    graph_edge_json
             FROM session_rewrites
             WHERE session_id = ?1 AND rewrite_idx >= ?2 AND rewrite_idx < ?3
             ORDER BY rewrite_idx ASC",
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let rows = stmt
        .query_map(params![id.to_string(), min_idx, max_idx], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<JsonColumnBytes>>(6)?
                    .map(JsonColumnBytes::into_bytes),
            ))
        })
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    rows.into_iter()
        .map(
            |(
                rewrite_idx,
                commit_json,
                parent_strand,
                parent_len,
                strand,
                strand_len,
                graph_edge_json,
            )| {
                let rewrite_idx = u64::try_from(rewrite_idx)
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
                let commit: TranscriptRewriteCommit = serde_json::from_slice(&commit_json)
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
                Ok((
                    rewrite_idx,
                    RewriteRow {
                        commit,
                        parent_strand: TranscriptStrandId::from_persisted(parent_strand),
                        parent_len: u64::try_from(parent_len)
                            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                        strand: TranscriptStrandId::from_persisted(strand),
                        strand_len: u64::try_from(strand_len)
                            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                        graph_edge_json,
                    },
                ))
            },
        )
        .collect()
}

/// Prove that `descendant` extends `ancestor` by the exact persisted rewrite
/// occurrence rows between their generation counters.
#[doc(hidden)]
pub fn verify_head_rewrite_prefix_descent_in_txn(
    tx: &Transaction<'_>,
    ancestor: &SessionHead,
    descendant: &SessionHead,
) -> Result<(), SessionStoreError> {
    if ancestor.id != descendant.id || ancestor.rewrite_count > descendant.rewrite_count {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: descendant.id.clone(),
            reason: "rewrite-prefix descent heads are unrelated or reversed".to_string(),
        });
    }
    let indexed = indexed_rewrite_rows_range_in_txn(
        tx,
        &descendant.id,
        ancestor.rewrite_count,
        descendant.rewrite_count,
    )?;
    let mut prefix = ancestor.rewrite_prefix.clone();
    for (offset, (stored_idx, row)) in indexed.iter().enumerate() {
        let expected_idx = ancestor
            .rewrite_count
            .checked_add(
                u64::try_from(offset)
                    .map_err(|_| SessionStoreError::Corrupted(descendant.id.clone()))?,
            )
            .ok_or_else(|| SessionStoreError::Corrupted(descendant.id.clone()))?;
        if *stored_idx != expected_idx {
            return Err(SessionStoreError::Corrupted(descendant.id.clone()));
        }
        prefix = prefix
            .extend(&row.commit)
            .map_err(SessionStoreError::from)?;
    }
    if prefix != descendant.rewrite_prefix {
        return Err(SessionStoreError::TranscriptContinuityViolation {
            id: descendant.id.clone(),
            previous_revision: ancestor.rewrite_prefix.digest().to_string(),
            incoming_revision: descendant.rewrite_prefix.digest().to_string(),
            reason: "persisted rewrite rows do not bridge runtime and physical prefix authority"
                .to_string(),
        });
    }
    Ok(())
}

fn rewrite_rows_up_to_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    limit: Option<i64>,
) -> Result<Vec<RewriteRow>, SessionStoreError> {
    let mut stmt = tx
        .prepare(
            "SELECT commit_json, parent_strand, parent_len, strand, strand_len,
                    graph_edge_json
             FROM session_rewrites
             WHERE session_id = ?1 AND (?2 IS NULL OR rewrite_idx < ?2)
             ORDER BY rewrite_idx ASC",
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let rows = stmt
        .query_map(params![id.to_string(), limit], |row| {
            Ok((
                row.get::<_, JsonColumnBytes>(0)?.into_bytes(),
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<JsonColumnBytes>>(5)?
                    .map(JsonColumnBytes::into_bytes),
            ))
        })
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    rows.into_iter()
        .map(
            |(commit_json, parent_strand, parent_len, strand, strand_len, graph_edge_json)| {
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
                    graph_edge_json,
                })
            },
        )
        .collect()
}

fn insert_rewrite_row_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    rewrite_idx: u64,
    row: &RewriteRow,
) -> Result<(), SessionStoreError> {
    let commit_json = serde_json::to_vec(&row.commit).map_err(SessionStoreError::from)?;
    let idx = i64::try_from(rewrite_idx).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let parent_len =
        i64::try_from(row.parent_len).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let strand_len =
        i64::try_from(row.strand_len).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let graph_edge_json = row
        .graph_edge_json
        .as_deref()
        .ok_or_else(|| legacy_head_canonical_rewrite_error(id))?;
    // Replacing an unadopted row abandons its child strand outright — a full
    // transcript of rows that no verb can reach once the row that named them
    // is gone. Collect them here rather than waiting for a head write.
    let superseded_child: Option<String> = tx
        .query_row(
            "SELECT strand FROM session_rewrites WHERE session_id = ?1 AND rewrite_idx = ?2",
            params![id.to_string(), idx],
            |existing| existing.get::<_, String>(0),
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .filter(|existing| existing != row.strand.as_str());
    tx.execute(
        "INSERT OR REPLACE INTO session_rewrites
             (session_id, rewrite_idx, parent_strand, parent_len, strand, strand_len,
              commit_json, graph_edge_json, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id.to_string(),
            idx,
            row.parent_strand.as_str(),
            parent_len,
            row.strand.as_str(),
            strand_len,
            commit_json,
            graph_edge_json,
            now_millis(),
        ],
    )
    .map_err(StoreError::from)
    .map_err(into_session_store_error)?;
    if let Some(abandoned) = superseded_child {
        let still_referenced: i64 = tx
            .query_row(
                "SELECT (SELECT COUNT(*) FROM session_rewrites
                          WHERE session_id = ?1 AND (parent_strand = ?2 OR strand = ?2))
                      + (SELECT COUNT(*) FROM session_heads
                          WHERE session_id = ?1 AND strand = ?2)
                      + (SELECT COUNT(*) FROM session_strand_links
                          WHERE session_id = ?1 AND successor = ?2)",
                params![id.to_string(), abandoned],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?;
        if still_referenced == 0 {
            for sql in [
                "DELETE FROM session_strand_messages WHERE session_id = ?1 AND strand = ?2",
                "DELETE FROM session_strand_links WHERE session_id = ?1 AND strand = ?2",
            ] {
                tx.execute(sql, params![id.to_string(), abandoned])
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
            }
        }
    }
    Ok(())
}

/// Exact current compact-edge bytes proved by the Session's validated graph.
///
/// Released 0.8.10 WholeBlob ingress validates and normalizes the graph before
/// this store seam runs. The store additionally binds every compact edge to
/// the exact ordered rewrite-prefix authority installed by that importer.
fn validated_graph_edge_bytes_for_session(
    session: &Session,
    expected_rewrite_prefix: &TranscriptRewritePrefixAccumulator,
) -> Result<Vec<Vec<u8>>, SessionStoreError> {
    let history = session
        .validated_transcript_history_state()
        .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!(
                "rewrite-edge migration requires validated compact transcript history: {error}"
            ),
        })?;
    let actual_count = history
        .as_ref()
        .map_or(0, |history| history.state().commit_count());
    if u64::try_from(actual_count).ok() != Some(expected_rewrite_prefix.occurrence_count()) {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!(
                "validated graph carries {actual_count} rewrites but the imported rewrite prefix carries {}",
                expected_rewrite_prefix.occurrence_count()
            ),
        });
    }
    let Some(history) = history else {
        if expected_rewrite_prefix != &TranscriptRewritePrefixAccumulator::empty() {
            return Err(SessionStoreError::Corrupted(session.id().clone()));
        }
        return Ok(Vec::new());
    };
    let mut prefix = TranscriptRewritePrefixAccumulator::empty();
    let mut edges = Vec::with_capacity(actual_count);
    for (index, edge) in history.state().edges().iter().enumerate() {
        let expected_generation = u64::try_from(index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or_else(|| SessionStoreError::Corrupted(session.id().clone()))?;
        prefix = prefix
            .extend(edge.commit())
            .map_err(SessionStoreError::from)?;
        if edge.rewrite_generation() != expected_generation || edge.rewrite_prefix() != &prefix {
            return Err(SessionStoreError::Corrupted(session.id().clone()));
        }
        let bytes = edge.to_replay_bytes().map_err(SessionStoreError::from)?;
        let decoded =
            TranscriptRevisionEdge::from_replay_bytes(&bytes).map_err(SessionStoreError::from)?;
        if &decoded != edge.as_ref() {
            return Err(SessionStoreError::Corrupted(session.id().clone()));
        }
        edges.push(bytes);
    }
    if &prefix != expected_rewrite_prefix {
        return Err(SessionStoreError::Corrupted(session.id().clone()));
    }
    Ok(edges)
}

/// Fill the per-occurrence compact edge column from a validated
/// Session. This runs only in the explicit 0.8.10/WholeBlob conversion
/// transaction; ordinary current writes require the bytes to exist already.
fn backfill_rewrite_graph_edges_from_validated_session_in_txn(
    tx: &Transaction<'_>,
    session: &Session,
    expected_rewrite_prefix: &TranscriptRewritePrefixAccumulator,
) -> Result<(), SessionStoreError> {
    let id = session.id();
    let expected_rewrite_count = expected_rewrite_prefix.occurrence_count();
    let edges = validated_graph_edge_bytes_for_session(session, expected_rewrite_prefix)?;
    let history = session
        .validated_transcript_history_state()
        .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: format!(
                "rewrite-edge migration requires validated compact transcript history: {error}"
            ),
        })?;
    let layout = strand_layout_for_history(session, history.as_ref())?;
    let rows = indexed_rewrite_rows_range_in_txn(tx, id, 0, expected_rewrite_count)?;
    if rows.len() != edges.len() || rows.len() != layout.rewrites.len() {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let mut expected_layout_source = layout.anchor_strand.clone();
    let mut expected_source_len = u64::try_from(layout.serialized_anchor.len())
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let mut expected_released_source = TranscriptStrandId::root();
    for (offset, (((rewrite_idx, row), edge_bytes), expected)) in
        rows.iter().zip(&edges).zip(&layout.rewrites).enumerate()
    {
        let expected_idx =
            u64::try_from(offset).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let edge = TranscriptRevisionEdge::from_replay_bytes(edge_bytes)
            .map_err(SessionStoreError::from)?;
        let layout_edge = validate_blob_rewrite_layout(
            id,
            offset,
            &expected_layout_source,
            expected_source_len,
            expected,
        )?;
        let expected_generation = expected_idx
            .checked_add(1)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if row.commit.rewrite_generation != 0 {
            // Only the released 0.8.10 writer omitted occurrence
            // generations and graph-edge bytes. A current-looking row with
            // a missing edge has no authorized migration provenance.
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let mut normalized_commit = row.commit.clone();
        normalized_commit.rewrite_generation = expected_generation;
        let expected_parent_len = expected
            .parent_base_seq
            .checked_add(
                u64::try_from(expected.serialized_parent_suffix.len())
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            )
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        // Released 0.8.10 HeadCanonical rows predate occurrence-scoped
        // strand identities. Exact-append parents reused the preceding
        // revision/root id, non-append parents used rebase:{digest}, and
        // children used the revision digest itself. Validate that released
        // physical vocabulary here; current compact migrations never enter
        // this backfill because their edge bytes are written atomically.
        let expected_released_parent = if edge.parent_advance().exact_splice().is_some() {
            TranscriptStrandId::rebase(&normalized_commit.parent_revision)
        } else {
            expected_released_source.clone()
        };
        let expected_released_child = TranscriptStrandId::from_rewrite(&normalized_commit);
        if *rewrite_idx != expected_idx
            || normalized_commit.rewrite_generation != expected_generation
            || normalized_commit != *edge.commit()
            || edge != layout_edge
            || normalized_commit != expected.commit
            || row.parent_strand != expected_released_parent
            || row.parent_len != expected_parent_len
            || row.strand != expected_released_child
            || row.strand_len != expected.link_splice.strand_len
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        if row.commit != normalized_commit {
            let commit_json =
                serde_json::to_vec(&normalized_commit).map_err(SessionStoreError::from)?;
            let changed = tx
                .execute(
                    "UPDATE session_rewrites
                     SET commit_json = ?3
                     WHERE session_id = ?1 AND rewrite_idx = ?2",
                    params![
                        id.to_string(),
                        i64::try_from(*rewrite_idx)
                            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                        commit_json,
                    ],
                )
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            if changed != 1 {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
        }
        match row.graph_edge_json.as_deref() {
            Some(stored) if stored == edge_bytes.as_slice() => {}
            Some(_) => return Err(SessionStoreError::Corrupted(id.clone())),
            None => {
                let changed = tx
                    .execute(
                        "UPDATE session_rewrites
                         SET graph_edge_json = ?3
                         WHERE session_id = ?1 AND rewrite_idx = ?2
                           AND graph_edge_json IS NULL",
                        params![
                            id.to_string(),
                            i64::try_from(*rewrite_idx)
                                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                            edge_bytes,
                        ],
                    )
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                if changed != 1 {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
            }
        }
        expected_layout_source = expected.strand.clone();
        expected_released_source = expected_released_child;
        expected_source_len = expected.link_splice.strand_len;
    }
    Ok(())
}

fn post_anchor_rewrite_graph_edge_is_missing_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    start_rewrite_count: u64,
    end_rewrite_count: u64,
) -> Result<bool, SessionStoreError> {
    let start_rewrite_count =
        i64::try_from(start_rewrite_count).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let end_rewrite_count =
        i64::try_from(end_rewrite_count).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    tx.query_row(
        "SELECT 1 FROM session_rewrites
         WHERE session_id = ?1 AND rewrite_idx >= ?2 AND rewrite_idx < ?3
           AND graph_edge_json IS NULL
         LIMIT 1",
        params![id.to_string(), start_rewrite_count, end_rewrite_count],
        |_row| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .map_err(StoreError::from)
    .map_err(into_session_store_error)
}

fn validate_blob_parent_splice(
    id: &SessionId,
    parent_splice: &PreparedHeadCanonicalParentSplice,
    expected_source: &TranscriptStrandId,
    expected_bridge: &TranscriptStrandId,
    observed_bridge: &TranscriptStrandId,
    expected_source_len: u64,
) -> Result<(), SessionStoreError> {
    let splice = parent_splice.link_splice();
    let replacement_len = u64::try_from(parent_splice.serialized_replacement().len())
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    if parent_splice.source_strand() != expected_source
        || observed_bridge != expected_bridge
        || observed_bridge == expected_source
        || replacement_len == 0
        || !splice.is_well_formed()
        || splice.strand_len != expected_source_len
        || splice.splice_end != splice.successor_end
        || splice.successor_len() != expected_source_len
        || splice.retained_rows() != replacement_len
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(())
}

fn validate_blob_rewrite_layout(
    id: &SessionId,
    rewrite_index: usize,
    expected_source: &TranscriptStrandId,
    expected_source_len: u64,
    rewrite: &StrandRewriteLayout,
) -> Result<TranscriptRevisionEdge, SessionStoreError> {
    let expected_generation = u64::try_from(rewrite_index)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?
        .checked_add(1)
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    let edge = TranscriptRevisionEdge::from_replay_bytes(&rewrite.serialized_graph_edge)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let canonical_edge = edge.to_replay_bytes().map_err(SessionStoreError::from)?;
    let parent_suffix =
        serialized_messages_for_lineage_replay(id, edge.parent_advance().appended())?;
    let replacement = serialized_messages_for_lineage_replay(id, edge.rewrite().replacement())?;
    let messages_before_base = u64::try_from(edge.messages_before_base())
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let messages_before = u64::try_from(edge.messages_before())
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let messages_after = u64::try_from(edge.messages_after())
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let (selection_start, selection_end) = rewrite.commit.selection.bounds();
    let selection_start =
        u64::try_from(selection_start).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let selection_end =
        u64::try_from(selection_end).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let replacement_len =
        u64::try_from(replacement.len()).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let expected_splice_end = selection_start
        .checked_add(replacement_len)
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    let parent_len = rewrite
        .parent_base_seq
        .checked_add(
            u64::try_from(rewrite.serialized_parent_suffix.len())
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
        )
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    let expected_child = TranscriptStrandId::from_rewrite_occurrence(&rewrite.commit);
    if canonical_edge != rewrite.serialized_graph_edge
        || edge.commit() != &rewrite.commit
        || edge.rewrite_generation() != expected_generation
        || rewrite.commit.rewrite_generation != expected_generation
        || messages_before_base != expected_source_len
        || rewrite.parent_base_seq != expected_source_len
        || parent_suffix != rewrite.serialized_parent_suffix
        || replacement != rewrite.serialized_replacement
        || parent_len != messages_before
        || rewrite.strand != expected_child
        || rewrite.strand == rewrite.parent_strand
        || !rewrite.link_splice.is_well_formed()
        || rewrite.link_splice.successor_len() != messages_before
        || rewrite.link_splice.strand_len != messages_after
        || rewrite.link_splice.splice_start != selection_start
        || rewrite.link_splice.splice_end != expected_splice_end
        || rewrite.link_splice.successor_end != selection_end
        || rewrite.link_splice.retained_rows() != replacement_len
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    match (
        &rewrite.parent_transition,
        edge.parent_advance().exact_splice(),
    ) {
        (PreparedHeadCanonicalParentTransition::ExactAppend, None) => {
            if rewrite.parent_strand != *expected_source {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
        }
        (
            PreparedHeadCanonicalParentTransition::ExactSplice(parent_splice),
            Some((at, replacement)),
        ) => {
            let expected_bridge =
                TranscriptStrandId::from_rewrite_parent_occurrence(&rewrite.commit);
            validate_blob_parent_splice(
                id,
                parent_splice,
                expected_source,
                &expected_bridge,
                &rewrite.parent_strand,
                expected_source_len,
            )?;
            let serialized = serialized_messages_for_lineage_replay(id, replacement)?;
            let splice = parent_splice.link_splice();
            let expected_start =
                u64::try_from(at).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            let expected_end = expected_start
                .checked_add(
                    u64::try_from(replacement.len())
                        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                )
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            if serialized != parent_splice.serialized_replacement()
                || splice.splice_start != expected_start
                || splice.splice_end != expected_end
            {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
        }
        _ => return Err(SessionStoreError::Corrupted(id.clone())),
    }
    Ok(edge)
}

fn layout_for_blob_session(
    session: &Session,
) -> Result<(StrandLayout, SessionHead), SessionStoreError> {
    let id = session.id();
    let history = session
        .validated_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: format!("stored transcript history state is malformed: {err}"),
        })?;
    let layout = strand_layout_for_history(session, history.as_ref())?;
    if layout.anchor_strand != TranscriptStrandId::root()
        || layout.head_len
            != u64::try_from(session.messages().len())
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?
        || history
            .as_ref()
            .map_or(0, |proved| proved.state().commit_count())
            != layout.rewrites.len()
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let mut row_lineage =
        SessionMessageRowPrefixAccumulator::from_serialized_rows(&layout.serialized_anchor)?;
    let mut current_strand = layout.anchor_strand.clone();
    let mut current_count = u64::try_from(layout.serialized_anchor.len())
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let mut rewrite_prefix = TranscriptRewritePrefixAccumulator::empty();
    for (index, rewrite) in layout.rewrites.iter().enumerate() {
        let edge =
            validate_blob_rewrite_layout(id, index, &current_strand, current_count, rewrite)?;
        match &rewrite.parent_transition {
            PreparedHeadCanonicalParentTransition::ExactAppend => {}
            PreparedHeadCanonicalParentTransition::ExactSplice(parent_splice) => {
                let splice = parent_splice.link_splice();
                row_lineage = row_lineage.replace_serialized_range(
                    splice.splice_start,
                    splice.splice_end,
                    parent_splice.serialized_replacement(),
                )?;
                current_strand = rewrite.parent_strand.clone();
            }
        }
        row_lineage = row_lineage.extend_serialized_rows(&rewrite.serialized_parent_suffix)?;
        let parent_count = rewrite
            .parent_base_seq
            .checked_add(
                u64::try_from(rewrite.serialized_parent_suffix.len())
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            )
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if parent_count != rewrite.link_splice.successor_len()
            || row_lineage != *edge.parent_row_prefix()
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        row_lineage = row_lineage.replace_serialized_range(
            rewrite.link_splice.splice_start,
            rewrite.link_splice.successor_end,
            &rewrite.serialized_replacement,
        )?;
        rewrite_prefix = rewrite_prefix
            .extend(&rewrite.commit)
            .map_err(SessionStoreError::from)?;
        if row_lineage.row_count() != rewrite.link_splice.strand_len
            || row_lineage != *edge.result_witness().row_prefix()
            || rewrite_prefix != *edge.rewrite_prefix()
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        current_strand = rewrite.strand.clone();
        current_count = rewrite.link_splice.strand_len;
    }
    if layout.tail_base_seq != current_count {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    if layout.head_strand != current_strand {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    row_lineage = row_lineage.extend_serialized_rows(&layout.serialized_tail)?;
    if row_lineage.row_count() != layout.head_len {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let proved_rewrite_prefix = session
        .transcript_rewrite_prefix_authority()
        .unwrap_or_else(TranscriptRewritePrefixAccumulator::empty);
    if proved_rewrite_prefix.occurrence_count()
        != u64::try_from(layout.rewrites.len())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?
        || proved_rewrite_prefix != rewrite_prefix
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let head = SessionHead::from_session_with_proved_inline_storage_authority(
        session,
        layout.head_strand.clone(),
        proved_rewrite_prefix,
        row_lineage,
    )?;
    Ok((layout, head))
}

fn persist_blob_strand_layout_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    layout: &StrandLayout,
) -> Result<(), SessionStoreError> {
    reconcile_serialized_strand_rows_in_txn(
        tx,
        id,
        &layout.anchor_strand,
        0,
        &layout.serialized_anchor,
    )?;
    let mut expected_source = layout.anchor_strand.clone();
    let mut expected_source_len = u64::try_from(layout.serialized_anchor.len())
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    for (index, rewrite) in layout.rewrites.iter().enumerate() {
        if rewrite.parent_base_seq != expected_source_len {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        match &rewrite.parent_transition {
            PreparedHeadCanonicalParentTransition::ExactAppend => {
                if rewrite.parent_strand != expected_source {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
            }
            PreparedHeadCanonicalParentTransition::ExactSplice(parent_splice) => {
                if parent_splice.source_strand() != &expected_source
                    || rewrite.parent_strand
                        != TranscriptStrandId::from_rewrite_parent_occurrence(&rewrite.commit)
                {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
                reconcile_exact_strand_link_in_txn(
                    tx,
                    id,
                    &rewrite.parent_strand,
                    parent_splice.source_strand(),
                    parent_splice.link_splice(),
                )?;
                reconcile_serialized_strand_rows_in_txn(
                    tx,
                    id,
                    &rewrite.parent_strand,
                    parent_splice.link_splice().splice_start,
                    parent_splice.serialized_replacement(),
                )?;
            }
        }
        reconcile_serialized_strand_rows_in_txn(
            tx,
            id,
            &rewrite.parent_strand,
            rewrite.parent_base_seq,
            &rewrite.serialized_parent_suffix,
        )?;
        let parent_len = rewrite
            .parent_base_seq
            .checked_add(
                u64::try_from(rewrite.serialized_parent_suffix.len())
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            )
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if parent_len != rewrite.link_splice.successor_len()
            || rewrite.link_splice.strand_len
                != u64::try_from(rewrite.commit.messages_after)
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?
            || rewrite.link_splice.retained_rows()
                != u64::try_from(rewrite.serialized_replacement.len())
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        insert_rewrite_row_in_txn(
            tx,
            id,
            u64::try_from(index).map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            &RewriteRow {
                commit: rewrite.commit.clone(),
                parent_strand: rewrite.parent_strand.clone(),
                parent_len,
                strand: rewrite.strand.clone(),
                strand_len: rewrite.link_splice.strand_len,
                graph_edge_json: Some(rewrite.serialized_graph_edge.clone()),
            },
        )?;
        reconcile_exact_strand_link_in_txn(
            tx,
            id,
            &rewrite.strand,
            &rewrite.parent_strand,
            rewrite.link_splice,
        )?;
        reconcile_serialized_strand_rows_in_txn(
            tx,
            id,
            &rewrite.strand,
            rewrite.link_splice.splice_start,
            &rewrite.serialized_replacement,
        )?;
        expected_source = rewrite.strand.clone();
        expected_source_len = rewrite.link_splice.strand_len;
    }
    if layout.tail_base_seq != expected_source_len {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    if layout.head_strand != expected_source {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    reconcile_serialized_strand_rows_in_txn(
        tx,
        id,
        &layout.head_strand,
        layout.tail_base_seq,
        &layout.serialized_tail,
    )?;
    let final_count = layout
        .tail_base_seq
        .checked_add(
            u64::try_from(layout.serialized_tail.len())
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
        )
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    if final_count != layout.head_len {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(())
}

fn blob_strand_messages(
    session: &Session,
    layout: &StrandLayout,
    strand: &TranscriptStrandId,
) -> Result<Vec<Message>, SessionStoreError> {
    if strand == &layout.head_strand {
        return Ok(session.messages().to_vec());
    }
    let history = session
        .validated_transcript_history_state()
        .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("stored transcript history state is malformed: {error}"),
        })?;
    let Some(history) = history else {
        return Err(SessionStoreError::Corrupted(session.id().clone()));
    };

    // A physical strand is extended only while it remains the exact parent
    // of the next occurrence. Prefer that maximal parent body over the
    // shorter child endpoint when both names address the same strand.
    if let Some(rewrite) = layout
        .rewrites
        .iter()
        .find(|rewrite| &rewrite.parent_strand == strand)
    {
        return history
            .materialize_rewrite_parent(&rewrite.commit)
            .map(|body| body.messages)
            .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!("failed to materialize blob rewrite parent: {error}"),
            });
    }
    if let Some(rewrite) = layout
        .rewrites
        .iter()
        .find(|rewrite| &rewrite.strand == strand)
    {
        return history
            .materialize_rewrite_child(&rewrite.commit)
            .map(|body| body.messages)
            .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!("failed to materialize blob rewrite child: {error}"),
            });
    }
    if strand == &layout.anchor_strand {
        return Ok(history.state().anchor().messages().to_vec());
    }
    Err(SessionStoreError::Corrupted(session.id().clone()))
}

fn activate_realtime_component_in_txn(
    tx: &Transaction<'_>,
    session: &mut Session,
) -> Result<(), SessionStoreError> {
    // Activation is a representation migration only. The typed component event
    // suffix and its derived root are the authority; the caller binds that root
    // into the upgraded SessionHead in this transaction.
    session
        .activate_realtime_component_sidecar()
        .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("failed to activate realtime component: {error}"),
        })?;
    if let Some(suffix) = session
        .prepare_realtime_component_event_suffix()
        .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("failed to seal activated realtime component: {error}"),
        })?
    {
        reconcile_prepared_component_suffix_in_txn(tx, &suffix)?;
    }
    Ok(())
}

/// One-time migration: lay out the legacy blob's strands and head inside the
/// caller's transaction. The blob row is left untouched as a frozen archive
/// and is never read again once the head row exists.
fn migrate_legacy_blob_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<Option<(SessionHead, String)>, SessionStoreError> {
    let Some(session) = load_session_snapshot_in_txn(tx, id).map_err(into_session_store_error)?
    else {
        return Ok(None);
    };
    let (layout, head) = layout_for_blob_session(&session)?;
    persist_blob_strand_layout_in_txn(tx, id, &layout)?;
    // Blob activation is the explicit one-time full-history path. Settle its
    // current store-issued anchor before publishing the canonical head so all
    // subsequent ordinary resumes start from direct rows regardless of how
    // many historical overlay edges the released blob contained.
    let links = strand_links_in_txn(tx, id)?;
    validate_strand_links_acyclic(id, &links)?;
    let settled =
        settle_resolved_strand_direct_in_txn(tx, id, &head.strand, head.message_count, &links)?;
    let expected = session
        .messages()
        .iter()
        .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
        .collect::<Result<Vec<_>, _>>()?;
    if settled != expected {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let token = write_head_row_only_in_txn(tx, &head)?;
    Ok(Some((head, token)))
}

/// Head row if present; otherwise migrate a legacy blob in this transaction
/// (the first incremental WRITE migrates; reads synthesize without writing).
fn ensure_head_canonical_for_write_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<Option<(SessionHead, String)>, SessionStoreError> {
    if let Some((head, stored_token)) = head_row_in_txn(tx, id)? {
        if session_head_cas_token(&head)? != stored_token {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let replay_start = head
            .row_lineage_anchor
            .as_ref()
            .map_or(0, |anchor| anchor.rewrite_count());
        let missing_graph_edge = post_anchor_rewrite_graph_edge_is_missing_in_txn(
            tx,
            id,
            replay_start,
            head.rewrite_count,
        )?;
        if head.message_row_prefix.is_some()
            && head.row_lineage_anchor.is_some()
            && !missing_graph_edge
        {
            return Ok(Some((head, stored_token)));
        }
        if head.realtime_event_prefix.is_some() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        if head.row_lineage_anchor.as_ref().is_some_and(|anchor| {
            anchor.rewrite_count() != head.rewrite_count
                || anchor.strand() != &head.strand
                || anchor.message_count() != head.message_count
        }) {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        // Released 0.8.10 canonical heads carried only the semantic transcript
        // digest, or no current replay anchor/compact edge column. Verify its
        // exact rows and validated compact graph, backfill every adopted edge,
        // and seed the current lineage anchor in this one transaction.
        let links = strand_links_in_txn(tx, id)?;
        validate_strand_links_acyclic(id, &links)?;
        let serialized_rows =
            resolve_strand_bytes_in_txn(tx, id, &head.strand, 0..head.message_count, &links)?;
        let session = head
            .clone()
            .into_session_from_serialized_rows(serialized_rows.clone())?;
        let proved_rewrite_prefix = session
            .transcript_rewrite_prefix_authority()
            .unwrap_or_else(TranscriptRewritePrefixAccumulator::empty);
        if proved_rewrite_prefix.occurrence_count() != head.rewrite_count
            || (head.rewrite_prefix.occurrence_count() == head.rewrite_count
                && head.rewrite_prefix != proved_rewrite_prefix)
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        backfill_rewrite_graph_edges_from_validated_session_in_txn(
            tx,
            &session,
            &proved_rewrite_prefix,
        )?;
        let exact_prefix =
            SessionMessageRowPrefixAccumulator::from_serialized_rows(&serialized_rows)?;
        let upgraded = SessionHead::from_session_with_proved_inline_storage_authority(
            &session,
            head.strand,
            proved_rewrite_prefix,
            exact_prefix,
        )?;
        let settled = settle_resolved_strand_direct_in_txn(
            tx,
            id,
            &upgraded.strand,
            upgraded.message_count,
            &links,
        )?;
        if settled != serialized_rows {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let upgraded_token = write_head_row_only_in_txn(tx, &upgraded)?;
        return Ok(Some((upgraded, upgraded_token)));
    }
    migrate_legacy_blob_in_txn(tx, id)
}

/// Read the exact physical canonical head and stored token in a co-tenant
/// runtime transaction, without materializing message rows.
///
/// The caller owns semantic/runtime-authority comparison. This seam only
/// ensures both facts come from the same transaction snapshot and never
/// synthesizes or migrates a missing head.
#[doc(hidden)]
pub fn load_head_canonical_for_runtime_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<Option<(SessionHead, String)>, SessionStoreError> {
    let current = head_row_in_txn(tx, id)?;
    if let Some((head, stored_token)) = current.as_ref()
        && session_head_cas_token(head)? != *stored_token
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    if let Some((head, _)) = current.as_ref()
        && head.message_row_prefix.is_none()
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: "canonical head predates exact message-row authority; explicit conversion has not completed"
                .to_string(),
        });
    }
    if let Some((head, _)) = current.as_ref()
        && head.realtime_event_prefix.is_none()
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason:
                "canonical head predates the authenticated realtime component root; explicit activation has not completed"
                    .to_string(),
        });
    }
    Ok(current)
}

/// Materialize one exact retained head inside a co-tenant recovery
/// transaction.
///
/// This exceptional seam may resolve and decode `O(document)` strand rows. It
/// exists only so recovery can re-prove a runtime-authorized predecessor head
/// after a newer physical head replaced it; ordinary commits must use the
/// bounded prepared-mutation helper instead.
#[doc(hidden)]
pub fn materialize_runtime_boundary_head_canonical_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
) -> Result<Session, SessionStoreError> {
    Ok(verify_runtime_boundary_head_canonical_in_txn(tx, head)?
        .session()
        .as_ref()
        .clone())
}

/// Materialize the current physical head for an explicit co-tenant runtime
/// check. The caller must separately bind it to the current head row/token in
/// the same transaction.
#[doc(hidden)]
pub fn materialize_physical_head_canonical_for_runtime_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
) -> Result<Session, SessionStoreError> {
    Ok(verify_physical_head_canonical_in_txn(tx, head)?
        .session()
        .as_ref()
        .clone())
}

/// Verify one retained runtime-boundary head against its exact metadata
/// owner reference plus the raw durable row/component vectors.
#[doc(hidden)]
pub fn verify_runtime_boundary_head_canonical_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
) -> Result<meerkat_core::VerifiedSessionHeadMaterialization, SessionStoreError> {
    verify_head_canonical_with_metadata_owner_in_txn(
        tx,
        head,
        HeadMetadataProjectionOwner::RuntimeBoundary,
    )
}

fn verify_physical_head_canonical_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
) -> Result<meerkat_core::VerifiedSessionHeadMaterialization, SessionStoreError> {
    verify_head_canonical_with_metadata_owner_in_txn(
        tx,
        head,
        HeadMetadataProjectionOwner::PhysicalHead,
    )
}

fn serialized_messages_for_lineage_replay(
    id: &SessionId,
    messages: &[Message],
) -> Result<Vec<Vec<u8>>, SessionStoreError> {
    messages
        .iter()
        .map(|message| {
            serde_json::to_vec(message).map_err(|error| {
                SessionStoreError::InvalidTranscriptRewrite {
                    id: id.clone(),
                    reason: format!(
                        "compact rewrite edge contains an unserializable message: {error}"
                    ),
                }
            })
        })
        .collect()
}

/// Verify that one linked strand owns exactly its replacement span plus the
/// direct append tail after the link's immutable endpoint.
///
/// A replacement-only strand is sparse, so `MAX(seq) + 1` may be far below
/// its logical message count. Once a non-empty tail exists, however, its last
/// row must be the logical tip. Keeping this distinction avoids rejecting a
/// healthy empty-tail rewrite while still detecting stray rows beyond a head.
fn verify_linked_strand_physical_shape_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    link: &StrandLinkRow,
    logical_count: u64,
) -> Result<(), SessionStoreError> {
    if logical_count < link.splice.strand_len {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let tail_len = logical_count
        .checked_sub(link.splice.strand_len)
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    let expected_owned_rows = link
        .splice
        .retained_rows()
        .checked_add(tail_len)
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    if materialized_row_count_in_txn(tx, id, strand)? != expected_owned_rows {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let expected_extent = if tail_len > 0 {
        logical_count
    } else if link.splice.retained_rows() > 0 {
        link.splice.splice_end
    } else {
        0
    };
    if physical_row_extent_in_txn(tx, id, strand)? != expected_extent {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(())
}

fn head_has_current_row_lineage_anchor(head: &SessionHead) -> Result<bool, SessionStoreError> {
    let Some(anchor) = head.row_lineage_anchor.as_ref() else {
        return Ok(false);
    };
    let prefix = head
        .message_row_prefix
        .as_ref()
        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    Ok(anchor.rewrite_count() == head.rewrite_count
        && anchor.strand() == &head.strand
        && anchor.message_count() == head.message_count
        && anchor.prefix() == prefix)
}

fn verify_direct_current_anchor_rows_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
) -> Result<(), SessionStoreError> {
    if !head_has_current_row_lineage_anchor(head)?
        || strand_link_in_txn(tx, &head.id, &head.strand)?.is_some()
        || materialized_row_count_in_txn(tx, &head.id, &head.strand)? != head.message_count
        || physical_row_extent_in_txn(tx, &head.id, &head.strand)? != head.message_count
    {
        return Err(SessionStoreError::Corrupted(head.id.clone()));
    }
    let rows = strand_row_bytes_in_txn(tx, &head.id, &head.strand, 0..head.message_count)?;
    let observed = SessionMessageRowPrefixAccumulator::from_serialized_rows(&rows)?;
    let anchor = head
        .row_lineage_anchor
        .as_ref()
        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    if observed != *anchor.materialized_prefix() {
        return Err(SessionStoreError::Corrupted(head.id.clone()));
    }
    Ok(())
}

struct ReplayedHeadCanonicalRows {
    serialized_rows: Vec<Vec<u8>>,
    lineage: meerkat_core::session_store::VerifiedSessionRowLineageReplay,
    anchor_is_current: bool,
    #[cfg(test)]
    decoded_rewrite_count: u64,
    #[cfg(test)]
    loaded_link_row_count: usize,
}

/// Cold-replay one head's exact operation lineage after its bounded anchor.
///
/// The anchor already binds the exact row and rewrite-prefix accumulators proved
/// before it. Ordinary materialization therefore resolves the anchor document
/// once and decodes only post-anchor rewrite edges; pre-anchor history belongs
/// to explicit audit/migration paths. The final logical document is resolved
/// once and semantic verification remains in core.
fn replay_head_canonical_rows_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
) -> Result<ReplayedHeadCanonicalRows, SessionStoreError> {
    let id = &head.id;
    let anchor = head.row_lineage_anchor.as_ref().ok_or_else(|| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: "current canonical head has no bounded row-lineage anchor".to_string(),
        }
    })?;
    let indexed =
        indexed_rewrite_rows_range_in_txn(tx, id, anchor.rewrite_count(), head.rewrite_count)?;
    let post_anchor_rewrite_count = head
        .rewrite_count
        .checked_sub(anchor.rewrite_count())
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    if u64::try_from(indexed.len()).ok() != Some(post_anchor_rewrite_count) {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let mut topology_roots = Vec::with_capacity(
        indexed
            .len()
            .checked_mul(2)
            .and_then(|count| count.checked_add(2))
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?,
    );
    topology_roots.push(anchor.strand().clone());
    topology_roots.push(head.strand.clone());
    for (_, row) in &indexed {
        topology_roots.push(row.parent_strand.clone());
        topology_roots.push(row.strand.clone());
    }
    let hop_budget = ordinary_topology_hop_budget(head, 0)?;
    let links = bounded_strand_links_for_roots_in_txn(tx, id, &topology_roots, hop_budget)?;
    let anchor_rows =
        resolve_strand_bytes_in_txn(tx, id, anchor.strand(), 0..anchor.message_count(), &links)?;
    let observed_anchor = SessionMessageRowPrefixAccumulator::from_serialized_rows(&anchor_rows)?;
    if &observed_anchor != anchor.materialized_prefix() {
        return Err(SessionStoreError::TranscriptContinuityViolation {
            id: id.clone(),
            previous_revision: anchor.materialized_prefix().digest().to_string(),
            incoming_revision: observed_anchor.digest().to_string(),
            reason: "physical anchor rows do not reproduce the sealed lineage origin".to_string(),
        });
    }
    let mut rewrite_prefix = anchor.rewrite_prefix().clone();
    let mut replay = head.begin_row_lineage_replay()?;
    let mut current_strand = anchor.strand().clone();
    let mut current_count = anchor.message_count();
    let mut final_replayed_endpoint = None;
    let mut decoded_rewrite_count = 0_u64;

    for (offset, (rewrite_idx, row)) in indexed.iter().enumerate() {
        let expected_idx = anchor
            .rewrite_count()
            .checked_add(
                u64::try_from(offset).map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            )
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if *rewrite_idx != expected_idx {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        rewrite_prefix = rewrite_prefix
            .extend(&row.commit)
            .map_err(SessionStoreError::from)?;
        let edge_bytes = row
            .graph_edge_json
            .as_deref()
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let edge = TranscriptRevisionEdge::from_replay_bytes(edge_bytes)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        decoded_rewrite_count = decoded_rewrite_count
            .checked_add(1)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let canonical_edge_bytes = edge.to_replay_bytes().map_err(SessionStoreError::from)?;
        if canonical_edge_bytes.as_slice() != edge_bytes
            || edge.commit() != &row.commit
            || edge.rewrite_generation()
                != expected_idx
                    .checked_add(1)
                    .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?
            || edge.rewrite_prefix() != &rewrite_prefix
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let messages_before_base = u64::try_from(edge.messages_before_base())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let messages_before = u64::try_from(edge.messages_before())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let messages_after = u64::try_from(edge.messages_after())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        if messages_before_base != current_count
            || row.parent_len != messages_before
            || row.strand_len != messages_after
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }

        let appended =
            serialized_messages_for_lineage_replay(id, edge.parent_advance().appended())?;
        let parent_count = current_count
            .checked_add(
                u64::try_from(appended.len())
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            )
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if parent_count != messages_before {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }

        if let Some((at, replacement_messages)) = edge.parent_advance().exact_splice() {
            let expected_bridge = TranscriptStrandId::from_rewrite_parent_occurrence(&row.commit);
            let bridge_link = links
                .get(row.parent_strand.as_str())
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            let replacement = serialized_messages_for_lineage_replay(id, replacement_messages)?;
            let splice_start =
                u64::try_from(at).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            let splice_end = splice_start
                .checked_add(
                    u64::try_from(replacement.len())
                        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                )
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            if replacement.is_empty()
                || row.parent_strand != expected_bridge
                || bridge_link.successor != current_strand
                || !bridge_link.splice.is_well_formed()
                || bridge_link.splice.strand_len != current_count
                || bridge_link.splice.splice_start != splice_start
                || bridge_link.splice.splice_end != splice_end
                || bridge_link.splice.successor_end != splice_end
                || bridge_link.splice.successor_len() != current_count
                || bridge_link.splice.retained_rows()
                    != u64::try_from(replacement.len())
                        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?
            {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            verify_serialized_strand_rows_in_txn(
                tx,
                id,
                &row.parent_strand,
                splice_start,
                &replacement,
            )?;
            replay.replace_serialized_range(
                row.parent_strand.clone(),
                splice_start,
                splice_end,
                &replacement,
            )?;
        } else if row.parent_strand != current_strand {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        verify_serialized_strand_rows_in_txn(tx, id, &row.parent_strand, current_count, &appended)?;
        replay.append_serialized_rows(&row.parent_strand, &appended)?;
        if let Some(parent_link) = links.get(row.parent_strand.as_str()) {
            verify_linked_strand_physical_shape_in_txn(
                tx,
                id,
                &row.parent_strand,
                parent_link,
                parent_count,
            )?;
        }

        let (selection_start, selection_end) = row.commit.selection.bounds();
        let selection_start =
            u64::try_from(selection_start).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let selection_end =
            u64::try_from(selection_end).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let rewrite_at = u64::try_from(edge.rewrite().at())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let replacement = serialized_messages_for_lineage_replay(id, edge.rewrite().replacement())?;
        let replacement_len = u64::try_from(replacement.len())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let child_link = links
            .get(row.strand.as_str())
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let expected_child = TranscriptStrandId::from_rewrite_occurrence(&row.commit);
        if row.strand != expected_child
            || row.strand == row.parent_strand
            || rewrite_at != selection_start
            || child_link.successor != row.parent_strand
            || !child_link.splice.is_well_formed()
            || child_link.splice.strand_len != messages_after
            || child_link.splice.successor_len() != messages_before
            || child_link.splice.splice_start != selection_start
            || child_link.splice.successor_end != selection_end
            || child_link.splice.retained_rows() != replacement_len
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        verify_serialized_strand_rows_in_txn(
            tx,
            id,
            &row.strand,
            child_link.splice.splice_start,
            &replacement,
        )?;
        replay.apply_rewrite_edge(row.strand.clone(), &edge)?;
        current_strand = row.strand.clone();
        current_count = messages_after;
        final_replayed_endpoint = Some(messages_after);
    }

    if rewrite_prefix != head.rewrite_prefix
        || current_strand != head.strand
        || current_count > head.message_count
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let tail = strand_row_bytes_in_txn(tx, id, &head.strand, current_count..head.message_count)?;
    replay.append_serialized_rows(&head.strand, &tail)?;
    match links.get(head.strand.as_str()) {
        Some(link) => {
            if final_replayed_endpoint.is_some_and(|endpoint| link.splice.strand_len != endpoint) {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            verify_linked_strand_physical_shape_in_txn(
                tx,
                id,
                &head.strand,
                link,
                head.message_count,
            )?;
        }
        None => {
            if final_replayed_endpoint.is_some()
                || materialized_row_count_in_txn(tx, id, &head.strand)? != head.message_count
                || physical_row_extent_in_txn(tx, id, &head.strand)? != head.message_count
            {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
        }
    }
    let lineage = replay.finish(head)?;
    let anchor_is_current = head_has_current_row_lineage_anchor(head)?;
    let serialized_rows = if anchor_is_current {
        anchor_rows
    } else {
        resolve_strand_bytes_in_txn(tx, id, &head.strand, 0..head.message_count, &links)?
    };
    tracing::trace!(
        session_id = %id,
        pre_anchor_rewrite_count = anchor.rewrite_count(),
        decoded_rewrite_count,
        loaded_link_row_count = links.len(),
        "verified HeadCanonical rows from bounded lineage anchor"
    );
    Ok(ReplayedHeadCanonicalRows {
        serialized_rows,
        lineage,
        anchor_is_current,
        #[cfg(test)]
        decoded_rewrite_count,
        #[cfg(test)]
        loaded_link_row_count: links.len(),
    })
}

fn verify_head_canonical_with_metadata_owner_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
    owner: HeadMetadataProjectionOwner,
) -> Result<meerkat_core::VerifiedSessionHeadMaterialization, SessionStoreError> {
    let mut head = head.clone();
    attach_head_metadata_projection(tx, &mut head, owner)?;
    let replayed = replay_head_canonical_rows_in_txn(tx, &head)?;
    match head.realtime_event_prefix.as_ref() {
        Some(realtime) => {
            let realtime = load_verified_component_sequence_in_txn(tx, realtime)?;
            head.verify_serialized_rows_with_component_sequences_and_lineage(
                replayed.serialized_rows,
                realtime,
                replayed.lineage,
            )
        }
        None if replayed.anchor_is_current => head.verify_serialized_rows(replayed.serialized_rows),
        _ => Err(SessionStoreError::Corrupted(head.id.clone())),
    }
}

/// Independently replay the retained runtime boundary and physical head.
///
/// A rewrite may replace the middle of the transcript or shrink it, so the
/// boundary is not necessarily a flat prefix of the physical document.
/// Recovery instead proves both exact heads from their own anchors, proves the
/// physical rewrite occurrence prefix descends from the boundary, and then
/// installs the independently verified boundary row token on the physical
/// materialization for the runtime handoff.
#[doc(hidden)]
pub fn verify_physical_head_retains_boundary_prefix_for_runtime_in_txn(
    tx: &Transaction<'_>,
    boundary_head: &SessionHead,
    physical_head: &SessionHead,
) -> Result<meerkat_core::VerifiedSessionHeadMaterialization, SessionStoreError> {
    if boundary_head.id != physical_head.id
        || boundary_head.version != physical_head.version
        || boundary_head.created_at != physical_head.created_at
    {
        return Err(SessionStoreError::Corrupted(physical_head.id.clone()));
    }
    let expected_boundary_prefix = boundary_head.message_row_prefix.as_ref().ok_or_else(|| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: boundary_head.id.clone(),
            reason: "runtime boundary has no exact message-row prefix authority".to_string(),
        }
    })?;
    verify_head_rewrite_prefix_descent_in_txn(tx, boundary_head, physical_head)?;
    let boundary = verify_head_canonical_with_metadata_owner_in_txn(
        tx,
        boundary_head,
        HeadMetadataProjectionOwner::RuntimeBoundary,
    )?;
    if boundary
        .exact_row_prefix_at(boundary_head.message_count)
        .as_ref()
        != Some(expected_boundary_prefix)
    {
        return Err(SessionStoreError::Corrupted(boundary_head.id.clone()));
    }
    let physical = verify_head_canonical_with_metadata_owner_in_txn(
        tx,
        physical_head,
        HeadMetadataProjectionOwner::PhysicalHead,
    )?;
    physical.with_verified_ancestor_row_prefix(expected_boundary_prefix.clone())
}

fn activation_boundary_strand_and_prove_rewrite_descent_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    boundary_rewrite_prefix: &TranscriptRewritePrefixAccumulator,
    physical_head: &SessionHead,
) -> Result<(TranscriptStrandId, u64), SessionStoreError> {
    if physical_head.rewrite_prefix.occurrence_count() != physical_head.rewrite_count {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: format!(
                "physical head rewrite-prefix authority covers {} commits but rewrite_count is {}",
                physical_head.rewrite_prefix.occurrence_count(),
                physical_head.rewrite_count
            ),
        });
    }
    let boundary_rewrite_count = boundary_rewrite_prefix.occurrence_count();
    if boundary_rewrite_count > physical_head.rewrite_count {
        return Err(SessionStoreError::TranscriptContinuityViolation {
            id: id.clone(),
            previous_revision: boundary_rewrite_prefix.digest().to_string(),
            incoming_revision: physical_head.rewrite_prefix.digest().to_string(),
            reason: format!(
                "physical head rewrite generation {} precedes frozen runtime boundary generation {boundary_rewrite_count}",
                physical_head.rewrite_count
            ),
        });
    }

    let tail = indexed_rewrite_rows_range_in_txn(
        tx,
        id,
        boundary_rewrite_count,
        physical_head.rewrite_count,
    )?;
    let expected_tail_len = physical_head
        .rewrite_count
        .checked_sub(boundary_rewrite_count)
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    if u64::try_from(tail.len()).ok() != Some(expected_tail_len) {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }

    let mut observed_physical_prefix = boundary_rewrite_prefix.clone();
    for (offset, (rewrite_idx, row)) in tail.iter().enumerate() {
        let expected_idx = boundary_rewrite_count
            .checked_add(
                u64::try_from(offset).map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            )
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if *rewrite_idx != expected_idx {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        observed_physical_prefix = observed_physical_prefix
            .extend(&row.commit)
            .map_err(SessionStoreError::from)?;
    }
    if observed_physical_prefix != physical_head.rewrite_prefix {
        return Err(SessionStoreError::TranscriptContinuityViolation {
            id: id.clone(),
            previous_revision: boundary_rewrite_prefix.digest().to_string(),
            incoming_revision: physical_head.rewrite_prefix.digest().to_string(),
            reason:
                "physical head rewrite-prefix authority is not an exact descendant of the frozen runtime boundary"
                    .to_string(),
        });
    }

    // With no intervening rewrite, A is an ordinary message prefix of H's
    // live strand. With a rewrite tail, the first persisted edge owns the
    // exact pre-rewrite parent strand; that retained strand, not H's possibly
    // compacted live strand, is the row authority for A.
    Ok(match tail.first() {
        Some((_idx, first)) => (first.parent_strand.clone(), first.parent_len),
        None => (physical_head.strand.clone(), physical_head.message_count),
    })
}

/// Derive a retained runtime-boundary head from a verified frozen Session and
/// the exact retained rows proved by the current physical head's lineage.
///
/// Explicit WholeBlob-to-HeadCanonical activation uses this after canonical
/// session conversion, before ordinary service is admitted. The returned head
/// is not installed as the physical head: it is the small A authority retained
/// by RuntimeStore while H may already be a newer intra-turn append or rewrite.
#[doc(hidden)]
pub fn derive_runtime_boundary_head_for_activation_in_txn(
    tx: &Transaction<'_>,
    session: &Session,
    boundary_rewrite_prefix: &TranscriptRewritePrefixAccumulator,
    physical_head: &SessionHead,
) -> Result<SessionHead, SessionStoreError> {
    if session.id() != &physical_head.id
        || session.version() != physical_head.version
        || session.created_at() != physical_head.created_at
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: "frozen runtime boundary does not share the physical head's immutable envelope"
                .to_string(),
        });
    }
    let boundary_count = session.messages().len() as u64;
    let (boundary_strand, retained_boundary_range) =
        activation_boundary_strand_and_prove_rewrite_descent_in_txn(
            tx,
            session.id(),
            boundary_rewrite_prefix,
            physical_head,
        )?;
    if boundary_count > retained_boundary_range {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!(
                "frozen runtime boundary contains {boundary_count} messages but its retained physical strand covers only {retained_boundary_range}"
            ),
        });
    }
    let links = strand_links_in_txn(tx, session.id())?;
    validate_strand_links_acyclic(session.id(), &links)?;
    let serialized_prefix = resolve_strand_bytes_in_txn(
        tx,
        session.id(),
        &boundary_strand,
        0..boundary_count,
        &links,
    )?;
    let decoded_prefix = serialized_prefix
        .iter()
        .map(|bytes| {
            serde_json::from_slice::<Message>(bytes)
                .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if decoded_prefix.as_slice() != session.messages() {
        return Err(SessionStoreError::TranscriptContinuityViolation {
            id: session.id().clone(),
            previous_revision: session
                .transcript_content_digest()
                .map_err(SessionStoreError::from)?,
            incoming_revision: transcript_messages_digest(&decoded_prefix)
                .map_err(SessionStoreError::from)?,
            reason:
                "physical canonical rows do not retain the frozen runtime boundary's exact typed prefix"
                    .to_string(),
        });
    }
    let exact_prefix =
        SessionMessageRowPrefixAccumulator::from_serialized_rows(&serialized_prefix)?;
    SessionHead::from_session_with_proved_storage_authority(
        session,
        boundary_strand,
        boundary_rewrite_prefix.clone(),
        exact_prefix,
    )
}

/// One-time blob-to-head conversion seam for the co-tenant runtime store.
///
/// This is intentionally hidden from generated documentation: ordinary
/// head-canonical boundaries must call
/// [`apply_prepared_head_canonical_mutation_in_txn`] and remain bounded.
/// The runtime may call this only from its explicit activation transaction,
/// after fencing the exact current runtime authority and proving the released
/// 0.8.10 source head. It borrows the caller's transaction and neither opens
/// nor commits one.
#[doc(hidden)]
pub fn ensure_head_canonical_for_runtime_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<Option<(SessionHead, String)>, SessionStoreError> {
    let Some((head, token)) = ensure_head_canonical_for_write_in_txn(tx, id)? else {
        return Ok(None);
    };
    match head.realtime_event_prefix.as_ref() {
        Some(_) => return Ok(Some((head, token))),
        None => {}
    }
    let links = strand_links_in_txn(tx, id)?;
    validate_strand_links_acyclic(id, &links)?;
    let serialized_rows =
        resolve_strand_bytes_in_txn(tx, id, &head.strand, 0..head.message_count, &links)?;
    let mut session = head
        .clone()
        .into_session_from_serialized_rows(serialized_rows)?;
    activate_realtime_component_in_txn(tx, &mut session)?;
    let message_row_prefix = head
        .message_row_prefix
        .clone()
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    let upgraded = SessionHead::from_session_with_proved_storage_authority(
        &session,
        head.strand.clone(),
        head.rewrite_prefix.clone(),
        message_row_prefix,
    )?;
    let upgraded_token = session_head_cas_token(&upgraded)?;
    reconcile_head_metadata_transition_in_txn(
        tx,
        Some(&head),
        Some(&token),
        &upgraded,
        &upgraded_token,
    )?;
    let upgraded_token = write_head_row_only_in_txn(tx, &upgraded)?;
    Ok(Some((upgraded, upgraded_token)))
}

/// Result of applying one sealed ordinary head-canonical mutation.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadCanonicalMutationApplyOutcome {
    /// The exact predecessor was current and the mutation was installed.
    Applied,
    /// The exact successor was already current; no rows were written.
    AlreadyApplied,
}

fn prepared_head_conflict(
    mutation: &PreparedHeadCanonicalMutation,
    actual: impl Into<String>,
) -> SessionStoreError {
    let expected = match mutation.predecessor_head_token() {
        Some(token) => token.to_string(),
        None => "absent canonical head".to_string(),
    };
    SessionStoreError::TranscriptRevisionConflict {
        id: mutation.session_id().clone(),
        expected,
        actual: actual.into(),
    }
}

fn point_check_materialized_append_boundary_in_txn(
    tx: &Transaction<'_>,
    mutation: &PreparedHeadCanonicalMutation,
) -> Result<(), SessionStoreError> {
    let id = mutation.session_id();
    let strand = mutation.strand();
    let base_seq = mutation.base_seq();
    let predecessor_lineage = match mutation.predecessor_head() {
        Some(predecessor) => {
            let canonical_token = session_head_cas_token(predecessor)?;
            if &predecessor.id != id
                || &predecessor.strand != strand
                || predecessor.message_count != base_seq
                || mutation.predecessor_head_token() != Some(canonical_token.as_str())
            {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            predecessor
                .message_row_prefix
                .as_ref()
                .filter(|lineage| lineage.row_count() == base_seq)
                .cloned()
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?
        }
        None if matches!(mutation.expected_cas(), SessionHeadCas::Create)
            && strand == &TranscriptStrandId::root()
            && base_seq == 0 =>
        {
            SessionMessageRowPrefixAccumulator::empty()
        }
        None => return Err(SessionStoreError::Corrupted(id.clone())),
    };
    let successor_lineage = predecessor_lineage
        .extend_serialized_rows(mutation.serialized_suffix())
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    if mutation.successor_head().message_row_prefix.as_ref() != Some(&successor_lineage) {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }

    let physical_extent = physical_row_extent_in_txn(tx, id, strand)?;
    let link = strand_link_in_txn(tx, id, strand)?;
    if mutation.predecessor_head().is_none() && link.is_some() {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let logical_len = match link {
        Some(link) => link.splice.strand_len.max(physical_extent),
        None => physical_extent,
    };
    if logical_len != base_seq {
        return Err(SessionStoreError::TranscriptContinuityViolation {
            id: id.clone(),
            previous_revision: format!("strand:{strand} logical-rows:{logical_len}"),
            incoming_revision: format!("append-base-seq:{base_seq}"),
            reason: "ordinary append base does not equal the exact logical strand tip".to_string(),
        });
    }
    let successor_count = i64::try_from(mutation.successor_head().message_count)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let trailing_row = tx
        .query_row(
            "SELECT seq FROM session_strand_messages
             WHERE session_id = ?1 AND strand = ?2 AND seq >= ?3
             ORDER BY seq ASC
             LIMIT 1",
            params![id.to_string(), strand.as_str(), successor_count],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    if let Some(existing_seq) = trailing_row {
        return Err(SessionStoreError::TranscriptContinuityViolation {
            id: id.clone(),
            previous_revision: format!("strand:{strand} trailing-seq:{existing_seq}"),
            incoming_revision: format!(
                "prepared-successor-count:{}",
                mutation.successor_head().message_count
            ),
            reason: "ordinary append cannot retract unadopted rows beyond its exact successor"
                .to_string(),
        });
    }
    Ok(())
}

fn reconcile_prepared_suffix_rows_in_txn(
    tx: &Transaction<'_>,
    mutation: &PreparedHeadCanonicalMutation,
) -> Result<(), SessionStoreError> {
    let id = mutation.session_id();
    let strand = mutation.strand();
    let start =
        i64::try_from(mutation.base_seq()).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let end = i64::try_from(mutation.successor_head().message_count)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let mut statement = tx
        .prepare(
            "SELECT seq, message_json FROM session_strand_messages
             WHERE session_id = ?1 AND strand = ?2 AND seq >= ?3 AND seq < ?4
             ORDER BY seq ASC",
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let existing_rows = statement
        .query_map(
            params![id.to_string(), strand.as_str(), start, end],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                ))
            },
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    drop(statement);

    let created_at_ms = now_millis();
    let mut existing = existing_rows.into_iter().peekable();
    for (offset, expected_bytes) in mutation.serialized_suffix().iter().enumerate() {
        let offset = u64::try_from(offset).map_err(|_| {
            SessionStoreError::Internal(format!(
                "session {id} ordinary suffix exceeds the durable u64 row range"
            ))
        })?;
        let expected_seq = mutation.base_seq().checked_add(offset).ok_or_else(|| {
            SessionStoreError::Internal(format!("session {id} ordinary suffix sequence overflow"))
        })?;
        let expected_seq_i64 =
            i64::try_from(expected_seq).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        match existing.peek() {
            Some((stored_seq, stored_bytes)) if *stored_seq == expected_seq_i64 => {
                if stored_bytes.as_slice() != expected_bytes.as_slice() {
                    return Err(SessionStoreError::TranscriptContinuityViolation {
                        id: id.clone(),
                        previous_revision: format!("strand:{strand} seq:{expected_seq}"),
                        incoming_revision: "divergent-bytes".to_string(),
                        reason:
                            "ordinary append encountered different bytes in an immutable suffix row"
                                .to_string(),
                    });
                }
                existing.next();
            }
            Some((stored_seq, _)) if *stored_seq < expected_seq_i64 => {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            _ => {
                tx.execute(
                    "INSERT INTO session_strand_messages
                         (session_id, strand, seq, message_json, created_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        id.to_string(),
                        strand.as_str(),
                        expected_seq_i64,
                        expected_bytes,
                        created_at_ms
                    ],
                )
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            }
        }
    }
    if existing.next().is_some() {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(())
}

fn reconcile_serialized_strand_rows_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    base_seq: u64,
    serialized_rows: &[Vec<u8>],
) -> Result<(), SessionStoreError> {
    let end = base_seq
        .checked_add(u64::try_from(serialized_rows.len()).map_err(|_| {
            SessionStoreError::Internal(format!(
                "session {id} serialized row delta exceeds the durable u64 range"
            ))
        })?)
        .ok_or_else(|| {
            SessionStoreError::Internal(format!(
                "session {id} serialized row delta sequence overflow"
            ))
        })?;
    let start_i64 =
        i64::try_from(base_seq).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let end_i64 = i64::try_from(end).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let mut statement = tx
        .prepare(
            "SELECT seq, message_json FROM session_strand_messages
             WHERE session_id = ?1 AND strand = ?2 AND seq >= ?3 AND seq < ?4
             ORDER BY seq ASC",
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let existing = statement
        .query_map(
            params![id.to_string(), strand.as_str(), start_i64, end_i64],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                ))
            },
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    drop(statement);

    let mut existing = existing.into_iter().peekable();
    let created_at_ms = now_millis();
    for (offset, expected_bytes) in serialized_rows.iter().enumerate() {
        let expected_seq = base_seq
            .checked_add(u64::try_from(offset).map_err(|_| {
                SessionStoreError::Internal(format!(
                    "session {id} serialized row offset exceeds u64"
                ))
            })?)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let expected_seq_i64 =
            i64::try_from(expected_seq).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        match existing.peek() {
            Some((stored_seq, stored_bytes)) if *stored_seq == expected_seq_i64 => {
                if stored_bytes.as_slice() != expected_bytes.as_slice() {
                    return Err(SessionStoreError::TranscriptContinuityViolation {
                        id: id.clone(),
                        previous_revision: format!("strand:{strand} seq:{expected_seq}"),
                        incoming_revision: "divergent-bytes".to_string(),
                        reason: "prepared rewrite encountered different immutable row bytes"
                            .to_string(),
                    });
                }
                existing.next();
            }
            Some((stored_seq, _)) if *stored_seq < expected_seq_i64 => {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            _ => {
                tx.execute(
                    "INSERT INTO session_strand_messages
                         (session_id, strand, seq, message_json, created_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        id.to_string(),
                        strand.as_str(),
                        expected_seq_i64,
                        expected_bytes,
                        created_at_ms
                    ],
                )
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            }
        }
    }
    if existing.next().is_some() {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(())
}

fn verify_serialized_strand_rows_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    base_seq: u64,
    serialized_rows: &[Vec<u8>],
) -> Result<(), SessionStoreError> {
    let end = base_seq
        .checked_add(u64::try_from(serialized_rows.len()).map_err(|_| {
            SessionStoreError::Internal(format!(
                "session {id} serialized row delta exceeds the durable u64 range"
            ))
        })?)
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    let stored = strand_row_bytes_in_txn(tx, id, strand, base_seq..end)?;
    if stored.as_slice() != serialized_rows {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(())
}

fn verify_prepared_suffix_rows_in_txn(
    tx: &Transaction<'_>,
    mutation: &PreparedHeadCanonicalMutation,
    require_current_tip: bool,
) -> Result<(), SessionStoreError> {
    let id = mutation.session_id();
    let strand = mutation.strand();
    let start =
        i64::try_from(mutation.base_seq()).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let end = i64::try_from(mutation.successor_head().message_count)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let mut statement = tx
        .prepare(
            "SELECT seq, message_json FROM session_strand_messages
             WHERE session_id = ?1 AND strand = ?2 AND seq >= ?3 AND seq < ?4
             ORDER BY seq ASC",
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let mut rows = statement
        .query(params![id.to_string(), strand.as_str(), start, end])
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    for (offset, expected_bytes) in mutation.serialized_suffix().iter().enumerate() {
        let Some(row) = rows
            .next()
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?
        else {
            return Err(SessionStoreError::Corrupted(id.clone()));
        };
        let stored_seq = row
            .get::<_, i64>(0)
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?;
        let stored_bytes = row
            .get::<_, JsonColumnBytes>(1)
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?
            .into_bytes();
        let offset = u64::try_from(offset).map_err(|_| {
            SessionStoreError::Internal(format!(
                "session {id} ordinary suffix exceeds the durable u64 row range"
            ))
        })?;
        let expected_seq = mutation
            .base_seq()
            .checked_add(offset)
            .and_then(|seq| i64::try_from(seq).ok())
            .ok_or_else(|| {
                SessionStoreError::Internal(format!(
                    "session {id} ordinary suffix sequence overflow"
                ))
            })?;
        if stored_seq != expected_seq || stored_bytes.as_slice() != expected_bytes.as_slice() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
    }
    if rows
        .next()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .is_some()
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    drop(rows);
    drop(statement);

    let trailing_row_exists = tx
        .query_row(
            "SELECT 1 FROM session_strand_messages
             WHERE session_id = ?1 AND strand = ?2 AND seq >= ?3
             ORDER BY seq ASC
             LIMIT 1",
            params![id.to_string(), strand.as_str(), end],
            |_row| Ok(()),
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .is_some();
    if require_current_tip && trailing_row_exists {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(())
}

fn component_rows_corrupted(id: &SessionId) -> SessionStoreError {
    SessionStoreError::Corrupted(id.clone())
}

fn load_verified_component_sequence_in_txn(
    tx: &Transaction<'_>,
    expected: &ComponentEventPrefixAuthority,
) -> Result<VerifiedComponentEventSequence, SessionStoreError> {
    let id = expected.session_id();
    let expected_count =
        i64::try_from(expected.event_count()).map_err(|_| component_rows_corrupted(id))?;
    let mut statement = tx
        .prepare(
            "SELECT seq, event_json, event_digest
             FROM session_component_events
             WHERE session_id = ?1 AND component = ?2 AND seq < ?3
             ORDER BY seq ASC",
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let rows = statement
        .query_map(
            params![
                id.to_string(),
                expected.component().as_str(),
                expected_count
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let mut stored = Vec::with_capacity(rows.len());
    for (seq, bytes, stored_digest) in rows {
        let seq = u64::try_from(seq).map_err(|_| component_rows_corrupted(id))?;
        let event = SerializedComponentEvent::from_canonical_bytes(bytes.clone())
            .map_err(|_| component_rows_corrupted(id))?;
        if event.digest().as_str() != stored_digest.as_str() {
            return Err(component_rows_corrupted(id));
        }
        stored.push(StoredComponentEventRow::new(seq, bytes));
    }
    VerifiedComponentEventSequence::verify_full(expected.clone(), stored)
        .map_err(|_| component_rows_corrupted(id))
}

fn point_check_component_predecessor_in_txn(
    tx: &Transaction<'_>,
    suffix: &PreparedComponentEventSuffix,
) -> Result<(), SessionStoreError> {
    let id = suffix.session_id();
    let component = suffix.component().as_str();
    if suffix.base_seq() > 0 {
        let predecessor_seq =
            i64::try_from(suffix.base_seq() - 1).map_err(|_| component_rows_corrupted(id))?;
        let predecessor_exists = tx
            .query_row(
                "SELECT 1 FROM session_component_events
                 WHERE session_id = ?1 AND component = ?2 AND seq = ?3",
                params![id.to_string(), component, predecessor_seq],
                |_row| Ok(()),
            )
            .optional()
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?
            .is_some();
        if !predecessor_exists {
            return Err(SessionStoreError::TranscriptContinuityViolation {
                id: id.clone(),
                previous_revision: suffix.predecessor().root_digest().to_string(),
                incoming_revision: suffix.successor().root_digest().to_string(),
                reason: format!(
                    "{} component predecessor row {} is missing",
                    suffix.component(),
                    suffix.base_seq() - 1
                ),
            });
        }
    }
    Ok(())
}

fn point_check_component_append_boundary_in_txn(
    tx: &Transaction<'_>,
    suffix: &PreparedComponentEventSuffix,
) -> Result<(), SessionStoreError> {
    point_check_component_predecessor_in_txn(tx, suffix)?;
    let id = suffix.session_id();
    let component = suffix.component().as_str();
    let successor_count = i64::try_from(suffix.successor().event_count())
        .map_err(|_| component_rows_corrupted(id))?;
    let trailing = tx
        .query_row(
            "SELECT seq FROM session_component_events
             WHERE session_id = ?1 AND component = ?2 AND seq >= ?3
             ORDER BY seq ASC LIMIT 1",
            params![id.to_string(), component, successor_count],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    if let Some(seq) = trailing {
        return Err(SessionStoreError::TranscriptContinuityViolation {
            id: id.clone(),
            previous_revision: format!("{}-component-trailing-seq:{seq}", suffix.component()),
            incoming_revision: suffix.successor().root_digest().to_string(),
            reason: "component append cannot retract rows beyond its exact successor".to_string(),
        });
    }
    Ok(())
}

fn reconcile_prepared_component_suffix_in_txn(
    tx: &Transaction<'_>,
    suffix: &PreparedComponentEventSuffix,
) -> Result<(), SessionStoreError> {
    point_check_component_append_boundary_in_txn(tx, suffix)?;
    let id = suffix.session_id();
    let component = suffix.component().as_str();
    let start = i64::try_from(suffix.base_seq()).map_err(|_| component_rows_corrupted(id))?;
    let end = i64::try_from(suffix.successor().event_count())
        .map_err(|_| component_rows_corrupted(id))?;
    let mut statement = tx
        .prepare(
            "SELECT seq, event_json, event_digest
             FROM session_component_events
             WHERE session_id = ?1 AND component = ?2 AND seq >= ?3 AND seq < ?4
             ORDER BY seq ASC",
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let existing = statement
        .query_map(params![id.to_string(), component, start, end], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    drop(statement);

    let mut existing = existing.into_iter().peekable();
    let created_at_ms = now_millis();
    for (offset, event) in suffix.events().iter().enumerate() {
        let offset = u64::try_from(offset).map_err(|_| component_rows_corrupted(id))?;
        let expected_seq = suffix
            .base_seq()
            .checked_add(offset)
            .ok_or_else(|| component_rows_corrupted(id))?;
        let expected_seq_i64 =
            i64::try_from(expected_seq).map_err(|_| component_rows_corrupted(id))?;
        match existing.peek() {
            Some((stored_seq, stored_bytes, stored_digest)) if *stored_seq == expected_seq_i64 => {
                if stored_bytes.as_slice() != event.bytes()
                    || stored_digest.as_str() != event.digest().as_str()
                {
                    return Err(SessionStoreError::TranscriptContinuityViolation {
                        id: id.clone(),
                        previous_revision: format!(
                            "{}-component-seq:{expected_seq}",
                            suffix.component()
                        ),
                        incoming_revision: event.digest().to_string(),
                        reason: "component append encountered different immutable event bytes"
                            .to_string(),
                    });
                }
                existing.next();
            }
            Some((stored_seq, _, _)) if *stored_seq < expected_seq_i64 => {
                return Err(component_rows_corrupted(id));
            }
            _ => {
                tx.execute(
                    "INSERT INTO session_component_events
                         (session_id, component, seq, event_json, event_digest, created_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        id.to_string(),
                        component,
                        expected_seq_i64,
                        event.bytes(),
                        event.digest().as_str(),
                        created_at_ms,
                    ],
                )
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            }
        }
    }
    if existing.next().is_some() {
        return Err(component_rows_corrupted(id));
    }
    Ok(())
}

fn verify_prepared_component_suffix_rows_in_txn(
    tx: &Transaction<'_>,
    suffix: &PreparedComponentEventSuffix,
    require_current_tip: bool,
) -> Result<(), SessionStoreError> {
    if require_current_tip {
        point_check_component_append_boundary_in_txn(tx, suffix)?;
    } else {
        point_check_component_predecessor_in_txn(tx, suffix)?;
    }
    let id = suffix.session_id();
    let component = suffix.component().as_str();
    let start = i64::try_from(suffix.base_seq()).map_err(|_| component_rows_corrupted(id))?;
    let end = i64::try_from(suffix.successor().event_count())
        .map_err(|_| component_rows_corrupted(id))?;
    let mut statement = tx
        .prepare(
            "SELECT seq, event_json, event_digest
             FROM session_component_events
             WHERE session_id = ?1 AND component = ?2 AND seq >= ?3 AND seq < ?4
             ORDER BY seq ASC",
        )
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let mut rows = statement
        .query(params![id.to_string(), component, start, end])
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    for (offset, event) in suffix.events().iter().enumerate() {
        let Some(row) = rows
            .next()
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?
        else {
            return Err(component_rows_corrupted(id));
        };
        let offset = u64::try_from(offset).map_err(|_| component_rows_corrupted(id))?;
        let expected_seq = suffix
            .base_seq()
            .checked_add(offset)
            .and_then(|seq| i64::try_from(seq).ok())
            .ok_or_else(|| component_rows_corrupted(id))?;
        let stored_seq = row
            .get::<_, i64>(0)
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?;
        let stored_bytes = row
            .get::<_, JsonColumnBytes>(1)
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?
            .into_bytes();
        let stored_digest = row
            .get::<_, String>(2)
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?;
        if stored_seq != expected_seq
            || stored_bytes.as_slice() != event.bytes()
            || stored_digest.as_str() != event.digest().as_str()
        {
            return Err(component_rows_corrupted(id));
        }
    }
    if rows
        .next()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .is_some()
    {
        return Err(component_rows_corrupted(id));
    }
    Ok(())
}

fn verify_prepared_component_suffix_in_txn(
    tx: &Transaction<'_>,
    suffix: &PreparedComponentEventSuffix,
) -> Result<(), SessionStoreError> {
    verify_prepared_component_suffix_rows_in_txn(tx, suffix, true)
}

fn verify_prepared_head_metadata_projection_for_owner_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
    owner: HeadMetadataProjectionOwner,
    expected_predecessor_head_token: Option<&str>,
) -> Result<(), SessionStoreError> {
    let Some(identity) = head.metadata_identity() else {
        return Ok(());
    };
    let projection = head
        .metadata_projection()
        .filter(|projection| projection.identity() == identity)
        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    if projection
        .mutations()
        .iter()
        .any(|mutation| !mutation.verify())
    {
        return Err(SessionStoreError::Corrupted(head.id.clone()));
    }
    let head_token = session_head_cas_token(head)?;
    let reference = metadata_owner_ref_in_txn(tx, &head.id, owner)?
        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    let state_id = reference.state_id.as_str();
    let state = metadata_state_in_txn(tx, &head.id, state_id)?
        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    if &state.identity != identity {
        return Err(SessionStoreError::Corrupted(head.id.clone()));
    }
    if projection.mutations().is_empty() {
        match projection.predecessor_identity() {
            Some(predecessor) if predecessor == identity => return Ok(()),
            None if identity.is_canonical_empty() && projection.is_full_snapshot() => {}
            _ => return Err(SessionStoreError::Corrupted(head.id.clone())),
        }
    }
    if state.transition_id != head_token || reference.head_cas_token != head_token {
        return Err(SessionStoreError::Corrupted(head.id.clone()));
    }
    let lineage = tx
        .query_row(
            r"
            SELECT transition_id, predecessor_head_cas_token,
                   predecessor_state_id, successor_state_id
            FROM session_head_metadata_head_lineage
            WHERE session_id = ?1 AND successor_head_cas_token = ?2
            ",
            params![head.id.to_string(), head_token],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
    if lineage.0 != head_token
        || lineage.1.as_deref() != expected_predecessor_head_token
        || lineage.2.as_deref() != state.predecessor_state_id.as_deref()
        || lineage.3 != state_id
    {
        return Err(SessionStoreError::Corrupted(head.id.clone()));
    }
    match projection.predecessor_identity() {
        Some(expected_predecessor) => {
            let predecessor_state_id = state
                .predecessor_state_id
                .as_deref()
                .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
            let predecessor_state = metadata_state_in_txn(tx, &head.id, predecessor_state_id)?
                .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
            if &predecessor_state.identity != expected_predecessor {
                return Err(SessionStoreError::Corrupted(head.id.clone()));
            }
        }
        None => {
            if state.predecessor_state_id.is_some()
                || !projection.is_full_snapshot()
                || (projection.mutations().is_empty() && !identity.is_canonical_empty())
            {
                return Err(SessionStoreError::Corrupted(head.id.clone()));
            }
        }
    }
    let stored_deltas = metadata_state_deltas_in_txn(tx, &head.id, state_id)?;
    if stored_deltas.len() != projection.mutations().len() {
        return Err(SessionStoreError::Corrupted(head.id.clone()));
    }
    for (stored, mutation) in stored_deltas.iter().zip(projection.mutations()) {
        if stored.key != mutation.key()
            || stored.key_route.as_slice() != mutation.key_route()
            || stored.predecessor_exact_value_digest.as_deref()
                != mutation
                    .predecessor()
                    .map(|identity| identity.exact_value_digest().as_str())
            || stored.successor_exact_value_digest.as_deref()
                != mutation
                    .successor()
                    .map(|cell| cell.identity().exact_value_digest().as_str())
        {
            return Err(SessionStoreError::Corrupted(head.id.clone()));
        }
        if let Some(predecessor) = mutation.predecessor() {
            let stored_predecessor = metadata_cell_in_txn(
                tx,
                &head.id,
                mutation.key(),
                predecessor.exact_value_digest().as_str(),
            )?
            .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
            if stored_predecessor.identity() != predecessor
                || stored_predecessor.key_route() != &mutation.key_route()
            {
                return Err(SessionStoreError::Corrupted(head.id.clone()));
            }
        }
        if let Some(cell) = mutation.successor() {
            let stored_cell = metadata_cell_in_txn(
                tx,
                &head.id,
                mutation.key(),
                cell.identity().exact_value_digest().as_str(),
            )?
            .ok_or_else(|| SessionStoreError::Corrupted(head.id.clone()))?;
            if stored_cell.identity() != cell.identity()
                || stored_cell.key_route() != cell.key_route()
                || stored_cell.canonical_json() != cell.canonical_json()
            {
                return Err(SessionStoreError::Corrupted(head.id.clone()));
            }
        }
        if matches!(owner, HeadMetadataProjectionOwner::PhysicalHead) {
            let current = current_metadata_cell_digest_in_txn(tx, &head.id, mutation.key())?;
            match (mutation.successor(), current) {
                (Some(cell), Some((route, digest)))
                    if route.as_slice() == mutation.key_route()
                        && digest == cell.identity().exact_value_digest().as_str() => {}
                (None, None) => {}
                _ => return Err(SessionStoreError::Corrupted(head.id.clone())),
            }
        }
    }
    Ok(())
}

/// Re-prove every SessionStore-owned row named by an exact prepared-mutation
/// retry without requiring the physical head to remain at that successor.
///
/// RuntimeStore's durable ordinary witness may stay authoritative after a
/// later physical append or rewrite. Message/component rows are immutable and
/// the successor metadata state remains reachable through the runtime-boundary
/// owner ref, so this helper validates those exact effects directly.
#[doc(hidden)]
pub fn verify_prepared_head_canonical_rows_for_exact_retry_in_txn(
    tx: &Transaction<'_>,
    mutation: &PreparedHeadCanonicalMutation,
) -> Result<(), SessionStoreError> {
    verify_prepared_suffix_rows_in_txn(tx, mutation, false)?;
    if let Some(suffix) = mutation.realtime_suffix() {
        verify_prepared_component_suffix_rows_in_txn(tx, suffix, false)?;
    }

    verify_prepared_head_metadata_projection_for_owner_in_txn(
        tx,
        mutation.successor_head(),
        HeadMetadataProjectionOwner::RuntimeBoundary,
        mutation.predecessor_head_token(),
    )
}

/// Apply a prepared ordinary create/append inside the caller's transaction.
///
/// The sealed carrier binds the exact predecessor and successor. This helper
/// revalidates that predecessor against the durable head, accepts the exact
/// successor as an idempotent retry, checks only indexed live-strand boundary
/// facts, inserts the pre-serialized suffix, and writes the successor head
/// without topology settlement. It never opens or commits a transaction and
/// never scans rewrite or strand-link history.
pub fn apply_prepared_head_canonical_mutation_in_txn(
    tx: &Transaction<'_>,
    mutation: &PreparedHeadCanonicalMutation,
) -> Result<HeadCanonicalMutationApplyOutcome, SessionStoreError> {
    let current = head_row_in_txn(tx, mutation.session_id())?;
    if let Some((current_head, stored_token)) = current.as_ref() {
        let canonical_token = session_head_cas_token(current_head)?;
        if canonical_token != *stored_token {
            return Err(SessionStoreError::Corrupted(mutation.session_id().clone()));
        }
        if stored_token == mutation.successor_head_token() {
            verify_prepared_suffix_rows_in_txn(tx, mutation, true)?;
            if let Some(suffix) = mutation.realtime_suffix() {
                verify_prepared_component_suffix_in_txn(tx, suffix)?;
            }
            verify_prepared_head_metadata_projection_for_owner_in_txn(
                tx,
                mutation.successor_head(),
                HeadMetadataProjectionOwner::PhysicalHead,
                mutation.predecessor_head_token(),
            )?;
            return Ok(HeadCanonicalMutationApplyOutcome::AlreadyApplied);
        }
    }

    match (mutation.expected_cas(), current.as_ref()) {
        (SessionHeadCas::Create, None) => {
            let legacy_blob_exists = tx
                .query_row(
                    "SELECT 1 FROM sessions WHERE session_id = ?1 LIMIT 1",
                    params![mutation.session_id().to_string()],
                    |_row| Ok(()),
                )
                .optional()
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?
                .is_some();
            if legacy_blob_exists {
                return Err(prepared_head_conflict(
                    mutation,
                    "legacy whole-blob session exists without a canonical head",
                ));
            }
        }
        (SessionHeadCas::IfToken(expected), Some((current_head, actual)))
            if expected == actual
                && mutation
                    .predecessor_head()
                    .is_some_and(|head| head == current_head) => {}
        (_, Some((_head, token))) => {
            return Err(prepared_head_conflict(mutation, token.clone()));
        }
        (_, None) => {
            return Err(prepared_head_conflict(mutation, "absent canonical head"));
        }
    }

    point_check_materialized_append_boundary_in_txn(tx, mutation)?;
    reconcile_prepared_suffix_rows_in_txn(tx, mutation)?;
    if let Some(suffix) = mutation.realtime_suffix() {
        reconcile_prepared_component_suffix_in_txn(tx, suffix)?;
    }
    reconcile_head_metadata_transition_in_txn(
        tx,
        mutation.predecessor_head(),
        mutation.predecessor_head_token(),
        mutation.successor_head(),
        mutation.successor_head_token(),
    )?;
    let written_token = write_head_row_only_in_txn(tx, mutation.successor_head())?;
    if written_token != mutation.successor_head_token() {
        return Err(SessionStoreError::Internal(format!(
            "session {} prepared successor token changed during head write",
            mutation.session_id()
        )));
    }
    Ok(HeadCanonicalMutationApplyOutcome::Applied)
}

fn prepared_rewrite_conflict(
    mutation: &PreparedHeadCanonicalRewriteMutation,
    actual: impl Into<String>,
) -> SessionStoreError {
    SessionStoreError::TranscriptRevisionConflict {
        id: mutation.session_id().clone(),
        expected: mutation.predecessor_head_token().to_string(),
        actual: actual.into(),
    }
}

fn reconcile_prepared_rewrite_row_in_txn(
    tx: &Transaction<'_>,
    mutation: &PreparedHeadCanonicalRewriteMutation,
    rewrite_idx: u64,
    step: &meerkat_core::session_store::PreparedHeadCanonicalRewriteStep,
) -> Result<(), SessionStoreError> {
    let id = mutation.session_id();
    let existing = indexed_rewrite_rows_range_in_txn(
        tx,
        id,
        rewrite_idx,
        rewrite_idx
            .checked_add(1)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?,
    )?;
    let expected = RewriteRow {
        commit: step.commit().clone(),
        parent_strand: step.parent_strand().clone(),
        parent_len: u64::try_from(step.commit().messages_before)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
        strand: step.strand().clone(),
        strand_len: u64::try_from(step.commit().messages_after)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
        graph_edge_json: Some(step.serialized_graph_edge().to_vec()),
    };
    match existing.as_slice() {
        [] => insert_rewrite_row_in_txn(tx, id, rewrite_idx, &expected),
        [(stored_idx, stored)]
            if *stored_idx == rewrite_idx
                && stored.commit == expected.commit
                && stored.parent_strand == expected.parent_strand
                && stored.parent_len == expected.parent_len
                && stored.strand == expected.strand
                && stored.strand_len == expected.strand_len
                && stored.graph_edge_json == expected.graph_edge_json =>
        {
            Ok(())
        }
        _ => Err(SessionStoreError::Corrupted(id.clone())),
    }
}

fn reconcile_prepared_rewrite_parent_transition_in_txn(
    tx: &Transaction<'_>,
    mutation: &PreparedHeadCanonicalRewriteMutation,
    step: &meerkat_core::session_store::PreparedHeadCanonicalRewriteStep,
    expected_source: &TranscriptStrandId,
    expected_source_len: u64,
) -> Result<(), SessionStoreError> {
    let id = mutation.session_id();
    match step.parent_transition() {
        PreparedHeadCanonicalParentTransition::ExactAppend => {
            if step.parent_strand() != expected_source {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: id.clone(),
                    reason:
                        "prepared exact-append rewrite parent is not the preceding occurrence strand"
                            .to_string(),
                });
            }
            Ok(())
        }
        PreparedHeadCanonicalParentTransition::ExactSplice(parent_splice) => {
            let expected_bridge = TranscriptStrandId::from_rewrite_parent_occurrence(step.commit());
            if parent_splice.source_strand() != expected_source
                || step.parent_strand() != &expected_bridge
                || step.parent_strand() == parent_splice.source_strand()
            {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: id.clone(),
                    reason: "prepared exact-splice parent bridge has inconsistent strand identity"
                        .to_string(),
                });
            }
            let splice = parent_splice.link_splice();
            let replacement_rows = u64::try_from(parent_splice.serialized_replacement().len())
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            if !splice.is_well_formed()
                || splice.strand_len != step.parent_base_seq()
                || replacement_rows == 0
                || splice.splice_end != splice.successor_end
                || splice.retained_rows() != replacement_rows
            {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: id.clone(),
                    reason:
                        "prepared exact-splice parent bridge does not describe an exact same-cardinality replacement"
                            .to_string(),
                });
            }
            if expected_source_len != splice.successor_len() {
                return Err(SessionStoreError::TranscriptContinuityViolation {
                    id: id.clone(),
                    previous_revision: format!(
                        "strand:{} logical-rows:{expected_source_len}",
                        parent_splice.source_strand()
                    ),
                    incoming_revision: format!(
                        "rewrite-parent-source-rows:{}",
                        splice.successor_len()
                    ),
                    reason:
                        "prepared exact-splice parent bridge does not target the exact preceding strand"
                            .to_string(),
                });
            }
            if let Some(existing) = strand_link_in_txn(tx, id, step.parent_strand())? {
                if existing.successor != *parent_splice.source_strand() || existing.splice != splice
                {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
            } else {
                if strand_has_physical_rows_in_txn(tx, id, step.parent_strand())? {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
                // The bridge id is occurrence-derived and the sealed carrier
                // proves it is not the preceding source. An absent row at
                // this exact key is therefore the only admissible fresh
                // state. Cold topology verification owns detection of
                // unrelated pre-existing graph corruption.
                tx.execute(
                    "INSERT INTO session_strand_links
                         (session_id, strand, successor, strand_len, splice_start, splice_end,
                          successor_end, created_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        id.to_string(),
                        step.parent_strand().as_str(),
                        parent_splice.source_strand().as_str(),
                        i64::try_from(splice.strand_len)
                            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                        i64::try_from(splice.splice_start)
                            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                        i64::try_from(splice.splice_end)
                            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                        i64::try_from(splice.successor_end)
                            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                        now_millis(),
                    ],
                )
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            }
            reconcile_serialized_strand_rows_in_txn(
                tx,
                id,
                step.parent_strand(),
                splice.splice_start,
                parent_splice.serialized_replacement(),
            )
        }
    }
}

fn reconcile_prepared_rewrite_link_in_txn(
    tx: &Transaction<'_>,
    mutation: &PreparedHeadCanonicalRewriteMutation,
    step: &meerkat_core::session_store::PreparedHeadCanonicalRewriteStep,
) -> Result<(), SessionStoreError> {
    let id = mutation.session_id();
    if step.strand() == step.parent_strand() {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: "prepared rewrite link points a strand at itself".to_string(),
        });
    }
    if let Some(existing) = strand_link_in_txn(tx, id, step.strand())? {
        if existing.successor == *step.parent_strand() && existing.splice == step.link_splice() {
            return Ok(());
        }
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    if strand_has_physical_rows_in_txn(tx, id, step.strand())? {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let splice = step.link_splice();
    let messages_before = u64::try_from(step.commit().messages_before)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let messages_after = u64::try_from(step.commit().messages_after)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let replacement_rows = u64::try_from(step.serialized_replacement().len())
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    if !splice.is_well_formed()
        || splice.successor_len() != messages_before
        || splice.strand_len != messages_after
        || splice.retained_rows() != replacement_rows
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: "prepared rewrite link does not match its sealed occurrence delta".to_string(),
        });
    }
    tx.execute(
        "INSERT INTO session_strand_links
             (session_id, strand, successor, strand_len, splice_start, splice_end,
              successor_end, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id.to_string(),
            step.strand().as_str(),
            step.parent_strand().as_str(),
            i64::try_from(splice.strand_len)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            i64::try_from(splice.splice_start)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            i64::try_from(splice.splice_end)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            i64::try_from(splice.successor_end)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            now_millis(),
        ],
    )
    .map_err(StoreError::from)
    .map_err(into_session_store_error)?;
    Ok(())
}

fn verify_named_strand_link_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    successor: &TranscriptStrandId,
    splice: StrandSplice,
) -> Result<(), SessionStoreError> {
    let stored = strand_link_in_txn(tx, id, strand)?
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    if stored.successor != *successor || stored.splice != splice {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(())
}

/// Derive the exact successor row lineage from the predecessor head and the
/// sealed mutation deltas.
///
/// The commitment is deliberately history/occurrence aware: a rewrite back
/// to byte-identical visible content remains a distinct successor. Every
/// transition hashes only its appended/replaced rows.
fn verify_prepared_rewrite_row_lineage(
    mutation: &PreparedHeadCanonicalRewriteMutation,
) -> Result<(), SessionStoreError> {
    let id = mutation.session_id();
    let predecessor_head = mutation.predecessor_head();
    let successor_head = mutation.successor_head();
    if &predecessor_head.id != id
        || &successor_head.id != id
        || session_head_cas_token(predecessor_head)? != mutation.predecessor_head_token()
        || session_head_cas_token(successor_head)? != mutation.successor_head_token()
        || !matches!(
            mutation.expected_cas(),
            SessionHeadCas::IfToken(token) if token == mutation.predecessor_head_token()
        )
        || predecessor_head.rewrite_prefix.occurrence_count() != predecessor_head.rewrite_count
        || successor_head.rewrite_prefix.occurrence_count() != successor_head.rewrite_count
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let mut row_lineage = predecessor_head
        .message_row_prefix
        .as_ref()
        .filter(|prefix| prefix.row_count() == predecessor_head.message_count)
        .cloned()
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    let successor_row_lineage = successor_head
        .message_row_prefix
        .as_ref()
        .filter(|prefix| prefix.row_count() == successor_head.message_count)
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    let mut rewrite_prefix = predecessor_head.rewrite_prefix.clone();
    let mut rewrite_count = predecessor_head.rewrite_count;
    let mut expected_source = predecessor_head.strand.clone();
    let mut expected_source_len = predecessor_head.message_count;
    for step in mutation.steps() {
        let messages_before = u64::try_from(step.commit().messages_before)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let messages_after = u64::try_from(step.commit().messages_after)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        if step.parent_base_seq() != expected_source_len {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let advanced = match step.parent_transition() {
            PreparedHeadCanonicalParentTransition::ExactAppend => {
                if step.parent_strand() != &expected_source {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
                row_lineage
            }
            PreparedHeadCanonicalParentTransition::ExactSplice(parent_splice) => {
                let expected_bridge =
                    TranscriptStrandId::from_rewrite_parent_occurrence(step.commit());
                let splice = parent_splice.link_splice();
                let replacement_rows = u64::try_from(parent_splice.serialized_replacement().len())
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
                if parent_splice.source_strand() != &expected_source
                    || step.parent_strand() != &expected_bridge
                    || replacement_rows == 0
                    || !splice.is_well_formed()
                    || splice.strand_len != step.parent_base_seq()
                    || splice.splice_end != splice.successor_end
                    || splice.successor_len() != expected_source_len
                    || splice.retained_rows() != replacement_rows
                {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
                row_lineage
                    .replace_serialized_range(
                        splice.splice_start,
                        splice.splice_end,
                        parent_splice.serialized_replacement(),
                    )
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?
            }
        };
        let parent_row_lineage = advanced
            .extend_serialized_rows(step.serialized_parent_suffix())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        if parent_row_lineage.row_count() != messages_before {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let splice = step.link_splice();
        let replacement_rows = u64::try_from(step.serialized_replacement().len())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        if !splice.is_well_formed()
            || splice.successor_len() != messages_before
            || splice.strand_len != messages_after
            || splice.retained_rows() != replacement_rows
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let result_row_lineage = parent_row_lineage
            .replace_serialized_range(
                splice.splice_start,
                splice.successor_end,
                step.serialized_replacement(),
            )
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        if result_row_lineage.row_count() != messages_after {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        rewrite_prefix = rewrite_prefix
            .extend(step.commit())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        rewrite_count = rewrite_count
            .checked_add(1)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        expected_source = step.strand().clone();
        expected_source_len = messages_after;
        row_lineage = result_row_lineage;
    }
    if rewrite_count != successor_head.rewrite_count
        || rewrite_prefix != successor_head.rewrite_prefix
        || expected_source != successor_head.strand
        || expected_source_len != mutation.tail_base_seq()
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let final_row_lineage = row_lineage
        .extend_serialized_rows(mutation.serialized_tail())
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    if &final_row_lineage != successor_row_lineage
        || final_row_lineage.row_count() != successor_head.message_count
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(())
}

/// Point-check every immutable physical fact named by one sealed rewrite.
///
/// This deliberately does not reload unchanged predecessor rows or the
/// accumulated strand graph. Exact byte-lineage is derived from the
/// predecessor head and the same prepared deltas below; cold materialization
/// re-proves only the bounded active topology and live document. Full-history
/// reproof belongs to explicit audit surfaces.
fn verify_prepared_head_canonical_rewrite_rows_named_in_txn(
    tx: &Transaction<'_>,
    mutation: &PreparedHeadCanonicalRewriteMutation,
    metadata_owner: HeadMetadataProjectionOwner,
) -> Result<(), SessionStoreError> {
    let id = mutation.session_id();
    let successor_head = mutation.successor_head();
    let successor_anchor_rotated = head_has_current_row_lineage_anchor(successor_head)?
        && successor_head.row_lineage_anchor != mutation.predecessor_head().row_lineage_anchor;
    verify_prepared_rewrite_row_lineage(mutation)?;

    let mut rewrite_idx = mutation.predecessor_head().rewrite_count;
    let mut expected_source = mutation.predecessor_head().strand.clone();
    let mut expected_source_len = mutation.predecessor_head().message_count;
    for step in mutation.steps() {
        let messages_before = u64::try_from(step.commit().messages_before)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let messages_after = u64::try_from(step.commit().messages_after)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        if step.parent_base_seq() != expected_source_len {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        match step.parent_transition() {
            PreparedHeadCanonicalParentTransition::ExactAppend => {
                if step.parent_strand() != &expected_source {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
            }
            PreparedHeadCanonicalParentTransition::ExactSplice(parent_splice) => {
                let expected_bridge =
                    TranscriptStrandId::from_rewrite_parent_occurrence(step.commit());
                let splice = parent_splice.link_splice();
                let replacement_rows = u64::try_from(parent_splice.serialized_replacement().len())
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
                if parent_splice.source_strand() != &expected_source
                    || step.parent_strand() != &expected_bridge
                    || replacement_rows == 0
                    || !splice.is_well_formed()
                    || splice.strand_len != step.parent_base_seq()
                    || splice.splice_end != splice.successor_end
                    || splice.retained_rows() != replacement_rows
                    || splice.successor_len() != expected_source_len
                {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
                verify_named_strand_link_in_txn(
                    tx,
                    id,
                    step.parent_strand(),
                    parent_splice.source_strand(),
                    splice,
                )?;
                verify_serialized_strand_rows_in_txn(
                    tx,
                    id,
                    step.parent_strand(),
                    splice.splice_start,
                    parent_splice.serialized_replacement(),
                )?;
            }
        }
        let parent_len = step
            .parent_base_seq()
            .checked_add(
                u64::try_from(step.serialized_parent_suffix().len())
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            )
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if parent_len != messages_before {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        verify_serialized_strand_rows_in_txn(
            tx,
            id,
            step.parent_strand(),
            step.parent_base_seq(),
            step.serialized_parent_suffix(),
        )?;
        if matches!(metadata_owner, HeadMetadataProjectionOwner::PhysicalHead)
            && let Some(parent_link) = strand_link_in_txn(tx, id, step.parent_strand())?
        {
            verify_linked_strand_physical_shape_in_txn(
                tx,
                id,
                step.parent_strand(),
                &parent_link,
                parent_len,
            )?;
        }
        let stored = indexed_rewrite_rows_range_in_txn(
            tx,
            id,
            rewrite_idx,
            rewrite_idx
                .checked_add(1)
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?,
        )?;
        match stored.as_slice() {
            [(stored_idx, row)]
                if *stored_idx == rewrite_idx
                    && row.commit == *step.commit()
                    && row.parent_strand == *step.parent_strand()
                    && row.parent_len == messages_before
                    && row.strand == *step.strand()
                    && row.strand_len == messages_after
                    && row.graph_edge_json.as_deref() == Some(step.serialized_graph_edge()) => {}
            _ => return Err(SessionStoreError::Corrupted(id.clone())),
        }
        let splice = step.link_splice();
        let replacement_rows = u64::try_from(step.serialized_replacement().len())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        if !splice.is_well_formed()
            || splice.successor_len() != messages_before
            || splice.strand_len != messages_after
            || splice.retained_rows() != replacement_rows
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let is_settled_successor =
            successor_anchor_rotated && step.strand() == &successor_head.strand;
        if !is_settled_successor || strand_link_in_txn(tx, id, step.strand())?.is_some() {
            verify_named_strand_link_in_txn(tx, id, step.strand(), step.parent_strand(), splice)?;
        }
        verify_serialized_strand_rows_in_txn(
            tx,
            id,
            step.strand(),
            step.link_splice().splice_start,
            step.serialized_replacement(),
        )?;
        rewrite_idx = rewrite_idx
            .checked_add(1)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        expected_source = step.strand().clone();
        expected_source_len = messages_after;
    }
    if rewrite_idx != successor_head.rewrite_count
        || expected_source != successor_head.strand
        || expected_source_len != mutation.tail_base_seq()
    {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    verify_serialized_strand_rows_in_txn(
        tx,
        id,
        &successor_head.strand,
        mutation.tail_base_seq(),
        mutation.serialized_tail(),
    )?;
    let final_message_count = mutation
        .tail_base_seq()
        .checked_add(
            u64::try_from(mutation.serialized_tail().len())
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
        )
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    if final_message_count != successor_head.message_count {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let final_link = strand_link_in_txn(tx, id, &successor_head.strand)?;
    match final_link.as_ref() {
        None if successor_anchor_rotated => {
            verify_direct_current_anchor_rows_in_txn(tx, successor_head)?;
        }
        None => return Err(SessionStoreError::Corrupted(id.clone())),
        Some(_) => {}
    }
    if matches!(metadata_owner, HeadMetadataProjectionOwner::PhysicalHead) {
        if let Some(final_link) = final_link {
            if final_link.splice.strand_len != mutation.tail_base_seq() {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            verify_linked_strand_physical_shape_in_txn(
                tx,
                id,
                &successor_head.strand,
                &final_link,
                successor_head.message_count,
            )?;
        }
    }
    if let Some(suffix) = mutation.realtime_suffix() {
        verify_prepared_component_suffix_rows_in_txn(tx, suffix, false)?;
    }
    verify_prepared_head_metadata_projection_for_owner_in_txn(
        tx,
        successor_head,
        metadata_owner,
        Some(mutation.predecessor_head_token()),
    )
}

/// Re-prove every immutable SessionStore effect named by a specialized
/// rewrite after RuntimeStore has already adopted its successor.
#[doc(hidden)]
pub fn verify_prepared_head_canonical_rewrite_rows_for_exact_retry_in_txn(
    tx: &Transaction<'_>,
    mutation: &PreparedHeadCanonicalRewriteMutation,
) -> Result<(), SessionStoreError> {
    verify_prepared_head_canonical_rewrite_rows_named_in_txn(
        tx,
        mutation,
        HeadMetadataProjectionOwner::RuntimeBoundary,
    )
}

/// Apply one sealed HeadCanonical rewrite using only bridge/replacement/tail
/// rows. Shared prefix/suffix rows are linked to earlier strands; no full
/// successor strand is ever staged.
pub fn apply_prepared_head_canonical_rewrite_mutation_in_txn(
    tx: &Transaction<'_>,
    mutation: &PreparedHeadCanonicalRewriteMutation,
) -> Result<HeadCanonicalMutationApplyOutcome, SessionStoreError> {
    let current = head_row_in_txn(tx, mutation.session_id())?;
    if let Some((current_head, stored_token)) = current.as_ref() {
        let canonical_token = session_head_cas_token(current_head)?;
        if canonical_token != *stored_token {
            return Err(SessionStoreError::Corrupted(mutation.session_id().clone()));
        }
        if stored_token == mutation.successor_head_token()
            && current_head == mutation.successor_head()
        {
            verify_prepared_head_canonical_rewrite_rows_named_in_txn(
                tx,
                mutation,
                HeadMetadataProjectionOwner::PhysicalHead,
            )?;
            return Ok(HeadCanonicalMutationApplyOutcome::AlreadyApplied);
        }
    }
    match current.as_ref() {
        Some((head, token))
            if token == mutation.predecessor_head_token()
                && head == mutation.predecessor_head() => {}
        Some((_head, token)) => {
            return Err(prepared_rewrite_conflict(mutation, token.clone()));
        }
        None => {
            return Err(prepared_rewrite_conflict(mutation, "absent canonical head"));
        }
    }

    verify_prepared_rewrite_row_lineage(mutation)?;

    let id = mutation.session_id();
    let mut rewrite_idx = mutation.predecessor_head().rewrite_count;
    let mut expected_source = mutation.predecessor_head().strand.clone();
    let mut expected_source_len = mutation.predecessor_head().message_count;
    for step in mutation.steps() {
        if step.parent_base_seq() != expected_source_len {
            return Err(SessionStoreError::TranscriptContinuityViolation {
                id: id.clone(),
                previous_revision: format!(
                    "strand:{} logical-rows:{expected_source_len}",
                    expected_source
                ),
                incoming_revision: format!("rewrite-parent-base:{}", step.parent_base_seq()),
                reason: "prepared rewrite parent bridge does not begin at the sealed strand tip"
                    .to_string(),
            });
        }
        reconcile_prepared_rewrite_parent_transition_in_txn(
            tx,
            mutation,
            step,
            &expected_source,
            expected_source_len,
        )?;
        reconcile_serialized_strand_rows_in_txn(
            tx,
            id,
            step.parent_strand(),
            step.parent_base_seq(),
            step.serialized_parent_suffix(),
        )?;
        let messages_before = u64::try_from(step.commit().messages_before)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let bridged_len = step
            .parent_base_seq()
            .checked_add(
                u64::try_from(step.serialized_parent_suffix().len())
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            )
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if bridged_len != messages_before {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        reconcile_prepared_rewrite_row_in_txn(tx, mutation, rewrite_idx, step)?;
        reconcile_prepared_rewrite_link_in_txn(tx, mutation, step)?;
        reconcile_serialized_strand_rows_in_txn(
            tx,
            id,
            step.strand(),
            step.link_splice().splice_start,
            step.serialized_replacement(),
        )?;
        rewrite_idx = rewrite_idx
            .checked_add(1)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        expected_source = step.strand().clone();
        expected_source_len = u64::try_from(step.commit().messages_after)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    }
    if rewrite_idx != mutation.successor_head().rewrite_count {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let final_strand = &mutation.successor_head().strand;
    if &expected_source != final_strand || expected_source_len != mutation.tail_base_seq() {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    reconcile_serialized_strand_rows_in_txn(
        tx,
        id,
        final_strand,
        mutation.tail_base_seq(),
        mutation.serialized_tail(),
    )?;
    let successor_anchor_rotated = head_has_current_row_lineage_anchor(mutation.successor_head())?
        && mutation.successor_head().row_lineage_anchor
            != mutation.predecessor_head().row_lineage_anchor;
    if successor_anchor_rotated {
        let additional_hops = mutation
            .steps()
            .len()
            .checked_mul(2)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let hop_budget =
            ordinary_topology_hop_budget(mutation.predecessor_head(), additional_hops)?;
        let links = bounded_strand_links_for_roots_in_txn(
            tx,
            id,
            std::slice::from_ref(final_strand),
            hop_budget,
        )?;
        let settled = settle_resolved_strand_direct_in_txn(
            tx,
            id,
            final_strand,
            mutation.successor_head().message_count,
            &links,
        )?;
        let observed = SessionMessageRowPrefixAccumulator::from_serialized_rows(&settled)?;
        let anchor = mutation
            .successor_head()
            .row_lineage_anchor
            .as_ref()
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        if observed != *anchor.materialized_prefix() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
    }
    if let Some(suffix) = mutation.realtime_suffix() {
        reconcile_prepared_component_suffix_in_txn(tx, suffix)?;
    }
    reconcile_head_metadata_transition_in_txn(
        tx,
        Some(mutation.predecessor_head()),
        Some(mutation.predecessor_head_token()),
        mutation.successor_head(),
        mutation.successor_head_token(),
    )?;
    let written_token = write_head_row_only_in_txn(tx, mutation.successor_head())?;
    if written_token != mutation.successor_head_token() {
        return Err(SessionStoreError::Internal(format!(
            "session {} prepared rewrite successor token changed during head write",
            mutation.session_id()
        )));
    }
    Ok(HeadCanonicalMutationApplyOutcome::Applied)
}

fn materialize_slim_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    head: &SessionHead,
    stored_token: &str,
) -> Result<Session, SessionStoreError> {
    if &head.id != id || session_head_cas_token(head)? != stored_token {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    Ok(verify_physical_head_canonical_in_txn(tx, head)?
        .session()
        .as_ref()
        .clone())
}

/// Head-canonical compat write: delta-append when the incoming transcript
/// extends the persisted head strand, otherwise a `rebase:` strand switch.
///
/// The rebase branch writes a whole transcript of rows, but the head write
/// point-settles (or, for rewrite-free state, deletes) only the exact strand
/// it left. It never scans the accumulated rewrite/link graph.
fn write_head_canonical_session_in_txn(
    tx: &Transaction<'_>,
    session: &Session,
    head: &SessionHead,
) -> Result<(), SessionStoreError> {
    let id = session.id();
    let live = session.messages();
    let prev_count = usize::try_from(head.message_count)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    // A superseded head strand no longer owns its rows, so it cannot be
    // appended to; such a head rebases onto a fresh materialized strand.
    let head_is_materialized = strand_link_in_txn(tx, id, &head.strand)?.is_none();
    let plain_append = head_is_materialized
        && live.len() >= prev_count
        && transcript_messages_digest(&live[..prev_count]).map_err(SessionStoreError::from)?
            == head.head_revision;
    let strand = if plain_append {
        if live.len() > prev_count {
            insert_strand_rows_in_txn(
                tx,
                id,
                &head.strand,
                head.message_count,
                &live[prev_count..],
            )?;
        }
        head.strand.clone()
    } else {
        let live_digest = transcript_messages_digest(live).map_err(SessionStoreError::from)?;
        let rebased = TranscriptStrandId::rebase(&live_digest);
        insert_strand_rows_in_txn(tx, id, &rebased, 0, live)?;
        rebased
    };
    let new_head = SessionHead::from_session(session, strand, head.rewrite_count)?;
    write_head_row_in_txn(tx, &new_head)?;
    Ok(())
}

/// SQLite-backed session store with one connection per operation.
pub struct SqliteSessionStore {
    path: PathBuf,
    options: SqliteConnectionOptions,
}

impl SqliteSessionStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        Self::open_with_options(path, SqliteConnectionOptions::default())
    }

    pub fn open_with_options(
        path: impl Into<PathBuf>,
        options: SqliteConnectionOptions,
    ) -> Result<Self, StoreError> {
        let path = path.into();
        let _guard = meerkat_sqlite::OperationGuard::for_database(&path)?;
        let conn = open_session_connection(&path, options)?;
        drop(conn);
        Ok(Self { path, options })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SqliteSessionStore {
    async fn in_write_txn<T, F>(&self, op: F) -> Result<T, SessionStoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<T, SessionStoreError> + Send + 'static,
    {
        let path = self.path.clone();
        let options = self.options;
        tokio::task::spawn_blocking(move || -> Result<T, SessionStoreError> {
            let _guard = meerkat_sqlite::OperationGuard::for_database(&path)
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            let mut conn =
                open_session_connection(&path, options).map_err(into_session_store_error)?;
            let tx = begin_immediate_transaction_with_options(&mut conn, options)
                .map_err(into_session_store_error)?;
            let value = op(&tx)?;
            tx.commit()
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            Ok(value)
        })
        .await
        .map_err(StoreError::Join)
        .map_err(into_session_store_error)?
    }

    /// Consistent multi-row read snapshot without taking the write lock.
    async fn in_read_txn<T, F>(&self, op: F) -> Result<T, SessionStoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<T, SessionStoreError> + Send + 'static,
    {
        let path = self.path.clone();
        let options = self.options;
        tokio::task::spawn_blocking(move || -> Result<T, SessionStoreError> {
            let _guard = meerkat_sqlite::OperationGuard::for_database(&path)
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            let mut conn =
                open_session_connection(&path, options).map_err(into_session_store_error)?;
            let tx = conn
                .transaction()
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            let value = op(&tx)?;
            tx.commit()
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            Ok(value)
        })
        .await
        .map_err(StoreError::Join)
        .map_err(into_session_store_error)?
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
        // F1 closure (wave-c C-H1): reject shrink-attempts at the trait
        // boundary before the row is overwritten on disk.
        let session = session.clone();
        self.in_write_txn(move |tx| {
            if let Some((head, stored_token)) = head_row_in_txn(tx, session.id())? {
                // Head-canonical: retained history lives out-of-line; the
                // plain save writes ONLY the delta rows + the small head.
                let previous = materialize_slim_in_txn(tx, session.id(), &head, &stored_token)?;
                head_canonical_plain_save_guard_with_prefix_witness(
                    &session,
                    &previous,
                    head.rewrite_count,
                    &head.rewrite_prefix,
                    SaveGuardWitness::none().with_previous_revision(&head.head_revision),
                )?;
                write_head_canonical_session_in_txn(tx, &session, &head)?;
                return Ok(());
            }
            let previous =
                load_session_snapshot_in_txn(tx, session.id()).map_err(into_session_store_error)?;
            meerkat_core::session_store::append_only_save_guard(&session, previous.as_ref())?;
            write_session_snapshot_in_txn(tx, &session).map_err(into_session_store_error)?;
            Ok(())
        })
        .await
    }

    async fn save_transcript_rewrite(
        &self,
        session: &Session,
        commit: &meerkat_core::TranscriptRewriteCommit,
    ) -> Result<(), SessionStoreError> {
        let session = session.clone();
        let commit = commit.clone();
        self.in_write_txn(move |tx| {
            if head_row_in_txn(tx, session.id())?.is_some() {
                // A TranscriptRewriteRecord does not bind the occurrence
                // generation, parent advance, graph prefix, or row-lineage
                // witnesses needed by cold replay. Never manufacture or
                // adopt a NULL edge for a current HeadCanonical session.
                return Err(legacy_head_canonical_rewrite_error(session.id()));
            }
            let previous =
                load_session_snapshot_in_txn(tx, session.id()).map_err(into_session_store_error)?;
            meerkat_core::session_store::transcript_rewrite_save_guard(
                &session,
                previous.as_ref(),
                &commit,
            )?;
            write_session_snapshot_in_txn(tx, &session).map_err(into_session_store_error)?;
            Ok(())
        })
        .await
    }

    async fn save_authoritative_projection(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        let session = session.clone();
        self.in_write_txn(move |tx| {
            if let Some((head, stored_token)) = head_row_in_txn(tx, session.id())? {
                if session_head_cas_token(&head)? != stored_token {
                    return Err(SessionStoreError::Corrupted(session.id().clone()));
                }
                write_head_canonical_session_in_txn(tx, &session, &head)?;
                return Ok(());
            }
            write_session_snapshot_in_txn(tx, &session).map_err(into_session_store_error)?;
            Ok(())
        })
        .await
    }

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), SessionStoreError> {
        let session = session.clone();
        self.in_write_txn(move |tx| {
            if let Some((head, stored_token)) = head_row_in_txn(tx, session.id())? {
                // The caller's token was computed over the slim
                // materialization it loaded; the same deterministic
                // materialization is compared here.
                let previous = materialize_slim_in_txn(tx, session.id(), &head, &stored_token)?;
                meerkat_core::session_store::authoritative_projection_current_revision_guard(
                    &session,
                    Some(&previous),
                    expected_current_revision.as_deref(),
                )?;
                write_head_canonical_session_in_txn(tx, &session, &head)?;
                return Ok(());
            }
            let previous =
                load_session_snapshot_in_txn(tx, session.id()).map_err(into_session_store_error)?;
            meerkat_core::session_store::authoritative_projection_current_revision_guard(
                &session,
                previous.as_ref(),
                expected_current_revision.as_deref(),
            )?;
            write_session_snapshot_in_txn(tx, &session).map_err(into_session_store_error)?;
            Ok(())
        })
        .await
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        let id = id.clone();
        self.in_read_txn(move |tx| {
            if let Some((head, stored_token)) = head_row_in_txn(tx, &id)? {
                // Slim, no history metadata — the O(live) cold-resume contract.
                return Ok(Some(materialize_slim_in_txn(
                    tx,
                    &id,
                    &head,
                    &stored_token,
                )?));
            }
            load_session_snapshot_in_txn(tx, &id).map_err(into_session_store_error)
        })
        .await
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        let path = self.path.clone();
        let options = self.options;
        tokio::task::spawn_blocking(move || -> Result<Vec<SessionMeta>, SessionStoreError> {
            enum ListedSession {
                Head {
                    raw_id: String,
                    head_json: Vec<u8>,
                    stored_token: String,
                },
                Legacy(SessionMeta),
            }

            let _guard = meerkat_sqlite::OperationGuard::for_database(&path)
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            let conn = open_session_connection(&path, options).map_err(into_session_store_error)?;
            let created_after = filter.created_after.map(system_time_millis);
            let updated_after = filter.updated_after.map(system_time_millis);
            let offset = i64::try_from(filter.offset.unwrap_or(0)).map_err(|_| {
                SessionStoreError::Internal("session list offset exceeds SQLite range".to_string())
            })?;
            // SQLite's documented LIMIT -1 form means "no limit" while still
            // admitting an OFFSET. This keeps pagination in the query for both
            // bounded and unbounded callers.
            let limit = match filter.limit {
                Some(limit) => i64::try_from(limit).map_err(|_| {
                    SessionStoreError::Internal(
                        "session list limit exceeds SQLite range".to_string(),
                    )
                })?,
                None => -1,
            };

            // Page the compact head/legacy projections first. In particular,
            // do not resolve every digest-addressed metadata payload only to
            // discard most of them during in-memory pagination.
            let mut statement = conn
                .prepare(
                    r"
                    SELECT session_id, created_at_ms, updated_at_ms, message_count,
                           total_tokens, projection_json, cas_token, is_head
                    FROM (
                        SELECT session_id, created_at_ms, updated_at_ms, message_count,
                               total_tokens, head_json AS projection_json,
                               cas_token, 1 AS is_head
                        FROM session_heads
                        WHERE (?1 IS NULL OR created_at_ms >= ?1)
                          AND (?2 IS NULL OR updated_at_ms >= ?2)

                        UNION ALL

                        SELECT session_id, created_at_ms, updated_at_ms, message_count,
                               total_tokens, metadata_json AS projection_json,
                               NULL AS cas_token, 0 AS is_head
                        FROM sessions AS legacy
                        WHERE (?1 IS NULL OR created_at_ms >= ?1)
                          AND (?2 IS NULL OR updated_at_ms >= ?2)
                          AND NOT EXISTS (
                              SELECT 1
                              FROM session_heads AS canonical
                              WHERE canonical.session_id = legacy.session_id
                          )
                    )
                    ORDER BY updated_at_ms DESC, session_id ASC
                    LIMIT ?3 OFFSET ?4
                    ",
                )
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            let selected = statement
                .query_map(
                    params![created_after, updated_after, limit, offset],
                    |row| {
                        if row.get::<_, i64>(7)? == 1 {
                            Ok(ListedSession::Head {
                                raw_id: row.get(0)?,
                                head_json: row.get::<_, JsonColumnBytes>(5)?.into_bytes(),
                                stored_token: row.get(6)?,
                            })
                        } else {
                            Ok(ListedSession::Legacy(session_meta_from_row(row)?))
                        }
                    },
                )
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            drop(statement);

            let mut metas = Vec::with_capacity(selected.len());
            for selected in selected {
                match selected {
                    ListedSession::Head {
                        raw_id,
                        head_json,
                        stored_token,
                    } => {
                        let id = parse_session_id(raw_id).map_err(into_session_store_error)?;
                        let mut head: SessionHead = serde_json::from_slice(&head_json)
                            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
                        if head.id != id || session_head_cas_token(&head)? != stored_token {
                            return Err(SessionStoreError::Corrupted(id));
                        }
                        attach_head_metadata_projection(
                            &conn,
                            &mut head,
                            HeadMetadataProjectionOwner::PhysicalHead,
                        )?;
                        metas.push(session_meta_from_head(&head)?);
                    }
                    ListedSession::Legacy(meta) => metas.push(meta),
                }
            }
            Ok(metas)
        })
        .await
        .map_err(StoreError::Join)
        .map_err(into_session_store_error)?
    }

    /// Metadata-only partial read over the durable head projection and its
    /// authenticated metadata cells — head row wins, the legacy session row
    /// is the fallback. Never touches `session_json` or strand rows.
    async fn load_meta(&self, id: &SessionId) -> Result<Option<SessionMeta>, SessionStoreError> {
        let path = self.path.clone();
        let options = self.options;
        let id = id.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<SessionMeta>, SessionStoreError> {
            let _guard = meerkat_sqlite::OperationGuard::for_database(&path)
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            let conn = open_session_connection(&path, options).map_err(into_session_store_error)?;
            let head_row = conn
                .query_row(
                    r"
                    SELECT head_json, cas_token
                    FROM session_heads
                    WHERE session_id = ?1
                    ",
                    params![id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, JsonColumnBytes>(0)?.into_bytes(),
                            row.get::<_, String>(1)?,
                        ))
                    },
                )
                .optional()
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            if let Some((head_json, stored_token)) = head_row {
                let mut head: SessionHead = serde_json::from_slice(&head_json)
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
                if head.id != id || session_head_cas_token(&head)? != stored_token {
                    return Err(SessionStoreError::Corrupted(id));
                }
                attach_head_metadata_projection(
                    &conn,
                    &mut head,
                    HeadMetadataProjectionOwner::PhysicalHead,
                )?;
                return Ok(Some(session_meta_from_head(&head)?));
            }
            conn.query_row(
                r"
                SELECT session_id, created_at_ms, updated_at_ms, message_count,
                       total_tokens, metadata_json
                FROM sessions
                WHERE session_id = ?1
                ",
                params![id.to_string()],
                session_meta_from_row,
            )
            .optional()
            .map_err(StoreError::from)
            .map_err(into_session_store_error)
        })
        .await
        .map_err(StoreError::Join)
        .map_err(into_session_store_error)?
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
        let id = id.clone();
        self.in_write_txn(move |tx| {
            delete_all_session_rows_in_txn(tx, &id)?;
            Ok(())
        })
        .await
    }

    async fn delete_if_current_revision(
        &self,
        id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        let session_id = id.clone();
        let expected_current_revision = expected_current_revision.to_string();
        self.in_write_txn(move |tx| {
            let previous = if let Some((head, stored_token)) = head_row_in_txn(tx, &session_id)? {
                Some(materialize_slim_in_txn(
                    tx,
                    &session_id,
                    &head,
                    &stored_token,
                )?)
            } else {
                load_session_snapshot_in_txn(tx, &session_id).map_err(into_session_store_error)?
            };
            let Some(previous) = previous else {
                return Ok(false);
            };
            let previous_token =
                meerkat_core::session_store::session_projection_cas_token(&previous)?;
            if previous_token != expected_current_revision {
                return Ok(false);
            }
            delete_all_session_rows_in_txn(tx, &session_id)?;
            Ok(true)
        })
        .await
    }

    fn as_incremental(self: Arc<Self>) -> Option<Arc<dyn IncrementalSessionStore>> {
        Some(self)
    }
}

fn session_meta_from_head(head: &SessionHead) -> Result<SessionMeta, SessionStoreError> {
    let message_count = usize::try_from(head.message_count)
        .map_err(|_| SessionStoreError::Corrupted(head.id.clone()))?;
    Ok(SessionMeta {
        id: head.id.clone(),
        created_at: head.created_at,
        updated_at: head.updated_at,
        message_count,
        total_tokens: head.usage.total_tokens(),
        metadata: head.materialized_metadata()?,
    })
}

fn session_meta_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionMeta> {
    let metadata_json = row.get::<_, JsonColumnBytes>(5)?.into_bytes();
    let metadata = serde_json::from_slice(&metadata_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let id = parse_session_id(row.get(0)?).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    // Derived projection counters are stored as i64; negative or
    // out-of-range values are impossible durable states and fail closed
    // (terminal-truth store-metadata cluster).
    let message_count = usize::try_from(row.get::<_, i64>(3)?).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Integer,
            Box::new(StoreError::Corrupted(id.clone())),
        )
    })?;
    let total_tokens = u64::try_from(row.get::<_, i64>(4)?).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Integer,
            Box::new(StoreError::Corrupted(id.clone())),
        )
    })?;
    Ok(SessionMeta {
        id,
        created_at: millis_to_system_time(row.get(1)?),
        updated_at: millis_to_system_time(row.get(2)?),
        message_count,
        total_tokens,
        metadata,
    })
}

fn delete_all_session_rows_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<(), SessionStoreError> {
    let retained_runtime_boundary = tx
        .query_row(
            r"
            SELECT 1
            FROM session_head_metadata_refs
            WHERE session_id = ?1 AND owner = 'runtime_boundary'
            LIMIT 1
            ",
            params![id.to_string()],
            |_row| Ok(()),
        )
        .optional()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .is_some();
    if retained_runtime_boundary {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason:
                "session deletion requires its retained runtime-boundary authority to be deleted first"
                    .to_string(),
        });
    }
    for sql in [
        "DELETE FROM sessions WHERE session_id = ?1",
        "DELETE FROM session_strand_messages WHERE session_id = ?1",
        "DELETE FROM session_strand_links WHERE session_id = ?1",
        "DELETE FROM session_rewrites WHERE session_id = ?1",
        "DELETE FROM session_component_events WHERE session_id = ?1",
        "DELETE FROM session_head_metadata_refs WHERE session_id = ?1",
        "DELETE FROM session_head_metadata_head_lineage WHERE session_id = ?1",
        "DELETE FROM session_head_metadata_state_deltas WHERE session_id = ?1",
        "DELETE FROM session_head_metadata_states WHERE session_id = ?1",
        "DELETE FROM session_head_metadata_current WHERE session_id = ?1",
        "DELETE FROM session_head_metadata_cells WHERE session_id = ?1",
        "DELETE FROM session_heads WHERE session_id = ?1",
    ] {
        tx.execute(sql, params![id.to_string()])
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?;
    }
    Ok(())
}

fn legacy_head_canonical_rewrite_error(id: &SessionId) -> SessionStoreError {
    SessionStoreError::InvalidTranscriptRewrite {
        id: id.clone(),
        reason: "HeadCanonical rewrites require PreparedHeadCanonicalRewriteMutation; the legacy TranscriptRewriteRecord path cannot prove an exact compact replay edge"
            .to_string(),
    }
}

#[async_trait]
impl IncrementalSessionStore for SqliteSessionStore {
    async fn append_messages(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        base_seq: u64,
        messages: &[Message],
    ) -> Result<(), SessionStoreError> {
        let id = id.clone();
        let strand = strand.clone();
        let messages = messages.to_vec();
        self.in_write_txn(move |tx| {
            // First incremental write on a blob-only session migrates it.
            let _ = ensure_head_canonical_for_write_in_txn(tx, &id)?;
            insert_strand_rows_in_txn(tx, &id, &strand, base_seq, &messages)
        })
        .await
    }

    async fn commit_rewrite(
        &self,
        id: &SessionId,
        _record: &TranscriptRewriteRecord,
        _expected: SessionHeadCas,
    ) -> Result<SessionHead, SessionStoreError> {
        Err(legacy_head_canonical_rewrite_error(id))
    }

    async fn save_head(
        &self,
        head: &SessionHead,
        expected: SessionHeadCas,
    ) -> Result<(), SessionStoreError> {
        let head = head.clone();
        self.in_write_txn(move |tx| {
            let stored = ensure_head_canonical_for_write_in_txn(tx, &head.id)?;
            let replay_start = head
                .row_lineage_anchor
                .as_ref()
                .map_or(0, |anchor| anchor.rewrite_count());
            if post_anchor_rewrite_graph_edge_is_missing_in_txn(
                tx,
                &head.id,
                replay_start,
                head.rewrite_count,
            )? {
                return Err(legacy_head_canonical_rewrite_error(&head.id));
            }
            let hop_budget = ordinary_topology_hop_budget(&head, 0)?;
            let links = bounded_strand_links_for_roots_in_txn(
                tx,
                &head.id,
                std::slice::from_ref(&head.strand),
                hop_budget,
            )?;
            let strand_len = strand_logical_len_in_txn(tx, &head.id, &head.strand, &links)?;
            let recorded = rewrite_row_count_in_txn(tx, &head.id)?;
            validate_save_head_transition(
                &head,
                stored.as_ref().map(|(h, t)| (h, t.as_str())),
                &expected,
                strand_len,
                recorded,
            )?;
            write_head_row_in_txn(tx, &head)?;
            Ok(())
        })
        .await
    }

    async fn load_head(&self, id: &SessionId) -> Result<Option<SessionHead>, SessionStoreError> {
        let id = id.clone();
        self.in_read_txn(move |tx| {
            if let Some((head, _token)) = head_row_in_txn(tx, &id)? {
                return Ok(Some(head));
            }
            // Blob-only session: synthesize read-only (no write). The layout
            // is deterministic, so the token a caller derives here matches
            // the one the first migrating write persists.
            let Some(session) =
                load_session_snapshot_in_txn(tx, &id).map_err(into_session_store_error)?
            else {
                return Ok(None);
            };
            let (_layout, head) = layout_for_blob_session(&session)?;
            Ok(Some(head))
        })
        .await
    }

    async fn apply_prepared_head_canonical_mutation(
        &self,
        mutation: &PreparedHeadCanonicalMutation,
    ) -> Result<String, SessionStoreError> {
        let mutation = mutation.clone();
        self.in_write_txn(move |tx| {
            apply_prepared_head_canonical_mutation_in_txn(tx, &mutation)?;
            Ok(mutation.successor_head_token().to_string())
        })
        .await
    }

    async fn apply_prepared_head_canonical_rewrite_mutation(
        &self,
        mutation: &PreparedHeadCanonicalRewriteMutation,
    ) -> Result<String, SessionStoreError> {
        let mutation = mutation.clone();
        self.in_write_txn(move |tx| {
            apply_prepared_head_canonical_rewrite_mutation_in_txn(tx, &mutation)?;
            Ok(mutation.successor_head_token().to_string())
        })
        .await
    }

    async fn materialize_head(
        &self,
        expected: &SessionHead,
    ) -> Result<meerkat_core::VerifiedSessionHeadMaterialization, SessionStoreError> {
        let expected = expected.clone();
        self.in_read_txn(move |tx| {
            let (current, stored_token) = head_row_in_txn(tx, &expected.id)?
                .ok_or_else(|| SessionStoreError::NotFound(expected.id.clone()))?;
            let current_token = session_head_cas_token(&current)?;
            if current_token != stored_token {
                return Err(SessionStoreError::Corrupted(expected.id.clone()));
            }
            let expected_token = session_head_cas_token(&expected)?;
            if expected_token != current_token {
                return Err(SessionStoreError::TranscriptRevisionConflict {
                    id: expected.id.clone(),
                    expected: expected_token,
                    actual: current_token,
                });
            }
            verify_physical_head_canonical_in_txn(tx, &current)
        })
        .await
    }

    async fn load_messages(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        range: std::ops::Range<u64>,
    ) -> Result<Vec<Message>, SessionStoreError> {
        let id = id.clone();
        let strand = strand.clone();
        self.in_read_txn(move |tx| {
            if head_row_in_txn(tx, &id)?.is_some() {
                return strand_messages_in_txn(tx, &id, &strand, range);
            }
            let Some(session) =
                load_session_snapshot_in_txn(tx, &id).map_err(into_session_store_error)?
            else {
                return Err(SessionStoreError::NotFound(id));
            };
            let (layout, _head) = layout_for_blob_session(&session)?;
            let rows = blob_strand_messages(&session, &layout, &strand)?;
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
        id: &SessionId,
    ) -> Result<Vec<TranscriptRewriteRecord>, SessionStoreError> {
        let id = id.clone();
        self.in_read_txn(move |tx| {
            if let Some((head, _token)) = head_row_in_txn(tx, &id)? {
                let rows = rewrite_rows_in_txn(tx, &id, head.rewrite_count)?;
                // One link map for the whole reconstruction: each retained
                // body resolves through its exact overlay chain.
                let links = strand_links_in_txn(tx, &id)?;
                return rows
                    .into_iter()
                    .map(|row| {
                        let parent_messages = strand_messages_with_links_in_txn(
                            tx,
                            &id,
                            &row.parent_strand,
                            0..row.parent_len,
                            &links,
                        )?;
                        let revision_messages = strand_messages_with_links_in_txn(
                            tx,
                            &id,
                            &row.strand,
                            0..row.strand_len,
                            &links,
                        )?;
                        reconstruct_rewrite_record(
                            &id,
                            row.commit,
                            parent_messages,
                            revision_messages,
                        )
                    })
                    .collect();
            }
            let Some(session) =
                load_session_snapshot_in_txn(tx, &id).map_err(into_session_store_error)?
            else {
                return Ok(Vec::new());
            };
            let history = session
                .validated_transcript_history_state()
                .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                    id: id.clone(),
                    reason: format!("stored transcript history state is malformed: {error}"),
                })?;
            let (layout, _head) = layout_for_blob_session(&session)?;
            let Some(history) = history else {
                if layout.rewrites.is_empty() {
                    return Ok(Vec::new());
                }
                return Err(SessionStoreError::Corrupted(id));
            };
            layout
                .rewrites
                .into_iter()
                .map(|rewrite| {
                    let parent_messages = history
                        .materialize_rewrite_parent(&rewrite.commit)
                        .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                            id: id.clone(),
                            reason: format!("failed to materialize blob rewrite parent: {error}"),
                        })?
                        .messages;
                    let revision_messages = history
                        .materialize_rewrite_child(&rewrite.commit)
                        .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                            id: id.clone(),
                            reason: format!("failed to materialize blob rewrite child: {error}"),
                        })?
                        .messages;
                    reconstruct_rewrite_record(
                        &id,
                        rewrite.commit,
                        parent_messages,
                        revision_messages,
                    )
                })
                .collect()
        })
        .await
    }

    async fn load_canonical_head(
        &self,
        id: &SessionId,
    ) -> Result<Option<SessionHead>, SessionStoreError> {
        let id = id.clone();
        // Head row ONLY: unlike `load_head`, a blob-only session gets `None`
        // rather than an O(document) synthesized head (no blob read at all).
        self.in_read_txn(move |tx| Ok(head_row_in_txn(tx, &id)?.map(|(head, _token)| head)))
            .await
    }

    async fn load_rewrite_commits(
        &self,
        id: &SessionId,
    ) -> Result<Vec<TranscriptRewriteCommit>, SessionStoreError> {
        let id = id.clone();
        self.in_read_txn(move |tx| {
            if let Some((head, _token)) = head_row_in_txn(tx, &id)? {
                // The commit half of the rewrite rows only (bounded by the
                // adopted count); no strand-body reads.
                return Ok(rewrite_rows_in_txn(tx, &id, head.rewrite_count)?
                    .into_iter()
                    .map(|row| row.commit)
                    .collect());
            }
            // Blob-only session: derive the commits from the frozen blob's
            // layout so the answer always equals `load_rewrites`' commits.
            // (Blob-only sessions are never served by the fast read path —
            // `load_canonical_head` is `None` — so this arm is contract
            // parity, not a hot path.)
            let Some(session) =
                load_session_snapshot_in_txn(tx, &id).map_err(into_session_store_error)?
            else {
                return Ok(Vec::new());
            };
            let (layout, _head) = layout_for_blob_session(&session)?;
            Ok(layout
                .rewrites
                .into_iter()
                .map(|rewrite| rewrite.commit)
                .collect())
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, Message, SystemMessage, UserMessage,
    };
    use meerkat_core::{StopReason, TranscriptRewriteReason, TranscriptRewriteSelection};
    use tempfile::TempDir;

    fn temp_store() -> (TempDir, SqliteSessionStore) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let store = SqliteSessionStore::open(&path).unwrap();
        (dir, store)
    }

    #[test]
    fn busy_writer_is_retried_with_per_store_policy() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("busy.sqlite3");
        let options = SqliteConnectionOptions {
            busy_timeout: Duration::from_millis(250),
        };
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let holder_path = path.clone();
        let holder = std::thread::spawn(move || {
            let mut connection = open_connection_with_options(&holder_path, options).unwrap();
            let transaction =
                begin_immediate_transaction_with_options(&mut connection, options).unwrap();
            locked_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(120));
            transaction.commit().unwrap();
        });
        locked_rx.recv().unwrap();

        let mut contender = open_connection_with_options(&path, options).unwrap();
        let transaction = begin_immediate_transaction_with_options(&mut contender, options)
            .expect("bounded busy retry should survive the concurrent writer");
        transaction.commit().unwrap();
        holder.join().unwrap();
    }

    #[test]
    fn ensure_schema_stamps_the_session_domain_ledger() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("schema.sqlite3");
        let mut conn = open_connection(&path).unwrap();
        ensure_schema(&mut conn).unwrap();
        assert_eq!(
            meerkat_sqlite::domain_version(&conn, SESSION_STORE_DOMAIN.name).unwrap(),
            Some(SESSION_STORE_DOMAIN.supported_version())
        );
        // The DDL actually ran under the ledger.
        conn.query_row("SELECT COUNT(*) FROM session_heads", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        conn.query_row("SELECT COUNT(*) FROM session_component_events", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        for table in [
            "session_head_metadata_cells",
            "session_head_metadata_current",
            "session_head_metadata_states",
            "session_head_metadata_state_deltas",
            "session_head_metadata_refs",
            "session_head_metadata_head_lineage",
        ] {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        }
        let route_index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'index' AND name = 'session_head_metadata_cells_route_key_idx'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            route_index, 1,
            "changed-cell collision checks require the covering route/key index"
        );
        let metadata_cell_columns = conn
            .prepare("PRAGMA table_info(session_head_metadata_cells)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            metadata_cell_columns,
            [
                "session_id",
                "metadata_key",
                "key_route",
                "exact_value_digest",
                "metadata_json",
                "created_at_ms",
            ],
            "metadata cells have one exact byte-derived identity column"
        );
    }

    #[test]
    fn ensure_schema_refuses_a_future_domain_version() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("future.sqlite3");
        let mut conn = open_connection(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL)",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO meerkat_schema (domain, version) VALUES (?1, ?2)",
            params![
                SESSION_STORE_DOMAIN.name,
                SESSION_STORE_DOMAIN.supported_version() + 1
            ],
        )
        .unwrap();
        let err = ensure_schema(&mut conn).expect_err("future schema must be refused");
        assert!(
            matches!(err, StoreError::SchemaFromTheFuture { .. }),
            "unexpected error: {err:?}"
        );
    }

    #[tokio::test]
    async fn save_load_roundtrip() {
        let (_dir, store) = temp_store();
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("hello".to_string())));

        store.save(&session).await.unwrap();
        let loaded = store.load(session.id()).await.unwrap().unwrap();
        assert_eq!(loaded.id(), session.id());
        assert_eq!(loaded.messages().len(), 1);
    }

    #[tokio::test]
    async fn load_surfaces_corrupt_session_blob_as_serialization_error() {
        let (_dir, store) = temp_store();
        let session = Session::new();
        store.save(&session).await.unwrap();

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE sessions SET session_json = ?1 WHERE session_id = ?2",
            params![
                b"{ not a serialized Session".as_slice(),
                session.id().to_string()
            ],
        )
        .unwrap();

        let error = store
            .load(session.id())
            .await
            .expect_err("corrupt persisted Session bytes must fail load");
        assert!(
            matches!(error, SessionStoreError::Serialization(_)),
            "corrupt persisted Session bytes must remain a typed serialization error, got {error:?}"
        );
    }

    #[tokio::test]
    async fn list_is_ordered_by_updated_desc() {
        let (_dir, store) = temp_store();
        let first = Session::new();
        store.save(&first).await.unwrap();
        std::thread::sleep(Duration::from_millis(10));

        let second = Session::new();
        store.save(&second).await.unwrap();

        let sessions = store.list(SessionFilter::default()).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, *second.id());
        assert_eq!(sessions[1].id, *first.id());
    }

    #[tokio::test]
    async fn delete_removes_session() {
        let (_dir, store) = temp_store();
        let session = Session::new();
        store.save(&session).await.unwrap();
        store.delete(session.id()).await.unwrap();
        assert!(store.load(session.id()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn reopen_reads_existing_data() {
        let (dir, store) = temp_store();
        let session = Session::new();
        store.save(&session).await.unwrap();

        let reopened = SqliteSessionStore::open(dir.path().join("sessions.sqlite3")).unwrap();
        assert!(reopened.load(session.id()).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn two_handles_share_same_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let first = SqliteSessionStore::open(&path).unwrap();
        let second = SqliteSessionStore::open(&path).unwrap();

        let session = Session::new();
        first.save(&session).await.unwrap();
        let loaded = second.load(session.id()).await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn save_transcript_rewrite_rejects_stale_parent_after_intervening_save() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let first = SqliteSessionStore::open(&path).unwrap();
        let second = SqliteSessionStore::open(&path).unwrap();

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("hello".to_string())));
        session.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "original".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        )));
        first.save(&session).await.unwrap();

        let mut stale = first.load(session.id()).await.unwrap().unwrap();
        let mut newer = second.load(session.id()).await.unwrap().unwrap();
        newer.push(Message::User(UserMessage::text("intervening".to_string())));
        second.save(&newer).await.unwrap();

        let commit = stale
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage::new(
                    vec![AssistantBlock::Text {
                        text: "replacement".to_string(),
                        meta: None,
                    }],
                    StopReason::EndTurn,
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("test".to_string()),
                None,
            )
            .unwrap();

        let err = first
            .save_transcript_rewrite(&stale, &commit)
            .await
            .expect_err("stale rewrite must not overwrite newer session state");
        assert!(
            matches!(err, SessionStoreError::TranscriptRevisionConflict { .. }),
            "unexpected error: {err}"
        );

        let saved = first.load(session.id()).await.unwrap().unwrap();
        assert_eq!(saved.messages().len(), newer.messages().len());
    }

    #[tokio::test]
    async fn authoritative_projection_expected_revision_rejects_stale_writer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let first = SqliteSessionStore::open(&path).unwrap();
        let second = SqliteSessionStore::open(&path).unwrap();

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("base".to_string())));
        first.save(&session).await.unwrap();
        let expected_revision = session.transcript_revision().unwrap();

        let mut newer = second.load(session.id()).await.unwrap().unwrap();
        newer.push(Message::User(UserMessage::text("newer".to_string())));
        second.save(&newer).await.unwrap();

        let mut stale_projection = session.clone();
        stale_projection.push(Message::User(UserMessage::text("stale".to_string())));
        let err = first
            .save_authoritative_projection_if_current_revision(
                &stale_projection,
                Some(expected_revision),
            )
            .await
            .expect_err("stale authoritative projection should be rejected");
        assert!(
            matches!(err, SessionStoreError::TranscriptContinuityViolation { .. }),
            "unexpected error: {err}"
        );

        let saved = first.load(session.id()).await.unwrap().unwrap();
        assert_eq!(saved.messages().len(), newer.messages().len());
        assert_eq!(
            saved.transcript_revision().unwrap(),
            newer.transcript_revision().unwrap()
        );
    }

    #[tokio::test]
    async fn delete_if_current_revision_only_deletes_matching_projection() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let first = SqliteSessionStore::open(&path).unwrap();
        let second = SqliteSessionStore::open(&path).unwrap();

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("base".to_string())));
        first.save(&session).await.unwrap();
        let stale_token =
            meerkat_core::session_store::session_projection_cas_token(&session).unwrap();

        let mut newer = second.load(session.id()).await.unwrap().unwrap();
        newer.push(Message::User(UserMessage::text("newer".to_string())));
        second.save(&newer).await.unwrap();

        assert!(
            !first
                .delete_if_current_revision(session.id(), &stale_token)
                .await
                .unwrap()
        );
        assert!(first.load(session.id()).await.unwrap().is_some());

        let current_token =
            meerkat_core::session_store::session_projection_cas_token(&newer).unwrap();
        assert!(
            first
                .delete_if_current_revision(session.id(), &current_token)
                .await
                .unwrap()
        );
        assert!(first.load(session.id()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_fails_closed_on_negative_durable_counter() {
        // Gate (row #238): a durable row carrying a negative message_count is
        // an impossible-state projection. list() must surface a typed error
        // rather than laundering it to usize::MAX. OLD behavior:
        // `usize::try_from(...).unwrap_or(usize::MAX)` returned a fabricated
        // count and list() succeeded.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let store = SqliteSessionStore::open(&path).unwrap();

        let session = Session::new();
        store.save(&session).await.unwrap();

        // Corrupt the derived counter column directly on disk.
        let conn = open_connection(&path).unwrap();
        conn.execute(
            "UPDATE sessions SET message_count = -1 WHERE session_id = ?1",
            params![session.id().to_string()],
        )
        .unwrap();
        drop(conn);

        let err = store
            .list(SessionFilter::default())
            .await
            .expect_err("list must fail closed on a negative durable counter");
        // Negative counters surface through the typed StoreError boundary,
        // not as usize::MAX.
        assert!(
            matches!(err, SessionStoreError::Internal(_)),
            "unexpected error: {err}"
        );

        // Canonical truth still recoverable from session_json via load().
        let loaded = store.load(session.id()).await.unwrap().unwrap();
        assert_eq!(loaded.id(), session.id());
    }

    // -----------------------------------------------------------------------
    // Incremental session persistence (OB3 ask 11)
    // -----------------------------------------------------------------------

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text.to_string()))
    }

    fn incremental(store: &SqliteSessionStore) -> Arc<dyn IncrementalSessionStore> {
        let store = SqliteSessionStore::open(store.path()).unwrap();
        Arc::new(store)
            .as_incremental()
            .expect("sqlite store must expose the incremental capability")
    }

    fn prepared_root_with_metadata(
        key: &str,
        value: serde_json::Value,
    ) -> (Session, PreparedHeadCanonicalMutation) {
        let mut session = Session::new();
        session.set_metadata(key, value);
        session.push(user("root"));
        let mutation = PreparedHeadCanonicalMutation::prepare_root(&session)
            .expect("prepare rooted HeadCanonical mutation");
        (session, mutation)
    }

    #[tokio::test]
    async fn head_canonical_roundtrip_preserves_ordered_system_rows_exactly() {
        let (_dir, store) = temp_store();
        let incremental = incremental(&store);
        let mut session = Session::new();
        session.push(Message::System(SystemMessage::new("")));
        session.push(user("between"));
        session.push(Message::System(SystemMessage::new(" \t\n")));
        session.push(Message::System(SystemMessage::new("duplicate")));
        session.push(Message::System(SystemMessage::new("duplicate")));
        let expected = session.messages().to_vec();

        let root =
            PreparedHeadCanonicalMutation::prepare_root(&session).expect("prepare exact rows");
        incremental
            .apply_prepared_head_canonical_mutation(&root)
            .await
            .expect("persist exact rows");

        let loaded = store
            .load(session.id())
            .await
            .expect("load exact rows")
            .expect("session exists");
        assert_eq!(loaded.messages(), expected);
        let connection = open_connection(store.path()).expect("open store");
        let component_rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM session_component_events WHERE session_id = ?1",
                params![session.id().to_string()],
                |row| row.get(0),
            )
            .expect("count component rows");
        assert_eq!(
            component_rows, 0,
            "ordinary System rows must not mint out-of-band component events"
        );
    }

    #[tokio::test]
    async fn incremental_trait_applies_prepared_mutation_atomically_and_returns_exact_token() {
        let (_dir, store) = temp_store();
        let (_session, root) =
            prepared_root_with_metadata("application", serde_json::json!({"value": 1}));
        let incremental = incremental(&store);
        assert_eq!(
            incremental
                .apply_prepared_head_canonical_mutation(&root)
                .await
                .unwrap(),
            root.successor_head_token()
        );
        assert_eq!(
            incremental
                .apply_prepared_head_canonical_mutation(&root)
                .await
                .expect("exact reply-loss retry"),
            root.successor_head_token()
        );
    }

    #[tokio::test]
    async fn canonical_empty_root_is_the_only_zero_mutation_initial_metadata_state() {
        let (_dir, store) = temp_store();
        let mut session = Session::new();
        session.push(user("empty metadata root"));
        let root = PreparedHeadCanonicalMutation::prepare_root(&session)
            .expect("prepare canonical empty root");
        let identity = root
            .successor_head()
            .metadata_identity()
            .expect("authenticated metadata identity");
        assert!(identity.is_canonical_empty());
        assert!(root.metadata_projection().mutations().is_empty());
        let persisted = root.clone();
        store
            .in_write_txn(move |tx| apply_prepared_head_canonical_mutation_in_txn(tx, &persisted))
            .await
            .unwrap();
        let exact_retry = root.clone();
        store
            .in_write_txn(move |tx| apply_prepared_head_canonical_mutation_in_txn(tx, &exact_retry))
            .await
            .expect("canonical empty root must be exactly retryable");
    }

    #[tokio::test]
    async fn authenticated_per_key_metadata_is_compact_hot_and_complete_cold() {
        let (_dir, store) = temp_store();
        let (mut session, root) =
            prepared_root_with_metadata("application", serde_json::json!({"value": 1}));
        let persisted = root.clone();
        store
            .in_write_txn(move |tx| apply_prepared_head_canonical_mutation_in_txn(tx, &persisted))
            .await
            .unwrap();
        root.acknowledge_session(&mut session, root.successor_head_token())
            .unwrap();

        let compact = incremental(&store)
            .load_head(session.id())
            .await
            .unwrap()
            .unwrap();
        assert!(compact.metadata_identity().is_some());
        assert!(
            compact.metadata_projection().is_none(),
            "ordinary load_head must not parse/hash metadata cells"
        );
        let materialized = incremental(&store)
            .materialize_head(&compact)
            .await
            .expect("explicit current-head materialization");
        assert_eq!(
            materialized.session().metadata().get("application"),
            Some(&serde_json::json!({"value": 1}))
        );
        assert!(materialized.head().realtime_event_prefix.is_some());
        session.push(user("ordinary delta"));
        let successor =
            PreparedHeadCanonicalMutation::prepare(&session, Some(compact.clone())).unwrap();
        assert_eq!(
            successor
                .predecessor_head()
                .and_then(SessionHead::metadata_identity),
            successor.successor_head().metadata_identity(),
            "ordinary transcript deltas must retain metadata identity"
        );
        let persisted = successor.clone();
        store
            .in_write_txn(move |tx| apply_prepared_head_canonical_mutation_in_txn(tx, &persisted))
            .await
            .unwrap();
        assert!(matches!(
            incremental(&store).materialize_head(&compact).await,
            Err(SessionStoreError::TranscriptRevisionConflict { .. })
        ));
        let stable_successor = successor.successor_head().clone();
        store
            .in_write_txn(move |tx| {
                let current = head_row_in_txn(tx, &stable_successor.id)?
                    .ok_or_else(|| SessionStoreError::NotFound(stable_successor.id.clone()))?;
                let before = tx
                    .query_row("SELECT total_changes()", [], |row| row.get::<_, i64>(0))
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                let stable_token = session_head_cas_token(&stable_successor)?;
                reconcile_head_metadata_transition_in_txn(
                    tx,
                    Some(&current.0),
                    Some(&current.1),
                    &stable_successor,
                    &stable_token,
                )?;
                let after = tx
                    .query_row("SELECT total_changes()", [], |row| row.get::<_, i64>(0))
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                assert_eq!(
                    after, before,
                    "same physical owner/digest must be a literal no-write path"
                );
                Ok(())
            })
            .await
            .unwrap();

        let loaded = store.load(session.id()).await.unwrap().unwrap();
        assert_eq!(
            loaded.metadata().get("application"),
            Some(&serde_json::json!({"value": 1}))
        );
        let meta = store.load_meta(session.id()).await.unwrap().unwrap();
        assert_eq!(
            meta.metadata.get("application"),
            Some(&serde_json::json!({"value": 1}))
        );
        let listed = store.list(SessionFilter::default()).await.unwrap();
        assert_eq!(
            listed[0].metadata.get("application"),
            Some(&serde_json::json!({"value": 1}))
        );

        let conn = open_connection(store.path()).unwrap();
        let inventory: (i64, i64, i64, i64, i64, i64, i64) = conn
            .query_row(
                r"
                SELECT
                    (SELECT COUNT(*) FROM session_head_metadata_cells
                     WHERE session_id = ?1),
                    (SELECT COUNT(*) FROM session_head_metadata_current
                     WHERE session_id = ?1),
                    (SELECT COUNT(*) FROM session_head_metadata_states
                     WHERE session_id = ?1),
                    (SELECT COUNT(*) FROM session_head_metadata_state_deltas
                     WHERE session_id = ?1),
                    (SELECT COUNT(*) FROM session_head_metadata_refs
                     WHERE session_id = ?1 AND owner = 'physical_head'),
                    (SELECT COUNT(*) FROM session_head_metadata_refs
                     WHERE session_id = ?1 AND owner = 'runtime_boundary'),
                    (SELECT COUNT(*) FROM session_head_metadata_head_lineage
                     WHERE session_id = ?1)
                ",
                params![session.id().to_string()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            inventory,
            (1, 1, 1, 1, 1, 0, 1),
            "an unchanged metadata root must reuse its one cell/state/delta/lineage"
        );
    }

    #[tokio::test]
    async fn metadata_reachability_bounds_owner_states_and_direct_retry_witness() {
        let (_dir, store) = temp_store();
        let (mut session, root) =
            prepared_root_with_metadata("application", serde_json::json!({"value": 1}));
        let persisted = root.clone();
        store
            .in_write_txn(move |tx| apply_prepared_head_canonical_mutation_in_txn(tx, &persisted))
            .await
            .unwrap();
        root.acknowledge_session(&mut session, root.successor_head_token())
            .unwrap();
        let runtime_boundary = root.successor_head().clone();
        let unretained = runtime_boundary.clone();
        assert!(matches!(
            store
                .in_read_txn(move |tx| {
                    verify_runtime_boundary_head_canonical_in_txn(tx, &unretained)
                })
                .await,
            Err(SessionStoreError::Corrupted(_))
        ));
        store
            .in_write_txn(move |tx| {
                retain_runtime_boundary_head_metadata_in_txn(tx, &runtime_boundary)
            })
            .await
            .unwrap();
        let stable_runtime_boundary = root.successor_head().clone();
        store
            .in_write_txn(move |tx| {
                let before = tx
                    .query_row("SELECT total_changes()", [], |row| row.get::<_, i64>(0))
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                retain_runtime_boundary_head_metadata_in_txn(tx, &stable_runtime_boundary)?;
                let after = tx
                    .query_row("SELECT total_changes()", [], |row| row.get::<_, i64>(0))
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                assert_eq!(
                    after, before,
                    "same runtime owner/digest must be a literal no-write path"
                );
                Ok(())
            })
            .await
            .unwrap();

        session.set_metadata("application", serde_json::json!({"value": 2}));
        let observed = incremental(&store)
            .load_head(session.id())
            .await
            .unwrap()
            .unwrap();
        let successor = PreparedHeadCanonicalMutation::prepare(&session, Some(observed)).unwrap();
        let persisted = successor.clone();
        store
            .in_write_txn(move |tx| apply_prepared_head_canonical_mutation_in_txn(tx, &persisted))
            .await
            .unwrap();

        let conn = open_connection(store.path()).unwrap();
        let divergent: (i64, i64, i64, i64, i64, i64, i64) = conn
            .query_row(
                r"
                SELECT
                    (SELECT COUNT(*) FROM session_head_metadata_cells
                     WHERE session_id = ?1),
                    (SELECT COUNT(*) FROM session_head_metadata_current
                     WHERE session_id = ?1),
                    (SELECT COUNT(*) FROM session_head_metadata_states
                     WHERE session_id = ?1),
                    (SELECT COUNT(*) FROM session_head_metadata_state_deltas
                     WHERE session_id = ?1),
                    (SELECT COUNT(*) FROM session_head_metadata_refs
                     WHERE session_id = ?1 AND owner = 'physical_head'),
                    (SELECT COUNT(*) FROM session_head_metadata_refs
                     WHERE session_id = ?1 AND owner = 'runtime_boundary'),
                    (SELECT COUNT(*) FROM session_head_metadata_head_lineage
                     WHERE session_id = ?1)
                ",
                params![session.id().to_string()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            divergent,
            (2, 1, 2, 2, 1, 1, 2),
            "divergent physical/runtime owners must retain both authenticated transitions"
        );
        drop(conn);
        let exact_retry = root.clone();
        store
            .in_read_txn(move |tx| {
                verify_prepared_head_canonical_rows_for_exact_retry_in_txn(tx, &exact_retry)
            })
            .await
            .expect("runtime-retained predecessor rows must remain exactly retryable");

        let new_runtime_boundary = successor.successor_head().clone();
        store
            .in_write_txn(move |tx| {
                retain_runtime_boundary_head_metadata_in_txn(tx, &new_runtime_boundary)
            })
            .await
            .unwrap();
        let conn = open_connection(store.path()).unwrap();
        let converged: (i64, i64, i64, i64, i64, i64, i64) = conn
            .query_row(
                r"
                SELECT
                    (SELECT COUNT(*) FROM session_head_metadata_cells
                     WHERE session_id = ?1),
                    (SELECT COUNT(*) FROM session_head_metadata_current
                     WHERE session_id = ?1),
                    (SELECT COUNT(*) FROM session_head_metadata_states
                     WHERE session_id = ?1),
                    (SELECT COUNT(*) FROM session_head_metadata_state_deltas
                     WHERE session_id = ?1),
                    (SELECT COUNT(*) FROM session_head_metadata_refs
                     WHERE session_id = ?1 AND owner = 'physical_head'),
                    (SELECT COUNT(*) FROM session_head_metadata_refs
                     WHERE session_id = ?1 AND owner = 'runtime_boundary'),
                    (SELECT COUNT(*) FROM session_head_metadata_head_lineage
                     WHERE session_id = ?1)
                ",
                params![session.id().to_string()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            converged,
            (2, 1, 2, 1, 1, 1, 1),
            "convergence must retain only the current state and its direct retry witness"
        );
        drop(conn);
        assert!(
            store.delete(session.id()).await.is_err(),
            "session deletion must not strand a retained runtime boundary"
        );
        let session_id = session.id().clone();
        store
            .in_write_txn(move |tx| clear_runtime_boundary_head_metadata_in_txn(tx, &session_id))
            .await
            .unwrap();
        store.delete(session.id()).await.unwrap();
        let conn = open_connection(store.path()).unwrap();
        for table in [
            "session_head_metadata_cells",
            "session_head_metadata_current",
            "session_head_metadata_states",
            "session_head_metadata_state_deltas",
            "session_head_metadata_refs",
            "session_head_metadata_head_lineage",
        ] {
            let retained: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE session_id = ?1"),
                    params![session.id().to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(retained, 0, "table {table} must be cleared by delete");
        }
    }

    #[tokio::test]
    async fn standalone_one_hot_metadata_mutations_keep_two_cells_and_one_retry_delta() {
        let (_dir, store) = temp_store();
        let mut session = Session::new();
        session.set_metadata("hot", serde_json::json!({"value": 0}));
        session.set_metadata(
            "large-cold",
            serde_json::json!({"payload": "x".repeat(16_384)}),
        );
        session.push(user("root"));
        let root = PreparedHeadCanonicalMutation::prepare_root(&session)
            .expect("prepare rooted HeadCanonical mutation");
        let persisted_root = root.clone();
        store
            .in_write_txn(move |tx| {
                apply_prepared_head_canonical_mutation_in_txn(tx, &persisted_root)
            })
            .await
            .unwrap();
        root.acknowledge_session(&mut session, root.successor_head_token())
            .unwrap();

        for value in 1..=16 {
            session.set_metadata("hot", serde_json::json!({"value": value}));
            let observed = incremental(&store)
                .load_head(session.id())
                .await
                .unwrap()
                .unwrap();
            let successor =
                PreparedHeadCanonicalMutation::prepare(&session, Some(observed)).unwrap();
            assert_eq!(
                successor.metadata_projection().mutations().len(),
                1,
                "one hot-key update must carry exactly one authenticated mutation"
            );
            let persisted = successor.clone();
            store
                .in_write_txn(move |tx| {
                    apply_prepared_head_canonical_mutation_in_txn(tx, &persisted)
                })
                .await
                .unwrap();
            successor
                .acknowledge_session(&mut session, successor.successor_head_token())
                .unwrap();

            let exact_retry = successor.clone();
            store
                .in_write_txn(move |tx| {
                    apply_prepared_head_canonical_mutation_in_txn(tx, &exact_retry)
                })
                .await
                .expect("the latest changed-cell bytes, proof, state, and lineage must re-prove");

            let conn = open_connection(store.path()).unwrap();
            let bounded: (i64, i64, i64, i64, i64, i64, i64) = conn
                .query_row(
                    r"
                    SELECT
                        (SELECT COUNT(*) FROM session_head_metadata_cells
                         WHERE session_id = ?1),
                        (SELECT COUNT(*) FROM session_head_metadata_current
                         WHERE session_id = ?1),
                        (SELECT COUNT(*) FROM session_head_metadata_states
                         WHERE session_id = ?1),
                        (SELECT COUNT(*) FROM session_head_metadata_state_deltas
                         WHERE session_id = ?1),
                        (SELECT COUNT(*) FROM session_head_metadata_refs
                         WHERE session_id = ?1 AND owner = 'physical_head'),
                        (SELECT COUNT(*) FROM session_head_metadata_refs
                         WHERE session_id = ?1 AND owner = 'runtime_boundary'),
                        (SELECT COUNT(*) FROM session_head_metadata_head_lineage
                         WHERE session_id = ?1)
                    ",
                    params![session.id().to_string()],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                        ))
                    },
                )
                .unwrap();
            assert_eq!(
                bounded,
                (3, 2, 2, 1, 1, 0, 1),
                "a standalone store must keep one cold cell plus current+retry hot versions"
            );
        }

        session.push(user("metadata-stable transcript delta"));
        let observed = incremental(&store)
            .load_head(session.id())
            .await
            .unwrap()
            .unwrap();
        let stable = PreparedHeadCanonicalMutation::prepare(&session, Some(observed)).unwrap();
        assert!(stable.metadata_projection().mutations().is_empty());
        let stable = stable.clone();
        store
            .in_write_txn(move |tx| {
                let before = tx
                    .query_row("SELECT total_changes()", [], |row| row.get::<_, i64>(0))
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                reconcile_head_metadata_transition_in_txn(
                    tx,
                    stable.predecessor_head(),
                    stable.predecessor_head_token(),
                    stable.successor_head(),
                    stable.successor_head_token(),
                )?;
                let after = tx
                    .query_row("SELECT total_changes()", [], |row| row.get::<_, i64>(0))
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                assert_eq!(
                    after, before,
                    "zero-dirty metadata must produce no SQLite metadata change and therefore no metadata WAL record"
                );
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn corrupted_metadata_cell_fails_cold_reads_as_corruption() {
        let (_dir, store) = temp_store();
        let (session, root) =
            prepared_root_with_metadata("application", serde_json::json!({"value": 1}));
        let initial = root.clone();
        store
            .in_write_txn(move |tx| apply_prepared_head_canonical_mutation_in_txn(tx, &initial))
            .await
            .unwrap();
        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            r"
            UPDATE session_head_metadata_cells
            SET metadata_json = X'7B7D'
            WHERE session_id = ?1
            ",
            params![session.id().to_string()],
        )
        .unwrap();
        drop(conn);

        let error = store
            .in_write_txn(move |tx| apply_prepared_head_canonical_mutation_in_txn(tx, &root))
            .await
            .expect_err("exact retry must byte-compare its physical metadata cell");
        assert!(matches!(error, SessionStoreError::Corrupted(id) if id == *session.id()));
        assert!(matches!(
            store.load(session.id()).await,
            Err(SessionStoreError::Corrupted(id)) if id == *session.id()
        ));
        assert!(matches!(
            store.load_meta(session.id()).await,
            Err(SessionStoreError::Corrupted(id)) if id == *session.id()
        ));
    }

    #[tokio::test]
    async fn exact_retry_recomputes_the_retained_predecessor_cell_bytes() {
        let (_dir, store) = temp_store();
        let (mut session, root) =
            prepared_root_with_metadata("application", serde_json::json!({"value": 1}));
        let persisted_root = root.clone();
        store
            .in_write_txn(move |tx| {
                apply_prepared_head_canonical_mutation_in_txn(tx, &persisted_root)
            })
            .await
            .unwrap();
        let runtime_root = root.successor_head().clone();
        store
            .in_write_txn(move |tx| retain_runtime_boundary_head_metadata_in_txn(tx, &runtime_root))
            .await
            .unwrap();
        root.acknowledge_session(&mut session, root.successor_head_token())
            .unwrap();

        session.set_metadata("application", serde_json::json!({"value": 2}));
        let observed = incremental(&store)
            .load_head(session.id())
            .await
            .unwrap()
            .unwrap();
        let successor = PreparedHeadCanonicalMutation::prepare(&session, Some(observed)).unwrap();
        let persisted = successor.clone();
        store
            .in_write_txn(move |tx| apply_prepared_head_canonical_mutation_in_txn(tx, &persisted))
            .await
            .unwrap();
        let runtime_successor = successor.successor_head().clone();
        store
            .in_write_txn(move |tx| {
                retain_runtime_boundary_head_metadata_in_txn(tx, &runtime_successor)
            })
            .await
            .unwrap();

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            r"
            UPDATE session_head_metadata_cells
            SET metadata_json = X'7B7D'
            WHERE session_id = ?1
              AND metadata_key = 'application'
              AND exact_value_digest <> (
                  SELECT exact_value_digest
                  FROM session_head_metadata_current
                  WHERE session_id = ?1 AND metadata_key = 'application'
              )
            ",
            params![session.id().to_string()],
        )
        .unwrap();
        drop(conn);

        let exact_retry = successor.clone();
        let error = store
            .in_read_txn(move |tx| {
                verify_prepared_head_canonical_rows_for_exact_retry_in_txn(tx, &exact_retry)
            })
            .await
            .expect_err("retry must recompute and reject corrupt retained predecessor bytes");
        assert!(matches!(error, SessionStoreError::Corrupted(id) if id == *session.id()));
    }

    #[tokio::test]
    async fn list_pages_compact_heads_before_metadata_hydration() {
        let (_dir, store) = temp_store();
        let (newer, newer_root) =
            prepared_root_with_metadata("application", serde_json::json!({"value": "newer"}));
        let (older, older_root) =
            prepared_root_with_metadata("application", serde_json::json!({"value": "older"}));
        store
            .in_write_txn(move |tx| {
                apply_prepared_head_canonical_mutation_in_txn(tx, &newer_root)?;
                apply_prepared_head_canonical_mutation_in_txn(tx, &older_root)?;
                Ok(())
            })
            .await
            .unwrap();

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_heads SET updated_at_ms = 2000 WHERE session_id = ?1",
            params![newer.id().to_string()],
        )
        .unwrap();
        conn.execute(
            "UPDATE session_heads SET updated_at_ms = 1000 WHERE session_id = ?1",
            params![older.id().to_string()],
        )
        .unwrap();
        conn.execute(
            "UPDATE session_head_metadata_cells SET metadata_json = X'7B7D' \
             WHERE session_id = ?1",
            params![older.id().to_string()],
        )
        .unwrap();
        drop(conn);

        let first_page = store
            .list(SessionFilter {
                limit: Some(1),
                ..SessionFilter::default()
            })
            .await
            .expect("unselected corrupt metadata payload must not be hydrated");
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].id, *newer.id());

        assert!(matches!(
            store
                .list(SessionFilter {
                    limit: Some(1),
                    offset: Some(1),
                    ..SessionFilter::default()
                })
                .await,
            Err(SessionStoreError::Corrupted(id)) if id == *older.id()
        ));
    }

    #[tokio::test]
    async fn prepared_retry_refuses_a_missing_metadata_cell() {
        let (_dir, store) = temp_store();
        let (session, root) =
            prepared_root_with_metadata("application", serde_json::json!({"value": 1}));
        let initial = root.clone();
        store
            .in_write_txn(move |tx| apply_prepared_head_canonical_mutation_in_txn(tx, &initial))
            .await
            .unwrap();
        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "DELETE FROM session_head_metadata_cells WHERE session_id = ?1",
            params![session.id().to_string()],
        )
        .unwrap();
        drop(conn);

        let error = store
            .in_write_txn(move |tx| apply_prepared_head_canonical_mutation_in_txn(tx, &root))
            .await
            .expect_err("exact retry must point-check its immutable metadata cell");
        assert!(matches!(error, SessionStoreError::Corrupted(id) if id == *session.id()));
    }

    /// Seed a head-canonical session through the incremental contract:
    /// root strand rows + a Create head.
    async fn seed_incremental(
        inc: &Arc<dyn IncrementalSessionStore>,
        session: &Session,
    ) -> SessionHead {
        let root = TranscriptStrandId::root();
        inc.append_messages(session.id(), &root, 0, session.messages())
            .await
            .unwrap();
        let head = SessionHead::from_session(session, root, 0).unwrap();
        inc.save_head(&head, SessionHeadCas::Create).await.unwrap();
        head
    }

    fn strand_row_count(path: &Path, id: &SessionId, strand: &TranscriptStrandId) -> i64 {
        let conn = open_connection(path).unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM session_strand_messages WHERE session_id = ?1 AND strand = ?2",
            params![id.to_string(), strand.as_str()],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// Every strand row the session persists, across all strands.
    fn total_strand_rows(path: &Path, id: &SessionId) -> i64 {
        let conn = open_connection(path).unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM session_strand_messages WHERE session_id = ?1",
            params![id.to_string()],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn compact_layout_physical_row_count(layout: &StrandLayout) -> usize {
        let rewrite_rows = layout
            .rewrites
            .iter()
            .map(|rewrite| {
                let parent_splice_rows = match &rewrite.parent_transition {
                    PreparedHeadCanonicalParentTransition::ExactAppend => 0,
                    PreparedHeadCanonicalParentTransition::ExactSplice(parent_splice) => {
                        parent_splice.serialized_replacement().len()
                    }
                };
                parent_splice_rows
                    + rewrite.serialized_parent_suffix.len()
                    + rewrite.serialized_replacement.len()
            })
            .sum::<usize>();
        layout.serialized_anchor.len() + rewrite_rows + layout.serialized_tail.len()
    }

    fn strand_link_count(path: &Path, id: &SessionId) -> i64 {
        let conn = open_connection(path).unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM session_strand_links WHERE session_id = ?1",
            params![id.to_string()],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn blob_row_bytes(path: &Path, id: &SessionId) -> Option<Vec<u8>> {
        let conn = open_connection(path).unwrap();
        conn.query_row(
            "SELECT session_json FROM sessions WHERE session_id = ?1",
            params![id.to_string()],
            |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
        )
        .optional()
        .unwrap()
    }

    #[tokio::test]
    async fn prepared_rewrite_persists_only_delta_rows_and_supports_linked_tail_append() {
        let (_dir, store) = temp_store();
        let incremental = incremental(&store);
        let mut session = Session::new();
        session.set_metadata("application", serde_json::json!({"value": 1}));
        for turn in 0..64 {
            session.push(user(&format!("turn {turn}")));
        }
        let root = PreparedHeadCanonicalMutation::prepare_root(&session).expect("prepare root");
        incremental
            .apply_prepared_head_canonical_mutation(&root)
            .await
            .expect("persist root");
        root.acknowledge_session(&mut session, root.successor_head_token())
            .expect("acknowledge root");
        let runtime_head = root.successor_head().clone();

        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 10, end: 11 },
                vec![user("replacement")],
                TranscriptRewriteReason::new("delta topology test"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("commit rewrite");
        let rewrite = PreparedHeadCanonicalRewriteMutation::prepare_intra_turn(
            &session,
            &runtime_head,
            runtime_head.clone(),
        )
        .expect("prepare rewrite");
        assert_eq!(rewrite.steps().len(), 1);
        assert!(rewrite.steps()[0].serialized_parent_suffix().is_empty());
        assert_eq!(rewrite.steps()[0].serialized_replacement().len(), 1);
        let rewrite_strand = rewrite.steps()[0].strand().clone();
        assert_eq!(
            incremental
                .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                .await
                .expect("persist rewrite"),
            rewrite.successor_head_token()
        );
        assert_eq!(
            incremental
                .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                .await
                .expect("exact rewrite retry"),
            rewrite.successor_head_token()
        );
        let replacement_only = store
            .load(session.id())
            .await
            .expect("replacement-only sparse head must cold replay")
            .expect("session present");
        assert_eq!(
            replacement_only.messages(),
            session.messages(),
            "an empty direct tail may end physically at the replacement span, below the logical head count"
        );
        assert_eq!(
            strand_row_count(store.path(), session.id(), &rewrite_strand),
            1
        );
        assert_eq!(strand_link_count(store.path(), session.id()), 1);
        assert_eq!(
            total_strand_rows(store.path(), session.id()),
            65,
            "one one-row rewrite must add one physical row, not another 64-row document"
        );

        rewrite
            .acknowledge_physical_projection(&mut session, rewrite.successor_head_token())
            .expect("acknowledge rewrite projection");
        session.push(user("linked tail"));
        let tail = PreparedHeadCanonicalMutation::prepare_intra_turn(
            &session,
            &runtime_head,
            rewrite.successor_head().clone(),
        )
        .expect("prepare linked tail");
        assert_eq!(tail.serialized_suffix().len(), 1);
        incremental
            .apply_prepared_head_canonical_mutation(&tail)
            .await
            .expect("append linked tail");
        tail.acknowledge_physical_projection(&mut session, tail.successor_head_token())
            .expect("acknowledge linked tail");

        assert_eq!(
            strand_row_count(store.path(), session.id(), &rewrite_strand),
            2
        );
        assert_eq!(
            total_strand_rows(store.path(), session.id()),
            66,
            "ordinary append on a linked head must add only its one tail row"
        );
        let replay_head = tail.successor_head().clone();
        let (decoded, loaded_links) = store
            .in_read_txn(move |tx| {
                let replayed = replay_head_canonical_rows_in_txn(tx, &replay_head)?;
                Ok((
                    replayed.decoded_rewrite_count,
                    replayed.loaded_link_row_count,
                ))
            })
            .await
            .expect("bounded post-anchor replay");
        assert_eq!(
            decoded, 1,
            "ordinary materialization must decode only the one post-anchor rewrite"
        );
        assert_eq!(
            loaded_links, 1,
            "ordinary materialization must point-read only the active child link"
        );
        let loaded = store
            .load(session.id())
            .await
            .expect("load linked head")
            .expect("session present");
        assert_eq!(loaded.messages(), session.messages());
    }

    #[tokio::test]
    async fn ordinary_cold_load_skips_long_pre_anchor_rewrite_history() {
        let (_dir, store) = temp_store();
        let mut session = Session::new();
        for turn in 0..32 {
            session.push(user(&format!("turn {turn}")));
        }
        for generation in 0..24 {
            session
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                    vec![user(&format!("summary {generation}"))],
                    TranscriptRewriteReason::new("bounded cold-load fixture"),
                    Some("unit-test".to_string()),
                    None,
                )
                .expect("commit fixture rewrite");
        }
        let expected_messages = session.messages().to_vec();
        let id = session.id().clone();
        let (layout, head) = layout_for_blob_session(&session).expect("prepare canonical layout");
        assert_eq!(
            head.row_lineage_anchor
                .as_ref()
                .expect("current bounded anchor")
                .rewrite_count(),
            head.rewrite_count
        );
        assert!(head.rewrite_count >= 24);
        let head_for_write = head.clone();
        let layout_for_write = layout.clone();
        store
            .in_write_txn(move |tx| {
                persist_blob_strand_layout_in_txn(tx, &id, &layout_for_write)?;
                let links = strand_links_in_txn(tx, &id)?;
                let settled = settle_resolved_strand_direct_in_txn(
                    tx,
                    &id,
                    &head_for_write.strand,
                    head_for_write.message_count,
                    &links,
                )?;
                let observed = SessionMessageRowPrefixAccumulator::from_serialized_rows(&settled)?;
                let anchor = head_for_write
                    .row_lineage_anchor
                    .as_ref()
                    .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
                if observed != *anchor.materialized_prefix() {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
                write_head_row_only_in_txn(tx, &head_for_write)?;
                Ok(())
            })
            .await
            .expect("persist current canonical fixture");

        let head_for_cost = head.clone();
        let (decoded, loaded_links) = store
            .in_read_txn(move |tx| {
                let replayed = replay_head_canonical_rows_in_txn(tx, &head_for_cost)?;
                assert_eq!(replayed.serialized_rows.len(), expected_messages.len());
                Ok((
                    replayed.decoded_rewrite_count,
                    replayed.loaded_link_row_count,
                ))
            })
            .await
            .expect("bounded replay");
        assert_eq!(
            decoded, 0,
            "a current anchor must make all preceding rewrite rows invisible to ordinary load"
        );
        assert_eq!(
            loaded_links, 0,
            "a settled current anchor must not load any historical topology"
        );
        assert!(
            strand_link_count(store.path(), session.id()) > 0,
            "the cost probe must retain historical links so zero loaded links cannot be an empty-history artifact"
        );

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_rewrites SET graph_edge_json = X'7B7D'
             WHERE session_id = ?1 AND rewrite_idx = 0",
            params![session.id().to_string()],
        )
        .unwrap();
        drop(conn);
        let loaded = store
            .load(session.id())
            .await
            .expect("ordinary load must not deep-audit pre-anchor history")
            .expect("session present");
        assert_eq!(loaded.messages(), session.messages());

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_strand_links SET successor = strand
             WHERE rowid = (
                 SELECT rowid FROM session_strand_links
                 WHERE session_id = ?1
                 LIMIT 1
             )",
            params![session.id().to_string()],
        )
        .unwrap();
        drop(conn);
        let loaded = store
            .load(session.id())
            .await
            .expect("ordinary load must not traverse disconnected historical links")
            .expect("session present");
        assert_eq!(loaded.messages(), session.messages());

        let conn = open_connection(store.path()).unwrap();
        let original_row: Vec<u8> = conn
            .query_row(
                "SELECT message_json FROM session_strand_messages
                 WHERE session_id = ?1 AND strand = ?2 AND seq = 0",
                params![session.id().to_string(), head.strand.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        let mut representation_only_corruption = original_row.clone();
        representation_only_corruption.push(b' ');
        conn.execute(
            "UPDATE session_strand_messages SET message_json = ?3
             WHERE session_id = ?1 AND strand = ?2 AND seq = 0",
            params![
                session.id().to_string(),
                head.strand.as_str(),
                representation_only_corruption
            ],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            store.load(session.id()).await,
            Err(SessionStoreError::Corrupted(corrupted)) if corrupted == *session.id()
        ));

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_strand_messages SET message_json = ?3
             WHERE session_id = ?1 AND strand = ?2 AND seq = 0",
            params![session.id().to_string(), head.strand.as_str(), original_row],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM session_strand_messages
             WHERE session_id = ?1 AND strand = ?2 AND seq = 0",
            params![session.id().to_string(), head.strand.as_str()],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            store.load(session.id()).await,
            Err(SessionStoreError::Corrupted(corrupted)) if corrupted == *session.id()
        ));
    }

    #[tokio::test]
    async fn prepared_rewrites_rotate_and_settle_the_active_topology_budget() {
        let (_dir, store) = temp_store();
        let incremental = incremental(&store);
        let mut session = Session::new();
        session.set_metadata("application", serde_json::json!({"value": 1}));
        for turn in 0..16 {
            session.push(user(&format!("turn {turn}")));
        }
        let root = PreparedHeadCanonicalMutation::prepare_root(&session).expect("prepare root");
        incremental
            .apply_prepared_head_canonical_mutation(&root)
            .await
            .expect("persist root");
        root.acknowledge_session(&mut session, root.successor_head_token())
            .expect("acknowledge root");
        let mut observed_head = root.successor_head().clone();

        let rewrite_total = SESSION_ROW_LINEAGE_REBASE_INTERVAL + 8;
        for generation in 0..rewrite_total {
            session
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                    vec![user(&format!("summary {generation}"))],
                    TranscriptRewriteReason::new("active topology budget probe"),
                    Some("unit-test".to_string()),
                    None,
                )
                .expect("commit rewrite");
            let runtime_boundary_head = observed_head.clone();
            let rewrite = PreparedHeadCanonicalRewriteMutation::prepare_intra_turn(
                &session,
                &runtime_boundary_head,
                observed_head,
            )
            .expect("prepare bounded rewrite");
            incremental
                .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                .await
                .expect("persist bounded rewrite");
            if generation + 1 == SESSION_ROW_LINEAGE_REBASE_INTERVAL {
                incremental
                    .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                    .await
                    .expect("settled anchor rewrite retries exactly");
            }
            rewrite
                .acknowledge_physical_projection(&mut session, rewrite.successor_head_token())
                .expect("acknowledge rewrite");
            observed_head = rewrite.successor_head().clone();
        }

        let anchor = observed_head
            .row_lineage_anchor
            .as_ref()
            .expect("bounded row-lineage anchor");
        assert_eq!(
            anchor.rewrite_count(),
            SESSION_ROW_LINEAGE_REBASE_INTERVAL,
            "the interval rewrite must rotate the store-issued replay origin"
        );
        let replay_head = observed_head.clone();
        let (decoded, loaded_links) = store
            .in_read_txn(move |tx| {
                let replayed = replay_head_canonical_rows_in_txn(tx, &replay_head)?;
                Ok((
                    replayed.decoded_rewrite_count,
                    replayed.loaded_link_row_count,
                ))
            })
            .await
            .expect("bounded active-topology replay");
        assert_eq!(decoded, 8);
        assert_eq!(
            loaded_links, 8,
            "ordinary replay must see only post-anchor active links"
        );
        assert!(
            usize::try_from(strand_link_count(store.path(), session.id())).unwrap() > loaded_links,
            "historical links must exist without being loaded by ordinary resume"
        );
        let named_id = session.id().clone();
        let named_strand = observed_head.strand.clone();
        let named_count = observed_head.message_count;
        let (named_messages, named_link_rows) = store
            .in_read_txn(move |tx| {
                strand_messages_with_bounded_topology_in_txn(
                    tx,
                    &named_id,
                    &named_strand,
                    0..named_count,
                )
            })
            .await
            .expect("bounded named-strand read");
        assert_eq!(named_messages, session.messages());
        assert_eq!(
            named_link_rows, 8,
            "load_messages must point-read only post-anchor active link rows"
        );
        let loaded = store
            .load(session.id())
            .await
            .expect("load bounded topology")
            .expect("session present");
        assert_eq!(loaded.messages(), session.messages());
    }

    #[tokio::test]
    async fn prepared_rewrite_exact_retry_point_checks_every_named_row_and_link() {
        let (_dir, store) = temp_store();
        let incremental = incremental(&store);
        let mut session = Session::new();
        for turn in 0..8 {
            session.push(user(&format!("turn {turn}")));
        }
        let root = PreparedHeadCanonicalMutation::prepare_root(&session).expect("prepare root");
        incremental
            .apply_prepared_head_canonical_mutation(&root)
            .await
            .expect("persist root");
        let runtime_head = root.successor_head().clone();
        root.acknowledge_session(&mut session, root.successor_head_token())
            .expect("acknowledge root");

        session.push(user("parent append"));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 2, end: 3 },
                vec![user("replacement")],
                TranscriptRewriteReason::new("named retry effects"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("commit rewrite");
        session.push(user("successor tail"));
        let rewrite = PreparedHeadCanonicalRewriteMutation::prepare_intra_turn(
            &session,
            &runtime_head,
            runtime_head.clone(),
        )
        .expect("prepare rewrite with parent and successor suffixes");
        assert_eq!(rewrite.steps().len(), 1);
        let step = &rewrite.steps()[0];
        assert_eq!(step.serialized_parent_suffix().len(), 1);
        assert_eq!(step.serialized_replacement().len(), 1);
        assert_eq!(rewrite.serialized_tail().len(), 1);
        incremental
            .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
            .await
            .expect("persist prepared rewrite");

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_strand_messages SET message_json = X'7B7D'
             WHERE session_id = ?1 AND strand = ?2 AND seq = ?3",
            params![
                session.id().to_string(),
                step.parent_strand().as_str(),
                i64::try_from(step.parent_base_seq()).unwrap()
            ],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            incremental
                .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                .await,
            Err(SessionStoreError::Corrupted(id)) if id == *session.id()
        ));
        assert!(matches!(
            store.load(session.id()).await,
            Err(SessionStoreError::Corrupted(id)) if id == *session.id()
        ));
        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_strand_messages SET message_json = ?4
             WHERE session_id = ?1 AND strand = ?2 AND seq = ?3",
            params![
                session.id().to_string(),
                step.parent_strand().as_str(),
                i64::try_from(step.parent_base_seq()).unwrap(),
                &step.serialized_parent_suffix()[0]
            ],
        )
        .unwrap();
        drop(conn);

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_strand_messages SET message_json = X'7B7D'
             WHERE session_id = ?1 AND strand = ?2 AND seq = ?3",
            params![
                session.id().to_string(),
                step.strand().as_str(),
                i64::try_from(step.link_splice().splice_start).unwrap()
            ],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            incremental
                .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                .await,
            Err(SessionStoreError::Corrupted(id)) if id == *session.id()
        ));
        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_strand_messages SET message_json = ?4
             WHERE session_id = ?1 AND strand = ?2 AND seq = ?3",
            params![
                session.id().to_string(),
                step.strand().as_str(),
                i64::try_from(step.link_splice().splice_start).unwrap(),
                &step.serialized_replacement()[0]
            ],
        )
        .unwrap();
        drop(conn);

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_strand_messages SET message_json = X'7B7D'
             WHERE session_id = ?1 AND strand = ?2 AND seq = ?3",
            params![
                session.id().to_string(),
                rewrite.successor_head().strand.as_str(),
                i64::try_from(rewrite.tail_base_seq()).unwrap()
            ],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            incremental
                .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                .await,
            Err(SessionStoreError::Corrupted(id)) if id == *session.id()
        ));
        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_strand_messages SET message_json = ?4
             WHERE session_id = ?1 AND strand = ?2 AND seq = ?3",
            params![
                session.id().to_string(),
                rewrite.successor_head().strand.as_str(),
                i64::try_from(rewrite.tail_base_seq()).unwrap(),
                &rewrite.serialized_tail()[0]
            ],
        )
        .unwrap();
        drop(conn);

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_strand_links SET successor = 'corrupt'
             WHERE session_id = ?1 AND strand = ?2",
            params![session.id().to_string(), step.strand().as_str()],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            incremental
                .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                .await,
            Err(SessionStoreError::Corrupted(id)) if id == *session.id()
        ));
        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_strand_links SET successor = ?3
             WHERE session_id = ?1 AND strand = ?2",
            params![
                session.id().to_string(),
                step.strand().as_str(),
                step.parent_strand().as_str()
            ],
        )
        .unwrap();
        drop(conn);

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_rewrites SET commit_json = X'7B7D'
             WHERE session_id = ?1 AND rewrite_idx = ?2",
            params![
                session.id().to_string(),
                i64::try_from(runtime_head.rewrite_count).unwrap()
            ],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            incremental
                .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                .await,
            Err(SessionStoreError::Corrupted(id)) if id == *session.id()
        ));
        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_rewrites SET commit_json = ?3
             WHERE session_id = ?1 AND rewrite_idx = ?2",
            params![
                session.id().to_string(),
                i64::try_from(runtime_head.rewrite_count).unwrap(),
                serde_json::to_vec(step.commit()).unwrap()
            ],
        )
        .unwrap();
        drop(conn);

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_rewrites SET graph_edge_json = X'7B7D'
             WHERE session_id = ?1 AND rewrite_idx = ?2",
            params![
                session.id().to_string(),
                i64::try_from(runtime_head.rewrite_count).unwrap()
            ],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            incremental
                .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                .await,
            Err(SessionStoreError::Corrupted(id)) if id == *session.id()
        ));
        assert!(matches!(
            store.load(session.id()).await,
            Err(SessionStoreError::Corrupted(id)) if id == *session.id()
        ));
        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_rewrites SET graph_edge_json = ?3
             WHERE session_id = ?1 AND rewrite_idx = ?2",
            params![
                session.id().to_string(),
                i64::try_from(runtime_head.rewrite_count).unwrap(),
                step.serialized_graph_edge()
            ],
        )
        .unwrap();
        drop(conn);

        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_heads SET head_json = X'7B7D' WHERE session_id = ?1",
            params![session.id().to_string()],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            incremental
                .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                .await,
            Err(SessionStoreError::Corrupted(id)) if id == *session.id()
        ));
        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "UPDATE session_heads SET head_json = ?2 WHERE session_id = ?1",
            params![
                session.id().to_string(),
                serde_json::to_vec(rewrite.successor_head()).unwrap()
            ],
        )
        .unwrap();
        drop(conn);

        assert_eq!(
            incremental
                .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                .await
                .expect("all restored named effects retry exactly"),
            rewrite.successor_head_token()
        );
    }

    #[tokio::test]
    async fn canonical_head_is_row_only_and_never_synthesizes_for_blob_only() {
        let (_dir, store) = temp_store();

        // WholeBlob session: plain save writes the document row; no canonical head
        // row exists.
        let mut blob_session = Session::new();
        blob_session.push(user("blob one"));
        store.save(&blob_session).await.unwrap();
        assert!(blob_row_bytes(store.path(), blob_session.id()).is_some());

        let inc = incremental(&store);
        // `load_head` still synthesizes (compat contract, unchanged)...
        assert!(inc.load_head(blob_session.id()).await.unwrap().is_some());
        // ...but the canonical probe must answer None without synthesizing.
        assert!(
            inc.load_canonical_head(blob_session.id())
                .await
                .unwrap()
                .is_none(),
            "a blob-only session has no canonical head row"
        );
        {
            let conn = open_connection(store.path()).unwrap();
            let heads: i64 = conn
                .query_row("SELECT COUNT(*) FROM session_heads", [], |row| row.get(0))
                .unwrap();
            assert_eq!(heads, 0, "the canonical probe must not write");
        }

        // Absent session: None, not an error.
        assert!(
            inc.load_canonical_head(&SessionId::new())
                .await
                .unwrap()
                .is_none()
        );

        // Head-canonical session: the canonical probe serves exactly the
        // persisted head row (== load_head for a head-canonical session).
        let mut head_session = Session::new();
        head_session.push(user("head one"));
        head_session.push(user("head two"));
        let saved_head = seed_incremental(&inc, &head_session).await;
        let canonical = inc
            .load_canonical_head(head_session.id())
            .await
            .unwrap()
            .expect("head-canonical session must advertise its head row");
        assert_eq!(canonical, saved_head);
        assert_eq!(
            Some(canonical),
            inc.load_head(head_session.id()).await.unwrap(),
            "for a head-canonical session the canonical probe equals load_head"
        );
    }

    #[tokio::test]
    async fn rewrite_commits_serve_adopted_commit_rows_without_bodies() {
        let (_dir, store) = temp_store();
        let inc = incremental(&store);
        let mut session = Session::new();
        session.push(user("one"));
        session.push(user("two"));
        let head = seed_incremental(&inc, &session).await;
        assert!(
            inc.load_rewrite_commits(session.id())
                .await
                .unwrap()
                .is_empty()
        );

        let commit = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                vec![user("[compacted] summary")],
                TranscriptRewriteReason::new("compaction"),
                Some("test".to_string()),
                None,
            )
            .unwrap();
        let parent_body = session
            .transcript_revision_body(&commit.parent_revision)
            .unwrap()
            .unwrap();
        let revision_body = session
            .transcript_revision_body(&commit.revision)
            .unwrap()
            .unwrap();
        let record = TranscriptRewriteRecord::new(commit.clone(), parent_body, revision_body)
            .expect("valid rewrite record");
        let token = session_head_cas_token(&head).unwrap();
        let next = inc
            .commit_rewrite(
                session.id(),
                &record,
                SessionHeadCas::IfToken(token.clone()),
            )
            .await
            .unwrap();

        // Recorded but not adopted: the commit view stays empty, exactly
        // like load_rewrites.
        assert!(
            inc.load_rewrite_commits(session.id())
                .await
                .unwrap()
                .is_empty(),
            "recorded-but-unadopted commits must not be served"
        );

        inc.save_head(&next, SessionHeadCas::IfToken(token))
            .await
            .unwrap();
        let commits = inc.load_rewrite_commits(session.id()).await.unwrap();
        assert_eq!(commits, vec![commit]);
        assert_eq!(
            commits,
            inc.load_rewrites(session.id())
                .await
                .unwrap()
                .into_iter()
                .map(|record| record.commit)
                .collect::<Vec<_>>(),
            "the commit view must equal load_rewrites' commits"
        );
    }

    #[tokio::test]
    async fn incremental_append_and_load_round_trip() {
        let (_dir, store) = temp_store();
        let inc = incremental(&store);
        let mut session = Session::new();
        session.push(user("one"));
        session.push(user("two"));
        seed_incremental(&inc, &session).await;

        let root = TranscriptStrandId::root();
        let loaded = inc.load_messages(session.id(), &root, 0..2).await.unwrap();
        assert_eq!(loaded.len(), 2);

        // Identical re-append is idempotent Ok.
        inc.append_messages(session.id(), &root, 0, session.messages())
            .await
            .expect("identical re-append must be idempotent");
        assert_eq!(strand_row_count(store.path(), session.id(), &root), 2);

        // base_seq gap fails closed.
        let err = inc
            .append_messages(session.id(), &root, 5, &[user("gap")])
            .await
            .expect_err("gap append must be rejected");
        assert!(
            matches!(err, SessionStoreError::TranscriptContinuityViolation { .. }),
            "unexpected error: {err}"
        );

        // Divergent bytes at an existing (strand, seq) fail closed.
        let err = inc
            .append_messages(session.id(), &root, 0, &[user("DIVERGENT")])
            .await
            .expect_err("divergent overwrite must be rejected");
        assert!(
            matches!(err, SessionStoreError::TranscriptContinuityViolation { .. }),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn incremental_save_head_guards() {
        let (_dir, store) = temp_store();
        let inc = incremental(&store);
        let mut session = Session::new();
        session.push(user("one"));
        session.push(user("two"));
        let head = seed_incremental(&inc, &session).await;

        // Create on an existing row conflicts.
        let err = inc
            .save_head(&head, SessionHeadCas::Create)
            .await
            .expect_err("Create over an existing head must conflict");
        assert!(matches!(
            err,
            SessionStoreError::TranscriptRevisionConflict { .. }
        ));

        // Stale IfToken conflicts.
        let err = inc
            .save_head(
                &head,
                SessionHeadCas::IfToken("head-sha256:stale".to_string()),
            )
            .await
            .expect_err("stale token must conflict");
        assert!(matches!(
            err,
            SessionStoreError::TranscriptRevisionConflict { .. }
        ));

        let token = session_head_cas_token(&head).unwrap();

        // Same-strand shrink is a MonotonicityViolation.
        let mut shrunk_session = Session::with_id(session.id().clone());
        shrunk_session.push(user("one"));
        let shrunk =
            SessionHead::from_session(&shrunk_session, TranscriptStrandId::root(), 0).unwrap();
        let err = inc
            .save_head(&shrunk, SessionHeadCas::IfToken(token.clone()))
            .await
            .expect_err("same-strand shrink must be rejected");
        assert!(matches!(
            err,
            SessionStoreError::MonotonicityViolation { .. }
        ));

        // Head pointing past persisted rows is rejected.
        let mut extended_session = session.clone();
        extended_session.push(user("three"));
        let past =
            SessionHead::from_session(&extended_session, TranscriptStrandId::root(), 0).unwrap();
        let err = inc
            .save_head(&past, SessionHeadCas::IfToken(token.clone()))
            .await
            .expect_err("head past persisted rows must be rejected");
        assert!(matches!(
            err,
            SessionStoreError::InvalidTranscriptRewrite { .. }
        ));

        // Strand-switch to a fully covered strand is Ok.
        let rebased = TranscriptStrandId::rebase("switch-target");
        inc.append_messages(session.id(), &rebased, 0, session.messages())
            .await
            .unwrap();
        let switched = SessionHead::from_session(&session, rebased, 0).unwrap();
        inc.save_head(&switched, SessionHeadCas::IfToken(token))
            .await
            .expect("strand switch to a covered strand must be accepted");
    }

    fn compacted_fixture() -> (Session, Session, meerkat_core::TranscriptRewriteCommit) {
        let mut parent = Session::new();
        parent.push(user("turn one"));
        parent.push(user("turn two"));
        parent.push(user("turn three"));
        parent.push(user("turn four"));
        let mut compacted = parent.clone();
        let commit = compacted
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 4 },
                vec![
                    user("[Context compacted] summary"),
                    user("turn four retained"),
                ],
                TranscriptRewriteReason::new("compaction"),
                Some("test".to_string()),
                None,
            )
            .unwrap();
        (parent, compacted, commit)
    }

    fn record_for(
        session: &Session,
        commit: &meerkat_core::TranscriptRewriteCommit,
    ) -> TranscriptRewriteRecord {
        let parent = session
            .transcript_revision_body(&commit.parent_revision)
            .unwrap()
            .expect("parent body");
        let child = session
            .transcript_revision_body(&commit.revision)
            .unwrap()
            .expect("child body");
        TranscriptRewriteRecord::new(commit.clone(), parent, child).unwrap()
    }

    fn imported_released_exact_splice_session() -> Session {
        fn revision(messages: &[Message]) -> String {
            transcript_messages_digest(messages).expect("digest released revision")
        }

        fn released_commit(
            parent: &[Message],
            child: &[Message],
            start: usize,
            end: usize,
            reason: &str,
        ) -> serde_json::Value {
            let commit = TranscriptRewriteCommit {
                rewrite_generation: 0,
                parent_revision: revision(parent),
                revision: revision(child),
                selection: TranscriptRewriteSelection::MessageRange { start, end },
                original_span_digest: transcript_messages_digest(&parent[start..end])
                    .expect("digest released original span"),
                replacement_digest: transcript_messages_digest(
                    &child[start..start + child.len() - (parent.len() - (end - start))],
                )
                .expect("digest released replacement"),
                messages_before: parent.len(),
                messages_after: child.len(),
                reason: TranscriptRewriteReason::new(reason),
                actor: Some("released-fixture".to_string()),
                committed_at: SystemTime::UNIX_EPOCH,
            };
            let mut value = serde_json::to_value(commit).expect("serialize released commit");
            value
                .as_object_mut()
                .expect("released commit object")
                .remove("rewrite_generation");
            value
        }

        let anchor = vec![
            user("retained prefix"),
            user("base row"),
            user("retained suffix"),
        ];
        let first = vec![
            user("retained prefix"),
            user("first rewrite"),
            user("retained suffix"),
        ];
        let second_parent = vec![
            user("retained prefix"),
            user("imported arbitrary-role splice"),
            user("retained suffix"),
            user("appended parent row"),
        ];
        let second = vec![
            user("retained prefix"),
            user("imported arbitrary-role splice"),
            user("second rewrite"),
            user("appended parent row"),
        ];
        let created_at = serde_json::to_value(SystemTime::UNIX_EPOCH).expect("serialize time");
        let revisions = [&anchor, &first, &second_parent, &second]
            .into_iter()
            .map(|messages| {
                serde_json::json!({
                    "revision": revision(messages),
                    "parent_revision": serde_json::Value::Null,
                    "messages": messages,
                    "created_at": created_at.clone(),
                })
            })
            .collect::<Vec<_>>();
        let history = serde_json::json!({
            "head": revision(&second),
            "commits": [
                released_commit(&anchor, &first, 1, 2, "released edit one"),
                released_commit(&second_parent, &second, 2, 3, "released edit two"),
            ],
            "revisions": revisions,
            "digest_format": 1,
        });
        let mut envelope = serde_json::to_value(Session::new()).expect("serialize envelope");
        envelope["version"] = serde_json::json!(2);
        envelope["messages"] = serde_json::to_value(&second).expect("serialize live messages");
        envelope["metadata"] = serde_json::json!({
            (meerkat_core::SESSION_TRANSCRIPT_HISTORY_STATE_KEY): history,
        });
        let bytes = serde_json::to_vec(&envelope).expect("serialize released fixture");
        meerkat_core::import_released_0810_session(&bytes)
            .expect("frozen released importer accepts exact splice fixture")
            .into_parts()
            .0
    }

    #[tokio::test]
    async fn released_v0_8_10_rewrite_rows_backfill_exact_edges_without_relabeling_rows() {
        let (_dir, store) = temp_store();
        let session = imported_released_exact_splice_session();

        let history = session
            .validated_transcript_history_state()
            .unwrap()
            .expect("fixture carries validated compact history");
        let layout =
            strand_layout_for_history(&session, Some(&history)).expect("current compact layout");
        assert_eq!(layout.rewrites.len(), 2);
        assert!(matches!(
            &layout.rewrites[0].parent_transition,
            PreparedHeadCanonicalParentTransition::ExactAppend
        ));
        assert!(matches!(
            &layout.rewrites[1].parent_transition,
            PreparedHeadCanonicalParentTransition::ExactSplice(_)
        ));

        let mut released_source = TranscriptStrandId::root();
        let released_rows = layout
            .rewrites
            .iter()
            .map(|rewrite| {
                let parent_strand = match &rewrite.parent_transition {
                    PreparedHeadCanonicalParentTransition::ExactAppend => released_source.clone(),
                    PreparedHeadCanonicalParentTransition::ExactSplice(_) => {
                        TranscriptStrandId::rebase(&rewrite.commit.parent_revision)
                    }
                };
                let strand = TranscriptStrandId::from_rewrite(&rewrite.commit);
                released_source = strand.clone();
                let mut released_commit = rewrite.commit.clone();
                released_commit.rewrite_generation = 0;
                (
                    released_commit,
                    parent_strand,
                    u64::try_from(rewrite.commit.messages_before).unwrap(),
                    strand,
                    u64::try_from(rewrite.commit.messages_after).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let expected_count = u64::try_from(released_rows.len()).unwrap();
        let id = session.id().clone();
        store
            .in_write_txn(move |tx| {
                for (index, (commit, parent, parent_len, strand, strand_len)) in
                    released_rows.iter().enumerate()
                {
                    tx.execute(
                        "INSERT INTO session_rewrites
                             (session_id, rewrite_idx, parent_strand, parent_len, strand,
                              strand_len, commit_json, graph_edge_json, created_at_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8)",
                        params![
                            id.to_string(),
                            i64::try_from(index)
                                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                            parent.as_str(),
                            i64::try_from(*parent_len)
                                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                            strand.as_str(),
                            i64::try_from(*strand_len)
                                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                            serde_json::to_vec(commit).map_err(SessionStoreError::from)?,
                            now_millis(),
                        ],
                    )
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                }

                let expected_prefix = session
                    .transcript_rewrite_prefix_authority()
                    .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
                if expected_prefix.occurrence_count() != expected_count {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
                backfill_rewrite_graph_edges_from_validated_session_in_txn(
                    tx,
                    &session,
                    &expected_prefix,
                )?;
                let stored = indexed_rewrite_rows_range_in_txn(tx, &id, 0, expected_count)?;
                assert_eq!(stored.len(), layout.rewrites.len());
                for ((_, row), (released, expected)) in stored
                    .iter()
                    .zip(released_rows.iter().zip(&layout.rewrites))
                {
                    assert_eq!(row.commit, expected.commit);
                    assert_eq!(row.parent_strand, released.1);
                    assert_eq!(row.strand, released.3);
                    assert_eq!(
                        row.graph_edge_json.as_deref(),
                        Some(expected.serialized_graph_edge.as_slice())
                    );
                    assert_ne!(
                        row.strand, expected.strand,
                        "released physical ids must not be confused with current occurrence ids"
                    );
                    if matches!(
                        &expected.parent_transition,
                        PreparedHeadCanonicalParentTransition::ExactSplice(_)
                    ) {
                        assert_ne!(
                            row.parent_strand, expected.parent_strand,
                            "released non-append parents use rebase ids, not current occurrence bridges"
                        );
                    }
                }
                Ok(())
            })
            .await
            .expect("released rows backfill without physical relabeling");
    }

    #[tokio::test]
    async fn legacy_head_canonical_rewrite_paths_fail_closed_without_replay_edges() {
        let (_dir, store) = temp_store();
        let inc = incremental(&store);
        let (parent, compacted, commit) = compacted_fixture();
        let head = seed_incremental(&inc, &parent).await;
        let token = session_head_cas_token(&head).unwrap();
        let record = record_for(&compacted, &commit);

        let error = inc
            .commit_rewrite(parent.id(), &record, SessionHeadCas::IfToken(token.clone()))
            .await
            .expect_err("record-only incremental rewrites cannot prove a replay edge");
        assert!(matches!(
            error,
            SessionStoreError::InvalidTranscriptRewrite { .. }
        ));
        let error = store
            .save_transcript_rewrite(&compacted, &commit)
            .await
            .expect_err("record-only compatibility save cannot prove a replay edge");
        assert!(matches!(
            error,
            SessionStoreError::InvalidTranscriptRewrite { .. }
        ));
        assert!(
            inc.load_rewrite_commits(parent.id())
                .await
                .unwrap()
                .is_empty()
        );

        // A released/preexisting record-only pending row cannot be smuggled
        // into current authority by calling save_head directly.
        let child = TranscriptStrandId::from_rewrite(&commit);
        let conn = open_connection(store.path()).unwrap();
        conn.execute(
            "INSERT INTO session_rewrites
                 (session_id, rewrite_idx, parent_strand, parent_len, strand,
                  strand_len, commit_json, graph_edge_json, created_at_ms)
             VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
            params![
                parent.id().to_string(),
                TranscriptStrandId::root().as_str(),
                i64::try_from(commit.messages_before).unwrap(),
                child.as_str(),
                i64::try_from(commit.messages_after).unwrap(),
                serde_json::to_vec(&commit).unwrap(),
                now_millis(),
            ],
        )
        .unwrap();
        drop(conn);
        let unproved_head = SessionHead::from_session(&compacted, child, 1).unwrap();
        let error = inc
            .save_head(&unproved_head, SessionHeadCas::IfToken(token))
            .await
            .expect_err("save_head must not adopt a rewrite without an exact replay edge");
        assert!(matches!(
            error,
            SessionStoreError::InvalidTranscriptRewrite { .. }
        ));
        assert!(
            inc.load_rewrite_commits(parent.id())
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(total_strand_rows(store.path(), parent.id()), 4);
    }

    #[tokio::test]
    async fn head_canonical_activation_from_whole_blob_snapshot() {
        let (_dir, store) = temp_store();

        // Supported whole-blob snapshot with two audited rewrites and a live
        // append tail. Activation must preserve both authorities: the graph
        // stays at the latest audited occurrence while the physical head
        // names the complete live transcript.
        let mut session = Session::new();
        session.push(user("turn one"));
        session.push(user("turn two"));
        store.save(&session).await.unwrap();
        let second_commit = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                vec![user("[compacted] summary one")],
                TranscriptRewriteReason::new("compaction"),
                Some("test".to_string()),
                None,
            )
            .unwrap();
        let first_commit_revision = session.transcript_revision().unwrap();
        session.push(user("turn three"));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                vec![user("[compacted] summary two")],
                TranscriptRewriteReason::new("compaction"),
                Some("test".to_string()),
                None,
            )
            .unwrap();
        let audited_graph = session
            .metadata()
            .get(meerkat_core::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
            .cloned()
            .expect("second rewrite installs audited graph");
        session.push(user("turn four"));
        for turn in 0..16 {
            session.push(user(&format!("ordinary turn {turn}")));
        }
        assert_eq!(
            session
                .metadata()
                .get(meerkat_core::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY),
            Some(&audited_graph),
            "ordinary whole-blob appends must leave audited graph bytes untouched"
        );
        let state = session.transcript_history_state().unwrap().unwrap();
        assert_eq!(state.head, second_commit.revision);
        assert_eq!(
            state.commits.last(),
            Some(&second_commit),
            "audited graph must end at the exact latest rewrite occurrence"
        );
        assert_ne!(
            state.head,
            session.transcript_content_digest().unwrap(),
            "live tail authority must remain separate from the audited graph head"
        );

        // Seed the supported WholeBlob side of the activation boundary.
        store.save_authoritative_projection(&session).await.unwrap();
        assert!(blob_row_bytes(store.path(), session.id()).is_some());

        let inc = incremental(&store);
        let proved_history = session
            .validated_transcript_history_state()
            .unwrap()
            .expect("fixture carries validated compact history");
        let compact_layout =
            strand_layout_for_history(&session, Some(&proved_history)).expect("compact layout");
        let expected_physical_rows = compact_layout_physical_row_count(&compact_layout);

        // Blob-only exceptional reads may explicitly materialize occurrence
        // bodies, but they must not force migration or revive a k-fold strand
        // vector in the layout carrier.
        let blob_records = inc.load_rewrites(session.id()).await.unwrap();
        assert_eq!(blob_records.len(), 2);
        let first_parent = &blob_records[0].parent_body.messages;
        let blob_root = inc
            .load_messages(
                session.id(),
                &TranscriptStrandId::root(),
                0..u64::try_from(first_parent.len()).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(&blob_root, first_parent);

        // load_head synthesizes deterministically without writing.
        let synthesized = inc.load_head(session.id()).await.unwrap().unwrap();
        assert_eq!(synthesized.rewrite_count, 2);
        assert_eq!(synthesized.message_count, session.messages().len() as u64);
        let synthesized_token = session_head_cas_token(&synthesized).unwrap();
        {
            let conn = open_connection(store.path()).unwrap();
            let heads: i64 = conn
                .query_row("SELECT COUNT(*) FROM session_heads", [], |row| row.get(0))
                .unwrap();
            assert_eq!(heads, 0, "load_head must not write");
        }

        // First incremental write migrates in-txn against the synthesized token.
        let migrated_head = SessionHead::from_session(
            &session,
            synthesized.strand.clone(),
            synthesized.rewrite_count,
        )
        .unwrap();
        inc.save_head(&migrated_head, SessionHeadCas::IfToken(synthesized_token))
            .await
            .expect("synthesized token must match the migrated head token");
        assert_eq!(
            strand_row_count(store.path(), session.id(), &migrated_head.strand),
            i64::try_from(session.messages().len()).unwrap(),
            "one-time activation must settle the current anchor into direct rows"
        );
        assert!(
            total_strand_rows(store.path(), session.id())
                <= i64::try_from(expected_physical_rows + session.messages().len()).unwrap(),
            "activation may add one settled live document but must never restore one full body per rewrite"
        );
        {
            let conn = open_connection(store.path()).unwrap();
            let missing_edges: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM session_rewrites
                     WHERE session_id = ?1 AND graph_edge_json IS NULL",
                    params![session.id().to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                missing_edges, 0,
                "every adopted rewrite must carry its exact cold-replay edge"
            );
        }

        // Slim load returns a byte-identical live transcript, no history metadata.
        let slim = store.load(session.id()).await.unwrap().unwrap();
        assert_eq!(
            transcript_messages_digest(slim.messages()).unwrap(),
            transcript_messages_digest(session.messages()).unwrap()
        );
        assert!(slim.transcript_history_state().unwrap().is_none());

        // list() yields exactly one entry for the migrated session.
        let listed = store.list(SessionFilter::default()).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, *session.id());

        // Adopted rewrites reconstruct from strand ranges.
        let records = inc.load_rewrites(session.id()).await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].commit.revision, first_commit_revision);
        let reopened = SqliteSessionStore::open(store.path()).expect("reopen migrated store");
        let cold = reopened
            .load(session.id())
            .await
            .expect("cold replay migrated head")
            .expect("session present after reopen");
        assert_eq!(cold.messages(), session.messages());

        // Corrupt the archived blob: loads must be unaffected (pins "blob
        // never read post-migration").
        {
            let conn = open_connection(store.path()).unwrap();
            conn.execute(
                "UPDATE sessions SET session_json = X'DEADBEEF' WHERE session_id = ?1",
                params![session.id().to_string()],
            )
            .unwrap();
        }
        let slim_after_corruption = store.load(session.id()).await.unwrap().unwrap();
        assert_eq!(
            slim_after_corruption.messages().len(),
            session.messages().len()
        );

        // delete removes rows from every session-domain table.
        store.delete(session.id()).await.unwrap();
        let conn = open_connection(store.path()).unwrap();
        for table in [
            "sessions",
            "session_strand_messages",
            "session_strand_links",
            "session_rewrites",
            "session_component_events",
            "session_head_metadata_refs",
            "session_head_metadata_head_lineage",
            "session_head_metadata_state_deltas",
            "session_head_metadata_states",
            "session_head_metadata_current",
            "session_head_metadata_cells",
            "session_heads",
        ] {
            let count: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE session_id = ?1"),
                    params![session.id().to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0, "table {table} must be cleared by delete");
        }
    }

    #[tokio::test]
    async fn head_canonical_save_paths() {
        let (_dir, store) = temp_store();
        let inc = incremental(&store);
        let mut session = Session::new();
        session.push(user("one"));
        session.push(user("two"));
        seed_incremental(&inc, &session).await;
        let root = TranscriptStrandId::root();

        // Plain save append writes ONLY delta rows.
        let mut appended = store.load(session.id()).await.unwrap().unwrap();
        appended.push(user("three"));
        store.save(&appended).await.unwrap();
        assert_eq!(strand_row_count(store.path(), session.id(), &root), 3);

        // Plain save shrink is rejected.
        let mut shrunk = Session::with_id(session.id().clone());
        shrunk.push(user("one"));
        let err = store
            .save(&shrunk)
            .await
            .expect_err("head-canonical shrink must be rejected");
        assert!(matches!(
            err,
            SessionStoreError::MonotonicityViolation { .. }
        ));

        // Record-only rewrite saves are deliberately unsupported after
        // activation: only the prepared carrier proves exact replay edges.
        let mut compacted = store.load(session.id()).await.unwrap().unwrap();
        let parent_revision = compacted.transcript_revision().unwrap();
        let commit = compacted
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 3 },
                vec![user("[compacted] summary")],
                TranscriptRewriteReason::new("compaction"),
                Some("test".to_string()),
                Some(parent_revision),
            )
            .unwrap();
        let error = store
            .save_transcript_rewrite(&compacted, &commit)
            .await
            .expect_err("HeadCanonical compatibility rewrite must fail closed");
        assert!(matches!(
            error,
            SessionStoreError::InvalidTranscriptRewrite { .. }
        ));
        let head = inc.load_head(session.id()).await.unwrap().unwrap();
        assert_eq!(head.rewrite_count, 0);
        assert_eq!(head.message_count, 3);
        let slim = store.load(session.id()).await.unwrap().unwrap();
        assert_eq!(slim.messages().len(), 3);

        // save_authoritative_projection_if_current_revision with the
        // materialized token succeeds; a stale token is rejected.
        let current = store.load(session.id()).await.unwrap().unwrap();
        let token = meerkat_core::session_store::session_projection_cas_token(&current).unwrap();
        let mut next = current.clone();
        next.push(user("post-compaction turn"));
        store
            .save_authoritative_projection_if_current_revision(&next, Some(token.clone()))
            .await
            .expect("materialized token must match");
        let err = store
            .save_authoritative_projection_if_current_revision(&next, Some(token))
            .await
            .expect_err("stale token must be rejected");
        assert!(matches!(
            err,
            SessionStoreError::TranscriptContinuityViolation { .. }
        ));
    }

    /// A rebase strand switch (authoritative projection that does not extend
    /// the head strand) must supersede the strand it left instead of leaving
    /// a second full copy behind.
    #[tokio::test]
    async fn rebase_strand_switch_does_not_leave_a_second_transcript_copy() {
        let (_dir, store) = temp_store();
        let inc = incremental(&store);
        let mut seeded = Session::new();
        for turn in 0..5 {
            seeded.push(user(&format!("turn {turn}")));
        }
        seed_incremental(&inc, &seeded).await;
        assert_eq!(total_strand_rows(store.path(), seeded.id()), 5);

        // A projection that replaces message 0 does not extend the head
        // strand, so the write rebases onto a fresh strand.
        let mut projection = Session::with_id(seeded.id().clone());
        projection.push(user("refreshed turn 0"));
        for turn in 1..5 {
            projection.push(user(&format!("turn {turn}")));
        }
        store
            .save_authoritative_projection(&projection)
            .await
            .unwrap();

        let head = inc.load_head(seeded.id()).await.unwrap().unwrap();
        assert_ne!(head.strand, TranscriptStrandId::root());
        assert_eq!(
            total_strand_rows(store.path(), seeded.id()),
            5,
            "a rebase must not leave the abandoned strand's rows behind"
        );
        assert_eq!(
            strand_link_count(store.path(), seeded.id()),
            0,
            "an abandoned strand no rewrite names must be collected outright"
        );
        let slim = store.load(seeded.id()).await.unwrap().unwrap();
        assert_eq!(slim.messages(), projection.messages());
    }
}
