use crate::error::{ScheduleDomainError, ScheduleStoreError};
use crate::lifecycle::{
    AuthorizedOccurrenceWrite, ScheduleLifecycleInput, ScheduleLifecycleMutator,
};
use crate::store::{OccurrenceFilter, ScheduleFilter, ScheduleRefillCandidate, ScheduleStore};
use crate::trigger::{next_due_after, occurrences_for_horizon};
use crate::types::{
    CreateScheduleRequest, Occurrence, OccurrencePhase, Schedule, ScheduleId, SchedulePhase,
    UpdateScheduleRequest,
};
use chrono::{Duration, Utc};
use meerkat_core::SessionId;
use std::collections::BTreeSet;
use std::sync::Arc;

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::{Mutex, watch};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::{Mutex, watch};

#[derive(Clone)]
pub struct ScheduleService {
    store: Arc<dyn ScheduleStore>,
    planning_lock: Arc<Mutex<()>>,
    mutation_generation: watch::Sender<u64>,
}

impl ScheduleService {
    pub fn new(store: Arc<dyn ScheduleStore>) -> Self {
        let (mutation_generation, _) = watch::channel(0);
        Self {
            store,
            planning_lock: Arc::new(Mutex::new(())),
            mutation_generation,
        }
    }

    pub fn store(&self) -> Arc<dyn ScheduleStore> {
        self.store.clone()
    }

    /// Subscribe to successful schedule mutations made through this service.
    ///
    /// The signal is deliberately advisory and process-local. It lets the
    /// local schedule host wake immediately after a create/update/resume
    /// instead of waiting for its declared durable push/poll wake. Durable
    /// store time and the store's next-action query remain the scheduling
    /// authority; this channel never carries schedule state.
    pub fn subscribe_mutations(&self) -> watch::Receiver<u64> {
        self.mutation_generation.subscribe()
    }

    fn notify_mutation(&self) {
        self.mutation_generation
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    pub async fn create(
        &self,
        request: CreateScheduleRequest,
    ) -> Result<Schedule, ScheduleDomainError> {
        request
            .validate_public_api()
            .map_err(ScheduleDomainError::InvalidSchedule)?;
        let _planning_guard = self.planning_lock.lock().await;
        let mut mutator = Schedule::apply(None, ScheduleLifecycleInput::Create(request))
            .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;
        let store_now = self.store.get_store_time_utc().await?;
        let planned = self
            .plan_schedule_occurrences(&mutator.schedule, store_now)
            .await?;
        if let Some(planning_mutator) = planned.schedule_mutator {
            mutator
                .absorb_followup(planning_mutator)
                .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        }
        let committed = self
            .store
            .commit_schedule_refill(
                mutator.into_authorized_write(),
                planned.occurrences,
                planned.next_refill_at_utc,
            )
            .await?;
        self.notify_mutation();
        Ok(committed)
    }

    pub async fn get(&self, schedule_id: &ScheduleId) -> Result<Schedule, ScheduleDomainError> {
        self.store
            .get_schedule(schedule_id)
            .await?
            .ok_or_else(|| ScheduleStoreError::ScheduleNotFound {
                schedule_id: schedule_id.clone(),
            })
            .map_err(Into::into)
    }

    pub async fn list(&self) -> Result<Vec<Schedule>, ScheduleDomainError> {
        self.store
            .list_schedules(ScheduleFilter {
                include_deleted: false,
                ..ScheduleFilter::default()
            })
            .await
            .map_err(Into::into)
    }

    /// Explicitly list non-deleted schedules with per-row tolerance. Ordinary
    /// driver ticks use the bounded refill-candidate store seam instead.
    pub async fn list_with_row_faults(
        &self,
    ) -> Result<(Vec<Schedule>, Vec<crate::ScheduleStoreRowFault>), ScheduleDomainError> {
        self.store
            .list_schedules_with_row_faults(ScheduleFilter {
                include_deleted: false,
                ..ScheduleFilter::default()
            })
            .await
            .map_err(Into::into)
    }

    pub async fn list_filtered(
        &self,
        filter: ScheduleFilter,
    ) -> Result<Vec<Schedule>, ScheduleDomainError> {
        self.store.list_schedules(filter).await.map_err(Into::into)
    }

    pub async fn update(
        &self,
        schedule_id: &ScheduleId,
        request: UpdateScheduleRequest,
    ) -> Result<Schedule, ScheduleDomainError> {
        request
            .validate_public_api()
            .map_err(ScheduleDomainError::InvalidSchedule)?;
        let _planning_guard = self.planning_lock.lock().await;
        let current = self.get(schedule_id).await?;
        let store_now = self.store.get_store_time_utc().await?;
        let mut mutator = Schedule::apply(
            Some(current),
            ScheduleLifecycleInput::Update {
                request,
                at_utc: store_now,
            },
        )
        .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;

        let planned = self
            .plan_schedule_occurrences(&mutator.schedule, store_now)
            .await?;
        if let Some(planning_mutator) = planned.schedule_mutator {
            mutator
                .absorb_followup(planning_mutator)
                .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        }
        let committed = self
            .store
            .commit_schedule_refill(
                mutator.into_authorized_write(),
                planned.occurrences,
                planned.next_refill_at_utc,
            )
            .await?;
        self.notify_mutation();
        Ok(committed)
    }

    pub async fn pause(&self, schedule_id: &ScheduleId) -> Result<Schedule, ScheduleDomainError> {
        let _planning_guard = self.planning_lock.lock().await;
        let current = self.get(schedule_id).await?;
        let mutator = Schedule::apply(
            Some(current),
            ScheduleLifecycleInput::Pause {
                at_utc: self.store.get_store_time_utc().await?,
            },
        )
        .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;
        let schedule = mutator.schedule.clone();
        self.store
            .commit_schedule_write(mutator.into_authorized_write())
            .await?;
        self.notify_mutation();
        Ok(schedule)
    }

    pub async fn resume(&self, schedule_id: &ScheduleId) -> Result<Schedule, ScheduleDomainError> {
        let _planning_guard = self.planning_lock.lock().await;
        let current = self.get(schedule_id).await?;
        let mut mutator = Schedule::apply(
            Some(current),
            ScheduleLifecycleInput::Resume {
                at_utc: self.store.get_store_time_utc().await?,
            },
        )
        .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;
        let store_now = self.store.get_store_time_utc().await?;
        let planned = self
            .plan_schedule_occurrences(&mutator.schedule, store_now)
            .await?;
        if let Some(planning_mutator) = planned.schedule_mutator {
            mutator
                .absorb_followup(planning_mutator)
                .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        }
        let committed = self
            .store
            .commit_schedule_refill(
                mutator.into_authorized_write(),
                planned.occurrences,
                planned.next_refill_at_utc,
            )
            .await?;
        self.notify_mutation();
        Ok(committed)
    }

    pub async fn delete(&self, schedule_id: &ScheduleId) -> Result<Schedule, ScheduleDomainError> {
        let _planning_guard = self.planning_lock.lock().await;
        let current = self.get(schedule_id).await?;
        let store_now = self.store.get_store_time_utc().await?;
        let mutator = Schedule::apply(
            Some(current),
            ScheduleLifecycleInput::Delete { at_utc: store_now },
        )
        .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;
        let committed = self
            .store
            .commit_schedule_mutation(mutator.into_authorized_write(), Vec::new())
            .await?;
        self.notify_mutation();
        Ok(committed)
    }

    pub async fn list_occurrences(
        &self,
        schedule_id: &ScheduleId,
    ) -> Result<Vec<Occurrence>, ScheduleDomainError> {
        self.list_occurrences_filtered(schedule_id, true).await
    }

    pub async fn list_occurrences_filtered(
        &self,
        schedule_id: &ScheduleId,
        include_terminal: bool,
    ) -> Result<Vec<Occurrence>, ScheduleDomainError> {
        self.store
            .list_occurrences(OccurrenceFilter {
                schedule_id: Some(schedule_id.clone()),
                include_terminal,
                ..OccurrenceFilter::default()
            })
            .await
            .map_err(Into::into)
    }

    pub async fn refill_horizon(
        &self,
        schedule_id: &ScheduleId,
    ) -> Result<Vec<Occurrence>, ScheduleDomainError> {
        let _planning_guard = self.planning_lock.lock().await;
        let schedule = self.get(schedule_id).await?;
        let store_now = self.store.get_store_time_utc().await?;
        let planned = self.plan_schedule_occurrences(&schedule, store_now).await?;
        let occurrences = planned
            .occurrences
            .iter()
            .map(|write| write.occurrence().clone())
            .collect();
        if let Some(planning_mutator) = planned.schedule_mutator {
            let _ = self
                .store
                .commit_schedule_refill(
                    planning_mutator.into_authorized_write(),
                    planned.occurrences,
                    planned.next_refill_at_utc,
                )
                .await?;
            self.notify_mutation();
        }
        Ok(occurrences)
    }

    /// Refill one store-selected durable candidate without re-reading its
    /// schedule, store clock, or Pending set through separate connections.
    pub(crate) async fn refill_candidate(
        &self,
        candidate: ScheduleRefillCandidate,
        store_now_utc: chrono::DateTime<Utc>,
    ) -> Result<Vec<Occurrence>, ScheduleDomainError> {
        let _planning_guard = self.planning_lock.lock().await;
        let planned = self.plan_schedule_occurrences_from_snapshot(
            &candidate.schedule,
            store_now_utc,
            &candidate.pending_occurrences,
        )?;
        let occurrences: Vec<Occurrence> = planned
            .occurrences
            .iter()
            .map(|write| write.occurrence().clone())
            .collect();
        if let Some(planning_mutator) = planned.schedule_mutator {
            let _ = self
                .store
                .commit_schedule_refill(
                    planning_mutator.into_authorized_write(),
                    planned.occurrences,
                    planned.next_refill_at_utc,
                )
                .await?;
        } else {
            self.store
                .record_refill_deadline_if_current(
                    &candidate.schedule.schedule_id,
                    candidate.schedule.revision,
                    candidate.refill_at_utc,
                    planned.next_refill_at_utc,
                )
                .await?;
        }
        Ok(occurrences)
    }

    pub async fn sync_occurrence_target_with_schedule(
        &self,
        mut occurrence: Occurrence,
    ) -> Result<Occurrence, ScheduleDomainError> {
        let current = match self.store.get_schedule(&occurrence.schedule_id).await? {
            Some(schedule) => schedule,
            None => return Ok(occurrence),
        };
        if current.revision != occurrence.schedule_revision {
            return Ok(occurrence);
        }
        if occurrence.target_snapshot == current.target {
            return Ok(occurrence);
        }
        let mutator = occurrence
            .apply(crate::OccurrenceLifecycleInput::SyncTargetSnapshot {
                target_snapshot: current.target.clone(),
            })
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        occurrence = mutator.occurrence.clone();
        self.store
            .commit_occurrence_write(mutator.into_authorized_write())
            .await?;
        self.notify_mutation();
        Ok(occurrence)
    }

    pub async fn bind_materialized_session_for_occurrence(
        &self,
        occurrence: &Occurrence,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        let Some(schedule) = self.store.get_schedule(&occurrence.schedule_id).await? else {
            return Ok(());
        };
        if schedule.revision != occurrence.schedule_revision {
            return Ok(());
        }

        let mut updated_schedule_target = schedule.target.clone();
        let schedule_changed = updated_schedule_target.bind_materialized_session(session_id);
        if schedule_changed {
            let mutator = Schedule::apply(
                Some(schedule),
                ScheduleLifecycleInput::SyncTargetSnapshot {
                    target: updated_schedule_target,
                },
            )
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
            self.store
                .commit_schedule_write(mutator.into_authorized_write())
                .await?;
            self.notify_mutation();
        }

        let pending = self
            .store
            .list_occurrences(OccurrenceFilter {
                schedule_id: Some(occurrence.schedule_id.clone()),
                include_terminal: false,
                phase: Some(OccurrencePhase::Pending),
                ..OccurrenceFilter::default()
            })
            .await?;

        let mut updated_pending = Vec::new();
        for pending_occurrence in pending {
            if pending_occurrence.schedule_revision != occurrence.schedule_revision {
                continue;
            }
            let mut updated_target = pending_occurrence.target_snapshot.clone();
            if updated_target.bind_materialized_session(session_id) {
                let mutator = pending_occurrence
                    .apply(crate::OccurrenceLifecycleInput::SyncTargetSnapshot {
                        target_snapshot: updated_target,
                    })
                    .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
                updated_pending.push(mutator.into_authorized_write());
            }
        }

        let pending_changed = !updated_pending.is_empty();
        if pending_changed {
            self.store.commit_occurrence_writes(updated_pending).await?;
            self.notify_mutation();
        }

        Ok(())
    }

    async fn plan_schedule_occurrences(
        &self,
        schedule: &Schedule,
        store_now_utc: chrono::DateTime<Utc>,
    ) -> Result<PlannedScheduleOccurrences, ScheduleDomainError> {
        let existing = self
            .store
            .list_occurrences(OccurrenceFilter {
                schedule_id: Some(schedule.schedule_id.clone()),
                include_terminal: false,
                phase: Some(OccurrencePhase::Pending),
                ..OccurrenceFilter::default()
            })
            .await?;
        self.plan_schedule_occurrences_from_snapshot(schedule, store_now_utc, &existing)
    }

    fn plan_schedule_occurrences_from_snapshot(
        &self,
        schedule: &Schedule,
        store_now_utc: chrono::DateTime<Utc>,
        existing: &[Occurrence],
    ) -> Result<PlannedScheduleOccurrences, ScheduleDomainError> {
        if schedule.phase != SchedulePhase::Active {
            return Ok(PlannedScheduleOccurrences::default());
        }

        let horizon_duration = Duration::days(i64::from(schedule.config.planning_horizon_days));
        let horizon_end_utc = store_now_utc + horizon_duration;
        let existing_due: BTreeSet<_> = existing
            .iter()
            .filter(|occurrence| occurrence.schedule_revision == schedule.revision)
            .map(|occurrence| occurrence.due_at_utc)
            .collect();

        let future_pending_count = existing
            .iter()
            .filter(|occurrence| {
                occurrence.schedule_revision == schedule.revision
                    && occurrence.due_at_utc <= horizon_end_utc
                    && occurrence.phase == OccurrencePhase::Pending
            })
            .count();

        let desired_count =
            usize::try_from(schedule.config.planning_horizon_occurrences).unwrap_or(usize::MAX);
        if desired_count == 0 || future_pending_count >= desired_count {
            return Ok(PlannedScheduleOccurrences::default());
        }

        let remaining = desired_count.saturating_sub(future_pending_count);
        let cursor = existing
            .iter()
            .filter(|occurrence| occurrence.schedule_revision == schedule.revision)
            .map(|occurrence| occurrence.due_at_utc)
            .max()
            .or(schedule.planning_cursor_utc)
            .unwrap_or_else(|| store_now_utc - Duration::minutes(1));

        let due_times = occurrences_for_horizon(
            &schedule.trigger,
            Some(cursor),
            horizon_end_utc,
            remaining.saturating_add(existing_due.len()),
        )?;

        let mut planned = Vec::new();
        let mut next_occurrence_ordinal = schedule.next_occurrence_ordinal;
        for due_at_utc in due_times {
            if existing_due.contains(&due_at_utc) {
                continue;
            }
            let occurrence = Occurrence::planned_write_from_schedule(
                schedule,
                next_occurrence_ordinal,
                due_at_utc,
            )
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
            next_occurrence_ordinal = next_occurrence_ordinal.next();
            planned.push(occurrence);
            if planned.len() >= remaining {
                break;
            }
        }

        let pending_after_plan = future_pending_count.saturating_add(planned.len());
        let next_refill_at_utc = if pending_after_plan >= desired_count {
            None
        } else {
            let next_cursor = planned
                .last()
                .map(|write| write.occurrence().due_at_utc)
                .unwrap_or(cursor);
            next_due_after(&schedule.trigger, Some(next_cursor))?
                .map(|next_due| (next_due - horizon_duration).max(store_now_utc))
        };

        if !planned.is_empty() {
            let Some(planning_cursor_utc) =
                planned.last().map(|write| write.occurrence().due_at_utc)
            else {
                return Ok(PlannedScheduleOccurrences {
                    occurrences: planned,
                    schedule_mutator: None,
                    next_refill_at_utc,
                });
            };
            let mutator = Schedule::apply(
                Some(schedule.clone()),
                ScheduleLifecycleInput::RecordPlanningWindow {
                    planning_cursor_utc,
                    next_occurrence_ordinal,
                },
            )
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
            return Ok(PlannedScheduleOccurrences {
                occurrences: planned,
                schedule_mutator: Some(mutator),
                next_refill_at_utc,
            });
        }

        Ok(PlannedScheduleOccurrences {
            occurrences: planned,
            schedule_mutator: None,
            next_refill_at_utc,
        })
    }
}

#[derive(Default)]
struct PlannedScheduleOccurrences {
    occurrences: Vec<AuthorizedOccurrenceWrite>,
    schedule_mutator: Option<ScheduleLifecycleMutator>,
    next_refill_at_utc: Option<chrono::DateTime<Utc>>,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::types::{
        DeliveryReceipt, HelperOptionsSpec, IntervalTriggerSpec, MisfirePolicy, MobTargetBinding,
        OccurrenceId, ResolvedSpawnSnapshot, ScheduleSpawnTooling, ScheduledSessionAction,
        SessionMaterializationSpec, SessionTargetBinding, TargetBinding, TriggerSpec,
    };
    use crate::{MemoryScheduleStore, OverlapPolicy};
    use crate::{OccurrenceLifecycleEffect, OccurrenceLifecycleInput};
    use chrono::{Duration, TimeZone};
    use meerkat_core::{ContentInput, ToolNameSet};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    struct AtomicMutationProbeStore {
        inner: Arc<dyn ScheduleStore>,
        atomic_calls: AtomicUsize,
        standalone_schedule_commits: AtomicUsize,
    }

    impl AtomicMutationProbeStore {
        fn new() -> Self {
            Self {
                inner: Arc::new(MemoryScheduleStore::new()),
                atomic_calls: AtomicUsize::new(0),
                standalone_schedule_commits: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ScheduleStore for AtomicMutationProbeStore {
        fn kind(&self) -> crate::ScheduleStoreKind {
            self.inner.kind()
        }

        fn wake_mode(&self) -> crate::ScheduleStoreWakeMode {
            self.inner.wake_mode()
        }

        async fn wait_for_durable_wake(&self) -> Result<(), ScheduleStoreError> {
            self.inner.wait_for_durable_wake().await
        }

        async fn get_store_time_utc(&self) -> Result<chrono::DateTime<Utc>, ScheduleStoreError> {
            self.inner.get_store_time_utc().await
        }

        async fn next_action_time_utc(
            &self,
        ) -> Result<crate::ScheduleStoreActionTime, ScheduleStoreError> {
            self.inner.next_action_time_utc().await
        }

        async fn read_due_refill_candidates(
            &self,
            limit: usize,
        ) -> Result<crate::ScheduleRefillBatch, ScheduleStoreError> {
            self.inner.read_due_refill_candidates(limit).await
        }

        async fn commit_schedule_write(
            &self,
            write: crate::AuthorizedScheduleWrite,
        ) -> Result<(), ScheduleStoreError> {
            self.standalone_schedule_commits
                .fetch_add(1, Ordering::SeqCst);
            self.inner.commit_schedule_write(write).await
        }

        async fn get_schedule(
            &self,
            schedule_id: &ScheduleId,
        ) -> Result<Option<Schedule>, ScheduleStoreError> {
            self.inner.get_schedule(schedule_id).await
        }

        async fn list_schedules(
            &self,
            filter: ScheduleFilter,
        ) -> Result<Vec<Schedule>, ScheduleStoreError> {
            self.inner.list_schedules(filter).await
        }

        async fn commit_occurrence_write(
            &self,
            write: AuthorizedOccurrenceWrite,
        ) -> Result<(), ScheduleStoreError> {
            self.inner.commit_occurrence_write(write).await
        }

        async fn commit_occurrence_writes(
            &self,
            writes: Vec<AuthorizedOccurrenceWrite>,
        ) -> Result<(), ScheduleStoreError> {
            self.inner.commit_occurrence_writes(writes).await
        }

        async fn commit_schedule_mutation(
            &self,
            schedule: crate::AuthorizedScheduleWrite,
            occurrences: Vec<AuthorizedOccurrenceWrite>,
        ) -> Result<Schedule, ScheduleStoreError> {
            self.atomic_calls.fetch_add(1, Ordering::SeqCst);
            self.inner
                .commit_schedule_mutation(schedule, occurrences)
                .await
        }

        async fn commit_schedule_refill(
            &self,
            schedule: crate::AuthorizedScheduleWrite,
            occurrences: Vec<AuthorizedOccurrenceWrite>,
            next_refill_at_utc: Option<chrono::DateTime<Utc>>,
        ) -> Result<Schedule, ScheduleStoreError> {
            self.atomic_calls.fetch_add(1, Ordering::SeqCst);
            self.inner
                .commit_schedule_refill(schedule, occurrences, next_refill_at_utc)
                .await
        }

        async fn record_refill_deadline_if_current(
            &self,
            schedule_id: &ScheduleId,
            expected_revision: crate::ScheduleRevision,
            expected_refill_at_utc: chrono::DateTime<Utc>,
            next_refill_at_utc: Option<chrono::DateTime<Utc>>,
        ) -> Result<(), ScheduleStoreError> {
            self.inner
                .record_refill_deadline_if_current(
                    schedule_id,
                    expected_revision,
                    expected_refill_at_utc,
                    next_refill_at_utc,
                )
                .await
        }

        async fn get_occurrence(
            &self,
            occurrence_id: &OccurrenceId,
        ) -> Result<Option<Occurrence>, ScheduleStoreError> {
            self.inner.get_occurrence(occurrence_id).await
        }

        async fn list_occurrences(
            &self,
            filter: OccurrenceFilter,
        ) -> Result<Vec<Occurrence>, ScheduleStoreError> {
            self.inner.list_occurrences(filter).await
        }

        async fn append_receipt(&self, receipt: DeliveryReceipt) -> Result<(), ScheduleStoreError> {
            self.inner.append_receipt(receipt).await
        }

        async fn list_receipts(
            &self,
            occurrence_id: &OccurrenceId,
        ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError> {
            self.inner.list_receipts(occurrence_id).await
        }

        async fn claim_due_occurrences(
            &self,
            request: crate::ClaimDueRequest,
        ) -> Result<crate::ClaimDueResult, ScheduleStoreError> {
            self.inner.claim_due_occurrences(request).await
        }

        async fn renew_occurrence_lease_if_current(
            &self,
            request: crate::RenewOccurrenceLeaseRequest,
        ) -> Result<crate::RenewOccurrenceLeaseResult, ScheduleStoreError> {
            self.inner.renew_occurrence_lease_if_current(request).await
        }

        async fn transition_occurrence_if_current(
            &self,
            occurrence_id: &OccurrenceId,
            expected_attempt: u32,
            expected_claim_token: Option<Uuid>,
            transition: OccurrenceLifecycleInput,
        ) -> Result<Option<(Occurrence, Vec<OccurrenceLifecycleEffect>)>, ScheduleStoreError>
        {
            self.inner
                .transition_occurrence_if_current(
                    occurrence_id,
                    expected_attempt,
                    expected_claim_token,
                    transition,
                )
                .await
        }

        async fn transition_occurrence_with_receipt_if_current(
            &self,
            occurrence_id: &OccurrenceId,
            expected_attempt: u32,
            expected_claim_token: Option<Uuid>,
            transition: OccurrenceLifecycleInput,
            runtime_outcome: Option<crate::RuntimeDeliveryOutcome>,
        ) -> Result<Option<Occurrence>, ScheduleStoreError> {
            self.inner
                .transition_occurrence_with_receipt_if_current(
                    occurrence_id,
                    expected_attempt,
                    expected_claim_token,
                    transition,
                    runtime_outcome,
                )
                .await
        }
    }

    #[tokio::test]
    async fn far_future_trigger_sleeps_until_exact_horizon_entry() {
        let memory = Arc::new(MemoryScheduleStore::new());
        let store = memory.clone() as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let now = Utc::now();
        let start_at_utc = Utc
            .timestamp_millis_opt((now + Duration::days(3)).timestamp_millis())
            .single()
            .expect("millisecond timestamp");

        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("far-future".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc,
                    every_seconds: 60,
                    end_at_utc: None,
                }),
                target: TargetBinding::session(SessionTargetBinding::ExactSession {
                    session_id: SessionId::new(),
                    action: ScheduledSessionAction::Prompt {
                        prompt: ContentInput::Text("wake later".into()),
                        system_prompt: None,
                        render_metadata: None,
                        skill_refs: Vec::new(),
                        additional_instructions: Vec::new(),
                    },
                }),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: crate::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("create far-future schedule");

        assert!(
            service
                .list_occurrences(&schedule.schedule_id)
                .await
                .expect("list occurrences")
                .is_empty(),
            "a trigger outside the horizon must not create an occurrence"
        );
        let action = store.next_action_time_utc().await.expect("next action");
        assert_eq!(
            action.next_action_at_utc,
            Some(start_at_utc - Duration::days(1)),
            "the durable wake is exactly when the trigger enters the horizon"
        );
        assert!(
            store
                .read_due_refill_candidates(32)
                .await
                .expect("read refill candidates")
                .candidates
                .is_empty(),
            "far-future schedules must not be swept on ordinary ticks"
        );
    }

    #[tokio::test]
    async fn successful_local_mutation_advances_host_wake_generation() {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);
        let mut mutations = service.subscribe_mutations();

        service
            .create(CreateScheduleRequest {
                name: Some("wake-host".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() + Duration::minutes(1),
                    every_seconds: 60,
                    end_at_utc: None,
                }),
                target: TargetBinding::session(SessionTargetBinding::ExactSession {
                    session_id: SessionId::new(),
                    action: ScheduledSessionAction::Prompt {
                        prompt: ContentInput::Text("wake".into()),
                        system_prompt: None,
                        render_metadata: None,
                        skill_refs: Vec::new(),
                        additional_instructions: Vec::new(),
                    },
                }),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: crate::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("schedule creation should commit");

        mutations
            .changed()
            .await
            .expect("service-owned mutation sender must remain live");
    }

    #[tokio::test]
    async fn create_rejects_parent_context_mob_helper_tooling() {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);

        let error = service
            .create(CreateScheduleRequest {
                name: Some("bad-helper".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() + Duration::minutes(1),
                    every_seconds: 60,
                    end_at_utc: None,
                }),
                target: TargetBinding::Mob(Box::new(MobTargetBinding::SpawnHelper {
                    mob_id: "ops".to_string(),
                    member_id: "helper".to_string(),
                    prompt: "check state".to_string(),
                    options: HelperOptionsSpec {
                        tooling: Some(ScheduleSpawnTooling::InheritParent {
                            allow_overlay: None,
                            deny_overlay: None,
                        }),
                        ..HelperOptionsSpec::default()
                    },
                })),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: crate::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect_err("parent-context helper tooling should be rejected");

        assert!(matches!(
            error,
            ScheduleDomainError::InvalidSchedule(message)
                if message.contains("requires parent agent context")
        ));
    }

    #[tokio::test]
    async fn create_rejects_untrusted_mob_helper_resolved_snapshot() {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);

        let error = service
            .create(CreateScheduleRequest {
                name: Some("bad-helper-snapshot".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() + Duration::minutes(1),
                    every_seconds: 60,
                    end_at_utc: None,
                }),
                target: mob_helper_target(HelperOptionsSpec {
                    resolved_spawn_snapshot: Some(resolved_spawn_snapshot_fixture()),
                    ..HelperOptionsSpec::default()
                }),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: crate::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect_err("untrusted helper snapshot should be rejected");

        assert!(matches!(
            error,
            ScheduleDomainError::InvalidSchedule(message)
                if message.contains("trusted internal schedule state")
        ));
    }

    #[tokio::test]
    async fn update_rejects_untrusted_mob_helper_resolved_snapshot()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);

        let created = service
            .create(CreateScheduleRequest {
                name: Some("snapshot-update".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() + Duration::minutes(1),
                    every_seconds: 60,
                    end_at_utc: None,
                }),
                target: materialize_on_demand_target("initial prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: crate::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;

        let error = service
            .update(
                &created.schedule_id,
                UpdateScheduleRequest {
                    expected_revision: Some(created.revision),
                    target: Some(mob_helper_target(HelperOptionsSpec {
                        resolved_spawn_snapshot: Some(resolved_spawn_snapshot_fixture()),
                        ..HelperOptionsSpec::default()
                    })),
                    ..UpdateScheduleRequest::default()
                },
            )
            .await
            .expect_err("untrusted helper snapshot update should be rejected");

        assert!(matches!(
            error,
            ScheduleDomainError::InvalidSchedule(message)
                if message.contains("trusted internal schedule state")
        ));
        Ok(())
    }

    #[tokio::test]
    async fn update_bumps_revision_and_supersedes_pending_future_occurrences()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);

        let created = service
            .create(CreateScheduleRequest {
                name: Some("nightly".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() + Duration::minutes(1),
                    every_seconds: 60,
                    end_at_utc: None,
                }),
                target: materialize_on_demand_target("initial prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: crate::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(4),
            })
            .await?;

        let updated = service
            .update(
                &created.schedule_id,
                UpdateScheduleRequest {
                    expected_revision: Some(created.revision),
                    trigger: Some(TriggerSpec::Interval(IntervalTriggerSpec {
                        start_at_utc: Utc::now() + Duration::minutes(2),
                        every_seconds: 120,
                        end_at_utc: None,
                    })),
                    ..UpdateScheduleRequest::default()
                },
            )
            .await?;

        let occurrences = service.list_occurrences(&created.schedule_id).await?;

        let superseded = occurrences
            .iter()
            .filter(|occurrence| {
                occurrence.phase == OccurrencePhase::Superseded
                    && occurrence.schedule_revision == created.revision
            })
            .count();
        let replanned = occurrences
            .iter()
            .filter(|occurrence| {
                occurrence.phase == OccurrencePhase::Pending
                    && occurrence.schedule_revision == updated.revision
            })
            .count();

        assert_eq!(updated.revision, created.revision.next());
        assert!(
            superseded > 0,
            "revision bump should supersede prior pending future occurrences"
        );
        assert!(
            occurrences
                .iter()
                .filter(|occurrence| {
                    occurrence.phase == OccurrencePhase::Superseded
                        && occurrence.schedule_revision == created.revision
                })
                .all(|occurrence| updated
                    .superseded_ack_ids
                    .contains(&occurrence.occurrence_id)),
            "supersession acks should be routed back through schedule authority"
        );
        assert!(
            replanned > 0,
            "revision bump should plan replacement pending occurrences"
        );
        Ok(())
    }

    #[tokio::test]
    async fn update_supersedes_overdue_pending_occurrences_from_prior_revision()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);

        let created = service
            .create(CreateScheduleRequest {
                name: Some("catch-up".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() - Duration::minutes(2),
                    every_seconds: 60,
                    end_at_utc: None,
                }),
                target: materialize_on_demand_target("initial prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: crate::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(4),
            })
            .await?;

        let created_occurrences = service.list_occurrences(&created.schedule_id).await?;
        assert!(
            created_occurrences.iter().any(|occurrence| {
                occurrence.schedule_revision == created.revision
                    && occurrence.phase == OccurrencePhase::Pending
                    && occurrence.due_at_utc < Utc::now()
            }),
            "fixture should include an overdue pending occurrence"
        );

        let updated = service
            .update(
                &created.schedule_id,
                UpdateScheduleRequest {
                    expected_revision: Some(created.revision),
                    trigger: Some(TriggerSpec::Interval(IntervalTriggerSpec {
                        start_at_utc: Utc::now() + Duration::minutes(5),
                        every_seconds: 300,
                        end_at_utc: None,
                    })),
                    ..UpdateScheduleRequest::default()
                },
            )
            .await?;

        let occurrences = service.list_occurrences(&created.schedule_id).await?;
        assert_eq!(
            occurrences
                .iter()
                .filter(|occurrence| {
                    occurrence.schedule_revision == created.revision
                        && occurrence.phase == OccurrencePhase::Pending
                })
                .count(),
            0,
            "older revisions must not retain overdue pending occurrences after update"
        );
        assert!(
            occurrences.iter().any(|occurrence| {
                occurrence.schedule_revision == created.revision
                    && occurrence.phase == OccurrencePhase::Superseded
                    && occurrence.due_at_utc < Utc::now()
            }),
            "revision bump should supersede overdue pending occurrences from the prior revision"
        );
        assert!(
            occurrences.iter().any(|occurrence| {
                occurrence.schedule_revision == updated.revision
                    && occurrence.phase == OccurrencePhase::Pending
            }),
            "updated revision should still have replacement pending occurrences"
        );
        Ok(())
    }

    #[tokio::test]
    async fn delete_bumps_revision_and_supersedes_pending_occurrences_against_deleted_revision()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);

        let created = service
            .create(CreateScheduleRequest {
                name: Some("delete-me".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() + Duration::minutes(1),
                    every_seconds: 60,
                    end_at_utc: None,
                }),
                target: materialize_on_demand_target("initial prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: crate::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(4),
            })
            .await?;

        let deleted = service.delete(&created.schedule_id).await?;
        let occurrences = service.list_occurrences(&created.schedule_id).await?;

        assert_eq!(
            deleted.revision,
            created.revision.next(),
            "delete should advance the schedule revision"
        );
        assert!(
            occurrences.iter().any(|occurrence| {
                occurrence.phase == OccurrencePhase::Superseded
                    && occurrence.schedule_revision == created.revision
                    && occurrence.superseded_by_revision == Some(deleted.revision)
            }),
            "delete should supersede pending occurrences against the new deleted revision"
        );
        assert!(
            occurrences
                .iter()
                .filter(|occurrence| {
                    occurrence.phase == OccurrencePhase::Superseded
                        && occurrence.schedule_revision == created.revision
                })
                .all(|occurrence| deleted
                    .superseded_ack_ids
                    .contains(&occurrence.occurrence_id)),
            "delete supersession acks should be routed back through schedule authority"
        );
        Ok(())
    }

    /// STAGE B (0.7.2 D1 — RED until wired): delete must revoke driver-claimed
    /// in-flight occurrences at commit time, not only Pending ones. The
    /// revocation flows through the occurrence authority's typed Supersede
    /// transition, and each revoked claim is accounted in the schedule
    /// authority's `superseded_ack_ids` (machine-owned accounting of the
    /// claims the delete revoked).
    #[tokio::test]
    async fn delete_revokes_and_accounts_driver_claimed_in_flight_occurrences()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());

        let created = service
            .create(CreateScheduleRequest {
                name: Some("delete-in-flight-accounting".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("initial prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: crate::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;

        let claimed = store
            .claim_due_occurrences(crate::ClaimDueRequest {
                owner_id: "driver-owner".into(),
                limit: 1,
                lease_duration: Duration::seconds(30),
            })
            .await?;
        let in_flight = claimed
            .claimed
            .into_iter()
            .next()
            .expect("due occurrence should be claimed");
        assert_eq!(in_flight.phase, OccurrencePhase::Claimed);

        let deleted = service.delete(&created.schedule_id).await?;

        let occurrences = service.list_occurrences(&created.schedule_id).await?;
        let revoked = occurrences
            .iter()
            .find(|occurrence| occurrence.occurrence_id == in_flight.occurrence_id)
            .expect("claimed occurrence should still exist");
        assert_eq!(
            revoked.phase,
            OccurrencePhase::Superseded,
            "delete must supersede the driver-claimed in-flight occurrence at commit time"
        );
        assert_eq!(revoked.superseded_by_revision, Some(deleted.revision));
        assert!(
            deleted
                .superseded_ack_ids
                .contains(&in_flight.occurrence_id),
            "the revoked in-flight claim must be accounted in superseded_ack_ids"
        );
        assert!(
            occurrences
                .iter()
                .all(|occurrence| occurrence.phase == OccurrencePhase::Superseded),
            "no live occurrence may outlive the deleted schedule"
        );
        Ok(())
    }

    #[tokio::test]
    async fn update_uses_atomic_store_mutation_for_replanning() -> Result<(), ScheduleDomainError> {
        let store = Arc::new(AtomicMutationProbeStore::new());
        let service = ScheduleService::new(store.clone() as Arc<dyn ScheduleStore>);

        let created = service
            .create(CreateScheduleRequest {
                name: Some("atomic-update".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() + Duration::minutes(1),
                    every_seconds: 60,
                    end_at_utc: None,
                }),
                target: materialize_on_demand_target("initial prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: crate::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(4),
            })
            .await?;
        let atomic_after_create = store.atomic_calls.load(Ordering::SeqCst);

        service
            .update(
                &created.schedule_id,
                UpdateScheduleRequest {
                    expected_revision: Some(created.revision),
                    trigger: Some(TriggerSpec::Interval(IntervalTriggerSpec {
                        start_at_utc: Utc::now() + Duration::minutes(2),
                        every_seconds: 120,
                        end_at_utc: None,
                    })),
                    ..UpdateScheduleRequest::default()
                },
            )
            .await?;

        assert!(
            store.atomic_calls.load(Ordering::SeqCst) > atomic_after_create,
            "update should route through the atomic schedule mutation seam"
        );
        assert_eq!(
            store.standalone_schedule_commits.load(Ordering::SeqCst),
            0,
            "update should not fall back to standalone schedule commits"
        );
        Ok(())
    }

    fn materialize_on_demand_target(prompt: &str) -> TargetBinding {
        TargetBinding::session(SessionTargetBinding::materialize_on_demand(
            SessionMaterializationSpec {
                model: "claude-sonnet-4-6".into(),
                system_prompt: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                provider_params: None,
                comms_name: Some("scheduled-worker".into()),
                peer_meta: None,
                labels: BTreeMap::new(),
                preload_skills: Vec::new(),
                additional_instructions: Vec::new(),
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                keep_alive: true,
                app_context: None,
            },
            ScheduledSessionAction::Prompt {
                prompt: ContentInput::from(prompt),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        ))
    }

    fn mob_helper_target(options: HelperOptionsSpec) -> TargetBinding {
        TargetBinding::Mob(Box::new(MobTargetBinding::SpawnHelper {
            mob_id: "ops".to_string(),
            member_id: "helper".to_string(),
            prompt: "check state".to_string(),
            options,
        }))
    }

    fn resolved_spawn_snapshot_fixture() -> ResolvedSpawnSnapshot {
        ResolvedSpawnSnapshot {
            tool_filter: meerkat_core::tool_scope::ToolFilter::Allow(ToolNameSet::from_iter([
                "shell".to_string(),
            ])),
            tool_filter_witnesses: Default::default(),
        }
    }

    #[test]
    fn materialize_on_demand_target_uses_current_fixture_model() {
        let target = materialize_on_demand_target("scheduled prompt");
        let spec = if let TargetBinding::Session(binding) = target {
            if let SessionTargetBinding::MaterializeOnDemandSession { create, .. } = *binding {
                create
            } else {
                return;
            }
        } else {
            return;
        };

        assert_eq!(spec.model, "claude-sonnet-4-6");
    }
}
