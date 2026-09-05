import sys
p='meerkat-mob/src/runtime/actor.rs'
s=open(p).read()
pairs=[]
def add(old,new): pairs.append((old,new))

add('''    Autonomous(Box<AutonomousDispatchMaterial>),
}

struct ParkedMemberTurnAdmission {''','''    Autonomous(Box<AutonomousDispatchMaterial>),
    /// TurnCompleted-ack delivery (local tracked completion or placed
    /// completion with its actor-registered waiter). Routed through the same
    /// lane as IngressAccepted deliveries so a completion-bearing send cannot
    /// overtake parked ordinary deliveries to the same member. The
    /// completed-delivery body owns the reply channel because its placed
    /// branches reply at several distinct custody points.
    Completion {
        member_ref: MemberRef,
        req: Box<meerkat_core::service::StartTurnRequest>,
        completion_tx: Option<super::handle::ExactTurnCompletionSender>,
        bounded_result_spec: Option<super::handle::BoundedResultSpec>,
        placed_identity: Option<AgentIdentity>,
        remote: Option<PreparedPlacedCompletionWait>,
    },
}

struct ParkedMemberTurnAdmission {''')
add('''                            self.spawn_turn_completed_reply(
                                self.provisioner.clone(),
                                agent_identity,
                                readiness,
                                member_ref,
                                req,
                                completion_tx,
                                bounded_result_spec,
                                reply_tx,
                                placed_identity.map(|identity| (self.command_tx.clone(), identity)),
                                remote,
                            );
''','''                            self.enqueue_member_turn_admission(
                                Box::new(PendingMemberTurnAdmission {
                                    agent_identity,
                                    readiness,
                                    dispatch: PendingTurnDispatch::Completion {
                                        member_ref,
                                        req,
                                        completion_tx,
                                        bounded_result_spec,
                                        placed_identity,
                                        remote,
                                    },
                                }),
                                reply_tx,
                            );
''')
add('''        self.actor_io_tasks.spawn(async move {
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
            // exited; its lanes died with it.''','''        self.actor_io_tasks.spawn(async move {
            Self::run_member_turn_admission(
                &context,
                &session_service,
                &command_tx,
                *pending,
                reply_tx,
            )
            .await;
            // Release the lane on the actor. A closed channel means the actor
            // exited; its lanes died with it.''')
add('''    /// Body of one detached member turn admission: caller liveness, then the
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
''','''    /// Body of one detached member turn admission: caller liveness, then the
    /// member-local readiness steps that used to run inline, then the
    /// runtime admission, autonomous dispatch, or completion-bearing
    /// delivery. Owns the reply channel: a delivery whose caller is gone is
    /// never executed, and the completion body replies at its own custody
    /// points.
    async fn run_member_turn_admission(
        context: &DetachedMemberReadinessContext,
        session_service: &Arc<dyn MobSessionService>,
        command_tx: &mpsc::Sender<RoutedMobCommand>,
        pending: PendingMemberTurnAdmission,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    ) {
        let PendingMemberTurnAdmission {
            agent_identity,
            readiness,
            dispatch,
        } = pending;
        if reply_tx.is_closed() {
            // Parked long enough for the caller to leave: never run a ghost
            // turn. Equivalent to a runtime admission failing after the DSL
            // admission, which the machine already tolerates (an ingress
            // effect has no compensating transition).
            tracing::debug!(
                agent_identity = %agent_identity,
                "member delivery abandoned before its lane admission; not executed"
            );
            return;
        }
        if let Some(readiness) = readiness {
            let member_ref = match &dispatch {
                PendingTurnDispatch::Admission { member_ref, .. }
                | PendingTurnDispatch::Completion { member_ref, .. } => member_ref,
                PendingTurnDispatch::Autonomous(material) => &material.member_ref,
            };
            if let Err(error) = Self::run_local_turn_readiness(
                context,
                session_service,
                command_tx,
                &agent_identity,
                &readiness,
                member_ref,
            )
            .await
            {
                let _ = reply_tx.send(Err(error));
                return;
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
                let result = Self::execute_turn_admission(
                    &context.provisioner,
                    member_ref,
                    req,
                    completion_tx,
                    llm_identity_applied_tx,
                    placed_identity.map(|identity| (command_tx.clone(), identity)),
                    placed_incarnation,
                    placed_input_id,
                )
                .await;
                let _ = reply_tx.send(result);
            }
            PendingTurnDispatch::Autonomous(material) => {
                let result = context
                    .dispatch_autonomous(&agent_identity, *material)
                    .await;
                let _ = reply_tx.send(result);
            }
            PendingTurnDispatch::Completion {
                member_ref,
                req,
                completion_tx,
                bounded_result_spec,
                placed_identity,
                remote,
            } => {
                Self::execute_turn_completed_delivery(
                    Arc::clone(&context.provisioner),
                    member_ref,
                    req,
                    completion_tx,
                    bounded_result_spec,
                    reply_tx,
                    placed_identity.map(|identity| (command_tx.clone(), identity)),
                    remote,
                )
                .await;
            }
        }
    }
''')
add('''    #[allow(clippy::too_many_arguments)]
    fn spawn_turn_completed_reply(
        &mut self,
        provisioner: Arc<dyn MobProvisioner>,
        agent_identity: AgentIdentity,
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
            {
                let _ = reply_tx.send(Err(error));
                return;
            }
            // Placed completion arrives with Record committed and its exact
''','''    /// Lane body of a TurnCompleted-ack delivery (formerly the detached
    /// `spawn_turn_completed_reply` task). Readiness has already run in the
    /// member's lane; this owns `reply_tx` because its placed branches reply
    /// at distinct custody points and may outlive the caller's interest.
    #[allow(clippy::too_many_arguments)]
    async fn execute_turn_completed_delivery(
        provisioner: Arc<dyn MobProvisioner>,
        member_ref: MemberRef,
        req: Box<meerkat_core::service::StartTurnRequest>,
        completion_tx: Option<super::handle::ExactTurnCompletionSender>,
        bounded_result_spec: Option<super::handle::BoundedResultSpec>,
        mut reply_tx: oneshot::Sender<Result<(), MobError>>,
        revival: Option<(mpsc::Sender<RoutedMobCommand>, AgentIdentity)>,
        remote: Option<PreparedPlacedCompletionWait>,
    ) {
        {
            // Placed completion arrives with Record committed and its exact
''')
add('''            let _ = reply_tx.send(result);
        });
    }

    // The detached reply task receives the complete admitted turn tuple; keep
    // those independent carriers visible at the spawn boundary.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_turn_driven_spawn_initial_turn(''','''            let _ = reply_tx.send(result);
        }
    }

    // The detached reply task receives the complete admitted turn tuple; keep
    // those independent carriers visible at the spawn boundary.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_turn_driven_spawn_initial_turn(''')
# B2
add('''struct MemberReadinessTarget {
    entry: RosterEntry,
    stage: super::state::LifecycleProgressStage,
}
''','''#[derive(Clone)]
struct MemberReadinessTarget {
    entry: RosterEntry,
    stage: super::state::LifecycleProgressStage,
}

/// Drain a readiness fan-out. A task that cannot be joined (the readiness
/// tasks catch their own panics, so this means the fan-out itself was torn
/// down) still yields a typed failed outcome for its member instead of a
/// silently missing one, so an absent observation can never read as success.
async fn collect_member_readiness_outcomes(
    mut readiness: tokio::task::JoinSet<MemberReadinessOutcome>,
    mut expected: BTreeMap<AgentIdentity, MemberReadinessTarget>,
    mob_id: &MobId,
) -> Vec<MemberReadinessOutcome> {
    let mut outcomes = Vec::with_capacity(expected.len());
    while let Some(joined) = readiness.join_next().await {
        match joined {
            Ok(outcome) => {
                expected.remove(&outcome.entry.agent_identity);
                outcomes.push(outcome);
            }
            Err(error) => {
                tracing::error!(
                    mob_id = %mob_id,
                    error = %error,
                    "member readiness task could not be joined"
                );
            }
        }
    }
    for (agent_identity, MemberReadinessTarget { entry, stage }) in expected {
        outcomes.push(MemberReadinessOutcome {
            entry,
            stage,
            result: Err(MobError::Internal(format!(
                "member readiness task for '{agent_identity}' produced no outcome"
            ))),
        });
    }
    outcomes
}

fn member_readiness_expectations(
    targets: &[MemberReadinessTarget],
) -> BTreeMap<AgentIdentity, MemberReadinessTarget> {
    targets
        .iter()
        .map(|target| (target.entry.agent_identity.clone(), target.clone()))
        .collect()
}
''')
add('''        let mut readiness = self.spawn_member_readiness_tasks(targets, progress.cloned());
        let mut outcomes = Vec::new();
        while let Some(joined) = readiness.join_next().await {
            match joined {
                Ok(outcome) => outcomes.push(outcome),
                Err(error) => {
                    return Err(MobError::Internal(format!(
                        "member readiness task failed: {error}"
                    )));
                }
            }
        }
        self.apply_member_readiness_outcomes(outcomes).await
    }
''','''        let expected = member_readiness_expectations(&targets);
        let readiness = self.spawn_member_readiness_tasks(targets, progress.cloned());
        let outcomes =
            collect_member_readiness_outcomes(readiness, expected, &self.definition.id).await;
        self.apply_member_readiness_outcomes(outcomes).await
    }
''')
add('''        let mut readiness = self.spawn_member_readiness_tasks(targets, progress);
        let command_tx = self.command_tx.clone();
        let mob_id = self.definition.id.clone();
        self.actor_io_tasks.spawn(async move {
            let mut outcomes = Vec::new();
            while let Some(joined) = readiness.join_next().await {
                match joined {
                    Ok(outcome) => outcomes.push(outcome),
                    Err(error) => {
                        // Readiness tasks catch their own panics; a join
                        // error here means the fan-out itself was torn down.
                        tracing::error!(
                            mob_id = %mob_id,
                            error = %error,
                            "explicit resume readiness task could not be joined"
                        );
                    }
                }
            }
            let _ = command_tx''','''        let expected = member_readiness_expectations(&targets);
        let readiness = self.spawn_member_readiness_tasks(targets, progress);
        let command_tx = self.command_tx.clone();
        let mob_id = self.definition.id.clone();
        self.actor_io_tasks.spawn(async move {
            let outcomes = collect_member_readiness_outcomes(readiness, expected, &mob_id).await;
            let _ = command_tx''')
add('''        let readiness_result = self.apply_member_readiness_outcomes(outcomes).await;
        match phase {
            ResumeLifecyclePhase::PreCommitReadiness => {
                if let Err(error) = readiness_result {
                    self.rollback_resume_lifecycle_pre_commit(error, reply_tx)
                        .await;
                    return;
                }
                self.commit_resume_lifecycle(admission, progress, reply_tx, Vec::new())
                    .await;
            }
            ResumeLifecyclePhase::PostCommitReadiness { post_commit_error } => {
                let post_commit_error = post_commit_error.or(readiness_result.err());
                let result = self
                    .finish_resume_lifecycle_post_commit(&progress, post_commit_error)
                    .await;
                let _ = reply_tx.send(result);
            }
        }
    }
''','''        let readiness_result = self.apply_member_readiness_outcomes(outcomes).await;
        // Other commands ran while the fan-out was in flight. Re-probe the
        // lifecycle before continuing so a Stop/Complete/Reset/Destroy that
        // interleaved wins: pre-commit, the durable Resume must still be
        // admissible; post-commit, the topology/orchestrator reconciliation
        // must not run against a mob that has since left Running.
        match phase {
            ResumeLifecyclePhase::PreCommitReadiness => {
                if let Err(error) = readiness_result {
                    self.rollback_resume_lifecycle_pre_commit(error, reply_tx)
                        .await;
                    return;
                }
                if let Err(error) = self.probe_command_admission(
                    mob_dsl::MobMachineInput::Resume,
                    MobState::Running,
                    "resume_readiness_resolved_admission",
                ) {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        error = %error,
                        "lifecycle changed during the explicit resume readiness fan-out; resume not committed"
                    );
                    self.rollback_resume_lifecycle_pre_commit(error, reply_tx)
                        .await;
                    return;
                }
                self.commit_resume_lifecycle(admission, progress, reply_tx, Vec::new())
                    .await;
            }
            ResumeLifecyclePhase::PostCommitReadiness { post_commit_error } => {
                let current_phase = self.state();
                if current_phase != MobState::Running {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        phase = ?current_phase,
                        "lifecycle left Running during the explicit resume readiness fan-out; post-commit reconciliation skipped"
                    );
                    let _ = reply_tx.send(Err(MobError::LifecycleOperationPending {
                        intent: format!("explicit_resume superseded by {current_phase:?}"),
                    }));
                    return;
                }
                let post_commit_error = post_commit_error.or(readiness_result.err());
                let result = self
                    .finish_resume_lifecycle_post_commit(&progress, post_commit_error)
                    .await;
                let _ = reply_tx.send(result);
            }
        }
    }
''')
# B4 actor
add('''                MobCommand::ReloadMemberRegistration {
                    agent_identity,
                    reply_tx,
                } => {
                    match self.prepare_member_registration_reload(&agent_identity).await {
                        Ok(prepared) => self.spawn_member_registration_reload(prepared, reply_tx),''','''                MobCommand::ReloadMemberRegistration {
                    agent_identity,
                    deadline,
                    reply_tx,
                } => {
                    match self.prepare_member_registration_reload(&agent_identity).await {
                        Ok(prepared) => {
                            self.spawn_member_registration_reload(prepared, deadline, reply_tx)
                        }''')
add('''    fn spawn_member_registration_reload(
        &mut self,
        prepared: PreparedMemberRegistrationReload,
        reply_tx: oneshot::Sender<Result<super::handle::MemberReloadOutcome, MobError>>,
    ) {''','''    fn spawn_member_registration_reload(
        &mut self,
        prepared: PreparedMemberRegistrationReload,
        deadline: Instant,
        reply_tx: oneshot::Sender<Result<super::handle::MemberReloadOutcome, MobError>>,
    ) {''')
add('''        self.actor_io_tasks.spawn(async move {
            let deadline = Instant::now() + super::provisioner::MEMBER_RETIRE_TOTAL_TIMEOUT;
            let result = async {
                let disposition = context
                    .provisioner
                    .reload_degraded_runtime_registration(&member_ref, deadline)
                    .await?;
                if disposition == super::handle::MemberReloadDisposition::Discarded {''','''        self.actor_io_tasks.spawn(async move {
            // One end-to-end bound (`MEMBER_RELOAD_TOTAL_TIMEOUT`, set by the
            // handle) across probe, discard, revival and readiness; the stage
            // that misses it is named in the typed timeout.
            let stage = Arc::new(std::sync::Mutex::new("durability_reload_discard"));
            let set_stage = |next: &'static str| {
                *stage
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
            };
            let body = async {
                let disposition = context
                    .provisioner
                    .reload_degraded_runtime_registration(&member_ref, deadline)
                    .await?;
                if disposition == super::handle::MemberReloadDisposition::Discarded {
                    set_stage("live_session_revival");''')
add('''                    Self::request_member_live_revival(
                        &command_tx,
                        &entry.agent_identity,
                        &bridge_session_id,
                    )
                    .await?;
                    if entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost {''','''                    Self::request_member_live_revival(
                        &command_tx,
                        &entry.agent_identity,
                        &bridge_session_id,
                    )
                    .await?;
                    set_stage("runtime_readiness");
                    if entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost {''')
add('''                Ok(super::handle::MemberReloadOutcome {
                    disposition,
                    session_id: bridge_session_id.clone(),
                    generation: entry.agent_runtime_id.generation,
                })
            }
            .await;
            if let Err(error) = &result {''','''                Ok(super::handle::MemberReloadOutcome {
                    disposition,
                    session_id: bridge_session_id.clone(),
                    generation: entry.agent_runtime_id.generation,
                })
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            let result = match tokio::time::timeout(remaining, body).await {
                Ok(result) => result,
                Err(_elapsed) => Err(MobError::MemberReloadTimedOut {
                    session_id: bridge_session_id.clone(),
                    stage: *stage
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                }),
            };
            if let Err(error) = &result {''')
for i,(old,new) in enumerate(pairs):
    c=s.count(old)
    if c!=1:
        print(f"PAIR {i} count={c}\n---\n{old[:400]}\n---"); sys.exit(1)
    s=s.replace(old,new)
open(p,'w').write(s)
print("actor ok")
