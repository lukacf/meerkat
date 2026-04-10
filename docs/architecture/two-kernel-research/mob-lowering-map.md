# Mob Lowering Map

Status: frozen implementation comparison handoff

This note maps the target-state `MobMachine` inputs/effects onto the current
codebase.

It is not part of the target proof itself. It exists so later refinement and
cutover review can answer two questions honestly:

1. where does the current implementation already carry this machine action?
2. where is the target machine still a deliberate delta over today's runtime?

Use it with:

- `mob-machine-freeze.md`
- `mob-machine-exact-current-freeze.md`
- `mob-input-effect-alphabet.md`
- `bridge-alphabet.md`

## Identity / authority lifecycle

| Target input/effect | Current lowering or delta |
| --- | --- |
| `RegisterIdentity` | Today this is implicit in `MobHandle::spawn*` and `spawn_spec(...)` through `MeerkatId`-keyed roster insertion. There is no separate current public identity-registration verb. |
| `ProvisionIncarnationFresh` | Current fresh provisioning lowers through `MobHandle::spawn*`, `MobHandle::spawn_spec(...)`, and actor-side spawn/provisioner paths. |
| `ProvisionIncarnationRecover` | Current recovery lowers through spawn/resume paths in `SpawnMemberSpec` plus restore/provision logic in `runtime/builder.rs`; there is no separate public recover verb yet. |
| `FinalizeSpawn` | Today this is the actor-side commit from pending-spawn lineage into roster presence and live member binding. |
| `FailProvision` | Today this lowers through builder/provisioner failure handling and restore-failure diagnostics surfaced by `diagnostic_restore_failures_snapshot()`. |
| `ResolveKickoff` | Today this lowers through the actor-owned kickoff barrier and kickoff wait paths behind the handle APIs and kickoff snapshots. |
| `RetireMember` | `MobHandle::retire(...)` -> actor `handle_retire(...)`. |
| `ResetMember` | Target-state per-member reset. Current code only has mob-wide `MobHandle::reset()` plus retire/spawn/restore patterns and supervisor `force_reset()`. |
| `DestroyMember` | Target-state per-member destroy. Current code has mob-wide `MobHandle::destroy()` and member teardown via retire/disposal paths, but no separate one-member destroy API. |
| `ProvisionIncarnation` | Current bridge realization is the Meerkat session spawn / restore path used by mob provisioning. |
| `RetireIncarnation` | Current bridge realization is member retire / disposal through the session bridge and actor teardown. |
| `DestroyIncarnation` | Current bridge realization is session teardown during mob/member destroy paths. |

## Topology / coordinator

| Target input/effect | Current lowering or delta |
| --- | --- |
| `BindCoordinator` | Current runtime carries this through `MobOrchestratorAuthority::BindCoordinator`, `MobTopologyService::bind_coordinator()`, and actor startup/resume/reset flows. |
| `AddTopologyEdge` | `MobHandle::wire(...)` -> actor `handle_wire(...)` -> roster + trust/wiring authorities. |
| `RemoveTopologyEdge` | `MobHandle::unwire(...)` -> actor `handle_unwire(...)` -> roster + trust/wiring authorities. |
| `PublishExternalSpec` | Current realization is external `wire(...)` using `PeerTarget::External(spec)`, which stores external peer specs in roster state. |
| `ClearExternalSpec` | Current realization is external `unwire(...)` using `PeerTarget::External(spec)` or equivalent actor cleanup. |

## Work ledger / member-directed work

| Target input/effect | Current lowering or delta |
| --- | --- |
| `SubmitWork` | Target-state explicit work ledger verb. Current code lowers this indirectly through flow engine dispatch and direct member-delivery paths; there is no current public `WorkRef` API. |
| `AcceptWork` | Current realization is implicit in flow/task dispatch progression rather than a public work-ledger API. |
| `StartWork` | Current realization is implicit in active flow/member execution progression. |
| `RejectWork` | Current realization runs through flow/member rejection outcomes rather than a standalone public work verb. |
| `CompleteWork` | Current realization runs through flow/member terminal outcomes and tracker/store updates. |
| `FailWork` | Current realization runs through flow/member terminal outcomes and tracker/store updates. |
| `CancelWork` | Current realization is indirect through `cancel_flow(...)` and teardown/disposal paths; no current public per-`WorkRef` API exists. |
| `CancelAllWork` | Current realization is indirect through retire/reset/destroy and flow cancellation; no current standalone public bulk-work API exists. |
| `SubmitWorkToMeerkat` | Today this lowers through session bridge delivery, flow engine dispatch, and `internal_turn()`-style member execution paths. |
| `CancelWorkInMeerkat` | Today this lowers through flow cancellation and runtime/session cancellation paths. |
| `CancelAllWorkInMeerkat` | Today this lowers through member retirement, reset, destroy, or flow cancellation cleanup rather than a first-class bridge verb. |

## Flow / frame / loop semantics

| Target input/effect | Current lowering or delta |
| --- | --- |
| `StartFlowRun` | `MobHandle::run_flow(...)` / `run_flow_with_stream(...)` -> actor `handle_run_flow(...)`. |
| `TrackRun` | Current actor bookkeeping over run tasks, cancel tokens, event streams, and durable `MobRun` persistence. |
| `CompleteRun` / `FailRun` / `CancelRun` / `FailRunNoDispatchCapacity` | Current actor + flow engine + durable `MobRunStore` progression. |
| `DispatchStep` | Current flow engine / `flow_run_kernel` dispatch path, including `AdmitStepWork`-style lowering. |
| `ResolveAnyJoin` | Current flow kernel readiness resolution before downstream dispatch. |
| `RecordCollectionContribution` | Target-state explicit verb. Current code observes contribution through flow/collection progression; the explicit machine counter is target-state cleanup over current durable flow truth. |
| `ResolveCollection` | Current flow kernel collection-resolution path. |
| `MarkStepCompleted` / `MarkStepFailed` / `MarkStepSkipped` / `MarkStepCanceled` | Current flow kernel and actor cleanup progression. |
| `OpenFrame` / `CloseFrame` | Current frame-engine / durable run-structure realization. |
| `OpenLoop` / `AdvanceLoopIteration` / `CloseLoop` | Current loop-engine / durable run-structure realization. |
| `PersistMobRun` | Current durable `MobRun` store writes and updates. |

Dispatch family note:

- exact-current runtime families already sustain one-to-one, fan-in, and
  fan-out dispatch behavior
- target `step_dispatch_mode` is the promotion of that sustained behavior into
  one explicit machine-owned per-step field

## Tasks / recovery / history / top-level lifecycle

| Target input/effect | Current lowering or delta |
| --- | --- |
| `OpenTask` / `UpdateTask` / `CloseTask` | Current actor-owned task-board/scoped event bookkeeping; it is not surfaced as a standalone public task API. |
| `RecordRestoreFailure` | Current restore/provision failure recording in builder/actor paths plus handle diagnostics. |
| `ClearRestoreFailure` | Current actor-side cleanup of restore-failure diagnostics after successful replacement or explicit repair. |
| `AdvanceCheckpointVersion` | Current checkpoint/continuity progression is implicit in durable store + restore alignment, not a public Mob API. |
| `AppendHistoryEvent` | Current event emission and per-identity durable history tracking in actor/runtime paths. |
| `StopMob` | Current top-level lifecycle transition carried by actor/orchestrator stop logic; not a separate public API. |
| `CompleteMob` | Current top-level completion posture carried by actor/orchestrator lifecycle once active runs drain. |
| `DestroyMob` | `MobHandle::destroy()` -> actor `handle_destroy()`. |
| `PersistRoster` | Current durable roster / identity / topology persistence path. |
| `PersistHistory` | Current durable history persistence path. |
| `PersistTaskBoard` | Current task-board persistence/projection path where task state is materialized. |
| `PublishMobEvent` | Current event streams, scoped event senders, and observer-facing mob/runtime notifications. |

## Freeze read

This map is sufficient to review the target `MobMachine` freeze against the
current implementation without pretending the cutover has already happened.

Where the target machine has no one-to-one current public API, that absence is
treated as an explicit target-state delta, not as missing documentation.
