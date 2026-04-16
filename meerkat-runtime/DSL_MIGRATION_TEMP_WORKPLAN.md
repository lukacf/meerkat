# Meerkat Runtime DSL Migration Workplan (Temporary)

Status capture date: 2026-04-16 (UTC+2)

## Current objective
The runtime should enforce state transitions only through the DSL-generated authorities/machines, with runtime shell code only projecting effects, not owning transition logic.

## What is currently complete
- Coarse Meerkat lifecycle phase transitions are already shadowed and validated via
  `meerkat_machine::dsl` / `MeerkatMachineMutator`:
  - control/session/ingress/drain command paths now call `stage_session_dsl_input(...)`
  - phase write-back is done with `sync_session_dsl_projection(...)` where appropriate
  - `dsl_authority.rs` has terminal-state guards so run return cannot clobber `Retired`/`Stopped`/`Destroyed`

## Remaining authoritative legacy domains (must be replaced)

### 1) Input lifecycle (`InputLifecycleAuthority`)
- Canonical transition owner is still `input_lifecycle_authority.rs` and `input_state.rs`:
  - `InputState::apply` / `set_terminal_outcome`
  - `driver/ephemeral.rs` (`accept`, `stage`, `consume`, `rollback`, `fail`, `abandon`)
  - `driver/persistent.rs` and `meerkat_machine/driver.rs` recovery normalization paths
- Effect: per-input state machine still bypasses generated DSL for terminal transitions.

### 2) Runtime ingress (`RuntimeIngressAuthority`)
- Canonical queue/projection owner remains in `runtime_ingress_authority.rs`:
  - `driver/ephemeral.rs` ingress field and methods (`accept`, queue, dequeue, clear queue, rebuild)
  - `driver/persistent.rs` integration path
  - `runtime_loop.rs` batch selection and admission boundary behavior
  - `meerkat_machine/driver.rs` rebuild/recovery (`machine_build_recovered_ingress_entry`, `machine_recover_ephemeral_driver`, `machine_recover_persistent_driver`)
- Effect: queue and run-boundary admission semantics still authored outside generated DSL.

### 3) Ops lifecycle (`OpsLifecycleAuthority`)
- Canonical owner remains:
  - `ops_lifecycle_authority.rs`
  - `ops_lifecycle.rs` (registry, I/O, completion retention)
- Effect: operation registration/provision/finalization not yet DSL-authoritative.

### 4) Comms drain / visibility (`CommsDrain` state)
- `CommsDrainLifecycleAuthority` no longer appears as an explicit authority, but drain state is still hand-mutated through `CommsDrainSlot` in:
  - `comms_drain.rs`
  - `visibility.rs`
  - `meerkat_machine/dispatch_drain.rs` shadow/safety paths
- Effect: not covered by generated DSL transition machine yet; drain state remains manually maintained.

## Concrete next migration slice (recommended)
1. Start with **input lifecycle** because it has the least blast radius compared to ingress loop semantics.
2. Introduce an `input_lifecycle` DSL schema and generated `InputLifecycleMachine`.
3. Replace direct calls that mutate `InputState::authority` with `MeerkatMachineInput`-style conversions in:
   - `driver/ephemeral.rs` and `driver/persistent.rs`
   - recovery normalization helpers in `meerkat_machine/driver.rs`
4. Keep command handlers as-is, but require projection updates through DSL results only.
5. Add focused regression coverage for accepted/recovered/rollback/terminal outcomes.

## Additional note for the shell tool issue (s16)
- The environment issue around `"no such file or directory"` still appears to be non-DLS: shell spawn failures (ENOENT) from the tool execution path.
- This remains a separate environment/path problem and should be tracked outside the DSL migration workstream.

## Safety constraints while migrating
- Keep `RuntimeLoop` behavior deterministic while swapping to DSL:
  - wake semantics
  - queue ordering
  - recovery replay
  - batch composition (`RunBoundary`)
- Any half-migrated path should remain behaviorally identical; prefer staged phase-by-phase migrations with dual-write/read checks first.

## Temporary checkpoints expected
- Record per-domain migration after each step (schema, shell writes, recovery, and tests) before moving on.
- Update this file after each pass to preserve exact callsite inventory and rollback path.

