# Machine-Driven Execution Control Inventory

Status: scoped high-blast pilot implemented; verification in progress
Date: 2026-06-16

This inventory tracks the execution-control proposal migration. It is not a
permanent ledger unless it keeps proving useful.

## Semantic Executor Inventory

| action | owning machine | current transition/input | current shell function | required identities | effect closure requirement | current bypass risk |
| --- | --- | --- | --- | --- | --- | --- |
| Materialize accepted input into physical queue projection | MeerkatMachine | `ResolveAdmissionPlan`, `QueueAccepted`, `SteerAccepted`, recovery inputs | `EphemeralRuntimeDriver::apply_persist_and_queue`, recovery rebuilds | input id, lane, admission sequence, runtime/session authority, recovery normalization | No semantic external effect; projection must be reconstructable from durable machine witnesses | Raw queue projection mutation is allowed only inside materialization/rebuild mechanics and is covered by `runtime-authority-bypass` |
| Select runtime-loop batch | MeerkatMachine | Admission runtime-loop grouping witnesses (stamped inline by `ResolveAdmissionPlan` live / `RecoverInputLifecycle` recovery) plus queued lane/order facts | `machine_authorize_runtime_loop_batch` | input ids, source lane, runtime boundary, execution kind, peer-response terminal intent, prompt grouping | No external effect; must be recomputable after restart | Selection returns `AuthorizedRuntimeLoopBatch` from generated machine-state command-plan selection; projection drift before dequeue fails closed |
| Dequeue runtime-loop batch from physical projection | MeerkatMachine | `AuthorizedRuntimeLoopBatch` command plan | `EphemeralRuntimeDriver::dequeue_batch_exact` | exact ordered input ids and source lane | No external effect; physical projection is non-authoritative | Exact prefix/source match plus queue/steer projection conformance; planted raw dequeue bypass gate |
| Stage input batch for run | MeerkatMachine | `StageForRun` | `machine_realize_authorized_stage_batch` | input ids, run id, source lane, lane/admission sequence, attempt count | No external effect; staging folds attempt increment into the transition | Raw staging is private to concrete driver mechanics; planted raw stage bypass gate |
| Commit runtime-loop run | MeerkatMachine | `RunCompleted`, `Commit`, `Fail`, `CancelRun`, `RollbackRun`, boundary receipt realization | `commit_runtime_loop_run` | run id, terminal input ids, boundary receipt sequence, current owner, terminal outcome, return projection | `TurnRunCompleted` / `TurnRunFailed` / `TurnRunCancelled` are declared as `AuthorizedRuntimeLoopRunCommit` closure obligations with `Authorized -> Attempted -> Realized -> Failed -> Cancelled -> Abandoned`; the runtime commit token carries and checks the completed-run obligation before realizing success | Shell completion cannot proceed without `AuthorizedRuntimeLoopRunCommit`, generated kernel marker, owner binding, outcome binding, return projection binding, and effect-closure obligation |
| Resolve public runtime completion result | MeerkatMachine | `ResolveRuntimeCompletionResult` | `machine_resolve_runtime_completion_result` and replay helpers | session id, run id, agent runtime id, fence token, generation, runtime epoch, result class | Existing `RuntimeCompletionResultResolved` disposition is exposed as `RuntimeCompletionResultAuthority` with `Authorized -> Attempted -> Realized -> Failed -> Cancelled -> Abandoned`; no new durable effect map was added because the chosen family is local surface-result alignment | Runtime waiter delivery consumes `RuntimeCompletionResultAuthority` through `RuntimeCompletionResultAttempt` / `RuntimeCompletionResultRealized`; raw completion senders remain private |
| Start/complete/fail mob member spawn | MobMachine / MeerkatMachine seam | `StageSpawn`, `CompleteSpawn`, `CancelPendingSpawn` | mob runtime provisioner/handle paths | mob id, member id/ref, agent identity, generation, runtime/session binding | Proposal vocabulary is represented by generated command plans `CanStartSpawn`, `SpawnStarted`, `SpawnEffect`, and `FailSpawn` mapped onto existing `StageSpawn`, `CompleteSpawn`, and `CancelPendingSpawn` transitions; closure lifecycles include `Cancelled` | Runtime spawn insertion/completion/cancellation consumes generated kernel markers for `CanStartSpawn`, `SpawnStarted`, `SpawnEffect`, and `FailSpawn` in addition to the existing pending-owner authority |

## Measured Deltas

Measured with repo-local `rg` / `git grep` against `origin/main` and this
branch after the Phase 2/3/4 follow-up. The raw-call production scan excludes
tests and reports the remaining internal queue mechanic separately from
semantic bypasses.

| metric | origin/main | current branch | note |
| --- | ---: | ---: | --- |
| command plans in catalog metadata | 0 | 10 | MeerkatMachine: `AuthorizedAcceptedInputMaterialization`, `AuthorizeRuntimeLoopBatch`, `AuthorizedStageForRun`, `AuthorizedRuntimeLoopRunCommit`, `AuthorizedRuntimeCompletionResultClosure`; MobMachine: `AuthorizedMobSpawnStart`, `CanStartSpawn`, `SpawnStarted`, `SpawnEffect`, `FailSpawn` |
| generated effect-closure rows in machine contracts | 0 | 17 | includes runtime-completion closure, run-commit effect obligations, and spawn start/started/effect/fail closures |
| generated guard-expansion rows in target contracts | 0 | 168 | command-plan guard expansion is reviewable in `specs/machines/*/contract.md` |
| `StageForRun` generated guard rows | 0 | 5 | all phases show queued/lane/sequence/recovery/run-binding guards |
| raw runtime production bypass hits | 9 | 1 | remaining hit is `InputQueue::enqueue` projection mechanics; raw public dequeue/stage and shell stage-drain validator are absent |
| planted bypass checks | 0 | 15 matching assertions | raw accepted-input enqueue, raw dequeue, and raw stage violations are planted in `runtime-authority-bypass` tests |
| runtime projection conformance search hits | 9 | 26 | queue/steer drift and exact dequeue checks are covered by runtime tests and driver code |
| Phase 4 proposal vocabulary hits | 0 | 54 | `CanStartSpawn`, `SpawnStarted`, `SpawnEffect`, and `FailSpawn` exist in generated contract/kernel output and runtime consumption |

## Phase 5 Decision

Decision: continue the generated command-plan/capability pattern for scoped
semantic executor families, and stop any broader platform rewrite.

Rationale:

- The high-blast pilot made misuse harder on the queue-to-run boundary: raw
  production dequeue/stage APIs collapsed to generated-capability paths and the
  bypass gate catches planted raw enqueue/dequeue/stage violations.
- The counted deltas improved enough to keep the pattern: command plans and
  effect closures are generated/reviewable, `StageForRun` guard expansion is in
  generated contracts, and the runtime now consumes generated markers for
  ingress/staging, run commit, completion resolution, and mob spawn.
- The model should remain a scoped authoring aid, not a new control-language
  platform. Future families should be accepted only when they reduce raw
  semantic calls or duplicated guard review in similarly counted ways.

Finding buckets from this pilot:

| bucket | count | resolution |
| --- | ---: | --- |
| predicate wrong | 1 | `StageForRun` strengthened with queued/lane/sequence/recovery/current-run guards |
| command missing | 3 | generated command plans added for runtime batch/stage, run commit, and mob spawn proposal vocabulary |
| durable witness missing | 1 | runtime-loop batch selection now recomputes from durable machine lane/order/grouping state |
| projection conformance failure | 2 | queue and steer drift fail before exact dequeue/stage |
| capability bypass gate failed | 3 | raw enqueue/dequeue/stage planted violations are caught by `runtime-authority-bypass` |
| ordinary shell mechanic | 1 | physical queue enqueue remains an internal projection mechanic, not semantic authority |

## Phase Status

| phase | status | evidence |
| --- | --- | --- |
| Phase 0 inventory | done | this document records action ownership, required identities, closure requirements, bypass risks, measured deltas, and the Phase 5 decision |
| Phase 1 ingress/staging pilot | done | command plans, generated guard docs, machine-state batch selector, batch/stage capabilities, exact dequeue, projection conformance tests, and AST bypass gate |
| Phase 2 required effect closure | done for chosen family | `RuntimeCompletionResultResolved` is bound to `RuntimeCompletionResultAuthority` with `Authorized`, `Attempted`, `Realized`, `Failed`, `Cancelled`, `Abandoned`; runtime waiter delivery consumes the phase token |
| Phase 3 run commit | done | `AuthorizedRuntimeLoopRunCommit` carries generated marker, run id, terminal input ids, current owner, terminal outcome, return projection, and completed-run effect-closure obligation |
| Phase 4 mob spawn | done | generated command plans expose `CanStartSpawn`, `SpawnStarted`, `SpawnEffect`, and `FailSpawn`; runtime consumes generated markers for start, started insertion, completion, and failure/cancellation |
| Phase 5 generalize-or-stop | done | decision recorded above: continue narrowly for counted semantic executor families; stop broad platform rewrite |
