# Mob-Meerkat Seam Hook Inventory

Status: active pre-cutover hook inventory

This note identifies the concrete live hook points that can drive the first
Mob↔Meerkat seam shadow checks.

It is the implementation companion to:

- `mob-meerkat-seam-shadow-checks.md`
- `mob-meerkat-composition-refinement-delta.md`

## Primary hook surfaces

### 1. Mob-side member/runtime projection

Primary hooks:

- `capture_mob_machine_snapshot(...)`
- runtime/member binding visible through Mob diagnostic snapshots

Best for:

- runtime identity
- fence/supersession posture
- member lifecycle posture

### 2. Meerkat-side joined snapshot

Primary hook:

- `capture_meerkat_machine_snapshot(...)`

Best for:

- corresponding runtime lifecycle/work posture on the Meerkat side

### 3. Session/runtime bridge helpers

Primary hooks:

- `start_turn(...)`
- internal turn / flow-step dispatch helpers
- `MeerkatMachine::interrupt_current_run(...)`
- `MeerkatMachine::update_peer_ingress_context(...)`

Best for:

- checking where seam work/lifecycle lowerings still pass through raw helper
  paths

### 4. Mob provisioning and destroy/retire hooks

Primary hooks:

- `MobHandle::spawn*`
- `MobHandle::run_flow*`
- `MobHandle::retire(...)`
- `MobHandle::destroy()`

Best for:

- lifecycle bridge checks
- destroy/retire bookkeeping

## Check-family to hook mapping

| Seam check family | First hook(s) to use |
| --- | --- |
| B1 lifecycle bridge | `capture_mob_machine_snapshot(...)`, `capture_meerkat_machine_snapshot(...)`, spawn/retire/destroy handle paths |
| B2 work bridge | flow-step/internal-turn/start-turn lowerings plus both joined machine snapshots |
| B3 supersession/fencing | Mob runtime/member projection + Meerkat joined binding/control snapshot |
| B4 destroy/retire bookkeeping | `MobHandle::retire(...)`, `MobHandle::destroy()`, Meerkat lifecycle snapshots |

## Recommended first implementation order

1. lifecycle bridge checks
2. supersession/fencing checks
3. destroy/retire bookkeeping checks
4. work bridge checks last, because they still contain the most helper-rich
   exact-current lowerings

## Read with

- `mob-meerkat-seam-shadow-checks.md`
- `mob-meerkat-composition-refinement-delta.md`
