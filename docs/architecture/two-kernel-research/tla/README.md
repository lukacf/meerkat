# Target TLA+ Scaffold

This directory holds the non-generated experimental TLA+ model for the target
state `MeerkatMachine`.

It is intentionally outside `specs/machines` and `specs/compositions` because
those trees are generated from the existing machine catalog. This model is the
direct executable scaffold for the frozen target machine in:

- `../meerkat-machine-freeze.md`
- `../meerkat-machine-state-schema.md`
- `../meerkat-machine-derived-predicates.md`
- `../meerkat-machine-transition-catalog.md`
- `../meerkat-machine-proof-obligations.md`
- `../meerkat-machine-coverage-matrix.md`
- `../meerkat-machine-fairness-assumptions.md`

Files:

- `MeerkatMachineTarget.tla`: target-state single-machine model
- `MeerkatMachineTarget.cfg`: bounded TLC configuration
- `MeerkatMachineTargetStress.cfg`: widened bounded TLC configuration

Current status:

- the scaffold passes bounded base TLC
- the scaffold passes widened stress TLC
- the strengthened safety set has already forced honest corrections into the
  frozen target machine, including:
  - explicit destroyed terminality
  - explicit peer terminalized/admitted lineage
  - applied-surface constraints for reload and removal lineage

The goal of this scaffold is not to prove the implementation directly. It is
to prove the frozen target machine honestly enough that later refinement
against the implementation baseline can be reviewed as an explicit delta,
rather than smuggling target choices into the generated formal trees.
