use crate::StoreError;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use meerkat_schedule::{
    ClaimDueRequest, ClaimDueResult, DeliveryReceipt, DeliveryReceiptStage, Occurrence,
    OccurrenceFailureClass, OccurrenceFilter, OccurrenceId, OccurrenceLifecycleAuthority,
    OccurrenceLifecycleError, OccurrenceLifecycleInput, PendingSupersession, Schedule,
    ScheduleFilter, ScheduleStore, ScheduleStoreError, ScheduleStoreKind,
};
use redb::{Database, ReadableTable, TableDefinition, WriteTransaction};
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

const SCHEDULES_BY_ID: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("schedule_redb_schedules_by_id");
const OCCURRENCES_BY_ID: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("schedule_redb_occurrences_by_id");
const RECEIPTS_BY_KEY: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("schedule_redb_receipts_by_key");

pub struct RedbScheduleStore {
    db: Arc<Database>,
}

impl RedbScheduleStore {
    pub fn database(&self) -> Arc<Database> {
        self.db.clone()
    }

    pub fn from_database(db: Arc<Database>) -> Result<Self, StoreError> {
        ensure_tables(&db)?;
        Ok(Self { db })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db = Database::create(path).map_err(|e| StoreError::Database(Box::new(e.into())))?;
        ensure_tables(&db)?;
        Ok(Self { db: Arc::new(db) })
    }

    async fn put_schedule_impl(&self, schedule: Schedule) -> Result<(), StoreError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            write_schedule_in_txn(&write_txn, &schedule)?;
            commit(write_txn)
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn get_schedule_impl(
        &self,
        schedule_id: &meerkat_schedule::ScheduleId,
    ) -> Result<Option<Schedule>, StoreError> {
        let db = self.db.clone();
        let schedule_key = schedule_key(schedule_id);
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let table = read_txn
                .open_table(SCHEDULES_BY_ID)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            table
                .get(schedule_key.as_slice())
                .map_err(|e| StoreError::Database(Box::new(e.into())))?
                .map(|value| {
                    serde_json::from_slice(value.value()).map_err(StoreError::Serialization)
                })
                .transpose()
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn list_schedules_impl(
        &self,
        filter: ScheduleFilter,
    ) -> Result<Vec<Schedule>, StoreError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let table = read_txn
                .open_table(SCHEDULES_BY_ID)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let iter = table
                .iter()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let mut schedules = Vec::new();
            for entry in iter {
                let (_, value) = entry.map_err(|e| StoreError::Database(Box::new(e.into())))?;
                let schedule: Schedule =
                    serde_json::from_slice(value.value()).map_err(StoreError::Serialization)?;
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
            schedules
                .sort_by_key(|schedule| (schedule.created_at_utc, schedule.schedule_id.clone()));
            Ok(schedules)
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn put_occurrence_impl(&self, occurrence: Occurrence) -> Result<(), StoreError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            write_occurrence_in_txn(&write_txn, &occurrence)?;
            commit(write_txn)
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn put_occurrences_impl(&self, occurrences: Vec<Occurrence>) -> Result<(), StoreError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            for occurrence in &occurrences {
                write_occurrence_in_txn(&write_txn, occurrence)?;
            }
            commit(write_txn)
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn get_occurrence_impl(
        &self,
        occurrence_id: &meerkat_schedule::OccurrenceId,
    ) -> Result<Option<Occurrence>, StoreError> {
        let db = self.db.clone();
        let occurrence_key = occurrence_key(occurrence_id);
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let table = read_txn
                .open_table(OCCURRENCES_BY_ID)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            table
                .get(occurrence_key.as_slice())
                .map_err(|e| StoreError::Database(Box::new(e.into())))?
                .map(|value| {
                    serde_json::from_slice(value.value()).map_err(StoreError::Serialization)
                })
                .transpose()
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn list_occurrences_impl(
        &self,
        filter: OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, StoreError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let table = read_txn
                .open_table(OCCURRENCES_BY_ID)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let iter = table
                .iter()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let mut occurrences = Vec::new();
            for entry in iter {
                let (_, value) = entry.map_err(|e| StoreError::Database(Box::new(e.into())))?;
                let occurrence: Occurrence =
                    serde_json::from_slice(value.value()).map_err(StoreError::Serialization)?;
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
            occurrences.sort_by_key(|occurrence| {
                (
                    occurrence.due_at_utc,
                    occurrence.schedule_revision,
                    occurrence.occurrence_ordinal,
                )
            });
            Ok(occurrences)
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn append_receipt_impl(&self, receipt: DeliveryReceipt) -> Result<(), StoreError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            write_receipt_in_txn(&write_txn, &receipt)?;
            update_occurrence_last_receipt_in_txn(&write_txn, &receipt)?;
            commit(write_txn)
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn list_receipts_impl(
        &self,
        occurrence_id: &meerkat_schedule::OccurrenceId,
    ) -> Result<Vec<DeliveryReceipt>, StoreError> {
        let db = self.db.clone();
        let occurrence_prefix = occurrence_key(occurrence_id);
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let table = read_txn
                .open_table(RECEIPTS_BY_KEY)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let iter = table
                .iter()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let mut receipts = Vec::new();
            for entry in iter {
                let (key, value) = entry.map_err(|e| StoreError::Database(Box::new(e.into())))?;
                if !key.value().starts_with(occurrence_prefix.as_slice()) {
                    continue;
                }
                receipts.push(
                    serde_json::from_slice(value.value()).map_err(StoreError::Serialization)?,
                );
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
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let store_now_utc = Utc::now();
            let authority = OccurrenceLifecycleAuthority;

            let active_schedule_ids = {
                let schedules = write_txn
                    .open_table(SCHEDULES_BY_ID)
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;
                let iter = schedules
                    .iter()
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;
                let mut ids = std::collections::BTreeSet::new();
                for entry in iter {
                    let (_, value) = entry.map_err(|e| StoreError::Database(Box::new(e.into())))?;
                    let schedule: Schedule =
                        serde_json::from_slice(value.value()).map_err(StoreError::Serialization)?;
                    if schedule.phase == meerkat_schedule::SchedulePhase::Active {
                        ids.insert(schedule.schedule_id);
                    }
                }
                ids
            };

            let mut candidates = {
                let occurrences = write_txn
                    .open_table(OCCURRENCES_BY_ID)
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;
                let iter = occurrences
                    .iter()
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;
                let mut out = Vec::new();
                for entry in iter {
                    let (_, value) = entry.map_err(|e| StoreError::Database(Box::new(e.into())))?;
                    let occurrence: Occurrence =
                        serde_json::from_slice(value.value()).map_err(StoreError::Serialization)?;
                    if active_schedule_ids.contains(&occurrence.schedule_id)
                        && occurrence.is_claimable_at(store_now_utc)
                    {
                        out.push(occurrence);
                    }
                }
                out
            };

            candidates.sort_by_key(|occurrence| {
                (
                    occurrence.due_at_utc,
                    occurrence.schedule_revision,
                    occurrence.occurrence_ordinal,
                )
            });
            candidates.truncate(request.limit);

            let mut claimed = Vec::new();
            for occurrence in candidates {
                let occurrence = if occurrence.is_reclaimable_at(store_now_utc) {
                    let expired = authority
                        .apply(
                            occurrence,
                            OccurrenceLifecycleInput::LeaseExpired {
                                at_utc: store_now_utc,
                            },
                        )
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
                    write_receipt_in_txn(&write_txn, &receipt)?;
                    write_occurrence_in_txn(&write_txn, &expired)?;
                    expired
                } else {
                    occurrence
                };
                let claim_token = Uuid::now_v7();
                let claimed_occurrence = authority
                    .apply(
                        occurrence,
                        OccurrenceLifecycleInput::Claim {
                            owner_id: request.owner_id.clone(),
                            at_utc: store_now_utc,
                            lease_expires_at_utc: store_now_utc + request.lease_duration,
                            claim_token,
                        },
                    )
                    .map_err(|error: OccurrenceLifecycleError| {
                        StoreError::Internal(error.to_string())
                    })?
                    .into_occurrence();
                write_occurrence_in_txn(&write_txn, &claimed_occurrence)?;
                claimed.push(claimed_occurrence);
            }

            commit(write_txn)?;
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
impl ScheduleStore for RedbScheduleStore {
    fn kind(&self) -> ScheduleStoreKind {
        ScheduleStoreKind::Redb
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, ScheduleStoreError> {
        Ok(Utc::now())
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
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            write_schedule_in_txn(&write_txn, &schedule)?;
            for occurrence in &occurrences {
                write_occurrence_in_txn(&write_txn, occurrence)?;
            }
            if let Some(supersession) = supersession {
                supersede_pending_occurrences_in_txn(&write_txn, &schedule, supersession)?;
            }
            commit(write_txn)
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
        let db = self.db.clone();
        let occurrence_key = occurrence_key(occurrence_id);
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let current = {
                let table = write_txn
                    .open_table(OCCURRENCES_BY_ID)
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;
                table
                    .get(occurrence_key.as_slice())
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?
                    .map(|value| {
                        serde_json::from_slice::<Occurrence>(value.value())
                            .map_err(StoreError::Serialization)
                    })
                    .transpose()?
            };
            let Some(current) = current else {
                commit(write_txn)?;
                return Ok(None);
            };
            if current.attempt_count != expected_attempt
                || current.claim_token() != expected_claim_token
            {
                commit(write_txn)?;
                return Ok(None);
            }

            let updated = OccurrenceLifecycleAuthority
                .apply(current, transition)
                .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?
                .into_occurrence();
            write_occurrence_in_txn(&write_txn, &updated)?;
            commit(write_txn)?;
            Ok(Some(updated))
        })
        .await
        .map_err(StoreError::Join)
        .and_then(|result| result)
        .map_err(into_schedule_store_error)
    }
}

fn ensure_tables(db: &Database) -> Result<(), StoreError> {
    let write_txn = db
        .begin_write()
        .map_err(|e| StoreError::Database(Box::new(e.into())))?;
    {
        let _ = write_txn
            .open_table(SCHEDULES_BY_ID)
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        let _ = write_txn
            .open_table(OCCURRENCES_BY_ID)
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        let _ = write_txn
            .open_table(RECEIPTS_BY_KEY)
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
    }
    commit(write_txn)
}

fn write_schedule_in_txn(
    write_txn: &WriteTransaction,
    schedule: &Schedule,
) -> Result<(), StoreError> {
    let mut table = write_txn
        .open_table(SCHEDULES_BY_ID)
        .map_err(|e| StoreError::Database(Box::new(e.into())))?;
    let json = serde_json::to_vec(schedule).map_err(StoreError::Serialization)?;
    table
        .insert(
            schedule_key(&schedule.schedule_id).as_slice(),
            json.as_slice(),
        )
        .map_err(|e| StoreError::Database(Box::new(e.into())))?;
    Ok(())
}

fn write_occurrence_in_txn(
    write_txn: &WriteTransaction,
    occurrence: &Occurrence,
) -> Result<(), StoreError> {
    let mut table = write_txn
        .open_table(OCCURRENCES_BY_ID)
        .map_err(|e| StoreError::Database(Box::new(e.into())))?;
    let json = serde_json::to_vec(occurrence).map_err(StoreError::Serialization)?;
    table
        .insert(
            occurrence_key(&occurrence.occurrence_id).as_slice(),
            json.as_slice(),
        )
        .map_err(|e| StoreError::Database(Box::new(e.into())))?;
    Ok(())
}

fn write_receipt_in_txn(
    write_txn: &WriteTransaction,
    receipt: &DeliveryReceipt,
) -> Result<(), StoreError> {
    let mut table = write_txn
        .open_table(RECEIPTS_BY_KEY)
        .map_err(|e| StoreError::Database(Box::new(e.into())))?;
    let json = serde_json::to_vec(receipt).map_err(StoreError::Serialization)?;
    table
        .insert(receipt_key(receipt).as_slice(), json.as_slice())
        .map_err(|e| StoreError::Database(Box::new(e.into())))?;
    Ok(())
}

fn supersede_pending_occurrences_in_txn(
    write_txn: &WriteTransaction,
    schedule: &Schedule,
    supersession: PendingSupersession,
) -> Result<(), StoreError> {
    let mut updated = Vec::new();
    {
        let table = write_txn
            .open_table(OCCURRENCES_BY_ID)
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        let iter = table
            .iter()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        for entry in iter {
            let (_, value) = entry.map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let occurrence: Occurrence =
                serde_json::from_slice(value.value()).map_err(StoreError::Serialization)?;
            if occurrence.schedule_id != schedule.schedule_id
                || occurrence.phase != meerkat_schedule::OccurrencePhase::Pending
                || occurrence.schedule_revision >= supersession.superseded_by_revision
            {
                continue;
            }
            let occurrence = OccurrenceLifecycleAuthority
                .apply(
                    occurrence,
                    OccurrenceLifecycleInput::Supersede {
                        superseded_by_revision: supersession.superseded_by_revision,
                        at_utc: supersession.at_utc,
                    },
                )
                .map_err(|error: OccurrenceLifecycleError| StoreError::Internal(error.to_string()))?
                .into_occurrence();
            updated.push(occurrence);
        }
    }
    for occurrence in &updated {
        write_occurrence_in_txn(write_txn, occurrence)?;
    }
    Ok(())
}

fn update_occurrence_last_receipt_in_txn(
    write_txn: &WriteTransaction,
    receipt: &DeliveryReceipt,
) -> Result<(), StoreError> {
    let occurrence_key = occurrence_key(&receipt.occurrence_id);
    let mut table = write_txn
        .open_table(OCCURRENCES_BY_ID)
        .map_err(|e| StoreError::Database(Box::new(e.into())))?;
    let occurrence: Option<Occurrence> = table
        .get(occurrence_key.as_slice())
        .map_err(|e| StoreError::Database(Box::new(e.into())))?
        .map(|value| serde_json::from_slice(value.value()).map_err(StoreError::Serialization))
        .transpose()?;
    let Some(mut occurrence) = occurrence else {
        return Ok(());
    };
    occurrence.last_receipt = Some(receipt.clone());
    let json = serde_json::to_vec(&occurrence).map_err(StoreError::Serialization)?;
    table
        .insert(occurrence_key.as_slice(), json.as_slice())
        .map_err(|e| StoreError::Database(Box::new(e.into())))?;
    Ok(())
}

fn schedule_key(schedule_id: &meerkat_schedule::ScheduleId) -> [u8; 16] {
    *schedule_id.0.as_bytes()
}

fn occurrence_key(occurrence_id: &meerkat_schedule::OccurrenceId) -> [u8; 16] {
    *occurrence_id.0.as_bytes()
}

fn receipt_key(receipt: &DeliveryReceipt) -> [u8; 40] {
    let mut key = [0_u8; 40];
    key[..16].copy_from_slice(receipt.occurrence_id.0.as_bytes());
    key[16..24].copy_from_slice(&recorded_millis(receipt.recorded_at_utc).to_be_bytes());
    key[24..].copy_from_slice(receipt.receipt_id.as_bytes());
    key
}

fn recorded_millis(value: DateTime<Utc>) -> u64 {
    u64::try_from(value.timestamp_millis()).unwrap_or_default()
}

fn commit(write_txn: WriteTransaction) -> Result<(), StoreError> {
    write_txn
        .commit()
        .map_err(|e| StoreError::Database(Box::new(e.into())))
}

fn into_schedule_store_error(error: StoreError) -> ScheduleStoreError {
    match error {
        StoreError::Io(err) => ScheduleStoreError::Io(err.to_string()),
        StoreError::Serialization(err) => ScheduleStoreError::Serialization(err.to_string()),
        other => ScheduleStoreError::Internal(other.to_string()),
    }
}
