# Mob Cutover Lowering Inventory

Status: active pre-cutover inventory

This note turns `mob-lowering-map.md` and `mob-machine-refinement-delta.md`
into a cutover-facing execution list.

It answers one concrete question:

- which `MobMachine` inputs/effects are already lowered through one plausible
  canonical seam, and which still need centralization before write-side cutover

## Classification

Each entry is classified as one of:

- `canonical_ready`
- `needs_centralization`
- `target_only`

## Identity / provisioning / authority

| Input/effect | Current lowering | Status |
| --- | --- | --- |
| `ProvisionIncarnationFresh` | `MobHandle::spawn*` / `spawn_spec(...)` | `needs_centralization` |
| `ProvisionIncarnationRecover` | spawn/resume + builder restore/provision logic | `needs_centralization` |
| `FinalizeSpawn` | actor-side pending lineage -> roster commit | `needs_centralization` |
| `FailProvision` | builder/provisioner failure + restore diagnostics | `needs_centralization` |
| `ResolveKickoff` | actor-owned kickoff barrier/wait paths | `needs_centralization` |
| `RetireMember` | `MobHandle::retire(...)` -> actor `handle_retire(...)` | `canonical_ready` |
| `ResetMember` | no per-member public reset; mob-wide patterns only | `target_only` |
| `DestroyMember` | no per-member public destroy; mob-wide destroy/teardown only | `target_only` |

## Topology / coordinator

| Input/effect | Current lowering | Status |
| --- | --- | --- |
| `BindCoordinator` | orchestrator/topology authority + actor startup | `needs_centralization` |
| `AddTopologyEdge` | `MobHandle::wire(...)` -> actor `handle_wire(...)` | `canonical_ready` |
| `RemoveTopologyEdge` | `MobHandle::unwire(...)` -> actor `handle_unwire(...)` | `canonical_ready` |
| `PublishExternalSpec` | external `wire(...)` using `PeerTarget::External` | `needs_centralization` |
| `ClearExternalSpec` | external `unwire(...)` / actor cleanup | `needs_centralization` |

## Work ledger

| Input/effect | Current lowering | Status |
| --- | --- | --- |
| `SubmitWork` | indirect through flow dispatch and direct member delivery | `target_only` |
| `AcceptWork` / `StartWork` | implicit in flow/member execution progression | `target_only` |
| `RejectWork` / `CompleteWork` / `FailWork` / `CancelWork` | implicit in flow/member outcomes and cleanup | `target_only` |
| `CancelAllWork` | indirect through retire/reset/destroy + flow cancel | `target_only` |
| `SubmitWorkToMeerkat` | session bridge delivery and internal turn/flow-step helpers | `needs_centralization` |
| `CancelWorkInMeerkat` | flow cancellation + runtime/session cancel paths | `needs_centralization` |
| `CancelAllWorkInMeerkat` | member retirement/reset/destroy cleanup | `needs_centralization` |

## Flow / frame / loop

| Input/effect | Current lowering | Status |
| --- | --- | --- |
| `StartFlowRun` | `MobHandle::run_flow*` -> actor `handle_run_flow(...)` | `canonical_ready` |
| `TrackRun` | actor bookkeeping over tasks/tokens/streams/store | `needs_centralization` |
| `CompleteRun` / `FailRun` / `CancelRun` | actor + flow engine + `MobRunStore` progression | `needs_centralization` |
| `DispatchStep` | flow engine / `flow_run_kernel` dispatch path | `needs_centralization` |
| `ResolveAnyJoin` / `ResolveCollection` | flow kernel resolution paths | `needs_centralization` |
| `RecordCollectionContribution` | sustained by runtime truth, not one explicit verb yet | `target_only` |
| `MarkStepCompleted` / `MarkStepFailed` / `MarkStepSkipped` / `MarkStepCanceled` | flow kernel + actor cleanup | `needs_centralization` |
| `OpenFrame` / `CloseFrame` | frame engine + durable run structure | `needs_centralization` |
| `OpenLoop` / `AdvanceLoopIteration` / `CloseLoop` | loop engine + durable run structure | `needs_centralization` |

## Tasks / recovery / history / top-level lifecycle

| Input/effect | Current lowering | Status |
| --- | --- | --- |
| `OpenTask` / `UpdateTask` / `CloseTask` | actor-owned task board / scoped event bookkeeping | `needs_centralization` |
| `RecordRestoreFailure` / `ClearRestoreFailure` | builder/actor diagnostics and cleanup | `needs_centralization` |
| `AdvanceCheckpointVersion` | implicit through durable continuity/store paths | `target_only` |
| `AppendHistoryEvent` | event emission + identity history tracking | `needs_centralization` |
| `StopMob` / `CompleteMob` | actor/orchestrator lifecycle logic | `needs_centralization` |
| `DestroyMob` | `MobHandle::destroy()` -> actor `handle_destroy()` | `canonical_ready` |

## Immediate cutover priorities

Prioritize these first:

1. centralize provisioning / kickoff / finalize-spawn as one machine seam
2. pull work ledger semantics out of implicit flow/member helper behavior
3. centralize flow-step dispatch / collection / terminalization on one machine
   seam
4. make task/recovery/history ownership stop depending on diagnostic joins
5. decide the first public or internal canonical lowering for per-member
   reset/destroy

## Read with

- `mob-machine-refinement-delta.md`
- `mob-lowering-map.md`
- `mob-machine-freeze.md`
- `mob-meerkat-composition-refinement-delta.md`
