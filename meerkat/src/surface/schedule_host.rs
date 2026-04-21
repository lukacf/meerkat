use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Duration as ChronoDuration;
use meerkat_core::{ContentInput, SessionId, types::RenderMetadata};
use meerkat_runtime::{AcceptOutcome, CompletionHandle, CompletionOutcome};
use meerkat_schedule::{
    DeliveryCompletion, DeliveryDispatch, DeliveryReceipt, DeliveryReceiptStage, DeliveryTerminal,
    MobTargetBinding, Occurrence, OccurrenceFailureClass, OccurrencePhase, ScheduleDomainError,
    ScheduleDriver, ScheduleDriverConfig, ScheduleService, ScheduleStoreKind,
    ScheduleTargetDelivery, ScheduleTargetProbe, ScheduledSessionAction,
    SessionMaterializationSpec, SessionTargetBinding, TargetBinding, TargetProbeOutcome,
};

#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::oneshot;
#[cfg(not(target_arch = "wasm32"))]
use tokio::task::JoinHandle;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::oneshot;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::task::JoinHandle;

pub struct ScheduleHostHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: JoinHandle<()>,
}

impl ScheduleHostHandle {
    pub async fn shutdown(mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        let _ = self.join.await;
    }
}

#[derive(Debug, Clone)]
struct ResolvedScheduledSession {
    session_id: SessionId,
    materialized_session_id: Option<SessionId>,
    allow_system_prompt_override: bool,
}

pub struct AcceptedScheduledInput {
    pub correlation_id: Option<String>,
    pub handle: Option<CompletionHandle>,
}

/// Typed projection of a runtime admission outcome into the schedule surface.
///
/// The schedule taxonomy (`OccurrenceFailureClass`) has domain-specific variants
/// (target/lease/mob) that the runtime does not know about, so we keep it distinct
/// from the runtime's `AcceptOutcome`. Callers exhaustively match this projection
/// and route the rejected arm through `immediate_delivery_failure` with the
/// correct failure class, rather than laundering it back through the accepted
/// path with a sentinel correlation id.
pub enum RuntimeAdmissionProjection {
    Admitted(AcceptedScheduledInput),
    Rejected { detail: String },
}

#[derive(Debug, Clone)]
pub struct ScheduledPromptDispatch {
    pub prompt: ContentInput,
    pub render_metadata: Option<RenderMetadata>,
    pub skill_references: Vec<String>,
    pub additional_instructions: Vec<String>,
    pub materialized_session_id: Option<SessionId>,
}

#[async_trait]
pub trait SurfaceScheduleSessionHost: Send + Sync {
    async fn probe_session_target(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError>;

    async fn materialize_session(
        &self,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError>;

    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;

    async fn deliver_event(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<RenderMetadata>,
        materialized_session_id: Option<SessionId>,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;
}

#[async_trait]
pub trait SurfaceScheduleMobHost: Send + Sync {
    async fn probe_mob_target(
        &self,
        binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError>;

    async fn deliver_mob_target(
        &self,
        occurrence: &Occurrence,
        binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;
}

pub struct NoopScheduleMobHost {
    detail: String,
}

impl NoopScheduleMobHost {
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

#[async_trait]
impl SurfaceScheduleMobHost for NoopScheduleMobHost {
    async fn probe_mob_target(
        &self,
        _binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        Ok(TargetProbeOutcome::Missing {
            detail: Some(self.detail.clone()),
        })
    }

    async fn deliver_mob_target(
        &self,
        occurrence: &Occurrence,
        _binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        Ok(immediate_delivery_failure(
            occurrence,
            self.detail.clone(),
            OccurrenceFailureClass::MobRejected,
            None,
            None,
        ))
    }
}

pub struct SharedScheduleTargetAdapter {
    schedule_service: ScheduleService,
    session_host: Arc<dyn SurfaceScheduleSessionHost>,
    mob_host: Arc<dyn SurfaceScheduleMobHost>,
}

impl SharedScheduleTargetAdapter {
    pub fn new(
        schedule_service: ScheduleService,
        session_host: Arc<dyn SurfaceScheduleSessionHost>,
        mob_host: Arc<dyn SurfaceScheduleMobHost>,
    ) -> Self {
        Self {
            schedule_service,
            session_host,
            mob_host,
        }
    }

    async fn resolve_session(
        &self,
        occurrence: &Occurrence,
        binding: &SessionTargetBinding,
    ) -> Result<ResolvedScheduledSession, DeliveryDispatch> {
        match binding {
            SessionTargetBinding::ExactSession { session_id, .. }
            | SessionTargetBinding::ResumableSession { session_id, .. } => {
                Ok(ResolvedScheduledSession {
                    session_id: session_id.clone(),
                    materialized_session_id: None,
                    allow_system_prompt_override: false,
                })
            }
            SessionTargetBinding::MaterializeOnDemandSession {
                bound_session_id: Some(session_id),
                ..
            } => Ok(ResolvedScheduledSession {
                session_id: session_id.clone(),
                materialized_session_id: Some(session_id.clone()),
                allow_system_prompt_override: false,
            }),
            SessionTargetBinding::MaterializeOnDemandSession {
                create,
                action,
                bound_session_id: None,
            } => {
                let prompt_system_prompt = match action {
                    ScheduledSessionAction::Prompt { system_prompt, .. } => {
                        system_prompt.as_deref()
                    }
                    ScheduledSessionAction::Event { .. } => None,
                };
                match self
                    .session_host
                    .materialize_session(create, prompt_system_prompt)
                    .await
                {
                    Ok(session_id) => {
                        if let Err(error) = self
                            .schedule_service
                            .bind_materialized_session_for_occurrence(occurrence, &session_id)
                            .await
                        {
                            return Err(immediate_delivery_failure(
                                occurrence,
                                error.to_string(),
                                OccurrenceFailureClass::InternalError,
                                None,
                                Some(session_id),
                            ));
                        }
                        Ok(ResolvedScheduledSession {
                            session_id: session_id.clone(),
                            materialized_session_id: Some(session_id),
                            allow_system_prompt_override: true,
                        })
                    }
                    Err(error) => Err(immediate_delivery_failure(
                        occurrence,
                        error.to_string(),
                        OccurrenceFailureClass::TargetMaterializationFailed,
                        None,
                        None,
                    )),
                }
            }
        }
    }
}

#[async_trait]
impl ScheduleTargetProbe for SharedScheduleTargetAdapter {
    async fn probe_target(
        &self,
        occurrence: &Occurrence,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        match &occurrence.target_snapshot {
            TargetBinding::Session(binding) => {
                self.session_host.probe_session_target(binding).await
            }
            TargetBinding::Mob(binding) => self.mob_host.probe_mob_target(binding).await,
        }
    }
}

#[async_trait]
impl ScheduleTargetDelivery for SharedScheduleTargetAdapter {
    async fn deliver_occurrence(
        &self,
        occurrence: &Occurrence,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        match &occurrence.target_snapshot {
            TargetBinding::Session(binding) => {
                let resolved = match self.resolve_session(occurrence, binding).await {
                    Ok(resolved) => resolved,
                    Err(dispatch) => return Ok(dispatch),
                };

                match binding.action() {
                    ScheduledSessionAction::Prompt {
                        prompt,
                        system_prompt,
                        render_metadata,
                        skill_references,
                        additional_instructions,
                    } => {
                        if system_prompt.is_some() && !resolved.allow_system_prompt_override {
                            return Ok(immediate_delivery_failure(
                                occurrence,
                                "scheduled system_prompt override is only supported when materializing a new session"
                                    .to_string(),
                                OccurrenceFailureClass::RuntimeRejected,
                                None,
                                resolved.materialized_session_id,
                            ));
                        }
                        self.session_host
                            .deliver_prompt(
                                &resolved.session_id,
                                occurrence,
                                ScheduledPromptDispatch {
                                    prompt: prompt.clone(),
                                    render_metadata: render_metadata.clone(),
                                    skill_references: skill_references.clone(),
                                    additional_instructions: additional_instructions.clone(),
                                    materialized_session_id: resolved.materialized_session_id,
                                },
                            )
                            .await
                    }
                    ScheduledSessionAction::Event {
                        event_type,
                        payload,
                        render_metadata,
                    } => {
                        self.session_host
                            .deliver_event(
                                &resolved.session_id,
                                occurrence,
                                event_type.clone(),
                                payload.clone(),
                                render_metadata.clone(),
                                resolved.materialized_session_id,
                            )
                            .await
                    }
                }
            }
            TargetBinding::Mob(binding) => {
                self.mob_host.deliver_mob_target(occurrence, binding).await
            }
        }
    }
}

pub fn schedule_host_supported(kind: ScheduleStoreKind) -> bool {
    !matches!(kind, ScheduleStoreKind::Disabled | ScheduleStoreKind::Jsonl)
}

pub fn spawn_schedule_host(
    schedule_service: ScheduleService,
    adapter: Arc<SharedScheduleTargetAdapter>,
    owner_id: impl Into<String>,
) -> ScheduleHostHandle {
    let driver = Arc::new(ScheduleDriver::new(
        schedule_service.clone(),
        schedule_service.store(),
        adapter.clone(),
        adapter,
        owner_id,
        ScheduleDriverConfig {
            claim_limit: 32,
            lease_duration: ChronoDuration::seconds(60),
        },
    ));
    let poll_interval = if cfg!(test) {
        Duration::from_millis(50)
    } else {
        Duration::from_millis(250)
    };
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    #[cfg(not(target_arch = "wasm32"))]
    let join = tokio::spawn(async move {
        let mut interval = tokio::time::interval(poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                _ = interval.tick() => {
                    let _ = driver.tick_once().await;
                }
            }
        }
    });
    #[cfg(target_arch = "wasm32")]
    let join = tokio_with_wasm::alias::task::spawn(async move {
        let mut interval = tokio_with_wasm::alias::time::interval(poll_interval);
        loop {
            tokio_with_wasm::alias::select! {
                _ = &mut shutdown_rx => break,
                () = interval.tick() => {
                    let _ = driver.tick_once().await;
                }
            }
        }
    });

    ScheduleHostHandle {
        shutdown_tx: Some(shutdown_tx),
        join,
    }
}

pub fn project_runtime_admission(
    outcome: AcceptOutcome,
    handle: Option<CompletionHandle>,
) -> RuntimeAdmissionProjection {
    match outcome {
        AcceptOutcome::Accepted { input_id, .. } => {
            RuntimeAdmissionProjection::Admitted(AcceptedScheduledInput {
                correlation_id: Some(input_id.to_string()),
                handle,
            })
        }
        AcceptOutcome::Deduplicated { existing_id, .. } => {
            RuntimeAdmissionProjection::Admitted(AcceptedScheduledInput {
                correlation_id: Some(existing_id.to_string()),
                handle,
            })
        }
        AcceptOutcome::Rejected { reason } => RuntimeAdmissionProjection::Rejected {
            detail: reason.to_string(),
        },
        // `AcceptOutcome` is `#[non_exhaustive]`; treat additions as rejections
        // until the surface is taught to project them.
        other => RuntimeAdmissionProjection::Rejected {
            detail: format!("unsupported runtime accept outcome: {other:?}"),
        },
    }
}

/// Build a `DeliveryDispatch` from a runtime admission projection.
///
/// Admitted inputs flow through `build_dispatch_from_accepted`; rejected inputs
/// flow through `immediate_delivery_failure` with `OccurrenceFailureClass::RuntimeRejected`.
pub fn dispatch_from_admission(
    occurrence: &Occurrence,
    projection: RuntimeAdmissionProjection,
    materialized_session_id: Option<SessionId>,
) -> DeliveryDispatch {
    match projection {
        RuntimeAdmissionProjection::Admitted(accepted) => {
            build_dispatch_from_accepted(occurrence, accepted, materialized_session_id)
        }
        RuntimeAdmissionProjection::Rejected { detail } => immediate_delivery_failure(
            occurrence,
            detail,
            OccurrenceFailureClass::RuntimeRejected,
            None,
            materialized_session_id,
        ),
    }
}

pub fn build_dispatch_from_accepted(
    occurrence: &Occurrence,
    accepted: AcceptedScheduledInput,
    materialized_session_id: Option<SessionId>,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchAccepted,
    );
    receipt.correlation_id = accepted.correlation_id.clone();
    receipt.materialized_session_id = materialized_session_id.clone();

    let occurrence_id = occurrence.occurrence_id.clone();
    let attempt_count = occurrence.attempt_count;
    let correlation_id = accepted.correlation_id.clone();
    let completed_materialized_session_id = materialized_session_id.clone();
    let completion = match accepted.handle {
        Some(handle) => scheduled_completion_future(handle, materialized_session_id.clone()),
        None => Box::pin(async move {
            let mut receipt = DeliveryReceipt::new(
                occurrence_id,
                attempt_count,
                DeliveryReceiptStage::Completed,
            );
            receipt.correlation_id = correlation_id;
            receipt.materialized_session_id = completed_materialized_session_id;
            Ok(DeliveryTerminal::completed(Some(receipt)))
        }),
    };

    DeliveryDispatch {
        receipt,
        correlation_id: accepted.correlation_id,
        materialized_session_id,
        completion,
    }
}

pub fn immediate_completed_dispatch(
    occurrence: &Occurrence,
    correlation_id: Option<String>,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchAccepted,
    );
    receipt.correlation_id = correlation_id.clone();
    DeliveryDispatch {
        receipt,
        correlation_id,
        materialized_session_id: None,
        completion: Box::pin(async { Ok(DeliveryTerminal::completed(None)) }),
    }
}

pub fn async_completion_dispatch(
    occurrence: &Occurrence,
    correlation_id: Option<String>,
    completion: DeliveryCompletion,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchAccepted,
    );
    receipt.correlation_id = correlation_id.clone();
    DeliveryDispatch {
        receipt,
        correlation_id,
        materialized_session_id: None,
        completion,
    }
}

pub fn immediate_delivery_failure(
    occurrence: &Occurrence,
    detail: String,
    failure_class: OccurrenceFailureClass,
    correlation_id: Option<String>,
    materialized_session_id: Option<SessionId>,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchStarted,
    );
    receipt.correlation_id = correlation_id.clone();
    receipt.materialized_session_id = materialized_session_id.clone();
    DeliveryDispatch {
        receipt,
        correlation_id,
        materialized_session_id: materialized_session_id.clone(),
        completion: Box::pin(async move {
            Ok(DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some(detail),
                failure_class: Some(failure_class),
                materialized_session_id,
            })
        }),
    }
}

pub fn schedule_attempt_idempotency_key(occurrence: &Occurrence) -> String {
    format!(
        "schedule:{}:occurrence:{}:attempt:{}",
        occurrence.schedule_id, occurrence.occurrence_id, occurrence.attempt_count
    )
}

/// Project a runtime `CompletionOutcome` into a schedule `DeliveryTerminal`.
///
/// This is an exhaustive typed mapping from the runtime's completion taxonomy
/// into the schedule's failure-class taxonomy (see `OccurrenceFailureClass`).
/// The taxonomies are distinct: the runtime surfaces callback/terminated
/// semantics that the schedule domain models as delivery failure categories.
fn scheduled_completion_future(
    handle: CompletionHandle,
    materialized_session_id: Option<SessionId>,
) -> DeliveryCompletion {
    Box::pin(async move {
        Ok(match handle.wait().await {
            CompletionOutcome::Completed(_) | CompletionOutcome::CompletedWithoutResult => {
                DeliveryTerminal {
                    phase: OccurrencePhase::Completed,
                    receipt: None,
                    detail: None,
                    failure_class: None,
                    materialized_session_id,
                }
            }
            CompletionOutcome::Abandoned(reason) => DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some(reason),
                failure_class: Some(OccurrenceFailureClass::RuntimeRejected),
                materialized_session_id,
            },
            CompletionOutcome::CallbackPending { tool_name, args } => DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some(format!("callback pending for tool '{tool_name}': {args}")),
                failure_class: Some(OccurrenceFailureClass::RuntimeRejected),
                materialized_session_id,
            },
            CompletionOutcome::RuntimeTerminated(reason) => DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some(reason),
                failure_class: Some(OccurrenceFailureClass::TransportError),
                materialized_session_id,
            },
        })
    })
}
