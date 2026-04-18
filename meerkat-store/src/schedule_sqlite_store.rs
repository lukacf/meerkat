use crate::StoreError;
use crate::sqlite_store::{begin_immediate_transaction, open_connection};
use async_trait::async_trait;
use chrono::{DateTime, LocalResult, TimeZone, Utc};
use meerkat_schedule::{
    ClaimDueRequest, ClaimDueResult, DeliveryReceipt, DeliveryReceiptStage, Occurrence,
    OccurrenceFailureClass, OccurrenceFilter, OccurrenceId, OccurrenceLifecycleError,
    OccurrenceLifecycleInput, PendingSupersession, Schedule, ScheduleFilter, ScheduleStore,
    ScheduleStoreError, ScheduleStoreKind,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use uuid::Uuid;

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

pub struct SqliteScheduleStore {
    path: PathBuf,
}

impl SqliteScheduleStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let conn = open_connection(&path)?;
        ensure_schedule_schema(&conn)?;
        drop(conn);
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    async fn put_schedule_impl(&self, schedule: Schedule) -> Result<(), StoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
            write_schedule_in_txn(&tx, &schedule)?;
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn get_schedule_impl(
        &self,
        schedule_id: &meerkat_schedule::ScheduleId,
    ) -> Result<Option<Schedule>, StoreError> {
        let path = self.path.clone();
        let schedule_id = schedule_id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            conn.query_row(
                "SELECT schedule_json FROM schedule_schedules WHERE schedule_id = ?1",
                params![schedule_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(StoreError::Serialization))
            .transpose()
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn list_schedules_impl(
        &self,
        filter: ScheduleFilter,
    ) -> Result<Vec<Schedule>, StoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let mut stmt = conn.prepare(
                "SELECT schedule_json FROM schedule_schedules ORDER BY created_at_ms ASC, schedule_id ASC",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
            let mut schedules = Vec::new();
            for row in rows {
                let bytes = row?;
                let schedule: Schedule =
                    serde_json::from_slice(&bytes).map_err(StoreError::Serialization)?;
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
        .map_err(StoreError::Join)?
    }

    async fn put_occurrence_impl(&self, occurrence: Occurrence) -> Result<(), StoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
            write_occurrence_in_txn(&tx, &occurrence)?;
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn put_occurrences_impl(&self, occurrences: Vec<Occurrence>) -> Result<(), StoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
            for occurrence in &occurrences {
                write_occurrence_in_txn(&tx, occurrence)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn get_occurrence_impl(
        &self,
        occurrence_id: &meerkat_schedule::OccurrenceId,
    ) -> Result<Option<Occurrence>, StoreError> {
        let path = self.path.clone();
        let occurrence_id = occurrence_id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            conn.query_row(
                "SELECT occurrence_json FROM schedule_occurrences WHERE occurrence_id = ?1",
                params![occurrence_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(StoreError::Serialization))
            .transpose()
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn list_occurrences_impl(
        &self,
        filter: OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, StoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let mut stmt = conn.prepare(
                "SELECT occurrence_json FROM schedule_occurrences ORDER BY due_at_ms ASC, schedule_revision ASC, occurrence_ordinal ASC",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
            let mut occurrences = Vec::new();
            for row in rows {
                let bytes = row?;
                let occurrence: Occurrence =
                    serde_json::from_slice(&bytes).map_err(StoreError::Serialization)?;
                if !filter.include_terminal && occurrence.phase.is_terminal() {
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
        .map_err(StoreError::Join)?
    }

    async fn append_receipt_impl(&self, receipt: DeliveryReceipt) -> Result<(), StoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
            write_receipt_in_txn(&tx, &receipt)?;
            update_occurrence_last_receipt_in_txn(&tx, &receipt)?;
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn list_receipts_impl(
        &self,
        occurrence_id: &meerkat_schedule::OccurrenceId,
    ) -> Result<Vec<DeliveryReceipt>, StoreError> {
        let path = self.path.clone();
        let occurrence_id = occurrence_id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let mut stmt = conn.prepare(
                "SELECT receipt_json FROM schedule_receipts WHERE occurrence_id = ?1 ORDER BY recorded_at_ms ASC, receipt_id ASC",
            )?;
            let rows = stmt.query_map(params![occurrence_id], |row| row.get::<_, Vec<u8>>(0))?;
            let mut receipts = Vec::new();
            for row in rows {
                let bytes = row?;
                receipts.push(serde_json::from_slice(&bytes).map_err(StoreError::Serialization)?);
            }
            Ok(receipts)
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn claim_due_occurrences_impl(
        &self,
        request: ClaimDueRequest,
    ) -> Result<ClaimDueResult, StoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
            let store_now_ms = select_store_now_ms(&tx)?;
            let store_now_utc = utc_from_millis(store_now_ms);

            let limit = i64::try_from(request.limit).unwrap_or(i64::MAX);
            let mut candidates = Vec::new();
            {
                let mut stmt = tx.prepare(
                    r"
                    SELECT o.occurrence_json
                    FROM schedule_occurrences o
                    JOIN schedule_schedules s ON s.schedule_id = o.schedule_id
                    WHERE s.phase = 'active'
                    ORDER BY o.due_at_ms ASC, o.schedule_revision ASC, o.occurrence_ordinal ASC
                    ",
                )?;
                let rows = stmt.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
                for row in rows {
                    let bytes = row?;
                    let occurrence: Occurrence =
                        serde_json::from_slice(&bytes).map_err(StoreError::Serialization)?;
                    if occurrence.is_claimable_at(store_now_utc) {
                        candidates.push(occurrence);
                    }
                }
            }
            candidates.truncate(usize::try_from(limit).unwrap_or(usize::MAX));

            let mut claimed = Vec::new();
            for occurrence in candidates {
                let occurrence = if occurrence.is_reclaimable_at(store_now_utc) {
                    let expired = occurrence
                        .apply(OccurrenceLifecycleInput::LeaseExpired {
                            at_utc: store_now_utc,
                        })
                        .map_err(|error: OccurrenceLifecycleError| {
                            StoreError::Internal(error.to_string())
                        })?
                        .into_occurrence();
                    let mut receipt = DeliveryReceipt::new(
                        expired.occurrence_id.clone(),
                        expired.attempt_count,
                        DeliveryReceiptStage::LeaseExpired,
                    );
                    receipt.failure_class = Some(OccurrenceFailureClass::LeaseLost);
                    receipt.detail = Some("lease expired before completion".to_string());
                    let mut expired = expired;
                    expired.last_receipt = Some(receipt.clone());
                    write_receipt_in_txn(&tx, &receipt)?;
                    write_occurrence_in_txn(&tx, &expired)?;
                    expired
                } else {
                    occurrence
                };

                let claim_token = Uuid::now_v7();
                let lease_expires_at_utc = store_now_utc + request.lease_duration;
                let claimed_occurrence = occurrence
                    .apply(OccurrenceLifecycleInput::Claim {
                        owner_id: request.owner_id.clone(),
                        at_utc: store_now_utc,
                        lease_expires_at_utc,
                        claim_token,
                    })
                    .map_err(|error: OccurrenceLifecycleError| {
                        StoreError::Internal(error.to_string())
                    })?
                    .into_occurrence();
                write_occurrence_in_txn(&tx, &claimed_occurrence)?;
                claimed.push(claimed_occurrence);
            }

            tx.commit()?;
            Ok(ClaimDueResult {
                store_now_utc,
                claimed,
            })
        })
        .await
        .map_err(StoreError::Join)?
    }
}

#[async_trait]
impl ScheduleStore for SqliteScheduleStore {
    fn kind(&self) -> ScheduleStoreKind {
        ScheduleStoreKind::Sqlite
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, ScheduleStoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<DateTime<Utc>, StoreError> {
            let conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            Ok(utc_from_millis(select_store_now_ms(&conn)?))
        })
        .await
        .map_err(StoreError::Join)
        .and_then(|result| result)
        .map_err(into_schedule_store_error)
    }

    async fn put_schedule(&self, schedule: Schedule) -> Result<(), ScheduleStoreError> {
        self.put_schedule_impl(schedule)
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

    async fn put_occurrence(&self, occurrence: Occurrence) -> Result<(), ScheduleStoreError> {
        self.put_occurrence_impl(occurrence)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn put_occurrences(
        &self,
        occurrences: Vec<Occurrence>,
    ) -> Result<(), ScheduleStoreError> {
        self.put_occurrences_impl(occurrences)
            .await
            .map_err(into_schedule_store_error)
    }

    async fn commit_schedule_mutation(
        &self,
        schedule: Schedule,
        occurrences: Vec<Occurrence>,
        supersession: Option<PendingSupersession>,
    ) -> Result<(), ScheduleStoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
            write_schedule_in_txn(&tx, &schedule)?;
            for occurrence in &occurrences {
                write_occurrence_in_txn(&tx, occurrence)?;
            }
            if let Some(supersession) = supersession {
                supersede_pending_occurrences_in_txn(&tx, &schedule, supersession)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(StoreError::Join)
        .and_then(|result| result)
        .map_err(into_schedule_store_error)
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

    async fn transition_occurrence_if_current(
        &self,
        occurrence_id: &OccurrenceId,
        expected_attempt: u32,
        expected_claim_token: Option<Uuid>,
        transition: OccurrenceLifecycleInput,
    ) -> Result<Option<Occurrence>, ScheduleStoreError> {
        let path = self.path.clone();
        let occurrence_id = occurrence_id.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
            let current = tx
                .query_row(
                    "SELECT occurrence_json FROM schedule_occurrences WHERE occurrence_id = ?1",
                    params![occurrence_id],
                    |row| row.get::<_, Vec<u8>>(0),
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

            let updated = current
                .apply(transition)
                .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?
                .into_occurrence();
            write_occurrence_in_txn(&tx, &updated)?;
            tx.commit()?;
            Ok(Some(updated))
        })
        .await
        .map_err(StoreError::Join)
        .and_then(|result| result)
        .map_err(into_schedule_store_error)
    }
}

fn ensure_schedule_schema(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(CREATE_SCHEDULES_TABLE_SQL)?;
    conn.execute_batch(CREATE_OCCURRENCES_TABLE_SQL)?;
    conn.execute_batch(CREATE_OCCURRENCES_DUE_INDEX_SQL)?;
    conn.execute_batch(CREATE_OCCURRENCES_SCHEDULE_INDEX_SQL)?;
    conn.execute_batch(CREATE_RECEIPTS_TABLE_SQL)?;
    conn.execute_batch(CREATE_RECEIPTS_OCCURRENCE_INDEX_SQL)?;
    Ok(())
}

fn write_schedule_in_txn(
    tx: &rusqlite::Transaction<'_>,
    schedule: &Schedule,
) -> Result<(), StoreError> {
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
            schedule_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        ON CONFLICT(schedule_id) DO UPDATE SET
            phase = excluded.phase,
            revision = excluded.revision,
            created_at_ms = excluded.created_at_ms,
            updated_at_ms = excluded.updated_at_ms,
            next_occurrence_ordinal = excluded.next_occurrence_ordinal,
            planning_cursor_at_ms = excluded.planning_cursor_at_ms,
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

fn write_occurrence_in_txn(
    tx: &rusqlite::Transaction<'_>,
    occurrence: &Occurrence,
) -> Result<(), StoreError> {
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
            occurrence_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        ON CONFLICT(occurrence_id) DO UPDATE SET
            schedule_id = excluded.schedule_id,
            phase = excluded.phase,
            schedule_revision = excluded.schedule_revision,
            occurrence_ordinal = excluded.occurrence_ordinal,
            due_at_ms = excluded.due_at_ms,
            lease_expires_at_ms = excluded.lease_expires_at_ms,
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

fn supersede_pending_occurrences_in_txn(
    tx: &rusqlite::Transaction<'_>,
    schedule: &Schedule,
    supersession: PendingSupersession,
) -> Result<(), StoreError> {
    let mut stmt = tx.prepare(
        "SELECT occurrence_json
         FROM schedule_occurrences
         WHERE schedule_id = ?1 AND phase = 'pending'
         ORDER BY due_at_ms ASC, schedule_revision ASC, occurrence_ordinal ASC",
    )?;
    let rows = stmt.query_map(params![schedule.schedule_id.to_string()], |row| {
        row.get::<_, Vec<u8>>(0)
    })?;
    for row in rows {
        let bytes = row?;
        let occurrence: Occurrence =
            serde_json::from_slice(&bytes).map_err(StoreError::Serialization)?;
        if occurrence.schedule_revision >= supersession.superseded_by_revision {
            continue;
        }
        let updated = occurrence
            .apply(OccurrenceLifecycleInput::Supersede {
                superseded_by_revision: supersession.superseded_by_revision,
                at_utc: supersession.at_utc,
            })
            .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?
            .into_occurrence();
        write_occurrence_in_txn(tx, &updated)?;
    }
    Ok(())
}

fn update_occurrence_last_receipt_in_txn(
    tx: &rusqlite::Transaction<'_>,
    receipt: &DeliveryReceipt,
) -> Result<(), StoreError> {
    let occurrence_id = receipt.occurrence_id.to_string();
    let Some(bytes) = tx
        .query_row(
            "SELECT occurrence_json FROM schedule_occurrences WHERE occurrence_id = ?1",
            params![occurrence_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?
    else {
        return Ok(());
    };
    let mut occurrence: Occurrence =
        serde_json::from_slice(&bytes).map_err(StoreError::Serialization)?;
    occurrence.last_receipt = Some(receipt.clone());
    write_occurrence_in_txn(tx, &occurrence)
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
        other => ScheduleStoreError::Internal(other.to_string()),
    }
}
