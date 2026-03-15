# Meerkat 0.5 Surface Cutover Matrix

Status: normative `0.5` rollout matrix

## Purpose

This document turns surface convergence from a vague expectation into explicit
release checkpoints.

Every public surface must satisfy the same core rule:

- convenience APIs may remain
- independent ordinary execution paths may not

## Shared Cutover Rule

For every surface:

1. identify the current legacy bypass, if any
2. add an early adapter into the canonical runtime path when needed
3. keep compatibility façades only when they have zero independent semantics
4. delete the bypass before `0.5` is declared complete

## Core Checkpoints

Surface work depends on these global checkpoints:

- `CORE-1`
  in-repo machine bundle and verification workflow landed
- `CORE-2`
  runtime/control/admission path is the only ordinary execution path in core
- `CORE-3`
  `OpsLifecycleMachine` seam landed as the shared async-operation lifecycle
  substrate, with child-agent orchestration routed only through mobs
- `CORE-4`
  `TurnExecutionMachine` narrowed and host direct path deleted

No surface is truly complete before `CORE-4`.

## Surface Matrix

| Surface | Current canonical path | Legacy bypass still present | Required bridge step | Final `0.5` release gate |
| --- | --- | --- | --- | --- |
| Rust SDK | runtime/session service path already exists | direct `Agent` host/execution ownership still visible | publish runtime-backed helpers and narrow `Agent` role | docs/examples/default path use runtime/session APIs; direct `Agent` is execution-only |
| CLI | `run` / `resume` already runtime-backed | stdin/event injector path | route host-driven external input as runtime `ExternalEvent` | no CLI code injects ordinary work outside runtime admission |
| REST | session routes already runtime-backed | legacy `/sessions/{id}/event` injector path | make `POST /sessions/{id}/external-events` canonical and keep low-level runtime admission explicit | no REST route uses `EventInjector` as an execution path and `/sessions/{id}/event` is gone |
| JSON-RPC | `session/*` and `turn/*` already runtime-backed | `event/push` injector path | make `session/external_event` canonical and keep `runtime/accept` authoritative | no RPC method has an execution side path outside runtime admission and `event/push` is gone |
| MCP server | not yet fully converged | `meerkat_run` / `meerkat_resume` direct session/turn path | route through runtime admission + completion waiting while keeping public names stable | MCP tool methods no longer call direct create/start-turn flow |
| WASM | runtime/service path already exists | direct session registry + direct `start_turn()` path + pre-init JS tool owner | explicit runtime bootstrap, runtime-backed browser bridge, façade handles, runtime-scoped browser-local tool surface, explicit capability envelope | no authoritative direct-session owner, no per-turn `build_agent()`, no pre-init global tool owner, unsupported features fail early |
| Python SDK | thin wrapper | inherits backend contract changes only | regenerate against canonical backend/browser contracts | wrapper owns no independent semantics |
| TypeScript SDK | thin wrapper | inherits backend/browser contract changes only | regenerate against canonical backend/browser contracts | wrapper owns no independent semantics |

## Surface-Specific Work

### Rust SDK

Current anchors:

- runtime/session-service integration already exists
- direct `Agent` construction remains visible to expert embedders

Required work:

1. make runtime/session-backed embedding the documented default
2. narrow `Agent` documentation to "turn executor" rather than host/runtime
   owner
3. keep low-level `Agent` entry points only as explicit advanced APIs

Release gate:

- docs and examples no longer imply that direct `Agent` is the ordinary
  multi-turn runtime owner

### CLI

Current legacy bypass:

- stdin/event injection path

Required work:

1. replace `stdin_events` injection with runtime `ExternalEvent` admission
2. keep CLI UX unchanged where possible
3. align any CLI-local mob orchestration helpers with the shared mob and ops
   substrates

Release gate:

- CLI has no ordinary work path outside runtime admission

### REST

Current legacy bypass:

- `POST /sessions/{id}/event`

Required work:

1. keep low-level runtime admission explicit as
   `POST /runtime/{id}/accept` with `Input::ExternalEvent`
2. make `POST /sessions/{id}/external-events` the canonical high-level REST
   convenience route
3. keep SSE/event streams as presentation over runtime events, not as
   execution ownership
4. delete `POST /sessions/{id}/event` after at most one early-adapter phase

Release gate:

- no REST handler uses the event injector as a parallel execution path
- `/sessions/{id}/event` is deleted

### JSON-RPC

Current legacy bypass:

- `event/push`

Required work:

1. keep `runtime/accept` as the authoritative low-level runtime/control method
2. make `session/external_event` the canonical high-level RPC convenience
   method
3. delete `event/push` after at most one early-adapter phase
4. keep callback tool registration/execution as transport/server concerns
   layered over runtime rather than a second execution path

Release gate:

- no RPC method has an execution side path outside runtime admission
- `event/push` is deleted

### MCP Server

Current legacy bypass:

- `meerkat_run`
- `meerkat_resume`

Required work:

1. route these tools through runtime/session admission plus completion waiting
2. keep returned tool contracts stable unless a deliberate public change is
   made
3. ensure tool-streaming / event callbacks continue to reflect runtime results

Release gate:

- the MCP server no longer owns a separate direct create-session/start-turn
  execution flow

### WASM

WASM keeps the same architectural truth where supported, but with a smaller
browser capability envelope.

Required bridge sequence:

1. require explicit runtime bootstrap before any session, mob, event, or
   browser-tool operation
2. make numeric direct-session handles façades over real runtime-backed
   `SessionId`
3. change `start_turn()` to runtime admission plus completion waiting
4. change polling/subscription APIs into adapters over runtime-backed event
   streams
5. move browser-local tool registration from pre-init global ownership into
   runtime-scoped browser tool-surface ownership
6. keep browser child-agent behavior, where supported, on the mob path only
7. fail unsupported capabilities explicitly instead of inventing browser-only
   substitute semantics

Compatibility stance:

- browser callers can keep a simple `runtime -> session -> turn()` UX
- they may not keep a second authoritative browser execution subsystem
- browser-local JS tools remain supported as a first-class runtime capability

Release gate:

- no authoritative direct-session registry
- no direct per-turn `build_agent()`
- no pre-init global JS tool owner
- unsupported features fail early and explicitly

### Python SDK

Required work:

1. track canonical backend/browser contract updates
2. expose new runtime event/external-event contract shapes where required
3. avoid adding SDK-local semantics that differ from runtime-backed behavior

Release gate:

- Python is only a façade over canonical backend/runtime semantics

### TypeScript SDK

Required work:

1. track canonical backend/browser contract updates
2. track WASM runtime cutover for browser-backed mode
3. keep compatibility helpers thin and early

Release gate:

- TypeScript owns no independent runtime semantics in either server or browser
  mode

## Deletion Gates

No surface is considered finished until its old bypass is deleted or reduced to
an early adapter with no independent execution semantics.

Deletion examples:

- CLI stdin injector as authoritative path
- REST `EventInjector` path and `/sessions/{id}/event`
- RPC `event/push`
- MCP direct `create_session` / `start_turn` path
- WASM direct session registry, direct per-turn `Agent` build path, and
  pre-init global JS tool owner

## Release Rule

`0.5` is complete only when every surface above satisfies its release gate.

There is no "we will finish that in 0.6" escape hatch in this plan.
