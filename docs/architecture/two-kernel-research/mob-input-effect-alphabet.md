# Mob Input / Effect Alphabet

Status: frozen target-state asset

This note defines the target-state machine input/effect alphabet for
`MobMachine`.

It is the Mob-side companion to:

- `mob-machine-freeze.md`
- `mob-machine-state-schema.md`
- `mob-machine-transition-catalog.md`
- `mob-machine-coverage-matrix.md`

The goal is to make the target orchestration machine explicit enough that TLA+
work and later implementation refinement cannot quietly smuggle in unnamed side
paths.

## Machine inputs

### Identity / authority lifecycle

| Input | Target meaning |
| --- | --- |
| `RegisterIdentity` | Create a previously absent durable identity inside the mob authority ledger. |
| `ProvisionIncarnationFresh` | Mint a fresh runtime incarnation for a known identity. |
| `ProvisionIncarnationRecover` | Rebind a recoverable runtime incarnation while preserving identity/generation continuity. |
| `FinalizeSpawn` | Commit a pending incarnation into rostered, runtime-bound member presence. |
| `FailProvision` | Terminalize a pending spawn as failed and record restore/provision failure truth. |
| `ResolveKickoff` | Clear autonomous kickoff barrier membership for one member. |
| `RetireMember` | Move one member into draining/retiring posture. |
| `ResetMember` | Advance generation and authority, replacing the active incarnation. |
| `DestroyMember` | Remove a member’s live binding and roster presence while preserving durable history. |

### Topology / coordinator

| Input | Target meaning |
| --- | --- |
| `BindCoordinator` | Establish or replace the active coordinator binding. |
| `AddTopologyEdge` | Add one collaboration edge to the topology intent ledger. |
| `RemoveTopologyEdge` | Remove one collaboration edge from the topology intent ledger. |
| `PublishExternalSpec` | Mark a member as externally spec-addressable at the Mob topology layer. |
| `ClearExternalSpec` | Remove external-spec presence for one member. |

### Work ledger

| Input | Target meaning |
| --- | --- |
| `SubmitWork` | Allocate one machine-owned `WorkRef` bound to an identity/runtime target outside flow-step dispatch. |
| `AcceptWork` | Move machine-owned work from pending into accepted state. |
| `StartWork` | Move accepted work into running state. |
| `RejectWork` | Terminalize pending work as rejected. |
| `CompleteWork` | Terminalize running work as completed. |
| `FailWork` | Terminalize running work as failed. |
| `CancelWork` | Terminalize one live work item as canceled. |
| `CancelAllWork` | Bulk-cancel all live work for one active identity/runtime binding. |

### Flow-run lifecycle

| Input | Target meaning |
| --- | --- |
| `StartFlowRun` | Create and initialize one durable flow run. |
| `TrackRun` | Attach run-tracker/cancel-token/stream membership to a known run. |
| `CompleteRun` | Terminalize a run as completed. |
| `FailRun` | Terminalize a run as failed. |
| `CancelRun` | Terminalize a run as canceled. |
| `FailRunNoDispatchCapacity` | Terminalize a run explicitly when undispatched ready work remains but no dispatch-capable member exists. |

### Step / branch / collection progress

| Input | Target meaning |
| --- | --- |
| `DispatchStep` | Dispatch one dependency-satisfied step and bind member work to it. |
| `ResolveAnyJoin` | Resolve a branch-backed `DependencyMode::Any` join into explicit machine-owned readiness. |
| `RecordCollectionContribution` | Record one observed contribution toward a `Quorum` step. |
| `ResolveCollection` | Complete a collection step once machine-owned policy conditions are satisfied. |
| `MarkStepCompleted` | Terminalize one directly dispatched non-quorum step as completed. |
| `MarkStepFailed` | Terminalize one step as failed and update machine-owned failure counters. |
| `MarkStepSkipped` | Terminalize one step as skipped. |
| `MarkStepCanceled` | Terminalize one step as canceled. |

`DispatchStep` consults explicit machine-owned per-step dispatch family
(`OneToOne`, `FanIn`, `FanOut`) rather than inferring it from helper code or
flow labels, and chooses actual dispatch multiplicity when the step leaves
machine-owned ready state.

### Frame / loop structure

| Input | Target meaning |
| --- | --- |
| `OpenFrame` | Materialize one frame region for a run. |
| `CloseFrame` | Finalize one frame region. |
| `OpenLoop` | Materialize one loop region for a run. |
| `AdvanceLoopIteration` | Advance the loop iteration ledger for one run. |
| `CloseLoop` | Finalize one loop region. |

### Tasks / recovery / history / top-level lifecycle

| Input | Target meaning |
| --- | --- |
| `OpenTask` | Create one task-board entry. |
| `UpdateTask` | Update task status or live run association. |
| `CloseTask` | Terminalize one task-board entry. |
| `RecordRestoreFailure` | Record one restore/provision failure as machine truth. |
| `ClearRestoreFailure` | Clear one restore failure record. |
| `AdvanceCheckpointVersion` | Advance checkpoint/continuity version for one identity. |
| `AppendHistoryEvent` | Append one durable history event. |
| `StopMob` | Move the top-level machine into stopped posture once active work has drained. |
| `CompleteMob` | Move the top-level machine into completed posture once active work has drained. |
| `DestroyMob` | Terminalize the top-level machine after roster and active-run teardown. |

## Machine effects

| Effect | Target meaning |
| --- | --- |
| `ProvisionIncarnation` | Request Meerkat-side incarnation provisioning for one identity/runtime/fence tuple. |
| `RetireIncarnation` | Request Meerkat-side drain/retirement for one active incarnation. |
| `DestroyIncarnation` | Request Meerkat-side destruction for one active incarnation. |
| `SubmitWorkToMeerkat` | Deliver one Mob-owned work item into the Meerkat bridge seam. |
| `CancelWorkInMeerkat` | Request Meerkat-side cancellation of one known work item. |
| `CancelAllWorkInMeerkat` | Request Meerkat-side bulk cancel for one active identity/runtime binding. |
| `PublishMobEvent` | Emit one machine-owned orchestration event to observers. |
| `PersistMobRun` | Persist durable run state. |
| `PersistRoster` | Persist durable identity/roster/topology state. |
| `PersistHistory` | Persist durable history state. |
| `PersistTaskBoard` | Persist durable task-board state. |

## Explicit exclusions

The target Mob alphabet does **not** include:

- raw `SessionId`
- `MemberRef`
- raw peer identifiers
- raw wire / unwire / peer-delivery mechanics
- Meerkat queue, barrier, drain, tool-surface, or transport internals
- scheduler / occurrence semantics
- public addressability policy beyond explicit external-spec presence

Those belong either to `MeerkatMachine` or to perimeter services.

## Freeze decision

This alphabet is the canonical target-state input/effect surface for
`MobMachine`.
