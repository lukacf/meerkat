use super::*;

impl MobActor {
    pub(super) async fn begin_resume_post_topology_members(
        &mut self,
        mut pending: PendingResumeLifecycle,
        topology_result: Result<(), MobError>,
    ) {
        let ResumeLifecyclePhase::PostCommitTopology { post_commit_error } = pending.phase else {
            self.durable_uncertainty_fail_stop = true;
            tracing::error!("post-topology work lost its matching continuation");
            return;
        };
        pending.phase = ResumeLifecyclePhase::PostCommitMembers {
            step: ResumePostCommitStep::OperationBindings,
            post_commit_error: post_commit_error.or(topology_result.err()),
        };
        pending
            .progress
            .awaiting_stage(crate::runtime::state::LifecycleProgressStage::ResumeOperationBindings);
        self.pending_resume_lifecycle = Some(pending);
        if self.dsl_authority.state().explicit_resume_cancel_requested {
            self.finish_resume_post_commit_members();
            return;
        }
        #[cfg(feature = "runtime-adapter")]
        if let Some(io) = self.resume_operation_binding_io() {
            let entries = self.roster.read().await.list().cloned().collect::<Vec<_>>();
            for entry in entries {
                match self.prepare_restored_member_operation_binding(entry) {
                    Ok(Some(plan)) => {
                        let identity = plan.entry.agent_identity.clone();
                        let io = io.clone();
                        self.spawn_resume_post_commit_member(
                            ResumePostCommitStep::OperationBindings,
                            identity,
                            boxed_arm_future(|| async move {
                                Self::realize_restored_member_operation_binding(io, plan).await
                            }),
                        );
                    }
                    Ok(None) => {}
                    Err(error) => self.retain_resume_post_commit_error(error),
                }
            }
        }
        if self.resume_post_commit_member_tasks.is_empty() {
            self.begin_resume_orchestrator_notifications().await;
        }
    }

    fn retain_resume_post_commit_error(&mut self, error: MobError) {
        tracing::warn!(%error, "resume post-commit work failed");
        match self
            .pending_resume_lifecycle
            .as_mut()
            .map(|pending| &mut pending.phase)
        {
            Some(ResumeLifecyclePhase::PostCommitMembers {
                post_commit_error, ..
            }) => {
                post_commit_error.get_or_insert(error);
            }
            _ => {
                self.durable_uncertainty_fail_stop = true;
            }
        }
    }

    fn spawn_resume_post_commit_member(
        &mut self,
        step: ResumePostCommitStep,
        identity: AgentIdentity,
        action: ActorCommandFuture<'static, Result<(), MobError>>,
    ) {
        let Some(attempt) = self.dsl_authority.state().explicit_resume_attempt.clone() else {
            self.durable_uncertainty_fail_stop = true;
            tracing::error!("post-commit member work has no resume authority");
            return;
        };
        if !self
            .resume_post_commit_member_tasks
            .insert(identity.clone())
        {
            self.durable_uncertainty_fail_stop = true;
            tracing::error!(%identity, "duplicate post-commit member task");
            return;
        }
        let worker = tokio::spawn(action);
        let command_tx = self.command_tx.clone();
        self.actor_io_tasks.spawn(async move {
            let outcome = match worker.await {
                Ok(result) => ResumePostCommitMemberOutcome::Settled(result),
                Err(error) => ResumePostCommitMemberOutcome::OwnerLost(error.to_string()),
            };
            if command_tx
                .send(RoutedMobCommand::internal(
                    MobCommand::ResumePostCommitMemberCompleted {
                        attempt,
                        step,
                        identity,
                        outcome,
                    },
                ))
                .await
                .is_err()
            {
                tracing::warn!("resume post-commit owner finished after the actor stopped");
            }
        });
    }

    pub(super) async fn resume_post_commit_member_completed(
        &mut self,
        attempt: mob_dsl::ResumeAttemptId,
        step: ResumePostCommitStep,
        identity: AgentIdentity,
        outcome: ResumePostCommitMemberOutcome,
    ) {
        let current_step =
            self.pending_resume_lifecycle
                .as_ref()
                .and_then(|pending| match &pending.phase {
                    ResumeLifecyclePhase::PostCommitMembers { step, .. } => Some(*step),
                    _ => None,
                });
        if self.dsl_authority.state().explicit_resume_attempt.as_ref() != Some(&attempt)
            || current_step != Some(step)
            || !self.resume_post_commit_member_tasks.contains(&identity)
        {
            tracing::warn!(%identity, "ignoring stale post-commit resume completion");
            return;
        }
        let result = match outcome {
            ResumePostCommitMemberOutcome::Settled(result) => result,
            ResumePostCommitMemberOutcome::OwnerLost(error) => {
                tracing::error!(%identity, %error, "post-commit owner disappeared without settlement");
                self.durable_uncertainty_fail_stop = true;
                return;
            }
        };
        self.resume_post_commit_member_tasks.remove(&identity);
        if let Err(error) = result {
            self.retain_resume_post_commit_error(error);
        }
        if let Some(pending) = self.pending_resume_lifecycle.as_ref() {
            pending.progress.member_progress(
                &identity,
                match step {
                    ResumePostCommitStep::OperationBindings => {
                        crate::runtime::state::LifecycleProgressStage::ResumeOperationBindings
                    }
                    ResumePostCommitStep::OrchestratorNotification => {
                        crate::runtime::state::LifecycleProgressStage::OrchestratorResumeNotification
                    }
                },
            );
        }
        if self.resume_post_commit_member_tasks.is_empty() {
            match step {
                ResumePostCommitStep::OperationBindings => {
                    self.begin_resume_orchestrator_notifications().await;
                }
                ResumePostCommitStep::OrchestratorNotification => {
                    self.finish_resume_post_commit_members();
                }
            }
        }
    }

    async fn begin_resume_orchestrator_notifications(&mut self) {
        let Some(pending) = self.pending_resume_lifecycle.as_mut() else {
            self.durable_uncertainty_fail_stop = true;
            return;
        };
        let ResumeLifecyclePhase::PostCommitMembers { step, .. } = &mut pending.phase else {
            self.durable_uncertainty_fail_stop = true;
            return;
        };
        *step = ResumePostCommitStep::OrchestratorNotification;
        pending.progress.awaiting_stage(
            crate::runtime::state::LifecycleProgressStage::OrchestratorResumeNotification,
        );
        if self.dsl_authority.state().explicit_resume_cancel_requested || !self.has_orchestrator {
            self.finish_resume_post_commit_members();
            return;
        }
        if let Err(error) = self.apply_dsl_signal(
            mob_dsl::MobMachineSignal::ResumeOrchestrator,
            "resume_orchestrator_after_durable_resume",
        ) {
            self.retain_resume_post_commit_error(error);
            self.finish_resume_post_commit_members();
            return;
        }
        if self.notify_orchestrator_on_resume {
            let identities = self
                .definition
                .orchestrator
                .as_ref()
                .map(|orchestrator| {
                    self.dsl_authority
                        .state()
                        .active_member_identities_for_profile(&orchestrator.profile)
                })
                .unwrap_or_default();
            for identity in identities {
                let entry = self.roster.read().await.get(&identity).cloned();
                let plan = match entry {
                    Some(entry) => crate::runtime::builder::plan_orchestrator_resume_notification(
                        self.definition.as_ref(),
                        &entry,
                        &self.dsl_authority,
                    ),
                    None => Err(MobError::Internal(format!(
                        "active orchestrator '{identity}' has no roster entry during resume"
                    ))),
                };
                match plan {
                    Ok(Some(plan)) => {
                        let service = Arc::clone(&self.session_service);
                        let provisioner = Arc::clone(&self.provisioner);
                        self.spawn_resume_post_commit_member(
                            ResumePostCommitStep::OrchestratorNotification,
                            identity,
                            boxed_arm_future(|| async move {
                                crate::runtime::builder::realize_planned_orchestrator_resume_notification(
                                    plan, service.as_ref(), provisioner.as_ref(),
                                ).await
                            }),
                        );
                    }
                    Ok(None) => {}
                    Err(error) => self.retain_resume_post_commit_error(error),
                }
            }
        }
        if self.resume_post_commit_member_tasks.is_empty() {
            self.finish_resume_post_commit_members();
        }
    }

    fn finish_resume_post_commit_members(&mut self) {
        if !self.resume_post_commit_member_tasks.is_empty() {
            tracing::error!("resume post-commit completion still has owned member tasks");
            self.durable_uncertainty_fail_stop = true;
            return;
        }
        let Some(pending) = self.pending_resume_lifecycle.take() else {
            self.durable_uncertainty_fail_stop = true;
            return;
        };
        let ResumeLifecyclePhase::PostCommitMembers {
            post_commit_error, ..
        } = pending.phase
        else {
            self.durable_uncertainty_fail_stop = true;
            return;
        };
        let result = if self.dsl_authority.state().explicit_resume_cancel_requested {
            Err(MobError::LifecycleOperationPending {
                intent: "explicit_resume post-commit work superseded by lifecycle control"
                    .to_string(),
            })
        } else {
            post_commit_error.map_or(Ok(()), Err)
        };
        let settled = self.apply_explicit_resume_input(
            |attempt| mob_dsl::MobMachineInput::SettleExplicitResumeTopology { attempt },
            "settle_explicit_resume_post_commit",
        );
        let result = settled.and_then(|()| self.finish_explicit_resume_attempt(result));
        let _ = pending.reply_tx.send(result);
    }
}
