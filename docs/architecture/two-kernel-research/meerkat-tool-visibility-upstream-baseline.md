# Meerkat Tool-Visibility Upstream Baseline

Status: active upstream-baseline note

This note records the current `origin/main` implementation baseline for the
`MeerkatMachine.tools` visibility subregion.

It does **not** claim that the current branch already has this behavior.
It exists so the next rebase can be machine-led instead of conflict-led.

## Why This Exists

The rebase probe showed that upstream has materially advanced the same
ownership surface our machine work had only partially frozen:

- durable session-owned tool visibility state
- exact catalog capabilities and canonical catalog projection
- control-plane vs session-plane tool separation
- explicit session-task mutation seams for visibility state

The old exact-current `K5` freeze on this branch is still honest for this
branch, but it is too coarse to serve as the upstream baseline.

## Upstream Ownership Read

### Durable owner exists

`origin/main` `meerkat-core/src/tool_scope.rs` now carries:

- `SessionToolVisibilityState`
- `ToolVisibilityWitness`
- `ToolScopeState.durable_state`
- `ToolScopeState.control_tool_names`
- `ToolScopeState.deferred_tool_names`

That means upstream no longer treats the tools region as only a router snapshot
plus a staged filter. Durable visibility ownership is already in the runtime
surface.

### ToolScope is already more projection-like

Upstream `ToolScope` now exposes behavior that is closer to a live projection /
bridge than a simple durable owner:

- `new_with_projection_names(...)`
- `set_visibility_state(...)`
- `visibility_state()`
- `visible_tool_names()`
- `missing_requested_names()`
- `missing_filter_names()`
- staged requested deferred-name support

That is strong evidence that `ToolScope` is already being pulled toward the
projection role described in the deferred-catalog refactor direction.

### Exact catalog is entering the dispatcher seam

Upstream `meerkat-core/src/gateway.rs` and `meerkat-mcp/src/adapter.rs` now
carry:

- `ToolCatalogCapabilities`
- `ToolCatalogEntry`
- exact-catalog support in the gateway path
- exact-catalog support in the MCP adapter path
- cached catalog and pending source projection

This means upstream has already started moving the callable-winner / exact
catalog story into the machine-adjacent tool layer.

### Session-task mutation seam exists

Upstream `meerkat-session/src/ephemeral.rs` now includes:

- `SessionCommand::SetToolVisibilityState`
- `SessionAgent::set_tool_visibility_state(...)`
- `set_session_tool_visibility_state(...)`

This matters because the mutation seam is no longer just inferred from helper
calls. A live session-task control seam now exists in the code.

## Machine Consequences

### Exact-current freeze needs two tools subregions

Against the upstream baseline, the old single `K5` tool-surface story must be
split conceptually into:

1. `tool_visibility`
   - durable visibility state
   - visibility witnesses
   - staged / active revision ownership
   - missing requested / missing filter names
   - session-task visibility mutation seam
   - control-plane vs session-plane naming inputs

2. `tool_surface`
   - router authority snapshot
   - staged add / remove / reload lifecycle
   - inflight-call / removal-timing semantics
   - published visible surface projection

### Target tools region likely needs to grow

The current target `MeerkatMachine.tools` region is probably too small for the
upstream baseline.

The next target review should assume the region may need explicit state for:

- durable visibility owner state
- exact catalog capability and exactness downgrade
- control-plane vs session-plane split
- committed published revision shared by schema generation and dispatch gating
- dormant missing intent
- ownership witnesses for durable requested/filter names

Whether each item becomes first-class machine state or a derived predicate
should be decided during the re-freeze pass, not guessed during merge.

## Rebase Guidance

The next successful rebase should treat these files as ownership alignment work:

- `meerkat-core/src/tool_scope.rs`
- `meerkat-core/src/gateway.rs`
- `meerkat-mcp/src/adapter.rs`
- `meerkat-session/src/ephemeral.rs`

The rebase should only be considered complete when:

- the exact-current Meerkat tools story is re-frozen honestly on the rebased
  branch
- the target `MeerkatMachine.tools` region is rechecked against that baseline
- Meerkat proof artifacts are rerun and still coherent
- Mob and composition proofs remain intact

## What This Note Is Not Claiming

This note does not claim that the current branch freeze is invalid.

It claims something narrower:

- the current branch freeze is still honest for this branch
- upstream has moved enough that the next rebase must absorb a richer tools
  ownership model before the machine can be considered aligned again
