# WorkExecutionLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::work_execution_lifecycle`

## State
- Phase enum: `Absent | LaunchRequested | LaunchUncertain | LaunchQuarantined | Running | EvidenceProjectionRequested | FailureEvidenceProjectionRequested | CancellationEvidenceProjectionRequested | LaunchFailureEvidenceProjectionRequested | WorkClosureRequested | FlowFailed | FlowCanceled | EvidenceProjected | WorkClosed | LaunchFailed`
- `binding_id`: `String`
- `run_id`: `String`
- `revision`: `u64`
- `last_failure_detail`: `Option<String>`
- `evidence_kind`: `Option<WorkExecutionEvidenceKind>`

## Inputs
- `Bind`(binding_id: String, run_id: String)
- `Recover`
- `ConfirmFlowStarted`
- `ObserveFlowRunning`
- `ObserveFlowCompleted`
- `ObserveFlowFailed`(detail: Option<String>)
- `ObserveFlowCanceled`(detail: Option<String>)
- `ObserveRunLost`(detail: String)
- `MarkLaunchUncertain`(detail: String)
- `QuarantineLaunch`(detail: String)
- `ResolveLaunchFailed`(detail: String)
- `ConfirmEvidenceProjected`
- `ConfirmFlowFailureEvidenceProjected`
- `ConfirmFlowCancellationEvidenceProjected`
- `ConfirmLaunchFailureEvidenceProjected`
- `ConfirmWorkClosed`
- `RefuseWorkClosure`(detail: String)
- `ClassifyRetryEligibility`

## Signals

## Effects
- `FlowLaunchRequested`(binding_id: String, run_id: String)
- `FlowLaunchAccepted`(binding_id: String, run_id: String)
- `FlowLaunchUncertain`(binding_id: String, run_id: String, detail: String)
- `FlowLaunchQuarantined`(binding_id: String, run_id: String, detail: String)
- `EvidenceProjectionRequested`(binding_id: String, run_id: String, kind: WorkExecutionEvidenceKind)
- `FlowFailureEvidenceProjectionRequested`(binding_id: String, run_id: String, kind: WorkExecutionEvidenceKind)
- `FlowCancellationEvidenceProjectionRequested`(binding_id: String, run_id: String, kind: WorkExecutionEvidenceKind)
- `LaunchFailureEvidenceProjectionRequested`(binding_id: String, run_id: String, detail: String, kind: WorkExecutionEvidenceKind)
- `WorkClosureRequested`(binding_id: String)
- `FlowFailed`(binding_id: String, run_id: String, detail: Option<String>)
- `FlowCanceled`(binding_id: String, run_id: String, detail: Option<String>)
- `LaunchFailed`(binding_id: String, run_id: String, detail: String)
- `EvidenceProjected`(binding_id: String, run_id: String)
- `WorkClosed`(binding_id: String)
- `RetryEligibilityClassified`(eligible: Bool)

## Invariants
- `identified_after_bind`
- `evidence_projection_is_typed`

## Transitions
### `BindExecution`
- From: `Absent`
- On: `Bind`(binding_id, run_id)
- Guards:
  - ``
- Emits: `FlowLaunchRequested`
- To: `LaunchRequested`

### `RecoverLaunchRequest`
- From: `LaunchRequested`
- On: `Recover`()
- Emits: `FlowLaunchRequested`
- To: `LaunchRequested`

### `RecoverUncertainLaunch`
- From: `LaunchUncertain`
- On: `Recover`()
- Emits: `FlowLaunchUncertain`
- To: `LaunchUncertain`

### `RecoverQuarantinedLaunch`
- From: `LaunchQuarantined`
- On: `Recover`()
- Emits: `FlowLaunchQuarantined`
- To: `LaunchQuarantined`

### `RecoverRunning`
- From: `Running`
- On: `Recover`()
- Emits: `FlowLaunchAccepted`
- To: `Running`

### `RecoverEvidenceProjection`
- From: `EvidenceProjectionRequested`
- On: `Recover`()
- Emits: `EvidenceProjectionRequested`
- To: `EvidenceProjectionRequested`

### `RecoverWorkClosure`
- From: `WorkClosureRequested`
- On: `Recover`()
- Emits: `WorkClosureRequested`
- To: `WorkClosureRequested`

### `RecoverFlowFailureEvidenceProjection`
- From: `FailureEvidenceProjectionRequested`
- On: `Recover`()
- Emits: `FlowFailureEvidenceProjectionRequested`
- To: `FailureEvidenceProjectionRequested`

### `RecoverFlowCancellationEvidenceProjection`
- From: `CancellationEvidenceProjectionRequested`
- On: `Recover`()
- Emits: `FlowCancellationEvidenceProjectionRequested`
- To: `CancellationEvidenceProjectionRequested`

### `RecoverLaunchFailureEvidenceProjection`
- From: `LaunchFailureEvidenceProjectionRequested`
- On: `Recover`()
- Emits: `LaunchFailureEvidenceProjectionRequested`
- To: `LaunchFailureEvidenceProjectionRequested`

### `RecoverFlowFailure`
- From: `FlowFailed`
- On: `Recover`()
- Emits: `FlowFailed`
- To: `FlowFailed`

### `RecoverFlowCancellation`
- From: `FlowCanceled`
- On: `Recover`()
- Emits: `FlowCanceled`
- To: `FlowCanceled`

### `RecoverEvidenceProjected`
- From: `EvidenceProjected`
- On: `Recover`()
- Emits: `EvidenceProjected`
- To: `EvidenceProjected`

### `RecoverClosedWork`
- From: `WorkClosed`
- On: `Recover`()
- Emits: `WorkClosed`
- To: `WorkClosed`

### `RecoverLaunchFailure`
- From: `LaunchFailed`
- On: `Recover`()
- Emits: `LaunchFailed`
- To: `LaunchFailed`

### `AcceptFlowLaunch`
- From: `LaunchRequested`, `LaunchUncertain`, `LaunchQuarantined`
- On: `ConfirmFlowStarted`()
- Emits: `FlowLaunchAccepted`
- To: `Running`

### `ObserveRunningFlow`
- From: `Running`, `LaunchRequested`, `LaunchUncertain`, `LaunchQuarantined`
- On: `ObserveFlowRunning`()
- Emits: `FlowLaunchAccepted`
- To: `Running`

### `ObserveCompletedFlow`
- From: `Running`, `LaunchRequested`, `LaunchUncertain`, `LaunchQuarantined`
- On: `ObserveFlowCompleted`()
- Emits: `EvidenceProjectionRequested`
- To: `EvidenceProjectionRequested`

### `ObserveFailedFlow`
- From: `Running`, `LaunchRequested`, `LaunchUncertain`, `LaunchQuarantined`
- On: `ObserveFlowFailed`(detail)
- Emits: `FlowFailureEvidenceProjectionRequested`
- To: `FailureEvidenceProjectionRequested`

### `ObserveCanceledFlow`
- From: `Running`, `LaunchRequested`, `LaunchUncertain`, `LaunchQuarantined`
- On: `ObserveFlowCanceled`(detail)
- Emits: `FlowCancellationEvidenceProjectionRequested`
- To: `CancellationEvidenceProjectionRequested`

### `ObserveLostRun`
- From: `Running`
- On: `ObserveRunLost`(detail)
- Emits: `FlowFailureEvidenceProjectionRequested`
- To: `FailureEvidenceProjectionRequested`

### `ObserveLostCompletedRunBeforeEvidence`
- From: `EvidenceProjectionRequested`
- On: `ObserveRunLost`(detail)
- Emits: `FlowFailureEvidenceProjectionRequested`
- To: `FailureEvidenceProjectionRequested`

### `RecordUncertainLaunch`
- From: `LaunchRequested`
- On: `MarkLaunchUncertain`(detail)
- Emits: `FlowLaunchUncertain`
- To: `LaunchUncertain`

### `QuarantineLaunch`
- From: `LaunchRequested`, `LaunchUncertain`
- On: `QuarantineLaunch`(detail)
- Emits: `FlowLaunchQuarantined`
- To: `LaunchQuarantined`

### `FailLaunch`
- From: `LaunchRequested`, `LaunchUncertain`
- On: `ResolveLaunchFailed`(detail)
- Emits: `LaunchFailureEvidenceProjectionRequested`
- To: `LaunchFailureEvidenceProjectionRequested`

### `CommitLaunchFailureEvidenceProjection`
- From: `LaunchFailureEvidenceProjectionRequested`
- On: `ConfirmLaunchFailureEvidenceProjected`()
- Emits: `LaunchFailed`
- To: `LaunchFailed`

### `CommitEvidenceProjection`
- From: `EvidenceProjectionRequested`
- On: `ConfirmEvidenceProjected`()
- Emits: `WorkClosureRequested`
- To: `WorkClosureRequested`

### `CommitFlowFailureEvidenceProjection`
- From: `FailureEvidenceProjectionRequested`
- On: `ConfirmFlowFailureEvidenceProjected`()
- Emits: `FlowFailed`
- To: `FlowFailed`

### `CommitFlowCancellationEvidenceProjection`
- From: `CancellationEvidenceProjectionRequested`
- On: `ConfirmFlowCancellationEvidenceProjected`()
- Emits: `FlowCanceled`
- To: `FlowCanceled`

### `CommitWorkClosure`
- From: `WorkClosureRequested`
- On: `ConfirmWorkClosed`()
- Emits: `WorkClosed`
- To: `WorkClosed`

### `RecordWorkClosureRefusal`
- From: `WorkClosureRequested`
- On: `RefuseWorkClosure`(detail)
- Emits: `EvidenceProjected`
- To: `EvidenceProjected`

### `ClassifyRetryEligibilityTerminalFlowFailed`
- From: `FlowFailed`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `FlowFailed`

### `ClassifyRetryEligibilityTerminalFlowCanceled`
- From: `FlowCanceled`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `FlowCanceled`

### `ClassifyRetryEligibilityTerminalEvidenceProjected`
- From: `EvidenceProjected`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `EvidenceProjected`

### `ClassifyRetryEligibilityTerminalWorkClosed`
- From: `WorkClosed`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `WorkClosed`

### `ClassifyRetryEligibilityTerminalLaunchFailed`
- From: `LaunchFailed`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `LaunchFailed`

### `ClassifyRetryEligibilityLiveAbsent`
- From: `Absent`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `Absent`

### `ClassifyRetryEligibilityLiveLaunchRequested`
- From: `LaunchRequested`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `LaunchRequested`

### `ClassifyRetryEligibilityLiveLaunchUncertain`
- From: `LaunchUncertain`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `LaunchUncertain`

### `ClassifyRetryEligibilityLiveLaunchQuarantined`
- From: `LaunchQuarantined`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `LaunchQuarantined`

### `ClassifyRetryEligibilityLiveRunning`
- From: `Running`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `Running`

### `ClassifyRetryEligibilityLiveEvidenceProjectionRequested`
- From: `EvidenceProjectionRequested`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `EvidenceProjectionRequested`

### `ClassifyRetryEligibilityLiveFailureEvidenceProjectionRequested`
- From: `FailureEvidenceProjectionRequested`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `FailureEvidenceProjectionRequested`

### `ClassifyRetryEligibilityLiveCancellationEvidenceProjectionRequested`
- From: `CancellationEvidenceProjectionRequested`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `CancellationEvidenceProjectionRequested`

### `ClassifyRetryEligibilityLiveLaunchFailureEvidenceProjectionRequested`
- From: `LaunchFailureEvidenceProjectionRequested`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `LaunchFailureEvidenceProjectionRequested`

### `ClassifyRetryEligibilityLiveWorkClosureRequested`
- From: `WorkClosureRequested`
- On: `ClassifyRetryEligibility`()
- Emits: `RetryEligibilityClassified`
- To: `WorkClosureRequested`

## Coverage
### Code Anchors
- `work_execution_lifecycle` (machine `WorkExecutionLifecycleMachine`): `crates/meerkat-workgraph/src/execution_machine.rs` — WorkExecutionMachine owns the durable bind, launch uncertainty, Flow observation, evidence projection, and WorkGraph closure handoff lifecycle

### Scenarios
- `work_execution_recovery_and_completion` — A binding commits before launch, ambiguous launch remains fail-closed, Flow success requests idempotent evidence, and closure feedback records either WorkClosed or EvidenceProjected
