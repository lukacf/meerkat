# RuntimeControlMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `2`
- Rust owner: `meerkat-runtime` / `generated::runtime_control`

## State
- Phase enum: `Initializing | Idle | Running | Recovering | Retired | Stopped | Destroyed`
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
- `RecoverRequested`
- `RecoverySucceeded`
- `RetireRequested`
- `ResetRequested`
- `StopRequested`
- `DestroyRequested`
- `ResumeRequested`
- `ExternalToolDeltaReceived`
- `RespawnRequested`
- `RespawnSucceeded`

## Effects
- `ResolveAdmission`(work_id: WorkId)
- `SubmitAdmittedIngressEffect`(work_id: WorkId, content_shape: ContentShape, handling_mode: HandlingMode, request_id: Option<RequestId>, reservation_key: Option<ReservationKey>, admission_effect: AdmissionEffect)
- `SubmitRunPrimitive`(run_id: RunId)
- `SignalWake`
- `SignalImmediateProcess`
- `EmitRuntimeNotice`(kind: String, detail: String)
- `ResolveCompletionAsTerminated`(reason: String)
- `ApplyControlPlaneCommand`(command: String)
- `InitiateRespawn`

## Invariants
- `running_implies_active_run`
- `active_run_only_while_running_or_retired`

## Transitions
### `Initialize`
- From: `Initializing`
- On: `Initialize`()
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

### `RunCompleted`
- From: `Running`
- On: `RunCompleted`(run_id)
- Guards:
  - `active_run_matches`
- To: `Idle`

### `RunFailed`
- From: `Running`
- On: `RunFailed`(run_id)
- Guards:
  - `active_run_matches`
- To: `Idle`

### `RunCancelled`
- From: `Running`
- On: `RunCancelled`(run_id)
- Guards:
  - `active_run_matches`
- To: `Idle`

### `RecoverRequestedFromIdle`
- From: `Idle`
- On: `RecoverRequested`()
- To: `Recovering`

### `RecoverRequestedFromRunning`
- From: `Running`
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

### `ResetRequested`
- From: `Initializing`, `Idle`, `Recovering`, `Retired`
- On: `ResetRequested`()
- Emits: `ApplyControlPlaneCommand`, `ResolveCompletionAsTerminated`
- To: `Idle`

### `StopRequested`
- From: `Initializing`, `Idle`, `Running`, `Recovering`, `Retired`
- On: `StopRequested`()
- Emits: `ApplyControlPlaneCommand`, `ResolveCompletionAsTerminated`
- To: `Stopped`

### `DestroyRequested`
- From: `Initializing`, `Idle`, `Running`, `Recovering`, `Retired`, `Stopped`
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

### `RespawnRequestedFromRetired`
- From: `Retired`
- On: `RespawnRequested`()
- Guards:
  - `no_active_run`
- Emits: `InitiateRespawn`
- To: `Recovering`

### `RespawnRequestedFromIdle`
- From: `Idle`
- On: `RespawnRequested`()
- Guards:
  - `no_active_run`
- Emits: `InitiateRespawn`
- To: `Recovering`

### `RespawnSucceeded`
- From: `Recovering`
- On: `RespawnSucceeded`()
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
