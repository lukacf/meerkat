# DetachedJobMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::detached_job`

## State
- Phase enum: `Unsubmitted | Queued | Claimed | Running | WaitingExternal | LossObserved | RetryScheduled | Succeeded | Failed | Cancelled | WorkerLost | NeedsAttention`
- `job_id`: `String`
- `restart_class`: `DetachedJobRestartClass`
- `attempt_count`: `u64`
- `current_attempt_id`: `Option<String>`
- `current_fence`: `u64`
- `current_worker_id`: `Option<String>`
- `lease_expires_at_ms`: `Option<u64>`
- `heartbeat_at_ms`: `Option<u64>`
- `checkpoint_ref`: `Option<String>`
- `runner_handle`: `Option<String>`
- `progress_cursor`: `u64`
- `lease_expired`: `Bool`
- `retry_due_at_ms`: `Option<u64>`
- `cancel_requested`: `Bool`
- `terminal_kind`: `Option<DetachedJobTerminalKind>`
- `terminal_delivery_sequence`: `u64`
- `terminal_delivery_applied`: `Bool`

## Inputs
- `Submit`(job_id: String, restart_class: DetachedJobRestartClass)
- `ClaimAttempt`(attempt_id: String, worker_id: String, claimed_at_ms: u64, lease_expires_at_ms: u64, runner_handle: String)
- `RenewLease`(attempt_id: String, fence: u64, heartbeat_at_ms: u64, lease_expires_at_ms: u64)
- `ReportProgress`(attempt_id: String, fence: u64, cursor: u64, observed_at_ms: u64)
- `RecordCheckpoint`(attempt_id: String, fence: u64, checkpoint_ref: String, observed_at_ms: u64)
- `WaitExternal`(attempt_id: String, fence: u64, observed_at_ms: u64)
- `ResumeRunning`(attempt_id: String, fence: u64, observed_at_ms: u64)
- `CompleteAttempt`(attempt_id: String, fence: u64, completed_at_ms: u64)
- `FailAttempt`(attempt_id: String, fence: u64, failed_at_ms: u64)
- `RequestCancel`
- `AcknowledgeCancel`(attempt_id: String, fence: u64, acknowledged_at_ms: u64)
- `LeaseExpired`(attempt_id: String, fence: u64, observed_at_ms: u64)
- `ScheduleRetry`(retry_due_at_ms: u64)
- `ClassifyWorkerLoss`(observed_at_ms: u64)
- `MarkNeedsAttention`(observed_at_ms: u64)
- `MarkDeliveryApplied`(delivery_sequence: u64)

## Signals

## Effects
- `JobSubmitted`(job_id: String)
- `AttemptClaimed`(attempt_id: String, attempt_count: u64, fence: u64, lease_expires_at_ms: u64, resume_checkpoint: Option<String>)
- `LeaseRenewed`(lease_expires_at_ms: u64)
- `ProgressAccepted`(cursor: u64)
- `CheckpointAccepted`(checkpoint_ref: String)
- `ExternalWaitAccepted`
- `RunningResumed`
- `CancelRequested`
- `LeaseExpiryRecorded`
- `RetryScheduled`(retry_due_at_ms: u64)
- `TerminalCommitted`(terminal_kind: DetachedJobTerminalKind, delivery_sequence: u64)
- `DeliveryApplied`(delivery_sequence: u64)

## Invariants
- `fence_tracks_claim_count`
- `no_attempt_has_no_attempt_authority`
- `checkpoint_requires_attempt`
- `active_execution_requires_attempt_authority`
- `terminal_requires_delivery`
- `nonterminal_has_no_terminal_delivery`

## Transitions
### `SubmitQueued`
- From: `Unsubmitted`
- On: `Submit`(job_id, restart_class)
- Guards:
  - ``
- Emits: `JobSubmitted`
- To: `Queued`

### `ClaimQueued`
- From: `Queued`
- On: `ClaimAttempt`(attempt_id, worker_id, claimed_at_ms, lease_expires_at_ms, runner_handle)
- Guards:
  - ``
- Emits: `AttemptClaimed`
- To: `Running`

### `ClaimRetryScheduled`
- From: `RetryScheduled`
- On: `ClaimAttempt`(attempt_id, worker_id, claimed_at_ms, lease_expires_at_ms, runner_handle)
- Guards:
  - ``
- Emits: `AttemptClaimed`
- To: `Running`

### `RenewRunningLease`
- From: `Running`
- On: `RenewLease`(attempt_id, fence, heartbeat_at_ms, lease_expires_at_ms)
- Guards:
  - ``
- Emits: `LeaseRenewed`
- To: `Running`

### `RenewExternalWaitLease`
- From: `WaitingExternal`
- On: `RenewLease`(attempt_id, fence, heartbeat_at_ms, lease_expires_at_ms)
- Guards:
  - ``
- Emits: `LeaseRenewed`
- To: `WaitingExternal`

### `ReportRunningProgress`
- From: `Running`
- On: `ReportProgress`(attempt_id, fence, cursor, observed_at_ms)
- Guards:
  - ``
- Emits: `ProgressAccepted`
- To: `Running`

### `ReportExternalWaitProgress`
- From: `WaitingExternal`
- On: `ReportProgress`(attempt_id, fence, cursor, observed_at_ms)
- Guards:
  - ``
- Emits: `ProgressAccepted`
- To: `WaitingExternal`

### `RecordRunningCheckpoint`
- From: `Running`
- On: `RecordCheckpoint`(attempt_id, fence, checkpoint_ref, observed_at_ms)
- Guards:
  - ``
- Emits: `CheckpointAccepted`
- To: `Running`

### `RecordExternalWaitCheckpoint`
- From: `WaitingExternal`
- On: `RecordCheckpoint`(attempt_id, fence, checkpoint_ref, observed_at_ms)
- Guards:
  - ``
- Emits: `CheckpointAccepted`
- To: `WaitingExternal`

### `WaitExternalFromRunning`
- From: `Running`
- On: `WaitExternal`(attempt_id, fence, observed_at_ms)
- Guards:
  - ``
- Emits: `ExternalWaitAccepted`
- To: `WaitingExternal`

### `ResumeRunningFromExternal`
- From: `WaitingExternal`
- On: `ResumeRunning`(attempt_id, fence, observed_at_ms)
- Guards:
  - ``
- Emits: `RunningResumed`
- To: `Running`

### `RequestCancelRunning`
- From: `Running`
- On: `RequestCancel`()
- Guards:
  - ``
- Emits: `CancelRequested`
- To: `Running`

### `RequestCancelWaitingExternal`
- From: `WaitingExternal`
- On: `RequestCancel`()
- Guards:
  - ``
- Emits: `CancelRequested`
- To: `WaitingExternal`

### `RequestCancelAlreadyRequestedRunning`
- From: `Running`
- On: `RequestCancel`()
- Guards:
  - ``
- Emits: `CancelRequested`
- To: `Running`

### `RequestCancelAlreadyRequestedWaitingExternal`
- From: `WaitingExternal`
- On: `RequestCancel`()
- Guards:
  - ``
- Emits: `CancelRequested`
- To: `WaitingExternal`

### `RequestCancelAlreadyCancelled`
- From: `Cancelled`
- On: `RequestCancel`()
- Emits: `CancelRequested`
- To: `Cancelled`

### `RequestCancelQueued`
- From: `Queued`
- On: `RequestCancel`()
- Emits: `TerminalCommitted`
- To: `Cancelled`

### `RequestCancelRetryScheduled`
- From: `RetryScheduled`
- On: `RequestCancel`()
- Emits: `TerminalCommitted`
- To: `Cancelled`

### `RequestCancelLossObserved`
- From: `LossObserved`
- On: `RequestCancel`()
- Emits: `TerminalCommitted`
- To: `Cancelled`

### `LeaseExpiresRunning`
- From: `Running`
- On: `LeaseExpired`(attempt_id, fence, observed_at_ms)
- Guards:
  - ``
- Emits: `LeaseExpiryRecorded`
- To: `LossObserved`

### `LeaseExpiresWaitingExternal`
- From: `WaitingExternal`
- On: `LeaseExpired`(attempt_id, fence, observed_at_ms)
- Guards:
  - ``
- Emits: `LeaseExpiryRecorded`
- To: `LossObserved`

### `ScheduleRetryAfterLoss`
- From: `LossObserved`
- On: `ScheduleRetry`(retry_due_at_ms)
- Guards:
  - ``
- Emits: `RetryScheduled`
- To: `RetryScheduled`

### `ClassifyNonResumableWorkerLoss`
- From: `LossObserved`
- On: `ClassifyWorkerLoss`(observed_at_ms)
- Guards:
  - ``
- Emits: `TerminalCommitted`
- To: `WorkerLost`

### `CompleteRunningAttempt`
- From: `Running`
- On: `CompleteAttempt`(attempt_id, fence, completed_at_ms)
- Guards:
  - ``
- Emits: `TerminalCommitted`
- To: `Succeeded`

### `CompleteWaitingExternalAttempt`
- From: `WaitingExternal`
- On: `CompleteAttempt`(attempt_id, fence, completed_at_ms)
- Guards:
  - ``
- Emits: `TerminalCommitted`
- To: `Succeeded`

### `FailRunningAttempt`
- From: `Running`
- On: `FailAttempt`(attempt_id, fence, failed_at_ms)
- Guards:
  - ``
- Emits: `TerminalCommitted`
- To: `Failed`

### `FailWaitingExternalAttempt`
- From: `WaitingExternal`
- On: `FailAttempt`(attempt_id, fence, failed_at_ms)
- Guards:
  - ``
- Emits: `TerminalCommitted`
- To: `Failed`

### `AcknowledgeRunningCancel`
- From: `Running`
- On: `AcknowledgeCancel`(attempt_id, fence, acknowledged_at_ms)
- Guards:
  - ``
- Emits: `TerminalCommitted`
- To: `Cancelled`

### `AcknowledgeWaitingExternalCancel`
- From: `WaitingExternal`
- On: `AcknowledgeCancel`(attempt_id, fence, acknowledged_at_ms)
- Guards:
  - ``
- Emits: `TerminalCommitted`
- To: `Cancelled`

### `MarkQueuedNeedsAttention`
- From: `Queued`
- On: `MarkNeedsAttention`(observed_at_ms)
- Guards:
  - ``
- Emits: `TerminalCommitted`
- To: `NeedsAttention`

### `MarkRunningNeedsAttention`
- From: `Running`
- On: `MarkNeedsAttention`(observed_at_ms)
- Guards:
  - ``
- Emits: `TerminalCommitted`
- To: `NeedsAttention`

### `MarkWaitingExternalNeedsAttention`
- From: `WaitingExternal`
- On: `MarkNeedsAttention`(observed_at_ms)
- Guards:
  - ``
- Emits: `TerminalCommitted`
- To: `NeedsAttention`

### `MarkLossObservedNeedsAttention`
- From: `LossObserved`
- On: `MarkNeedsAttention`(observed_at_ms)
- Guards:
  - ``
- Emits: `TerminalCommitted`
- To: `NeedsAttention`

### `MarkRetryScheduledNeedsAttention`
- From: `RetryScheduled`
- On: `MarkNeedsAttention`(observed_at_ms)
- Guards:
  - ``
- Emits: `TerminalCommitted`
- To: `NeedsAttention`

### `ApplySucceededDelivery`
- From: `Succeeded`
- On: `MarkDeliveryApplied`(delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `Succeeded`

### `ApplyFailedDelivery`
- From: `Failed`
- On: `MarkDeliveryApplied`(delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `Failed`

### `ApplyCancelledDelivery`
- From: `Cancelled`
- On: `MarkDeliveryApplied`(delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `Cancelled`

### `ApplyWorkerLostDelivery`
- From: `WorkerLost`
- On: `MarkDeliveryApplied`(delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `WorkerLost`

### `ApplyNeedsAttentionDelivery`
- From: `NeedsAttention`
- On: `MarkDeliveryApplied`(delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `NeedsAttention`

### `ObserveSucceededDeliveryAlreadyApplied`
- From: `Succeeded`
- On: `MarkDeliveryApplied`(delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `Succeeded`

### `ObserveFailedDeliveryAlreadyApplied`
- From: `Failed`
- On: `MarkDeliveryApplied`(delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `Failed`

### `ObserveCancelledDeliveryAlreadyApplied`
- From: `Cancelled`
- On: `MarkDeliveryApplied`(delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `Cancelled`

### `ObserveWorkerLostDeliveryAlreadyApplied`
- From: `WorkerLost`
- On: `MarkDeliveryApplied`(delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `WorkerLost`

### `ObserveNeedsAttentionDeliveryAlreadyApplied`
- From: `NeedsAttention`
- On: `MarkDeliveryApplied`(delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `NeedsAttention`

## Coverage
### Code Anchors
- `detached_job_authority` (machine `DetachedJobMachine`): `meerkat-jobs/src/service.rs` — generated detached-job lifecycle authority with mechanical CAS and typed projection

### Scenarios
- `detached_job_reopen_preserves_committed_authority` — recovery rehydrates the committed attempt, fence, lease, checkpoint, and runner handle without minting new authority
- `detached_job_terminal_outbox_is_atomic` — all terminal kinds commit typed result delivery and acknowledge it through generated authority
