use std::sync::Arc;

use async_trait::async_trait;

#[cfg(feature = "comms")]
use super::configure_peer_ingress;
use super::schedule_host::{
    ScheduleAdmissionFence, accept_fenced_schedule_runtime_input_with_reconciliation,
    schedule_runtime_correlation_id, schedule_runtime_delivery_idempotency_key,
};
use super::{
    NoopScheduleMobHost, PersistentRuntimeExecutor, ScheduledEventDispatch,
    ScheduledPromptDispatch, SharedScheduleTargetAdapter, SurfaceScheduleMobHost,
    SurfaceScheduleSessionHost, SurfaceSessionRecoveryContext, SurfaceSessionRecoveryOverrides,
    build_recovered_session, materialize_session_with_reserved_admission_and_actor_slot,
    recover_mob_member_identity_from_session_target, schedule_host_supported, spawn_schedule_host,
};
use crate::{
    Config, CreateSessionRequest, PersistentSessionService, ScheduleDomainError, ScheduleService,
    Session, SessionAgentBuilder, SessionMaterializationSpec, SessionService, SessionTargetBinding,
    TargetProbeOutcome, WorkGraphService,
};
use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions};
use meerkat_core::types::{ContentInput, SessionId};
use meerkat_runtime::MeerkatMachine;
use meerkat_schedule::{IdentityTargetBinding, ScheduleDeliveryIdentity, ScheduleRunnableHost};

/// Spawn a runtime-backed schedule host that delivers session targets
/// through `service` and reports mob targets as unsupported.
///
/// The host's session host holds `service` and judges `schedule_service`'s
/// identity targets only while the host runs; no clone of the schedule
/// service keeps it after the host stops. See [`spawn_schedule_host`].
///
/// In a stock-governed realm, build `schedule_service` with
/// [`crate::PersistenceBundle::schedule_service`]: this host composes no
/// scheduled-session agent binding, so it then refuses each occurrence as
/// `schedule_mandate_unavailable` when it is delivered, after the driver's
/// target probe, before the target is resolved for delivery or a session is
/// materialized. A service from [`ScheduleService::new`] over a governed
/// realm's store commissions no agent, so its schedules fire unchecked and
/// each fire materializes its session and then fails late (see Known issues).
#[allow(clippy::too_many_arguments)]
pub fn spawn_runtime_backed_schedule_host<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
    runtime_adapter: Arc<MeerkatMachine>,
    config: Config,
    schedule_service: ScheduleService,
    build_template: SessionBuildOptions,
    runnable_host: Option<Arc<dyn ScheduleRunnableHost>>,
    workgraph_service: Option<WorkGraphService>,
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
        runnable_host,
        workgraph_service,
        owner_id,
    )
}

/// Spawn a runtime-backed schedule host with an explicit mob delivery adapter.
///
/// Embedders that maintain mob state should pass a real
/// [`SurfaceScheduleMobHost`] here, for example the `meerkat-mob-mcp`
/// schedule host adapter. The default [`spawn_runtime_backed_schedule_host`]
/// remains session-only and reports explicit mob target failures.
///
/// `runnable_host` attaches the host's [`ScheduleRunnableHost`] (typically a
/// `HostRunnableRegistry`) so `HostRunnable` schedule targets dispatch
/// through this host's occurrence driver; `None` leaves host-runnable
/// targets probing as missing (the schedule's `MissingTargetPolicy` then
/// applies).
///
/// `service` (through the host's session host) and `mob_host` judge
/// `schedule_service`'s identity targets only while the host runs; no clone
/// of the schedule service, such as one inside agent schedule tools in the
/// factory builder's `default_schedule_tools` slot, keeps them after the
/// host stops. See [`spawn_schedule_host`].
#[allow(clippy::too_many_arguments)]
pub fn spawn_runtime_backed_schedule_host_with_mobs<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
    runtime_adapter: Arc<MeerkatMachine>,
    _config: Config,
    schedule_service: ScheduleService,
    build_template: SessionBuildOptions,
    mob_host: Arc<dyn SurfaceScheduleMobHost>,
    runnable_host: Option<Arc<dyn ScheduleRunnableHost>>,
    workgraph_service: Option<WorkGraphService>,
    owner_id: impl Into<String>,
) -> Option<super::ScheduleHostHandle> {
    spawn_schedule_session_host(
        RuntimeBackedScheduleSessionHost::new(
            service,
            runtime_adapter,
            build_template,
            workgraph_service,
        ),
        schedule_service,
        mob_host,
        runnable_host,
        owner_id,
    )
}

/// [`spawn_runtime_backed_schedule_host_with_mobs`] for a host whose delivery
/// owners subscribe to `continuations`.
///
/// Each time this host attaches or reuses a scheduled session's exact
/// executor attachment and completes its admission, it announces
/// [`crate::ContinuationHostBindings::source_custody_changed`], so the
/// delivery owners retry the rows they hold blocked. The announcement grants
/// nothing: they read the actual owners again. Teardown is as for
/// [`spawn_runtime_backed_schedule_host_with_mobs`].
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
pub fn spawn_runtime_backed_schedule_host_with_mobs_and_continuations<
    B: SessionAgentBuilder + 'static,
>(
    service: Arc<PersistentSessionService<B>>,
    runtime_adapter: Arc<MeerkatMachine>,
    _config: Config,
    schedule_service: ScheduleService,
    build_template: SessionBuildOptions,
    mob_host: Arc<dyn SurfaceScheduleMobHost>,
    runnable_host: Option<Arc<dyn ScheduleRunnableHost>>,
    workgraph_service: Option<WorkGraphService>,
    continuations: Arc<crate::ContinuationHostBindings>,
    owner_id: impl Into<String>,
) -> Option<super::ScheduleHostHandle> {
    spawn_schedule_session_host(
        RuntimeBackedScheduleSessionHost::new(
            service,
            runtime_adapter,
            build_template,
            workgraph_service,
        )
        .with_continuations(continuations),
        schedule_service,
        mob_host,
        runnable_host,
        owner_id,
    )
}

fn spawn_schedule_session_host<B: SessionAgentBuilder + 'static>(
    session_host: RuntimeBackedScheduleSessionHost<B>,
    schedule_service: ScheduleService,
    mob_host: Arc<dyn SurfaceScheduleMobHost>,
    runnable_host: Option<Arc<dyn ScheduleRunnableHost>>,
    owner_id: impl Into<String>,
) -> Option<super::ScheduleHostHandle> {
    if !schedule_host_supported(schedule_service.store().kind()) {
        return None;
    }

    let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(session_host);
    let adapter =
        SharedScheduleTargetAdapter::new(schedule_service.clone(), session_host, mob_host);
    let adapter = match runnable_host {
        Some(runnable_host) => adapter.with_runnable_host(runnable_host),
        None => adapter,
    };
    Some(spawn_schedule_host(
        schedule_service,
        Arc::new(adapter),
        owner_id,
    ))
}

struct RuntimeBackedScheduleSessionHost<B: SessionAgentBuilder> {
    service: Arc<PersistentSessionService<B>>,
    runtime_adapter: Arc<MeerkatMachine>,
    build_template: SessionBuildOptions,
    workgraph_service: Option<WorkGraphService>,
    /// Told after each completed ordinary admission; see
    /// [`spawn_runtime_backed_schedule_host_with_mobs_and_continuations`].
    #[cfg(not(target_arch = "wasm32"))]
    continuations: Option<Arc<crate::ContinuationHostBindings>>,
}

#[derive(Debug)]
enum ScheduledSessionAdmission {
    Ordinary,
    MobOwned {
        attachment: meerkat_runtime::RuntimeExecutorAttachmentWitness,
        expected_member:
            Option<meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation>,
    },
}

impl ScheduledSessionAdmission {
    /// The fence [`RuntimeBackedScheduleSessionHost::accept_scheduled_input`]
    /// admits under, which a redelivery's existing-only admission keeps.
    fn fence(&self) -> ScheduleAdmissionFence<'_> {
        match self {
            Self::Ordinary => ScheduleAdmissionFence::Unfenced,
            Self::MobOwned {
                attachment,
                expected_member,
            } => ScheduleAdmissionFence::AttachmentAndMemberResidency {
                attachment,
                expected_member: expected_member.as_ref(),
            },
        }
    }
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

impl<B: SessionAgentBuilder + 'static> RuntimeBackedScheduleSessionHost<B> {
    fn new(
        service: Arc<PersistentSessionService<B>>,
        runtime_adapter: Arc<MeerkatMachine>,
        build_template: SessionBuildOptions,
        workgraph_service: Option<WorkGraphService>,
    ) -> Self {
        Self {
            service,
            runtime_adapter,
            build_template,
            workgraph_service,
            #[cfg(not(target_arch = "wasm32"))]
            continuations: None,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn with_continuations(mut self, continuations: Arc<crate::ContinuationHostBindings>) -> Self {
        self.continuations = Some(continuations);
        self
    }

    fn recovered_schedule_request(
        &self,
        session: Session,
    ) -> Result<CreateSessionRequest, ScheduleDomainError> {
        let recovered = build_recovered_session(
            session,
            &SurfaceSessionRecoveryOverrides::default(),
            SurfaceSessionRecoveryContext {
                llm_client_override: self.build_template.llm_client_override.clone(),
                agent_llm_client_decorator: self.build_template.agent_llm_client_decorator.clone(),
                external_tools: self.build_template.external_tools.clone(),
                checkpointer: None,
                // The materialization transaction replaces this placeholder
                // with its exact machine-issued SessionOwned bindings.
                runtime_build_mode: meerkat_core::RuntimeBuildMode::StandaloneEphemeral,
                realm_id: self.build_template.realm_id.clone(),
                instance_id: self.build_template.instance_id.clone(),
                backend: self
                    .build_template
                    .backend
                    .as_ref()
                    .map(|backend| backend.as_str().to_string()),
                config_generation: self.build_template.config_generation,
            },
        )
        .map_err(schedule_internal)?;
        Ok(recovered.into_deferred_create_request())
    }

    async fn ensure_runtime_session_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<ScheduledSessionAdmission, ScheduleDomainError> {
        self.ensure_session_target_exists(session_id).await?;

        // Rounds never change the session metadata read here, so a run that
        // holds a round commit is served the committed snapshot.
        let session = self
            .service
            .load_committed_session_tolerating_tail(session_id)
            .await
            .map_err(schedule_internal)?
            .ok_or_else(|| {
                ScheduleDomainError::InvalidSchedule(format!("session not found: {session_id}"))
            })?;
        let mob_member_binding = session
            .session_metadata()
            .and_then(|metadata| metadata.mob_member_binding);
        let admission = if let Some(binding) = mob_member_binding.as_ref() {
            // A mob member's executor and exact incarnation are owned by the
            // mob host materializer. The generic schedule surface may use an
            // already-serving attachment, but must never reconstruct or
            // replace it with ordinary runtime plumbing.
            let incarnation = self.runtime_adapter.member_incarnation(session_id);
            if incarnation.as_ref().is_some_and(|incarnation| {
                incarnation.mob_id != binding.mob_id
                    || incarnation.agent_identity != binding.member
                    || incarnation.member_session_id != session_id.to_string()
            }) {
                return Err(ScheduleDomainError::Internal(format!(
                    "mob-owned session {session_id} is serving an incarnation that does not match its durable owner {}/{}",
                    binding.mob_id, binding.member
                )));
            }
            let attachment = self
                .runtime_adapter
                .current_executor_attachment_witness(session_id)
                .await
                .ok_or_else(|| {
                    ScheduleDomainError::Internal(format!(
                        "mob-owned session {session_id} has no exact executor attachment"
                    ))
                })?;
            ScheduledSessionAdmission::MobOwned {
                attachment,
                // `None` deliberately means true PeerOnly at admission, not
                // "unfenced". The machine rejects VacantPlaced, preserving
                // local in-process mob delivery while refusing a failed or
                // not-yet-revived remote placement.
                expected_member: incarnation,
            }
        } else {
            // B stabilizes the exact service actor while the machine creates
            // or observes its M-owned attachment. A fresh actor slot is used
            // even for an already-live actor so the executor never receives a
            // logical SessionId publication capability.
            let turn_boundary = self
                .service
                .acquire_runtime_turn_finalization_guard(session_id)
                .await;
            if let Some(actor_witness) = self.service.live_session_actor_witness(session_id).await {
                let actor_witness_slot = crate::LiveSessionActorWitnessSlot::default();
                actor_witness_slot
                    .publish(actor_witness)
                    .map_err(schedule_internal)?;
                let service = Arc::clone(&self.service);
                let runtime_adapter = Arc::clone(&self.runtime_adapter);
                let workgraph_service = self.workgraph_service.clone();
                match self
                    .runtime_adapter
                    .ensure_session_with_executor_factory_under_runtime_turn_finalization_boundary(
                        session_id.clone(),
                        move |attachment_witness| {
                            Box::new(
                                PersistentRuntimeExecutor::new_for_actor_slot_and_attachment(
                                    service,
                                    runtime_adapter,
                                    attachment_witness,
                                    actor_witness_slot,
                                    workgraph_service,
                                ),
                            )
                        },
                    )
                    .await
                    .map_err(schedule_internal)?
                {
                    meerkat_runtime::EnsureRuntimeExecutorAttachment::Pending(mut pending) => {
                        if let Err(commit_error) = pending
                            .try_commit_with_under_runtime_turn_finalization_boundary(false, |_| {
                                Ok(())
                            })
                            .await
                        {
                            let cleanup_error = pending
                                .abort_under_runtime_turn_finalization_boundary()
                                .await
                                .err();
                            return Err(schedule_internal(match cleanup_error {
                                Some(cleanup_error) => format!(
                                    "{commit_error}; boundary-aware schedule attachment cleanup also failed: {cleanup_error}"
                                ),
                                None => commit_error.to_string(),
                            }));
                        }
                    }
                    meerkat_runtime::EnsureRuntimeExecutorAttachment::Existing(witness) => {
                        self.runtime_adapter
                            .publication_handle_for_executor_attachment(&witness)
                            .await
                            .map_err(schedule_internal)?;
                    }
                }
                drop(turn_boundary);
            } else {
                // No service actor exists. Release the sampled B and rebuild
                // through the canonical materialization transaction, which
                // reacquires B before M and either refreshes the existing
                // attachment's immutable handle or attaches a slot-bound
                // executor in a cold process.
                drop(turn_boundary);
                let request = self.recovered_schedule_request(session.clone())?;
                let reserved_admission = self
                    .service
                    .reserve_create_session_admission()
                    .await
                    .map_err(schedule_internal)?;
                let service = Arc::clone(&self.service);
                let runtime_adapter = Arc::clone(&self.runtime_adapter);
                let workgraph_service = self.workgraph_service.clone();
                materialize_session_with_reserved_admission_and_actor_slot(
                    &self.service,
                    &self.runtime_adapter,
                    session,
                    request,
                    reserved_admission,
                    move |_session_id, attachment_witness, actor_witness_slot| {
                        Box::new(
                            PersistentRuntimeExecutor::new_for_actor_slot_and_attachment(
                                service,
                                runtime_adapter,
                                attachment_witness,
                                actor_witness_slot,
                                workgraph_service,
                            ),
                        )
                    },
                )
                .await
                .map_err(schedule_internal)?;
            }
            ScheduledSessionAdmission::Ordinary
        };
        self.ensure_schedule_peer_ingress_for_admission(session_id, &admission)
            .await?;
        // An ordinary admission attached or reused this host's exact
        // attachment and checked its publication authority. A mob-owned one
        // only consumes the mob's attachment, so it announces nothing.
        #[cfg(not(target_arch = "wasm32"))]
        if matches!(admission, ScheduledSessionAdmission::Ordinary)
            && let Some(continuations) = &self.continuations
        {
            continuations.source_custody_changed();
        }
        Ok(admission)
    }

    async fn ensure_schedule_peer_ingress_for_admission(
        &self,
        session_id: &SessionId,
        admission: &ScheduledSessionAdmission,
    ) -> Result<(), ScheduleDomainError> {
        if let ScheduledSessionAdmission::MobOwned {
            expected_member, ..
        } = admission
        {
            // Mob-owned sessions already carry the transport selected by
            // their owning actor. Scheduling may consume that transport but
            // must never attach or refresh generic SessionOwned plumbing.
            #[cfg(feature = "comms")]
            if let Some(expected_member) = expected_member {
                return match self.runtime_adapter.peer_ingress_owner(session_id).await {
                    meerkat_runtime::PeerIngressOwner::MobOwned { mob_id, .. }
                        if mob_id.0 == expected_member.mob_id =>
                    {
                        Ok(())
                    }
                    owner => Err(ScheduleDomainError::Internal(format!(
                        "mob-owned session {session_id} expected peer ingress for mob '{}', found {owner:?}",
                        expected_member.mob_id
                    ))),
                };
            }
            #[cfg(not(feature = "comms"))]
            let _ = expected_member;
            return Ok(());
        }
        self.ensure_schedule_peer_ingress(session_id).await
    }

    async fn accept_scheduled_input(
        &self,
        session_id: &SessionId,
        input: meerkat_runtime::Input,
        admission: &ScheduledSessionAdmission,
    ) -> Result<
        (
            meerkat_runtime::AcceptOutcome,
            Option<meerkat_runtime::CompletionHandle>,
        ),
        ScheduleDomainError,
    > {
        match admission {
            ScheduledSessionAdmission::Ordinary => {
                self.runtime_adapter
                    .accept_input_with_completion(session_id, input)
                    .await
            }
            ScheduledSessionAdmission::MobOwned {
                attachment,
                expected_member,
            } => {
                self.runtime_adapter
                    .accept_input_with_completion_for_attachment_and_member_residency(
                        attachment,
                        input,
                        expected_member.as_ref(),
                    )
                    .await
            }
        }
        .map_err(|error| ScheduleDomainError::Internal(error.to_string()))
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

    #[cfg(feature = "comms")]
    async fn detach_schedule_peer_ingress(
        &self,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        self.runtime_adapter
            .update_peer_ingress_context(session_id, false, None)
            .await
            .map_err(schedule_internal)?;
        Ok(())
    }

    #[cfg(feature = "comms")]
    async fn session_keep_alive(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, ScheduleDomainError> {
        let session = self
            .service
            .load_committed_session_tolerating_tail(session_id)
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

    async fn update_peer_ingress_context(
        &self,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        #[cfg(feature = "comms")]
        {
            let session = self
                .service
                .load_committed_session_tolerating_tail(session_id)
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

    async fn session_target_owner_identity(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, ScheduleDomainError> {
        let Some(session) = self
            .service
            .load_committed_session_tolerating_tail(session_id)
            .await
            .map_err(schedule_internal)?
        else {
            return Ok(None);
        };
        Ok(Some(session))
    }

    fn build_materialized_request(
        &self,
        create: &SessionMaterializationSpec,
    ) -> Result<CreateSessionRequest, ScheduleDomainError> {
        let build = materialized_build_options(&self.build_template, create)?;

        Ok(CreateSessionRequest {
            injected_context: Vec::new(),
            model: create.model.clone(),
            prompt: ContentInput::Text(String::new()),
            system_prompt: match create.system_prompt.clone() {
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
    /// Its prompt and event deliveries admit through
    /// `accept_fenced_schedule_runtime_input_with_reconciliation` on the
    /// calling task, under the fence of their own admission.
    fn decides_existing_admission(&self) -> bool {
        true
    }

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

    async fn recover_session_target_identity(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<Option<IdentityTargetBinding>, ScheduleDomainError> {
        let SessionTargetBinding::ResumableSession { session_id, .. } = binding else {
            return Ok(None);
        };
        let session = self.session_target_owner_identity(session_id).await?;
        Ok(recover_mob_member_identity_from_session_target(
            binding,
            session.as_ref(),
        ))
    }

    async fn materialize_session(
        &self,
        occurrence: &crate::Occurrence,
        create: &SessionMaterializationSpec,
    ) -> Result<SessionId, ScheduleDomainError> {
        let request = self.build_materialized_request(create)?;
        let keep_alive = request.build.as_ref().is_some_and(|build| build.keep_alive);
        // Layer A: derive the materialized SessionId deterministically from the
        // occurrence identity (NOT attempt_count) so a redrive reuses the same
        // durable session via the create-or-reuse `resume_session` recovery
        // path in `materialize_session`, instead of minting a fresh random id
        // that would orphan the prior session on a crash/lease-expiry/cancel
        // landing inside the materialize->bind window.
        let session = Session::with_id(occurrence.materialized_session_id());
        let reserved_admission = self
            .service
            .reserve_create_session_admission()
            .await
            .map_err(schedule_internal)?;
        let result = Box::pin(materialize_session_with_reserved_admission_and_actor_slot(
            &self.service,
            &self.runtime_adapter,
            session,
            request,
            reserved_admission,
            {
                let service = Arc::clone(&self.service);
                let runtime_adapter = Arc::clone(&self.runtime_adapter);
                let workgraph_service = self.workgraph_service.clone();
                move |_session_id, attachment_witness, actor_witness_slot| {
                    Box::new(
                        PersistentRuntimeExecutor::new_for_actor_slot_and_attachment(
                            service,
                            runtime_adapter,
                            attachment_witness,
                            actor_witness_slot,
                            workgraph_service,
                        ),
                    )
                }
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
        identity: &ScheduleDeliveryIdentity,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<crate::DeliveryDispatch, ScheduleDomainError> {
        let admission = self.ensure_runtime_session_registered(session_id).await?;

        let turn_metadata = meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
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
            additional_instructions: (!dispatch.additional_instructions.is_empty()).then(|| {
                dispatch
                    .additional_instructions
                    .iter()
                    .map(
                        |body| meerkat_core::lifecycle::run_primitive::TurnInstruction {
                            kind: meerkat_core::lifecycle::run_primitive::TurnInstructionKind::Host,
                            body: body.clone(),
                        },
                    )
                    .collect()
            }),
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
        };
        // The attention overlay is deliberately NOT composed here: a queued
        // prompt can sit behind a running turn that mutates the work item,
        // and a projection snapshotted at enqueue time would fail exact-
        // currency validation at apply. The runtime executor (wired with
        // this host's workgraph service) injects a fresh projection at
        // apply time instead.
        let mut prompt_input =
            meerkat_runtime::PromptInput::from_content_input(dispatch.prompt, Some(turn_metadata));
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

        accept_fenced_schedule_runtime_input_with_reconciliation(
            self.runtime_adapter.as_ref(),
            session_id,
            occurrence,
            identity,
            meerkat_runtime::Input::Prompt(prompt_input),
            dispatch.materialized_session_id,
            admission.fence(),
            |input| self.accept_scheduled_input(session_id, input, &admission),
        )
        .await
    }

    async fn deliver_event(
        &self,
        session_id: &SessionId,
        occurrence: &crate::Occurrence,
        identity: &ScheduleDeliveryIdentity,
        dispatch: ScheduledEventDispatch,
    ) -> Result<crate::DeliveryDispatch, ScheduleDomainError> {
        let admission = self.ensure_runtime_session_registered(session_id).await?;

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

        accept_fenced_schedule_runtime_input_with_reconciliation(
            self.runtime_adapter.as_ref(),
            session_id,
            occurrence,
            identity,
            input,
            dispatch.materialized_session_id,
            admission.fence(),
            |input| self.accept_scheduled_input(session_id, input, &admission),
        )
        .await
    }
}

fn schedule_internal(error: impl std::fmt::Display) -> ScheduleDomainError {
    ScheduleDomainError::Internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        not(target_arch = "wasm32")
    ))]
    use crate::surface::{default_persistent_executor, materialize_session};
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

    fn sample_materialization_spec() -> SessionMaterializationSpec {
        SessionMaterializationSpec {
            model: "gpt-5.4".to_string(),
            system_prompt: Some("scheduled exact publication".to_string()),
            max_tokens: None,
            provider: None,
            output_schema: None,
            structured_output_retries: None,
            provider_params: None,
            comms_name: Some("scheduled-exact-publication".to_string()),
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

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    fn build_announcing_schedule_host(
        temp: &tempfile::TempDir,
    ) -> (
        RuntimeBackedScheduleSessionHost<crate::FactoryAgentBuilder>,
        Arc<PersistentSessionService<crate::FactoryAgentBuilder>>,
        Arc<crate::ContinuationHostBindings>,
    ) {
        let persistence = crate::PersistenceBundle::new(
            Arc::new(meerkat_store::MemoryStore::new()) as Arc<dyn crate::SessionStore>,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new())
                as Arc<dyn meerkat_runtime::RuntimeStore>,
            Arc::new(crate::MemoryBlobStore::new()),
        )
        .expect("construct runtime authority");
        let continuations = persistence.continuation_bindings();
        let mut builder = crate::FactoryAgentBuilder::new(
            crate::AgentFactory::new(temp.path().join("sessions")),
            crate::Config::default(),
        );
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        )));
        let (service, runtime_adapter) =
            crate::surface::build_runtime_backed_service(builder, 4, persistence)
                .expect("compose the runtime-backed service");
        let service = Arc::new(service);
        let host = RuntimeBackedScheduleSessionHost::new(
            Arc::clone(&service),
            runtime_adapter,
            SessionBuildOptions::default(),
            None,
        )
        .with_continuations(Arc::clone(&continuations));
        (host, service, continuations)
    }

    /// A planned first attempt (ordinal `ordinal`) whose target is the exact
    /// session `session_id`.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    fn exact_session_occurrence(session_id: &SessionId, ordinal: u64) -> crate::Occurrence {
        let schedule = meerkat_schedule::Schedule::new(meerkat_schedule::CreateScheduleRequest {
            name: Some("runtime-host-existing-admission".to_string()),
            description: None,
            trigger: meerkat_schedule::TriggerSpec::Interval(
                meerkat_schedule::IntervalTriggerSpec {
                    start_at_utc: chrono::Utc::now(),
                    every_seconds: 60,
                    end_at_utc: None,
                },
            ),
            target: crate::TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id: session_id.clone(),
                action: meerkat_schedule::ScheduledSessionAction::Prompt {
                    prompt: ContentInput::Text("scheduled prompt".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }),
            misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
            overlap_policy: meerkat_schedule::OverlapPolicy::SkipIfRunning,
            missing_target_policy: meerkat_schedule::MissingTargetPolicy::MarkMisfired,
            labels: Default::default(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("schedule creation should pass generated authority");
        let mut occurrence = meerkat_schedule::Occurrence::planned_from_schedule(
            &schedule,
            meerkat_schedule::OccurrenceOrdinal(ordinal),
            chrono::Utc::now(),
        )
        .expect("occurrence planning should pass generated authority");
        occurrence.attempt_count = 1;
        occurrence
    }

    /// The runtime's durable admission under the occurrence's runtime key.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    async fn durable_admission(
        runtime_adapter: &MeerkatMachine,
        session_id: &SessionId,
        occurrence: &crate::Occurrence,
    ) -> Option<meerkat_core::lifecycle::InputId> {
        use meerkat_runtime::SessionServiceRuntimeExt as _;
        let identity = ScheduleDeliveryIdentity::for_occurrence(occurrence);
        let key = schedule_runtime_delivery_idempotency_key(
            runtime_adapter,
            session_id,
            occurrence,
            &identity.idempotency_key,
        )
        .await
        .expect("the runtime key");
        runtime_adapter
            .durable_input_state_by_idempotency_key(session_id, &key)
            .await
            .expect("the durable index read")
            .map(|stored| stored.state.input_id)
    }

    /// Through the real facade runtime host, a redelivery only finds an
    /// earlier admission: an unproven late one the runtime never admitted
    /// answers not admitted and writes nothing under its key; an admitted
    /// one replays the earlier input (deduplicated, the same input); an
    /// admitted one the runtime does not know ends delivery_failed with
    /// nothing admitted. The adapter's scope witness stops a debug build if
    /// the host admits without reading the redelivery scope.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn the_runtime_host_redelivers_only_an_existing_admission() {
        use meerkat_schedule::{
            DeliveryReceiptStage, ExistingDelivery, ExistingDeliveryAnswer, OccurrencePhase,
            RuntimeDeliveryOutcome, ScheduleTargetDelivery as _,
        };
        let temp = tempfile::tempdir().expect("tempdir");
        let (host, _service, _continuations) = build_announcing_schedule_host(&temp);
        let runtime_adapter = Arc::clone(&host.runtime_adapter);
        let mut create = sample_materialization_spec();
        create.comms_name = None;
        let session_id = host
            .materialize_session(&sample_occurrence(), &create)
            .await
            .expect("scheduled materialization");
        assert!(host.decides_existing_admission());
        let adapter = SharedScheduleTargetAdapter::new(
            ScheduleService::new(Arc::new(meerkat_schedule::MemoryScheduleStore::new())),
            Arc::new(host),
            Arc::new(NoopScheduleMobHost::new("mob targets unused here")),
        );

        let late = exact_session_occurrence(&session_id, 0);
        let answer = adapter
            .deliver_existing_occurrence(
                &late,
                &ScheduleDeliveryIdentity::for_occurrence(&late),
                ExistingDelivery::UnprovenLate,
            )
            .await
            .expect("the unproven late redelivery answers");
        assert!(
            matches!(answer, ExistingDeliveryAnswer::NotAdmittedPastDeadline),
            "{answer:?}"
        );
        assert_eq!(
            durable_admission(&runtime_adapter, &session_id, &late).await,
            None,
            "nothing is admitted under the late occurrence's key"
        );

        let first = exact_session_occurrence(&session_id, 1);
        let first_identity = ScheduleDeliveryIdentity::for_occurrence(&first);
        let dispatch = adapter
            .deliver_occurrence(&first, &first_identity)
            .await
            .expect("the ordinary delivery admits the input");
        assert_eq!(
            dispatch.receipt.runtime_outcome,
            Some(RuntimeDeliveryOutcome::AdmissionAccepted)
        );
        let admitted = durable_admission(&runtime_adapter, &session_id, &first)
            .await
            .expect("the ordinary delivery's input is indexed under its key");
        let answer = adapter
            .deliver_existing_occurrence(&first, &first_identity, ExistingDelivery::Admitted)
            .await
            .expect("the admitted redelivery answers");
        let ExistingDeliveryAnswer::Dispatch(replay) = answer else {
            panic!("an admitted redelivery answers with a dispatch: {answer:?}");
        };
        assert_eq!(
            replay.receipt.runtime_outcome,
            Some(RuntimeDeliveryOutcome::AdmissionDeduplicated),
            "the redelivery replays the earlier input"
        );
        assert_eq!(
            durable_admission(&runtime_adapter, &session_id, &first).await,
            Some(admitted),
            "the key still names the earlier input"
        );

        let gone = exact_session_occurrence(&session_id, 2);
        let answer = adapter
            .deliver_existing_occurrence(
                &gone,
                &ScheduleDeliveryIdentity::for_occurrence(&gone),
                ExistingDelivery::Admitted,
            )
            .await
            .expect("the admitted redelivery answers");
        let ExistingDeliveryAnswer::Dispatch(failed) = answer else {
            panic!("an admitted redelivery answers with a dispatch: {answer:?}");
        };
        assert_eq!(failed.receipt.stage, DeliveryReceiptStage::DispatchStarted);
        let terminal = failed.completion.await.expect("the failure terminal");
        assert_eq!(terminal.phase, OccurrencePhase::DeliveryFailed);
        assert!(
            terminal
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("no longer known to the runtime")),
            "{:?}",
            terminal.detail
        );
        assert_eq!(
            durable_admission(&runtime_adapter, &session_id, &gone).await,
            None,
            "nothing is admitted for an input the runtime does not know"
        );
    }

    /// A completed ordinary admission announces the custody change to the
    /// delivery owners once, whether it reused the exact attachment or
    /// rebuilt the actor over it; a refused admission announces nothing.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn a_completed_scheduled_admission_announces_its_custody_once() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (host, service, continuations) = build_announcing_schedule_host(&temp);
        let mut create = sample_materialization_spec();
        create.comms_name = None;
        let session_id = host
            .materialize_session(&sample_occurrence(), &create)
            .await
            .expect("scheduled materialization");
        let source_changes = continuations.subscribe_source_changes();
        let before = *source_changes.borrow();

        let refused = host
            .ensure_runtime_session_registered(&SessionId::new())
            .await
            .expect_err("a missing session target is refused");
        assert!(
            matches!(refused, ScheduleDomainError::InvalidSchedule(_)),
            "{refused:?}"
        );
        assert_eq!(
            *source_changes.borrow(),
            before,
            "a refused admission announces nothing"
        );

        let admission = host
            .ensure_runtime_session_registered(&session_id)
            .await
            .expect("the live actor reuses its exact attachment");
        assert!(matches!(admission, ScheduledSessionAdmission::Ordinary));
        assert_eq!(
            *source_changes.borrow(),
            before + 1,
            "a reused attachment announces its custody once"
        );

        let turn_boundary = service
            .acquire_runtime_turn_finalization_guard(&session_id)
            .await;
        service
            .discard_live_session_under_runtime_turn_boundary(&session_id)
            .await
            .expect("discard the live actor while keeping the attachment");
        drop(turn_boundary);
        host.ensure_runtime_session_registered(&session_id)
            .await
            .expect("the cold target is rebuilt over its exact attachment");
        assert_eq!(
            *source_changes.borrow(),
            before + 2,
            "a rebuilt admission announces its custody once"
        );
    }

    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        not(target_arch = "wasm32")
    ))]
    #[tokio::test]
    async fn schedule_actor_recovery_refreshes_exact_terminal_publication_handle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service_runtime = Arc::new(
            crate::CommsRuntime::inproc_only("schedule-exact-publication-service")
                .expect("service comms runtime"),
        );
        let (service, runtime_adapter) =
            build_comms_test_service(&temp, Arc::clone(&service_runtime)).await;
        let host = RuntimeBackedScheduleSessionHost::new(
            Arc::clone(&service),
            Arc::clone(&runtime_adapter),
            SessionBuildOptions::default(),
            None,
        );
        let occurrence = sample_occurrence();
        let session_id = host
            .materialize_session(&occurrence, &sample_materialization_spec())
            .await
            .expect("schedule materialization must bind exact actor publication authority");
        let attachment = runtime_adapter
            .current_executor_attachment_witness(&session_id)
            .await
            .expect("scheduled session must retain an exact executor attachment");
        let actor_a_publication = runtime_adapter
            .publication_handle_for_executor_attachment(&attachment)
            .await
            .expect("scheduled attachment must retain actor A publication authority");
        let actor_a = service
            .live_session_actor_witness(&session_id)
            .await
            .expect("scheduled actor A");
        assert!(
            actor_a_publication
                .actor_identity()
                .is_some_and(|named| named.is_same_actor(&actor_a.executor_actor_identity())),
            "the attachment's retained owner names actor A"
        );
        let owner_boundary = actor_a_publication
            .turn_finalization_boundary()
            .expect("the owner lends its session's boundary")
            .acquire()
            .await
            .expect("the owner's boundary");
        {
            let waiting = service.acquire_runtime_turn_finalization_guard(&session_id);
            tokio::pin!(waiting);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(200), &mut waiting)
                    .await
                    .is_err(),
                "the owner's boundary is the service's own per-session boundary"
            );
            drop(owner_boundary);
            drop(
                tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
                    .await
                    .expect("released with the owner's boundary"),
            );
        }

        let turn_boundary = service
            .acquire_runtime_turn_finalization_guard(&session_id)
            .await;
        service
            .discard_live_session_under_runtime_turn_boundary(&session_id)
            .await
            .expect("discard actor A while preserving the scheduled attachment");
        drop(turn_boundary);

        host.ensure_runtime_session_registered(&session_id)
            .await
            .expect("cold scheduled target must reconstruct actor B exactly");
        assert_eq!(
            runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await,
            Some(attachment.clone()),
            "schedule actor recovery must preserve the exact attachment"
        );
        let actor_b_publication = runtime_adapter
            .publication_handle_for_executor_attachment(&attachment)
            .await
            .expect("scheduled attachment must advance to actor B publication authority");
        assert!(
            !Arc::ptr_eq(&actor_a_publication, &actor_b_publication),
            "machine-retained scheduled publication authority must advance from A to B"
        );
        let actor_b = service
            .live_session_actor_witness(&session_id)
            .await
            .expect("scheduled actor B");
        assert!(
            actor_b_publication
                .actor_identity()
                .is_some_and(|named| named.is_same_actor(&actor_b.executor_actor_identity())),
            "the attachment's retained owner advanced to actor B"
        );
        assert!(
            actor_a_publication.actor_identity().is_some_and(|named| {
                !named.is_same_actor(&actor_b.executor_actor_identity()) && !named.is_live()
            }),
            "actor A's owner names only revoked actor A, never B"
        );

        let terminal = meerkat_core::event::AgentEvent::InteractionComplete {
            interaction_id: meerkat_core::interaction::InteractionId(
                meerkat_core::time_compat::new_uuid_v7(),
            ),
            result: "scheduled exact actor B terminal".to_string(),
            structured_output: None,
        };
        actor_a_publication
            .publish_interaction_terminals(std::slice::from_ref(&terminal))
            .await
            .expect_err("revoked scheduled actor A publication must fail closed");
        let receipts = actor_b_publication
            .publish_interaction_terminals(std::slice::from_ref(&terminal))
            .await
            .expect("scheduled actor B publication must converge");
        assert_eq!(receipts.len(), 1);
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
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new())
                as Arc<dyn meerkat_runtime::RuntimeStore>,
            Arc::new(crate::MemoryBlobStore::new()),
        )
        .expect("construct runtime authority");
        let factory = crate::AgentFactory::new(temp.path().join("sessions"))
            .with_comms_runtime(shared_runtime);
        let mut builder = crate::FactoryAgentBuilder::new(factory, crate::Config::default());
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        )));
        let (service, runtime_adapter) =
            crate::surface::build_runtime_backed_service(builder, 4, persistence)
                .expect("compose the runtime-backed service");
        let service = service.with_event_projection(
            Arc::new(meerkat_session::event_store::FileEventStore::new(
                temp.path().join("events"),
            )),
            Arc::new(meerkat_session::projector::SessionProjector::new(
                temp.path().join("projection"),
            )),
        );
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
            injected_context: Vec::new(),
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
    async fn missing_mob_owned_attachment_is_not_rebuilt_by_generic_schedule_runtime() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service_runtime = Arc::new(
            crate::CommsRuntime::inproc_only("schedule-mob-owned-service-runtime")
                .expect("service comms runtime"),
        );
        let (service, runtime_adapter) =
            build_comms_test_service(&temp, Arc::clone(&service_runtime)).await;

        let mut request = make_runtime_backed_request("mob-owned-scheduled-session", true);
        request
            .build
            .as_mut()
            .expect("fixture request carries build options")
            .mob_member_binding = Some(meerkat_core::MobMemberBinding {
            mob_id: "scheduled-mob".to_string(),
            role: "worker".to_string(),
            member: "remote-worker".to_string(),
        });
        let created = service
            .create_session(request)
            .await
            .expect("persist mob-owned session without machine attachment");
        let session_id = created.session_id;
        assert!(
            runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await
                .is_none(),
            "fixture must begin without an executor attachment"
        );

        let host = RuntimeBackedScheduleSessionHost::new(
            Arc::clone(&service),
            Arc::clone(&runtime_adapter),
            SessionBuildOptions::default(),
            None,
        );
        let error = host
            .ensure_runtime_session_registered(&session_id)
            .await
            .expect_err("generic schedule runtime must not claim mob-owned session authority");
        assert!(
            error
                .to_string()
                .contains("has no exact executor attachment"),
            "typed refusal should identify the missing owner: {error}"
        );
        assert!(
            runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await
                .is_none(),
            "failed schedule delivery must leave mob-owned attachment absent"
        );

        service
            .discard_live_session(&session_id)
            .await
            .expect("discard fixture session");
    }

    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        not(target_arch = "wasm32")
    ))]
    #[tokio::test]
    async fn live_peer_only_mob_attachment_is_preserved_and_fenced() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service_runtime = Arc::new(
            crate::CommsRuntime::inproc_only("schedule-local-mob-service-runtime")
                .expect("service comms runtime"),
        );
        let (service, runtime_adapter) =
            build_comms_test_service(&temp, Arc::clone(&service_runtime)).await;

        let mut request = make_runtime_backed_request("local-mob-scheduled-session", true);
        request
            .build
            .as_mut()
            .expect("fixture request carries build options")
            .mob_member_binding = Some(meerkat_core::MobMemberBinding {
            mob_id: "local-scheduled-mob".to_string(),
            role: "worker".to_string(),
            member: "local-worker".to_string(),
        });
        let session = Session::new();
        let result = Box::pin(materialize_session(
            &service,
            &runtime_adapter,
            session,
            request,
            {
                let service = Arc::clone(&service);
                let runtime_adapter = Arc::clone(&runtime_adapter);
                move |session_id| default_persistent_executor(service, runtime_adapter, session_id)
            },
        ))
        .await
        .expect("materialize live peer-only mob session");
        let witness_before = runtime_adapter
            .current_executor_attachment_witness(&result.session_id)
            .await
            .expect("local mob session has a serving attachment");
        assert!(
            runtime_adapter
                .member_incarnation(&result.session_id)
                .is_none(),
            "local in-process mob session is peer-only, not remotely placed"
        );

        let host = RuntimeBackedScheduleSessionHost::new(
            Arc::clone(&service),
            Arc::clone(&runtime_adapter),
            SessionBuildOptions::default(),
            None,
        );
        let admission = host
            .ensure_runtime_session_registered(&result.session_id)
            .await
            .expect("live peer-only mob attachment remains usable");
        assert!(matches!(
            admission,
            ScheduledSessionAdmission::MobOwned {
                expected_member: None,
                ..
            }
        ));
        let occurrence = sample_occurrence();
        let identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        let dispatch = host
            .deliver_prompt(
                &result.session_id,
                &occurrence,
                &identity,
                ScheduledPromptDispatch {
                    prompt: ContentInput::Text("scheduled local mob prompt".to_string()),
                    transient_turn_context: None,
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                    materialized_session_id: None,
                },
            )
            .await
            .expect("live peer-only mob schedule delivery is admitted under its residency fence");
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(2), dispatch.completion)
            .await
            .expect("scheduled local mob prompt should complete")
            .expect("delivery terminal should resolve");
        assert_eq!(
            terminal.runtime_completion_outcome,
            Some(meerkat_schedule::RuntimeCompletionOutcome::Completed)
        );
        let witness_after = runtime_adapter
            .current_executor_attachment_witness(&result.session_id)
            .await
            .expect("local mob attachment remains serving");
        assert_eq!(
            witness_after, witness_before,
            "schedule preparation must not replace a live local mob attachment"
        );

        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live peer-only mob session");
        runtime_adapter
            .unregister_session(&result.session_id)
            .await
            .expect("unregister peer-only mob runtime session");
    }

    /// A live in-process mob member session (peer-only, no placed residency)
    /// on a fresh comms service, built as
    /// `live_peer_only_mob_attachment_is_preserved_and_fenced` builds it.
    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        not(target_arch = "wasm32")
    ))]
    async fn live_peer_only_mob_session(
        temp: &tempfile::TempDir,
        name: &str,
    ) -> (
        Arc<PersistentSessionService<crate::FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
        SessionId,
    ) {
        let service_runtime = Arc::new(
            crate::CommsRuntime::inproc_only(&format!("{name}-service-runtime"))
                .expect("service comms runtime"),
        );
        let (service, runtime_adapter) =
            build_comms_test_service(temp, Arc::clone(&service_runtime)).await;
        let mut request = make_runtime_backed_request(name, true);
        request
            .build
            .as_mut()
            .expect("fixture request carries build options")
            .mob_member_binding = Some(meerkat_core::MobMemberBinding {
            mob_id: format!("{name}-mob"),
            role: "worker".to_string(),
            member: format!("{name}-worker"),
        });
        let result = Box::pin(materialize_session(
            &service,
            &runtime_adapter,
            Session::new(),
            request,
            {
                let service = Arc::clone(&service);
                let runtime_adapter = Arc::clone(&runtime_adapter);
                move |session_id| default_persistent_executor(service, runtime_adapter, session_id)
            },
        ))
        .await
        .expect("materialize a live peer-only mob session");
        (service, runtime_adapter, result.session_id)
    }

    /// A live orchestrator over a fixture's service, addressing sessions as
    /// `session_addressing`, with no live host composed.
    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        feature = "live",
        not(target_arch = "wasm32")
    ))]
    fn fixture_live_orchestrator<'a>(
        service: &'a Arc<PersistentSessionService<crate::FactoryAgentBuilder>>,
        runtime_adapter: &'a Arc<MeerkatMachine>,
        staged_sessions: &'a Arc<crate::StagedSessionRegistry>,
        staged_capacity_admissions: &'a crate::session_runtime::admission::StagedCapacityAdmissions,
        session_addressing: crate::session_runtime::live_orchestration::LiveSessionAddressing,
    ) -> crate::session_runtime::live_orchestration::LiveOrchestrator<'a> {
        crate::session_runtime::live_orchestration::LiveOrchestrator {
            service,
            staged_sessions,
            staged_capacity_admissions,
            runtime_adapter,
            host: None,
            config_runtime: None,
            default_llm_client: None,
            agent_llm_client_decorator: None,
            external_tools: None,
            archive_runtime_cleanup: crate::session_runtime::runtime_state::ArchiveRuntimeCleanup {
                runtime_adapter: Arc::clone(runtime_adapter),
                pending_session_event_streams: None,
                mcp_state: None,
                mob_state: None,
            },
            realm_id: None,
            instance_id: None,
            backend: None,
            ingress_reconciler: None,
            actor_witness_capture: Arc::new(
                |_session_id: SessionId,
                 _actor_witness_slot: crate::LiveSessionActorWitnessSlot| {},
            ),
            session_addressing,
        }
    }

    /// A session-scoped surface opens no live channel on a mob member's
    /// session, live or not, and restores none from storage: the member's
    /// mob opens its channels after checking the member. The open's refusal
    /// is typed (the mob-member kind); a live member session is still found
    /// for the reads behind a channel that exists, and nothing is restored.
    /// The member live host addresses the same session through its mob and
    /// is never refused. Before, a session-scoped open of a live member
    /// session went ahead without any member check.
    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        feature = "live",
        not(target_arch = "wasm32")
    ))]
    #[tokio::test]
    async fn a_session_scoped_surface_opens_no_mob_member_session_live_or_cold() {
        use crate::session_runtime::live_orchestration::LiveSessionAddressing;

        let temp = tempfile::tempdir().expect("tempdir");
        let (service, runtime_adapter, session_id) =
            live_peer_only_mob_session(&temp, "member-open-refusal").await;
        let staged_sessions = Arc::new(crate::StagedSessionRegistry::new());
        let staged_capacity_admissions: crate::session_runtime::admission::StagedCapacityAdmissions =
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let scoped = fixture_live_orchestrator(
            &service,
            &runtime_adapter,
            &staged_sessions,
            &staged_capacity_admissions,
            LiveSessionAddressing::SessionScoped,
        );
        let member_host = fixture_live_orchestrator(
            &service,
            &runtime_adapter,
            &staged_sessions,
            &staged_capacity_admissions,
            LiveSessionAddressing::MobMember,
        );

        let refused = scoped
            .refuse_mob_member_session_by_id(&session_id)
            .await
            .expect_err("a session-scoped open refuses a live member session");
        assert!(
            refused.is_mob_member_session_addressed_by_id(),
            "{refused:?}"
        );
        scoped
            .recover_live_session_for_realtime_open(&session_id)
            .await
            .expect("a live member session is found, not restored");
        member_host
            .refuse_mob_member_session_by_id(&session_id)
            .await
            .expect("the member live host opens through the mob");

        // Cold: the live actor is gone, the durable member session stays.
        service
            .discard_live_session(&session_id)
            .await
            .expect("discard the live member session");
        // Twice: a first refusal that had restored the session would leave
        // it live, and the second attempt would then find it instead.
        for attempt in 0..2 {
            let refused = scoped
                .recover_live_session_for_realtime_open(&session_id)
                .await
                .expect_err("a session-scoped recovery refuses a cold member session");
            assert!(
                refused.is_mob_member_session_addressed_by_id(),
                "attempt {attempt}: {refused:?}"
            );
        }
        if let Err(error) = member_host
            .recover_live_session_for_realtime_open(&session_id)
            .await
        {
            assert!(
                !error.is_mob_member_session_addressed_by_id(),
                "the member live host is never refused as a session-scoped surface: {error:?}"
            );
        }

        let _ = service.discard_live_session(&session_id).await;
        runtime_adapter
            .unregister_session(&session_id)
            .await
            .expect("unregister the member session");
    }

    /// A redelivery to a mob member's session runs its existing-only
    /// admission under the session's own fence (its exact attachment and its
    /// member residency), as the ordinary admission does, and with the fence
    /// intact it finds the earlier admission: deduplicated, the same input.
    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        not(target_arch = "wasm32")
    ))]
    #[tokio::test]
    async fn a_mob_member_redelivery_finds_its_admission_under_the_sessions_fence() {
        use meerkat_schedule::{
            ExistingDelivery, ExistingDeliveryAnswer, RuntimeDeliveryOutcome,
            ScheduleTargetDelivery as _,
        };
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, runtime_adapter, session_id) =
            live_peer_only_mob_session(&temp, "fenced-redelivery").await;
        let host = RuntimeBackedScheduleSessionHost::new(
            Arc::clone(&service),
            Arc::clone(&runtime_adapter),
            SessionBuildOptions::default(),
            None,
        );
        let admission = host
            .ensure_runtime_session_registered(&session_id)
            .await
            .expect("the mob session's attachment is usable");
        assert!(matches!(
            admission.fence(),
            ScheduleAdmissionFence::AttachmentAndMemberResidency {
                expected_member: None,
                ..
            }
        ));
        let adapter = SharedScheduleTargetAdapter::new(
            ScheduleService::new(Arc::new(meerkat_schedule::MemoryScheduleStore::new())),
            Arc::new(host),
            Arc::new(NoopScheduleMobHost::new("mob targets unused here")),
        );
        let first = exact_session_occurrence(&session_id, 1);
        let identity = ScheduleDeliveryIdentity::for_occurrence(&first);
        let dispatch = adapter
            .deliver_occurrence(&first, &identity)
            .await
            .expect("the ordinary delivery admits under the fence");
        assert_eq!(
            dispatch.receipt.runtime_outcome,
            Some(RuntimeDeliveryOutcome::AdmissionAccepted)
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), dispatch.completion)
            .await
            .expect("the scheduled prompt completes")
            .expect("the delivery terminal resolves");
        let admitted = durable_admission(&runtime_adapter, &session_id, &first)
            .await
            .expect("the ordinary delivery's input is indexed under its key");

        let answer = adapter
            .deliver_existing_occurrence(&first, &identity, ExistingDelivery::Admitted)
            .await
            .expect("the admitted redelivery answers");
        let ExistingDeliveryAnswer::Dispatch(replay) = answer else {
            panic!("an admitted redelivery answers with a dispatch: {answer:?}");
        };
        assert_eq!(
            replay.receipt.runtime_outcome,
            Some(RuntimeDeliveryOutcome::AdmissionDeduplicated)
        );
        assert_eq!(
            durable_admission(&runtime_adapter, &session_id, &first).await,
            Some(admitted),
            "the key still names the earlier input"
        );

        service
            .discard_live_session(&session_id)
            .await
            .expect("discard the mob session");
        runtime_adapter
            .unregister_session(&session_id)
            .await
            .expect("unregister the mob session");
    }

    /// A session host whose admission carries a fence that no longer holds:
    /// an executor attachment taken from another runtime, so every fenced
    /// admission through it is refused, as one whose attachment changed after
    /// the host took it.
    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        not(target_arch = "wasm32")
    ))]
    struct StaleFenceHost {
        runtime: Arc<MeerkatMachine>,
        stale_attachment: meerkat_runtime::RuntimeExecutorAttachmentWitness,
    }

    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        not(target_arch = "wasm32")
    ))]
    #[async_trait]
    impl SurfaceScheduleSessionHost for StaleFenceHost {
        fn decides_existing_admission(&self) -> bool {
            true
        }

        async fn probe_session_target(
            &self,
            _binding: &SessionTargetBinding,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            Ok(TargetProbeOutcome::Ready)
        }

        async fn materialize_session(
            &self,
            _occurrence: &crate::Occurrence,
            _create: &SessionMaterializationSpec,
        ) -> Result<SessionId, ScheduleDomainError> {
            Err(ScheduleDomainError::Internal(
                "this host targets an exact session".to_string(),
            ))
        }

        async fn deliver_prompt(
            &self,
            session_id: &SessionId,
            occurrence: &crate::Occurrence,
            identity: &ScheduleDeliveryIdentity,
            dispatch: ScheduledPromptDispatch,
        ) -> Result<crate::DeliveryDispatch, ScheduleDomainError> {
            let mut prompt =
                meerkat_runtime::PromptInput::from_content_input(dispatch.prompt, None);
            prompt.header.idempotency_key = Some(meerkat_runtime::IdempotencyKey::new(
                schedule_runtime_delivery_idempotency_key(
                    self.runtime.as_ref(),
                    session_id,
                    occurrence,
                    &identity.idempotency_key,
                )
                .await?,
            ));
            prompt.header.correlation_id = Some(schedule_runtime_correlation_id(identity)?);
            accept_fenced_schedule_runtime_input_with_reconciliation(
                self.runtime.as_ref(),
                session_id,
                occurrence,
                identity,
                meerkat_runtime::Input::Prompt(prompt),
                dispatch.materialized_session_id,
                ScheduleAdmissionFence::AttachmentAndMemberResidency {
                    attachment: &self.stale_attachment,
                    expected_member: None,
                },
                |input| {
                    self.runtime
                        .accept_input_with_completion_for_attachment_and_member_residency(
                            &self.stale_attachment,
                            input,
                            None,
                        )
                },
            )
            .await
        }

        async fn deliver_event(
            &self,
            _session_id: &SessionId,
            _occurrence: &crate::Occurrence,
            _identity: &ScheduleDeliveryIdentity,
            _dispatch: ScheduledEventDispatch,
        ) -> Result<crate::DeliveryDispatch, ScheduleDomainError> {
            Err(ScheduleDomainError::Internal(
                "this host delivers prompts only".to_string(),
            ))
        }
    }

    /// A redelivery whose fence no longer holds admits nothing and answers
    /// nothing about the earlier admission: the occurrence stays reclaimable
    /// (`DeliveryRepairDeferred`), and the next attempt takes the fence
    /// afresh. Before, the existing-only admission ignored the host's fence
    /// and answered deduplicated (the mutation control: route it unfenced and
    /// this row sees a dispatch).
    #[cfg(all(
        feature = "session-store",
        feature = "comms",
        feature = "jsonl-store",
        not(target_arch = "wasm32")
    ))]
    #[tokio::test]
    async fn a_redelivery_whose_fence_no_longer_holds_admits_nothing_and_stays_reclaimable() {
        use meerkat_schedule::{
            ExistingDelivery, RuntimeDeliveryOutcome, ScheduleTargetDelivery as _,
        };
        let temp = tempfile::tempdir().expect("tempdir");
        let other_temp = tempfile::tempdir().expect("tempdir");
        let (service, runtime_adapter, session_id) =
            live_peer_only_mob_session(&temp, "stale-fence").await;
        let (other_service, other_runtime, other_session_id) =
            live_peer_only_mob_session(&other_temp, "stale-fence-other").await;
        let stale_attachment = other_runtime
            .current_executor_attachment_witness(&other_session_id)
            .await
            .expect("the other runtime's session has a serving attachment");

        let host = RuntimeBackedScheduleSessionHost::new(
            Arc::clone(&service),
            Arc::clone(&runtime_adapter),
            SessionBuildOptions::default(),
            None,
        );
        let adapter = SharedScheduleTargetAdapter::new(
            ScheduleService::new(Arc::new(meerkat_schedule::MemoryScheduleStore::new())),
            Arc::new(host),
            Arc::new(NoopScheduleMobHost::new("mob targets unused here")),
        );
        let first = exact_session_occurrence(&session_id, 1);
        let identity = ScheduleDeliveryIdentity::for_occurrence(&first);
        let dispatch = adapter
            .deliver_occurrence(&first, &identity)
            .await
            .expect("the ordinary delivery admits under the session's own fence");
        assert_eq!(
            dispatch.receipt.runtime_outcome,
            Some(RuntimeDeliveryOutcome::AdmissionAccepted)
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), dispatch.completion)
            .await
            .expect("the scheduled prompt completes")
            .expect("the delivery terminal resolves");
        let admitted = durable_admission(&runtime_adapter, &session_id, &first)
            .await
            .expect("the earlier input is indexed under its key");

        let stale = SharedScheduleTargetAdapter::new(
            ScheduleService::new(Arc::new(meerkat_schedule::MemoryScheduleStore::new())),
            Arc::new(StaleFenceHost {
                runtime: Arc::clone(&runtime_adapter),
                stale_attachment,
            }),
            Arc::new(NoopScheduleMobHost::new("mob targets unused here")),
        );
        for existing in [ExistingDelivery::Admitted, ExistingDelivery::UnprovenLate] {
            let answer = stale
                .deliver_existing_occurrence(&first, &identity, existing)
                .await;
            assert!(
                matches!(
                    answer,
                    Err(ScheduleDomainError::DeliveryRepairDeferred { .. })
                ),
                "a fence refusal leaves the {existing:?} redelivery reclaimable: {answer:?}"
            );
        }
        assert_eq!(
            durable_admission(&runtime_adapter, &session_id, &first).await,
            Some(admitted),
            "nothing is admitted and the earlier input is left as it is"
        );

        for (service, runtime, session_id) in [
            (&service, &runtime_adapter, &session_id),
            (&other_service, &other_runtime, &other_session_id),
        ] {
            service
                .discard_live_session(session_id)
                .await
                .expect("discard the mob session");
            runtime
                .unregister_session(session_id)
                .await
                .expect("unregister the mob session");
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
        let reserved_admission = service
            .reserve_create_session_admission()
            .await
            .expect("reserve exact actor materialization admission");
        let result = Box::pin(materialize_session_with_reserved_admission_and_actor_slot(
            &service,
            &runtime_adapter,
            session,
            make_runtime_backed_request("scheduled-attached-session", true),
            reserved_admission,
            {
                let service = Arc::clone(&service);
                let runtime_adapter = Arc::clone(&runtime_adapter);
                move |_session_id, attachment_witness, actor_witness_slot| {
                    Box::new(
                        PersistentRuntimeExecutor::new_for_actor_slot_and_attachment(
                            service,
                            runtime_adapter,
                            attachment_witness,
                            actor_witness_slot,
                            None,
                        ),
                    )
                }
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
            None,
        );
        let occurrence = sample_occurrence();
        let identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        let dispatch = host
            .deliver_prompt(
                &result.session_id,
                &occurrence,
                &identity,
                ScheduledPromptDispatch {
                    prompt: ContentInput::Text("scheduled prompt".to_string()),
                    transient_turn_context: None,
                    system_prompt: None,
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
        runtime_adapter
            .unregister_session(&result.session_id)
            .await
            .expect("unregister runtime session");
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
        let reserved_admission = service
            .reserve_create_session_admission()
            .await
            .expect("reserve exact actor materialization admission");
        let result = Box::pin(materialize_session_with_reserved_admission_and_actor_slot(
            &service,
            &runtime_adapter,
            session,
            make_runtime_backed_request("scheduled-detach-session", false),
            reserved_admission,
            {
                let service = Arc::clone(&service);
                let runtime_adapter = Arc::clone(&runtime_adapter);
                move |_session_id, attachment_witness, actor_witness_slot| {
                    Box::new(
                        PersistentRuntimeExecutor::new_for_actor_slot_and_attachment(
                            service,
                            runtime_adapter,
                            attachment_witness,
                            actor_witness_slot,
                            None,
                        ),
                    )
                }
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
            None,
        );
        let occurrence = sample_occurrence();
        let identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        let dispatch = host
            .deliver_prompt(
                &result.session_id,
                &occurrence,
                &identity,
                ScheduledPromptDispatch {
                    prompt: ContentInput::Text("scheduled prompt".to_string()),
                    transient_turn_context: None,
                    system_prompt: None,
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
        runtime_adapter
            .unregister_session(&result.session_id)
            .await
            .expect("unregister runtime session");
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

    /// The runtime-backed host (the one rkat's mob host composes) over a
    /// service that commissions schedules for an agent, as
    /// `PersistenceBundle::schedule_service` builds one for a stock-governed
    /// realm, and composed without a session agent binding fires none: the
    /// fire fails as `schedule_mandate_unavailable` and no session is
    /// materialized for it.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn a_commissioned_runtime_backed_host_without_a_binding_materializes_nothing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let persistence = crate::PersistenceBundle::new(
            Arc::new(meerkat_store::MemoryStore::new()) as Arc<dyn crate::SessionStore>,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new())
                as Arc<dyn meerkat_runtime::RuntimeStore>,
            Arc::new(crate::MemoryBlobStore::new()),
        )
        .expect("construct runtime authority");
        let mut builder = crate::FactoryAgentBuilder::new(
            crate::AgentFactory::new(temp.path().join("sessions")),
            crate::Config::default(),
        );
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        )));
        let (service, runtime_adapter) =
            crate::surface::build_runtime_backed_service(builder, 4, persistence)
                .expect("compose the runtime-backed service");
        let service = Arc::new(service);
        let store = Arc::new(meerkat_schedule::MemoryScheduleStore::new())
            as Arc<dyn meerkat_schedule::ScheduleStore>;
        let agent =
            meerkat_core::PrincipalRef::new(meerkat_core::PrincipalKind::PersonalAgent, "agent-a")
                .expect("agent principal");
        let schedules = ScheduleService::new_commissioned(store, agent);
        let host = spawn_runtime_backed_schedule_host_with_mobs(
            Arc::clone(&service),
            runtime_adapter,
            crate::Config::default(),
            schedules.clone(),
            SessionBuildOptions::default(),
            Arc::new(NoopScheduleMobHost::new("mob targets are not under test")),
            None,
            None,
            "commissioned-runtime-host",
        )
        .expect("a memory schedule store is hosted");

        let mut create = sample_materialization_spec();
        create.comms_name = None;
        let schedule = schedules
            .create(meerkat_schedule::CreateScheduleRequest {
                name: Some("commissioned-runtime-fire".to_string()),
                description: None,
                trigger: meerkat_schedule::TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() - chrono::Duration::seconds(1),
                },
                target: crate::TargetBinding::session(SessionTargetBinding::materialize_on_demand(
                    create,
                    meerkat_schedule::ScheduledSessionAction::Prompt {
                        prompt: ContentInput::Text("scheduled check".to_string()),
                        system_prompt: None,
                        render_metadata: None,
                        skill_refs: Vec::new(),
                        additional_instructions: Vec::new(),
                    },
                )),
                misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
                overlap_policy: meerkat_schedule::OverlapPolicy::AllowConcurrent,
                missing_target_policy: meerkat_schedule::MissingTargetPolicy::Skip,
                labels: Default::default(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("the schedule is created");

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let occurrence = loop {
            let occurrence = schedules
                .list_occurrences(&schedule.schedule_id)
                .await
                .expect("occurrences")
                .into_iter()
                .next()
                .expect("the planned occurrence");
            if !matches!(
                occurrence.phase,
                meerkat_schedule::OccurrencePhase::Pending
                    | meerkat_schedule::OccurrencePhase::Claimed
                    | meerkat_schedule::OccurrencePhase::Dispatching
                    | meerkat_schedule::OccurrencePhase::AwaitingCompletion
            ) {
                break occurrence;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the fire never terminalized: {occurrence:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert_eq!(
            occurrence.phase,
            meerkat_schedule::OccurrencePhase::DeliveryFailed
        );
        assert_eq!(
            occurrence.failure_class,
            Some(meerkat_schedule::OccurrenceFailureClass::ScheduleMandateUnavailable),
            "detail: {:?}",
            occurrence.failure_detail
        );
        assert!(
            service
                .load_authoritative_session(&occurrence.materialized_session_id())
                .await
                .expect("session read")
                .is_none(),
            "no session is materialized for a refused fire"
        );
        host.shutdown().await;
    }
}
