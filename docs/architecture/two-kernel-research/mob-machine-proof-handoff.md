# MobMachine Proof Handoff

Status: frozen target-state proof handoff

This note is the final reviewer-facing handoff for `MobMachine`.

It distinguishes:

- what the target machine is
- what bounded TLC has already validated
- what wider exploratory TLC has exercised without failure
- what still belongs to the next proof phase rather than to freeze hardening

Use together with:

- `mob-machine-freeze.md`
- `mob-machine-freeze-closeout.md`
- `mob-machine-package-audit.md`
- `mob-machine-self-containment-audit.md`
- `mob-machine-traceability-audit.md`
- `mob-machine-state-schema.md`
- `mob-machine-transition-catalog.md`
- `mob-machine-proof-obligations.md`
- `mob-machine-proof-coverage.md`
- `tla/MobMachineTarget.tla`

## Target Machine

The canonical target machine is:

- `mob-machine-freeze.md`

The canonical executable scaffold is:

- `tla/MobMachineTarget.tla`

The exact-current implementation comparison baseline is:

- `mob-machine-exact-current-freeze.md`
- `mob-machine-refinement-map.md`
- `mob-machine-refinement-delta.md`

## What Is Already Validated

### Bounded TLC, passing

`MobMachineTarget.cfg`

- bounded safety pass succeeds
- no invariant violations
- no transition-catalog mismatch

`MobMachineTargetStress.cfg`

- widened bounded safety pass succeeds
- no invariant violations
- no transition-catalog mismatch

These are the canonical passing model checks for the frozen target package.

Named temporal properties now exist in `MobMachineTarget.tla` for:

- destroyed-state absorption
- eventual pending-spawn clearance
- eventual kickoff clearance
- accepted work eventually leaves `Accepted`
- running work eventually terminalizes
- pending steps eventually leave `Pending`
- dispatched steps eventually leave `Dispatched`

The focused canonical liveness harnesses now pass for:

- lifecycle / destroyed-state absorption
- provisioning / kickoff progress
- recovery failure clearance
- task lifecycle closure
- work-ledger progress
- flow-step progress

Those harnesses are now the reviewer-facing proof baseline for the frozen
target machine.

## What Has Been Explored Beyond The Canonical Envelope

`MobMachineTargetAudit.cfg`

- same target machine
- same invariant set
- widened step bound (`MaxSteps = 10`)
- intentionally exploratory rather than canonical

The audit run explored far beyond the normal stress envelope and was manually
stopped only because state growth was continuing strongly:

- `8,042,572` states generated
- `2,379,112` distinct states
- depth `11`
- no invariant violations observed before termination

This exploratory result is evidence that the frozen target machine is holding up
well under a significantly wider safety search, but it is **not** itself the
canonical pass/fail gate for freeze completion.

The wider `MobMachineTargetLiveness.cfg` run is treated the same way: useful
for exploration under full `FairSpec`, but not the canonical freeze gate now
that `DestroyedAbsorbingProp`, `PendingSpawnEventuallyClearsProp`, and
`KickoffEventuallyClearsProp` are covered by the focused lifecycle harness.

## What We Explicitly Learned From Temporal Proof Iteration

The focused temporal harnesses forced real target corrections before they
passed:

- terminal run transitions now rewrite any remaining incomplete step state to
  terminal canceled step state
- loop / frame transitions are now loop-flow-only machine structure, not
  generic running-flow churn
- flow fairness is now split into dispatch, terminal, and structure progress,
  and the focused flow harness includes the work-ledger progress it depends on

Those were model defects, not TLC artifacts, and the freeze package was updated
to match the corrected target machine.

## What The Freeze Now Guarantees

- the target machine is single-kernel and explicit about:
  - identity / fencing
  - provisioning / kickoff
  - roster / topology
  - flows / steps / frames / loops
  - dispatch family and dispatch width
  - collection policy and quorum contribution state
  - work ledger coupling
  - task bindings
  - history
  - restore failure and recovery bookkeeping
  - top-level lifecycle
- every target input and target effect has a named home
- every documented proof obligation has a mapped executable home or an explicit
  future-refinement classification
- the transition catalog and executable `Next` relation match

## What Remains For The Next Proof Phase

The freeze phase is complete. The next phase is deeper proof, not freeze
repair.

That next phase focuses on:

- widening temporal search beyond the current focused harnesses
- strengthening proof over `FairSpec` / audit configurations
- reviewing the target/current delta through:
  - `mob-machine-refinement-map.md`
  - `mob-machine-refinement-delta.md`
- deciding which exact-current differences become:
  - accepted target-state cleanup
  - required implementation work
  - rejected target overreach

## Review Rule

If a reviewer cannot answer all of the following from the package, the freeze is
not strong enough:

1. what the target machine state is
2. what the target machine transitions are
3. what the current implementation differs on
4. what has already been mechanically checked
5. what still remains for the next proof phase

This handoff exists to make those five answers explicit.

The package-level reviewer audits that close the remaining trust gap are:

- `mob-machine-package-audit.md`
- `mob-machine-self-containment-audit.md`
- `mob-machine-traceability-audit.md`
