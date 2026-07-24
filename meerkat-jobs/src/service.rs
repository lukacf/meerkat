use std::sync::Arc;

use crate::machines::detached_job as dsl;
use crate::store::{InsertJobOutcome, StoredJob};
use crate::{
    AttemptClaim, AttemptClaimReceipt, AttemptId, AttemptWriteAuthority, CheckpointRef,
    DetachedJobError, DetachedJobStore, FenceToken, JobFailureCode, JobId, JobOutboxEntry,
    JobProgress, JobReceipt, JobResultRef, JobSnapshot, JobTerminalResult,
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
            outbox: Vec::new(),
        };
        match self.store.insert_deduplicated(stored).await? {
            InsertJobOutcome::Inserted(inserted) => Ok(JobReceipt {
                job_id: inserted.job_id,
                deduplicated: false,
                restart_class: inserted.spec.restart_class,
            }),
            InsertJobOutcome::Existing(existing) => Ok(JobReceipt {
                job_id: existing.job_id,
                deduplicated: true,
                restart_class: existing.spec.restart_class,
            }),
        }
    }

    pub async fn get(&self, job_id: &JobId) -> Result<Option<JobSnapshot>, DetachedJobError> {
        self.store.get(job_id).await?.map(job_snapshot).transpose()
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
        let (stored, _) = self
            .apply(
                job_id,
                dsl::DetachedJobInput::MarkDeliveryApplied { delivery_sequence },
                move |job, effects| {
                    let emitted = effects.iter().filter(|effect| {
                        matches!(
                            effect,
                            dsl::DetachedJobEffect::DeliveryApplied {
                                delivery_sequence: emitted
                            } if *emitted == delivery_sequence
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
    if job.terminal_result.is_some() || !job.outbox.is_empty() {
        return Err(DetachedJobError::Store(
            "job already has terminal projection state".into(),
        ));
    }
    job.terminal_result = Some(result.clone());
    job.outbox.push(JobOutboxEntry {
        job_id: job.job_id.clone(),
        delivery_sequence,
        terminal_kind,
        terminal_result: result.clone(),
        applied: false,
    });
    Ok(true)
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
        outbox: job.outbox,
    })
}
