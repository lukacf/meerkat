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

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `ExternalToolSurfaceMachine`

### Code Anchors
- `mcp_router`: `meerkat-mcp/src/router.rs` — staged MCP surface lifecycle precursor
- `mcp_adapter`: `meerkat-mcp/src/adapter.rs` — runtime-facing tool-surface adapter precursor
- `agent_tool_state`: `meerkat-core/src/agent/state.rs` — turn-boundary external-tool update consumer precursor

### Scenarios
- `add-reload-remove` — surface add, reload, and removal produce canonical typed deltas
- `draining-removal` — removing surfaces drain inflight work before final removal
- `runtime-scoped-browser-tools` — browser-local tools remain runtime-scoped external surfaces

### Transitions
- `StageAdd`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `StageRemove`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `StageReload`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `ApplyBoundaryAdd`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `ApplyBoundaryReload`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `ApplyBoundaryRemoveDraining`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `ApplyBoundaryRemoveNoop`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `PendingSucceededAdd`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `PendingSucceededReload`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `PendingFailedAdd`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `draining-removal`, `runtime-scoped-browser-tools`
- `PendingFailedReload`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `CallStartedActive`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `CallStartedRejectWhileRemoving`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `CallStartedRejectWhileUnavailable`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `CallFinishedActive`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `CallFinishedRemoving`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `FinalizeRemovalClean`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `FinalizeRemovalForced`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `SnapshotAligned`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `Shutdown`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`

### Effects
- `ScheduleSurfaceCompletion`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `RefreshVisibleSurfaceSet`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `EmitExternalToolDelta`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `CloseSurfaceConnection`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `RejectSurfaceCall`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`

### Invariants
- `visible_surfaces_subset_of_known_surfaces`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `base_state_keys_subset_of_known_surfaces`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `pending_op_keys_subset_of_known_surfaces`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `staged_op_keys_subset_of_known_surfaces`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `staged_intent_sequence_keys_subset_of_known_surfaces`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `pending_task_sequence_keys_subset_of_known_surfaces`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `pending_lineage_sequence_keys_subset_of_known_surfaces`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `inflight_calls_keys_subset_of_known_surfaces`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `last_delta_operation_keys_subset_of_known_surfaces`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `last_delta_phase_keys_subset_of_known_surfaces`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `removing_or_removed_surfaces_are_not_visible`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `visible_membership_matches_active_base_state`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `removing_surfaces_have_no_pending_add_or_reload`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `removed_surfaces_only_allow_pending_none_or_add`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `inflight_calls_only_exist_for_active_or_removing_surfaces`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `reload_pending_requires_active_base_state`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `removed_surfaces_have_zero_inflight_calls`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `forced_delta_phase_is_always_a_remove_delta`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `add-reload-remove`, `runtime-scoped-browser-tools`
- `staged_sequence_matches_staged_presence`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `pending_lineage_matches_pending_presence`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`
- `snapshot_alignment_epoch_not_ahead`
  - anchors: `mcp_router`, `mcp_adapter`, `agent_tool_state`
  - scenarios: `runtime-scoped-browser-tools`


<!-- GENERATED_COVERAGE_END -->
