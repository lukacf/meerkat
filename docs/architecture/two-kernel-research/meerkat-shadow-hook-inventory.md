# Meerkat Shadow Hook Inventory

Status: active pre-cutover hook inventory

This note identifies the concrete live hook points that can drive the first
`MeerkatMachine` shadow-validation implementation.

It is the implementation companion to:

- `meerkat-shadow-validation-plan.md`
- `meerkat-cutover-lowering-inventory.md`

## Primary hook surfaces

### 1. Session adapter spine snapshot

Primary hook:

- `MeerkatMachine::meerkat_machine_spine_snapshot(...)`
  in `meerkat-runtime/src/meerkat_machine.rs`

Best for:

- lifecycle/control lane
- queue / steer visibility
- completion-waiter carrier
- drain state

### 2. Joined Meerkat snapshot

Primary hook:

- `capture_meerkat_machine_snapshot(...)` in
  `meerkat/src/meerkat_machine.rs`

Best for:

- end-to-end joined validation
- cross-region comparison after control, peer, tools, or drain mutations

### 3. Turn execution snapshot

Primary hooks:

- `Runner::execution_snapshot()` in `meerkat-core/src/agent/runner.rs`
- session-layer forwarding in `meerkat-session/src/ephemeral.rs` and
  `meerkat-session/src/persistent.rs`

Best for:

- turn / ops / barrier lane
- pending-op refs
- active run posture

### 4. Peer ingress runtime snapshot

Primary hook:

- `CommsRuntime::peer_ingress_runtime_snapshot()` with forwarding into the core
  and Meerkat join

Best for:

- peer trust/auth mode
- classified backlog
- admitted / terminalized peer lineage

### 5. Tool visibility and tool surface snapshots

Primary hooks:

- `Runner::tool_scope_snapshot()` in `meerkat-core/src/agent/runner.rs`
- `EphemeralSessionService::set_session_tool_visibility_state(...)`
- `Runner::external_tool_surface_snapshot()` in
  `meerkat-core/src/agent/runner.rs`
- session/factory forwarding for tool-surface snapshots

Best for:

- tools lane
- visibility-state mutation checks
- exact-catalog projection checks
- surface staging / apply / finalize checks

### 6. Drain hooks

Primary hooks:

- `MeerkatMachine::maybe_spawn_comms_drain(...)`
- `MeerkatMachine::notify_comms_drain_exited(...)`
- `MeerkatMachine::abort_comms_drain(...)`
- joined Meerkat drain snapshot in `meerkat/src/meerkat_machine.rs`

Best for:

- drain lane

## Lane-to-hook mapping

| Shadow lane | First hook(s) to use |
| --- | --- |
| S1 lifecycle/control | `meerkat_machine_spine_snapshot(...)`, joined `capture_meerkat_machine_snapshot(...)` |
| S2 turn/ops/barrier | `execution_snapshot()`, joined `capture_meerkat_machine_snapshot(...)` |
| S3 peer ingress | `peer_ingress_runtime_snapshot()`, joined `capture_meerkat_machine_snapshot(...)` |
| S4 tools | `tool_scope_snapshot()`, `external_tool_surface_snapshot()`, `set_session_tool_visibility_state(...)`, joined `capture_meerkat_machine_snapshot(...)` |
| S5 drain | `maybe_spawn_comms_drain(...)`, `notify_comms_drain_exited(...)`, `abort_comms_drain(...)`, joined `capture_meerkat_machine_snapshot(...)` |

## Recommended first implementation order

1. lifecycle/control using `meerkat_machine_spine_snapshot(...)`
2. tools lane using `tool_scope_snapshot()` + `external_tool_surface_snapshot()`
3. peer ingress using `peer_ingress_runtime_snapshot()`
4. turn/ops/barrier using `execution_snapshot()`
5. drain lane last, because it already composes several of the others

## Read with

- `meerkat-shadow-validation-plan.md`
- `meerkat-cutover-lowering-inventory.md`
