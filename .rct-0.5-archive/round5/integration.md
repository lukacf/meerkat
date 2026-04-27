# Round 5 Integration Log

All compile probes, regen steps, quick probes, and full Build Gate results are recorded here.

## Logging Template

### Timestamp
- lane:
- milestone:
- branch:
- handoff commit:
- action:
- commands:
  - `...`
- result:
- excerpt:
- follow-up:

## Pending

- none

## Execution Start

### 2026-04-21 Round 5 kickoff
- lane: orchestrator
- milestone: all lanes
- branch: `codex/fix-plenty-of-dogma-violations`
- handoff commit: none
- action: spawned lane workers and transitioned milestones to implementation
- commands:
  - `spawn lane A worker -> codex/dogma-A-runtime-authority`
  - `spawn lane B worker -> codex/dogma-B-public-surfaces`
  - `spawn lane C worker -> codex/dogma-C-comms-trust`
  - `spawn lane D1 worker -> codex/dogma-D1-auth-identity`
  - `spawn lane D2 worker -> codex/dogma-D2-auth-policy`
  - `spawn lane EG worker -> codex/dogma-EG-session-schedule`
- result: in progress
- excerpt: all six lanes are implementing under the no-cargo rule; first static reviews pending
- follow-up: wait for lane handoffs, then run static gate review milestone-by-milestone

### 2026-04-21 D2-1 static gate
- lane: `D2`
- milestone: `D2-1`
- branch: `codex/dogma-D2-auth-policy`
- handoff commit: working tree handoff
- action: static gate review
- commands:
  - `git diff -- meerkat-contracts/src/capability/registry.rs meerkat-contracts/src/capability/mod.rs meerkat/src/surface.rs meerkat/src/factory.rs .rct/round5/agents/D2-plan.md .rct/round5/agents/D2-checklist.md`
  - `rg -n "build_capabilities\\(|available_capabilities\\(|resolve_capabilities\\(" meerkat/src/factory.rs meerkat/src/surface.rs meerkat-contracts/src/capability -g '!target'`
- result: pass
- excerpt: `DOGMA-25` scope is isolated to config-aware capability truth and factory seeding; `D2-2` remains honestly blocked in checklist and handoff notes.
- follow-up: run D2-1 quick probe

### 2026-04-21 D2-1 quick probe
- lane: `D2`
- milestone: `D2-1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe on isolated integration worktree
- commands:
  - `git worktree add /Users/luka/.codex/worktrees/ec4d/meerkat-round5-int codex/dogma-round5-integration`
  - `git diff --binary -- meerkat-contracts/src/capability/registry.rs meerkat-contracts/src/capability/mod.rs meerkat/src/surface.rs meerkat/src/factory.rs .rct/round5/agents/D2-plan.md .rct/round5/agents/D2-checklist.md > /tmp/round5-D2-1.patch`
  - `git apply /tmp/round5-D2-1.patch`
  - `./scripts/repo-cargo check -p meerkat`
  - `./scripts/repo-cargo check -p meerkat-skills`
  - `./scripts/repo-cargo check -p meerkat-contracts`
- result: fail
- excerpt: `meerkat/src/surface.rs` calls `meerkat_contracts::resolve_capabilities(config)`, but `resolve_capabilities` is not exported from the crate root.
- follow-up: return compile error to `D2` and rerun quick probe after fix

### 2026-04-21 D2-1 quick probe rerun
- lane: `D2`
- milestone: `D2-1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe rerun after compile fix
- commands:
  - `git reset --hard HEAD`
  - `git diff --binary -- meerkat-contracts/src/lib.rs meerkat-contracts/src/capability/registry.rs meerkat-contracts/src/capability/mod.rs meerkat/src/surface.rs meerkat/src/factory.rs .rct/round5/agents/D2-plan.md .rct/round5/agents/D2-checklist.md > /tmp/round5-D2-1.patch`
  - `git apply /tmp/round5-D2-1.patch`
  - `./scripts/repo-cargo check -p meerkat`
  - `./scripts/repo-cargo check -p meerkat-skills`
  - `./scripts/repo-cargo check -p meerkat-contracts`
- result: pass
- excerpt: all requested `D2-1` quick-probe checks completed successfully after exporting `resolve_capabilities` from `meerkat-contracts` crate root.
- follow-up: run D2-1 full gate

### 2026-04-21 D2-1 full gate
- lane: `D2`
- milestone: `D2-1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate on isolated integration worktree
- commands:
  - `./scripts/repo-cargo nextest run -p meerkat-skills --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- result: pass
- excerpt: `meerkat-skills` lib tests passed, `meerkat` lib tests passed, and `e2e-fast` passed on the isolated integration tree.
- follow-up: mark `D2-1` build-approved and queue `D1-1` quick probe

### 2026-04-21 C1 static gate
- lane: `C`
- milestone: `C1`
- branch: `codex/dogma-C-comms-trust`
- handoff commit: working tree handoff
- action: static gate review
- commands:
  - `git diff -- meerkat-comms/src/io_task.rs meerkat-comms/src/inbox.rs meerkat-runtime/src/comms_drain.rs .rct/round5/agents/C-plan.md .rct/round5/agents/C-checklist.md`
- result: pass
- excerpt: ingress trust ownership, ack timing, and deferred trust publication all land inside the intended seam and the checklist is fully honest for `C1`.
- follow-up: run C1 quick probe

### 2026-04-21 EG1a static gate
- lane: `EG`
- milestone: `EG1a`
- branch: `codex/dogma-EG-session-schedule`
- handoff commit: working tree handoff
- action: static gate review
- commands:
  - `git diff -- meerkat-rpc/src/realtime_ws.rs .rct/round5/agents/EG-plan.md .rct/round5/agents/EG-checklist.md`
- result: pass
- excerpt: `EG1a` is an honest first-stop handoff for `DOGMA-20`; observer rebinding and product-session replacement are in the owned websocket seam and later EG milestones remain unchecked.
- follow-up: run EG1a quick probe

### 2026-04-21 C1 quick probe
- lane: `C`
- milestone: `C1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe on cumulative integration worktree
- commands:
  - `git reset --hard HEAD`
  - `git diff --binary -- meerkat-contracts/src/lib.rs meerkat-contracts/src/capability/registry.rs meerkat-contracts/src/capability/mod.rs meerkat/src/surface.rs meerkat/src/factory.rs > /tmp/round5-D2-1-code.patch`
  - `git diff --binary -- meerkat-comms/src/io_task.rs meerkat-comms/src/inbox.rs meerkat-runtime/src/comms_drain.rs > /tmp/round5-C1-code.patch`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-C1-code.patch`
  - `./scripts/repo-cargo check -p meerkat-comms`
  - `./scripts/repo-cargo check -p meerkat-runtime`
- result: fail
- excerpt: `meerkat-runtime/src/comms_drain.rs` references `SupervisorBindingStageError` in the new rollback helpers without importing it into scope.
- follow-up: return compile failure to `C` and rerun quick probe after fix

### 2026-04-21 C1 quick probe rerun
- lane: `C`
- milestone: `C1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe rerun after import fix
- commands:
  - `git reset --hard HEAD`
  - `git diff --binary -- meerkat-comms/src/io_task.rs meerkat-comms/src/inbox.rs meerkat-runtime/src/comms_drain.rs > /tmp/round5-C1-code.patch`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-C1-code.patch`
  - `./scripts/repo-cargo check -p meerkat-comms`
  - `./scripts/repo-cargo check -p meerkat-runtime`
- result: pass
- excerpt: both requested `C1` quick-probe checks completed successfully after moving `SupervisorBindingStageError` into non-test scope.
- follow-up: run C1 full gate

### 2026-04-21 C1 full gate
- lane: `C`
- milestone: `C1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate on cumulative integration worktree
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-C1-code.patch`
  - `./scripts/repo-cargo nextest run -p meerkat-comms --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-runtime --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- result: fail
- excerpt: `meerkat-comms/src/io_task.rs` test module now references `InboxItem` in several assertions without importing it into scope.
- follow-up: return compile failure to `C` and rerun full gate after fix

### 2026-04-21 C1 full gate rerun
- lane: `C`
- milestone: `C1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate rerun after restoring `InboxItem` test import
- commands:
  - `git diff --binary -- meerkat-comms/src/io_task.rs meerkat-comms/src/inbox.rs meerkat-runtime/src/comms_drain.rs > /tmp/round5-C1-code.patch`
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-C1-code.patch`
  - `./scripts/repo-cargo nextest run -p meerkat-comms --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-runtime --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- result: fail
- excerpt: compile passed, but `io_task::tests::test_drop_untrusted_sender` and `io_task::tests::test_io_task_checks_trust` now fail because the new single-owner ingress trust behavior changed their old expectations.
- follow-up: return behavior-level test failures to `C` and rerun full gate after expectations are updated

### 2026-04-21 C1 full gate rerun #2
- lane: `C`
- milestone: `C1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate rerun after behavior-test expectation fix
- commands:
  - `git diff --binary -- meerkat-comms/src/io_task.rs meerkat-comms/src/inbox.rs meerkat-runtime/src/comms_drain.rs > /tmp/round5-C1-code.patch`
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-C1-code.patch`
  - `./scripts/repo-cargo nextest run -p meerkat-comms --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-runtime --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- result: fail
- excerpt: `meerkat-comms` lib tests are now green, but a new `meerkat-runtime/src/comms_drain.rs` rollback test moves `payload` while borrowing `payload.supervisor.peer_id`, triggering `error[E0505]`.
- follow-up: return the rollback-test borrow error to `C` and rerun full gate again after fix

### 2026-04-21 C1 full gate rerun #3
- lane: `C`
- milestone: `C1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate rerun after rollback-test borrow fix
- commands:
  - `git diff --binary -- meerkat-comms/src/io_task.rs meerkat-comms/src/inbox.rs meerkat-runtime/src/comms_drain.rs > /tmp/round5-C1-code.patch`
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-C1-code.patch`
  - `./scripts/repo-cargo nextest run -p meerkat-comms --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-runtime --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- result: pass
- excerpt: `meerkat-comms`, `meerkat-runtime`, and the final `e2e-fast` lane all passed on the cumulative integration tree.
- follow-up: mark `C1` build-approved

### 2026-04-21 EG1a quick probe
- lane: `EG`
- milestone: `EG1a`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe on cumulative integration worktree
- commands:
  - `git reset --hard HEAD`
  - `git diff --binary -- meerkat-rpc/src/realtime_ws.rs > /tmp/round5-EG1a-code.patch`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-EG1a-code.patch`
  - `./scripts/repo-cargo check -p meerkat-rpc`
  - `./scripts/repo-cargo check -p meerkat-mob`
- result: fail
- excerpt: `meerkat-rpc/src/realtime_ws.rs` still drops `_bridge_observer` and `_wake_observer` after the refactor renamed the guards to `_bridge_observer_guard` and `_wake_observer_guard`.
- follow-up: return compile failure to `EG` and rerun quick probe after fix

### 2026-04-21 EG1a quick probe rerun
- lane: `EG`
- milestone: `EG1a`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe rerun after observer-guard fix
- commands:
  - `git reset --hard HEAD`
  - `git diff --binary -- meerkat-rpc/src/realtime_ws.rs > /tmp/round5-EG1a-code.patch`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-EG1a-code.patch`
  - `./scripts/repo-cargo check -p meerkat-rpc`
  - `./scripts/repo-cargo check -p meerkat-mob`
- result: pass
- excerpt: both requested `EG1a` quick-probe checks completed successfully after updating the stale observer-drop names.
- follow-up: run EG1a full gate

### 2026-04-21 EG1a full gate
- lane: `EG`
- milestone: `EG1a`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate on cumulative integration worktree
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-EG1a-code.patch`
  - `./scripts/repo-cargo nextest run -p meerkat-rpc --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rpc --test realtime_ws_protocol --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-mob --test member_realtime_bindings --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- result: pass
- excerpt: all focused EG1a test legs passed and the final `e2e-fast` lane passed on the isolated integration tree.
- follow-up: mark `EG1a` build-approved and unblock `B2`

### 2026-04-21 A1a quick probe
- lane: `A`
- milestone: `A1a`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe on cumulative integration worktree
- commands:
  - `git reset --hard HEAD`
  - `git diff --binary -- meerkat-core/src/handles.rs meerkat-core/src/lib.rs meerkat-runtime/src/runtime_loop.rs meerkat-runtime/src/ops_lifecycle.rs > /tmp/round5-A1a-code.patch`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-A1a-code.patch`
  - `./scripts/repo-cargo check -p meerkat-runtime`
  - `./scripts/repo-cargo check -p meerkat-core`
- result: fail
- excerpt: `meerkat-runtime/src/handles/turn_state.rs` still has a `TurnStateSnapshot` initializer missing `active_run_id`, and `meerkat-runtime/src/ops_lifecycle.rs` needs an explicit type for `dsl_ids`.
- follow-up: return compile failure to `A` and rerun quick probe after fix

### 2026-04-21 A1a quick probe rerun
- lane: `A`
- milestone: `A1a`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe rerun from refreshed A1a patch
- commands:
  - `git diff --binary -- meerkat-core/src/handles.rs meerkat-core/src/lib.rs meerkat-runtime/src/runtime_loop.rs meerkat-runtime/src/ops_lifecycle.rs meerkat-runtime/src/handles/turn_state.rs .rct/round5/agents/A-plan.md .rct/round5/agents/A-checklist.md .rct/round5/agents/A-runtime-authority-scan.sh > /tmp/round5-A1a-code.patch`
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-A1a-code.patch`
  - `./scripts/repo-cargo check -p meerkat-runtime`
  - `./scripts/repo-cargo check -p meerkat-core`
- result: pass
- excerpt: both requested `A1a` quick-probe checks completed successfully after replaying the refreshed A1a-only patch.
- follow-up: run A1a full gate

### 2026-04-21 A1a full gate
- lane: `A`
- milestone: `A1a`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate on cumulative integration worktree
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-A1a-code.patch`
  - `./scripts/repo-cargo nextest run -p meerkat-runtime --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-core --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-runtime --test driver_ephemeral --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-runtime --test regression_comms_runtime --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- result: pass
- excerpt: all focused A1a test legs passed and the final `e2e-fast` lane passed on the isolated integration tree.
- follow-up: mark A1a build-approved and continue A1b only

### 2026-04-21 D2-2 static gate
- lane: `D2`
- milestone: `D2-2`
- branch: `codex/dogma-D2-auth-policy`
- handoff commit: working tree handoff
- action: static gate review
- commands:
  - `git diff -- meerkat-auth-core/src/resolver.rs meerkat-openai/src/runtime/mod.rs meerkat-anthropic/src/runtime/mod.rs meerkat-gemini/src/runtime/mod.rs .rct/round5/agents/D2-plan.md .rct/round5/agents/D2-checklist.md`
  - `rg -n "DynamicLease|resolve_external_authorizer|constraints|metadata_defaults|connection_ref" meerkat-auth-core/src/resolver.rs meerkat-openai/src/runtime/mod.rs meerkat-anthropic/src/runtime/mod.rs meerkat-gemini/src/runtime/mod.rs -g '!target'`
- result: pass
- excerpt: `D2-2` now consumes the green D1 `connection_ref` contract directly, materializes real external-authorizer lease paths in owned runtimes, and centrally enforces metadata defaults and policy constraints.
- follow-up: run D2-2 quick probe

### 2026-04-21 D2-2 quick probe
- lane: `D2`
- milestone: `D2-2`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe on cumulative integration worktree
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - copy owned `D2-2` files from the main workspace into the integration worktree:
    - `meerkat-auth-core/src/resolver.rs`
    - `meerkat-openai/src/runtime/mod.rs`
    - `meerkat-anthropic/src/runtime/mod.rs`
    - `meerkat-gemini/src/runtime/mod.rs`
  - `./scripts/repo-cargo check -p meerkat-openai`
  - `./scripts/repo-cargo check -p meerkat-anthropic`
  - `./scripts/repo-cargo check -p meerkat-gemini`
  - `./scripts/repo-cargo check -p meerkat`
  - `./scripts/repo-cargo check -p meerkat-skills`
- result: pass
- excerpt: all requested `D2-2` quick-probe checks completed successfully; remaining findings are warning-level cleanup only.
- follow-up: run D2-2 full gate

### 2026-04-21 D2-2 full gate
- lane: `D2`
- milestone: `D2-2`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate on cumulative integration worktree
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - sync owned `D2-2` files from the main workspace into the integration worktree:
    - `meerkat-auth-core/src/resolver.rs`
    - `meerkat-openai/src/runtime/mod.rs`
    - `meerkat-anthropic/src/runtime/mod.rs`
    - `meerkat-gemini/src/runtime/mod.rs`
  - `./scripts/repo-cargo nextest run -p meerkat-openai --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-anthropic --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-gemini --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-skills --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- result: pass
- excerpt: all focused `D2-2` test legs passed and the final `e2e-fast` lane passed on the isolated integration tree; remaining signal is warning-only cleanup in provider runtime modules.
- follow-up: mark `D2-2` build-approved

### 2026-04-21 D2-2 full gate confirmation
- lane: `D2`
- milestone: `D2-2`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: reran tail of full gate to capture final result explicitly
- commands:
  - `./scripts/repo-cargo nextest run -p meerkat --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- result: pass
- excerpt: `meerkat --lib` and `e2e-fast` both passed; provider runtime changes still emit warnings only.
- follow-up: keep warnings as non-blocking cleanup unless final gate policy changes

### 2026-04-21 A1a static gate
- lane: `A`
- milestone: `A1a`
- branch: `codex/dogma-A-runtime-authority`
- handoff commit: working tree handoff
- action: static gate review
- commands:
  - `git diff -- meerkat-core/src/handles.rs meerkat-core/src/lib.rs meerkat-runtime/src/runtime_loop.rs meerkat-runtime/src/ops_lifecycle.rs .rct/round5/agents/A-plan.md .rct/round5/agents/A-checklist.md .rct/round5/agents/A-runtime-authority-scan.sh`
- result: pass
- excerpt: `DOGMA-08` and `DOGMA-09` land cleanly while `DOGMA-13` remains honestly partial and not promoted.
- follow-up: run A1a quick probe

### 2026-04-21 D1-1 static gate
- lane: `D1`
- milestone: `D1-1`
- branch: `codex/dogma-D1-auth-identity`
- handoff commit: working tree handoff
- action: static gate review
- commands:
  - `git diff -- meerkat-llm-core/src/provider_runtime/binding.rs meerkat-llm-core/src/provider_runtime/runtime.rs meerkat-llm-core/src/provider_runtime/registry.rs meerkat-openai/src/runtime/mod.rs meerkat-anthropic/src/runtime/mod.rs meerkat-gemini/src/runtime/mod.rs meerkat-rest/src/auth_endpoints.rs meerkat-rpc/src/handlers/auth.rs meerkat-web-runtime/src/external_auth.rs scripts/d1_auth_identity_scan.sh .rct/round5/agents/D1-plan.md .rct/round5/agents/D1-checklist.md`
- result: pass
- excerpt: canonical binding-scoped identity now flows through `ValidatedBinding.connection_ref`, provider validation, auth handlers, and WASM external auth; `profile_id` remains informational metadata only.
- follow-up: run D1-1 quick probe

### 2026-04-21 D1-1 quick probe
- lane: `D1`
- milestone: `D1-1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe on cumulative integration worktree
- commands:
  - `git diff --binary -- meerkat-llm-core/src/provider_runtime/binding.rs meerkat-llm-core/src/provider_runtime/runtime.rs meerkat-llm-core/src/provider_runtime/registry.rs meerkat-openai/src/runtime/mod.rs meerkat-anthropic/src/runtime/mod.rs meerkat-gemini/src/runtime/mod.rs meerkat-rest/src/auth_endpoints.rs meerkat-rpc/src/handlers/auth.rs meerkat-web-runtime/src/external_auth.rs scripts/d1_auth_identity_scan.sh .rct/round5/agents/D1-plan.md .rct/round5/agents/D1-checklist.md > /tmp/round5-D1-1.patch`
  - `git apply /tmp/round5-D1-1.patch`
  - `./scripts/repo-cargo check -p meerkat-core`
  - `./scripts/repo-cargo check -p meerkat-llm-core`
  - `./scripts/repo-cargo check -p meerkat-auth-core`
  - `./scripts/repo-cargo check -p meerkat-web-runtime`
  - `./scripts/repo-cargo check -p meerkat-rpc`
  - `./scripts/repo-cargo check -p meerkat-rest`
- result: fail
- excerpt: `meerkat-rpc/src/handlers/auth.rs` moves `realm.realm_id` out after borrowing `realm` through `lookup_binding(...)`, producing `error[E0505]: cannot move out of realm.realm_id because it is borrowed`.
- follow-up: return compile failure to `D1` and rerun quick probe after fix

### 2026-04-21 D1-1 quick probe rerun
- lane: `D1`
- milestone: `D1-1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe rerun after borrow-fix
- commands:
  - `git reset --hard HEAD`
  - `git diff --binary -- meerkat-llm-core/src/provider_runtime/binding.rs meerkat-llm-core/src/provider_runtime/runtime.rs meerkat-llm-core/src/provider_runtime/registry.rs meerkat-openai/src/runtime/mod.rs meerkat-anthropic/src/runtime/mod.rs meerkat-gemini/src/runtime/mod.rs meerkat-rest/src/auth_endpoints.rs meerkat-rpc/src/handlers/auth.rs meerkat-web-runtime/src/external_auth.rs > /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `./scripts/repo-cargo check -p meerkat-core`
  - `./scripts/repo-cargo check -p meerkat-llm-core`
  - `./scripts/repo-cargo check -p meerkat-auth-core`
  - `./scripts/repo-cargo check -p meerkat-web-runtime`
  - `./scripts/repo-cargo check -p meerkat-rpc`
  - `./scripts/repo-cargo check -p meerkat-rest`
- result: pass
- excerpt: all requested `D1-1` quick-probe checks completed successfully after fixing the borrowed `realm.realm_id` move in auth handlers.
- follow-up: run D1-1 full gate

### 2026-04-21 D1-1 full gate attempt
- lane: `D1`
- milestone: `D1-1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate attempt
- commands:
  - `./scripts/repo-cargo nextest run -p meerkat-core --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-llm-core --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-auth-core --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-web-runtime --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rpc --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rest --test rest_auth_endpoints --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- result: aborted
- excerpt: duplicate long-lived cargo/nextest shells created lock contention; the attempt was intentionally torn down so the gate can be rerun cleanly once the process table is quiet.
- follow-up: rerun D1-1 full gate cleanly; do not treat this attempt as pass or fail

### 2026-04-21 D1-1 full gate rerun
- lane: `D1`
- milestone: `D1-1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate rerun with corrected REST auth test invocation
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `./scripts/repo-cargo nextest run -p meerkat-core --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-llm-core --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-auth-core --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-web-runtime --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rpc --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rest --test rest_auth_endpoints --features integration-real-tests --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- result: pass
- excerpt: all focused D1 test legs passed and the final `e2e-fast` lane passed on the isolated integration tree.
- follow-up: mark `D1-1` build-approved and notify `D2` that `D2-2` is unblocked

### 2026-04-21 B1 static gate
- lane: `B`
- milestone: `B1`
- branch: `codex/dogma-B-public-surfaces`
- handoff commit: working tree handoff
- action: static gate review
- commands:
  - `git diff -- meerkat-contracts/src/rest_catalog.rs meerkat-contracts/src/rpc_catalog.rs meerkat-mob-mcp/src/public_mcp.rs meerkat-rest/src/lib.rs meerkat-rpc/src/handlers/session.rs meerkat-rpc/src/router.rs sdks/typescript/src/client.ts sdks/typescript/src/types.ts sdks/python/meerkat/client.py .rct/round5/agents/B-plan.md .rct/round5/agents/B-checklist.md`
- result: fail
- excerpt: rename patch crossed the wrong contract seam by mapping realtime-attachment-status to `submission` / `submissions`, while the locked rename map reserves those names for input state/list surfaces.
- follow-up: returned to `B` for correction before any quick probe

### 2026-04-21 B1 static gate rerun
- lane: `B`
- milestone: `B1`
- branch: `codex/dogma-B-public-surfaces`
- handoff commit: working tree handoff
- action: static gate rerun
- commands:
  - `git diff -- meerkat-contracts/src/rest_catalog.rs meerkat-contracts/src/rpc_catalog.rs meerkat-mob-mcp/src/public_mcp.rs meerkat-rest/src/lib.rs meerkat-rpc/src/handlers/session.rs meerkat-rpc/src/router.rs sdks/typescript/src/client.ts sdks/typescript/src/types.ts sdks/python/meerkat/client.py sdks/python/tests/test_audit_parity.py sdks/python/tests/test_types.py scripts/session_control_public_name_scan.sh Makefile .rct/round5/agents/B-plan.md .rct/round5/agents/B-checklist.md`
  - `scripts/session_control_public_name_scan.sh`
  - `scripts/verify-rpc-surface-alignment.sh`
  - `scripts/verify-sdk-wrapper-freshness.sh`
- result: pass
- excerpt: the locked rename map is respected, the banned-name scan exists and passes, and RPC/SDK surface freshness checks are green.
- follow-up: run B1 quick probe and allow B2 implementation to begin

### 2026-04-21 B1 quick probe attempt
- lane: `B`
- milestone: `B1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe attempt
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-EG1a-code.patch`
  - `git apply /tmp/round5-B1-code.patch`
  - `make regen-schemas`
  - `make verify-schema-freshness`
  - `make verify-rpc-surface-alignment`
  - `make verify-sdk-wrapper-freshness`
  - `./scripts/repo-cargo check -p meerkat-contracts`
  - `./scripts/repo-cargo check -p meerkat-rpc`
  - `./scripts/repo-cargo check -p meerkat-rest`
  - `./scripts/repo-cargo check -p meerkat-mob-mcp`
- result: aborted
- excerpt: `make regen-schemas` invoked `python3` 3.9.6, but `tools/sdk-codegen/generate.py` uses Python 3.10+ `match` syntax and failed with `SyntaxError: invalid syntax`.
- follow-up: rerun B1 quick probe using `python3.11 tools/sdk-codegen/generate.py` instead of the default `python3`

### 2026-04-21 B1 quick probe continuation
- lane: `B`
- milestone: `B1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: continued quick probe with Python 3.11 SDK codegen
- commands:
  - `./scripts/repo-cargo run -p meerkat-contracts --features schema --bin emit-schemas`
  - `python3.11 tools/sdk-codegen/generate.py`
  - `scripts/verify-rpc-surface-alignment.sh`
  - `scripts/verify-sdk-wrapper-freshness.sh`
  - `./scripts/repo-cargo check -p meerkat-contracts`
  - `./scripts/repo-cargo check -p meerkat-rpc`
  - `./scripts/repo-cargo check -p meerkat-rest`
  - `./scripts/repo-cargo check -p meerkat-mob-mcp`
- result: fail
- excerpt: docs alignment still drifts: `docs/api/rpc.mdx` overview table is missing `session/status`, `session/submit`, `session/retire`, `session/reset`, `session/submission`, and `session/submissions`, while still listing the retired runtime-era names.
- follow-up: return docs overview drift to `B` before any further probe/gate work on B1

### 2026-04-21 B1 quick probe rerun
- lane: `B`
- milestone: `B1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe rerun with complete B1 file set and Python 3.11 SDK codegen
- commands:
  - `git diff --binary -- Makefile scripts/session_control_public_name_scan.sh docs/api/rpc.mdx docs/api/rest.mdx meerkat-contracts/src/rest_catalog.rs meerkat-contracts/src/rpc_catalog.rs meerkat-contracts/src/wire/runtime.rs meerkat-mob-mcp/src/public_mcp.rs meerkat-rest/src/lib.rs meerkat-rest/tests/live_rest_matrix.rs meerkat-rpc/src/handlers/runtime.rs meerkat-rpc/src/handlers/session.rs meerkat-rpc/src/router.rs meerkat-rpc/tests/regression_rpc.rs meerkat-rpc/tests/integration_server.rs meerkat-rpc/tests/live_smoke_rpc.rs meerkat-rpc/tests/tcp_e2e.rs sdks/typescript/src/client.ts sdks/typescript/src/types.ts sdks/python/meerkat/client.py sdks/python/meerkat/generated/types.py sdks/python/tests/test_audit_parity.py sdks/python/tests/test_e2e_smoke.py > /tmp/round5-B1-code.patch`
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-EG1a-code.patch`
  - `git apply /tmp/round5-B1-code.patch`
  - `./scripts/repo-cargo run -p meerkat-contracts --features schema --bin emit-schemas`
  - `python3.11 tools/sdk-codegen/generate.py`
  - `scripts/verify-rpc-surface-alignment.sh`
  - `scripts/verify-sdk-wrapper-freshness.sh`
  - `./scripts/repo-cargo check -p meerkat-contracts`
  - `./scripts/repo-cargo check -p meerkat-rpc`
  - `./scripts/repo-cargo check -p meerkat-rest`
  - `./scripts/repo-cargo check -p meerkat-mob-mcp`
- result: pass
- excerpt: schema/codegen replay completed successfully, RPC/docs and SDK alignment checks passed, and all requested Rust compile checks passed on the cumulative integration tree.
- follow-up: run B1 full gate

### 2026-04-21 B1 full gate
- lane: `B`
- milestone: `B1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate on cumulative integration worktree
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-EG1a-code.patch`
  - `git apply /tmp/round5-B1-code.patch`
  - `./scripts/repo-cargo run -p meerkat-contracts --features schema --bin emit-schemas`
  - `python3.11 tools/sdk-codegen/generate.py`
  - `./scripts/repo-cargo nextest run -p meerkat-contracts --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-mob-mcp --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rpc --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rest --lib --show-progress none --status-level none --final-status-level fail`
  - `python3.11 -m pytest sdks/python/tests/test_types.py sdks/python/tests/test_audit_parity.py`
  - `npm --prefix sdks/typescript run build`
  - `env -u MEERKAT_BIN_PATH -u RKAT_ANTHROPIC_API_KEY -u ANTHROPIC_API_KEY -u RKAT_OPENAI_API_KEY -u OPENAI_API_KEY -u SMOKE_MODEL -u SMOKE_MODEL_ANTHROPIC -u SMOKE_MODEL_OPENAI npm --prefix sdks/typescript test`
  - `./scripts/repo-cargo e2e-fast`
- result: pass
- excerpt: regen/codegen replay, Rust library tests, Python SDK tests, TypeScript SDK tests against the integration-tree `rkat-rpc`, and the final `e2e-fast` lane all passed.
- follow-up: mark `B1` build-approved and proceed with `B2` / `EG2`

### 2026-04-21 B1 full gate confirmation
- lane: `B`
- milestone: `B1`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: clean TypeScript SDK rerun and final integration sanity confirmation
- commands:
  - create `target-codex/debug/rkat-rpc` symlink to the integration-tree `rkat-rpc`
  - `npm --prefix sdks/typescript run build`
  - `env -u MEERKAT_BIN_PATH -u RKAT_ANTHROPIC_API_KEY -u ANTHROPIC_API_KEY -u RKAT_OPENAI_API_KEY -u OPENAI_API_KEY -u SMOKE_MODEL -u SMOKE_MODEL_ANTHROPIC -u SMOKE_MODEL_OPENAI npm --prefix sdks/typescript test`
  - `./scripts/repo-cargo e2e-fast`
- result: pass
- excerpt: packaged local TypeScript SDK smoke passed against the integration-tree binary, scripted-stream fixture tests skipped on known fixture-version drift, live smoke skipped without provider env, and the final `e2e-fast` lane passed from a clean B1 state.
- follow-up: proceed with `B2` and `EG2`

### 2026-04-21 B2 static gate
- lane: `B`
- milestone: `B2`
- branch: `codex/dogma-B-public-surfaces`
- handoff commit: working tree handoff
- action: static gate review
- commands:
  - `git diff -- meerkat-rpc/src/realtime_ws.rs .rct/round5/agents/B-plan.md .rct/round5/agents/B-checklist.md`
  - `sed -n '2948,2972p' meerkat-rpc/src/realtime_ws.rs`
- result: pass
- excerpt: the B2 delta is limited to websocket-side `attempt_count` parity for `ReplacementPending` and `ReattachRequired`, matching the already-green RPC/public status semantics.
- follow-up: run B2 quick probe

### 2026-04-21 B2 quick probe
- lane: `B`
- milestone: `B2`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe on cumulative integration worktree
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-EG1a-code.patch`
  - sync `meerkat-rpc/src/realtime_ws.rs` from the main workspace into the integration worktree
  - `./scripts/repo-cargo check -p meerkat-core`
  - `./scripts/repo-cargo check -p meerkat-runtime`
- result: pass
- excerpt: the B2 parity-only delta compiles cleanly on top of the green B1 and EG1a foundations; remaining output was warning-only noise from still-open A1b work elsewhere in the tree.
- follow-up: run B2 full gate

### 2026-04-21 B2 full gate
- lane: `B`
- milestone: `B2`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate on cumulative integration worktree
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-EG1a-code.patch`
  - `git apply /tmp/round5-B1-code.patch`
  - sync `meerkat-rpc/src/realtime_ws.rs` from the main workspace into the integration worktree
  - `./scripts/repo-cargo nextest run -p meerkat-rpc --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-mob-mcp --lib --show-progress none --status-level none --final-status-level fail`
  - `npm --prefix sdks/typescript run build`
  - `env -u MEERKAT_BIN_PATH -u RKAT_ANTHROPIC_API_KEY -u ANTHROPIC_API_KEY -u RKAT_OPENAI_API_KEY -u OPENAI_API_KEY -u SMOKE_MODEL -u SMOKE_MODEL_ANTHROPIC -u SMOKE_MODEL_OPENAI npm --prefix sdks/typescript test`
  - `./scripts/repo-cargo e2e-fast`
- result: pass
- excerpt: the websocket-side `attempt_count` parity fix passed the focused RPC/MCP tests, the packaged TypeScript SDK tests, and the final `e2e-fast` lane.
- follow-up: mark `B2` build-approved; B lane complete

### 2026-04-21 A1b static gate
- lane: `A`
- milestone: `A1b`
- branch: `codex/dogma-A-runtime-authority`
- handoff commit: working tree handoff
- action: static gate review
- commands:
  - `git diff -- meerkat-core/src/agent/state.rs meerkat-core/src/agent/runner.rs meerkat-core/src/handles.rs meerkat-runtime/src/handles/turn_state.rs .rct/round5/agents/A-plan.md .rct/round5/agents/A-checklist.md .rct/round5/agents/A-runtime-authority-scan.sh`
- result: pass
- excerpt: the remaining pre-start local-shadow authority sites are now removed from the runtime-backed path, and `DOGMA-13` is at an honest handoff-ready milestone.
- follow-up: run A1b quick probe

### 2026-04-21 A1b quick probe
- lane: `A`
- milestone: `A1b`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe on cumulative integration worktree using file-sync for A1b-over-A1a state
- commands:
  - sync owned `A1b` files from the main workspace into the integration worktree:
    - `meerkat-core/src/agent/state.rs`
    - `meerkat-core/src/agent/runner.rs`
    - `meerkat-core/src/handles.rs`
    - `meerkat-runtime/src/handles/turn_state.rs`
  - `./scripts/repo-cargo check -p meerkat-core`
  - `./scripts/repo-cargo check -p meerkat-runtime`
- result: fail
- excerpt: `meerkat-runtime/src/handles/turn_state.rs` tries to read `state.active_run` and use `RunId::from_domain`, but the machine state exposes a different field shape (`current_run_id`) and no matching helper.
- follow-up: return the runtime turn-state field mismatch to `A` and rerun the quick probe after fix

### 2026-04-21 A1b quick probe rerun
- lane: `A`
- milestone: `A1b`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe rerun after runtime turn-state field-mapping fix
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-A1a-code.patch`
  - sync owned `A1b` files from the main workspace into the integration worktree
  - `./scripts/repo-cargo check -p meerkat-core`
  - `./scripts/repo-cargo check -p meerkat-runtime`
- result: pass
- excerpt: the runtime-backed turn-state field mapping now compiles cleanly; remaining signal is warning-only `dead_code` on `LocalTurnExecutionState::can_accept`.
- follow-up: run A1b full gate

### 2026-04-21 A1b full gate
- lane: `A`
- milestone: `A1b`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: full gate on cumulative integration worktree
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-A1a-code.patch`
  - sync owned `A1b` files from the main workspace into the integration worktree
  - `./scripts/repo-cargo nextest run -p meerkat-core --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-runtime --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- result: pass
- excerpt: `meerkat-core`, `meerkat-runtime`, and the final `e2e-fast` lane all passed on the cumulative integration tree; remaining signal is warning-only `dead_code` on `LocalTurnExecutionState::can_accept`.
- follow-up: mark `A1b` build-approved and close lane A

### 2026-04-21 EG1b-a static gate
- lane: `EG`
- milestone: `EG1b-a`
- branch: `codex/dogma-EG-session-schedule`
- handoff commit: working tree handoff
- action: static gate review for the `DOGMA-14` half only
- commands:
  - `git diff -- meerkat-core/src/session_recovery.rs meerkat-session/src/persistent.rs .rct/round5/agents/EG-plan.md .rct/round5/agents/EG-checklist.md`
- result: pass
- excerpt: the recovery fallback change is internally coherent and honestly limited to `DOGMA-14`; `DOGMA-23` remains open and is not being claimed by this handoff.
- follow-up: run `EG1b-a` quick probe

### 2026-04-21 EG1b-a quick probe
- lane: `EG`
- milestone: `EG1b-a`
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: quick probe on cumulative integration worktree for the `DOGMA-14` half
- commands:
  - `git reset --hard HEAD`
  - `git apply /tmp/round5-D2-1-code.patch`
  - `git apply /tmp/round5-D1-1-code.patch`
  - `git apply /tmp/round5-EG1a-code.patch`
  - sync `meerkat-core/src/session_recovery.rs` and `meerkat-session/src/persistent.rs` from the main workspace into the integration worktree
  - `./scripts/repo-cargo check -p meerkat-session`
  - `./scripts/repo-cargo check -p meerkat-core`
- result: pass
- excerpt: the `DOGMA-14` recovery fallback changes compile cleanly on top of the green integration baseline.
- follow-up: run `EG1b-a` full gate

### 2026-04-22 Final Gate
- lane: orchestrator
- milestone: round5-final
- branch: `codex/dogma-round5-integration`
- handoff commit: working tree handoff
- action: final verification on the integrated Round 5 tree
- commands:
  - `make verify-schema-freshness`
  - `make verify-rpc-surface-alignment`
  - `make verify-sdk-wrapper-freshness`
  - `./scripts/repo-cargo nextest run --workspace --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
  - `./scripts/repo-cargo e2e-system`
  - `make test-sdk-typescript`
  - `make test-sdk-python`
- result: pass
- excerpt: freshness gates, full workspace nextest, explicit `e2e-fast`, explicit `e2e-system`, and both SDK suites passed after final-gate fixes in deferred runtime registration, source-identity default-root dedupe, CLI binary-path helpers, and SDK capability-gated mob stream smoke tests.
- follow-up: mark all Round 5 milestones and rows `final-green`
