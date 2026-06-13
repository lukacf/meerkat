use crate::error::ScheduleStoreError;
use crate::lifecycle::{
    AuthorizedOccurrenceWrite, AuthorizedScheduleWrite, OccurrenceDueAction,
    OccurrenceLifecycleError, OccurrenceLifecycleInput, OccurrenceLifecycleMutator,
    OccurrenceSupersessionAck, ScheduleLifecycleInput,
};
use crate::types::{
    DeliveryReceipt, Occurrence, OccurrenceId, OccurrencePhase, RuntimeDeliveryOutcome, Schedule,
    ScheduleId, SchedulePhase, ScheduleRevision,
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
    at_utc: DateTime<Utc>,
    superseded_by_revision: ScheduleRevision,
}

impl PendingSupersession {
    pub(crate) fn from_schedule_effect(effect: &crate::ScheduleLifecycleEffect) -> Option<Self> {
        if let crate::ScheduleLifecycleEffect::SupersedePendingOccurrences {
            superseding_revision,
            at_utc,
        } = effect
        {
            Some(Self {
                at_utc: *at_utc,
                superseded_by_revision: *superseding_revision,
            })
        } else {
            None
        }
    }

    pub fn at_utc(&self) -> DateTime<Utc> {
        self.at_utc
    }

    pub fn superseded_by_revision(&self) -> ScheduleRevision {
        self.superseded_by_revision
    }
}

pub fn apply_supersession_feedback(
    mut schedule: Schedule,
    acks: Vec<OccurrenceSupersessionAck>,
) -> Result<Schedule, ScheduleStoreError> {
    for ack in acks {
        schedule = Schedule::apply(
            Some(schedule),
            ScheduleLifecycleInput::ConfirmOccurrencesSuperseded { ack },
        )
        .map_err(|error| ScheduleStoreError::Internal(error.to_string()))?
        .into_schedule();
    }
    Ok(schedule)
}

#[derive(Debug, Clone)]
pub(crate) struct ExpiredOccurrenceLease {
    pub(crate) occurrence: Occurrence,
    pub(crate) receipt: DeliveryReceipt,
}

pub(crate) fn expire_occurrence_lease(
    occurrence: Occurrence,
    at_utc: DateTime<Utc>,
) -> Result<ExpiredOccurrenceLease, OccurrenceLifecycleError> {
    let expired = occurrence
        .apply(OccurrenceLifecycleInput::LeaseExpired { at_utc })?
        .into_occurrence();
    let receipt = expired.delivery_receipt_from_authority(None)?;
    let expired = expired
        .apply(OccurrenceLifecycleInput::RecordReceipt {
            runtime_outcome: receipt.runtime_outcome.clone(),
            receipt: receipt.clone(),
        })?
        .into_occurrence();
    Ok(ExpiredOccurrenceLease {
        occurrence: expired,
        receipt,
    })
}

pub(crate) fn claim_occurrence(
    occurrence: Occurrence,
    request: &ClaimDueRequest,
    at_utc: DateTime<Utc>,
) -> Result<Occurrence, OccurrenceLifecycleError> {
    occurrence
        .apply(OccurrenceLifecycleInput::Claim {
            owner_id: request.owner_id.clone(),
            at_utc,
            lease_expires_at_utc: at_utc + request.lease_duration,
            claim_token: Uuid::now_v7(),
        })
        .map(OccurrenceLifecycleMutator::into_occurrence)
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

    async fn commit_schedule_write(
        &self,
        write: AuthorizedScheduleWrite,
    ) -> Result<(), ScheduleStoreError>;

    async fn get_schedule(
        &self,
        schedule_id: &ScheduleId,
    ) -> Result<Option<Schedule>, ScheduleStoreError>;

    async fn list_schedules(
        &self,
        filter: ScheduleFilter,
    ) -> Result<Vec<Schedule>, ScheduleStoreError>;

    async fn commit_occurrence_write(
        &self,
        write: AuthorizedOccurrenceWrite,
    ) -> Result<(), ScheduleStoreError>;

    async fn commit_occurrence_writes(
        &self,
        writes: Vec<AuthorizedOccurrenceWrite>,
    ) -> Result<(), ScheduleStoreError> {
        for write in writes {
            self.commit_occurrence_write(write).await?;
        }
        Ok(())
    }

    async fn commit_schedule_mutation(
        &self,
        schedule: AuthorizedScheduleWrite,
        occurrences: Vec<AuthorizedOccurrenceWrite>,
    ) -> Result<Schedule, ScheduleStoreError>;

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

    async fn transition_occurrence_with_receipt_if_current(
        &self,
        occurrence_id: &OccurrenceId,
        expected_attempt: u32,
        expected_claim_token: Option<Uuid>,
        transition: OccurrenceLifecycleInput,
        runtime_outcome: Option<RuntimeDeliveryOutcome>,
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

    async fn commit_schedule_write(
        &self,
        _write: AuthorizedScheduleWrite,
    ) -> Result<(), ScheduleStoreError> {
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

    async fn commit_occurrence_write(
        &self,
        _write: AuthorizedOccurrenceWrite,
    ) -> Result<(), ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn commit_schedule_mutation(
        &self,
        _schedule: AuthorizedScheduleWrite,
        _occurrences: Vec<AuthorizedOccurrenceWrite>,
    ) -> Result<Schedule, ScheduleStoreError> {
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

    async fn transition_occurrence_with_receipt_if_current(
        &self,
        _occurrence_id: &OccurrenceId,
        _expected_attempt: u32,
        _expected_claim_token: Option<Uuid>,
        _transition: OccurrenceLifecycleInput,
        _runtime_outcome: Option<RuntimeDeliveryOutcome>,
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

    async fn commit_schedule_write(
        &self,
        write: AuthorizedScheduleWrite,
    ) -> Result<(), ScheduleStoreError> {
        reject_standalone_supersession_write(&write)?;
        let mut state = self.inner.write().await;
        write
            .precondition()
            .check_current(state.schedules.get(write.schedule_id()))
            .map_err(ScheduleStoreError::Concurrency)?;
        let schedule = write.into_schedule();
        schedule
            .validate_machine_projection()
            .map_err(ScheduleStoreError::Internal)?;
        state
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

    async fn commit_occurrence_write(
        &self,
        write: AuthorizedOccurrenceWrite,
    ) -> Result<(), ScheduleStoreError> {
        let mut state = self.inner.write().await;
        write
            .precondition()
            .check_current(state.occurrences.get(write.occurrence_id()))
            .map_err(ScheduleStoreError::Concurrency)?;
        let occurrence = write.into_occurrence();
        occurrence
            .validate_machine_projection()
            .map_err(ScheduleStoreError::Internal)?;
        state
            .occurrences
            .insert(occurrence.occurrence_id.clone(), occurrence);
        Ok(())
    }

    async fn commit_schedule_mutation(
        &self,
        schedule: AuthorizedScheduleWrite,
        occurrences: Vec<AuthorizedOccurrenceWrite>,
    ) -> Result<Schedule, ScheduleStoreError> {
        let mut state = self.inner.write().await;
        schedule
            .precondition()
            .check_current(state.schedules.get(schedule.schedule_id()))
            .map_err(ScheduleStoreError::Concurrency)?;
        for occurrence in &occurrences {
            occurrence
                .precondition()
                .check_current(state.occurrences.get(occurrence.occurrence_id()))
                .map_err(ScheduleStoreError::Concurrency)?;
        }
        let (schedule, supersession) = schedule.into_parts();
        let mut committed_schedule = schedule;
        committed_schedule
            .validate_machine_projection()
            .map_err(ScheduleStoreError::Internal)?;
        state.schedules.insert(
            committed_schedule.schedule_id.clone(),
            committed_schedule.clone(),
        );
        for occurrence in occurrences {
            let occurrence = occurrence.into_occurrence();
            occurrence
                .validate_machine_projection()
                .map_err(ScheduleStoreError::Internal)?;
            state
                .occurrences
                .insert(occurrence.occurrence_id.clone(), occurrence);
        }
        let mut occurrence_acks = Vec::new();
        if let Some(supersession) = supersession {
            for occurrence in state.occurrences.values_mut() {
                if occurrence.schedule_id != committed_schedule.schedule_id
                    || occurrence.phase != OccurrencePhase::Pending
                    || occurrence.schedule_revision >= supersession.superseded_by_revision()
                {
                    continue;
                }
                let mutator = occurrence
                    .clone()
                    .apply(OccurrenceLifecycleInput::Supersede {
                        superseded_by_revision: supersession.superseded_by_revision(),
                        at_utc: supersession.at_utc(),
                    })
                    .map_err(|error| ScheduleStoreError::Internal(error.to_string()))?;
                let (updated, _effects, acks) = mutator.into_parts_with_supersession_feedback();
                occurrence_acks.extend(acks);
                *occurrence = updated;
            }
        }
        committed_schedule = apply_supersession_feedback(committed_schedule, occurrence_acks)?;
        state.schedules.insert(
            committed_schedule.schedule_id.clone(),
            committed_schedule.clone(),
        );
        Ok(committed_schedule)
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
                (filter.include_terminal || !occurrence.is_terminal())
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
        let Some(occurrence) = state.occurrences.get(&receipt.occurrence_id).cloned() else {
            return Err(ScheduleStoreError::OccurrenceNotFound {
                occurrence_id: receipt.occurrence_id,
            });
        };
        let updated = occurrence
            .apply(OccurrenceLifecycleInput::RecordReceipt {
                runtime_outcome: receipt.runtime_outcome.clone(),
                receipt: receipt.clone(),
            })
            .map_err(|error| ScheduleStoreError::Internal(error.to_string()))?
            .into_occurrence();
        let canonical_receipt = updated.last_receipt.clone().ok_or_else(|| {
            ScheduleStoreError::Internal(
                "generated occurrence authority did not produce a receipt".to_string(),
            )
        })?;
        state
            .receipts
            .entry(receipt.occurrence_id.clone())
            .or_default()
            .push(canonical_receipt);
        state
            .occurrences
            .insert(updated.occurrence_id.clone(), updated);
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
        let store_now_utc = Utc::now();
        let mut state = self.inner.write().await;

        let active_schedules: BTreeMap<ScheduleId, SchedulePhase> = state
            .schedules
            .iter()
            .map(|(schedule_id, schedule)| (schedule_id.clone(), schedule.phase))
            .collect();

        let mut occurrence_order: Vec<_> = state
            .occurrences
            .values()
            .filter(|occurrence| {
                active_schedules
                    .get(&occurrence.schedule_id)
                    .is_some_and(|phase| *phase == SchedulePhase::Active)
            })
            .map(|occurrence| {
                (
                    (
                        occurrence.due_at_utc,
                        occurrence.schedule_revision,
                        occurrence.occurrence_ordinal,
                    ),
                    occurrence.occurrence_id.clone(),
                )
            })
            .collect();
        occurrence_order.sort_by_key(|(key, _)| *key);

        let mut claimed = Vec::new();
        for (_, occurrence_id) in occurrence_order {
            let Some(existing) = state.occurrences.get(&occurrence_id).cloned() else {
                continue;
            };
            let action = existing
                .classify_due_action(store_now_utc)
                .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?;
            match action {
                Some(OccurrenceDueAction::MisfireRequired) => {
                    let detail = Some(existing.due_misfire_detail_at(store_now_utc));
                    let mut updated = existing
                        .apply(OccurrenceLifecycleInput::ResolveDueMisfire {
                            detail: detail.clone(),
                            at_utc: store_now_utc,
                        })
                        .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?
                        .into_occurrence();
                    let receipt = updated
                        .delivery_receipt_from_authority(None)
                        .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?;
                    updated = updated
                        .apply(OccurrenceLifecycleInput::RecordReceipt {
                            runtime_outcome: receipt.runtime_outcome.clone(),
                            receipt,
                        })
                        .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?
                        .into_occurrence();
                    let canonical_receipt = updated.last_receipt.clone().ok_or_else(|| {
                        ScheduleStoreError::Concurrency(
                            "generated occurrence authority did not produce a receipt".to_string(),
                        )
                    })?;
                    state
                        .receipts
                        .entry(updated.occurrence_id.clone())
                        .or_default()
                        .push(canonical_receipt);
                    state
                        .occurrences
                        .insert(updated.occurrence_id.clone(), updated);
                }
                Some(OccurrenceDueAction::ClaimEligible) => {
                    if claimed.len() >= request.limit {
                        continue;
                    }
                    let updated = claim_occurrence(existing, &request, store_now_utc)
                        .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?;
                    state
                        .occurrences
                        .insert(updated.occurrence_id.clone(), updated.clone());
                    claimed.push(updated);
                }
                Some(OccurrenceDueAction::LeaseExpired) => {
                    if claimed.len() >= request.limit {
                        continue;
                    }
                    let lease_expired = expire_occurrence_lease(existing, store_now_utc)
                        .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?;
                    state
                        .receipts
                        .entry(lease_expired.receipt.occurrence_id.clone())
                        .or_default()
                        .push(lease_expired.receipt.clone());
                    state.occurrences.insert(
                        lease_expired.occurrence.occurrence_id.clone(),
                        lease_expired.occurrence.clone(),
                    );
                    let Ok(updated) =
                        claim_occurrence(lease_expired.occurrence, &request, store_now_utc)
                    else {
                        continue;
                    };
                    state
                        .occurrences
                        .insert(updated.occurrence_id.clone(), updated.clone());
                    claimed.push(updated);
                }
                None => {}
            }
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
        let mut state = self.inner.write().await;
        let Some(current) = state.occurrences.get(occurrence_id).cloned() else {
            return Ok(None);
        };
        if current.attempt_count != expected_attempt
            || current.claim_token() != expected_claim_token
        {
            return Ok(None);
        }
        let updated = current
            .apply(transition)
            .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?
            .into_occurrence();
        state
            .occurrences
            .insert(updated.occurrence_id.clone(), updated.clone());
        Ok(Some(updated))
    }

    async fn transition_occurrence_with_receipt_if_current(
        &self,
        occurrence_id: &OccurrenceId,
        expected_attempt: u32,
        expected_claim_token: Option<Uuid>,
        transition: OccurrenceLifecycleInput,
        runtime_outcome: Option<RuntimeDeliveryOutcome>,
    ) -> Result<Option<Occurrence>, ScheduleStoreError> {
        let mut state = self.inner.write().await;
        let Some(current) = state.occurrences.get(occurrence_id).cloned() else {
            return Ok(None);
        };
        if current.attempt_count != expected_attempt
            || current.claim_token() != expected_claim_token
        {
            return Ok(None);
        }
        let terminalized = current
            .apply(transition)
            .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?
            .into_occurrence();
        let receipt = terminalized
            .delivery_receipt_from_authority(runtime_outcome)
            .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?;
        let updated = terminalized
            .apply(OccurrenceLifecycleInput::RecordReceipt {
                runtime_outcome: receipt.runtime_outcome.clone(),
                receipt,
            })
            .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?
            .into_occurrence();
        let canonical_receipt = updated.last_receipt.clone().ok_or_else(|| {
            ScheduleStoreError::Concurrency(
                "generated occurrence authority did not produce a receipt".to_string(),
            )
        })?;
        state
            .receipts
            .entry(updated.occurrence_id.clone())
            .or_default()
            .push(canonical_receipt);
        state
            .occurrences
            .insert(updated.occurrence_id.clone(), updated.clone());
        Ok(Some(updated))
    }
}

fn unsupported(kind: ScheduleStoreKind) -> ScheduleStoreError {
    ScheduleStoreError::UnsupportedBackend { backend: kind }
}

fn reject_standalone_supersession_write(
    write: &AuthorizedScheduleWrite,
) -> Result<(), ScheduleStoreError> {
    if write.has_pending_supersession() {
        return Err(ScheduleStoreError::Internal(
            "generated schedule supersession requires atomic schedule mutation".into(),
        ));
    }
    Ok(())
}
