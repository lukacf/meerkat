p='meerkat-mob/src/runtime/actor.rs'
s=open(p).read()

# ---------------- replace ensure_autonomous_runtimes_from_roster
start = s.index('    async fn ensure_autonomous_runtimes_from_roster(')
end = s.index('    /// Chokepoint (a) - MobCommand admission (DEC-P5E-4, ADJ-P5-12/15).')
new_fn = '''    /// Startup/post-commit inline form of the member readiness fan-out: run
    /// every member's bounded readiness step concurrently and apply the
    /// outcomes. Explicit Resume uses the split form
    /// (`spawn_resume_readiness_fanout` + `resume_lifecycle_readiness_resolved`)
    /// so the command loop keeps draining while members come up (#1102).
    async fn ensure_autonomous_runtimes_from_roster(
        &mut self,
        allow_stopped_resume_reopen: bool,
        progress: Option<&super::state::LifecycleProgressSignal>,
    ) -> Result<(), MobError> {
        let Some(targets) = self
            .collect_member_readiness_targets(allow_stopped_resume_reopen)
            .await?
        else {
            return Ok(());
        };
        let mut readiness = self.spawn_member_readiness_tasks(targets, progress.cloned());
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
        self.apply_member_readiness_outcomes(outcomes)
    }

    /// Resolve which local members need a readiness step and which step.
    /// `Ok(None)` means readiness is fenced by durable lifecycle intent and
    /// nothing must run.
    async fn collect_member_readiness_targets(
        &self,
        allow_stopped_resume_reopen: bool,
    ) -> Result<Option<Vec<MemberReadinessTarget>>, MobError> {
        let lifecycle = self.dsl_authority.state();
        if lifecycle_origin_fenced(lifecycle) {
            if !allow_stopped_resume_reopen {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    intent = ?lifecycle.placed_completion_lifecycle_intent,
                    "autonomous runtime startup remains fenced by durable lifecycle intent"
                );
                return Ok(None);
            }
            if self.state() != MobState::Stopped
                || lifecycle.placed_completion_lifecycle_intent
                    != Some(mob_dsl::PlacedCompletionLifecycleIntentKind::Stop)
            {
                return Err(MobError::LifecycleOperationPending {
                    intent: format!("{:?}", lifecycle.placed_completion_lifecycle_intent),
                });
            }
        }
        let broken_members = self
            .dsl_authority
            .state()
            .member_restore_failures
            .keys()
            .map(|identity| AgentIdentity::from(identity.0.as_str()))
            .collect::<HashSet<_>>();
        let placed_members = self
            .dsl_authority
            .state()
            .member_placement
            .keys()
            .map(|identity| AgentIdentity::from(identity.0.as_str()))
            .collect::<HashSet<_>>();
        let roster = self.roster.read().await;
        let targets = roster
            .list()
            .filter(|entry| {
                !broken_members.contains(&entry.agent_identity)
                    && !placed_members.contains(&entry.agent_identity)
            })
            .map(|entry| MemberReadinessTarget {
                entry: entry.clone(),
                // Turn-driven resumed members still need their mob-owned comms
                // drain rebound even though they do not need autonomous
                // dispatch; autonomous members need both, through one step.
                stage: if entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost {
                    super::state::LifecycleProgressStage::AutonomousRuntimeReadiness
                } else {
                    super::state::LifecycleProgressStage::MemberCommsReadiness
                },
            })
            .collect::<Vec<_>>();
        Ok(Some(targets))
    }

    /// Run every target's readiness step concurrently, each bounded by
    /// [`RESUME_MEMBER_READINESS_BOUND`]. A timeout is a confirmed-never
    /// fault (`ReadyWaitTimedOut`), not a resumable no-op, so Resume cannot
    /// report success when readiness was never observed. Readiness steps are
    /// idempotent ensures with no durable effect, so a panicking step is
    /// converted into that member's typed failure instead of taking the
    /// actor down.
    fn spawn_member_readiness_tasks(
        &self,
        targets: Vec<MemberReadinessTarget>,
        progress: Option<super::state::LifecycleProgressSignal>,
    ) -> tokio::task::JoinSet<MemberReadinessOutcome> {
        let mut readiness = tokio::task::JoinSet::new();
        let context = self.detached_member_readiness_context();
        for MemberReadinessTarget { entry, stage } in targets {
            let context = context.clone();
            let progress = progress.clone();
            readiness.spawn(async move {
                if let Some(progress) = &progress {
                    progress.awaiting_member(&entry.agent_identity, stage);
                }
                let agent_identity = entry.agent_identity.clone();
                let member_ref = entry.member_ref.clone();
                let work = async {
                    match stage {
                        super::state::LifecycleProgressStage::AutonomousRuntimeReadiness => {
                            context
                                .ensure_autonomous_runtime_ready(&agent_identity, &member_ref)
                                .await
                        }
                        _ => {
                            context
                                .provisioner
                                .ensure_runtime_session_state(&member_ref)
                                .await?;
                            context
                                .ensure_mob_comms_drain(&agent_identity, &member_ref)
                                .await
                        }
                    }
                };
                let result = match tokio::time::timeout(
                    RESUME_MEMBER_READINESS_BOUND,
                    std::panic::AssertUnwindSafe(work).catch_unwind(),
                )
                .await
                {
                    Ok(Ok(result)) => result,
                    Ok(Err(_panic)) => Err(MobError::Internal(format!(
                        "member readiness step panicked for '{agent_identity}'"
                    ))),
                    Err(_elapsed) => {
                        tracing::warn!(
                            agent_identity = %agent_identity,
                            stage = stage.as_str(),
                            bound_ms = RESUME_MEMBER_READINESS_BOUND.as_millis() as u64,
                            "timed out ensuring member runtime readiness"
                        );
                        Err(MobError::ReadyWaitTimedOut {
                            pending_member_ids: vec![AgentIdentity::from(
                                agent_identity.as_str(),
                            )],
                        })
                    }
                };
                if let Some(progress) = &progress {
                    progress.member_progress(&agent_identity, stage);
                }
                MemberReadinessOutcome {
                    entry,
                    stage,
                    result,
                }
            });
        }
        readiness
    }

    /// Apply concurrent readiness outcomes on the actor: log failures, keep
    /// the first typed error, and publish `StartupMarkReady` for every
    /// autonomous member whose readiness was observed while Running.
    ///
    /// Cold Running recovery rebuilds session runtime mechanics before the
    /// actor starts, then reaches this shared check without the volatile
    /// startup marker created by the original process. The successful
    /// observation feeds the same exact-runtime/fence machine transition used
    /// by fresh spawn. It is never inferred from the durable roster and never
    /// applied while the mob is Stopped: same-handle resume performs these
    /// checks before its durable Resume commit, while a rebuilt attachment
    /// checks again after it.
    fn apply_member_readiness_outcomes(
        &mut self,
        outcomes: Vec<MemberReadinessOutcome>,
    ) -> Result<(), MobError> {
        let mut first_error: Option<MobError> = None;
        for MemberReadinessOutcome {
            entry,
            stage,
            result,
        } in outcomes
        {
            if let Err(error) = result {
                tracing::warn!(
                    agent_identity = %entry.agent_identity,
                    stage = stage.as_str(),
                    error = %error,
                    "failed ensuring member runtime readiness"
                );
                first_error.get_or_insert(error);
                continue;
            }
            if stage != super::state::LifecycleProgressStage::AutonomousRuntimeReadiness
                || self.state() != MobState::Running
            {
                continue;
            }
            let runtime_id = mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
            if !self
                .dsl_authority
                .state()
                .member_startup_ready
                .contains(&runtime_id)
                && let Err(error) = self.apply_dsl_input(
                    mob_dsl::MobMachineInput::StartupMarkReady {
                        agent_runtime_id: runtime_id,
                        fence_token: mob_dsl::FenceToken::from_domain(entry.fence_token),
                    },
                    "ensure_autonomous_runtimes_from_roster/startup_mark_ready",
                )
            {
                tracing::warn!(
                    agent_identity = %entry.agent_identity,
                    error = %error,
                    "failed publishing autonomous runtime startup readiness"
                );
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    // ------------------------------------------------------------------
    // #1102: explicit Resume with the readiness fan-out off the loop.
    // ------------------------------------------------------------------

    async fn begin_resume_lifecycle(
        &mut self,
        deadline: Instant,
        admission: super::state::LifecycleAdmissionSignal,
        progress: super::state::LifecycleProgressSignal,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    ) {
        if self.pending_resume_lifecycle.is_some() {
            let _ = reply_tx.send(Err(MobError::LifecycleOperationPending {
                intent: "explicit_resume".to_string(),
            }));
            return;
        }
        if Instant::now() >= deadline {
            let _ = reply_tx.send(Err(MobError::LifecycleOperationAdmissionPending {
                intent: "explicit_resume".to_string(),
                stage: "actor_command_execution",
            }));
            return;
        }
        if let Err(error) = self.probe_command_admission(
            mob_dsl::MobMachineInput::Resume,
            MobState::Running,
            "resume_command_admission",
        ) {
            let _ = reply_tx.send(Err(error));
            return;
        }
        // Re-enable checkpointers cancelled during stop.
        self.provisioner.rearm_all_checkpointers().await;

        let rebuild = match self
            .prepare_explicit_resume_member_sessions(admission.clone(), &progress)
            .await
        {
            Ok(rebuild) => rebuild,
            Err(error) => {
                self.provisioner.cancel_all_checkpointers().await;
                let _ = reply_tx.send(Err(error));
                return;
            }
        };
        if !rebuild.is_empty() {
            // A reconstructed handle cannot become ready until its foreign
            // attachments have been retired and the durable Resume transition
            // authorizes the existing machine-owned revival seam.
            self.commit_resume_lifecycle(admission, progress, reply_tx, rebuild)
                .await;
            return;
        }
        // Same-handle resume preserves the prior pre-commit readiness
        // contract: every member's readiness is observed before the durable
        // Resume commit. The observations run concurrently off the loop.
        match self.collect_member_readiness_targets(true).await {
            Err(error) => {
                self.rollback_resume_lifecycle_pre_commit(error, reply_tx)
                    .await;
            }
            Ok(None) => {
                self.commit_resume_lifecycle(admission, progress, reply_tx, Vec::new())
                    .await;
            }
            Ok(Some(targets)) => {
                self.spawn_resume_readiness_fanout(
                    targets,
                    None,
                    PendingResumeLifecycle {
                        ticket: 0,
                        phase: ResumeLifecyclePhase::PreCommitReadiness,
                        admission,
                        progress,
                        reply_tx,
                    },
                );
            }
        }
    }

    async fn rollback_resume_lifecycle_pre_commit(
        &mut self,
        error: MobError,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    ) {
        if let Err(stop_error) = self.stop_all_autonomous_members_for_rollback().await {
            tracing::warn!(
                mob_id = %self.definition.id,
                error = %stop_error,
                "resume cleanup failed while stopping autonomous loops"
            );
        }
        self.provisioner.cancel_all_checkpointers().await;
        let _ = reply_tx.send(Err(error));
    }

    fn spawn_resume_readiness_fanout(
        &mut self,
        targets: Vec<MemberReadinessTarget>,
        progress: Option<super::state::LifecycleProgressSignal>,
        mut pending: PendingResumeLifecycle,
    ) {
        self.next_resume_lifecycle_ticket = self.next_resume_lifecycle_ticket.wrapping_add(1);
        let ticket = self.next_resume_lifecycle_ticket;
        pending.ticket = ticket;
        self.pending_resume_lifecycle = Some(pending);
        let mut readiness = self.spawn_member_readiness_tasks(targets, progress);
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
            let _ = command_tx
                .send(RoutedMobCommand::internal(
                    MobCommand::ResumeLifecycleReadinessResolved { ticket, outcomes },
                ))
                .await;
        });
    }

    async fn resume_lifecycle_readiness_resolved(
        &mut self,
        ticket: u64,
        outcomes: Vec<MemberReadinessOutcome>,
    ) {
        let Some(pending) = self
            .pending_resume_lifecycle
            .take_if(|pending| pending.ticket == ticket)
        else {
            tracing::warn!(
                mob_id = %self.definition.id,
                ticket,
                "ignoring stale explicit resume readiness resolution"
            );
            return;
        };
        let PendingResumeLifecycle {
            ticket: _,
            phase,
            admission,
            progress,
            reply_tx,
        } = pending;
        let readiness_result = self.apply_member_readiness_outcomes(outcomes);
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

    /// Resume's durable End{Stop}/Resumed carrier is the commit point. The
    /// external coordinator notification is not enqueued before it: an
    /// absent/failed carrier must leave both the mob and coordinator stopped.
    /// Same-handle pre-commit failures still roll back freshly restarted
    /// loops. A reconstructed handle may already have retired foreign
    /// attachments; failure remains a stopped, discoverable session set that
    /// an explicit retry can rebuild.
    async fn commit_resume_lifecycle(
        &mut self,
        admission: super::state::LifecycleAdmissionSignal,
        progress: super::state::LifecycleProgressSignal,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
        rebuild: Vec<ExplicitResumeMemberRebuild>,
    ) {
        let rebuilt_attachment = !rebuild.is_empty();
        admission.admit();
        progress.awaiting_stage(super::state::LifecycleProgressStage::DurableResumeTransition);
        if let Err(error) = self.resume_lifecycle_after_quiesce().await {
            if !rebuilt_attachment
                && let Err(stop_error) = self.stop_all_autonomous_members_for_rollback().await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    error = %stop_error,
                    "resume transition rollback failed while stopping autonomous loops"
                );
            }
            self.provisioner.cancel_all_checkpointers().await;
            let _ = reply_tx.send(Err(error));
            return;
        }

        if rebuilt_attachment {
            let post_commit_error = self
                .rebuild_explicit_resume_member_sessions(rebuild, &progress)
                .await
                .err();
            progress.awaiting_stage(super::state::LifecycleProgressStage::PostRebuildReadiness);
            match self.collect_member_readiness_targets(false).await {
                Err(error) => {
                    let result = self
                        .finish_resume_lifecycle_post_commit(
                            &progress,
                            post_commit_error.or(Some(error)),
                        )
                        .await;
                    let _ = reply_tx.send(result);
                }
                Ok(None) => {
                    let result = self
                        .finish_resume_lifecycle_post_commit(&progress, post_commit_error)
                        .await;
                    let _ = reply_tx.send(result);
                }
                Ok(Some(targets)) => {
                    self.spawn_resume_readiness_fanout(
                        targets,
                        Some(progress.clone()),
                        PendingResumeLifecycle {
                            ticket: 0,
                            phase: ResumeLifecyclePhase::PostCommitReadiness { post_commit_error },
                            admission,
                            progress,
                            reply_tx,
                        },
                    );
                }
            }
            return;
        }

        let result = self
            .finish_resume_lifecycle_post_commit(&progress, None)
            .await;
        let _ = reply_tx.send(result);
    }

    async fn finish_resume_lifecycle_post_commit(
        &mut self,
        progress: &super::state::LifecycleProgressSignal,
        mut post_commit_error: Option<MobError>,
    ) -> Result<(), MobError> {
        #[cfg(feature = "runtime-adapter")]
        {
            progress.awaiting_stage(
                super::state::LifecycleProgressStage::ResumeTopologyReconciliation,
            );
            // All exact session attachments are settled before topology
            // repair. The shared reconciler consumes its generated trust
            // handoffs directly; routing the same effects again here would
            // duplicate live mutations.
            let mut topology_roster = self.roster.read().await.snapshot();
            let topology_result = super::builder::reconcile_resume_topology(
                &self.definition,
                &mut topology_roster,
                self.provisioner.as_ref(),
                &self.supervisor_bridge,
                &self.runtime_metadata,
                &mut self.dsl_authority,
                &self.dsl_topology_epoch,
            )
            .await;
            *self.roster.write().await = RosterAuthority::from_roster(topology_roster);
            self.publish_machine_state_projection();
            if let Err(error) = topology_result
                && post_commit_error.is_none()
            {
                post_commit_error = Some(error);
            }
        }
        #[cfg(not(feature = "runtime-adapter"))]
        let _ = progress;

        // A cold actor reconstructed while Stopped skips the startup-time
        // operation-binding restore. Once explicit Resume has settled every
        // attachment and the shared topology seam has recovered the exact
        // current peer endpoint, rebuild those generated owner bindings
        // through the same seam used by a cold Running actor. Peer-only
        // members have no local bridge session of their own, so this is their
        // only durable route back to the owner bridge's operation registry
        // before respawn/retire. Binding after topology also avoids anchoring
        // a legacy pre-rebind address.
        if let Err(error) = self.restore_generated_member_operation_bindings().await
            && post_commit_error.is_none()
        {
            post_commit_error = Some(error);
        }

        if self.has_orchestrator {
            let orchestrator_transition_succeeded = match self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::ResumeOrchestrator,
                "resume_orchestrator_after_durable_resume",
            ) {
                Ok(()) => true,
                Err(error) => {
                    if post_commit_error.is_none() {
                        // The mob is durably Running; surface the local
                        // transition failure without pretending Resume rolled
                        // back.
                        post_commit_error = Some(MobError::Internal(format!(
                            "mob resumed durably but orchestrator ResumeOrchestrator transition failed: {error}"
                        )));
                    }
                    false
                }
            };
            if orchestrator_transition_succeeded && self.notify_orchestrator_on_resume {
                let orchestrator_entries = if let Some(orchestrator) =
                    self.definition.orchestrator.as_ref()
                {
                    let orchestrator_identities = self
                        .dsl_authority
                        .state()
                        .active_member_identities_for_profile(&orchestrator.profile);
                    let roster = self.roster.read().await;
                    orchestrator_identities
                        .into_iter()
                        .map(|orchestrator_identity| {
                            roster
                                .get(&orchestrator_identity)
                                .cloned()
                                .ok_or_else(|| {
                                    MobError::Internal(format!(
                                        "active MobMachine orchestrator '{orchestrator_identity}' has no mechanical roster entry during explicit resume"
                                    ))
                                })
                        })
                        .collect::<Result<Vec<_>, MobError>>()
                } else {
                    Ok(Vec::new())
                };
                match orchestrator_entries {
                    Ok(orchestrator_entries) => {
                        for orchestrator_entry in orchestrator_entries {
                            if let Err(error) =
                                super::builder::realize_orchestrator_resume_notification(
                                    self.definition.as_ref(),
                                    &orchestrator_entry,
                                    self.session_service.as_ref(),
                                    self.provisioner.as_ref(),
                                    &self.dsl_authority,
                                )
                                .await
                                && post_commit_error.is_none()
                            {
                                post_commit_error = Some(error);
                            }
                        }
                    }
                    Err(error) if post_commit_error.is_none() => {
                        post_commit_error = Some(error);
                    }
                    Err(_) => {}
                }
            }
        }
        post_commit_error.map_or(Ok(()), Err)
    }

'''
s = s[:start] + new_fn + s[end:]

# ---------------- replace the ResumeLifecycle arm
start = s.index('''                MobCommand::ResumeLifecycle {
                    deadline,
                    admission,
                    progress,
                    reply_tx,
                } => {''')
end = s.index('''                MobCommand::Complete { reply_tx } => {''')
new_arm = '''                MobCommand::ResumeLifecycle {
                    deadline,
                    admission,
                    progress,
                    reply_tx,
                } => {
                    self.inline_step_watchdog.set_step("resume_lifecycle");
                    // #1102: the per-member readiness fan-out runs detached;
                    // the reply is sent by whichever phase finishes the resume.
                    Box::pin(self.begin_resume_lifecycle(deadline, admission, progress, reply_tx))
                        .await;
                }
'''
s = s[:start] + new_arm + s[end:]
open(p,'w').write(s)
print("ok")
