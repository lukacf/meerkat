# Meerkat Tool-Visibility Freeze

Status: frozen exact-current-state asset

This note extends `K5` from `meerkat-cutover-checklist.md` after the successful
rebase onto `origin/main`.

It captures the exact current live Meerkat semantics for the
`tool_visibility` side of the `tools` region as it now exists on the rebased
branch.

This note is exact-current, not target-state.

## Scope

This freeze covers:

- durable session-owned tool visibility state through
  `SessionToolVisibilityState`
- visibility witnesses through `ToolVisibilityWitness`
- `ToolScope` as a live projection bridge over durable visibility state
- exact-catalog capability and catalog projection at the gateway / MCP adapter
  seam
- explicit session-task visibility mutation through
  `SessionCommand::SetToolVisibilityState`
- missing requested and missing filter names as current live projection facts

This freeze does **not** claim:

- the full target-state transactional publication contract for provider schema
  generation and dispatch gating
- that exact catalog is globally proven across every possible dispatcher shape
- that the current branch already implements the full deferred-catalog
  ownership refactor target

## Exact current semantics

### 1. Durable visibility owner now exists in the live branch

Exact current live behavior:

- `ToolScopeState` now owns `durable_state: SessionToolVisibilityState`
- durable state carries:
  - `inherited_base_filter`
  - `active_filter`
  - `staged_filter`
  - `active_requested_deferred_names`
  - `staged_requested_deferred_names`
  - `active_revision`
  - `staged_revision`
  - requested and filter witnesses

This means the rebased branch no longer has only a router-snapshot tools seam.

### 2. `ToolScope` is now a projection bridge, not only a staged filter shell

Exact current live behavior:

- `ToolScope` still owns live projection behavior for the active turn
- `ToolScope` now also exposes durable-visibility bridge methods such as:
  - `set_visibility_state(...)`
  - `visibility_state()`
  - `visible_tool_names()`
  - `missing_requested_names()`
  - `missing_filter_names()`
  - staged requested deferred-name mutation

That means the exact-current branch already carries a partial
`tool_visibility + tool_surface` split internally, even though the target
machine may still refine it further.

### 3. Exact catalog is now part of the current dispatcher seam

Exact current live behavior:

- `ToolGateway` exposes `tool_catalog_capabilities()` and `tool_catalog()`
- `McpRouterAdapter` exposes exact catalog support and pending catalog sources
- the exact-current branch now has a real catalog/callability seam, not only a
  visible-tools seam

This is current implementation truth on the rebased branch, not only an
upstream note.

### 4. Session-task visibility mutation seam is current live behavior

Exact current live behavior:

- `EphemeralSessionService` now has
  `set_session_tool_visibility_state(...)`
- the live session command surface now includes
  `SessionCommand::SetToolVisibilityState`

That means visibility mutation is no longer only helper folklore. It has an
explicit current live mutation seam.

### 5. Missing intent is currently preserved as projection truth

Exact current live behavior:

- `missing_requested_names()` reports requested deferred names absent from the
  current base snapshot
- `missing_filter_names()` reports durable filter names absent from the current
  base snapshot

This does not yet prove the full target dormant-intent publication story, but
it is exact-current live branch behavior.

## Exact-vs-target-state classification

What is frozen as exact current live behavior:

- durable session-owned visibility state exists
- visibility witnesses exist
- exact catalog exists at gateway / MCP adapter seams
- session-task visibility mutation exists
- missing requested and missing filter names are current live projection facts

What is explicitly **not** frozen as exact current live behavior:

- full target-state transactional publication of one committed visible set to
  both provider schema generation and dispatch gating
- global proof that every dispatcher composition preserves exact catalog
- final target-state split of `MeerkatMachine.tools` into explicit
  `tool_visibility` and `tool_surface` subregions in the TLA model

## Proof / verification inventory

Current exact-current evidence comes from:

- `ToolScope` snapshot / projection tests in `meerkat-core/src/tool_scope.rs`
- exact-catalog gateway tests in `meerkat-core/src/gateway.rs`
- exact-catalog / pending-source adapter tests in `meerkat-mcp/src/adapter.rs`
- live session-task visibility mutation seam in
  `meerkat-session/src/ephemeral.rs`
- post-rebase machine compile/test/proof verification on the rebased branch

## Freeze decision

The rebased exact-current `MeerkatMachine.tools` region is now best read as:

- `tool_visibility`
- `tool_surface`

Both are part of the current exact-current Meerkat boundary.

The target machine may still strengthen or further split that region, but the
exact-current branch no longer honestly supports the older story that `K5` is
only router authority plus staged filter projection.
