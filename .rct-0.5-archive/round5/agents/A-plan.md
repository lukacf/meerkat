# Lane A Plan

## Owned Rows

- `DOGMA-08`
- `DOGMA-09`
- `DOGMA-13`

## Owned Milestones

- `A1a` = `DOGMA-08`, `DOGMA-09`
- `A1b` = `DOGMA-13`

## Owned Files

- `meerkat-runtime/src/runtime_loop.rs`
- `meerkat-runtime/src/ops_lifecycle.rs`
- `meerkat-core/src/agent/state.rs`
- `meerkat-core/src/agent/runner.rs`
- likely adjacent contract files if needed:
  - `meerkat-core/src/agent/turn_state.rs`
  - `meerkat-core/src/handles.rs`
  - `meerkat-runtime/src/handles/turn_state.rs`

## Shared Files Touched Under Round Map

- none pre-mapped outside lane-owned files

## Blocked By

- `A1b` blocked by `A1a`

## Unblocks

- `A1b`

## Public / Type / Schema Changes

- no public rename work
- may change internal authority contracts for turn-state snapshots/handles
- no schema or SDK regen expected by default

## Defensive Scan Commitments

- add or extend a scan banning runtime-backed authority on `LocalTurnExecutionState`

## Requested Orchestrator Probes

- `./scripts/repo-cargo check -p meerkat-runtime`
- `./scripts/repo-cargo check -p meerkat-core`

## Requested Build Gate Commands

- Quick probe:
  - `./scripts/repo-cargo check -p meerkat-runtime`
  - `./scripts/repo-cargo check -p meerkat-core`
- Full gate:
  - `./scripts/repo-cargo nextest run -p meerkat-runtime --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-core --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-runtime --test driver_ephemeral --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-runtime --test regression_comms_runtime --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`

## Risks

- `DOGMA-08` and `DOGMA-09` are structurally linked to `DOGMA-13` through `WaitingForOps`
- `DOGMA-13` needs an honest runtime-backed integration test, not just local snapshot coverage
- no compatibility shims are allowed in handoff commits

## Open Decisions

- none

## Handoff Notes

- handoff commit: none
- changed files:
  - `meerkat-core/src/handles.rs`
  - `meerkat-core/src/lib.rs`
  - `meerkat-core/src/agent/turn_state.rs`
  - `meerkat-core/src/agent/state.rs`
  - `meerkat-core/src/agent/runner.rs`
  - `meerkat-runtime/src/handles/turn_state.rs`
  - `meerkat-runtime/src/runtime_loop.rs`
  - `meerkat-runtime/src/ops_lifecycle.rs`
  - `.rct/round5/agents/A-runtime-authority-scan.sh`
  - `.rct/round5/agents/A-checklist.md`
  - `.rct/round5/agents/A-plan.md`
- generated output request: none
- known risks:
  - `A1a` remains approved and unchanged: peer projection/context-key ownership lives in the shared core seam, `wait_all` authority setup is centralized in `ShellState::begin_wait_all_authority`, and `terminate_owner` target selection is centralized in `ShellState::owner_termination_targets`.
  - `A1b` is now handoff-ready for current runtime-backed turn flows: the runtime snapshot contract carries `active_run_id`, `StartConversationRun` seeds the runtime handle before `PrimitiveApplied`, execution snapshots prefer runtime truth, and runtime-backed cancel / extraction / terminal / `WaitingForOps` reads go through runtime snapshot helpers in `meerkat-core/src/agent/state.rs`.
  - The `A1b` scan now covers the identified runtime-backed `LocalTurnExecutionState` authority sites, including the former pre-start `can_accept` / `primitive_kind` / local-active-run fallbacks.
  - Follow-up risk, not a current blocker: `apply_turn_input_via_runtime_handle()` still treats runtime-backed turns as conversation-run flows. If a future runtime-backed immediate-append/immediate-context path is introduced, that branch will need explicit handling instead of the current no-op `StartImmediate*` path plus bare `primitive_applied()`.
  - No `cargo` commands were run per instruction. Orchestrator should run the requested probes/tests before merging.
  - Current worktree branch reported by `git status` is `codex/dogma-D2-auth-policy`, even though Lane A work started on `codex/dogma-A-runtime-authority`. I did not force another checkout because the worktree is shared and dirty; orchestrator should reconcile/cherry-pick the lane diff explicitly.
