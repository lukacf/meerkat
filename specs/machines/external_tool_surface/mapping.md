# ExternalToolSurfaceMachine Mapping Note

This note maps the normative `0.5` `ExternalToolSurfaceMachine` contract onto
current implementation anchors.

## Rust anchors

- primary lifecycle owner:
  - `meerkat-mcp/src/router.rs`
- runtime-facing adapter:
  - `meerkat-mcp/src/adapter.rs`
- runtime/agent boundary consumers:
  - `meerkat-core/src/agent/state.rs`
  - `meerkat/src/surface.rs`
  - `meerkat-rest/src/lib.rs`
  - `meerkat-rpc/src/session_runtime.rs`

## What is already aligned

- staged add/remove/reload already exists via `stage_add`, `stage_remove`, and
  `stage_reload`
- boundary apply already exists via `apply_staged()`
- background activation/reload completion already resolves asynchronously
- removals are already inflight-aware and support forced degradation after
  timeout
- visible tool membership already excludes removing/removed servers

## What the formal model abstracts

The TLA+ model deliberately abstracts away:

- concrete MCP connection objects
- tool definitions and tool-name maps
- generation counters used to discard stale pending completions
- batching of multiple staged operations in one boundary apply
- adapter cache locks and fast-path atomics
- transport-specific drain tasks in REST/RPC
- browser-local registration mechanics for runtime-scoped local tools

Those are shell details refining the same lifecycle semantics rather than
separate machine behavior.

## Intentional `0.5` shift

The canonical outward contract is now one typed delta family:

- `ExternalToolDelta { target, operation, phase, persisted, applied_at_turn }`

That means:

- `McpLifecycleAction` is no longer an authoritative public contract
- `ExternalToolNotice` is no longer an authoritative public contract
- transcript/system notices are projections only
- transport-specific surface updates in REST/RPC/WASM must derive from the same
  typed delta stream

## Known precursor divergences

- lifecycle actions and async completion notices are still split across
  multiple outward shapes today
- some surface-specific drain emission in REST/RPC is still transport-ish
  rather than purely machine-owned
- synthetic transcript notice patterns around pending MCP state still exist as
  transitional UX even though typed deltas are the authoritative `0.5`
  contract
