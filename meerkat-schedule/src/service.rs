use crate::authority::{ScheduleLifecycleAuthority, ScheduleLifecycleInput};
use crate::error::{ScheduleDomainError, ScheduleStoreError};
use crate::store::{OccurrenceFilter, PendingSupersession, ScheduleFilter, ScheduleStore};
use crate::trigger::occurrences_for_horizon;
use crate::types::{
    CreateScheduleRequest, Occurrence, OccurrencePhase, Schedule, ScheduleId, SchedulePhase,
    UpdateScheduleRequest,
};
use chrono::{Duration, Utc};
use meerkat_core::SessionId;
use std::collections::BTreeSet;
use std::sync::Arc;

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct ScheduleService {
    store: Arc<dyn ScheduleStore>,
    schedule_authority: Arc<ScheduleLifecycleAuthority>,
    planning_lock: Arc<Mutex<()>>,
}

impl ScheduleService {
    pub fn new(store: Arc<dyn ScheduleStore>) -> Self {
        Self {
            store,
            schedule_authority: Arc::new(ScheduleLifecycleAuthority),
            planning_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn store(&self) -> Arc<dyn ScheduleStore> {
        self.store.clone()
    }

    pub async fn create(
        &self,
        request: CreateScheduleRequest,
    ) -> Result<Schedule, ScheduleDomainError> {
        let _planning_guard = self.planning_lock.lock().await;
        let mut mutator = self
            .schedule_authority
            .apply(None, ScheduleLifecycleInput::Create(request))
            .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;
        let store_now = self.store.get_store_time_utc().await?;
        let planned = self
            .plan_schedule_occurrences(&mut mutator.schedule, store_now)
            .await?;
        self.store
            .commit_schedule_mutation(mutator.schedule.clone(), planned, None)
            .await?;
        Ok(mutator.schedule)
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

    pub async fn update(
        &self,
        schedule_id: &ScheduleId,
        request: UpdateScheduleRequest,
    ) -> Result<Schedule, ScheduleDomainError> {
        let _planning_guard = self.planning_lock.lock().await;
        let current = self.get(schedule_id).await?;
        let mut mutator = self
            .schedule_authority
            .apply(Some(current), ScheduleLifecycleInput::Update(request))
            .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;
        let store_now = self.store.get_store_time_utc().await?;

        let planned = self
            .plan_schedule_occurrences(&mut mutator.schedule, store_now)
            .await?;
        self.store
            .commit_schedule_mutation(
                mutator.schedule.clone(),
                planned,
                mutator.revision_bumped.then_some(PendingSupersession {
                    at_utc: store_now,
                    superseded_by_revision: mutator.schedule.revision,
                }),
            )
            .await?;
        Ok(mutator.schedule)
    }

    pub async fn pause(&self, schedule_id: &ScheduleId) -> Result<Schedule, ScheduleDomainError> {
        let _planning_guard = self.planning_lock.lock().await;
        let current = self.get(schedule_id).await?;
        let mutator = self
            .schedule_authority
            .apply(
                Some(current),
                ScheduleLifecycleInput::Pause {
                    at_utc: self.store.get_store_time_utc().await?,
                },
            )
            .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;
        self.store.put_schedule(mutator.schedule.clone()).await?;
        Ok(mutator.schedule)
    }

    pub async fn resume(&self, schedule_id: &ScheduleId) -> Result<Schedule, ScheduleDomainError> {
        let _planning_guard = self.planning_lock.lock().await;
        let current = self.get(schedule_id).await?;
        let mut mutator = self
            .schedule_authority
            .apply(
                Some(current),
                ScheduleLifecycleInput::Resume {
                    at_utc: self.store.get_store_time_utc().await?,
                },
            )
            .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;
        let store_now = self.store.get_store_time_utc().await?;
        let planned = self
            .plan_schedule_occurrences(&mut mutator.schedule, store_now)
            .await?;
        self.store
            .commit_schedule_mutation(mutator.schedule.clone(), planned, None)
            .await?;
        Ok(mutator.schedule)
    }

    pub async fn delete(&self, schedule_id: &ScheduleId) -> Result<Schedule, ScheduleDomainError> {
        let _planning_guard = self.planning_lock.lock().await;
        let current = self.get(schedule_id).await?;
        let store_now = self.store.get_store_time_utc().await?;
        let mutator = self
            .schedule_authority
            .apply(
                Some(current),
                ScheduleLifecycleInput::Delete { at_utc: store_now },
            )
            .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;
        let deleted = mutator.schedule.clone();
        self.store
            .commit_schedule_mutation(
                deleted.clone(),
                Vec::new(),
                Some(PendingSupersession {
                    at_utc: store_now,
                    superseded_by_revision: deleted.revision,
                }),
            )
            .await?;
        Ok(deleted)
    }

    pub async fn list_occurrences(
        &self,
        schedule_id: &ScheduleId,
    ) -> Result<Vec<Occurrence>, ScheduleDomainError> {
        self.store
            .list_occurrences(OccurrenceFilter {
                schedule_id: Some(schedule_id.clone()),
                include_terminal: true,
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
        let mut schedule = self.get(schedule_id).await?;
        let store_now = self.store.get_store_time_utc().await?;
        let planned = self
            .plan_schedule_occurrences(&mut schedule, store_now)
            .await?;
        if !planned.is_empty() {
            self.store
                .commit_schedule_mutation(schedule, planned.clone(), None)
                .await?;
        }
        Ok(planned)
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
        occurrence.target_snapshot = current.target.clone();
        self.store.put_occurrence(occurrence.clone()).await?;
        Ok(occurrence)
    }

    pub async fn bind_materialized_session_for_occurrence(
        &self,
        occurrence: &Occurrence,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        let Some(mut schedule) = self.store.get_schedule(&occurrence.schedule_id).await? else {
            return Ok(());
        };
        if schedule.revision != occurrence.schedule_revision {
            return Ok(());
        }

        let schedule_changed = schedule.target.bind_materialized_session(session_id);
        if schedule_changed {
            schedule.touch();
            self.store.put_schedule(schedule).await?;
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
        for mut pending_occurrence in pending {
            if pending_occurrence.schedule_revision != occurrence.schedule_revision {
                continue;
            }
            if pending_occurrence
                .target_snapshot
                .bind_materialized_session(session_id)
            {
                updated_pending.push(pending_occurrence);
            }
        }

        if !updated_pending.is_empty() {
            self.store.put_occurrences(updated_pending).await?;
        }

        Ok(())
    }

    async fn plan_schedule_occurrences(
        &self,
        schedule: &mut Schedule,
        store_now_utc: chrono::DateTime<Utc>,
    ) -> Result<Vec<Occurrence>, ScheduleDomainError> {
        if schedule.phase != SchedulePhase::Active {
            return Ok(Vec::new());
        }

        let horizon_end_utc =
            store_now_utc + Duration::days(i64::from(schedule.planning_horizon_days));
        let existing = self
            .store
            .list_occurrences(OccurrenceFilter {
                schedule_id: Some(schedule.schedule_id.clone()),
                include_terminal: false,
                phase: Some(OccurrencePhase::Pending),
                ..OccurrenceFilter::default()
            })
            .await?;

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
            usize::try_from(schedule.planning_horizon_occurrences).unwrap_or(usize::MAX);
        if future_pending_count >= desired_count {
            return Ok(Vec::new());
        }

        let remaining = desired_count.saturating_sub(future_pending_count);
        let cursor = existing
            .iter()
            .filter(|occurrence| occurrence.schedule_revision == schedule.revision)
            .map(|occurrence| occurrence.due_at_utc)
            .max()
            .or(schedule.planning_cursor_utc)
            .unwrap_or_else(|| schedule.updated_at_utc - Duration::minutes(1));

        let due_times = occurrences_for_horizon(
            &schedule.trigger,
            Some(cursor),
            horizon_end_utc,
            remaining.saturating_add(existing_due.len()),
        )?;

        let mut planned = Vec::new();
        for due_at_utc in due_times {
            if existing_due.contains(&due_at_utc) {
                continue;
            }
            let occurrence = Occurrence::planned_from_schedule(
                schedule,
                schedule.next_occurrence_ordinal,
                due_at_utc,
            );
            schedule.next_occurrence_ordinal = schedule.next_occurrence_ordinal.next();
            planned.push(occurrence);
            if planned.len() >= remaining {
                break;
            }
        }

        if !planned.is_empty() {
            let Some(planning_cursor_utc) = planned.last().map(|occurrence| occurrence.due_at_utc)
            else {
                return Ok(planned);
            };
            let mutator = self
                .schedule_authority
                .apply(
                    Some(schedule.clone()),
                    ScheduleLifecycleInput::RecordPlanningWindow {
                        planning_cursor_utc,
                        next_occurrence_ordinal: schedule.next_occurrence_ordinal,
                    },
                )
                .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
            *schedule = mutator.schedule;
            schedule.touch();
        }

        Ok(planned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OccurrenceLifecycleInput;
    use crate::types::{
        DeliveryReceipt, IntervalTriggerSpec, MisfirePolicy, OccurrenceId, ScheduledSessionAction,
        SessionMaterializationSpec, SessionTargetBinding, TargetBinding, TriggerSpec,
    };
    use crate::{MemoryScheduleStore, OverlapPolicy};
    use chrono::Duration;
    use meerkat_core::ContentInput;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    struct AtomicMutationProbeStore {
        inner: Arc<dyn ScheduleStore>,
        atomic_calls: AtomicUsize,
        direct_schedule_writes: AtomicUsize,
    }

    impl AtomicMutationProbeStore {
        fn new() -> Self {
            Self {
                inner: Arc::new(MemoryScheduleStore::new()),
                atomic_calls: AtomicUsize::new(0),
                direct_schedule_writes: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ScheduleStore for AtomicMutationProbeStore {
        fn kind(&self) -> crate::ScheduleStoreKind {
            self.inner.kind()
        }

        async fn get_store_time_utc(&self) -> Result<chrono::DateTime<Utc>, ScheduleStoreError> {
            self.inner.get_store_time_utc().await
        }

        async fn put_schedule(&self, schedule: Schedule) -> Result<(), ScheduleStoreError> {
            self.direct_schedule_writes.fetch_add(1, Ordering::SeqCst);
            self.inner.put_schedule(schedule).await
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

        async fn put_occurrence(&self, occurrence: Occurrence) -> Result<(), ScheduleStoreError> {
            self.inner.put_occurrence(occurrence).await
        }

        async fn put_occurrences(
            &self,
            occurrences: Vec<Occurrence>,
        ) -> Result<(), ScheduleStoreError> {
            self.inner.put_occurrences(occurrences).await
        }

        async fn commit_schedule_mutation(
            &self,
            schedule: Schedule,
            occurrences: Vec<Occurrence>,
            supersession: Option<PendingSupersession>,
        ) -> Result<(), ScheduleStoreError> {
            self.atomic_calls.fetch_add(1, Ordering::SeqCst);
            self.inner
                .commit_schedule_mutation(schedule, occurrences, supersession)
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

        async fn transition_occurrence_if_current(
            &self,
            occurrence_id: &OccurrenceId,
            expected_attempt: u32,
            expected_claim_token: Option<Uuid>,
            transition: OccurrenceLifecycleInput,
        ) -> Result<Option<Occurrence>, ScheduleStoreError> {
            self.inner
                .transition_occurrence_if_current(
                    occurrence_id,
                    expected_attempt,
                    expected_claim_token,
                    transition,
                )
                .await
        }
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
            store.direct_schedule_writes.load(Ordering::SeqCst),
            0,
            "update should not fall back to piecemeal put_schedule writes"
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
                structured_output_retries: 0,
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
                skill_references: Vec::new(),
                additional_instructions: Vec::new(),
            },
        ))
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
