use crate::authority::{OccurrenceLifecycleAuthority, OccurrenceLifecycleInput};
use crate::error::ScheduleDomainError;
use crate::service::ScheduleService;
use crate::store::{ClaimDueRequest, ScheduleStore};
use crate::types::{
    DeliveryReceipt, DeliveryReceiptStage, Occurrence, OccurrenceFailureClass, OccurrencePhase,
    OverlapPolicy, SchedulePhase,
};
use async_trait::async_trait;
use chrono::{Duration, Utc};
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
    pub failure_class: Option<OccurrenceFailureClass>,
    pub materialized_session_id: Option<SessionId>,
}

impl DeliveryTerminal {
    pub fn completed(receipt: Option<DeliveryReceipt>) -> Self {
        Self {
            phase: OccurrencePhase::Completed,
            receipt,
            detail: None,
            failure_class: None,
            materialized_session_id: None,
        }
    }

    pub fn skipped(detail: impl Into<String>, failure_class: OccurrenceFailureClass) -> Self {
        Self {
            phase: OccurrencePhase::Skipped,
            receipt: None,
            detail: Some(detail.into()),
            failure_class: Some(failure_class),
            materialized_session_id: None,
        }
    }

    pub fn misfired(detail: impl Into<String>, failure_class: OccurrenceFailureClass) -> Self {
        Self {
            phase: OccurrencePhase::Misfired,
            receipt: None,
            detail: Some(detail.into()),
            failure_class: Some(failure_class),
            materialized_session_id: None,
        }
    }

    pub fn delivery_failed(
        detail: impl Into<String>,
        failure_class: OccurrenceFailureClass,
    ) -> Self {
        Self {
            phase: OccurrencePhase::DeliveryFailed,
            receipt: None,
            detail: Some(detail.into()),
            failure_class: Some(failure_class),
            materialized_session_id: None,
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

pub struct ScheduleDriver {
    service: ScheduleService,
    store: Arc<dyn ScheduleStore>,
    probe: Arc<dyn ScheduleTargetProbe>,
    delivery: Arc<dyn ScheduleTargetDelivery>,
    occurrence_authority: Arc<OccurrenceLifecycleAuthority>,
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
            occurrence_authority: Arc::new(OccurrenceLifecycleAuthority),
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
        let mut occurrence = self
            .service
            .sync_occurrence_target_with_schedule(occurrence)
            .await?;

        match self.probe.probe_target(&occurrence).await? {
            TargetProbeOutcome::Ready => {}
            TargetProbeOutcome::Busy { detail } => {
                if occurrence.overlap_policy == OverlapPolicy::AllowConcurrent {
                    // Delivery continues for explicit concurrent schedules.
                } else {
                    self.terminalize_occurrence(
                        occurrence,
                        OccurrenceLifecycleInput::Skip {
                            detail: detail.or_else(|| Some("target busy".to_string())),
                            failure_class: Some(OccurrenceFailureClass::TargetBusy),
                            at_utc: store_now_utc,
                        },
                        DeliveryReceiptStage::Skipped,
                        None,
                    )
                    .await?;
                    return Ok(true);
                }
            }
            TargetProbeOutcome::Missing { detail } => {
                let lifecycle = if matches!(
                    occurrence.missing_target_policy,
                    crate::types::MissingTargetPolicy::Skip
                ) {
                    OccurrenceLifecycleInput::Skip {
                        detail: detail.or_else(|| Some("target missing".to_string())),
                        failure_class: Some(OccurrenceFailureClass::TargetMissing),
                        at_utc: store_now_utc,
                    }
                } else {
                    OccurrenceLifecycleInput::Misfire {
                        detail: detail.or_else(|| Some("target missing".to_string())),
                        failure_class: Some(OccurrenceFailureClass::TargetMissing),
                        at_utc: store_now_utc,
                    }
                };
                let stage = match lifecycle {
                    OccurrenceLifecycleInput::Skip { .. } => DeliveryReceiptStage::Skipped,
                    _ => DeliveryReceiptStage::Misfired,
                };
                self.terminalize_occurrence(occurrence, lifecycle, stage, None)
                    .await?;
                return Ok(true);
            }
        }

        let dispatch = match self.delivery.deliver_occurrence(&occurrence).await {
            Ok(dispatch) => dispatch,
            Err(error) => {
                let detail = error.to_string();
                self.terminalize_occurrence(
                    occurrence,
                    OccurrenceLifecycleInput::DeliveryFailed {
                        receipt: None,
                        failure_class: OccurrenceFailureClass::TransportError,
                        detail: Some(detail),
                        at_utc: store_now_utc,
                    },
                    DeliveryReceiptStage::DeliveryFailed,
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

        let mut dispatching = self
            .occurrence_authority
            .apply(
                occurrence,
                OccurrenceLifecycleInput::DispatchStarted {
                    correlation_id: dispatch.correlation_id.clone(),
                    at_utc: store_now_utc,
                },
            )
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?
            .into_occurrence();
        self.store.put_occurrence(dispatching.clone()).await?;
        self.store.append_receipt(dispatch.receipt.clone()).await?;

        dispatching = self
            .occurrence_authority
            .apply(
                dispatching,
                OccurrenceLifecycleInput::AwaitCompletion {
                    at_utc: store_now_utc,
                },
            )
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?
            .into_occurrence();
        self.store.put_occurrence(dispatching.clone()).await?;

        self.spawn_completion_waiter(dispatching, dispatch.completion);
        Ok(false)
    }

    async fn terminalize_occurrence(
        &self,
        occurrence: Occurrence,
        lifecycle: OccurrenceLifecycleInput,
        stage: DeliveryReceiptStage,
        receipt: Option<DeliveryReceipt>,
    ) -> Result<(), ScheduleDomainError> {
        let _ = terminalize_occurrence_inner(
            self.store.clone(),
            occurrence,
            lifecycle,
            stage,
            receipt,
            None,
        )
        .await?;
        Ok(())
    }

    fn spawn_completion_waiter(&self, occurrence: Occurrence, completion: DeliveryCompletion) {
        let store = self.store.clone();
        crate::tokio::spawn(async move {
            let terminal = match completion.await {
                Ok(terminal) => terminal,
                Err(error) => DeliveryTerminal::delivery_failed(
                    error.to_string(),
                    OccurrenceFailureClass::TransportError,
                ),
            };
            let _ = complete_dispatched_occurrence(store, occurrence, terminal).await;
        });
    }
}

async fn complete_dispatched_occurrence(
    store: Arc<dyn ScheduleStore>,
    occurrence: Occurrence,
    terminal: DeliveryTerminal,
) -> Result<(), ScheduleDomainError> {
    let store_now_utc = store.get_store_time_utc().await?;
    let completed_receipt = matches!(terminal.phase, OccurrencePhase::Completed).then(|| {
        build_terminal_receipt(
            &occurrence,
            DeliveryReceiptStage::Completed,
            terminal.receipt.clone(),
            terminal.failure_class,
            terminal.detail.clone(),
            terminal.materialized_session_id.clone(),
        )
    });
    let lifecycle = match terminal.phase {
        OccurrencePhase::Completed => {
            let receipt = completed_receipt.clone().ok_or_else(|| {
                ScheduleDomainError::Internal(
                    "completed terminal must carry a completed receipt".to_string(),
                )
            })?;
            OccurrenceLifecycleInput::Complete {
                receipt,
                at_utc: store_now_utc,
            }
        }
        OccurrencePhase::Skipped => OccurrenceLifecycleInput::Skip {
            detail: terminal.detail.clone(),
            failure_class: terminal.failure_class,
            at_utc: store_now_utc,
        },
        OccurrencePhase::Misfired => OccurrenceLifecycleInput::Misfire {
            detail: terminal.detail.clone(),
            failure_class: terminal.failure_class,
            at_utc: store_now_utc,
        },
        OccurrencePhase::DeliveryFailed => OccurrenceLifecycleInput::DeliveryFailed {
            receipt: terminal.receipt.clone(),
            failure_class: terminal
                .failure_class
                .unwrap_or(OccurrenceFailureClass::InternalError),
            detail: terminal.detail.clone(),
            at_utc: store_now_utc,
        },
        other => {
            return Err(ScheduleDomainError::Internal(format!(
                "delivery terminal returned non-terminal occurrence phase: {other:?}"
            )));
        }
    };
    let receipt_stage = match terminal.phase {
        OccurrencePhase::Completed => DeliveryReceiptStage::Completed,
        OccurrencePhase::Skipped => DeliveryReceiptStage::Skipped,
        OccurrencePhase::Misfired => DeliveryReceiptStage::Misfired,
        OccurrencePhase::DeliveryFailed => DeliveryReceiptStage::DeliveryFailed,
        _ => DeliveryReceiptStage::DeliveryFailed,
    };

    let _ = terminalize_occurrence_inner(
        store,
        occurrence,
        lifecycle,
        receipt_stage,
        completed_receipt.or(terminal.receipt),
        terminal.materialized_session_id,
    )
    .await?;
    Ok(())
}

async fn terminalize_occurrence_inner(
    store: Arc<dyn ScheduleStore>,
    occurrence: Occurrence,
    lifecycle: OccurrenceLifecycleInput,
    stage: DeliveryReceiptStage,
    receipt: Option<DeliveryReceipt>,
    materialized_session_id: Option<SessionId>,
) -> Result<bool, ScheduleDomainError> {
    let Some(mut updated) = store
        .transition_occurrence_if_current(
            &occurrence.occurrence_id,
            occurrence.attempt_count,
            occurrence.claim_token(),
            lifecycle,
        )
        .await?
    else {
        return Ok(false);
    };
    let final_receipt = build_terminal_receipt(
        &updated,
        stage,
        receipt,
        updated.failure_class,
        updated.failure_detail.clone(),
        materialized_session_id,
    );
    updated.last_receipt = Some(final_receipt.clone());
    store.put_occurrence(updated).await?;
    store.append_receipt(final_receipt).await?;
    Ok(true)
}

fn build_terminal_receipt(
    occurrence: &Occurrence,
    stage: DeliveryReceiptStage,
    receipt: Option<DeliveryReceipt>,
    failure_class: Option<OccurrenceFailureClass>,
    detail: Option<String>,
    materialized_session_id: Option<SessionId>,
) -> DeliveryReceipt {
    let mut receipt = receipt.unwrap_or_else(|| {
        DeliveryReceipt::new(
            occurrence.occurrence_id.clone(),
            occurrence.attempt_count,
            stage,
        )
    });
    receipt.stage = stage;
    if receipt.correlation_id.is_none() {
        receipt.correlation_id = occurrence.delivery_correlation_id.clone();
    }
    if receipt.failure_class.is_none() {
        receipt.failure_class = failure_class;
    }
    if receipt.detail.is_none() {
        receipt.detail = detail;
    }
    if receipt.materialized_session_id.is_none() {
        receipt.materialized_session_id = materialized_session_id.or_else(|| {
            occurrence
                .last_receipt
                .as_ref()
                .and_then(|last_receipt| last_receipt.materialized_session_id.clone())
        });
    }
    receipt
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::{
        CreateScheduleRequest, ScheduledSessionAction, SessionMaterializationSpec,
        SessionTargetBinding, TargetBinding,
    };
    use crate::{MemoryScheduleStore, MisfirePolicy, MissingTargetPolicy, TriggerSpec};
    use chrono::Duration;
    use meerkat_core::ContentInput;
    use std::collections::BTreeMap;
    use tokio::sync::{Mutex, oneshot};
    use tokio::time::sleep;

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
                        failure_class: Some(OccurrenceFailureClass::TargetMaterializationFailed),
                        materialized_session_id: None,
                    })
                }),
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

        let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
        let last_receipt = receipts.last().ok_or_else(|| {
            ScheduleDomainError::Internal("delivery failure receipt should exist".to_string())
        })?;
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::DeliveryFailed);
        assert_eq!(
            last_receipt.failure_class,
            Some(OccurrenceFailureClass::TargetMaterializationFailed)
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

    fn materialize_on_demand_target(prompt: &str) -> TargetBinding {
        TargetBinding::session(SessionTargetBinding::materialize_on_demand(
            SessionMaterializationSpec {
                model: "gpt-4.1-mini".into(),
                system_prompt: None,
                max_tokens: None,
                provider: None,
                output_schema_json: None,
                structured_output_retries: 0,
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
                skill_references: Vec::new(),
                additional_instructions: Vec::new(),
            },
        ))
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
}
