# MobOrchestratorMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `machines::mob_orchestrator`

## State
- Phase enum: `Creating | Running | Stopped | Completed | Destroyed`
- `coordinator_bound`: `Bool`
- `pending_spawn_count`: `u32`
- `active_flow_count`: `u32`
- `topology_revision`: `u32`
- `supervisor_active`: `Bool`

## Inputs
- `InitializeOrchestrator`
- `BindCoordinator`
- `UnbindCoordinator`
- `StageSpawn`
- `CompleteSpawn`
- `StartFlow`
- `CompleteFlow`
- `StopOrchestrator`
- `ResumeOrchestrator`
- `MarkCompleted`
- `DestroyOrchestrator`

## Effects
- `ActivateSupervisor`
- `DeactivateSupervisor`
- `EmitOrchestratorNotice`

## Invariants
- `destroyed_is_terminal`

## Transitions
### `InitializeOrchestrator`
- From: `Creating`
- On: `InitializeOrchestrator`()
- Emits: `ActivateSupervisor`
- To: `Running`

### `BindCoordinator`
- From: `Running`, `Stopped`, `Completed`
- On: `BindCoordinator`()
- Guards:
  - `coordinator_is_not_bound`
- Emits: `EmitOrchestratorNotice`
- To: `Running`

### `UnbindCoordinator`
- From: `Running`, `Stopped`, `Completed`
- On: `UnbindCoordinator`()
- Guards:
  - `coordinator_is_bound`
  - `no_pending_spawns`
- Emits: `EmitOrchestratorNotice`
- To: `Stopped`

### `StageSpawn`
- From: `Running`
- On: `StageSpawn`()
- Guards:
  - `coordinator_is_bound`
- Emits: `EmitOrchestratorNotice`
- To: `Running`

### `CompleteSpawn`
- From: `Running`, `Stopped`
- On: `CompleteSpawn`()
- Guards:
  - `has_pending_spawns`
- Emits: `EmitOrchestratorNotice`
- To: `Running`

### `StartFlow`
- From: `Running`, `Completed`
- On: `StartFlow`()
- Guards:
  - `coordinator_is_bound`
- Emits: `EmitOrchestratorNotice`
- To: `Running`

### `CompleteFlow`
- From: `Running`, `Completed`
- On: `CompleteFlow`()
- Guards:
  - `has_active_flows`
- Emits: `EmitOrchestratorNotice`
- To: `Running`

### `StopOrchestrator`
- From: `Running`, `Completed`
- On: `StopOrchestrator`()
- Guards:
  - `no_active_flows`
- Emits: `DeactivateSupervisor`
- To: `Stopped`

### `ResumeOrchestrator`
- From: `Stopped`
- On: `ResumeOrchestrator`()
- Guards:
  - `coordinator_is_bound`
- Emits: `ActivateSupervisor`
- To: `Running`

### `MarkCompleted`
- From: `Running`, `Stopped`
- On: `MarkCompleted`()
- Guards:
  - `no_active_flows`
  - `no_pending_spawns`
- Emits: `EmitOrchestratorNotice`
- To: `Completed`

### `DestroyOrchestrator`
- From: `Stopped`, `Completed`
- On: `DestroyOrchestrator`()
- Guards:
  - `no_pending_spawns`
  - `no_active_flows`
- Emits: `DeactivateSupervisor`, `EmitOrchestratorNotice`
- To: `Destroyed`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/actor.rs` — orchestration owner precursor
- `meerkat-mob/src/runtime/builder.rs` — runtime-mode/builder orchestration precursor
- `meerkat-mob/src/definition.rs` — definition-level coordinator/topology precursor

### Scenarios
- `coordinator-bind-and-supervise` — orchestrator binds coordinator authority and supervision
- `pending-spawn-ledger` — pending spawn and completion semantics remain explicit
- `topology-revision` — topology/orchestration revisions remain monotonic and owned
