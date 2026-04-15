# MeerkatMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-runtime` / `generated::meerkat_machine`

## State
- Phase enum: `Initializing | Idle | Attached | Running | Retired | Stopped | Destroyed`
- `session_id`: `Option<SessionId>`
- `active_runtime_id`: `Option<AgentRuntimeId>`
- `active_fence_token`: `Option<FenceToken>`
- `active_generation`: `Option<Generation>`
- `active_work_id`: `Option<WorkId>`
- `attachment_live`: `Bool`
- `wake_pending`: `Bool`
- `process_pending`: `Bool`
- `pre_run_phase`: `Option<String>`
- `peer_ingress_configured`: `Bool`
- `drain_running`: `Bool`
- `current_llm_identity`: `Option<SessionLlmIdentity>`
- `current_capability_surface`: `Option<SessionLlmCapabilitySurface>`
- `capability_surface_status`: `SessionLlmCapabilitySurfaceStatus`
- `capability_base_filter`: `ToolFilter`
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
- `ReconfigureSessionLlmIdentity`(previous_identity: SessionLlmIdentity, previous_visibility_state: SessionToolVisibilityState, previous_capability_surface: Option<SessionLlmCapabilitySurface>, previous_capability_surface_status: SessionLlmCapabilitySurfaceStatus, target_identity: SessionLlmIdentity, target_capability_surface: SessionLlmCapabilitySurface, next_visibility_state: SessionToolVisibilityState, next_capability_base_filter: ToolFilter, next_active_visibility_revision: u64, tool_visibility_delta: SessionToolVisibilityDelta)
- `PrepareBindings`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `SetPeerIngressContext`(keep_alive: Bool)
- `NotifyDrainExited`(reason: String)
- `InterruptCurrentRun`
- `CancelAfterBoundary`
- `StagePersistentFilter`(filter: ToolFilter, witnesses: Map<String, ToolVisibilityWitness>)
- `RequestDeferredTools`(names: Set<String>, witnesses: Map<String, ToolVisibilityWitness>)
- `PublishCommittedVisibleSet`(active_filter: ToolFilter, staged_filter: ToolFilter, active_requested_deferred_names: Set<String>, staged_requested_deferred_names: Set<String>, active_visibility_revision: u64, staged_visibility_revision: u64)
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

## Surface-only Inputs
- `ContainsSession`
- `SessionHasExecutor`
- `SessionHasComms`
- `OpsLifecycleRegistry`
- `InputState`
- `ListActiveInputs`
- `RuntimeState`
- `LoadBoundaryReceipt`

## Signals
- `Initialize`
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
- `OwnerTerminated`
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
- `CompletionProduced`(seq: u64, operation_id: OperationId, kind: OperationKind)
- `WaitAllSatisfied`
- `CollectCompletedResult`
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
- `fence_requires_bound_runtime`
- `destroyed_has_no_active_work`
- `drain_requires_ingress_context`
- `active_requested_names_subset_of_staged`
- `committed_visibility_not_ahead_of_active`

## Transitions
### `Initialize`
- From: `Initializing`
- On: `Initialize`()
- To: `Idle`

### `RegisterSessionIdle`
- From: `Idle`
- On: `RegisterSession`(session_id)
- To: `Idle`

### `RegisterSessionAttached`
- From: `Attached`
- On: `RegisterSession`(session_id)
- To: `Attached`

### `RegisterSessionRunning`
- From: `Running`
- On: `RegisterSession`(session_id)
- To: `Running`

### `RegisterSessionRetired`
- From: `Retired`
- On: `RegisterSession`(session_id)
- To: `Retired`

### `RegisterSessionStopped`
- From: `Stopped`
- On: `RegisterSession`(session_id)
- To: `Stopped`

### `UnregisterSessionIdle`
- From: `Idle`
- On: `UnregisterSession`(session_id)
- Guards:
  - `session_matches_current`
- To: `Idle`

### `UnregisterSessionAttached`
- From: `Attached`
- On: `UnregisterSession`(session_id)
- Guards:
  - `session_matches_current`
- To: `Idle`

### `UnregisterSessionRunning`
- From: `Running`
- On: `UnregisterSession`(session_id)
- Guards:
  - `session_matches_current`
- To: `Idle`

### `UnregisterSessionRetired`
- From: `Retired`
- On: `UnregisterSession`(session_id)
- Guards:
  - `session_matches_current`
- To: `Idle`

### `UnregisterSessionStopped`
- From: `Stopped`
- On: `UnregisterSession`(session_id)
- Guards:
  - `session_matches_current`
- To: `Idle`

### `ReconfigureSessionLlmIdentityAttached`
- From: `Attached`
- On: `ReconfigureSessionLlmIdentity`(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta)
- Guards:
  - `session_registered`
  - `runtime_is_bound`
  - `reconfigure_visibility_revision_is_stable_or_bumped`
- To: `Attached`

### `ReconfigureSessionLlmIdentityRunning`
- From: `Running`
- On: `ReconfigureSessionLlmIdentity`(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta)
- Guards:
  - `session_registered`
  - `runtime_is_bound`
  - `reconfigure_visibility_revision_is_stable_or_bumped`
- To: `Running`

### `StagePersistentFilterIdle`
- From: `Idle`
- On: `StagePersistentFilter`(filter, witnesses)
- Guards:
  - `session_registered`
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

### `StagePersistentFilterRetired`
- From: `Retired`
- On: `StagePersistentFilter`(filter, witnesses)
- Guards:
  - `session_registered`
- To: `Retired`

### `StagePersistentFilterStopped`
- From: `Stopped`
- On: `StagePersistentFilter`(filter, witnesses)
- Guards:
  - `session_registered`
- To: `Stopped`

### `RequestDeferredToolsIdle`
- From: `Idle`
- On: `RequestDeferredTools`(names, witnesses)
- Guards:
  - `session_registered`
- To: `Idle`

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

### `RequestDeferredToolsRetired`
- From: `Retired`
- On: `RequestDeferredTools`(names, witnesses)
- Guards:
  - `session_registered`
- To: `Retired`

### `RequestDeferredToolsStopped`
- From: `Stopped`
- On: `RequestDeferredTools`(names, witnesses)
- Guards:
  - `session_registered`
- To: `Stopped`

### `PrepareBindingsInitializing`
- From: `Initializing`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation)
- Emits: `RuntimeBound`
- To: `Initializing`

### `PrepareBindingsIdle`
- From: `Idle`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation)
- Emits: `RuntimeBound`
- To: `Attached`

### `PrepareBindingsAttached`
- From: `Attached`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation)
- Emits: `RuntimeBound`
- To: `Attached`

### `PrepareBindingsRunning`
- From: `Running`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation)
- Emits: `RuntimeBound`
- To: `Running`

### `PrepareBindingsRetired`
- From: `Retired`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation)
- Emits: `RuntimeBound`
- To: `Retired`

### `PrepareBindingsStopped`
- From: `Stopped`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation)
- Emits: `RuntimeBound`
- To: `Stopped`

### `SetPeerIngressContextIdle`
- From: `Idle`
- On: `SetPeerIngressContext`(keep_alive)
- Guards:
  - `session_registered`
- To: `Idle`

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

### `SetPeerIngressContextRetired`
- From: `Retired`
- On: `SetPeerIngressContext`(keep_alive)
- Guards:
  - `session_registered`
- To: `Retired`

### `SetPeerIngressContextStopped`
- From: `Stopped`
- On: `SetPeerIngressContext`(keep_alive)
- Guards:
  - `session_registered`
- To: `Stopped`

### `NotifyDrainExitedIdle`
- From: `Idle`
- On: `NotifyDrainExited`(reason)
- Guards:
  - `session_registered`
- Emits: `RuntimeNotice`
- To: `Idle`

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

### `NotifyDrainExitedRetired`
- From: `Retired`
- On: `NotifyDrainExited`(reason)
- Guards:
  - `session_registered`
- Emits: `RuntimeNotice`
- To: `Retired`

### `NotifyDrainExitedStopped`
- From: `Stopped`
- On: `NotifyDrainExited`(reason)
- Guards:
  - `session_registered`
- Emits: `RuntimeNotice`
- To: `Stopped`

### `InterruptCurrentRunAttached`
- From: `Attached`
- On: `InterruptCurrentRun`()
- Guards:
  - `attachment_live`
- Emits: `WakeInterrupt`, `RequestCancellationAtBoundary`
- To: `Attached`

### `InterruptCurrentRun`
- From: `Running`
- On: `InterruptCurrentRun`()
- Guards:
  - `attachment_live`
- Emits: `WakeInterrupt`, `RequestCancellationAtBoundary`
- To: `Running`

### `CancelAfterBoundaryAttached`
- From: `Attached`
- On: `CancelAfterBoundary`()
- Guards:
  - `attachment_live`
- Emits: `RequestCancellationAtBoundary`
- To: `Attached`

### `CancelAfterBoundary`
- From: `Running`
- On: `CancelAfterBoundary`()
- Guards:
  - `attachment_live`
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

### `PublishCommittedVisibleSetIdle`
- From: `Idle`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `active_requested_names_subset_of_staged_input`
  - `equal_revision_requires_equal_active_and_staged_input`
- Emits: `CommittedVisibleSetPublished`
- To: `Idle`

### `PublishCommittedVisibleSetAttached`
- From: `Attached`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `active_requested_names_subset_of_staged_input`
  - `equal_revision_requires_equal_active_and_staged_input`
- Emits: `CommittedVisibleSetPublished`
- To: `Attached`

### `PublishCommittedVisibleSetRunning`
- From: `Running`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `active_requested_names_subset_of_staged_input`
  - `equal_revision_requires_equal_active_and_staged_input`
- Emits: `CommittedVisibleSetPublished`
- To: `Running`

### `PublishCommittedVisibleSetRetired`
- From: `Retired`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `active_requested_names_subset_of_staged_input`
  - `equal_revision_requires_equal_active_and_staged_input`
- Emits: `CommittedVisibleSetPublished`
- To: `Retired`

### `PublishCommittedVisibleSetStopped`
- From: `Stopped`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `active_requested_names_subset_of_staged_input`
  - `equal_revision_requires_equal_active_and_staged_input`
- Emits: `CommittedVisibleSetPublished`
- To: `Stopped`

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

### `RecoverFromRetired`
- From: `Retired`
- On: `Recover`()
- Emits: `RuntimeNotice`
- To: `Retired`

### `RecoverFromStopped`
- From: `Stopped`
- On: `Recover`()
- Emits: `RuntimeNotice`
- To: `Stopped`

### `RecoverFromInitializing`
- From: `Initializing`
- On: `Recover`()
- Emits: `RuntimeNotice`
- To: `Initializing`

### `RetireRequestedFromIdle`
- From: `Idle`, `Attached`, `Running`
- On: `Retire`()
- Emits: `RuntimeRetired`
- To: `Retired`

### `Reset`
- From: `Initializing`, `Idle`, `Attached`, `Retired`
- On: `Reset`()
- Emits: `RuntimeNotice`
- To: `Idle`

### `StopRuntimeExecutorDetached`
- From: `Initializing`, `Idle`, `Attached`, `Running`, `Retired`
- On: `StopRuntimeExecutor`()
- Guards:
  - `attachment_not_live`
- Emits: `RuntimeNotice`
- To: `Stopped`

### `StopRuntimeExecutorLiveAttached`
- From: `Attached`
- On: `StopRuntimeExecutor`()
- Guards:
  - `attachment_live`
- Emits: `RuntimeNotice`
- To: `Attached`

### `StopRuntimeExecutorLiveRunning`
- From: `Running`
- On: `StopRuntimeExecutor`()
- Guards:
  - `attachment_live`
- Emits: `RuntimeNotice`
- To: `Running`

### `Destroy`
- From: `Initializing`, `Idle`, `Attached`, `Running`, `Retired`, `Stopped`
- On: `Destroy`()
- Guards:
  - `runtime_is_bound`
- Emits: `RuntimeDestroyed`
- To: `Destroyed`

### `EnsureSessionWithExecutorIdle`
- From: `Idle`
- On: `EnsureSessionWithExecutor`(session_id)
- To: `Attached`

### `EnsureSessionWithExecutorAttached`
- From: `Attached`
- On: `EnsureSessionWithExecutor`(session_id)
- To: `Attached`

### `EnsureSessionWithExecutorRunning`
- From: `Running`
- On: `EnsureSessionWithExecutor`(session_id)
- To: `Running`

### `EnsureSessionWithExecutorRetired`
- From: `Retired`
- On: `EnsureSessionWithExecutor`(session_id)
- To: `Retired`

### `EnsureSessionWithExecutorStopped`
- From: `Stopped`
- On: `EnsureSessionWithExecutor`(session_id)
- To: `Stopped`

### `SetSilentIntentsIdle`
- From: `Idle`
- On: `SetSilentIntents`(session_id, intents)
- Guards:
  - `session_registered`
- To: `Idle`

### `SetSilentIntentsAttached`
- From: `Attached`
- On: `SetSilentIntents`(session_id, intents)
- Guards:
  - `session_registered`
- To: `Attached`

### `SetSilentIntentsRunning`
- From: `Running`
- On: `SetSilentIntents`(session_id, intents)
- Guards:
  - `session_registered`
- To: `Running`

### `SetSilentIntentsRetired`
- From: `Retired`
- On: `SetSilentIntents`(session_id, intents)
- Guards:
  - `session_registered`
- To: `Retired`

### `SetSilentIntentsStopped`
- From: `Stopped`
- On: `SetSilentIntents`(session_id, intents)
- Guards:
  - `session_registered`
- To: `Stopped`

### `AbortIdle`
- From: `Idle`
- On: `Abort`(session_id)
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

### `AbortRetired`
- From: `Retired`
- On: `Abort`(session_id)
- Guards:
  - `session_registered`
- To: `Retired`

### `AbortStopped`
- From: `Stopped`
- On: `Abort`(session_id)
- Guards:
  - `session_registered`
- To: `Stopped`

### `WaitIdle`
- From: `Idle`
- On: `Wait`(session_id)
- Guards:
  - `session_registered`
- To: `Idle`

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

### `WaitRetired`
- From: `Retired`
- On: `Wait`(session_id)
- Guards:
  - `session_registered`
- To: `Retired`

### `WaitStopped`
- From: `Stopped`
- On: `Wait`(session_id)
- Guards:
  - `session_registered`
- To: `Stopped`

### `AbortAllIdle`
- From: `Idle`
- On: `AbortAll`()
- To: `Idle`

### `AbortAllAttached`
- From: `Attached`
- On: `AbortAll`()
- To: `Attached`

### `AbortAllRunning`
- From: `Running`
- On: `AbortAll`()
- To: `Running`

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

### `IngestIdle`
- From: `Idle`
- On: `Ingest`(runtime_id)
- Guards:
  - `session_registered`
- Emits: `ResolveAdmission`
- To: `Idle`

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

### `PublishEventIdle`
- From: `Idle`
- On: `PublishEvent`(kind)
- Guards:
  - `session_registered`
- Emits: `IngressNotice`
- To: `Idle`

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

### `PublishEventRetired`
- From: `Retired`
- On: `PublishEvent`(kind)
- Guards:
  - `session_registered`
- Emits: `IngressNotice`
- To: `Retired`

### `PublishEventStopped`
- From: `Stopped`
- On: `PublishEvent`(kind)
- Guards:
  - `session_registered`
- Emits: `IngressNotice`
- To: `Stopped`

### `AcceptWithCompletionIdle`
- From: `Idle`
- On: `AcceptWithCompletion`(input_id)
- Guards:
  - `session_registered`
- Emits: `IngressAccepted`
- To: `Idle`

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

### `AcceptWithoutWakeIdle`
- From: `Idle`
- On: `AcceptWithoutWake`(input_id)
- Guards:
  - `session_registered`
- Emits: `IngressAccepted`
- To: `Idle`

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

### `PrepareIdle`
- From: `Idle`
- On: `Prepare`(session_id)
- Guards:
  - `session_registered`
- Emits: `SubmitRunPrimitive`
- To: `Running`

### `PrepareAttached`
- From: `Attached`
- On: `Prepare`(session_id)
- Guards:
  - `session_registered`
- Emits: `SubmitRunPrimitive`
- To: `Running`

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

### `CommitRunningToIdle`
- From: `Running`
- On: `Commit`(input_id, run_id)
- Guards:
  - `pre_run_phase_matches_idle`
- To: `Idle`

### `FailRunningToIdle`
- From: `Running`
- On: `Fail`(run_id)
- Guards:
  - `pre_run_phase_matches_idle`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `CommitRunningToAttached`
- From: `Running`
- On: `Commit`(input_id, run_id)
- Guards:
  - `pre_run_phase_matches_attached`
- To: `Attached`

### `FailRunningToAttached`
- From: `Running`
- On: `Fail`(run_id)
- Guards:
  - `pre_run_phase_matches_attached`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

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

### `OwnerTerminatedRunning`
- From: `Running`
- On: `OwnerTerminated`()
- Guards:
  - `has_active_work`
- Emits: `CompletionResolved`
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
