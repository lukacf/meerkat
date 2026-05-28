use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Duration as ChronoDuration;
use meerkat_core::{ContentInput, SessionId, skills::SkillRef, types::RenderMetadata};
use meerkat_runtime::{
    CompletionHandle,
    completion::{CompletionOutcome, CompletionWaitError},
};
use meerkat_schedule::{
    DeliveryCompletion, DeliveryCompletionFailureReason, DeliveryDispatch, DeliveryFailureReason,
    DeliveryReceipt, DeliveryReceiptStage, DeliveryTerminal, MobTargetBinding, Occurrence,
    OccurrencePhase, ScheduleDomainError, ScheduleDriver, ScheduleDriverConfig, ScheduleService,
    ScheduleStoreKind, ScheduleTargetDelivery, ScheduleTargetProbe, ScheduledSessionAction,
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

pub enum AcceptedScheduledInputCompletion {
    RuntimeHandle(CompletionHandle),
    RuntimeCompletionAuthorityUnavailable { detail: String },
}

pub struct AcceptedScheduledInput {
    pub correlation_id: Option<String>,
    pub completion: AcceptedScheduledInputCompletion,
}

impl AcceptedScheduledInput {
    pub fn with_runtime_handle(correlation_id: Option<String>, handle: CompletionHandle) -> Self {
        Self {
            correlation_id,
            completion: AcceptedScheduledInputCompletion::RuntimeHandle(handle),
        }
    }

    pub fn with_authority_unavailable(
        correlation_id: Option<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            correlation_id,
            completion: AcceptedScheduledInputCompletion::RuntimeCompletionAuthorityUnavailable {
                detail: detail.into(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScheduledPromptDispatch {
    pub prompt: ContentInput,
    pub render_metadata: Option<RenderMetadata>,
    pub skill_refs: Vec<SkillRef>,
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
            DeliveryFailureReason::MobRejected,
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
                                DeliveryFailureReason::InternalError,
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
                        DeliveryFailureReason::TargetMaterializationFailed,
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
                        skill_refs,
                        additional_instructions,
                    } => {
                        if system_prompt.is_some() && !resolved.allow_system_prompt_override {
                            return Ok(immediate_delivery_failure(
                                occurrence,
                                "scheduled system_prompt override is only supported when materializing a new session"
                                    .to_string(),
                                DeliveryFailureReason::RuntimeRejected,
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
                                    skill_refs: skill_refs.clone(),
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

    let completion = schedule_completion_from_runtime_completion(
        accepted.completion,
        materialized_session_id.clone(),
    );

    DeliveryDispatch {
        receipt,
        correlation_id: accepted.correlation_id,
        materialized_session_id,
        completion,
    }
}

fn schedule_completion_from_runtime_completion(
    completion: AcceptedScheduledInputCompletion,
    materialized_session_id: Option<SessionId>,
) -> DeliveryCompletion {
    Box::pin(async move {
        let outcome = match completion {
            AcceptedScheduledInputCompletion::RuntimeHandle(handle) => {
                match handle.try_wait().await {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return Err(ScheduleDomainError::DeliveryCompletionFailed {
                            reason: completion_wait_failure_reason(&error),
                            detail: format!("runtime completion authority unavailable: {error}"),
                        });
                    }
                }
            }
            AcceptedScheduledInputCompletion::RuntimeCompletionAuthorityUnavailable { detail } => {
                return Err(ScheduleDomainError::DeliveryCompletionFailed {
                    reason: DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable,
                    detail,
                });
            }
        };
        Ok(delivery_terminal_from_completion_outcome(
            outcome,
            materialized_session_id,
        ))
    })
}

fn completion_wait_failure_reason(error: &CompletionWaitError) -> DeliveryCompletionFailureReason {
    match error {
        CompletionWaitError::ChannelClosed => {
            DeliveryCompletionFailureReason::RuntimeCompletionChannelClosed
        }
        CompletionWaitError::AuthorityUnavailable(_) => {
            DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable
        }
    }
}

fn delivery_terminal_from_completion_outcome(
    outcome: CompletionOutcome,
    _materialized_session_id: Option<SessionId>,
) -> DeliveryTerminal {
    match outcome {
        CompletionOutcome::Completed(_) | CompletionOutcome::CompletedWithoutResult => {
            DeliveryTerminal::runtime_completion(
                meerkat_schedule::RuntimeCompletionOutcome::Completed,
                None,
                None,
            )
        }
        CompletionOutcome::CallbackPending { tool_name, args } => {
            let runtime_outcome =
                meerkat_schedule::RuntimeDeliveryOutcome::CompletionCallbackPending {
                    tool_name,
                    payload: args,
                };
            terminal_from_runtime_completion_outcome(
                meerkat_schedule::RuntimeCompletionOutcome::CallbackPending,
                runtime_outcome,
            )
        }
        CompletionOutcome::Cancelled => {
            let runtime_outcome = meerkat_schedule::RuntimeDeliveryOutcome::CompletionAbandoned {
                detail: "request cancelled".to_string(),
            };
            terminal_from_runtime_completion_outcome(
                meerkat_schedule::RuntimeCompletionOutcome::Cancelled,
                runtime_outcome,
            )
        }
        CompletionOutcome::Abandoned(reason) => {
            let runtime_outcome =
                meerkat_schedule::RuntimeDeliveryOutcome::CompletionAbandoned { detail: reason };
            terminal_from_runtime_completion_outcome(
                meerkat_schedule::RuntimeCompletionOutcome::Abandoned,
                runtime_outcome,
            )
        }
        CompletionOutcome::AbandonedWithError { reason, error } => {
            let error_detail =
                serde_json::to_string(&error).unwrap_or_else(|_| "<unserializable>".to_string());
            let runtime_outcome = meerkat_schedule::RuntimeDeliveryOutcome::CompletionAbandoned {
                detail: format!("{reason}; error={error_detail}"),
            };
            terminal_from_runtime_completion_outcome(
                meerkat_schedule::RuntimeCompletionOutcome::Abandoned,
                runtime_outcome,
            )
        }
        CompletionOutcome::CompletedWithFinalizationFailure { error, .. } => {
            DeliveryTerminal::runtime_completion(
                meerkat_schedule::RuntimeCompletionOutcome::FinalizationFailed,
                Some(
                    error
                        .detail
                        .unwrap_or_else(|| "turn finalization failed".to_string()),
                ),
                None,
            )
        }
        CompletionOutcome::RuntimeTerminated(reason) => {
            let runtime_outcome =
                meerkat_schedule::RuntimeDeliveryOutcome::CompletionRuntimeTerminated {
                    detail: reason,
                };
            terminal_from_runtime_completion_outcome(
                meerkat_schedule::RuntimeCompletionOutcome::RuntimeTerminated,
                runtime_outcome,
            )
        }
    }
}

fn terminal_from_runtime_completion_outcome(
    outcome: meerkat_schedule::RuntimeCompletionOutcome,
    runtime_outcome: meerkat_schedule::RuntimeDeliveryOutcome,
) -> DeliveryTerminal {
    let detail = runtime_outcome.detail();
    DeliveryTerminal::runtime_completion(outcome, Some(detail), Some(runtime_outcome))
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
    failure_reason: DeliveryFailureReason,
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
        materialized_session_id,
        completion: Box::pin(async move {
            Ok(DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some(detail),
                delivery_failure_reason: Some(failure_reason),
                runtime_completion_outcome: None,
                runtime_outcome: None,
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    fn sample_occurrence() -> Occurrence {
        let schedule = meerkat_schedule::Schedule::new(meerkat_schedule::CreateScheduleRequest {
            name: Some("schedule-host-test".to_string()),
            description: None,
            trigger: meerkat_schedule::TriggerSpec::Interval(
                meerkat_schedule::IntervalTriggerSpec {
                    start_at_utc: chrono::Utc::now(),
                    every_seconds: 60,
                    end_at_utc: None,
                },
            ),
            target: TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id: SessionId::new(),
                action: ScheduledSessionAction::Prompt {
                    prompt: ContentInput::Text("hello".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }),
            misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
            overlap_policy: meerkat_schedule::OverlapPolicy::SkipIfRunning,
            missing_target_policy: meerkat_schedule::MissingTargetPolicy::Skip,
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("sample schedule creation should pass generated authority");
        let mut occurrence = Occurrence::planned_from_schedule(
            &schedule,
            meerkat_schedule::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("sample occurrence planning should pass generated authority");
        occurrence.attempt_count = 1;
        occurrence
    }

    #[tokio::test]
    async fn noop_mob_host_reports_clear_feature_required_failure() {
        let host = NoopScheduleMobHost::new(
            "scheduled mob targets require the mob feature on the CLI host",
        );
        let binding = MobTargetBinding::Member {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            action: meerkat_schedule::ScheduledMobAction::Send {
                content: ContentInput::Text("Check deploy state.".to_string()),
                render_metadata: None,
            },
        };

        let probe = host
            .probe_mob_target(&binding)
            .await
            .expect("probe should succeed");
        let TargetProbeOutcome::Missing { detail } = probe else {
            panic!("expected no-op mob host to report missing, got {probe:?}");
        };
        assert_eq!(
            detail.as_deref(),
            Some("scheduled mob targets require the mob feature on the CLI host")
        );

        let occurrence = sample_occurrence();
        let dispatch = host
            .deliver_mob_target(&occurrence, &binding)
            .await
            .expect("delivery dispatch");
        let terminal = dispatch.completion.await.expect("delivery terminal");

        assert_eq!(terminal.phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            terminal.detail.as_deref(),
            Some("scheduled mob targets require the mob feature on the CLI host")
        );
        assert_eq!(
            terminal.delivery_failure_reason,
            Some(DeliveryFailureReason::MobRejected)
        );
    }

    #[tokio::test]
    async fn accepted_schedule_dispatch_waits_for_runtime_completion_failure() {
        let terminal = delivery_terminal_from_completion_outcome(
            CompletionOutcome::CallbackPending {
                tool_name: "external_approval".to_string(),
                args: serde_json::json!({"ticket": "INC-1"}),
            },
            None,
        );

        assert_eq!(terminal.phase, OccurrencePhase::AwaitingCompletion);
        assert_eq!(
            terminal.runtime_completion_outcome,
            Some(meerkat_schedule::RuntimeCompletionOutcome::CallbackPending)
        );
        assert!(
            terminal
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("external_approval")
        );
        assert!(terminal.runtime_outcome.is_some());
    }

    #[tokio::test]
    async fn accepted_schedule_dispatch_without_runtime_authority_reports_typed_completion_failure()
    {
        let occurrence = sample_occurrence();
        let dispatch = build_dispatch_from_accepted(
            &occurrence,
            AcceptedScheduledInput::with_authority_unavailable(
                Some("corr-1".to_string()),
                "runtime completion authority unavailable for terminal input",
            ),
            None,
        );

        let error = dispatch.completion.await.expect_err("completion failure");
        match error {
            ScheduleDomainError::DeliveryCompletionFailed { reason, detail } => {
                assert_eq!(
                    reason,
                    DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable
                );
                assert_eq!(
                    detail,
                    "runtime completion authority unavailable for terminal input"
                );
            }
            other => panic!("unexpected completion error: {other}"),
        }
    }
}
