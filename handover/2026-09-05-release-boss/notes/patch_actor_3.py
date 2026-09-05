p='meerkat-mob/src/runtime/actor.rs'
s=open(p).read()
def rep(old, new, count=1):
    global s
    assert s.count(old)==count, (s.count(old), old[:160])
    s=s.replace(old,new)

# ---- fix spawn_turn_completed_reply identity plumbing
rep('''    fn spawn_turn_completed_reply(
        &mut self,
        provisioner: Arc<dyn MobProvisioner>,
        readiness: Option<LocalTurnAdmissionReadiness>,
        member_ref: MemberRef,
''','''    #[allow(clippy::too_many_arguments)]
    fn spawn_turn_completed_reply(
        &mut self,
        provisioner: Arc<dyn MobProvisioner>,
        agent_identity: AgentIdentity,
        readiness: Option<LocalTurnAdmissionReadiness>,
        member_ref: MemberRef,
''')
rep('''        let readiness_context = self.detached_member_readiness_context();
        let session_service = Arc::clone(&self.session_service);
        let command_tx = self.command_tx.clone();
        let readiness_identity = remote
            .as_ref()
            .map(|remote| remote.identity.clone())
            .or_else(|| revival.as_ref().map(|(_, identity)| identity.clone()));
        self.actor_io_tasks.spawn(async move {
            // #1102: the local member's live-session probe and autonomous
            // readiness run here, off the loop, before the tracked turn.
            if let Some(readiness) = readiness
                && let Err(error) = Self::run_local_turn_readiness(
                    &readiness_context,
                    &session_service,
                    &command_tx,
                    readiness_identity.as_ref(),
                    &readiness,
                    &member_ref,
                )
                .await
            {''','''        let readiness_context = self.detached_member_readiness_context();
        let session_service = Arc::clone(&self.session_service);
        let command_tx = self.command_tx.clone();
        self.actor_io_tasks.spawn(async move {
            // #1102: the local member's live-session probe and autonomous
            // readiness run here, off the loop, before the tracked turn.
            if let Some(readiness) = readiness
                && let Err(error) = Self::run_local_turn_readiness(
                    &readiness_context,
                    &session_service,
                    &command_tx,
                    &agent_identity,
                    &readiness,
                    &member_ref,
                )
                .await
            {''')
rep('''                            self.spawn_turn_completed_reply(
                                self.provisioner.clone(),
                                readiness,
                                member_ref,
''','''                            self.spawn_turn_completed_reply(
                                self.provisioner.clone(),
                                agent_identity,
                                readiness,
                                member_ref,
''')
rep('''                        Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                            readiness,
                            member_ref,
                            req,
                            completion_tx,
                            bounded_result_spec,
                            placed_identity,
                            placed_incarnation,
                            placed_input_id,
                            placed_completion_obligation,
                            placed_completion_context,
                        }) => {
                            // Register the exact waiter on the actor before''','''                        Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                            agent_identity,
                            readiness,
                            member_ref,
                            req,
                            completion_tx,
                            bounded_result_spec,
                            placed_identity,
                            placed_incarnation,
                            placed_input_id,
                            placed_completion_obligation,
                            placed_completion_context,
                        }) => {
                            // Register the exact waiter on the actor before''')
rep('''    AwaitTurnCompletion {
        /// Member-local readiness executed by the detached sender before the
        /// tracked turn starts (same #37 probe as the admission lane).
        readiness: Option<LocalTurnAdmissionReadiness>,
        member_ref: MemberRef,''','''    AwaitTurnCompletion {
        agent_identity: AgentIdentity,
        /// Member-local readiness executed by the detached sender before the
        /// tracked turn starts (same #37 probe as the admission lane).
        readiness: Option<LocalTurnAdmissionReadiness>,
        member_ref: MemberRef,''')
rep('''                    Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                        readiness: None,
                        member_ref: machine_member_ref,''','''                    Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                        agent_identity: entry.agent_identity.clone(),
                        readiness: None,
                        member_ref: machine_member_ref,''')
rep('''                        Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                            readiness,
                            member_ref: machine_member_ref,''','''                        Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                            agent_identity: entry.agent_identity.clone(),
                            readiness,
                            member_ref: machine_member_ref,''')

# ---- run_member_turn_admission uses run_local_turn_readiness
rep('''        if let Some(readiness) = readiness {
            if readiness.check_live_session {
                match session_service
                    .live_session_actor_registered(&readiness.bridge_session_id)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) | Err(meerkat_core::service::SessionError::NotFound { .. }) => {
                        // #37: the live materialization is gone while MobMachine
                        // still owns the member as Active. Revival is machine
                        // authority, so it re-enters the actor and this task
                        // waits for its typed verdict.
                        Self::request_member_live_revival(
                            command_tx,
                            &agent_identity,
                            &readiness.bridge_session_id,
                        )
                        .await?;
                    }
                    Err(error) => return Err(MobError::SessionError(error)),
                }
            }
            if readiness.autonomous_runtime {
                let member_ref = match &dispatch {
                    PendingTurnDispatch::Admission { member_ref, .. } => member_ref.clone(),
                    PendingTurnDispatch::Autonomous(material) => material.member_ref.clone(),
                };
                context
                    .ensure_autonomous_runtime_ready(&agent_identity, &member_ref)
                    .await?;
            }
        }
        match dispatch {''','''        if let Some(readiness) = readiness {
            let member_ref = match &dispatch {
                PendingTurnDispatch::Admission { member_ref, .. } => member_ref,
                PendingTurnDispatch::Autonomous(material) => &material.member_ref,
            };
            Self::run_local_turn_readiness(
                context,
                session_service,
                command_tx,
                &agent_identity,
                &readiness,
                member_ref,
            )
            .await?;
        }
        match dispatch {''')
rep('''    async fn request_member_live_revival(
        command_tx: &mpsc::Sender<RoutedMobCommand>,''','''    /// The member-local readiness steps that ran inline on the loop before
    /// #1102: the #37 live-session probe (revival re-enters the actor) and,
    /// for AutonomousHost members, comms-drain plus injector readiness.
    async fn run_local_turn_readiness(
        context: &DetachedMemberReadinessContext,
        session_service: &Arc<dyn MobSessionService>,
        command_tx: &mpsc::Sender<RoutedMobCommand>,
        agent_identity: &AgentIdentity,
        readiness: &LocalTurnAdmissionReadiness,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        if readiness.check_live_session {
            match session_service
                .live_session_actor_registered(&readiness.bridge_session_id)
                .await
            {
                Ok(true) => {}
                Ok(false) | Err(meerkat_core::service::SessionError::NotFound { .. }) => {
                    // #37: the live materialization is gone while MobMachine
                    // still owns the member as Active. Revival is machine
                    // authority, so it re-enters the actor and this task waits
                    // for its typed verdict.
                    Self::request_member_live_revival(
                        command_tx,
                        agent_identity,
                        &readiness.bridge_session_id,
                    )
                    .await?;
                }
                Err(error) => return Err(MobError::SessionError(error)),
            }
        }
        if readiness.autonomous_runtime {
            context
                .ensure_autonomous_runtime_ready(agent_identity, member_ref)
                .await?;
        }
        Ok(())
    }

    async fn request_member_live_revival(
        command_tx: &mpsc::Sender<RoutedMobCommand>,''')

# ---- PreparedMemberRegistrationReload + DetachedMemberReadinessContext impl + PendingResumeLifecycle (after MemberReadinessOutcome)
rep('''/// Budget after which one inline actor-loop step is reported as a stall
/// suspect. Any step that legitimately needs longer belongs off the loop.
const ACTOR_INLINE_STEP_BUDGET: Duration = Duration::from_secs(2);
''','''struct PreparedMemberRegistrationReload {
    entry: RosterEntry,
    member_ref: MemberRef,
    bridge_session_id: SessionId,
}

/// Explicit Resume parked between its detached per-member readiness fan-out
/// and the actor-side continuation (#1102). Everything the continuation needs
/// to reply, commit, roll back, or finish is carried here; the loop is free
/// while the fan-out runs.
pub(super) struct PendingResumeLifecycle {
    ticket: u64,
    phase: ResumeLifecyclePhase,
    admission: super::state::LifecycleAdmissionSignal,
    progress: super::state::LifecycleProgressSignal,
    reply_tx: oneshot::Sender<Result<(), MobError>>,
}

enum ResumeLifecyclePhase {
    /// Same-handle resume: readiness runs before the durable Resume commit.
    PreCommitReadiness,
    /// Rebuilt attachments: readiness runs after the commit and the rebuild.
    PostCommitReadiness {
        post_commit_error: Option<MobError>,
    },
}

/// Per-member bound for one readiness step in the explicit-Resume fan-out.
/// The steps run concurrently, so the fan-out as a whole is bounded by this
/// value rather than by member count.
const RESUME_MEMBER_READINESS_BOUND: Duration = Duration::from_secs(5);

struct MemberReadinessTarget {
    entry: RosterEntry,
    stage: super::state::LifecycleProgressStage,
}

impl DetachedMemberReadinessContext {
    /// Bind (or re-bind) the member's mob comms drain onto its runtime
    /// session. Idempotent; a placed member must never reach this.
    pub(super) async fn ensure_mob_comms_drain(
        &self,
        agent_identity: &AgentIdentity,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        #[cfg(all(not(target_arch = "wasm32"), feature = "runtime-adapter"))]
        {
            let Some(bridge_session_id) = member_ref.bridge_session_id() else {
                return Ok(());
            };

            let adapter =
                self.runtime_adapter
                    .clone()
                    .ok_or_else(|| MobError::MissingMemberCapability {
                        member_id: agent_identity.clone(),
                        capability: crate::error::MobMemberCapability::OutboundCommsRuntime,
                        context: "local member comms-drain runtime adapter",
                    })?;
            let comms_runtime = self
                .provisioner
                .comms_runtime(member_ref)
                .await
                .ok_or_else(|| MobError::MissingMemberCapability {
                    member_id: agent_identity.clone(),
                    capability: crate::error::MobMemberCapability::OutboundCommsRuntime,
                    context: "local member comms-drain startup",
                })?;
            let mob_id = meerkat_runtime::meerkat_machine::dsl::MobId::from(self.mob_id.as_ref());
            let spawned = adapter
                .maybe_spawn_mob_comms_drain(bridge_session_id, comms_runtime, mob_id)
                .await
                .map_err(|err| {
                    MobError::Internal(format!(
                        "mob comms drain spawn failed for session {bridge_session_id}: {err}"
                    ))
                })?;
            if spawned {
                tracing::debug!(
                    agent_identity = %agent_identity,
                    session_id = %bridge_session_id,
                    "updated peer ingress for mob member"
                );
            }
        }

        #[cfg(any(target_arch = "wasm32", not(feature = "runtime-adapter")))]
        {
            let _ = (agent_identity, member_ref);
        }

        Ok(())
    }

    /// Comms drain plus injector capability for a local AutonomousHost member.
    /// Session registration + RuntimeLoop attachment is owned by the
    /// provisioner's lazy `runtime_session_state()` init (called during
    /// provision_member); stop preserves registration, so resume just needs
    /// the drain re-spawned and the injector re-checked.
    pub(super) async fn ensure_autonomous_runtime_ready(
        &self,
        agent_identity: &AgentIdentity,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        self.ensure_mob_comms_drain(agent_identity, member_ref)
            .await?;
        MobActor::ensure_autonomous_dispatch_capability_for_provisioner(
            &self.provisioner,
            agent_identity,
            member_ref,
        )
        .await
    }

    /// Whether a local autonomous Steer must route through real runtime
    /// admission instead of a direct inbox inject. Fails closed: an
    /// indeterminate runtime state requires the admission barrier, because a
    /// direct inject would ack Completed before the machine admitted the turn.
    async fn autonomous_steer_requires_admission_barrier(
        &self,
        agent_identity: &AgentIdentity,
        member_ref: &MemberRef,
        handling_mode: meerkat_core::types::HandlingMode,
        ack_mode: crate::mob_machine::SubmitWorkAckMode,
    ) -> Result<bool, MobError> {
        if handling_mode != meerkat_core::types::HandlingMode::Steer
            || ack_mode != crate::mob_machine::SubmitWorkAckMode::IngressAccepted
        {
            return Ok(false);
        }

        #[cfg(feature = "runtime-adapter")]
        if let (Some(adapter), Some(session_id)) =
            (&self.runtime_adapter, member_ref.bridge_session_id())
        {
            use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;

            match adapter.runtime_state(session_id).await {
                Ok(meerkat_runtime::RuntimeState::Running) => {
                    tracing::debug!(
                        agent_identity = %agent_identity,
                        session_id = %session_id,
                        "active steer admission barrier enabled by running runtime state"
                    );
                    return Ok(true);
                }
                Ok(state) => {
                    let session_active = self
                        .provisioner
                        .is_member_active(member_ref)
                        .await?
                        .unwrap_or(false);
                    tracing::debug!(
                        agent_identity = %agent_identity,
                        session_id = %session_id,
                        runtime_state = ?state,
                        session_active,
                        "active steer admission barrier checked non-running runtime state"
                    );
                    return Ok(session_active);
                }
                Err(error) => {
                    tracing::debug!(
                        agent_identity = %agent_identity,
                        session_id = %session_id,
                        error = %error,
                        "runtime state unavailable; requiring autonomous steer admission barrier (fail closed)"
                    );
                    return Ok(true);
                }
            }
        }

        #[cfg(not(feature = "runtime-adapter"))]
        let _ = (agent_identity, member_ref);
        Ok(false)
    }

    /// Off-loop half of local autonomous dispatch: admit a runtime turn when
    /// the steer barrier demands it, otherwise inject the inbox event.
    async fn dispatch_autonomous(
        &self,
        agent_identity: &AgentIdentity,
        material: AutonomousDispatchMaterial,
    ) -> Result<(), MobError> {
        let AutonomousDispatchMaterial {
            member_ref,
            bridge_session_id,
            content,
            system_prompt,
            turn_metadata,
            handling_mode,
            external_delivery_identity,
            interaction_id,
            objective_id,
            event_tx,
            completion_tx,
            llm_identity_applied_tx,
            ack_mode,
        } = material;
        let render_metadata = turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.render_metadata.clone());
        if self
            .autonomous_steer_requires_admission_barrier(
                agent_identity,
                &member_ref,
                handling_mode,
                ack_mode,
            )
            .await?
        {
            let req = meerkat_core::service::StartTurnRequest {
                // The admission barrier is steer-only; steer dispatch with
                // injected context was rejected before the mode fork, so this
                // carrier is invariantly empty here.
                injected_context: Vec::new(),
                prompt: content,
                system_prompt,
                event_tx,
                runtime: submit_work_runtime_semantics(
                    handling_mode,
                    turn_metadata,
                    external_delivery_identity.as_ref(),
                ),
            };
            return if let Some(completion_tx) = completion_tx {
                self.provisioner
                    .admit_tracked_turn(&member_ref, req, completion_tx, llm_identity_applied_tx)
                    .await
            } else {
                self.provisioner.admit_turn(&member_ref, req).await
            };
        }
        let injector = self
            .provisioner
            .interaction_event_injector(&bridge_session_id)
            .await
            .ok_or_else(|| MobError::MissingMemberCapability {
                member_id: agent_identity.clone(),
                capability: crate::error::MobMemberCapability::InteractionEventInjector,
                context: "autonomous direct turn delivery",
            })?;
        // A host-supplied interaction id rides the injected inbox event so the
        // comms classification (and therefore the runtime transcript identity)
        // carries the SAME id as the host's live interaction frames instead
        // of minting a fresh unrelated one.
        let inject_result = match external_delivery_identity {
            Some(identity) => injector.inject_with_delivery_identity(
                meerkat_core::service::StartTurnInputIdentity {
                    idempotency_key: identity.idempotency_key,
                    correlation_id: identity.correlation_id,
                },
                objective_id,
                content,
                meerkat_core::PlainEventSource::Rpc,
                handling_mode,
                render_metadata,
            ),
            None => injector.inject_with_turn_identity(
                interaction_id,
                objective_id,
                content,
                meerkat_core::PlainEventSource::Rpc,
                handling_mode,
                render_metadata,
            ),
        };
        inject_result.map_err(|error| {
            MobError::Internal(format!(
                "autonomous dispatch inject failed for '{agent_identity}': {error}"
            ))
        })
    }
}

/// Budget after which one inline actor-loop step is reported as a stall
/// suspect. Any step that legitimately needs longer belongs off the loop.
const ACTOR_INLINE_STEP_BUDGET: Duration = Duration::from_secs(2);
''')

# ---- item 5: typed fast reject in handle_submit_work
rep('''        let initial_entry_present = initial_entry.is_some();
        let entry = match initial_entry {
            Some(e) => {
                self.ensure_member_not_broken(&e.agent_identity).await?;
                e
            }
''','''        let initial_entry_present = initial_entry.is_some();
        let entry = match initial_entry {
            Some(e) => {
                self.ensure_member_not_broken(&e.agent_identity).await?;
                self.ensure_member_runtime_durability_ready(&e).await?;
                e
            }
''')
rep('''    async fn ensure_member_not_broken(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Result<(), MobError> {''','''    /// #1102 fast rejection: a local runtime-backed member whose persistent
    /// runtime shell is `ReloadRequired` refuses every input at admission
    /// (`RecoveryRepairBlocked`), so deliveries to it must not be dispatched.
    /// Reads the same per-session gate the runtime ingress path consults and
    /// returns the typed `MemberReloadRequired` before any dispatch work.
    async fn ensure_member_runtime_durability_ready(
        &self,
        entry: &RosterEntry,
    ) -> Result<(), MobError> {
        #[cfg(feature = "runtime-adapter")]
        if let Some(adapter) = self.runtime_adapter.as_ref()
            && !super::member_runtime_is_host_owned(
                self.dsl_authority.state(),
                &entry.agent_identity,
            )
            && let Some(bridge_session_id) = entry.member_ref.bridge_session_id()
            && let Some(required) = adapter.durability_reload_required(bridge_session_id).await
        {
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %entry.agent_identity,
                session_id = %bridge_session_id,
                operation = %required.operation,
                "rejecting delivery: member runtime registration requires a cold reload"
            );
            return Err(MobError::MemberReloadRequired {
                member_id: entry.agent_identity.clone(),
                reason: required.to_string(),
            });
        }
        #[cfg(not(feature = "runtime-adapter"))]
        let _ = entry;
        Ok(())
    }

    async fn ensure_member_not_broken(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Result<(), MobError> {''')

open(p,'w').write(s)
print("ok")
