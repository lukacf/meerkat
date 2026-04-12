# Mob-Meerkat Composition Proof Handoff

This handoff records what the current composition proof surface establishes and
what it deliberately leaves to later refinement work.

Package closeout:

- [mob-meerkat-composition-closeout.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/mob-meerkat-composition-closeout.md)
- [mob-meerkat-composition-package-audit.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/mob-meerkat-composition-package-audit.md)
- [mob-meerkat-composition-refinement-delta.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/mob-meerkat-composition-refinement-delta.md)

## Canonical Proof Lanes

- base safety:
  [MobMeerkatCompositionTarget.cfg](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/tla/MobMeerkatCompositionTarget.cfg)
- widened safety:
  [MobMeerkatCompositionTargetStress.cfg](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/tla/MobMeerkatCompositionTargetStress.cfg)
- lifecycle liveness:
  [MobMeerkatCompositionTargetLifecycleLiveness.cfg](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/tla/MobMeerkatCompositionTargetLifecycleLiveness.cfg)
- flow/work liveness:
  [MobMeerkatCompositionTargetFlowWorkLiveness.cfg](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/tla/MobMeerkatCompositionTargetFlowWorkLiveness.cfg)

Observed bounded results:

- base safety: `173` generated / `47` distinct / depth `8`
- widened safety: `589` generated / `152` distinct / depth `10`
- lifecycle liveness: `407` generated / `197` distinct / depth `15`
- flow/work liveness: `25` generated / `19` distinct / depth `13`

## What These Proofs Establish

- Mob depends only on lifecycle/work seam state, not hidden Meerkat internals.
- Runtime supersession by fence token is a first-class seam phenomenon.
- Pending submit may legally target a retiring runtime while a replacement
  runtime is provisioning.
- Stale lifecycle/work events are explicitly representable and removable.
- `DestroyMember` cannot be reissued once a destroy is already in-flight.
- Flow-step progress can be driven entirely by seam-visible work transitions.

## What Is Still Deferred

- full implementation refinement from the joined exact-current snapshots to this
  seam abstraction
- composition with richer multi-step / multi-member concurrent flow surfaces
- proof that all Meerkat internal temporal guarantees refine the abstract seam
  liveness assumptions

The active implementation-facing gap list is recorded in:

- [mob-meerkat-composition-refinement-delta.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/mob-meerkat-composition-refinement-delta.md)

## Reopen Conditions

Reopen this composition freeze if any of the following happens:

- Mob needs hidden Meerkat state to make a semantic decision
- the bridge alphabet grows in a way the composition model does not represent
- fence-superseded pending submit is no longer legal target behavior
- destroy / retire bookkeeping semantics change at the seam
