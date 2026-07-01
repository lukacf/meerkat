use std::sync::{Arc, Weak};

use async_trait::async_trait;
use meerkat::surface::{
    AcceptedScheduledInput, NoopScheduleMobHost, ScheduledPromptDispatch,
    SharedScheduleTargetAdapter, SurfaceScheduleMobHost, SurfaceScheduleSessionHost,
    build_dispatch_from_accepted, immediate_delivery_failure,
    recover_mob_member_identity_from_session_target, schedule_attempt_idempotency_key,
    schedule_host_supported, spawn_schedule_host,
};
use meerkat::{
    AgentBuildConfig, DeliveryDispatch, DeliveryFailureReason, IdentityTargetBinding, Occurrence,
    ScheduleDomainError, SessionMaterializationSpec, SessionTargetBinding, TargetProbeOutcome,
};
use meerkat_core::SessionId;
#[cfg(feature = "mob")]
use meerkat_mob_mcp::MobMcpScheduleHost;

use super::{SessionRuntime, SessionState};

fn accepted_scheduled_input_from_runtime_handle(
    correlation_id: Option<String>,
    handle: Option<meerkat_runtime::completion::CompletionHandle>,
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
    occurrence: &Occurrence,
    outcome: meerkat_runtime::accept::AcceptOutcome,
    handle: Option<meerkat_runtime::completion::CompletionHandle>,
    materialized_session_id: Option<SessionId>,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
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

/// Project a schedule materialization spec into the spec-derived
/// [`AgentBuildConfig`] fields.
///
/// Runtime-scoped fallbacks (realm/instance/backend inherited from the
/// runtime, config generation, default LLM client) are layered on by
/// `SessionRuntime::materialize_scheduled_session`.
fn scheduled_build_config(
    create: &SessionMaterializationSpec,
    prompt_system_prompt: Option<&str>,
) -> Result<AgentBuildConfig, ScheduleDomainError> {
    let mut build_config = AgentBuildConfig::new(create.model.clone());
    build_config.provider = create.provider;
    build_config.max_tokens = create.max_tokens;
    // Parse the schedule-spec prompt representation once at this ingest
    // boundary: an occurrence-rendered prompt wins over the spec prompt;
    // either becomes an explicit `Set`, absence inherits.
    build_config.system_prompt = match prompt_system_prompt
        .map(str::to_owned)
        .or_else(|| create.system_prompt.clone())
    {
        Some(prompt) => meerkat::SystemPromptOverride::Set(prompt),
        None => meerkat::SystemPromptOverride::Inherit,
    };
    build_config.output_schema = create.output_schema.clone();
    build_config.structured_output_retries = create.structured_output_retries;
    build_config.provider_params = create.provider_params.clone();
    build_config.comms_name = create.comms_name.clone();
    build_config.peer_meta = create.peer_meta.clone();
    // Typed pass-through: `SessionMaterializationSpec.preload_skills` is the
    // canonical `Vec<SkillKey>` carrier; an empty list normalizes to `None`
    // (metadata-only inventory), matching the facade schedule host.
    build_config.preload_skills =
        (!create.preload_skills.is_empty()).then(|| create.preload_skills.clone());
    build_config.additional_instructions = (!create.additional_instructions.is_empty())
        .then(|| create.additional_instructions.clone());
    // Schedule specs carry the realm as a plain slug string; parse it once
    // at this ingest boundary. A malformed slug fails closed.
    build_config.realm_id = create
        .realm_id
        .as_deref()
        .map(meerkat_core::RealmId::parse)
        .transpose()
        .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
    build_config.instance_id = create.instance_id.clone();
    build_config.backend = create
        .backend
        .as_deref()
        .and_then(meerkat_core::RecoveryBackendKind::parse);
    build_config.keep_alive = create.keep_alive;
    build_config.app_context = create.app_context.clone();
    Ok(build_config)
}

struct RpcScheduleTargetAdapter {
    runtime: Weak<SessionRuntime>,
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
}

#[async_trait]
impl SurfaceScheduleSessionHost for RpcScheduleTargetAdapter {
    async fn probe_session_target(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;
        let Some(session_id) = binding.resolved_session_id() else {
            return Ok(TargetProbeOutcome::Ready);
        };

        // Row #98: a store fault on the live session-state read is a typed
        // failure, not "session absent" -> falling through to load_persisted.
        if let Some(info) = runtime
            .session_state(session_id)
            .await
            .map_err(rpc_to_schedule)?
        {
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
            .map_err(rpc_to_schedule)?;
        match persisted {
            Some(_) => Ok(TargetProbeOutcome::Ready),
            _ => Ok(TargetProbeOutcome::Missing {
                detail: Some(format!("session not found: {session_id}")),
            }),
        }
    }

    async fn recover_session_target_identity(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<Option<IdentityTargetBinding>, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;
        let Some(session_id) = binding.resolved_session_id() else {
            return Ok(None);
        };
        let session = runtime
            .load_persisted_session(session_id)
            .await
            .map_err(rpc_to_schedule)?;
        Ok(recover_mob_member_identity_from_session_target(
            binding,
            session.as_ref(),
        ))
    }

    async fn materialize_session(
        &self,
        _occurrence: &Occurrence,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError> {
        // Idempotent reuse of an already-bound materialized session is enforced
        // by `SharedScheduleTargetAdapter::resolve_session` (Layer B) before
        // this is reached, so this surface need not re-derive a deterministic id.
        let runtime = self.upgrade_runtime()?;
        Box::pin(runtime.materialize_scheduled_session(create, prompt_system_prompt)).await
    }

    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;
        runtime
            .deliver_scheduled_prompt(session_id, occurrence, dispatch)
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
        let runtime = self.upgrade_runtime()?;
        runtime
            .deliver_scheduled_event(
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

impl SessionRuntime {
    pub async fn ensure_schedule_host_started(self: &Arc<Self>) -> Result<(), ScheduleDomainError> {
        if !schedule_host_supported(self.schedule_service.store().kind()) {
            return Ok(());
        }

        let mut slot = self.schedule_host.lock().await;
        if slot.is_some() {
            return Ok(());
        }

        let session_host: Arc<dyn SurfaceScheduleSessionHost> =
            Arc::new(RpcScheduleTargetAdapter::new(self));
        let mob_host = rpc_schedule_mob_host(self);
        let shared_adapter = Arc::new(SharedScheduleTargetAdapter::new(
            self.schedule_service(),
            session_host,
            mob_host,
        ));
        *slot = Some(spawn_schedule_host(
            self.schedule_service(),
            shared_adapter,
            self.schedule_owner_id(),
        ));
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
        let mut build_config = scheduled_build_config(create, prompt_system_prompt)?;
        // Runtime-scoped fallbacks for facts the spec leaves unset.
        build_config.realm_id = build_config.realm_id.or_else(|| self.inner.realm_id());
        build_config.instance_id = build_config
            .instance_id
            .or_else(|| self.inner.instance_id());
        build_config.backend = build_config.backend.or_else(|| {
            self.inner
                .backend()
                .as_deref()
                .and_then(meerkat_core::RecoveryBackendKind::parse)
        });
        build_config.config_generation = if let Some(runtime) = self.config_runtime() {
            runtime.get().await.ok().map(|snapshot| snapshot.generation)
        } else {
            create.config_generation
        };
        if build_config.llm_client_override.is_none()
            && let Some(default_llm_client) = self.default_llm_client()
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
        dispatch: ScheduledPromptDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        self.ensure_runtime_executor(session_id)
            .await
            .map_err(rpc_to_schedule)?;

        let comms_rt = self.service.comms_runtime(session_id).await;
        #[cfg(feature = "comms")]
        let peer_ingress_enabled = {
            let keep_alive = self
                .persisted_keep_alive(session_id)
                .await
                .map_err(rpc_to_schedule)?;
            self.preserve_existing_peer_ingress(session_id, keep_alive)
                .await
                .map_err(rpc_to_schedule)?
        };
        #[cfg(not(feature = "comms"))]
        let peer_ingress_enabled = false;
        if comms_rt.is_some() || peer_ingress_enabled {
            self.runtime_adapter
                .update_peer_ingress_context(session_id, peer_ingress_enabled, comms_rt)
                .await
                .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        }

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

    async fn deliver_scheduled_event(
        self: &Arc<Self>,
        session_id: &SessionId,
        occurrence: &Occurrence,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        materialized_session_id: Option<SessionId>,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        self.ensure_runtime_executor(session_id)
            .await
            .map_err(rpc_to_schedule)?;

        let comms_rt = self.service.comms_runtime(session_id).await;
        #[cfg(feature = "comms")]
        let peer_ingress_enabled = {
            let keep_alive = self
                .persisted_keep_alive(session_id)
                .await
                .map_err(rpc_to_schedule)?;
            self.preserve_existing_peer_ingress(session_id, keep_alive)
                .await
                .map_err(rpc_to_schedule)?
        };
        #[cfg(not(feature = "comms"))]
        let peer_ingress_enabled = false;
        if comms_rt.is_some() || peer_ingress_enabled {
            self.runtime_adapter
                .update_peer_ingress_context(session_id, peer_ingress_enabled, comms_rt)
                .await
                .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        }

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

    fn schedule_owner_id(&self) -> String {
        let realm_owned = self.inner.realm_id();
        let realm = realm_owned.as_ref().map(|r| r.as_str()).unwrap_or("realm");
        let instance_owned = self.inner.instance_id();
        let instance = instance_owned.as_deref().unwrap_or("rpc");
        format!("rpc-scheduler:{realm}:{instance}")
    }
}

fn rpc_to_schedule(error: crate::protocol::RpcError) -> ScheduleDomainError {
    ScheduleDomainError::Internal(error.message)
}

fn rpc_schedule_mob_host(runtime: &Arc<SessionRuntime>) -> Arc<dyn SurfaceScheduleMobHost> {
    #[cfg(feature = "mob")]
    {
        if let Some(mob_state) = runtime.mob_state() {
            return Arc::new(MobMcpScheduleHost::new(mob_state));
        }
        Arc::new(NoopScheduleMobHost::new(
            "mob state unavailable on RPC host",
        ))
    }

    #[cfg(not(feature = "mob"))]
    {
        let _ = runtime;
        Arc::new(NoopScheduleMobHost::new(
            "scheduled mob targets require the mob feature on the RPC host",
        ))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
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

    fn materialization_spec() -> SessionMaterializationSpec {
        SessionMaterializationSpec {
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

    #[test]
    fn scheduled_build_config_forwards_preload_skill_keys() {
        // Delivery pin for the typed pass-through: scheduled-session preload
        // skills reach the build config instead of being discarded.
        let key = fixture_skill_key("email");
        let mut create = materialization_spec();
        create.preload_skills = vec![key.clone()];

        let build = scheduled_build_config(&create, None).expect("valid spec");

        assert_eq!(build.preload_skills, Some(vec![key]));
    }

    #[test]
    fn scheduled_build_config_empty_preload_skills_normalizes_to_none() {
        let build = scheduled_build_config(&materialization_spec(), None).expect("valid spec");
        assert_eq!(build.preload_skills, None);
    }

    #[test]
    fn scheduled_build_config_system_prompt_ingest() {
        // Occurrence-rendered prompt wins over the spec prompt.
        let mut create = materialization_spec();
        create.system_prompt = Some("spec prompt".to_string());
        let build = scheduled_build_config(&create, Some("rendered prompt")).expect("valid spec");
        assert_eq!(
            build.system_prompt,
            meerkat::SystemPromptOverride::Set("rendered prompt".to_string())
        );

        // Spec prompt applies when no rendered prompt is present.
        let build = scheduled_build_config(&create, None).expect("valid spec");
        assert_eq!(
            build.system_prompt,
            meerkat::SystemPromptOverride::Set("spec prompt".to_string())
        );

        // Absence of both inherits.
        let build = scheduled_build_config(&materialization_spec(), None).expect("valid spec");
        assert!(build.system_prompt.is_inherit());
    }
}
