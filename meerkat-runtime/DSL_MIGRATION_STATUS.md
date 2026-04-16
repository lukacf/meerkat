# MeerkatMachine DSL Migration Status (temporary checkpoint)

Date: 2026-04-16

Temporary execution plan doc added: `DSL_MIGRATION_TEMP_WORKPLAN.md`.

## What is already migrated

- `MeerkatMachine` command entrypoints now use DSL shadow/application for coarse lifecycle transitions in:
  - `execute_meerkat_machine_control_command`:
    - `Retire`, `Reset`, `Recover`, `Destroy`, `Recycle`, `StopRuntimeExecutor`
  - `execute_meerkat_machine_session_command`:
    - `RegisterSession`, `UnregisterSession`, `PrepareBindings`, `ReconfigureSessionLlmIdentity`
  - `dispatch_drain`:
    - `SetPeerIngressContext`, `NotifyDrainExited`
  - `dispatch_ingress`:
    - `AcceptWithCompletion`, `AcceptWithoutWake`, `Prepare`
- A DSL authority shim exists in `meerkat-runtime/src/meerkat_machine/dsl_authority.rs`
  with:
  - `MeerkatPhaseResult`, `dsl_phase_to_result`, `project_phase`
  - terminal-projection guards for `Retired|Stopped|Destroyed`.
- `AcceptWithoutWake`, `AcceptWithCompletion`, and `Prepare` now emit DSL shadows:
  - `AcceptWithCompletion` records `MeerkatMachineInput::AcceptWithCompletion` for all admission paths.
  - `AcceptWithoutWake` records `MeerkatMachineInput::AcceptWithoutWake`.
  - `Prepare` records `MeerkatMachineInput::Prepare`.

## Remaining non-DSL authoritative domains (still “raw” state owners)

These are the key mismatches remaining versus the architecture goal:

1. **Input lifecycle**
   - `InputState` owns canonical lifecycle in `InputLifecycleAuthority`:
     - `meerkat-runtime/src/input_state.rs`
     - `apply`, `can_accept`, `set_terminal_outcome`
   - Directly used by drivers and runtime shell:
     - `meerkat-runtime/src/driver/ephemeral.rs`
     - `meerkat-runtime/src/driver/persistent.rs`
     - `meerkat-runtime/src/meerkat_machine/driver.rs` (functions like `machine_validate_*`, `machine_apply_recovered_input_normalization`, recovery helpers)
   - This means input terminal/non-terminal state and stage/consumption sequencing are still not enforced by a unified DSL machine.

2. **Runtime ingress**
   - Canonical queue/handling/lifecycle seam remains `RuntimeIngressAuthority`:
     - `meerkat-runtime/src/runtime_ingress_authority.rs`
     - `meerkat-runtime/src/driver/ephemeral.rs` (`ingress` field, `accept_input`, queue projections, recovery)
  - `meerkat-runtime/src/meerkat_machine/driver.rs` (`machine_input_boundary`, `machine_select_runtime_loop_batch`, recovery rebuild paths)
  - `meerkat-runtime/src/runtime_loop.rs` (batch routing depends on ingress state)
  - RuntimeLoop batching and steering currently depend on this authority shape and therefore are not DSL-owned yet.
  - `AcceptWithCompletion` is now shadowed for attached+immediate as a best-effort path via generated run_id placeholders.

3. **Comms drain**
   - `CommsDrainLifecycleAuthority` has been removed from comms-drain runtime control:
     - `meerkat-runtime/src/meerkat_machine/comms_drain.rs` now transitions `CommsDrainSlot` directly via
       `phase`/`mode` updates and explicit `handle` lifecycle.
  - `meerkat-runtime/src/meerkat_machine/dispatch_drain.rs` safety-net now applies slot transitions directly.
  - `meerkat-runtime/src/meerkat_machine/visibility.rs` snapshot reads `slot.phase` and `slot.mode`.
  - `meerkat-runtime/src/meerkat_machine_tests.rs` helper now seeds/reads local `CommsDrainSlot` state directly.
  - Note: Comms drain is now maintained in-kernel and does not currently flow through generated DSL transitions.

4. **Ops lifecycle**
   - The async operation lifecycle is still shell-authored:
     - `meerkat-runtime/src/ops_lifecycle_authority.rs`
     - `meerkat-runtime/src/ops_lifecycle.rs`
     - session command surface still exposes `MeerkatMachineCommand::OpsLifecycleRegistry` only as a registry query in `dispatch_session.rs`, not as DSL-driven transitions.

## Immediate risks to close next

- `MeerkatMachine` still allows mutation through these old authorities before/while DSL projection updates. This violates the “single mutation surface” target.
- Any migration must preserve RuntimeLoop behavior (especially ingest ordering / steering / wake semantics) while replacing these owners.
- A partial migration will increase the chance of inconsistent phase bookkeeping unless runtime projection snapshots are updated together with write-path transitions.

## Outstanding legacy authority dependencies (exact callsites)

- **Runtime ingress authority** (`RuntimeIngressAuthority`) is still authoritative for:
  - input acceptance metadata, queue state, and boundary classification used by the runtime loop.
  - `meerkat-runtime/src/runtime_ingress_authority.rs`
  - `meerkat-runtime/src/driver/ephemeral.rs` (`RuntimeDriver` methods with `ingress` field)
  - `meerkat-runtime/src/driver/persistent.rs` (same `ingress` integration path)
  - `meerkat-runtime/src/meerkat_machine/driver.rs` (`machine_input_boundary`, `machine_select_runtime_loop_batch`, recovery normalization)
  - `meerkat-runtime/src/runtime_loop.rs` (steady-state batch scheduling).
- **Input lifecycle authority** (`InputLifecycleAuthority`) is still authoritative for:
  - stage/consume/terminal transitions, output outcomes, replay safety.
  - `meerkat-runtime/src/input_lifecycle_authority.rs`
  - `meerkat-runtime/src/input_state.rs`
  - `meerkat-runtime/src/input_ledger.rs` integration points
  - `meerkat-runtime/src/driver/ephemeral.rs`
  - `meerkat-runtime/src/driver/persistent.rs`
  - `meerkat-runtime/src/meerkat_machine/driver.rs` (apply/rollback helpers that consume `InputLifecycleError`).
- **Ops lifecycle authority** (`OpsLifecycleAuthority`) is still authoritative for:
  - per-operation registration/progression/success/failure and bounded completion lists.
  - `meerkat-runtime/src/ops_lifecycle_authority.rs`
  - `meerkat-runtime/src/ops_lifecycle.rs`
  - `meerkat-runtime/src/meerkat_machine/session_management.rs` (operation command surface remains registry-oriented only)

## Suggested next execution plan

1. **Stabilize DSL transition coverage**
   - Enumerate all command/event transitions currently performed through legacy authorities and encode equivalent `*_Input`/`*_Effect` in schema-backed machines.
2. **Port one domain at a time**
   - `comms_drain` is now using direct in-kernel drain state and no longer uses generated `comms_drain_spawn/abort` protocols.
   - Next migrate `runtime ingress` (higher risk; touches `runtime_loop.rs` batching and recovery).
   - Then migrate `input lifecycle` and `ops lifecycle`.
3. **After each domain swap**
   - Update `meerkat_machine_tests` snapshots that observe those fields.
   - Add targeted regression checks for phase restoration during run completion and command-level rollback.

## Notes

- `meerkat-machine` migration is intentionally incremental; this document is a checkpoint.
- The branch already has terminal-run return guard in `machine_apply_run_return_projection`.
- Query-only command list continues to intentionally remain direct queries (e.g. `InputState`, `ListActiveInputs`, etc.) until the corresponding write machines are introduced.
