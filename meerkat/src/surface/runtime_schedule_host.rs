use std::sync::Arc;

use async_trait::async_trait;

#[cfg(feature = "comms")]
use super::configure_peer_ingress;
use super::{
    NoopScheduleMobHost, ScheduledPromptDispatch, SharedScheduleTargetAdapter,
    SurfaceScheduleMobHost, SurfaceScheduleSessionHost, default_persistent_executor,
    materialize_session, schedule_host_supported, spawn_schedule_host,
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
    config: Config,
    schedule_service: ScheduleService,
    build_template: SessionBuildOptions,
    owner_id: impl Into<String>,
) -> Option<super::ScheduleHostHandle> {
    let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
        "scheduled mob targets are not supported by this runtime host",
    ));
    spawn_runtime_backed_schedule_host_with_mobs(
        service,
        runtime_adapter,
        config,
        schedule_service,
        build_template,
        mob_host,
        owner_id,
    )
}

/// Spawn a runtime-backed schedule host with an explicit mob delivery adapter.
///
/// Embedders that maintain mob state should pass a real
/// [`SurfaceScheduleMobHost`] here, for example the `meerkat-mob-mcp`
/// schedule host adapter. The default [`spawn_runtime_backed_schedule_host`]
/// remains session-only and reports explicit mob target failures.
pub fn spawn_runtime_backed_schedule_host_with_mobs(
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: Arc<MeerkatMachine>,
    _config: Config,
    schedule_service: ScheduleService,
    build_template: SessionBuildOptions,
    mob_host: Arc<dyn SurfaceScheduleMobHost>,
    owner_id: impl Into<String>,
) -> Option<super::ScheduleHostHandle> {
    if !schedule_host_supported(schedule_service.store().kind()) {
        return None;
    }

    let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(
        RuntimeBackedScheduleSessionHost::new(service, runtime_adapter, build_template),
    );
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

fn materialized_build_options(
    template: &SessionBuildOptions,
    create: &SessionMaterializationSpec,
) -> SessionBuildOptions {
    let mut build = template.clone();
    build.provider = None;
    build.output_schema = create.output_schema.clone();
    build.structured_output_retries = create.structured_output_retries;
    build.comms_name = create.comms_name.clone();
    build.peer_meta = create.peer_meta.clone();
    build.provider_params = None;
    build.preload_skills = None;
    build.realm_id = create.realm_id.clone();
    build.instance_id = create.instance_id.clone();
    build.backend = create.backend.clone();
    build.config_generation = create.config_generation;
    build.keep_alive = false;
    build.app_context = create.app_context.clone();
    build.additional_instructions = None;
    build.initial_turn_metadata = create
        .initial_turn_metadata()
        .or(build.initial_turn_metadata);
    build
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
        Ok(())
    }

    fn build_materialized_request(
        &self,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<CreateSessionRequest, ScheduleDomainError> {
        let model = create
            .require_model_name()
            .map_err(ScheduleDomainError::InvalidSchedule)?
            .to_string();
        let build = materialized_build_options(&self.build_template, create);

        Ok(CreateSessionRequest {
            model,
            prompt: ContentInput::Text(String::new()),
            system_prompt: prompt_system_prompt
                .map(str::to_owned)
                .or_else(|| create.system_prompt.clone()),
            max_tokens: create.max_tokens,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: Some(create.labels.clone()),
        })
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
        let request = self.build_materialized_request(create, prompt_system_prompt)?;
        let keep_alive = create.requests_keep_alive();
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

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::lifecycle::run_primitive::{
        KeepAliveMode, KeepAlivePolicy, ModelId, RuntimeTurnMetadata, TurnMetadataOverride,
    };
    use meerkat_core::skills::{SkillKey, SkillName, SourceUuid};
    use std::time::Duration;

    fn fixture_skill_key(name: &str) -> SkillKey {
        let skill_name = match SkillName::parse(name) {
            Ok(skill_name) => skill_name,
            Err(err) => unreachable!("static skill fixture is invalid: {err}"),
        };
        SkillKey::new(SourceUuid::builtin(), skill_name)
    }

    #[test]
    fn materialized_build_options_forwards_preload_skill_keys_through_metadata() {
        let key = fixture_skill_key("email");
        let create = SessionMaterializationSpec {
            turn_metadata: RuntimeTurnMetadata {
                model: Some(ModelId::new("claude-sonnet-4-6")),
                skill_references: Some(vec![key.clone()]),
                ..Default::default()
            },
            system_prompt: None,
            max_tokens: None,
            output_schema: None,
            structured_output_retries: 0,
            comms_name: None,
            peer_meta: None,
            labels: Default::default(),
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            app_context: None,
        };

        let build = materialized_build_options(&SessionBuildOptions::default(), &create);

        assert_eq!(build.preload_skills, None);
        assert_eq!(
            build
                .initial_turn_metadata
                .and_then(|metadata| metadata.skill_references),
            Some(vec![key])
        );
    }

    #[test]
    fn materialized_keep_alive_comes_from_canonical_turn_metadata() {
        let create = SessionMaterializationSpec {
            turn_metadata: RuntimeTurnMetadata {
                model: Some(ModelId::new("claude-sonnet-4-6")),
                keep_alive: Some(TurnMetadataOverride::Set(KeepAlivePolicy {
                    ttl: Duration::from_secs(30),
                    policy: KeepAliveMode::Pinned,
                })),
                ..Default::default()
            },
            comms_name: Some("scheduled-agent".to_string()),
            ..Default::default()
        };

        let build = materialized_build_options(&SessionBuildOptions::default(), &create);

        assert!(!build.keep_alive);
        assert!(create.requests_keep_alive());
    }

    #[test]
    fn runtime_registration_does_not_reapply_split_keep_alive_default() {
        let source = include_str!("runtime_schedule_host.rs");
        let start = source
            .find("    async fn ensure_runtime_session_registered")
            .expect("registration helper should exist");
        let end = source
            .find("    fn build_materialized_request")
            .expect("materialized request helper should follow registration");
        let body = &source[start..end];

        assert!(
            !body.contains("update_peer_ingress_context"),
            "runtime schedule registration must not overwrite canonical keep_alive with a split default"
        );
        assert!(
            !body.contains("let keep_alive = false"),
            "runtime schedule registration must not synthesize split keep_alive defaults"
        );
    }
}
