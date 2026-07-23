use meerkat_machine_dsl::machine;

use super::OptionValueExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DetachedJobRestartClass {
    Adoptable,
    CheckpointResumable,
    Replayable,
    #[default]
    NonResumable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DetachedJobTerminalKind {
    #[default]
    Succeeded,
    Failed,
    Cancelled,
    WorkerLost,
    NeedsAttention,
}

machine! {
    machine DetachedJobMachine {
        version: 1,
        rust: "self" / "catalog::dsl::detached_job",

        state {
            lifecycle_phase: DetachedJobPhase,
            job_id: String,
            restart_class: Enum<DetachedJobRestartClass>,
            attempt_count: u64,
            current_attempt_id: Option<String>,
            current_fence: u64,
            current_worker_id: Option<String>,
            lease_expires_at_ms: Option<u64>,
            heartbeat_at_ms: Option<u64>,
            checkpoint_ref: Option<String>,
            runner_handle: Option<String>,
            progress_cursor: u64,
            lease_expired: bool,
            retry_due_at_ms: Option<u64>,
            cancel_requested: bool,
            terminal_kind: Option<Enum<DetachedJobTerminalKind>>,
            terminal_delivery_sequence: u64,
            terminal_delivery_applied: bool,
        }

        init(Unsubmitted) {
            job_id = "",
            restart_class = DetachedJobRestartClass::NonResumable,
            attempt_count = 0,
            current_attempt_id = None,
            current_fence = 0,
            current_worker_id = None,
            lease_expires_at_ms = None,
            heartbeat_at_ms = None,
            checkpoint_ref = None,
            runner_handle = None,
            progress_cursor = 0,
            lease_expired = false,
            retry_due_at_ms = None,
            cancel_requested = false,
            terminal_kind = None,
            terminal_delivery_sequence = 0,
            terminal_delivery_applied = false,
        }

        terminal [Succeeded, Failed, Cancelled, WorkerLost, NeedsAttention]

        phase DetachedJobPhase {
            Unsubmitted,
            Queued,
            Claimed,
            Running,
            WaitingExternal,
            LossObserved,
            RetryScheduled,
            Succeeded,
            Failed,
            Cancelled,
            WorkerLost,
            NeedsAttention,
        }

        input DetachedJobInput {
            Submit {
                job_id: String,
                restart_class: Enum<DetachedJobRestartClass>,
            },
            ClaimAttempt {
                attempt_id: String,
                worker_id: String,
                claimed_at_ms: u64,
                lease_expires_at_ms: u64,
                runner_handle: String,
            },
            RenewLease {
                attempt_id: String,
                fence: u64,
                heartbeat_at_ms: u64,
                lease_expires_at_ms: u64,
            },
            ReportProgress {
                attempt_id: String,
                fence: u64,
                cursor: u64,
                observed_at_ms: u64,
            },
            RecordCheckpoint {
                attempt_id: String,
                fence: u64,
                checkpoint_ref: String,
                observed_at_ms: u64,
            },
            WaitExternal {
                attempt_id: String,
                fence: u64,
                observed_at_ms: u64,
            },
            ResumeRunning {
                attempt_id: String,
                fence: u64,
                observed_at_ms: u64,
            },
            CompleteAttempt {
                attempt_id: String,
                fence: u64,
                completed_at_ms: u64,
            },
            FailAttempt {
                attempt_id: String,
                fence: u64,
                failed_at_ms: u64,
            },
            RequestCancel {},
            AcknowledgeCancel {
                attempt_id: String,
                fence: u64,
                acknowledged_at_ms: u64,
            },
            LeaseExpired {
                attempt_id: String,
                fence: u64,
                observed_at_ms: u64,
            },
            ScheduleRetry {
                retry_due_at_ms: u64,
            },
            ClassifyWorkerLoss {
                observed_at_ms: u64,
            },
            MarkNeedsAttention {
                observed_at_ms: u64,
            },
            MarkDeliveryApplied {
                delivery_sequence: u64,
            },
        }

        effect DetachedJobEffect {
            JobSubmitted { job_id: String },
            AttemptClaimed {
                attempt_id: String,
                attempt_count: u64,
                fence: u64,
                lease_expires_at_ms: u64,
                resume_checkpoint: Option<String>,
            },
            LeaseRenewed {
                lease_expires_at_ms: u64,
            },
            ProgressAccepted {
                cursor: u64,
            },
            CheckpointAccepted {
                checkpoint_ref: String,
            },
            ExternalWaitAccepted,
            RunningResumed,
            CancelRequested,
            LeaseExpiryRecorded,
            RetryScheduled {
                retry_due_at_ms: u64,
            },
            TerminalCommitted {
                terminal_kind: Enum<DetachedJobTerminalKind>,
                delivery_sequence: u64,
            },
            DeliveryApplied {
                delivery_sequence: u64,
            },
        }

        invariant fence_tracks_claim_count {
            self.current_fence == self.attempt_count
        }

        invariant no_attempt_has_no_attempt_authority {
            self.attempt_count != 0
                || (
                    self.current_attempt_id == None
                    && self.current_worker_id == None
                    && self.lease_expires_at_ms == None
                    && self.heartbeat_at_ms == None
                    && self.runner_handle == None
                )
        }

        invariant checkpoint_requires_attempt {
            self.checkpoint_ref == None || self.attempt_count > 0
        }

        invariant active_execution_requires_attempt_authority {
            (
                self.lifecycle_phase != Phase::Running
                && self.lifecycle_phase != Phase::WaitingExternal
                && self.lifecycle_phase != Phase::LossObserved
                && self.lifecycle_phase != Phase::RetryScheduled
            )
            || (
                self.current_attempt_id != None
                && self.current_worker_id != None
                && self.lease_expires_at_ms != None
                && self.runner_handle != None
                && self.current_fence > 0
            )
        }

        invariant terminal_requires_delivery {
            (
                self.lifecycle_phase != Phase::Succeeded
                && self.lifecycle_phase != Phase::Failed
                && self.lifecycle_phase != Phase::Cancelled
                && self.lifecycle_phase != Phase::WorkerLost
                && self.lifecycle_phase != Phase::NeedsAttention
            )
            || (
                self.terminal_kind != None
                && self.terminal_delivery_sequence > 0
            )
        }

        invariant nonterminal_has_no_terminal_delivery {
            (
                self.lifecycle_phase == Phase::Succeeded
                || self.lifecycle_phase == Phase::Failed
                || self.lifecycle_phase == Phase::Cancelled
                || self.lifecycle_phase == Phase::WorkerLost
                || self.lifecycle_phase == Phase::NeedsAttention
            )
            || (
                self.terminal_kind == None
                && self.terminal_delivery_sequence == 0
                && self.terminal_delivery_applied == false
            )
        }

        disposition JobSubmitted => local seam SurfaceResultAlignment,
        disposition AttemptClaimed => local seam SurfaceResultAlignment,
        disposition LeaseRenewed => local seam SurfaceResultAlignment,
        disposition ProgressAccepted => local seam SurfaceResultAlignment,
        disposition CheckpointAccepted => local seam SurfaceResultAlignment,
        disposition ExternalWaitAccepted => local seam SurfaceResultAlignment,
        disposition RunningResumed => local seam SurfaceResultAlignment,
        disposition CancelRequested => local seam SurfaceResultAlignment,
        disposition LeaseExpiryRecorded => local seam SurfaceResultAlignment,
        disposition RetryScheduled => local seam SurfaceResultAlignment,
        disposition TerminalCommitted => local seam SurfaceResultAlignment,
        disposition DeliveryApplied => local seam SurfaceResultAlignment,

        transition SubmitQueued {
            on input Submit { job_id, restart_class }
            guard {
                self.lifecycle_phase == Phase::Unsubmitted
                && job_id != ""
            }
            update {
                self.job_id = job_id;
                self.restart_class = restart_class;
            }
            to Queued
            emit JobSubmitted { job_id: self.job_id }
        }

        transition ClaimQueued {
            on input ClaimAttempt {
                attempt_id,
                worker_id,
                claimed_at_ms,
                lease_expires_at_ms,
                runner_handle
            }
            guard {
                self.lifecycle_phase == Phase::Queued
                && attempt_id != ""
                && worker_id != ""
                && runner_handle != ""
                && lease_expires_at_ms > claimed_at_ms
            }
            update {
                self.attempt_count += 1;
                self.current_fence += 1;
                self.current_attempt_id = Some(attempt_id);
                self.current_worker_id = Some(worker_id);
                self.lease_expires_at_ms = Some(lease_expires_at_ms);
                self.heartbeat_at_ms = Some(claimed_at_ms);
                self.runner_handle = Some(runner_handle);
                self.lease_expired = false;
                self.retry_due_at_ms = None;
                self.cancel_requested = false;
            }
            to Running
            emit AttemptClaimed {
                attempt_id: attempt_id,
                attempt_count: self.attempt_count,
                fence: self.current_fence,
                lease_expires_at_ms: lease_expires_at_ms,
                resume_checkpoint: self.checkpoint_ref
            }
        }

        transition ClaimRetryScheduled {
            on input ClaimAttempt {
                attempt_id,
                worker_id,
                claimed_at_ms,
                lease_expires_at_ms,
                runner_handle
            }
            guard {
                self.lifecycle_phase == Phase::RetryScheduled
                && attempt_id != ""
                && worker_id != ""
                && runner_handle != ""
                && self.retry_due_at_ms != None
                && claimed_at_ms >= self.retry_due_at_ms.get("value")
                && lease_expires_at_ms > claimed_at_ms
                && attempt_id != self.current_attempt_id.get("value")
            }
            update {
                self.attempt_count += 1;
                self.current_fence += 1;
                self.current_attempt_id = Some(attempt_id);
                self.current_worker_id = Some(worker_id);
                self.lease_expires_at_ms = Some(lease_expires_at_ms);
                self.heartbeat_at_ms = Some(claimed_at_ms);
                self.runner_handle = Some(runner_handle);
                self.lease_expired = false;
                self.retry_due_at_ms = None;
                self.cancel_requested = false;
            }
            to Running
            emit AttemptClaimed {
                attempt_id: attempt_id,
                attempt_count: self.attempt_count,
                fence: self.current_fence,
                lease_expires_at_ms: lease_expires_at_ms,
                resume_checkpoint: self.checkpoint_ref
            }
        }

        transition RenewRunningLease {
            on input RenewLease {
                attempt_id,
                fence,
                heartbeat_at_ms,
                lease_expires_at_ms
            }
            guard {
                self.lifecycle_phase == Phase::Running
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && self.heartbeat_at_ms != None
                && heartbeat_at_ms > self.heartbeat_at_ms.get("value")
                && heartbeat_at_ms <= self.lease_expires_at_ms.get("value")
                && lease_expires_at_ms > self.lease_expires_at_ms.get("value")
            }
            update {
                self.heartbeat_at_ms = Some(heartbeat_at_ms);
                self.lease_expires_at_ms = Some(lease_expires_at_ms);
            }
            to Running
            emit LeaseRenewed { lease_expires_at_ms: lease_expires_at_ms }
        }

        transition RenewExternalWaitLease {
            on input RenewLease {
                attempt_id,
                fence,
                heartbeat_at_ms,
                lease_expires_at_ms
            }
            guard {
                self.lifecycle_phase == Phase::WaitingExternal
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && self.heartbeat_at_ms != None
                && heartbeat_at_ms > self.heartbeat_at_ms.get("value")
                && heartbeat_at_ms <= self.lease_expires_at_ms.get("value")
                && lease_expires_at_ms > self.lease_expires_at_ms.get("value")
            }
            update {
                self.heartbeat_at_ms = Some(heartbeat_at_ms);
                self.lease_expires_at_ms = Some(lease_expires_at_ms);
            }
            to WaitingExternal
            emit LeaseRenewed { lease_expires_at_ms: lease_expires_at_ms }
        }

        transition ReportRunningProgress {
            on input ReportProgress {
                attempt_id,
                fence,
                cursor,
                observed_at_ms
            }
            guard {
                self.lifecycle_phase == Phase::Running
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && observed_at_ms <= self.lease_expires_at_ms.get("value")
                && cursor > self.progress_cursor
            }
            update {
                self.progress_cursor = cursor;
            }
            to Running
            emit ProgressAccepted { cursor: cursor }
        }

        transition ReportExternalWaitProgress {
            on input ReportProgress {
                attempt_id,
                fence,
                cursor,
                observed_at_ms
            }
            guard {
                self.lifecycle_phase == Phase::WaitingExternal
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && observed_at_ms <= self.lease_expires_at_ms.get("value")
                && cursor > self.progress_cursor
            }
            update {
                self.progress_cursor = cursor;
            }
            to WaitingExternal
            emit ProgressAccepted { cursor: cursor }
        }

        transition RecordRunningCheckpoint {
            on input RecordCheckpoint {
                attempt_id,
                fence,
                checkpoint_ref,
                observed_at_ms
            }
            guard {
                self.lifecycle_phase == Phase::Running
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && observed_at_ms <= self.lease_expires_at_ms.get("value")
                && checkpoint_ref != ""
            }
            update {
                self.checkpoint_ref = Some(checkpoint_ref);
            }
            to Running
            emit CheckpointAccepted { checkpoint_ref: checkpoint_ref }
        }

        transition RecordExternalWaitCheckpoint {
            on input RecordCheckpoint {
                attempt_id,
                fence,
                checkpoint_ref,
                observed_at_ms
            }
            guard {
                self.lifecycle_phase == Phase::WaitingExternal
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && observed_at_ms <= self.lease_expires_at_ms.get("value")
                && checkpoint_ref != ""
            }
            update {
                self.checkpoint_ref = Some(checkpoint_ref);
            }
            to WaitingExternal
            emit CheckpointAccepted { checkpoint_ref: checkpoint_ref }
        }

        transition WaitExternalFromRunning {
            on input WaitExternal { attempt_id, fence, observed_at_ms }
            guard {
                self.lifecycle_phase == Phase::Running
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && observed_at_ms <= self.lease_expires_at_ms.get("value")
            }
            update {}
            to WaitingExternal
            emit ExternalWaitAccepted
        }

        transition ResumeRunningFromExternal {
            on input ResumeRunning { attempt_id, fence, observed_at_ms }
            guard {
                self.lifecycle_phase == Phase::WaitingExternal
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && observed_at_ms <= self.lease_expires_at_ms.get("value")
            }
            update {}
            to Running
            emit RunningResumed
        }

        transition RequestCancelRunning {
            on input RequestCancel {}
            guard {
                self.lifecycle_phase == Phase::Running
                && self.cancel_requested == false
            }
            update {
                self.cancel_requested = true;
            }
            to Running
            emit CancelRequested
        }

        transition RequestCancelWaitingExternal {
            on input RequestCancel {}
            guard {
                self.lifecycle_phase == Phase::WaitingExternal
                && self.cancel_requested == false
            }
            update {
                self.cancel_requested = true;
            }
            to WaitingExternal
            emit CancelRequested
        }

        transition RequestCancelAlreadyRequestedRunning {
            on input RequestCancel {}
            guard {
                self.lifecycle_phase == Phase::Running
                && self.cancel_requested
            }
            update {}
            to Running
            emit CancelRequested
        }

        transition RequestCancelAlreadyRequestedWaitingExternal {
            on input RequestCancel {}
            guard {
                self.lifecycle_phase == Phase::WaitingExternal
                && self.cancel_requested
            }
            update {}
            to WaitingExternal
            emit CancelRequested
        }

        transition RequestCancelAlreadyCancelled {
            on input RequestCancel {}
            guard {
                self.lifecycle_phase == Phase::Cancelled
            }
            update {}
            to Cancelled
            emit CancelRequested
        }

        transition RequestCancelQueued {
            on input RequestCancel {}
            guard {
                self.lifecycle_phase == Phase::Queued
            }
            update {
                self.cancel_requested = true;
                self.terminal_kind = Some(DetachedJobTerminalKind::Cancelled);
                self.terminal_delivery_sequence += 1;
            }
            to Cancelled
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::Cancelled,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition RequestCancelRetryScheduled {
            on input RequestCancel {}
            guard {
                self.lifecycle_phase == Phase::RetryScheduled
            }
            update {
                self.cancel_requested = true;
                self.terminal_kind = Some(DetachedJobTerminalKind::Cancelled);
                self.terminal_delivery_sequence += 1;
            }
            to Cancelled
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::Cancelled,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition RequestCancelLossObserved {
            on input RequestCancel {}
            guard {
                self.lifecycle_phase == Phase::LossObserved
            }
            update {
                self.cancel_requested = true;
                self.terminal_kind = Some(DetachedJobTerminalKind::Cancelled);
                self.terminal_delivery_sequence += 1;
            }
            to Cancelled
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::Cancelled,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition LeaseExpiresRunning {
            on input LeaseExpired { attempt_id, fence, observed_at_ms }
            guard {
                self.lifecycle_phase == Phase::Running
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && observed_at_ms > self.lease_expires_at_ms.get("value")
            }
            update {
                self.lease_expired = true;
            }
            to LossObserved
            emit LeaseExpiryRecorded
        }

        transition LeaseExpiresWaitingExternal {
            on input LeaseExpired { attempt_id, fence, observed_at_ms }
            guard {
                self.lifecycle_phase == Phase::WaitingExternal
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && observed_at_ms > self.lease_expires_at_ms.get("value")
            }
            update {
                self.lease_expired = true;
            }
            to LossObserved
            emit LeaseExpiryRecorded
        }

        transition ScheduleRetryAfterLoss {
            on input ScheduleRetry { retry_due_at_ms }
            guard {
                self.lifecycle_phase == Phase::LossObserved
                && self.lease_expired
                && self.restart_class != DetachedJobRestartClass::NonResumable
                && (
                    self.restart_class != DetachedJobRestartClass::CheckpointResumable
                    || self.checkpoint_ref != None
                )
                && self.lease_expires_at_ms != None
                && retry_due_at_ms > self.lease_expires_at_ms.get("value")
            }
            update {
                self.retry_due_at_ms = Some(retry_due_at_ms);
            }
            to RetryScheduled
            emit RetryScheduled { retry_due_at_ms: retry_due_at_ms }
        }

        transition ClassifyNonResumableWorkerLoss {
            on input ClassifyWorkerLoss { observed_at_ms }
            guard {
                self.lifecycle_phase == Phase::LossObserved
                && self.lease_expired
                && self.restart_class == DetachedJobRestartClass::NonResumable
                && self.lease_expires_at_ms != None
                && observed_at_ms > self.lease_expires_at_ms.get("value")
            }
            update {
                self.terminal_kind = Some(DetachedJobTerminalKind::WorkerLost);
                self.terminal_delivery_sequence += 1;
            }
            to WorkerLost
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::WorkerLost,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition CompleteRunningAttempt {
            on input CompleteAttempt { attempt_id, fence, completed_at_ms }
            guard {
                self.lifecycle_phase == Phase::Running
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && completed_at_ms <= self.lease_expires_at_ms.get("value")
            }
            update {
                self.terminal_kind = Some(DetachedJobTerminalKind::Succeeded);
                self.terminal_delivery_sequence += 1;
            }
            to Succeeded
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::Succeeded,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition CompleteWaitingExternalAttempt {
            on input CompleteAttempt { attempt_id, fence, completed_at_ms }
            guard {
                self.lifecycle_phase == Phase::WaitingExternal
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && completed_at_ms <= self.lease_expires_at_ms.get("value")
            }
            update {
                self.terminal_kind = Some(DetachedJobTerminalKind::Succeeded);
                self.terminal_delivery_sequence += 1;
            }
            to Succeeded
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::Succeeded,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition FailRunningAttempt {
            on input FailAttempt { attempt_id, fence, failed_at_ms }
            guard {
                self.lifecycle_phase == Phase::Running
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && failed_at_ms <= self.lease_expires_at_ms.get("value")
            }
            update {
                self.terminal_kind = Some(DetachedJobTerminalKind::Failed);
                self.terminal_delivery_sequence += 1;
            }
            to Failed
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::Failed,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition FailWaitingExternalAttempt {
            on input FailAttempt { attempt_id, fence, failed_at_ms }
            guard {
                self.lifecycle_phase == Phase::WaitingExternal
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && failed_at_ms <= self.lease_expires_at_ms.get("value")
            }
            update {
                self.terminal_kind = Some(DetachedJobTerminalKind::Failed);
                self.terminal_delivery_sequence += 1;
            }
            to Failed
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::Failed,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition AcknowledgeRunningCancel {
            on input AcknowledgeCancel { attempt_id, fence, acknowledged_at_ms }
            guard {
                self.lifecycle_phase == Phase::Running
                && self.cancel_requested
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && acknowledged_at_ms <= self.lease_expires_at_ms.get("value")
            }
            update {
                self.terminal_kind = Some(DetachedJobTerminalKind::Cancelled);
                self.terminal_delivery_sequence += 1;
            }
            to Cancelled
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::Cancelled,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition AcknowledgeWaitingExternalCancel {
            on input AcknowledgeCancel { attempt_id, fence, acknowledged_at_ms }
            guard {
                self.lifecycle_phase == Phase::WaitingExternal
                && self.cancel_requested
                && self.current_attempt_id == Some(attempt_id)
                && self.current_fence == fence
                && self.lease_expired == false
                && self.lease_expires_at_ms != None
                && acknowledged_at_ms <= self.lease_expires_at_ms.get("value")
            }
            update {
                self.terminal_kind = Some(DetachedJobTerminalKind::Cancelled);
                self.terminal_delivery_sequence += 1;
            }
            to Cancelled
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::Cancelled,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition MarkQueuedNeedsAttention {
            on input MarkNeedsAttention { observed_at_ms }
            guard {
                self.lifecycle_phase == Phase::Queued
                && observed_at_ms > 0
            }
            update {
                self.terminal_kind = Some(DetachedJobTerminalKind::NeedsAttention);
                self.terminal_delivery_sequence += 1;
            }
            to NeedsAttention
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::NeedsAttention,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition MarkRunningNeedsAttention {
            on input MarkNeedsAttention { observed_at_ms }
            guard {
                self.lifecycle_phase == Phase::Running
                && observed_at_ms > 0
            }
            update {
                self.terminal_kind = Some(DetachedJobTerminalKind::NeedsAttention);
                self.terminal_delivery_sequence += 1;
            }
            to NeedsAttention
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::NeedsAttention,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition MarkWaitingExternalNeedsAttention {
            on input MarkNeedsAttention { observed_at_ms }
            guard {
                self.lifecycle_phase == Phase::WaitingExternal
                && observed_at_ms > 0
            }
            update {
                self.terminal_kind = Some(DetachedJobTerminalKind::NeedsAttention);
                self.terminal_delivery_sequence += 1;
            }
            to NeedsAttention
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::NeedsAttention,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition MarkLossObservedNeedsAttention {
            on input MarkNeedsAttention { observed_at_ms }
            guard {
                self.lifecycle_phase == Phase::LossObserved
                && observed_at_ms > 0
            }
            update {
                self.terminal_kind = Some(DetachedJobTerminalKind::NeedsAttention);
                self.terminal_delivery_sequence += 1;
            }
            to NeedsAttention
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::NeedsAttention,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition MarkRetryScheduledNeedsAttention {
            on input MarkNeedsAttention { observed_at_ms }
            guard {
                self.lifecycle_phase == Phase::RetryScheduled
                && observed_at_ms > 0
            }
            update {
                self.terminal_kind = Some(DetachedJobTerminalKind::NeedsAttention);
                self.terminal_delivery_sequence += 1;
            }
            to NeedsAttention
            emit TerminalCommitted {
                terminal_kind: DetachedJobTerminalKind::NeedsAttention,
                delivery_sequence: self.terminal_delivery_sequence
            }
        }

        transition ApplySucceededDelivery {
            on input MarkDeliveryApplied { delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Succeeded
                && self.terminal_delivery_applied == false
                && delivery_sequence == self.terminal_delivery_sequence
            }
            update {
                self.terminal_delivery_applied = true;
            }
            to Succeeded
            emit DeliveryApplied { delivery_sequence: delivery_sequence }
        }

        transition ApplyFailedDelivery {
            on input MarkDeliveryApplied { delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Failed
                && self.terminal_delivery_applied == false
                && delivery_sequence == self.terminal_delivery_sequence
            }
            update {
                self.terminal_delivery_applied = true;
            }
            to Failed
            emit DeliveryApplied { delivery_sequence: delivery_sequence }
        }

        transition ApplyCancelledDelivery {
            on input MarkDeliveryApplied { delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Cancelled
                && self.terminal_delivery_applied == false
                && delivery_sequence == self.terminal_delivery_sequence
            }
            update {
                self.terminal_delivery_applied = true;
            }
            to Cancelled
            emit DeliveryApplied { delivery_sequence: delivery_sequence }
        }

        transition ApplyWorkerLostDelivery {
            on input MarkDeliveryApplied { delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::WorkerLost
                && self.terminal_delivery_applied == false
                && delivery_sequence == self.terminal_delivery_sequence
            }
            update {
                self.terminal_delivery_applied = true;
            }
            to WorkerLost
            emit DeliveryApplied { delivery_sequence: delivery_sequence }
        }

        transition ApplyNeedsAttentionDelivery {
            on input MarkDeliveryApplied { delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::NeedsAttention
                && self.terminal_delivery_applied == false
                && delivery_sequence == self.terminal_delivery_sequence
            }
            update {
                self.terminal_delivery_applied = true;
            }
            to NeedsAttention
            emit DeliveryApplied { delivery_sequence: delivery_sequence }
        }

        transition ObserveSucceededDeliveryAlreadyApplied {
            on input MarkDeliveryApplied { delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Succeeded
                && self.terminal_delivery_applied
                && delivery_sequence == self.terminal_delivery_sequence
            }
            update {}
            to Succeeded
            emit DeliveryApplied { delivery_sequence: delivery_sequence }
        }

        transition ObserveFailedDeliveryAlreadyApplied {
            on input MarkDeliveryApplied { delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Failed
                && self.terminal_delivery_applied
                && delivery_sequence == self.terminal_delivery_sequence
            }
            update {}
            to Failed
            emit DeliveryApplied { delivery_sequence: delivery_sequence }
        }

        transition ObserveCancelledDeliveryAlreadyApplied {
            on input MarkDeliveryApplied { delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Cancelled
                && self.terminal_delivery_applied
                && delivery_sequence == self.terminal_delivery_sequence
            }
            update {}
            to Cancelled
            emit DeliveryApplied { delivery_sequence: delivery_sequence }
        }

        transition ObserveWorkerLostDeliveryAlreadyApplied {
            on input MarkDeliveryApplied { delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::WorkerLost
                && self.terminal_delivery_applied
                && delivery_sequence == self.terminal_delivery_sequence
            }
            update {}
            to WorkerLost
            emit DeliveryApplied { delivery_sequence: delivery_sequence }
        }

        transition ObserveNeedsAttentionDeliveryAlreadyApplied {
            on input MarkDeliveryApplied { delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::NeedsAttention
                && self.terminal_delivery_applied
                && delivery_sequence == self.terminal_delivery_sequence
            }
            update {}
            to NeedsAttention
            emit DeliveryApplied { delivery_sequence: delivery_sequence }
        }
    }
}
