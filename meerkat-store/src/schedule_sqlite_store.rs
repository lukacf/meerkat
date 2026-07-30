use crate::StoreError;
use crate::json_column::JsonColumnBytes;
use crate::sqlite_store::begin_immediate_transaction;
use async_trait::async_trait;
use chrono::{DateTime, LocalResult, TimeZone, Utc};
use meerkat_schedule::{
    AuthorizedOccurrenceWrite, AuthorizedScheduleWrite, ClaimDueRequest, ClaimDueResult,
    DeliveryReceipt, Occurrence, OccurrenceDueAction, OccurrenceFilter, OccurrenceId,
    OccurrenceLifecycleEffect, OccurrenceLifecycleError, OccurrenceLifecycleInput,
    OccurrenceSupersessionAck, PendingSupersession, RenewOccurrenceLeaseOutcome,
    RenewOccurrenceLeaseRequest, RenewOccurrenceLeaseResult, RuntimeDeliveryOutcome, Schedule,
    ScheduleFilter, SchedulePhase, ScheduleRefillBatch, ScheduleRefillCandidate, ScheduleStore,
    ScheduleStoreActionTime, ScheduleStoreError, ScheduleStoreKind, ScheduleStoreRowFault,
    ScheduleStoreRowFaultKind, ScheduleStoreWakeMode, apply_supersession_feedback,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Duration as StdDuration;
use tokio::sync::oneshot;
use uuid::Uuid;

fn migration_0001_schedule_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(CREATE_SCHEDULES_TABLE_SQL)?;
    tx.execute_batch(CREATE_OCCURRENCES_TABLE_SQL)?;
    tx.execute_batch(CREATE_OCCURRENCES_DUE_INDEX_SQL)?;
    tx.execute_batch(CREATE_OCCURRENCES_SCHEDULE_INDEX_SQL)?;
    tx.execute_batch(CREATE_RECEIPTS_TABLE_SQL)?;
    tx.execute_batch(CREATE_RECEIPTS_OCCURRENCE_INDEX_SQL)?;
    Ok(())
}

fn migration_0002_schedule_work_projections(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(ADD_SCHEDULES_NEXT_REFILL_COLUMN_SQL)?;
    tx.execute_batch(ADD_OCCURRENCES_ACTION_AT_COLUMN_SQL)?;
    tx.execute_batch(BACKFILL_SCHEDULES_NEXT_REFILL_SQL)?;
    tx.execute_batch(BACKFILL_OCCURRENCES_ACTION_AT_SQL)?;
    tx.execute_batch(CREATE_SCHEDULES_REFILL_INDEX_SQL)?;
    tx.execute_batch(CREATE_OCCURRENCES_ACTION_INDEX_SQL)?;
    tx.execute_batch(CREATE_OCCURRENCES_REFILL_PENDING_INDEX_SQL)?;
    tx.execute_batch(CREATE_OCCURRENCES_ACTION_COHERENCE_TRIGGERS_SQL)?;
    tx.execute_batch(CREATE_SCHEDULES_REFILL_COHERENCE_TRIGGERS_SQL)?;
    Ok(())
}

fn initialize_current_schedule_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    migration_0001_schedule_schema(tx)?;
    migration_0002_schedule_work_projections(tx)
}

const RELEASED_0_8_10_SCHEDULE_OBJECTS: &[meerkat_sqlite::SchemaObject] = &[
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "schedule_schedules",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "schedule_occurrences",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "schedule_receipts",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "schedule_occurrences_due_idx",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "schedule_occurrences_schedule_idx",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "schedule_receipts_occurrence_idx",
    },
];

fn verify_released_0_8_10_schedule_schema(conn: &Connection) -> Result<(), String> {
    meerkat_sqlite::verify_released_schema_fingerprint(
        conn,
        &SCHEDULE_STORE_DOMAIN,
        RELEASED_0_8_10_SCHEDULE_OBJECTS,
        migration_0001_schedule_schema,
    )
}

/// The schedule store's schema domain in the per-file migration ledger.
/// (Co-tenants the sessions file in the sqlite realm backend; the ledger
/// keys strictly by domain, so co-tenancy is safe.)
pub const SCHEDULE_STORE_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "schedule-store",
    migrations: &[
        meerkat_sqlite::Migration {
            version: 1,
            name: "base-schema",
            apply: migration_0001_schedule_schema,
        },
        meerkat_sqlite::Migration {
            version: 2,
            name: "active-work-projections",
            apply: migration_0002_schedule_work_projections,
        },
    ],
    initialize_current: initialize_current_schedule_schema,
    allowed_existing_versions: &[1, 2],
    released_predecessors: &[meerkat_sqlite::SchemaPredecessor {
        version: 1,
        verify: verify_released_0_8_10_schedule_schema,
    }],
    owned_objects: &[
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "schedule_schedules",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "schedule_occurrences",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "schedule_receipts",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "schedule_occurrences_due_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "schedule_occurrences_schedule_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "schedule_receipts_occurrence_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "schedule_schedules_refill_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "schedule_occurrences_refill_pending_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "schedule_occurrences_action_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Trigger,
            name: "schedule_occurrences_action_insert",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Trigger,
            name: "schedule_occurrences_action_update",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Trigger,
            name: "schedule_occurrences_schedule_phase_update",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Trigger,
            name: "schedule_refill_insert",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Trigger,
            name: "schedule_refill_phase_revision_update",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Trigger,
            name: "schedule_occurrences_refill_departure",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Trigger,
            name: "schedule_occurrences_refill_delete",
        },
    ],
    retired_objects: &[],
};

#[cfg(test)]
mod schema_floor_tests {
    use super::*;

    fn released_v1() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open");
        let tx = conn.transaction().expect("tx");
        migration_0001_schedule_schema(&tx).expect("released schema");
        tx.commit().expect("commit");
        conn.execute_batch(
            "CREATE TABLE meerkat_schema (
                 domain TEXT PRIMARY KEY,
                 version INTEGER NOT NULL
             );
             INSERT INTO meerkat_schema VALUES ('schedule-store', 1);",
        )
        .expect("ledger");
        conn
    }

    #[test]
    fn exact_released_v1_upgrades_to_current() {
        let mut conn = released_v1();
        let report = meerkat_sqlite::apply_domain_migrations(&mut conn, &SCHEDULE_STORE_DOMAIN)
            .expect("upgrade");
        assert_eq!(report.from_version, 1);
        assert_eq!(report.to_version, 2);
    }

    #[test]
    fn released_v1_final_column_index_and_trigger_collisions_are_refused_unmutated() {
        for collision in [
            "ALTER TABLE schedule_occurrences ADD COLUMN action_at_ms INTEGER",
            "CREATE INDEX schedule_occurrences_action_idx
                 ON schedule_occurrences(schedule_id)",
            "CREATE TRIGGER schedule_occurrences_action_insert
                 AFTER INSERT ON schedule_occurrences BEGIN SELECT 1; END",
        ] {
            let mut conn = released_v1();
            conn.execute_batch(collision).expect("collision");
            let err = meerkat_sqlite::apply_domain_migrations(&mut conn, &SCHEDULE_STORE_DOMAIN)
                .expect_err("refuse collision");
            assert!(matches!(
                err,
                meerkat_sqlite::SqliteStoreError::SchemaFingerprintMismatch { version: 1, .. }
            ));
            assert_eq!(
                meerkat_sqlite::domain_version(&conn, SCHEDULE_STORE_DOMAIN.name).expect("ledger"),
                Some(1)
            );
        }
    }
}

const SCHEDULE_BUSY_TIMEOUT: StdDuration = StdDuration::from_secs(5);

/// Create/migrate the schedule database once at store construction.
fn initialize_schedule_database(path: &Path) -> Result<(), StoreError> {
    let _guard = meerkat_sqlite::OperationGuard::for_database(path)?;
    let mut conn = meerkat_sqlite::open_with(
        path,
        meerkat_sqlite::ConnectionProfile::PRIMARY,
        meerkat_sqlite::OpenOptions {
            // The schedule store preflights its own domain (not its
            // co-tenants'): an ineligible schedule-store file is refused
            // before the Primary profile's WAL conversion.
            schema_preflight: &[&SCHEDULE_STORE_DOMAIN],
            busy_timeout: Some(SCHEDULE_BUSY_TIMEOUT),
        },
    )
    .map_err(StoreError::from)?;
    meerkat_sqlite::apply_domain_migrations(&mut conn, &SCHEDULE_STORE_DOMAIN)?;
    Ok(())
}

/// Open one already-certified operation connection without creating a new
/// database. The caller holds the operation fence for this connection's
/// complete lifetime, so an exclusive maintenance fence drains every SQLite
/// handle before rename/archive and no live store can keep writing the old
/// inode afterward.
fn open_schedule_operation_connection(path: &Path) -> Result<Connection, StoreError> {
    meerkat_sqlite::open_with(
        path,
        meerkat_sqlite::ConnectionProfile::Primary { create: false },
        meerkat_sqlite::OpenOptions {
            schema_preflight: &[&SCHEDULE_STORE_DOMAIN],
            busy_timeout: Some(SCHEDULE_BUSY_TIMEOUT),
        },
    )
    .map_err(StoreError::from)
}

const CREATE_SCHEDULES_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS schedule_schedules (
    schedule_id TEXT PRIMARY KEY,
    phase TEXT NOT NULL,
    revision INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    next_occurrence_ordinal INTEGER NOT NULL,
    planning_cursor_at_ms INTEGER NULL,
    schedule_json BLOB NOT NULL
)";

const CREATE_OCCURRENCES_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS schedule_occurrences (
    occurrence_id TEXT PRIMARY KEY,
    schedule_id TEXT NOT NULL,
    phase TEXT NOT NULL,
    schedule_revision INTEGER NOT NULL,
    occurrence_ordinal INTEGER NOT NULL,
    due_at_ms INTEGER NOT NULL,
    lease_expires_at_ms INTEGER NULL,
    occurrence_json BLOB NOT NULL,
    FOREIGN KEY(schedule_id) REFERENCES schedule_schedules(schedule_id)
)";

const CREATE_OCCURRENCES_DUE_INDEX_SQL: &str = r"
CREATE INDEX IF NOT EXISTS schedule_occurrences_due_idx
ON schedule_occurrences(phase, due_at_ms ASC, schedule_revision ASC, occurrence_ordinal ASC)";

const CREATE_OCCURRENCES_SCHEDULE_INDEX_SQL: &str = r"
CREATE INDEX IF NOT EXISTS schedule_occurrences_schedule_idx
ON schedule_occurrences(schedule_id, due_at_ms ASC)";

const ADD_OCCURRENCES_ACTION_AT_COLUMN_SQL: &str =
    "ALTER TABLE schedule_occurrences ADD COLUMN action_at_ms INTEGER NULL";

const ADD_SCHEDULES_NEXT_REFILL_COLUMN_SQL: &str =
    "ALTER TABLE schedule_schedules ADD COLUMN next_refill_at_ms INTEGER NULL";

const BACKFILL_SCHEDULES_NEXT_REFILL_SQL: &str = r"
UPDATE schedule_schedules
SET next_refill_at_ms = CASE
    WHEN phase = 'active'
    THEN CAST((julianday('now') - 2440587.5) * 86400000.0 AS INTEGER)
    ELSE NULL
END";

const CREATE_SCHEDULES_REFILL_INDEX_SQL: &str = r"
CREATE INDEX schedule_schedules_refill_idx
ON schedule_schedules(next_refill_at_ms ASC, schedule_id ASC)
WHERE next_refill_at_ms IS NOT NULL";

const CREATE_OCCURRENCES_REFILL_PENDING_INDEX_SQL: &str = r"
CREATE INDEX schedule_occurrences_refill_pending_idx
ON schedule_occurrences(
    schedule_id ASC,
    schedule_revision ASC,
    due_at_ms ASC,
    occurrence_ordinal ASC,
    occurrence_id ASC
)
WHERE phase = 'pending'";

/// Mechanical active-schedule action projection.
///
/// This is not schedule/occurrence authority: every selected row is still
/// decoded and classified by the generated machines. It is the exact
/// cross-table/index carrier SQLite needs to seek one globally ordered stream
/// without scanning inactive schedules or merging a CASE/OR expression.
const BACKFILL_OCCURRENCES_ACTION_AT_SQL: &str = r"
UPDATE schedule_occurrences
SET action_at_ms = CASE
    WHEN EXISTS (
        SELECT 1
        FROM schedule_schedules s
        WHERE s.schedule_id = schedule_occurrences.schedule_id
          AND s.phase = 'active'
    )
    THEN CASE
        WHEN phase = 'pending' THEN due_at_ms
        WHEN phase IN ('claimed', 'dispatching', 'awaiting_completion')
            THEN lease_expires_at_ms
        ELSE NULL
    END
    ELSE NULL
END";

const CREATE_OCCURRENCES_ACTION_INDEX_SQL: &str = r"
CREATE INDEX schedule_occurrences_action_idx
ON schedule_occurrences(
    action_at_ms ASC,
    due_at_ms ASC,
    schedule_revision ASC,
    occurrence_ordinal ASC,
    occurrence_id ASC,
    schedule_id ASC
)
WHERE action_at_ms IS NOT NULL";

/// Defensive projection coherence for every SQL writer, including operational
/// repair tooling. Normal store writes supply the exact value themselves, so
/// the occurrence triggers' WHEN guards avoid a second row/index mutation.
const CREATE_OCCURRENCES_ACTION_COHERENCE_TRIGGERS_SQL: &str = r"
CREATE TRIGGER schedule_occurrences_action_insert
AFTER INSERT ON schedule_occurrences
WHEN NEW.action_at_ms IS NOT (
    CASE
        WHEN EXISTS (
            SELECT 1 FROM schedule_schedules s
            WHERE s.schedule_id = NEW.schedule_id AND s.phase = 'active'
        )
        THEN CASE
            WHEN NEW.phase = 'pending' THEN NEW.due_at_ms
            WHEN NEW.phase IN ('claimed', 'dispatching', 'awaiting_completion')
                THEN NEW.lease_expires_at_ms
            ELSE NULL
        END
        ELSE NULL
    END
)
BEGIN
    UPDATE schedule_occurrences
    SET action_at_ms = CASE
        WHEN EXISTS (
            SELECT 1 FROM schedule_schedules s
            WHERE s.schedule_id = NEW.schedule_id AND s.phase = 'active'
        )
        THEN CASE
            WHEN NEW.phase = 'pending' THEN NEW.due_at_ms
            WHEN NEW.phase IN ('claimed', 'dispatching', 'awaiting_completion')
                THEN NEW.lease_expires_at_ms
            ELSE NULL
        END
        ELSE NULL
    END
    WHERE occurrence_id = NEW.occurrence_id;
END;

CREATE TRIGGER schedule_occurrences_action_update
AFTER UPDATE OF schedule_id, phase, due_at_ms, lease_expires_at_ms, action_at_ms
ON schedule_occurrences
WHEN NEW.action_at_ms IS NOT (
    CASE
        WHEN EXISTS (
            SELECT 1 FROM schedule_schedules s
            WHERE s.schedule_id = NEW.schedule_id AND s.phase = 'active'
        )
        THEN CASE
            WHEN NEW.phase = 'pending' THEN NEW.due_at_ms
            WHEN NEW.phase IN ('claimed', 'dispatching', 'awaiting_completion')
                THEN NEW.lease_expires_at_ms
            ELSE NULL
        END
        ELSE NULL
    END
)
BEGIN
    UPDATE schedule_occurrences
    SET action_at_ms = CASE
        WHEN EXISTS (
            SELECT 1 FROM schedule_schedules s
            WHERE s.schedule_id = NEW.schedule_id AND s.phase = 'active'
        )
        THEN CASE
            WHEN NEW.phase = 'pending' THEN NEW.due_at_ms
            WHEN NEW.phase IN ('claimed', 'dispatching', 'awaiting_completion')
                THEN NEW.lease_expires_at_ms
            ELSE NULL
        END
        ELSE NULL
    END
    WHERE occurrence_id = NEW.occurrence_id;
END;

CREATE TRIGGER schedule_occurrences_schedule_phase_update
AFTER UPDATE OF phase ON schedule_schedules
WHEN OLD.phase IS NOT NEW.phase
BEGIN
    UPDATE schedule_occurrences
    SET action_at_ms = CASE
        WHEN NEW.phase = 'active' THEN CASE
            WHEN phase = 'pending' THEN due_at_ms
            WHEN phase IN ('claimed', 'dispatching', 'awaiting_completion')
                THEN lease_expires_at_ms
            ELSE NULL
        END
        ELSE NULL
    END
    WHERE schedule_id = NEW.schedule_id;
END";

/// A current-revision Pending row leaving Pending consumes one slot in the
/// planning horizon. Enqueue its active schedule at the store clock in the
/// same transaction, including raw-SQL repair/deletion paths.
const CREATE_SCHEDULES_REFILL_COHERENCE_TRIGGERS_SQL: &str = r"
CREATE TRIGGER schedule_refill_insert
AFTER INSERT ON schedule_schedules
WHEN NEW.phase = 'active' AND NEW.next_refill_at_ms IS NULL
BEGIN
    UPDATE schedule_schedules
    SET next_refill_at_ms =
        CAST((julianday('now') - 2440587.5) * 86400000.0 AS INTEGER)
    WHERE schedule_id = NEW.schedule_id;
END;

CREATE TRIGGER schedule_refill_phase_revision_update
AFTER UPDATE OF phase, revision ON schedule_schedules
WHEN (
    NEW.phase <> 'active' AND NEW.next_refill_at_ms IS NOT NULL
) OR (
    NEW.phase = 'active'
    AND (OLD.phase <> 'active' OR OLD.revision <> NEW.revision)
    AND NEW.next_refill_at_ms IS OLD.next_refill_at_ms
)
BEGIN
    UPDATE schedule_schedules
    SET next_refill_at_ms = CASE
        WHEN NEW.phase = 'active'
        THEN CAST((julianday('now') - 2440587.5) * 86400000.0 AS INTEGER)
        ELSE NULL
    END
    WHERE schedule_id = NEW.schedule_id;
END;

CREATE TRIGGER schedule_occurrences_refill_departure
AFTER UPDATE OF schedule_id, phase, schedule_revision ON schedule_occurrences
WHEN OLD.phase = 'pending'
 AND (
    NEW.phase IS NOT 'pending'
    OR NEW.schedule_id IS NOT OLD.schedule_id
    OR NEW.schedule_revision IS NOT OLD.schedule_revision
 )
BEGIN
    UPDATE schedule_schedules
    SET next_refill_at_ms = MIN(
        COALESCE(
            next_refill_at_ms,
            CAST((julianday('now') - 2440587.5) * 86400000.0 AS INTEGER)
        ),
        CAST((julianday('now') - 2440587.5) * 86400000.0 AS INTEGER)
    )
    WHERE schedule_id = OLD.schedule_id
      AND phase = 'active'
      AND revision = OLD.schedule_revision;
END;

CREATE TRIGGER schedule_occurrences_refill_delete
AFTER DELETE ON schedule_occurrences
WHEN OLD.phase = 'pending'
BEGIN
    UPDATE schedule_schedules
    SET next_refill_at_ms = MIN(
        COALESCE(
            next_refill_at_ms,
            CAST((julianday('now') - 2440587.5) * 86400000.0 AS INTEGER)
        ),
        CAST((julianday('now') - 2440587.5) * 86400000.0 AS INTEGER)
    )
    WHERE schedule_id = OLD.schedule_id
      AND phase = 'active'
      AND revision = OLD.schedule_revision;
END";

const CREATE_RECEIPTS_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS schedule_receipts (
    receipt_id TEXT PRIMARY KEY,
    occurrence_id TEXT NOT NULL,
    recorded_at_ms INTEGER NOT NULL,
    receipt_json BLOB NOT NULL
)";

const CREATE_RECEIPTS_OCCURRENCE_INDEX_SQL: &str = r"
CREATE INDEX IF NOT EXISTS schedule_receipts_occurrence_idx
ON schedule_receipts(occurrence_id, recorded_at_ms ASC)";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClaimScanCursor {
    action_at_ms: i64,
    due_at_ms: i64,
    schedule_revision: i64,
    occurrence_ordinal: i64,
    occurrence_id: String,
}

struct ClaimCandidateRow {
    cursor: ClaimScanCursor,
    schedule_id: String,
    occurrence_json: Vec<u8>,
    schedule_json: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefillScanCursor {
    refill_at_ms: i64,
    schedule_id: String,
}

struct RefillCandidateRow {
    cursor: RefillScanCursor,
    revision: i64,
    schedule_json: Vec<u8>,
}

// `CROSS JOIN` is intentional: SQLite promises not to reorder it, so the
// action index is always the bounded outer loop and schedule activity is one
// primary-key validation per candidate.
const NEXT_OCCURRENCE_ACTION_TIME_SQL: &str = r"
SELECT o.action_at_ms
FROM schedule_occurrences o INDEXED BY schedule_occurrences_action_idx
CROSS JOIN schedule_schedules s
WHERE o.action_at_ms IS NOT NULL
  AND s.schedule_id = o.schedule_id
  AND s.phase = 'active'
ORDER BY o.action_at_ms ASC, o.due_at_ms ASC, o.schedule_revision ASC,
         o.occurrence_ordinal ASC, o.occurrence_id ASC
LIMIT 1";

const NEXT_REFILL_TIME_SQL: &str = r"
SELECT next_refill_at_ms
FROM schedule_schedules INDEXED BY schedule_schedules_refill_idx
WHERE next_refill_at_ms IS NOT NULL
  AND phase = 'active'
ORDER BY next_refill_at_ms ASC, schedule_id ASC
LIMIT 1";

const REFILL_CANDIDATE_PAGE_START_SQL: &str = r"
SELECT schedule_id, revision, schedule_json, next_refill_at_ms
FROM schedule_schedules INDEXED BY schedule_schedules_refill_idx
WHERE next_refill_at_ms IS NOT NULL
  AND next_refill_at_ms <= ?1
  AND phase = 'active'
ORDER BY next_refill_at_ms ASC, schedule_id ASC
LIMIT ?2";

const REFILL_CANDIDATE_PAGE_AFTER_SQL: &str = r"
SELECT schedule_id, revision, schedule_json, next_refill_at_ms
FROM schedule_schedules INDEXED BY schedule_schedules_refill_idx
WHERE next_refill_at_ms IS NOT NULL
  AND next_refill_at_ms <= ?1
  AND phase = 'active'
  AND (next_refill_at_ms, schedule_id) > (?2, ?3)
ORDER BY next_refill_at_ms ASC, schedule_id ASC
LIMIT ?4";

const REFILL_CANDIDATE_PAGE_THROUGH_SQL: &str = r"
SELECT schedule_id, revision, schedule_json, next_refill_at_ms
FROM schedule_schedules INDEXED BY schedule_schedules_refill_idx
WHERE next_refill_at_ms IS NOT NULL
  AND next_refill_at_ms <= ?1
  AND phase = 'active'
  AND (next_refill_at_ms, schedule_id) <= (?2, ?3)
ORDER BY next_refill_at_ms ASC, schedule_id ASC
LIMIT ?4";

const REFILL_PENDING_OCCURRENCES_SQL: &str = r"
SELECT occurrence_id, occurrence_json
FROM schedule_occurrences INDEXED BY schedule_occurrences_refill_pending_idx
WHERE schedule_id = ?1
  AND phase = 'pending'
  AND schedule_revision = ?2
ORDER BY due_at_ms ASC, occurrence_ordinal ASC, occurrence_id ASC";

const CLAIM_CANDIDATE_PAGE_START_SQL: &str = r"
SELECT
  o.occurrence_id,
  o.schedule_id,
  o.occurrence_json,
  s.schedule_json,
  o.action_at_ms,
  o.due_at_ms,
  o.schedule_revision,
  o.occurrence_ordinal
FROM schedule_occurrences o INDEXED BY schedule_occurrences_action_idx
CROSS JOIN schedule_schedules s
WHERE o.action_at_ms IS NOT NULL
  AND o.action_at_ms <= ?1
  AND s.schedule_id = o.schedule_id
  AND s.phase = 'active'
ORDER BY o.action_at_ms ASC, o.due_at_ms ASC, o.schedule_revision ASC,
         o.occurrence_ordinal ASC, o.occurrence_id ASC
LIMIT ?2";

const CLAIM_CANDIDATE_PAGE_AFTER_SQL: &str = r"
SELECT
  o.occurrence_id,
  o.schedule_id,
  o.occurrence_json,
  s.schedule_json,
  o.action_at_ms,
  o.due_at_ms,
  o.schedule_revision,
  o.occurrence_ordinal
FROM schedule_occurrences o INDEXED BY schedule_occurrences_action_idx
CROSS JOIN schedule_schedules s
WHERE o.action_at_ms IS NOT NULL
  AND o.action_at_ms <= ?1
  AND s.schedule_id = o.schedule_id
  AND s.phase = 'active'
  AND (
    o.action_at_ms,
    o.due_at_ms,
    o.schedule_revision,
    o.occurrence_ordinal,
    o.occurrence_id
  ) > (?2, ?3, ?4, ?5, ?6)
ORDER BY o.action_at_ms ASC, o.due_at_ms ASC, o.schedule_revision ASC,
         o.occurrence_ordinal ASC, o.occurrence_id ASC
LIMIT ?7";

const CLAIM_CANDIDATE_PAGE_THROUGH_SQL: &str = r"
SELECT
  o.occurrence_id,
  o.schedule_id,
  o.occurrence_json,
  s.schedule_json,
  o.action_at_ms,
  o.due_at_ms,
  o.schedule_revision,
  o.occurrence_ordinal
FROM schedule_occurrences o INDEXED BY schedule_occurrences_action_idx
CROSS JOIN schedule_schedules s
WHERE o.action_at_ms IS NOT NULL
  AND o.action_at_ms <= ?1
  AND s.schedule_id = o.schedule_id
  AND s.phase = 'active'
  AND (
    o.action_at_ms,
    o.due_at_ms,
    o.schedule_revision,
    o.occurrence_ordinal,
    o.occurrence_id
  ) <= (?2, ?3, ?4, ?5, ?6)
ORDER BY o.action_at_ms ASC, o.due_at_ms ASC, o.schedule_revision ASC,
         o.occurrence_ordinal ASC, o.occurrence_id ASC
LIMIT ?7";

/// Maximum idle time for one guarded schedule connection.
///
/// A host tick performs several store verbs back-to-back, so this retains one
/// connection for the burst instead of reopening SQLite for every verb. The
/// connection and its shared maintenance guard are dropped together after
/// this short quiet period, bounding how long offline maintenance can wait for
/// an otherwise idle store to drain.
const SCHEDULE_CONNECTION_IDLE_TIMEOUT: StdDuration = StdDuration::from_millis(250);

type ScheduleConnectionJob = Box<dyn FnOnce(&mut ScheduleConnectionWorkerState) + Send + 'static>;

enum ScheduleConnectionCommand {
    Run(ScheduleConnectionJob),
}

struct GuardedScheduleConnection {
    // Field order is deliberate: close SQLite before releasing the operation
    // guard, preserving the maintenance fence's "drained means no handle"
    // contract.
    connection: Connection,
    _guard: meerkat_sqlite::OperationGuard,
}

struct ScheduleConnectionWorkerState {
    path: PathBuf,
    guarded: Option<GuardedScheduleConnection>,
    #[cfg(test)]
    connection_open_count: Arc<AtomicU64>,
}

impl ScheduleConnectionWorkerState {
    fn connection(&mut self) -> Result<&mut Connection, StoreError> {
        if self.guarded.is_none() {
            let guard = meerkat_sqlite::OperationGuard::for_database(&self.path)?;
            let connection = open_schedule_operation_connection(&self.path)?;
            self.guarded = Some(GuardedScheduleConnection {
                connection,
                _guard: guard,
            });
            #[cfg(test)]
            self.connection_open_count.fetch_add(1, Ordering::Relaxed);
        }
        self.guarded
            .as_mut()
            .map(|guarded| &mut guarded.connection)
            .ok_or_else(|| {
                StoreError::Internal("schedule SQLite worker lost its connection".to_string())
            })
    }

    fn run<T, F>(&mut self, op: F) -> Result<T, StoreError>
    where
        F: FnOnce(&mut Connection) -> Result<T, StoreError>,
    {
        op(self.connection()?)
    }

    fn close_idle_connection(&mut self) {
        self.guarded = None;
    }
}

struct ScheduleConnectionWorker {
    sender: mpsc::Sender<ScheduleConnectionCommand>,
    #[cfg(test)]
    connection_open_count: Arc<AtomicU64>,
}

impl ScheduleConnectionWorker {
    fn spawn(path: PathBuf) -> Result<Self, StoreError> {
        let (sender, receiver) = mpsc::channel();
        #[cfg(test)]
        let connection_open_count = Arc::new(AtomicU64::new(0));
        #[cfg(test)]
        let worker_connection_open_count = Arc::clone(&connection_open_count);
        std::thread::Builder::new()
            .name("meerkat-schedule-sqlite".to_string())
            .spawn(move || {
                let mut state = ScheduleConnectionWorkerState {
                    path,
                    guarded: None,
                    #[cfg(test)]
                    connection_open_count: worker_connection_open_count,
                };
                loop {
                    let command = if state.guarded.is_some() {
                        match receiver.recv_timeout(SCHEDULE_CONNECTION_IDLE_TIMEOUT) {
                            Ok(command) => command,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                state.close_idle_connection();
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match receiver.recv() {
                            Ok(command) => command,
                            Err(_) => break,
                        }
                    };
                    match command {
                        ScheduleConnectionCommand::Run(job) => job(&mut state),
                    }
                }
            })
            .map_err(StoreError::Io)?;
        Ok(Self {
            sender,
            #[cfg(test)]
            connection_open_count,
        })
    }

    async fn run<T, F>(&self, op: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let (result_sender, result_receiver) = oneshot::channel();
        self.sender
            .send(ScheduleConnectionCommand::Run(Box::new(move |state| {
                let _ = result_sender.send(state.run(op));
            })))
            .map_err(|_| {
                StoreError::Internal("schedule SQLite connection worker stopped".to_string())
            })?;
        result_receiver.await.map_err(|_| {
            StoreError::Internal(
                "schedule SQLite connection worker dropped an operation result".to_string(),
            )
        })?
    }

    #[cfg(test)]
    fn connection_open_count(&self) -> u64 {
        self.connection_open_count.load(Ordering::Relaxed)
    }
}

enum ScheduleRefillDeadlineProjection {
    Derive,
    Exact(Option<DateTime<Utc>>),
}

pub struct SqliteScheduleStore {
    path: PathBuf,
    worker: ScheduleConnectionWorker,
    /// Mechanical keyset cursor for bounded claim-page fairness. It carries
    /// no eligibility truth: every visited row is still classified by the
    /// occurrence machine under the write transaction.
    claim_scan_cursor: Mutex<Option<ClaimScanCursor>>,
    refill_scan_cursor: Mutex<Option<RefillScanCursor>>,
}

impl SqliteScheduleStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        initialize_schedule_database(&path)?;
        let worker = ScheduleConnectionWorker::spawn(path.clone())?;
        Ok(Self {
            path,
            worker,
            claim_scan_cursor: Mutex::new(None),
            refill_scan_cursor: Mutex::new(None),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run one store operation on the bounded SQLite worker.
    ///
    /// The worker serializes `rusqlite::Connection` access off the async
    /// executor, shares one handle across a short host-tick burst, and drops
    /// both the handle and its operation guard after bounded idle time. This
    /// keeps `SqliteScheduleStore` Send + Sync without per-verb connection
    /// churn or an indefinitely held maintenance fence.
    async fn with_conn<T, F>(&self, op: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        self.worker.run(op).await
    }

    async fn commit_schedule_write_impl(
        &self,
        write: AuthorizedScheduleWrite,
    ) -> Result<(), StoreError> {
        self.with_conn(move |conn| {
            let tx = begin_immediate_transaction(conn)?;
            reject_standalone_supersession_write(&write)?;
            verify_authorized_schedule_write_in_txn(&tx, &write)?;
            let schedule = write.into_schedule();
            write_schedule_in_txn(&tx, &schedule)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn commit_schedule_mutation_impl(
        &self,
        schedule: AuthorizedScheduleWrite,
        occurrences: Vec<AuthorizedOccurrenceWrite>,
        refill_deadline: ScheduleRefillDeadlineProjection,
    ) -> Result<Schedule, StoreError> {
        self.with_conn(move |conn| {
            let tx = begin_immediate_transaction(conn)?;
            verify_authorized_schedule_write_in_txn(&tx, &schedule)?;
            for occurrence in &occurrences {
                verify_authorized_occurrence_write_in_txn(&tx, occurrence)?;
            }
            let (schedule, supersession) = schedule.into_parts();
            let mut committed_schedule = schedule;
            write_schedule_in_txn(&tx, &committed_schedule)?;
            for occurrence in occurrences {
                let occurrence = occurrence.into_occurrence();
                write_occurrence_in_txn(&tx, &occurrence)?;
            }
            if let Some(supersession) = supersession {
                let acks = supersede_outstanding_occurrences_in_txn(
                    &tx,
                    &committed_schedule,
                    supersession,
                )?;
                committed_schedule = apply_supersession_feedback(committed_schedule, acks)
                    .map_err(|error| StoreError::Internal(error.to_string()))?;
                write_schedule_in_txn(&tx, &committed_schedule)?;
            }
            if let ScheduleRefillDeadlineProjection::Exact(next_refill_at_utc) = refill_deadline {
                set_schedule_refill_deadline_in_txn(&tx, &committed_schedule, next_refill_at_utc)?;
            }
            tx.commit()?;
            Ok(committed_schedule)
        })
        .await
    }

    async fn get_schedule_impl(
        &self,
        schedule_id: &meerkat_schedule::ScheduleId,
    ) -> Result<Option<Schedule>, StoreError> {
        let schedule_id = schedule_id.to_string();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT schedule_json FROM schedule_schedules WHERE schedule_id = ?1",
                params![schedule_id],
                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
            )
            .optional()?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(StoreError::Serialization))
            .transpose()
        })
        .await
    }

    async fn list_schedules_impl(
        &self,
        filter: ScheduleFilter,
    ) -> Result<Vec<Schedule>, StoreError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT schedule_id, schedule_json FROM schedule_schedules ORDER BY created_at_ms ASC, schedule_id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                ))
            })?;
            let mut schedules = Vec::new();
            for row in rows {
                let (schedule_id, bytes) = row?;
                // Strict listing fails wholesale on a poisoned row (the
                // tolerant `list_schedules_with_row_faults` is the skipping
                // read), but the failure names the row so the operator can
                // find it without bisecting the table.
                let schedule: Schedule = serde_json::from_slice(&bytes).map_err(|error| {
                    StoreError::Internal(format!(
                        "schedule row '{schedule_id}' failed typed recovery: {error}"
                    ))
                })?;
                if !filter.include_deleted
                    && schedule.phase == meerkat_schedule::SchedulePhase::Deleted
                {
                    continue;
                }
                if filter.phase.is_some_and(|phase| schedule.phase != phase) {
                    continue;
                }
                schedules.push(schedule);
                if filter.limit.is_some_and(|limit| schedules.len() >= limit) {
                    break;
                }
            }
            Ok(schedules)
        })
        .await
    }

    async fn list_schedules_with_row_faults_impl(
        &self,
        filter: ScheduleFilter,
    ) -> Result<(Vec<Schedule>, Vec<ScheduleStoreRowFault>), StoreError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT schedule_id, schedule_json FROM schedule_schedules ORDER BY created_at_ms ASC, schedule_id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                ))
            })?;
            let mut schedules = Vec::new();
            let mut row_faults = Vec::new();
            for row in rows {
                let (schedule_id, bytes) = row?;
                // Per-row tolerance: one row that fails typed recovery is
                // surfaced as an attributable fault instead of failing the
                // whole listing — a single poisoned row (e.g. a legacy
                // tombstone) must not take down every schedule.
                let schedule: Schedule = match serde_json::from_slice(&bytes) {
                    Ok(schedule) => schedule,
                    Err(error) => {
                        row_faults.push(ScheduleStoreRowFault {
                            schedule_id: Some(schedule_id),
                            occurrence_id: None,
                            kind: ScheduleStoreRowFaultKind::Deserialization,
                            detail: error.to_string(),
                        });
                        continue;
                    }
                };
                if !filter.include_deleted
                    && schedule.phase == meerkat_schedule::SchedulePhase::Deleted
                {
                    continue;
                }
                if filter.phase.is_some_and(|phase| schedule.phase != phase) {
                    continue;
                }
                schedules.push(schedule);
                if filter.limit.is_some_and(|limit| schedules.len() >= limit) {
                    break;
                }
            }
            Ok((schedules, row_faults))
        })
        .await
    }

    async fn commit_occurrence_write_impl(
        &self,
        write: AuthorizedOccurrenceWrite,
    ) -> Result<(), StoreError> {
        self.with_conn(move |conn| {
            let tx = begin_immediate_transaction(conn)?;
            verify_authorized_occurrence_write_in_txn(&tx, &write)?;
            let occurrence = write.into_occurrence();
            write_occurrence_in_txn(&tx, &occurrence)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn commit_occurrence_writes_impl(
        &self,
        writes: Vec<AuthorizedOccurrenceWrite>,
    ) -> Result<(), StoreError> {
        self.with_conn(move |conn| {
            let tx = begin_immediate_transaction(conn)?;
            for write in &writes {
                verify_authorized_occurrence_write_in_txn(&tx, write)?;
            }
            for write in writes {
                let occurrence = write.into_occurrence();
                write_occurrence_in_txn(&tx, &occurrence)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn get_occurrence_impl(
        &self,
        occurrence_id: &meerkat_schedule::OccurrenceId,
    ) -> Result<Option<Occurrence>, StoreError> {
        let occurrence_id = occurrence_id.to_string();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT occurrence_json FROM schedule_occurrences WHERE occurrence_id = ?1",
                params![occurrence_id],
                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
            )
            .optional()?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(StoreError::Serialization))
            .transpose()
        })
        .await
    }

    async fn list_occurrences_impl(
        &self,
        filter: OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, StoreError> {
        self.with_conn(move |conn| {
            // Ask 19's claim-scan doctrine, applied to the listing (2026-07-29
            // incident: the unbounded `SELECT *` deserialized every row ever
            // written, so accumulated terminal history made every list call
            // O(all rows) — a past deployment's operator remedy was wiping
            // the tables). The indexed columns are write-coherent projections
            // of the canonical JSON, so SQL prefilters the scan; the Rust
            // chain below is retained unchanged as the deciding authority for
            // every row the prefilter admits.
            let (sql, sql_params) = occurrence_list_query(&filter);
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(sql_params), |row| {
                Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes())
            })?;
            let mut occurrences = Vec::new();
            for row in rows {
                let bytes = row?;
                let occurrence: Occurrence =
                    serde_json::from_slice(&bytes).map_err(StoreError::Serialization)?;
                if !filter.include_terminal && occurrence.is_terminal() {
                    continue;
                }
                if filter
                    .schedule_id
                    .as_ref()
                    .is_some_and(|schedule_id| &occurrence.schedule_id != schedule_id)
                {
                    continue;
                }
                if filter.phase.is_some_and(|phase| occurrence.phase != phase) {
                    continue;
                }
                if filter
                    .due_after_utc
                    .is_some_and(|due_after| occurrence.due_at_utc < due_after)
                {
                    continue;
                }
                if filter
                    .due_before_utc
                    .is_some_and(|due_before| occurrence.due_at_utc > due_before)
                {
                    continue;
                }
                occurrences.push(occurrence);
                if filter.limit.is_some_and(|limit| occurrences.len() >= limit) {
                    break;
                }
            }
            Ok(occurrences)
        })
        .await
    }

    async fn append_receipt_impl(&self, receipt: DeliveryReceipt) -> Result<(), StoreError> {
        self.with_conn(move |conn| {
            let tx = begin_immediate_transaction(conn)?;
            let canonical_receipt = record_occurrence_receipt_in_txn(&tx, &receipt)?;
            write_receipt_in_txn(&tx, &canonical_receipt)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn list_receipts_impl(
        &self,
        occurrence_id: &meerkat_schedule::OccurrenceId,
    ) -> Result<Vec<DeliveryReceipt>, StoreError> {
        let occurrence_id = occurrence_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT receipt_json FROM schedule_receipts WHERE occurrence_id = ?1 ORDER BY recorded_at_ms ASC, receipt_id ASC",
            )?;
            let rows = stmt.query_map(params![occurrence_id], |row| {
                Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes())
            })?;
            let mut receipts = Vec::new();
            for row in rows {
                let bytes = row?;
                receipts.push(serde_json::from_slice(&bytes).map_err(StoreError::Serialization)?);
            }
            Ok(receipts)
        })
        .await
    }

    async fn read_due_refill_candidates_impl(
        &self,
        limit: usize,
    ) -> Result<ScheduleRefillBatch, StoreError> {
        let scan_after = self
            .refill_scan_cursor
            .lock()
            .map_err(|_| StoreError::Internal("schedule refill cursor mutex poisoned".to_string()))?
            .clone();
        let advance_cursor = limit > 0;
        let (batch, next_scan_cursor) = self
            .with_conn(move |conn| {
                let tx = conn.transaction()?;
                let store_now_ms = select_store_now_ms(&tx)?;
                let store_now_utc = utc_from_millis(store_now_ms);
                if limit == 0 {
                    tx.commit()?;
                    return Ok((
                        ScheduleRefillBatch {
                            store_now_utc,
                            candidates: Vec::new(),
                            row_faults: Vec::new(),
                        },
                        scan_after,
                    ));
                }
                let scan_limit = i64::try_from(limit).unwrap_or(i64::MAX);
                let mut rows = read_refill_candidate_page_after(
                    &tx,
                    store_now_ms,
                    scan_after.as_ref(),
                    scan_limit,
                )?;
                if let Some(scan_after) = scan_after.as_ref() {
                    let remaining =
                        scan_limit.saturating_sub(i64::try_from(rows.len()).unwrap_or(scan_limit));
                    if remaining > 0 {
                        rows.extend(read_refill_candidate_page_through(
                            &tx,
                            store_now_ms,
                            scan_after,
                            remaining,
                        )?);
                    }
                }
                let next_scan_cursor = rows.last().map(|row| row.cursor.clone());
                let mut candidates = Vec::new();
                let mut row_faults = Vec::new();
                for row in rows {
                    let schedule_id = row.cursor.schedule_id.clone();
                    let schedule: Schedule = match serde_json::from_slice(&row.schedule_json) {
                        Ok(schedule) => schedule,
                        Err(error) => {
                            row_faults.push(ScheduleStoreRowFault {
                                schedule_id: Some(schedule_id),
                                occurrence_id: None,
                                kind: ScheduleStoreRowFaultKind::Deserialization,
                                detail: format!("refill schedule row: {error}"),
                            });
                            continue;
                        }
                    };
                    let projected_revision = i64::try_from(schedule.revision.0).unwrap_or(i64::MAX);
                    if schedule.schedule_id.to_string() != schedule_id
                        || schedule.phase != SchedulePhase::Active
                        || projected_revision != row.revision
                    {
                        row_faults.push(ScheduleStoreRowFault {
                            schedule_id: Some(schedule_id),
                            occurrence_id: None,
                            kind: ScheduleStoreRowFaultKind::Deserialization,
                            detail: "refill schedule SQL projection disagrees with typed row"
                                .to_string(),
                        });
                        continue;
                    }
                    let (pending_occurrences, pending_faults, poisoned) =
                        read_pending_occurrences_for_refill(
                            &tx,
                            &row.cursor.schedule_id,
                            row.revision,
                        )?;
                    row_faults.extend(pending_faults);
                    // An incomplete Pending snapshot could create a duplicate
                    // due time under a fresh occurrence id. Report it and skip
                    // the whole candidate; the keyset cursor still advances.
                    if poisoned {
                        continue;
                    }
                    candidates.push(ScheduleRefillCandidate {
                        schedule,
                        pending_occurrences,
                        refill_at_utc: utc_from_millis(row.cursor.refill_at_ms),
                    });
                }
                tx.commit()?;
                Ok((
                    ScheduleRefillBatch {
                        store_now_utc,
                        candidates,
                        row_faults,
                    },
                    next_scan_cursor,
                ))
            })
            .await?;
        if advance_cursor {
            *self.refill_scan_cursor.lock().map_err(|_| {
                StoreError::Internal("schedule refill cursor mutex poisoned".to_string())
            })? = next_scan_cursor;
        }
        Ok(batch)
    }

    async fn claim_due_occurrences_impl(
        &self,
        request: ClaimDueRequest,
    ) -> Result<ClaimDueResult, StoreError> {
        let scan_after = self
            .claim_scan_cursor
            .lock()
            .map_err(|_| StoreError::Internal("schedule claim cursor mutex poisoned".to_string()))?
            .clone();
        let advance_cursor = request.limit > 0;
        let (result, next_scan_cursor) = self
            .with_conn(move |conn| {
                let tx = begin_immediate_transaction(conn)?;
                let store_now_ms = select_store_now_ms(&tx)?;
                let store_now_utc = utc_from_millis(store_now_ms);

                let limit = request.limit;
                if limit == 0 {
                    tx.commit()?;
                    return Ok((
                        ClaimDueResult {
                            store_now_utc,
                            claimed: Vec::new(),
                            row_faults: Vec::new(),
                        },
                        scan_after,
                    ));
                }
                let scan_limit = claim_scan_limit(limit);
                // Visit one bounded keyset page after the last row observed by
                // this store object. If the page reaches the end, fill the
                // remaining budget from the beginning through the prior cursor.
                // A poisoned row therefore advances the mechanical cursor rather
                // than becoming a permanent front-of-order wall.
                let mut candidates = read_claim_candidate_page_after(
                    &tx,
                    store_now_ms,
                    scan_after.as_ref(),
                    scan_limit,
                )?;
                if let Some(scan_after) = scan_after.as_ref() {
                    let remaining = scan_limit
                        .saturating_sub(i64::try_from(candidates.len()).unwrap_or(scan_limit));
                    if remaining > 0 {
                        candidates.extend(read_claim_candidate_page_through(
                            &tx,
                            store_now_ms,
                            scan_after,
                            remaining,
                        )?);
                    }
                }
                let next_scan_cursor = candidates.last().map(|row| row.cursor.clone());
                let mut occurrences = Vec::new();
                let mut row_faults = Vec::new();
                for candidate in candidates {
                    let occurrence_id = candidate.cursor.occurrence_id;
                    let schedule_id = candidate.schedule_id;
                    // Per-row tolerance: one poisoned row is reported and the
                    // keyset cursor still advances past it.
                    let schedule: Schedule = match serde_json::from_slice(&candidate.schedule_json)
                    {
                        Ok(schedule) => schedule,
                        Err(error) => {
                            row_faults.push(ScheduleStoreRowFault {
                                schedule_id: Some(schedule_id),
                                occurrence_id: Some(occurrence_id),
                                kind: ScheduleStoreRowFaultKind::Deserialization,
                                detail: format!("schedule row: {error}"),
                            });
                            continue;
                        }
                    };
                    if schedule.phase != SchedulePhase::Active {
                        continue;
                    }
                    let occurrence: Occurrence =
                        match serde_json::from_slice(&candidate.occurrence_json) {
                            Ok(occurrence) => occurrence,
                            Err(error) => {
                                row_faults.push(ScheduleStoreRowFault {
                                    schedule_id: Some(schedule_id),
                                    occurrence_id: Some(occurrence_id),
                                    kind: ScheduleStoreRowFaultKind::Deserialization,
                                    detail: format!("occurrence row: {error}"),
                                });
                                continue;
                            }
                        };
                    occurrences.push(occurrence);
                }

                let mut claimed = Vec::new();
                for occurrence in occurrences {
                    // Machine-owned due classification, tolerated per row: a
                    // refusal skips only this occurrence (typed fault) and the
                    // remaining rows still claim.
                    let action = match occurrence.classify_due_action(store_now_utc) {
                        Ok(action) => action,
                        Err(error) => {
                            row_faults.push(claim_row_fault(
                                &occurrence,
                                ScheduleStoreRowFaultKind::DueClassification,
                                format!("due classification: {error}"),
                            ));
                            continue;
                        }
                    };
                    match action {
                        Some(OccurrenceDueAction::MisfireRequired) => {
                            if let Some(fault) =
                                resolve_due_misfire_in_txn(&tx, &occurrence, store_now_utc)?
                            {
                                row_faults.push(fault);
                            }
                        }
                        Some(OccurrenceDueAction::ClaimEligible) => {
                            if claimed.len() >= limit {
                                continue;
                            }
                            let claimed_occurrence =
                                claim_occurrence_for_sqlite(occurrence, &request, store_now_utc)?;
                            write_occurrence_in_txn(&tx, &claimed_occurrence)?;
                            claimed.push(claimed_occurrence);
                        }
                        Some(OccurrenceDueAction::LeaseExpired) => {
                            if claimed.len() >= limit {
                                continue;
                            }
                            let (expired, receipt) = match expire_occurrence_lease_for_sqlite(
                                occurrence.clone(),
                                store_now_utc,
                            ) {
                                Ok(expired) => expired,
                                Err(error) => {
                                    row_faults.push(claim_row_fault(
                                        &occurrence,
                                        ScheduleStoreRowFaultKind::DueClassification,
                                        format!("lease expiry: {error}"),
                                    ));
                                    continue;
                                }
                            };
                            write_receipt_in_txn(&tx, &receipt)?;
                            write_occurrence_in_txn(&tx, &expired)?;
                            // A machine refusal of the follow-up claim is this
                            // row's typed fault, never a silent skip: the expiry
                            // above stays committed and the row re-enters the
                            // scan on the next tick.
                            let claimed_occurrence =
                                match claim_occurrence_for_sqlite(expired, &request, store_now_utc)
                                {
                                    Ok(claimed_occurrence) => claimed_occurrence,
                                    Err(error) => {
                                        row_faults.push(claim_row_fault(
                                            &occurrence,
                                            ScheduleStoreRowFaultKind::DueClassification,
                                            format!("lease-expiry reclaim: {error}"),
                                        ));
                                        continue;
                                    }
                                };
                            write_occurrence_in_txn(&tx, &claimed_occurrence)?;
                            claimed.push(claimed_occurrence);
                        }
                        None => {}
                    }
                }

                tx.commit()?;
                Ok((
                    ClaimDueResult {
                        store_now_utc,
                        claimed,
                        row_faults,
                    },
                    next_scan_cursor,
                ))
            })
            .await?;
        if advance_cursor {
            *self.claim_scan_cursor.lock().map_err(|_| {
                StoreError::Internal("schedule claim cursor mutex poisoned".to_string())
            })? = next_scan_cursor;
        }
        Ok(result)
    }
}

#[async_trait]
impl ScheduleStore for SqliteScheduleStore {
    fn kind(&self) -> ScheduleStoreKind {
        ScheduleStoreKind::Sqlite
    }

    fn wake_mode(&self) -> ScheduleStoreWakeMode {
        ScheduleStoreWakeMode::BoundedPoll {
            max_interval: StdDuration::from_secs(5),
        }
    }

    async fn wait_for_durable_wake(&self) -> Result<(), ScheduleStoreError> {
        Err(ScheduleStoreError::DurableWakeUnsupported {
            backend: self.kind(),
        })
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, ScheduleStoreError> {
        self.with_conn(move |conn| Ok(utc_from_millis(select_store_now_ms(conn)?)))
            .await
            .map_err(into_schedule_store_error)
    }

    async fn next_action_time_utc(&self) -> Result<ScheduleStoreActionTime, ScheduleStoreError> {
        self.with_conn(move |conn| {
            let store_now_utc = utc_from_millis(select_store_now_ms(conn)?);
            let next_occurrence_action_ms = conn
                .query_row(NEXT_OCCURRENCE_ACTION_TIME_SQL, [], |row| {
                    row.get::<_, i64>(0)
                })
                .optional()?;
            let next_refill_ms = conn
                .query_row(NEXT_REFILL_TIME_SQL, [], |row| row.get::<_, i64>(0))
                .optional()?;
            let next_action_ms = match (next_occurrence_action_ms, next_refill_ms) {
                (Some(occurrence), Some(refill)) => Some(occurrence.min(refill)),
                (Some(occurrence), None) => Some(occurrence),
                (None, Some(refill)) => Some(refill),
                (None, None) => None,
            };
            Ok(ScheduleStoreActionTime {
                store_now_utc,
                next_action_at_utc: next_action_ms.map(utc_from_millis),
            })
        })
        .await
        .map_err(into_schedule_store_error)
    }

    async fn read_due_refill_candidates(
        &self,
        limit: usize,
    ) -> Result<ScheduleRefillBatch, ScheduleStoreError> {
        self.read_due_refill_candidates_impl(limit)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn commit_schedule_write(
        &self,
        write: AuthorizedScheduleWrite,
    ) -> Result<(), ScheduleStoreError> {
        self.commit_schedule_write_impl(write)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn get_schedule(
        &self,
        schedule_id: &meerkat_schedule::ScheduleId,
    ) -> Result<Option<Schedule>, ScheduleStoreError> {
        self.get_schedule_impl(schedule_id)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn list_schedules(
        &self,
        filter: ScheduleFilter,
    ) -> Result<Vec<Schedule>, ScheduleStoreError> {
        self.list_schedules_impl(filter)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn list_schedules_with_row_faults(
        &self,
        filter: ScheduleFilter,
    ) -> Result<(Vec<Schedule>, Vec<ScheduleStoreRowFault>), ScheduleStoreError> {
        self.list_schedules_with_row_faults_impl(filter)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn commit_occurrence_write(
        &self,
        write: AuthorizedOccurrenceWrite,
    ) -> Result<(), ScheduleStoreError> {
        self.commit_occurrence_write_impl(write)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn commit_occurrence_writes(
        &self,
        writes: Vec<AuthorizedOccurrenceWrite>,
    ) -> Result<(), ScheduleStoreError> {
        self.commit_occurrence_writes_impl(writes)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn commit_schedule_mutation(
        &self,
        schedule: AuthorizedScheduleWrite,
        occurrences: Vec<AuthorizedOccurrenceWrite>,
    ) -> Result<Schedule, ScheduleStoreError> {
        self.commit_schedule_mutation_impl(
            schedule,
            occurrences,
            ScheduleRefillDeadlineProjection::Derive,
        )
        .await
        .map_err(into_schedule_store_error)
    }

    async fn commit_schedule_refill(
        &self,
        schedule: AuthorizedScheduleWrite,
        occurrences: Vec<AuthorizedOccurrenceWrite>,
        next_refill_at_utc: Option<DateTime<Utc>>,
    ) -> Result<Schedule, ScheduleStoreError> {
        self.commit_schedule_mutation_impl(
            schedule,
            occurrences,
            ScheduleRefillDeadlineProjection::Exact(next_refill_at_utc),
        )
        .await
        .map_err(into_schedule_store_error)
    }

    async fn record_refill_deadline_if_current(
        &self,
        schedule_id: &meerkat_schedule::ScheduleId,
        expected_revision: meerkat_schedule::ScheduleRevision,
        expected_refill_at_utc: DateTime<Utc>,
        next_refill_at_utc: Option<DateTime<Utc>>,
    ) -> Result<(), ScheduleStoreError> {
        let schedule_id_for_write = schedule_id.clone();
        let updated = self
            .with_conn(move |conn| {
                let changed = conn.execute(
                    r"
                    UPDATE schedule_schedules
                    SET next_refill_at_ms = ?1
                    WHERE schedule_id = ?2
                      AND phase = 'active'
                      AND revision = ?3
                      AND next_refill_at_ms = ?4
                    ",
                    params![
                        next_refill_at_utc.map(millis),
                        schedule_id_for_write.to_string(),
                        i64::try_from(expected_revision.0).unwrap_or(i64::MAX),
                        millis(expected_refill_at_utc),
                    ],
                )?;
                Ok(changed == 1)
            })
            .await
            .map_err(into_schedule_store_error)?;
        if !updated {
            return Err(ScheduleStoreError::Concurrency(format!(
                "schedule {schedule_id} refill token changed"
            )));
        }
        Ok(())
    }

    async fn get_occurrence(
        &self,
        occurrence_id: &meerkat_schedule::OccurrenceId,
    ) -> Result<Option<Occurrence>, ScheduleStoreError> {
        self.get_occurrence_impl(occurrence_id)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn list_occurrences(
        &self,
        filter: OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, ScheduleStoreError> {
        self.list_occurrences_impl(filter)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn append_receipt(&self, receipt: DeliveryReceipt) -> Result<(), ScheduleStoreError> {
        self.append_receipt_impl(receipt)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn list_receipts(
        &self,
        occurrence_id: &meerkat_schedule::OccurrenceId,
    ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError> {
        self.list_receipts_impl(occurrence_id)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn claim_due_occurrences(
        &self,
        request: ClaimDueRequest,
    ) -> Result<ClaimDueResult, ScheduleStoreError> {
        self.claim_due_occurrences_impl(request)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn renew_occurrence_lease_if_current(
        &self,
        request: RenewOccurrenceLeaseRequest,
    ) -> Result<RenewOccurrenceLeaseResult, ScheduleStoreError> {
        self.with_conn(move |conn| {
            let tx = begin_immediate_transaction(conn)?;
            let store_now_utc = utc_from_millis(select_store_now_ms(&tx)?);
            let Some(current) = read_occurrence_in_txn(&tx, &request.occurrence_id)? else {
                tx.commit()?;
                return Ok(RenewOccurrenceLeaseResult {
                    store_now_utc,
                    outcome: RenewOccurrenceLeaseOutcome::StaleClaim,
                });
            };
            if current.attempt_count != request.expected_attempt
                || current.claim_token() != Some(request.claim_token)
                || current.claimed_by.as_deref() != Some(request.expected_owner_id.as_str())
            {
                tx.commit()?;
                return Ok(RenewOccurrenceLeaseResult {
                    store_now_utc,
                    outcome: RenewOccurrenceLeaseOutcome::StaleClaim,
                });
            }

            let renewed = current
                .apply(OccurrenceLifecycleInput::RenewLease {
                    claim_token: request.claim_token,
                    lease_expires_at_utc: store_now_utc + request.lease_duration,
                    at_utc: store_now_utc,
                })
                .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?
                .into_occurrence();
            write_occurrence_in_txn(&tx, &renewed)?;
            tx.commit()?;
            Ok(RenewOccurrenceLeaseResult {
                store_now_utc,
                outcome: RenewOccurrenceLeaseOutcome::Renewed(renewed),
            })
        })
        .await
        .map_err(into_schedule_store_error)
    }

    async fn transition_occurrence_if_current(
        &self,
        occurrence_id: &OccurrenceId,
        expected_attempt: u32,
        expected_claim_token: Option<Uuid>,
        transition: OccurrenceLifecycleInput,
    ) -> Result<Option<(Occurrence, Vec<OccurrenceLifecycleEffect>)>, ScheduleStoreError> {
        let occurrence_id = occurrence_id.to_string();
        self.with_conn(move |conn| {
            let tx = begin_immediate_transaction(conn)?;
            let current = tx
                .query_row(
                    "SELECT occurrence_json FROM schedule_occurrences WHERE occurrence_id = ?1",
                    params![occurrence_id],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()?
                .map(|bytes| serde_json::from_slice::<Occurrence>(&bytes))
                .transpose()
                .map_err(StoreError::Serialization)?;
            let Some(current) = current else {
                tx.commit()?;
                return Ok(None);
            };
            if current.attempt_count != expected_attempt
                || current.claim_token() != expected_claim_token
            {
                tx.commit()?;
                return Ok(None);
            }

            let mutator =
                current
                    .apply(transition)
                    .map_err(|error: OccurrenceLifecycleError| {
                        StoreError::Internal(error.to_string())
                    })?;
            let (updated, effects) = mutator.into_parts();
            write_occurrence_in_txn(&tx, &updated)?;
            tx.commit()?;
            Ok(Some((updated, effects)))
        })
        .await
        .map_err(into_schedule_store_error)
    }

    async fn transition_occurrence_with_receipt_if_current(
        &self,
        occurrence_id: &OccurrenceId,
        expected_attempt: u32,
        expected_claim_token: Option<Uuid>,
        transition: OccurrenceLifecycleInput,
        runtime_outcome: Option<RuntimeDeliveryOutcome>,
    ) -> Result<Option<Occurrence>, ScheduleStoreError> {
        let occurrence_id = occurrence_id.clone();
        self.with_conn(move |conn| {
            let tx = begin_immediate_transaction(conn)?;
            let Some(current) = read_occurrence_in_txn(&tx, &occurrence_id)? else {
                tx.commit()?;
                return Ok(None);
            };
            if current.attempt_count != expected_attempt
                || current.claim_token() != expected_claim_token
            {
                tx.commit()?;
                return Ok(None);
            }

            let terminalized = current
                .apply(transition)
                .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?
                .into_occurrence();
            let receipt = terminalized
                .delivery_receipt_from_authority(runtime_outcome)
                .map_err(|error: OccurrenceLifecycleError| {
                    StoreError::Internal(error.to_string())
                })?;
            let updated = terminalized
                .apply(OccurrenceLifecycleInput::RecordReceipt {
                    runtime_outcome: receipt.runtime_outcome.clone(),
                    receipt,
                })
                .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?
                .into_occurrence();
            let canonical_receipt = updated.last_receipt.clone().ok_or_else(|| {
                StoreError::Internal(
                    "generated occurrence authority did not produce a receipt".to_string(),
                )
            })?;
            write_occurrence_in_txn(&tx, &updated)?;
            write_receipt_in_txn(&tx, &canonical_receipt)?;
            tx.commit()?;
            Ok(Some(updated))
        })
        .await
        .map_err(into_schedule_store_error)
    }
}

fn reject_standalone_supersession_write(write: &AuthorizedScheduleWrite) -> Result<(), StoreError> {
    if write.has_pending_supersession() {
        return Err(StoreError::Internal(
            "generated schedule supersession requires atomic schedule mutation".into(),
        ));
    }
    Ok(())
}

fn read_schedule_in_txn(
    tx: &rusqlite::Transaction<'_>,
    schedule_id: &meerkat_schedule::ScheduleId,
) -> Result<Option<Schedule>, StoreError> {
    tx.query_row(
        "SELECT schedule_json FROM schedule_schedules WHERE schedule_id = ?1",
        params![schedule_id.to_string()],
        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
    )
    .optional()?
    .map(|bytes| serde_json::from_slice(&bytes).map_err(StoreError::Serialization))
    .transpose()
}

fn read_occurrence_in_txn(
    tx: &rusqlite::Transaction<'_>,
    occurrence_id: &OccurrenceId,
) -> Result<Option<Occurrence>, StoreError> {
    tx.query_row(
        "SELECT occurrence_json FROM schedule_occurrences WHERE occurrence_id = ?1",
        params![occurrence_id.to_string()],
        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
    )
    .optional()?
    .map(|bytes| serde_json::from_slice(&bytes).map_err(StoreError::Serialization))
    .transpose()
}

fn verify_authorized_schedule_write_in_txn(
    tx: &rusqlite::Transaction<'_>,
    write: &AuthorizedScheduleWrite,
) -> Result<(), StoreError> {
    let current = read_schedule_in_txn(tx, write.schedule_id())?;
    write
        .precondition()
        .check_current(current.as_ref())
        .map_err(StoreError::Internal)
}

fn verify_authorized_occurrence_write_in_txn(
    tx: &rusqlite::Transaction<'_>,
    write: &AuthorizedOccurrenceWrite,
) -> Result<(), StoreError> {
    let current = read_occurrence_in_txn(tx, write.occurrence_id())?;
    write
        .precondition()
        .check_current(current.as_ref())
        .map_err(StoreError::Internal)
}

fn write_schedule_in_txn(
    tx: &rusqlite::Transaction<'_>,
    schedule: &Schedule,
) -> Result<(), StoreError> {
    schedule
        .validate_machine_projection()
        .map_err(StoreError::Internal)?;
    let schedule_json = serde_json::to_vec(schedule)?;
    tx.execute(
        r"
        INSERT INTO schedule_schedules (
            schedule_id,
            phase,
            revision,
            created_at_ms,
            updated_at_ms,
            next_occurrence_ordinal,
            planning_cursor_at_ms,
            next_refill_at_ms,
            schedule_json
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7,
            CASE
                WHEN ?2 = 'active'
                THEN CAST((julianday('now') - 2440587.5) * 86400000.0 AS INTEGER)
                ELSE NULL
            END,
            ?8
        )
        ON CONFLICT(schedule_id) DO UPDATE SET
            phase = excluded.phase,
            revision = excluded.revision,
            created_at_ms = excluded.created_at_ms,
            updated_at_ms = excluded.updated_at_ms,
            next_occurrence_ordinal = excluded.next_occurrence_ordinal,
            planning_cursor_at_ms = excluded.planning_cursor_at_ms,
            next_refill_at_ms = CASE
                WHEN excluded.phase <> 'active' THEN NULL
                WHEN schedule_schedules.phase <> 'active'
                  OR schedule_schedules.revision <> excluded.revision
                    THEN CAST((julianday('now') - 2440587.5) * 86400000.0 AS INTEGER)
                ELSE schedule_schedules.next_refill_at_ms
            END,
            schedule_json = excluded.schedule_json
        ",
        params![
            schedule.schedule_id.to_string(),
            schedule_phase_label(schedule.phase),
            i64::try_from(schedule.revision.0).unwrap_or(i64::MAX),
            millis(schedule.config.created_at_utc),
            millis(schedule.config.updated_at_utc),
            i64::try_from(schedule.next_occurrence_ordinal.0).unwrap_or(i64::MAX),
            schedule.planning_cursor_utc.map(millis),
            schedule_json,
        ],
    )?;
    Ok(())
}

fn set_schedule_refill_deadline_in_txn(
    tx: &rusqlite::Transaction<'_>,
    schedule: &Schedule,
    next_refill_at_utc: Option<DateTime<Utc>>,
) -> Result<(), StoreError> {
    let next_refill_at_ms = if schedule.phase == SchedulePhase::Active {
        next_refill_at_utc.map(millis)
    } else {
        None
    };
    let changed = tx.execute(
        r"
        UPDATE schedule_schedules
        SET next_refill_at_ms = ?1
        WHERE schedule_id = ?2 AND revision = ?3
        ",
        params![
            next_refill_at_ms,
            schedule.schedule_id.to_string(),
            i64::try_from(schedule.revision.0).unwrap_or(i64::MAX),
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::Internal(format!(
            "schedule {} changed while recording refill deadline",
            schedule.schedule_id
        )));
    }
    Ok(())
}

fn write_occurrence_in_txn(
    tx: &rusqlite::Transaction<'_>,
    occurrence: &Occurrence,
) -> Result<(), StoreError> {
    occurrence
        .validate_machine_projection()
        .map_err(StoreError::Internal)?;
    let occurrence_json = serde_json::to_vec(occurrence)?;
    tx.execute(
        r"
        INSERT INTO schedule_occurrences (
            occurrence_id,
            schedule_id,
            phase,
            schedule_revision,
            occurrence_ordinal,
            due_at_ms,
            lease_expires_at_ms,
            action_at_ms,
            occurrence_json
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7,
            CASE
                WHEN (
                    SELECT phase FROM schedule_schedules WHERE schedule_id = ?2
                ) = 'active'
                THEN CASE
                    WHEN ?3 = 'pending' THEN ?6
                    WHEN ?3 IN ('claimed', 'dispatching', 'awaiting_completion') THEN ?7
                    ELSE NULL
                END
                ELSE NULL
            END,
            ?8
        )
        ON CONFLICT(occurrence_id) DO UPDATE SET
            schedule_id = excluded.schedule_id,
            phase = excluded.phase,
            schedule_revision = excluded.schedule_revision,
            occurrence_ordinal = excluded.occurrence_ordinal,
            due_at_ms = excluded.due_at_ms,
            lease_expires_at_ms = excluded.lease_expires_at_ms,
            action_at_ms = excluded.action_at_ms,
            occurrence_json = excluded.occurrence_json
        ",
        params![
            occurrence.occurrence_id.to_string(),
            occurrence.schedule_id.to_string(),
            occurrence_phase_label(occurrence.phase),
            i64::try_from(occurrence.schedule_revision.0).unwrap_or(i64::MAX),
            i64::try_from(occurrence.occurrence_ordinal.0).unwrap_or(i64::MAX),
            millis(occurrence.due_at_utc),
            occurrence.lease_expires_at_utc.map(millis),
            occurrence_json,
        ],
    )?;
    Ok(())
}

fn claim_row_fault(
    occurrence: &Occurrence,
    kind: ScheduleStoreRowFaultKind,
    detail: String,
) -> ScheduleStoreRowFault {
    ScheduleStoreRowFault {
        schedule_id: Some(occurrence.schedule_id.to_string()),
        occurrence_id: Some(occurrence.occurrence_id.to_string()),
        kind,
        detail,
    }
}

/// Bound the amount of JSON/machine work performed while holding the claim
/// writer transaction. The budget deliberately exceeds the requested claim
/// count so a small number of poisoned or misfired rows cannot hide healthy
/// neighbors, but has an absolute ceiling so accumulated degraded state
/// cannot starve lease renewals.
fn claim_scan_limit(claim_limit: usize) -> i64 {
    let budget = claim_limit.saturating_mul(4).clamp(64, 4096);
    i64::try_from(budget).unwrap_or(4096)
}

fn claim_candidate_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClaimCandidateRow> {
    Ok(ClaimCandidateRow {
        cursor: ClaimScanCursor {
            occurrence_id: row.get(0)?,
            action_at_ms: row.get(4)?,
            due_at_ms: row.get(5)?,
            schedule_revision: row.get(6)?,
            occurrence_ordinal: row.get(7)?,
        },
        schedule_id: row.get(1)?,
        occurrence_json: row.get::<_, JsonColumnBytes>(2)?.into_bytes(),
        schedule_json: row.get::<_, JsonColumnBytes>(3)?.into_bytes(),
    })
}

fn collect_claim_candidates<F>(
    rows: rusqlite::MappedRows<'_, F>,
) -> Result<Vec<ClaimCandidateRow>, StoreError>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<ClaimCandidateRow>,
{
    let mut candidates = Vec::new();
    for row in rows {
        candidates.push(row?);
    }
    Ok(candidates)
}

fn refill_candidate_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RefillCandidateRow> {
    Ok(RefillCandidateRow {
        cursor: RefillScanCursor {
            schedule_id: row.get(0)?,
            refill_at_ms: row.get(3)?,
        },
        revision: row.get(1)?,
        schedule_json: row.get::<_, JsonColumnBytes>(2)?.into_bytes(),
    })
}

fn collect_refill_candidate_rows<F>(
    rows: rusqlite::MappedRows<'_, F>,
) -> Result<Vec<RefillCandidateRow>, StoreError>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<RefillCandidateRow>,
{
    let mut candidates = Vec::new();
    for row in rows {
        candidates.push(row?);
    }
    Ok(candidates)
}

fn read_refill_candidate_page_after(
    tx: &Transaction<'_>,
    store_now_ms: i64,
    after: Option<&RefillScanCursor>,
    limit: i64,
) -> Result<Vec<RefillCandidateRow>, StoreError> {
    if let Some(after) = after {
        let mut stmt = tx.prepare(REFILL_CANDIDATE_PAGE_AFTER_SQL)?;
        let rows = stmt.query_map(
            params![
                store_now_ms,
                after.refill_at_ms,
                after.schedule_id.as_str(),
                limit,
            ],
            refill_candidate_from_row,
        )?;
        collect_refill_candidate_rows(rows)
    } else {
        let mut stmt = tx.prepare(REFILL_CANDIDATE_PAGE_START_SQL)?;
        let rows = stmt.query_map(params![store_now_ms, limit], refill_candidate_from_row)?;
        collect_refill_candidate_rows(rows)
    }
}

fn read_refill_candidate_page_through(
    tx: &Transaction<'_>,
    store_now_ms: i64,
    through: &RefillScanCursor,
    limit: i64,
) -> Result<Vec<RefillCandidateRow>, StoreError> {
    let mut stmt = tx.prepare(REFILL_CANDIDATE_PAGE_THROUGH_SQL)?;
    let rows = stmt.query_map(
        params![
            store_now_ms,
            through.refill_at_ms,
            through.schedule_id.as_str(),
            limit,
        ],
        refill_candidate_from_row,
    )?;
    collect_refill_candidate_rows(rows)
}

fn read_pending_occurrences_for_refill(
    tx: &Transaction<'_>,
    schedule_id: &str,
    revision: i64,
) -> Result<(Vec<Occurrence>, Vec<ScheduleStoreRowFault>, bool), StoreError> {
    let mut stmt = tx.prepare(REFILL_PENDING_OCCURRENCES_SQL)?;
    let rows = stmt.query_map(params![schedule_id, revision], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
        ))
    })?;
    let mut occurrences = Vec::new();
    let mut row_faults = Vec::new();
    let mut poisoned = false;
    for row in rows {
        let (occurrence_id, bytes) = row?;
        let occurrence: Occurrence = match serde_json::from_slice(&bytes) {
            Ok(occurrence) => occurrence,
            Err(error) => {
                poisoned = true;
                row_faults.push(ScheduleStoreRowFault {
                    schedule_id: Some(schedule_id.to_string()),
                    occurrence_id: Some(occurrence_id),
                    kind: ScheduleStoreRowFaultKind::Deserialization,
                    detail: format!("refill pending row: {error}"),
                });
                continue;
            }
        };
        let projected_revision = i64::try_from(occurrence.schedule_revision.0).unwrap_or(i64::MAX);
        if occurrence.occurrence_id.to_string() != occurrence_id
            || occurrence.schedule_id.to_string() != schedule_id
            || occurrence.phase != meerkat_schedule::OccurrencePhase::Pending
            || projected_revision != revision
        {
            poisoned = true;
            row_faults.push(ScheduleStoreRowFault {
                schedule_id: Some(schedule_id.to_string()),
                occurrence_id: Some(occurrence_id),
                kind: ScheduleStoreRowFaultKind::Deserialization,
                detail: "refill pending SQL projection disagrees with typed row".to_string(),
            });
            continue;
        }
        occurrences.push(occurrence);
    }
    Ok((occurrences, row_faults, poisoned))
}

fn read_claim_candidate_page_after(
    tx: &Transaction<'_>,
    store_now_ms: i64,
    after: Option<&ClaimScanCursor>,
    limit: i64,
) -> Result<Vec<ClaimCandidateRow>, StoreError> {
    if let Some(after) = after {
        let mut stmt = tx.prepare(CLAIM_CANDIDATE_PAGE_AFTER_SQL)?;
        let rows = stmt.query_map(
            params![
                store_now_ms,
                after.action_at_ms,
                after.due_at_ms,
                after.schedule_revision,
                after.occurrence_ordinal,
                after.occurrence_id.as_str(),
                limit,
            ],
            claim_candidate_from_row,
        )?;
        collect_claim_candidates(rows)
    } else {
        let mut stmt = tx.prepare(CLAIM_CANDIDATE_PAGE_START_SQL)?;
        let rows = stmt.query_map(params![store_now_ms, limit], claim_candidate_from_row)?;
        collect_claim_candidates(rows)
    }
}

fn read_claim_candidate_page_through(
    tx: &Transaction<'_>,
    store_now_ms: i64,
    through: &ClaimScanCursor,
    limit: i64,
) -> Result<Vec<ClaimCandidateRow>, StoreError> {
    let mut stmt = tx.prepare(CLAIM_CANDIDATE_PAGE_THROUGH_SQL)?;
    let rows = stmt.query_map(
        params![
            store_now_ms,
            through.action_at_ms,
            through.due_at_ms,
            through.schedule_revision,
            through.occurrence_ordinal,
            through.occurrence_id.as_str(),
            limit,
        ],
        claim_candidate_from_row,
    )?;
    collect_claim_candidates(rows)
}

/// Realize a machine-classified due misfire for one row. A machine refusal
/// anywhere in the row's own transition chain surfaces as `Ok(Some(fault))`
/// (nothing is written for the row); a store WRITE failure returns `Err` so
/// the whole claim transaction aborts — a half-written misfire (receipt
/// committed, occurrence row not terminalized) must never commit.
fn resolve_due_misfire_in_txn(
    tx: &rusqlite::Transaction<'_>,
    occurrence: &Occurrence,
    store_now_utc: chrono::DateTime<chrono::Utc>,
) -> Result<Option<ScheduleStoreRowFault>, StoreError> {
    let detail = Some(occurrence.due_misfire_detail_at(store_now_utc));
    let mut updated = match occurrence
        .clone()
        .apply(OccurrenceLifecycleInput::ResolveDueMisfire {
            detail,
            at_utc: store_now_utc,
        }) {
        Ok(mutator) => mutator.into_occurrence(),
        Err(error) => {
            return Ok(Some(claim_row_fault(
                occurrence,
                ScheduleStoreRowFaultKind::DueClassification,
                format!("misfire resolution: {error}"),
            )));
        }
    };
    let receipt = match updated.delivery_receipt_from_authority(None) {
        Ok(receipt) => receipt,
        Err(error) => {
            return Ok(Some(claim_row_fault(
                occurrence,
                ScheduleStoreRowFaultKind::DueClassification,
                format!("misfire receipt: {error}"),
            )));
        }
    };
    updated = match updated.apply(OccurrenceLifecycleInput::RecordReceipt {
        runtime_outcome: receipt.runtime_outcome.clone(),
        receipt: receipt.clone(),
    }) {
        Ok(mutator) => mutator.into_occurrence(),
        Err(error) => {
            return Ok(Some(claim_row_fault(
                occurrence,
                ScheduleStoreRowFaultKind::DueClassification,
                format!("misfire receipt record: {error}"),
            )));
        }
    };
    write_receipt_in_txn(tx, &receipt)?;
    write_occurrence_in_txn(tx, &updated)?;
    Ok(None)
}

fn write_receipt_in_txn(
    tx: &rusqlite::Transaction<'_>,
    receipt: &DeliveryReceipt,
) -> Result<(), StoreError> {
    let receipt_json = serde_json::to_vec(receipt)?;
    tx.execute(
        r"
        INSERT INTO schedule_receipts (
            receipt_id,
            occurrence_id,
            recorded_at_ms,
            receipt_json
        ) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(receipt_id) DO UPDATE SET
            occurrence_id = excluded.occurrence_id,
            recorded_at_ms = excluded.recorded_at_ms,
            receipt_json = excluded.receipt_json
        ",
        params![
            receipt.receipt_id.to_string(),
            receipt.occurrence_id.to_string(),
            millis(receipt.recorded_at_utc),
            receipt_json,
        ],
    )?;
    Ok(())
}

fn expire_occurrence_lease_for_sqlite(
    occurrence: Occurrence,
    at_utc: DateTime<Utc>,
) -> Result<(Occurrence, DeliveryReceipt), StoreError> {
    let expired = occurrence
        .apply(OccurrenceLifecycleInput::LeaseExpired { at_utc })
        .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?
        .into_occurrence();
    let receipt = expired
        .delivery_receipt_from_authority(None)
        .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?;
    let expired = expired
        .apply(OccurrenceLifecycleInput::RecordReceipt {
            runtime_outcome: receipt.runtime_outcome.clone(),
            receipt: receipt.clone(),
        })
        .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?
        .into_occurrence();
    Ok((expired, receipt))
}

fn claim_occurrence_for_sqlite(
    occurrence: Occurrence,
    request: &ClaimDueRequest,
    at_utc: DateTime<Utc>,
) -> Result<Occurrence, StoreError> {
    occurrence
        .apply(OccurrenceLifecycleInput::Claim {
            owner_id: request.owner_id.clone(),
            at_utc,
            lease_expires_at_utc: at_utc + request.lease_duration,
            claim_token: Uuid::now_v7(),
        })
        .map(|mutator| mutator.into_occurrence())
        .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))
}

fn supersede_outstanding_occurrences_in_txn(
    tx: &rusqlite::Transaction<'_>,
    schedule: &Schedule,
    supersession: PendingSupersession,
) -> Result<Vec<OccurrenceSupersessionAck>, StoreError> {
    let mut stmt = tx.prepare(
        "SELECT occurrence_json
         FROM schedule_occurrences
         WHERE schedule_id = ?1
         ORDER BY due_at_ms ASC, schedule_revision ASC, occurrence_ordinal ASC",
    )?;
    let rows = stmt.query_map(params![schedule.schedule_id.to_string()], |row| {
        Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes())
    })?;
    let mut acks = Vec::new();
    for row in rows {
        let bytes = row?;
        let occurrence: Occurrence =
            serde_json::from_slice(&bytes).map_err(StoreError::Serialization)?;
        // 0.7.2 D1: supersede every non-terminal row regardless of phase
        // (Pending, Claimed, Dispatching, AwaitingCompletion). The old
        // `phase != Pending → continue` filter was shell policy narrowing
        // machine-declared acceptance; the machine's Supersede transition
        // accepts all non-terminal phases.
        if occurrence.is_terminal()
            || occurrence.schedule_revision >= supersession.superseded_by_revision()
        {
            continue;
        }
        let mutator = occurrence
            .apply(OccurrenceLifecycleInput::Supersede {
                superseded_by_revision: supersession.superseded_by_revision(),
                at_utc: supersession.at_utc(),
            })
            .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?;
        let (updated, _effects, mutator_acks) = mutator.into_parts_with_supersession_feedback();
        // The commit-time sweep is the sole receipt minter for supersession
        // (0.7.2 D1): mint exactly one superseded receipt per swept row.
        let receipt = updated
            .delivery_receipt_from_authority(None)
            .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?;
        write_receipt_in_txn(tx, &receipt)?;
        acks.extend(mutator_acks);
        write_occurrence_in_txn(tx, &updated)?;
    }
    Ok(acks)
}

fn record_occurrence_receipt_in_txn(
    tx: &rusqlite::Transaction<'_>,
    receipt: &DeliveryReceipt,
) -> Result<DeliveryReceipt, StoreError> {
    let occurrence_id = receipt.occurrence_id.to_string();
    let Some(bytes) = tx
        .query_row(
            "SELECT occurrence_json FROM schedule_occurrences WHERE occurrence_id = ?1",
            params![&occurrence_id],
            |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
        )
        .optional()?
    else {
        return Err(StoreError::Internal(format!(
            "occurrence {occurrence_id} not found while recording receipt"
        )));
    };
    let occurrence: Occurrence =
        serde_json::from_slice(&bytes).map_err(StoreError::Serialization)?;
    let occurrence = occurrence
        .apply(OccurrenceLifecycleInput::RecordReceipt {
            runtime_outcome: receipt.runtime_outcome.clone(),
            receipt: receipt.clone(),
        })
        .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?
        .into_occurrence();
    let canonical_receipt = occurrence.last_receipt.clone().ok_or_else(|| {
        StoreError::Internal("generated occurrence authority did not produce a receipt".to_string())
    })?;
    write_occurrence_in_txn(tx, &occurrence)?;
    Ok(canonical_receipt)
}

fn schedule_phase_label(phase: meerkat_schedule::SchedulePhase) -> &'static str {
    match phase {
        meerkat_schedule::SchedulePhase::Active => "active",
        meerkat_schedule::SchedulePhase::Paused => "paused",
        meerkat_schedule::SchedulePhase::Deleted => "deleted",
    }
}

fn occurrence_phase_label(phase: meerkat_schedule::OccurrencePhase) -> &'static str {
    match phase {
        meerkat_schedule::OccurrencePhase::Pending => "pending",
        meerkat_schedule::OccurrencePhase::Claimed => "claimed",
        meerkat_schedule::OccurrencePhase::Dispatching => "dispatching",
        meerkat_schedule::OccurrencePhase::AwaitingCompletion => "awaiting_completion",
        meerkat_schedule::OccurrencePhase::Completed => "completed",
        meerkat_schedule::OccurrencePhase::Skipped => "skipped",
        meerkat_schedule::OccurrencePhase::Misfired => "misfired",
        meerkat_schedule::OccurrencePhase::Superseded => "superseded",
        meerkat_schedule::OccurrencePhase::DeliveryFailed => "delivery_failed",
    }
}

/// Every `OccurrencePhase` variant, in declaration order. A new variant
/// fails the exhaustiveness ratchet in `occurrence_phase_live_for_claim`
/// (and `occurrence_phase_label` above) before it can be forgotten here.
const ALL_OCCURRENCE_PHASES: [meerkat_schedule::OccurrencePhase; 9] = [
    meerkat_schedule::OccurrencePhase::Pending,
    meerkat_schedule::OccurrencePhase::Claimed,
    meerkat_schedule::OccurrencePhase::Dispatching,
    meerkat_schedule::OccurrencePhase::AwaitingCompletion,
    meerkat_schedule::OccurrencePhase::Completed,
    meerkat_schedule::OccurrencePhase::Skipped,
    meerkat_schedule::OccurrencePhase::Misfired,
    meerkat_schedule::OccurrencePhase::Superseded,
    meerkat_schedule::OccurrencePhase::DeliveryFailed,
];

/// Whether a phase can still owe claim-path work (claim, misfire, or lease
/// expiry). The exhaustive match is the compile-time ratchet for the claim
/// scan's SQL prefilter: adding an `OccurrencePhase` variant refuses to
/// compile until the new phase is classified live-or-terminal here.
fn occurrence_phase_live_for_claim(phase: meerkat_schedule::OccurrencePhase) -> bool {
    match phase {
        meerkat_schedule::OccurrencePhase::Pending
        | meerkat_schedule::OccurrencePhase::Claimed
        | meerkat_schedule::OccurrencePhase::Dispatching
        | meerkat_schedule::OccurrencePhase::AwaitingCompletion => true,
        meerkat_schedule::OccurrencePhase::Completed
        | meerkat_schedule::OccurrencePhase::Skipped
        | meerkat_schedule::OccurrencePhase::Misfired
        | meerkat_schedule::OccurrencePhase::Superseded
        | meerkat_schedule::OccurrencePhase::DeliveryFailed => false,
    }
}

/// SQL `IN (...)` list of live-phase labels for the claim scan prefilter,
/// derived from the same label + liveness ratchets the write path uses.
fn live_occurrence_phase_sql_list() -> String {
    let mut out = String::new();
    for phase in ALL_OCCURRENCE_PHASES {
        if occurrence_phase_live_for_claim(phase) {
            if !out.is_empty() {
                out.push_str(", ");
            }
            out.push('\'');
            out.push_str(occurrence_phase_label(phase));
            out.push('\'');
        }
    }
    out
}

/// Build the occurrence-listing query with every filter predicate SQL can
/// express pushed onto the indexed projection columns, in canonical order.
///
/// Pushdown honesty notes (the Rust chain in `list_occurrences_impl` stays
/// the deciding authority for every admitted row):
/// - `schedule_id` and `phase` columns are write-coherent projections of the
///   canonical JSON (`write_occurrence_in_txn` sets them from the same
///   occurrence), so those predicates are exact.
/// - terminal exclusion uses the live-phase label set as a superset
///   prefilter: `Occurrence::is_terminal()` is machine-owned and fails
///   closed, so a live-phase row the machine refuses to classify is still
///   dropped by the retained Rust check.
/// - the due bounds compare truncated milliseconds while the Rust check
///   compares full-precision timestamps, so they are conservative supersets
///   (`millis` floors, and flooring is monotone).
/// - `LIMIT` is pushed only when every active predicate is exact in SQL —
///   a limit over a conservative superset could return fewer rows than the
///   unpushed path even though more matching rows exist.
fn occurrence_list_query(filter: &OccurrenceFilter) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value;
    let mut sql = String::from("SELECT occurrence_json FROM schedule_occurrences");
    let mut sql_params: Vec<Value> = Vec::new();
    let mut conditions: Vec<String> = Vec::new();
    if let Some(schedule_id) = &filter.schedule_id {
        sql_params.push(Value::Text(schedule_id.to_string()));
        conditions.push(format!("schedule_id = ?{}", sql_params.len()));
    }
    if let Some(phase) = filter.phase {
        sql_params.push(Value::Text(occurrence_phase_label(phase).to_string()));
        conditions.push(format!("phase = ?{}", sql_params.len()));
    }
    if !filter.include_terminal {
        conditions.push(format!(
            "phase IN ({live_phases})",
            live_phases = live_occurrence_phase_sql_list()
        ));
    }
    if let Some(due_after) = filter.due_after_utc {
        sql_params.push(Value::Integer(millis(due_after)));
        conditions.push(format!("due_at_ms >= ?{}", sql_params.len()));
    }
    if let Some(due_before) = filter.due_before_utc {
        sql_params.push(Value::Integer(millis(due_before)));
        conditions.push(format!("due_at_ms <= ?{}", sql_params.len()));
    }
    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }
    sql.push_str(" ORDER BY due_at_ms ASC, schedule_revision ASC, occurrence_ordinal ASC");
    let limit_pushdown_is_exact = filter.include_terminal
        && filter.due_after_utc.is_none()
        && filter.due_before_utc.is_none();
    if let Some(limit) = filter.limit
        && limit_pushdown_is_exact
    {
        sql_params.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        sql.push_str(&format!(" LIMIT ?{}", sql_params.len()));
    }
    (sql, sql_params)
}

fn select_store_now_ms(conn: &Connection) -> Result<i64, StoreError> {
    conn.query_row(
        "SELECT CAST((julianday('now') - 2440587.5) * 86400000.0 AS INTEGER)",
        [],
        |row| row.get(0),
    )
    .map_err(StoreError::from)
}

fn millis(value: DateTime<Utc>) -> i64 {
    value.timestamp_millis()
}

fn utc_from_millis(value: i64) -> DateTime<Utc> {
    match Utc.timestamp_millis_opt(value) {
        LocalResult::Single(dt) => dt,
        _ => Utc::now(),
    }
}

fn into_schedule_store_error(error: StoreError) -> ScheduleStoreError {
    match error {
        StoreError::Io(err) => ScheduleStoreError::Io(err.to_string()),
        StoreError::Serialization(err) => ScheduleStoreError::Serialization(err.to_string()),
        // Bounded busy-handler exhaustion is a mechanism-level retry class,
        // distinct from semantic concurrency (stale claim/CAS evidence).
        StoreError::Busy(err) => ScheduleStoreError::Transient(err.to_string()),
        other => ScheduleStoreError::Internal(other.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn explain_query_plan<P: rusqlite::Params>(
        conn: &Connection,
        sql: &str,
        params: P,
    ) -> Vec<String> {
        let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
        let mut stmt = conn.prepare(&explain_sql).expect("prepare query plan");
        stmt.query_map(params, |row| row.get::<_, String>(3))
            .expect("read query plan")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect query plan")
    }

    fn assert_action_index_plan(plan: &[String], require_range_search: bool) {
        let rendered = plan.join("\n");
        assert!(
            plan.iter()
                .any(|detail| detail.contains("schedule_occurrences_action_idx")),
            "action index missing from query plan:\n{rendered}"
        );
        if require_range_search {
            assert!(
                plan.iter().any(|detail| {
                    detail.contains("SEARCH o USING INDEX schedule_occurrences_action_idx")
                }),
                "claim page must be an action-index range search:\n{rendered}"
            );
        }
        assert!(
            !plan.iter().any(|detail| detail.contains("USE TEMP B-TREE")),
            "action query must not sort or group through a temp B-tree:\n{rendered}"
        );
        assert!(
            !plan.iter().any(|detail| detail == "SCAN o"),
            "action query must not scan the occurrence table:\n{rendered}"
        );
        assert!(
            plan.iter().any(|detail| {
                detail.contains("SEARCH s USING INDEX") && detail.contains("schedule_id=?")
            }),
            "each candidate must use one schedule primary-key lookup:\n{rendered}"
        );
        assert!(
            !plan.iter().any(|detail| detail == "SCAN s"),
            "action query must not scan the schedule table:\n{rendered}"
        );
    }

    #[test]
    fn action_queries_pin_one_index_order_without_computed_predicates() {
        const ORDER: &str = "ORDER BY o.action_at_ms ASC, o.due_at_ms ASC, \
             o.schedule_revision ASC,\n         o.occurrence_ordinal ASC, o.occurrence_id ASC";

        for sql in [
            CLAIM_CANDIDATE_PAGE_START_SQL,
            CLAIM_CANDIDATE_PAGE_AFTER_SQL,
            CLAIM_CANDIDATE_PAGE_THROUGH_SQL,
        ] {
            assert!(sql.contains("INDEXED BY schedule_occurrences_action_idx"));
            assert!(sql.contains("CROSS JOIN schedule_schedules s"));
            assert!(sql.contains(ORDER), "{sql}");
            assert!(!sql.contains("CASE"), "{sql}");
            assert!(!sql.contains(" OR "), "{sql}");
        }
        assert!(
            NEXT_OCCURRENCE_ACTION_TIME_SQL.contains("INDEXED BY schedule_occurrences_action_idx")
        );
        assert!(NEXT_OCCURRENCE_ACTION_TIME_SQL.contains("CROSS JOIN schedule_schedules s"));
        assert!(NEXT_OCCURRENCE_ACTION_TIME_SQL.contains(ORDER));
        assert!(NEXT_OCCURRENCE_ACTION_TIME_SQL.contains("LIMIT 1"));
        assert!(!NEXT_OCCURRENCE_ACTION_TIME_SQL.contains("MIN("));
        assert!(!NEXT_OCCURRENCE_ACTION_TIME_SQL.contains("CASE"));
    }

    #[test]
    fn action_queries_have_bounded_index_plans_without_temp_sorting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("schedule.sqlite3");
        let _store = SqliteScheduleStore::open(&path).expect("open store");
        let conn = open_schedule_operation_connection(&path).expect("open operation connection");

        assert_action_index_plan(
            &explain_query_plan(
                &conn,
                CLAIM_CANDIDATE_PAGE_START_SQL,
                params![1_000_i64, 64_i64],
            ),
            true,
        );
        assert_action_index_plan(
            &explain_query_plan(
                &conn,
                CLAIM_CANDIDATE_PAGE_AFTER_SQL,
                params![1_000_i64, 0_i64, 0_i64, 0_i64, 0_i64, "", 64_i64],
            ),
            true,
        );
        assert_action_index_plan(
            &explain_query_plan(
                &conn,
                CLAIM_CANDIDATE_PAGE_THROUGH_SQL,
                params![
                    1_000_i64,
                    1_000_i64,
                    1_000_i64,
                    1_000_i64,
                    1_000_i64,
                    "occurrence-z",
                    64_i64
                ],
            ),
            true,
        );
        // With no upper bound, next_action starts at the head of the same
        // ordered index and LIMIT 1 stops on the first coherent active row.
        assert_action_index_plan(
            &explain_query_plan(&conn, NEXT_OCCURRENCE_ACTION_TIME_SQL, []),
            false,
        );
    }

    #[test]
    fn refill_queries_have_bounded_index_plans_without_temp_sorting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("schedule.sqlite3");
        let _store = SqliteScheduleStore::open(&path).expect("open store");
        let conn = open_schedule_operation_connection(&path).expect("open operation connection");

        for (sql, plan) in [
            (
                REFILL_CANDIDATE_PAGE_START_SQL,
                explain_query_plan(
                    &conn,
                    REFILL_CANDIDATE_PAGE_START_SQL,
                    params![1_000_i64, 32_i64],
                ),
            ),
            (
                REFILL_CANDIDATE_PAGE_AFTER_SQL,
                explain_query_plan(
                    &conn,
                    REFILL_CANDIDATE_PAGE_AFTER_SQL,
                    params![1_000_i64, 0_i64, "", 32_i64],
                ),
            ),
            (
                REFILL_CANDIDATE_PAGE_THROUGH_SQL,
                explain_query_plan(
                    &conn,
                    REFILL_CANDIDATE_PAGE_THROUGH_SQL,
                    params![1_000_i64, 1_000_i64, "schedule-z", 32_i64],
                ),
            ),
        ] {
            let rendered = plan.join("\n");
            assert!(sql.contains("INDEXED BY schedule_schedules_refill_idx"));
            assert!(!sql.contains("CASE"));
            assert!(!sql.contains(" OR "));
            assert!(
                plan.iter().any(|detail| {
                    detail.contains(
                        "SEARCH schedule_schedules USING INDEX schedule_schedules_refill_idx",
                    )
                }),
                "refill page must be an indexed due-time range:\n{rendered}"
            );
            assert!(
                !plan.iter().any(|detail| detail.contains("USE TEMP B-TREE")),
                "refill page must not use a temp sort:\n{rendered}"
            );
        }

        let pending_plan = explain_query_plan(
            &conn,
            REFILL_PENDING_OCCURRENCES_SQL,
            params!["schedule-a", 1_i64],
        );
        let rendered = pending_plan.join("\n");
        assert!(
            pending_plan.iter().any(|detail| {
                detail.contains(
                    "SEARCH schedule_occurrences USING INDEX \
                     schedule_occurrences_refill_pending_idx",
                )
            }),
            "pending snapshot must not scan schedule history:\n{rendered}"
        );
        assert!(
            !pending_plan
                .iter()
                .any(|detail| detail.contains("USE TEMP B-TREE")),
            "pending snapshot must not use a temp sort:\n{rendered}"
        );

        let next_plan = explain_query_plan(&conn, NEXT_REFILL_TIME_SQL, []);
        let rendered = next_plan.join("\n");
        assert!(
            next_plan
                .iter()
                .any(|detail| detail.contains("schedule_schedules_refill_idx")),
            "next refill must read the refill index head:\n{rendered}"
        );
        assert!(
            !next_plan
                .iter()
                .any(|detail| detail.contains("USE TEMP B-TREE")),
            "next refill must not use a temp sort:\n{rendered}"
        );
    }

    #[test]
    fn released_v1_rows_backfill_directly_into_final_v2_work_indexes() {
        let mut conn = Connection::open_in_memory().expect("open in-memory database");
        {
            let tx = conn.transaction().expect("begin v1 schema transaction");
            migration_0001_schedule_schema(&tx).expect("apply base schema");
            tx.execute(
                r"
                INSERT INTO schedule_schedules (
                    schedule_id, phase, revision, created_at_ms, updated_at_ms,
                    next_occurrence_ordinal, planning_cursor_at_ms, schedule_json
                ) VALUES ('schedule-a', 'active', 1, 0, 0, 1, NULL, ?1)
                ",
                params![b"{}".as_slice()],
            )
            .expect("insert v1 schedule");
            tx.execute(
                r"
                INSERT INTO schedule_occurrences (
                    occurrence_id, schedule_id, phase, schedule_revision,
                    occurrence_ordinal, due_at_ms, lease_expires_at_ms,
                    occurrence_json
                ) VALUES
                    ('pending-a', 'schedule-a', 'pending', 1, 0, 100, NULL, ?1),
                    ('claimed-a', 'schedule-a', 'claimed', 1, 1, 50, 200, ?1),
                    ('completed-a', 'schedule-a', 'completed', 1, 2, 25, NULL, ?1)
                ",
                params![b"{}".as_slice()],
            )
            .expect("insert v1 occurrences");
            tx.commit().expect("commit v1 fixture");
        }

        {
            let tx = conn.transaction().expect("begin final v2 migration");
            migration_0002_schedule_work_projections(&tx)
                .expect("apply final work-projection migration");
            tx.commit().expect("commit final v2 migration");
        }

        let projected = {
            let mut stmt = conn
                .prepare(
                    r"
                    SELECT occurrence_id, action_at_ms
                    FROM schedule_occurrences
                    ORDER BY occurrence_id
                    ",
                )
                .expect("prepare projections");
            stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .expect("query projections")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect projections")
        };
        assert_eq!(
            projected,
            vec![
                ("claimed-a".to_string(), Some(200)),
                ("completed-a".to_string(), None),
                ("pending-a".to_string(), Some(100)),
            ]
        );

        let next_refill_at_ms: Option<i64> = conn
            .query_row(
                "SELECT next_refill_at_ms FROM schedule_schedules WHERE schedule_id = 'schedule-a'",
                [],
                |row| row.get(0),
            )
            .expect("read refill projection");
        assert!(next_refill_at_ms.is_some());
    }

    #[test]
    fn action_projection_triggers_cover_raw_sql_repairs_and_schedule_pauses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("schedule.sqlite3");
        let _store = SqliteScheduleStore::open(&path).expect("open store");
        let conn = open_schedule_operation_connection(&path).expect("open operation connection");

        conn.execute(
            r"
            INSERT INTO schedule_schedules (
                schedule_id, phase, revision, created_at_ms, updated_at_ms,
                next_occurrence_ordinal, planning_cursor_at_ms, schedule_json
            ) VALUES ('schedule-a', 'active', 1, 0, 0, 1, NULL, ?1)
            ",
            params![b"{}".as_slice()],
        )
        .expect("insert schedule");
        conn.execute(
            r"
            INSERT INTO schedule_occurrences (
                occurrence_id, schedule_id, phase, schedule_revision,
                occurrence_ordinal, due_at_ms, lease_expires_at_ms,
                occurrence_json
            ) VALUES ('occurrence-a', 'schedule-a', 'pending', 1, 0, 100, NULL, ?1)
            ",
            params![b"{}".as_slice()],
        )
        .expect("insert pending occurrence");

        let action_at = || {
            conn.query_row(
                "SELECT action_at_ms FROM schedule_occurrences WHERE occurrence_id = 'occurrence-a'",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .expect("read action projection")
        };
        let next_refill_at = || {
            conn.query_row(
                "SELECT next_refill_at_ms FROM schedule_schedules WHERE schedule_id = 'schedule-a'",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .expect("read refill projection")
        };
        assert_eq!(action_at(), Some(100));
        assert!(next_refill_at().is_some());

        conn.execute(
            r"
            UPDATE schedule_schedules
            SET next_refill_at_ms = 9223372036854775807
            WHERE schedule_id = 'schedule-a'
            ",
            [],
        )
        .expect("move refill deadline into the future");
        conn.execute(
            r"
            UPDATE schedule_occurrences
            SET phase = 'claimed', lease_expires_at_ms = 200
            WHERE occurrence_id = 'occurrence-a'
            ",
            [],
        )
        .expect("claim occurrence");
        assert_eq!(action_at(), Some(200));
        assert!(
            next_refill_at().is_some_and(|deadline| deadline < i64::MAX),
            "Pending departure must enqueue an immediate bounded refill"
        );

        conn.execute(
            "UPDATE schedule_schedules SET phase = 'paused' WHERE schedule_id = 'schedule-a'",
            [],
        )
        .expect("pause schedule");
        assert_eq!(action_at(), None);
        assert_eq!(next_refill_at(), None);

        conn.execute(
            "UPDATE schedule_schedules SET phase = 'active' WHERE schedule_id = 'schedule-a'",
            [],
        )
        .expect("resume schedule");
        assert_eq!(action_at(), Some(200));
        assert!(next_refill_at().is_some());

        conn.execute(
            r"
            UPDATE schedule_occurrences
            SET phase = 'completed'
            WHERE occurrence_id = 'occurrence-a'
            ",
            [],
        )
        .expect("complete occurrence");
        assert_eq!(action_at(), None);
    }

    fn sqlite_failure(code: rusqlite::ErrorCode) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code,
                extended_code: 0,
            },
            None,
        )
    }

    #[test]
    fn classified_busy_maps_to_the_typed_transient_class() {
        for code in [
            rusqlite::ErrorCode::DatabaseBusy,
            rusqlite::ErrorCode::DatabaseLocked,
        ] {
            let err = into_schedule_store_error(StoreError::from(sqlite_failure(code)));
            assert!(
                matches!(err, ScheduleStoreError::Transient(_)),
                "unexpected error: {err:?}"
            );
        }
    }

    #[test]
    fn classified_corruption_stays_terminal() {
        let err = into_schedule_store_error(StoreError::from(sqlite_failure(
            rusqlite::ErrorCode::DatabaseCorrupt,
        )));
        assert!(
            matches!(err, ScheduleStoreError::Internal(_)),
            "unexpected error: {err:?}"
        );
    }

    fn sample_schedule_mutator() -> meerkat_schedule::ScheduleLifecycleMutator {
        Schedule::apply(
            None,
            meerkat_schedule::ScheduleLifecycleInput::Create(
                meerkat_schedule::CreateScheduleRequest {
                    name: Some("held-connection".to_string()),
                    description: None,
                    trigger: meerkat_schedule::TriggerSpec::Interval(
                        meerkat_schedule::IntervalTriggerSpec {
                            start_at_utc: Utc::now(),
                            every_seconds: 60,
                            end_at_utc: None,
                        },
                    ),
                    target: meerkat_schedule::TargetBinding::session(
                        meerkat_schedule::SessionTargetBinding::ExactSession {
                            session_id: meerkat_core::SessionId::new(),
                            action: meerkat_schedule::ScheduledSessionAction::Prompt {
                                prompt: meerkat_core::ContentInput::from("scheduled prompt"),
                                system_prompt: None,
                                render_metadata: None,
                                skill_refs: Vec::new(),
                                additional_instructions: Vec::new(),
                            },
                        },
                    ),
                    misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
                    overlap_policy: meerkat_schedule::OverlapPolicy::SkipIfRunning,
                    missing_target_policy: meerkat_schedule::MissingTargetPolicy::MarkMisfired,
                    labels: std::collections::BTreeMap::new(),
                    planning_horizon_days: Some(1),
                    planning_horizon_occurrences: Some(1),
                },
            ),
        )
        .expect("sample schedule creation should pass generated authority")
    }

    #[tokio::test]
    async fn one_host_tick_burst_reuses_one_bounded_worker_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("schedule.sqlite3");
        let store = SqliteScheduleStore::open(&path).expect("open store");
        assert_eq!(store.worker.connection_open_count(), 0);

        let (refill, claims, next_action) = tokio::join!(
            store.read_due_refill_candidates(16),
            store.claim_due_occurrences(ClaimDueRequest {
                owner_id: "connection-reuse-test".to_string(),
                limit: 16,
                lease_duration: chrono::Duration::seconds(30),
            }),
            store.next_action_time_utc(),
        );
        refill.expect("read refill candidates");
        claims.expect("claim due occurrences");
        next_action.expect("read next action");

        assert_eq!(
            store.worker.connection_open_count(),
            1,
            "the three ordinary host-tick verbs must share one SQLite open"
        );
        assert!(
            SCHEDULE_CONNECTION_IDLE_TIMEOUT < StdDuration::from_secs(1),
            "idle connection retention must stay below the maintenance drain deadline"
        );
    }

    /// A live store may retain one guarded connection for a short operation
    /// burst. The worker must close it within the bounded idle timeout so an
    /// exclusive fence can drain, rename/archive the database, and prevent the
    /// old store from following the renamed inode through a cached handle.
    #[tokio::test]
    async fn maintenance_fence_drains_all_schedule_connections_before_archive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("schedule.sqlite3");
        let store = SqliteScheduleStore::open(&path).expect("open store");

        let mutator = sample_schedule_mutator();
        let schedule = mutator.schedule.clone();
        store
            .commit_schedule_write(mutator.into_authorized_write())
            .await
            .expect("commit schedule");
        store
            .get_schedule(&schedule.schedule_id)
            .await
            .expect("get schedule");

        let fence =
            meerkat_sqlite::ExclusiveFence::acquire(&path, std::time::Duration::from_secs(1))
                .expect("all operation connections must have drained");
        let archived = dir.path().join("schedule.archived.sqlite3");
        std::fs::rename(&path, &archived).expect("archive rename");
        drop(fence);

        let error = store
            .get_schedule(&schedule.schedule_id)
            .await
            .expect_err("old store path must not follow the archived inode");
        assert!(
            matches!(error, ScheduleStoreError::Internal(_)),
            "unexpected old-path error: {error:?}"
        );
        assert!(
            !path.exists(),
            "no-create operation open must not recreate the archived path"
        );

        let archived_store = SqliteScheduleStore::open(&archived).expect("open archived store");
        assert!(
            archived_store
                .get_schedule(&schedule.schedule_id)
                .await
                .expect("read archived schedule")
                .is_some()
        );
    }

    /// The SQL predicates `occurrence_list_query` builds must mirror the
    /// filter exactly, and LIMIT must only be pushed when every active
    /// predicate is exact in SQL (a limit over a conservative superset could
    /// under-return).
    #[test]
    fn occurrence_list_query_pushes_only_exact_limits() {
        let unbounded = occurrence_list_query(&OccurrenceFilter {
            include_terminal: true,
            limit: Some(5),
            ..OccurrenceFilter::default()
        });
        assert!(unbounded.0.contains("LIMIT"), "{}", unbounded.0);

        // Terminal exclusion is a fail-closed superset (machine-owned
        // `is_terminal`), so a limit must not be pushed under it.
        let live_only = occurrence_list_query(&OccurrenceFilter {
            limit: Some(5),
            ..OccurrenceFilter::default()
        });
        assert!(!live_only.0.contains("LIMIT"), "{}", live_only.0);
        assert!(live_only.0.contains("phase IN ("), "{}", live_only.0);

        // Due bounds compare truncated milliseconds (conservative), so a
        // limit must not be pushed alongside them either.
        let due_bounded = occurrence_list_query(&OccurrenceFilter {
            include_terminal: true,
            due_after_utc: Some(Utc::now()),
            limit: Some(5),
            ..OccurrenceFilter::default()
        });
        assert!(!due_bounded.0.contains("LIMIT"), "{}", due_bounded.0);
        assert!(
            due_bounded.0.contains("due_at_ms >= ?"),
            "{}",
            due_bounded.0
        );
    }
}
