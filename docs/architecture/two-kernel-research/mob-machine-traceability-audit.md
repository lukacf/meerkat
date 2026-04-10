# MobMachine Traceability Audit

Status: frozen target-state traceability audit

This note records the final doc-to-model traceability audit for `M2 = MobMachine`.

It exists to answer one reviewer question:

> Do the named target machine artifacts actually resolve to the executable model,
> or is some of the freeze still only narrative?

## What Was Checked

The audit checked three things across the active target-facing Mob package:

1. document references resolve to real files
2. documented transition names resolve to executable definitions in
   `tla/MobMachineTarget.tla`
3. documented derived predicate names resolve to executable definitions in
   `tla/MobMachineTarget.tla`

## Reference Resolution

The target-facing Mob freeze docs resolve cleanly to their referenced files,
including references into the `tla/` subdirectory.

No missing target-facing `*.md`, `*.tla`, or `*.cfg` references were found.

## Transition Resolution

The transition catalog in `mob-machine-transition-catalog.md` contains `51`
named transition entries.

All `51` resolve to executable definitions in `tla/MobMachineTarget.tla`.

No missing transition definitions were found.

## Derived Predicate Resolution

The derived-predicate catalog in `mob-machine-derived-predicates.md` contains
`52` named predicates.

All `52` resolve to executable definitions in `tla/MobMachineTarget.tla` after
signature normalization.

No missing derived predicate definitions were found.

## Why This Matters

A freeze package can still look complete while leaving reviewers to trust that
the named vocabulary exists in the executable model.

This audit closes that gap directly:

- named docs resolve
- named transitions resolve
- named predicates resolve

## Conclusion

The target-facing `MobMachine` freeze package is traceable to the executable
model.

That means the package is not only complete as documentation; it is also
mechanically anchored to the TLA scaffold that the proof phase will use.
