# MobMachine Self-Containment Audit

Status: frozen target-state self-containment audit

This note records the final self-containment audit for `M2 = MobMachine`.

It exists to answer one specific reviewer question:

> Does the target `MobMachine` freeze stand on its own, or does it secretly rely
> on the exact-current baseline to define machine semantics?

## Answer

The target freeze stands on its own.

The exact-current baseline remains necessary as an implementation comparison
artifact, but it is not required to define target machine meaning.

## What Was Checked

The target freeze package was reviewed for target-facing semantic dependence on:

- `mob-machine-exact-current-freeze.md`
- `mob-machine-refinement-map.md`
- `mob-machine-refinement-delta.md`

The target-facing package includes:

- `mob-machine-freeze.md`
- `mob-machine-state-schema.md`
- `mob-machine-derived-predicates.md`
- `mob-machine-transition-catalog.md`
- `mob-machine-proof-obligations.md`
- `mob-machine-proof-coverage.md`
- `mob-machine-effect-coverage.md`
- `mob-machine-flow-family-coverage.md`
- `mob-machine-fairness-assumptions.md`
- `mob-machine-freeze-closeout.md`
- `mob-machine-package-audit.md`
- `mob-machine-proof-handoff.md`

## Audit Result

The target-facing package uses the exact-current baseline only for:

- naming the comparison artifact
- explaining why the target freeze exists
- framing refinement work for later implementation and cutover

It does not use the exact-current baseline to define:

- target state regions
- target state fields
- target transitions
- target predicates
- target invariants
- target fairness assumptions
- target proof obligations

Those are all defined directly by the target package itself.

## Why This Matters

A freeze package can look complete while still forcing reviewers to reconstruct
core semantics from an implementation baseline.

That is exactly the failure mode this audit is meant to exclude.

The target `MobMachine` now reads as:

- a self-contained machine asset
- with a separate implementation comparison package
- and a separate refinement/cutover package

## Conclusion

`MobMachine` target semantics are self-contained.

The exact-current Mob docs are comparison and refinement aids, not hidden
semantic dependencies.

That means the next honest step is proof against the target machine, not more
freeze authoring or more target/current disentangling.
