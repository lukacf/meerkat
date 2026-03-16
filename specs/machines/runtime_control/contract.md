# RuntimeControlMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-runtime` / `machines::runtime_control`

## State
- Phase enum: `Initializing | Idle | Running | Recovering | Retired | Stopped | Destroyed`
- `current_run_id`: `Option<RunId>`
- `pre_run_state`: `Option<RuntimeState>`
- `wake_pending`: `Bool`
- `process_pending`: `Bool`

## Inputs
- `Initialize`
- `SubmitCandidate`(candidate_id: CandidateId, candidate_kind: InputKind)
- `AdmissionAccepted`(candidate_id: CandidateId, candidate_kind: InputKind, admission_effect: AdmissionEffect, wake: Bool, process: Bool)
- `AdmissionRejected`(candidate_id: CandidateId, reason: String)
- `AdmissionDeduplicated`(candidate_id: CandidateId, existing_input_id: InputId)
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

## Effects
- `ResolveAdmission`(candidate_id: CandidateId)
- `SubmitAdmittedIngressEffect`(candidate_id: CandidateId, candidate_kind: InputKind, admission_effect: AdmissionEffect)
- `SubmitRunPrimitive`(run_id: RunId)
- `SignalWake`
- `SignalImmediateProcess`
- `EmitRuntimeNotice`(kind: String, detail: String)
- `ResolveCompletionAsTerminated`(reason: String)
- `ApplyControlPlaneCommand`(command: String)

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
- Emits: `ApplyControlPlaneCommand`
- To: `Idle`

### `StopRequested`
- From: `Initializing`, `Idle`, `Running`, `Recovering`, `Retired`
- On: `StopRequested`()
- Emits: `ResolveCompletionAsTerminated`
- To: `Stopped`

### `DestroyRequested`
- From: `Initializing`, `Idle`, `Running`, `Recovering`, `Retired`, `Stopped`
- On: `DestroyRequested`()
- Emits: `ResolveCompletionAsTerminated`
- To: `Destroyed`

### `ResumeRequested`
- From: `Recovering`
- On: `ResumeRequested`()
- To: `Idle`

### `SubmitCandidateFromIdle`
- From: `Idle`
- On: `SubmitCandidate`(candidate_id, candidate_kind)
- Emits: `ResolveAdmission`
- To: `Idle`

### `SubmitCandidateFromRunning`
- From: `Running`
- On: `SubmitCandidate`(candidate_id, candidate_kind)
- Emits: `ResolveAdmission`
- To: `Running`

### `AdmissionAcceptedIdleNone`
- From: `Idle`
- On: `AdmissionAccepted`(candidate_id, candidate_kind, admission_effect, wake, process)
- Guards:
  - `wake_is_false`
  - `process_is_false`
- Emits: `SubmitAdmittedIngressEffect`
- To: `Idle`

### `AdmissionAcceptedIdleWake`
- From: `Idle`
- On: `AdmissionAccepted`(candidate_id, candidate_kind, admission_effect, wake, process)
- Guards:
  - `wake_is_true`
  - `process_is_false`
- Emits: `SubmitAdmittedIngressEffect`, `SignalWake`
- To: `Idle`

### `AdmissionAcceptedIdleProcess`
- From: `Idle`
- On: `AdmissionAccepted`(candidate_id, candidate_kind, admission_effect, wake, process)
- Guards:
  - `wake_is_false`
  - `process_is_true`
- Emits: `SubmitAdmittedIngressEffect`, `SignalImmediateProcess`
- To: `Idle`

### `AdmissionAcceptedIdleWakeAndProcess`
- From: `Idle`
- On: `AdmissionAccepted`(candidate_id, candidate_kind, admission_effect, wake, process)
- Guards:
  - `wake_is_true`
  - `process_is_true`
- Emits: `SubmitAdmittedIngressEffect`, `SignalWake`, `SignalImmediateProcess`
- To: `Idle`

### `AdmissionAcceptedRunningNone`
- From: `Running`
- On: `AdmissionAccepted`(candidate_id, candidate_kind, admission_effect, wake, process)
- Guards:
  - `wake_is_false`
  - `process_is_false`
- Emits: `SubmitAdmittedIngressEffect`
- To: `Running`

### `AdmissionAcceptedRunningWake`
- From: `Running`
- On: `AdmissionAccepted`(candidate_id, candidate_kind, admission_effect, wake, process)
- Guards:
  - `wake_is_true`
  - `process_is_false`
- Emits: `SubmitAdmittedIngressEffect`, `SignalWake`
- To: `Running`

### `AdmissionAcceptedRunningProcess`
- From: `Running`
- On: `AdmissionAccepted`(candidate_id, candidate_kind, admission_effect, wake, process)
- Guards:
  - `wake_is_false`
  - `process_is_true`
- Emits: `SubmitAdmittedIngressEffect`, `SignalImmediateProcess`
- To: `Running`

### `AdmissionAcceptedRunningWakeAndProcess`
- From: `Running`
- On: `AdmissionAccepted`(candidate_id, candidate_kind, admission_effect, wake, process)
- Guards:
  - `wake_is_true`
  - `process_is_true`
- Emits: `SubmitAdmittedIngressEffect`, `SignalWake`, `SignalImmediateProcess`
- To: `Running`

### `AdmissionRejectedIdle`
- From: `Idle`
- On: `AdmissionRejected`(candidate_id, reason)
- Emits: `EmitRuntimeNotice`
- To: `Idle`

### `AdmissionRejectedRunning`
- From: `Running`
- On: `AdmissionRejected`(candidate_id, reason)
- Emits: `EmitRuntimeNotice`
- To: `Running`

### `AdmissionDeduplicatedIdle`
- From: `Idle`
- On: `AdmissionDeduplicated`(candidate_id, existing_input_id)
- Emits: `EmitRuntimeNotice`
- To: `Idle`

### `AdmissionDeduplicatedRunning`
- From: `Running`
- On: `AdmissionDeduplicated`(candidate_id, existing_input_id)
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

## Coverage
### Code Anchors
- `meerkat-runtime/src/runtime_state.rs` — runtime lifecycle state precursor
- `meerkat-runtime/src/state_machine.rs` — runtime control reducer precursor
- `meerkat-runtime/src/runtime_loop.rs` — control-plane select loop and run coordination precursor

### Scenarios
- `control-preempts-ingress` — control commands preempt ordinary ingress work
- `begin-run-complete` — runtime transitions idle to running to idle for a completed run
- `retire-stop-destroy` — runtime transitions through retire/stop/destroy commands without reopening ordinary work
