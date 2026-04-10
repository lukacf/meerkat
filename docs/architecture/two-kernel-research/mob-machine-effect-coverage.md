# MobMachine Effect Coverage

Status: frozen target-state effect handoff

This note closes the effect-side gap in the `MobMachine` freeze package.

The target freeze names semantic effects such as `ProvisionIncarnation` and
`PersistMobRun`, while the executable TLA model records them through the
`effects` region in `tla/MobMachineTarget.tla`.

This file is the canonical mapping between those two views.

Use together with:

- `mob-input-effect-alphabet.md`
- `mob-machine-transition-catalog.md`
- `mob-machine-coverage-matrix.md`
- `tla/MobMachineTarget.tla`

## Effect mapping

| Target effect | TLA effect field | Emitting transitions |
| --- | --- | --- |
| `ProvisionIncarnation` | `effects.provision_requests` | `ProvisionIncarnationFresh`, `ProvisionIncarnationRecover`, `ResetMember` |
| `RetireIncarnation` | `effects.retire_requests` | `RetireMember` |
| `DestroyIncarnation` | `effects.destroy_requests` | `DestroyMember` |
| `SubmitWorkToMeerkat` | `effects.work_submissions` | `SubmitWork`, `DispatchStep` |
| `CancelWorkInMeerkat` | `effects.work_cancellations` | `CancelWork` |
| `CancelAllWorkInMeerkat` | `effects.bulk_work_cancellations` | `CancelAllWork` |
| `PublishMobEvent` | `effects.published_events` | `FinalizeSpawn`, `AppendHistoryEvent` |
| `PersistMobRun` | `effects.persisted_runs` | `StartFlowRun`, `TrackRun`, `CompleteRun`, `FailRun`, `CancelRun`, `FailRunNoDispatchCapacity`, `DispatchStep`, `ResolveAnyJoin`, `RecordCollectionContribution`, `ResolveCollection`, `MarkStepCompleted`, `MarkStepFailed`, `MarkStepSkipped`, `MarkStepCanceled`, `OpenFrame`, `CloseFrame`, `OpenLoop`, `AdvanceLoopIteration`, `CloseLoop` |
| `PersistRoster` | `effects.persisted_roster` | `FinalizeSpawn`, `DestroyMember` |
| `PersistHistory` | `effects.persisted_history` | `FinalizeSpawn`, `AppendHistoryEvent` |
| `PersistTaskBoard` | `effects.persisted_tasks` | `OpenTask`, `UpdateTask`, `CloseTask` |

`SubmitWork` and `DispatchStep` intentionally share the same outward effect
channel but not the same machine meaning. `SubmitWork` is external member work;
`DispatchStep` is flow-owned step dispatch and is the only transition that
materializes flow-step work bindings and dispatch width.

## Review rule

If a named target effect cannot be pointed to:

1. a concrete `effects.*` field in `MobMachineTarget.tla`, and
2. at least one named emitting transition,

then the target machine is not honestly frozen yet.
