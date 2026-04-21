use std::sync::{Arc, Weak};
#[cfg(feature = "mob")]
use std::time::Duration;

use async_trait::async_trait;
#[cfg(feature = "mob")]
use meerkat::DeliveryTerminal;
use meerkat::surface::{
    RuntimeAdmissionProjection, ScheduledPromptDispatch, SharedScheduleTargetAdapter,
    SurfaceScheduleMobHost, SurfaceScheduleSessionHost, dispatch_from_admission,
    immediate_delivery_failure, project_runtime_admission, schedule_attempt_idempotency_key,
    schedule_host_supported, spawn_schedule_host,
};
#[cfg(feature = "mob")]
use meerkat::surface::{async_completion_dispatch, immediate_completed_dispatch};
use meerkat::{
    AgentBuildConfig, DeliveryDispatch, MobTargetBinding, Occurrence, OccurrenceFailureClass,
    ScheduleDomainError, SessionMaterializationSpec, SessionTargetBinding, TargetProbeOutcome,
};
#[cfg(feature = "mob")]
use meerkat::{
    ForkContextSpec, HelperOptionsSpec, ScheduledMobBackendKind, ScheduledMobRuntimeMode,
};
use meerkat_contracts::SkillsParams;
#[cfg(feature = "mob")]
use meerkat_core::types::HandlingMode;
use meerkat_core::{ContentInput, SessionId};
#[cfg(feature = "mob")]
use meerkat_mob::{
    AgentIdentity, FlowId, ForkContext, HelperOptions, MobBackendKind, MobId, MobRunStatus, RunId,
};
#[cfg(feature = "mob")]
use meerkat_mob_mcp::MobMcpState;
use meerkat_runtime::{
    CorrelationId, IdempotencyKey, Input, InputDurability, InputHeader, InputOrigin,
    InputVisibility, PromptInput,
};
#[cfg(feature = "mob")]
use tokio::time::sleep;

use super::{SessionRuntime, SessionState, session_metadata_marks_archived};

struct RpcScheduleTargetAdapter {
    runtime: Weak<SessionRuntime>,
}

impl RpcScheduleTargetAdapter {
    fn new(runtime: &Arc<SessionRuntime>) -> Self {
        Self {
            runtime: Arc::downgrade(runtime),
        }
    }

    fn upgrade_runtime(&self) -> Result<Arc<SessionRuntime>, ScheduleDomainError> {
        self.runtime
            .upgrade()
            .ok_or(ScheduleDomainError::DriverStopped)
    }

    #[cfg(feature = "mob")]
    fn mob_state(
        &self,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<Arc<MobMcpState>, ScheduleDomainError> {
        runtime.mob_state().ok_or_else(|| {
            ScheduleDomainError::Internal("mob state unavailable on RPC host".into())
        })
    }
}

#[async_trait]
impl SurfaceScheduleSessionHost for RpcScheduleTargetAdapter {
    async fn probe_session_target(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;
        let Some(session_id) = binding.resolved_session_id() else {
            return Ok(TargetProbeOutcome::Ready);
        };

        if let Some(info) = runtime.session_state(session_id).await {
            return Ok(if info.state == SessionState::Running {
                TargetProbeOutcome::Busy {
                    detail: Some(format!("session still running: {session_id}")),
                }
            } else {
                TargetProbeOutcome::Ready
            });
        }

        let persisted = runtime
            .load_persisted_session(session_id)
            .await
            .map_err(rpc_to_schedule)?;
        match persisted {
            Some(session) if !session_metadata_marks_archived(&session) => {
                Ok(TargetProbeOutcome::Ready)
            }
            _ => Ok(TargetProbeOutcome::Missing {
                detail: Some(format!("session not found: {session_id}")),
            }),
        }
    }

    async fn materialize_session(
        &self,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;
        runtime
            .materialize_scheduled_session(create, prompt_system_prompt)
            .await
    }

    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;
        runtime
            .deliver_scheduled_prompt(session_id, occurrence, dispatch)
            .await
    }

    async fn deliver_event(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        materialized_session_id: Option<SessionId>,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;
        runtime
            .deliver_scheduled_event(
                session_id,
                occurrence,
                event_type,
                payload,
                render_metadata,
                materialized_session_id,
            )
            .await
    }
}

#[async_trait]
impl SurfaceScheduleMobHost for RpcScheduleTargetAdapter {
    async fn probe_mob_target(
        &self,
        binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;

        #[cfg(feature = "mob")]
        {
            let mob_state = self.mob_state(&runtime)?;
            let mob_id = MobId::from(mob_binding_mob_id(binding));
            let handle = match mob_state.handle_for(&mob_id).await {
                Ok(handle) => handle,
                Err(error) => {
                    return Ok(TargetProbeOutcome::Missing {
                        detail: Some(error.to_string()),
                    });
                }
            };

            return match binding {
                MobTargetBinding::Member { member_id, .. } => {
                    let identity = AgentIdentity::from(member_id.as_str());
                    Ok(if handle.get_member(&identity).await.is_some() {
                        TargetProbeOutcome::Ready
                    } else {
                        TargetProbeOutcome::Missing {
                            detail: Some(format!("mob member not found: {member_id}")),
                        }
                    })
                }
                MobTargetBinding::Flow { flow_id, .. } => {
                    let expected = FlowId::from(flow_id.as_str());
                    Ok(
                        if handle.list_flows().into_iter().any(|flow| flow == expected) {
                            TargetProbeOutcome::Ready
                        } else {
                            TargetProbeOutcome::Missing {
                                detail: Some(format!("mob flow not found: {flow_id}")),
                            }
                        },
                    )
                }
                MobTargetBinding::SpawnHelper { member_id, .. } => {
                    let identity = AgentIdentity::from(member_id.as_str());
                    Ok(if handle.get_member(&identity).await.is_some() {
                        TargetProbeOutcome::Busy {
                            detail: Some(format!("mob member already exists: {member_id}")),
                        }
                    } else {
                        TargetProbeOutcome::Ready
                    })
                }
                MobTargetBinding::ForkHelper {
                    source_member_id,
                    member_id,
                    ..
                } => {
                    let source = AgentIdentity::from(source_member_id.as_str());
                    if handle.get_member(&source).await.is_none() {
                        return Ok(TargetProbeOutcome::Missing {
                            detail: Some(format!(
                                "mob source member not found: {source_member_id}"
                            )),
                        });
                    }
                    let target = AgentIdentity::from(member_id.as_str());
                    Ok(if handle.get_member(&target).await.is_some() {
                        TargetProbeOutcome::Busy {
                            detail: Some(format!("mob member already exists: {member_id}")),
                        }
                    } else {
                        TargetProbeOutcome::Ready
                    })
                }
            };
        }

        #[cfg(not(feature = "mob"))]
        {
            let _ = runtime;
            let _ = binding;
            Ok(TargetProbeOutcome::Missing {
                detail: Some("scheduled mob targets are not yet available on the RPC host".into()),
            })
        }
    }

    async fn deliver_mob_target(
        &self,
        occurrence: &Occurrence,
        binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;

        #[cfg(feature = "mob")]
        {
            let mob_state = self.mob_state(&runtime)?;
            let mob_id = MobId::from(mob_binding_mob_id(binding));

            return match binding {
                MobTargetBinding::Member {
                    member_id,
                    action:
                        meerkat::ScheduledMobAction::Send {
                            content,
                            render_metadata,
                        },
                    ..
                } => match mob_state
                    .mob_member_send(
                        &mob_id,
                        AgentIdentity::from(member_id.as_str()),
                        content.clone(),
                        HandlingMode::Queue,
                        render_metadata.clone(),
                    )
                    .await
                {
                    Ok(receipt) => Ok(immediate_completed_dispatch(
                        occurrence,
                        Some(receipt.identity.to_string()),
                    )),
                    Err(error) => Ok(immediate_delivery_failure(
                        occurrence,
                        error.to_string(),
                        OccurrenceFailureClass::MobRejected,
                        None,
                        None,
                    )),
                },
                MobTargetBinding::Flow {
                    flow_id, params, ..
                } => {
                    let params: serde_json::Value =
                        serde_json::from_str(params.get()).map_err(|error| {
                            ScheduleDomainError::InvalidSchedule(format!(
                                "invalid mob flow params: {error}"
                            ))
                        })?;
                    match mob_state
                        .mob_run_flow(&mob_id, FlowId::from(flow_id.as_str()), params)
                        .await
                    {
                        Ok(run_id) => Ok(async_completion_dispatch(
                            occurrence,
                            Some(run_id.to_string()),
                            mob_flow_completion_future(mob_state, mob_id, run_id),
                        )),
                        Err(error) => Ok(immediate_delivery_failure(
                            occurrence,
                            error.to_string(),
                            OccurrenceFailureClass::MobRejected,
                            None,
                            None,
                        )),
                    }
                }
                MobTargetBinding::SpawnHelper {
                    member_id,
                    prompt,
                    options,
                    ..
                } => {
                    let meerkat_id = AgentIdentity::from(member_id.as_str());
                    let helper_options = helper_options_from_spec(options)?;
                    let prompt = prompt.clone();
                    Ok(async_completion_dispatch(
                        occurrence,
                        Some(member_id.clone()),
                        Box::pin(async move {
                            match mob_state
                                .mob_spawn_helper(&mob_id, meerkat_id, prompt, helper_options)
                                .await
                            {
                                Ok(_) => Ok(DeliveryTerminal::completed(None)),
                                Err(error) => Ok(mob_delivery_failed_terminal(error)),
                            }
                        }),
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
                    let meerkat_id = AgentIdentity::from(member_id.as_str());
                    let helper_options = helper_options_from_spec(options)?;
                    let fork_context = fork_context_from_spec(fork_context);
                    let prompt = prompt.clone();
                    Ok(async_completion_dispatch(
                        occurrence,
                        Some(member_id.clone()),
                        Box::pin(async move {
                            match mob_state
                                .mob_fork_helper(
                                    &mob_id,
                                    &source_identity,
                                    meerkat_id,
                                    prompt,
                                    fork_context,
                                    helper_options,
                                )
                                .await
                            {
                                Ok(_) => Ok(DeliveryTerminal::completed(None)),
                                Err(error) => Ok(mob_delivery_failed_terminal(error)),
                            }
                        }),
                    ))
                }
            };
        }

        #[cfg(not(feature = "mob"))]
        {
            let _ = runtime;
            let _ = binding;
            Ok(immediate_delivery_failure(
                occurrence,
                "scheduled mob targets are not yet available on the RPC host".to_string(),
                OccurrenceFailureClass::MobRejected,
                None,
                None,
            ))
        }
    }
}

impl SessionRuntime {
    pub async fn ensure_schedule_host_started(self: &Arc<Self>) -> Result<(), ScheduleDomainError> {
        if !schedule_host_supported(self.schedule_service.store().kind()) {
            return Ok(());
        }

        let mut slot = self.schedule_host.lock().await;
        if slot.is_some() {
            return Ok(());
        }

        let surface_adapter = Arc::new(RpcScheduleTargetAdapter::new(self));
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = surface_adapter.clone();
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = surface_adapter;
        let shared_adapter = Arc::new(SharedScheduleTargetAdapter::new(
            self.schedule_service(),
            session_host,
            mob_host,
        ));
        *slot = Some(spawn_schedule_host(
            self.schedule_service(),
            shared_adapter,
            self.schedule_owner_id(),
        ));
        Ok(())
    }

    pub(super) async fn shutdown_schedule_host(&self) {
        let mut slot = self.schedule_host.lock().await;
        let Some(handle) = slot.take() else {
            return;
        };
        drop(slot);
        handle.shutdown().await;
    }

    pub(super) async fn materialize_scheduled_session(
        &self,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError> {
        let mut build_config = AgentBuildConfig::new(create.model.clone());
        build_config.provider = create.provider;
        build_config.max_tokens = create.max_tokens;
        build_config.system_prompt = prompt_system_prompt
            .map(str::to_owned)
            .or_else(|| create.system_prompt.clone());
        build_config.output_schema = create.output_schema.clone();
        build_config.structured_output_retries = create.structured_output_retries;
        build_config.provider_params = create.provider_params.clone();
        build_config.comms_name = create.comms_name.clone();
        build_config.peer_meta = create.peer_meta.clone();
        build_config.preload_skills = (!create.preload_skills.is_empty()).then(|| {
            create
                .preload_skills
                .iter()
                .cloned()
                .map(Into::into)
                .collect()
        });
        build_config.additional_instructions = (!create.additional_instructions.is_empty())
            .then(|| create.additional_instructions.clone());
        build_config.realm_id = create.realm_id.clone().or_else(|| self.realm_id.clone());
        build_config.instance_id = create
            .instance_id
            .clone()
            .or_else(|| self.instance_id.clone());
        build_config.backend = create.backend.clone().or_else(|| self.backend.clone());
        build_config.keep_alive = create.keep_alive;
        build_config.app_context = create.app_context.clone();
        build_config.config_generation = if let Some(runtime) = self.config_runtime() {
            runtime.get().await.ok().map(|snapshot| snapshot.generation)
        } else {
            create.config_generation
        };
        if build_config.llm_client_override.is_none()
            && let Some(default_llm_client) = self.default_llm_client()
        {
            build_config.llm_client_override = Some(default_llm_client);
        }

        self.create_session(build_config, Some(create.labels.clone()), None)
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.message))
    }

    async fn deliver_scheduled_prompt(
        self: &Arc<Self>,
        session_id: &SessionId,
        occurrence: &Occurrence,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        match self
            .accept_scheduled_prompt_with_completion(
                session_id,
                occurrence,
                dispatch.prompt,
                dispatch.render_metadata,
                dispatch.skill_references,
                dispatch.additional_instructions,
            )
            .await
        {
            Ok(projection) => Ok(dispatch_from_admission(
                occurrence,
                projection,
                dispatch.materialized_session_id,
            )),
            Err(error) => Ok(immediate_delivery_failure(
                occurrence,
                error.to_string(),
                OccurrenceFailureClass::RuntimeRejected,
                None,
                dispatch.materialized_session_id,
            )),
        }
    }

    async fn deliver_scheduled_event(
        self: &Arc<Self>,
        session_id: &SessionId,
        occurrence: &Occurrence,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        materialized_session_id: Option<SessionId>,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        match self
            .accept_scheduled_event_with_completion(
                session_id,
                occurrence,
                event_type,
                payload,
                render_metadata,
            )
            .await
        {
            Ok(projection) => Ok(dispatch_from_admission(
                occurrence,
                projection,
                materialized_session_id,
            )),
            Err(error) => Ok(immediate_delivery_failure(
                occurrence,
                error.to_string(),
                OccurrenceFailureClass::RuntimeRejected,
                None,
                materialized_session_id,
            )),
        }
    }

    async fn accept_scheduled_prompt_with_completion(
        self: &Arc<Self>,
        session_id: &SessionId,
        occurrence: &Occurrence,
        prompt: ContentInput,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        skill_references: Vec<String>,
        additional_instructions: Vec<String>,
    ) -> Result<RuntimeAdmissionProjection, ScheduleDomainError> {
        if self
            .live_session_is_stale(session_id)
            .await
            .map_err(rpc_to_schedule)?
        {
            let _ = self.service.discard_live_session(session_id).await;
            self.runtime_adapter.unregister_session(session_id).await;
        }

        let skill_keys = canonical_skill_keys(self, skill_references)?;
        let turn_metadata = Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: None,
                keep_alive: None,
                skill_references: skill_keys,
                flow_tool_overlay: None,
                additional_instructions: (!additional_instructions.is_empty())
                    .then_some(additional_instructions),
                model: None,
                provider: None,
                provider_params: None,
                render_metadata,
                execution_kind: None,
                connection_ref: None,
            },
        );
        let mut prompt_input = PromptInput::from_content_input(prompt, turn_metadata);
        prompt_input.header.source = InputOrigin::System;
        prompt_input.header.idempotency_key = Some(IdempotencyKey::new(
            schedule_attempt_idempotency_key(occurrence),
        ));
        prompt_input.header.correlation_id =
            Some(CorrelationId::from_uuid(occurrence.occurrence_id.0));

        #[cfg(feature = "mcp")]
        {
            let (event_tx, _event_rx) = tokio::sync::mpsc::channel(1);
            let mut mcp_text = String::new();
            self.apply_mcp_boundary(session_id, &event_tx, &mut mcp_text)
                .await
                .map_err(rpc_to_schedule)?;
            if !mcp_text.is_empty() {
                let mut blocks = prompt_input.blocks.clone().unwrap_or_else(|| {
                    vec![meerkat_core::types::ContentBlock::Text {
                        text: prompt_input.text.clone(),
                    }]
                });
                blocks.insert(
                    0,
                    meerkat_core::types::ContentBlock::Text { text: mcp_text },
                );
                prompt_input = PromptInput::from_content_input(
                    ContentInput::Blocks(blocks),
                    prompt_input.turn_metadata.clone(),
                );
                prompt_input.header.source = InputOrigin::System;
                prompt_input.header.idempotency_key = Some(IdempotencyKey::new(
                    schedule_attempt_idempotency_key(occurrence),
                ));
                prompt_input.header.correlation_id =
                    Some(CorrelationId::from_uuid(occurrence.occurrence_id.0));
            }
        }

        self.ensure_runtime_executor(session_id)
            .await
            .map_err(rpc_to_schedule)?;
        update_peer_ingress_context(self, session_id).await;

        let (outcome, handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, Input::Prompt(prompt_input))
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        Ok(project_runtime_admission(outcome, handle))
    }

    async fn accept_scheduled_event_with_completion(
        self: &Arc<Self>,
        session_id: &SessionId,
        occurrence: &Occurrence,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
    ) -> Result<RuntimeAdmissionProjection, ScheduleDomainError> {
        if self
            .live_session_is_stale(session_id)
            .await
            .map_err(rpc_to_schedule)?
        {
            let _ = self.service.discard_live_session(session_id).await;
            self.runtime_adapter.unregister_session(session_id).await;
        }

        self.ensure_runtime_executor(session_id)
            .await
            .map_err(rpc_to_schedule)?;
        update_peer_ingress_context(self, session_id).await;

        let input = Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: InputOrigin::External {
                    source_name: format!("schedule:{}", occurrence.schedule_id),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: Some(IdempotencyKey::new(schedule_attempt_idempotency_key(
                    occurrence,
                ))),
                supersession_key: None,
                correlation_id: Some(CorrelationId::from_uuid(occurrence.occurrence_id.0)),
            },
            event_type,
            payload,
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata,
        });

        let (outcome, handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, input)
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        Ok(project_runtime_admission(outcome, handle))
    }

    fn schedule_owner_id(&self) -> String {
        let realm = self.realm_id.as_deref().unwrap_or("realm");
        let instance = self.instance_id.as_deref().unwrap_or("rpc");
        format!("rpc-scheduler:{realm}:{instance}")
    }
}

#[cfg(feature = "comms")]
async fn update_peer_ingress_context(runtime: &Arc<SessionRuntime>, session_id: &SessionId) {
    let keep_alive = runtime
        .load_persisted_session(session_id)
        .await
        .ok()
        .flatten()
        .and_then(|session| {
            session
                .session_metadata()
                .map(|metadata| metadata.keep_alive)
        })
        .unwrap_or(false);
    let comms_rt = runtime.service.comms_runtime(session_id).await;
    runtime
        .runtime_adapter
        .update_peer_ingress_context(session_id, keep_alive, comms_rt)
        .await;
}

#[cfg(not(feature = "comms"))]
async fn update_peer_ingress_context(_runtime: &Arc<SessionRuntime>, _session_id: &SessionId) {}

fn canonical_skill_keys(
    runtime: &SessionRuntime,
    skill_references: Vec<String>,
) -> Result<Option<Vec<meerkat_core::skills::SkillKey>>, ScheduleDomainError> {
    if skill_references.is_empty() {
        return Ok(None);
    }

    SkillsParams {
        preload_skills: None,
        skill_refs: None,
        skill_references: Some(skill_references),
    }
    .canonical_skill_keys_with_registry(&runtime.skill_identity_registry())
    .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))
}

fn rpc_to_schedule(error: crate::protocol::RpcError) -> ScheduleDomainError {
    ScheduleDomainError::Internal(error.message)
}

#[cfg(feature = "mob")]
fn mob_binding_mob_id(binding: &MobTargetBinding) -> &str {
    match binding {
        MobTargetBinding::Member { mob_id, .. }
        | MobTargetBinding::Flow { mob_id, .. }
        | MobTargetBinding::SpawnHelper { mob_id, .. }
        | MobTargetBinding::ForkHelper { mob_id, .. } => mob_id,
    }
}

#[cfg(feature = "mob")]
fn helper_options_from_spec(
    spec: &HelperOptionsSpec,
) -> Result<HelperOptions, ScheduleDomainError> {
    let mut options = HelperOptions::default();
    options.role_name = spec.role_name.clone().map(Into::into);
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

#[cfg(feature = "mob")]
fn fork_context_from_spec(spec: &ForkContextSpec) -> ForkContext {
    match spec {
        ForkContextSpec::FullHistory => ForkContext::FullHistory,
        ForkContextSpec::LastMessages { count } => ForkContext::LastMessages { count: *count },
    }
}

#[cfg(feature = "mob")]
fn mob_delivery_failed_terminal(error: meerkat_mob::MobError) -> DeliveryTerminal {
    DeliveryTerminal::delivery_failed(error.to_string(), OccurrenceFailureClass::MobRejected)
}

#[cfg(feature = "mob")]
fn mob_flow_completion_future(
    mob_state: Arc<MobMcpState>,
    mob_id: MobId,
    run_id: RunId,
) -> meerkat::DeliveryCompletion {
    Box::pin(async move {
        loop {
            match mob_state.mob_flow_status(&mob_id, run_id.clone()).await {
                Ok(Some(run)) if run.status == MobRunStatus::Completed => {
                    return Ok(DeliveryTerminal::completed(None));
                }
                Ok(Some(run)) if run.status.is_terminal() => {
                    return Ok(DeliveryTerminal::delivery_failed(
                        format!("mob flow terminated as {:?}", run.status),
                        OccurrenceFailureClass::MobRejected,
                    ));
                }
                Ok(Some(_) | None) => sleep(Duration::from_millis(100)).await,
                Err(error) => return Ok(mob_delivery_failed_terminal(error)),
            }
        }
    })
}
