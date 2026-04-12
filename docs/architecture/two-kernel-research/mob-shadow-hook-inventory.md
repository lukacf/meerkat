# Mob Shadow Hook Inventory

Status: active pre-cutover hook inventory

This note identifies the concrete live hook points that can drive the first
`MobMachine` shadow-validation implementation.

It is the implementation companion to:

- `mob-shadow-validation-plan.md`
- `mob-cutover-lowering-inventory.md`

## Primary hook surfaces

### 1. Joined Mob snapshot

Primary hook:

- `capture_mob_machine_snapshot(...)` in `meerkat-mob/src/mob_machine.rs`

Best for:

- end-to-end Mob validation
- provisioning/lifecycle
- topology
- tracked run structure
- restore failures

### 2. Actor diagnostic kernel snapshot

Primary hooks:

- actor diagnostic snapshot paths in `meerkat-mob/src/runtime/actor.rs`
- handle/diagnostic forwarding in `meerkat-mob/src/runtime/handle.rs`

Best for:

- lifecycle/orchestrator/topology state
- pending spawn lineage
- kickoff state
- run tracker/task tracker alignment

### 3. Restore-failure snapshot

Primary hook:

- `diagnostic_restore_failures_snapshot()` on the runtime handle path

Best for:

- provisioning/lifecycle lane
- task/history/recovery lane

### 4. Kickoff / pending-spawn state

Primary hooks:

- actor-side pending-spawn snapshotting
- kickoff barrier snapshot paths in `meerkat-mob/src/runtime/actor.rs`

Best for:

- provisioning/lifecycle lane

### 5. Flow/run durable snapshots

Primary hooks:

- tracked `MobRun` snapshot path used by `capture_mob_machine_snapshot(...)`
- flow-run kernel / actor cleanup paths

Best for:

- work ledger lane
- flow / frame / loop lane

### 6. Task/history projections

Primary hooks:

- task-board service projections
- history/event emission alignment exposed through diagnostic projections

Best for:

- task/history/recovery lane

## Lane-to-hook mapping

| Shadow lane | First hook(s) to use |
| --- | --- |
| S1 provisioning/lifecycle | `capture_mob_machine_snapshot(...)`, pending-spawn snapshot, kickoff snapshot, restore-failure snapshot |
| S2 work ledger | `capture_mob_machine_snapshot(...)`, tracked run durable snapshot |
| S3 flow/frame/loop | `capture_mob_machine_snapshot(...)`, flow-run kernel / tracked run snapshot |
| S4 topology/coordinator | `capture_mob_machine_snapshot(...)`, actor topology/orchestrator snapshot |
| S5 task/history/recovery | `capture_mob_machine_snapshot(...)`, restore-failure snapshot, task/history projections |

## Recommended first implementation order

1. provisioning/lifecycle
2. flow/frame/loop
3. topology/coordinator
4. task/history/recovery
5. work ledger once the bridge-side checks are live

## Read with

- `mob-shadow-validation-plan.md`
- `mob-cutover-lowering-inventory.md`
