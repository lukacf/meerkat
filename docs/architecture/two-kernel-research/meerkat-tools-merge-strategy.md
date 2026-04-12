# Meerkat Tools Merge Strategy

Status: active rebase strategy

This note turns the tool-region alignment work into a concrete merge strategy
for the next full rebase onto `origin/main`.

The aim is to restore a fully functioning, honestly frozen `MeerkatMachine`
without treating the tools seam as generic conflict noise.

## Why This Exists

The upstream baseline and the current branch are now converging on the same
ownership problem from different directions:

- the current branch has a frozen `tool_surface` exact-current story
- `origin/main` has already moved toward explicit durable `tool_visibility`
  ownership and exact-catalog support

That means the next rebase needs file-by-file source-of-truth rules.

## Merge Principle

The merge should preserve three things simultaneously:

1. upstream durable `tool_visibility` ownership
2. current-branch machine/proof vocabulary for `tool_surface`
3. a clean separation between exact-current rebased baseline and target-state
   `MeerkatMachine.tools`

In other words:

- take upstream where it introduces the richer live ownership seam
- retain current-branch machine artifacts where they are observational scaffolds
  or proof assets
- do not force old branch assumptions back over richer upstream ownership

## File-By-File Source Of Truth

### `meerkat-core/src/tool_scope.rs`

Primary source of truth: `origin/main`

Reason:

- upstream already carries `SessionToolVisibilityState`
- upstream already carries `ToolVisibilityWitness`
- upstream `ToolScope` is already closer to the projection role we want

Required preservation from current branch:

- any observational snapshot types or helpers still needed by the current
  exact-current machine scaffolding must be reintroduced only if they can be
  expressed as projections over the upstream owner state
- do not restore old branch assumptions that `ToolScope` is the durable owner

Target merge posture:

- `ToolScope` becomes the live projection / bridge over durable visibility
  state plus current tool-surface projection
- diagnostic snapshot helpers are permitted, but only as derived readers

### `meerkat-core/src/gateway.rs`

Primary source of truth: `origin/main`

Reason:

- upstream exact-catalog support is already present
- current branch does not yet have the richer catalog vocabulary here

Required preservation from current branch:

- keep compatibility with current machine docs only by updating docs, not by
  stripping exact-catalog support back out

Target merge posture:

- gateway exact-catalog support becomes part of the rebased exact-current
  baseline
- machine docs should follow the merged gateway semantics

### `meerkat-mcp/src/adapter.rs`

Primary source of truth: `origin/main`

Reason:

- upstream already carries exact catalog caching and pending catalog-source
  projection
- current branch still models the adapter mainly as surface-snapshot transport

Required preservation from current branch:

- keep the current branch's machine-level understanding that the adapter is not
  the semantic owner; it is a projection seam over router/tool visibility
  ownership

Target merge posture:

- adopt upstream exact-catalog and pending-source behavior
- re-check the exact-current freeze so the adapter is modeled as projection,
  not as a second owner

### `meerkat-session/src/ephemeral.rs`

Primary source of truth: `origin/main`

Reason:

- upstream session-task mutation seam for visibility state already exists
- this is the live mutation seam the rebased exact-current machine should use

Required preservation from current branch:

- any diagnostic snapshot forwarding surfaces still needed by the machine work
  should be restored or relocated only if they are still exercised and still
  honest under the new ownership split

Target merge posture:

- accept upstream `SetToolVisibilityState` seam
- re-evaluate whether old diagnostic snapshot commands should survive here, or
  whether the rebased machine package should observe the richer owner surfaces
  elsewhere

### `meerkat-core/src/lib.rs`

Primary source of truth: merged result

Reason:

- likely export fallout from the upstream seam

Target merge posture:

- keep exported types needed by the richer exact-current tools story
- do not preserve exports only because the old branch used them once

### `examples/035-mdm-tux-rs/src/bin/target.rs`

Primary source of truth: `origin/main`

Reason:

- peripheral example fallout
- not a machine source of truth

## Merge Sequence

1. Rebase again and stop on the first conflict wave.
2. Resolve `tool_scope.rs` by taking upstream ownership and reintroducing only
   projection-level machine helpers.
3. Resolve `gateway.rs` and `adapter.rs` in favor of upstream exact-catalog
   semantics.
4. Resolve `ephemeral.rs` in favor of the upstream session-task visibility
   mutation seam.
5. Only after those are merged, revisit exact-current Meerkat docs:
   - `meerkat-machine-exact-current-freeze.md`
   - `meerkat-tool-surface-freeze.md`
   - `meerkat-machine-freeze.md`
   - `meerkat-input-effect-alphabet.md`
   - `meerkat-lowering-map.md`
6. Re-run Meerkat proof/package lanes.
7. Re-check Mob and seam packages.

## Success Criteria

The merge is not complete when the conflicts are gone.

It is complete only when:

- rebased exact-current Meerkat docs describe the merged tools seam honestly
- target `MeerkatMachine.tools` has been re-reviewed against the new baseline
- Meerkat freeze/proof lanes are green
- Mob and composition lanes are green
- no tool-region assumption in docs still depends on the pre-upstream
  router-only ownership story
