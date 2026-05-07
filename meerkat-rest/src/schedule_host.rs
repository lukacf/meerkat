use std::sync::Arc;

use async_trait::async_trait;
#[cfg(not(feature = "mob"))]
use meerkat::surface::NoopScheduleMobHost;
use meerkat::surface::{
    ScheduledPromptDispatch, SharedScheduleTargetAdapter, SurfaceScheduleMobHost,
    SurfaceScheduleSessionHost, schedule_host_supported, spawn_schedule_host,
};
use meerkat::{
    AgentBuildConfig, DeliveryDispatch, Occurrence, ScheduleDomainError,
    SessionMaterializationSpec, SessionService, SessionTargetBinding, TargetProbeOutcome,
};
use meerkat_contracts::SkillsParams;
use meerkat_core::service::{
    CreateSessionRequest as SvcCreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy,
};
use meerkat_core::{ContentInput, SessionId};
#[cfg(feature = "mob")]
use meerkat_mob_mcp::MobMcpScheduleHost;
use meerkat_runtime::{
    CorrelationId, IdempotencyKey, Input, InputDurability, InputHeader, InputOrigin,
    InputVisibility, PromptInput,
};
use tokio::sync::Mutex;

use crate::{AppState, RestRuntimeExecutorContext, RestSessionRuntimeExecutor};

fn materialized_preload_skills(
    preload_skills: &[meerkat_core::skills::SkillKey],
) -> Option<Vec<meerkat_core::skills::SkillKey>> {
    (!preload_skills.is_empty()).then(|| preload_skills.to_vec())
}

#[derive(Default)]
pub struct ScheduleHostState {
    inner: Mutex<Option<meerkat::surface::ScheduleHostHandle>>,
}

#[derive(Clone)]
struct RestScheduleContext {
    runtime: RestRuntimeExecutorContext,
}

impl RestScheduleContext {
    fn from_state(state: &AppState) -> Self {
        Self {
            runtime: state.runtime_executor_context(),
        }
    }

    async fn ensure_runtime_session_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        // `service.read()` is the single authoritative liveness+presence
        // check post-wave-c. Archived and genuinely-missing sessions both
        // fail to read — both map to "session not found".
        if self.runtime.session_service.read(session_id).await.is_err() {
            return Err(ScheduleDomainError::InvalidSchedule(format!(
                "session not found: {session_id}"
            )));
        }

        let executor = Box::new(RestSessionRuntimeExecutor::new(
            self.runtime.clone(),
            session_id.clone(),
        ));
        self.runtime
            .runtime_adapter
            .ensure_session_with_executor(session_id.clone(), executor)
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
        match self.runtime.session_service.read(session_id).await {
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
            .runtime
            .session_service
            .reserve_create_session_admission()
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        let prepared = meerkat::surface::prepare_surface_session(&self.runtime.runtime_adapter)
            .await
            .map_err(ScheduleDomainError::Internal)?;
        let session_id = prepared.session_id;
        let pre_session = prepared.session;
        let runtime_bindings = prepared.bindings;

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
        build_config.preload_skills = materialized_preload_skills(&create.preload_skills);
        build_config.additional_instructions = (!create.additional_instructions.is_empty())
            .then(|| create.additional_instructions.clone());
        build_config.realm_id = create
            .realm_id
            .clone()
            .or_else(|| Some(self.runtime.realm.to_string()));
        build_config.instance_id = create
            .instance_id
            .clone()
            .or_else(|| self.runtime.instance_id.clone());
        build_config.backend = create
            .backend
            .clone()
            .or_else(|| Some(self.runtime.backend.clone()));
        build_config.keep_alive = create.keep_alive;
        build_config.app_context = create.app_context.clone();
        build_config.resume_session = Some(pre_session);
        build_config.runtime_build_mode =
            meerkat_core::RuntimeBuildMode::SessionOwned(runtime_bindings);
        build_config.config_generation = self
            .runtime
            .config_runtime
            .get()
            .await
            .ok()
            .map(|snapshot| snapshot.generation)
            .or(create.config_generation);
        if build_config.llm_client_override.is_none()
            && let Some(default_llm_client) = self.runtime.llm_client_override.clone()
        {
            build_config.llm_client_override = Some(default_llm_client);
        }

        let create_req = SvcCreateSessionRequest {
            model: build_config.model.clone(),
            prompt: ContentInput::Text(String::new()),
            render_metadata: None,
            system_prompt: build_config.system_prompt.clone(),
            max_tokens: build_config.max_tokens,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build_config.to_session_build_options()),
            labels: Some(create.labels.clone()),
        };
        let result = match self
            .runtime
            .session_service
            .create_session_with_reserved_admission(create_req, create_admission)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                self.runtime
                    .runtime_adapter
                    .unregister_session(&session_id)
                    .await;
                return Err(ScheduleDomainError::Internal(error.to_string()));
            }
        };
        Ok(result.session_id)
    }
}

struct RestScheduleTargetAdapter {
    context: RestScheduleContext,
}

impl RestScheduleTargetAdapter {
    fn new(context: RestScheduleContext) -> Self {
        Self { context }
    }
}

fn scheduled_materialization_preload_skills(
    create: &SessionMaterializationSpec,
) -> Option<Vec<meerkat_core::skills::SkillKey>> {
    (!create.preload_skills.is_empty()).then(|| create.preload_skills.clone())
}

#[async_trait]
impl SurfaceScheduleSessionHost for RestScheduleTargetAdapter {
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
        deliver_scheduled_prompt(&self.context, session_id, occurrence, dispatch).await
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

impl AppState {
    pub async fn ensure_schedule_host_started(&self) -> Result<(), ScheduleDomainError> {
        if !schedule_host_supported(self.schedule_service.store().kind()) {
            return Ok(());
        }

        let mut slot = self.schedule_host.inner.lock().await;
        if slot.is_some() {
            return Ok(());
        }

        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(
            RestScheduleTargetAdapter::new(RestScheduleContext::from_state(self)),
        );
        let mob_host = rest_schedule_mob_host(self);
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

    pub async fn shutdown_schedule_host(&self) {
        let mut slot = self.schedule_host.inner.lock().await;
        let Some(handle) = slot.take() else {
            return;
        };
        drop(slot);
        handle.shutdown().await;
    }

    fn schedule_owner_id(&self) -> String {
        let instance = self.instance_id.as_deref().unwrap_or("rest");
        format!("rest-scheduler:{}:{instance}", self.realm)
    }
}

fn rest_schedule_mob_host(state: &AppState) -> Arc<dyn SurfaceScheduleMobHost> {
    #[cfg(feature = "mob")]
    {
        Arc::new(MobMcpScheduleHost::new(Arc::clone(&state.mob_state)))
    }

    #[cfg(not(feature = "mob"))]
    {
        let _ = state;
        Arc::new(NoopScheduleMobHost::new(
            "scheduled mob targets require the mob feature on the REST host",
        ))
    }
}

async fn deliver_scheduled_prompt(
    _context: &RestScheduleContext,
    _session_id: &SessionId,
    _occurrence: &Occurrence,
    _dispatch: ScheduledPromptDispatch,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    Err(ScheduleDomainError::Internal(
        "rest deliver_prompt no longer reinterprets runtime terminal classes into schedule-local failure classes; the schedule surface must consume the runtime's typed CompletionOutcome directly".to_string(),
    ))
}

async fn deliver_scheduled_event(
    _context: &RestScheduleContext,
    _session_id: &SessionId,
    _occurrence: &Occurrence,
    _event_type: String,
    _payload: serde_json::Value,
    _render_metadata: Option<meerkat_core::types::RenderMetadata>,
    _materialized_session_id: Option<SessionId>,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    Err(ScheduleDomainError::Internal(
        "rest deliver_event no longer reinterprets runtime terminal classes into schedule-local failure classes; the schedule surface must consume the runtime's typed CompletionOutcome directly".to_string(),
    ))
}

#[cfg(feature = "comms")]
async fn update_peer_ingress_context(context: &RestScheduleContext, session_id: &SessionId) {
    // Post-wave-c the raw `load_persisted` escape hatch is gone.
    // `SessionInfo` (returned by `service.read()`) does not surface
    // `keep_alive`, so we default to `false` — matches the pre-retype
    // `.unwrap_or(false)` fallback. Sessions that need comms-driven
    // keep-alive configure it through the canonical
    // `SessionBuildOptions.keep_alive` on create.
    let keep_alive = false;
    let comms_rt = context
        .runtime
        .session_service
        .comms_runtime(session_id)
        .await;
    context
        .runtime
        .runtime_adapter
        .update_peer_ingress_context(session_id, keep_alive, comms_rt)
        .await;
}

#[cfg(not(feature = "comms"))]
async fn update_peer_ingress_context(_context: &RestScheduleContext, _session_id: &SessionId) {}

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
