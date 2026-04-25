use std::sync::Arc;

use async_trait::async_trait;

#[cfg(feature = "comms")]
use super::configure_peer_ingress;
use super::{
    NoopScheduleMobHost, ScheduledPromptDispatch, SharedScheduleTargetAdapter,
    SurfaceScheduleSessionHost, default_persistent_executor, materialize_session,
    schedule_host_supported, spawn_schedule_host,
};
use crate::{
    Config, CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService,
    ScheduleDomainError, ScheduleService, Session, SessionMaterializationSpec, SessionService,
    SessionTargetBinding, TargetProbeOutcome,
};
use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions};
use meerkat_core::types::{ContentInput, SessionId};
use meerkat_runtime::MeerkatMachine;

pub fn spawn_runtime_backed_schedule_host(
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: Arc<MeerkatMachine>,
    _config: Config,
    schedule_service: ScheduleService,
    build_template: SessionBuildOptions,
    owner_id: impl Into<String>,
) -> Option<super::ScheduleHostHandle> {
    if !schedule_host_supported(schedule_service.store().kind()) {
        return None;
    }

    let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(
        RuntimeBackedScheduleSessionHost::new(service, runtime_adapter, build_template),
    );
    let mob_host = Arc::new(NoopScheduleMobHost::new(
        "scheduled mob targets are not supported by this runtime host",
    ));
    let adapter = Arc::new(SharedScheduleTargetAdapter::new(
        schedule_service.clone(),
        session_host,
        mob_host,
    ));
    Some(spawn_schedule_host(schedule_service, adapter, owner_id))
}

struct RuntimeBackedScheduleSessionHost {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: Arc<MeerkatMachine>,
    build_template: SessionBuildOptions,
}

impl RuntimeBackedScheduleSessionHost {
    fn new(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        runtime_adapter: Arc<MeerkatMachine>,
        build_template: SessionBuildOptions,
    ) -> Self {
        Self {
            service,
            runtime_adapter,
            build_template,
        }
    }

    async fn ensure_runtime_session_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        // `service.read()` is the single authoritative liveness check.
        // It returns `Ok` only for sessions that are materialized in
        // the service (present and not archived). Archived and
        // genuinely-missing sessions both fail to read — both map to
        // "session not found" here, which matches the scheduler
        // contract.
        if self.service.read(session_id).await.is_err() {
            return Err(ScheduleDomainError::InvalidSchedule(format!(
                "session not found: {session_id}"
            )));
        }

        self.runtime_adapter
            .ensure_session_with_executor(
                session_id.clone(),
                default_persistent_executor(
                    Arc::clone(&self.service),
                    Arc::clone(&self.runtime_adapter),
                    session_id.clone(),
                ),
            )
            .await;
        self.update_peer_ingress_context(session_id).await;
        Ok(())
    }

    async fn update_peer_ingress_context(&self, session_id: &SessionId) {
        #[cfg(feature = "comms")]
        {
            // Post-wave-c the raw `load_persisted` escape hatch is
            // gone. `SessionInfo` (returned by `service.read()`) does
            // not currently surface `keep_alive`, so we default to
            // `false` — matches the pre-retype `.unwrap_or(false)`
            // fallback. Sessions that legitimately need
            // comms-driven keep-alive configure it through the
            // canonical `SessionBuildOptions.keep_alive` on create.
            let keep_alive = false;
            configure_peer_ingress(&self.runtime_adapter, &self.service, session_id, keep_alive)
                .await;
        }
        #[cfg(not(feature = "comms"))]
        let _ = session_id;
    }

    fn build_materialized_request(
        &self,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> CreateSessionRequest {
        let mut build = self.build_template.clone();
        build.provider = create.provider;
        build.output_schema = create.output_schema.clone();
        build.structured_output_retries = create.structured_output_retries;
        build.comms_name = create.comms_name.clone();
        build.peer_meta = create.peer_meta.clone();
        build.provider_params = create.provider_params.clone();
        // Schedule wire type `SessionMaterializationSpec.preload_skills: Vec<String>`
        // carries only slug halves — no lossless projection to the typed
        // `SkillKey` (source_uuid + skill_name) required by the session
        // build. Callers use `skill_refs` / `skill_references` for
        // typed per-turn skill injection.
        build.preload_skills = None;
        build.realm_id = create.realm_id.clone();
        build.instance_id = create.instance_id.clone();
        build.backend = create.backend.clone();
        build.config_generation = create.config_generation;
        build.keep_alive = create.keep_alive;
        build.app_context = create.app_context.clone();
        build.additional_instructions = (!create.additional_instructions.is_empty())
            .then(|| create.additional_instructions.clone())
            .or(build.additional_instructions);

        CreateSessionRequest {
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
        }
    }
}

#[async_trait]
impl SurfaceScheduleSessionHost for RuntimeBackedScheduleSessionHost {
    async fn probe_session_target(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let Some(session_id) = binding.resolved_session_id() else {
            return Ok(TargetProbeOutcome::Ready);
        };

        // `service.read()` is the single authoritative liveness+presence
        // check post-wave-c. Archived sessions and genuinely-missing
        // sessions both fail to read; the scheduler treats both as
        // `Missing`, which matches the pre-retype archive-metadata
        // outcome for this probe.
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

    async fn materialize_session(
        &self,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError> {
        let request = self.build_materialized_request(create, prompt_system_prompt);
        let keep_alive = request.build.as_ref().is_some_and(|build| build.keep_alive);
        let result = Box::pin(materialize_session(
            &self.service,
            &self.runtime_adapter,
            Session::new(),
            request,
            {
                let service = Arc::clone(&self.service);
                let runtime_adapter = Arc::clone(&self.runtime_adapter);
                move |session_id| default_persistent_executor(service, runtime_adapter, session_id)
            },
        ))
        .await
        .map_err(schedule_internal)?;
        #[cfg(feature = "comms")]
        configure_peer_ingress(
            &self.runtime_adapter,
            &self.service,
            &result.session_id,
            keep_alive,
        )
        .await;
        #[cfg(not(feature = "comms"))]
        let _ = keep_alive;
        Ok(result.session_id)
    }

    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        _occurrence: &crate::Occurrence,
        _dispatch: ScheduledPromptDispatch,
    ) -> Result<crate::DeliveryDispatch, ScheduleDomainError> {
        self.ensure_runtime_session_registered(session_id).await?;
        Err(ScheduleDomainError::Internal(
            "runtime-backed deliver_prompt no longer reinterprets runtime terminal classes into schedule-local failure classes; the schedule surface must consume the runtime's typed CompletionOutcome directly".to_string(),
        ))
    }

    async fn deliver_event(
        &self,
        session_id: &SessionId,
        _occurrence: &crate::Occurrence,
        _event_type: String,
        _payload: serde_json::Value,
        _render_metadata: Option<meerkat_core::types::RenderMetadata>,
        _materialized_session_id: Option<SessionId>,
    ) -> Result<crate::DeliveryDispatch, ScheduleDomainError> {
        self.ensure_runtime_session_registered(session_id).await?;
        Err(ScheduleDomainError::Internal(
            "runtime-backed deliver_event no longer reinterprets runtime terminal classes into schedule-local failure classes; the schedule surface must consume the runtime's typed CompletionOutcome directly".to_string(),
        ))
    }
}

fn schedule_internal(error: impl std::fmt::Display) -> ScheduleDomainError {
    ScheduleDomainError::Internal(error.to_string())
}
