# external_tool_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `external_tool_bundle`

### Code Anchors
- `mcp_router`: `meerkat-mcp/src/router.rs` — tool-surface lifecycle precursor
- `agent_tool_state`: `meerkat-core/src/agent/state.rs` — turn-boundary tool update consumer precursor
- `surface_projection`: `meerkat/src/surface.rs` — surface projection precursor

### Scenarios
- `tool-delta-to-runtime` — external-tool deltas reach runtime through canonical control/runtime surfaces
- `reload-remove-during-turns` — live tool surface changes coordinate with turn boundaries
- `browser-local-tool-surface` — WASM/browser local tools follow the same runtime-owned tool surface

### Routes
- `surface_delta_notifies_runtime_control`
  - anchors: `mcp_router`, `agent_tool_state`, `surface_projection`
  - scenarios: `tool-delta-to-runtime`, `browser-local-tool-surface`
- `turn_boundary_applies_surface_changes`
  - anchors: `mcp_router`, `agent_tool_state`, `surface_projection`
  - scenarios: `reload-remove-during-turns`, `browser-local-tool-surface`

### Scheduler Rules
- `PreemptWhenReady(control_plane, surface_boundary)`
  - anchors: `mcp_router`, `agent_tool_state`, `surface_projection`
  - scenarios: `tool-delta-to-runtime`

### Invariants
- `external_tool_delta_enters_runtime_control`
  - anchors: `mcp_router`, `agent_tool_state`, `surface_projection`
  - scenarios: `tool-delta-to-runtime`, `browser-local-tool-surface`
- `boundary_application_reaches_surface_authority`
  - anchors: `mcp_router`, `agent_tool_state`, `surface_projection`
  - scenarios: `reload-remove-during-turns`, `browser-local-tool-surface`
- `control_preempts_surface_boundary`
  - anchors: `mcp_router`, `agent_tool_state`, `surface_projection`
  - scenarios: `tool-delta-to-runtime`
- `surface_completion_protocol_covered`
  - anchors: `mcp_router`, `agent_tool_state`, `surface_projection`
  - scenarios: `reload-remove-during-turns`, `browser-local-tool-surface`


<!-- GENERATED_COVERAGE_END -->
