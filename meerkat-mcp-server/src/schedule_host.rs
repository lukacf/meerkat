use std::sync::Arc;
#[cfg(feature = "mob")]
use std::time::Duration;

use async_trait::async_trait;
#[cfg(feature = "mob")]
use meerkat::DeliveryTerminal;
#[cfg(not(feature = "mob"))]
use meerkat::surface::immediate_delivery_failure;
use meerkat::surface::prepare_surface_session;
use meerkat::surface::{
    ScheduledPromptDispatch, SharedScheduleTargetAdapter, SurfaceScheduleMobHost,
    SurfaceScheduleSessionHost, schedule_host_supported, spawn_schedule_host,
};
#[cfg(feature = "mob")]
use meerkat::surface::{
    async_completion_dispatch, immediate_completed_dispatch, immediate_delivery_failure,
};
use meerkat::{
    DeliveryDispatch, MobTargetBinding, Occurrence, OccurrenceFailureClass, ScheduleDomainError,
    SessionMaterializationSpec, SessionService, SessionTargetBinding, TargetProbeOutcome,
};
#[cfg(feature = "mob")]
use meerkat::{
    ForkContextSpec, HelperOptionsSpec, ScheduledMobBackendKind, ScheduledMobRuntimeMode,
};
use meerkat_core::agent::AgentToolDispatcher;
use meerkat_core::handles::ExternalToolSurfaceHandle;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
};
use meerkat_core::types::HandlingMode;
use meerkat_core::{ContentInput, Session, SessionId};
use meerkat_mcp::{McpRouter, McpRouterAdapter};
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

use crate::{
    MeerkatMcpState, canonical_skill_keys, compose_external_tool_dispatchers, runtime_ingress,
};

fn materialized_preload_skills(
    preload_skills: &[meerkat_core::skills::SkillKey],
) -> Option<Vec<meerkat_core::skills::SkillKey>> {
    (!preload_skills.is_empty()).then(|| preload_skills.to_vec())
}

#[derive(Clone)]
struct McpScheduleContext {
    service: Arc<meerkat::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    config_runtime: Arc<meerkat_core::ConfigRuntime>,
    realm_id: meerkat_core::connection::RealmId,
    instance_id: Option<String>,
    backend: String,
    mcp_adapters: runtime_ingress::SharedMcpAdapters,
    runtime_sessions: runtime_ingress::SharedMcpRuntimeSessions,
    #[cfg(feature = "mob")]
    mob_state: Arc<MobMcpState>,
}

impl McpScheduleContext {
    fn from_state(state: &MeerkatMcpState) -> Self {
        Self {
            service: Arc::clone(&state.service),
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
        // `service.read()` is the single authoritative liveness+presence
        // check post-wave-c. Archived and genuinely-missing sessions
        // both fail to read — both map to "session not found" here,
        // which matches the scheduler contract.
        if self.service.read(session_id).await.is_err() {
            return Err(ScheduleDomainError::InvalidSchedule(format!(
                "session not found: {session_id}"
            )));
        }

        let ingress = self.ingress_context();
        let callback_tools = ingress.current_callback_tools(session_id).await;
        ingress
            .configure_session(session_id, callback_tools, false)
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

        // `service.read()` is the single authoritative liveness+presence
        // check post-wave-c. Archived sessions and genuinely-missing
        // sessions both fail to read; the scheduler treats both as
        // `Missing`.
        match self.service.read(session_id).await {
            Ok(view) if view.state.is_active => Ok(TargetProbeOutcome::Busy {
                detail: Some(format!("session still running: {session_id}")),
            }),
            Ok(_) => Ok(TargetProbeOutcome::Ready),
            Err(_) => Ok(TargetProbeOutcome::Missing {
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
        let runtime_bindings = prepared.bindings;

        let output_schema = create.output_schema.clone();

        let mcp_adapter = self
            .seed_realm_mcp_adapter(Arc::clone(&runtime_bindings.external_tool_surface))
            .await?;
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
            self_hosted_server_id: None,
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
            override_builtins: meerkat_core::ToolCategoryOverride::Inherit,
            override_shell: meerkat_core::ToolCategoryOverride::Inherit,
            override_memory: meerkat_core::ToolCategoryOverride::Inherit,
            override_schedule: meerkat_core::ToolCategoryOverride::Inherit,
            override_mob: meerkat_core::ToolCategoryOverride::Inherit,
            schedule_tools: None,
            mob_tool_authority_context: None,
            preload_skills: materialized_preload_skills(&create.preload_skills),
            realm_id: create
                .realm_id
                .clone()
                .or_else(|| Some(self.realm_id.to_string())),
            instance_id: create
                .instance_id
                .clone()
                .or_else(|| self.instance_id.clone()),
            backend: create
                .backend
                .clone()
                .or_else(|| Some(self.backend.clone())),
            config_generation: current_generation,
            connection_ref: None,
            keep_alive: create.keep_alive,
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            app_context: create.app_context.clone(),
            additional_instructions: (!create.additional_instructions.is_empty())
                .then(|| create.additional_instructions.clone()),
            shell_env: None,
            resume_override_mask: Default::default(),
            blob_store_override: None,
            mob_tools: None,
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(runtime_bindings),
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
            self.ingress_context()
                .configure_session(&session_id, None, false)
                .await;
            update_peer_ingress_context(self, &session_id).await;
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

    async fn seed_realm_mcp_adapter(
        &self,
        external_tool_surface: Arc<dyn ExternalToolSurfaceHandle>,
    ) -> Result<Arc<McpRouterAdapter>, ScheduleDomainError> {
        let adapter = Arc::new(McpRouterAdapter::new(McpRouter::new_with_surface_handle(
            external_tool_surface,
        )));
        let server_configs = self
            .config_runtime
            .get()
            .await
            .ok()
            .map(|snapshot| snapshot.config.tools.mcp_servers)
            .unwrap_or_default();

        for config in &server_configs {
            adapter
                .stage_add(config.clone())
                .await
                .map_err(ScheduleDomainError::Internal)?;
        }
        if !server_configs.is_empty() {
            adapter
                .apply_staged()
                .await
                .map_err(ScheduleDomainError::Internal)?;
        }

        Ok(adapter)
    }
}

struct McpScheduleTargetAdapter {
    context: McpScheduleContext,
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
                        detail: Some(format!("mob source member not found: {source_member_id}")),
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
        }
    }
}

fn scheduled_materialization_preload_skills(
    create: &SessionMaterializationSpec,
) -> Option<Vec<meerkat_core::skills::SkillKey>> {
    (!create.preload_skills.is_empty()).then(|| create.preload_skills.clone())
}

#[async_trait]
impl SurfaceScheduleSessionHost for McpScheduleTargetAdapter {
    async fn probe_session_target(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        self.context.session_target_probe(binding).await
    }

    async fn materialize_session(
        &self,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError> {
        self.context
            .materialize_scheduled_session(create, prompt_system_prompt)
            .await
    }

    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        Box::pin(deliver_scheduled_prompt(
            &self.context,
            session_id,
            occurrence,
            dispatch,
        ))
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
        deliver_scheduled_event(
            &self.context,
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
impl SurfaceScheduleMobHost for McpScheduleTargetAdapter {
    async fn probe_mob_target(
        &self,
        binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        #[cfg(feature = "mob")]
        {
            self.probe_mob_binding(binding).await
        }

        #[cfg(not(feature = "mob"))]
        {
            let _ = binding;
            Ok(TargetProbeOutcome::Missing {
                detail: Some(
                    "scheduled mob targets require the mob feature on the MCP host".into(),
                ),
            })
        }
    }

    async fn deliver_mob_target(
        &self,
        occurrence: &Occurrence,
        binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        #[cfg(feature = "mob")]
        {
            let mob_state = Arc::clone(&self.context.mob_state);
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
                    let identity = AgentIdentity::from(member_id.as_str());
                    let helper_options = helper_options_from_spec(options)?;
                    let prompt = prompt.clone();
                    Ok(async_completion_dispatch(
                        occurrence,
                        Some(member_id.clone()),
                        Box::pin(async move {
                            match mob_state
                                .mob_spawn_helper(&mob_id, identity, prompt, helper_options)
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
            let _ = binding;
            Ok(immediate_delivery_failure(
                occurrence,
                "scheduled mob targets require the mob feature on the MCP host".to_string(),
                OccurrenceFailureClass::MobRejected,
                None,
                None,
            ))
        }
    }
}

impl MeerkatMcpState {
    pub(crate) fn start_schedule_host(&self) -> Result<(), ScheduleDomainError> {
        if !schedule_host_supported(self.schedule_service.store().kind()) {
            return Ok(());
        }

        let mut slot = self
            .schedule_host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_some() {
            return Ok(());
        }

        let surface_adapter = Arc::new(McpScheduleTargetAdapter::new(
            McpScheduleContext::from_state(self),
        ));
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = surface_adapter.clone();
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = surface_adapter;
        let shared_adapter = Arc::new(SharedScheduleTargetAdapter::new(
            self.schedule_service.clone(),
            session_host,
            mob_host,
        ));
        *slot = Some(spawn_schedule_host(
            self.schedule_service.clone(),
            shared_adapter,
            self.schedule_owner_id(),
        ));
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
            && let Some(handle) = slot.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                handle.shutdown().await;
            });
        }
    }
}

async fn deliver_scheduled_prompt(
    _context: &McpScheduleContext,
    _session_id: &SessionId,
    _occurrence: &Occurrence,
    _dispatch: ScheduledPromptDispatch,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    Err(ScheduleDomainError::Internal(
        "mcp-server deliver_prompt no longer reinterprets runtime terminal classes into schedule-local failure classes; the schedule surface must consume the runtime's typed CompletionOutcome directly".to_string(),
    ))
}

async fn deliver_scheduled_event(
    _context: &McpScheduleContext,
    _session_id: &SessionId,
    _occurrence: &Occurrence,
    _event_type: String,
    _payload: serde_json::Value,
    _render_metadata: Option<meerkat_core::types::RenderMetadata>,
    _materialized_session_id: Option<SessionId>,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    Err(ScheduleDomainError::Internal(
        "mcp-server deliver_event no longer reinterprets runtime terminal classes into schedule-local failure classes; the schedule surface must consume the runtime's typed CompletionOutcome directly".to_string(),
    ))
}

async fn update_peer_ingress_context(_context: &McpScheduleContext, _session_id: &SessionId) {
    #[cfg(feature = "comms")]
    {
        // Post-wave-c the raw `load_persisted` escape hatch is gone.
        // `SessionInfo` (returned by `service.read()`) does not surface
        // `keep_alive`, so we default to `false` — matches the
        // pre-retype `.unwrap_or(false)` fallback. Sessions that need
        // comms-driven keep-alive configure it through the canonical
        // `SessionBuildOptions.keep_alive` on create.
        let keep_alive = false;
        let comms_rt = _context.service.comms_runtime(_session_id).await;
        _context
            .runtime_adapter
            .update_peer_ingress_context(_session_id, keep_alive, comms_rt)
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

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::skills::{SkillKey, SkillName, SourceUuid};

    fn fixture_skill_key(name: &str) -> SkillKey {
        let skill_name = SkillName::parse(name).expect("fixture skill name should be valid");
        SkillKey::new(SourceUuid::builtin(), skill_name)
    }

    #[test]
    fn materialized_preload_skills_preserves_typed_skill_keys() {
        let key = fixture_skill_key("email");

        assert_eq!(
            materialized_preload_skills(std::slice::from_ref(&key)),
            Some(vec![key])
        );
    }

    #[test]
    fn materialized_preload_skills_leaves_empty_preload_unset() {
        let preload_skills: Vec<SkillKey> = Vec::new();

        assert_eq!(materialized_preload_skills(&preload_skills), None);
    }
}
