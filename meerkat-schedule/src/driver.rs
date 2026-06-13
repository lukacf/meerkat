use crate::error::{ScheduleDomainError, ScheduleStoreError};
use crate::lifecycle::{
    ClaimedDispatchDisposition, CompletionSupersessionDisposition, OccurrenceLifecycleInput,
};
use crate::service::ScheduleService;
use crate::store::{ClaimDueRequest, ScheduleStore};
use crate::types::{
    DeliveryCompletionFailureReason, DeliveryFailureReason, DeliveryReceipt, Occurrence,
    OccurrencePhase, OccurrenceTargetProbeOutcome, RuntimeCompletionOutcome,
    RuntimeDeliveryOutcome, SchedulePhase,
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use futures::Future;
use meerkat_core::SessionId;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

pub type DeliveryCompletion =
    Pin<Box<dyn Future<Output = Result<DeliveryTerminal, ScheduleDomainError>> + Send + 'static>>;

pub struct DeliveryDispatch {
    pub receipt: DeliveryReceipt,
    pub correlation_id: Option<String>,
    pub materialized_session_id: Option<SessionId>,
    pub completion: DeliveryCompletion,
}

impl fmt::Debug for DeliveryDispatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeliveryDispatch")
            .field("receipt", &self.receipt)
            .field("correlation_id", &self.correlation_id)
            .field("materialized_session_id", &self.materialized_session_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct DeliveryTerminal {
    pub phase: OccurrencePhase,
    pub receipt: Option<DeliveryReceipt>,
    pub detail: Option<String>,
    pub delivery_failure_reason: Option<DeliveryFailureReason>,
    pub runtime_completion_outcome: Option<RuntimeCompletionOutcome>,
    pub runtime_outcome: Option<RuntimeDeliveryOutcome>,
}

impl DeliveryTerminal {
    pub fn completed(receipt: Option<DeliveryReceipt>) -> Self {
        Self {
            phase: OccurrencePhase::Completed,
            receipt,
            detail: None,
            delivery_failure_reason: None,
            runtime_completion_outcome: None,
            runtime_outcome: None,
        }
    }

    pub fn delivery_failed(detail: impl Into<String>, reason: DeliveryFailureReason) -> Self {
        Self {
            phase: OccurrencePhase::DeliveryFailed,
            receipt: None,
            detail: Some(detail.into()),
            delivery_failure_reason: Some(reason),
            runtime_completion_outcome: None,
            runtime_outcome: None,
        }
    }

    pub fn runtime_completion(
        outcome: RuntimeCompletionOutcome,
        detail: Option<String>,
        runtime_outcome: Option<RuntimeDeliveryOutcome>,
    ) -> Self {
        Self {
            phase: OccurrencePhase::AwaitingCompletion,
            receipt: None,
            detail,
            delivery_failure_reason: None,
            runtime_completion_outcome: Some(outcome),
            runtime_outcome,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TargetProbeOutcome {
    Ready,
    Busy { detail: Option<String> },
    Missing { detail: Option<String> },
}

#[async_trait]
pub trait ScheduleTargetProbe: Send + Sync {
    async fn probe_target(
        &self,
        occurrence: &Occurrence,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError>;
}

#[async_trait]
pub trait ScheduleTargetDelivery: Send + Sync {
    async fn deliver_occurrence(
        &self,
        occurrence: &Occurrence,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;
}

#[derive(Debug, Clone)]
pub struct ScheduleDriverConfig {
    pub claim_limit: usize,
    pub lease_duration: Duration,
}

impl Default for ScheduleDriverConfig {
    fn default() -> Self {
        Self {
            claim_limit: 32,
            lease_duration: Duration::seconds(60),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScheduleTickReport {
    pub planned_occurrences: usize,
    pub claimed_occurrences: usize,
    pub terminalized_occurrences: usize,
}

enum ClaimedOccurrenceDispatchState {
    Ready(Occurrence),
    Frozen,
    Supersede {
        occurrence: Occurrence,
        superseded_by_revision: crate::ScheduleRevision,
    },
}

enum TargetProbeResolution {
    Continue(Box<Occurrence>),
    Terminalized,
    StaleClaim,
}

pub struct ScheduleDriver {
    service: ScheduleService,
    store: Arc<dyn ScheduleStore>,
    probe: Arc<dyn ScheduleTargetProbe>,
    delivery: Arc<dyn ScheduleTargetDelivery>,
    owner_id: String,
    config: ScheduleDriverConfig,
}

impl ScheduleDriver {
    pub fn new(
        service: ScheduleService,
        store: Arc<dyn ScheduleStore>,
        probe: Arc<dyn ScheduleTargetProbe>,
        delivery: Arc<dyn ScheduleTargetDelivery>,
        owner_id: impl Into<String>,
        config: ScheduleDriverConfig,
    ) -> Self {
        Self {
            service,
            store,
            probe,
            delivery,
            owner_id: owner_id.into(),
            config,
        }
    }

    pub async fn tick_once(&self) -> Result<ScheduleTickReport, ScheduleDomainError> {
        let mut report = ScheduleTickReport::default();

        for schedule in self.service.list().await? {
            if schedule.phase != SchedulePhase::Active {
                continue;
            }
            let planned = self.service.refill_horizon(&schedule.schedule_id).await?;
            report.planned_occurrences += planned.len();
        }

        let claimed = self
            .store
            .claim_due_occurrences(ClaimDueRequest {
                owner_id: self.owner_id.clone(),
                limit: self.config.claim_limit,
                lease_duration: self.config.lease_duration,
            })
            .await?;
        report.claimed_occurrences = claimed.claimed.len();

        for occurrence in claimed.claimed {
            if self
                .handle_claimed_occurrence(occurrence, claimed.store_now_utc)
                .await?
            {
                report.terminalized_occurrences += 1;
            }
        }

        Ok(report)
    }

    async fn handle_claimed_occurrence(
        &self,
        occurrence: Occurrence,
        store_now_utc: chrono::DateTime<Utc>,
    ) -> Result<bool, ScheduleDomainError> {
        let frozen_occurrence = occurrence.clone();
        let occurrence = match self
            .reconcile_claimed_occurrence_before_dispatch(occurrence)
            .await?
        {
            ClaimedOccurrenceDispatchState::Ready(occurrence) => occurrence,
            ClaimedOccurrenceDispatchState::Frozen => {
                let released = self
                    .store
                    .transition_occurrence_if_current(
                        &frozen_occurrence.occurrence_id,
                        frozen_occurrence.attempt_count,
                        frozen_occurrence.claim_token(),
                        OccurrenceLifecycleInput::ReleaseLeaseForPausedSchedule {
                            at_utc: store_now_utc,
                        },
                    )
                    .await?;
                if let Some(released) = released {
                    let receipt = released
                        .delivery_receipt_from_authority(None)
                        .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
                    self.store.append_receipt(receipt).await?;
                }
                return Ok(false);
            }
            ClaimedOccurrenceDispatchState::Supersede {
                occurrence,
                superseded_by_revision,
            } => {
                self.terminalize_occurrence(
                    occurrence,
                    OccurrenceLifecycleInput::Supersede {
                        superseded_by_revision,
                        at_utc: store_now_utc,
                    },
                    None,
                )
                .await?;
                return Ok(true);
            }
        };

        let mut occurrence = match self.resolve_target_probe(occurrence, store_now_utc).await? {
            TargetProbeResolution::Continue(occurrence) => *occurrence,
            TargetProbeResolution::Terminalized => return Ok(true),
            TargetProbeResolution::StaleClaim => return Ok(false),
        };

        let dispatch = match self.delivery.deliver_occurrence(&occurrence).await {
            Ok(dispatch) => dispatch,
            Err(error) => {
                let detail = error.to_string();
                self.terminalize_occurrence(
                    occurrence,
                    OccurrenceLifecycleInput::ResolveDeliveryFailure {
                        reason: DeliveryFailureReason::TransportError,
                        detail: Some(detail),
                        at_utc: store_now_utc,
                    },
                    None,
                )
                .await?;
                return Ok(true);
            }
        };

        if let Some(materialized_session_id) = dispatch.materialized_session_id.clone() {
            self.service
                .bind_materialized_session_for_occurrence(&occurrence, &materialized_session_id)
                .await?;
            occurrence = self
                .service
                .sync_occurrence_target_with_schedule(occurrence)
                .await?;
        }

        let dispatch_mutator = occurrence
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: dispatch.correlation_id.clone(),
                at_utc: store_now_utc,
            })
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        let dispatching = dispatch_mutator.occurrence.clone();
        // The delivery adapter's receipt is transport metadata; the recorded
        // public receipt class is projected from OccurrenceLifecycleMachine.
        let dispatch_receipt = dispatching
            .delivery_receipt_from_authority(dispatch.receipt.runtime_outcome.clone())
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        self.store
            .commit_occurrence_write(dispatch_mutator.into_authorized_write())
            .await?;
        self.store.append_receipt(dispatch_receipt).await?;

        let dispatching = self
            .store
            .get_occurrence(&dispatching.occurrence_id)
            .await?
            .ok_or_else(|| ScheduleStoreError::OccurrenceNotFound {
                occurrence_id: dispatching.occurrence_id.clone(),
            })?;
        let await_mutator = dispatching
            .apply(OccurrenceLifecycleInput::AwaitCompletion {
                at_utc: store_now_utc,
            })
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        let dispatching = await_mutator.occurrence.clone();
        self.store
            .commit_occurrence_write(await_mutator.into_authorized_write())
            .await?;

        self.spawn_completion_waiter(dispatching, dispatch.completion);
        Ok(false)
    }

    async fn reconcile_claimed_occurrence_before_dispatch(
        &self,
        occurrence: Occurrence,
    ) -> Result<ClaimedOccurrenceDispatchState, ScheduleDomainError> {
        let current = match self.service.get(&occurrence.schedule_id).await {
            Ok(schedule) => schedule,
            Err(ScheduleDomainError::Store(crate::ScheduleStoreError::ScheduleNotFound {
                ..
            })) => {
                return Err(ScheduleDomainError::Internal(format!(
                    "claimed occurrence references missing schedule {}",
                    occurrence.schedule_id
                )));
            }
            Err(error) => return Err(error),
        };

        // The schedule's current phase and revision are pure observations. The
        // OccurrenceLifecycleMachine — not this driver — classifies the
        // pre-dispatch disposition; we mirror the emitted verdict and fail
        // closed if no disposition is emitted.
        let verdict = occurrence
            .classify_claimed_dispatch_disposition(current.phase, current.revision)
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;

        match verdict.disposition {
            ClaimedDispatchDisposition::FutureRevision => {
                Err(ScheduleDomainError::Internal(format!(
                    "claimed occurrence {} has future revision {} ahead of schedule {}",
                    occurrence.occurrence_id, occurrence.schedule_revision.0, current.revision.0
                )))
            }
            ClaimedDispatchDisposition::Frozen => Ok(ClaimedOccurrenceDispatchState::Frozen),
            ClaimedDispatchDisposition::Supersede => {
                let superseded_by_revision = verdict.superseded_by_revision.ok_or_else(|| {
                    ScheduleDomainError::Internal(
                        "occurrence authority classified Supersede without a superseding revision"
                            .to_string(),
                    )
                })?;
                Ok(ClaimedOccurrenceDispatchState::Supersede {
                    occurrence,
                    superseded_by_revision,
                })
            }
            ClaimedDispatchDisposition::Ready => {
                let occurrence = self
                    .service
                    .sync_occurrence_target_with_schedule(occurrence)
                    .await?;
                Ok(ClaimedOccurrenceDispatchState::Ready(occurrence))
            }
        }
    }

    async fn terminalize_occurrence(
        &self,
        occurrence: Occurrence,
        lifecycle: OccurrenceLifecycleInput,
        receipt: Option<DeliveryReceipt>,
    ) -> Result<(), ScheduleDomainError> {
        let _ =
            terminalize_occurrence_inner(self.store.clone(), occurrence, lifecycle, receipt, None)
                .await?;
        Ok(())
    }

    async fn resolve_target_probe(
        &self,
        occurrence: Occurrence,
        store_now_utc: DateTime<Utc>,
    ) -> Result<TargetProbeResolution, ScheduleDomainError> {
        let (outcome, detail) = match self.probe.probe_target(&occurrence).await? {
            TargetProbeOutcome::Ready => (OccurrenceTargetProbeOutcome::Ready, None),
            TargetProbeOutcome::Busy { detail } => (
                OccurrenceTargetProbeOutcome::Busy,
                detail.or_else(|| Some("target busy".to_string())),
            ),
            TargetProbeOutcome::Missing { detail } => (
                OccurrenceTargetProbeOutcome::Missing,
                detail.or_else(|| Some("target missing".to_string())),
            ),
        };

        let lifecycle = OccurrenceLifecycleInput::ResolveTargetProbe {
            outcome,
            detail,
            at_utc: store_now_utc,
        };
        let predicted = occurrence
            .clone()
            .apply(lifecycle.clone())
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?
            .into_occurrence();
        let updated = match predicted.phase {
            OccurrencePhase::Claimed => {
                self.store
                    .transition_occurrence_if_current(
                        &occurrence.occurrence_id,
                        occurrence.attempt_count,
                        occurrence.claim_token(),
                        lifecycle,
                    )
                    .await?
            }
            OccurrencePhase::Skipped | OccurrencePhase::Misfired => {
                self.store
                    .transition_occurrence_with_receipt_if_current(
                        &occurrence.occurrence_id,
                        occurrence.attempt_count,
                        occurrence.claim_token(),
                        lifecycle,
                        None,
                    )
                    .await?
            }
            other => {
                return Err(ScheduleDomainError::Internal(format!(
                    "generated occurrence authority resolved target probe to unsupported phase: {other:?}"
                )));
            }
        };
        let Some(updated) = updated else {
            return Ok(TargetProbeResolution::StaleClaim);
        };

        match updated.phase {
            OccurrencePhase::Claimed => Ok(TargetProbeResolution::Continue(Box::new(updated))),
            OccurrencePhase::Skipped | OccurrencePhase::Misfired => {
                Ok(TargetProbeResolution::Terminalized)
            }
            other => Err(ScheduleDomainError::Internal(format!(
                "generated occurrence authority resolved target probe to unsupported phase: {other:?}"
            ))),
        }
    }

    fn spawn_completion_waiter(&self, occurrence: Occurrence, completion: DeliveryCompletion) {
        let store = self.store.clone();
        let schedule_id = occurrence.schedule_id.clone();
        let occurrence_id = occurrence.occurrence_id.clone();
        crate::tokio::spawn(async move {
            if let Err(error) =
                complete_dispatched_occurrence(store, occurrence, completion.await).await
            {
                eprintln!(
                    "schedule completion waiter failed to append terminal receipt: schedule_id={schedule_id:?} occurrence_id={occurrence_id:?}: {error}"
                );
            }
        });
    }
}

async fn complete_dispatched_occurrence(
    store: Arc<dyn ScheduleStore>,
    occurrence: Occurrence,
    completion: Result<DeliveryTerminal, ScheduleDomainError>,
) -> Result<(), ScheduleDomainError> {
    let store_now_utc = store.get_store_time_utc().await?;
    let current_schedule = store.get_schedule(&occurrence.schedule_id).await?;
    if let Some(schedule) = current_schedule {
        // The schedule's current phase and revision are pure observations. The
        // OccurrenceLifecycleMachine — not this driver — classifies whether the
        // completed delivery is superseded; we mirror the emitted verdict and
        // fail closed if no disposition is emitted.
        let verdict = occurrence
            .classify_completion_supersession(schedule.phase, schedule.revision)
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        match verdict.disposition {
            CompletionSupersessionDisposition::Supersede => {
                let superseded_by_revision = verdict.superseded_by_revision.ok_or_else(|| {
                    ScheduleDomainError::Internal(
                        "occurrence authority classified completion Supersede without a superseding revision"
                            .to_string(),
                    )
                })?;
                let _ = terminalize_occurrence_inner(
                    store,
                    occurrence,
                    OccurrenceLifecycleInput::Supersede {
                        superseded_by_revision,
                        at_utc: store_now_utc,
                    },
                    None,
                    None,
                )
                .await?;
                return Ok(());
            }
            CompletionSupersessionDisposition::Proceed => {}
            // 0.7.2 D2a: the occurrence snapshot is already Superseded (the
            // schedule-commit supersession sweep landed between dispatch and
            // completion). Fall through: the delivery resolution below lands
            // on the occurrence authority's late-arrival transitions as a
            // typed record, never a guard rejection.
            CompletionSupersessionDisposition::AlreadySuperseded => {}
        }
    }

    let terminal = match completion {
        Ok(terminal) => terminal,
        Err(error) => {
            let (reason, detail) = delivery_completion_failure_evidence(error);
            let _ = terminalize_occurrence_inner(
                store,
                occurrence,
                OccurrenceLifecycleInput::ResolveDeliveryCompletionFailure {
                    reason,
                    detail,
                    at_utc: store_now_utc,
                },
                None,
                None,
            )
            .await?;
            return Ok(());
        }
    };

    let lifecycle = if let Some(outcome) = terminal.runtime_completion_outcome {
        OccurrenceLifecycleInput::ResolveRuntimeCompletion {
            outcome,
            detail: terminal.detail.clone(),
            at_utc: store_now_utc,
        }
    } else {
        match terminal.phase {
            OccurrencePhase::Completed => OccurrenceLifecycleInput::Complete {
                at_utc: store_now_utc,
            },
            OccurrencePhase::Skipped | OccurrencePhase::Misfired => {
                return Err(ScheduleDomainError::Internal(format!(
                    "delivery terminal returned unsupported adapter-selected occurrence phase: {:?}",
                    terminal.phase
                )));
            }
            OccurrencePhase::DeliveryFailed => OccurrenceLifecycleInput::ResolveDeliveryFailure {
                reason: terminal.delivery_failure_reason.ok_or_else(|| {
                    ScheduleDomainError::Internal(
                        "delivery failed terminal omitted generated failure reason".to_string(),
                    )
                })?,
                detail: terminal.detail.clone(),
                at_utc: store_now_utc,
            },
            other => {
                return Err(ScheduleDomainError::Internal(format!(
                    "delivery terminal returned non-terminal occurrence phase: {other:?}"
                )));
            }
        }
    };

    let _ = terminalize_occurrence_inner(
        store,
        occurrence,
        lifecycle,
        terminal.receipt,
        terminal.runtime_outcome,
    )
    .await?;
    Ok(())
}

fn delivery_completion_failure_evidence(
    error: ScheduleDomainError,
) -> (DeliveryCompletionFailureReason, Option<String>) {
    match error {
        ScheduleDomainError::DeliveryCompletionFailed { reason, detail } => (reason, Some(detail)),
        other => (
            DeliveryCompletionFailureReason::CompletionFutureFailed,
            Some(other.to_string()),
        ),
    }
}

async fn terminalize_occurrence_inner(
    store: Arc<dyn ScheduleStore>,
    occurrence: Occurrence,
    lifecycle: OccurrenceLifecycleInput,
    _receipt: Option<DeliveryReceipt>,
    runtime_outcome: Option<RuntimeDeliveryOutcome>,
) -> Result<bool, ScheduleDomainError> {
    store
        .transition_occurrence_with_receipt_if_current(
            &occurrence.occurrence_id,
            occurrence.attempt_count,
            occurrence.claim_token(),
            lifecycle,
            runtime_outcome,
        )
        .await
        .map(|updated| updated.is_some())
        .map_err(ScheduleDomainError::from)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::large_futures, clippy::panic)]

    use super::*;
    use crate::types::{
        CreateScheduleRequest, DeliveryReceiptStage, IntervalTriggerSpec, OccurrenceFailureClass,
        ScheduledSessionAction, SessionMaterializationSpec, SessionTargetBinding, TargetBinding,
    };
    use crate::{
        MemoryScheduleStore, MisfirePolicy, MissingTargetPolicy, OverlapPolicy, TriggerSpec,
        UpdateScheduleRequest,
    };
    use chrono::Duration;
    use meerkat_core::ContentInput;
    use std::collections::BTreeMap;
    use tokio::sync::{Mutex, oneshot};
    use tokio::time::sleep;
    use uuid::Uuid;

    struct ReadyProbe;

    #[async_trait]
    impl ScheduleTargetProbe for ReadyProbe {
        async fn probe_target(
            &self,
            _occurrence: &Occurrence,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            Ok(TargetProbeOutcome::Ready)
        }
    }

    struct StaticProbe(TargetProbeOutcome);

    #[async_trait]
    impl ScheduleTargetProbe for StaticProbe {
        async fn probe_target(
            &self,
            _occurrence: &Occurrence,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            Ok(self.0.clone())
        }
    }

    struct MaterializationFailureDelivery;

    #[async_trait]
    impl ScheduleTargetDelivery for MaterializationFailureDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            let receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchStarted,
            );
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some("dispatch-correlation".into()),
                materialized_session_id: None,
                completion: Box::pin(async {
                    Ok(DeliveryTerminal {
                        phase: OccurrencePhase::DeliveryFailed,
                        receipt: None,
                        detail: Some("session creation failed".into()),
                        delivery_failure_reason: Some(
                            DeliveryFailureReason::TargetMaterializationFailed,
                        ),
                        runtime_completion_outcome: None,
                        runtime_outcome: None,
                    })
                }),
            })
        }
    }

    #[derive(Default)]
    struct CompletingDelivery {
        dispatched_occurrences: Arc<Mutex<Vec<crate::OccurrenceId>>>,
    }

    #[async_trait]
    impl ScheduleTargetDelivery for CompletingDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            self.dispatched_occurrences
                .lock()
                .await
                .push(occurrence.occurrence_id.clone());
            let receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchStarted,
            );
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some(format!("dispatch-attempt-{}", occurrence.attempt_count)),
                materialized_session_id: None,
                completion: Box::pin(async { Ok(DeliveryTerminal::completed(None)) }),
            })
        }
    }

    #[derive(Default)]
    struct ControlledCompletionDelivery {
        senders: Arc<Mutex<Vec<oneshot::Sender<DeliveryTerminal>>>>,
    }

    #[async_trait]
    impl ScheduleTargetDelivery for ControlledCompletionDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            let receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchStarted,
            );
            let (tx, rx) = oneshot::channel();
            self.senders.lock().await.push(tx);
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some(format!("dispatch-attempt-{}", occurrence.attempt_count)),
                materialized_session_id: None,
                completion: Box::pin(async move {
                    rx.await.map_err(|_| ScheduleDomainError::DriverStopped)
                }),
            })
        }
    }

    #[derive(Default)]
    struct CountingProbe {
        calls: Arc<Mutex<u32>>,
    }

    #[async_trait]
    impl ScheduleTargetProbe for CountingProbe {
        async fn probe_target(
            &self,
            _occurrence: &Occurrence,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            *self.calls.lock().await += 1;
            Ok(TargetProbeOutcome::Ready)
        }
    }

    #[derive(Default)]
    struct CountingDelivery {
        calls: Arc<Mutex<u32>>,
    }

    #[async_trait]
    impl ScheduleTargetDelivery for CountingDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            *self.calls.lock().await += 1;
            let receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchStarted,
            );
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some(format!("dispatch-attempt-{}", occurrence.attempt_count)),
                materialized_session_id: None,
                completion: Box::pin(async { Ok(DeliveryTerminal::completed(None)) }),
            })
        }
    }

    struct StandaloneReceiptFailingStore {
        inner: Arc<dyn ScheduleStore>,
    }

    #[async_trait]
    impl ScheduleStore for StandaloneReceiptFailingStore {
        fn kind(&self) -> crate::ScheduleStoreKind {
            self.inner.kind()
        }

        async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, ScheduleStoreError> {
            self.inner.get_store_time_utc().await
        }

        async fn commit_schedule_write(
            &self,
            write: crate::AuthorizedScheduleWrite,
        ) -> Result<(), ScheduleStoreError> {
            self.inner.commit_schedule_write(write).await
        }

        async fn get_schedule(
            &self,
            schedule_id: &crate::ScheduleId,
        ) -> Result<Option<crate::Schedule>, ScheduleStoreError> {
            self.inner.get_schedule(schedule_id).await
        }

        async fn list_schedules(
            &self,
            filter: crate::ScheduleFilter,
        ) -> Result<Vec<crate::Schedule>, ScheduleStoreError> {
            self.inner.list_schedules(filter).await
        }

        async fn commit_occurrence_write(
            &self,
            write: crate::AuthorizedOccurrenceWrite,
        ) -> Result<(), ScheduleStoreError> {
            self.inner.commit_occurrence_write(write).await
        }

        async fn commit_occurrence_writes(
            &self,
            writes: Vec<crate::AuthorizedOccurrenceWrite>,
        ) -> Result<(), ScheduleStoreError> {
            self.inner.commit_occurrence_writes(writes).await
        }

        async fn commit_schedule_mutation(
            &self,
            schedule: crate::AuthorizedScheduleWrite,
            occurrences: Vec<crate::AuthorizedOccurrenceWrite>,
        ) -> Result<crate::Schedule, ScheduleStoreError> {
            self.inner
                .commit_schedule_mutation(schedule, occurrences)
                .await
        }

        async fn get_occurrence(
            &self,
            occurrence_id: &crate::OccurrenceId,
        ) -> Result<Option<Occurrence>, ScheduleStoreError> {
            self.inner.get_occurrence(occurrence_id).await
        }

        async fn list_occurrences(
            &self,
            filter: crate::OccurrenceFilter,
        ) -> Result<Vec<Occurrence>, ScheduleStoreError> {
            self.inner.list_occurrences(filter).await
        }

        async fn append_receipt(
            &self,
            _receipt: DeliveryReceipt,
        ) -> Result<(), ScheduleStoreError> {
            Err(ScheduleStoreError::Internal(
                "standalone receipt append disabled for regression".into(),
            ))
        }

        async fn list_receipts(
            &self,
            occurrence_id: &crate::OccurrenceId,
        ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError> {
            self.inner.list_receipts(occurrence_id).await
        }

        async fn claim_due_occurrences(
            &self,
            request: ClaimDueRequest,
        ) -> Result<crate::ClaimDueResult, ScheduleStoreError> {
            self.inner.claim_due_occurrences(request).await
        }

        async fn transition_occurrence_if_current(
            &self,
            occurrence_id: &crate::OccurrenceId,
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

        async fn transition_occurrence_with_receipt_if_current(
            &self,
            occurrence_id: &crate::OccurrenceId,
            expected_attempt: u32,
            expected_claim_token: Option<Uuid>,
            transition: OccurrenceLifecycleInput,
            runtime_outcome: Option<RuntimeDeliveryOutcome>,
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
    async fn target_probe_terminality_comes_from_occurrence_authority()
    -> Result<(), ScheduleDomainError> {
        let cases = [
            (
                TargetProbeOutcome::Busy {
                    detail: Some("target already running".to_string()),
                },
                OverlapPolicy::SkipIfRunning,
                MissingTargetPolicy::MarkMisfired,
                OccurrencePhase::Skipped,
                DeliveryReceiptStage::Skipped,
                OccurrenceFailureClass::TargetBusy,
            ),
            (
                TargetProbeOutcome::Missing {
                    detail: Some("target disappeared".to_string()),
                },
                OverlapPolicy::AllowConcurrent,
                MissingTargetPolicy::Skip,
                OccurrencePhase::Skipped,
                DeliveryReceiptStage::Skipped,
                OccurrenceFailureClass::TargetMissing,
            ),
            (
                TargetProbeOutcome::Missing {
                    detail: Some("target disappeared".to_string()),
                },
                OverlapPolicy::AllowConcurrent,
                MissingTargetPolicy::MarkMisfired,
                OccurrencePhase::Misfired,
                DeliveryReceiptStage::Misfired,
                OccurrenceFailureClass::TargetMissing,
            ),
        ];

        for (
            probe_outcome,
            overlap_policy,
            missing_target_policy,
            expected_phase,
            expected_stage,
            expected_failure_class,
        ) in cases
        {
            let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
            let service = ScheduleService::new(store.clone());
            let schedule = service
                .create(CreateScheduleRequest {
                    name: Some(format!("target-probe-{expected_phase:?}")),
                    description: None,
                    trigger: TriggerSpec::Once {
                        due_at_utc: Utc::now() - Duration::seconds(1),
                    },
                    target: materialize_on_demand_target("scheduled prompt"),
                    misfire_policy: MisfirePolicy::Skip,
                    overlap_policy,
                    missing_target_policy,
                    labels: BTreeMap::new(),
                    planning_horizon_days: Some(1),
                    planning_horizon_occurrences: Some(1),
                })
                .await?;
            let delivery = Arc::new(CompletingDelivery::default());
            let driver = ScheduleDriver::new(
                service.clone(),
                store.clone(),
                Arc::new(StaticProbe(probe_outcome)),
                delivery.clone(),
                "driver-owner",
                ScheduleDriverConfig {
                    claim_limit: 8,
                    lease_duration: Duration::seconds(30),
                },
            );

            let report = driver.tick_once().await?;
            assert_eq!(report.claimed_occurrences, 1);
            assert_eq!(report.terminalized_occurrences, 1);
            assert!(delivery.dispatched_occurrences.lock().await.is_empty());

            let occurrence =
                wait_for_occurrence_phase(&service, &schedule.schedule_id, expected_phase).await?;
            assert_eq!(occurrence.failure_class, Some(expected_failure_class));

            let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
            let last_receipt = receipts.last().ok_or_else(|| {
                ScheduleDomainError::Internal(
                    "target probe terminality should emit a receipt".to_string(),
                )
            })?;
            assert_eq!(last_receipt.stage, expected_stage);
            assert_eq!(last_receipt.failure_class, Some(expected_failure_class));
        }

        Ok(())
    }

    #[tokio::test]
    async fn target_probe_busy_allow_concurrent_continues_to_delivery()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("target-busy-allowed".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::AllowConcurrent,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(CompletingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store,
            Arc::new(StaticProbe(TargetProbeOutcome::Busy {
                detail: Some("target already running".to_string()),
            })),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;

        let occurrence =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await?;
        assert_eq!(occurrence.failure_class, None);
        assert_eq!(delivery.dispatched_occurrences.lock().await.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn driver_misfires_long_overdue_skip_occurrence_without_dispatch()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("skip-misfire".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() - Duration::minutes(2),
                    every_seconds: 61,
                    end_at_utc: None,
                }),
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(CompletingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        let report = driver.tick_once().await?;
        assert_eq!(report.claimed_occurrences, 0);
        assert_eq!(report.terminalized_occurrences, 0);
        assert!(
            delivery.dispatched_occurrences.lock().await.is_empty(),
            "skip policy should not dispatch materially late pending occurrences"
        );

        let occurrence =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Misfired)
                .await?;
        assert_eq!(occurrence.attempt_count, 0);

        let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
        let last_receipt = receipts.last().ok_or_else(|| {
            ScheduleDomainError::Internal("misfired occurrence should emit a receipt".to_string())
        })?;
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::Misfired);
        assert!(
            last_receipt
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("skip policy")),
            "misfire receipt should explain why overdue work was skipped"
        );
        Ok(())
    }

    #[tokio::test]
    async fn driver_catches_up_overdue_occurrence_within_window() -> Result<(), ScheduleDomainError>
    {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("catch-up-window".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() - Duration::minutes(2),
                    every_seconds: 61,
                    end_at_utc: None,
                }),
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::CatchUpWithin {
                    window_seconds: 120,
                },
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(CompletingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        let report = driver.tick_once().await?;
        assert_eq!(report.claimed_occurrences, 1);
        assert_eq!(delivery.dispatched_occurrences.lock().await.len(), 1);

        let occurrence =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await?;
        let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
        assert_eq!(
            receipts.last().map(|receipt| receipt.stage),
            Some(DeliveryReceiptStage::Completed),
            "catch-up policy should still allow overdue work within its window"
        );
        Ok(())
    }

    #[tokio::test]
    async fn driver_misfires_overdue_occurrence_past_catch_up_window()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("catch-up-expired".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() - Duration::minutes(2),
                    every_seconds: 61,
                    end_at_utc: None,
                }),
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::CatchUpWithin { window_seconds: 30 },
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(CompletingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        let report = driver.tick_once().await?;
        assert_eq!(report.claimed_occurrences, 0);
        assert!(
            delivery.dispatched_occurrences.lock().await.is_empty(),
            "expired catch-up window should prevent stale dispatch"
        );

        let occurrence =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Misfired)
                .await?;
        let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
        let last_receipt = receipts.last().ok_or_else(|| {
            ScheduleDomainError::Internal("misfired occurrence should emit a receipt".to_string())
        })?;
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::Misfired);
        assert!(
            last_receipt
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("catch-up window")),
            "misfire receipt should explain the expired catch-up window"
        );
        Ok(())
    }

    #[tokio::test]
    async fn driver_preserves_target_materialization_failure_classification()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("materialize-now".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;

        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            Arc::new(MaterializationFailureDelivery),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        let report = driver.tick_once().await?;
        assert_eq!(report.claimed_occurrences, 1);
        assert_eq!(report.terminalized_occurrences, 0);

        let occurrence = loop {
            let occurrences = service.list_occurrences(&schedule.schedule_id).await?;
            if let Some(occurrence) = occurrences
                .into_iter()
                .find(|occurrence| occurrence.phase == OccurrencePhase::DeliveryFailed)
            {
                break occurrence;
            }
            sleep(std::time::Duration::from_millis(10)).await;
        };

        assert_eq!(
            occurrence.failure_class,
            Some(OccurrenceFailureClass::TargetMaterializationFailed)
        );
        assert_eq!(
            occurrence.failure_detail.as_deref(),
            Some("session creation failed")
        );

        let last_receipt = loop {
            let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
            if let Some(receipt) = receipts
                .last()
                .filter(|receipt| receipt.stage == DeliveryReceiptStage::DeliveryFailed)
            {
                break receipt.clone();
            }
            sleep(std::time::Duration::from_millis(10)).await;
        };
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::DeliveryFailed);
        assert_eq!(
            last_receipt.failure_class,
            Some(OccurrenceFailureClass::TargetMaterializationFailed)
        );
        Ok(())
    }

    #[tokio::test]
    async fn driver_preserves_dispatch_receipt_on_in_flight_occurrence_projection()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("dispatch-receipt-projection".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        let report = driver.tick_once().await?;
        assert_eq!(report.claimed_occurrences, 1);
        wait_for_sender_count(&delivery, 1).await;

        let occurrence = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;
        let last_receipt = occurrence.last_receipt.as_ref().ok_or_else(|| {
            ScheduleDomainError::Internal(
                "dispatch receipt should remain projected on in-flight occurrences".to_string(),
            )
        })?;
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::DispatchStarted);
        assert_eq!(
            last_receipt.correlation_id.as_deref(),
            Some("dispatch-attempt-1")
        );
        Ok(())
    }

    #[tokio::test]
    async fn delivery_failed_without_generated_failure_reason_fails_closed()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("missing-failure-class".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store,
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;

        let occurrence = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;

        let sender = delivery.senders.lock().await.remove(0);
        sender
            .send(DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some("missing generated failure reason".into()),
                delivery_failure_reason: None,
                runtime_completion_outcome: None,
                runtime_outcome: None,
            })
            .expect("completion receiver should be open");
        sleep(std::time::Duration::from_millis(30)).await;

        let after = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|candidate| candidate.occurrence_id == occurrence.occurrence_id)
            .ok_or_else(|| ScheduleDomainError::Internal("occurrence should exist".to_string()))?;
        assert_eq!(after.phase, OccurrencePhase::AwaitingCompletion);
        assert_eq!(after.failure_class, None);
        Ok(())
    }

    #[tokio::test]
    async fn completion_terminalizes_and_records_receipt_without_standalone_append()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("atomic-terminal-receipt".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        let awaiting = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;

        let terminal_store = Arc::new(StandaloneReceiptFailingStore {
            inner: store.clone(),
        }) as Arc<dyn ScheduleStore>;
        let terminalized = terminalize_occurrence_inner(
            terminal_store,
            awaiting.clone(),
            OccurrenceLifecycleInput::Complete { at_utc: Utc::now() },
            None,
            None,
        )
        .await?;

        assert!(terminalized);
        let completed =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await?;
        let receipts = store.list_receipts(&completed.occurrence_id).await?;
        let last_receipt = receipts.last().ok_or_else(|| {
            ScheduleDomainError::Internal(
                "terminal completion should append generated receipt".to_string(),
            )
        })?;
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::Completed);
        assert_eq!(
            completed
                .last_receipt
                .as_ref()
                .map(|receipt| receipt.receipt_id),
            Some(last_receipt.receipt_id)
        );
        Ok(())
    }

    #[tokio::test]
    async fn adapter_selected_terminal_skip_or_misfire_fails_closed()
    -> Result<(), ScheduleDomainError> {
        for phase in [OccurrencePhase::Skipped, OccurrencePhase::Misfired] {
            let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
            let service = ScheduleService::new(store.clone());
            let schedule = service
                .create(CreateScheduleRequest {
                    name: Some(format!("adapter-selected-{phase:?}")),
                    description: None,
                    trigger: TriggerSpec::Once {
                        due_at_utc: Utc::now() - Duration::seconds(1),
                    },
                    target: materialize_on_demand_target("scheduled prompt"),
                    misfire_policy: MisfirePolicy::Skip,
                    overlap_policy: OverlapPolicy::SkipIfRunning,
                    missing_target_policy: MissingTargetPolicy::MarkMisfired,
                    labels: BTreeMap::new(),
                    planning_horizon_days: Some(1),
                    planning_horizon_occurrences: Some(1),
                })
                .await?;
            let delivery = Arc::new(ControlledCompletionDelivery::default());
            let driver = ScheduleDriver::new(
                service.clone(),
                store,
                Arc::new(ReadyProbe),
                delivery.clone(),
                "driver-owner",
                ScheduleDriverConfig {
                    claim_limit: 8,
                    lease_duration: Duration::seconds(30),
                },
            );

            driver.tick_once().await?;
            wait_for_sender_count(&delivery, 1).await;

            let occurrence = wait_for_occurrence_phase(
                &service,
                &schedule.schedule_id,
                OccurrencePhase::AwaitingCompletion,
            )
            .await?;

            let sender = delivery.senders.lock().await.remove(0);
            sender
                .send(DeliveryTerminal {
                    phase,
                    receipt: None,
                    detail: Some("adapter-selected terminality".into()),
                    delivery_failure_reason: None,
                    runtime_completion_outcome: None,
                    runtime_outcome: None,
                })
                .expect("completion receiver should be open");
            sleep(std::time::Duration::from_millis(30)).await;

            let after = service
                .list_occurrences(&schedule.schedule_id)
                .await?
                .into_iter()
                .find(|candidate| candidate.occurrence_id == occurrence.occurrence_id)
                .ok_or_else(|| {
                    ScheduleDomainError::Internal("occurrence should exist".to_string())
                })?;
            assert_eq!(after.phase, OccurrencePhase::AwaitingCompletion);
            assert_eq!(after.failure_class, None);
        }
        Ok(())
    }

    #[tokio::test]
    async fn completion_future_failure_classification_comes_from_occurrence_authority()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("completion-future-failure".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store,
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        drop(delivery.senders.lock().await.remove(0));

        let after = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::DeliveryFailed,
        )
        .await?;
        assert_eq!(
            after.failure_class,
            Some(OccurrenceFailureClass::TransportError)
        );
        assert!(
            after
                .failure_detail
                .as_deref()
                .is_some_and(|detail| detail.contains("schedule driver stopped"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn driver_reclaims_expired_awaiting_completion_occurrences()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("reclaim-now".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::milliseconds(25),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        wait_for_occurrence_attempt(&service, &schedule.schedule_id, 1).await?;

        sleep(std::time::Duration::from_millis(35)).await;
        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 2).await;
        let occurrence = wait_for_occurrence_attempt(&service, &schedule.schedule_id, 2).await?;

        assert_eq!(occurrence.phase, OccurrencePhase::AwaitingCompletion);
        let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
        assert!(
            receipts
                .iter()
                .any(|receipt| receipt.stage == DeliveryReceiptStage::LeaseExpired),
            "lease expiry reclaim should append a lease-expired receipt"
        );
        Ok(())
    }

    #[tokio::test]
    async fn late_completion_from_expired_attempt_does_not_overwrite_reclaimed_attempt()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("late-completion".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::milliseconds(25),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        wait_for_occurrence_attempt(&service, &schedule.schedule_id, 1).await?;

        sleep(std::time::Duration::from_millis(35)).await;
        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 2).await;
        wait_for_occurrence_attempt(&service, &schedule.schedule_id, 2).await?;

        let mut senders = delivery.senders.lock().await;
        let first_attempt = senders.remove(0);
        let second_attempt = senders.remove(0);
        drop(senders);

        first_attempt
            .send(DeliveryTerminal::completed(None))
            .expect("first attempt sender should be open");
        sleep(std::time::Duration::from_millis(20)).await;

        let after_stale_completion = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| ScheduleDomainError::Internal("occurrence should exist".to_string()))?;
        assert_eq!(after_stale_completion.attempt_count, 2);
        assert_eq!(
            after_stale_completion.phase,
            OccurrencePhase::AwaitingCompletion
        );

        second_attempt
            .send(DeliveryTerminal::completed(None))
            .expect("second attempt sender should be open");

        let completed = loop {
            let occurrence = service
                .list_occurrences(&schedule.schedule_id)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    ScheduleDomainError::Internal("occurrence should exist".to_string())
                })?;
            if occurrence.phase == OccurrencePhase::Completed {
                break occurrence;
            }
            sleep(std::time::Duration::from_millis(10)).await;
        };

        assert_eq!(completed.attempt_count, 2);
        let receipts = store.list_receipts(&completed.occurrence_id).await?;
        assert_eq!(
            receipts.last().map(|receipt| receipt.attempt),
            Some(2),
            "late completion from the expired lease must not overwrite the reclaimed attempt"
        );
        Ok(())
    }

    #[tokio::test]
    async fn paused_claimed_occurrence_is_released_before_probe_or_delivery()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("pause-claimed".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let probe = Arc::new(CountingProbe::default());
        let delivery = Arc::new(CountingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            probe.clone(),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        let claimed = store
            .claim_due_occurrences(ClaimDueRequest {
                owner_id: "driver-owner".into(),
                limit: 1,
                lease_duration: Duration::seconds(30),
            })
            .await?;
        let occurrence = claimed
            .claimed
            .into_iter()
            .next()
            .expect("claimed occurrence");
        service.pause(&schedule.schedule_id).await?;

        let terminalized = driver
            .handle_claimed_occurrence(occurrence.clone(), claimed.store_now_utc)
            .await?;
        let current = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|item| item.occurrence_id == occurrence.occurrence_id)
            .expect("occurrence should still exist");

        assert!(!terminalized, "paused claimed work should be frozen");
        assert_eq!(current.phase, OccurrencePhase::Pending);
        assert_eq!(*probe.calls.lock().await, 0, "pause should block probes");
        assert_eq!(
            *delivery.calls.lock().await,
            0,
            "pause should block delivery"
        );
        let receipts = store.list_receipts(&current.occurrence_id).await?;
        assert!(
            receipts
                .iter()
                .any(|receipt| receipt.stage == DeliveryReceiptStage::LeaseExpired),
            "pause should release the claim immediately"
        );
        Ok(())
    }

    #[tokio::test]
    async fn deleted_claimed_occurrence_is_superseded_before_delivery()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("delete-claimed".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let probe = Arc::new(CountingProbe::default());
        let delivery = Arc::new(CountingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            probe.clone(),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        let claimed = store
            .claim_due_occurrences(ClaimDueRequest {
                owner_id: "driver-owner".into(),
                limit: 1,
                lease_duration: Duration::seconds(30),
            })
            .await?;
        let occurrence = claimed
            .claimed
            .into_iter()
            .next()
            .expect("claimed occurrence");
        service.delete(&schedule.schedule_id).await?;

        let terminalized = driver
            .handle_claimed_occurrence(occurrence.clone(), claimed.store_now_utc)
            .await?;
        let current = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|item| item.occurrence_id == occurrence.occurrence_id)
            .expect("occurrence should still exist");

        assert!(terminalized, "deleted claimed work should supersede");
        assert_eq!(current.phase, OccurrencePhase::Superseded);
        assert_eq!(*probe.calls.lock().await, 0, "delete should block probes");
        assert_eq!(
            *delivery.calls.lock().await,
            0,
            "delete should block delivery"
        );
        Ok(())
    }

    #[tokio::test]
    async fn stale_revision_claimed_occurrence_is_superseded_before_delivery()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("stale-claimed".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let probe = Arc::new(CountingProbe::default());
        let delivery = Arc::new(CountingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            probe.clone(),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        let claimed = store
            .claim_due_occurrences(ClaimDueRequest {
                owner_id: "driver-owner".into(),
                limit: 1,
                lease_duration: Duration::seconds(30),
            })
            .await?;
        let occurrence = claimed
            .claimed
            .into_iter()
            .next()
            .expect("claimed occurrence");
        let updated = service
            .update(
                &schedule.schedule_id,
                UpdateScheduleRequest {
                    expected_revision: Some(schedule.revision),
                    trigger: Some(TriggerSpec::Interval(IntervalTriggerSpec {
                        start_at_utc: Utc::now() + Duration::minutes(5),
                        every_seconds: 300,
                        end_at_utc: None,
                    })),
                    ..UpdateScheduleRequest::default()
                },
            )
            .await?;

        let terminalized = driver
            .handle_claimed_occurrence(occurrence.clone(), claimed.store_now_utc)
            .await?;
        let current = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|item| item.occurrence_id == occurrence.occurrence_id)
            .expect("occurrence should still exist");

        assert!(terminalized, "stale claimed work should supersede");
        assert_eq!(current.phase, OccurrencePhase::Superseded);
        assert_eq!(
            current.superseded_by_revision,
            Some(updated.revision),
            "stale claimed work should record the current schedule revision"
        );
        assert_eq!(
            *probe.calls.lock().await,
            0,
            "stale revision should block probes"
        );
        assert_eq!(
            *delivery.calls.lock().await,
            0,
            "stale revision should block delivery"
        );
        Ok(())
    }

    #[tokio::test]
    async fn awaiting_completion_occurrence_is_superseded_when_schedule_is_deleted()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("delete-awaiting".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        let awaiting = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;

        let deleted = service.delete(&schedule.schedule_id).await?;
        let sender = delivery.senders.lock().await.remove(0);
        sender
            .send(DeliveryTerminal::completed(None))
            .expect("sender should stay open");

        let superseded = loop {
            let occurrence = service
                .list_occurrences(&schedule.schedule_id)
                .await?
                .into_iter()
                .find(|item| item.occurrence_id == awaiting.occurrence_id)
                .expect("occurrence should still exist");
            if occurrence.phase == OccurrencePhase::Superseded {
                break occurrence;
            }
            sleep(std::time::Duration::from_millis(10)).await;
        };

        assert_eq!(superseded.superseded_by_revision, Some(deleted.revision));
        Ok(())
    }

    #[tokio::test]
    async fn awaiting_completion_occurrence_is_superseded_when_schedule_revision_advances()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("update-awaiting".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        let awaiting = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;

        let updated = service
            .update(
                &schedule.schedule_id,
                UpdateScheduleRequest {
                    expected_revision: Some(schedule.revision),
                    trigger: Some(TriggerSpec::Interval(IntervalTriggerSpec {
                        start_at_utc: Utc::now() + Duration::minutes(5),
                        every_seconds: 300,
                        end_at_utc: None,
                    })),
                    ..UpdateScheduleRequest::default()
                },
            )
            .await?;
        let sender = delivery.senders.lock().await.remove(0);
        sender
            .send(DeliveryTerminal::completed(None))
            .expect("sender should stay open");

        let superseded = loop {
            let occurrence = service
                .list_occurrences(&schedule.schedule_id)
                .await?
                .into_iter()
                .find(|item| item.occurrence_id == awaiting.occurrence_id)
                .expect("occurrence should still exist");
            if occurrence.phase == OccurrencePhase::Superseded {
                break occurrence;
            }
            sleep(std::time::Duration::from_millis(10)).await;
        };

        assert_eq!(superseded.superseded_by_revision, Some(updated.revision));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 0.7.2 disciplined shell inputs (D1/D2a) — shell interleaving tests.
    // Tests marked "STAGE B" assert the wired shell behavior and are expected
    // RED after Stage A codegen (the DSL totality is in; the shell sequencing
    // is not). The lead records them on the red list.
    // -----------------------------------------------------------------------

    /// STAGE B (RED until wired): `service.delete()` must revoke the
    /// driver-claimed in-flight occurrence AT COMMIT by superseding it through
    /// the occurrence authority's typed Supersede transition — not leave it
    /// AwaitingCompletion for the completion waiter to discover later. The
    /// waiter's late resolution then lands as the typed late-arrival record,
    /// never a guard rejection and never a silent drop.
    #[tokio::test]
    async fn delete_revokes_in_flight_claim_at_commit_and_late_completion_lands_typed()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("delete-revokes-in-flight".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        let awaiting = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;

        // Teardown commits while the completion waiter is still in flight.
        let deleted = service.delete(&schedule.schedule_id).await?;

        // D1: the delete commit itself revokes the in-flight claim.
        let at_commit = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|item| item.occurrence_id == awaiting.occurrence_id)
            .ok_or_else(|| {
                ScheduleDomainError::Internal("occurrence should still exist".to_string())
            })?;
        assert_eq!(
            at_commit.phase,
            OccurrencePhase::Superseded,
            "delete must supersede the driver-claimed in-flight occurrence at commit time"
        );
        assert_eq!(at_commit.superseded_by_revision, Some(deleted.revision));
        assert!(
            deleted
                .superseded_ack_ids
                .contains(&awaiting.occurrence_id),
            "the revoked in-flight claim must be accounted in the schedule authority's ack set"
        );

        // The waiter resolves AFTER the teardown committed: typed late-arrival
        // record, zero corruption of the recorded supersession.
        let sender = delivery.senders.lock().await.remove(0);
        sender
            .send(DeliveryTerminal::completed(None))
            .expect("completion receiver should be open");

        let late = wait_for_late_completion_record(&service, &schedule.schedule_id).await?;
        assert_eq!(late.phase, OccurrencePhase::Superseded);
        assert_eq!(late.superseded_by_revision, Some(deleted.revision));
        assert_eq!(
            late.machine_state.late_completion_resolution,
            Some(crate::machines::occurrence_lifecycle::LateCompletionResolutionClass::DeliveryCompleted)
        );
        Ok(())
    }

    /// STAGE B (RED until wired): delete supersedes a CLAIMED (pre-dispatch)
    /// occurrence at commit; the driver's subsequent reconcile of its held
    /// claim is a benign idempotent no-op — no probe, no delivery, no
    /// duplicate superseded receipt.
    #[tokio::test]
    async fn delete_supersedes_claimed_occurrence_at_commit_without_duplicate_receipt()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("delete-claimed-at-commit".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let probe = Arc::new(CountingProbe::default());
        let delivery = Arc::new(CountingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            probe.clone(),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        let claimed = store
            .claim_due_occurrences(ClaimDueRequest {
                owner_id: "driver-owner".into(),
                limit: 1,
                lease_duration: Duration::seconds(30),
            })
            .await?;
        let occurrence = claimed
            .claimed
            .clone()
            .into_iter()
            .next()
            .expect("claimed occurrence");

        let deleted = service.delete(&schedule.schedule_id).await?;

        // D1: revoked at commit, not on the driver's next decision point.
        let at_commit = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|item| item.occurrence_id == occurrence.occurrence_id)
            .expect("occurrence should still exist");
        assert_eq!(
            at_commit.phase,
            OccurrencePhase::Superseded,
            "delete must supersede the driver-claimed occurrence at commit time"
        );
        assert_eq!(at_commit.superseded_by_revision, Some(deleted.revision));

        // The driver still holds the pre-delete claim snapshot; handling it
        // must stay benign (typed idempotent no-op): no probe, no delivery,
        // no error, no duplicate superseded receipt.
        driver
            .handle_claimed_occurrence(occurrence.clone(), claimed.store_now_utc)
            .await?;
        assert_eq!(*probe.calls.lock().await, 0, "delete should block probes");
        assert_eq!(
            *delivery.calls.lock().await,
            0,
            "delete should block delivery"
        );
        let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
        assert_eq!(
            receipts
                .iter()
                .filter(|receipt| receipt.stage == DeliveryReceiptStage::Superseded)
                .count(),
            1,
            "the commit-time sweep mints the canonical superseded receipt; the driver's \
             idempotent reconcile path must not duplicate it"
        );
        Ok(())
    }

    /// STAGE B (RED until wired): a completion arrival whose claim evidence is
    /// stale (lease expired and the occurrence was reclaimed for attempt 2)
    /// must be recorded as the occurrence authority's typed
    /// ClassifyStaleCompletionArrival fact — never silently dropped via
    /// `Ok(false)`.
    #[tokio::test]
    async fn stale_completion_arrival_is_recorded_as_typed_machine_fact()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("stale-arrival-typed".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::milliseconds(25),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        wait_for_occurrence_attempt(&service, &schedule.schedule_id, 1).await?;

        sleep(std::time::Duration::from_millis(35)).await;
        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 2).await;
        wait_for_occurrence_attempt(&service, &schedule.schedule_id, 2).await?;

        let first_attempt = delivery.senders.lock().await.remove(0);
        first_attempt
            .send(DeliveryTerminal::completed(None))
            .expect("first attempt sender should be open");

        let recorded = loop_until_stale_arrival_recorded(&service, &schedule.schedule_id).await?;
        assert_eq!(
            recorded.phase,
            OccurrencePhase::AwaitingCompletion,
            "the stale arrival must not disturb the reclaimed attempt"
        );
        assert_eq!(recorded.attempt_count, 2);
        assert_eq!(
            recorded.machine_state.stale_completion_arrivals, 1,
            "the screened stale arrival must be recorded as a typed machine fact"
        );
        Ok(())
    }

    /// GREEN pin (by-design semantics): pause does NOT supersede an in-flight
    /// dispatched delivery; its completion lands normally on the paused
    /// schedule's occurrence.
    #[tokio::test]
    async fn pause_does_not_supersede_in_flight_delivery_completion_lands_normally()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("pause-in-flight-completes".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        let awaiting = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;

        service.pause(&schedule.schedule_id).await?;
        let sender = delivery.senders.lock().await.remove(0);
        sender
            .send(DeliveryTerminal::completed(None))
            .expect("completion receiver should be open");

        let completed = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::Completed,
        )
        .await?;
        assert_eq!(completed.occurrence_id, awaiting.occurrence_id);
        let receipts = store.list_receipts(&completed.occurrence_id).await?;
        assert_eq!(
            receipts.last().map(|receipt| receipt.stage),
            Some(DeliveryReceiptStage::Completed),
            "in-flight delivery under a paused schedule completes and records normally"
        );
        Ok(())
    }

    async fn wait_for_late_completion_record(
        service: &ScheduleService,
        schedule_id: &crate::ScheduleId,
    ) -> Result<Occurrence, ScheduleDomainError> {
        for _ in 0..50 {
            let occurrences = service.list_occurrences(schedule_id).await?;
            if let Some(occurrence) = occurrences.into_iter().find(|occurrence| {
                occurrence
                    .machine_state
                    .late_completion_resolution
                    .is_some()
            }) {
                return Ok(occurrence);
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        Err(ScheduleDomainError::Internal(
            "timed out waiting for typed late-completion record".to_string(),
        ))
    }

    async fn loop_until_stale_arrival_recorded(
        service: &ScheduleService,
        schedule_id: &crate::ScheduleId,
    ) -> Result<Occurrence, ScheduleDomainError> {
        for _ in 0..50 {
            let occurrences = service.list_occurrences(schedule_id).await?;
            if let Some(occurrence) = occurrences
                .into_iter()
                .find(|occurrence| occurrence.machine_state.stale_completion_arrivals > 0)
            {
                return Ok(occurrence);
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        Err(ScheduleDomainError::Internal(
            "timed out waiting for typed stale-completion-arrival record".to_string(),
        ))
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
                comms_name: Some("scheduled-materializer".into()),
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

    async fn wait_for_sender_count(delivery: &ControlledCompletionDelivery, expected: usize) {
        for _ in 0..50 {
            if delivery.senders.lock().await.len() >= expected {
                return;
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {expected} delivery senders");
    }

    async fn wait_for_occurrence_attempt(
        service: &ScheduleService,
        schedule_id: &crate::ScheduleId,
        attempt_count: u32,
    ) -> Result<Occurrence, ScheduleDomainError> {
        for _ in 0..50 {
            let occurrences = service.list_occurrences(schedule_id).await?;
            if let Some(occurrence) = occurrences
                .into_iter()
                .find(|occurrence| occurrence.attempt_count == attempt_count)
            {
                return Ok(occurrence);
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        Err(ScheduleDomainError::Internal(format!(
            "timed out waiting for occurrence attempt {attempt_count}"
        )))
    }

    async fn wait_for_occurrence_phase(
        service: &ScheduleService,
        schedule_id: &crate::ScheduleId,
        expected_phase: OccurrencePhase,
    ) -> Result<Occurrence, ScheduleDomainError> {
        for _ in 0..50 {
            let occurrences = service.list_occurrences(schedule_id).await?;
            if let Some(occurrence) = occurrences
                .into_iter()
                .find(|occurrence| occurrence.phase == expected_phase)
            {
                return Ok(occurrence);
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        Err(ScheduleDomainError::Internal(format!(
            "timed out waiting for occurrence phase {expected_phase:?}"
        )))
    }
}
