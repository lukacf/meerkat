//! [`SqliteDecisionStore`]: durable decisions in one SQLite file.
//!
//! The decisions of a host's persistence live in
//! `approval_decisions.sqlite3` beside its approval records
//! (`approvals.json`) under the store path, schema domain
//! `approval-decisions`, version 1. The approval records are untouched: a
//! binary that knows nothing of durable decisions never opens this file.
//!
//! The file is created by the first hold. Opening the store reads and
//! creates nothing, and a read of a missing file answers empty without
//! creating it. Each call opens its own connection under the realm's
//! maintenance fence. Every write is one `BEGIN IMMEDIATE` transaction on a
//! connection with a zero busy timeout, conditional on the row it expects:
//! a database another writer holds is [`ApprovalStoreError::Contended`],
//! never a wait.
//!
//! A row keeps its whole JSON beside the columns its queries read. The
//! owed, due, count and filtered list queries read their indexes; a row
//! whose JSON does not decode is answered unreadable for that decision
//! only.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use meerkat_core::approval::ApprovalStoreError;
use meerkat_core::approval::decision::{
    DECISION_PAGE_MAX, DecisionId, DecisionListFilter, DecisionPage, DecisionPrincipalKey,
    DecisionRow, DecisionStore, StoredDecision, UnreadableDecision,
};
use rusqlite::types::Value;
use rusqlite::{Connection, ToSql, Transaction, TransactionBehavior, params_from_iter};

/// The file name of a host's durable decisions, under its store path.
pub const APPROVAL_DECISIONS_FILE: &str = "approval_decisions.sqlite3";

const CREATE_DECISIONS_TABLE_SQL: &str = r"
CREATE TABLE approval_decisions (
    decision_id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    requester_key TEXT NOT NULL,
    approver_id TEXT NOT NULL,
    reviewer_key TEXT,
    phase TEXT NOT NULL,
    notice TEXT NOT NULL,
    deadline_at_ms INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    row_json BLOB NOT NULL
)";

const CREATE_DECISIONS_INDEXES_SQL: &str = r"
CREATE INDEX approval_decisions_phase_deadline_idx
    ON approval_decisions(phase, deadline_at_ms);
CREATE INDEX approval_decisions_notice_idx
    ON approval_decisions(notice);
CREATE INDEX approval_decisions_approver_phase_idx
    ON approval_decisions(approver_id, phase);
CREATE INDEX approval_decisions_requester_phase_idx
    ON approval_decisions(requester_key, phase);
CREATE INDEX approval_decisions_kind_phase_idx
    ON approval_decisions(kind, phase);
";

fn migration_0001_approval_decisions_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(CREATE_DECISIONS_TABLE_SQL)?;
    tx.execute_batch(CREATE_DECISIONS_INDEXES_SQL)?;
    Ok(())
}

/// The durable decision store's schema domain in the file's migration
/// ledger. Every file of the domain was created with its ledger row, so the
/// offline bridge infers no version for an unledgered one.
pub const APPROVAL_DECISIONS_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "approval-decisions",
    migrations: &[meerkat_sqlite::Migration {
        version: 1,
        name: "base-schema",
        apply: migration_0001_approval_decisions_schema,
    }],
    initialize_current: migration_0001_approval_decisions_schema,
    allowed_existing_versions: &[1],
    bridge_recoverable_versions: &[],
    released_predecessors: &[],
    owned_objects: &[
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "approval_decisions",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "approval_decisions_phase_deadline_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "approval_decisions_notice_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "approval_decisions_approver_phase_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "approval_decisions_requester_phase_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "approval_decisions_kind_phase_idx",
        },
    ],
    retired_objects: &[],
};

const GET_SQL: &str = "SELECT decision_id, row_json FROM approval_decisions WHERE decision_id = ?1";

const INSERT_SQL: &str = "INSERT INTO approval_decisions (decision_id, kind, operation_id, \
    requester_key, approver_id, reviewer_key, phase, notice, deadline_at_ms, revision, row_json) \
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) ON CONFLICT(decision_id) DO NOTHING";

const REPLACE_SQL: &str = "UPDATE approval_decisions SET kind = ?2, operation_id = ?3, \
    requester_key = ?4, approver_id = ?5, reviewer_key = ?6, phase = ?7, notice = ?8, \
    deadline_at_ms = ?9, revision = ?10, row_json = ?11 WHERE decision_id = ?1 AND revision = ?12";

/// The rows that owe work: approved, or owing their notice. The phase and
/// notice tokens are `DecisionPhase::Approved` and `DecisionNotice::Owed`.
const OWED_SQL: &str = "SELECT decision_id, row_json FROM approval_decisions \
    WHERE (phase = 'approved' OR notice = 'owed') AND decision_id > ?1 \
    ORDER BY decision_id LIMIT ?2";

/// The open and approved rows at or past their deadline.
const DUE_SQL: &str = "SELECT decision_id, row_json FROM approval_decisions \
    WHERE phase IN ('open', 'approved') AND deadline_at_ms <= ?1 AND decision_id > ?2 \
    ORDER BY decision_id LIMIT ?3";

/// One operation's connection; the fence guard lives exactly as long as the
/// connection it admits.
struct DecisionConnection {
    conn: Connection,
    _guard: meerkat_sqlite::OperationGuard,
}

fn open_error(error: meerkat_sqlite::SqliteStoreError) -> ApprovalStoreError {
    match error {
        meerkat_sqlite::SqliteStoreError::Sqlite(error)
            if meerkat_sqlite::is_busy_or_locked(&error) =>
        {
            ApprovalStoreError::Contended
        }
        other => ApprovalStoreError::NotCommitted(other.to_string()),
    }
}

fn sqlite_error(error: &rusqlite::Error) -> ApprovalStoreError {
    if meerkat_sqlite::is_busy_or_locked(error) {
        ApprovalStoreError::Contended
    } else {
        ApprovalStoreError::NotCommitted(error.to_string())
    }
}

/// A failed `COMMIT`: a held database rolls the transaction back, and any
/// other failure may have committed it.
fn commit_error(error: &rusqlite::Error) -> ApprovalStoreError {
    if meerkat_sqlite::is_busy_or_locked(error) {
        ApprovalStoreError::Contended
    } else {
        ApprovalStoreError::CommitUnknown(error.to_string())
    }
}

fn integer(value: u64, what: &str) -> Result<i64, ApprovalStoreError> {
    i64::try_from(value).map_err(|_| {
        ApprovalStoreError::NotCommitted(format!("the decision's {what} does not fit the store"))
    })
}

/// The stored row of `decision_id` from its JSON.
fn decode(decision_id: String, bytes: &[u8]) -> DecisionRow {
    match serde_json::from_slice::<StoredDecision>(bytes) {
        Ok(row) if row.decision_id.as_str() == decision_id => DecisionRow::Readable(Box::new(row)),
        Ok(row) => DecisionRow::Unreadable(UnreadableDecision {
            decision_id,
            reason: format!("the row names the decision {}", row.decision_id),
        }),
        Err(error) => DecisionRow::Unreadable(UnreadableDecision {
            decision_id,
            reason: error.to_string(),
        }),
    }
}

/// The `column = ?n` clauses of `filter`, numbered after `bound` earlier
/// parameters, with their values.
fn filter_clauses(filter: &DecisionListFilter, bound: usize) -> (Vec<String>, Vec<Value>) {
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    let mut push = |column: &str, value: &str| {
        values.push(Value::Text(value.to_owned()));
        clauses.push(format!("{column} = ?{}", bound + values.len()));
    };
    if let Some(kind) = &filter.kind {
        push("kind", kind.as_str());
    }
    if let Some(phase) = filter.phase {
        push("phase", phase.as_str());
    }
    if let Some(approver_id) = &filter.approver_id {
        push("approver_id", approver_id.as_str());
    }
    if let Some(requester) = &filter.requester {
        push("requester_key", requester.as_str());
    }
    (clauses, values)
}

/// The list query of `filter` after `after`, reading one row past `limit`.
fn list_query(
    filter: &DecisionListFilter,
    after: Option<&str>,
    limit: usize,
) -> (String, Vec<Value>) {
    let (filters, filter_values) = filter_clauses(filter, 1);
    let mut sql =
        String::from("SELECT decision_id, row_json FROM approval_decisions WHERE decision_id > ?1");
    for clause in &filters {
        let _ = write!(sql, " AND {clause}");
    }
    let mut values = Vec::with_capacity(filter_values.len() + 2);
    values.push(Value::Text(after.unwrap_or_default().to_owned()));
    values.extend(filter_values);
    values.push(Value::Integer(fetch_limit(limit)));
    let _ = write!(sql, " ORDER BY decision_id LIMIT ?{}", values.len());
    (sql, values)
}

/// The count query of `filter`.
fn count_query(filter: &DecisionListFilter) -> (String, Vec<Value>) {
    let (filters, values) = filter_clauses(filter, 0);
    let mut sql = String::from("SELECT COUNT(*) FROM approval_decisions");
    if !filters.is_empty() {
        let _ = write!(sql, " WHERE {}", filters.join(" AND "));
    }
    (sql, values)
}

/// The page size of `limit`, and the rows a query reads for it: one more,
/// to know whether another page follows.
fn page_limit(limit: usize) -> usize {
    limit.clamp(1, DECISION_PAGE_MAX)
}

fn fetch_limit(limit: usize) -> i64 {
    i64::try_from(page_limit(limit) + 1).unwrap_or(i64::MAX)
}

/// One page of `limit` rows from `sql`, which reads one more.
fn read_page(
    conn: &Connection,
    sql: &str,
    values: &[Value],
    limit: usize,
) -> Result<DecisionPage, ApprovalStoreError> {
    let limit = page_limit(limit);
    let mut statement = conn.prepare(sql).map_err(|error| sqlite_error(&error))?;
    let mut rows = statement
        .query(params_from_iter(values.iter()))
        .map_err(|error| sqlite_error(&error))?;
    let mut page = DecisionPage::default();
    while let Some(row) = rows.next().map_err(|error| sqlite_error(&error))? {
        if page.rows.len() == limit {
            page.next_after = page.rows.last().map(|last| last.decision_id().to_owned());
            break;
        }
        let decision_id: String = row.get(0).map_err(|error| sqlite_error(&error))?;
        let entry = match row
            .get_ref(1)
            .map_err(|error| sqlite_error(&error))?
            .as_bytes()
        {
            Ok(bytes) => decode(decision_id, bytes),
            Err(error) => DecisionRow::Unreadable(UnreadableDecision {
                decision_id,
                reason: error.to_string(),
            }),
        };
        page.rows.push(entry);
    }
    Ok(page)
}

/// Durable decisions in one SQLite file. See the module docs.
#[derive(Debug, Clone)]
pub struct SqliteDecisionStore {
    path: PathBuf,
}

impl SqliteDecisionStore {
    /// The store of the database file at `path`. Opening touches nothing.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The store of a host's durable decisions: `approval_decisions.sqlite3`
    /// under its store path, beside `approvals.json`.
    #[must_use]
    pub fn in_store_path(store_path: &Path) -> Self {
        Self::new(store_path.join(APPROVAL_DECISIONS_FILE))
    }

    /// The database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A connection to the file, created when `create` is set. Every open
    /// checks the domain's ledger before anything else: a file a newer
    /// binary migrated is refused, never read.
    fn connect(&self, create: bool) -> Result<DecisionConnection, ApprovalStoreError> {
        let guard = meerkat_sqlite::OperationGuard::for_database(&self.path).map_err(open_error)?;
        let mut conn = meerkat_sqlite::open_with(
            &self.path,
            meerkat_sqlite::ConnectionProfile::Primary { create },
            meerkat_sqlite::OpenOptions {
                busy_timeout: Some(Duration::ZERO),
                schema_preflight: &[&APPROVAL_DECISIONS_DOMAIN],
            },
        )
        .map_err(open_error)?;
        meerkat_sqlite::apply_domain_migrations(&mut conn, &APPROVAL_DECISIONS_DOMAIN)
            .map_err(open_error)?;
        Ok(DecisionConnection {
            conn,
            _guard: guard,
        })
    }

    /// A connection for a read, or `None` when the file does not exist yet:
    /// a read of a missing file creates nothing, not even a lock file.
    fn read_connection(&self) -> Result<Option<DecisionConnection>, ApprovalStoreError> {
        if !self.path.exists() {
            meerkat_sqlite::OperationGuard::admit_missing_database(&self.path)
                .map_err(open_error)?;
            return Ok(None);
        }
        self.connect(false).map(Some)
    }

    /// Write `row` in one immediate transaction with `sql`, which binds the
    /// row's columns as `?1`..`?11` and `extra` after them. A statement that
    /// changes no row is [`ApprovalStoreError::Conflict`].
    fn write(
        &self,
        create: bool,
        sql: &str,
        row: &StoredDecision,
        extra: Option<i64>,
    ) -> Result<(), ApprovalStoreError> {
        let deadline_at_ms = integer(row.descriptor.deadline_at_ms, "deadline")?;
        let revision = integer(row.revision, "revision")?;
        let row_json = serde_json::to_vec(row)
            .map_err(|error| ApprovalStoreError::NotCommitted(error.to_string()))?;
        let decision_id = row.decision_id.as_str();
        let kind = row.descriptor.kind.as_str();
        let operation_id = row.descriptor.operation_id.as_str();
        let requester_key = row.descriptor.requester.as_str();
        let approver_id = row.descriptor.approver_id.as_str();
        let reviewer_key = row
            .descriptor
            .reviewer
            .as_ref()
            .map(DecisionPrincipalKey::as_str);
        let phase = row.state.phase.as_str();
        let notice = row.state.notice.as_str();
        let columns: [&dyn ToSql; 11] = [
            &decision_id,
            &kind,
            &operation_id,
            &requester_key,
            &approver_id,
            &reviewer_key,
            &phase,
            &notice,
            &deadline_at_ms,
            &revision,
            &row_json,
        ];
        let mut values = columns.to_vec();
        if let Some(extra) = &extra {
            values.push(extra);
        }
        let mut connection = self.connect(create)?;
        let tx = connection
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| sqlite_error(&error))?;
        let changed = tx
            .execute(sql, values.as_slice())
            .map_err(|error| sqlite_error(&error))?;
        if changed == 0 {
            return Err(ApprovalStoreError::Conflict);
        }
        tx.commit().map_err(|error| commit_error(&error))
    }
}

impl DecisionStore for SqliteDecisionStore {
    fn get(&self, decision_id: &DecisionId) -> Result<Option<DecisionRow>, ApprovalStoreError> {
        let Some(connection) = self.read_connection()? else {
            return Ok(None);
        };
        let page = read_page(
            &connection.conn,
            GET_SQL,
            &[Value::Text(decision_id.as_str().to_owned())],
            1,
        )?;
        Ok(page.rows.into_iter().next())
    }

    /// Creates the file when it does not exist: the first hold does.
    fn insert_if_absent(&self, row: &StoredDecision) -> Result<(), ApprovalStoreError> {
        self.write(true, INSERT_SQL, row, None)
    }

    fn replace_if_revision(
        &self,
        expected_revision: u64,
        row: &StoredDecision,
    ) -> Result<(), ApprovalStoreError> {
        if !self.path.exists() {
            // No row to replace, and nothing is created for it.
            meerkat_sqlite::OperationGuard::admit_missing_database(&self.path)
                .map_err(open_error)?;
            return Err(ApprovalStoreError::Conflict);
        }
        let expected_revision = integer(expected_revision, "revision")?;
        self.write(false, REPLACE_SQL, row, Some(expected_revision))
    }

    fn list(
        &self,
        filter: &DecisionListFilter,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError> {
        let Some(connection) = self.read_connection()? else {
            return Ok(DecisionPage::default());
        };
        let (sql, values) = list_query(filter, after, limit);
        read_page(&connection.conn, &sql, &values, limit)
    }

    fn owed(&self, after: Option<&str>, limit: usize) -> Result<DecisionPage, ApprovalStoreError> {
        let Some(connection) = self.read_connection()? else {
            return Ok(DecisionPage::default());
        };
        read_page(
            &connection.conn,
            OWED_SQL,
            &[
                Value::Text(after.unwrap_or_default().to_owned()),
                Value::Integer(fetch_limit(limit)),
            ],
            limit,
        )
    }

    fn due(
        &self,
        now_ms: u64,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError> {
        let Some(connection) = self.read_connection()? else {
            return Ok(DecisionPage::default());
        };
        read_page(
            &connection.conn,
            DUE_SQL,
            &[
                Value::Integer(i64::try_from(now_ms).unwrap_or(i64::MAX)),
                Value::Text(after.unwrap_or_default().to_owned()),
                Value::Integer(fetch_limit(limit)),
            ],
            limit,
        )
    }

    fn count(&self, filter: &DecisionListFilter) -> Result<u64, ApprovalStoreError> {
        let Some(connection) = self.read_connection()? else {
            return Ok(0);
        };
        let (sql, values) = count_query(filter);
        let count: i64 = connection
            .conn
            .query_row(&sql, params_from_iter(values.iter()), |row| row.get(0))
            .map_err(|error| sqlite_error(&error))?;
        Ok(u64::try_from(count).unwrap_or_default())
    }

    fn is_persistent(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests;
