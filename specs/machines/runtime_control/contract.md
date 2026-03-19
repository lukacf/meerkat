# RuntimeControlMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `2`
- Rust owner: `meerkat-runtime` / `generated::runtime_control`

## State
- Phase enum: `Initializing | Idle | Attached | Running | Recovering | Retired | Stopped | Destroyed`
- `current_run_id`: `Option<RunId>`
- `pre_run_state`: `Option<RuntimeState>`
- `wake_pending`: `Bool`
- `process_pending`: `Bool`

## Inputs
- `Initialize`
- `SubmitWork`(work_id: WorkId, content_shape: ContentShape, handling_mode: HandlingMode, request_id: Option<RequestId>, reservation_key: Option<ReservationKey>)
- `AdmissionAccepted`(work_id: WorkId, content_shape: ContentShape, handling_mode: HandlingMode, request_id: Option<RequestId>, reservation_key: Option<ReservationKey>, admission_effect: AdmissionEffect)
- `AdmissionRejected`(work_id: WorkId, reason: String)
- `AdmissionDeduplicated`(work_id: WorkId, existing_work_id: WorkId)
- `BeginRun`(run_id: RunId)
- `RunCompleted`(run_id: RunId)
- `RunFailed`(run_id: RunId)
- `RunCancelled`(run_id: RunId)
- `AttachExecutor`
- `DetachExecutor`
- `RecoverRequested`
- `RecoverySucceeded`
- `RetireRequested`
- `ResetRequested`
- `StopRequested`
- `DestroyRequested`
- `ResumeRequested`
- `ExternalToolDeltaReceived`
- `RecycleRequested`
- `RecycleSucceeded`

## Effects
- `ResolveAdmission`(work_id: WorkId)
- `SubmitAdmittedIngressEffect`(work_id: WorkId, content_shape: ContentShape, handling_mode: HandlingMode, request_id: Option<RequestId>, reservation_key: Option<ReservationKey>, admission_effect: AdmissionEffect)
- `SubmitRunPrimitive`(run_id: RunId)
- `SignalWake`
- `SignalImmediateProcess`
- `EmitRuntimeNotice`(kind: String, detail: String)
- `ResolveCompletionAsTerminated`(reason: String)
- `ApplyControlPlaneCommand`(command: String)
- `InitiateRecycle`

## Invariants
- `running_implies_active_run`
- `active_run_only_while_running_or_retired`

## Transitions
### `Initialize`
- From: `Initializing`
- On: `Initialize`()
- To: `Idle`

### `AttachFromIdle`
- From: `Idle`
- On: `AttachExecutor`()
- To: `Attached`

### `DetachToIdle`
- From: `Attached`
- On: `DetachExecutor`()
- To: `Idle`

### `BeginRunFromIdle`
- From: `Idle`
- On: `BeginRun`(run_id)
- Guards:
  - `no_active_run`
- Emits: `SubmitRunPrimitive`
- To: `Running`

### `BeginRunFromRetired`
- From: `Retired`
- On: `BeginRun`(run_id)
- Guards:
  - `no_active_run`
- Emits: `SubmitRunPrimitive`
- To: `Running`

### `BeginRunFromAttached`
- From: `Attached`
- On: `BeginRun`(run_id)
- Guards:
  - `no_active_run`
- Emits: `SubmitRunPrimitive`
- To: `Running`

### `BeginRunFromRecovering`
- From: `Recovering`
- On: `BeginRun`(run_id)
- Guards:
  - `no_active_run`
- Emits: `SubmitRunPrimitive`
- To: `Running`

### `RunCompletedToIdle`
- From: `Running`
- On: `RunCompleted`(run_id)
- Guards:
  - `active_run_matches`
  - `pre_run_state_is_idle`
- To: `Idle`

### `RunCompletedToAttached`
- From: `Running`
- On: `RunCompleted`(run_id)
- Guards:
  - `active_run_matches`
  - `pre_run_state_is_attached`
- To: `Attached`

### `RunCompletedToRetired`
- From: `Running`
- On: `RunCompleted`(run_id)
- Guards:
  - `active_run_matches`
  - `pre_run_state_is_retired`
- To: `Retired`

### `RunFailedToIdle`
- From: `Running`
- On: `RunFailed`(run_id)
- Guards:
  - `active_run_matches`
  - `pre_run_state_is_idle`
- To: `Idle`

### `RunFailedToAttached`
- From: `Running`
- On: `RunFailed`(run_id)
- Guards:
  - `active_run_matches`
  - `pre_run_state_is_attached`
- To: `Attached`

### `RunFailedToRetired`
- From: `Running`
- On: `RunFailed`(run_id)
- Guards:
  - `active_run_matches`
  - `pre_run_state_is_retired`
- To: `Retired`

### `RunCancelledToIdle`
- From: `Running`
- On: `RunCancelled`(run_id)
- Guards:
  - `active_run_matches`
  - `pre_run_state_is_idle`
- To: `Idle`

### `RunCancelledToAttached`
- From: `Running`
- On: `RunCancelled`(run_id)
- Guards:
  - `active_run_matches`
  - `pre_run_state_is_attached`
- To: `Attached`

### `RunCancelledToRetired`
- From: `Running`
- On: `RunCancelled`(run_id)
- Guards:
  - `active_run_matches`
  - `pre_run_state_is_retired`
- To: `Retired`

### `RunCompletedFromRetiredInFlight`
- From: `Retired`
- On: `RunCompleted`(run_id)
- Guards:
  - `active_run_matches`
- To: `Retired`

### `RunFailedFromRetiredInFlight`
- From: `Retired`
- On: `RunFailed`(run_id)
- Guards:
  - `active_run_matches`
- To: `Retired`

### `RunCancelledFromRetiredInFlight`
- From: `Retired`
- On: `RunCancelled`(run_id)
- Guards:
  - `active_run_matches`
- To: `Retired`

### `RecoverRequestedFromIdle`
- From: `Idle`
- On: `RecoverRequested`()
- To: `Recovering`

### `RecoverRequestedFromRunning`
- From: `Running`
- On: `RecoverRequested`()
- To: `Recovering`

### `RecoverRequestedFromAttached`
- From: `Attached`
- On: `RecoverRequested`()
- To: `Recovering`

### `RecoverySucceeded`
- From: `Recovering`
- On: `RecoverySucceeded`()
- To: `Idle`

### `RetireRequestedFromIdle`
- From: `Idle`
- On: `RetireRequested`()
- Emits: `ApplyControlPlaneCommand`
- To: `Retired`

### `RetireRequestedFromRunning`
- From: `Running`
- On: `RetireRequested`()
- Emits: `ApplyControlPlaneCommand`
- To: `Retired`

### `RetireRequestedFromAttached`
- From: `Attached`
- On: `RetireRequested`()
- Emits: `ApplyControlPlaneCommand`
- To: `Retired`

### `ResetRequested`
- From: `Initializing`, `Idle`, `Attached`, `Recovering`, `Retired`
- On: `ResetRequested`()
- Emits: `ApplyControlPlaneCommand`, `ResolveCompletionAsTerminated`
- To: `Idle`

### `StopRequested`
- From: `Initializing`, `Idle`, `Attached`, `Running`, `Recovering`, `Retired`
- On: `StopRequested`()
- Emits: `ApplyControlPlaneCommand`, `ResolveCompletionAsTerminated`
- To: `Stopped`

### `DestroyRequested`
- From: `Initializing`, `Idle`, `Attached`, `Running`, `Recovering`, `Retired`, `Stopped`
- On: `DestroyRequested`()
- Emits: `ApplyControlPlaneCommand`, `ResolveCompletionAsTerminated`
- To: `Destroyed`

### `ResumeRequested`
- From: `Recovering`
- On: `ResumeRequested`()
- To: `Idle`

### `SubmitWorkFromIdle`
- From: `Idle`
- On: `SubmitWork`(work_id, content_shape, handling_mode, request_id, reservation_key)
- Emits: `ResolveAdmission`
- To: `Idle`

### `SubmitWorkFromRunning`
- From: `Running`
- On: `SubmitWork`(work_id, content_shape, handling_mode, request_id, reservation_key)
- Emits: `ResolveAdmission`
- To: `Running`

### `SubmitWorkFromAttached`
- From: `Attached`
- On: `SubmitWork`(work_id, content_shape, handling_mode, request_id, reservation_key)
- Emits: `ResolveAdmission`
- To: `Attached`

### `AdmissionAcceptedIdleQueue`
- From: `Idle`
- On: `AdmissionAccepted`(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
- Guards:
  - `handling_mode_is_queue`
- Emits: `SubmitAdmittedIngressEffect`, `SignalWake`
- To: `Idle`

### `AdmissionAcceptedIdleSteer`
- From: `Idle`
- On: `AdmissionAccepted`(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
- Guards:
  - `handling_mode_is_steer`
- Emits: `SubmitAdmittedIngressEffect`, `SignalWake`, `SignalImmediateProcess`
- To: `Idle`

### `AdmissionAcceptedRunningQueue`
- From: `Running`
- On: `AdmissionAccepted`(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
- Guards:
  - `handling_mode_is_queue`
- Emits: `SubmitAdmittedIngressEffect`
- To: `Running`

### `AdmissionAcceptedRunningSteer`
- From: `Running`
- On: `AdmissionAccepted`(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
- Guards:
  - `handling_mode_is_steer`
- Emits: `SubmitAdmittedIngressEffect`, `SignalWake`, `SignalImmediateProcess`
- To: `Running`

### `AdmissionAcceptedAttachedQueue`
- From: `Attached`
- On: `AdmissionAccepted`(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
- Guards:
  - `handling_mode_is_queue`
- Emits: `SubmitAdmittedIngressEffect`, `SignalWake`
- To: `Attached`

### `AdmissionAcceptedAttachedSteer`
- From: `Attached`
- On: `AdmissionAccepted`(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
- Guards:
  - `handling_mode_is_steer`
- Emits: `SubmitAdmittedIngressEffect`, `SignalWake`, `SignalImmediateProcess`
- To: `Attached`

### `AdmissionRejectedIdle`
- From: `Idle`
- On: `AdmissionRejected`(work_id, reason)
- Emits: `EmitRuntimeNotice`
- To: `Idle`

### `AdmissionRejectedRunning`
- From: `Running`
- On: `AdmissionRejected`(work_id, reason)
- Emits: `EmitRuntimeNotice`
- To: `Running`

### `AdmissionRejectedAttached`
- From: `Attached`
- On: `AdmissionRejected`(work_id, reason)
- Emits: `EmitRuntimeNotice`
- To: `Attached`

### `AdmissionDeduplicatedIdle`
- From: `Idle`
- On: `AdmissionDeduplicated`(work_id, existing_work_id)
- Emits: `EmitRuntimeNotice`
- To: `Idle`

### `AdmissionDeduplicatedRunning`
- From: `Running`
- On: `AdmissionDeduplicated`(work_id, existing_work_id)
- Emits: `EmitRuntimeNotice`
- To: `Running`

### `AdmissionDeduplicatedAttached`
- From: `Attached`
- On: `AdmissionDeduplicated`(work_id, existing_work_id)
- Emits: `EmitRuntimeNotice`
- To: `Attached`

### `ExternalToolDeltaReceivedIdle`
- From: `Idle`
- On: `ExternalToolDeltaReceived`()
- Emits: `EmitRuntimeNotice`
- To: `Idle`

### `ExternalToolDeltaReceivedRunning`
- From: `Running`
- On: `ExternalToolDeltaReceived`()
- Emits: `EmitRuntimeNotice`
- To: `Running`

### `ExternalToolDeltaReceivedRecovering`
- From: `Recovering`
- On: `ExternalToolDeltaReceived`()
- Emits: `EmitRuntimeNotice`
- To: `Recovering`

### `ExternalToolDeltaReceivedRetired`
- From: `Retired`
- On: `ExternalToolDeltaReceived`()
- Emits: `EmitRuntimeNotice`
- To: `Retired`

### `ExternalToolDeltaReceivedAttached`
- From: `Attached`
- On: `ExternalToolDeltaReceived`()
- Emits: `EmitRuntimeNotice`
- To: `Attached`

### `RecycleRequestedFromRetired`
- From: `Retired`
- On: `RecycleRequested`()
- Guards:
  - `no_active_run`
- Emits: `InitiateRecycle`
- To: `Recovering`

### `RecycleRequestedFromIdle`
- From: `Idle`
- On: `RecycleRequested`()
- Guards:
  - `no_active_run`
- Emits: `InitiateRecycle`
- To: `Recovering`

### `RecycleRequestedFromAttached`
- From: `Attached`
- On: `RecycleRequested`()
- Guards:
  - `no_active_run`
- Emits: `InitiateRecycle`
- To: `Recovering`

### `RecycleSucceeded`
- From: `Recovering`
- On: `RecycleSucceeded`()
- Emits: `EmitRuntimeNotice`
- To: `Idle`

## Coverage
### Code Anchors
- `meerkat-runtime/src/runtime_state.rs` — runtime lifecycle state precursor
- `meerkat-runtime/src/state_machine.rs` — runtime control reducer precursor
- `meerkat-runtime/src/runtime_loop.rs` — control-plane select loop and run coordination precursor
- `meerkat-runtime/src/control_plane.rs` — stop/preemption seam and completion-resolution precursor
- `meerkat-runtime/src/session_adapter.rs` — surface-facing lifecycle and completion owner precursor

### Scenarios
- `control-preempts-ingress` — control commands preempt ordinary ingress work
- `prompt-queue` — queued ordinary work waits for the next outer-loop turn without modifying the current run
- `prompt-steer` — steered ordinary work drains into the active run at the earliest admissible boundary
- `begin-run-complete` — runtime transitions idle to running to idle for a completed run
- `retire-stop-destroy` — runtime transitions through retire/stop/destroy commands without reopening ordinary work
- `reset-terminates-waiters` — reset abandons pending work and resolves completion waiters exactly once
