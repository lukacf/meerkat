# Meerkat Cutover Checklist

Status: frozen exact-current-state checklist

This is the Meerkat-only execution checklist for moving from observational
`MeerkatMachine` scaffolding to Meerkat semantic freeze.

This checklist is now closed for the exact current `MeerkatMachine` freeze.
It remains useful as the audit trail that produced the implementation baseline.
The target-state machine is now frozen separately in
`meerkat-machine-freeze.md`.

Post-freeze alignment note:

- the current branch exact-current freeze still stands as written
- a rebase probe against `origin/main` showed concentrated upstream drift in
  the `tools` region
- that drift is recorded in `meerkat-upstream-tool-alignment.md`
- the next full rebase should therefore reopen `MeerkatMachine.tools`
  deliberately rather than treating those conflicts as generic merge noise

It is derived from:

- `cutover-gate.md`
- `meerkat-kernel-shape.md`
- `meerkat-owned-facts-ledger.md`
- the live convergence log in `build-progress.md`

It is intentionally concrete. Each item exists because it currently blocks one
or more stop/go checks in `cutover-gate.md`.

## Nomenclature

Top-level machine milestones use:

- `M1` = full `MeerkatMachine`
- `M2` = `MobMachine`

This checklist is **not** that top-level milestone list. To avoid collision,
the internal Meerkat freeze steps below use `K1`-`K10`.

## Reading rule

- `Done` means the area is converged enough that new slices are mostly
  tightening tests or removing bypasses.
- `In Progress` means the owner boundary looks right, but live behavior is
  still being learned or the machine action/effect story is not frozen.
- `Pending` means the area is still an explicit freeze blocker.

## Current Meerkat posture

What is already relatively converged:

- registered vs attached lifecycle coverage for `retire`, `stop`, `reset`,
  `recover`, `recycle`, and `destroy`
- split-lifetime ownership between input completion waiters and ops-owned
  `wait_all`
- plain and attached steer-lane lifecycle behavior
- `interrupt_current_run` on plain and attached runtimes
- runtime-adapter `InterruptYielding` delivery, with no separate current
  session-layer interrupt lowering
- exact-current-state interrupt freeze note in `meerkat-interrupt-freeze.md`
- exact-current-state detached-wake freeze note in `meerkat-detached-wake-freeze.md`
- widened post-rebase validation is green again, including workspace-wide lib,
  test, test-check, and all-target compile lanes, so merge fallout is no
  longer a standalone freeze blocker
- resurrected wait-interrupt builder/binder API has been removed again, so the
  interrupt boundary is back to the real runtime/session seams only
- final adversarial rebase audit is clean: no remaining conflict markers
  outside `specs/**`, no resurrected source surfaces left in code, and only
  the expected observational machine warnings remain
- remaining compatibility/fallback markers have been classified as intentional
  current-state product seams (`drain_peer_input_candidates`, classified-inbox
  raw channel vestige, detached-wake legacy fallback), not as merge fallout
- settled teardown behavior for `current_run_id`, queue/steer visibility, and
  completion-waiter teardown across most lifecycle families

What no longer blocks semantic freeze:

- turn/ops/barrier coupling now has an exact-current freeze note
- peer-ingress lifecycle is frozen as a live queue/authority seam, including
  the explicit absence of durable queue replay
- the rebased tools region is now frozen as:
  - durable `tool_visibility`
  - live router-authority `tool_surface`
- drain / keep-alive lifecycle is frozen as the live adapter-owned seam
- the Meerkat input/effect alphabet exists
- the Meerkat lowering map exists
- remaining ownership decisions are explicit

Current `K1` read:

- `K1` is frozen for the exact current Meerkat boundary
- the exact-current-state asset is `meerkat-interrupt-freeze.md`
- the asset is review-ready and carries its own verification lane and reopen
  conditions
- there is no distinct current `meerkat-session` interrupt seam inside this
  frozen boundary
- `cancel_after_boundary` is explicitly classified there as a live
  runtime/session boundary-cancel lowering

Current `K2` read:

- `K2` is frozen for the exact current Meerkat boundary
- the exact-current-state asset is `meerkat-detached-wake-freeze.md`
- feed-backed detached wake is the canonical live path for registered runtimes
- legacy `DetachedWakeState` behavior is explicitly frozen as compatibility
  fallback behavior

## Checklist

| ID | Item | Status | Why it blocks freeze | Exit criteria |
| --- | --- | --- | --- | --- |
| K1 | Freeze interrupt and cancel semantics | Done | Exact current interrupt/cancel semantics are now frozen, including the live `cancel_after_boundary` lowering | `meerkat-interrupt-freeze.md` exists; plain and attached runtime proofs exist for `interrupt_current_run`; runtime-adapter `InterruptYielding` delivery is captured; no distinct current session-layer interrupt lowering is discovered; `cancel_after_boundary` now has a live runtime/session lowering |
| K2 | Freeze detached-wake and continuation interaction | Done | Exact current detached-wake and continuation behavior is now frozen, including canonical feed-backed behavior and the legacy compatibility fallback | `meerkat-detached-wake-freeze.md` exists; feed-backed idle/post-drain continuation injection has direct machine proofs; non-quiescent defer behavior is captured; completion-kind filtering is captured; legacy `DetachedWakeState` behavior is explicitly classified as compatibility fallback |
| K3 | Freeze turn / ops / barrier coupling | Done | Barrier satisfaction and turn/ops coupling are now frozen as exact-current live semantics | `meerkat-turn-ops-barrier-freeze.md` exists; authority proofs, live runner lowering, and joined validator proofs exist |
| K4 | Freeze peer-ingress lifecycle and recovery | Done | Peer-ingress lifecycle is now frozen as the exact current live queue/authority seam, including the explicit absence of durable queue replay | `meerkat-peer-ingress-freeze.md` exists; trust/classification/runtime-snapshot proofs exist; exact-current recovery classification is explicit |
| K5 | Freeze external tool visibility / surface lifecycle and recovery | Done | The rebased exact-current tools region is now frozen as durable `tool_visibility` plus live router-authority `tool_surface` | `meerkat-tool-visibility-freeze.md` exists; `meerkat-tool-surface-freeze.md` exists; staged/apply/finalize/router proofs and joined Meerkat proof exist; current exact-current ownership split is explicit |
| K6 | Freeze drain and keep-alive lifecycle | Done | Drain lifecycle is now frozen as the exact current live adapter-owned seam | `meerkat-drain-freeze.md` exists; adapter, joined Meerkat, and seam-classification proofs exist |
| K7 | Write and freeze the Meerkat input/effect alphabet | Done | The exact current Meerkat alphabet now exists | `meerkat-input-effect-alphabet.md` exists and explicitly classifies live inputs/effects vs excluded target-state verbs |
| K8 | Write the Meerkat lowering map | Done | The exact current lowerings from real code into machine inputs/effects are now explicit | `meerkat-lowering-map.md` exists and maps the live code paths |
| K9 | Freeze remaining open ownership questions | Done | The remaining ownership questions are now explicitly decided for the exact current boundary | `meerkat-ownership-decisions.md` exists |
| K10 | Final Meerkat freeze review | Done | The exact current Meerkat freeze set now exists as one coherent review-ready asset | `meerkat-machine-exact-current-freeze.md` exists; focused and widened verification lanes are documented; exact-vs-target-state classification is explicit |

## Next active step

The next active step is no longer Meerkat freeze closure.

The next active step is:

1. use `meerkat-machine-exact-current-freeze.md` as the implementation baseline
2. use `meerkat-machine-freeze.md` as the target-state machine asset
3. use `meerkat-machine-proof-obligations.md` as the proof handoff package
4. extend the existing target-state TLC scaffold in `tla/`, which now passes
   bounded base and stress TLC runs, into the full proof model
5. review implementation-vs-target deltas before any write-side switch planning

If the next full rebase is attempted before cutover work, use:

- `meerkat-upstream-tool-alignment.md`
- `meerkat-tool-visibility-upstream-baseline.md`
- `meerkat-tools-realignment-plan.md`
- `meerkat-tools-merge-strategy.md`

as the machine-led source of truth for the tools region.
