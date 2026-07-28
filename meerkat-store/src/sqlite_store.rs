//! SQLite-backed session store.

use crate::error::into_session_store_error;
use crate::json_column::JsonColumnBytes;
use crate::{SessionFilter, SessionStore, SessionStoreError, StoreError};
use async_trait::async_trait;
use meerkat_core::session_store::{
    IncrementalSessionStore, SaveGuardWitness, SessionHead, SessionHeadCas, StrandLayout,
    StrandSegment, StrandSplice, TranscriptStrandId, head_canonical_plain_save_guard_with_witness,
    reconstruct_rewrite_record, session_head_cas_token, strand_layout_for_history,
    validate_commit_rewrite_transition, validate_save_head_transition,
};
use meerkat_core::time_compat::SystemTime;
use meerkat_core::transcript_messages_digest;
use meerkat_core::types::Message;
use meerkat_core::{
    Session, SessionId, SessionMeta, TranscriptRewriteCommit, TranscriptRewriteRecord,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
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
// Row-materialization rule (per session): AT MOST ONE strand is fully
// materialized — the live head. Every strand the head has left behind is
// superseded: it keeps only the rows its successor cannot reproduce (its
// `session_strand_links` splice span) and resolves the rest through that
// successor. Without this rule each rewrite persists a whole fresh copy of
// the transcript (measured in the field: 98 rewrites of a 371-message
// transcript => 16,672 rows / 24 MB for one session).
const CREATE_SESSION_STRAND_MESSAGES_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS session_strand_messages (
    session_id TEXT NOT NULL,
    strand TEXT NOT NULL,
    seq INTEGER NOT NULL,
    message_json BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, strand, seq)
)";

// Supersession edges: `strand` was replaced by `successor`, and the rows it
// still owns are exactly `[splice_start, splice_end)`. A strand with NO row
// here is materialized (all of its logical rows are present physically).
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
/// preflighted at open so a future file is refused before the profile's WAL
/// conversion touches it.
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
            // Future-schema refusal must fire before the Primary profile's
            // journal-mode conversion mutates the file.
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
    ],
};

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

/// Write the head row and restore the row-materialization rule.
///
/// Every head write is a potential strand transition, so this is the one
/// place supersession has to run: the strand the head just left becomes a
/// splice delta of the strand it moved to, and orphaned strands are
/// collected. Reads never depend on it having run (a materialized strand is
/// always legal) — it is what keeps durable growth bounded.
fn write_head_row_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
) -> Result<String, SessionStoreError> {
    let previous_strand = head_row_in_txn(tx, &head.id)?.map(|(stored, _)| stored.strand);
    let cas_token = write_head_row_only_in_txn(tx, head)?;
    settle_strand_topology_in_txn(tx, head, previous_strand.as_ref())?;
    Ok(cas_token)
}

fn write_head_row_only_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
) -> Result<String, SessionStoreError> {
    let head_json = serde_json::to_vec(head).map_err(SessionStoreError::from)?;
    let cas_token = session_head_cas_token(head)?;
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

// ---------------------------------------------------------------------------
// Strand supersession: at most one materialized strand per session.
// ---------------------------------------------------------------------------

/// One persisted supersession edge.
#[derive(Debug, Clone)]
struct StrandLinkRow {
    successor: TranscriptStrandId,
    splice: StrandSplice,
}

/// Every supersession edge of one session, keyed by superseded strand.
///
/// Loaded once per operation: the set is at most one row per strand the head
/// has ever left behind, and resolution is then pure map lookups plus the
/// physical row reads for retained spans.
type StrandLinks = std::collections::HashMap<String, StrandLinkRow>;

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
        Some(link) => Ok(link.splice.strand_len),
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
    if hops == 0 || range.start > range.end || range.end > link.splice.strand_len {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let wanted = usize::try_from(range.end - range.start)
        .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    let mut rows = Vec::with_capacity(wanted);
    for segment in link.splice.segments(range) {
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
    Ok(rows)
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
    let links = strand_links_in_txn(tx, id)?;
    if links.contains_key(strand.as_str()) {
        return Ok(());
    }
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

/// Delete rows and edges of strands no read verb can reach: neither the head
/// strand, nor an endpoint of a recorded rewrite, nor a hop on the
/// supersession chain of one of those.
fn prune_unreachable_strands_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    head: &SessionHead,
    recorded: &[RewriteRow],
) -> Result<(), SessionStoreError> {
    let links = strand_links_in_txn(tx, id)?;
    let mut frontier = vec![head.strand.as_str().to_string()];
    for row in recorded {
        frontier.push(row.parent_strand.as_str().to_string());
        frontier.push(row.strand.as_str().to_string());
    }
    let mut reachable = std::collections::BTreeSet::new();
    while let Some(strand) = frontier.pop() {
        let Some(link) = links.get(&strand) else {
            reachable.insert(strand);
            continue;
        };
        let successor = link.successor.as_str().to_string();
        if reachable.insert(strand) {
            frontier.push(successor);
        }
    }

    let mut stmt = tx
        .prepare("SELECT DISTINCT strand FROM session_strand_messages WHERE session_id = ?1")
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    let materialized = stmt
        .query_map(params![id.to_string()], |row| row.get::<_, String>(0))
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
    drop(stmt);

    let orphans = materialized
        .into_iter()
        .chain(links.keys().cloned())
        .filter(|strand| !reachable.contains(strand))
        .collect::<std::collections::BTreeSet<_>>();
    for strand in orphans {
        for sql in [
            "DELETE FROM session_strand_messages WHERE session_id = ?1 AND strand = ?2",
            "DELETE FROM session_strand_links WHERE session_id = ?1 AND strand = ?2",
        ] {
            tx.execute(sql, params![id.to_string(), strand])
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
        }
    }
    Ok(())
}

/// Restore the row-materialization rule after a head write.
///
/// Every strand the head has left behind is superseded by the strand that
/// replaced it (a rewrite's child for a rewrite parent, the new head strand
/// for the strand the head just left), and everything unreachable is
/// collected. Pending — recorded but unadopted — rewrite children stay
/// materialized: their rewrite row may still be replaced by a retry, so they
/// are not yet history.
fn settle_strand_topology_in_txn(
    tx: &Transaction<'_>,
    head: &SessionHead,
    previous_strand: Option<&TranscriptStrandId>,
) -> Result<(), SessionStoreError> {
    let id = &head.id;
    let mut mutated = previous_strand.is_some_and(|previous| *previous != head.strand);
    if !mutated && head.rewrite_count == 0 {
        // A rewrite-free session that stayed on its strand has left nothing
        // behind; the append hot path must not pay for a topology sweep.
        return Ok(());
    }
    let recorded = all_rewrite_rows_in_txn(tx, id)?;
    let adopted = usize::try_from(head.rewrite_count).unwrap_or(usize::MAX);
    let pending = recorded
        .iter()
        .skip(adopted)
        .map(|row| row.strand.clone())
        .collect::<Vec<_>>();

    let mut edges = recorded
        .iter()
        .take(adopted)
        .map(|row| (row.parent_strand.clone(), row.strand.clone()))
        .collect::<Vec<_>>();
    if let Some(previous) = previous_strand {
        edges.push((previous.clone(), head.strand.clone()));
    }
    let mut links = strand_links_in_txn(tx, id)?;
    for (strand, successor) in edges {
        if strand == head.strand || pending.contains(&strand) || links.contains_key(strand.as_str())
        {
            continue;
        }
        supersede_strand_in_txn(tx, id, &strand, &successor)?;
        links = strand_links_in_txn(tx, id)?;
        mutated = true;
    }
    if !mutated {
        return Ok(());
    }
    prune_unreachable_strands_in_txn(tx, id, head, &recorded)
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

/// A strand's logical messages, following supersession edges. Every read
/// path goes through here: a superseded strand owns only its splice span.
fn strand_messages_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    strand: &TranscriptStrandId,
    range: std::ops::Range<u64>,
) -> Result<Vec<Message>, SessionStoreError> {
    let links = strand_links_in_txn(tx, id)?;
    strand_messages_with_links_in_txn(tx, id, strand, range, &links)
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
    let links = strand_links_in_txn(tx, id)?;
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

/// Every recorded rewrite row, adopted or not. Strand topology has to see
/// pending rows: their child strands are reachable and must not be collected.
fn all_rewrite_rows_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<Vec<RewriteRow>, SessionStoreError> {
    rewrite_rows_up_to_in_txn(tx, id, None)
}

fn rewrite_rows_up_to_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    limit: Option<i64>,
) -> Result<Vec<RewriteRow>, SessionStoreError> {
    let mut stmt = tx
        .prepare(
            "SELECT commit_json, parent_strand, parent_len, strand, strand_len
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
            ))
        })
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
        .map_err(into_session_store_error)?;
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
             (session_id, rewrite_idx, parent_strand, parent_len, strand, strand_len, commit_json, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id.to_string(),
            idx,
            row.parent_strand.as_str(),
            parent_len,
            row.strand.as_str(),
            strand_len,
            commit_json,
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

fn layout_for_blob_session(
    session: &Session,
) -> Result<(StrandLayout, SessionHead), SessionStoreError> {
    let state = session.transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("stored transcript history state is malformed: {err}"),
        }
    })?;
    let layout = strand_layout_for_history(session.id(), state.as_ref(), session.messages())?;
    let head = SessionHead::from_session(
        session,
        layout.head_strand.clone(),
        layout.rewrites.len() as u64,
    )?;
    Ok((layout, head))
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
    for (strand, rows) in &layout.strands {
        insert_strand_rows_in_txn(tx, id, strand, 0, rows)?;
    }
    for (idx, rewrite) in layout.rewrites.iter().enumerate() {
        insert_rewrite_row_in_txn(
            tx,
            id,
            idx as u64,
            &RewriteRow {
                commit: rewrite.commit.clone(),
                parent_strand: rewrite.parent_strand.clone(),
                parent_len: rewrite.parent_len,
                strand: rewrite.strand.clone(),
                strand_len: rewrite.strand_len,
            },
        )?;
    }
    // The layout materializes every historical strand in full; the head
    // write below supersedes each rewrite parent, and this closes the one
    // edge the rewrite rows do not name — a live vector that rebased off the
    // last revision body onto its own strand.
    if let Some(last) = layout.rewrites.last()
        && last.strand != head.strand
    {
        supersede_strand_in_txn(tx, id, &last.strand, &head.strand)?;
    }
    let token = write_head_row_in_txn(tx, &head)?;
    Ok(Some((head, token)))
}

/// Head row if present; otherwise migrate a legacy blob in this transaction
/// (the first incremental WRITE migrates; reads synthesize without writing).
fn ensure_head_canonical_for_write_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<Option<(SessionHead, String)>, SessionStoreError> {
    if let Some(existing) = head_row_in_txn(tx, id)? {
        return Ok(Some(existing));
    }
    migrate_legacy_blob_in_txn(tx, id)
}

fn materialize_slim_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    head: &SessionHead,
) -> Result<Session, SessionStoreError> {
    let messages = strand_messages_in_txn(tx, id, &head.strand, 0..head.message_count)?;
    head.clone().into_session(messages)
}

/// Head-canonical compat write: delta-append when the incoming transcript
/// extends the persisted head strand, otherwise a `rebase:` strand switch.
///
/// The rebase branch writes a whole transcript of rows, but the head write
/// that follows immediately supersedes the strand it left, so the durable
/// steady state holds one materialized transcript plus the rebase's splice
/// span — not one copy per rebase.
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
    let head_is_materialized = !strand_links_in_txn(tx, id)?.contains_key(head.strand.as_str());
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
            if let Some((head, _token)) = head_row_in_txn(tx, session.id())? {
                // Head-canonical: retained history lives out-of-line; the
                // plain save writes ONLY the delta rows + the small head.
                let adopted = rewrite_rows_in_txn(tx, session.id(), head.rewrite_count)?
                    .into_iter()
                    .map(|row| row.commit)
                    .collect::<Vec<_>>();
                let previous = materialize_slim_in_txn(tx, session.id(), &head)?;
                head_canonical_plain_save_guard_with_witness(
                    &session,
                    &previous,
                    &adopted,
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
            if let Some(stored) = head_row_in_txn(tx, session.id())? {
                // Head-canonical: build the record from the incoming
                // session's retained bodies and run the commit_rewrite +
                // adopt-head sequence in one transaction, preserving the
                // legacy error surface (TranscriptRevisionConflict on a
                // stale parent).
                let incoming_revision = session
                    .transcript_content_digest()
                    .map_err(SessionStoreError::from)?;
                if incoming_revision != commit.revision {
                    return Err(SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: format!(
                            "incoming current transcript digest {incoming_revision} does not match commit revision {}",
                            commit.revision
                        ),
                    });
                }
                let record = rewrite_record_from_session_bodies(&session, &commit)?;
                let expected = SessionHeadCas::IfToken(stored.1.clone());
                let next =
                    commit_rewrite_in_txn(tx, session.id(), &record, &expected, &stored)?;
                // Adopt immediately with the incoming session's envelope.
                let adopted_head = SessionHead::from_session(
                    &session,
                    next.strand.clone(),
                    next.rewrite_count,
                )?;
                write_head_row_in_txn(tx, &adopted_head)?;
                return Ok(());
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
            if let Some((head, _token)) = head_row_in_txn(tx, session.id())? {
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
            if let Some((head, _token)) = head_row_in_txn(tx, session.id())? {
                // The caller's token was computed over the slim
                // materialization it loaded; the same deterministic
                // materialization is compared here.
                let previous = materialize_slim_in_txn(tx, session.id(), &head)?;
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
            if let Some((head, _token)) = head_row_in_txn(tx, &id)? {
                // Slim, no history metadata — the O(live) cold-resume contract.
                return Ok(Some(materialize_slim_in_txn(tx, &id, &head)?));
            }
            load_session_snapshot_in_txn(tx, &id).map_err(into_session_store_error)
        })
        .await
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        let path = self.path.clone();
        let options = self.options;
        tokio::task::spawn_blocking(move || -> Result<Vec<SessionMeta>, SessionStoreError> {
            let _guard = meerkat_sqlite::OperationGuard::for_database(&path)
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            let conn = open_session_connection(&path, options).map_err(into_session_store_error)?;
            let created_after = filter.created_after.map(system_time_millis);
            let updated_after = filter.updated_after.map(system_time_millis);

            let mut metas: Vec<SessionMeta> = Vec::new();
            {
                let mut stmt = conn
                    .prepare(
                        r"
                        SELECT session_id, created_at_ms, updated_at_ms, message_count,
                               total_tokens, metadata_json
                        FROM session_heads
                        WHERE (?1 IS NULL OR created_at_ms >= ?1)
                          AND (?2 IS NULL OR updated_at_ms >= ?2)
                        ",
                    )
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                let rows = stmt
                    .query_map(params![created_after, updated_after], session_meta_from_row)
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                for row in rows {
                    metas.push(
                        row.map_err(StoreError::from)
                            .map_err(into_session_store_error)?,
                    );
                }
            }
            {
                // Legacy rows without a head row keep their blob-derived meta.
                let mut stmt = conn
                    .prepare(
                        r"
                        SELECT session_id, created_at_ms, updated_at_ms, message_count,
                               total_tokens, metadata_json
                        FROM sessions
                        WHERE (?1 IS NULL OR created_at_ms >= ?1)
                          AND (?2 IS NULL OR updated_at_ms >= ?2)
                          AND session_id NOT IN (SELECT session_id FROM session_heads)
                        ",
                    )
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                let rows = stmt
                    .query_map(params![created_after, updated_after], session_meta_from_row)
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                for row in rows {
                    metas.push(
                        row.map_err(StoreError::from)
                            .map_err(into_session_store_error)?,
                    );
                }
            }
            metas.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
            });
            let offset = filter.offset.unwrap_or(0);
            let limit = filter.limit.unwrap_or(usize::MAX);
            Ok(metas.into_iter().skip(offset).take(limit).collect())
        })
        .await
        .map_err(StoreError::Join)
        .map_err(into_session_store_error)?
    }

    /// Metadata-only partial read over the durable projection columns —
    /// head row wins, legacy blob row is the fallback. Never touches
    /// `session_json` or strand rows, so it survives a corrupt or unreadable
    /// full session document.
    async fn load_meta(&self, id: &SessionId) -> Result<Option<SessionMeta>, SessionStoreError> {
        let path = self.path.clone();
        let options = self.options;
        let session_id = id.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<SessionMeta>, SessionStoreError> {
            let _guard = meerkat_sqlite::OperationGuard::for_database(&path)
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            let conn = open_session_connection(&path, options).map_err(into_session_store_error)?;
            let mut meta = conn
                .query_row(
                    r"
                    SELECT session_id, created_at_ms, updated_at_ms, message_count,
                           total_tokens, metadata_json
                    FROM session_heads
                    WHERE session_id = ?1
                    ",
                    params![session_id],
                    session_meta_from_row,
                )
                .optional()
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            if meta.is_none() {
                meta = conn
                    .query_row(
                        r"
                        SELECT session_id, created_at_ms, updated_at_ms, message_count,
                               total_tokens, metadata_json
                        FROM sessions
                        WHERE session_id = ?1
                        ",
                        params![session_id],
                        session_meta_from_row,
                    )
                    .optional()
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
            }
            Ok(meta)
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
            let previous = if let Some((head, _token)) = head_row_in_txn(tx, &session_id)? {
                Some(materialize_slim_in_txn(tx, &session_id, &head)?)
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
    for sql in [
        "DELETE FROM sessions WHERE session_id = ?1",
        "DELETE FROM session_strand_messages WHERE session_id = ?1",
        "DELETE FROM session_strand_links WHERE session_id = ?1",
        "DELETE FROM session_rewrites WHERE session_id = ?1",
        "DELETE FROM session_heads WHERE session_id = ?1",
    ] {
        tx.execute(sql, params![id.to_string()])
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?;
    }
    Ok(())
}

fn rewrite_record_from_session_bodies(
    session: &Session,
    commit: &TranscriptRewriteCommit,
) -> Result<TranscriptRewriteRecord, SessionStoreError> {
    let parent_body = session
        .transcript_revision_body(&commit.parent_revision)
        .map_err(SessionStoreError::from)?
        .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!(
                "incoming rewrite omitted parent revision body {}",
                commit.parent_revision
            ),
        })?;
    let revision_body = session
        .transcript_revision_body(&commit.revision)
        .map_err(SessionStoreError::from)?
        .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!(
                "incoming rewrite omitted new revision body {}",
                commit.revision
            ),
        })?;
    TranscriptRewriteRecord::new(commit.clone(), parent_body, revision_body).map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("transcript rewrite record failed validation: {err}"),
        }
    })
}

/// Shared commit_rewrite body used by the trait method and the compat
/// `save_transcript_rewrite` rewiring.
fn commit_rewrite_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
    record: &TranscriptRewriteRecord,
    expected: &SessionHeadCas,
    stored: &(SessionHead, String),
) -> Result<SessionHead, SessionStoreError> {
    let (stored_head, stored_token) = stored;
    // CAS races and stale parents must surface as TranscriptRevisionConflict
    // BEFORE the parent strand range read, which would otherwise fail on an
    // unrelated shape (the advanced head strand is shorter than the stale
    // commit's messages_before).
    match expected {
        SessionHeadCas::Create => {
            return Err(SessionStoreError::TranscriptRevisionConflict {
                id: id.clone(),
                expected: "<create>".to_string(),
                actual: stored_token.clone(),
            });
        }
        SessionHeadCas::IfToken(expected_token) => {
            if expected_token != stored_token {
                return Err(SessionStoreError::TranscriptRevisionConflict {
                    id: id.clone(),
                    expected: expected_token.clone(),
                    actual: stored_token.clone(),
                });
            }
        }
    }
    if record.commit.parent_revision != stored_head.head_revision {
        return Err(SessionStoreError::TranscriptRevisionConflict {
            id: id.clone(),
            expected: record.commit.parent_revision.clone(),
            actual: stored_head.head_revision.clone(),
        });
    }
    let before = record.commit.messages_before as u64;
    let links = strand_links_in_txn(tx, id)?;
    if before > strand_logical_len_in_txn(tx, id, &stored_head.strand, &links)? {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: format!(
                "commit messages_before {before} exceeds persisted rows of strand {}",
                stored_head.strand
            ),
        });
    }
    let parent_rows =
        strand_messages_with_links_in_txn(tx, id, &stored_head.strand, 0..before, &links)?;
    let parent_digest =
        transcript_messages_digest(&parent_rows).map_err(SessionStoreError::from)?;
    let next = validate_commit_rewrite_transition(
        id,
        record,
        stored_head,
        stored_token,
        expected,
        &parent_digest,
    )?;
    insert_rewrite_row_in_txn(
        tx,
        id,
        stored_head.rewrite_count,
        &RewriteRow {
            commit: record.commit.clone(),
            parent_strand: stored_head.strand.clone(),
            parent_len: before,
            strand: next.strand.clone(),
            strand_len: record.commit.messages_after as u64,
        },
    )?;
    insert_strand_rows_in_txn(tx, id, &next.strand, 0, &record.revision_body.messages)?;
    Ok(next)
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
        record: &TranscriptRewriteRecord,
        expected: SessionHeadCas,
    ) -> Result<SessionHead, SessionStoreError> {
        let id = id.clone();
        let record = record.clone();
        self.in_write_txn(move |tx| {
            let stored = ensure_head_canonical_for_write_in_txn(tx, &id)?.ok_or_else(|| {
                SessionStoreError::InvalidTranscriptRewrite {
                    id: id.clone(),
                    reason: "rewrite target has no persisted session head".to_string(),
                }
            })?;
            commit_rewrite_in_txn(tx, &id, &record, &expected, &stored)
        })
        .await
    }

    async fn save_head(
        &self,
        head: &SessionHead,
        expected: SessionHeadCas,
    ) -> Result<(), SessionStoreError> {
        let head = head.clone();
        self.in_write_txn(move |tx| {
            let stored = ensure_head_canonical_for_write_in_txn(tx, &head.id)?;
            let links = strand_links_in_txn(tx, &head.id)?;
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
        id: &SessionId,
    ) -> Result<Vec<TranscriptRewriteRecord>, SessionStoreError> {
        let id = id.clone();
        self.in_read_txn(move |tx| {
            if let Some((head, _token)) = head_row_in_txn(tx, &id)? {
                let rows = rewrite_rows_in_txn(tx, &id, head.rewrite_count)?;
                // One link map for the whole reconstruction: each retained
                // body resolves through the supersession chain back to the
                // live strand.
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
            let (layout, _head) = layout_for_blob_session(&session)?;
            layout
                .rewrites
                .into_iter()
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
    use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, Message, UserMessage};
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
    async fn canonical_head_is_row_only_and_never_synthesizes_for_blob_only() {
        let (_dir, store) = temp_store();

        // Blob-only session: plain save writes the legacy blob row; no head
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
        rewrite_record_from_session_bodies(session, commit).unwrap()
    }

    #[tokio::test]
    async fn incremental_commit_rewrite_guards_and_load_rewrites() {
        let (_dir, store) = temp_store();
        let inc = incremental(&store);
        let (parent, compacted, commit) = compacted_fixture();
        let head = seed_incremental(&inc, &parent).await;
        let token = session_head_cas_token(&head).unwrap();
        let record = record_for(&compacted, &commit);

        // Stale parent (after an intervening append + head bump) conflicts —
        // twin of save_transcript_rewrite_rejects_stale_parent_after_intervening_save.
        {
            let (_dir2, store2) = temp_store();
            let inc2 = incremental(&store2);
            let mut newer = parent.clone();
            let head2 = seed_incremental(&inc2, &newer).await;
            newer.push(user("intervening"));
            inc2.append_messages(
                newer.id(),
                &head2.strand,
                head2.message_count,
                &[user("intervening")],
            )
            .await
            .unwrap();
            let bumped = SessionHead::from_session(&newer, head2.strand.clone(), 0).unwrap();
            let token2 = session_head_cas_token(&head2).unwrap();
            inc2.save_head(&bumped, SessionHeadCas::IfToken(token2))
                .await
                .unwrap();
            let bumped_token = session_head_cas_token(&bumped).unwrap();
            let err = inc2
                .commit_rewrite(newer.id(), &record, SessionHeadCas::IfToken(bumped_token))
                .await
                .expect_err("stale parent revision must conflict");
            assert!(
                matches!(err, SessionStoreError::TranscriptRevisionConflict { .. }),
                "unexpected error: {err}"
            );
        }

        // Unadopted commits are invisible to load_rewrites; re-commit is
        // idempotent.
        let next = inc
            .commit_rewrite(parent.id(), &record, SessionHeadCas::IfToken(token.clone()))
            .await
            .unwrap();
        assert!(inc.load_rewrites(parent.id()).await.unwrap().is_empty());
        let retried = inc
            .commit_rewrite(parent.id(), &record, SessionHeadCas::IfToken(token.clone()))
            .await
            .expect("unadopted re-commit must be idempotent");
        assert_eq!(retried, next);

        // Adoption makes the record visible and valid.
        inc.save_head(&next, SessionHeadCas::IfToken(token))
            .await
            .unwrap();
        let records = inc.load_rewrites(parent.id()).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].commit, commit);
        // Every returned record passed TranscriptRewriteRecord::new inside
        // load_rewrites; double-check it revalidates here.
        TranscriptRewriteRecord::new(
            records[0].commit.clone(),
            records[0].parent_body.clone(),
            records[0].revision_body.clone(),
        )
        .expect("reconstructed record must validate");
    }

    #[tokio::test]
    async fn incremental_migration_from_legacy_blob() {
        let (_dir, store) = temp_store();

        // Legacy blob fixture with 2 compaction commits + retained bodies.
        let mut session = Session::new();
        session.push(user("turn one"));
        session.push(user("turn two"));
        store.save(&session).await.unwrap();
        session
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
        session.push(user("turn four"));

        // Reconstruct the pre-fix shape: every ordinary append retained a
        // complete head body even though it was not an audited rewrite.
        let mut legacy_revisions = session
            .transcript_history_state()
            .unwrap()
            .unwrap()
            .revisions;
        for turn in 0..16 {
            session.push(user(&format!("legacy ordinary turn {turn}")));
            let state = session.transcript_history_state().unwrap().unwrap();
            let head = state
                .revisions
                .iter()
                .find(|body| body.revision == state.head)
                .unwrap()
                .clone();
            if legacy_revisions
                .iter()
                .all(|body| body.revision != head.revision)
            {
                legacy_revisions.push(head);
            }
        }
        // Write the fat blob through the legacy authoritative path.
        store.save_authoritative_projection(&session).await.unwrap();
        let mut legacy_envelope = serde_json::to_value(&session).unwrap();
        legacy_envelope["metadata"][meerkat_core::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY]
            ["revisions"] = serde_json::to_value(&legacy_revisions).unwrap();
        {
            let conn = open_connection(store.path()).unwrap();
            conn.execute(
                "UPDATE sessions SET session_json = ?1 WHERE session_id = ?2",
                params![
                    serde_json::to_vec(&legacy_envelope).unwrap(),
                    session.id().to_string()
                ],
            )
            .unwrap();
        }
        assert!(blob_row_bytes(store.path(), session.id()).is_some());

        let inc = incremental(&store);
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
    async fn incremental_migration_heals_pre_0_7_14_legacy_digests() {
        // Pre-0.7.14 blobs carry bookkeeping-inclusive revision strings; the
        // migration parse path must heal them exactly like Session::deserialize.
        let (_dir, store) = temp_store();
        let mut session = Session::new();
        session.push(user("before rewrite"));
        session.push(user("retained tail"));
        store.save(&session).await.unwrap();
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![user("after rewrite")],
                TranscriptRewriteReason::new("compaction"),
                Some("legacy-test".to_string()),
                None,
            )
            .unwrap();
        store.save_authoritative_projection(&session).await.unwrap();

        // Rewrite the stored blob's revision strings to the legacy
        // (bookkeeping-inclusive) digest of each retained body — the exact
        // shape a pre-0.7.14 writer persisted.
        let blob = blob_row_bytes(store.path(), session.id()).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&blob).unwrap();
        // The pre-0.7.14 digest was computed over the serialized `Message`
        // vector (bookkeeping-inclusive, image-normalized only) — decode the
        // stored Value back into `Message`s before digesting so the byte
        // shape matches what a legacy writer hashed.
        let legacy_digest = |messages: &serde_json::Value| -> String {
            use sha2::{Digest, Sha256};
            let typed: Vec<Message> = serde_json::from_value(messages.clone()).unwrap();
            let bytes = serde_json::to_vec(&typed).unwrap();
            let digest = Sha256::digest(bytes);
            let mut out = String::new();
            for byte in digest {
                out.push_str(&format!("{byte:02x}"));
            }
            format!("sha256:{out}")
        };
        {
            let state = value
                .get_mut("metadata")
                .unwrap()
                .get_mut(meerkat_core::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
                .unwrap();
            // Source the bodies from a TYPED parse, not the raw array: the
            // durable encoding of `revisions` is an anchor plus a chain of
            // splices, so only entry 0 spells `messages` verbatim. The typed
            // parse materializes every body, which is what the legacy digest
            // is computed over. The raw string-replace below is unaffected —
            // revision strings appear in the delta spelling too.
            let typed: meerkat_core::TranscriptHistoryState =
                serde_json::from_value(state.clone()).unwrap();
            let mut remap: Vec<(String, String)> = Vec::new();
            for body in &typed.revisions {
                let current = body.revision.clone();
                let legacy = legacy_digest(&serde_json::to_value(&body.messages).unwrap());
                remap.push((current, legacy));
            }
            let mut raw = serde_json::to_string(state).unwrap();
            for (current, legacy) in &remap {
                raw = raw.replace(current, legacy);
            }
            *state = serde_json::from_str(&raw).unwrap();
            // A pre-0.7.14 writer predates the digest-format marker; the
            // fixture blob was produced by the CURRENT writer (which stamps
            // it), so strip the marker or the forged legacy strings ride a
            // stamp that legitimately skips the decode-time heal probe.
            state.as_object_mut().unwrap().remove("digest_format");
        }
        {
            let conn = open_connection(store.path()).unwrap();
            conn.execute(
                "UPDATE sessions SET session_json = ?1 WHERE session_id = ?2",
                params![
                    serde_json::to_vec(&value).unwrap(),
                    session.id().to_string()
                ],
            )
            .unwrap();
        }

        let inc = incremental(&store);
        let synthesized = inc
            .load_head(session.id())
            .await
            .expect("legacy-digest blob must synthesize")
            .unwrap();
        assert_eq!(synthesized.rewrite_count, 1);
        // The healed head revision is content-addressed (matches the live digest).
        assert_eq!(
            synthesized.head_revision,
            transcript_messages_digest(session.messages()).unwrap()
        );
        let records = inc.load_rewrites(session.id()).await.unwrap();
        assert_eq!(records.len(), 1);
    }

    #[tokio::test]
    async fn head_canonical_compat_save_paths() {
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

        // save_transcript_rewrite adopts a commit with the legacy error surface.
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
        store
            .save_transcript_rewrite(&compacted, &commit)
            .await
            .unwrap();
        let head = inc.load_head(session.id()).await.unwrap().unwrap();
        assert_eq!(head.rewrite_count, 1);
        assert_eq!(head.message_count, 1);
        let slim = store.load(session.id()).await.unwrap().unwrap();
        assert_eq!(slim.messages().len(), 1);

        // A STALE rewrite (same parent, replayed against the advanced head)
        // surfaces the legacy TranscriptRevisionConflict.
        let err = store
            .save_transcript_rewrite(&compacted, &commit)
            .await
            .expect_err("stale rewrite must conflict");
        assert!(
            matches!(err, SessionStoreError::TranscriptRevisionConflict { .. }),
            "unexpected error: {err}"
        );

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

    /// FIELD PIN (blob-growth regression): a compaction that removes half
    /// the transcript persists O(live-after) — the reachable head-strand row
    /// count equals messages_after, `sessions.session_json` is never
    /// written, and an append-only follow-up inserts exactly the delta rows.
    #[tokio::test]
    async fn field_pin_compaction_shrinks_persisted_head() {
        let (_dir, store) = temp_store();
        let inc = incremental(&store);
        let (parent, compacted, commit) = compacted_fixture();
        let head = seed_incremental(&inc, &parent).await;
        let token = session_head_cas_token(&head).unwrap();

        assert!(
            blob_row_bytes(store.path(), parent.id()).is_none(),
            "incremental sessions must never write the legacy blob"
        );

        let record = record_for(&compacted, &commit);
        let next = inc
            .commit_rewrite(parent.id(), &record, SessionHeadCas::IfToken(token.clone()))
            .await
            .unwrap();
        inc.save_head(&next, SessionHeadCas::IfToken(token))
            .await
            .unwrap();

        assert!(commit.messages_after < commit.messages_before);
        assert_eq!(
            strand_row_count(store.path(), parent.id(), &next.strand) as usize,
            commit.messages_after,
            "reachable head-strand rows must equal messages_after (the shrink)"
        );
        assert!(
            blob_row_bytes(store.path(), parent.id()).is_none(),
            "compaction must not write the legacy blob"
        );

        // Append-only follow-up turn inserts exactly the delta rows.
        let mut followed = store.load(parent.id()).await.unwrap().unwrap();
        followed.push(user("follow-up question"));
        followed.push(user("follow-up answer"));
        store.save(&followed).await.unwrap();
        assert_eq!(
            strand_row_count(store.path(), parent.id(), &next.strand) as usize,
            commit.messages_after + 2,
            "follow-up must append exactly the delta rows"
        );
        assert!(blob_row_bytes(store.path(), parent.id()).is_none());
    }

    // -----------------------------------------------------------------
    // Strand growth: durable rows must stay bounded by the live
    // transcript plus the spans superseded strands genuinely retain.
    // -----------------------------------------------------------------

    /// Replace message 0 in place, keeping every later message — the field
    /// shape (a per-resume system-prompt refresh). It shares NO prefix with
    /// its parent, which is exactly the case whole-strand storage cannot
    /// compress.
    fn leading_rewrite(session: &mut Session, text: &str) -> meerkat_core::TranscriptRewriteCommit {
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![user(text)],
                TranscriptRewriteReason::new("system-refresh"),
                Some("test".to_string()),
                None,
            )
            .unwrap()
    }

    /// FIELD PIN (strand-growth regression): rewrites that share no prefix
    /// must cost their retained span, not a fresh copy of the transcript.
    ///
    /// Measured pre-fix on a production dump: 98 rewrites of a 371-message
    /// transcript accumulated 16,672 strand rows. The bound here is
    /// `live + one retained row per rewrite`.
    #[tokio::test]
    async fn field_pin_leading_rewrites_do_not_copy_the_transcript_per_revision() {
        const TURNS: usize = 12;
        const REWRITES: usize = 6;

        let (_dir, store) = temp_store();
        let inc = incremental(&store);
        let mut seeded = Session::new();
        for turn in 0..TURNS {
            seeded.push(user(&format!("turn {turn}")));
        }
        seed_incremental(&inc, &seeded).await;

        // Each round mirrors production: load the slim transcript, rewrite
        // message 0, persist through the compat rewrite path.
        let mut expected_bodies: Vec<(Vec<Message>, Vec<Message>)> = Vec::new();
        let mut live = store.load(seeded.id()).await.unwrap().unwrap();
        let original = live.messages().to_vec();
        for round in 0..REWRITES {
            let parent_body = live.messages().to_vec();
            let commit = leading_rewrite(&mut live, &format!("[system refresh {round}]"));
            expected_bodies.push((parent_body, live.messages().to_vec()));
            store.save_transcript_rewrite(&live, &commit).await.unwrap();
            live = store.load(seeded.id()).await.unwrap().unwrap();
        }

        // The bound: one materialized transcript plus one retained row per
        // superseded strand.
        assert_eq!(
            total_strand_rows(store.path(), seeded.id()),
            (TURNS + REWRITES) as i64,
            "strand rows must be bounded by live transcript + retained deltas"
        );
        assert_eq!(
            strand_link_count(store.path(), seeded.id()),
            REWRITES as i64,
            "each superseded strand must carry exactly one supersession edge"
        );

        // The live head strand stays fully materialized, so resume never
        // walks the supersession chain.
        let head = inc.load_head(seeded.id()).await.unwrap().unwrap();
        assert_eq!(head.rewrite_count, REWRITES as u64);
        assert_eq!(
            strand_row_count(store.path(), seeded.id(), &head.strand) as usize,
            TURNS,
            "the live strand must remain materialized"
        );
        let slim = store.load(seeded.id()).await.unwrap().unwrap();
        assert_eq!(slim.messages(), live.messages());

        // Every retained body still reconstructs exactly, in commit order.
        let records = inc.load_rewrites(seeded.id()).await.unwrap();
        assert_eq!(records.len(), REWRITES);
        for (index, (record, (parent_body, revision_body))) in
            records.iter().zip(&expected_bodies).enumerate()
        {
            assert_eq!(
                &record.parent_body.messages, parent_body,
                "rewrite {index} parent body must reconstruct from the splice"
            );
            assert_eq!(
                &record.revision_body.messages, revision_body,
                "rewrite {index} revision body must reconstruct from the splice"
            );
            TranscriptRewriteRecord::new(
                record.commit.clone(),
                record.parent_body.clone(),
                record.revision_body.clone(),
            )
            .expect("reconstructed record must revalidate");
        }

        // The oldest strand — physically down to one row — still serves its
        // whole logical body through the chain.
        let root = TranscriptStrandId::root();
        assert_eq!(
            strand_row_count(store.path(), seeded.id(), &root),
            1,
            "the root strand must retain only the message the rewrite replaced"
        );
        let served = inc
            .load_messages(seeded.id(), &root, 0..TURNS as u64)
            .await
            .unwrap();
        assert_eq!(
            served, original,
            "a superseded strand must serve its original rows through the chain"
        );
        for start in 0..TURNS as u64 {
            let page = inc
                .load_messages(seeded.id(), &root, start..TURNS as u64)
                .await
                .unwrap();
            assert_eq!(
                page,
                original[start as usize..].to_vec(),
                "sub-range {start}.. of a superseded strand must page-serve exactly"
            );
        }
        assert!(
            inc.load_messages(seeded.id(), &root, 0..(TURNS as u64 + 1))
                .await
                .is_err(),
            "a read past a superseded strand's logical length must fail closed"
        );
    }

    /// A full-transcript compaction shares nothing with its child, so it
    /// legitimately retains its whole parent — but the edge is still
    /// recorded, and a later leading rewrite on top must not re-copy.
    #[tokio::test]
    async fn compaction_retains_its_parent_and_still_bounds_later_rewrites() {
        let (_dir, store) = temp_store();
        let inc = incremental(&store);
        let (parent, compacted, commit) = compacted_fixture();
        seed_incremental(&inc, &parent).await;
        store
            .save_transcript_rewrite(&compacted, &commit)
            .await
            .unwrap();

        // 4 parent rows retained in full (nothing is shared) + 2 live rows.
        assert_eq!(total_strand_rows(store.path(), parent.id()), 6);
        assert_eq!(strand_link_count(store.path(), parent.id()), 1);

        let mut live = store.load(parent.id()).await.unwrap().unwrap();
        let second = leading_rewrite(&mut live, "[system refresh]");
        store.save_transcript_rewrite(&live, &second).await.unwrap();

        // The compacted strand shares its tail with the refreshed one, so it
        // retains a single row: 4 + 1 + 2.
        assert_eq!(
            total_strand_rows(store.path(), parent.id()),
            7,
            "a leading rewrite over a compacted transcript must retain one row"
        );
        let records = inc.load_rewrites(parent.id()).await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].parent_body.messages, parent.messages().to_vec());
        assert_eq!(
            records[0].revision_body.messages,
            compacted.messages().to_vec()
        );
        assert_eq!(records[1].revision_body.messages, live.messages().to_vec());
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

    /// Replacing an unadopted rewrite row abandons its child strand; those
    /// rows are reachable by no verb and must not linger.
    #[tokio::test]
    async fn replacing_an_unadopted_rewrite_collects_the_abandoned_child() {
        let (_dir, store) = temp_store();
        let inc = incremental(&store);
        let mut seeded = Session::new();
        for turn in 0..4 {
            seeded.push(user(&format!("turn {turn}")));
        }
        let head = seed_incremental(&inc, &seeded).await;
        let token = session_head_cas_token(&head).unwrap();

        let mut first = seeded.clone();
        let first_commit = leading_rewrite(&mut first, "[refresh A]");
        let mut second = seeded.clone();
        let second_commit = leading_rewrite(&mut second, "[refresh B]");
        assert_ne!(first_commit.revision, second_commit.revision);

        inc.commit_rewrite(
            seeded.id(),
            &record_for(&first, &first_commit),
            SessionHeadCas::IfToken(token.clone()),
        )
        .await
        .unwrap();
        let abandoned = TranscriptStrandId::from_rewrite(&first_commit);
        assert_eq!(strand_row_count(store.path(), seeded.id(), &abandoned), 4);

        let next = inc
            .commit_rewrite(
                seeded.id(),
                &record_for(&second, &second_commit),
                SessionHeadCas::IfToken(token.clone()),
            )
            .await
            .unwrap();
        assert_eq!(
            strand_row_count(store.path(), seeded.id(), &abandoned),
            0,
            "the replaced rewrite's child strand must be collected"
        );

        inc.save_head(&next, SessionHeadCas::IfToken(token))
            .await
            .unwrap();
        assert_eq!(
            total_strand_rows(store.path(), seeded.id()),
            5,
            "one live transcript plus the superseded root's retained row"
        );
        let records = inc.load_rewrites(seeded.id()).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].commit.revision, second_commit.revision);
        assert_eq!(records[0].parent_body.messages, seeded.messages().to_vec());
    }

    /// A superseded strand's rows are immutable history: identical bytes
    /// stay idempotent, divergent or extending bytes fail closed.
    #[tokio::test]
    async fn superseded_strand_rows_stay_immutable() {
        let (_dir, store) = temp_store();
        let inc = incremental(&store);
        let mut seeded = Session::new();
        for turn in 0..3 {
            seeded.push(user(&format!("turn {turn}")));
        }
        seed_incremental(&inc, &seeded).await;
        let mut live = store.load(seeded.id()).await.unwrap().unwrap();
        let original = live.messages().to_vec();
        let commit = leading_rewrite(&mut live, "[system refresh]");
        store.save_transcript_rewrite(&live, &commit).await.unwrap();

        let root = TranscriptStrandId::root();
        assert_eq!(strand_link_count(store.path(), seeded.id()), 1);

        // Re-appending the exact rows the strand still holds is idempotent.
        inc.append_messages(seeded.id(), &root, 0, &original)
            .await
            .expect("identical re-append of a superseded strand must be idempotent");

        // Diverging bytes at a retained position fail closed.
        let err = inc
            .append_messages(seeded.id(), &root, 0, &[user("divergent")])
            .await
            .expect_err("divergent overwrite of superseded history must be rejected");
        assert!(
            matches!(err, SessionStoreError::TranscriptContinuityViolation { .. }),
            "unexpected error: {err}"
        );

        // Extending a strand whose future is already recorded fails closed.
        let err = inc
            .append_messages(seeded.id(), &root, 3, &[user("late append")])
            .await
            .expect_err("extending a superseded strand must be rejected");
        assert!(
            matches!(err, SessionStoreError::TranscriptContinuityViolation { .. }),
            "unexpected error: {err}"
        );
        assert_eq!(total_strand_rows(store.path(), seeded.id()), 4);
    }
}
