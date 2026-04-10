# MobMachine Freeze Closeout

Status: frozen target-state closeout

This note is the final closeout for `M2 = MobMachine`.

It exists to answer one reviewer question directly:

> Is `MobMachine` still being authored, or is the target machine now frozen
> strongly enough that the next honest step is proof?

The answer is: the target machine is frozen strongly enough that the next
honest step is proof.

Use together with:

- `mob-machine-freeze.md`
- `mob-machine-proof-handoff.md`
- `mob-cutover-checklist.md`
- `mob-machine-package-audit.md`
- `mob-machine-self-containment-audit.md`
- `mob-machine-traceability-audit.md`
- `tla/MobMachineTarget.tla`

## What Is Frozen

The frozen target machine is the single-kernel `MobMachine` defined in:

- `mob-machine-freeze.md`
- `mob-machine-state-schema.md`
- `mob-machine-derived-predicates.md`
- `mob-machine-transition-catalog.md`
- `mob-machine-proof-obligations.md`

Its exact-current implementation comparison baseline is:

- `mob-machine-exact-current-freeze.md`
- `mob-machine-refinement-map.md`
- `mob-machine-refinement-delta.md`

Its proof handoff package is:

- `mob-machine-proof-handoff.md`
- `mob-machine-proof-coverage.md`
- `mob-machine-effect-coverage.md`
- `mob-machine-flow-family-coverage.md`
- `mob-machine-fairness-assumptions.md`
- `mob-input-effect-alphabet.md`
- `mob-lowering-map.md`
- `mob-ownership-decisions.md`

Its executable target scaffold is:

- `tla/MobMachineTarget.tla`
- `tla/MobMachineTarget.cfg`
- `tla/MobMachineTargetStress.cfg`
- `tla/MobMachineTargetAudit.cfg`

## What Was Mechanically Checked

The frozen target package now has all of the following:

- canonical bounded TLC base pass
- canonical bounded TLC stress pass
- canonical focused lifecycle liveness pass
- canonical focused recovery liveness pass
- canonical focused task-lifecycle liveness pass
- exploratory widened audit run with no invariant failures observed before
  deliberate termination
- transition catalog aligned with executable `Next`
- proof-obligation coverage mapped to invariants, transitions, fairness bundles,
  or explicit next-phase proof work
- effect coverage mapped to executable targets
- refinement map and refinement delta separating exact-current from target-state
  commitments
- active target-facing Mob docs cleaned of hedge language

## Why Freeze Authoring Is Complete

Freeze authoring is complete because the package now answers all of these
questions explicitly:

1. what the target machine state is
2. what the target machine transitions are
3. what the target/current delta is
4. what has already been mechanically checked
5. what remains for the next proof phase

At this point, more freeze authoring would mostly duplicate or restate those
answers rather than strengthening the target machine.

## What Would Reopen The Freeze

Reopen the freeze only if one of the following happens:

- TLC or later proof work exposes a real contradiction in the target machine
- a catalog/model mismatch is found
- a target-facing doc is found to promise a fact not represented in the model
- a supposedly target-owned fact is discovered to belong outside `MobMachine`

Do not reopen the freeze just because:

- wider full-`FairSpec` liveness exploration has not yet been completed
- implementation refinement is still pending
- cutover work remains to be done

Those are next-phase tasks, not signs that the target freeze is incomplete.

## Next Honest Step

The next honest step is:

1. treat `mob-machine-freeze.md` as the target machine
2. treat `mob-machine-exact-current-freeze.md` as the implementation baseline
3. treat `mob-machine-proof-handoff.md` and `tla/MobMachineTarget.tla` as the
   proof handoff
4. start the real TLA+ proof phase
