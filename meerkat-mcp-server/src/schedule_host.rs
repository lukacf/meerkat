use std::sync::Arc;

use async_trait::async_trait;
#[cfg(not(feature = "mob"))]
use meerkat::surface::NoopScheduleMobHost;
use meerkat::surface::{
    ScheduledPromptDispatch, SharedScheduleTargetAdapter, SurfaceScheduleMobHost,
    SurfaceScheduleSessionHost, recover_mob_member_identity_from_session_target,
    schedule_host_supported, spawn_schedule_host,
};
use meerkat::{
    DeliveryDispatch, IdentityTargetBinding, Occurrence, ScheduleDomainError, Session,
    SessionMaterializationSpec, SessionService, SessionTargetBinding, TargetProbeOutcome,
};
use meerkat_core::agent::AgentToolDispatcher;
use meerkat_core::handles::ExternalToolSurfaceHandle;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
};
use meerkat_core::types::HandlingMode;
use meerkat_core::{ContentInput, SessionId};
use meerkat_mcp::{McpRouter, McpRouterAdapter};
#[cfg(feature = "mob")]
use meerkat_mob_mcp::MobMcpScheduleHost;
use meerkat_runtime::{
    CorrelationId, IdempotencyKey, InputDurability, InputHeader, InputOrigin, InputVisibility,
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
    sidecars: runtime_ingress::SharedMcpSidecars,
    runtime_registration_locks: runtime_ingress::SharedMcpRuntimeRegistrationLocks,
    workgraph_service: meerkat::WorkGraphService,
    #[cfg(test)]
    test_llm_client_override: Option<Arc<dyn meerkat_client::LlmClient>>,
    #[cfg(test)]
    test_resume_override_mask: meerkat_core::service::ResumeOverrideMask,
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
            sidecars: Arc::clone(&state.sidecars),
            runtime_registration_locks: Arc::clone(&state.runtime_registration_locks),
            workgraph_service: state.workgraph_service.clone(),
            #[cfg(test)]
            test_llm_client_override: None,
            #[cfg(test)]
            test_resume_override_mask: Default::default(),
        }
    }

    #[cfg(test)]
    fn with_deterministic_test_client(mut self) -> Self {
        self.test_llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        self
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
                sidecars: Arc::clone(&self.sidecars),
                runtime_registration_locks: Arc::clone(&self.runtime_registration_locks),
                workgraph_service: self.workgraph_service.clone(),
            },
        )
    }

    async fn session_target_probe(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let Some(session_id) = binding.resolved_session_id() else {
            return Ok(TargetProbeOutcome::Ready);
        };

        // `service.read()` is the authoritative liveness+presence check.
        // Only a typed NotFound can become scheduler Missing; other read
        // failures are authority failures and must not become lifecycle facts.
        match self.service.read(session_id).await {
            Ok(view) if view.state.is_active => Ok(TargetProbeOutcome::Busy {
                detail: Some(format!("session still running: {session_id}")),
            }),
            Ok(_) => Ok(TargetProbeOutcome::Ready),
            Err(meerkat_core::service::SessionError::NotFound { .. }) => {
                Ok(TargetProbeOutcome::Missing {
                    detail: Some(format!("session not found: {session_id}")),
                })
            }
            Err(error) => Err(ScheduleDomainError::Internal(format!(
                "failed to read session target {session_id}: {error}"
            ))),
        }
    }

    async fn ensure_session_target_exists(
        &self,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        match self.service.read(session_id).await {
            Ok(_) => Ok(()),
            Err(meerkat_core::service::SessionError::NotFound { .. }) => Err(
                ScheduleDomainError::InvalidSchedule(format!("session not found: {session_id}")),
            ),
            Err(error) => Err(ScheduleDomainError::Internal(format!(
                "failed to read session target {session_id}: {error}"
            ))),
        }
    }

    async fn materialize_scheduled_session(
        &self,
        occurrence: &Occurrence,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError> {
        // Layer A: derive the materialized SessionId deterministically from the
        // occurrence identity (NOT attempt_count) so a redrive of the same
        // occurrence reuses the existing exact attachment instead of minting a
        // second actor if a prior attempt died inside the materialize->bind
        // window.
        let session = Session::with_id(occurrence.materialized_session_id());
        let session_id = session.id().clone();
        let ingress = self.ingress_context();
        let authoritative_session = self
            .service
            .load_authoritative_session(&session_id)
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        let preexisting_authoritative = authoritative_session.is_some();
        let session = authoritative_session.clone().unwrap_or(session);
        let authoritative_archived =
            if let Some(authoritative_session) = authoritative_session.as_ref() {
                self.service
                    .session_archived_by_authority(&session_id, authoritative_session)
                    .await
                    .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?
            } else {
                false
            };
        if authoritative_archived {
            return Err(ScheduleDomainError::InvalidSchedule(format!(
                "materialized session is archived: {session_id}"
            )));
        }
        let existing_router = ingress.logical_router(&session_id).await;
        if preexisting_authoritative {
            if existing_router.is_some() {
                match ingress.ensure_session(&session_id).await {
                    Ok(_) => {
                        update_peer_ingress_context(self, &session_id).await?;
                        return Ok(session_id);
                    }
                    Err(meerkat_core::service::SessionError::FailedWithData { data, .. })
                        if data
                            .get("session_resume_required")
                            .and_then(serde_json::Value::as_bool)
                            == Some(true) =>
                    {
                        // Durable scheduling is a trusted autonomous owner.
                        // It may wake the persisted target, but only through
                        // the same exact resume seam as `meerkat_resume`.
                    }
                    Err(error) => {
                        return Err(ScheduleDomainError::Internal(format!(
                            "failed to inspect scheduled session {session_id}: {error}"
                        )));
                    }
                }
            }
        } else if existing_router.is_some()
            || ingress
                .current_attachment_witness(&session_id)
                .await
                .is_some()
            || self
                .runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await
                .is_some()
        {
            return Err(ScheduleDomainError::Internal(format!(
                "new scheduled materialization {session_id} found orphaned process-local runtime state; explicit meerkat_resume or exact cleanup is required"
            )));
        }

        // Schedule specs carry the realm as a plain slug string; validate it
        // before opening the prepared-materialization transaction.
        let realm_id = create
            .realm_id
            .as_deref()
            .map(meerkat_core::connection::RealmId::parse)
            .transpose()
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?
            .or_else(|| Some(self.realm_id.clone()));

        let create_admission = self
            .service
            .reserve_create_session_admission()
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        let mode = if preexisting_authoritative {
            crate::McpActorMaterializationMode::Resume
        } else {
            crate::McpActorMaterializationMode::Fresh
        };
        let materialization = crate::materialize_mcp_actor_transaction(
            &self.service,
            &self.runtime_adapter,
            &ingress,
            &session_id,
            mode,
            Some(create_admission),
            |runtime_bindings, exact_config| async move {
                let mcp_adapter = match exact_config.router.clone() {
                    Some(adapter) => adapter,
                    None => self
                        .seed_realm_mcp_adapter(Arc::clone(
                            runtime_bindings.external_tool_surface(),
                        ))
                        .await
                        .map_err(|error| crate::ToolCallError::internal(error.to_string()))?,
                };
                let mcp_tools: Arc<dyn AgentToolDispatcher> = mcp_adapter.clone();
                let external_tools =
                    compose_external_tool_dispatchers(exact_config.callback_tools, Some(mcp_tools))
                        .map_err(crate::ToolCallError::internal)?;

                let current_generation = self
                    .config_runtime
                    .get()
                    .await
                    .ok()
                    .map(|snapshot| snapshot.generation)
                    .or(create.config_generation);
                let build = SessionBuildOptions {
                    custom_models: std::collections::BTreeMap::new(),
                    image_generation_provider: None,
                    auto_compact_threshold_override: None,
                    provider: create.provider,
                    override_comms: Default::default(),
                    self_hosted_server_id: None,
                    output_schema: create.output_schema.clone(),
                    structured_output_retries: create.structured_output_retries,
                    hooks_override: Default::default(),
                    comms_name: create.comms_name.clone(),
                    peer_meta: create.peer_meta.clone(),
                    resume_session: Some(session),
                    budget_limits: None,
                    provider_params: create.provider_params.clone(),
                    call_timeout_override: meerkat_core::CallTimeoutOverride::Inherit,
                    external_tools,
                    mcp_servers: Vec::new(),
                    recoverable_tool_defs: None,
                    llm_client_override: {
                        #[cfg(test)]
                        {
                            self.test_llm_client_override
                                .clone()
                                .map(meerkat::encode_llm_client_override_for_service)
                        }
                        #[cfg(not(test))]
                        {
                            None
                        }
                    },
                    session_comms_runtime_override: None,
                    host_prompt_sections: Default::default(),
                    agent_llm_client_decorator: None,
                    override_builtins: meerkat_core::ToolCategoryOverride::Inherit,
                    override_shell: meerkat_core::ToolCategoryOverride::Inherit,
                    override_memory: meerkat_core::ToolCategoryOverride::Inherit,
                    override_schedule: meerkat_core::ToolCategoryOverride::Inherit,
                    override_workgraph: meerkat_core::ToolCategoryOverride::Inherit,
                    override_mob: meerkat_core::ToolCategoryOverride::Inherit,
                    override_image_generation: meerkat_core::ToolCategoryOverride::Inherit,
                    override_web_search: meerkat_core::ToolCategoryOverride::Inherit,
                    schedule_tools: None,
                    workgraph_tools: None,
                    mob_tool_authority_context: None,
                    preload_skills: materialized_preload_skills(&create.preload_skills),
                    realm_id,
                    instance_id: create
                        .instance_id
                        .clone()
                        .or_else(|| self.instance_id.clone()),
                    backend: create
                        .backend
                        .as_deref()
                        .and_then(meerkat_core::RecoveryBackendKind::parse)
                        .or_else(|| meerkat_core::RecoveryBackendKind::parse(&self.backend)),
                    config_generation: current_generation,
                    auth_binding: None,
                    mob_member_binding: None,
                    keep_alive: create.keep_alive,
                    checkpointer: None,
                    silent_comms_intents: Vec::new(),
                    max_inline_peer_notifications: None,
                    app_context: create.app_context.clone(),
                    additional_instructions: (!create.additional_instructions.is_empty())
                        .then(|| create.additional_instructions.clone()),
                    initial_metadata_entries: std::collections::BTreeMap::new(),
                    initial_tool_filter: None,
                    tool_access_policy: None,
                    shell_env: None,
                    resume_override_mask: {
                        #[cfg(test)]
                        {
                            self.test_resume_override_mask
                        }
                        #[cfg(not(test))]
                        {
                            Default::default()
                        }
                    },
                    blob_store_override: None,
                    mob_tools: None,
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(
                        runtime_bindings,
                    ),
                    initial_turn_metadata: None,
                };
                let request = CreateSessionRequest {
                    injected_context: Vec::new(),
                    model: create.model.clone(),
                    prompt: ContentInput::Text(String::new()),
                    system_prompt: match prompt_system_prompt
                        .map(str::to_owned)
                        .or_else(|| create.system_prompt.clone())
                    {
                        Some(prompt) => meerkat::SystemPromptOverride::Set(prompt),
                        None => meerkat::SystemPromptOverride::Inherit,
                    },
                    max_tokens: create.max_tokens,
                    event_tx: None,
                    initial_turn: InitialTurnPolicy::Defer,
                    deferred_prompt_policy: DeferredPromptPolicy::Discard,
                    build: Some(build),
                    labels: Some(create.labels.clone()),
                };
                Ok(crate::McpActorMaterializationPlan {
                    request,
                    callback_config: runtime_ingress::McpCallbackConfig::Preserve,
                    router_to_publish: exact_config
                        .router
                        .is_none()
                        .then(|| Arc::clone(&mcp_adapter)),
                })
            },
        )
        .await
        .map_err(|error| ScheduleDomainError::Internal(error.message))?;

        if materialization.committed {
            update_peer_ingress_context(self, &session_id).await?;
        }
        materialization.result.map_err(|error| {
            ScheduleDomainError::Internal(format!(
                "scheduled materialization failed for {session_id}: {error}"
            ))
        })?;
        Ok(session_id)
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

    async fn recover_session_target_identity(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<Option<IdentityTargetBinding>, ScheduleDomainError> {
        let Some(session_id) = binding.resolved_session_id() else {
            return Ok(None);
        };
        let session = self
            .context
            .service
            .load_authoritative_session(session_id)
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        Ok(recover_mob_member_identity_from_session_target(
            binding,
            session.as_ref(),
        ))
    }

    async fn materialize_session(
        &self,
        occurrence: &Occurrence,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError> {
        self.context
            .materialize_scheduled_session(occurrence, create, prompt_system_prompt)
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

async fn update_peer_ingress_context(
    _context: &McpScheduleContext,
    _session_id: &SessionId,
) -> Result<(), ScheduleDomainError> {
    #[cfg(feature = "comms")]
    {
        let session = _context
            .service
            .load_authoritative_session(_session_id)
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ScheduleDomainError::InvalidSchedule(format!("session not found: {_session_id}"))
            })?;
        let keep_alive = session
            .session_metadata()
            .ok_or_else(|| {
                ScheduleDomainError::Internal(format!(
                    "session {_session_id} is missing session metadata"
                ))
            })?
            .keep_alive;
        _context
            .ingress_context()
            .configure_peer_ingress_exact(_session_id, keep_alive)
            .await
            .map_err(|error| {
                ScheduleDomainError::Internal(format!(
                    "failed to update exact peer ingress for {_session_id}: {error}"
                ))
            })?;
    }
    #[cfg(not(feature = "comms"))]
    let _ = (_context, _session_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::skills::{SkillKey, SkillName, SourceUuid};

    fn sample_occurrence() -> Occurrence {
        let schedule = meerkat::Schedule::new(meerkat::CreateScheduleRequest {
            name: Some("mcp-schedule-sidecar-test".to_string()),
            description: None,
            trigger: meerkat::TriggerSpec::Once {
                due_at_utc: chrono::Utc::now(),
            },
            target: meerkat::TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id: SessionId::new(),
                action: meerkat::ScheduledSessionAction::Prompt {
                    prompt: ContentInput::Text("scheduled".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }),
            misfire_policy: meerkat::MisfirePolicy::Skip,
            overlap_policy: meerkat::OverlapPolicy::SkipIfRunning,
            missing_target_policy: meerkat::MissingTargetPolicy::Skip,
            labels: Default::default(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("schedule fixture should be valid");
        Occurrence::planned_from_schedule(
            &schedule,
            meerkat::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("occurrence fixture should be valid")
    }

    fn materialization_spec(model: &str) -> SessionMaterializationSpec {
        SessionMaterializationSpec {
            model: model.to_string(),
            system_prompt: None,
            max_tokens: Some(128),
            provider: None,
            output_schema: None,
            structured_output_retries: None,
            provider_params: None,
            comms_name: None,
            peer_meta: None,
            labels: Default::default(),
            preload_skills: Vec::new(),
            additional_instructions: Vec::new(),
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            keep_alive: false,
            app_context: None,
        }
    }

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

    #[tokio::test]
    async fn scheduled_actor_missing_redrive_uses_shared_resume_seam() {
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let context = McpScheduleContext::from_state(&state).with_deterministic_test_client();
        let occurrence = sample_occurrence();
        let create = materialization_spec("mock-scheduled-materialization");

        let first = context
            .materialize_scheduled_session(&occurrence, &create, None)
            .await
            .expect("first scheduled materialization should succeed");
        let ingress = context.ingress_context();
        let first_router = ingress
            .logical_router(&first)
            .await
            .expect("first materialization publishes a logical router");
        let first_witness = ingress
            .current_attachment_witness(&first)
            .await
            .expect("first materialization publishes an exact attachment");
        assert_eq!(
            context
                .materialize_scheduled_session(&occurrence, &create, None)
                .await
                .expect("live exact redrive should reuse its existing incarnation"),
            first,
        );
        context
            .service
            .discard_live_session(&first)
            .await
            .expect("open actor-missing exact-sidecar redrive window");

        let redriven = context
            .materialize_scheduled_session(&occurrence, &create, None)
            .await
            .expect("durable scheduling may wake its target through the shared resume seam");
        assert_eq!(redriven, first);
        assert!(Arc::ptr_eq(
            &first_router,
            &ingress
                .logical_router(&first)
                .await
                .expect("autonomous resume preserves logical config"),
        ));
        let replacement_witness = ingress
            .current_attachment_witness(&first)
            .await
            .expect("autonomous resume publishes an exact replacement attachment");
        assert_ne!(
            replacement_witness, first_witness,
            "redrive must mint a new exact incarnation rather than reuse stale attachment A"
        );
        assert!(
            context
                .service
                .has_live_session(&first)
                .await
                .expect("live actor lookup"),
            "the shared resume seam reconstructs the missing actor"
        );
    }

    #[tokio::test]
    async fn cold_scheduled_redrive_recovers_and_preserves_authoritative_transcript() {
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let context = McpScheduleContext::from_state(&state).with_deterministic_test_client();
        let occurrence = sample_occurrence();
        let create = materialization_spec("mock-scheduled-materialization");
        let session_id = context
            .materialize_scheduled_session(&occurrence, &create, None)
            .await
            .expect("first scheduled materialization should succeed");
        let ingress = context.ingress_context();
        context
            .service
            .append_external_user_content(
                &session_id,
                ContentInput::Text("durable redrive marker".to_string()),
            )
            .await
            .expect("non-LLM marker append should persist");
        let before = context
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load authoritative transcript before redrive")
            .expect("scheduled session remains authoritative")
            .messages()
            .to_vec();
        assert!(
            !before.is_empty(),
            "marker prompt must persist transcript state"
        );

        ingress
            .clear_session(&session_id)
            .await
            .expect("open cold redrive window");
        let redriven = context
            .materialize_scheduled_session(&occurrence, &create, None)
            .await
            .expect("durable scheduling may reconstruct a cold target through shared resume");
        assert_eq!(redriven, session_id);
        let after = context
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load authoritative transcript after redrive")
            .expect("redriven session remains authoritative")
            .messages()
            .to_vec();
        assert_eq!(before, after, "cold redrive must preserve transcript state");
        assert!(ingress.logical_router(&session_id).await.is_some());
        assert!(
            ingress
                .current_attachment_witness(&session_id)
                .await
                .is_some()
        );
    }
}
