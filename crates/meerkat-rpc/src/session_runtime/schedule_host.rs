use std::sync::{Arc, Weak};

use async_trait::async_trait;
use meerkat::surface::{
    NoopScheduleMobHost, ScheduledEventDispatch, ScheduledPromptDispatch,
    SharedScheduleTargetAdapter, SurfaceScheduleMobHost, SurfaceScheduleSessionHost,
    accept_schedule_runtime_input_with_reconciliation,
    recover_mob_member_identity_from_session_target, schedule_host_supported,
    schedule_runtime_correlation_id, schedule_runtime_delivery_idempotency_key,
    spawn_schedule_host,
};
use meerkat::{
    AgentBuildConfig, DeliveryDispatch, IdentityTargetBinding, Occurrence,
    ScheduleDeliveryIdentity, ScheduleDomainError, Session, SessionMaterializationSpec,
    SessionTargetBinding, TargetProbeOutcome,
};
use meerkat_core::SessionId;
#[cfg(feature = "mob")]
use meerkat_mob_mcp::MobMcpScheduleHost;

use super::{SessionRuntime, SessionState};

/// Project a schedule materialization spec into the spec-derived
/// [`AgentBuildConfig`] fields.
///
/// Runtime-scoped fallbacks (realm/instance/backend inherited from the
/// runtime, config generation, default LLM client) are layered on by
/// `SessionRuntime::materialize_scheduled_session`.
fn scheduled_build_config(
    create: &SessionMaterializationSpec,
) -> Result<AgentBuildConfig, ScheduleDomainError> {
    let mut build_config = AgentBuildConfig::new(create.model.clone());
    build_config.provider = create.provider;
    build_config.max_tokens = create.max_tokens;
    build_config.system_prompt = match create.system_prompt.clone() {
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
    /// Its prompt and event deliveries admit through
    /// `accept_schedule_runtime_input_with_reconciliation` on the calling task.
    fn decides_existing_admission(&self) -> bool {
        true
    }

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

        // Presence and archive state, read body-free: a session another
        // process runs holds a provisional tail from its first round commit,
        // and a body read refuses throughout it.
        match super::committed_source_archived(&runtime.service, session_id)
            .await
            .map_err(|error| rpc_to_schedule(super::session_error_to_rpc(error)))?
        {
            Some(false) => Ok(TargetProbeOutcome::Ready),
            Some(true) | None => Ok(TargetProbeOutcome::Missing {
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
        // The owner identity is committed metadata, which a running durable
        // run's provisional tail does not change: it reads the committed
        // snapshot, and an archived session has none.
        let session = runtime
            .service
            .load_committed_session_tolerating_tail(session_id)
            .await
            .map_err(|error| rpc_to_schedule(super::session_error_to_rpc(error)))?;
        let session = match session {
            Some(session) => {
                let archived = runtime
                    .service
                    .session_archived_by_authority(session_id, &session)
                    .await
                    .map_err(|error| rpc_to_schedule(super::session_error_to_rpc(error)))?;
                (!archived).then_some(session)
            }
            None => None,
        };
        Ok(recover_mob_member_identity_from_session_target(
            binding,
            session.as_ref(),
        ))
    }

    async fn materialize_session(
        &self,
        occurrence: &Occurrence,
        create: &SessionMaterializationSpec,
    ) -> Result<SessionId, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;
        let session_id = occurrence.materialized_session_id();
        Box::pin(runtime.materialize_scheduled_session(&session_id, create)).await
    }

    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;
        runtime
            .deliver_scheduled_prompt(session_id, occurrence, identity, dispatch)
            .await
    }

    async fn deliver_event(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        dispatch: ScheduledEventDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let runtime = self.upgrade_runtime()?;
        runtime
            .deliver_scheduled_event(session_id, occurrence, identity, dispatch)
            .await
    }
}

impl SessionRuntime {
    pub async fn ensure_schedule_host_started(self: &Arc<Self>) -> Result<(), ScheduleDomainError> {
        if !schedule_host_supported(self.schedule_service.store().kind()) {
            return Ok(());
        }

        let mut slot = self.schedule_host.lock().await;
        // A runtime whose shutdown began has stopped its firing host for
        // good: a firing would admit work into a runtime that is stopping.
        // Read under the slot, so an arming that waited on the shutdown's
        // stop of the host does not start another one after it.
        if self.runtime_registration_fence.is_closing() {
            return Ok(());
        }
        if slot.as_ref().is_some_and(|handle| handle.is_running()) {
            return Ok(());
        }
        let stale_handle = slot.take();

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
        drop(slot);
        if let Some(stale_handle) = stale_handle {
            stale_handle.shutdown().await;
        }
        Ok(())
    }

    /// Arm the schedule firing host as soon as this runtime can execute
    /// agent work.
    ///
    /// The agent-facing `meerkat_schedule_*` tools installed at construction
    /// bind to THIS runtime's schedule service — so a schedule authored
    /// through them must have an in-process firing authority regardless of
    /// whether the embedder wired the RPC router (which arms eagerly at
    /// startup). Without this, a `SessionRuntime` embedded without the router
    /// hands agents a schedule store nothing drives: `meerkat_schedule_create`
    /// returns an id, the planner mints occurrences, and nothing ever claims
    /// them (field, 0.7.23: agent-authored schedules stranded pending 12h+
    /// past due while the embedder's own host drove a different store).
    ///
    /// Idempotent. The handle slot is the singular liveness authority: a
    /// finished supervisor is replaced on the next arming call rather than
    /// hidden behind a process-lifetime latch.
    pub(super) async fn arm_schedule_host_for_agent_tools(self: &Arc<Self>) {
        match self.ensure_schedule_host_started().await {
            Ok(()) => {}
            Err(error) => {
                tracing::warn!(
                    %error,
                    "failed to arm the schedule firing host; agent-authored schedules \
                     will not fire until it starts"
                );
            }
        }
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
        self: &Arc<Self>,
        session_id: &SessionId,
        create: &SessionMaterializationSpec,
    ) -> Result<SessionId, ScheduleDomainError> {
        // Create-or-reuse is keyed solely by the occurrence-derived session
        // identity. A crash after materialization but before the schedule bind
        // must reopen this exact actor, never mint a second orphan.
        if self
            .session_state(session_id)
            .await
            .map_err(rpc_to_schedule)?
            .is_some()
        {
            return Ok(session_id.clone());
        }
        // Presence and archive state, read body-free: the session may be
        // running in another process, its provisional tail ahead of the
        // committed snapshot.
        match super::committed_source_archived(&self.service, session_id)
            .await
            .map_err(|error| rpc_to_schedule(super::session_error_to_rpc(error)))?
        {
            Some(true) => {
                return Err(ScheduleDomainError::InvalidSchedule(format!(
                    "materialized session is archived: {session_id}"
                )));
            }
            Some(false) => return Ok(session_id.clone()),
            None => {}
        }

        let mut build_config = scheduled_build_config(create)?;
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

        self.create_session_from_seed(
            Session::with_id(session_id.clone()),
            build_config,
            Some(create.labels.clone()),
            None,
            Vec::new(),
            true,
        )
        .await
        .map_err(|error| ScheduleDomainError::Internal(error.message))
    }

    async fn deliver_scheduled_prompt(
        self: &Arc<Self>,
        session_id: &SessionId,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        self.ensure_runtime_executor(session_id)
            .await
            .map_err(rpc_to_schedule)?;

        let comms_rt = self.service.comms_runtime(session_id).await;
        #[cfg(feature = "comms")]
        let peer_ingress_enabled = {
            let keep_alive = self
                .staged_or_persisted_keep_alive(session_id)
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
                work_authorization: None,
                handling_mode: None,
                keep_alive: None,
                skill_references: (!dispatch.skill_refs.is_empty()).then(|| {
                    dispatch
                        .skill_refs
                        .iter()
                        .map(|skill_ref| skill_ref.key().clone())
                        .collect()
                }),
                turn_tool_overlay: None,
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
                system_prompts: dispatch.system_prompt.into_iter().collect(),
                transient_turn_context: dispatch.transient_turn_context,
                transient_turn_context_appends: Vec::new(),
                model: None,
                provider: None,
                self_hosted_server_id: None,
                provider_params: None,
                render_metadata: dispatch.render_metadata.clone(),
                execution_kind: None,
                peer_response_terminal_apply_intent: None,
                directed_interaction_ids: Vec::new(),
                auth_binding: None,
                transcript_identity: Default::default(),
                request_reasoning: None,
                request_reasoning_disposition: None,
            },
        );
        let mut prompt_input =
            meerkat_runtime::PromptInput::from_content_input(dispatch.prompt, turn_metadata);
        prompt_input.header.source = meerkat_runtime::InputOrigin::System;
        prompt_input.header.idempotency_key = Some(meerkat_runtime::IdempotencyKey::new(
            schedule_runtime_delivery_idempotency_key(
                self.runtime_adapter.as_ref(),
                session_id,
                occurrence,
                &identity.idempotency_key,
            )
            .await?,
        ));
        prompt_input.header.correlation_id = Some(schedule_runtime_correlation_id(identity)?);

        accept_schedule_runtime_input_with_reconciliation(
            self.runtime_adapter.as_ref(),
            session_id,
            occurrence,
            identity,
            meerkat_runtime::Input::Prompt(prompt_input),
            dispatch.materialized_session_id,
            |input| {
                self.runtime_adapter
                    .accept_input_with_completion(session_id, input)
            },
        )
        .await
    }

    async fn deliver_scheduled_event(
        self: &Arc<Self>,
        session_id: &SessionId,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        dispatch: ScheduledEventDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        self.ensure_runtime_executor(session_id)
            .await
            .map_err(rpc_to_schedule)?;

        let comms_rt = self.service.comms_runtime(session_id).await;
        #[cfg(feature = "comms")]
        let peer_ingress_enabled = {
            let keep_alive = self
                .staged_or_persisted_keep_alive(session_id)
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
            objective_id: None,
            header: meerkat_runtime::input::InputHeader {
                ingress_context: None,
                retained_resume: None,
                authority_association: None,
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: meerkat_runtime::InputOrigin::External {
                    source_name: format!("schedule:{}", occurrence.schedule_id),
                },
                durability: meerkat_runtime::input::InputDurability::Durable,
                visibility: meerkat_runtime::input::InputVisibility::default(),
                idempotency_key: Some(meerkat_runtime::IdempotencyKey::new(
                    schedule_runtime_delivery_idempotency_key(
                        self.runtime_adapter.as_ref(),
                        session_id,
                        occurrence,
                        &identity.idempotency_key,
                    )
                    .await?,
                )),
                supersession_key: None,
                correlation_id: Some(schedule_runtime_correlation_id(identity)?),
            },
            event_type: dispatch.event_type,
            payload: dispatch.payload,
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: dispatch.render_metadata,
        });

        accept_schedule_runtime_input_with_reconciliation(
            self.runtime_adapter.as_ref(),
            session_id,
            occurrence,
            identity,
            input,
            dispatch.materialized_session_id,
            |input| {
                self.runtime_adapter
                    .accept_input_with_completion(session_id, input)
            },
        )
        .await
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

/// What a scheduled mob target hears while the RPC host has no mob state.
#[cfg(feature = "mob")]
const MOB_STATE_UNAVAILABLE: &str = "mob state unavailable on RPC host";

/// The RPC host's scheduled mob targets, served by the runtime's current mob
/// state, resolved once per operation and held only for that operation.
///
/// The schedule service keeps this host for its create-time identity probe
/// for as long as any clone of the service lives, and the session builder's
/// schedule tools hold such a clone. Holding the mob state itself here would
/// keep the state alive past shutdown, and with it the session service, the
/// runtime machine and this runtime. Like the session host, it holds the
/// runtime weakly and reports a gone runtime as a stopped driver; a live
/// runtime without a mob state answers as the unavailable host does.
#[cfg(feature = "mob")]
struct RpcScheduleMobHost {
    runtime: Weak<SessionRuntime>,
}

#[cfg(feature = "mob")]
impl RpcScheduleMobHost {
    fn current(&self) -> Result<Box<dyn SurfaceScheduleMobHost>, ScheduleDomainError> {
        let runtime = self
            .runtime
            .upgrade()
            .ok_or(ScheduleDomainError::DriverStopped)?;
        Ok(match runtime.mob_state() {
            Some(state) => Box::new(MobMcpScheduleHost::new(state)),
            None => Box::new(NoopScheduleMobHost::new(MOB_STATE_UNAVAILABLE)),
        })
    }
}

#[cfg(feature = "mob")]
#[async_trait]
impl SurfaceScheduleMobHost for RpcScheduleMobHost {
    async fn probe_mob_target(
        &self,
        binding: &meerkat::MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let host = self.current()?;
        host.probe_mob_target(binding).await
    }

    async fn deliver_mob_target(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        binding: &meerkat::MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let host = self.current()?;
        host.deliver_mob_target(occurrence, identity, binding).await
    }

    async fn probe_identity_target(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<TargetProbeOutcome>, ScheduleDomainError> {
        let host = self.current()?;
        host.probe_identity_target(binding).await
    }

    async fn resolve_identity_target(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<SessionId>, ScheduleDomainError> {
        let host = self.current()?;
        host.resolve_identity_target(binding).await
    }

    async fn deliver_identity_target(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<DeliveryDispatch>, ScheduleDomainError> {
        let host = self.current()?;
        host.deliver_identity_target(occurrence, identity, binding)
            .await
    }

    /// A redelivery is the current mob host's to decide: its existing-only
    /// answer reads the mob's durable external-delivery record, where the
    /// trait default answers an unproven late one not admitted unread. A
    /// gone runtime cannot show whether the earlier attempt was admitted,
    /// so the occurrence stays reclaimable.
    async fn deliver_existing_mob_target(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        binding: &meerkat::MobTargetBinding,
        existing: meerkat::ExistingDelivery,
    ) -> Result<meerkat::ExistingDeliveryAnswer, ScheduleDomainError> {
        let host = self.current().map_err(redelivery_mob_host_unavailable)?;
        host.deliver_existing_mob_target(occurrence, identity, binding, existing)
            .await
    }

    async fn deliver_existing_identity_target(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        binding: &IdentityTargetBinding,
        existing: meerkat::ExistingDelivery,
    ) -> Result<Option<meerkat::ExistingDeliveryAnswer>, ScheduleDomainError> {
        let host = self.current().map_err(redelivery_mob_host_unavailable)?;
        host.deliver_existing_identity_target(occurrence, identity, binding, existing)
            .await
    }
}

/// The schedule error of a redelivery whose mob host is gone.
#[cfg(feature = "mob")]
fn redelivery_mob_host_unavailable(error: ScheduleDomainError) -> ScheduleDomainError {
    ScheduleDomainError::DeliveryRepairDeferred {
        detail: format!("mob host is unavailable for a redelivery: {error}"),
    }
}

fn rpc_schedule_mob_host(runtime: &Arc<SessionRuntime>) -> Arc<dyn SurfaceScheduleMobHost> {
    #[cfg(feature = "mob")]
    {
        Arc::new(RpcScheduleMobHost {
            runtime: Arc::downgrade(runtime),
        })
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
#[allow(clippy::expect_used, clippy::panic)]
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

        let build = scheduled_build_config(&create).expect("valid spec");

        assert_eq!(build.preload_skills, Some(vec![key]));
    }

    #[test]
    fn scheduled_build_config_empty_preload_skills_normalizes_to_none() {
        let build = scheduled_build_config(&materialization_spec()).expect("valid spec");
        assert_eq!(build.preload_skills, None);
    }

    #[test]
    fn scheduled_build_config_uses_only_materialization_system_prompt() {
        let mut create = materialization_spec();
        create.system_prompt = Some("spec prompt".to_string());
        let build = scheduled_build_config(&create).expect("valid spec");
        assert_eq!(
            build.system_prompt,
            meerkat::SystemPromptOverride::Set("spec prompt".to_string())
        );

        let build = scheduled_build_config(&materialization_spec()).expect("valid spec");
        assert!(build.system_prompt.is_inherit());
    }

    /// The schedule service keeps its schedule host's mob host (through the
    /// create-time identity probe) for as long as anything holds the service,
    /// and the mob state holds the service through its own session service.
    /// A runtime that armed its schedule host with a mob state, then shut
    /// down and was dropped, releases that mob state and its machine (and so
    /// the machine's execution custody), so the realm can be reopened in the
    /// same process.
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn shutdown_releases_the_mob_state_its_schedule_host_targets() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (_, bundle) = meerkat::open_realm_persistence_in(
            temp.path(),
            "schedule-mob-host",
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
        )
        .await
        .expect("realm persistence");
        let machine = bundle.runtime_adapter();
        let config_store: Arc<dyn meerkat_core::ConfigStore> =
            Arc::new(meerkat_core::MemoryConfigStore::new(
                meerkat_core::Config::default(),
                meerkat_models::canonical(),
            ));
        let runtime = Arc::new(
            SessionRuntime::new_with_config_store(
                meerkat::AgentFactory::new(temp.path().join("sessions")),
                meerkat_core::Config::default(),
                Arc::clone(&config_store),
                1,
                bundle,
                crate::router::NotificationSink::noop(),
            )
            .expect("compose the RPC session runtime"),
        );
        let mob_state =
            crate::router::compose_rpc_mob_state(&runtime, &config_store, None).expect("mob state");
        let released = Arc::downgrade(&mob_state);
        runtime.set_mob_state(mob_state);
        runtime
            .ensure_schedule_host_started()
            .await
            .expect("schedule host");

        runtime.try_shutdown().await.expect("shutdown");
        drop(runtime);

        assert_eq!(released.strong_count(), 0, "the mob state is released");
        assert_eq!(
            Arc::strong_count(&machine),
            1,
            "only this test still holds the machine"
        );
    }

    /// One occurrence of a schedule firing `target`.
    #[cfg(feature = "mob")]
    fn scheduled_occurrence(target: meerkat::TargetBinding) -> Occurrence {
        let schedule = meerkat::Schedule::new(meerkat::CreateScheduleRequest {
            name: Some("rpc-mob-redelivery".to_string()),
            description: None,
            trigger: meerkat::TriggerSpec::Interval(meerkat::IntervalTriggerSpec {
                start_at_utc: chrono::Utc::now(),
                every_seconds: 60,
                end_at_utc: None,
            }),
            target,
            misfire_policy: meerkat::MisfirePolicy::Skip,
            overlap_policy: meerkat::OverlapPolicy::SkipIfRunning,
            missing_target_policy: meerkat::MissingTargetPolicy::Skip,
            labels: Default::default(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("a mob schedule");
        Occurrence::planned_from_schedule(
            &schedule,
            meerkat::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("the occurrence plans")
    }

    /// The durable external-delivery record a first attempt leaves in mob
    /// `ops`: its Begin under the occurrence's stable identity, then its
    /// completed terminal.
    #[cfg(feature = "mob")]
    async fn record_completed_ops_delivery(
        mob_state: &meerkat_mob_mcp::MobMcpState,
        identity: &ScheduleDeliveryIdentity,
        target_kind: meerkat_mob::MobExternalDeliveryTargetKind,
        canonical_action: &str,
    ) {
        let intent = meerkat_mob::MobExternalDeliveryIntent::new(
            meerkat_mob::MobId::from("ops"),
            meerkat_mob::MobExternalDeliveryIdentity::new(
                identity.idempotency_key.clone(),
                identity.correlation_id.clone(),
            )
            .expect("the occurrence's delivery identity"),
            target_kind,
            canonical_action.as_bytes(),
        )
        .expect("the first attempt's intent");
        assert_eq!(
            mob_state
                .mob_begin_external_delivery(&intent)
                .await
                .expect("the first attempt's Begin"),
            meerkat_mob::MobExternalDeliveryBeginOutcome::Begun
        );
        mob_state
            .mob_complete_external_delivery(
                &intent,
                &meerkat_mob::MobExternalDeliveryTerminal::Completed,
            )
            .await
            .expect("the first attempt's terminal");
    }

    /// The RPC host's mob host forwards a redelivery to the mob host it
    /// wraps, so a late one reads the mob's durable external-delivery
    /// record: a mob target and a mob member's identity whose first attempt
    /// mob `ops` recorded answer from that record (the mob has no member, so
    /// only the record can complete them). The trait defaults the wrapper
    /// fell back to answered both not admitted without the read.
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn late_redeliveries_read_the_mob_record_through_the_rpc_mob_host() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (_, bundle) = meerkat::open_realm_persistence_in(
            temp.path(),
            "schedule-mob-redelivery",
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
        )
        .await
        .expect("realm persistence");
        let config_store: Arc<dyn meerkat_core::ConfigStore> =
            Arc::new(meerkat_core::MemoryConfigStore::new(
                meerkat_core::Config::default(),
                meerkat_models::canonical(),
            ));
        let runtime = Arc::new(
            SessionRuntime::new_with_config_store(
                meerkat::AgentFactory::new(temp.path().join("sessions")),
                meerkat_core::Config::default(),
                config_store,
                1,
                bundle,
                crate::router::NotificationSink::noop(),
            )
            .expect("compose the RPC session runtime"),
        );
        let mob_state = meerkat_mob_mcp::MobMcpState::new_in_memory();
        let definition: meerkat_mob::MobDefinition = serde_json::from_value(serde_json::json!({
            "id": "ops",
            "profiles": {
                "worker": {
                    "model": "gpt-5.4",
                    "external_addressable": true,
                    "tools": { "comms": true }
                }
            }
        }))
        .expect("test mob definition should decode");
        mob_state
            .mob_create_definition(definition)
            .await
            .expect("the mob that recorded the first attempts");
        runtime.set_mob_state(Arc::clone(&mob_state));
        let host = rpc_schedule_mob_host(&runtime);

        let mob_target = meerkat::MobTargetBinding::Member {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            action: meerkat::ScheduledMobAction::Send {
                content: "Check deploy state.".to_string().into(),
                render_metadata: None,
            },
        };
        let occurrence =
            scheduled_occurrence(meerkat::TargetBinding::Mob(Box::new(mob_target.clone())));
        let identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        record_completed_ops_delivery(
            &mob_state,
            &identity,
            meerkat_mob::MobExternalDeliveryTargetKind::MemberSend,
            &mob_target.stable_key().expect("stable mob target key"),
        )
        .await;
        let answer = host
            .deliver_existing_mob_target(
                &occurrence,
                &identity,
                &mob_target,
                meerkat::ExistingDelivery::UnprovenLate,
            )
            .await
            .expect("the mob host's record read answers");
        let meerkat::ExistingDeliveryAnswer::Dispatch(dispatch) = answer else {
            panic!("a recorded mob delivery replays from its record");
        };
        assert_eq!(
            dispatch.receipt.stage,
            meerkat::DeliveryReceiptStage::DispatchAccepted
        );
        assert_eq!(
            dispatch.completion.await.expect("replay completion").phase,
            meerkat::OccurrencePhase::Completed
        );

        let identity_target = IdentityTargetBinding::resumable(
            meerkat::surface::mob_member_schedule_identity(&meerkat_core::MobMemberBinding {
                mob_id: "ops".to_string(),
                role: "worker".to_string(),
                member: "deploy-monitor".to_string(),
            }),
            meerkat::ScheduledSessionAction::Prompt {
                prompt: "Check deploy state.".to_string().into(),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        );
        let occurrence = scheduled_occurrence(meerkat::TargetBinding::Identity(Box::new(
            identity_target.clone(),
        )));
        let identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        record_completed_ops_delivery(
            &mob_state,
            &identity,
            meerkat_mob::MobExternalDeliveryTargetKind::MemberSend,
            &identity_target
                .stable_key()
                .expect("stable identity target key"),
        )
        .await;
        let answer = host
            .deliver_existing_identity_target(
                &occurrence,
                &identity,
                &identity_target,
                meerkat::ExistingDelivery::UnprovenLate,
            )
            .await
            .expect("the mob host's record read answers")
            .expect("the mob host owns a mob member's identity");
        let meerkat::ExistingDeliveryAnswer::Dispatch(dispatch) = answer else {
            panic!("a recorded identity delivery replays from its record");
        };
        assert_eq!(
            dispatch.receipt.stage,
            meerkat::DeliveryReceiptStage::DispatchAccepted
        );
        assert_eq!(
            dispatch.completion.await.expect("replay completion").phase,
            meerkat::OccurrencePhase::Completed
        );

        runtime.try_shutdown().await.expect("shutdown");
    }

    /// A scheduled fire into a session that another runtime over the same
    /// SQLite realm is running, its durable run held after a round commit:
    /// the probe and the materialized-session reuse read presence and archive
    /// state, and the owner identity reads committed metadata. Each read the
    /// committed body before, which refuses while the run's provisional tail
    /// stands.
    #[tokio::test(flavor = "multi_thread")]
    async fn rpc_schedule_fire_into_a_session_running_elsewhere() {
        use super::super::committed_reads_tests::{HeldRun, mid_run, sqlite_runtime};

        let held = HeldRun::start(true).await;
        let reader = sqlite_runtime(held.root.path()).await;
        let adapter = RpcScheduleTargetAdapter::new(&reader);
        let binding = SessionTargetBinding::ResumableSession {
            session_id: held.session_id.clone(),
            action: meerkat::ScheduledSessionAction::Prompt {
                prompt: meerkat_core::ContentInput::Text("scheduled".to_string()),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        };
        let probe = mid_run(adapter.probe_session_target(&binding)).await;
        assert!(
            matches!(probe, Ok(TargetProbeOutcome::Ready)),
            "the session running elsewhere probes present: {probe:?}"
        );
        let identity = mid_run(adapter.recover_session_target_identity(&binding)).await;
        assert!(
            matches!(identity, Ok(None)),
            "an ordinary session's owner identity reads from its committed metadata: {identity:?}"
        );
        let materialized = mid_run(
            reader.materialize_scheduled_session(&held.session_id, &materialization_spec()),
        )
        .await;
        assert_eq!(
            materialized.as_ref().ok(),
            Some(&held.session_id),
            "the occurrence reuses the running session: {materialized:?}"
        );
        let (result, _held) = held.release_and_finish().await;
        assert_eq!(result.expect("the held run completes").text, "held answer");
    }
}
