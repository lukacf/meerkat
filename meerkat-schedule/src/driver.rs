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
                self.terminalize_occurrence(occurrence, lifecycle, stage)
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
                )
                .await?;
                return Ok(true);
            }
        };

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

        let terminal = dispatch.completion.await?;
        let lifecycle = match terminal.phase {
            OccurrencePhase::Completed => OccurrenceLifecycleInput::Complete {
                receipt: terminal.receipt.unwrap_or_else(|| {
                    DeliveryReceipt::new(
                        dispatching.occurrence_id.clone(),
                        dispatching.attempt_count,
                        DeliveryReceiptStage::Completed,
                    )
                }),
                at_utc: store_now_utc,
            },
            OccurrencePhase::Skipped => OccurrenceLifecycleInput::Skip {
                detail: terminal.detail,
                failure_class: terminal.failure_class,
                at_utc: store_now_utc,
            },
            OccurrencePhase::Misfired => OccurrenceLifecycleInput::Misfire {
                detail: terminal.detail,
                failure_class: terminal.failure_class,
                at_utc: store_now_utc,
            },
            OccurrencePhase::DeliveryFailed => OccurrenceLifecycleInput::DeliveryFailed {
                receipt: terminal.receipt,
                failure_class: terminal
                    .failure_class
                    .unwrap_or(OccurrenceFailureClass::InternalError),
                detail: terminal.detail,
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
        self.terminalize_occurrence(dispatching, lifecycle, receipt_stage)
            .await?;
        Ok(true)
    }

    async fn terminalize_occurrence(
        &self,
        occurrence: Occurrence,
        lifecycle: OccurrenceLifecycleInput,
        stage: DeliveryReceiptStage,
    ) -> Result<(), ScheduleDomainError> {
        let updated = self
            .occurrence_authority
            .apply(occurrence.clone(), lifecycle)
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?
            .into_occurrence();
        self.store.put_occurrence(updated.clone()).await?;
        let mut receipt = updated.last_receipt.clone().unwrap_or_else(|| {
            DeliveryReceipt::new(updated.occurrence_id.clone(), updated.attempt_count, stage)
        });
        receipt.stage = stage;
        receipt.failure_class = updated.failure_class.clone();
        receipt.detail = updated.failure_detail.clone();
        self.store.append_receipt(receipt).await?;
        Ok(())
    }
}

#[cfg(test)]
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

    #[tokio::test]
    async fn driver_preserves_target_materialization_failure_classification() {
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
            .await
            .expect("schedule should be created");

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

        let report = driver.tick_once().await.expect("tick should succeed");
        assert_eq!(report.claimed_occurrences, 1);
        assert_eq!(report.terminalized_occurrences, 1);

        let occurrences = service
            .list_occurrences(&schedule.schedule_id)
            .await
            .expect("occurrences should list");
        let occurrence = occurrences
            .iter()
            .find(|occurrence| occurrence.phase == OccurrencePhase::DeliveryFailed)
            .expect("occurrence should terminalize as delivery_failed");

        assert_eq!(
            occurrence.failure_class,
            Some(OccurrenceFailureClass::TargetMaterializationFailed)
        );
        assert_eq!(
            occurrence.failure_detail.as_deref(),
            Some("session creation failed")
        );

        let receipts = store
            .list_receipts(&occurrence.occurrence_id)
            .await
            .expect("receipts should list");
        let last_receipt = receipts
            .last()
            .expect("delivery failure receipt should exist");
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::DeliveryFailed);
        assert_eq!(
            last_receipt.failure_class,
            Some(OccurrenceFailureClass::TargetMaterializationFailed)
        );
    }

    fn materialize_on_demand_target(prompt: &str) -> TargetBinding {
        TargetBinding::Session(SessionTargetBinding::MaterializeOnDemandSession {
            create: SessionMaterializationSpec {
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
            action: ScheduledSessionAction::Prompt {
                prompt: ContentInput::from(prompt),
                system_prompt: None,
                render_metadata: None,
                skill_references: Vec::new(),
                additional_instructions: Vec::new(),
            },
            bound_session_id: None,
        })
    }
}
