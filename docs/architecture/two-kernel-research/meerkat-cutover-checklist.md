# Meerkat Cutover Checklist

Status: active working checklist

This is the Meerkat-only execution checklist for moving from observational
`MeerkatMachine` scaffolding to Meerkat semantic freeze.

It is derived from:

- `cutover-gate.md`
- `meerkat-kernel-shape.md`
- `meerkat-owned-facts-ledger.md`
- the live convergence log in `build-progress.md`

It is intentionally concrete. Each item exists because it currently blocks one
or more stop/go checks in `cutover-gate.md`.

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
- runtime-adapter `InterruptYielding` delivery vs session-layer cooperative
  interrupt behavior
- exact-current-state `M1` freeze note in `meerkat-m1-freeze.md`
- exact-current-state `M2` freeze note in `meerkat-m2-freeze.md`
- settled teardown behavior for `current_run_id`, queue/steer visibility, and
  completion-waiter teardown across most lifecycle families

What still blocks semantic freeze:

- turn/ops/barrier coupling on the live path
- peer-ingress lifecycle and recovery depth
- tool-surface lifecycle and recovery depth
- drain/keep-alive cutover semantics
- stable Meerkat input/effect alphabet
- lowering from real functions into Meerkat machine inputs/effects

Current `M1` read:

- `M1` is frozen for the exact current Meerkat boundary
- the exact-current-state asset is `meerkat-m1-freeze.md`
- the asset is review-ready and carries its own verification lane and reopen
  conditions
- `cancel_after_boundary` is explicitly classified there as an authority-local
  or target-state verb until a real lowering exists

Current `M2` read:

- `M2` is frozen for the exact current Meerkat boundary
- the exact-current-state asset is `meerkat-m2-freeze.md`
- feed-backed detached wake is the canonical live path for registered runtimes
- legacy `DetachedWakeState` behavior is explicitly frozen as compatibility
  fallback behavior

## Checklist

| ID | Item | Status | Why it blocks freeze | Exit criteria |
| --- | --- | --- | --- | --- |
| M1 | Freeze interrupt and cancel semantics | Done | Exact current interrupt/cancel semantics are now frozen, and `cancel_after_boundary` is explicitly classified as not yet lowered into the live Meerkat boundary | `meerkat-m1-freeze.md` exists; plain and attached runtime proofs exist for `interrupt_current_run`; runtime-adapter `InterruptYielding` delivery is captured; session-layer cooperative interrupt path is captured; `cancel_after_boundary` is explicitly classified out of exact-current cutover scope until a lowering exists |
| M2 | Freeze detached-wake and continuation interaction | Done | Exact current detached-wake and continuation behavior is now frozen, including canonical feed-backed behavior and the legacy compatibility fallback | `meerkat-m2-freeze.md` exists; feed-backed idle/post-drain continuation injection has direct machine proofs; non-quiescent defer behavior is captured; completion-kind filtering is captured; legacy `DetachedWakeState` behavior is explicitly classified as compatibility fallback |
| M3 | Freeze turn / ops / barrier coupling | Pending | Barrier satisfaction and turn/ops coupling are still under-modeled at the joined runtime level | Live proofs connect operation registry truth, barrier satisfaction, and turn terminalization without relying on non-atomic snapshot folklore |
| M4 | Freeze peer-ingress lifecycle and recovery | Pending | Peer state is visible, but full lifecycle, recovery, and post-admission ownership are not yet frozen enough for cutover | Trust mutation, classification, typed peer submission, dropped/delivered states, and recovery all have explicit machine-backed proofs |
| M5 | Freeze external tool-surface lifecycle and recovery | Pending | Tool-surface state is visible and partially validated, but staged/recovery/cutover semantics are not yet complete enough for freeze | Staged add/remove/reload, draining removal, snapshot publication, and recovery/reconciliation all have explicit machine-backed proofs |
| M6 | Freeze drain and keep-alive lifecycle | Pending | Drain state is visible, but not yet tied tightly enough to lifecycle cutover semantics | Spawn, stop, exit, abort, respawn, and suppression semantics are all represented as current owner truth |
| M7 | Write and freeze the Meerkat input/effect alphabet | Pending | The gate still fails on “shell actions map to machine actions/effects” | A single Meerkat input/effect document exists, and every important shell/runtime action in scope maps to a named machine input or effect |
| M8 | Write the Meerkat lowering map | Pending | No cutover without real lowering from code into machine authority | Real functions, adapter entry points, and runtime loop edges are mapped to machine inputs/effects and compatibility shims are explicit |
| M9 | Freeze remaining open ownership questions | Pending | A few Meerkat ledger questions still leave room for semantic motion | Completion-waiter subregion vs carrier, raw peer lineage retention, tool projection scope, and hidden epoch policy are explicitly decided or explicitly deferred out of cutover scope |
| M10 | Final Meerkat freeze review | Pending | We need an explicit “go/no-go” point before TLA+ and switch prep | Several consecutive slices with no owner-boundary movement, no semantic backtracks, stable alphabet, explicit remaining gaps, and a documented exact-vs-target-state classification |

## Immediate execution order

1. `M3` Freeze turn / ops / barrier coupling
2. `M4` Freeze peer-ingress lifecycle and recovery
3. `M5` Freeze external tool-surface lifecycle and recovery
4. `M6` Freeze drain and keep-alive lifecycle
5. `M7` Write and freeze the Meerkat input/effect alphabet
6. `M8` Write the Meerkat lowering map
7. `M9` Freeze remaining open ownership questions
8. `M10` Final Meerkat freeze review

## Notes on pace

The checklist is intentionally more aggressive than the earlier convergence
phase.

That does **not** mean skipping backtracks. It means:

- choosing higher-leverage freeze blockers first
- preferring slices that remove uncertainty from the gate
- rejecting pretty-but-false stories faster
- moving toward machine-owned actions/effects, not only read-side observation
