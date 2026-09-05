p='meerkat-mob/src/runtime/actor.rs'
s=open(p).read()
def rep(old, new, count=1):
    global s
    assert s.count(old)==count, (s.count(old), old[:160])
    s=s.replace(old,new)

# ---------------------------------------------------------------- SubmitWork arm
rep('''                        match Box::pin(self.handle_submit_work(payload)).await {
                        Ok(SubmitWorkDispatchCompletion::Completed) => {
                            if !self.respawn_topology_reply_withheld {
                                let _ = reply_tx.send(Ok(()));
                            }
                        }
                        Ok(SubmitWorkDispatchCompletion::AwaitTurnAdmission {
                            operation_id: _,
                            member_ref,
                            req,
                            completion_tx,
                            llm_identity_applied_tx,
                            placed_identity,
                            placed_incarnation,
                            placed_input_id,
                        }) => {
                            if !self.respawn_topology_reply_withheld {
                                self.spawn_turn_admission_reply(
                                    self.provisioner.clone(),
                                    member_ref,
                                    req,
                                    completion_tx,
                                    llm_identity_applied_tx,
                                    reply_tx,
                                    placed_identity
                                        .map(|identity| (self.command_tx.clone(), identity)),
                                    placed_incarnation,
                                    placed_input_id,
                                );
                            }
                        }
                        Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                            member_ref,
''','''                        self.inline_step_watchdog.set_step("handle_submit_work");
                        match Box::pin(self.handle_submit_work(payload)).await {
                        Ok(SubmitWorkDispatchCompletion::Completed) => {
                            if !self.respawn_topology_reply_withheld {
                                let _ = reply_tx.send(Ok(()));
                            }
                        }
                        Ok(SubmitWorkDispatchCompletion::AwaitTurnAdmission {
                            operation_id: _,
                            agent_identity,
                            readiness,
                            member_ref,
                            req,
                            completion_tx,
                            llm_identity_applied_tx,
                            placed_identity,
                            placed_incarnation,
                            placed_input_id,
                        }) => {
                            if !self.respawn_topology_reply_withheld {
                                self.enqueue_member_turn_admission(
                                    Box::new(PendingMemberTurnAdmission {
                                        agent_identity,
                                        readiness,
                                        dispatch: PendingTurnDispatch::Admission {
                                            member_ref,
                                            req,
                                            completion_tx,
                                            llm_identity_applied_tx,
                                            placed_identity,
                                            placed_incarnation,
                                            placed_input_id,
                                        },
                                    }),
                                    reply_tx,
                                );
                            }
                        }
                        Ok(SubmitWorkDispatchCompletion::AwaitAutonomousDispatch {
                            agent_identity,
                            readiness,
                            material,
                        }) => {
                            if !self.respawn_topology_reply_withheld {
                                self.enqueue_member_turn_admission(
                                    Box::new(PendingMemberTurnAdmission {
                                        agent_identity,
                                        readiness,
                                        dispatch: PendingTurnDispatch::Autonomous(material),
                                    }),
                                    reply_tx,
                                );
                            }
                        }
                        Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                            readiness,
                            member_ref,
''')
rep('''                            self.spawn_turn_completed_reply(
                                self.provisioner.clone(),
                                member_ref,
                                req,
                                completion_tx,
                                bounded_result_spec,
                                reply_tx,
                                placed_identity.map(|identity| (self.command_tx.clone(), identity)),
                                remote,
                            );
''','''                            self.spawn_turn_completed_reply(
                                self.provisioner.clone(),
                                readiness,
                                member_ref,
                                req,
                                completion_tx,
                                bounded_result_spec,
                                reply_tx,
                                placed_identity.map(|identity| (self.command_tx.clone(), identity)),
                                remote,
                            );
''')

# ---------------------------------------------------------------- new command arms (insert before RevivePlacedMember arm)
rep('''                MobCommand::RevivePlacedMember {
                    agent_identity,
                    reason,
                } => {
                    // Internal fire-and-forget trigger: the outcome is
''','''                MobCommand::MemberTurnAdmissionSettled {
                    agent_identity,
                    ticket,
                } => {
                    self.settle_member_turn_admission(&agent_identity, ticket);
                }
                MobCommand::ReviveMemberLiveMaterialization {
                    agent_identity,
                    bridge_session_id,
                    reply_tx,
                } => {
                    self.inline_step_watchdog
                        .set_step("revive_member_live_materialization");
                    let result = Box::pin(
                        self.revive_member_live_materialization_for_delivery(
                            &agent_identity,
                            &bridge_session_id,
                        ),
                    )
                    .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::ReloadMemberRegistration {
                    agent_identity,
                    reply_tx,
                } => {
                    match self.prepare_member_registration_reload(&agent_identity).await {
                        Ok(prepared) => self.spawn_member_registration_reload(prepared, reply_tx),
                        Err(error) => {
                            let _ = reply_tx.send(Err(error));
                        }
                    }
                }
                MobCommand::ResumeLifecycleReadinessResolved { ticket, outcomes } => {
                    self.inline_step_watchdog
                        .set_step("resume_lifecycle_readiness_resolved");
                    Box::pin(self.resume_lifecycle_readiness_resolved(ticket, outcomes)).await;
                }
                MobCommand::RevivePlacedMember {
                    agent_identity,
                    reason,
                } => {
                    // Internal fire-and-forget trigger: the outcome is
''')

# ---------------------------------------------------------------- abandoned control + watchdog in run loop
rep('''            let control = if let Some(control) = Self::abandoned_observation_control(&cmd) {
                tracing::debug!(
                    command_kind = cmd.kind(),
                    "MobActor skipped abandoned observation"
                );
                control
            } else {''','''            let control = if let Some(control) = Self::abandoned_command_control(&cmd) {
                tracing::debug!(
                    command_kind = cmd.kind(),
                    "MobActor skipped abandoned command (caller dropped its reply receiver)"
                );
                control
            } else {
                self.inline_step_watchdog.begin(cmd.kind(), "dispatch_command");''')
rep('''                self.dispatch_command_boxed(
                    &authority,
                    cmd,
                    &mut command_rx,
                    &mut deferred_commands,
                    &mut host_status_polls_in_flight,
                )
                .await
            };
            match control {''','''                let control = self
                    .dispatch_command_boxed(
                        &authority,
                        cmd,
                        &mut command_rx,
                        &mut deferred_commands,
                        &mut host_status_polls_in_flight,
                    )
                    .await;
                self.inline_step_watchdog.end(&self.definition.id);
                control
            };
            match control {''')
rep('''    fn abandoned_observation_control(cmd: &MobCommand) -> Option<ActorLoopControl> {
        cmd.is_abandoned_observation()
            .then_some(ActorLoopControl::ProceedBoundary)
    }
''','''    fn abandoned_command_control(cmd: &MobCommand) -> Option<ActorLoopControl> {
        (cmd.is_abandoned_observation() || cmd.is_abandoned_delivery())
            .then_some(ActorLoopControl::ProceedBoundary)
    }
''')
# start/stop checker in run()
rep('''    pub(super) async fn run(mut self, mut command_rx: mpsc::Receiver<RoutedMobCommand>) {
        if !boxed_arm_future(|| self.prepare_actor_run()).await {
            return;
        }
''','''    pub(super) async fn run(mut self, mut command_rx: mpsc::Receiver<RoutedMobCommand>) {
        self.inline_step_watchdog
            .start_checker(self.definition.id.clone());
        if !boxed_arm_future(|| self.prepare_actor_run()).await {
            self.inline_step_watchdog.stop();
            return;
        }
''')
rep('''        // Unconditional crash-style epilogue: command-channel EOF, explicit
        // shutdown, fail-stop, and internal dispatch failure all share the
        // same join barrier.''','''        self.inline_step_watchdog.stop();
        // Unconditional crash-style epilogue: command-channel EOF, explicit
        // shutdown, fail-stop, and internal dispatch failure all share the
        // same join barrier.''')

# ---------------------------------------------------------------- readiness helpers -> detached context
rep('''    async fn ensure_mob_comms_drain(
        &self,
        agent_identity: &AgentIdentity,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        if super::member_runtime_is_host_owned(self.dsl_authority.state(), agent_identity) {
            // The member host owns this session and its drain. The optional
            // session carried by a projected BackendPeer is a remote fence,
            // never a controller-local runtime key.
            return Ok(());
        }
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
            let mob_id =
                meerkat_runtime::meerkat_machine::dsl::MobId::from(self.definition.id.as_ref());
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
''','''    async fn ensure_mob_comms_drain(
        &self,
        agent_identity: &AgentIdentity,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        if super::member_runtime_is_host_owned(self.dsl_authority.state(), agent_identity) {
            // The member host owns this session and its drain. The optional
            // session carried by a projected BackendPeer is a remote fence,
            // never a controller-local runtime key.
            return Ok(());
        }
        self.detached_member_readiness_context()
            .ensure_mob_comms_drain(agent_identity, member_ref)
            .await
    }

    /// Snapshot the actor material that member-local readiness needs so it
    /// can run on a detached task (#1102). Placement must already have been
    /// decided on the loop: this context serves local members only.
    pub(super) fn detached_member_readiness_context(&self) -> DetachedMemberReadinessContext {
        DetachedMemberReadinessContext {
            provisioner: Arc::clone(&self.provisioner),
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter: self.runtime_adapter.clone(),
            mob_id: self.definition.id.clone(),
        }
    }
''')
rep('''    async fn ensure_autonomous_dispatch_capability(
        &self,
        agent_identity: &AgentIdentity,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        if super::member_runtime_is_host_owned(self.dsl_authority.state(), agent_identity) {
            // Placed autonomous members receive input through the supervisor
            // bridge; their injector capability is owned by the member host.
            return Ok(());
        }
        Self::ensure_autonomous_dispatch_capability_for_provisioner(
            &self.provisioner,
            agent_identity,
            member_ref,
        )
        .await
    }
''','''    async fn ensure_autonomous_dispatch_capability(
        &self,
        agent_identity: &AgentIdentity,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        if super::member_runtime_is_host_owned(self.dsl_authority.state(), agent_identity) {
            // Placed autonomous members receive input through the supervisor
            // bridge; their injector capability is owned by the member host.
            return Ok(());
        }
        Self::ensure_autonomous_dispatch_capability_for_provisioner(
            &self.provisioner,
            agent_identity,
            member_ref,
        )
        .await
    }

    // ------------------------------------------------------------------
    // #1102: per-member single-flight admission lanes.
    // ------------------------------------------------------------------

    /// Hand one DSL-admitted delivery to its member's admission lane. The
    /// first delivery for an idle member starts immediately on a detached
    /// task; later deliveries park in FIFO order behind it. The loop never
    /// awaits any of them.
    fn enqueue_member_turn_admission(
        &mut self,
        pending: Box<PendingMemberTurnAdmission>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    ) {
        let agent_identity = pending.agent_identity.clone();
        let lane = self
            .member_admission_lanes
            .entry(agent_identity.clone())
            .or_default();
        if lane.inflight.is_none() {
            self.spawn_member_turn_admission(pending, reply_tx);
            return;
        }
        if lane.parked.len() >= super::handle::MEMBER_ADMISSION_LANE_CAPACITY {
            let depth = lane.parked.len();
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                depth,
                "member admission lane is full; rejecting delivery typed"
            );
            let _ = reply_tx.send(Err(MobError::MemberAdmissionBacklogFull {
                member_id: agent_identity,
                depth,
            }));
            return;
        }
        lane.parked
            .push_back(ParkedMemberTurnAdmission { pending, reply_tx });
        let depth = lane.parked.len();
        self.member_admission_backlog.record(&agent_identity, depth);
        tracing::debug!(
            mob_id = %self.definition.id,
            agent_identity = %agent_identity,
            depth,
            "parked member delivery behind the member's in-flight admission"
        );
    }

    fn spawn_member_turn_admission(
        &mut self,
        pending: Box<PendingMemberTurnAdmission>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    ) {
        let agent_identity = pending.agent_identity.clone();
        self.next_member_admission_ticket = self.next_member_admission_ticket.wrapping_add(1);
        let ticket = self.next_member_admission_ticket;
        self.member_admission_lanes
            .entry(agent_identity.clone())
            .or_default()
            .inflight = Some(ticket);
        let context = self.detached_member_readiness_context();
        let session_service = Arc::clone(&self.session_service);
        let command_tx = self.command_tx.clone();
        self.actor_io_tasks.spawn(async move {
            let result = Self::run_member_turn_admission(
                &context,
                &session_service,
                &command_tx,
                *pending,
                &reply_tx,
            )
            .await;
            let _ = reply_tx.send(result);
            // Release the lane on the actor. A closed channel means the actor
            // exited; its lanes died with it.
            let _ = command_tx
                .send(RoutedMobCommand::internal(
                    MobCommand::MemberTurnAdmissionSettled {
                        agent_identity,
                        ticket,
                    },
                ))
                .await;
        });
    }

    /// Release a member's admission lane and start its next parked delivery,
    /// skipping any whose caller has already gone.
    fn settle_member_turn_admission(&mut self, agent_identity: &AgentIdentity, ticket: u64) {
        let Some(lane) = self.member_admission_lanes.get_mut(agent_identity) else {
            return;
        };
        if lane.inflight != Some(ticket) {
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                ticket,
                "ignoring stale member admission settlement"
            );
            return;
        }
        lane.inflight = None;
        let mut next = None;
        while let Some(parked) = lane.parked.pop_front() {
            if parked.reply_tx.is_closed() {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    agent_identity = %agent_identity,
                    "skipping parked delivery whose caller dropped its reply receiver"
                );
                continue;
            }
            next = Some(parked);
            break;
        }
        let depth = lane.parked.len();
        let lane_idle = next.is_none() && depth == 0;
        self.member_admission_backlog.record(agent_identity, depth);
        if lane_idle {
            self.member_admission_lanes.remove(agent_identity);
            return;
        }
        if let Some(ParkedMemberTurnAdmission { pending, reply_tx }) = next {
            self.spawn_member_turn_admission(pending, reply_tx);
        }
    }

    /// Body of one detached member turn admission: caller liveness, then the
    /// member-local readiness steps that used to run inline, then the
    /// runtime admission or autonomous dispatch.
    async fn run_member_turn_admission(
        context: &DetachedMemberReadinessContext,
        session_service: &Arc<dyn MobSessionService>,
        command_tx: &mpsc::Sender<RoutedMobCommand>,
        pending: PendingMemberTurnAdmission,
        reply_tx: &oneshot::Sender<Result<(), MobError>>,
    ) -> Result<(), MobError> {
        let PendingMemberTurnAdmission {
            agent_identity,
            readiness,
            dispatch,
        } = pending;
        if reply_tx.is_closed() {
            // Parked long enough for the caller to leave: never run a ghost
            // turn. Equivalent to the caller not having sent it.
            tracing::debug!(
                agent_identity = %agent_identity,
                "member delivery abandoned before its lane admission; not executed"
            );
            return Err(MobError::ActorCommandTimedOut {
                command_kind: "SubmitWork",
                stage: "member_admission_lane",
            });
        }
        if let Some(readiness) = readiness {
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
        match dispatch {
            PendingTurnDispatch::Admission {
                member_ref,
                req,
                completion_tx,
                llm_identity_applied_tx,
                placed_identity,
                placed_incarnation,
                placed_input_id,
            } => {
                Self::execute_turn_admission(
                    &context.provisioner,
                    member_ref,
                    req,
                    completion_tx,
                    llm_identity_applied_tx,
                    placed_identity.map(|identity| (command_tx.clone(), identity)),
                    placed_incarnation,
                    placed_input_id,
                )
                .await
            }
            PendingTurnDispatch::Autonomous(material) => {
                context
                    .dispatch_autonomous(&agent_identity, *material)
                    .await
            }
        }
    }

    async fn request_member_live_revival(
        command_tx: &mpsc::Sender<RoutedMobCommand>,
        agent_identity: &AgentIdentity,
        bridge_session_id: &SessionId,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(RoutedMobCommand::internal(
                MobCommand::ReviveMemberLiveMaterialization {
                    agent_identity: agent_identity.clone(),
                    bridge_session_id: bridge_session_id.clone(),
                    reply_tx,
                },
            ))
            .await
            .map_err(|_| MobError::ActorCommandChannelClosed)?;
        reply_rx
            .await
            .map_err(|_| MobError::ActorReplyChannelClosed)?
    }

    /// Actor-side half of the #37 revival requested by a detached admission
    /// task. Re-resolves the member against current machine state so a stale
    /// request (retired, respawned, or rebound member) fails typed instead of
    /// reviving the wrong incarnation.
    async fn revive_member_live_materialization_for_delivery(
        &mut self,
        agent_identity: &AgentIdentity,
        bridge_session_id: &SessionId,
    ) -> Result<(), MobError> {
        let entry = self
            .roster
            .read()
            .await
            .get(agent_identity)
            .cloned()
            .ok_or_else(|| MobError::MemberNotFound(agent_identity.clone()))?;
        self.ensure_member_not_broken(agent_identity).await?;
        let member_ref = self.machine_member_ref_for_behavior(&entry, "member turn delivery")?;
        if member_ref.bridge_session_id() != Some(bridge_session_id) {
            return Err(MobError::Internal(format!(
                "member '{agent_identity}' session binding changed before delivery revival of '{bridge_session_id}'"
            )));
        }
        self.revive_member_live_materialization(
            &entry,
            &member_ref,
            bridge_session_id,
            None,
            false,
            true,
        )
        .await
    }

    // ------------------------------------------------------------------
    // #1102 section 5: non-destructive member registration reload.
    // ------------------------------------------------------------------

    async fn prepare_member_registration_reload(
        &mut self,
        agent_identity: &AgentIdentity,
    ) -> Result<PreparedMemberRegistrationReload, MobError> {
        let entry = self
            .roster
            .read()
            .await
            .get(agent_identity)
            .cloned()
            .ok_or_else(|| MobError::MemberNotFound(agent_identity.clone()))?;
        if super::member_runtime_is_host_owned(self.dsl_authority.state(), agent_identity) {
            return Err(MobError::UnsupportedForMode {
                mode: entry.runtime_mode,
                reason: "placed members are reloaded by their member host".to_string(),
            });
        }
        let member_ref =
            self.machine_member_ref_for_behavior(&entry, "member registration reload")?;
        let bridge_session_id = member_ref.bridge_session_id().cloned().ok_or_else(|| {
            MobError::UnsupportedForMode {
                mode: entry.runtime_mode,
                reason: "registration reload requires a session-backed member".to_string(),
            }
        })?;
        Ok(PreparedMemberRegistrationReload {
            entry,
            member_ref,
            bridge_session_id,
        })
    }

    fn spawn_member_registration_reload(
        &mut self,
        prepared: PreparedMemberRegistrationReload,
        reply_tx: oneshot::Sender<Result<super::handle::MemberReloadOutcome, MobError>>,
    ) {
        let PreparedMemberRegistrationReload {
            entry,
            member_ref,
            bridge_session_id,
        } = prepared;
        let context = self.detached_member_readiness_context();
        let command_tx = self.command_tx.clone();
        self.actor_io_tasks.spawn(async move {
            let deadline = Instant::now() + super::provisioner::MEMBER_RETIRE_TOTAL_TIMEOUT;
            let result = async {
                let disposition = context
                    .provisioner
                    .reload_degraded_runtime_registration(&member_ref, deadline)
                    .await?;
                if disposition == super::handle::MemberReloadDisposition::Discarded {
                    // The degraded shell is gone; rebuild the live session for
                    // the same session id through the machine-authorized
                    // revival seam (executor re-registration from durable
                    // truth), then re-arm the member's runtime readiness.
                    Self::request_member_live_revival(
                        &command_tx,
                        &entry.agent_identity,
                        &bridge_session_id,
                    )
                    .await?;
                    if entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost {
                        context
                            .ensure_autonomous_runtime_ready(&entry.agent_identity, &member_ref)
                            .await?;
                    } else {
                        context
                            .ensure_mob_comms_drain(&entry.agent_identity, &member_ref)
                            .await?;
                    }
                }
                Ok(super::handle::MemberReloadOutcome {
                    disposition,
                    session_id: bridge_session_id.clone(),
                    generation: entry.agent_runtime_id.generation,
                })
            }
            .await;
            if let Err(error) = &result {
                tracing::warn!(
                    agent_identity = %entry.agent_identity,
                    session_id = %bridge_session_id,
                    error = %error,
                    "member registration reload failed"
                );
            } else {
                tracing::info!(
                    agent_identity = %entry.agent_identity,
                    session_id = %bridge_session_id,
                    disposition = ?result.as_ref().map(|outcome| outcome.disposition),
                    "member registration reload completed"
                );
            }
            let _ = reply_tx.send(result);
        });
    }
''')

# ---------------------------------------------------------------- dispatch_member_turn_after_machine_admission
rep('''        let effective_interaction_id = interaction_id;
        // §19.L5/W-D.2: the member HOST owns a placed member's liveness - the
        // controlling realm never holds its session, so the local ensure
        // below would mint a dishonest DurableSnapshotMissing. Placed
        // revival is delivery-failure-triggered (fire_placed_revival_trigger)
        // and host-observed instead.
        if placed_identity.is_none()
            && !live_steer_admission
            && let Some(bridge_session_id) = machine_member_ref.bridge_session_id()
        {
            tracing::debug!(
                agent_identity = %entry.agent_identity,
                session_id = %bridge_session_id,
                "dispatch_member_turn_after_machine_admission checking live session actor"
            );
            match self
                .session_service
                .live_session_actor_registered(bridge_session_id)
                .await
            {
                Ok(true) => {
                    tracing::debug!(
                        agent_identity = %entry.agent_identity,
                        session_id = %bridge_session_id,
                        "dispatch_member_turn_after_machine_admission live session actor exists"
                    );
                }
                Ok(false) | Err(meerkat_core::service::SessionError::NotFound { .. }) => {
                    // #37: the live materialization is gone while MobMachine
                    // still owns the member as Active. Machine-authorized
                    // revival rebuilds the live session from the durable
                    // snapshot through the existing resume materialization
                    // path; an unrecoverable or failed revival resolves into
                    // the typed terminal `MemberRestoreFailed`.
                    self.revive_member_live_materialization(
                        entry,
                        &machine_member_ref,
                        bridge_session_id,
                        None,
                        false,
                        true,
                    )
                    .await?;
                    tracing::debug!(
                        agent_identity = %entry.agent_identity,
                        session_id = %bridge_session_id,
                        "dispatch_member_turn_after_machine_admission revived live session"
                    );
                }
                Err(error) => return Err(MobError::SessionError(error)),
            }
        }
''','''        let effective_interaction_id = interaction_id;
        // §19.L5/W-D.2: the member HOST owns a placed member's liveness - the
        // controlling realm never holds its session, so a local ensure would
        // mint a dishonest DurableSnapshotMissing. Placed revival is
        // delivery-failure-triggered (fire_placed_revival_trigger) and
        // host-observed instead.
        //
        // #1102: for local members the #37 live-session probe (and the
        // machine-authorized revival it may request) plus autonomous runtime
        // readiness run in the member's detached admission lane, not here.
        // Only the placement/mode decision and the machine-owned material are
        // resolved on the loop.
        let readiness = if placed_identity.is_none()
            && let Some(bridge_session_id) = machine_member_ref.bridge_session_id()
        {
            Some(LocalTurnAdmissionReadiness {
                bridge_session_id: bridge_session_id.clone(),
                check_live_session: !live_steer_admission,
                autonomous_runtime: entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost,
            })
        } else {
            None
        };
''')
# placed branch: add agent_identity/readiness to AwaitTurnAdmission and readiness to AwaitTurnCompletion
rep('''            return match ack_mode {
                crate::mob_machine::SubmitWorkAckMode::IngressAccepted => {
                    Ok(SubmitWorkDispatchCompletion::AwaitTurnAdmission {
                        operation_id,
                        member_ref: machine_member_ref,
                        req: Box::new(req),
                        completion_tx,
                        llm_identity_applied_tx,
                        placed_identity,
                        placed_incarnation,
                        placed_input_id,
                    })
                }
                crate::mob_machine::SubmitWorkAckMode::TurnCompleted => {
                    Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                        member_ref: machine_member_ref,
''','''            return match ack_mode {
                crate::mob_machine::SubmitWorkAckMode::IngressAccepted => {
                    Ok(SubmitWorkDispatchCompletion::AwaitTurnAdmission {
                        operation_id,
                        agent_identity: entry.agent_identity.clone(),
                        readiness: None,
                        member_ref: machine_member_ref,
                        req: Box::new(req),
                        completion_tx,
                        llm_identity_applied_tx,
                        placed_identity,
                        placed_incarnation,
                        placed_input_id,
                    })
                }
                crate::mob_machine::SubmitWorkAckMode::TurnCompleted => {
                    Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                        readiness: None,
                        member_ref: machine_member_ref,
''')
# autonomous branch
rep('''                self.ensure_autonomous_runtime_ready(&entry.agent_identity, &machine_member_ref)
                    .await?;

                let render_metadata = turn_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.render_metadata.clone());

                if self
                    .autonomous_steer_requires_admission_barrier(
                        entry,
                        &machine_member_ref,
                        handling_mode,
                        ack_mode,
                    )
                    .await?
                {
                    let req = meerkat_core::service::StartTurnRequest {
                        // The admission barrier is steer-only; steer dispatch
                        // with injected context was rejected before the mode
                        // fork, so this carrier is invariantly empty here.
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
                    return Ok(SubmitWorkDispatchCompletion::AwaitTurnAdmission {
                        operation_id,
                        member_ref: machine_member_ref,
                        req: Box::new(req),
                        completion_tx,
                        llm_identity_applied_tx,
                        placed_identity,
                        placed_incarnation,
                        placed_input_id,
                    });
                }
                // Injected context on the autonomous inbox path was rejected
                // pre-admission in `handle_submit_work` (the plain-event path
                // has no user-channel work boundary); the carrier is
                // invariantly empty here.
                let injector = self
                    .provisioner
                    .interaction_event_injector(&bridge_session_id)
                    .await
                    .ok_or_else(|| MobError::MissingMemberCapability {
                        member_id: crate::ids::AgentIdentity::from(entry.agent_identity.as_str()),
                        capability: crate::error::MobMemberCapability::InteractionEventInjector,
                        context: "autonomous direct turn delivery",
                    })?;
                // A host-supplied interaction id rides the injected inbox
                // event so the comms classification (and therefore the
                // runtime transcript identity) carries the SAME id as the
                // host's live interaction frames instead of minting a fresh
                // unrelated one.
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
                        "autonomous dispatch inject failed for '{}': {}",
                        entry.agent_identity, error
                    ))
                })?;
                Ok(SubmitWorkDispatchCompletion::Completed)
            }
''','''                // Runtime readiness, the steer admission-barrier probe, and
                // the admit-or-inject decision are member-local and run in
                // the member's admission lane (#1102). Injected context on the
                // autonomous inbox path was rejected pre-admission in
                // `handle_submit_work`, so the carrier is invariantly empty.
                debug_assert!(injected_context.is_empty());
                Ok(SubmitWorkDispatchCompletion::AwaitAutonomousDispatch {
                    agent_identity: entry.agent_identity.clone(),
                    readiness,
                    material: Box::new(AutonomousDispatchMaterial {
                        member_ref: machine_member_ref,
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
                    }),
                })
            }
''')
# turn-driven branch completions
rep('''                        Ok(SubmitWorkDispatchCompletion::AwaitTurnAdmission {
                            operation_id,
                            member_ref: machine_member_ref,
                            req,
                            completion_tx,
                            llm_identity_applied_tx,
                            placed_identity,
                            placed_incarnation,
                            placed_input_id,
                        })
                    }
                    crate::mob_machine::SubmitWorkAckMode::TurnCompleted => {''','''                        Ok(SubmitWorkDispatchCompletion::AwaitTurnAdmission {
                            operation_id,
                            agent_identity: entry.agent_identity.clone(),
                            readiness,
                            member_ref: machine_member_ref,
                            req,
                            completion_tx,
                            llm_identity_applied_tx,
                            placed_identity,
                            placed_incarnation,
                            placed_input_id,
                        })
                    }
                    crate::mob_machine::SubmitWorkAckMode::TurnCompleted => {''')
rep('''                        Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                            member_ref: machine_member_ref,
                            req,
                            completion_tx,
                            bounded_result_spec,
                            placed_identity,
                            placed_incarnation,
                            placed_input_id,
                            placed_completion_obligation,
                            placed_completion_context,
                        })
                    }
                }
            }
        }
    }
''','''                        Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                            readiness,
                            member_ref: machine_member_ref,
                            req,
                            completion_tx,
                            bounded_result_spec,
                            placed_identity,
                            placed_incarnation,
                            placed_input_id,
                            placed_completion_obligation,
                            placed_completion_context,
                        })
                    }
                }
            }
        }
    }
''')

# autonomous_steer_requires_admission_barrier: delegate to context
rep('''    async fn autonomous_steer_requires_admission_barrier(
        &self,
        entry: &RosterEntry,
        member_ref: &MemberRef,
        handling_mode: meerkat_core::types::HandlingMode,
        ack_mode: crate::mob_machine::SubmitWorkAckMode,
    ) -> Result<bool, MobError> {
        if handling_mode != meerkat_core::types::HandlingMode::Steer
            || ack_mode != crate::mob_machine::SubmitWorkAckMode::IngressAccepted
        {
            return Ok(false);
        }

        if super::member_runtime_is_host_owned(self.dsl_authority.state(), &entry.agent_identity) {
            // A placed autonomous runtime has no controller-local state to
            // probe. If this helper is reached independently, require the
            // admission path; start_turn_with_correlation owns the remote send.
            return Ok(true);
        }

        #[cfg(feature = "runtime-adapter")]
        if let (Some(adapter), Some(session_id)) =
            (&self.runtime_adapter, member_ref.bridge_session_id())
        {
            use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;

            match adapter.runtime_state(session_id).await {
                Ok(meerkat_runtime::RuntimeState::Running) => {
                    tracing::debug!(
                        agent_identity = %entry.agent_identity,
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
                        agent_identity = %entry.agent_identity,
                        session_id = %session_id,
                        runtime_state = ?state,
                        session_active,
                        "active steer admission barrier checked non-running runtime state"
                    );
                    if session_active {
                        return Ok(true);
                    }
                    return Ok(false);
                }
                Err(error) => {
                    // Fail closed: an indeterminate runtime state must REQUIRE the
                    // admission barrier so the steer routes through real machine
                    // admission instead of bypassing it with a direct event inject.
                    // Bypassing on an unknown state would ack Completed before the
                    // machine admits the turn, so we demand admission whenever the
                    // runtime state cannot be determined.
                    tracing::debug!(
                        agent_identity = %entry.agent_identity,
                        session_id = %session_id,
                        error = %error,
                        "runtime state unavailable; requiring autonomous steer admission barrier (fail closed)"
                    );
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }
''','''    /// Detached admission-task version of the former inline
    /// `spawn_turn_admission_reply` body: the runtime (or placed transport)
    /// admission for one already machine-admitted turn.
    #[allow(clippy::too_many_arguments)]
    async fn execute_turn_admission(
        provisioner: &Arc<dyn MobProvisioner>,
        member_ref: MemberRef,
        req: Box<meerkat_core::service::StartTurnRequest>,
        completion_tx: Option<super::handle::ExactTurnCompletionSender>,
        llm_identity_applied_tx: Option<super::handle::MemberTurnLlmIdentityAppliedSender>,
        revival: Option<(mpsc::Sender<RoutedMobCommand>, AgentIdentity)>,
        placed_incarnation: Option<super::bridge_protocol::BridgeMemberIncarnation>,
        placed_input_id: Option<String>,
    ) -> Result<(), MobError> {
        debug_assert_eq!(revival.is_some(), placed_incarnation.is_some());
        let result = match (placed_incarnation, placed_input_id) {
            (Some(expected_member), Some(input_id)) => {
                if completion_tx.is_some() || llm_identity_applied_tx.is_some() {
                    Err(MobError::UnsupportedForMode {
                        mode: crate::MobRuntimeMode::TurnDriven,
                        reason: "tracked completion is not supported for remotely hosted members"
                            .to_string(),
                    })
                } else {
                    match placed_turn_supplied_interaction_id(&req) {
                        Ok(transcript_interaction_id) => {
                            let expected_receipt = input_id.clone();
                            match provisioner
                                .start_turn_with_correlation(
                                    &member_ref,
                                    *req,
                                    Some(super::provisioner::PlacedTurnDeliveryContext {
                                        input_id,
                                        transcript_interaction_id: transcript_interaction_id
                                            .map(|interaction_id| interaction_id.0.to_string()),
                                        expected_member,
                                        // IngressAccepted carries no retained
                                        // terminal-publication custody.
                                        outcome_tracking: None,
                                        bounded_result_spec: None,
                                    }),
                                )
                                .await
                            {
                                Ok(receipt)
                                    if receipt.as_deref() == Some(expected_receipt.as_str()) =>
                                {
                                    Ok(())
                                }
                                Ok(receipt) => Err(MobError::Internal(format!(
                                    "placed admission returned transport receipt {receipt:?}, expected '{expected_receipt}'"
                                ))),
                                Err(error) => Err(error),
                            }
                        }
                        Err(error) => Err(error),
                    }
                }
            }
            (None, None) => {
                if let Some(completion_tx) = completion_tx {
                    provisioner
                        .admit_tracked_turn(&member_ref, *req, completion_tx, llm_identity_applied_tx)
                        .await
                } else {
                    provisioner.admit_turn(&member_ref, *req).await
                }
            }
            _ => Err(MobError::Internal(
                "turn admission placement/transport fields drifted".to_string(),
            )),
        };
        if let Err(error) = &result {
            Self::fire_placed_revival_trigger(revival, error).await;
        }
        result
    }
''')

# remove old spawn_turn_admission_reply entirely
start = s.index('    fn spawn_turn_admission_reply(')
end = s.index('    async fn dispatch_turn_driven_spawn_initial_turn(')
s = s[:start] + s[end:]

# spawn_turn_completed_reply: readiness param + run at task start
rep('''    fn spawn_turn_completed_reply(
        &mut self,
        provisioner: Arc<dyn MobProvisioner>,
        member_ref: MemberRef,
        req: Box<meerkat_core::service::StartTurnRequest>,
        completion_tx: Option<super::handle::ExactTurnCompletionSender>,
        bounded_result_spec: Option<super::handle::BoundedResultSpec>,
        mut reply_tx: oneshot::Sender<Result<(), MobError>>,
        revival: Option<(mpsc::Sender<RoutedMobCommand>, AgentIdentity)>,
        remote: Option<PreparedPlacedCompletionWait>,
    ) {
        self.actor_io_tasks.spawn(async move {
            // Placed completion arrives with Record committed and its exact
''','''    fn spawn_turn_completed_reply(
        &mut self,
        provisioner: Arc<dyn MobProvisioner>,
        readiness: Option<LocalTurnAdmissionReadiness>,
        member_ref: MemberRef,
        req: Box<meerkat_core::service::StartTurnRequest>,
        completion_tx: Option<super::handle::ExactTurnCompletionSender>,
        bounded_result_spec: Option<super::handle::BoundedResultSpec>,
        mut reply_tx: oneshot::Sender<Result<(), MobError>>,
        revival: Option<(mpsc::Sender<RoutedMobCommand>, AgentIdentity)>,
        remote: Option<PreparedPlacedCompletionWait>,
    ) {
        let readiness_context = self.detached_member_readiness_context();
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
            {
                let _ = reply_tx.send(Err(error));
                return;
            }
            // Placed completion arrives with Record committed and its exact
''')

open(p,'w').write(s)
print("ok")
