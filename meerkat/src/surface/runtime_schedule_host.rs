use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;

#[cfg(feature = "comms")]
use super::configure_peer_ingress;
use super::{
    NoopScheduleMobHost, ScheduledPromptDispatch, SharedScheduleTargetAdapter,
    SurfaceScheduleSessionHost, accepted_scheduled_input_from_runtime_outcome,
    build_dispatch_from_accepted, default_persistent_executor, immediate_delivery_failure,
    materialize_session, schedule_attempt_idempotency_key, schedule_host_supported,
    spawn_schedule_host,
};
use crate::{
    Config, CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService,
    ScheduleDomainError, ScheduleService, Session, SessionMaterializationSpec, SessionService,
    SessionTargetBinding, TargetProbeOutcome,
};
use meerkat_contracts::SkillsParams;
use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions};
use meerkat_core::types::{ContentInput, SessionId};
use meerkat_runtime::{
    CorrelationId, IdempotencyKey, Input, InputDurability, InputHeader, InputOrigin,
    InputVisibility, MeerkatMachine, PromptInput,
};

pub fn spawn_runtime_backed_schedule_host(
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: Arc<MeerkatMachine>,
    config: Config,
    schedule_service: ScheduleService,
    build_template: SessionBuildOptions,
    owner_id: impl Into<String>,
) -> Option<super::ScheduleHostHandle> {
    if !schedule_host_supported(schedule_service.store().kind()) {
        return None;
    }

    let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(
        RuntimeBackedScheduleSessionHost::new(service, runtime_adapter, config, build_template),
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
    config: Config,
    build_template: SessionBuildOptions,
}

impl RuntimeBackedScheduleSessionHost {
    fn new(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        runtime_adapter: Arc<MeerkatMachine>,
        config: Config,
        build_template: SessionBuildOptions,
    ) -> Self {
        Self {
            service,
            runtime_adapter,
            config,
            build_template,
        }
    }

    async fn ensure_runtime_session_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        let persisted = self
            .service
            .load_persisted(session_id)
            .await
            .map_err(schedule_internal)?;
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
            let keep_alive = self
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
        build.preload_skills = (!create.preload_skills.is_empty()).then(|| {
            create
                .preload_skills
                .iter()
                .cloned()
                .map(Into::into)
                .collect()
        });
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

    fn canonical_skill_keys(
        &self,
        skill_references: Vec<String>,
    ) -> Result<Option<Vec<meerkat_core::skills::SkillKey>>, ScheduleDomainError> {
        if skill_references.is_empty() {
            return Ok(None);
        }

        let registry = self
            .config
            .skills
            .build_source_identity_registry()
            .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;

        SkillsParams {
            preload_skills: None,
            skill_refs: None,
            skill_references: Some(skill_references),
        }
        .canonical_skill_keys_with_registry(&registry)
        .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))
    }

    async fn accept_scheduled_prompt_with_completion(
        &self,
        session_id: &SessionId,
        occurrence: &crate::Occurrence,
        prompt: ContentInput,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        skill_references: Vec<String>,
        additional_instructions: Vec<String>,
    ) -> Result<super::AcceptedScheduledInput, ScheduleDomainError> {
        self.ensure_runtime_session_registered(session_id).await?;
        let skill_keys = self.canonical_skill_keys(skill_references)?;

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

        let (outcome, handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, Input::Prompt(prompt_input))
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        Ok(accepted_scheduled_input_from_runtime_outcome(
            outcome, handle,
        ))
    }

    async fn accept_scheduled_event_with_completion(
        &self,
        session_id: &SessionId,
        occurrence: &crate::Occurrence,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
    ) -> Result<super::AcceptedScheduledInput, ScheduleDomainError> {
        self.ensure_runtime_session_registered(session_id).await?;

        let input = Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: Utc::now(),
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
        Ok(accepted_scheduled_input_from_runtime_outcome(
            outcome, handle,
        ))
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
            .map_err(schedule_internal)?;
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
        let request = self.build_materialized_request(create, prompt_system_prompt);
        let keep_alive = request.build.as_ref().is_some_and(|build| build.keep_alive);
        let result = materialize_session(
            &self.service,
            &self.runtime_adapter,
            Session::new(),
            request,
            {
                let service = Arc::clone(&self.service);
                let runtime_adapter = Arc::clone(&self.runtime_adapter);
                move |session_id| default_persistent_executor(service, runtime_adapter, session_id)
            },
        )
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
        occurrence: &crate::Occurrence,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<crate::DeliveryDispatch, ScheduleDomainError> {
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
            Ok(accepted) => Ok(build_dispatch_from_accepted(
                occurrence,
                accepted,
                dispatch.materialized_session_id,
            )),
            Err(error) => Ok(immediate_delivery_failure(
                occurrence,
                error.to_string(),
                crate::OccurrenceFailureClass::RuntimeRejected,
                None,
                dispatch.materialized_session_id,
            )),
        }
    }

    async fn deliver_event(
        &self,
        session_id: &SessionId,
        occurrence: &crate::Occurrence,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        materialized_session_id: Option<SessionId>,
    ) -> Result<crate::DeliveryDispatch, ScheduleDomainError> {
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
                crate::OccurrenceFailureClass::RuntimeRejected,
                None,
                materialized_session_id,
            )),
        }
    }
}

fn schedule_internal(error: impl std::fmt::Display) -> ScheduleDomainError {
    ScheduleDomainError::Internal(error.to_string())
}

fn session_metadata_marks_archived(session: &Session) -> bool {
    session
        .metadata()
        .get("session_archived")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
