# external_tool_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `external_tool_surface`: `ExternalToolSurfaceMachine` @ actor `surface_boundary`
- `runtime_control`: `RuntimeControlMachine` @ actor `control_plane`
- `turn_execution`: `TurnExecutionMachine` @ actor `turn_executor`

## Routes
- `surface_delta_notifies_runtime_control`: `external_tool_surface`.`EmitExternalToolDelta` -> `runtime_control`.`ExternalToolDeltaReceived` [Immediate]
- `turn_boundary_applies_surface_changes`: `turn_execution`.`BoundaryApplied` -> `external_tool_surface`.`ApplyBoundary` [Immediate]

## Scheduler Rules
- `PreemptWhenReady(control_plane, surface_boundary)`

## Structural Requirements
- `control_preempts_surface_boundary` — runtime control outranks surface-boundary work when both are ready
- `surface_completion_protocol_covered` — ScheduleSurfaceCompletion effect is covered by surface_completion handoff protocol
- `surface_snapshot_alignment_protocol_covered` — RefreshVisibleSurfaceSet effect is covered by surface_snapshot_alignment handoff protocol

## Behavioral Invariants
- `external_tool_delta_enters_runtime_control` — canonical external-tool deltas enter runtime through the runtime-control boundary
- `boundary_application_reaches_surface_authority` — turn-execution boundary application enters external-tool surface authority through the explicit owner-bridged route with owner-selected surface identity and applied_at_turn

## Coverage
### Code Anchors
- `meerkat-mcp/src/router.rs` — tool-surface lifecycle precursor
- `meerkat-core/src/agent/state.rs` — turn-boundary tool update consumer precursor
- `meerkat/src/surface.rs` — surface projection precursor

### Scenarios
- `tool-delta-to-runtime` — external-tool deltas reach runtime through canonical control/runtime surfaces
- `reload-remove-during-turns` — live tool surface changes coordinate with turn boundaries
- `browser-local-tool-surface` — WASM/browser local tools follow the same runtime-owned tool surface
