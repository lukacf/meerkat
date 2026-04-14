# MeerkatMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-runtime` / `generated::meerkat_machine`

## State
- Phase enum: `Initializing | Idle | Attached | Running | Recovering | Retired | Stopped | Destroyed`
- `session_id`: `Option<SessionId>`
- `active_runtime_id`: `Option<AgentRuntimeId>`
- `active_fence_token`: `Option<FenceToken>`
- `active_generation`: `Option<Generation>`
- `active_work_id`: `Option<WorkId>`
- `wake_pending`: `Bool`
- `process_pending`: `Bool`
- `peer_ingress_configured`: `Bool`
- `drain_running`: `Bool`
- `resolved_peer_keys`: `Set<ReachabilityKey>`
- `peer_reachability`: `Map<ReachabilityKey, PeerReachability>`
- `peer_last_reason`: `Map<ReachabilityKey, Option<PeerReachabilityReason>>`
- `interrupt_pending`: `Bool`
- `shutdown_pending`: `Bool`
- `inherited_base_filter`: `ToolFilter`
- `active_filter`: `ToolFilter`
- `staged_filter`: `ToolFilter`
- `active_requested_deferred_names`: `Set<String>`
- `staged_requested_deferred_names`: `Set<String>`
- `requested_witnesses`: `Map<String, ToolVisibilityWitness>`
- `filter_witnesses`: `Map<String, ToolVisibilityWitness>`
- `active_visibility_revision`: `u64`
- `staged_visibility_revision`: `u64`
- `committed_visibility_revision`: `u64`

## Inputs
- `RegisterSession`(session_id: SessionId)
- `UnregisterSession`(session_id: SessionId)
- `PrepareBindings`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `SetPeerIngressContext`(keep_alive: Bool)
- `NotifyDrainExited`(reason: String)
- `InterruptCurrentRun`
- `CancelAfterBoundary`
- `PublishCommittedVisibleSet`(revision: u64)
- `Recover`
- `Retire`
- `Reset`
- `StopRuntimeExecutor`
- `Destroy`
- `EnsureSessionWithExecutor`(session_id: SessionId)
- `SetSilentIntents`(session_id: SessionId, intents: Set<String>)
- `ContainsSession`(session_id: SessionId)
- `SessionHasExecutor`(session_id: SessionId)
- `SessionHasComms`(session_id: SessionId)
- `OpsLifecycleRegistry`(session_id: SessionId)
- `InputState`(session_id: SessionId, input_id: InputId)
- `ListActiveInputs`(session_id: SessionId)
- `Abort`(session_id: SessionId)
- `AbortAll`
- `Wait`(session_id: SessionId)
- `Ingest`(runtime_id: String)
- `PublishEvent`(kind: String)
- `RuntimeState`(runtime_id: String)
- `LoadBoundaryReceipt`(runtime_id: String, sequence: u64)
- `AcceptWithCompletion`(input_id: InputId)
- `AcceptWithoutWake`(input_id: InputId)
- `Prepare`(session_id: SessionId)
- `Commit`(input_id: InputId, run_id: RunId)
- `Fail`(run_id: RunId)
- `Recycle`

## Signals
- `Initialize`
- `SubmitMobWork`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId)
- `ReconcileResolvedDirectory`(keys: Set<ReachabilityKey>, reachability: Map<ReachabilityKey, PeerReachability>, last_reason: Map<ReachabilityKey, Option<PeerReachabilityReason>>)
- `RecordSendSucceeded`(key: ReachabilityKey)
- `RecordSendFailed`(key: ReachabilityKey, reason: PeerReachabilityReason)
- `StagePersistentFilter`(filter: ToolFilter, witnesses: Map<String, ToolVisibilityWitness>)
- `RequestDeferredTools`(names: Set<String>, witnesses: Map<String, ToolVisibilityWitness>)
- `BoundaryApplied`(revision: u64)
- `RunCompleted`(work_id: WorkId)
- `RunFailed`(work_id: WorkId)
- `RunCancelled`(work_id: WorkId)
- `AdmitQueued`
- `AdmitConsumedOnAccept`
- `StageDrainSnapshot`
- `SupersedeQueuedInput`
- `CoalesceQueuedInputs`
- `SetSilentIntentOverrides`
- `StartConversationRun`
- `StartImmediateAppend`
- `StartImmediateContext`
- `PrimitiveApplied`
- `LlmReturnedToolCalls`
- `LlmReturnedTerminal`
- `RegisterPendingOps`
- `ToolCallsResolved`
- `OpsBarrierSatisfied`
- `BoundaryContinue`
- `BoundaryComplete`
- `RecoverableFailure`
- `FatalFailure`
- `RetryRequested`
- `CancelNow`
- `CancellationObserved`
- `AcknowledgeTerminal`
- `TurnLimitReached`
- `BudgetExhausted`
- `TimeBudgetExceeded`
- `EnterExtraction`
- `ExtractionValidationPassed`
- `ExtractionValidationFailed`
- `ExtractionStart`
- `ForceCancelNoRun`
- `RegisterOperation`
- `ProvisioningSucceeded`
- `ProvisioningFailed`
- `AbortProvisioning`
- `PeerReady`
- `RegisterWatcher`
- `ProgressReported`
- `CompleteOperation`
- `FailOperation`
- `CancelOperation`
- `RetireRequested`
- `RetireCompleted`
- `CollectTerminal`
- `BeginWaitAll`
- `CancelWaitAll`
- `ClassifyExternalEnvelope`
- `ClassifyPlainEvent`
- `EnsureDrainRunning`
- `StageAdd`
- `StageRemove`
- `StageReload`
- `ApplySurfaceBoundary`
- `PendingSucceeded`
- `PendingFailed`
- `CallStarted`
- `CallFinished`
- `FinalizeRemovalClean`
- `FinalizeRemovalForced`
- `SnapshotAligned`
- `ShutdownSurface`

## Effects
- `RuntimeBound`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RuntimeRetired`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RuntimeDestroyed`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `WorkCompleted`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, work_id: WorkId)
- `WorkFailed`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, work_id: WorkId)
- `WorkCancelled`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, work_id: WorkId)
- `RequestCancellationAtBoundary`
- `WakeInterrupt`
- `CommittedVisibleSetPublished`(revision: u64)
- `RuntimeNotice`(kind: String, detail: String)
- `ResolveAdmission`
- `SubmitAdmittedIngressEffect`
- `SubmitRunPrimitive`
- `ResolveCompletionAsTerminated`
- `ApplyControlPlaneCommand`
- `InitiateRecycle`
- `IngressAccepted`
- `ReadyForRun`
- `InputLifecycleNotice`
- `CompletionResolved`
- `IngressNotice`
- `SilentIntentApplied`
- `CheckCompaction`
- `RecordTerminalOutcome`
- `RecordRunAssociation`
- `RecordBoundarySequence`
- `SubmitOpEvent`
- `NotifyOpWatcher`
- `ExposeOperationPeer`
- `RetainTerminalRecord`
- `EvictCompletedRecord`
- `WaitAllSatisfied`
- `CollectCompletedResult`
- `ConcurrencyLimitExceeded`
- `EnqueueClassifiedEntry`
- `SpawnDrainTask`
- `ScheduleSurfaceCompletion`
- `RefreshVisibleSurfaceSet`
- `EmitExternalToolDelta`
- `CloseSurfaceConnection`
- `RejectSurfaceCall`

## Helpers
- `HasPendingVisibilityPromotion`() -> `Bool`
- `RequestedWitnessKeys`() -> `Set<String>`
- `FilterWitnessKeys`() -> `Set<String>`

## Invariants
- `running_has_active_work`
- `bound_runtime_has_fence`
- `destroyed_has_no_active_work`
- `interrupt_pending_only_while_active`
- `drain_requires_ingress_context`
- `peer_reachability_keys_are_resolved`
- `peer_last_reason_keys_are_resolved`
- `active_visibility_revision_not_ahead_of_staged`
- `active_requested_names_subset_of_staged`
- `equal_visibility_revision_means_equal_active_and_staged_state`
- `committed_visibility_not_ahead_of_active`

## Transitions
### `Initialize`
- From: `Initializing`
- On: `Initialize`()
- To: `Idle`

### `RegisterSession`
- From: `Idle`, `Stopped`, `Retired`
- On: `RegisterSession`(session_id)
- To: `Idle`

### `UnregisterSession`
- From: `Idle`, `Stopped`, `Retired`
- On: `UnregisterSession`(session_id)
- Guards:
  - `session_matches_current`
- To: `Idle`

### `StagePersistentFilterAttached`
- From: `Attached`
- On: `StagePersistentFilter`(filter, witnesses)
- Guards:
  - `session_registered`
- To: `Attached`

### `StagePersistentFilterRunning`
- From: `Running`
- On: `StagePersistentFilter`(filter, witnesses)
- Guards:
  - `session_registered`
- To: `Running`

### `RequestDeferredToolsAttached`
- From: `Attached`
- On: `RequestDeferredTools`(names, witnesses)
- Guards:
  - `session_registered`
- To: `Attached`

### `RequestDeferredToolsRunning`
- From: `Running`
- On: `RequestDeferredTools`(names, witnesses)
- Guards:
  - `session_registered`
- To: `Running`

### `PrepareBindings`
- From: `Idle`, `Stopped`, `Retired`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation)
- Guards:
  - `session_registered`
- Emits: `RuntimeBound`
- To: `Attached`

### `SetPeerIngressContextAttached`
- From: `Attached`
- On: `SetPeerIngressContext`(keep_alive)
- Guards:
  - `session_registered`
- To: `Attached`

### `SetPeerIngressContextRunning`
- From: `Running`
- On: `SetPeerIngressContext`(keep_alive)
- Guards:
  - `session_registered`
- To: `Running`

### `NotifyDrainExitedAttached`
- From: `Attached`
- On: `NotifyDrainExited`(reason)
- Guards:
  - `session_registered`
- Emits: `RuntimeNotice`
- To: `Attached`

### `NotifyDrainExitedRunning`
- From: `Running`
- On: `NotifyDrainExited`(reason)
- Guards:
  - `session_registered`
- Emits: `RuntimeNotice`
- To: `Running`

### `ReconcileResolvedDirectoryAttached`
- From: `Attached`
- On: `ReconcileResolvedDirectory`(keys, reachability, last_reason)
- Guards:
  - `reachability_keys_subset_of_resolved`
  - `last_reason_keys_subset_of_resolved`
- To: `Attached`

### `ReconcileResolvedDirectoryRunning`
- From: `Running`
- On: `ReconcileResolvedDirectory`(keys, reachability, last_reason)
- Guards:
  - `reachability_keys_subset_of_resolved`
  - `last_reason_keys_subset_of_resolved`
- To: `Running`

### `RecordSendSucceededAttached`
- From: `Attached`
- On: `RecordSendSucceeded`(key)
- Guards:
  - `peer_key_is_resolved`
- To: `Attached`

### `RecordSendSucceededRunning`
- From: `Running`
- On: `RecordSendSucceeded`(key)
- Guards:
  - `peer_key_is_resolved`
- To: `Running`

### `RecordSendFailedAttached`
- From: `Attached`
- On: `RecordSendFailed`(key, reason)
- Guards:
  - `peer_key_is_resolved`
- To: `Attached`

### `RecordSendFailedRunning`
- From: `Running`
- On: `RecordSendFailed`(key, reason)
- Guards:
  - `peer_key_is_resolved`
- To: `Running`

### `BeginRunFromIdle`
- From: `Attached`
- On: `SubmitMobWork`(agent_runtime_id, fence_token, work_id)
- Guards:
  - `runtime_is_bound`
- To: `Running`

### `InterruptCurrentRun`
- From: `Running`
- On: `InterruptCurrentRun`()
- Guards:
  - `has_active_work`
- Emits: `WakeInterrupt`, `RequestCancellationAtBoundary`
- To: `Running`

### `CancelAfterBoundary`
- From: `Running`
- On: `CancelAfterBoundary`()
- Guards:
  - `has_active_work`
- Emits: `RequestCancellationAtBoundary`
- To: `Running`

### `BoundaryAppliedPromote`
- From: `Running`
- On: `BoundaryApplied`(revision)
- Guards:
  - `has_pending_visibility_promotion`
  - `revision_matches_staged`
- Emits: `CommittedVisibleSetPublished`
- To: `Running`

### `BoundaryAppliedNoop`
- From: `Running`
- On: `BoundaryApplied`(revision)
- Guards:
  - `no_pending_visibility_promotion`
  - `revision_not_ahead_of_active`
- Emits: `CommittedVisibleSetPublished`
- To: `Running`

### `PublishCommittedVisibleSetAttached`
- From: `Attached`
- On: `PublishCommittedVisibleSet`(revision)
- Guards:
  - `revision_matches_active`
- Emits: `CommittedVisibleSetPublished`
- To: `Attached`

### `PublishCommittedVisibleSetRunning`
- From: `Running`
- On: `PublishCommittedVisibleSet`(revision)
- Guards:
  - `revision_matches_active`
- Emits: `CommittedVisibleSetPublished`
- To: `Running`

### `RunCompleted`
- From: `Running`
- On: `RunCompleted`(work_id)
- Guards:
  - `work_matches_active`
- Emits: `WorkCompleted`
- To: `Attached`

### `RunFailed`
- From: `Running`
- On: `RunFailed`(work_id)
- Guards:
  - `work_matches_active`
- Emits: `WorkFailed`
- To: `Attached`

### `RunCancelled`
- From: `Running`
- On: `RunCancelled`(work_id)
- Guards:
  - `work_matches_active`
- Emits: `WorkCancelled`
- To: `Attached`

### `RecoverFromIdle`
- From: `Idle`
- On: `Recover`()
- Emits: `RuntimeNotice`
- To: `Idle`

### `RecoverFromAttached`
- From: `Attached`
- On: `Recover`()
- Emits: `RuntimeNotice`
- To: `Attached`

### `RetireRequestedFromIdle`
- From: `Idle`, `Attached`, `Running`
- On: `Retire`()
- Emits: `RuntimeRetired`
- To: `Retired`

### `Reset`
- From: `Initializing`, `Idle`, `Attached`, `Recovering`, `Retired`
- On: `Reset`()
- Emits: `RuntimeNotice`
- To: `Idle`

### `StopRuntimeExecutor`
- From: `Initializing`, `Idle`, `Attached`, `Running`, `Recovering`, `Retired`
- On: `StopRuntimeExecutor`()
- Emits: `RuntimeNotice`
- To: `Stopped`

### `Destroy`
- From: `Initializing`, `Idle`, `Attached`, `Running`, `Recovering`, `Retired`, `Stopped`
- On: `Destroy`()
- Guards:
  - `runtime_is_bound`
- Emits: `RuntimeDestroyed`
- To: `Destroyed`

### `EnsureSessionWithExecutorIdle`
- From: `Idle`
- On: `EnsureSessionWithExecutor`(session_id)
- Guards:
  - `session_registered`
- To: `Idle`

### `SetSilentIntentsIdle`
- From: `Idle`
- On: `SetSilentIntents`(session_id, intents)
- Guards:
  - `session_registered`
- To: `Idle`

### `ContainsSessionIdle`
- From: `Idle`
- On: `ContainsSession`(session_id)
- Guards:
  - `session_registered`
- To: `Idle`

### `SessionHasExecutorIdle`
- From: `Idle`
- On: `SessionHasExecutor`(session_id)
- Guards:
  - `session_registered`
- To: `Idle`

### `SessionHasCommsIdle`
- From: `Idle`
- On: `SessionHasComms`(session_id)
- Guards:
  - `session_registered`
- To: `Idle`

### `OpsLifecycleRegistryIdle`
- From: `Idle`
- On: `OpsLifecycleRegistry`(session_id)
- Guards:
  - `session_registered`
- To: `Idle`

### `InputStateIdle`
- From: `Idle`
- On: `InputState`(session_id, input_id)
- Guards:
  - `session_registered`
- To: `Idle`

### `ListActiveInputsIdle`
- From: `Idle`
- On: `ListActiveInputs`(session_id)
- Guards:
  - `session_registered`
- To: `Idle`

### `AbortAttached`
- From: `Attached`
- On: `Abort`(session_id)
- Guards:
  - `session_registered`
- To: `Attached`

### `AbortRunning`
- From: `Running`
- On: `Abort`(session_id)
- Guards:
  - `session_registered`
- To: `Running`

### `WaitAttached`
- From: `Attached`
- On: `Wait`(session_id)
- Guards:
  - `session_registered`
- To: `Attached`

### `WaitRunning`
- From: `Running`
- On: `Wait`(session_id)
- Guards:
  - `session_registered`
- To: `Running`

### `AbortAllAttached`
- From: `Attached`
- On: `AbortAll`()
- To: `Attached`

### `AbortAllRunning`
- From: `Running`
- On: `AbortAll`()
- To: `Running`

### `AbortAllRecovering`
- From: `Recovering`
- On: `AbortAll`()
- To: `Recovering`

### `AbortAllRetired`
- From: `Retired`
- On: `AbortAll`()
- To: `Retired`

### `AbortAllStopped`
- From: `Stopped`
- On: `AbortAll`()
- To: `Stopped`

### `EnsureDrainRunningAttached`
- From: `Attached`
- On: `EnsureDrainRunning`()
- Guards:
  - `session_registered`
  - `peer_ingress_configured`
- Emits: `SpawnDrainTask`
- To: `Attached`

### `EnsureDrainRunningRunning`
- From: `Running`
- On: `EnsureDrainRunning`()
- Guards:
  - `session_registered`
  - `peer_ingress_configured`
- Emits: `SpawnDrainTask`
- To: `Running`

### `IngestAttached`
- From: `Attached`
- On: `Ingest`(runtime_id)
- Guards:
  - `session_registered`
- Emits: `ResolveAdmission`
- To: `Attached`

### `IngestRunning`
- From: `Running`
- On: `Ingest`(runtime_id)
- Guards:
  - `session_registered`
- Emits: `ResolveAdmission`
- To: `Running`

### `PublishEventAttached`
- From: `Attached`
- On: `PublishEvent`(kind)
- Guards:
  - `session_registered`
- Emits: `IngressNotice`
- To: `Attached`

### `PublishEventRunning`
- From: `Running`
- On: `PublishEvent`(kind)
- Guards:
  - `session_registered`
- Emits: `IngressNotice`
- To: `Running`

### `AcceptWithCompletionAttached`
- From: `Attached`
- On: `AcceptWithCompletion`(input_id)
- Guards:
  - `session_registered`
- Emits: `IngressAccepted`
- To: `Attached`

### `AcceptWithCompletionRunning`
- From: `Running`
- On: `AcceptWithCompletion`(input_id)
- Guards:
  - `session_registered`
- Emits: `IngressAccepted`
- To: `Running`

### `AcceptWithoutWakeAttached`
- From: `Attached`
- On: `AcceptWithoutWake`(input_id)
- Guards:
  - `session_registered`
- Emits: `IngressAccepted`
- To: `Attached`

### `AcceptWithoutWakeRunning`
- From: `Running`
- On: `AcceptWithoutWake`(input_id)
- Guards:
  - `session_registered`
- Emits: `IngressAccepted`
- To: `Running`

### `ClassifyExternalEnvelopeAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`()
- Guards:
  - `session_registered`
- Emits: `EnqueueClassifiedEntry`
- To: `Attached`

### `ClassifyExternalEnvelopeRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`()
- Guards:
  - `session_registered`
- Emits: `EnqueueClassifiedEntry`
- To: `Running`

### `ClassifyPlainEventAttached`
- From: `Attached`
- On: `ClassifyPlainEvent`()
- Guards:
  - `session_registered`
- Emits: `EnqueueClassifiedEntry`
- To: `Attached`

### `ClassifyPlainEventRunning`
- From: `Running`
- On: `ClassifyPlainEvent`()
- Guards:
  - `session_registered`
- Emits: `EnqueueClassifiedEntry`
- To: `Running`

### `RuntimeStateAttached`
- From: `Attached`
- On: `RuntimeState`(runtime_id)
- Guards:
  - `runtime_is_bound`
- To: `Attached`

### `RuntimeStateRunning`
- From: `Running`
- On: `RuntimeState`(runtime_id)
- Guards:
  - `runtime_is_bound`
- To: `Running`

### `LoadBoundaryReceiptAttached`
- From: `Attached`
- On: `LoadBoundaryReceipt`(runtime_id, sequence)
- Guards:
  - `runtime_is_bound`
- To: `Attached`

### `LoadBoundaryReceiptRunning`
- From: `Running`
- On: `LoadBoundaryReceipt`(runtime_id, sequence)
- Guards:
  - `runtime_is_bound`
- To: `Running`

### `PrepareAttached`
- From: `Attached`
- On: `Prepare`(session_id)
- Guards:
  - `session_registered`
- Emits: `SubmitRunPrimitive`
- To: `Attached`

### `StartConversationRunAttached`
- From: `Attached`
- On: `StartConversationRun`()
- Guards:
  - `session_registered`
- Emits: `SubmitRunPrimitive`
- To: `Attached`

### `StartImmediateAppendAttached`
- From: `Attached`
- On: `StartImmediateAppend`()
- Guards:
  - `session_registered`
- Emits: `SubmitRunPrimitive`
- To: `Attached`

### `StartImmediateContextAttached`
- From: `Attached`
- On: `StartImmediateContext`()
- Guards:
  - `session_registered`
- Emits: `SubmitRunPrimitive`
- To: `Attached`

### `CommitRunning`
- From: `Running`
- On: `Commit`(input_id, run_id)
- Guards:
  - `has_active_work`
- To: `Running`

### `FailRunning`
- From: `Running`
- On: `Fail`(run_id)
- Guards:
  - `has_active_work`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `AdmitQueuedRunning`
- From: `Running`
- On: `AdmitQueued`()
- Guards:
  - `has_active_work`
- Emits: `ResolveAdmission`
- To: `Running`

### `AdmitConsumedOnAcceptRunning`
- From: `Running`
- On: `AdmitConsumedOnAccept`()
- Guards:
  - `has_active_work`
- Emits: `ResolveAdmission`
- To: `Running`

### `StageDrainSnapshotRunning`
- From: `Running`
- On: `StageDrainSnapshot`()
- Guards:
  - `has_active_work`
- To: `Running`

### `SupersedeQueuedInputRunning`
- From: `Running`
- On: `SupersedeQueuedInput`()
- Guards:
  - `has_active_work`
- To: `Running`

### `CoalesceQueuedInputsRunning`
- From: `Running`
- On: `CoalesceQueuedInputs`()
- Guards:
  - `has_active_work`
- To: `Running`

### `SetSilentIntentOverridesRunning`
- From: `Running`
- On: `SetSilentIntentOverrides`()
- Guards:
  - `has_active_work`
- Emits: `SilentIntentApplied`
- To: `Running`

### `PrimitiveAppliedRunning`
- From: `Running`
- On: `PrimitiveApplied`()
- Guards:
  - `has_active_work`
- Emits: `SubmitRunPrimitive`
- To: `Running`

### `LlmReturnedToolCallsRunning`
- From: `Running`
- On: `LlmReturnedToolCalls`()
- Guards:
  - `has_active_work`
- To: `Running`

### `LlmReturnedTerminalRunning`
- From: `Running`
- On: `LlmReturnedTerminal`()
- Guards:
  - `has_active_work`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `RegisterPendingOpsRunning`
- From: `Running`
- On: `RegisterPendingOps`()
- Guards:
  - `has_active_work`
- Emits: `SubmitOpEvent`
- To: `Running`

### `ToolCallsResolvedRunning`
- From: `Running`
- On: `ToolCallsResolved`()
- Guards:
  - `has_active_work`
- Emits: `SubmitOpEvent`
- To: `Running`

### `OpsBarrierSatisfiedRunning`
- From: `Running`
- On: `OpsBarrierSatisfied`()
- Guards:
  - `has_active_work`
- Emits: `SubmitOpEvent`
- To: `Running`

### `BoundaryContinueRunning`
- From: `Running`
- On: `BoundaryContinue`()
- Guards:
  - `has_active_work`
- To: `Running`

### `BoundaryCompleteRunning`
- From: `Running`
- On: `BoundaryComplete`()
- Guards:
  - `has_active_work`
- Emits: `RecordBoundarySequence`
- To: `Running`

### `RecoverableFailureRunning`
- From: `Running`
- On: `RecoverableFailure`()
- Guards:
  - `has_active_work`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `FatalFailureRunning`
- From: `Running`
- On: `FatalFailure`()
- Guards:
  - `has_active_work`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `RetryRequestedRunning`
- From: `Running`
- On: `RetryRequested`()
- Guards:
  - `has_active_work`
- Emits: `SubmitRunPrimitive`
- To: `Running`

### `CancelNowRunning`
- From: `Running`
- On: `CancelNow`()
- Guards:
  - `has_active_work`
- Emits: `RequestCancellationAtBoundary`
- To: `Running`

### `CancellationObservedRunning`
- From: `Running`
- On: `CancellationObserved`()
- Guards:
  - `has_active_work`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `AcknowledgeTerminalRunning`
- From: `Running`
- On: `AcknowledgeTerminal`()
- Guards:
  - `has_active_work`
- To: `Running`

### `TurnLimitReachedRunning`
- From: `Running`
- On: `TurnLimitReached`()
- Guards:
  - `has_active_work`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `BudgetExhaustedRunning`
- From: `Running`
- On: `BudgetExhausted`()
- Guards:
  - `has_active_work`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `TimeBudgetExceededRunning`
- From: `Running`
- On: `TimeBudgetExceeded`()
- Guards:
  - `has_active_work`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `EnterExtractionRunning`
- From: `Running`
- On: `EnterExtraction`()
- Guards:
  - `has_active_work`
- To: `Running`

### `ExtractionValidationPassedRunning`
- From: `Running`
- On: `ExtractionValidationPassed`()
- Guards:
  - `has_active_work`
- To: `Running`

### `ExtractionValidationFailedRunning`
- From: `Running`
- On: `ExtractionValidationFailed`()
- Guards:
  - `has_active_work`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `ExtractionStartRunning`
- From: `Running`
- On: `ExtractionStart`()
- Guards:
  - `has_active_work`
- To: `Running`

### `ForceCancelNoRunRunning`
- From: `Running`
- On: `ForceCancelNoRun`()
- Guards:
  - `has_active_work`
- Emits: `RequestCancellationAtBoundary`
- To: `Running`

### `RegisterOperationRunning`
- From: `Running`
- On: `RegisterOperation`()
- Guards:
  - `has_active_work`
- Emits: `SubmitOpEvent`
- To: `Running`

### `ProvisioningSucceededRunning`
- From: `Running`
- On: `ProvisioningSucceeded`()
- Guards:
  - `has_active_work`
- Emits: `NotifyOpWatcher`
- To: `Running`

### `ProvisioningFailedRunning`
- From: `Running`
- On: `ProvisioningFailed`()
- Guards:
  - `has_active_work`
- Emits: `NotifyOpWatcher`
- To: `Running`

### `AbortProvisioningRunning`
- From: `Running`
- On: `AbortProvisioning`()
- Guards:
  - `has_active_work`
- Emits: `NotifyOpWatcher`
- To: `Running`

### `PeerReadyRunning`
- From: `Running`
- On: `PeerReady`()
- Guards:
  - `has_active_work`
- Emits: `ExposeOperationPeer`
- To: `Running`

### `RegisterWatcherRunning`
- From: `Running`
- On: `RegisterWatcher`()
- Guards:
  - `has_active_work`
- Emits: `NotifyOpWatcher`
- To: `Running`

### `ProgressReportedRunning`
- From: `Running`
- On: `ProgressReported`()
- Guards:
  - `has_active_work`
- Emits: `NotifyOpWatcher`
- To: `Running`

### `CompleteOperationRunning`
- From: `Running`
- On: `CompleteOperation`()
- Guards:
  - `has_active_work`
- Emits: `CompletionResolved`
- To: `Running`

### `FailOperationRunning`
- From: `Running`
- On: `FailOperation`()
- Guards:
  - `has_active_work`
- Emits: `CompletionResolved`
- To: `Running`

### `CancelOperationRunning`
- From: `Running`
- On: `CancelOperation`()
- Guards:
  - `has_active_work`
- Emits: `CompletionResolved`
- To: `Running`

### `RetireRequestedRunning`
- From: `Running`
- On: `RetireRequested`()
- Guards:
  - `has_active_work`
- Emits: `CheckCompaction`
- To: `Running`

### `RetireCompletedRunning`
- From: `Running`
- On: `RetireCompleted`()
- Guards:
  - `has_active_work`
- Emits: `CheckCompaction`
- To: `Running`

### `CollectTerminalRunning`
- From: `Running`
- On: `CollectTerminal`()
- Guards:
  - `has_active_work`
- Emits: `CollectCompletedResult`
- To: `Running`

### `BeginWaitAllRunning`
- From: `Running`
- On: `BeginWaitAll`()
- Guards:
  - `has_active_work`
- To: `Running`

### `CancelWaitAllRunning`
- From: `Running`
- On: `CancelWaitAll`()
- Guards:
  - `has_active_work`
- To: `Running`

### `StageAddAttached`
- From: `Attached`
- On: `StageAdd`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Attached`

### `StageAddRunning`
- From: `Running`
- On: `StageAdd`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Running`

### `StageRemoveAttached`
- From: `Attached`
- On: `StageRemove`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Attached`

### `StageRemoveRunning`
- From: `Running`
- On: `StageRemove`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Running`

### `StageReloadAttached`
- From: `Attached`
- On: `StageReload`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Attached`

### `StageReloadRunning`
- From: `Running`
- On: `StageReload`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Running`

### `ApplySurfaceBoundaryAttached`
- From: `Attached`
- On: `ApplySurfaceBoundary`()
- Guards:
  - `session_registered`
- Emits: `ScheduleSurfaceCompletion`
- To: `Attached`

### `ApplySurfaceBoundaryRunning`
- From: `Running`
- On: `ApplySurfaceBoundary`()
- Guards:
  - `session_registered`
- Emits: `ScheduleSurfaceCompletion`
- To: `Running`

### `PendingSucceededAttached`
- From: `Attached`
- On: `PendingSucceeded`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Attached`

### `PendingSucceededRunning`
- From: `Running`
- On: `PendingSucceeded`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Running`

### `PendingFailedAttached`
- From: `Attached`
- On: `PendingFailed`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Attached`

### `PendingFailedRunning`
- From: `Running`
- On: `PendingFailed`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Running`

### `CallStartedAttached`
- From: `Attached`
- On: `CallStarted`()
- Guards:
  - `session_registered`
- To: `Attached`

### `CallStartedRunning`
- From: `Running`
- On: `CallStarted`()
- Guards:
  - `session_registered`
- To: `Running`

### `CallFinishedAttached`
- From: `Attached`
- On: `CallFinished`()
- Guards:
  - `session_registered`
- To: `Attached`

### `CallFinishedRunning`
- From: `Running`
- On: `CallFinished`()
- Guards:
  - `session_registered`
- To: `Running`

### `FinalizeRemovalCleanAttached`
- From: `Attached`
- On: `FinalizeRemovalClean`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Attached`

### `FinalizeRemovalCleanRunning`
- From: `Running`
- On: `FinalizeRemovalClean`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Running`

### `FinalizeRemovalForcedAttached`
- From: `Attached`
- On: `FinalizeRemovalForced`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Attached`

### `FinalizeRemovalForcedRunning`
- From: `Running`
- On: `FinalizeRemovalForced`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Running`

### `SnapshotAlignedAttached`
- From: `Attached`
- On: `SnapshotAligned`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Attached`

### `SnapshotAlignedRunning`
- From: `Running`
- On: `SnapshotAligned`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Running`

### `ShutdownSurfaceAttached`
- From: `Attached`
- On: `ShutdownSurface`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Attached`

### `ShutdownSurfaceRunning`
- From: `Running`
- On: `ShutdownSurface`()
- Guards:
  - `session_registered`
- Emits: `EmitExternalToolDelta`
- To: `Running`

### `RecycleFromIdleOrRetired`
- From: `Idle`, `Retired`
- On: `Recycle`()
- Guards:
  - `runtime_is_bound`
- Emits: `InitiateRecycle`
- To: `Idle`

### `RecycleFromAttached`
- From: `Attached`
- On: `Recycle`()
- Guards:
  - `runtime_is_bound`
- Emits: `InitiateRecycle`
- To: `Attached`

## Coverage
### Code Anchors
- `meerkat-runtime/src/meerkat_machine.rs` — authoritative MeerkatMachine command dispatch and state ownership
- `meerkat/src/meerkat_machine.rs` — MeerkatMachine snapshot/diagnostic facade
- `meerkat-comms/src/peer_directory_reachability_authority.rs` — peer directory reachability state now owned as a MeerkatMachine-internal region

### Scenarios
- `bind-run-boundary-terminal` — runtime binds, runs work, applies a boundary, and reports a terminal outcome
- `retire-reset-destroy` — runtime retires, resets, stops, and destroys without reopening superseded work
- `staged_visibility_apply` — tool visibility staged state promotes into the committed visible revision at a boundary
- `turn_interrupt_and_shutdown` — running work records interrupt and shutdown intent without escaping the Meerkat authority boundary
- `peer_reachability_probe` — resolved peer directory updates and send outcomes mutate Meerkat-owned peer reachability state
