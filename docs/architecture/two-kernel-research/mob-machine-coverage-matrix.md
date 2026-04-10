# MobMachine Coverage Matrix

Status: frozen target-state coverage handoff

This matrix ensures the target `MobMachine` freeze has no silent gaps.

## Region coverage

| Region | State schema | Transitions | Proof obligations |
| --- | --- | --- | --- |
| `identity` | Yes | identity lifecycle | authority and binding safety |
| `lifecycle` | Yes | flow-run lifecycle, member lifecycle | phase and run-count coherence |
| `roster` | Yes | spawn/finalize/retire/destroy | roster membership coherence |
| `topology` | Yes | bind/add/remove edge | coordinator/topology safety |
| `provisioning` | Yes | provision/fail/finalize/kickoff | pending spawn and kickoff safety |
| `work` | Yes | submit/accept/start/terminal/cancel | work-ledger safety |
| `flows` | Yes | run/step/dispatch/branch/collection/frame/loop | flow structural safety, dispatch-family semantics, and work/step coupling |
| `tasks` | Yes | open/update/close task | task binding safety |
| `recovery` | Yes | record/clear restore, checkpoint advance | recovery safety |
| `history` | Yes | append history event | sequence monotonicity |

## Alphabet coverage

### Inputs

- `RegisterIdentity`
- `ProvisionIncarnationFresh`
- `ProvisionIncarnationRecover`
- `FinalizeSpawn`
- `FailProvision`
- `ResolveKickoff`
- `RetireMember`
- `ResetMember`
- `DestroyMember`
- `BindCoordinator`
- `AddTopologyEdge`
- `RemoveTopologyEdge`
- `PublishExternalSpec`
- `ClearExternalSpec`
- `SubmitWork`
- `AcceptWork`
- `StartWork`
- `RejectWork`
- `CompleteWork`
- `FailWork`
- `CancelWork`
- `CancelAllWork`
- `StartFlowRun`
- `TrackRun`
- `CompleteRun`
- `FailRun`
- `CancelRun`
- `FailRunNoDispatchCapacity`
- `DispatchStep`
- `MarkStepCompleted`
- `MarkStepFailed`
- `MarkStepSkipped`
- `MarkStepCanceled`
- `ResolveAnyJoin`
- `RecordCollectionContribution`
- `ResolveCollection`
- `OpenFrame`
- `CloseFrame`
- `OpenLoop`
- `AdvanceLoopIteration`
- `CloseLoop`
- `RecordRestoreFailure`
- `ClearRestoreFailure`
- `AdvanceCheckpointVersion`
- `AppendHistoryEvent`
- `OpenTask`
- `UpdateTask`
- `CloseTask`
- `StopMob`
- `CompleteMob`
- `DestroyMob`

### Effects

The frozen target machine emits:

- `ProvisionIncarnation`
- `RetireIncarnation`
- `DestroyIncarnation`
- `SubmitWorkToMeerkat`
- `CancelWorkInMeerkat`
- `CancelAllWorkInMeerkat`
- `PublishMobEvent`
- `PersistMobRun`
- `PersistRoster`
- `PersistHistory`
- `PersistTaskBoard`

## Derived vocabulary coverage

Every predicate listed in `mob-machine-derived-predicates.md` has a direct
definition in `tla/MobMachineTarget.tla`.

## Review rule

If a region, input, or effect cannot be pointed to in this matrix, it is not
part of the frozen target machine yet.
