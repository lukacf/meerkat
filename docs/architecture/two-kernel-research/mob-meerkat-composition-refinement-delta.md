# Mob-Meerkat Composition Refinement Delta

Status: active pre-cutover refinement note

This note records the remaining gap between the frozen target seam in
`mob-meerkat-composition-freeze.md` and the rebased exact-current branch.

It is not a statement that the seam freeze is wrong. It is the checklist of
what still needs explicit implementation lowering before the cutover can be
called honest.

## Why this note exists

The composition freeze already proves the target seam:

- Mob depends only on lifecycle/work seam state
- Meerkat internal queues, barriers, epochs, peer internals, and tool internals
  do not leak into Mob semantics

The exact-current branch still reaches parts of that seam through lower-level
helpers and product-specific bindings. This note isolates those remaining
lowering gaps.

## Remaining exact-current leaks

### 1. Session identity still appears below the seam

The target seam is keyed by runtime identity and fence token.

Exact current implementation still routes important paths through session-bound
services and helpers, which means `SessionId` remains part of the concrete
lowering even where the target seam would rather speak only in terms of member
runtime identity.

This is an implementation refinement gap, not a target-machine uncertainty.

### 2. Work lowerings still ride raw turn-delivery helpers

The target seam speaks in terms of abstract work submit / accept / reject /
terminal transitions.

Exact current implementation still reaches that seam through concrete helpers
such as internal turn start and flow-step start paths. Those helpers are valid
today, but they are still lower than the frozen seam alphabet.

The cutover needs an explicit lowering map from those helpers into canonical
seam work verbs.

### 3. Lifecycle lowerings are still helper-rich

Provision, recover, reset, retire, and destroy are all frozen at the seam.

Exact current implementation still spreads those through service, session, and
runtime helper layers. The semantic contract is already frozen, but the
implementation-to-seam projection is not yet centralized enough to call the
seam directly cutover-ready.

### 4. Fence / supersession truth is still split across concrete paths

The seam correctly models supersession via runtime identity plus fence token.

Exact current implementation still reconstructs parts of that behavior from a
mix of runtime state, service-level membership, and in-flight task bookkeeping.
That needs one explicit refinement map during cutover work.

## What does not need reopening

This delta does **not** reopen the composition freeze.

It does **not** claim:

- the seam alphabet is wrong
- Mob needs hidden Meerkat state
- Meerkat internal machine regions need to be exposed to Mob
- the target composition proof should be weakened

The target seam stays frozen. This note is only about implementation lowering.

## Cutover consequence

The next cutover-facing work for the seam should be:

1. write the exact-current implementation -> seam lowering map
2. centralize fence/runtime identity projection
3. route work lowerings through named seam verbs
4. prove the real implementation shadows the frozen seam under those lowerings

Until then, the seam is proven as a target abstraction, but not yet proven as a
full implementation refinement.
