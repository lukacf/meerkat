use crate::{
    MeerkatMcpState, canonical_skill_keys, compose_external_tool_dispatchers, preload_skill_ids,
    runtime_ingress,
};
use async_trait::async_trait;
use chrono::Duration as ChronoDuration;
use meerkat::surface::prepare_surface_session;
use meerkat::{
    DeliveryDispatch, DeliveryReceipt, DeliveryReceiptStage, DeliveryTerminal, MobTargetBinding,
    Occurrence, OccurrenceFailureClass, OccurrencePhase, ScheduleDomainError, ScheduleDriver,
    ScheduleDriverConfig, ScheduleService, ScheduleStoreKind, ScheduleTargetDelivery,
    ScheduleTargetProbe, ScheduledSessionAction, SessionMaterializationSpec, SessionTargetBinding,
    TargetBinding, TargetProbeOutcome,
};
use meerkat_core::agent::AgentToolDispatcher;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, ResumeOverrideMask,
    SessionBuildOptions, SessionService,
};
use meerkat_core::{ContentInput, OutputSchema, Session, SessionId, types::HandlingMode};
use meerkat_mcp::{McpRouter, McpRouterAdapter};
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
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, oneshot};
#[cfg(feature = "mob")]
use tokio::time::sleep;
#[cfg(feature = "mob")]
use {
    meerkat::ForkContextSpec, meerkat::HelperOptionsSpec, meerkat::ScheduledMobBackendKind,
    meerkat::ScheduledMobRuntimeMode, meerkat_core::ops::ToolAccessPolicy,
};

pub(crate) struct ScheduleHostHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: tokio::task::JoinHandle<()>,
}

impl ScheduleHostHandle {
    fn shutdown(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.join.abort();
    }
}

#[derive(Clone)]
struct McpScheduleContext {
    service: Arc<meerkat::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
    schedule_service: ScheduleService,
    runtime_adapter: Arc<meerkat_runtime::RuntimeSessionAdapter>,
    config_runtime: Arc<meerkat_core::ConfigRuntime>,
    realm_id: String,
    instance_id: Option<String>,
    backend: String,
    mcp_adapters: Arc<Mutex<HashMap<String, Arc<McpRouterAdapter>>>>,
    runtime_sessions: runtime_ingress::SharedMcpRuntimeSessions,
    #[cfg(feature = "mob")]
    mob_state: Arc<MobMcpState>,
}

impl McpScheduleContext {
    fn from_state(state: &MeerkatMcpState) -> Self {
        Self {
            service: Arc::clone(&state.service),
            schedule_service: state.schedule_service.clone(),
            runtime_adapter: Arc::clone(&state.runtime_adapter),
            config_runtime: Arc::clone(&state.config_runtime),
            realm_id: state.realm_id.clone(),
            instance_id: state.instance_id.clone(),
            backend: state.backend.clone(),
            mcp_adapters: Arc::clone(&state.mcp_adapters),
            runtime_sessions: Arc::clone(&state.runtime_sessions),
            #[cfg(feature = "mob")]
            mob_state: Arc::clone(&state.mob_state),
        }
    }

    fn ingress_context(&self) -> runtime_ingress::McpRuntimeIngressContext {
        runtime_ingress::McpRuntimeIngressContext::new(
            runtime_ingress::McpRuntimeIngressResources {
                service: Arc::clone(&self.service),
                runtime_adapter: Arc::clone(&self.runtime_adapter),
                config_runtime: Arc::clone(&self.config_runtime),
                realm_id: self.realm_id.clone(),
                instance_id: self.instance_id.clone(),
                backend: self.backend.clone(),
                mcp_adapters: Arc::clone(&self.mcp_adapters),
                runtime_sessions: Arc::clone(&self.runtime_sessions),
            },
        )
    }

    async fn ensure_runtime_session_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        let persisted = self
            .service
            .load_persisted(session_id)
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        if persisted
            .as_ref()
            .is_some_and(session_metadata_marks_archived)
        {
            return Err(ScheduleDomainError::InvalidSchedule(format!(
                "session not found: {session_id}"
            )));
        }

        let session_exists = if persisted.is_some() {
            true
        } else {
            self.service.read(session_id).await.is_ok()
        };
        if !session_exists {
            return Err(ScheduleDomainError::InvalidSchedule(format!(
                "session not found: {session_id}"
            )));
        }
        self.ingress_context()
            .configure_session(session_id, None, false)
            .await;
        Ok(())
    }

    async fn session_target_probe(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let Some(session_id) = binding.resolved_session_id() else {
            return Ok(TargetProbeOutcome::Ready);
        };

        if let Ok(view) = self.service.read(session_id).await {
            return Ok(if view.state.is_active {
                TargetProbeOutcome::Busy {
                    detail: Some(format!("session still running: {session_id}")),
                }
            } else {
                TargetProbeOutcome::Ready
            });
        }

        let persisted = self
            .service
            .load_persisted(session_id)
            .await
            .map_err(|error| ScheduleDomainError::ProbeFailed(error.to_string()))?;
        match persisted {
            Some(session) if !session_metadata_marks_archived(&session) => {
                Ok(TargetProbeOutcome::Ready)
            }
            _ => Ok(TargetProbeOutcome::Missing {
                detail: Some(format!("session not found: {session_id}")),
            }),
        }
    }

    async fn materialize_scheduled_session(
        &self,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError> {
        let prepared = prepare_surface_session(&self.runtime_adapter)
            .await
            .map_err(ScheduleDomainError::Internal)?;
        let session = prepared.session;
        let session_id = prepared.session_id;
        let ops_lifecycle = prepared.ops_lifecycle;

        let output_schema = create
            .output_schema_json
            .clone()
            .map(OutputSchema::from_json_value)
            .transpose()
            .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;

        let mcp_adapter = Arc::new(McpRouterAdapter::new(McpRouter::new()));
        let mcp_tools: Arc<dyn AgentToolDispatcher> = mcp_adapter.clone();
        let external_tools = compose_external_tool_dispatchers(None, Some(mcp_tools))
            .map_err(ScheduleDomainError::Internal)?;

        let current_generation = self
            .config_runtime
            .get()
            .await
            .ok()
            .map(|snapshot| snapshot.generation)
            .or(create.config_generation);
        let build = SessionBuildOptions {
            provider: create.provider,
            output_schema,
            structured_output_retries: create.structured_output_retries,
            hooks_override: Default::default(),
            comms_name: create.comms_name.clone(),
            peer_meta: create.peer_meta.clone(),
            resume_session: Some(session),
            budget_limits: None,
            provider_params: create.provider_params.clone(),
            call_timeout_override: meerkat_core::CallTimeoutOverride::Inherit,
            external_tools,
            recoverable_tool_defs: None,
            llm_client_override: None,
            ops_lifecycle_override: Some(ops_lifecycle),
            override_builtins: None,
            override_shell: None,
            override_memory: None,
            override_mob: None,
            preload_skills: preload_skill_ids(Some(create.preload_skills.clone())),
            realm_id: create
                .realm_id
                .clone()
                .or_else(|| Some(self.realm_id.clone())),
            instance_id: create
                .instance_id
                .clone()
                .or_else(|| self.instance_id.clone()),
            backend: create
                .backend
                .clone()
                .or_else(|| Some(self.backend.clone())),
            config_generation: current_generation,
            keep_alive: create.keep_alive,
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            app_context: create.app_context.clone(),
            additional_instructions: (!create.additional_instructions.is_empty())
                .then(|| create.additional_instructions.clone()),
            shell_env: None,
            resume_override_mask: ResumeOverrideMask::default(),
            blob_store_override: None,
            mob_tools: None,
        };

        let request = CreateSessionRequest {
            model: create.model.clone(),
            prompt: ContentInput::Text(String::new()),
            render_metadata: None,
            system_prompt: prompt_system_prompt
                .map(str::to_owned)
                .or_else(|| create.system_prompt.clone()),
            max_tokens: create.max_tokens,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: Some(create.labels.clone()),
        };

        let result = self.service.create_session(request).await;
        let session_exists = self.service.read(&session_id).await.is_ok();
        if session_exists {
            self.mcp_adapters
                .lock()
                .await
                .insert(session_id.to_string(), Arc::clone(&mcp_adapter));
            maybe_spawn_comms_drain(self, &session_id).await;
        }

        match result {
            Ok(_) => Ok(session_id),
            Err(error) => {
                if session_exists {
                    let _ = self.service.archive(&session_id).await;
                }
                self.ingress_context().clear_session(&session_id).await;
                Err(ScheduleDomainError::Internal(error.to_string()))
            }
        }
    }
}

struct McpScheduleTargetAdapter {
    context: McpScheduleContext,
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

struct ScheduledPromptDispatch {
    prompt: ContentInput,
    render_metadata: Option<meerkat_core::types::RenderMetadata>,
    skill_references: Vec<String>,
    additional_instructions: Vec<String>,
    materialized_session_id: Option<SessionId>,
}

impl McpScheduleTargetAdapter {
    fn new(context: McpScheduleContext) -> Self {
        Self { context }
    }

    #[cfg(feature = "mob")]
    async fn probe_mob_binding(
        &self,
        binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let mob_id = MobId::from(mob_binding_mob_id(binding));
        let handle = match self.context.mob_state.handle_for(&mob_id).await {
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
        _binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        Ok(TargetProbeOutcome::Missing {
            detail: Some("scheduled mob targets require the mob feature on the MCP host".into()),
        })
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
                    .context
                    .materialize_scheduled_session(create, prompt_system_prompt)
                    .await
                {
                    Ok(session_id) => {
                        if let Err(error) = self
                            .context
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

    #[cfg(feature = "mob")]
    async fn deliver_mob_binding(
        &self,
        occurrence: &Occurrence,
        binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let mob_state = Arc::clone(&self.context.mob_state);
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
            } => match self
                .context
                .mob_state
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
        occurrence: &Occurrence,
        _binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        Ok(immediate_delivery_failure(
            occurrence,
            "scheduled mob targets require the mob feature on the MCP host".to_string(),
            OccurrenceFailureClass::MobRejected,
            None,
            None,
        ))
    }
}

#[async_trait]
impl ScheduleTargetProbe for McpScheduleTargetAdapter {
    async fn probe_target(
        &self,
        occurrence: &Occurrence,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        match &occurrence.target_snapshot {
            TargetBinding::Session(binding) => self.context.session_target_probe(binding).await,
            TargetBinding::Mob(binding) => self.probe_mob_binding(binding).await,
        }
    }
}

#[async_trait]
impl ScheduleTargetDelivery for McpScheduleTargetAdapter {
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
                        deliver_scheduled_prompt(
                            &self.context,
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
                        deliver_scheduled_event(
                            &self.context,
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
            TargetBinding::Mob(binding) => self.deliver_mob_binding(occurrence, binding).await,
        }
    }
}

impl MeerkatMcpState {
    pub(crate) fn start_schedule_host(&self) -> Result<(), ScheduleDomainError> {
        let kind = self.schedule_service.store().kind();
        if matches!(kind, ScheduleStoreKind::Disabled | ScheduleStoreKind::Jsonl) {
            return Ok(());
        }

        let mut slot = self
            .schedule_host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_some() {
            return Ok(());
        }

        let context = McpScheduleContext::from_state(self);
        let adapter = Arc::new(McpScheduleTargetAdapter::new(context.clone()));
        let driver = Arc::new(ScheduleDriver::new(
            context.schedule_service.clone(),
            context.schedule_service.store(),
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

    fn schedule_owner_id(&self) -> String {
        let instance = self.instance_id.as_deref().unwrap_or("mcp");
        format!("mcp-scheduler:{}:{instance}", self.realm_id)
    }
}

impl Drop for MeerkatMcpState {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.schedule_host.lock()
            && let Some(mut handle) = slot.take()
        {
            handle.shutdown();
        }
    }
}

async fn deliver_scheduled_prompt(
    context: &McpScheduleContext,
    session_id: &SessionId,
    occurrence: &Occurrence,
    dispatch: ScheduledPromptDispatch,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    match accept_scheduled_prompt_with_completion(
        context,
        session_id,
        occurrence,
        dispatch.prompt,
        dispatch.render_metadata,
        dispatch.skill_references,
        dispatch.additional_instructions,
    )
    .await
    {
        Ok(accepted) => Ok(build_dispatch_from_accepted(
            occurrence,
            accepted,
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
    context: &McpScheduleContext,
    session_id: &SessionId,
    occurrence: &Occurrence,
    event_type: String,
    payload: serde_json::Value,
    render_metadata: Option<meerkat_core::types::RenderMetadata>,
    materialized_session_id: Option<SessionId>,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    match accept_scheduled_event_with_completion(
        context,
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
    context: &McpScheduleContext,
    session_id: &SessionId,
    occurrence: &Occurrence,
    prompt: ContentInput,
    render_metadata: Option<meerkat_core::types::RenderMetadata>,
    skill_references: Vec<String>,
    additional_instructions: Vec<String>,
) -> Result<AcceptedScheduledInput, ScheduleDomainError> {
    let config = context
        .config_runtime
        .get()
        .await
        .map(|snapshot| snapshot.config)
        .unwrap_or_default();
    let skill_keys = canonical_skill_keys(&config, None, Some(skill_references))
        .map_err(ScheduleDomainError::InvalidSchedule)?;

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
    prompt_input.header.correlation_id = Some(CorrelationId::from_uuid(occurrence.occurrence_id.0));

    context
        .ensure_runtime_session_registered(session_id)
        .await?;
    maybe_spawn_comms_drain(context, session_id).await;

    let (outcome, handle) = context
        .ingress_context()
        .accept_input_with_completion(session_id, Input::Prompt(prompt_input), None)
        .await
        .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
    Ok(accepted_from_outcome(outcome, handle))
}

async fn accept_scheduled_event_with_completion(
    context: &McpScheduleContext,
    session_id: &SessionId,
    occurrence: &Occurrence,
    event_type: String,
    payload: serde_json::Value,
    render_metadata: Option<meerkat_core::types::RenderMetadata>,
) -> Result<AcceptedScheduledInput, ScheduleDomainError> {
    context
        .ensure_runtime_session_registered(session_id)
        .await?;
    maybe_spawn_comms_drain(context, session_id).await;

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
        handling_mode: HandlingMode::Queue,
        render_metadata,
    });

    let (outcome, handle) = context
        .ingress_context()
        .accept_input_with_completion(session_id, input, None)
        .await
        .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
    Ok(accepted_from_outcome(outcome, handle))
}

async fn maybe_spawn_comms_drain(context: &McpScheduleContext, session_id: &SessionId) {
    #[cfg(feature = "comms")]
    {
        let keep_alive = context
            .service
            .load_persisted(session_id)
            .await
            .ok()
            .flatten()
            .and_then(|session| {
                session
                    .session_metadata()
                    .map(|metadata| metadata.keep_alive)
            })
            .unwrap_or(false);
        let comms_rt = context.service.comms_runtime(session_id).await;
        context
            .runtime_adapter
            .maybe_spawn_comms_drain(session_id, keep_alive, comms_rt)
            .await;
    }
}

fn session_metadata_marks_archived(session: &Session) -> bool {
    session
        .metadata()
        .get("session_archived")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
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

#[cfg(feature = "mob")]
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

#[cfg(feature = "mob")]
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
                Ok(Some(_) | None) => sleep(Duration::from_millis(100)).await,
                Err(error) => return Ok(mob_delivery_failed_terminal(error)),
            }
        }
    })
}
