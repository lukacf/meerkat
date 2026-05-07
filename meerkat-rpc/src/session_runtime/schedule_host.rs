use std::sync::{Arc, Weak};

use async_trait::async_trait;
use meerkat::surface::{
    AcceptedScheduledInput, NoopScheduleMobHost, ScheduledPromptDispatch,
    SharedScheduleTargetAdapter, SurfaceScheduleMobHost, SurfaceScheduleSessionHost,
    build_dispatch_from_accepted, immediate_delivery_failure, schedule_attempt_idempotency_key,
    schedule_host_supported, spawn_schedule_host,
};
use meerkat::{
    AgentBuildConfig, DeliveryDispatch, Occurrence, OccurrenceFailureClass, ScheduleDomainError,
    SessionMaterializationSpec, SessionTargetBinding, TargetProbeOutcome,
};
use meerkat_core::SessionId;
#[cfg(feature = "mob")]
use meerkat_mob_mcp::MobMcpScheduleHost;

use super::{SessionRuntime, SessionState};

fn runtime_delivery_dispatch(
    occurrence: &Occurrence,
    outcome: meerkat_runtime::accept::AcceptOutcome,
    handle: Option<meerkat_runtime::completion::CompletionHandle>,
    materialized_session_id: Option<SessionId>,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    match outcome {
        meerkat_runtime::accept::AcceptOutcome::Accepted { input_id, .. } => {
            Ok(build_dispatch_from_accepted(
                occurrence,
                AcceptedScheduledInput {
                    correlation_id: Some(input_id.to_string()),
                    handle,
                },
                materialized_session_id,
            ))
        }
        meerkat_runtime::accept::AcceptOutcome::Deduplicated { existing_id, .. } => {
            Ok(build_dispatch_from_accepted(
                occurrence,
                AcceptedScheduledInput {
                    correlation_id: Some(existing_id.to_string()),
                    handle: None,
                },
                materialized_session_id,
            ))
        }
        meerkat_runtime::accept::AcceptOutcome::Rejected { reason } => {
            Ok(immediate_delivery_failure(
                occurrence,
                reason.to_string(),
                OccurrenceFailureClass::RuntimeRejected,
                None,
                materialized_session_id,
            ))
        }
        _ => Ok(immediate_delivery_failure(
            occurrence,
            "runtime returned an unknown admission outcome".to_string(),
            OccurrenceFailureClass::RuntimeRejected,
            None,
            materialized_session_id,
        )),
    }
}

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
            Some(_) => Ok(TargetProbeOutcome::Ready),
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
        Box::pin(runtime.materialize_scheduled_session(create, prompt_system_prompt)).await
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

impl SessionRuntime {
    pub async fn ensure_schedule_host_started(self: &Arc<Self>) -> Result<(), ScheduleDomainError> {
        if !schedule_host_supported(self.schedule_service.store().kind()) {
            return Ok(());
        }

        let mut slot = self.schedule_host.lock().await;
        if slot.is_some() {
            return Ok(());
        }

        let session_host: Arc<dyn SurfaceScheduleSessionHost> =
            Arc::new(RpcScheduleTargetAdapter::new(self));
        let mob_host = rpc_schedule_mob_host(self);
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
        // Post-wave-a dogma: typed `SkillKey` path only; legacy string
        // `preload_skills: Vec<String>` from schedule types is not auto-parsed.
        // When the schedule surface needs to carry preload skills it must do
        // so via the typed `SkillKey` seam.
        let _ = &create.preload_skills;
        build_config.preload_skills = None;
        build_config.additional_instructions = (!create.additional_instructions.is_empty())
            .then(|| create.additional_instructions.clone());
        build_config.realm_id = create
            .realm_id
            .clone()
            .or_else(|| self.realm_id.as_ref().map(|r| r.as_str().to_string()));
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
        self.ensure_runtime_executor(session_id)
            .await
            .map_err(rpc_to_schedule)?;

        let comms_rt = self.service.comms_runtime(session_id).await;
        #[cfg(feature = "comms")]
        let peer_ingress_enabled = self.preserve_existing_peer_ingress(session_id, false).await;
        #[cfg(not(feature = "comms"))]
        let peer_ingress_enabled = false;
        if comms_rt.is_some() || peer_ingress_enabled {
            self.runtime_adapter
                .update_peer_ingress_context(session_id, peer_ingress_enabled, comms_rt)
                .await;
        }

        let turn_metadata = Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: None,
                keep_alive: None,
                skill_references: (!dispatch.skill_refs.is_empty()).then(|| {
                    dispatch
                        .skill_refs
                        .iter()
                        .map(|skill_ref| skill_ref.key().clone())
                        .collect()
                }),
                flow_tool_overlay: None,
                additional_instructions: (!dispatch.additional_instructions.is_empty()).then(
                    || {
                        dispatch
                            .additional_instructions
                            .iter()
                            .map(|body| {
                                meerkat_core::lifecycle::run_primitive::TurnInstruction {
                                    kind: meerkat_core::lifecycle::run_primitive::TurnInstructionKind::Host,
                                    body: body.clone(),
                                }
                            })
                            .collect()
                    },
                ),
                model: None,
                provider: None,
                provider_params: None,
                render_metadata: dispatch.render_metadata.clone(),
                execution_kind: None,
                peer_response_terminal_apply_intent: None,
                auth_binding: None,
            },
        );
        let mut prompt_input =
            meerkat_runtime::PromptInput::from_content_input(dispatch.prompt, turn_metadata);
        prompt_input.header.source = meerkat_runtime::InputOrigin::System;
        prompt_input.header.idempotency_key = Some(meerkat_runtime::IdempotencyKey::new(
            schedule_attempt_idempotency_key(occurrence),
        ));
        prompt_input.header.correlation_id = Some(meerkat_runtime::CorrelationId::from_uuid(
            occurrence.occurrence_id.0,
        ));

        let (outcome, handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, meerkat_runtime::Input::Prompt(prompt_input))
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;

        runtime_delivery_dispatch(
            occurrence,
            outcome,
            handle,
            dispatch.materialized_session_id,
        )
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
        self.ensure_runtime_executor(session_id)
            .await
            .map_err(rpc_to_schedule)?;

        let comms_rt = self.service.comms_runtime(session_id).await;
        #[cfg(feature = "comms")]
        let peer_ingress_enabled = self.preserve_existing_peer_ingress(session_id, false).await;
        #[cfg(not(feature = "comms"))]
        let peer_ingress_enabled = false;
        if comms_rt.is_some() || peer_ingress_enabled {
            self.runtime_adapter
                .update_peer_ingress_context(session_id, peer_ingress_enabled, comms_rt)
                .await;
        }

        let input = meerkat_runtime::Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
            header: meerkat_runtime::input::InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: meerkat_runtime::InputOrigin::External {
                    source_name: format!("schedule:{}", occurrence.schedule_id),
                },
                durability: meerkat_runtime::input::InputDurability::Durable,
                visibility: meerkat_runtime::input::InputVisibility::default(),
                idempotency_key: Some(meerkat_runtime::IdempotencyKey::new(
                    schedule_attempt_idempotency_key(occurrence),
                )),
                supersession_key: None,
                correlation_id: Some(meerkat_runtime::CorrelationId::from_uuid(
                    occurrence.occurrence_id.0,
                )),
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

        runtime_delivery_dispatch(occurrence, outcome, handle, materialized_session_id)
    }

    fn schedule_owner_id(&self) -> String {
        let realm = self
            .realm_id
            .as_ref()
            .map(|r| r.as_str())
            .unwrap_or("realm");
        let instance = self.instance_id.as_deref().unwrap_or("rpc");
        format!("rpc-scheduler:{realm}:{instance}")
    }
}

fn rpc_to_schedule(error: crate::protocol::RpcError) -> ScheduleDomainError {
    ScheduleDomainError::Internal(error.message)
}

fn rpc_schedule_mob_host(runtime: &Arc<SessionRuntime>) -> Arc<dyn SurfaceScheduleMobHost> {
    #[cfg(feature = "mob")]
    {
        if let Some(mob_state) = runtime.mob_state() {
            return Arc::new(MobMcpScheduleHost::new(mob_state));
        }
        Arc::new(NoopScheduleMobHost::new(
            "mob state unavailable on RPC host",
        ))
    }

    #[cfg(not(feature = "mob"))]
    {
        let _ = runtime;
        Arc::new(NoopScheduleMobHost::new(
            "scheduled mob targets require the mob feature on the RPC host",
        ))
    }
}
