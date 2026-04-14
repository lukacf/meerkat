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
- `committed_visibility_revision`: `u32`

## Inputs
- `Initialize`
- `RegisterSession`(session_id: SessionId)
- `PrepareBindings`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `SubmitMobWork`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId)
- `InterruptCurrentRun`
- `CancelAfterBoundary`
- `PublishCommittedVisibleSet`(revision: u32)
- `BoundaryApplied`(revision: u32)
- `RunCompleted`(work_id: WorkId)
- `RunFailed`(work_id: WorkId)
- `RunCancelled`(work_id: WorkId)
- `RecoverRuntime`
- `RetireRuntime`
- `ResetRuntime`
- `StopRuntimeExecutor`
- `DestroyRuntime`

## Effects
- `RuntimeBound`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RuntimeRetired`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RuntimeDestroyed`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `WorkCompleted`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, work_id: WorkId)
- `WorkFailed`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, work_id: WorkId)
- `WorkCancelled`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, work_id: WorkId)
- `RequestCancellationAtBoundary`
- `CommittedVisibleSetPublished`(revision: u32)
- `RuntimeNotice`(kind: String, detail: String)

## Invariants
- `running_has_active_work`
- `bound_runtime_has_fence`
- `destroyed_has_no_active_work`

## Transitions
### `Initialize`
- From: `Initializing`
- On: `Initialize`()
- To: `Idle`

### `RegisterSession`
- From: `Idle`, `Stopped`, `Retired`
- On: `RegisterSession`(session_id)
- To: `Idle`

### `PrepareBindings`
- From: `Idle`, `Stopped`, `Retired`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation)
- Guards:
  - `session_registered`
- Emits: `RuntimeBound`
- To: `Attached`

### `BeginRunFromIdle`
- From: `Attached`
- On: `SubmitMobWork`(agent_runtime_id, fence_token, work_id)
- Guards:
  - `runtime_is_bound`
- To: `Running`

### `AdmissionAcceptedIdleSteer`
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
- Emits: `RequestCancellationAtBoundary`
- To: `Running`

### `CancelAfterBoundary`
- From: `Running`
- On: `CancelAfterBoundary`()
- Guards:
  - `has_active_work`
- Emits: `RequestCancellationAtBoundary`
- To: `Running`

### `BoundaryApplied`
- From: `Running`
- On: `BoundaryApplied`(revision)
- Emits: `CommittedVisibleSetPublished`
- To: `Running`

### `PublishCommittedVisibleSet`
- From: `Attached`, `Running`
- On: `PublishCommittedVisibleSet`(revision)
- Emits: `CommittedVisibleSetPublished`
- To: `Attached`

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

### Scenarios
- `bind-run-boundary-terminal` — runtime binds, runs work, applies a boundary, and reports a terminal outcome
- `retire-reset-destroy` — runtime retires, resets, stops, and destroys without reopening superseded work
