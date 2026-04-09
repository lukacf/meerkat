# Two-Kernel Cutover Gate

This document defines the decision point for moving from the current
observational two-kernel scaffolding to the actual replacement architecture.

It exists to answer one question precisely:

When do we stop refining the machine boundary, freeze semantics, do the TLA+
work, and then perform a hard cut to the new kernels?

## Current posture

Today:

- `MeerkatMachine` and `MobMachine` exist as joined diagnostic models over the
  current codebase.
- They read real owner state and validate cross-region invariants.
- They are not yet the canonical write authority for runtime execution.

This means we are still in the **observation and convergence** phase, not the
**ownership and cutover** phase.

## Non-goals for the gate

The gate is **not** reached simply because:

- a lot of diagnostic code exists
- many validators pass
- many historical bugs are represented
- the new machine shape feels cleaner than the old one

Those are useful signals, but they are not enough.

The gate is reached only when the **ownership model has stopped moving**.

## Semantic freeze criteria

The architecture is ready to freeze when all of the following are true.

### 1. Owner boundaries are stable

For several consecutive slices:

- no new semantic fact is discovered
- no existing fact changes canonical owner
- no major region has to move between `MeerkatMachine`, `MobMachine`, or
  perimeter/auxiliary machines

Allowed:

- parser cleanup
- read-path cleanup
- bypass removal
- stronger tests for already-agreed ownership

Not allowed:

- "this fact actually belongs to a different machine"
- "this lifecycle phase is not really machine truth"
- "this supposedly hidden state must be exposed at the boundary after all"

### 2. Full lifecycle is represented

For both kernels, the machine shape must cover the real lifecycle, including:

- initialization / startup
- normal steady-state execution
- cancellation
- retire / stop
- reset
- recovery / resume
- terminalization / destroy

This is important because most historical seam failures happened in handoff
states, not in steady-state transitions.

### 3. Shell actions map to machine actions

Every important runtime or orchestration action in scope must map to:

- a named machine input
- a named machine-owned effect
- or an explicitly out-of-scope auxiliary/perimeter domain

There must be no important semantic behavior that still exists only as:

- helper convention
- shell branch
- cache bookkeeping
- event-stream folklore
- unmodeled recovery glue

### 4. Historical bug families are machine-expressible

The in-scope historical bug classes must be representable as one of:

- machine invariants
- machine transition legality
- seam obligations
- recovery reconstruction invariants

If a known bug family cannot be stated in the machine language, the model is
not ready.

### 5. Remaining gaps are explicit

Everything not being modeled yet must be one of:

- deliberately out of scope
- delegated to an auxiliary machine
- or captured as a named follow-up gap

Nothing important should remain in the category of "we have not looked at this
yet but probably it is fine."

## TLA+ readiness criteria

Once semantic freeze is reached, the model is ready for formalization when the
following are true.

### 1. Kernel state is typed and readable from code

The machine state needed for proof is available through typed read paths, not
implicit `KernelValue` spelunking or shell reconstruction.

### 2. The proof boundary is explicit

The formal scope is written down as:

- `MeerkatMachine`
- `MobMachine`
- auxiliary/perimeter machines outside the proof core

For now, scheduling remains outside the two-kernel core unless explicitly
promoted.

### 3. Inputs and effects are named

The model has a stable alphabet:

- inputs
- effects
- hidden internal transitions
- seam obligations

If the implementation still needs to invent inputs ad hoc during lowering, the
alphabet is not ready.

### 4. We know what is exact vs target-state

Before proof work begins, every major item must be classified as one of:

- exact current implementation behavior
- intended replacement behavior
- temporary compatibility behavior to be deleted at cutover

This prevents proving a fantasy architecture instead of the actual cutover
target.

## Hard-cut prerequisites

Before switching write authority to the new kernels, all of these must hold.

### 1. Lowering exists

Real functions and runtime operations are wired to named machine inputs and
effects.

### 2. No dual semantic authority

There may be a brief shadow-read phase, but there must not be a long-lived
dual-write or dual-authority phase.

The old wiring must stop being semantically active once cutover begins.

### 3. Cut happens one kernel at a time

Recommended order:

1. hard-cut `MeerkatMachine`
2. stabilize and classify failures
3. hard-cut `MobMachine`

This keeps failure attribution tractable.

### 4. Failure triage is predefined

Every cutover failure must be classified as exactly one of:

- **Implementation detail**
  The machine is correct, but the lowering/adapter/recovery code is wrong.
- **Semantic gap**
  The machine is missing a fact, transition, or obligation.
- **Dogma violation**
  The failure exists because shadow truth, bypass mutation, or shell-owned
  meaning still survives somewhere.

If post-cut failures mostly land in implementation detail, the gate was likely
called correctly.

If they mostly land in semantic gap, the gate was called too early.

If they mostly land in dogma violation, the old architecture was not actually
removed.

## Recommended cutover sequence

1. Reach semantic freeze.
2. Write the TLA+ model for the frozen boundary.
3. Prove the safety and seam obligations in scope.
4. Build lowering from real functions into machine inputs/effects.
5. Run a short shadow-read/shadow-validate phase.
6. Hard-cut `MeerkatMachine` to single-write authority.
7. Triage and fix failures.
8. Hard-cut `MobMachine` to single-write authority.
9. Remove obsolete legacy wiring.

## Stop / go checklist

Do **not** begin TLA+ and hard-cut planning unless every item below is true:

- owner boundaries have been stable for multiple consecutive slices
- no recent backtrack changed canonical ownership
- startup, reset, recovery, cancel, retire, and terminalization are represented
- important shell actions map to machine inputs/effects
- in-scope historical bug families can be stated in machine terms
- remaining gaps are explicit and accepted
- auxiliary/perimeter domains are explicitly named
- typed state readers exist for proof-relevant facts
- the machine input/effect alphabet is stable

If any item above is false, continue the convergence phase.

## Practical reading of the gate

The gate is not "the machines look good."

The gate is:

"We are no longer learning new semantics by observing the code; we are now
mostly removing bypasses, tightening lowering, and preparing to replace the old
ownership graph with the new one."
