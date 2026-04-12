# Mob-Meerkat Composition Closeout

The Mob-Meerkat seam composition package is closed enough to support proof and
cutover review.

## What Is Closed

- target seam freeze:
  [mob-meerkat-composition-freeze.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/mob-meerkat-composition-freeze.md)
- proof handoff:
  [mob-meerkat-composition-proof-handoff.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/mob-meerkat-composition-proof-handoff.md)
- executable model:
  [MobMeerkatCompositionTarget.tla](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/tla/MobMeerkatCompositionTarget.tla)

## What Was Mechanically Checked

- base seam safety
- widened seam safety
- focused lifecycle liveness
- focused flow/work liveness

## What Would Reopen This Package

- Mob needs hidden Meerkat state to decide lifecycle or work semantics
- the bridge alphabet changes without matching composition transitions
- stale pending submit against a superseded runtime stops being valid target
  behavior
- destroy/reprovision bookkeeping changes at the seam

## What This Does Not Claim

This package does not claim full implementation refinement. It claims that the
target Mob and Meerkat machines can compose honestly through the smaller seam
they are supposed to share.
