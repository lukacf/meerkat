# Meerkat Tools Target Delta

Status: active target-state realignment note

This note records the likely target-state changes to `MeerkatMachine.tools`
after absorbing the richer upstream tools ownership model.

It does **not** declare the target region re-frozen yet. It exists so the next
rebase and re-freeze can distinguish:

- what is already frozen target-state Meerkat design
- what the upstream tools baseline strongly suggests the target region must grow
  to include

## Why This Exists

The current target `MeerkatMachine.tools` region is intentionally compact:

- known surfaces
- visible surfaces
- staged intents
- pending mutation lineage
- inflight call counts
- published snapshot alignment

That was a reasonable target before upstream grew a richer live ownership seam.
After the rebase probe, it is now likely too small.

## Candidate Target Growth

The target `tools` region should be reviewed as two tightly coupled internal
subregions:

1. `tool_visibility`
2. `tool_surface`

They still belong to one top-level `MeerkatMachine.tools` region, but they no
longer fit comfortably into one undifferentiated state bucket.

## `tool_visibility`

Likely target machine state:

- durable visibility owner state
- inherited/base filter truth
- active visibility filter
- staged visibility filter
- active requested deferred names
- staged requested deferred names
- active revision
- staged revision
- missing requested names
- missing filter names
- control-plane tool-name set
- deferred-eligible name set
- visibility witnesses / stable-owner proofs
- exact catalog capability and exactness downgrade state

Likely target machine effects:

- `PublishCommittedVisibleSet`
- `PublishCatalogProjection`
- `EmitVisibilityRevisionChanged`

The exact-current branch now has an explicit live lowering for
`PublishCommittedVisibleSet` through `Agent::publish_committed_visible_set()`.
What still remains here is any broader unification of provider publication and
dispatch gating into one fully centralized tools-region authority seam.

Likely target machine invariants:

- provider schema generation and dispatch-time gating observe the same committed
  visible set at one committed revision
- dormant missing intent is preserved until a canonical visible winner or
  explicit deletion changes it
- policy-hidden names do not survive into visible or deferred-eligible output
- visibility witnesses do not upgrade unstable ownership into durable deferred
  intent

## `tool_surface`

Likely target machine state, still mostly aligned with the current freeze:

- known surfaces
- visible surfaces
- base state
- staged surface mutation intent
- pending mutation lineage
- inflight call counts
- last-delta operation / phase
- publication alignment

Likely target machine effects:

- `PublishToolSurfaceSnapshot`
- `EmitExternalToolUpdate`

Likely target machine invariants:

- staged surface state cannot outrun the last applied boundary
- visible surface publication reflects one committed state
- inflight calls only exist for compatible base states
- surface removal timing exists iff base state is `Removing`

## New Target Inputs To Review

The target alphabet should be reviewed for explicit tools-region inputs along
the following lines:

- `ReplaceToolVisibilityState`
- `StageVisibilityFilter`
- `RequestDeferredTools`
- `ApplyVisibilityBoundary`
- `RefreshCatalogProjection`
- `ExactCatalogDowngraded`

These names are provisional; the important point is that the target machine
should stop relying on implicit visibility/publication behavior.

## What This Note Is Not Claiming

This note does not claim the current target freeze is invalid.

It claims something narrower:

- the current target freeze still names the right top-level region
- the internal `tools` state is likely underspecified relative to the upstream
  ownership move
- the next re-freeze should revisit `tools` as `tool_visibility +
  tool_surface`, not just expand the old surface-only story ad hoc
