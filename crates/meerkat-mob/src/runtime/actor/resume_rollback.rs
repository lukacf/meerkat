use super::*;

impl MobActor {
    pub(super) async fn begin_explicit_resume_rollback(
        &mut self,
        error: MobError,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
        progress: crate::runtime::state::LifecycleProgressSignal,
    ) {
        if self.pending_resume_rollback.is_some() {
            let _ = reply_tx.send(Err(MobError::LifecycleOperationPending {
                intent: "explicit_resume rollback already retains custody".to_string(),
            }));
            return;
        }
        let Some(attempt) = self.dsl_authority.state().explicit_resume_attempt.clone() else {
            let _ = reply_tx.send(Err(MobError::Internal(
                "resume rollback lost its machine attempt".to_string(),
            )));
            return;
        };
        let admitted = self
            .apply_dsl_input(
                mob_dsl::MobMachineInput::CancelExplicitResume {
                    attempt: attempt.clone(),
                },
                "cancel_resume_before_rollback",
            )
            .and_then(|()| {
                self.apply_dsl_input(
                    mob_dsl::MobMachineInput::BeginExplicitResumeCleanup {
                        attempt: attempt.clone(),
                    },
                    "begin_explicit_resume_cleanup",
                )
            });
        if let Err(error) = admitted {
            let _ = reply_tx.send(Err(error));
            return;
        }
        progress.awaiting_stage(crate::runtime::state::LifecycleProgressStage::ResumeRollback);
        self.pending_resume_rollback = Some(PendingResumeRollback {
            attempt: attempt.clone(),
            original_error: error,
            reply: ResumeRollbackReply::Pending(reply_tx),
            progress,
            deadline: Instant::now() + ROLLBACK_AUTONOMOUS_STOP_DEADLINE,
            in_flight: false,
        });
        self.drive_explicit_resume_rollback(attempt).await;
    }

    pub(super) fn retry_explicit_resume_rollback(&mut self) {
        if let Some(pending) = self.pending_resume_rollback.as_mut()
            && !pending.in_flight
        {
            pending.deadline = Instant::now() + ROLLBACK_AUTONOMOUS_STOP_DEADLINE;
            self.schedule_explicit_resume_rollback(Duration::ZERO);
        }
    }

    fn schedule_explicit_resume_rollback(&mut self, delay: Duration) {
        let Some(pending) = self.pending_resume_rollback.as_mut() else {
            return;
        };
        if pending.in_flight {
            return;
        }
        pending.in_flight = true;
        let attempt = pending.attempt.clone();
        let command_tx = self.command_tx.clone();
        self.actor_io_tasks.spawn(async move {
            tokio::time::sleep(delay).await;
            if command_tx
                .send(RoutedMobCommand::internal(
                    MobCommand::ResumeLifecycleRollbackStep { attempt },
                ))
                .await
                .is_err()
            {
                tracing::warn!("resume rollback retry lost its actor");
            }
        });
    }

    pub(super) async fn drive_explicit_resume_rollback(
        &mut self,
        attempt: mob_dsl::ResumeAttemptId,
    ) {
        let Some(pending) = self
            .pending_resume_rollback
            .as_mut()
            .filter(|pending| pending.attempt == attempt)
        else {
            tracing::warn!("ignoring stale resume rollback wake");
            return;
        };
        pending.in_flight = false;
        if Instant::now() >= pending.deadline {
            self.report_explicit_resume_cleanup_held(MobError::LifecycleOperationPending {
                intent: "explicit_resume rollback observation deadline reached".to_string(),
            });
            return;
        }
        let targets = match self.prepare_all_autonomous_member_stops().await {
            Ok(targets) => targets,
            Err(
                MobError::AutonomousStopInterruptsPending { .. }
                | MobError::PlacedKickoffCleanupPending { .. }
                | MobError::LifecycleOperationPending { .. },
            ) => {
                self.schedule_explicit_resume_rollback(ROLLBACK_AUTONOMOUS_STOP_POLL_INTERVAL);
                return;
            }
            Err(error) => {
                self.report_explicit_resume_cleanup_held(error);
                return;
            }
        };
        if let Some(pending) = self.pending_resume_rollback.as_mut() {
            pending.in_flight = true;
        }
        let context = self.detached_member_readiness_context();
        let command_tx = self.command_tx.clone();
        self.actor_io_tasks.spawn(async move {
            let outcomes =
                futures::future::join_all(targets.into_iter().map(|(identity, incarnation)| {
                    let context = context.clone();
                    async move {
                        let result = context
                            .finish_autonomous_member_stop(&identity, &incarnation)
                            .await;
                        ResumeRollbackMemberOutcome {
                            identity,
                            incarnation,
                            result,
                        }
                    }
                }))
                .await;
            context.provisioner.cancel_all_checkpointers().await;
            if command_tx
                .send(RoutedMobCommand::internal(
                    MobCommand::ResumeLifecycleRollbackFinalized { attempt, outcomes },
                ))
                .await
                .is_err()
            {
                tracing::warn!("resume rollback finalization settled after its actor stopped");
            }
        });
    }

    pub(super) fn explicit_resume_rollback_finalized(
        &mut self,
        attempt: mob_dsl::ResumeAttemptId,
        outcomes: Vec<ResumeRollbackMemberOutcome>,
    ) {
        let Some(pending) = self
            .pending_resume_rollback
            .as_mut()
            .filter(|pending| pending.attempt == attempt)
        else {
            tracing::warn!("ignoring stale resume rollback finalization");
            return;
        };
        pending.in_flight = false;
        let mut failure = None;
        for outcome in outcomes {
            if self.autonomous_stop_interrupted.get(&outcome.identity) != Some(&outcome.incarnation)
            {
                failure.get_or_insert_with(|| MobError::LifecycleOperationPending {
                    intent: format!("member {} changed during resume rollback", outcome.identity),
                });
                continue;
            }
            match outcome.result {
                Ok(()) => {
                    self.autonomous_stop_interrupted.remove(&outcome.identity);
                    pending.progress.member_progress(
                        &outcome.identity,
                        crate::runtime::state::LifecycleProgressStage::ResumeRollback,
                    );
                }
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        if let Some(error) = failure {
            if matches!(
                error,
                MobError::AutonomousStopInterruptsPending { .. }
                    | MobError::PlacedKickoffCleanupPending { .. }
                    | MobError::LifecycleOperationPending { .. }
            ) && Instant::now() < pending.deadline
            {
                self.schedule_explicit_resume_rollback(ROLLBACK_AUTONOMOUS_STOP_POLL_INTERVAL);
            } else {
                self.report_explicit_resume_cleanup_held(error);
            }
            return;
        }
        if let Err(error) = self.apply_dsl_input(
            mob_dsl::MobMachineInput::SettleExplicitResumeCleanup { attempt },
            "settle_explicit_resume_cleanup",
        ) {
            self.report_explicit_resume_cleanup_held(error);
            return;
        }
        let Some(pending) = self.pending_resume_rollback.take() else {
            self.durable_uncertainty_fail_stop = true;
            tracing::error!("resume rollback lost its settled continuation");
            return;
        };
        let result = self.finish_explicit_resume_attempt(Err(pending.original_error));
        if let ResumeRollbackReply::Pending(reply_tx) = pending.reply {
            let _ = reply_tx.send(result);
        } else if let Err(error) = result {
            tracing::info!(%error, "previously reported resume failure now has settled cleanup");
        }
    }

    fn report_explicit_resume_cleanup_held(&mut self, error: MobError) {
        let Some(pending) = self.pending_resume_rollback.as_mut() else {
            tracing::error!(%error, "resume cleanup error has no retained continuation");
            self.durable_uncertainty_fail_stop = true;
            return;
        };
        pending.in_flight = false;
        let message = format!(
            "explicit_resume cleanup retained: {error}; original failure: {}",
            pending.original_error
        );
        tracing::error!(%message, "resume cancellation remains pending");
        let reply = std::mem::replace(&mut pending.reply, ResumeRollbackReply::Reported);
        if let ResumeRollbackReply::Pending(reply_tx) = reply {
            let _ = reply_tx.send(Err(MobError::LifecycleOperationPending {
                intent: message.clone(),
            }));
        }
        for control in self.pending_resume_controls.drain(..) {
            control
                .cmd
                .reject_with_error(MobError::LifecycleOperationPending {
                    intent: message.clone(),
                });
        }
    }
}
