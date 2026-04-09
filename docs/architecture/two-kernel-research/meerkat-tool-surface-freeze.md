# Meerkat Tool-Surface Freeze

Status: frozen exact-current-state asset

This note closes `K5` from `meerkat-cutover-checklist.md`.

It captures the exact current live Meerkat semantics for external tool-surface
lifecycle as observed through the MCP router / authority snapshot boundary.

## Scope

This freeze covers:

- live `ExternalToolSurfaceSnapshot` forwarding through core, session, and
  service-factory seams
- staged add / remove / reload intent
- boundary apply semantics
- inflight-call / pending-op / removal-timing shape
- joined `MeerkatMachine` tool-surface validation rules

This freeze does **not** claim:

- a separate store-backed Meerkat recovery log for tool-surface state
- that every router-internal cache or connection detail is a machine-owned fact

## Exact current semantics

### 1. Tool-surface truth is published from the live router authority

Exact current live behavior:

- the Meerkat snapshot reaches tool-surface truth through:
  - core runner snapshot forwarding
  - session-service snapshot forwarding
  - service-factory snapshot forwarding
  - router-owned `ExternalToolSurfaceSnapshot`

### 2. Staging and boundary apply are distinct

Exact current live behavior:

- `stage_add`, `stage_remove`, and `stage_reload` record intent
- `apply_staged()` owns the boundary transition into pending / applied state
- add and reload are non-blocking
- remove becomes hidden on boundary apply while finalization continues through
  the authority

### 3. Shape invariants are machine-owned today

The exact current joined validator freezes the following tool-surface rules:

- visible membership must match active base state
- pending op must be compatible with base state
- inflight calls only exist for `Active | Removing`
- forced delta phase implies remove delta operation
- staged intent sequence exists iff staged op is not `None`
- pending task / lineage sequences exist iff pending op is not `None`
- removal timing exists iff base state is `Removing`

### 4. Recovery is exact-current live reconstruction from router state

Exact current live behavior:

- the Meerkat boundary freezes the live router authority snapshot
- it does **not** freeze an additional store-backed recovery layer above that
- router recovery / reconciliation is therefore represented exactly as
  “whatever the live router authority snapshot currently publishes”

That absence of a second recovery layer is part of the exact current freeze.

## Exact-vs-target-state classification

What is frozen as exact current live behavior:

- snapshot forwarding through runner / session / factory seams
- stage / apply / finalize lifecycle as owned by the router authority
- the current machine-visible tool-surface shape rules

What is explicitly **not** frozen as exact current live behavior:

- deeper transport / connection caches as machine facts
- a separate store-backed Meerkat recovery protocol for tool-surface state

## Proof inventory

Core / session forwarding proofs:

- `snapshot_reflects_active_and_staged_scope_state`
- `external_tool_surface_snapshot_returns_live_agent_tool_surface_state`
- `capture_meerkat_machine_snapshot_joins_live_external_tool_surface_state`

Router / authority lifecycle proofs:

- `stage_add_records_intent`
- `stage_remove_records_intent`
- `stage_reload_requires_active_base`
- `stage_reload_succeeds_when_active`
- `staged_add_remove_reload_transitions`
- `remove_is_immediately_hidden_on_boundary_apply`
- `apply_staged_add_is_non_blocking`
- `add_remove_add_discards_stale_generation`

Joined Meerkat proof:

- `validate_meerkat_machine_snapshot_reports_tool_surface_shape_violations`

## Reviewer Verification Lane

The narrow verification lane for this freeze asset is:

- `cargo test -p meerkat-core --lib snapshot_reflects_active_and_staged_scope_state`
- `cargo test -p meerkat-session --test regression_lifecycle external_tool_surface_snapshot_returns_live_agent_tool_surface_state`
- `cargo test -p meerkat --lib --features mcp capture_meerkat_machine_snapshot_joins_live_external_tool_surface_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_tool_surface_shape_violations`
- `cargo test -p meerkat-mcp --lib staged_add_remove_reload_transitions`
- `cargo test -p meerkat-mcp --lib remove_is_immediately_hidden_on_boundary_apply`
- `cargo test -p meerkat-mcp --lib apply_staged_add_is_non_blocking`
- `cargo test -p meerkat-mcp --lib add_remove_add_discards_stale_generation`

## Freeze decision

`K5` is considered frozen for the exact current Meerkat cutover boundary.

What is frozen:

- live tool-surface lifecycle and snapshot publication
- the current machine-visible staged / pending / removal-timing semantics

What is explicitly not frozen:

- a second store-backed recovery story above the live router authority
- transport/cache internals beyond the published snapshot
