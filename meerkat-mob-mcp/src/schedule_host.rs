use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat::surface::{
    ScheduleAdmissionOutcome, SurfaceScheduleMobHost,
    async_completion_dispatch_with_admission_outcome,
    immediate_completed_dispatch_with_admission_outcome, immediate_delivery_failure,
    parse_mob_member_schedule_identity,
};
use meerkat::{
    DeliveryCompletion, DeliveryDispatch, DeliveryFailureReason, DeliveryTerminal, ForkContextSpec,
    HelperOptionsSpec, IdentityTargetBinding, MobTargetBinding, Occurrence,
    ScheduleDeliveryIdentity, ScheduleDomainError, ScheduleSpawnTooling, ScheduledMobAction,
    ScheduledMobBackendKind, ScheduledMobRuntimeMode, ScheduledSessionAction, TargetProbeOutcome,
};
use meerkat_core::types::{ContentInput, HandlingMode, RenderMetadata};
use meerkat_mob::{
    AgentIdentity, FlowId, ForkContext, HelperOptions, MobBackendKind, MobError,
    MobExternalDeliveryBeginOutcome, MobExternalDeliveryCompleteOutcome,
    MobExternalDeliveryIdentity, MobExternalDeliveryIntent, MobExternalDeliveryRepairOutcome,
    MobExternalDeliveryRepairState, MobExternalDeliveryTargetKind, MobExternalDeliveryTerminal,
    MobFailureClass, MobFlowRunPublicResultClass, MobId, MobRunStatus, RunId,
    mob_machine_run_public_result_class, mob_machine_run_status_is_terminal,
};

use crate::MobMcpState;

#[cfg(target_arch = "wasm32")]
use crate::tokio::time::sleep;
#[cfg(not(target_arch = "wasm32"))]
use tokio::time::sleep;

#[async_trait]
pub(crate) trait ScheduleMobRuntime: Send + Sync {
    async fn begin_external_delivery(
        &self,
        intent: &MobExternalDeliveryIntent,
    ) -> Result<MobExternalDeliveryBeginOutcome, MobError>;

    async fn complete_external_delivery(
        &self,
        intent: &MobExternalDeliveryIntent,
        terminal: &MobExternalDeliveryTerminal,
    ) -> Result<MobExternalDeliveryCompleteOutcome, MobError>;

    async fn schedule_external_delivery_repair(
        &self,
        intent: &MobExternalDeliveryIntent,
    ) -> Result<MobExternalDeliveryRepairOutcome, MobError>;

    /// Explicit target-owned proof that re-entering one `ExistingBegun`
    /// delivery with the same stable identity cannot repeat its effect.
    fn supports_external_delivery_redrive(
        &self,
        target_kind: MobExternalDeliveryTargetKind,
    ) -> bool;

    async fn member_exists(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<bool, MobError>;

    async fn flow_exists(&self, mob_id: &MobId, flow_id: &FlowId) -> Result<bool, MobError>;

    async fn member_send(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        content: ContentInput,
        render_metadata: Option<RenderMetadata>,
        delivery_identity: MobExternalDeliveryIdentity,
    ) -> Result<(), MobError>;

    async fn run_flow(
        &self,
        mob_id: &MobId,
        flow_id: FlowId,
        params: serde_json::Value,
        delivery_identity: &MobExternalDeliveryIdentity,
    ) -> Result<RunId, MobError>;

    async fn flow_status(
        &self,
        mob_id: &MobId,
        run_id: RunId,
    ) -> Result<Option<MobRunStatus>, MobError>;

    async fn spawn_helper(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        prompt: String,
        options: HelperOptions,
    ) -> Result<(), MobError>;

    async fn fork_helper(
        &self,
        mob_id: &MobId,
        source_identity: &AgentIdentity,
        identity: AgentIdentity,
        prompt: String,
        fork_context: ForkContext,
        options: HelperOptions,
    ) -> Result<(), MobError>;
}

#[async_trait]
impl ScheduleMobRuntime for MobMcpState {
    async fn begin_external_delivery(
        &self,
        intent: &MobExternalDeliveryIntent,
    ) -> Result<MobExternalDeliveryBeginOutcome, MobError> {
        self.mob_begin_external_delivery(intent).await
    }

    async fn complete_external_delivery(
        &self,
        intent: &MobExternalDeliveryIntent,
        terminal: &MobExternalDeliveryTerminal,
    ) -> Result<MobExternalDeliveryCompleteOutcome, MobError> {
        self.mob_complete_external_delivery(intent, terminal).await
    }

    async fn schedule_external_delivery_repair(
        &self,
        intent: &MobExternalDeliveryIntent,
    ) -> Result<MobExternalDeliveryRepairOutcome, MobError> {
        self.mob_schedule_external_delivery_repair(intent).await
    }

    fn supports_external_delivery_redrive(
        &self,
        target_kind: MobExternalDeliveryTargetKind,
    ) -> bool {
        match target_kind {
            // A stable input id only remains a dedupe proof across process
            // death when the member session ledger is itself durable.
            MobExternalDeliveryTargetKind::MemberSend => {
                self.session_service().supports_persistent_sessions()
            }
            // A flow's RunId is stable, but its pre-run target provisioner is
            // an embedder-owned effect with no delivery-identity/dedupe
            // contract. The RunId therefore cannot authorize replay of the
            // whole target boundary after an ambiguous Begin.
            MobExternalDeliveryTargetKind::Flow
            | MobExternalDeliveryTargetKind::SpawnHelper
            | MobExternalDeliveryTargetKind::ForkHelper => false,
            _ => false,
        }
    }

    async fn member_exists(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<bool, MobError> {
        Ok(self
            .handle_for(mob_id)
            .await?
            .get_member(identity)
            .await?
            .is_some())
    }

    async fn flow_exists(&self, mob_id: &MobId, flow_id: &FlowId) -> Result<bool, MobError> {
        Ok(self
            .handle_for(mob_id)
            .await?
            .list_flows()
            .into_iter()
            .any(|candidate| candidate == *flow_id))
    }

    async fn member_send(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        content: ContentInput,
        render_metadata: Option<RenderMetadata>,
        delivery_identity: MobExternalDeliveryIdentity,
    ) -> Result<(), MobError> {
        self.mob_member_send_with_external_identity(
            mob_id,
            identity,
            content,
            HandlingMode::Queue,
            render_metadata,
            delivery_identity,
        )
        .await
        .map(|_| ())
    }

    async fn run_flow(
        &self,
        mob_id: &MobId,
        flow_id: FlowId,
        params: serde_json::Value,
        delivery_identity: &MobExternalDeliveryIdentity,
    ) -> Result<RunId, MobError> {
        self.mob_run_flow_with_external_identity(mob_id, flow_id, params, delivery_identity)
            .await
    }

    async fn flow_status(
        &self,
        mob_id: &MobId,
        run_id: RunId,
    ) -> Result<Option<MobRunStatus>, MobError> {
        Ok(self
            .mob_flow_status(mob_id, run_id)
            .await?
            .map(|run| run.status))
    }

    async fn spawn_helper(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        prompt: String,
        options: HelperOptions,
    ) -> Result<(), MobError> {
        self.mob_spawn_helper(mob_id, identity, prompt, options)
            .await
            .map(|_| ())
    }

    async fn fork_helper(
        &self,
        mob_id: &MobId,
        source_identity: &AgentIdentity,
        identity: AgentIdentity,
        prompt: String,
        fork_context: ForkContext,
        options: HelperOptions,
    ) -> Result<(), MobError> {
        self.mob_fork_helper(
            mob_id,
            source_identity,
            identity,
            prompt,
            fork_context,
            options,
        )
        .await
        .map(|_| ())
    }
}

/// Reusable schedule-to-mob delivery adapter for surfaces backed by `MobMcpState`.
///
/// Scheduled mob targets remain identity-first: member bindings are resolved as
/// `(mob_id, AgentIdentity)` by the mob runtime at probe/delivery time.
pub struct MobMcpScheduleHost {
    runtime: Arc<dyn ScheduleMobRuntime>,
}

impl MobMcpScheduleHost {
    pub fn new(state: Arc<MobMcpState>) -> Self {
        Self { runtime: state }
    }

    #[cfg(test)]
    pub(crate) fn from_runtime(runtime: Arc<dyn ScheduleMobRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl SurfaceScheduleMobHost for MobMcpScheduleHost {
    async fn probe_mob_target(
        &self,
        binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let mob_id = MobId::from(mob_binding_mob_id(binding));

        match binding {
            MobTargetBinding::Member { member_id, .. } => {
                let identity = AgentIdentity::from(member_id.as_str());
                match self.runtime.member_exists(&mob_id, &identity).await {
                    Ok(true) => Ok(TargetProbeOutcome::Ready),
                    Ok(false) => Ok(TargetProbeOutcome::Missing {
                        detail: Some(format!("mob member not found: {member_id}")),
                    }),
                    Err(error) => mob_probe_error_outcome(error),
                }
            }
            MobTargetBinding::Flow { flow_id, .. } => {
                let flow_id = FlowId::from(flow_id.as_str());
                match self.runtime.flow_exists(&mob_id, &flow_id).await {
                    Ok(true) => Ok(TargetProbeOutcome::Ready),
                    Ok(false) => Ok(TargetProbeOutcome::Missing {
                        detail: Some(format!("mob flow not found: {flow_id}")),
                    }),
                    Err(error) => mob_probe_error_outcome(error),
                }
            }
            MobTargetBinding::SpawnHelper { member_id, .. } => {
                let identity = AgentIdentity::from(member_id.as_str());
                match self.runtime.member_exists(&mob_id, &identity).await {
                    Ok(true) => Ok(TargetProbeOutcome::Busy {
                        detail: Some(format!("mob member already exists: {member_id}")),
                    }),
                    Ok(false) => Ok(TargetProbeOutcome::Ready),
                    Err(error) => mob_probe_error_outcome(error),
                }
            }
            MobTargetBinding::ForkHelper {
                source_member_id,
                member_id,
                ..
            } => {
                let source = AgentIdentity::from(source_member_id.as_str());
                match self.runtime.member_exists(&mob_id, &source).await {
                    Ok(true) => {}
                    Ok(false) => {
                        return Ok(TargetProbeOutcome::Missing {
                            detail: Some(format!(
                                "mob source member not found: {source_member_id}"
                            )),
                        });
                    }
                    Err(error) => return mob_probe_error_outcome(error),
                }

                let target = AgentIdentity::from(member_id.as_str());
                match self.runtime.member_exists(&mob_id, &target).await {
                    Ok(true) => Ok(TargetProbeOutcome::Busy {
                        detail: Some(format!("mob member already exists: {member_id}")),
                    }),
                    Ok(false) => Ok(TargetProbeOutcome::Ready),
                    Err(error) => mob_probe_error_outcome(error),
                }
            }
        }
    }

    async fn deliver_mob_target(
        &self,
        occurrence: &Occurrence,
        schedule_identity: &ScheduleDeliveryIdentity,
        binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let mob_id = MobId::from(mob_binding_mob_id(binding));
        validate_mob_delivery_binding(binding)?;
        let target_kind = match binding {
            MobTargetBinding::Member { .. } => MobExternalDeliveryTargetKind::MemberSend,
            MobTargetBinding::Flow { .. } => MobExternalDeliveryTargetKind::Flow,
            MobTargetBinding::SpawnHelper { .. } => MobExternalDeliveryTargetKind::SpawnHelper,
            MobTargetBinding::ForkHelper { .. } => MobExternalDeliveryTargetKind::ForkHelper,
        };
        let (delivery_identity, delivery_intent) = mob_delivery_intent(
            mob_id.clone(),
            schedule_identity,
            target_kind,
            binding
                .stable_key()
                .map_err(ScheduleDomainError::InvalidSchedule)?,
        )?;
        let (admission_outcome, repair_state) = match self
            .runtime
            .begin_external_delivery(&delivery_intent)
            .await
            .map_err(|error| ScheduleDomainError::DeliveryRepairDeferred {
                detail: format!(
                    "durable mob external-delivery Begin is unavailable before target effect: {error}"
                ),
            })?
        {
            MobExternalDeliveryBeginOutcome::ExistingTerminal(terminal) => {
                return Ok(dispatch_for_external_terminal(
                    occurrence,
                    schedule_identity,
                    terminal,
                    ScheduleAdmissionOutcome::Deduplicated,
                ));
            }
            MobExternalDeliveryBeginOutcome::ExistingBegun { repair }
                if !self
                    .runtime
                    .supports_external_delivery_redrive(target_kind) =>
            {
                return Ok(repair_refused_external_delivery_dispatch(
                    occurrence,
                    schedule_identity,
                    Arc::clone(&self.runtime),
                    delivery_intent,
                    target_kind,
                    repair,
                ));
            }
            MobExternalDeliveryBeginOutcome::Begun => {
                (ScheduleAdmissionOutcome::Accepted, None)
            }
            MobExternalDeliveryBeginOutcome::ExistingBegun { repair } => (
                ScheduleAdmissionOutcome::Deduplicated,
                Some(repair),
            ),
        };

        match binding {
            MobTargetBinding::Member {
                member_id,
                action:
                    ScheduledMobAction::Send {
                        content,
                        render_metadata,
                    },
                ..
            } => {
                let runtime = Arc::clone(&self.runtime);
                let effect_runtime = Arc::clone(&runtime);
                let repair_runtime = Arc::clone(&runtime);
                let effect_intent = delivery_intent.clone();
                let mob_id = mob_id.clone();
                let member_id = AgentIdentity::from(member_id.as_str());
                let content = content.clone();
                let render_metadata = render_metadata.clone();
                let delivery_identity = delivery_identity.clone();
                Ok(async_completion_dispatch_with_admission_outcome(
                    occurrence,
                    Some(schedule_identity.correlation_id.clone()),
                    admission_outcome,
                    durable_external_delivery_completion(
                        runtime,
                        delivery_intent,
                        repair_state,
                        Box::pin(async move {
                            wait_external_delivery_repair_deadline(repair_state).await?;
                            match effect_runtime
                                .member_send(
                                    &mob_id,
                                    member_id,
                                    content,
                                    render_metadata,
                                    delivery_identity,
                                )
                                .await
                            {
                                Ok(()) => Ok(DeliveryTerminal::completed(None)),
                                Err(error) => {
                                    defer_external_delivery_after_target_error(
                                        repair_runtime,
                                        &effect_intent,
                                        repair_state,
                                        error,
                                    )
                                    .await
                                }
                            }
                        }),
                    ),
                ))
            }
            MobTargetBinding::Flow {
                flow_id, params, ..
            } => {
                let params: serde_json::Value =
                    serde_json::from_str(params.get()).map_err(|error| {
                        ScheduleDomainError::InvalidSchedule(format!(
                            "invalid mob flow params: {error}"
                        ))
                    })?;
                let flow_id = FlowId::from(flow_id.as_str());
                let runtime = Arc::clone(&self.runtime);
                let effect_runtime = Arc::clone(&runtime);
                let repair_runtime = Arc::clone(&runtime);
                let effect_intent = delivery_intent.clone();
                let mob_id = mob_id.clone();
                let delivery_identity = delivery_identity.clone();
                Ok(async_completion_dispatch_with_admission_outcome(
                    occurrence,
                    Some(schedule_identity.correlation_id.clone()),
                    admission_outcome,
                    durable_external_delivery_completion(
                        runtime,
                        delivery_intent,
                        repair_state,
                        Box::pin(async move {
                            wait_external_delivery_repair_deadline(repair_state).await?;
                            let run_id = match effect_runtime
                                .run_flow(&mob_id, flow_id, params, &delivery_identity)
                                .await
                            {
                                Ok(run_id) => run_id,
                                Err(error) => {
                                    return defer_external_delivery_after_target_error(
                                        repair_runtime,
                                        &effect_intent,
                                        repair_state,
                                        error,
                                    )
                                    .await;
                                }
                            };
                            mob_flow_completion_future(effect_runtime, mob_id, run_id).await
                        }),
                    ),
                ))
            }
            MobTargetBinding::SpawnHelper {
                member_id,
                prompt,
                options,
                ..
            } => {
                let identity = AgentIdentity::from(member_id.as_str());
                let helper_options = helper_options_from_spec(options)?;
                let prompt = prompt.clone();
                let runtime = Arc::clone(&self.runtime);
                let effect_runtime = Arc::clone(&runtime);
                Ok(async_completion_dispatch_with_admission_outcome(
                    occurrence,
                    Some(schedule_identity.correlation_id.clone()),
                    admission_outcome,
                    durable_external_delivery_completion(
                        runtime,
                        delivery_intent,
                        repair_state,
                        Box::pin(async move {
                            match effect_runtime
                                .spawn_helper(&mob_id, identity, prompt, helper_options)
                                .await
                            {
                                Ok(()) => Ok(DeliveryTerminal::completed(None)),
                                Err(error) => Ok(mob_delivery_failed_terminal(error)),
                            }
                        }),
                    ),
                ))
            }
            MobTargetBinding::ForkHelper {
                source_member_id,
                member_id,
                prompt,
                fork_context,
                options,
                ..
            } => {
                let source_identity = AgentIdentity::from(source_member_id.as_str());
                let identity = AgentIdentity::from(member_id.as_str());
                let helper_options = helper_options_from_spec(options)?;
                let fork_context = fork_context_from_spec(fork_context);
                let prompt = prompt.clone();
                let runtime = Arc::clone(&self.runtime);
                let effect_runtime = Arc::clone(&runtime);
                Ok(async_completion_dispatch_with_admission_outcome(
                    occurrence,
                    Some(schedule_identity.correlation_id.clone()),
                    admission_outcome,
                    durable_external_delivery_completion(
                        runtime,
                        delivery_intent,
                        repair_state,
                        Box::pin(async move {
                            match effect_runtime
                                .fork_helper(
                                    &mob_id,
                                    &source_identity,
                                    identity,
                                    prompt,
                                    fork_context,
                                    helper_options,
                                )
                                .await
                            {
                                Ok(()) => Ok(DeliveryTerminal::completed(None)),
                                Err(error) => Ok(mob_delivery_failed_terminal(error)),
                            }
                        }),
                    ),
                ))
            }
        }
    }

    async fn probe_identity_target(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<TargetProbeOutcome>, ScheduleDomainError> {
        let Some(identity) = parse_mob_member_schedule_identity(binding.identity()) else {
            return Ok(None);
        };
        let mob_id = MobId::from(identity.mob_id.as_str());
        let member = AgentIdentity::from(identity.member.as_str());
        match self.runtime.member_exists(&mob_id, &member).await {
            Ok(true) => Ok(Some(TargetProbeOutcome::Ready)),
            Ok(false) => Ok(Some(TargetProbeOutcome::Missing {
                detail: Some(format!("mob member not found: {}", identity.member)),
            })),
            Err(error) => mob_probe_error_outcome(error).map(Some),
        }
    }

    async fn deliver_identity_target(
        &self,
        occurrence: &Occurrence,
        schedule_identity: &ScheduleDeliveryIdentity,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<DeliveryDispatch>, ScheduleDomainError> {
        let Some(identity) = parse_mob_member_schedule_identity(binding.identity()) else {
            return Ok(None);
        };
        let mob_id = MobId::from(identity.mob_id.as_str());
        let member = AgentIdentity::from(identity.member.as_str());
        let ScheduledSessionAction::Prompt {
            prompt,
            system_prompt,
            render_metadata,
            skill_refs,
            additional_instructions,
        } = binding.action()
        else {
            return Ok(Some(immediate_delivery_failure(
                occurrence,
                "scheduled mob-member identity targets only support prompt actions".to_string(),
                DeliveryFailureReason::RuntimeRejected,
                Some(schedule_identity.correlation_id.clone()),
                None,
            )));
        };
        if system_prompt.is_some() || !skill_refs.is_empty() || !additional_instructions.is_empty()
        {
            return Ok(Some(immediate_delivery_failure(
                occurrence,
                "scheduled mob-member identity targets do not support session-only prompt overrides"
                    .to_string(),
                DeliveryFailureReason::RuntimeRejected,
                Some(schedule_identity.correlation_id.clone()),
                None,
            )));
        }
        let (delivery_identity, delivery_intent) = mob_delivery_intent(
            mob_id.clone(),
            schedule_identity,
            MobExternalDeliveryTargetKind::MemberSend,
            binding
                .stable_key()
                .map_err(ScheduleDomainError::InvalidSchedule)?,
        )?;
        let (admission_outcome, repair_state) = match self
            .runtime
            .begin_external_delivery(&delivery_intent)
            .await
            .map_err(|error| ScheduleDomainError::DeliveryRepairDeferred {
                detail: format!(
                    "durable mob external-delivery Begin is unavailable before target effect: {error}"
                ),
            })?
        {
            MobExternalDeliveryBeginOutcome::Begun => {
                (ScheduleAdmissionOutcome::Accepted, None)
            }
            MobExternalDeliveryBeginOutcome::ExistingBegun { repair }
                if self
                    .runtime
                    .supports_external_delivery_redrive(
                        MobExternalDeliveryTargetKind::MemberSend,
                    ) =>
            {
                (
                    ScheduleAdmissionOutcome::Deduplicated,
                    Some(repair),
                )
            }
            MobExternalDeliveryBeginOutcome::ExistingBegun { repair } => {
                return Ok(Some(repair_refused_external_delivery_dispatch(
                    occurrence,
                    schedule_identity,
                    Arc::clone(&self.runtime),
                    delivery_intent,
                    MobExternalDeliveryTargetKind::MemberSend,
                    repair,
                )));
            }
            MobExternalDeliveryBeginOutcome::ExistingTerminal(terminal) => {
                return Ok(Some(dispatch_for_external_terminal(
                    occurrence,
                    schedule_identity,
                    terminal,
                    ScheduleAdmissionOutcome::Deduplicated,
                )));
            }
        };
        let runtime = Arc::clone(&self.runtime);
        let effect_runtime = Arc::clone(&runtime);
        let repair_runtime = Arc::clone(&runtime);
        let effect_intent = delivery_intent.clone();
        let prompt = prompt.clone();
        let render_metadata = render_metadata.clone();
        Ok(Some(async_completion_dispatch_with_admission_outcome(
            occurrence,
            Some(schedule_identity.correlation_id.clone()),
            admission_outcome,
            durable_external_delivery_completion(
                runtime,
                delivery_intent,
                repair_state,
                Box::pin(async move {
                    wait_external_delivery_repair_deadline(repair_state).await?;
                    match effect_runtime
                        .member_send(&mob_id, member, prompt, render_metadata, delivery_identity)
                        .await
                    {
                        Ok(()) => Ok(DeliveryTerminal::completed(None)),
                        Err(error) => {
                            defer_external_delivery_after_target_error(
                                repair_runtime,
                                &effect_intent,
                                repair_state,
                                error,
                            )
                            .await
                        }
                    }
                }),
            ),
        )))
    }
}

fn mob_delivery_intent(
    mob_id: MobId,
    schedule_identity: &ScheduleDeliveryIdentity,
    target_kind: MobExternalDeliveryTargetKind,
    canonical_action: String,
) -> Result<(MobExternalDeliveryIdentity, MobExternalDeliveryIntent), ScheduleDomainError> {
    let identity = MobExternalDeliveryIdentity::new(
        schedule_identity.idempotency_key.clone(),
        schedule_identity.correlation_id.clone(),
    )
    .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
    let intent = MobExternalDeliveryIntent::new(
        mob_id,
        identity.clone(),
        target_kind,
        canonical_action.as_bytes(),
    )
    .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
    Ok((identity, intent))
}

fn dispatch_for_external_terminal(
    occurrence: &Occurrence,
    schedule_identity: &ScheduleDeliveryIdentity,
    terminal: MobExternalDeliveryTerminal,
    admission_outcome: ScheduleAdmissionOutcome,
) -> DeliveryDispatch {
    match terminal {
        MobExternalDeliveryTerminal::Completed => {
            immediate_completed_dispatch_with_admission_outcome(
                occurrence,
                Some(schedule_identity.correlation_id.clone()),
                admission_outcome,
            )
        }
        MobExternalDeliveryTerminal::Failed {
            failure_class,
            detail,
        } => external_delivery_failure_dispatch(
            occurrence,
            detail,
            delivery_failure_reason_for_class(failure_class),
            schedule_identity,
            admission_outcome,
        ),
    }
}

fn external_delivery_failure_dispatch(
    occurrence: &Occurrence,
    detail: String,
    failure_reason: DeliveryFailureReason,
    schedule_identity: &ScheduleDeliveryIdentity,
    admission_outcome: ScheduleAdmissionOutcome,
) -> DeliveryDispatch {
    let detail = meerkat_core::panic_payload::panic_safe_detail(&detail);
    async_completion_dispatch_with_admission_outcome(
        occurrence,
        Some(schedule_identity.correlation_id.clone()),
        admission_outcome,
        Box::pin(async move { Ok(DeliveryTerminal::delivery_failed(detail, failure_reason)) }),
    )
}

const EXTERNAL_DELIVERY_LIVE_TERMINAL_REPAIR_ATTEMPTS: u32 = 8;

fn external_delivery_terminal_commit_dispatch(
    occurrence: &Occurrence,
    schedule_identity: &ScheduleDeliveryIdentity,
    admission_outcome: ScheduleAdmissionOutcome,
    runtime: Arc<dyn ScheduleMobRuntime>,
    intent: MobExternalDeliveryIntent,
    repair: Option<MobExternalDeliveryRepairState>,
    terminal: MobExternalDeliveryTerminal,
) -> DeliveryDispatch {
    async_completion_dispatch_with_admission_outcome(
        occurrence,
        Some(schedule_identity.correlation_id.clone()),
        admission_outcome,
        Box::pin(async move {
            let durable =
                commit_external_delivery_terminal_with_repair(runtime, &intent, terminal, repair)
                    .await?;
            Ok(delivery_terminal_from_external_terminal(durable))
        }),
    )
}

fn repair_refused_external_delivery_dispatch(
    occurrence: &Occurrence,
    schedule_identity: &ScheduleDeliveryIdentity,
    runtime: Arc<dyn ScheduleMobRuntime>,
    intent: MobExternalDeliveryIntent,
    target_kind: MobExternalDeliveryTargetKind,
    repair: MobExternalDeliveryRepairState,
) -> DeliveryDispatch {
    let terminal = MobExternalDeliveryTerminal::failed_with_class(
        MobFailureClass::RuntimeRejected,
        format!(
            "repair_refused: durable external-delivery Begin for {target_kind:?} has no terminal, and this target contract does not authorize same-ID re-execution"
        ),
    );
    external_delivery_terminal_commit_dispatch(
        occurrence,
        schedule_identity,
        ScheduleAdmissionOutcome::Deduplicated,
        runtime,
        intent,
        Some(repair),
        terminal,
    )
}

async fn defer_external_delivery_after_target_error(
    runtime: Arc<dyn ScheduleMobRuntime>,
    intent: &MobExternalDeliveryIntent,
    prior_repair: Option<MobExternalDeliveryRepairState>,
    error: MobError,
) -> Result<DeliveryTerminal, ScheduleDomainError> {
    match runtime.schedule_external_delivery_repair(intent).await {
        Ok(MobExternalDeliveryRepairOutcome::ExistingTerminal(terminal)) => {
            Ok(delivery_terminal_from_external_terminal(terminal))
        }
        Ok(MobExternalDeliveryRepairOutcome::Scheduled(repair)) => {
            Err(ScheduleDomainError::DeliveryRepairDeferred {
                detail: format!(
                    "target admission remained ambiguous ({error}); durable repair advanced from attempt {} to {} with retry deadline {}",
                    prior_repair.map_or(0, |state| state.attempt),
                    repair.attempt,
                    repair.retry_not_before_ms,
                ),
            })
        }
        Err(repair_error) => Err(ScheduleDomainError::DeliveryRepairDeferred {
            detail: format!(
                "target admission remained ambiguous ({error}); durable repair scheduling failed ({repair_error})"
            ),
        }),
    }
}

async fn wait_external_delivery_repair_deadline(
    repair: Option<MobExternalDeliveryRepairState>,
) -> Result<(), ScheduleDomainError> {
    let Some(repair) = repair else {
        return Ok(());
    };
    let delay_ms = repair.retry_delay_ms(external_delivery_now_ms()?);
    if delay_ms > 0 {
        sleep(Duration::from_millis(delay_ms)).await;
    }
    Ok(())
}

fn durable_external_delivery_completion(
    runtime: Arc<dyn ScheduleMobRuntime>,
    intent: MobExternalDeliveryIntent,
    repair: Option<MobExternalDeliveryRepairState>,
    completion: DeliveryCompletion,
) -> DeliveryCompletion {
    Box::pin(async move {
        let terminal = completion.await?;
        let durable_terminal = external_terminal_from_delivery_terminal(&terminal);
        let durable = commit_external_delivery_terminal_with_repair(
            runtime,
            &intent,
            durable_terminal,
            repair,
        )
        .await?;
        Ok(delivery_terminal_from_external_terminal(durable))
    })
}

async fn commit_external_delivery_terminal_with_repair(
    runtime: Arc<dyn ScheduleMobRuntime>,
    intent: &MobExternalDeliveryIntent,
    terminal: MobExternalDeliveryTerminal,
    mut repair: Option<MobExternalDeliveryRepairState>,
) -> Result<MobExternalDeliveryTerminal, ScheduleDomainError> {
    for live_attempt in 1..=EXTERNAL_DELIVERY_LIVE_TERMINAL_REPAIR_ATTEMPTS {
        if let Some(state) = repair {
            let now_ms = external_delivery_now_ms()?;
            let delay_ms = state.retry_delay_ms(now_ms);
            if delay_ms > 0 {
                sleep(Duration::from_millis(delay_ms)).await;
            }
        }
        match runtime.complete_external_delivery(intent, &terminal).await {
            Ok(_) => return Ok(terminal),
            Err(commit_error) => {
                let scheduled = match runtime.schedule_external_delivery_repair(intent).await {
                    Ok(outcome) => outcome,
                    Err(repair_error) => {
                        return Err(ScheduleDomainError::DeliveryRepairDeferred {
                            detail: format!(
                                "terminal commit failed ({commit_error}); durable repair scheduling also failed ({repair_error})"
                            ),
                        });
                    }
                };
                match scheduled {
                    MobExternalDeliveryRepairOutcome::ExistingTerminal(existing) => {
                        return Ok(existing);
                    }
                    MobExternalDeliveryRepairOutcome::Scheduled(next) => {
                        tracing::warn!(
                            mob_id = %intent.mob_id,
                            idempotency_key = %intent.identity.idempotency_key,
                            live_attempt,
                            durable_repair_attempt = next.attempt,
                            retry_not_before_ms = next.retry_not_before_ms,
                            error = %commit_error,
                            "mob external-delivery terminal commit failed; durable repair deadline advanced"
                        );
                        repair = Some(next);
                    }
                }
                if live_attempt == EXTERNAL_DELIVERY_LIVE_TERMINAL_REPAIR_ATTEMPTS {
                    return Err(ScheduleDomainError::DeliveryRepairDeferred {
                        detail: format!(
                            "mob external-delivery terminal remained uncommitted after {live_attempt} bounded live attempts; durable repair attempt {} is reclaimable at/after {}",
                            repair.map_or(0, |state| state.attempt),
                            repair.map_or(0, |state| state.retry_not_before_ms),
                        ),
                    });
                }
            }
        }
    }
    unreachable!("non-empty bounded terminal repair loop")
}

fn external_delivery_now_ms() -> Result<u64, ScheduleDomainError> {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .map_err(|error| {
            ScheduleDomainError::Internal(format!("external-delivery repair clock failed: {error}"))
        })
}

fn delivery_terminal_from_external_terminal(
    terminal: MobExternalDeliveryTerminal,
) -> DeliveryTerminal {
    match terminal {
        MobExternalDeliveryTerminal::Completed => DeliveryTerminal::completed(None),
        MobExternalDeliveryTerminal::Failed {
            failure_class,
            detail,
        } => DeliveryTerminal::delivery_failed(
            detail,
            delivery_failure_reason_for_class(failure_class),
        ),
    }
}

fn external_terminal_from_delivery_terminal(
    terminal: &DeliveryTerminal,
) -> MobExternalDeliveryTerminal {
    match terminal.delivery_failure_reason {
        None => MobExternalDeliveryTerminal::Completed,
        Some(reason) => MobExternalDeliveryTerminal::failed_with_class(
            mob_failure_class_for_delivery_reason(reason),
            terminal
                .detail
                .as_deref()
                .unwrap_or("mob delivery failed without detail"),
        ),
    }
}

fn mob_binding_mob_id(binding: &MobTargetBinding) -> &str {
    match binding {
        MobTargetBinding::Member { mob_id, .. }
        | MobTargetBinding::Flow { mob_id, .. }
        | MobTargetBinding::SpawnHelper { mob_id, .. }
        | MobTargetBinding::ForkHelper { mob_id, .. } => mob_id,
    }
}

fn validate_mob_delivery_binding(binding: &MobTargetBinding) -> Result<(), ScheduleDomainError> {
    match binding {
        MobTargetBinding::Member { .. } => Ok(()),
        MobTargetBinding::Flow { params, .. } => {
            serde_json::from_str::<serde_json::Value>(params.get())
                .map(|_| ())
                .map_err(|error| {
                    ScheduleDomainError::InvalidSchedule(format!(
                        "invalid mob flow params: {error}"
                    ))
                })
        }
        MobTargetBinding::SpawnHelper { options, .. } => {
            helper_options_from_spec(options).map(|_| ())
        }
        MobTargetBinding::ForkHelper {
            fork_context,
            options,
            ..
        } => {
            let _ = fork_context_from_spec(fork_context);
            helper_options_from_spec(options).map(|_| ())
        }
    }
}

fn helper_options_from_spec(
    spec: &HelperOptionsSpec,
) -> Result<HelperOptions, ScheduleDomainError> {
    let mut options = HelperOptions::default();
    options.role_name = spec.role_name.clone().map(Into::into);
    if spec.resolved_spawn_snapshot.is_some() {
        return Err(ScheduleDomainError::InvalidSchedule(
            "scheduled mob helper resolved spawn snapshots are compatibility mirrors and cannot authorize inherited tool visibility without a live generated parent authority"
                .to_string(),
        ));
    }
    if let Some(tooling) = &spec.tooling {
        match tooling {
            ScheduleSpawnTooling::InheritParent { .. } | ScheduleSpawnTooling::Minimal
                if spec.resolved_spawn_snapshot.is_none() =>
            {
                return Err(ScheduleDomainError::InvalidSchedule(
                    "scheduled mob helper tooling requires a pre-resolved spawn snapshot"
                        .to_string(),
                ));
            }
            ScheduleSpawnTooling::Profile {
                name,
                allow_overlay,
                deny_overlay,
            } => {
                if options.role_name.is_none() {
                    options.role_name = Some(name.clone().into());
                }
                if spec.resolved_spawn_snapshot.is_none()
                    && (allow_overlay.is_some() || deny_overlay.is_some())
                {
                    return Err(ScheduleDomainError::InvalidSchedule(
                        "scheduled mob helper profile tooling overlays require a pre-resolved spawn snapshot"
                            .to_string(),
                    ));
                }
            }
            _ => {}
        }
    }
    options.runtime_mode = spec.runtime_mode.map(|mode| match mode {
        ScheduledMobRuntimeMode::AutonomousHost => meerkat_mob::MobRuntimeMode::AutonomousHost,
        ScheduledMobRuntimeMode::TurnDriven => meerkat_mob::MobRuntimeMode::TurnDriven,
    });
    options.backend = spec.backend.map(|backend| match backend {
        ScheduledMobBackendKind::Session => MobBackendKind::Session,
        ScheduledMobBackendKind::External => MobBackendKind::External,
    });
    options.tool_access_policy = spec.tool_access_policy.clone();
    Ok(options)
}

fn fork_context_from_spec(spec: &ForkContextSpec) -> ForkContext {
    match spec {
        ForkContextSpec::FullHistory => ForkContext::FullHistory,
        ForkContextSpec::LastMessages { count } => ForkContext::LastMessages { count: *count },
    }
}

fn mob_delivery_failed_terminal(error: MobError) -> DeliveryTerminal {
    let failure_reason = delivery_failure_reason_for(&error);
    DeliveryTerminal::delivery_failed(
        meerkat_core::panic_payload::panic_safe_detail(&error.to_string()),
        failure_reason,
    )
}

fn mob_probe_error_outcome(error: MobError) -> Result<TargetProbeOutcome, ScheduleDomainError> {
    if error.is_missing_target() {
        Ok(TargetProbeOutcome::Missing {
            detail: Some(error.to_string()),
        })
    } else {
        Err(ScheduleDomainError::ProbeFailed(error.to_string()))
    }
}

/// Map the mob-owned [`MobFailureClass`] onto the schedule-domain
/// [`DeliveryFailureReason`].
///
/// The classification of which `MobError` variant means what is owned by
/// `meerkat-mob` (`MobError::failure_class`); this consumer only translates
/// that mob-owned class into the schedule delivery vocabulary, since
/// `DeliveryFailureReason` is owned by a crate `meerkat-mob` does not depend on.
fn delivery_failure_reason_for(error: &MobError) -> DeliveryFailureReason {
    delivery_failure_reason_for_class(error.failure_class())
}

fn delivery_failure_reason_for_class(failure_class: MobFailureClass) -> DeliveryFailureReason {
    match failure_class {
        MobFailureClass::TargetMissing => DeliveryFailureReason::TargetMissing,
        // Archived-but-intact: the target exists but refuses work in its
        // current lifecycle — the schedule vocabulary's closest honest class
        // is busy, NOT missing (the transcript is on disk).
        MobFailureClass::TargetArchived => DeliveryFailureReason::TargetBusy,
        MobFailureClass::TargetBusy => DeliveryFailureReason::TargetBusy,
        MobFailureClass::Transport => DeliveryFailureReason::TransportError,
        MobFailureClass::RuntimeRejected => DeliveryFailureReason::RuntimeRejected,
        MobFailureClass::Internal => DeliveryFailureReason::InternalError,
        MobFailureClass::MobRejected => DeliveryFailureReason::MobRejected,
    }
}

fn mob_failure_class_for_delivery_reason(failure_reason: DeliveryFailureReason) -> MobFailureClass {
    match failure_reason {
        DeliveryFailureReason::TargetMaterializationFailed
        | DeliveryFailureReason::InternalError => MobFailureClass::Internal,
        DeliveryFailureReason::TargetMissing => MobFailureClass::TargetMissing,
        DeliveryFailureReason::TargetBusy => MobFailureClass::TargetBusy,
        DeliveryFailureReason::RuntimeRejected => MobFailureClass::RuntimeRejected,
        DeliveryFailureReason::MobRejected => MobFailureClass::MobRejected,
        DeliveryFailureReason::TransportError => MobFailureClass::Transport,
    }
}

fn mob_flow_completion_future(
    runtime: Arc<dyn ScheduleMobRuntime>,
    mob_id: MobId,
    run_id: RunId,
) -> DeliveryCompletion {
    Box::pin(async move {
        loop {
            match runtime.flow_status(&mob_id, run_id.clone()).await {
                Ok(Some(status)) => {
                    let terminal = match mob_machine_run_status_is_terminal(&run_id, &status) {
                        Ok(terminal) => terminal,
                        Err(error) => return Ok(mob_delivery_failed_terminal(error)),
                    };
                    if terminal {
                        let result_class =
                            match mob_machine_run_public_result_class(&run_id, &status) {
                                Ok(result_class) => result_class,
                                Err(error) => return Ok(mob_delivery_failed_terminal(error)),
                            };
                        return Ok(match result_class {
                            MobFlowRunPublicResultClass::Success => {
                                DeliveryTerminal::completed(None)
                            }
                            MobFlowRunPublicResultClass::Error => {
                                DeliveryTerminal::delivery_failed(
                                    format!("mob flow terminated as {status:?}"),
                                    DeliveryFailureReason::MobRejected,
                                )
                            }
                        });
                    }
                    sleep(Duration::from_millis(100)).await;
                }
                Ok(None) => {
                    return Ok(DeliveryTerminal::delivery_failed(
                        format!("mob flow run disappeared: {run_id}"),
                        DeliveryFailureReason::TargetMissing,
                    ));
                }
                Err(error) => return Ok(mob_delivery_failed_terminal(error)),
            }
        }
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    use meerkat::{
        CreateScheduleRequest, IntervalTriggerSpec, MisfirePolicy, MissingTargetPolicy,
        OccurrenceOrdinal, OverlapPolicy, ResolvedSpawnSnapshot, Schedule, TargetBinding,
        TriggerSpec,
    };
    use meerkat_core::tool_scope::ToolFilter;
    use meerkat_mob::{InMemoryMobEventStore, MobEventStore, MobStoreError};

    #[derive(Debug)]
    struct RecordingMobRuntime {
        members: Mutex<BTreeSet<String>>,
        flows: Mutex<BTreeSet<String>>,
        external_deliveries: InMemoryMobEventStore,
        member_delivery_keys: Mutex<BTreeSet<String>>,
        sent_members: Mutex<Vec<(String, String, String, String)>>,
        flow_runs: Mutex<BTreeMap<String, RunId>>,
        run_flows: Mutex<Vec<(String, String, serde_json::Value)>>,
        spawn_helpers: Mutex<Vec<(String, String, HelperOptions)>>,
        fork_helpers: Mutex<Vec<(String, String, String)>>,
        member_exists_error: Mutex<Option<String>>,
        flow_run_missing: Mutex<bool>,
        flow_status: Mutex<MobRunStatus>,
        next_run_id: Mutex<Option<RunId>>,
        terminal_commit_always_fails: Mutex<bool>,
        terminal_commit_calls: Mutex<u32>,
        repair_schedule_calls: Mutex<u32>,
        zero_repair_delay: Mutex<bool>,
    }

    impl Default for RecordingMobRuntime {
        fn default() -> Self {
            Self {
                members: Mutex::new(BTreeSet::new()),
                flows: Mutex::new(BTreeSet::new()),
                external_deliveries: InMemoryMobEventStore::default(),
                member_delivery_keys: Mutex::new(BTreeSet::new()),
                sent_members: Mutex::new(Vec::new()),
                flow_runs: Mutex::new(BTreeMap::new()),
                run_flows: Mutex::new(Vec::new()),
                spawn_helpers: Mutex::new(Vec::new()),
                fork_helpers: Mutex::new(Vec::new()),
                member_exists_error: Mutex::new(None),
                flow_run_missing: Mutex::new(false),
                flow_status: Mutex::new(MobRunStatus::Completed),
                next_run_id: Mutex::new(None),
                terminal_commit_always_fails: Mutex::new(false),
                terminal_commit_calls: Mutex::new(0),
                repair_schedule_calls: Mutex::new(0),
                zero_repair_delay: Mutex::new(false),
            }
        }
    }

    impl RecordingMobRuntime {
        fn with_member(&self, member_id: &str) {
            self.members
                .lock()
                .expect("members lock")
                .insert(member_id.to_string());
        }

        fn with_flow(&self, flow_id: &str) {
            self.flows
                .lock()
                .expect("flows lock")
                .insert(flow_id.to_string());
        }
    }

    #[async_trait]
    impl ScheduleMobRuntime for RecordingMobRuntime {
        async fn begin_external_delivery(
            &self,
            intent: &MobExternalDeliveryIntent,
        ) -> Result<MobExternalDeliveryBeginOutcome, MobError> {
            self.external_deliveries
                .begin_external_delivery(intent)
                .await
                .map_err(Into::into)
        }

        async fn complete_external_delivery(
            &self,
            intent: &MobExternalDeliveryIntent,
            terminal: &MobExternalDeliveryTerminal,
        ) -> Result<MobExternalDeliveryCompleteOutcome, MobError> {
            *self
                .terminal_commit_calls
                .lock()
                .expect("terminal commit calls lock") += 1;
            if *self
                .terminal_commit_always_fails
                .lock()
                .expect("terminal commit failure lock")
            {
                return Err(MobStoreError::WriteFailed(
                    "injected terminal commit outage".to_string(),
                )
                .into());
            }
            self.external_deliveries
                .complete_external_delivery(intent, terminal)
                .await
                .map_err(Into::into)
        }

        async fn schedule_external_delivery_repair(
            &self,
            intent: &MobExternalDeliveryIntent,
        ) -> Result<MobExternalDeliveryRepairOutcome, MobError> {
            *self
                .repair_schedule_calls
                .lock()
                .expect("repair schedule calls lock") += 1;
            let outcome = self
                .external_deliveries
                .schedule_external_delivery_repair(intent)
                .await
                .map_err(MobError::from)?;
            if *self
                .zero_repair_delay
                .lock()
                .expect("zero repair delay lock")
                && let MobExternalDeliveryRepairOutcome::Scheduled(mut repair) = outcome.clone()
            {
                repair.retry_not_before_ms = 0;
                return Ok(MobExternalDeliveryRepairOutcome::Scheduled(repair));
            }
            Ok(outcome)
        }

        fn supports_external_delivery_redrive(
            &self,
            target_kind: MobExternalDeliveryTargetKind,
        ) -> bool {
            matches!(
                target_kind,
                MobExternalDeliveryTargetKind::MemberSend | MobExternalDeliveryTargetKind::Flow
            )
        }

        async fn member_exists(
            &self,
            _mob_id: &MobId,
            identity: &AgentIdentity,
        ) -> Result<bool, MobError> {
            if let Some(error) = self
                .member_exists_error
                .lock()
                .expect("member exists error lock")
                .clone()
            {
                return Err(MobError::Internal(error));
            }
            Ok(self
                .members
                .lock()
                .expect("members lock")
                .contains(identity.as_str()))
        }

        async fn flow_exists(&self, _mob_id: &MobId, flow_id: &FlowId) -> Result<bool, MobError> {
            Ok(self
                .flows
                .lock()
                .expect("flows lock")
                .contains(flow_id.as_str()))
        }

        async fn member_send(
            &self,
            mob_id: &MobId,
            identity: AgentIdentity,
            content: ContentInput,
            _render_metadata: Option<RenderMetadata>,
            delivery_identity: MobExternalDeliveryIdentity,
        ) -> Result<(), MobError> {
            let content = match content {
                ContentInput::Text(text) => text,
                other => format!("{other:?}"),
            };
            if !self
                .member_delivery_keys
                .lock()
                .expect("member delivery keys lock")
                .insert(delivery_identity.idempotency_key.clone())
            {
                return Ok(());
            }
            self.sent_members.lock().expect("sent lock").push((
                mob_id.to_string(),
                identity.to_string(),
                content,
                delivery_identity.idempotency_key,
            ));
            Ok(())
        }

        async fn run_flow(
            &self,
            mob_id: &MobId,
            flow_id: FlowId,
            params: serde_json::Value,
            delivery_identity: &MobExternalDeliveryIdentity,
        ) -> Result<RunId, MobError> {
            if let Some(run_id) = self
                .flow_runs
                .lock()
                .expect("flow runs lock")
                .get(&delivery_identity.idempotency_key)
                .cloned()
            {
                return Ok(run_id);
            }
            self.run_flows.lock().expect("run lock").push((
                mob_id.to_string(),
                flow_id.to_string(),
                params,
            ));
            let run_id = self
                .next_run_id
                .lock()
                .expect("run id lock")
                .clone()
                .unwrap_or_default();
            self.flow_runs
                .lock()
                .expect("flow runs lock")
                .insert(delivery_identity.idempotency_key.clone(), run_id.clone());
            Ok(run_id)
        }

        async fn flow_status(
            &self,
            _mob_id: &MobId,
            _run_id: RunId,
        ) -> Result<Option<MobRunStatus>, MobError> {
            if *self.flow_run_missing.lock().expect("flow run missing lock") {
                return Ok(None);
            }
            Ok(Some(
                self.flow_status.lock().expect("flow status lock").clone(),
            ))
        }

        async fn spawn_helper(
            &self,
            mob_id: &MobId,
            identity: AgentIdentity,
            _prompt: String,
            options: HelperOptions,
        ) -> Result<(), MobError> {
            self.spawn_helpers.lock().expect("spawn lock").push((
                mob_id.to_string(),
                identity.to_string(),
                options,
            ));
            Ok(())
        }

        async fn fork_helper(
            &self,
            mob_id: &MobId,
            source_identity: &AgentIdentity,
            identity: AgentIdentity,
            _prompt: String,
            _fork_context: ForkContext,
            _options: HelperOptions,
        ) -> Result<(), MobError> {
            self.fork_helpers.lock().expect("fork lock").push((
                mob_id.to_string(),
                source_identity.to_string(),
                identity.to_string(),
            ));
            Ok(())
        }
    }

    fn flow_params(value: &str) -> meerkat_schedule::FlowParams {
        meerkat_schedule::FlowParams::parse(value).expect("valid flow params json")
    }

    fn sample_occurrence_for_target(target: TargetBinding) -> Occurrence {
        let schedule = Schedule::new(CreateScheduleRequest {
            name: Some("mob-schedule-host-test".to_string()),
            description: None,
            trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc: chrono::Utc::now(),
                every_seconds: 60,
                end_at_utc: None,
            }),
            target,
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::SkipIfRunning,
            missing_target_policy: MissingTargetPolicy::Skip,
            labels: Default::default(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("sample schedule creation should pass generated authority");
        let mut occurrence =
            Occurrence::planned_from_schedule(&schedule, OccurrenceOrdinal(0), chrono::Utc::now())
                .expect("sample occurrence planning should pass generated authority");
        occurrence.attempt_count = 1;
        occurrence
    }

    fn sample_occurrence(binding: MobTargetBinding) -> Occurrence {
        sample_occurrence_for_target(TargetBinding::Mob(Box::new(binding)))
    }

    fn sample_delivery_identity(occurrence: &Occurrence) -> ScheduleDeliveryIdentity {
        ScheduleDeliveryIdentity::for_occurrence(occurrence)
    }

    async fn begin_test_delivery(
        runtime: &RecordingMobRuntime,
        occurrence: &Occurrence,
        binding: &MobTargetBinding,
        target_kind: MobExternalDeliveryTargetKind,
    ) -> (MobExternalDeliveryIdentity, MobExternalDeliveryIntent) {
        let schedule_identity = sample_delivery_identity(occurrence);
        let (delivery_identity, intent) = mob_delivery_intent(
            MobId::from(mob_binding_mob_id(binding)),
            &schedule_identity,
            target_kind,
            binding.stable_key().expect("stable binding key"),
        )
        .expect("valid test delivery intent");
        assert_eq!(
            runtime
                .begin_external_delivery(&intent)
                .await
                .expect("begin test delivery"),
            MobExternalDeliveryBeginOutcome::Begun
        );
        (delivery_identity, intent)
    }

    #[tokio::test]
    async fn member_send_target_resolves_member_id_as_agent_identity() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        runtime.with_member("deploy-monitor");
        let host = MobMcpScheduleHost::from_runtime(runtime.clone());
        let binding = MobTargetBinding::Member {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            action: ScheduledMobAction::Send {
                content: ContentInput::Text("Check deploy state.".to_string()),
                render_metadata: None,
            },
        };
        let occurrence = sample_occurrence(binding.clone());
        let delivery_identity = sample_delivery_identity(&occurrence);

        let dispatch = host
            .deliver_mob_target(&occurrence, &delivery_identity, &binding)
            .await
            .expect("delivery dispatch");

        assert_eq!(
            dispatch.correlation_id.as_deref(),
            Some(delivery_identity.correlation_id.as_str())
        );
        assert_eq!(
            dispatch.completion.await.expect("member completion").phase,
            meerkat::OccurrencePhase::Completed
        );
        assert_eq!(
            runtime.sent_members.lock().expect("sent lock").as_slice(),
            &[(
                "ops".to_string(),
                "deploy-monitor".to_string(),
                "Check deploy state.".to_string(),
                delivery_identity.idempotency_key,
            )]
        );
    }

    #[tokio::test]
    async fn member_send_redrive_reuses_stable_runtime_input_after_effect_before_terminal() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        let host = MobMcpScheduleHost::from_runtime(runtime.clone());
        let binding = MobTargetBinding::Member {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            action: ScheduledMobAction::Send {
                content: ContentInput::Text("Check deploy state.".to_string()),
                render_metadata: None,
            },
        };
        let occurrence = sample_occurrence(binding.clone());
        let schedule_identity = sample_delivery_identity(&occurrence);
        let (delivery_identity, _intent) = begin_test_delivery(
            runtime.as_ref(),
            &occurrence,
            &binding,
            MobExternalDeliveryTargetKind::MemberSend,
        )
        .await;

        runtime
            .member_send(
                &MobId::from("ops"),
                AgentIdentity::from("deploy-monitor"),
                ContentInput::Text("Check deploy state.".to_string()),
                None,
                delivery_identity,
            )
            .await
            .expect("simulate admitted member effect before crash");

        let dispatch = host
            .deliver_mob_target(&occurrence, &schedule_identity, &binding)
            .await
            .expect("redrive dispatch");
        assert_eq!(
            dispatch.receipt.runtime_outcome,
            Some(meerkat_schedule::RuntimeDeliveryOutcome::AdmissionDeduplicated)
        );
        assert_eq!(
            dispatch.completion.await.expect("redrive completion").phase,
            meerkat::OccurrencePhase::Completed
        );
        assert_eq!(
            runtime.sent_members.lock().expect("sent lock").len(),
            1,
            "stable runtime input identity must suppress a duplicate member effect"
        );
    }

    #[tokio::test]
    async fn bounded_live_terminal_repair_defers_to_durable_lease_reclaim() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        runtime.with_member("deploy-monitor");
        *runtime
            .terminal_commit_always_fails
            .lock()
            .expect("terminal commit failure lock") = true;
        *runtime
            .zero_repair_delay
            .lock()
            .expect("zero repair delay lock") = true;
        let host = MobMcpScheduleHost::from_runtime(runtime.clone());
        let binding = MobTargetBinding::Member {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            action: ScheduledMobAction::Send {
                content: ContentInput::Text("Check deploy state.".to_string()),
                render_metadata: None,
            },
        };
        let occurrence = sample_occurrence(binding.clone());
        let schedule_identity = sample_delivery_identity(&occurrence);
        let (_, intent) = mob_delivery_intent(
            MobId::from("ops"),
            &schedule_identity,
            MobExternalDeliveryTargetKind::MemberSend,
            binding.stable_key().expect("stable binding key"),
        )
        .expect("valid test delivery intent");

        let dispatch = host
            .deliver_mob_target(&occurrence, &schedule_identity, &binding)
            .await
            .expect("delivery dispatch");
        assert!(matches!(
            dispatch.completion.await,
            Err(ScheduleDomainError::DeliveryRepairDeferred { .. })
        ));
        assert_eq!(
            *runtime
                .terminal_commit_calls
                .lock()
                .expect("terminal commit calls lock"),
            EXTERNAL_DELIVERY_LIVE_TERMINAL_REPAIR_ATTEMPTS
        );
        assert_eq!(
            *runtime
                .repair_schedule_calls
                .lock()
                .expect("repair schedule calls lock"),
            EXTERNAL_DELIVERY_LIVE_TERMINAL_REPAIR_ATTEMPTS
        );
        assert!(matches!(
            runtime
                .begin_external_delivery(&intent)
                .await
                .expect("durable begun repair state"),
            MobExternalDeliveryBeginOutcome::ExistingBegun {
                repair: MobExternalDeliveryRepairState {
                    attempt: EXTERNAL_DELIVERY_LIVE_TERMINAL_REPAIR_ATTEMPTS,
                    ..
                }
            }
        ));
        assert_eq!(
            runtime.sent_members.lock().expect("sent lock").len(),
            1,
            "bounded terminal persistence retries must never repeat the target effect"
        );
    }

    #[tokio::test]
    async fn mob_member_identity_target_delivers_to_member_authority() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        runtime.with_member("deploy-monitor");
        let host = MobMcpScheduleHost::from_runtime(runtime.clone());
        let identity =
            meerkat::surface::mob_member_schedule_identity(&meerkat_core::MobMemberBinding {
                mob_id: "ops".to_string(),
                role: "old-profile".to_string(),
                member: "deploy-monitor".to_string(),
            });
        assert!(
            !identity.contains("old-profile"),
            "schedule identity must not include profile/role material"
        );
        let binding = IdentityTargetBinding::resumable(
            identity,
            ScheduledSessionAction::Prompt {
                prompt: ContentInput::Text("Check deploy state.".to_string()),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        );
        let occurrence =
            sample_occurrence_for_target(TargetBinding::Identity(Box::new(binding.clone())));
        let delivery_identity = sample_delivery_identity(&occurrence);

        let probe = host
            .probe_identity_target(&binding)
            .await
            .expect("identity probe")
            .expect("mob identity should be handled by mob host");
        assert!(matches!(probe, TargetProbeOutcome::Ready));

        let dispatch = host
            .deliver_identity_target(&occurrence, &delivery_identity, &binding)
            .await
            .expect("identity delivery")
            .expect("mob identity should be delivered by mob host");

        assert_eq!(
            dispatch.correlation_id.as_deref(),
            Some(delivery_identity.correlation_id.as_str())
        );
        assert_eq!(
            dispatch
                .completion
                .await
                .expect("identity member completion")
                .phase,
            meerkat::OccurrencePhase::Completed
        );
        assert_eq!(
            runtime.sent_members.lock().expect("sent lock").as_slice(),
            &[(
                "ops".to_string(),
                "deploy-monitor".to_string(),
                "Check deploy state.".to_string(),
                delivery_identity.idempotency_key,
            )]
        );
    }

    #[tokio::test]
    async fn flow_target_starts_mob_flow() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        runtime.with_flow("release-check");
        let run_id = RunId::new();
        *runtime.next_run_id.lock().expect("run id lock") = Some(run_id.clone());
        let host = MobMcpScheduleHost::from_runtime(runtime.clone());
        let binding = MobTargetBinding::Flow {
            mob_id: "ops".to_string(),
            flow_id: "release-check".to_string(),
            params: flow_params(r#"{"sha":"abc123"}"#),
        };
        let occurrence = sample_occurrence(binding.clone());
        let delivery_identity = sample_delivery_identity(&occurrence);

        let dispatch = host
            .deliver_mob_target(&occurrence, &delivery_identity, &binding)
            .await
            .expect("delivery dispatch");

        assert_eq!(
            dispatch.correlation_id.as_deref(),
            Some(delivery_identity.correlation_id.as_str())
        );
        assert_eq!(
            dispatch.completion.await.expect("flow completion").phase,
            meerkat::OccurrencePhase::Completed
        );
        assert_eq!(
            runtime.run_flows.lock().expect("run lock").as_slice(),
            &[(
                "ops".to_string(),
                "release-check".to_string(),
                serde_json::json!({"sha": "abc123"})
            )]
        );
    }

    #[tokio::test]
    async fn explicitly_deduplicated_flow_redrive_reuses_stable_run_after_dispatch_crash() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        let run_id = RunId::new();
        *runtime.next_run_id.lock().expect("run id lock") = Some(run_id);
        let host = MobMcpScheduleHost::from_runtime(runtime.clone());
        let binding = MobTargetBinding::Flow {
            mob_id: "ops".to_string(),
            flow_id: "release-check".to_string(),
            params: flow_params(r#"{"sha":"abc123"}"#),
        };
        let occurrence = sample_occurrence(binding.clone());
        let delivery_identity = sample_delivery_identity(&occurrence);

        let first_dispatch = host
            .deliver_mob_target(&occurrence, &delivery_identity, &binding)
            .await
            .expect("initial flow dispatch");
        drop(first_dispatch);

        let redrive = host
            .deliver_mob_target(&occurrence, &delivery_identity, &binding)
            .await
            .expect("redriven flow dispatch");
        assert_eq!(
            redrive.receipt.runtime_outcome,
            Some(meerkat_schedule::RuntimeDeliveryOutcome::AdmissionDeduplicated)
        );
        assert_eq!(
            redrive.completion.await.expect("redrive completion").phase,
            meerkat::OccurrencePhase::Completed
        );
        assert_eq!(
            runtime.run_flows.lock().expect("run lock").len(),
            1,
            "flow redrive must reuse the durable RunId instead of creating a second run"
        );

        let terminal_replay = host
            .deliver_mob_target(&occurrence, &delivery_identity, &binding)
            .await
            .expect("terminal replay");
        assert_eq!(
            terminal_replay.receipt.runtime_outcome,
            Some(meerkat_schedule::RuntimeDeliveryOutcome::AdmissionDeduplicated)
        );
        assert_eq!(runtime.run_flows.lock().expect("run lock").len(), 1);
    }

    #[tokio::test]
    async fn spawn_helper_redrive_holds_ambiguous_begin_after_effect() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        let host = MobMcpScheduleHost::from_runtime(runtime.clone());
        let binding = MobTargetBinding::SpawnHelper {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            prompt: "Check deploy state.".to_string(),
            options: HelperOptionsSpec::default(),
        };
        let occurrence = sample_occurrence(binding.clone());
        let delivery_identity = sample_delivery_identity(&occurrence);
        let (_external_identity, intent) = begin_test_delivery(
            runtime.as_ref(),
            &occurrence,
            &binding,
            MobExternalDeliveryTargetKind::SpawnHelper,
        )
        .await;
        runtime
            .spawn_helper(
                &MobId::from("ops"),
                AgentIdentity::from("deploy-monitor"),
                "Check deploy state.".to_string(),
                HelperOptions::default(),
            )
            .await
            .expect("simulate helper effect before crash");

        let redrive = host
            .deliver_mob_target(&occurrence, &delivery_identity, &binding)
            .await
            .expect("redriven helper dispatch");
        assert_eq!(
            redrive.receipt.runtime_outcome,
            Some(meerkat_schedule::RuntimeDeliveryOutcome::AdmissionDeduplicated)
        );
        let terminal = redrive.completion.await.expect("held completion");
        assert_eq!(terminal.phase, meerkat::OccurrencePhase::DeliveryFailed);
        assert_eq!(
            terminal.delivery_failure_reason,
            Some(DeliveryFailureReason::RuntimeRejected)
        );
        assert_eq!(
            runtime.spawn_helpers.lock().expect("spawn lock").len(),
            1,
            "an ambiguous helper Begin must fail closed instead of re-executing"
        );
        assert!(matches!(
            runtime
                .begin_external_delivery(&intent)
                .await
                .expect("load durable repair refusal"),
            MobExternalDeliveryBeginOutcome::ExistingTerminal(
                MobExternalDeliveryTerminal::Failed { ref detail, .. }
            ) if detail.contains("repair_refused")
        ));
    }

    #[tokio::test]
    async fn fork_helper_redrive_holds_ambiguous_begin_after_effect() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        let host = MobMcpScheduleHost::from_runtime(runtime.clone());
        let binding = MobTargetBinding::ForkHelper {
            mob_id: "ops".to_string(),
            source_member_id: "source".to_string(),
            member_id: "deploy-monitor".to_string(),
            prompt: "Check deploy state.".to_string(),
            fork_context: ForkContextSpec::FullHistory,
            options: HelperOptionsSpec::default(),
        };
        let occurrence = sample_occurrence(binding.clone());
        let delivery_identity = sample_delivery_identity(&occurrence);
        let (_external_identity, intent) = begin_test_delivery(
            runtime.as_ref(),
            &occurrence,
            &binding,
            MobExternalDeliveryTargetKind::ForkHelper,
        )
        .await;
        runtime
            .fork_helper(
                &MobId::from("ops"),
                &AgentIdentity::from("source"),
                AgentIdentity::from("deploy-monitor"),
                "Check deploy state.".to_string(),
                ForkContext::FullHistory,
                HelperOptions::default(),
            )
            .await
            .expect("simulate fork effect before crash");

        let redrive = host
            .deliver_mob_target(&occurrence, &delivery_identity, &binding)
            .await
            .expect("redriven fork dispatch");
        assert_eq!(
            redrive.receipt.runtime_outcome,
            Some(meerkat_schedule::RuntimeDeliveryOutcome::AdmissionDeduplicated)
        );
        let terminal = redrive.completion.await.expect("held completion");
        assert_eq!(terminal.phase, meerkat::OccurrencePhase::DeliveryFailed);
        assert_eq!(
            terminal.delivery_failure_reason,
            Some(DeliveryFailureReason::RuntimeRejected)
        );
        assert_eq!(
            runtime.fork_helpers.lock().expect("fork lock").len(),
            1,
            "an ambiguous fork Begin must fail closed instead of re-executing"
        );
        assert!(matches!(
            runtime
                .begin_external_delivery(&intent)
                .await
                .expect("load durable fork repair refusal"),
            MobExternalDeliveryBeginOutcome::ExistingTerminal(
                MobExternalDeliveryTerminal::Failed { ref detail, .. }
            ) if detail.contains("repair_refused")
        ));
    }

    #[tokio::test]
    async fn missing_member_probe_reports_missing() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        let host = MobMcpScheduleHost::from_runtime(runtime);
        let binding = MobTargetBinding::Member {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            action: ScheduledMobAction::Send {
                content: ContentInput::Text("Check deploy state.".to_string()),
                render_metadata: None,
            },
        };

        let outcome = host
            .probe_mob_target(&binding)
            .await
            .expect("probe should succeed");

        let TargetProbeOutcome::Missing { detail } = outcome else {
            panic!("expected missing member probe, got {outcome:?}");
        };
        assert_eq!(
            detail.as_deref(),
            Some("mob member not found: deploy-monitor")
        );
    }

    #[tokio::test]
    async fn helper_target_reports_busy_when_identity_exists() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        runtime.with_member("deploy-monitor");
        let host = MobMcpScheduleHost::from_runtime(runtime);
        let binding = MobTargetBinding::SpawnHelper {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            prompt: "Check deploy state.".to_string(),
            options: HelperOptionsSpec::default(),
        };

        let outcome = host
            .probe_mob_target(&binding)
            .await
            .expect("probe should succeed");

        let TargetProbeOutcome::Busy { detail } = outcome else {
            panic!("expected busy helper probe, got {outcome:?}");
        };
        assert_eq!(
            detail.as_deref(),
            Some("mob member already exists: deploy-monitor")
        );
    }

    #[tokio::test]
    async fn probe_runtime_error_is_reported_as_probe_failure() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        *runtime
            .member_exists_error
            .lock()
            .expect("member exists error lock") = Some("storage unavailable".to_string());
        let host = MobMcpScheduleHost::from_runtime(runtime);
        let binding = MobTargetBinding::Member {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            action: ScheduledMobAction::Send {
                content: ContentInput::Text("Check deploy state.".to_string()),
                render_metadata: None,
            },
        };

        let error = host
            .probe_mob_target(&binding)
            .await
            .expect_err("probe error should not be downgraded to missing");

        assert!(
            matches!(error, ScheduleDomainError::ProbeFailed(detail) if detail == "internal error: storage unavailable")
        );
    }

    #[test]
    fn helper_options_reject_resolved_spawn_snapshot_without_live_authority() {
        let filter = ToolFilter::Allow(["send", "read_file"].into_iter().collect());
        let filter_witnesses: std::collections::BTreeMap<
            String,
            meerkat_core::ToolVisibilityWitness,
        > = [
            (
                "send".to_string(),
                meerkat_core::ToolVisibilityWitness {
                    last_seen_provenance: Some(meerkat_core::ToolProvenance {
                        kind: meerkat_core::ToolSourceKind::Callback,
                        source_id: "send".into(),
                    }),
                },
            ),
            (
                "read_file".to_string(),
                meerkat_core::ToolVisibilityWitness {
                    last_seen_provenance: Some(meerkat_core::ToolProvenance {
                        kind: meerkat_core::ToolSourceKind::Callback,
                        source_id: "read_file".into(),
                    }),
                },
            ),
        ]
        .into_iter()
        .collect();
        let spec = HelperOptionsSpec {
            tooling: Some(ScheduleSpawnTooling::Profile {
                name: "delegate".to_string(),
                allow_overlay: None,
                deny_overlay: None,
            }),
            resolved_spawn_snapshot: Some(ResolvedSpawnSnapshot {
                tool_filter: filter,
                tool_filter_witnesses: filter_witnesses,
            }),
            ..HelperOptionsSpec::default()
        };

        let error = helper_options_from_spec(&spec)
            .expect_err("resolved snapshots must not authorize scheduled inherited visibility");

        assert!(
            matches!(&error, ScheduleDomainError::InvalidSchedule(message)
                if message.contains("compatibility mirrors")
                    && message.contains("live generated parent authority")),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn helper_options_reject_malformed_resolved_snapshot_without_live_authority() {
        let spec = HelperOptionsSpec {
            resolved_spawn_snapshot: Some(ResolvedSpawnSnapshot {
                tool_filter: ToolFilter::Allow(["send", "read_file"].into_iter().collect()),
                tool_filter_witnesses: [(
                    "send".to_string(),
                    meerkat_core::ToolVisibilityWitness {
                        last_seen_provenance: Some(meerkat_core::ToolProvenance {
                            kind: meerkat_core::ToolSourceKind::Callback,
                            source_id: "send".into(),
                        }),
                    },
                )]
                .into_iter()
                .collect(),
            }),
            ..HelperOptionsSpec::default()
        };

        let error = helper_options_from_spec(&spec)
            .expect_err("invalid snapshot witness set should fail closed");

        assert!(
            matches!(&error, ScheduleDomainError::InvalidSchedule(message)
                if message.contains("compatibility mirrors")
                    && message.contains("live generated parent authority")),
            "unexpected error: {error:?}"
        );
    }

    #[tokio::test]
    async fn flow_completion_reports_missing_when_run_disappears() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        *runtime
            .flow_run_missing
            .lock()
            .expect("flow run missing lock") = true;
        let run_id = RunId::new();
        *runtime.next_run_id.lock().expect("run id lock") = Some(run_id.clone());
        let host = MobMcpScheduleHost::from_runtime(runtime);
        let binding = MobTargetBinding::Flow {
            mob_id: "ops".to_string(),
            flow_id: "release-check".to_string(),
            params: flow_params(r"{}"),
        };
        let occurrence = sample_occurrence(binding.clone());
        let delivery_identity = sample_delivery_identity(&occurrence);

        let dispatch = host
            .deliver_mob_target(&occurrence, &delivery_identity, &binding)
            .await
            .expect("delivery dispatch");
        let terminal = (dispatch.completion).await.expect("completion");

        assert_eq!(terminal.phase, meerkat::OccurrencePhase::DeliveryFailed);
        assert_eq!(
            terminal.delivery_failure_reason,
            Some(DeliveryFailureReason::TargetMissing)
        );
        assert_eq!(
            terminal.detail.as_deref(),
            Some(format!("mob flow run disappeared: {run_id}").as_str())
        );
    }

    #[tokio::test]
    async fn flow_completion_uses_generated_public_result_class_for_success() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        let run_id = RunId::new();
        *runtime.next_run_id.lock().expect("run id lock") = Some(run_id.clone());
        let host = MobMcpScheduleHost::from_runtime(runtime);
        let binding = MobTargetBinding::Flow {
            mob_id: "ops".to_string(),
            flow_id: "release-check".to_string(),
            params: flow_params(r"{}"),
        };
        let occurrence = sample_occurrence(binding.clone());
        let delivery_identity = sample_delivery_identity(&occurrence);

        let dispatch = host
            .deliver_mob_target(&occurrence, &delivery_identity, &binding)
            .await
            .expect("delivery dispatch");
        let terminal = (dispatch.completion).await.expect("completion");

        assert_eq!(terminal.phase, meerkat::OccurrencePhase::Completed);
        assert_eq!(terminal.delivery_failure_reason, None);
        assert_eq!(terminal.detail, None);
    }

    #[tokio::test]
    async fn flow_completion_uses_generated_public_result_class_for_failure() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        *runtime.flow_status.lock().expect("flow status lock") = MobRunStatus::Failed;
        let run_id = RunId::new();
        *runtime.next_run_id.lock().expect("run id lock") = Some(run_id.clone());
        let host = MobMcpScheduleHost::from_runtime(runtime);
        let binding = MobTargetBinding::Flow {
            mob_id: "ops".to_string(),
            flow_id: "release-check".to_string(),
            params: flow_params(r"{}"),
        };
        let occurrence = sample_occurrence(binding.clone());
        let delivery_identity = sample_delivery_identity(&occurrence);

        let dispatch = host
            .deliver_mob_target(&occurrence, &delivery_identity, &binding)
            .await
            .expect("delivery dispatch");
        let terminal = (dispatch.completion).await.expect("completion");

        assert_eq!(terminal.phase, meerkat::OccurrencePhase::DeliveryFailed);
        assert_eq!(
            terminal.delivery_failure_reason,
            Some(DeliveryFailureReason::MobRejected)
        );
        assert_eq!(
            terminal.detail.as_deref(),
            Some("mob flow terminated as Failed")
        );
    }

    #[test]
    fn delivery_failure_reason_maps_common_target_failures() {
        assert_eq!(
            delivery_failure_reason_for(&MobError::MemberNotFound(AgentIdentity::from("missing"))),
            DeliveryFailureReason::TargetMissing
        );
        assert_eq!(
            delivery_failure_reason_for(&MobError::MemberAlreadyExists(AgentIdentity::from(
                "busy"
            ))),
            DeliveryFailureReason::TargetBusy
        );
        assert_eq!(
            delivery_failure_reason_for(&MobError::Internal("boom".to_string())),
            DeliveryFailureReason::InternalError
        );
    }
}
