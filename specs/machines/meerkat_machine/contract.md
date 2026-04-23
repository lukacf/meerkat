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
- `realtime_reconnect_attempt_count`: `u64`
- `realtime_reconnect_next_retry_at_ms`: `Option<u64>`
- `realtime_reconnect_deadline_at_ms`: `Option<u64>`
- `live_topology_phase`: `LiveTopologyPhase`
- `mcp_server_states`: `Map<McpServerId, McpServerState>`
- `pending_peer_requests`: `Map<PeerCorrelationId, OutboundPeerRequestState>`
- `inbound_peer_requests`: `Map<PeerCorrelationId, InboundPeerRequestState>`
- `last_session_context_updated_at_ms`: `u64`
- `reserved_interaction_streams`: `Set<PeerCorrelationId>`
- `attached_interaction_streams`: `Set<PeerCorrelationId>`
- `realtime_product_turn_phase`: `RealtimeProductTurnPhase`
- `realtime_projection_freshness`: `RealtimeProjectionFreshness`
- `realtime_projection_frontier_ms`: `u64`
- `realtime_reconnect_policy`: `RealtimeReconnectPolicy`
- `peer_ingress_owner_kind`: `PeerIngressOwnerKind`
- `peer_ingress_comms_runtime_id`: `Option<CommsRuntimeId>`
- `peer_ingress_mob_id`: `Option<MobId>`
- `supervisor_binding_kind`: `SupervisorBindingKind`
- `supervisor_bound_name`: `Option<String>`
- `supervisor_bound_peer_id`: `Option<String>`
- `supervisor_bound_address`: `Option<String>`
- `supervisor_bound_epoch`: `Option<u64>`
- `local_endpoint`: `Option<PeerEndpoint>`
- `direct_peer_endpoints`: `Set<PeerEndpoint>`
- `mob_overlay_peer_endpoints`: `Set<PeerEndpoint>`
- `peer_projection_epoch`: `u64`
- `mob_overlay_epoch`: `u64`

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
- `Ingest`(runtime_id: AgentRuntimeId, work_id: WorkId, origin: WorkOrigin)
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
- `ProjectRealtimeReconnectProgress`(attempt_count: u64, next_retry_at_ms: Option<u64>, deadline_at_ms: Option<u64>)
- `ClearRealtimeReconnectProgress`
- `OpsBarrierSatisfied`(operation_ids: Set<OperationId>)
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
- `InteractionStreamReserved`(corr_id: PeerCorrelationId)
- `InteractionStreamAttached`(corr_id: PeerCorrelationId)
- `InteractionStreamCompleted`(corr_id: PeerCorrelationId)
- `InteractionStreamExpired`(corr_id: PeerCorrelationId)
- `InteractionStreamClosedEarly`(corr_id: PeerCorrelationId)
- `ProductTurnInFlight`
- `ProductTurnCommitted`
- `ProductOutputStarted`
- `ProductTurnInterrupted`
- `ProductTurnTerminal`
- `RealtimeProjectionAdvanceObserved`(advanced_at_ms: u64)
- `RealtimeProjectionRefreshed`(observed_ms: u64)
- `RealtimeProjectionReset`(baseline_ms: u64)
- `ClassifyRealtimeClientInputSubmitted`
- `ClassifyRealtimeMidTurnActivity`
- `ClassifyRealtimeTurnTerminated`
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
- `BindSupervisor`(name: String, peer_id: String, address: String, epoch: u64)
- `AuthorizeSupervisor`(name: String, peer_id: String, address: String, epoch: u64)
- `RevokeSupervisor`(peer_id: String, epoch: u64)
- `SupervisorTrustEdgePublished`(peer_id: String, epoch: u64)
- `SupervisorTrustEdgePublishFailed`(peer_id: String, epoch: u64, reason: String)
- `SupervisorTrustEdgeRevoked`(peer_id: String, epoch: u64)
- `SupervisorTrustEdgeRevokeFailed`(peer_id: String, epoch: u64, reason: String)
- `PublishLocalEndpoint`(endpoint: PeerEndpoint)
- `ClearLocalEndpoint`
- `AddDirectPeerEndpoint`(endpoint: PeerEndpoint)
- `RemoveDirectPeerEndpoint`(endpoint: PeerEndpoint)
- `ApplyMobPeerOverlay`(epoch: u64, endpoints: Set<PeerEndpoint>)

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
- `RuntimeNotice`(kind: RuntimeNoticeKind, detail: String)
- `ResolveAdmission`
- `SubmitAdmittedIngressEffect`
- `SubmitRunPrimitive`
- `ResolveCompletionAsTerminated`
- `ApplyControlPlaneCommand`
- `InitiateRecycle`
- `IngressAccepted`
- `PostAdmissionSignal`(signal: PostAdmissionSignalKind)
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
- `RealtimeReconnectProgressProjected`(attempt_count: u64, next_retry_at_ms: Option<u64>, deadline_at_ms: Option<u64>)
- `McpServerStateChanged`(server_id: McpServerId, new_state: McpServerState)
- `McpServerReloadRequested`(server_id: McpServerId)
- `PeerInteractionStateChanged`(corr_id: PeerCorrelationId, new_state: OutboundPeerRequestState)
- `PeerInteractionCleanup`(corr_id: PeerCorrelationId)
- `InboundPeerInteractionStateChanged`(corr_id: PeerCorrelationId, new_state: InboundPeerRequestState)
- `SessionContextAdvanced`(updated_at_ms: u64)
- `InteractionStreamStateChanged`(corr_id: PeerCorrelationId, new_state: InteractionStreamState)
- `InteractionStreamCleanup`(corr_id: PeerCorrelationId)
- `RealtimeProductTurnPhaseChanged`(new_phase: RealtimeProductTurnPhase)
- `RealtimeProjectionFreshnessChanged`(new_freshness: RealtimeProjectionFreshness, frontier_ms: u64)
- `RealtimeReconnectPolicyChanged`(new_policy: RealtimeReconnectPolicy)
- `LiveTopologyPhaseChanged`
- `LocalEndpointChanged`(endpoint: Option<PeerEndpoint>)
- `PeerProjectionChanged`(peer_projection_epoch: u64)
- `CommsTrustReconcileRequested`(peer_projection_epoch: u64)

## Invariants
- `fence_requires_bound_runtime`
- `running_has_current_run`
- `current_run_only_while_running_or_retired`
- `realtime_binding_epoch_consistency`
- `peer_ingress_owner_consistency`
- `supervisor_binding_consistency`

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

### `ProjectRealtimeReconnectProgressIdle`
- From: `Idle`
- On: `ProjectRealtimeReconnectProgress`(attempt_count, next_retry_at_ms, deadline_at_ms)
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Idle`

### `ProjectRealtimeReconnectProgressAttached`
- From: `Attached`
- On: `ProjectRealtimeReconnectProgress`(attempt_count, next_retry_at_ms, deadline_at_ms)
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Attached`

### `ProjectRealtimeReconnectProgressRunning`
- From: `Running`
- On: `ProjectRealtimeReconnectProgress`(attempt_count, next_retry_at_ms, deadline_at_ms)
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Running`

### `ProjectRealtimeReconnectProgressRetired`
- From: `Retired`
- On: `ProjectRealtimeReconnectProgress`(attempt_count, next_retry_at_ms, deadline_at_ms)
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Retired`

### `ProjectRealtimeReconnectProgressStopped`
- From: `Stopped`
- On: `ProjectRealtimeReconnectProgress`(attempt_count, next_retry_at_ms, deadline_at_ms)
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Stopped`

### `ClearRealtimeReconnectProgressIdle`
- From: `Idle`
- On: `ClearRealtimeReconnectProgress`()
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Idle`

### `ClearRealtimeReconnectProgressAttached`
- From: `Attached`
- On: `ClearRealtimeReconnectProgress`()
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Attached`

### `ClearRealtimeReconnectProgressRunning`
- From: `Running`
- On: `ClearRealtimeReconnectProgress`()
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Running`

### `ClearRealtimeReconnectProgressRetired`
- From: `Retired`
- On: `ClearRealtimeReconnectProgress`()
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Retired`

### `ClearRealtimeReconnectProgressStopped`
- From: `Stopped`
- On: `ClearRealtimeReconnectProgress`()
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
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

### `InteractionStreamReservedIdle`
- From: `Idle`
- On: `InteractionStreamReserved`(corr_id)
- Guards:
  - `not_reserved`
  - `not_attached`
- Emits: `InteractionStreamStateChanged`
- To: `Idle`

### `InteractionStreamReservedAttached`
- From: `Attached`
- On: `InteractionStreamReserved`(corr_id)
- Guards:
  - `not_reserved`
  - `not_attached`
- Emits: `InteractionStreamStateChanged`
- To: `Attached`

### `InteractionStreamReservedRunning`
- From: `Running`
- On: `InteractionStreamReserved`(corr_id)
- Guards:
  - `not_reserved`
  - `not_attached`
- Emits: `InteractionStreamStateChanged`
- To: `Running`

### `InteractionStreamReservedRetired`
- From: `Retired`
- On: `InteractionStreamReserved`(corr_id)
- Guards:
  - `not_reserved`
  - `not_attached`
- Emits: `InteractionStreamStateChanged`
- To: `Retired`

### `InteractionStreamReservedStopped`
- From: `Stopped`
- On: `InteractionStreamReserved`(corr_id)
- Guards:
  - `not_reserved`
  - `not_attached`
- Emits: `InteractionStreamStateChanged`
- To: `Stopped`

### `InteractionStreamAttachedIdle`
- From: `Idle`
- On: `InteractionStreamAttached`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`
- To: `Idle`

### `InteractionStreamAttachedAttached`
- From: `Attached`
- On: `InteractionStreamAttached`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`
- To: `Attached`

### `InteractionStreamAttachedRunning`
- From: `Running`
- On: `InteractionStreamAttached`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`
- To: `Running`

### `InteractionStreamAttachedRetired`
- From: `Retired`
- On: `InteractionStreamAttached`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`
- To: `Retired`

### `InteractionStreamAttachedStopped`
- From: `Stopped`
- On: `InteractionStreamAttached`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`
- To: `Stopped`

### `InteractionStreamCompletedIdle`
- From: `Idle`
- On: `InteractionStreamCompleted`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Idle`

### `InteractionStreamCompletedAttached`
- From: `Attached`
- On: `InteractionStreamCompleted`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Attached`

### `InteractionStreamCompletedRunning`
- From: `Running`
- On: `InteractionStreamCompleted`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Running`

### `InteractionStreamCompletedRetired`
- From: `Retired`
- On: `InteractionStreamCompleted`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Retired`

### `InteractionStreamCompletedStopped`
- From: `Stopped`
- On: `InteractionStreamCompleted`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Stopped`

### `InteractionStreamExpiredIdle`
- From: `Idle`
- On: `InteractionStreamExpired`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Idle`

### `InteractionStreamExpiredAttached`
- From: `Attached`
- On: `InteractionStreamExpired`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Attached`

### `InteractionStreamExpiredRunning`
- From: `Running`
- On: `InteractionStreamExpired`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Running`

### `InteractionStreamExpiredRetired`
- From: `Retired`
- On: `InteractionStreamExpired`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Retired`

### `InteractionStreamExpiredStopped`
- From: `Stopped`
- On: `InteractionStreamExpired`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Stopped`

### `InteractionStreamClosedEarlyIdle`
- From: `Idle`
- On: `InteractionStreamClosedEarly`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Idle`

### `InteractionStreamClosedEarlyAttached`
- From: `Attached`
- On: `InteractionStreamClosedEarly`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Attached`

### `InteractionStreamClosedEarlyRunning`
- From: `Running`
- On: `InteractionStreamClosedEarly`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Running`

### `InteractionStreamClosedEarlyRetired`
- From: `Retired`
- On: `InteractionStreamClosedEarly`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Retired`

### `InteractionStreamClosedEarlyStopped`
- From: `Stopped`
- On: `InteractionStreamClosedEarly`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Stopped`

### `ProductTurnInFlightInitializing`
- From: `Initializing`
- On: `ProductTurnInFlight`()
- Guards:
  - `only_from_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductTurnInFlightIdle`
- From: `Idle`
- On: `ProductTurnInFlight`()
- Guards:
  - `only_from_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductTurnInFlightAttached`
- From: `Attached`
- On: `ProductTurnInFlight`()
- Guards:
  - `only_from_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductTurnInFlightRunning`
- From: `Running`
- On: `ProductTurnInFlight`()
- Guards:
  - `only_from_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductTurnInFlightRetired`
- From: `Retired`
- On: `ProductTurnInFlight`()
- Guards:
  - `only_from_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductTurnInFlightStopped`
- From: `Stopped`
- On: `ProductTurnInFlight`()
- Guards:
  - `only_from_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductTurnCommittedFromAwaitingInitializing`
- From: `Initializing`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductTurnCommittedFromAwaitingIdle`
- From: `Idle`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductTurnCommittedFromAwaitingAttached`
- From: `Attached`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductTurnCommittedFromAwaitingRunning`
- From: `Running`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductTurnCommittedFromAwaitingRetired`
- From: `Retired`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductTurnCommittedFromAwaitingStopped`
- From: `Stopped`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductTurnCommittedFromOutputInitializing`
- From: `Initializing`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductTurnCommittedFromOutputIdle`
- From: `Idle`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductTurnCommittedFromOutputAttached`
- From: `Attached`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductTurnCommittedFromOutputRunning`
- From: `Running`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductTurnCommittedFromOutputRetired`
- From: `Retired`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductTurnCommittedFromOutputStopped`
- From: `Stopped`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductOutputStartedFromAwaitingInitializing`
- From: `Initializing`
- On: `ProductOutputStarted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductOutputStartedFromAwaitingIdle`
- From: `Idle`
- On: `ProductOutputStarted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductOutputStartedFromAwaitingAttached`
- From: `Attached`
- On: `ProductOutputStarted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductOutputStartedFromAwaitingRunning`
- From: `Running`
- On: `ProductOutputStarted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductOutputStartedFromAwaitingRetired`
- From: `Retired`
- On: `ProductOutputStarted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductOutputStartedFromAwaitingStopped`
- From: `Stopped`
- On: `ProductOutputStarted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductOutputStartedFromCommittedInitializing`
- From: `Initializing`
- On: `ProductOutputStarted`()
- Guards:
  - `from_committed`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductOutputStartedFromCommittedIdle`
- From: `Idle`
- On: `ProductOutputStarted`()
- Guards:
  - `from_committed`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductOutputStartedFromCommittedAttached`
- From: `Attached`
- On: `ProductOutputStarted`()
- Guards:
  - `from_committed`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductOutputStartedFromCommittedRunning`
- From: `Running`
- On: `ProductOutputStarted`()
- Guards:
  - `from_committed`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductOutputStartedFromCommittedRetired`
- From: `Retired`
- On: `ProductOutputStarted`()
- Guards:
  - `from_committed`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductOutputStartedFromCommittedStopped`
- From: `Stopped`
- On: `ProductOutputStarted`()
- Guards:
  - `from_committed`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductTurnInterruptedFromPreemptibleInitializing`
- From: `Initializing`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_preemptible`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductTurnInterruptedFromPreemptibleIdle`
- From: `Idle`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_preemptible`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductTurnInterruptedFromPreemptibleAttached`
- From: `Attached`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_preemptible`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductTurnInterruptedFromPreemptibleRunning`
- From: `Running`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_preemptible`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductTurnInterruptedFromPreemptibleRetired`
- From: `Retired`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_preemptible`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductTurnInterruptedFromPreemptibleStopped`
- From: `Stopped`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_preemptible`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductTurnInterruptedFromOutputInitializing`
- From: `Initializing`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductTurnInterruptedFromOutputIdle`
- From: `Idle`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductTurnInterruptedFromOutputAttached`
- From: `Attached`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductTurnInterruptedFromOutputRunning`
- From: `Running`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductTurnInterruptedFromOutputRetired`
- From: `Retired`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductTurnInterruptedFromOutputStopped`
- From: `Stopped`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductTurnTerminalInitializing`
- From: `Initializing`
- On: `ProductTurnTerminal`()
- Guards:
  - `not_already_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductTurnTerminalIdle`
- From: `Idle`
- On: `ProductTurnTerminal`()
- Guards:
  - `not_already_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductTurnTerminalAttached`
- From: `Attached`
- On: `ProductTurnTerminal`()
- Guards:
  - `not_already_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductTurnTerminalRunning`
- From: `Running`
- On: `ProductTurnTerminal`()
- Guards:
  - `not_already_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductTurnTerminalRetired`
- From: `Retired`
- On: `ProductTurnTerminal`()
- Guards:
  - `not_already_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductTurnTerminalStopped`
- From: `Stopped`
- On: `ProductTurnTerminal`()
- Guards:
  - `not_already_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `RealtimeProjectionAdvanceDuringTurnInitializing`
- From: `Initializing`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_in_flight`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Initializing`

### `RealtimeProjectionAdvanceDuringTurnIdle`
- From: `Idle`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_in_flight`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Idle`

### `RealtimeProjectionAdvanceDuringTurnAttached`
- From: `Attached`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_in_flight`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Attached`

### `RealtimeProjectionAdvanceDuringTurnRunning`
- From: `Running`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_in_flight`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Running`

### `RealtimeProjectionAdvanceDuringTurnRetired`
- From: `Retired`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_in_flight`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Retired`

### `RealtimeProjectionAdvanceDuringTurnStopped`
- From: `Stopped`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_in_flight`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Stopped`

### `RealtimeProjectionAdvanceWhileIdleInitializing`
- From: `Initializing`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_idle`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Initializing`

### `RealtimeProjectionAdvanceWhileIdleIdle`
- From: `Idle`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_idle`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Idle`

### `RealtimeProjectionAdvanceWhileIdleAttached`
- From: `Attached`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_idle`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Attached`

### `RealtimeProjectionAdvanceWhileIdleRunning`
- From: `Running`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_idle`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Running`

### `RealtimeProjectionAdvanceWhileIdleRetired`
- From: `Retired`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_idle`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Retired`

### `RealtimeProjectionAdvanceWhileIdleStopped`
- From: `Stopped`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_idle`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Stopped`

### `RealtimeProjectionRefreshedInitializing`
- From: `Initializing`
- On: `RealtimeProjectionRefreshed`(observed_ms)
- Guards:
  - `not_behind_frontier`
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Initializing`

### `RealtimeProjectionRefreshedIdle`
- From: `Idle`
- On: `RealtimeProjectionRefreshed`(observed_ms)
- Guards:
  - `not_behind_frontier`
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Idle`

### `RealtimeProjectionRefreshedAttached`
- From: `Attached`
- On: `RealtimeProjectionRefreshed`(observed_ms)
- Guards:
  - `not_behind_frontier`
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Attached`

### `RealtimeProjectionRefreshedRunning`
- From: `Running`
- On: `RealtimeProjectionRefreshed`(observed_ms)
- Guards:
  - `not_behind_frontier`
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Running`

### `RealtimeProjectionRefreshedRetired`
- From: `Retired`
- On: `RealtimeProjectionRefreshed`(observed_ms)
- Guards:
  - `not_behind_frontier`
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Retired`

### `RealtimeProjectionRefreshedStopped`
- From: `Stopped`
- On: `RealtimeProjectionRefreshed`(observed_ms)
- Guards:
  - `not_behind_frontier`
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Stopped`

### `RealtimeProjectionResetInitializing`
- From: `Initializing`
- On: `RealtimeProjectionReset`(baseline_ms)
- Guards:
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Initializing`

### `RealtimeProjectionResetIdle`
- From: `Idle`
- On: `RealtimeProjectionReset`(baseline_ms)
- Guards:
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Idle`

### `RealtimeProjectionResetAttached`
- From: `Attached`
- On: `RealtimeProjectionReset`(baseline_ms)
- Guards:
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Attached`

### `RealtimeProjectionResetRunning`
- From: `Running`
- On: `RealtimeProjectionReset`(baseline_ms)
- Guards:
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Running`

### `RealtimeProjectionResetRetired`
- From: `Retired`
- On: `RealtimeProjectionReset`(baseline_ms)
- Guards:
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Retired`

### `RealtimeProjectionResetStopped`
- From: `Stopped`
- On: `RealtimeProjectionReset`(baseline_ms)
- Guards:
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Stopped`

### `ClassifyRealtimeClientInputSubmittedInitializing`
- From: `Initializing`
- On: `ClassifyRealtimeClientInputSubmitted`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Initializing`

### `ClassifyRealtimeClientInputSubmittedIdle`
- From: `Idle`
- On: `ClassifyRealtimeClientInputSubmitted`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Idle`

### `ClassifyRealtimeClientInputSubmittedAttached`
- From: `Attached`
- On: `ClassifyRealtimeClientInputSubmitted`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Attached`

### `ClassifyRealtimeClientInputSubmittedRunning`
- From: `Running`
- On: `ClassifyRealtimeClientInputSubmitted`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Running`

### `ClassifyRealtimeClientInputSubmittedRetired`
- From: `Retired`
- On: `ClassifyRealtimeClientInputSubmitted`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Retired`

### `ClassifyRealtimeClientInputSubmittedStopped`
- From: `Stopped`
- On: `ClassifyRealtimeClientInputSubmitted`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Stopped`

### `ClassifyRealtimeMidTurnActivityInitializing`
- From: `Initializing`
- On: `ClassifyRealtimeMidTurnActivity`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Initializing`

### `ClassifyRealtimeMidTurnActivityIdle`
- From: `Idle`
- On: `ClassifyRealtimeMidTurnActivity`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Idle`

### `ClassifyRealtimeMidTurnActivityAttached`
- From: `Attached`
- On: `ClassifyRealtimeMidTurnActivity`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Attached`

### `ClassifyRealtimeMidTurnActivityRunning`
- From: `Running`
- On: `ClassifyRealtimeMidTurnActivity`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Running`

### `ClassifyRealtimeMidTurnActivityRetired`
- From: `Retired`
- On: `ClassifyRealtimeMidTurnActivity`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Retired`

### `ClassifyRealtimeMidTurnActivityStopped`
- From: `Stopped`
- On: `ClassifyRealtimeMidTurnActivity`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Stopped`

### `ClassifyRealtimeTurnTerminatedInitializing`
- From: `Initializing`
- On: `ClassifyRealtimeTurnTerminated`()
- Guards:
  - `actually_changing`
- Emits: `RealtimeReconnectPolicyChanged`, `RealtimeProjectionFreshnessChanged`
- To: `Initializing`

### `ClassifyRealtimeTurnTerminatedIdle`
- From: `Idle`
- On: `ClassifyRealtimeTurnTerminated`()
- Guards:
  - `actually_changing`
- Emits: `RealtimeReconnectPolicyChanged`, `RealtimeProjectionFreshnessChanged`
- To: `Idle`

### `ClassifyRealtimeTurnTerminatedAttached`
- From: `Attached`
- On: `ClassifyRealtimeTurnTerminated`()
- Guards:
  - `actually_changing`
- Emits: `RealtimeReconnectPolicyChanged`, `RealtimeProjectionFreshnessChanged`
- To: `Attached`

### `ClassifyRealtimeTurnTerminatedRunning`
- From: `Running`
- On: `ClassifyRealtimeTurnTerminated`()
- Guards:
  - `actually_changing`
- Emits: `RealtimeReconnectPolicyChanged`, `RealtimeProjectionFreshnessChanged`
- To: `Running`

### `ClassifyRealtimeTurnTerminatedRetired`
- From: `Retired`
- On: `ClassifyRealtimeTurnTerminated`()
- Guards:
  - `actually_changing`
- Emits: `RealtimeReconnectPolicyChanged`, `RealtimeProjectionFreshnessChanged`
- To: `Retired`

### `ClassifyRealtimeTurnTerminatedStopped`
- From: `Stopped`
- On: `ClassifyRealtimeTurnTerminated`()
- Guards:
  - `actually_changing`
- Emits: `RealtimeReconnectPolicyChanged`, `RealtimeProjectionFreshnessChanged`
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

### `BindSupervisorIdle`
- From: `Idle`
- On: `BindSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_unbound`
- To: `Idle`

### `BindSupervisorAttached`
- From: `Attached`
- On: `BindSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_unbound`
- To: `Attached`

### `BindSupervisorRunning`
- From: `Running`
- On: `BindSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_unbound`
- To: `Running`

### `BindSupervisorRetired`
- From: `Retired`
- On: `BindSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_unbound`
- To: `Retired`

### `BindSupervisorStopped`
- From: `Stopped`
- On: `BindSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_unbound`
- To: `Stopped`

### `AuthorizeSupervisorIdle`
- From: `Idle`
- On: `AuthorizeSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_bound`
- To: `Idle`

### `AuthorizeSupervisorAttached`
- From: `Attached`
- On: `AuthorizeSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_bound`
- To: `Attached`

### `AuthorizeSupervisorRunning`
- From: `Running`
- On: `AuthorizeSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_bound`
- To: `Running`

### `AuthorizeSupervisorRetired`
- From: `Retired`
- On: `AuthorizeSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_bound`
- To: `Retired`

### `AuthorizeSupervisorStopped`
- From: `Stopped`
- On: `AuthorizeSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_bound`
- To: `Stopped`

### `RevokeSupervisorIdle`
- From: `Idle`
- On: `RevokeSupervisor`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Idle`

### `RevokeSupervisorAttached`
- From: `Attached`
- On: `RevokeSupervisor`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Attached`

### `RevokeSupervisorRunning`
- From: `Running`
- On: `RevokeSupervisor`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Running`

### `RevokeSupervisorRetired`
- From: `Retired`
- On: `RevokeSupervisor`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Retired`

### `RevokeSupervisorStopped`
- From: `Stopped`
- On: `RevokeSupervisor`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Stopped`

### `SupervisorTrustEdgePublishedIdle`
- From: `Idle`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Idle`

### `SupervisorTrustEdgePublishedAttached`
- From: `Attached`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Attached`

### `SupervisorTrustEdgePublishedRunning`
- From: `Running`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Running`

### `SupervisorTrustEdgePublishedRetired`
- From: `Retired`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Retired`

### `SupervisorTrustEdgePublishedStopped`
- From: `Stopped`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Stopped`

### `SupervisorTrustEdgePublishFailedIdle`
- From: `Idle`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Idle`

### `SupervisorTrustEdgePublishFailedAttached`
- From: `Attached`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Attached`

### `SupervisorTrustEdgePublishFailedRunning`
- From: `Running`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Running`

### `SupervisorTrustEdgePublishFailedRetired`
- From: `Retired`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Retired`

### `SupervisorTrustEdgePublishFailedStopped`
- From: `Stopped`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Stopped`

### `SupervisorTrustEdgeRevokedIdle`
- From: `Idle`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Idle`

### `SupervisorTrustEdgeRevokedAttached`
- From: `Attached`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Attached`

### `SupervisorTrustEdgeRevokedRunning`
- From: `Running`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Running`

### `SupervisorTrustEdgeRevokedRetired`
- From: `Retired`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Retired`

### `SupervisorTrustEdgeRevokedStopped`
- From: `Stopped`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Stopped`

### `SupervisorTrustEdgeRevokeFailedIdle`
- From: `Idle`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Idle`

### `SupervisorTrustEdgeRevokeFailedAttached`
- From: `Attached`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Attached`

### `SupervisorTrustEdgeRevokeFailedRunning`
- From: `Running`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Running`

### `SupervisorTrustEdgeRevokeFailedRetired`
- From: `Retired`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Retired`

### `SupervisorTrustEdgeRevokeFailedStopped`
- From: `Stopped`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Stopped`

### `OpsBarrierSatisfiedAttached`
- From: `Attached`
- On: `OpsBarrierSatisfied`(operation_ids)
- Guards:
  - `session_registered`
- To: `Attached`

### `OpsBarrierSatisfiedRunning`
- From: `Running`
- On: `OpsBarrierSatisfied`(operation_ids)
- Guards:
  - `session_registered`
- To: `Running`

### `PublishLocalEndpointIdle`
- From: `Idle`
- On: `PublishLocalEndpoint`(endpoint)
- Emits: `LocalEndpointChanged`
- To: `Idle`

### `PublishLocalEndpointAttached`
- From: `Attached`
- On: `PublishLocalEndpoint`(endpoint)
- Emits: `LocalEndpointChanged`
- To: `Attached`

### `PublishLocalEndpointRunning`
- From: `Running`
- On: `PublishLocalEndpoint`(endpoint)
- Emits: `LocalEndpointChanged`
- To: `Running`

### `ClearLocalEndpointIdle`
- From: `Idle`
- On: `ClearLocalEndpoint`()
- Guards:
  - `local_endpoint_present`
- Emits: `LocalEndpointChanged`
- To: `Idle`

### `ClearLocalEndpointAttached`
- From: `Attached`
- On: `ClearLocalEndpoint`()
- Guards:
  - `local_endpoint_present`
- Emits: `LocalEndpointChanged`
- To: `Attached`

### `ClearLocalEndpointRunning`
- From: `Running`
- On: `ClearLocalEndpoint`()
- Guards:
  - `local_endpoint_present`
- Emits: `LocalEndpointChanged`
- To: `Running`

### `AddDirectPeerEndpointIdle`
- From: `Idle`
- On: `AddDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_not_already_direct`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Idle`

### `AddDirectPeerEndpointAttached`
- From: `Attached`
- On: `AddDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_not_already_direct`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Attached`

### `AddDirectPeerEndpointRunning`
- From: `Running`
- On: `AddDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_not_already_direct`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Running`

### `RemoveDirectPeerEndpointIdle`
- From: `Idle`
- On: `RemoveDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_present_in_direct`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Idle`

### `RemoveDirectPeerEndpointAttached`
- From: `Attached`
- On: `RemoveDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_present_in_direct`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Attached`

### `RemoveDirectPeerEndpointRunning`
- From: `Running`
- On: `RemoveDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_present_in_direct`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Running`

### `ApplyMobPeerOverlayIdle`
- From: `Idle`
- On: `ApplyMobPeerOverlay`(epoch, endpoints)
- Guards:
  - `stale_overlay_epoch`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Idle`

### `ApplyMobPeerOverlayAttached`
- From: `Attached`
- On: `ApplyMobPeerOverlay`(epoch, endpoints)
- Guards:
  - `stale_overlay_epoch`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Attached`

### `ApplyMobPeerOverlayRunning`
- From: `Running`
- On: `ApplyMobPeerOverlay`(epoch, endpoints)
- Guards:
  - `stale_overlay_epoch`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Running`

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
