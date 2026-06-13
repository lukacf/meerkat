use crate::StoreError;
use crate::sqlite_store::{begin_immediate_transaction, open_connection};
use async_trait::async_trait;
use chrono::{DateTime, LocalResult, TimeZone, Utc};
use meerkat_schedule::{
    AuthorizedOccurrenceWrite, AuthorizedScheduleWrite, ClaimDueRequest, ClaimDueResult,
    DeliveryReceipt, Occurrence, OccurrenceDueAction, OccurrenceFilter, OccurrenceId,
    OccurrenceLifecycleEffect, OccurrenceLifecycleError, OccurrenceLifecycleInput,
    OccurrenceSupersessionAck, PendingSupersession, RuntimeDeliveryOutcome, Schedule,
    ScheduleFilter, SchedulePhase, ScheduleStore, ScheduleStoreError, ScheduleStoreKind,
    apply_supersession_feedback,
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

    async fn commit_schedule_write_impl(
        &self,
        write: AuthorizedScheduleWrite,
    ) -> Result<(), StoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
            reject_standalone_supersession_write(&write)?;
            verify_authorized_schedule_write_in_txn(&tx, &write)?;
            let schedule = write.into_schedule();
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

    async fn commit_occurrence_write_impl(
        &self,
        write: AuthorizedOccurrenceWrite,
    ) -> Result<(), StoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
            verify_authorized_occurrence_write_in_txn(&tx, &write)?;
            let occurrence = write.into_occurrence();
            write_occurrence_in_txn(&tx, &occurrence)?;
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn commit_occurrence_writes_impl(
        &self,
        writes: Vec<AuthorizedOccurrenceWrite>,
    ) -> Result<(), StoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
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
        .map_err(StoreError::Join)?
    }

    async fn append_receipt_impl(&self, receipt: DeliveryReceipt) -> Result<(), StoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
            let canonical_receipt = record_occurrence_receipt_in_txn(&tx, &receipt)?;
            write_receipt_in_txn(&tx, &canonical_receipt)?;
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

            let limit = request.limit;
            let mut occurrences = Vec::new();
            {
                let mut stmt = tx.prepare(
                    r"
                    SELECT o.occurrence_json, s.schedule_json
                    FROM schedule_occurrences o
                    JOIN schedule_schedules s ON s.schedule_id = o.schedule_id
                    ORDER BY o.due_at_ms ASC, o.schedule_revision ASC, o.occurrence_ordinal ASC
                    ",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
                })?;
                for row in rows {
                    let (occurrence_bytes, schedule_bytes) = row?;
                    let schedule: Schedule = serde_json::from_slice(&schedule_bytes)
                        .map_err(StoreError::Serialization)?;
                    if schedule.phase != SchedulePhase::Active {
                        continue;
                    }
                    let occurrence: Occurrence = serde_json::from_slice(&occurrence_bytes)
                        .map_err(StoreError::Serialization)?;
                    occurrences.push(occurrence);
                }
            }

            let mut claimed = Vec::new();
            for occurrence in occurrences {
                let action = occurrence.classify_due_action(store_now_utc).map_err(
                    |error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()),
                )?;
                match action {
                    Some(OccurrenceDueAction::MisfireRequired) => {
                        let detail = Some(occurrence.due_misfire_detail_at(store_now_utc));
                        let mut updated = occurrence
                            .apply(OccurrenceLifecycleInput::ResolveDueMisfire {
                                detail: detail.clone(),
                                at_utc: store_now_utc,
                            })
                            .map_err(|error: OccurrenceLifecycleError| {
                                StoreError::Internal(error.to_string())
                            })?
                            .into_occurrence();
                        let receipt = updated.delivery_receipt_from_authority(None).map_err(
                            |error: OccurrenceLifecycleError| {
                                StoreError::Internal(error.to_string())
                            },
                        )?;
                        updated = updated
                            .apply(OccurrenceLifecycleInput::RecordReceipt {
                                runtime_outcome: receipt.runtime_outcome.clone(),
                                receipt: receipt.clone(),
                            })
                            .map_err(|error: OccurrenceLifecycleError| {
                                StoreError::Internal(error.to_string())
                            })?
                            .into_occurrence();
                        write_receipt_in_txn(&tx, &receipt)?;
                        write_occurrence_in_txn(&tx, &updated)?;
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
                        let (expired, receipt) =
                            expire_occurrence_lease_for_sqlite(occurrence, store_now_utc)?;
                        write_receipt_in_txn(&tx, &receipt)?;
                        write_occurrence_in_txn(&tx, &expired)?;
                        let Ok(claimed_occurrence) =
                            claim_occurrence_for_sqlite(expired, &request, store_now_utc)
                        else {
                            continue;
                        };
                        write_occurrence_in_txn(&tx, &claimed_occurrence)?;
                        claimed.push(claimed_occurrence);
                    }
                    None => {}
                }
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
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
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
            tx.commit()?;
            Ok(committed_schedule)
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
    ) -> Result<Option<(Occurrence, Vec<OccurrenceLifecycleEffect>)>, ScheduleStoreError> {
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
        .map_err(StoreError::Join)
        .and_then(|result| result)
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
        let path = self.path.clone();
        let occurrence_id = occurrence_id.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            ensure_schedule_schema(&conn)?;
            let tx = begin_immediate_transaction(&mut conn)?;
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
        |row| row.get::<_, Vec<u8>>(0),
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
        |row| row.get::<_, Vec<u8>>(0),
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
        row.get::<_, Vec<u8>>(0)
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
            |row| row.get::<_, Vec<u8>>(0),
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
