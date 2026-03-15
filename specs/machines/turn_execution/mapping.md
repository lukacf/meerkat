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
