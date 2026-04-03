use super::{SessionRuntime, SessionState, session_metadata_marks_archived};
use async_trait::async_trait;
use chrono::Duration as ChronoDuration;
use meerkat::{
    AgentBuildConfig, DeliveryDispatch, DeliveryReceipt, DeliveryReceiptStage, DeliveryTerminal,
    ForkContextSpec, HelperOptionsSpec, MobTargetBinding, Occurrence, OccurrenceFailureClass,
    OccurrencePhase, ScheduleDomainError, ScheduleDriver, ScheduleDriverConfig, ScheduleStoreKind,
    ScheduleTargetDelivery, ScheduleTargetProbe, ScheduledMobBackendKind, ScheduledMobRuntimeMode,
    ScheduledSessionAction, SessionMaterializationSpec, SessionTargetBinding, TargetBinding,
    TargetProbeOutcome,
};
use meerkat_contracts::SkillsParams;
use meerkat_core::ops::ToolAccessPolicy;
use meerkat_core::skills::SkillRef;
use meerkat_core::{ContentInput, OutputSchema, SessionId, types::HandlingMode};
#[cfg(feature = "mob")]
use meerkat_mob::{
    FlowId, ForkContext, HelperOptions, MeerkatId, MobBackendKind, MobId, MobRunStatus, RunId,
};
#[cfg(feature = "mob")]
use meerkat_mob_mcp::MobMcpState;
use meerkat_runtime::completion::{CompletionHandle, CompletionOutcome};
use meerkat_runtime::{
    AcceptOutcome, CorrelationId, IdempotencyKey, Input, InputDurability, InputHeader, InputOrigin,
    InputVisibility, PromptInput,
};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::oneshot;
#[cfg(feature = "mob")]
use tokio::time::sleep;

pub(super) struct ScheduleHostHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: tokio::task::JoinHandle<()>,
}

impl ScheduleHostHandle {
    async fn shutdown(mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        let _ = self.join.await;
    }
}

struct RpcScheduleTargetAdapter {
    runtime: Weak<SessionRuntime>,
}

struct ResolvedScheduledSession {
    session_id: SessionId,
    materialized_session_id: Option<SessionId>,
    allow_system_prompt_override: bool,
}

struct AcceptedScheduledInput {
    correlation_id: String,
    handle: Option<CompletionHandle>,
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

    async fn probe_session_binding(
        &self,
        runtime: &Arc<SessionRuntime>,
        binding: &SessionTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
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
            .map_err(|error| ScheduleDomainError::ProbeFailed(error.message))?;
        match persisted {
            Some(session) if !session_metadata_marks_archived(&session) => {
                Ok(TargetProbeOutcome::Ready)
            }
            _ => Ok(TargetProbeOutcome::Missing {
                detail: Some(format!("session not found: {session_id}")),
            }),
        }
    }

    async fn resolve_session(
        &self,
        runtime: &Arc<SessionRuntime>,
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
                match runtime
                    .materialize_scheduled_session(create, prompt_system_prompt)
                    .await
                {
                    Ok(session_id) => {
                        if let Err(error) = runtime
                            .schedule_service()
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

    #[cfg(feature = "mob")]
    fn mob_state(
        &self,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<Arc<MobMcpState>, ScheduleDomainError> {
        runtime.mob_state().ok_or_else(|| {
            ScheduleDomainError::Internal("mob state unavailable on RPC host".into())
        })
    }

    #[cfg(feature = "mob")]
    async fn probe_mob_binding(
        &self,
        runtime: &Arc<SessionRuntime>,
        binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let mob_state = self.mob_state(runtime)?;
        let mob_id = MobId::from(mob_binding_mob_id(binding));
        let handle = match mob_state.handle_for(&mob_id).await {
            Ok(handle) => handle,
            Err(error) => {
                return Ok(TargetProbeOutcome::Missing {
                    detail: Some(error.to_string()),
                });
            }
        };

        match binding {
            MobTargetBinding::Member { member_id, .. } => {
                let meerkat_id = MeerkatId::from(member_id.as_str());
                Ok(if handle.get_member(&meerkat_id).await.is_some() {
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
                let meerkat_id = MeerkatId::from(member_id.as_str());
                Ok(if handle.get_member(&meerkat_id).await.is_some() {
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
                let source = MeerkatId::from(source_member_id.as_str());
                if handle.get_member(&source).await.is_none() {
                    return Ok(TargetProbeOutcome::Missing {
                        detail: Some(format!("mob source member not found: {source_member_id}")),
                    });
                }
                let target = MeerkatId::from(member_id.as_str());
                Ok(if handle.get_member(&target).await.is_some() {
                    TargetProbeOutcome::Busy {
                        detail: Some(format!("mob member already exists: {member_id}")),
                    }
                } else {
                    TargetProbeOutcome::Ready
                })
            }
        }
    }

    #[cfg(not(feature = "mob"))]
    async fn probe_mob_binding(
        &self,
        _runtime: &Arc<SessionRuntime>,
        _binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        Ok(TargetProbeOutcome::Missing {
            detail: Some("scheduled mob targets are not yet available on the RPC host".into()),
        })
    }

    #[cfg(feature = "mob")]
    async fn deliver_mob_binding(
        &self,
        runtime: &Arc<SessionRuntime>,
        occurrence: &Occurrence,
        binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let mob_state = self.mob_state(runtime)?;
        let mob_id = MobId::from(mob_binding_mob_id(binding));

        match binding {
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
                    MeerkatId::from(member_id.as_str()),
                    content.clone(),
                    HandlingMode::Queue,
                    render_metadata.clone(),
                )
                .await
            {
                Ok(receipt) => Ok(immediate_completed_dispatch(
                    occurrence,
                    Some(receipt.session_id.to_string()),
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
            } => match mob_state
                .mob_run_flow(&mob_id, FlowId::from(flow_id.as_str()), params.clone())
                .await
            {
                Ok(run_id) => {
                    let mut receipt = DeliveryReceipt::new(
                        occurrence.occurrence_id.clone(),
                        occurrence.attempt_count,
                        DeliveryReceiptStage::DispatchAccepted,
                    );
                    receipt.correlation_id = Some(run_id.to_string());
                    Ok(DeliveryDispatch {
                        receipt,
                        correlation_id: Some(run_id.to_string()),
                        materialized_session_id: None,
                        completion: mob_flow_completion_future(mob_state, mob_id, run_id),
                    })
                }
                Err(error) => Ok(immediate_delivery_failure(
                    occurrence,
                    error.to_string(),
                    OccurrenceFailureClass::MobRejected,
                    None,
                    None,
                )),
            },
            MobTargetBinding::SpawnHelper {
                member_id,
                prompt,
                options,
                ..
            } => {
                let meerkat_id = MeerkatId::from(member_id.as_str());
                let helper_options = helper_options_from_spec(options)?;
                let prompt = prompt.clone();
                Ok(async_mob_completion_dispatch(
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
                let source_member_id = MeerkatId::from(source_member_id.as_str());
                let meerkat_id = MeerkatId::from(member_id.as_str());
                let helper_options = helper_options_from_spec(options)?;
                let fork_context = fork_context_from_spec(fork_context);
                let prompt = prompt.clone();
                Ok(async_mob_completion_dispatch(
                    occurrence,
                    Some(member_id.clone()),
                    Box::pin(async move {
                        match mob_state
                            .mob_fork_helper(
                                &mob_id,
                                &source_member_id,
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
        }
    }

    #[cfg(not(feature = "mob"))]
    async fn deliver_mob_binding(
        &self,
        _runtime: &Arc<SessionRuntime>,
        occurrence: &Occurrence,
        _binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        Ok(immediate_delivery_failure(
            occurrence,
            "scheduled mob targets are not yet available on the RPC host".to_string(),
            OccurrenceFailureClass::MobRejected,
            None,
            None,
        ))
    }
}

#[async_trait]
impl ScheduleTargetProbe for RpcScheduleTargetAdapter {
    async fn probe_target(
        &self,
        occurrence: &Occurrence,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;
        match &occurrence.target_snapshot {
            TargetBinding::Session(binding) => self.probe_session_binding(&runtime, binding).await,
            TargetBinding::Mob(binding) => self.probe_mob_binding(&runtime, binding).await,
        }
    }
}

#[async_trait]
impl ScheduleTargetDelivery for RpcScheduleTargetAdapter {
    async fn deliver_occurrence(
        &self,
        occurrence: &Occurrence,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;
        match &occurrence.target_snapshot {
            TargetBinding::Session(binding) => {
                let resolved = match self.resolve_session(&runtime, occurrence, binding).await {
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
                        runtime
                            .deliver_scheduled_prompt(
                                &resolved.session_id,
                                occurrence,
                                prompt.clone(),
                                render_metadata.clone(),
                                skill_references.clone(),
                                additional_instructions.clone(),
                                resolved.materialized_session_id,
                            )
                            .await
                    }
                    ScheduledSessionAction::Event {
                        event_type,
                        payload,
                        render_metadata,
                    } => {
                        runtime
                            .deliver_scheduled_event(
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
                self.deliver_mob_binding(&runtime, occurrence, binding)
                    .await
            }
        }
    }
}

impl SessionRuntime {
    pub async fn ensure_schedule_host_started(self: &Arc<Self>) -> Result<(), ScheduleDomainError> {
        let kind = self.schedule_service.store().kind();
        if matches!(kind, ScheduleStoreKind::Disabled | ScheduleStoreKind::Jsonl) {
            return Ok(());
        }

        let mut slot = self.schedule_host.lock().await;
        if slot.is_some() {
            return Ok(());
        }

        let adapter = Arc::new(RpcScheduleTargetAdapter::new(self));
        let driver = Arc::new(ScheduleDriver::new(
            self.schedule_service(),
            self.schedule_service.store(),
            adapter.clone(),
            adapter,
            self.schedule_owner_id(),
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

        *slot = Some(ScheduleHostHandle {
            shutdown_tx: Some(shutdown_tx),
            join,
        });
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
        if let Some(output_schema_json) = create.output_schema_json.clone() {
            build_config.output_schema = Some(
                OutputSchema::from_json_value(output_schema_json)
                    .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?,
            );
        }
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
        if build_config.config_generation.is_none() {
            build_config.config_generation = if let Some(runtime) = &self.config_runtime {
                runtime.get().await.ok().map(|snapshot| snapshot.generation)
            } else {
                create.config_generation
            };
        }
        if build_config.llm_client_override.is_none()
            && let Some(default_llm_client) = self.default_llm_client.clone()
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
        prompt: ContentInput,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        skill_references: Vec<String>,
        additional_instructions: Vec<String>,
        materialized_session_id: Option<SessionId>,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        match self
            .accept_scheduled_prompt_with_completion(
                session_id,
                occurrence,
                prompt,
                render_metadata,
                skill_references,
                additional_instructions,
            )
            .await
        {
            Ok(accepted) => Ok(build_dispatch_from_accepted(
                occurrence,
                accepted,
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
            Ok(accepted) => Ok(build_dispatch_from_accepted(
                occurrence,
                accepted,
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
    ) -> Result<AcceptedScheduledInput, ScheduleDomainError> {
        if self
            .live_session_is_stale(session_id)
            .await
            .map_err(rpc_to_schedule)?
        {
            let _ = self.service.discard_live_session(session_id).await;
            self.runtime_adapter.unregister_session(session_id).await;
        }

        let skill_keys = canonical_skill_keys(&self.skill_identity_registry(), skill_references)?;

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
        maybe_spawn_comms_drain(self, session_id).await;

        let (outcome, handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, Input::Prompt(prompt_input))
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        Ok(accepted_from_outcome(outcome, handle))
    }

    async fn accept_scheduled_event_with_completion(
        self: &Arc<Self>,
        session_id: &SessionId,
        occurrence: &Occurrence,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
    ) -> Result<AcceptedScheduledInput, ScheduleDomainError> {
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
        maybe_spawn_comms_drain(self, session_id).await;

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
        Ok(accepted_from_outcome(outcome, handle))
    }

    fn schedule_owner_id(&self) -> String {
        let realm = self.realm_id.as_deref().unwrap_or("realm");
        let instance = self.instance_id.as_deref().unwrap_or("rpc");
        format!("rpc-scheduler:{realm}:{instance}")
    }
}

async fn maybe_spawn_comms_drain(runtime: &Arc<SessionRuntime>, session_id: &SessionId) {
    #[cfg(feature = "comms")]
    {
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
            .maybe_spawn_comms_drain(session_id, keep_alive, comms_rt)
            .await;
    }
}

fn canonical_skill_keys(
    registry: &meerkat_core::skills::SourceIdentityRegistry,
    skill_references: Vec<String>,
) -> Result<Option<Vec<meerkat_core::skills::SkillKey>>, ScheduleDomainError> {
    if skill_references.is_empty() {
        return Ok(None);
    }
    SkillsParams {
        preload_skills: None,
        skill_refs: Some(
            skill_references
                .into_iter()
                .map(SkillRef::Legacy)
                .collect::<Vec<_>>(),
        ),
        skill_references: None,
    }
    .canonical_skill_keys_with_registry(registry)
    .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))
}

fn accepted_from_outcome(
    outcome: AcceptOutcome,
    handle: Option<CompletionHandle>,
) -> AcceptedScheduledInput {
    match outcome {
        AcceptOutcome::Accepted { input_id, .. } => AcceptedScheduledInput {
            correlation_id: input_id.to_string(),
            handle,
        },
        AcceptOutcome::Deduplicated { existing_id, .. } => AcceptedScheduledInput {
            correlation_id: existing_id.to_string(),
            handle,
        },
        AcceptOutcome::Rejected { reason } => AcceptedScheduledInput {
            correlation_id: "rejected".into(),
            handle: Some(CompletionHandle::already_resolved(
                CompletionOutcome::Abandoned(reason),
            )),
        },
        _ => AcceptedScheduledInput {
            correlation_id: "rejected".into(),
            handle: Some(CompletionHandle::already_resolved(
                CompletionOutcome::Abandoned("unsupported runtime accept outcome".into()),
            )),
        },
    }
}

fn build_dispatch_from_accepted(
    occurrence: &Occurrence,
    accepted: AcceptedScheduledInput,
    materialized_session_id: Option<SessionId>,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchAccepted,
    );
    receipt.correlation_id = Some(accepted.correlation_id.clone());
    receipt.materialized_session_id = materialized_session_id.clone();

    let occurrence_id = occurrence.occurrence_id.clone();
    let attempt_count = occurrence.attempt_count;
    let correlation_id = accepted.correlation_id.clone();
    let completion_materialized_session_id = materialized_session_id.clone();
    let completed_receipt_materialized_session_id = materialized_session_id.clone();
    let completion = match accepted.handle {
        Some(handle) => scheduled_completion_future(handle, completion_materialized_session_id),
        None => Box::pin(async move {
            Ok(DeliveryTerminal::completed(Some({
                let mut receipt = DeliveryReceipt::new(
                    occurrence_id,
                    attempt_count,
                    DeliveryReceiptStage::Completed,
                );
                receipt.correlation_id = Some(correlation_id);
                receipt.materialized_session_id = completed_receipt_materialized_session_id;
                receipt
            })))
        }),
    };

    DeliveryDispatch {
        receipt,
        correlation_id: Some(accepted.correlation_id),
        materialized_session_id,
        completion,
    }
}

fn immediate_completed_dispatch(
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

fn async_mob_completion_dispatch(
    occurrence: &Occurrence,
    correlation_id: Option<String>,
    completion: meerkat::DeliveryCompletion,
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

fn scheduled_completion_future(
    handle: CompletionHandle,
    materialized_session_id: Option<SessionId>,
) -> meerkat::DeliveryCompletion {
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
            CompletionOutcome::CallbackPending { tool_name, .. } => DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some(format!(
                    "scheduled delivery is waiting for unsupported external tool results: {tool_name}"
                )),
                failure_class: Some(OccurrenceFailureClass::RuntimeRejected),
                materialized_session_id,
            },
            CompletionOutcome::Abandoned(reason) => DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some(reason),
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

fn immediate_delivery_failure(
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

fn schedule_attempt_idempotency_key(occurrence: &Occurrence) -> String {
    format!(
        "schedule:{}:occurrence:{}:attempt:{}",
        occurrence.schedule_id, occurrence.occurrence_id, occurrence.attempt_count
    )
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
    options.profile_name = spec.profile_name.clone().map(Into::into);
    options.runtime_mode = spec.runtime_mode.map(|mode| match mode {
        ScheduledMobRuntimeMode::AutonomousHost => meerkat_mob::MobRuntimeMode::AutonomousHost,
        ScheduledMobRuntimeMode::TurnDriven => meerkat_mob::MobRuntimeMode::TurnDriven,
    });
    options.backend = spec.backend.map(|backend| match backend {
        ScheduledMobBackendKind::Session => MobBackendKind::Session,
        ScheduledMobBackendKind::External => MobBackendKind::External,
    });
    options.tool_access_policy = spec
        .tool_access_policy
        .as_ref()
        .map(|value| serde_json::from_value::<ToolAccessPolicy>(value.clone()))
        .transpose()
        .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;
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
                Ok(Some(_)) | Ok(None) => sleep(Duration::from_millis(100)).await,
                Err(error) => return Ok(mob_delivery_failed_terminal(error)),
            }
        }
    })
}
