use std::sync::Arc;

use async_trait::async_trait;
#[cfg(not(feature = "mob"))]
use meerkat::surface::NoopScheduleMobHost;
use meerkat::surface::prepare_surface_session;
use meerkat::surface::{
    ScheduledPromptDispatch, SharedScheduleTargetAdapter, SurfaceScheduleMobHost,
    SurfaceScheduleSessionHost, schedule_host_supported, spawn_schedule_host,
};
use meerkat::{
    DeliveryDispatch, Occurrence, ScheduleDomainError, SessionMaterializationSpec, SessionService,
    SessionTargetBinding, TargetProbeOutcome,
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
use meerkat_mob_mcp::MobMcpScheduleHost;
use meerkat_runtime::{
    CorrelationId, IdempotencyKey, Input, InputDurability, InputHeader, InputOrigin,
    InputVisibility, PromptInput,
};

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
    runtime_pre_admissions: runtime_ingress::SharedMcpRuntimePreAdmissions,
    runtime_registration_locks: runtime_ingress::SharedMcpRuntimeRegistrationLocks,
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
            runtime_pre_admissions: Arc::clone(&state.runtime_pre_admissions),
            runtime_registration_locks: Arc::clone(&state.runtime_registration_locks),
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
                runtime_pre_admissions: Arc::clone(&self.runtime_pre_admissions),
                runtime_registration_locks: Arc::clone(&self.runtime_registration_locks),
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
        let create_admission = self
            .service
            .reserve_create_session_admission()
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        let prepared = prepare_surface_session(&self.runtime_adapter)
            .await
            .map_err(ScheduleDomainError::Internal)?;
        let session = prepared.session;
        let session_id = prepared.session_id;
        let runtime_bindings = prepared.bindings;

        let output_schema = create.output_schema.clone();

        let mcp_adapter = match self
            .seed_realm_mcp_adapter(Arc::clone(runtime_bindings.external_tool_surface()))
            .await
        {
            Ok(adapter) => adapter,
            Err(error) => {
                self.ingress_context().clear_session(&session_id).await;
                return Err(error);
            }
        };
        let mcp_tools: Arc<dyn AgentToolDispatcher> = mcp_adapter.clone();
        let external_tools = match compose_external_tool_dispatchers(None, Some(mcp_tools)) {
            Ok(tools) => tools,
            Err(error) => {
                self.ingress_context().clear_session(&session_id).await;
                return Err(ScheduleDomainError::Internal(error));
            }
        };

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
            auth_binding: None,
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
            initial_turn_metadata: None,
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

        let result = self
            .service
            .create_session_with_reserved_admission(request, create_admission)
            .await;
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

        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(
            McpScheduleTargetAdapter::new(McpScheduleContext::from_state(self)),
        );
        let mob_host = mcp_schedule_mob_host(self);
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

fn mcp_schedule_mob_host(state: &MeerkatMcpState) -> Arc<dyn SurfaceScheduleMobHost> {
    #[cfg(feature = "mob")]
    {
        Arc::new(MobMcpScheduleHost::new(Arc::clone(&state.mob_state)))
    }

    #[cfg(not(feature = "mob"))]
    {
        let _ = state;
        Arc::new(NoopScheduleMobHost::new(
            "scheduled mob targets require the mob feature on the MCP host",
        ))
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
