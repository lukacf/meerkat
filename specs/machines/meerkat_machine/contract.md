# MeerkatMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::meerkat_machine`

## State
- Phase enum: `Initializing | Idle | Attached | Running | Retired | Stopped | Destroyed`
- `session_id`: `Option<SessionId>`
- `active_runtime_id`: `Option<AgentRuntimeId>`
- `active_fence_token`: `Option<FenceToken>`
- `current_run_id`: `Option<RunId>`
- `pre_run_phase`: `Option<String>`
- `silent_intent_overrides`: `Set<String>`
- `realtime_intent_present`: `Bool`
- `realtime_binding_state`: `RealtimeBindingState`
- `realtime_binding_authority_epoch`: `Option<u64>`
- `realtime_reattach_required`: `Bool`
- `realtime_next_authority_epoch`: `u64`
- `live_topology_phase`: `LiveTopologyPhase`
- `mcp_server_states`: `Map<McpServerId, McpServerState>`
- `pending_peer_requests`: `Map<PeerCorrelationId, OutboundPeerRequestState>`
- `inbound_peer_requests`: `Map<PeerCorrelationId, InboundPeerRequestState>`
- `last_session_context_updated_at_ms`: `u64`
- `peer_ingress_owner_kind`: `PeerIngressOwnerKind`
- `peer_ingress_comms_runtime_id`: `Option<CommsRuntimeId>`
- `peer_ingress_mob_id`: `Option<MobId>`

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
- `Ingest`(runtime_id: AgentRuntimeId, work_id: WorkId, origin: String)
- `PublishEvent`(kind: String)
- `RuntimeState`(runtime_id: String)
- `RuntimeRealtimeAttachmentStatus`(session_id: SessionId)
- `LoadBoundaryReceipt`(runtime_id: String, sequence: u64)
- `AcceptWithCompletion`(input_id: InputId, request_immediate_processing: Bool, interrupt_yielding: Bool, wake_if_idle: Bool, run_id: RunId)
- `AcceptWithoutWake`(input_id: InputId)
- `Prepare`(session_id: SessionId, run_id: RunId)
- `Commit`(input_id: InputId, run_id: RunId)
- `Fail`(run_id: RunId)
- `Recycle`
- `ProjectRealtimeIntent`(present: Bool)
- `BeginRealtimeBinding`
- `ReplaceRealtimeBinding`
- `DetachRealtimeBinding`
- `RequireRealtimeReattach`
- `PublishRealtimeSignal`(authority_epoch: u64, next_binding_state: RealtimeBindingState)
- `McpServerConnectPending`(server_id: McpServerId)
- `McpServerConnected`(server_id: McpServerId)
- `McpServerFailed`(server_id: McpServerId, error: String)
- `McpServerDisconnected`(server_id: McpServerId)
- `McpServerReload`(server_id: McpServerId)
- `PeerRequestSent`(corr_id: PeerCorrelationId, to: String)
- `PeerResponseProgressArrived`(corr_id: PeerCorrelationId)
- `PeerResponseTerminalArrived`(corr_id: PeerCorrelationId, disposition: PeerTerminalDisposition)
- `PeerRequestTimedOut`(corr_id: PeerCorrelationId)
- `PeerRequestReceived`(corr_id: PeerCorrelationId)
- `PeerResponseReplied`(corr_id: PeerCorrelationId)
- `AdvanceSessionContext`(updated_at_ms: u64)
- `BeginLiveTopologyReconfigure`(authority_epoch: u64)
- `MarkLiveTopologyDetached`
- `ApplyLiveTopologyIdentity`
- `ApplyLiveTopologyVisibility`
- `CompleteLiveTopology`
- `AbortLiveTopologyBeforeDetach`
- `FailLiveTopologyAfterDetach`
- `AttachSessionIngress`(comms_runtime_id: CommsRuntimeId)
- `AttachMobIngress`(comms_runtime_id: CommsRuntimeId, mob_id: MobId)
- `DetachIngress`

## Surface-only Inputs
- `ContainsSession`
- `SessionHasExecutor`
- `SessionHasComms`
- `OpsLifecycleRegistry`
- `InputState`
- `ListActiveInputs`
- `RuntimeState`
- `RuntimeRealtimeAttachmentStatus`
- `LoadBoundaryReceipt`
- `Recover`

## Signals
- `Initialize`
- `BoundaryApplied`(revision: u64)
- `DrainQueuedRun`(run_id: RunId)
- `StartConversationRun`
- `StartImmediateAppend`
- `StartImmediateContext`
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
- `RuntimeBound`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `RuntimeRetired`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `RuntimeDestroyed`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
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
- `PostAdmissionSignal`(signal: String)
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
- `RealtimeIntentProjected`(present: Bool)
- `RealtimeBindingRotated`(authority_epoch: u64)
- `McpServerStateChanged`(server_id: McpServerId, new_state: McpServerState)
- `McpServerReloadRequested`(server_id: McpServerId)
- `PeerInteractionStateChanged`(corr_id: PeerCorrelationId, new_state: OutboundPeerRequestState)
- `PeerInteractionCleanup`(corr_id: PeerCorrelationId)
- `InboundPeerInteractionStateChanged`(corr_id: PeerCorrelationId, new_state: InboundPeerRequestState)
- `SessionContextAdvanced`(updated_at_ms: u64)
- `LiveTopologyPhaseChanged`

## Invariants
- `fence_requires_bound_runtime`
- `running_has_current_run`
- `current_run_only_while_running_or_retired`
- `realtime_binding_epoch_consistency`
- `peer_ingress_owner_consistency`

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
- To: `Attached`

### `ReconfigureSessionLlmIdentityRunning`
- From: `Running`
- On: `ReconfigureSessionLlmIdentity`(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta)
- Guards:
  - `session_registered`
  - `runtime_is_bound`
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
- Emits: `WakeInterrupt`, `RequestCancellationAtBoundary`
- To: `Attached`

### `InterruptCurrentRun`
- From: `Running`
- On: `InterruptCurrentRun`()
- Emits: `WakeInterrupt`, `RequestCancellationAtBoundary`
- To: `Running`

### `CancelAfterBoundaryAttached`
- From: `Attached`
- On: `CancelAfterBoundary`()
- Emits: `RequestCancellationAtBoundary`
- To: `Attached`

### `CancelAfterBoundary`
- From: `Running`
- On: `CancelAfterBoundary`()
- Emits: `RequestCancellationAtBoundary`
- To: `Running`

### `BoundaryAppliedPublish`
- From: `Running`
- On: `BoundaryApplied`(revision)
- Emits: `CommittedVisibleSetPublished`
- To: `Running`

### `PublishCommittedVisibleSetIdle`
- From: `Idle`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_requested_subset_of_staged_requested`
- Emits: `CommittedVisibleSetPublished`
- To: `Idle`

### `PublishCommittedVisibleSetAttached`
- From: `Attached`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_requested_subset_of_staged_requested`
- Emits: `CommittedVisibleSetPublished`
- To: `Attached`

### `PublishCommittedVisibleSetRunning`
- From: `Running`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_requested_subset_of_staged_requested`
- Emits: `CommittedVisibleSetPublished`
- To: `Running`

### `PublishCommittedVisibleSetRetired`
- From: `Retired`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_requested_subset_of_staged_requested`
- Emits: `CommittedVisibleSetPublished`
- To: `Retired`

### `PublishCommittedVisibleSetStopped`
- From: `Stopped`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_requested_subset_of_staged_requested`
- Emits: `CommittedVisibleSetPublished`
- To: `Stopped`

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

### `StopRuntimeExecutorUnbound`
- From: `Initializing`, `Idle`, `Retired`
- On: `StopRuntimeExecutor`()
- Emits: `RuntimeNotice`
- To: `Stopped`

### `StopRuntimeExecutorAttached`
- From: `Attached`
- On: `StopRuntimeExecutor`()
- Emits: `RuntimeNotice`
- To: `Attached`

### `StopRuntimeExecutorRunning`
- From: `Running`
- On: `StopRuntimeExecutor`()
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
- Emits: `SpawnDrainTask`
- To: `Attached`

### `EnsureDrainRunningRunning`
- From: `Running`
- On: `EnsureDrainRunning`()
- Guards:
  - `session_registered`
- Emits: `SpawnDrainTask`
- To: `Running`

### `IngestIdle`
- From: `Idle`
- On: `Ingest`(runtime_id, work_id, origin)
- Guards:
  - `session_registered`
- Emits: `ResolveAdmission`
- To: `Idle`

### `IngestAttached`
- From: `Attached`
- On: `Ingest`(runtime_id, work_id, origin)
- Guards:
  - `session_registered`
- Emits: `ResolveAdmission`
- To: `Attached`

### `IngestRunning`
- From: `Running`
- On: `Ingest`(runtime_id, work_id, origin)
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

### `AcceptWithCompletionIdleQueued`
- From: `Idle`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`
- To: `Idle`

### `AcceptWithCompletionIdleImmediate`
- From: `Idle`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`
- To: `Idle`

### `AcceptWithCompletionAttachedImmediate`
- From: `Attached`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`, `SubmitRunPrimitive`
- To: `Running`

### `AcceptWithCompletionAttachedQueued`
- From: `Attached`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`
- To: `Attached`

### `AcceptWithCompletionRunningQueuedPassive`
- From: `Running`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
  - `wake_if_idle`
- Emits: `IngressAccepted`
- To: `Running`

### `AcceptWithCompletionRunningQueuedWakeIfIdle`
- From: `Running`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
  - `wake_if_idle`
- Emits: `IngressAccepted`, `PostAdmissionSignal`
- To: `Running`

### `AcceptWithCompletionRunningInterruptYielding`
- From: `Running`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`
- To: `Running`

### `AcceptWithCompletionRunningImmediate`
- From: `Running`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`
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
- On: `Prepare`(session_id, run_id)
- Guards:
  - `session_registered`
- Emits: `SubmitRunPrimitive`
- To: `Running`

### `PrepareAttached`
- From: `Attached`
- On: `Prepare`(session_id, run_id)
- Guards:
  - `session_registered`
- Emits: `SubmitRunPrimitive`
- To: `Running`

### `DrainQueuedRunRetired`
- From: `Retired`
- On: `DrainQueuedRun`(run_id)
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
  - `current_run_id_matches_binding`
- To: `Idle`

### `CommitRunningToAttached`
- From: `Running`
- On: `Commit`(input_id, run_id)
- Guards:
  - `pre_run_phase_matches_attached`
  - `current_run_id_matches_binding`
- To: `Attached`

### `CommitRunningToRetired`
- From: `Running`
- On: `Commit`(input_id, run_id)
- Guards:
  - `pre_run_phase_matches_retired`
  - `current_run_id_matches_binding`
- To: `Retired`

### `FailRunningToIdle`
- From: `Running`
- On: `Fail`(run_id)
- Guards:
  - `pre_run_phase_matches_idle`
  - `current_run_id_matches_binding`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `FailRunningToAttached`
- From: `Running`
- On: `Fail`(run_id)
- Guards:
  - `pre_run_phase_matches_attached`
  - `current_run_id_matches_binding`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

### `FailRunningToRetired`
- From: `Running`
- On: `Fail`(run_id)
- Guards:
  - `pre_run_phase_matches_retired`
  - `current_run_id_matches_binding`
- Emits: `RecordTerminalOutcome`
- To: `Retired`

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

### `ProjectRealtimeIntentIdle`
- From: `Idle`
- On: `ProjectRealtimeIntent`(present)
- Guards:
  - `session_registered`
- Emits: `RealtimeIntentProjected`
- To: `Idle`

### `ProjectRealtimeIntentAttached`
- From: `Attached`
- On: `ProjectRealtimeIntent`(present)
- Guards:
  - `session_registered`
- Emits: `RealtimeIntentProjected`
- To: `Attached`

### `ProjectRealtimeIntentRunning`
- From: `Running`
- On: `ProjectRealtimeIntent`(present)
- Guards:
  - `session_registered`
- Emits: `RealtimeIntentProjected`
- To: `Running`

### `ProjectRealtimeIntentRetired`
- From: `Retired`
- On: `ProjectRealtimeIntent`(present)
- Guards:
  - `session_registered`
- Emits: `RealtimeIntentProjected`
- To: `Retired`

### `ProjectRealtimeIntentStopped`
- From: `Stopped`
- On: `ProjectRealtimeIntent`(present)
- Guards:
  - `session_registered`
- Emits: `RealtimeIntentProjected`
- To: `Stopped`

### `BeginRealtimeBindingIdle`
- From: `Idle`
- On: `BeginRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Idle`

### `BeginRealtimeBindingAttached`
- From: `Attached`
- On: `BeginRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Attached`

### `BeginRealtimeBindingRunning`
- From: `Running`
- On: `BeginRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Running`

### `BeginRealtimeBindingRetired`
- From: `Retired`
- On: `BeginRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Retired`

### `BeginRealtimeBindingStopped`
- From: `Stopped`
- On: `BeginRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Stopped`

### `ReplaceRealtimeBindingIdle`
- From: `Idle`
- On: `ReplaceRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Idle`

### `ReplaceRealtimeBindingAttached`
- From: `Attached`
- On: `ReplaceRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Attached`

### `ReplaceRealtimeBindingRunning`
- From: `Running`
- On: `ReplaceRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Running`

### `ReplaceRealtimeBindingRetired`
- From: `Retired`
- On: `ReplaceRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Retired`

### `ReplaceRealtimeBindingStopped`
- From: `Stopped`
- On: `ReplaceRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Stopped`

### `DetachRealtimeBindingIdle`
- From: `Idle`
- On: `DetachRealtimeBinding`()
- Guards:
  - `session_registered`
- To: `Idle`

### `DetachRealtimeBindingAttached`
- From: `Attached`
- On: `DetachRealtimeBinding`()
- Guards:
  - `session_registered`
- To: `Attached`

### `DetachRealtimeBindingRunning`
- From: `Running`
- On: `DetachRealtimeBinding`()
- Guards:
  - `session_registered`
- To: `Running`

### `DetachRealtimeBindingRetired`
- From: `Retired`
- On: `DetachRealtimeBinding`()
- Guards:
  - `session_registered`
- To: `Retired`

### `DetachRealtimeBindingStopped`
- From: `Stopped`
- On: `DetachRealtimeBinding`()
- Guards:
  - `session_registered`
- To: `Stopped`

### `RequireRealtimeReattachIdle`
- From: `Idle`
- On: `RequireRealtimeReattach`()
- Guards:
  - `session_registered`
- To: `Idle`

### `RequireRealtimeReattachAttached`
- From: `Attached`
- On: `RequireRealtimeReattach`()
- Guards:
  - `session_registered`
- To: `Attached`

### `RequireRealtimeReattachRunning`
- From: `Running`
- On: `RequireRealtimeReattach`()
- Guards:
  - `session_registered`
- To: `Running`

### `RequireRealtimeReattachRetired`
- From: `Retired`
- On: `RequireRealtimeReattach`()
- Guards:
  - `session_registered`
- To: `Retired`

### `RequireRealtimeReattachStopped`
- From: `Stopped`
- On: `RequireRealtimeReattach`()
- Guards:
  - `session_registered`
- To: `Stopped`

### `PublishRealtimeSignalIdle`
- From: `Idle`
- On: `PublishRealtimeSignal`(authority_epoch, next_binding_state)
- Guards:
  - `authority_matches_current`
  - `no_topology_reconfigure_in_progress`
  - `valid_next_state`
- To: `Idle`

### `PublishRealtimeSignalAttached`
- From: `Attached`
- On: `PublishRealtimeSignal`(authority_epoch, next_binding_state)
- Guards:
  - `authority_matches_current`
  - `no_topology_reconfigure_in_progress`
  - `valid_next_state`
- To: `Attached`

### `PublishRealtimeSignalRunning`
- From: `Running`
- On: `PublishRealtimeSignal`(authority_epoch, next_binding_state)
- Guards:
  - `authority_matches_current`
  - `no_topology_reconfigure_in_progress`
  - `valid_next_state`
- To: `Running`

### `PublishRealtimeSignalRetired`
- From: `Retired`
- On: `PublishRealtimeSignal`(authority_epoch, next_binding_state)
- Guards:
  - `authority_matches_current`
  - `no_topology_reconfigure_in_progress`
  - `valid_next_state`
- To: `Retired`

### `PublishRealtimeSignalStopped`
- From: `Stopped`
- On: `PublishRealtimeSignal`(authority_epoch, next_binding_state)
- Guards:
  - `authority_matches_current`
  - `no_topology_reconfigure_in_progress`
  - `valid_next_state`
- To: `Stopped`

### `McpServerConnectPendingIdle`
- From: `Idle`
- On: `McpServerConnectPending`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Idle`

### `McpServerConnectPendingAttached`
- From: `Attached`
- On: `McpServerConnectPending`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Attached`

### `McpServerConnectPendingRunning`
- From: `Running`
- On: `McpServerConnectPending`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Running`

### `McpServerConnectPendingRetired`
- From: `Retired`
- On: `McpServerConnectPending`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Retired`

### `McpServerConnectPendingStopped`
- From: `Stopped`
- On: `McpServerConnectPending`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Stopped`

### `McpServerConnectedIdle`
- From: `Idle`
- On: `McpServerConnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Idle`

### `McpServerConnectedAttached`
- From: `Attached`
- On: `McpServerConnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Attached`

### `McpServerConnectedRunning`
- From: `Running`
- On: `McpServerConnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Running`

### `McpServerConnectedRetired`
- From: `Retired`
- On: `McpServerConnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Retired`

### `McpServerConnectedStopped`
- From: `Stopped`
- On: `McpServerConnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Stopped`

### `McpServerFailedIdle`
- From: `Idle`
- On: `McpServerFailed`(server_id, error)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Idle`

### `McpServerFailedAttached`
- From: `Attached`
- On: `McpServerFailed`(server_id, error)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Attached`

### `McpServerFailedRunning`
- From: `Running`
- On: `McpServerFailed`(server_id, error)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Running`

### `McpServerFailedRetired`
- From: `Retired`
- On: `McpServerFailed`(server_id, error)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Retired`

### `McpServerFailedStopped`
- From: `Stopped`
- On: `McpServerFailed`(server_id, error)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Stopped`

### `McpServerDisconnectedIdle`
- From: `Idle`
- On: `McpServerDisconnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Idle`

### `McpServerDisconnectedAttached`
- From: `Attached`
- On: `McpServerDisconnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Attached`

### `McpServerDisconnectedRunning`
- From: `Running`
- On: `McpServerDisconnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Running`

### `McpServerDisconnectedRetired`
- From: `Retired`
- On: `McpServerDisconnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Retired`

### `McpServerDisconnectedStopped`
- From: `Stopped`
- On: `McpServerDisconnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Stopped`

### `McpServerReloadIdle`
- From: `Idle`
- On: `McpServerReload`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerReloadRequested`, `McpServerStateChanged`
- To: `Idle`

### `McpServerReloadAttached`
- From: `Attached`
- On: `McpServerReload`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerReloadRequested`, `McpServerStateChanged`
- To: `Attached`

### `McpServerReloadRunning`
- From: `Running`
- On: `McpServerReload`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerReloadRequested`, `McpServerStateChanged`
- To: `Running`

### `McpServerReloadRetired`
- From: `Retired`
- On: `McpServerReload`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerReloadRequested`, `McpServerStateChanged`
- To: `Retired`

### `McpServerReloadStopped`
- From: `Stopped`
- On: `McpServerReload`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerReloadRequested`, `McpServerStateChanged`
- To: `Stopped`

### `PeerRequestSentIdle`
- From: `Idle`
- On: `PeerRequestSent`(corr_id, to)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Idle`

### `PeerRequestSentAttached`
- From: `Attached`
- On: `PeerRequestSent`(corr_id, to)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Attached`

### `PeerRequestSentRunning`
- From: `Running`
- On: `PeerRequestSent`(corr_id, to)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Running`

### `PeerRequestSentRetired`
- From: `Retired`
- On: `PeerRequestSent`(corr_id, to)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Retired`

### `PeerRequestSentStopped`
- From: `Stopped`
- On: `PeerRequestSent`(corr_id, to)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Stopped`

### `PeerResponseProgressArrivedIdle`
- From: `Idle`
- On: `PeerResponseProgressArrived`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`
- To: `Idle`

### `PeerResponseProgressArrivedAttached`
- From: `Attached`
- On: `PeerResponseProgressArrived`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`
- To: `Attached`

### `PeerResponseProgressArrivedRunning`
- From: `Running`
- On: `PeerResponseProgressArrived`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`
- To: `Running`

### `PeerResponseProgressArrivedRetired`
- From: `Retired`
- On: `PeerResponseProgressArrived`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`
- To: `Retired`

### `PeerResponseProgressArrivedStopped`
- From: `Stopped`
- On: `PeerResponseProgressArrived`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`
- To: `Stopped`

### `PeerResponseTerminalArrivedCompletedIdle`
- From: `Idle`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `completed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Idle`

### `PeerResponseTerminalArrivedCompletedAttached`
- From: `Attached`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `completed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Attached`

### `PeerResponseTerminalArrivedCompletedRunning`
- From: `Running`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `completed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Running`

### `PeerResponseTerminalArrivedCompletedRetired`
- From: `Retired`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `completed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Retired`

### `PeerResponseTerminalArrivedCompletedStopped`
- From: `Stopped`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `completed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Stopped`

### `PeerResponseTerminalArrivedFailedIdle`
- From: `Idle`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `failed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Idle`

### `PeerResponseTerminalArrivedFailedAttached`
- From: `Attached`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `failed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Attached`

### `PeerResponseTerminalArrivedFailedRunning`
- From: `Running`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `failed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Running`

### `PeerResponseTerminalArrivedFailedRetired`
- From: `Retired`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `failed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Retired`

### `PeerResponseTerminalArrivedFailedStopped`
- From: `Stopped`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `failed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Stopped`

### `PeerRequestTimedOutIdle`
- From: `Idle`
- On: `PeerRequestTimedOut`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Idle`

### `PeerRequestTimedOutAttached`
- From: `Attached`
- On: `PeerRequestTimedOut`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Attached`

### `PeerRequestTimedOutRunning`
- From: `Running`
- On: `PeerRequestTimedOut`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Running`

### `PeerRequestTimedOutRetired`
- From: `Retired`
- On: `PeerRequestTimedOut`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Retired`

### `PeerRequestTimedOutStopped`
- From: `Stopped`
- On: `PeerRequestTimedOut`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Stopped`

### `PeerRequestReceivedIdle`
- From: `Idle`
- On: `PeerRequestReceived`(corr_id)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Idle`

### `PeerRequestReceivedAttached`
- From: `Attached`
- On: `PeerRequestReceived`(corr_id)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Attached`

### `PeerRequestReceivedRunning`
- From: `Running`
- On: `PeerRequestReceived`(corr_id)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Running`

### `PeerRequestReceivedRetired`
- From: `Retired`
- On: `PeerRequestReceived`(corr_id)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Retired`

### `PeerRequestReceivedStopped`
- From: `Stopped`
- On: `PeerRequestReceived`(corr_id)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Stopped`

### `PeerResponseRepliedIdle`
- From: `Idle`
- On: `PeerResponseReplied`(corr_id)
- Guards:
  - `inbound_exists`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Idle`

### `PeerResponseRepliedAttached`
- From: `Attached`
- On: `PeerResponseReplied`(corr_id)
- Guards:
  - `inbound_exists`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Attached`

### `PeerResponseRepliedRunning`
- From: `Running`
- On: `PeerResponseReplied`(corr_id)
- Guards:
  - `inbound_exists`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Running`

### `PeerResponseRepliedRetired`
- From: `Retired`
- On: `PeerResponseReplied`(corr_id)
- Guards:
  - `inbound_exists`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Retired`

### `PeerResponseRepliedStopped`
- From: `Stopped`
- On: `PeerResponseReplied`(corr_id)
- Guards:
  - `inbound_exists`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Stopped`

### `AdvanceSessionContextIdle`
- From: `Idle`
- On: `AdvanceSessionContext`(updated_at_ms)
- Guards:
  - `monotonic`
- Emits: `SessionContextAdvanced`
- To: `Idle`

### `AdvanceSessionContextAttached`
- From: `Attached`
- On: `AdvanceSessionContext`(updated_at_ms)
- Guards:
  - `monotonic`
- Emits: `SessionContextAdvanced`
- To: `Attached`

### `AdvanceSessionContextRunning`
- From: `Running`
- On: `AdvanceSessionContext`(updated_at_ms)
- Guards:
  - `monotonic`
- Emits: `SessionContextAdvanced`
- To: `Running`

### `AdvanceSessionContextRetired`
- From: `Retired`
- On: `AdvanceSessionContext`(updated_at_ms)
- Guards:
  - `monotonic`
- Emits: `SessionContextAdvanced`
- To: `Retired`

### `AdvanceSessionContextStopped`
- From: `Stopped`
- On: `AdvanceSessionContext`(updated_at_ms)
- Guards:
  - `monotonic`
- Emits: `SessionContextAdvanced`
- To: `Stopped`

### `BeginLiveTopologyReconfigureIdle`
- From: `Idle`
- On: `BeginLiveTopologyReconfigure`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
  - `topology_idle`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `BeginLiveTopologyReconfigureAttached`
- From: `Attached`
- On: `BeginLiveTopologyReconfigure`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
  - `topology_idle`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `BeginLiveTopologyReconfigureRunning`
- From: `Running`
- On: `BeginLiveTopologyReconfigure`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
  - `topology_idle`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `BeginLiveTopologyReconfigureRetired`
- From: `Retired`
- On: `BeginLiveTopologyReconfigure`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
  - `topology_idle`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `BeginLiveTopologyReconfigureStopped`
- From: `Stopped`
- On: `BeginLiveTopologyReconfigure`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
  - `topology_idle`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `MarkLiveTopologyDetachedIdle`
- From: `Idle`
- On: `MarkLiveTopologyDetached`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
  - `no_active_run`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `MarkLiveTopologyDetachedAttached`
- From: `Attached`
- On: `MarkLiveTopologyDetached`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
  - `no_active_run`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `MarkLiveTopologyDetachedRunning`
- From: `Running`
- On: `MarkLiveTopologyDetached`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
  - `no_active_run`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `MarkLiveTopologyDetachedRetired`
- From: `Retired`
- On: `MarkLiveTopologyDetached`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
  - `no_active_run`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `MarkLiveTopologyDetachedStopped`
- From: `Stopped`
- On: `MarkLiveTopologyDetached`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
  - `no_active_run`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `ApplyLiveTopologyIdentityIdle`
- From: `Idle`
- On: `ApplyLiveTopologyIdentity`()
- Guards:
  - `session_registered`
  - `topology_detached`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `ApplyLiveTopologyIdentityAttached`
- From: `Attached`
- On: `ApplyLiveTopologyIdentity`()
- Guards:
  - `session_registered`
  - `topology_detached`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `ApplyLiveTopologyIdentityRunning`
- From: `Running`
- On: `ApplyLiveTopologyIdentity`()
- Guards:
  - `session_registered`
  - `topology_detached`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `ApplyLiveTopologyIdentityRetired`
- From: `Retired`
- On: `ApplyLiveTopologyIdentity`()
- Guards:
  - `session_registered`
  - `topology_detached`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `ApplyLiveTopologyIdentityStopped`
- From: `Stopped`
- On: `ApplyLiveTopologyIdentity`()
- Guards:
  - `session_registered`
  - `topology_detached`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `ApplyLiveTopologyVisibilityIdle`
- From: `Idle`
- On: `ApplyLiveTopologyVisibility`()
- Guards:
  - `session_registered`
  - `host_identity_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `ApplyLiveTopologyVisibilityAttached`
- From: `Attached`
- On: `ApplyLiveTopologyVisibility`()
- Guards:
  - `session_registered`
  - `host_identity_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `ApplyLiveTopologyVisibilityRunning`
- From: `Running`
- On: `ApplyLiveTopologyVisibility`()
- Guards:
  - `session_registered`
  - `host_identity_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `ApplyLiveTopologyVisibilityRetired`
- From: `Retired`
- On: `ApplyLiveTopologyVisibility`()
- Guards:
  - `session_registered`
  - `host_identity_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `ApplyLiveTopologyVisibilityStopped`
- From: `Stopped`
- On: `ApplyLiveTopologyVisibility`()
- Guards:
  - `session_registered`
  - `host_identity_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `CompleteLiveTopologyIdle`
- From: `Idle`
- On: `CompleteLiveTopology`()
- Guards:
  - `session_registered`
  - `host_visibility_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `CompleteLiveTopologyAttached`
- From: `Attached`
- On: `CompleteLiveTopology`()
- Guards:
  - `session_registered`
  - `host_visibility_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `CompleteLiveTopologyRunning`
- From: `Running`
- On: `CompleteLiveTopology`()
- Guards:
  - `session_registered`
  - `host_visibility_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `CompleteLiveTopologyRetired`
- From: `Retired`
- On: `CompleteLiveTopology`()
- Guards:
  - `session_registered`
  - `host_visibility_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `CompleteLiveTopologyStopped`
- From: `Stopped`
- On: `CompleteLiveTopology`()
- Guards:
  - `session_registered`
  - `host_visibility_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `AbortLiveTopologyBeforeDetachIdle`
- From: `Idle`
- On: `AbortLiveTopologyBeforeDetach`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `AbortLiveTopologyBeforeDetachAttached`
- From: `Attached`
- On: `AbortLiveTopologyBeforeDetach`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `AbortLiveTopologyBeforeDetachRunning`
- From: `Running`
- On: `AbortLiveTopologyBeforeDetach`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `AbortLiveTopologyBeforeDetachRetired`
- From: `Retired`
- On: `AbortLiveTopologyBeforeDetach`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `AbortLiveTopologyBeforeDetachStopped`
- From: `Stopped`
- On: `AbortLiveTopologyBeforeDetach`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `FailLiveTopologyAfterDetachIdle`
- From: `Idle`
- On: `FailLiveTopologyAfterDetach`()
- Guards:
  - `session_registered`
  - `topology_past_detach`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `FailLiveTopologyAfterDetachAttached`
- From: `Attached`
- On: `FailLiveTopologyAfterDetach`()
- Guards:
  - `session_registered`
  - `topology_past_detach`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `FailLiveTopologyAfterDetachRunning`
- From: `Running`
- On: `FailLiveTopologyAfterDetach`()
- Guards:
  - `session_registered`
  - `topology_past_detach`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `FailLiveTopologyAfterDetachRetired`
- From: `Retired`
- On: `FailLiveTopologyAfterDetach`()
- Guards:
  - `session_registered`
  - `topology_past_detach`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `FailLiveTopologyAfterDetachStopped`
- From: `Stopped`
- On: `FailLiveTopologyAfterDetach`()
- Guards:
  - `session_registered`
  - `topology_past_detach`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `AttachSessionIngressIdle`
- From: `Idle`
- On: `AttachSessionIngress`(comms_runtime_id)
- Guards:
  - `session_registered`
  - `owner_is_unattached`
- To: `Idle`

### `AttachSessionIngressAttached`
- From: `Attached`
- On: `AttachSessionIngress`(comms_runtime_id)
- Guards:
  - `session_registered`
  - `owner_is_unattached`
- To: `Attached`

### `AttachSessionIngressRunning`
- From: `Running`
- On: `AttachSessionIngress`(comms_runtime_id)
- Guards:
  - `session_registered`
  - `owner_is_unattached`
- To: `Running`

### `AttachSessionIngressRetired`
- From: `Retired`
- On: `AttachSessionIngress`(comms_runtime_id)
- Guards:
  - `session_registered`
  - `owner_is_unattached`
- To: `Retired`

### `AttachSessionIngressStopped`
- From: `Stopped`
- On: `AttachSessionIngress`(comms_runtime_id)
- Guards:
  - `session_registered`
  - `owner_is_unattached`
- To: `Stopped`

### `AttachMobIngressIdle`
- From: `Idle`
- On: `AttachMobIngress`(comms_runtime_id, mob_id)
- Guards:
  - `session_registered`
  - `owner_allows_mob_attach`
- To: `Idle`

### `AttachMobIngressAttached`
- From: `Attached`
- On: `AttachMobIngress`(comms_runtime_id, mob_id)
- Guards:
  - `session_registered`
  - `owner_allows_mob_attach`
- To: `Attached`

### `AttachMobIngressRunning`
- From: `Running`
- On: `AttachMobIngress`(comms_runtime_id, mob_id)
- Guards:
  - `session_registered`
  - `owner_allows_mob_attach`
- To: `Running`

### `AttachMobIngressRetired`
- From: `Retired`
- On: `AttachMobIngress`(comms_runtime_id, mob_id)
- Guards:
  - `session_registered`
  - `owner_allows_mob_attach`
- To: `Retired`

### `AttachMobIngressStopped`
- From: `Stopped`
- On: `AttachMobIngress`(comms_runtime_id, mob_id)
- Guards:
  - `session_registered`
  - `owner_allows_mob_attach`
- To: `Stopped`

### `DetachIngressIdle`
- From: `Idle`
- On: `DetachIngress`()
- Guards:
  - `session_registered`
  - `owner_is_attached`
- To: `Idle`

### `DetachIngressAttached`
- From: `Attached`
- On: `DetachIngress`()
- Guards:
  - `session_registered`
  - `owner_is_attached`
- To: `Attached`

### `DetachIngressRunning`
- From: `Running`
- On: `DetachIngress`()
- Guards:
  - `session_registered`
  - `owner_is_attached`
- To: `Running`

### `DetachIngressRetired`
- From: `Retired`
- On: `DetachIngress`()
- Guards:
  - `session_registered`
  - `owner_is_attached`
- To: `Retired`

### `DetachIngressStopped`
- From: `Stopped`
- On: `DetachIngress`()
- Guards:
  - `session_registered`
  - `owner_is_attached`
- To: `Stopped`

## Coverage
### Code Anchors
- `meerkat-runtime/src/meerkat_machine/mod.rs` — authoritative MeerkatMachine command dispatch and state ownership
- `meerkat/src/meerkat_machine.rs` — MeerkatMachine snapshot/diagnostic facade
- `meerkat-comms/src/peer_directory_reachability_authority.rs` — peer directory reachability state now owned as a MeerkatMachine-internal region

### Scenarios
- `bind-run-boundary-terminal` — runtime binds, runs work, applies a boundary, and reports a terminal outcome
- `retire-reset-destroy` — runtime retires, resets, stops, and destroys without reopening superseded work
- `staged_visibility_apply` — tool visibility staged state promotes into the committed visible revision at a boundary
- `turn_interrupt_and_shutdown` — running work records interrupt and shutdown intent without escaping the Meerkat authority boundary
- `peer_reachability_probe` — resolved peer directory updates and send outcomes mutate Meerkat-owned peer reachability state
