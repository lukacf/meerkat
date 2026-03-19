# TurnExecutionMachine Mapping Note

This note maps the normative `0.5` `TurnExecutionMachine` contract onto
current `0.4` execution anchors.

## Rust anchors

- core execution state and loop:
  - `meerkat-core/src/state.rs`
  - `meerkat-core/src/agent/state.rs`
  - `meerkat-core/src/agent/runner.rs`
- core execution interface:
  - `meerkat-core/src/lifecycle/run_primitive.rs`
  - `meerkat-core/src/lifecycle/run_event.rs`
  - `meerkat-core/src/lifecycle/core_executor.rs`
- runtime driver/execution bridge:
  - `meerkat-runtime/src/runtime_loop.rs`
  - `meerkat-runtime/src/session_adapter.rs`
- persistence/session-runtime bridge:
  - `meerkat-session/src/persistent.rs`
  - `meerkat-rpc/src/session_runtime.rs`

## What is already aligned

- `RunPrimitive` is already the canonical execution input family
- `RunEvent` is already the canonical cross-boundary execution effect family
- `LoopState` already formalizes the core LLM/tool execution loop
- runtime-backed execution already stages inputs, executes through
  `CoreExecutor::apply()`, and commits `BoundaryApplied` / terminal run events

## What the formal model abstracts

The TLA+ model deliberately abstracts away:

- detailed session message mutation
- hook invocation and patch content
- exact tool-call payloads
- extraction-mode details
- compaction and memory indexing
- persistence and checkpoint I/O internals
- event-tap/subscriber streaming mechanics
- unbounded long-run turn loops; the checked-in CI profile uses an explicit
  boundary-count bound so the model remains finite and CI-suitable

Those are important implementation details, but they refine the same turn
execution semantics rather than changing the machine boundary.

## Key `0.5` narrowing

The formal machine deliberately excludes the current host-mode orchestration in
`meerkat-core/src/agent/comms_impl.rs`.

That exclusion is intentional:

- `Agent` remains the execution component inside `TurnExecutionMachine`
- host-mode idle waiting, inbox draining, continuation scheduling, and
  multi-turn runtime coordination move upward into runtime control

## Known `0.4` divergence

- the current `Agent` still owns host-mode orchestration helpers in addition to
  the pure turn loop
- direct/session-service execution paths still exist alongside runtime-driven
  execution paths
- some immediate/context-only behavior is still realized through current
  runtime loop conversions rather than a fully explicit turn-execution owner

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `TurnExecutionMachine`

### Code Anchors
- `turn_state`: `meerkat-core/src/agent/state.rs` — core turn loop state precursor
- `turn_runner`: `meerkat-core/src/agent/runner.rs` — turn runner precursor
- `run_primitive`: `meerkat-core/src/lifecycle/run_primitive.rs` — canonical run primitive input precursor
- `run_event`: `meerkat-core/src/lifecycle/run_event.rs` — canonical run event/effect precursor

### Scenarios
- `conversation-run` — conversation run starts, applies boundaries, and completes cleanly
- `tool-and-retry-loop` — tool calls and retry/yield semantics stay inside the turn owner
- `cancel-and-fail` — cancelled and failed runs produce explicit terminal outcomes

### Transitions
- `StartConversationRun`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `StartImmediateAppend`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `StartImmediateContext`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `PrimitiveAppliedConversationTurn`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `PrimitiveAppliedImmediateAppend`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `PrimitiveAppliedImmediateAppendCancelsAfterBoundary`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `PrimitiveAppliedImmediateContext`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `PrimitiveAppliedImmediateContextCancelsAfterBoundary`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `LlmReturnedToolCalls`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `tool-and-retry-loop`
- `ToolCallsResolved`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `tool-and-retry-loop`
- `LlmReturnedTerminal`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `BoundaryContinue`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `BoundaryContinueCancelsAfterBoundary`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `BoundaryComplete`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `BoundaryCompleteCancelsAfterBoundary`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `RecoverableFailureFromCallingLlm`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `tool-and-retry-loop`
- `RecoverableFailureFromWaitingForOps`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `tool-and-retry-loop`
- `RecoverableFailureFromDrainingBoundary`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `tool-and-retry-loop`
- `RetryRequested`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `tool-and-retry-loop`
- `FatalFailureFromApplyingPrimitive`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `FatalFailureFromCallingLlm`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `FatalFailureFromWaitingForOps`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `FatalFailureFromDrainingBoundary`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `FatalFailureFromErrorRecovery`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `CancelNowFromApplyingPrimitive`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `CancelNowFromCallingLlm`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `CancelNowFromWaitingForOps`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `CancelNowFromDrainingBoundary`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `CancelNowFromErrorRecovery`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `CancelAfterBoundaryFromApplyingPrimitive`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `CancelAfterBoundaryFromCallingLlm`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `CancelAfterBoundaryFromWaitingForOps`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `CancelAfterBoundaryFromDrainingBoundary`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `CancelAfterBoundaryFromErrorRecovery`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `CancellationObserved`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `TurnLimitReachedFromApplyingPrimitive`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `TurnLimitReachedFromCallingLlm`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `TurnLimitReachedFromWaitingForOps`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `TurnLimitReachedFromDrainingBoundary`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `TurnLimitReachedFromErrorRecovery`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `BudgetExhaustedFromApplyingPrimitive`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `BudgetExhaustedFromCallingLlm`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `BudgetExhaustedFromWaitingForOps`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `BudgetExhaustedFromDrainingBoundary`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `BudgetExhaustedFromErrorRecovery`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `ForceCancelNoRunFromReady`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `ForceCancelNoRunFromApplyingPrimitive`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `ForceCancelNoRunFromCallingLlm`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `ForceCancelNoRunFromWaitingForOps`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `ForceCancelNoRunFromDrainingBoundary`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `ForceCancelNoRunFromErrorRecovery`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `ForceCancelNoRunFromCancelling`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `AcknowledgeTerminalFromCompleted`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `AcknowledgeTerminalFromFailed`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `AcknowledgeTerminalFromCancelled`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`

### Effects
- `RunStarted`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `BoundaryApplied`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `RunCompleted`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `RunFailed`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `RunCancelled`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`

### Invariants
- `ready_has_no_active_run`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `ready_has_no_admitted_content`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `non_ready_has_active_run`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `waiting_for_ops_implies_pending_tools`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `tool-and-retry-loop`
- `ready_has_no_boundary_cancel_request`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `cancel-and-fail`
- `immediate_primitives_skip_llm_and_recovery`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `terminal_states_match_terminal_outcome`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`
- `completed_runs_have_seen_a_boundary`
  - anchors: `turn_state`, `turn_runner`, `run_primitive`, `run_event`
  - scenarios: `conversation-run`


<!-- GENERATED_COVERAGE_END -->
