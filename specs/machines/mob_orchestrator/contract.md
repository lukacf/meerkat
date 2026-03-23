# MobOrchestratorMachine

Status: normative `0.5` machine contract, first formal-spec draft

## Purpose

`MobOrchestratorMachine` owns top-level orchestration authority for one mob.

It is the authoritative owner of:

- orchestration-owner readiness and coordinator binding
- orchestration-owned pending spawn bookkeeping
- orchestration-owned active flow bookkeeping
- supervisor activation at the orchestration layer
- topology revision ownership
- ownership composition over `MobLifecycleMachine` and `FlowRunMachine`

It is **not** the owner of:

- durable per-run flow state
- top-level mob lifecycle transitions themselves
- member peer traffic
- task board mutation
- spawn policy evaluation logic

## Scope Boundary

This machine exists above `MobLifecycleMachine` and `FlowRunMachine`.

It coordinates orchestration ownership and cross-concern policy:

- who is currently acting as coordinator/orchestrator
- whether orchestration may start new flows/spawns
- whether supervision is active

It does not replace the submachines it owns.

## Authoritative State Model

For one mob instance, the machine state is the tuple:

- `orchestrator_state: OrchestratorState`
- `coordinator_bound: Bool`
- `pending_spawn_count: u32`
- `active_flow_count: u32`
- `topology_revision: u32`
- `supervisor_active: Bool`

`OrchestratorState` is the closed state set:

- `Creating`
- `Running`
- `Stopped`
- `Completed`
- `Destroyed`

Terminal state:

- `Destroyed`

## Input Alphabet

The closed external input/command alphabet for this machine is:

- `InitializeOrchestrator`
- `BindCoordinator`
- `UnbindCoordinator`
- `StageSpawn`
- `CompleteSpawn`
- `StartFlowOwnership`
- `FinishFlowOwnership`
- `AdvanceTopology`
- `StopOrchestration`
- `ResumeOrchestration`
- `CompleteOrchestration`
- `DestroyOrchestration`
- `ResetOrchestration`

Notes:

- `BindCoordinator` / `UnbindCoordinator` exist because current code derives
  the orchestrator identity from definition + roster rather than a dedicated
  persisted owner
- this machine therefore makes that owner-semantics boundary explicit in `0.5`

## Effect Family

The closed machine-boundary effect family is:

- `DriveMobLifecycle(command)`
- `DriveFlowRun(command)`
- `EmitOrchestratorNotice(kind)`
- `ApplyTopologyRevision(revision)`
- `ActivateSupervisor`
- `DeactivateSupervisor`

Architecture rule:

- `MobOrchestratorMachine` coordinates submachines and services; it does not
  collapse them back into one mega-actor loop

## Transition Relation

### Initialization and coordinator binding

1. `InitializeOrchestrator`

Preconditions:

- `orchestrator_state = Creating`

State updates:

- `Creating -> Running`
- `coordinator_bound := TRUE`
- `supervisor_active := TRUE`

2. `BindCoordinator`

Preconditions:

- `orchestrator_state ∈ {Creating, Running, Stopped, Completed}`
- `coordinator_bound = FALSE`

State updates:

- `coordinator_bound := TRUE`
- if `orchestrator_state = Running`, then `supervisor_active := TRUE`

3. `UnbindCoordinator`

Preconditions:

- `orchestrator_state ∈ {Running, Stopped, Completed}`
- `coordinator_bound = TRUE`

State updates:

- `coordinator_bound := FALSE`
- `pending_spawn_count := 0`
- `active_flow_count := 0`
- `supervisor_active := FALSE`

### Orchestration-owned work tracking

4. `StageSpawn`

Preconditions:

- `orchestrator_state = Running`
- `coordinator_bound = TRUE`

State updates:

- `pending_spawn_count += 1`

5. `CompleteSpawn`

Preconditions:

- `pending_spawn_count > 0`

State updates:

- `pending_spawn_count -= 1`

6. `StartFlowOwnership`

Preconditions:

- `orchestrator_state = Running`
- `coordinator_bound = TRUE`

State updates:

- `active_flow_count += 1`

7. `FinishFlowOwnership`

Preconditions:

- `active_flow_count > 0`

State updates:

- `active_flow_count -= 1`

8. `AdvanceTopology`

Preconditions:

- `orchestrator_state ∈ {Running, Stopped, Completed}`

State updates:

- `topology_revision += 1`

### Lifecycle-like orchestration control

9. `StopOrchestration`

Preconditions:

- `orchestrator_state = Running`

State updates:

- `Running -> Stopped`
- `pending_spawn_count := 0`
- `active_flow_count := 0`
- `supervisor_active := FALSE`

10. `ResumeOrchestration`

Preconditions:

- `orchestrator_state = Stopped`
- `coordinator_bound = TRUE`

State updates:

- `Stopped -> Running`
- `supervisor_active := TRUE`

11. `CompleteOrchestration`

Preconditions:

- `orchestrator_state = Running`
- `pending_spawn_count = 0`
- `active_flow_count = 0`

State updates:

- `Running -> Completed`
- `supervisor_active := FALSE`

12. `DestroyOrchestration`

Preconditions:

- `orchestrator_state ∈ {Creating, Running, Stopped, Completed}`

State updates:

- `-> Destroyed`
- `coordinator_bound := FALSE`
- `pending_spawn_count := 0`
- `active_flow_count := 0`
- `supervisor_active := FALSE`

13. `ResetOrchestration`

Preconditions:

- `orchestrator_state ∈ {Running, Stopped, Completed}`

State updates:

- `-> Running`
- `pending_spawn_count := 0`
- `active_flow_count := 0`
- `supervisor_active := coordinator_bound`

Hard rule:

- `Destroyed` rejects all further transitions

## Invariants

The machine must maintain:

1. destroyed orchestrators are drained

- `Destroyed` implies no coordinator binding, no pending spawns, no active
  flows, and no active supervisor

2. active flow ownership requires a running, bound coordinator

- `active_flow_count > 0` implies `Running` and `coordinator_bound = TRUE`

3. pending spawn ownership requires a running, bound coordinator

- `pending_spawn_count > 0` implies `Running` and `coordinator_bound = TRUE`

4. supervisor activation requires a running, bound coordinator

- `supervisor_active = TRUE` implies `Running` and `coordinator_bound = TRUE`

5. completed orchestration is quiescent

- `Completed` implies no pending spawns, no active flows, and no active
  supervisor

## Model-check candidates

Best candidates for formal checking:

- destroyed/quiescent cleanup of orchestration-owned trackers
- impossibility of active flows or pending spawns while coordinator is absent
- supervisor activation legality
