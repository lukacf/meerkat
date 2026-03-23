# MobLifecycleMachine

Status: normative `0.5` machine contract, first formal-spec draft

## Purpose

`MobLifecycleMachine` owns top-level lifecycle transitions for one mob runtime
instance.

It is the authoritative owner of:

- mob lifecycle phase
- host/runtime activity gating at the mob level
- top-level mob completion/destroy/reset semantics
- lifecycle-visible cleanup of flow-tracker ownership

It is **not** the owner of:

- roster membership and wiring details
- task-board mutation
- spawn policy
- per-run flow semantics
- child-agent conversational traffic

## Scope Boundary

This machine begins when a mob runtime is created and ends when it is
destroyed.

It coordinates top-level lifecycle semantics for the mob owner; lower-level
member topology, task board, and flow execution are separate owners in `0.5`.

## Authoritative State Model

For one mob runtime instance, the machine state is the tuple:

- `mob_state: MobLifecycleState`
- `host_runtime_active: Bool`
- `mcp_surface_active: Bool`
- `tracked_flow_count: u32`

`MobLifecycleState` is the closed state set:

- `Creating`
- `Running`
- `Stopped`
- `Completed`
- `Destroyed`

Terminal state:

- `Destroyed`

## Input Alphabet

The closed external input/command alphabet for this machine is:

- `EnterRunning`
- `TrackFlow`
- `ReleaseTrackedFlow`
- `StopMob`
- `ResumeMob`
- `CompleteMob`
- `DestroyMob`
- `ResetMob`
- `ShutdownMob`

Notes:

- `TrackFlow` and `ReleaseTrackedFlow` do not model full flow semantics; they
  model the lifecycle owner's obligation to know whether active flow work is
  still attached to the mob
- `ShutdownMob` is a lifecycle-authority path that converges on destruction

## Effect Family

The closed machine-boundary effect family is:

- `StartHostRuntime`
- `StopHostRuntime`
- `StartMcpSurface`
- `StopMcpSurface`
- `EmitMobLifecycleNotice(new_state)`
- `ClearTrackedFlowOwnership`

## Transition Relation

### Startup and steady-state

1. `EnterRunning`

Preconditions:

- `mob_state = Creating`

State updates:

- `Creating -> Running`
- `host_runtime_active := TRUE`
- `mcp_surface_active := TRUE`

Effects:

- `StartHostRuntime`
- `StartMcpSurface`

2. `TrackFlow`

Preconditions:

- `mob_state = Running`

State updates:

- `tracked_flow_count += 1`

3. `ReleaseTrackedFlow`

Preconditions:

- `tracked_flow_count > 0`

State updates:

- `tracked_flow_count -= 1`

### Lifecycle control

4. `StopMob`

Preconditions:

- `mob_state = Running`

State updates:

- `Running -> Stopped`
- `host_runtime_active := FALSE`
- `mcp_surface_active := FALSE`
- `tracked_flow_count := 0`

Effects:

- `StopHostRuntime`
- `StopMcpSurface`
- `ClearTrackedFlowOwnership`

5. `ResumeMob`

Preconditions:

- `mob_state = Stopped`

State updates:

- `Stopped -> Running`
- `host_runtime_active := TRUE`
- `mcp_surface_active := TRUE`

6. `CompleteMob`

Preconditions:

- `mob_state = Running`
- `tracked_flow_count = 0`

State updates:

- `Running -> Completed`
- `host_runtime_active := FALSE`
- `mcp_surface_active := FALSE`

7. `DestroyMob` / `ShutdownMob`

Preconditions:

- `mob_state ∈ {Creating, Running, Stopped, Completed}`

State updates:

- `-> Destroyed`
- `host_runtime_active := FALSE`
- `mcp_surface_active := FALSE`
- `tracked_flow_count := 0`

8. `ResetMob`

Preconditions:

- `mob_state ∈ {Running, Stopped, Completed}`

State updates:

- `-> Running`
- `host_runtime_active := TRUE`
- `mcp_surface_active := TRUE`
- `tracked_flow_count := 0`

Hard rule:

- `Destroyed` rejects all further transitions

## Invariants

The machine must maintain:

1. destroyed mobs are drained

- `Destroyed` implies no host runtime, no MCP surface, and no tracked flows

2. running implies active mob infrastructure

- `Running` implies host/runtime activity and MCP surface activity

3. tracked flows only exist while running

- `tracked_flow_count > 0` implies `mob_state = Running`

4. completed mobs do not retain active flow ownership

- `Completed` implies `tracked_flow_count = 0`

## Model-check candidates

Best candidates for formal checking:

- destruction clears all tracked lifecycle-owned resources
- stopped/completed states do not retain active flow trackers
- host/MCP activity does not survive destruction or stop
