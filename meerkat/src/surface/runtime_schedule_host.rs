use std::sync::Arc;

use async_trait::async_trait;

#[cfg(feature = "comms")]
use super::configure_peer_ingress;
use super::{
    AcceptedScheduledInput, NoopScheduleMobHost, ScheduledPromptDispatch,
    SharedScheduleTargetAdapter, SurfaceScheduleMobHost, SurfaceScheduleSessionHost,
    build_dispatch_from_accepted, default_persistent_executor, immediate_delivery_failure,
    materialize_session, schedule_attempt_idempotency_key, schedule_host_supported,
    spawn_schedule_host,
};
use crate::{
    Config, CreateSessionRequest, PersistentSessionService, ScheduleDomainError, ScheduleService,
    Session, SessionAgentBuilder, SessionMaterializationSpec, SessionService, SessionTargetBinding,
    TargetProbeOutcome,
};
use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions};
use meerkat_core::types::{ContentInput, SessionId};
use meerkat_runtime::MeerkatMachine;
use meerkat_schedule::DeliveryFailureReason;

pub fn spawn_runtime_backed_schedule_host<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
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
pub fn spawn_runtime_backed_schedule_host_with_mobs<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
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

struct RuntimeBackedScheduleSessionHost<B: SessionAgentBuilder> {
    service: Arc<PersistentSessionService<B>>,
    runtime_adapter: Arc<MeerkatMachine>,
    build_template: SessionBuildOptions,
}

fn materialized_build_options(
    template: &SessionBuildOptions,
    create: &SessionMaterializationSpec,
) -> Result<SessionBuildOptions, ScheduleDomainError> {
    let mut build = template.clone();
    build.provider = create.provider;
    build.output_schema = create.output_schema.clone();
    build.structured_output_retries = create.structured_output_retries;
    build.comms_name = create.comms_name.clone();
    build.peer_meta = create.peer_meta.clone();
    build.provider_params = create.provider_params.clone();
    if !create.preload_skills.is_empty() {
        build.preload_skills = Some(create.preload_skills.clone());
    }
    // Schedule specs carry the realm as a plain slug string; parse it once at
    // this surface ingest boundary into the typed carrier. A malformed slug
    // fails closed (matching the CLI/MCP/REST/RPC schedule-host boundaries) —
    // it must NOT silently materialize the session with no realm, which would
    // collapse the credential/storage isolation boundary to env-default.
    build.realm_id = create
        .realm_id
        .as_deref()
        .map(meerkat_core::RealmId::parse)
        .transpose()
        .map_err(schedule_internal)?;
    build.instance_id = create.instance_id.clone();
    build.backend = create
        .backend
        .as_deref()
        .and_then(meerkat_core::RecoveryBackendKind::parse);
    build.config_generation = create.config_generation;
    build.keep_alive = create.keep_alive;
    build.app_context = create.app_context.clone();
    build.additional_instructions = (!create.additional_instructions.is_empty())
        .then(|| create.additional_instructions.clone())
        .or(build.additional_instructions);
    Ok(build)
}

fn accepted_scheduled_input_from_runtime_handle(
    correlation_id: Option<String>,
    handle: Option<meerkat_runtime::CompletionHandle>,
) -> AcceptedScheduledInput {
    match handle {
        Some(handle) => AcceptedScheduledInput::with_runtime_handle(correlation_id, handle),
        None => AcceptedScheduledInput::with_authority_unavailable(
            correlation_id,
            "runtime completion handle missing after accepted dispatch",
        ),
    }
}

fn runtime_delivery_dispatch(
    occurrence: &crate::Occurrence,
    outcome: meerkat_runtime::accept::AcceptOutcome,
    handle: Option<meerkat_runtime::CompletionHandle>,
    materialized_session_id: Option<SessionId>,
) -> Result<crate::DeliveryDispatch, ScheduleDomainError> {
    match outcome {
        meerkat_runtime::accept::AcceptOutcome::Accepted { input_id, .. } => {
            let accepted =
                accepted_scheduled_input_from_runtime_handle(Some(input_id.to_string()), handle);
            Ok(build_dispatch_from_accepted(
                occurrence,
                accepted,
                materialized_session_id,
            ))
        }
        meerkat_runtime::accept::AcceptOutcome::Deduplicated { existing_id, .. } => {
            let accepted = match handle {
                Some(handle) => AcceptedScheduledInput::with_runtime_handle(
                    Some(existing_id.to_string()),
                    handle,
                ),
                None => AcceptedScheduledInput::with_authority_unavailable(
                    Some(existing_id.to_string()),
                    format!(
                        "runtime completion authority unavailable for terminal deduplicated input {existing_id}"
                    ),
                ),
            };
            Ok(build_dispatch_from_accepted(
                occurrence,
                accepted,
                materialized_session_id,
            ))
        }
        meerkat_runtime::accept::AcceptOutcome::Rejected { reason } => {
            Ok(immediate_delivery_failure(
                occurrence,
                reason.to_string(),
                DeliveryFailureReason::RuntimeRejected,
                None,
                materialized_session_id,
            ))
        }
        _ => Ok(immediate_delivery_failure(
            occurrence,
            "runtime returned an unknown admission outcome".to_string(),
            DeliveryFailureReason::RuntimeRejected,
            None,
            materialized_session_id,
        )),
    }
}

impl<B: SessionAgentBuilder + 'static> RuntimeBackedScheduleSessionHost<B> {
    fn new(
        service: Arc<PersistentSessionService<B>>,
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
        self.ensure_session_target_exists(session_id).await?;

        self.runtime_adapter
            .ensure_session_with_executor(
                session_id.clone(),
                default_persistent_executor(
                    Arc::clone(&self.service),
                    Arc::clone(&self.runtime_adapter),
                    session_id.clone(),
                ),
            )
            .await
            .map_err(schedule_internal)?;
        self.ensure_schedule_peer_ingress(session_id).await?;
        Ok(())
    }

    async fn ensure_schedule_peer_ingress(
        &self,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        #[cfg(feature = "comms")]
        {
            match self.runtime_adapter.peer_ingress_owner(session_id).await {
                meerkat_runtime::PeerIngressOwner::Unattached => {}
                meerkat_runtime::PeerIngressOwner::SessionOwned { .. } => {
                    if !self.session_keep_alive(session_id).await? {
                        return self.detach_schedule_peer_ingress(session_id).await;
                    }
                    // Scheduled delivery injects through the runtime input
                    // queue. If session-owned peer ingress already exists,
                    // refresh the authorized drain task without re-attaching
                    // with a possibly different CommsRuntime handle.
                    self.runtime_adapter
                        .refresh_session_owned_peer_ingress(session_id)
                        .await
                        .map_err(schedule_internal)?;
                    return Ok(());
                }
                meerkat_runtime::PeerIngressOwner::MobOwned { .. } => {
                    // Mob ownership is authoritative. Schedule delivery must
                    // not claim or downgrade the peer-ingress transport.
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }

        self.update_peer_ingress_context(session_id).await
    }

    async fn detach_schedule_peer_ingress(
        &self,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        #[cfg(feature = "comms")]
        {
            self.runtime_adapter
                .update_peer_ingress_context(session_id, false, None)
                .await
                .map_err(schedule_internal)?;
        }
        #[cfg(not(feature = "comms"))]
        let _ = session_id;
        Ok(())
    }

    async fn session_keep_alive(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, ScheduleDomainError> {
        #[cfg(feature = "comms")]
        {
            let session = self
                .service
                .load_authoritative_session(session_id)
                .await
                .map_err(schedule_internal)?
                .ok_or_else(|| {
                    ScheduleDomainError::InvalidSchedule(format!("session not found: {session_id}"))
                })?;
            session
                .session_metadata()
                .map(|metadata| metadata.keep_alive)
                .ok_or_else(|| {
                    ScheduleDomainError::Internal(format!(
                        "session {session_id} is missing session metadata"
                    ))
                })
        }
        #[cfg(not(feature = "comms"))]
        {
            let _ = session_id;
            Ok(false)
        }
    }

    async fn update_peer_ingress_context(
        &self,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        #[cfg(feature = "comms")]
        {
            let session = self
                .service
                .load_authoritative_session(session_id)
                .await
                .map_err(schedule_internal)?
                .ok_or_else(|| {
                    ScheduleDomainError::InvalidSchedule(format!("session not found: {session_id}"))
                })?;
            let keep_alive = session
                .session_metadata()
                .ok_or_else(|| {
                    ScheduleDomainError::Internal(format!(
                        "session {session_id} is missing session metadata"
                    ))
                })?
                .keep_alive;
            configure_peer_ingress(&self.runtime_adapter, &self.service, session_id, keep_alive)
                .await
                .map_err(schedule_internal)?;
        }
        #[cfg(not(feature = "comms"))]
        let _ = session_id;
        Ok(())
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

    fn build_materialized_request(
        &self,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<CreateSessionRequest, ScheduleDomainError> {
        let build = materialized_build_options(&self.build_template, create)?;

        Ok(CreateSessionRequest {
            model: create.model.clone(),
            prompt: ContentInput::Text(String::new()),
            // Parse the schedule-spec prompt representation once at this
            // ingest boundary: an occurrence-rendered prompt wins over the
            // spec prompt; either becomes an explicit `Set`, absence inherits.
            system_prompt: match prompt_system_prompt
                .map(str::to_owned)
                .or_else(|| create.system_prompt.clone())
            {
                Some(prompt) => crate::SystemPromptOverride::Set(prompt),
                None => crate::SystemPromptOverride::Inherit,
            },
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
impl<B: SessionAgentBuilder + 'static> SurfaceScheduleSessionHost
    for RuntimeBackedScheduleSessionHost<B>
{
    async fn probe_session_target(
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

    async fn materialize_session(
        &self,
        occurrence: &crate::Occurrence,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError> {
        let request = self.build_materialized_request(create, prompt_system_prompt)?;
        let keep_alive = request.build.as_ref().is_some_and(|build| build.keep_alive);
        // Layer A: derive the materialized SessionId deterministically from the
        // occurrence identity (NOT attempt_count) so a redrive reuses the same
        // durable session via the create-or-reuse `resume_session` recovery
        // path in `materialize_session`, instead of minting a fresh random id
        // that would orphan the prior session on a crash/lease-expiry/cancel
        // landing inside the materialize->bind window.
        let session = Session::with_id(occurrence.materialized_session_id());
        let result = Box::pin(materialize_session(
            &self.service,
            &self.runtime_adapter,
            session,
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
        .await
        .map_err(schedule_internal)?;
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
        self.ensure_runtime_session_registered(session_id).await?;

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

    async fn deliver_event(
        &self,
        session_id: &SessionId,
        occurrence: &crate::Occurrence,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        materialized_session_id: Option<SessionId>,
    ) -> Result<crate::DeliveryDispatch, ScheduleDomainError> {
        self.ensure_runtime_session_registered(session_id).await?;

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
}

fn schedule_internal(error: impl std::fmt::Display) -> ScheduleDomainError {
    ScheduleDomainError::Internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::skills::{SkillKey, SkillName, SourceUuid};

    fn fixture_skill_key(name: &str) -> SkillKey {
        let skill_name = match SkillName::parse(name) {
            Ok(skill_name) => skill_name,
            Err(err) => unreachable!("static skill fixture is invalid: {err}"),
        };
        SkillKey::new(SourceUuid::builtin(), skill_name)
    }

    #[test]
    fn materialized_build_options_forwards_preload_skill_keys() {
        let key = fixture_skill_key("email");
        let create = SessionMaterializationSpec {
            model: "claude-sonnet-4-6".to_string(),
            system_prompt: None,
            max_tokens: None,
            provider: None,
            output_schema: None,
            structured_output_retries: None,
            provider_params: None,
            comms_name: None,
            peer_meta: None,
            labels: Default::default(),
            preload_skills: vec![key.clone()],
            additional_instructions: Vec::new(),
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            keep_alive: false,
            app_context: None,
        };

        let build = materialized_build_options(&SessionBuildOptions::default(), &create)
            .expect("valid realm slug");

        assert_eq!(build.preload_skills, Some(vec![key]));
    }

    fn sample_occurrence() -> crate::Occurrence {
        let schedule = meerkat_schedule::Schedule::new(meerkat_schedule::CreateScheduleRequest {
            name: Some("runtime-schedule-host-test".to_string()),
            description: None,
            trigger: meerkat_schedule::TriggerSpec::Interval(
                meerkat_schedule::IntervalTriggerSpec {
                    start_at_utc: chrono::Utc::now(),
                    every_seconds: 60,
                    end_at_utc: None,
                },
            ),
            target: crate::TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id: SessionId::new(),
                action: meerkat_schedule::ScheduledSessionAction::Prompt {
                    prompt: ContentInput::Text("hello".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }),
            misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
            overlap_policy: meerkat_schedule::OverlapPolicy::SkipIfRunning,
            missing_target_policy: meerkat_schedule::MissingTargetPolicy::Skip,
            labels: Default::default(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("sample schedule creation should pass generated authority");
        meerkat_schedule::Occurrence::planned_from_schedule(
            &schedule,
            meerkat_schedule::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("sample occurrence planning should pass generated authority")
    }

    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        not(target_arch = "wasm32")
    ))]
    async fn build_comms_test_service(
        temp: &tempfile::TempDir,
        shared_runtime: Arc<crate::CommsRuntime>,
    ) -> (
        Arc<PersistentSessionService<crate::FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        let jsonl_store = Arc::new(crate::JsonlStore::new(temp.path().join("sessions")));
        jsonl_store.init().await.expect("init jsonl store");
        let persistence = crate::PersistenceBundle::new(
            jsonl_store as Arc<dyn crate::SessionStore>,
            Some(Arc::new(meerkat_runtime::InMemoryRuntimeStore::new())
                as Arc<dyn meerkat_runtime::RuntimeStore>),
            Arc::new(crate::MemoryBlobStore::new()),
        );
        let factory = crate::AgentFactory::new(temp.path().join("sessions"))
            .with_comms_runtime(shared_runtime);
        let mut builder = crate::FactoryAgentBuilder::new(factory, crate::Config::default());
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
        let (service, runtime_adapter) =
            crate::surface::build_runtime_backed_service(builder, 4, persistence);
        (Arc::new(service), runtime_adapter)
    }

    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        not(target_arch = "wasm32")
    ))]
    fn make_runtime_backed_request(comms_name: &str, keep_alive: bool) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "gpt-5.4".to_string(),
            prompt: ContentInput::Text(String::new()),
            system_prompt: crate::SystemPromptOverride::Set(
                "scheduled attached-session regression".to_string(),
            ),
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                comms_name: Some(comms_name.to_string()),
                keep_alive,
                ..Default::default()
            }),
            labels: None,
        }
    }

    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        not(target_arch = "wasm32")
    ))]
    #[tokio::test]
    async fn deliver_prompt_to_already_session_owned_ingress_does_not_reattach() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service_runtime = Arc::new(
            crate::CommsRuntime::inproc_only("schedule-service-runtime")
                .expect("service comms runtime"),
        );
        let preattached_runtime = Arc::new(
            crate::CommsRuntime::inproc_only("schedule-preattached-runtime")
                .expect("preattached comms runtime"),
        );
        let (service, runtime_adapter) =
            build_comms_test_service(&temp, Arc::clone(&service_runtime)).await;

        let session = Session::new();
        let session_id = session.id().clone();
        let result = Box::pin(materialize_session(
            &service,
            &runtime_adapter,
            session,
            make_runtime_backed_request("scheduled-attached-session", true),
            {
                let service = Arc::clone(&service);
                let runtime_adapter = Arc::clone(&runtime_adapter);
                move |session_id| default_persistent_executor(service, runtime_adapter, session_id)
            },
        ))
        .await
        .expect("materialize runtime-backed session");

        let preattached_runtime: Arc<dyn meerkat_core::agent::CommsRuntime> = preattached_runtime;
        runtime_adapter
            .update_peer_ingress_context(&session_id, true, Some(Arc::clone(&preattached_runtime)))
            .await
            .expect("pre-attach session-owned ingress");
        assert!(matches!(
            runtime_adapter.peer_ingress_owner(&session_id).await,
            meerkat_runtime::PeerIngressOwner::SessionOwned { .. }
        ));

        let host = RuntimeBackedScheduleSessionHost::new(
            Arc::clone(&service),
            Arc::clone(&runtime_adapter),
            SessionBuildOptions::default(),
        );
        let occurrence = sample_occurrence();
        let dispatch = host
            .deliver_prompt(
                &result.session_id,
                &occurrence,
                ScheduledPromptDispatch {
                    prompt: ContentInput::Text("scheduled prompt".to_string()),
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                    materialized_session_id: None,
                },
            )
            .await
            .expect("scheduled prompt delivery should enqueue without reattaching ingress");

        let terminal = tokio::time::timeout(std::time::Duration::from_secs(2), dispatch.completion)
            .await
            .expect("scheduled prompt should complete")
            .expect("delivery terminal should resolve");
        assert_eq!(
            terminal.phase,
            meerkat_schedule::OccurrencePhase::AwaitingCompletion
        );
        assert_eq!(
            terminal.runtime_completion_outcome,
            Some(meerkat_schedule::RuntimeCompletionOutcome::Completed)
        );

        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        runtime_adapter.unregister_session(&result.session_id).await;
    }

    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        not(target_arch = "wasm32")
    ))]
    #[tokio::test]
    async fn deliver_prompt_to_session_owned_non_keep_alive_ingress_detaches_without_reattach() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service_runtime = Arc::new(
            crate::CommsRuntime::inproc_only("schedule-service-runtime-detach")
                .expect("service comms runtime"),
        );
        let preattached_runtime = Arc::new(
            crate::CommsRuntime::inproc_only("schedule-preattached-runtime-detach")
                .expect("preattached comms runtime"),
        );
        let (service, runtime_adapter) =
            build_comms_test_service(&temp, Arc::clone(&service_runtime)).await;

        let session = Session::new();
        let session_id = session.id().clone();
        let result = Box::pin(materialize_session(
            &service,
            &runtime_adapter,
            session,
            make_runtime_backed_request("scheduled-detach-session", false),
            {
                let service = Arc::clone(&service);
                let runtime_adapter = Arc::clone(&runtime_adapter);
                move |session_id| default_persistent_executor(service, runtime_adapter, session_id)
            },
        ))
        .await
        .expect("materialize runtime-backed session");

        let preattached_runtime: Arc<dyn meerkat_core::agent::CommsRuntime> = preattached_runtime;
        runtime_adapter
            .update_peer_ingress_context(&session_id, true, Some(Arc::clone(&preattached_runtime)))
            .await
            .expect("pre-attach session-owned ingress");
        assert!(matches!(
            runtime_adapter.peer_ingress_owner(&session_id).await,
            meerkat_runtime::PeerIngressOwner::SessionOwned { .. }
        ));

        let host = RuntimeBackedScheduleSessionHost::new(
            Arc::clone(&service),
            Arc::clone(&runtime_adapter),
            SessionBuildOptions::default(),
        );
        let occurrence = sample_occurrence();
        let dispatch = host
            .deliver_prompt(
                &result.session_id,
                &occurrence,
                ScheduledPromptDispatch {
                    prompt: ContentInput::Text("scheduled prompt".to_string()),
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                    materialized_session_id: None,
                },
            )
            .await
            .expect("scheduled prompt delivery should detach without reattaching ingress");

        let owner = runtime_adapter.peer_ingress_owner(&session_id).await;
        assert!(
            matches!(owner, meerkat_runtime::PeerIngressOwner::Unattached),
            "non-keep-alive session-owned ingress must detach instead of reattaching, got {owner:?}"
        );

        let terminal = tokio::time::timeout(std::time::Duration::from_secs(2), dispatch.completion)
            .await
            .expect("scheduled prompt should complete")
            .expect("delivery terminal should resolve");
        assert_eq!(
            terminal.phase,
            meerkat_schedule::OccurrencePhase::AwaitingCompletion
        );
        assert_eq!(
            terminal.runtime_completion_outcome,
            Some(meerkat_schedule::RuntimeCompletionOutcome::Completed)
        );

        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        runtime_adapter.unregister_session(&result.session_id).await;
    }

    #[test]
    fn materialized_session_id_is_deterministic_per_occurrence_and_attempt_invariant() {
        let mut occurrence = sample_occurrence();
        occurrence.attempt_count = 1;
        let first = occurrence.materialized_session_id();

        // Same occurrence id, a later reclaim attempt: the derived session id
        // MUST collapse to the same value so a redrive reuses, not orphans.
        occurrence.attempt_count = 7;
        let after_reclaim = occurrence.materialized_session_id();
        assert_eq!(
            first, after_reclaim,
            "materialized session id must be invariant across attempt_count"
        );

        // A distinct occurrence must derive a distinct session id.
        let other = sample_occurrence();
        assert_ne!(
            occurrence.occurrence_id, other.occurrence_id,
            "fixture occurrences must have distinct ids"
        );
        assert_ne!(
            occurrence.materialized_session_id(),
            other.materialized_session_id(),
            "distinct occurrences must derive distinct session ids"
        );
    }
}
