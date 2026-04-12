# Mob-Meerkat Composition Freeze

This note freezes the target composition proof surface between `MobMachine` and
`MeerkatMachine`.

The composition does **not** model the full internal product of both target
machines. Instead, it freezes the smaller claim we actually need for cutover:

- `MeerkatMachine` refines to a small lifecycle/work seam
- `MobMachine` depends only on that seam
- runtime supersession, stale events, and destroy / retire bookkeeping are
  handled inside the seam instead of leaking hidden Meerkat internals into Mob

## Scope

The composition model covers:

- identity registration, fresh provision, recovery, reset, retire, and destroy
- runtime supersession via runtime ID and fence token
- work submit / accept / reject / complete / fail / cancel
- stale lifecycle and stale work events
- one active flow-step projection carried by Mob
- the exact seam states Mob should need:
  - lifecycle: provisioning, ready, retiring, destroyed, failed
  - work: pending decision, accepted, terminal

The composition intentionally does **not** expose:

- Meerkat queue internals
- Meerkat epoch, cursor, or barrier internals
- peer ingress internals
- tool-surface internals
- detached-wake internals

Those remain proven inside the single-machine `MeerkatMachine` target.

## Current Status

Canonical proof lanes are green:

- base safety: `173` generated / `47` distinct / depth `8`
- widened safety: `589` generated / `152` distinct / depth `10`
- lifecycle liveness: `407` generated / `197` distinct / depth `15`
- flow/work liveness: `25` generated / `19` distinct / depth `13`

Review closeout:

- [mob-meerkat-composition-closeout.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/mob-meerkat-composition-closeout.md)
- [mob-meerkat-composition-package-audit.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/mob-meerkat-composition-package-audit.md)

## Canonical Model

Executable model:

- [MobMeerkatCompositionTarget.tla](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/tla/MobMeerkatCompositionTarget.tla)

Canonical configs:

- base safety: [MobMeerkatCompositionTarget.cfg](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/tla/MobMeerkatCompositionTarget.cfg)
- widened safety: [MobMeerkatCompositionTargetStress.cfg](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/tla/MobMeerkatCompositionTargetStress.cfg)
- lifecycle liveness:
  [MobMeerkatCompositionTargetLifecycleLiveness.cfg](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/tla/MobMeerkatCompositionTargetLifecycleLiveness.cfg)
- flow/work liveness:
  [MobMeerkatCompositionTargetFlowWorkLiveness.cfg](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/tla/MobMeerkatCompositionTargetFlowWorkLiveness.cfg)

## Key Proven Shape

The composition model now makes these seam claims explicit:

- a member may reprovision while a submit is still pending against the retiring
  runtime/fence; this is not a hidden contradiction
- stale work and lifecycle events are explicit seam states and are ignored
  rather than consumed as current truth
- `DestroyMember` cannot be reissued once the bridge has already destroyed the
  runtime and the system is waiting on the terminal event
- Mob flow-step progress depends only on seam-visible work outcomes, not on
  hidden Meerkat runtime internals

## Relationship To The Single-Machine Proofs

This composition freeze depends on:

- [meerkat-machine-freeze.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/meerkat-machine-freeze.md)
- [mob-machine-freeze.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/mob-machine-freeze.md)
- [abstract-member-contract.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/abstract-member-contract.md)
- [bridge-alphabet.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/bridge-alphabet.md)

Those seam docs are now frozen composition-facing artifacts rather than open
drafts.

It is the first proof surface for “together,” but it is still a seam model, not
the full implementation refinement.
