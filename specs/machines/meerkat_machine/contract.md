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
- `Initialize`
- `RegisterSession`(session_id: SessionId)
- `UnregisterSession`(session_id: SessionId)
- `PrepareBindings`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `SubmitMobWork`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId)
- `SetPeerIngressContext`(keep_alive: Bool)
- `NotifyDrainExited`(reason: String)
- `ReconcileResolvedDirectory`(keys: Set<ReachabilityKey>, reachability: Map<ReachabilityKey, PeerReachability>, last_reason: Map<ReachabilityKey, Option<PeerReachabilityReason>>)
- `RecordSendSucceeded`(key: ReachabilityKey)
- `RecordSendFailed`(key: ReachabilityKey, reason: PeerReachabilityReason)
- `InterruptCurrentRun`
- `CancelAfterBoundary`
- `StagePersistentFilter`(filter: ToolFilter, witnesses: Map<String, ToolVisibilityWitness>)
- `RequestDeferredTools`(names: Set<String>, witnesses: Map<String, ToolVisibilityWitness>)
- `PublishCommittedVisibleSet`(revision: u64)
- `BoundaryApplied`(revision: u64)
- `RunCompleted`(work_id: WorkId)
- `RunFailed`(work_id: WorkId)
- `RunCancelled`(work_id: WorkId)
- `RecoverRuntime`
- `RetireRuntime`
- `ResetRuntime`
- `StopRuntimeExecutor`
- `DestroyRuntime`
- `EnsureSessionWithExecutor`(session_id: SessionId)
- `SetSilentIntents`(session_id: SessionId, intents: Set<String>)
- `ContainsSession`(session_id: SessionId)
- `SessionHasExecutor`(session_id: SessionId)
- `SessionHasComms`(session_id: SessionId)
- `OpsLifecycleRegistry`(session_id: SessionId)
- `InputState`(session_id: SessionId, input_id: InputId)
- `ListActiveInputs`(session_id: SessionId)
- `AbortDrain`(session_id: SessionId)
- `AbortAllDrains`
- `WaitDrain`(session_id: SessionId)
- `Ingest`(runtime_id: String)
- `PublishEvent`(kind: String)
- `RuntimeState`(runtime_id: String)
- `LoadBoundaryReceipt`(runtime_id: String, sequence: u64)
- `AcceptWithCompletion`(input_id: InputId)
- `AcceptWithoutWake`(input_id: InputId)
- `PrepareLegacyRun`(session_id: SessionId)
- `CommitLegacyRun`(input_id: InputId, run_id: RunId)
- `FailLegacyRun`(run_id: RunId)
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
- `RecycleRuntime`

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

### `RecoverRuntime`
- From: `Idle`, `Stopped`, `Retired`
- On: `RecoverRuntime`()
- Emits: `RuntimeNotice`
- To: `Recovering`

### `RetireRequestedFromIdle`
- From: `Attached`, `Running`
- On: `RetireRuntime`()
- Emits: `RuntimeRetired`
- To: `Retired`

### `ResetRuntime`
- From: `Attached`, `Retired`, `Stopped`, `Recovering`
- On: `ResetRuntime`()
- Emits: `RuntimeNotice`
- To: `Idle`

### `StopRuntimeExecutor`
- From: `Attached`, `Retired`, `Recovering`
- On: `StopRuntimeExecutor`()
- Emits: `RuntimeNotice`
- To: `Stopped`

### `DestroyRuntime`
- From: `Attached`, `Running`, `Recovering`, `Retired`, `Stopped`
- On: `DestroyRuntime`()
- Guards:
  - `runtime_is_bound`
- Emits: `RuntimeDestroyed`
- To: `Destroyed`

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
