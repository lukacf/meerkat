use crate::authority::{OccurrenceLifecycleAuthority, OccurrenceLifecycleInput};
use crate::error::ScheduleStoreError;
use crate::types::{
    DeliveryReceipt, DeliveryReceiptStage, Occurrence, OccurrenceFailureClass, OccurrenceId,
    OccurrencePhase, Schedule, ScheduleId, SchedulePhase, ScheduleRevision,
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimDueRequest {
    pub owner_id: String,
    pub limit: usize,
    pub lease_duration: Duration,
}

#[derive(Debug, Clone)]
pub struct ClaimDueResult {
    pub store_now_utc: DateTime<Utc>,
    pub claimed: Vec<Occurrence>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduleFilter {
    pub phase: Option<SchedulePhase>,
    pub include_deleted: bool,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OccurrenceFilter {
    pub schedule_id: Option<ScheduleId>,
    pub phase: Option<OccurrencePhase>,
    pub include_terminal: bool,
    pub due_after_utc: Option<DateTime<Utc>>,
    pub due_before_utc: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSupersession {
    pub at_utc: DateTime<Utc>,
    pub superseded_by_revision: ScheduleRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleStoreKind {
    Disabled,
    Memory,
    Jsonl,
    Sqlite,
    Custom,
}

impl ScheduleStoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Memory => "memory",
            Self::Jsonl => "jsonl",
            Self::Sqlite => "sqlite",
            Self::Custom => "custom",
        }
    }
}

impl std::fmt::Display for ScheduleStoreKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[async_trait]
pub trait ScheduleStore: Send + Sync {
    fn kind(&self) -> ScheduleStoreKind;

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, ScheduleStoreError>;

    async fn put_schedule(&self, schedule: Schedule) -> Result<(), ScheduleStoreError>;

    async fn get_schedule(
        &self,
        schedule_id: &ScheduleId,
    ) -> Result<Option<Schedule>, ScheduleStoreError>;

    async fn list_schedules(
        &self,
        filter: ScheduleFilter,
    ) -> Result<Vec<Schedule>, ScheduleStoreError>;

    async fn put_occurrence(&self, occurrence: Occurrence) -> Result<(), ScheduleStoreError>;

    async fn put_occurrences(
        &self,
        occurrences: Vec<Occurrence>,
    ) -> Result<(), ScheduleStoreError> {
        for occurrence in occurrences {
            self.put_occurrence(occurrence).await?;
        }
        Ok(())
    }

    async fn commit_schedule_mutation(
        &self,
        schedule: Schedule,
        occurrences: Vec<Occurrence>,
        supersession: Option<PendingSupersession>,
    ) -> Result<(), ScheduleStoreError>;

    async fn get_occurrence(
        &self,
        occurrence_id: &OccurrenceId,
    ) -> Result<Option<Occurrence>, ScheduleStoreError>;

    async fn list_occurrences(
        &self,
        filter: OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, ScheduleStoreError>;

    async fn append_receipt(&self, receipt: DeliveryReceipt) -> Result<(), ScheduleStoreError>;

    async fn list_receipts(
        &self,
        occurrence_id: &OccurrenceId,
    ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError>;

    async fn claim_due_occurrences(
        &self,
        request: ClaimDueRequest,
    ) -> Result<ClaimDueResult, ScheduleStoreError>;

    async fn transition_occurrence_if_current(
        &self,
        occurrence_id: &OccurrenceId,
        expected_attempt: u32,
        expected_claim_token: Option<Uuid>,
        transition: OccurrenceLifecycleInput,
    ) -> Result<Option<Occurrence>, ScheduleStoreError>;
}

#[derive(Default)]
pub struct DisabledScheduleStore;

#[async_trait]
impl ScheduleStore for DisabledScheduleStore {
    fn kind(&self) -> ScheduleStoreKind {
        ScheduleStoreKind::Disabled
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn put_schedule(&self, _schedule: Schedule) -> Result<(), ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn get_schedule(
        &self,
        _schedule_id: &ScheduleId,
    ) -> Result<Option<Schedule>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn list_schedules(
        &self,
        _filter: ScheduleFilter,
    ) -> Result<Vec<Schedule>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn put_occurrence(&self, _occurrence: Occurrence) -> Result<(), ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn commit_schedule_mutation(
        &self,
        _schedule: Schedule,
        _occurrences: Vec<Occurrence>,
        _supersession: Option<PendingSupersession>,
    ) -> Result<(), ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn get_occurrence(
        &self,
        _occurrence_id: &OccurrenceId,
    ) -> Result<Option<Occurrence>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn list_occurrences(
        &self,
        _filter: OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn append_receipt(&self, _receipt: DeliveryReceipt) -> Result<(), ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn list_receipts(
        &self,
        _occurrence_id: &OccurrenceId,
    ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn claim_due_occurrences(
        &self,
        _request: ClaimDueRequest,
    ) -> Result<ClaimDueResult, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn transition_occurrence_if_current(
        &self,
        _occurrence_id: &OccurrenceId,
        _expected_attempt: u32,
        _expected_claim_token: Option<Uuid>,
        _transition: OccurrenceLifecycleInput,
    ) -> Result<Option<Occurrence>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }
}

#[derive(Default)]
pub struct MemoryScheduleStore {
    inner: Arc<RwLock<MemoryScheduleState>>,
}

#[derive(Default)]
struct MemoryScheduleState {
    schedules: BTreeMap<ScheduleId, Schedule>,
    occurrences: BTreeMap<OccurrenceId, Occurrence>,
    receipts: BTreeMap<OccurrenceId, Vec<DeliveryReceipt>>,
}

impl MemoryScheduleStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ScheduleStore for MemoryScheduleStore {
    fn kind(&self) -> ScheduleStoreKind {
        ScheduleStoreKind::Memory
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, ScheduleStoreError> {
        Ok(Utc::now())
    }

    async fn put_schedule(&self, schedule: Schedule) -> Result<(), ScheduleStoreError> {
        self.inner
            .write()
            .await
            .schedules
            .insert(schedule.schedule_id.clone(), schedule);
        Ok(())
    }

    async fn get_schedule(
        &self,
        schedule_id: &ScheduleId,
    ) -> Result<Option<Schedule>, ScheduleStoreError> {
        Ok(self.inner.read().await.schedules.get(schedule_id).cloned())
    }

    async fn list_schedules(
        &self,
        filter: ScheduleFilter,
    ) -> Result<Vec<Schedule>, ScheduleStoreError> {
        let mut schedules: Vec<Schedule> = self
            .inner
            .read()
            .await
            .schedules
            .values()
            .filter(|schedule| {
                (filter.include_deleted || schedule.phase != SchedulePhase::Deleted)
                    && filter.phase.is_none_or(|phase| schedule.phase == phase)
            })
            .cloned()
            .collect();
        schedules
            .sort_by_key(|schedule| (schedule.config.created_at_utc, schedule.schedule_id.clone()));
        if let Some(limit) = filter.limit {
            schedules.truncate(limit);
        }
        Ok(schedules)
    }

    async fn put_occurrence(&self, occurrence: Occurrence) -> Result<(), ScheduleStoreError> {
        self.inner
            .write()
            .await
            .occurrences
            .insert(occurrence.occurrence_id.clone(), occurrence);
        Ok(())
    }

    async fn commit_schedule_mutation(
        &self,
        schedule: Schedule,
        occurrences: Vec<Occurrence>,
        supersession: Option<PendingSupersession>,
    ) -> Result<(), ScheduleStoreError> {
        let mut state = self.inner.write().await;
        state
            .schedules
            .insert(schedule.schedule_id.clone(), schedule.clone());
        for occurrence in occurrences {
            state
                .occurrences
                .insert(occurrence.occurrence_id.clone(), occurrence);
        }
        if let Some(supersession) = supersession {
            for occurrence in state.occurrences.values_mut() {
                if occurrence.schedule_id != schedule.schedule_id
                    || occurrence.phase != OccurrencePhase::Pending
                    || occurrence.schedule_revision >= supersession.superseded_by_revision
                {
                    continue;
                }
                let updated = OccurrenceLifecycleAuthority
                    .apply(
                        occurrence.clone(),
                        OccurrenceLifecycleInput::Supersede {
                            superseded_by_revision: supersession.superseded_by_revision,
                            at_utc: supersession.at_utc,
                        },
                    )
                    .map_err(|error| ScheduleStoreError::Internal(error.to_string()))?
                    .into_occurrence();
                *occurrence = updated;
            }
        }
        Ok(())
    }

    async fn get_occurrence(
        &self,
        occurrence_id: &OccurrenceId,
    ) -> Result<Option<Occurrence>, ScheduleStoreError> {
        Ok(self
            .inner
            .read()
            .await
            .occurrences
            .get(occurrence_id)
            .cloned())
    }

    async fn list_occurrences(
        &self,
        filter: OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, ScheduleStoreError> {
        let mut occurrences: Vec<Occurrence> = self
            .inner
            .read()
            .await
            .occurrences
            .values()
            .filter(|occurrence| {
                (filter.include_terminal || !occurrence.phase.is_terminal())
                    && filter
                        .schedule_id
                        .as_ref()
                        .is_none_or(|schedule_id| &occurrence.schedule_id == schedule_id)
                    && filter.phase.is_none_or(|phase| occurrence.phase == phase)
                    && filter
                        .due_after_utc
                        .is_none_or(|due_after| occurrence.due_at_utc >= due_after)
                    && filter
                        .due_before_utc
                        .is_none_or(|due_before| occurrence.due_at_utc <= due_before)
            })
            .cloned()
            .collect();
        occurrences.sort_by_key(|occurrence| {
            (
                occurrence.due_at_utc,
                occurrence.schedule_revision,
                occurrence.occurrence_ordinal,
            )
        });
        if let Some(limit) = filter.limit {
            occurrences.truncate(limit);
        }
        Ok(occurrences)
    }

    async fn append_receipt(&self, receipt: DeliveryReceipt) -> Result<(), ScheduleStoreError> {
        let mut state = self.inner.write().await;
        state
            .receipts
            .entry(receipt.occurrence_id.clone())
            .or_default()
            .push(receipt.clone());
        if let Some(occurrence) = state.occurrences.get_mut(&receipt.occurrence_id) {
            occurrence.last_receipt = Some(receipt);
        }
        Ok(())
    }

    async fn list_receipts(
        &self,
        occurrence_id: &OccurrenceId,
    ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError> {
        Ok(self
            .inner
            .read()
            .await
            .receipts
            .get(occurrence_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn claim_due_occurrences(
        &self,
        request: ClaimDueRequest,
    ) -> Result<ClaimDueResult, ScheduleStoreError> {
        let authority = OccurrenceLifecycleAuthority;
        let store_now_utc = Utc::now();
        let mut state = self.inner.write().await;

        let active_schedules: BTreeMap<ScheduleId, SchedulePhase> = state
            .schedules
            .iter()
            .map(|(schedule_id, schedule)| (schedule_id.clone(), schedule.phase))
            .collect();

        let misfired_ids: Vec<OccurrenceId> = state
            .occurrences
            .values()
            .filter(|occurrence| {
                active_schedules
                    .get(&occurrence.schedule_id)
                    .is_some_and(|phase| *phase == SchedulePhase::Active)
                    && occurrence.should_misfire_at(store_now_utc)
            })
            .map(|occurrence| occurrence.occurrence_id.clone())
            .collect();

        for occurrence_id in misfired_ids {
            let Some(existing) = state.occurrences.get(&occurrence_id).cloned() else {
                continue;
            };
            let detail = existing.misfire_detail_at(store_now_utc);
            let mut updated = authority
                .apply(
                    existing,
                    OccurrenceLifecycleInput::Misfire {
                        detail: detail.clone(),
                        failure_class: None,
                        at_utc: store_now_utc,
                    },
                )
                .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?
                .into_occurrence();
            let mut receipt = DeliveryReceipt::new(
                updated.occurrence_id.clone(),
                updated.attempt_count,
                DeliveryReceiptStage::Misfired,
            );
            receipt.detail = detail;
            state
                .receipts
                .entry(updated.occurrence_id.clone())
                .or_default()
                .push(receipt.clone());
            updated.last_receipt = Some(receipt);
            state
                .occurrences
                .insert(updated.occurrence_id.clone(), updated);
        }

        let mut candidate_ids: Vec<OccurrenceId> = state
            .occurrences
            .values()
            .filter(|occurrence| {
                active_schedules
                    .get(&occurrence.schedule_id)
                    .is_some_and(|phase| *phase == SchedulePhase::Active)
                    && occurrence.is_claimable_at(store_now_utc)
            })
            .map(|occurrence| occurrence.occurrence_id.clone())
            .collect();

        candidate_ids.sort_by_key(|occurrence_id| {
            state
                .occurrences
                .get(occurrence_id)
                .map(|occurrence| {
                    (
                        occurrence.due_at_utc,
                        occurrence.schedule_revision,
                        occurrence.occurrence_ordinal,
                    )
                })
                .unwrap_or((
                    Utc::now(),
                    crate::types::ScheduleRevision(0),
                    crate::types::OccurrenceOrdinal(0),
                ))
        });
        candidate_ids.truncate(request.limit);

        let mut claimed = Vec::new();
        for occurrence_id in candidate_ids {
            let Some(existing) = state.occurrences.get(&occurrence_id).cloned() else {
                continue;
            };
            let existing = if existing.is_reclaimable_at(store_now_utc) {
                let lease_expired = authority
                    .apply(
                        existing,
                        OccurrenceLifecycleInput::LeaseExpired {
                            at_utc: store_now_utc,
                        },
                    )
                    .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?
                    .into_occurrence();
                let mut receipt = DeliveryReceipt::new(
                    lease_expired.occurrence_id.clone(),
                    lease_expired.attempt_count,
                    DeliveryReceiptStage::LeaseExpired,
                );
                receipt.failure_class = Some(OccurrenceFailureClass::LeaseLost);
                receipt.detail = Some("lease expired before completion".to_string());
                state
                    .receipts
                    .entry(lease_expired.occurrence_id.clone())
                    .or_default()
                    .push(receipt.clone());
                let mut lease_expired = lease_expired;
                lease_expired.last_receipt = Some(receipt);
                state
                    .occurrences
                    .insert(lease_expired.occurrence_id.clone(), lease_expired.clone());
                lease_expired
            } else {
                existing
            };
            let lease_expires_at_utc = store_now_utc + request.lease_duration;
            let claim_token = Uuid::now_v7();
            let updated = authority
                .apply(
                    existing,
                    OccurrenceLifecycleInput::Claim {
                        owner_id: request.owner_id.clone(),
                        at_utc: store_now_utc,
                        lease_expires_at_utc,
                        claim_token,
                    },
                )
                .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?
                .into_occurrence();
            state
                .occurrences
                .insert(updated.occurrence_id.clone(), updated.clone());
            claimed.push(updated);
        }

        Ok(ClaimDueResult {
            store_now_utc,
            claimed,
        })
    }

    async fn transition_occurrence_if_current(
        &self,
        occurrence_id: &OccurrenceId,
        expected_attempt: u32,
        expected_claim_token: Option<Uuid>,
        transition: OccurrenceLifecycleInput,
    ) -> Result<Option<Occurrence>, ScheduleStoreError> {
        let authority = OccurrenceLifecycleAuthority;
        let mut state = self.inner.write().await;
        let Some(current) = state.occurrences.get(occurrence_id).cloned() else {
            return Ok(None);
        };
        if current.attempt_count != expected_attempt
            || current.claim_token() != expected_claim_token
        {
            return Ok(None);
        }
        let updated = authority
            .apply(current, transition)
            .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?
            .into_occurrence();
        state
            .occurrences
            .insert(updated.occurrence_id.clone(), updated.clone());
        Ok(Some(updated))
    }
}

fn unsupported(kind: ScheduleStoreKind) -> ScheduleStoreError {
    ScheduleStoreError::UnsupportedBackend { backend: kind }
}
