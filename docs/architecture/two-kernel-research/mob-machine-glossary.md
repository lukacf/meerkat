# MobMachine Glossary

Status: frozen target-state terminology

## Core identity terms

- `AgentIdentity`
  - Durable mob-owned member identity.
- `Generation`
  - Reset epoch for an identity.
- `AgentRuntimeId`
  - Current runtime incarnation for an identity.
- `FenceToken`
  - Monotonic authority token for the current live incarnation.

## Lifecycle terms

- `MobPhase`
  - Top-level machine lifecycle:
    - `Creating`
    - `Running`
    - `Stopped`
    - `Completed`
    - `Destroyed`
- `MemberStatus`
  - Per-member lifecycle posture:
    - `Pending`
    - `Active`
    - `Retiring`
    - `Broken`
    - `Completed`
    - `Destroyed`

## Flow terms

- `RunStatus`
  - Durable run lifecycle:
    - `Pending`
    - `Running`
    - `Completed`
    - `Failed`
    - `Canceled`
- `StepStatus`
  - Materialized step lifecycle:
    - `Pending`
    - `Dispatched`
    - `Completed`
    - `Failed`
    - `Skipped`
    - `Canceled`
- `DependencyMode`
  - Dependency semantics for a step:
    - `All`
    - `Any`
- `DispatchMode`
  - Dispatch-family semantics for a step:
    - `OneToOne`
    - `FanIn`
    - `FanOut`
- `CollectionPolicyKind`
  - Collection semantics for a step:
    - `All`
    - `Any`
    - `Quorum`
- `AggregateShapeClass`
  - Derived aggregate-shape class for a step:
    - scalar-like (`OneToOne + Any`)
    - array-style (`FanIn`)
    - keyed-object (all other cases)

## Structural terms

- `Frame`
  - Persisted frame-local execution frontier.
- `Loop`
  - Persisted loop-instance state.
- `LoopIteration`
  - Ledger row connecting one loop iteration to one body frame.
- `TopologyIntent`
  - Mob-owned collaboration graph over identities.

## Recovery terms

- `RestoreFailure`
  - Durable machine record that a member failed to restore its hidden runtime
    binding.
- `CheckpointVersion`
  - Monotonic continuity-facing checkpoint generation used during recovery.

## Work terms

- `WorkRef`
  - Mob-owned handle for one unit of member-directed work.
- `WorkStatus`
  - Lifecycle of a work item:
    - `Pending`
    - `Accepted`
    - `Rejected`
    - `Running`
    - `Completed`
    - `Failed`
    - `Canceled`

## History / task terms

- `EventSeq`
  - Monotonic durable event sequence for identity-scoped history.
- `TaskStatus`
  - Coordination/task-board status:
    - `Pending`
    - `InProgress`
    - `Blocked`
    - `Completed`
    - `Canceled`
