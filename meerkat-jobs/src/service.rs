use std::sync::Arc;

use crate::machines::detached_job as dsl;
use crate::store::{InsertJobOutcome, StoredJob};
use crate::{
    AttemptClaim, AttemptClaimReceipt, AttemptId, AttemptWriteAuthority, CheckpointRef,
    DetachedJobError, DetachedJobStore, FenceToken, JobDescription, JobFailureCode,
    JobHealthCondition, JobHealthSnapshot, JobId, JobNotification, JobNotificationReceipt,
    JobOutboxEntry, JobOutboxPayload, JobProgress, JobReceipt, JobReference, JobResultRef,
    JobSnapshot, JobSubscription, JobSubscriptionId, JobTerminalResult, NotificationId,
    PredicateEvaluation, PredicateEvaluationReceipt, PredicateObservation, PredicateWatch,
};

#[derive(Clone)]
pub struct DetachedJobService {
    store: Arc<dyn DetachedJobStore>,
}

impl std::fmt::Debug for DetachedJobService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DetachedJobService").finish_non_exhaustive()
    }
}

impl DetachedJobService {
    pub fn new(store: Arc<dyn DetachedJobStore>) -> Self {
        Self { store }
    }

    pub async fn submit(&self, spec: crate::JobSpec) -> Result<JobReceipt, DetachedJobError> {
        if spec.realm_id.is_empty()
            || spec.realm_id.trim() != spec.realm_id
            || spec.realm_id.chars().any(char::is_control)
        {
            return Err(DetachedJobError::InvalidInput(
                "realm id must be non-empty, canonical, and contain no control characters".into(),
            ));
        }
        let job_id = JobId::generated();
        let mut authority = dsl::DetachedJobMachineAuthority::new();
        dsl::DetachedJobMachineMutator::apply(
            &mut authority,
            dsl::DetachedJobInput::Submit {
                job_id: job_id.as_str().to_string(),
                restart_class: spec.restart_class,
            },
        )
        .map_err(|error| DetachedJobError::InvalidTransition {
            job_id: job_id.clone(),
            detail: format!("{error:?}"),
        })?;
        let stored = StoredJob {
            job_id: job_id.clone(),
            spec,
            revision: 1,
            machine_state: authority.state().clone(),
            progress: None,
            terminal_result: None,
            subscriptions: Vec::new(),
            outbox: Vec::new(),
        };
        match self.store.insert_deduplicated(stored).await? {
            InsertJobOutcome::Inserted(inserted) => Ok(JobReceipt {
                job_id: inserted.job_id,
                phase: inserted.machine_state.lifecycle_phase,
                deduplicated: false,
                restart_class: inserted.spec.restart_class,
            }),
            InsertJobOutcome::Existing(existing) => Ok(JobReceipt {
                job_id: existing.job_id,
                phase: existing.machine_state.lifecycle_phase,
                deduplicated: true,
                restart_class: existing.spec.restart_class,
            }),
        }
    }

    pub async fn get(&self, job_id: &JobId) -> Result<Option<JobSnapshot>, DetachedJobError> {
        self.store.get(job_id).await?.map(job_snapshot).transpose()
    }

    /// Resolve a realm-qualified job reference for its owning session.
    ///
    /// Cross-realm and cross-session references deliberately collapse to
    /// `NotFound`, so callers cannot use the await surface as an existence
    /// oracle for jobs they do not own.
    pub async fn get_authorized_for_session(
        &self,
        reference: &JobReference,
        session_id: &meerkat_core::SessionId,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let stored = self
            .store
            .get(reference.job_id())
            .await?
            .filter(|job| {
                job.spec.realm_id == reference.realm_id()
                    && &job.spec.origin_session_id == session_id
            })
            .ok_or_else(|| DetachedJobError::NotFound(reference.job_id().clone()))?;
        job_snapshot(stored)
    }

    /// Resolve a realm-qualified reference for a trusted cross-domain
    /// projector. Public/session-facing callers should use
    /// [`Self::get_authorized_for_session`] instead.
    pub async fn get_for_reference(
        &self,
        reference: &JobReference,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let stored = self
            .store
            .get(reference.job_id())
            .await?
            .filter(|job| job.spec.realm_id == reference.realm_id())
            .ok_or_else(|| DetachedJobError::NotFound(reference.job_id().clone()))?;
        job_snapshot(stored)
    }

    pub async fn describe(
        &self,
        job_id: &JobId,
    ) -> Result<Option<JobDescription>, DetachedJobError> {
        self.store
            .get(job_id)
            .await?
            .map(job_description)
            .transpose()
    }

    pub async fn describe_for_realm(
        &self,
        realm_id: &str,
        job_id: &JobId,
    ) -> Result<Option<JobDescription>, DetachedJobError> {
        self.store
            .get(job_id)
            .await?
            .filter(|job| job.spec.realm_id == realm_id)
            .map(job_description)
            .transpose()
    }

    pub async fn list_descriptions_for_origin(
        &self,
        realm_id: &str,
        origin_session_id: &meerkat_core::SessionId,
        limit: usize,
    ) -> Result<Vec<JobDescription>, DetachedJobError> {
        self.store
            .list_for_origin(realm_id, origin_session_id, limit)
            .await?
            .into_iter()
            .map(job_description)
            .collect()
    }

    /// Derive operational health from committed generated-machine state.
    ///
    /// This is deliberately a projection: it neither expires a lease nor
    /// claims/retries work. Those decisions remain explicit machine inputs.
    pub async fn health_snapshot(
        &self,
        observed_at_ms: u64,
        limit: usize,
    ) -> Result<JobHealthSnapshot, DetachedJobError> {
        self.health_snapshot_for_realm_filter(None, observed_at_ms, limit)
            .await
    }

    pub async fn health_snapshot_for_realm(
        &self,
        realm_id: &str,
        observed_at_ms: u64,
        limit: usize,
    ) -> Result<JobHealthSnapshot, DetachedJobError> {
        self.health_snapshot_for_realm_filter(Some(realm_id), observed_at_ms, limit)
            .await
    }

    async fn health_snapshot_for_realm_filter(
        &self,
        realm_id: Option<&str>,
        observed_at_ms: u64,
        limit: usize,
    ) -> Result<JobHealthSnapshot, DetachedJobError> {
        let jobs = self.store.list_all(limit).await?;
        let mut health = JobHealthSnapshot::default();
        for job in jobs
            .into_iter()
            .filter(|job| realm_id.is_none_or(|realm_id| job.spec.realm_id == realm_id))
        {
            let state = &job.machine_state;
            match state.lifecycle_phase {
                dsl::DetachedJobPhase::Queued | dsl::DetachedJobPhase::RetryScheduled => {
                    health.queued = health.queued.saturating_add(1);
                }
                dsl::DetachedJobPhase::Running | dsl::DetachedJobPhase::WaitingExternal => {
                    health.running = health.running.saturating_add(1);
                    if state
                        .lease_expires_at_ms
                        .is_some_and(|expires_at_ms| expires_at_ms < observed_at_ms)
                    {
                        health.stale_leases = health.stale_leases.saturating_add(1);
                    }
                }
                dsl::DetachedJobPhase::NeedsAttention => {
                    health.needs_attention = health.needs_attention.saturating_add(1);
                }
                dsl::DetachedJobPhase::Unsubmitted
                | dsl::DetachedJobPhase::Claimed
                | dsl::DetachedJobPhase::LossObserved
                | dsl::DetachedJobPhase::Succeeded
                | dsl::DetachedJobPhase::Failed
                | dsl::DetachedJobPhase::Cancelled
                | dsl::DetachedJobPhase::WorkerLost => {}
            }
            health.delivery_backlog = health.delivery_backlog.saturating_add(
                u64::try_from(job.outbox.iter().filter(|entry| !entry.applied).count())
                    .unwrap_or(u64::MAX),
            );
        }
        Ok(health)
    }

    pub async fn subscribe(
        &self,
        job_id: &JobId,
        subscription: JobSubscription,
    ) -> Result<JobSnapshot, DetachedJobError> {
        loop {
            let mut current = self.required(job_id).await?;
            if let Some(existing) = current
                .subscriptions
                .iter()
                .find(|existing| existing.subscription_id() == subscription.subscription_id())
            {
                if existing == &subscription {
                    return job_snapshot(current);
                }
                return Err(DetachedJobError::InvalidInput(format!(
                    "subscription {} is already bound to a different target",
                    subscription.subscription_id()
                )));
            }
            current.subscriptions.push(subscription.clone());
            let expected_revision = current.revision;
            match self
                .store
                .compare_and_swap(expected_revision, current)
                .await
            {
                Ok(committed) => return job_snapshot(committed),
                Err(DetachedJobError::StaleRevision { .. }) => {
                    // Subscription registration composes with terminal
                    // commitment. Reloading is mechanical: if terminality won
                    // the race, the returned snapshot carries that generated
                    // truth and the await owner can resolve from it directly.
                    // No attempt, retry, or fence transition is invented here.
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub async fn unsubscribe(
        &self,
        job_id: &JobId,
        subscription_id: &JobSubscriptionId,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let mut current = self.required(job_id).await?;
        let previous_len = current.subscriptions.len();
        current
            .subscriptions
            .retain(|subscription| subscription.subscription_id() != subscription_id);
        if current.subscriptions.len() == previous_len {
            return job_snapshot(current);
        }
        let expected_revision = current.revision;
        let committed = self
            .store
            .compare_and_swap(expected_revision, current)
            .await?;
        job_snapshot(committed)
    }

    pub async fn claim_attempt(
        &self,
        job_id: &JobId,
        claim: AttemptClaim,
    ) -> Result<AttemptClaimReceipt, DetachedJobError> {
        let attempt_id = AttemptId::generated();
        let input = dsl::DetachedJobInput::ClaimAttempt {
            attempt_id: attempt_id.as_str().to_string(),
            worker_id: claim.worker_id.as_str().to_string(),
            claimed_at_ms: claim.claimed_at_ms,
            lease_expires_at_ms: claim.lease_expires_at_ms,
            runner_handle: claim.runner_handle.as_str().to_string(),
        };
        let (stored, effects) = self.apply(job_id, input, |_, _| Ok(())).await?;
        let mut claimed = None;
        for effect in effects {
            if let dsl::DetachedJobEffect::AttemptClaimed {
                attempt_id: emitted_attempt_id,
                attempt_count,
                fence,
                lease_expires_at_ms,
                resume_checkpoint,
            } = effect
            {
                if claimed.is_some() {
                    return Err(DetachedJobError::InvalidTransition {
                        job_id: job_id.clone(),
                        detail: "machine emitted multiple claim acknowledgements".into(),
                    });
                }
                claimed = Some(AttemptClaimReceipt {
                    attempt_id: AttemptId::new(emitted_attempt_id)?,
                    attempt_count,
                    fence: FenceToken::new(fence),
                    lease_expires_at_ms,
                    resume_checkpoint: resume_checkpoint.map(CheckpointRef::new).transpose()?,
                });
            }
        }
        let receipt = claimed.ok_or_else(|| DetachedJobError::InvalidTransition {
            job_id: job_id.clone(),
            detail: "machine emitted no claim acknowledgement".into(),
        })?;
        debug_assert_eq!(stored.machine_state.attempt_count, receipt.attempt_count);
        Ok(receipt)
    }

    pub async fn report_progress(
        &self,
        job_id: &JobId,
        write: AttemptWriteAuthority,
        progress: JobProgress,
        observed_at_ms: u64,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let current = self.required(job_id).await?;
        ensure_current_writer(job_id, &current, &write)?;
        let input = dsl::DetachedJobInput::ReportProgress {
            attempt_id: write.attempt_id.as_str().to_string(),
            fence: write.fence.get(),
            cursor: progress.cursor,
            observed_at_ms,
        };
        let (stored, _) = self
            .apply_from(current, input, move |job, _| {
                job.progress = Some(progress);
                Ok(())
            })
            .await?;
        job_snapshot(stored)
    }

    pub async fn renew_lease(
        &self,
        job_id: &JobId,
        write: AttemptWriteAuthority,
        heartbeat_at_ms: u64,
        lease_expires_at_ms: u64,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let current = self.required(job_id).await?;
        ensure_current_writer(job_id, &current, &write)?;
        let input = dsl::DetachedJobInput::RenewLease {
            attempt_id: write.attempt_id.as_str().to_string(),
            fence: write.fence.get(),
            heartbeat_at_ms,
            lease_expires_at_ms,
        };
        let (stored, _) = self.apply_from(current, input, |_, _| Ok(())).await?;
        job_snapshot(stored)
    }

    pub async fn record_checkpoint(
        &self,
        job_id: &JobId,
        write: AttemptWriteAuthority,
        checkpoint_ref: CheckpointRef,
        observed_at_ms: u64,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let current = self.required(job_id).await?;
        ensure_current_writer(job_id, &current, &write)?;
        let input = dsl::DetachedJobInput::RecordCheckpoint {
            attempt_id: write.attempt_id.as_str().to_string(),
            fence: write.fence.get(),
            checkpoint_ref: checkpoint_ref.as_str().to_string(),
            observed_at_ms,
        };
        let (stored, _) = self.apply_from(current, input, |_, _| Ok(())).await?;
        job_snapshot(stored)
    }

    pub async fn emit_notification(
        &self,
        job_id: &JobId,
        write: AttemptWriteAuthority,
        observed_at_ms: u64,
        notification: JobNotification,
    ) -> Result<JobNotificationReceipt, DetachedJobError> {
        let current = self.required(job_id).await?;
        ensure_current_writer(job_id, &current, &write)?;
        let runtime_delivery_id =
            format!("{}:notification:{}", job_id, notification.notification_id());
        let input = dsl::DetachedJobInput::EmitNotification {
            attempt_id: write.attempt_id.as_str().to_string(),
            fence: write.fence.get(),
            notification_id: notification.notification_id().as_str().to_string(),
            idempotency_key: notification.idempotency_key().to_string(),
            runtime_delivery_id,
            observed_at_ms,
        };
        let projected = notification.clone();
        let (stored, effects) = self
            .apply_from(current, input, move |job, effects| {
                project_notification(job, effects, &projected)
            })
            .await?;
        let mut outcome = effects.iter().filter_map(|effect| match effect {
            dsl::DetachedJobEffect::NotificationCommitted {
                notification_id,
                delivery_sequence,
                ..
            } => Some((notification_id.as_str(), *delivery_sequence, false)),
            dsl::DetachedJobEffect::NotificationSuppressed {
                notification_id,
                delivery_sequence,
                ..
            } => Some((notification_id.as_str(), *delivery_sequence, true)),
            _ => None,
        });
        let Some((notification_id, delivery_sequence, deduplicated)) = outcome.next() else {
            return Err(DetachedJobError::InvalidTransition {
                job_id: job_id.clone(),
                detail: "machine emitted no notification disposition".into(),
            });
        };
        if outcome.next().is_some() {
            return Err(DetachedJobError::Store(
                "machine emitted multiple notification dispositions".into(),
            ));
        }
        Ok(JobNotificationReceipt {
            snapshot: job_snapshot(stored)?,
            notification_id: NotificationId::new(notification_id)?,
            delivery_sequence,
            deduplicated,
        })
    }

    pub async fn evaluate_predicate(
        &self,
        job_id: &JobId,
        write: AttemptWriteAuthority,
        watch: &PredicateWatch,
        observation: PredicateObservation,
        observed_at_ms: u64,
    ) -> Result<PredicateEvaluationReceipt, DetachedJobError> {
        let current = self.required(job_id).await?;
        ensure_current_writer(job_id, &current, &write)?;
        let checkpoint = current
            .machine_state
            .checkpoint_ref
            .as_deref()
            .map(decode_predicate_checkpoint)
            .transpose()?;
        let evaluation = watch
            .evaluate(checkpoint.as_ref(), observation)
            .map_err(|error| DetachedJobError::InvalidInput(error.to_string()))?;
        let notification = match evaluation.notification().cloned() {
            Some(notification) => Some(
                self.emit_notification(job_id, write.clone(), observed_at_ms, notification)
                    .await?,
            ),
            None => None,
        };
        let snapshot = match evaluation.checkpoint() {
            Some(checkpoint) => {
                let encoded = serde_json::to_string(checkpoint).map_err(|error| {
                    DetachedJobError::InvalidInput(format!(
                        "cannot encode predicate checkpoint: {error}"
                    ))
                })?;
                self.record_checkpoint(
                    job_id,
                    write.clone(),
                    CheckpointRef::new(format!("predicate:{encoded}"))?,
                    observed_at_ms,
                )
                .await?
            }
            None => self
                .get(job_id)
                .await?
                .ok_or_else(|| DetachedJobError::NotFound(job_id.clone()))?,
        };
        let health_cursor = snapshot.progress.as_ref().map_or(Ok(1), |progress| {
            progress.cursor.checked_add(1).ok_or_else(|| {
                DetachedJobError::InvalidInput("predicate health cursor exhausted u64".into())
            })
        })?;
        let (condition, detail) = match &evaluation {
            PredicateEvaluation::SourceUnavailable {
                reason,
                retry_after_secs,
                ..
            } => (
                JobHealthCondition::PredicateSourceUnavailable {
                    retry_after_secs: *retry_after_secs,
                },
                format!("predicate_source_unavailable:{reason}"),
            ),
            PredicateEvaluation::Baseline { .. }
            | PredicateEvaluation::Unchanged { .. }
            | PredicateEvaluation::Crossed { .. } => (
                JobHealthCondition::Healthy,
                "predicate_source_healthy".to_string(),
            ),
        };
        let snapshot = self
            .report_progress(
                job_id,
                write,
                JobProgress::health(health_cursor, condition, detail)?,
                observed_at_ms,
            )
            .await?;
        Ok(PredicateEvaluationReceipt {
            evaluation,
            notification,
            snapshot,
        })
    }

    pub async fn wait_external(
        &self,
        job_id: &JobId,
        write: AttemptWriteAuthority,
        observed_at_ms: u64,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let current = self.required(job_id).await?;
        ensure_current_writer(job_id, &current, &write)?;
        let input = dsl::DetachedJobInput::WaitExternal {
            attempt_id: write.attempt_id.as_str().to_string(),
            fence: write.fence.get(),
            observed_at_ms,
        };
        let (stored, _) = self.apply_from(current, input, |_, _| Ok(())).await?;
        job_snapshot(stored)
    }

    pub async fn resume_running(
        &self,
        job_id: &JobId,
        write: AttemptWriteAuthority,
        observed_at_ms: u64,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let current = self.required(job_id).await?;
        ensure_current_writer(job_id, &current, &write)?;
        let input = dsl::DetachedJobInput::ResumeRunning {
            attempt_id: write.attempt_id.as_str().to_string(),
            fence: write.fence.get(),
            observed_at_ms,
        };
        let (stored, _) = self.apply_from(current, input, |_, _| Ok(())).await?;
        job_snapshot(stored)
    }

    pub async fn observe_lease_expired(
        &self,
        job_id: &JobId,
        write: AttemptWriteAuthority,
        observed_at_ms: u64,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let current = self.required(job_id).await?;
        ensure_current_writer(job_id, &current, &write)?;
        let input = dsl::DetachedJobInput::LeaseExpired {
            attempt_id: write.attempt_id.as_str().to_string(),
            fence: write.fence.get(),
            observed_at_ms,
        };
        let (stored, _) = self.apply_from(current, input, |_, _| Ok(())).await?;
        job_snapshot(stored)
    }

    pub async fn schedule_retry(
        &self,
        job_id: &JobId,
        retry_due_at_ms: u64,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let (stored, _) = self
            .apply(
                job_id,
                dsl::DetachedJobInput::ScheduleRetry { retry_due_at_ms },
                |_, _| Ok(()),
            )
            .await?;
        job_snapshot(stored)
    }

    pub async fn complete_attempt(
        &self,
        job_id: &JobId,
        write: AttemptWriteAuthority,
        completed_at_ms: u64,
        result_ref: Option<JobResultRef>,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let current = self.required(job_id).await?;
        ensure_current_writer(job_id, &current, &write)?;
        self.apply_terminal(
            current,
            dsl::DetachedJobInput::CompleteAttempt {
                attempt_id: write.attempt_id.as_str().to_string(),
                fence: write.fence.get(),
                completed_at_ms,
            },
            JobTerminalResult::Succeeded { result_ref },
        )
        .await
    }

    pub async fn fail_attempt(
        &self,
        job_id: &JobId,
        write: AttemptWriteAuthority,
        failed_at_ms: u64,
        code: JobFailureCode,
        detail_ref: Option<JobResultRef>,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let current = self.required(job_id).await?;
        ensure_current_writer(job_id, &current, &write)?;
        self.apply_terminal(
            current,
            dsl::DetachedJobInput::FailAttempt {
                attempt_id: write.attempt_id.as_str().to_string(),
                fence: write.fence.get(),
                failed_at_ms,
            },
            JobTerminalResult::Failed { code, detail_ref },
        )
        .await
    }

    pub async fn request_cancel(&self, job_id: &JobId) -> Result<JobSnapshot, DetachedJobError> {
        let result = JobTerminalResult::Cancelled;
        let (stored, _) = self
            .apply(
                job_id,
                dsl::DetachedJobInput::RequestCancel {},
                move |job, effects| {
                    project_optional_terminal(job, effects, &result)?;
                    Ok(())
                },
            )
            .await?;
        job_snapshot(stored)
    }

    pub async fn acknowledge_cancel(
        &self,
        job_id: &JobId,
        write: AttemptWriteAuthority,
        acknowledged_at_ms: u64,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let current = self.required(job_id).await?;
        ensure_current_writer(job_id, &current, &write)?;
        self.apply_terminal(
            current,
            dsl::DetachedJobInput::AcknowledgeCancel {
                attempt_id: write.attempt_id.as_str().to_string(),
                fence: write.fence.get(),
                acknowledged_at_ms,
            },
            JobTerminalResult::Cancelled,
        )
        .await
    }

    pub async fn classify_worker_loss(
        &self,
        job_id: &JobId,
        observed_at_ms: u64,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let current = self.required(job_id).await?;
        self.apply_terminal(
            current,
            dsl::DetachedJobInput::ClassifyWorkerLoss { observed_at_ms },
            JobTerminalResult::WorkerLost,
        )
        .await
    }

    pub async fn mark_needs_attention(
        &self,
        job_id: &JobId,
        observed_at_ms: u64,
        reason: JobFailureCode,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let current = self.required(job_id).await?;
        self.apply_terminal(
            current,
            dsl::DetachedJobInput::MarkNeedsAttention { observed_at_ms },
            JobTerminalResult::NeedsAttention { reason },
        )
        .await
    }

    pub async fn mark_delivery_applied(
        &self,
        job_id: &JobId,
        delivery_sequence: u64,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let current = self.required(job_id).await?;
        let entry = current
            .outbox
            .iter()
            .find(|entry| entry.delivery_sequence == delivery_sequence)
            .ok_or_else(|| {
                DetachedJobError::Store(format!(
                    "delivery {delivery_sequence} has no committed outbox entry"
                ))
            })?;
        let delivery_id = entry.delivery_id.clone();
        let machine_delivery_id = delivery_id.clone();
        let (stored, _) = self
            .apply_from(
                current,
                dsl::DetachedJobInput::MarkDeliveryApplied {
                    delivery_id: machine_delivery_id,
                    delivery_sequence,
                },
                move |job, effects| {
                    let emitted = effects.iter().filter(|effect| {
                        matches!(
                            effect,
                            dsl::DetachedJobEffect::DeliveryApplied {
                                delivery_id: emitted_id,
                                delivery_sequence: emitted
                            } if emitted_id == &delivery_id && *emitted == delivery_sequence
                        )
                    });
                    if emitted.count() != 1 {
                        return Err(DetachedJobError::Store(
                            "machine did not authorize exactly one delivery acknowledgement".into(),
                        ));
                    }
                    let entry = job
                        .outbox
                        .iter_mut()
                        .find(|entry| entry.delivery_sequence == delivery_sequence)
                        .ok_or_else(|| {
                            DetachedJobError::Store(format!(
                                "delivery {delivery_sequence} has no committed outbox entry"
                            ))
                        })?;
                    if entry.applied {
                        return Ok(());
                    }
                    entry.applied = true;
                    Ok(())
                },
            )
            .await?;
        job_snapshot(stored)
    }

    async fn apply_terminal(
        &self,
        current: StoredJob,
        input: dsl::DetachedJobInput,
        result: JobTerminalResult,
    ) -> Result<JobSnapshot, DetachedJobError> {
        let job_id = current.job_id.clone();
        let (stored, _) = self
            .apply_from(current, input, move |job, effects| {
                let committed = project_optional_terminal(job, effects, &result)?;
                if !committed {
                    return Err(DetachedJobError::InvalidTransition {
                        job_id: job_id.clone(),
                        detail: "terminal transition emitted no terminal commitment".into(),
                    });
                }
                Ok(())
            })
            .await?;
        job_snapshot(stored)
    }

    async fn required(&self, job_id: &JobId) -> Result<StoredJob, DetachedJobError> {
        self.store
            .get(job_id)
            .await?
            .ok_or_else(|| DetachedJobError::NotFound(job_id.clone()))
    }

    async fn apply(
        &self,
        job_id: &JobId,
        input: dsl::DetachedJobInput,
        project: impl FnOnce(&mut StoredJob, &[dsl::DetachedJobEffect]) -> Result<(), DetachedJobError>,
    ) -> Result<(StoredJob, Vec<dsl::DetachedJobEffect>), DetachedJobError> {
        let current = self.required(job_id).await?;
        self.apply_from(current, input, project).await
    }

    async fn apply_from(
        &self,
        current: StoredJob,
        input: dsl::DetachedJobInput,
        project: impl FnOnce(&mut StoredJob, &[dsl::DetachedJobEffect]) -> Result<(), DetachedJobError>,
    ) -> Result<(StoredJob, Vec<dsl::DetachedJobEffect>), DetachedJobError> {
        let job_id = current.job_id.clone();
        let mut authority =
            dsl::DetachedJobMachineAuthority::recover_from_state(current.machine_state.clone())
                .map_err(|error| DetachedJobError::InvalidTransition {
                    job_id: job_id.clone(),
                    detail: format!("machine recovery rejected persisted state: {error:?}"),
                })?;
        let transition =
            dsl::DetachedJobMachineMutator::apply(&mut authority, input).map_err(|error| {
                DetachedJobError::InvalidTransition {
                    job_id: job_id.clone(),
                    detail: format!("{error:?}"),
                }
            })?;
        let effects = transition.effects().to_vec();
        let original = current.clone();
        let mut replacement = current;
        replacement.machine_state = authority.state().clone();
        project(&mut replacement, &effects)?;
        if replacement == original {
            return Ok((original, effects));
        }
        let expected_revision = replacement.revision;
        let committed = self
            .store
            .compare_and_swap(expected_revision, replacement)
            .await?;
        Ok((committed, effects))
    }
}

fn ensure_current_writer(
    job_id: &JobId,
    job: &StoredJob,
    write: &AttemptWriteAuthority,
) -> Result<(), DetachedJobError> {
    let current_attempt = job.machine_state.current_attempt_id.as_deref();
    if current_attempt != Some(write.attempt_id.as_str())
        || job.machine_state.current_fence != write.fence.get()
    {
        return Err(DetachedJobError::StaleAttempt {
            job_id: job_id.clone(),
            attempt_id: write.attempt_id.clone(),
            fence: write.fence,
        });
    }
    Ok(())
}

fn job_description(job: StoredJob) -> Result<JobDescription, DetachedJobError> {
    let snapshot = job_snapshot(job.clone())?;
    Ok(JobDescription {
        job_id: snapshot.job_id,
        runner: job.spec.runner,
        phase: snapshot.phase,
        restart_class: job.spec.restart_class,
        attempt_count: snapshot.attempt_count,
        progress: snapshot.progress,
        cancel_requested: snapshot.cancel_requested,
        terminal_result: snapshot.terminal_result,
        subscription_count: u64::try_from(snapshot.subscriptions.len()).unwrap_or(u64::MAX),
        delivery_backlog: u64::try_from(
            snapshot
                .outbox
                .iter()
                .filter(|entry| !entry.applied)
                .count(),
        )
        .unwrap_or(u64::MAX),
    })
}

fn decode_predicate_checkpoint(
    encoded: &str,
) -> Result<crate::PredicateCheckpoint, DetachedJobError> {
    let payload = encoded.strip_prefix("predicate:").ok_or_else(|| {
        DetachedJobError::InvalidInput(
            "predicate evaluator cannot consume a non-predicate checkpoint".into(),
        )
    })?;
    serde_json::from_str(payload).map_err(|error| {
        DetachedJobError::InvalidInput(format!("predicate checkpoint is invalid: {error}"))
    })
}

fn project_optional_terminal(
    job: &mut StoredJob,
    effects: &[dsl::DetachedJobEffect],
    result: &JobTerminalResult,
) -> Result<bool, DetachedJobError> {
    let mut committed = effects.iter().filter_map(|effect| match effect {
        dsl::DetachedJobEffect::TerminalCommitted {
            job_id,
            terminal_kind,
            delivery_sequence,
        } => Some((job_id.as_str(), *terminal_kind, *delivery_sequence)),
        _ => None,
    });
    let Some((emitted_job_id, terminal_kind, delivery_sequence)) = committed.next() else {
        return Ok(false);
    };
    if committed.next().is_some() {
        return Err(DetachedJobError::Store(
            "machine emitted multiple terminal commitments".into(),
        ));
    }
    if terminal_kind != result.kind() {
        return Err(DetachedJobError::Store(format!(
            "machine terminal kind {terminal_kind:?} disagrees with typed result {:?}",
            result.kind()
        )));
    }
    if emitted_job_id != job.job_id.as_str() {
        return Err(DetachedJobError::Store(format!(
            "machine terminal job id {emitted_job_id} disagrees with stored job {}",
            job.job_id
        )));
    }
    if job.terminal_result.is_some()
        || job
            .outbox
            .iter()
            .any(|entry| matches!(&entry.payload, JobOutboxPayload::Terminal(_)))
    {
        return Err(DetachedJobError::Store(
            "job already has terminal projection state".into(),
        ));
    }
    job.terminal_result = Some(result.clone());
    job.outbox.push(JobOutboxEntry {
        job_id: job.job_id.clone(),
        delivery_id: "terminal".to_string(),
        delivery_sequence,
        payload: JobOutboxPayload::Terminal(result.clone()),
        targets: job.subscriptions.clone(),
        applied: false,
    });
    Ok(true)
}

fn project_notification(
    job: &mut StoredJob,
    effects: &[dsl::DetachedJobEffect],
    notification: &JobNotification,
) -> Result<(), DetachedJobError> {
    let mut disposition = effects.iter().filter_map(|effect| match effect {
        dsl::DetachedJobEffect::NotificationCommitted {
            notification_id,
            idempotency_key,
            runtime_delivery_id,
            delivery_sequence,
        } => Some((
            notification_id.as_str(),
            idempotency_key.as_str(),
            runtime_delivery_id.as_str(),
            *delivery_sequence,
            false,
        )),
        dsl::DetachedJobEffect::NotificationSuppressed {
            notification_id,
            idempotency_key,
            runtime_delivery_id,
            delivery_sequence,
        } => Some((
            notification_id.as_str(),
            idempotency_key.as_str(),
            runtime_delivery_id.as_str(),
            *delivery_sequence,
            true,
        )),
        _ => None,
    });
    let Some((
        notification_id,
        idempotency_key,
        runtime_delivery_id,
        delivery_sequence,
        suppressed,
    )) = disposition.next()
    else {
        return Err(DetachedJobError::Store(
            "machine emitted no notification disposition".into(),
        ));
    };
    if disposition.next().is_some() {
        return Err(DetachedJobError::Store(
            "machine emitted multiple notification dispositions".into(),
        ));
    }
    if idempotency_key != notification.idempotency_key() {
        return Err(DetachedJobError::Store(
            "machine notification idempotency key disagrees with input".into(),
        ));
    }
    if suppressed {
        let entry = job
            .outbox
            .iter()
            .find(|entry| {
                matches!(
                    &entry.payload,
                    JobOutboxPayload::Notification(existing)
                        if existing.idempotency_key() == idempotency_key
                )
            })
            .ok_or_else(|| {
                DetachedJobError::Store(
                    "machine suppressed a notification without a committed outbox entry".into(),
                )
            })?;
        let JobOutboxPayload::Notification(existing) = &entry.payload else {
            return Err(DetachedJobError::Store(
                "notification idempotency entry has a terminal payload".into(),
            ));
        };
        if existing.title() != notification.title() || existing.body() != notification.body() {
            return Err(DetachedJobError::InvalidInput(format!(
                "notification idempotency key {idempotency_key} was reused with different content"
            )));
        }
        if entry.delivery_id != notification_id
            || entry.runtime_delivery_id() != runtime_delivery_id
            || entry.delivery_sequence != delivery_sequence
        {
            return Err(DetachedJobError::Store(
                "suppressed notification disagrees with committed identity or sequence".into(),
            ));
        }
        return Ok(());
    }
    if notification_id != notification.notification_id().as_str() {
        return Err(DetachedJobError::Store(
            "machine notification id disagrees with input".into(),
        ));
    }
    let expected_runtime_delivery_id = format!("{}:notification:{notification_id}", job.job_id);
    if runtime_delivery_id != expected_runtime_delivery_id {
        return Err(DetachedJobError::Store(
            "machine notification runtime delivery id disagrees with job identity".into(),
        ));
    }
    if job
        .outbox
        .iter()
        .any(|entry| entry.delivery_sequence == delivery_sequence)
    {
        return Err(DetachedJobError::Store(format!(
            "notification delivery sequence {delivery_sequence} is already committed"
        )));
    }
    job.outbox.push(JobOutboxEntry {
        job_id: job.job_id.clone(),
        delivery_id: notification_id.to_string(),
        delivery_sequence,
        payload: JobOutboxPayload::Notification(notification.clone()),
        targets: job.subscriptions.clone(),
        applied: false,
    });
    Ok(())
}

fn job_snapshot(job: StoredJob) -> Result<JobSnapshot, DetachedJobError> {
    let state = &job.machine_state;
    Ok(JobSnapshot {
        job_id: job.job_id,
        revision: job.revision,
        phase: state.lifecycle_phase,
        attempt_count: state.attempt_count,
        current_attempt_id: state
            .current_attempt_id
            .clone()
            .map(AttemptId::new)
            .transpose()?,
        current_fence: FenceToken::new(state.current_fence),
        lease_expires_at_ms: state.lease_expires_at_ms,
        checkpoint_ref: state
            .checkpoint_ref
            .clone()
            .map(CheckpointRef::new)
            .transpose()?,
        runner_handle: state
            .runner_handle
            .clone()
            .map(crate::RunnerHandleRef::new)
            .transpose()?,
        progress: job.progress,
        cancel_requested: state.cancel_requested,
        terminal_kind: state.terminal_kind,
        terminal_result: job.terminal_result,
        subscriptions: job.subscriptions,
        outbox: job.outbox,
    })
}
