# Target TLA+ Scaffold

This directory holds the non-generated experimental TLA+ models for the target
state kernels:

- `MeerkatMachine`
- `MobMachine`

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
- `../mob-machine-freeze.md`
- `../mob-input-effect-alphabet.md`
- `../mob-lowering-map.md`
- `../mob-ownership-decisions.md`
- `../mob-machine-state-schema.md`
- `../mob-machine-derived-predicates.md`
- `../mob-machine-transition-catalog.md`
- `../mob-machine-proof-obligations.md`
- `../mob-machine-proof-handoff.md`
- `../mob-machine-proof-coverage.md`
- `../mob-machine-effect-coverage.md`
- `../mob-machine-flow-family-coverage.md`
- `../mob-machine-refinement-map.md`
- `../mob-machine-coverage-matrix.md`
- `../mob-machine-fairness-assumptions.md`
- `../mob-machine-refinement-delta.md`
- `../mob-meerkat-composition-freeze.md`
- `../mob-meerkat-composition-proof-handoff.md`

Files:

- `MeerkatMachineTarget.tla`: target-state single-machine model for Meerkat
- `MeerkatMachineTarget.cfg`: bounded TLC configuration
- `MeerkatMachineTargetStress.cfg`: widened bounded TLC configuration
- `MobMachineTarget.tla`: target-state single-machine model for Mob
- `MobMachineTarget.cfg`: bounded TLC configuration
- `MobMachineTargetStress.cfg`: widened bounded TLC configuration
- `MobMachineTargetLifecycleLiveness.cfg`: focused lifecycle/provisioning
  liveness harness
- `MobMachineTargetProvisionLiveness.cfg`: focused provisioning/kickoff
  liveness harness
- `MobMachineTargetRecoveryLiveness.cfg`: focused recovery liveness harness
- `MobMachineTargetTaskLiveness.cfg`: focused task-lifecycle liveness harness
- `MobMachineTargetWorkLiveness.cfg`: focused work-ledger liveness harness
- `MobMachineTargetFlowLiveness.cfg`: focused flow-step liveness harness
- `MobMachineTargetLiveness.cfg`: wider full-`FairSpec` liveness exploration
- `MobMeerkatCompositionTarget.tla`: target seam composition model between the
  abstract Mob and Meerkat kernels
- `MobMeerkatCompositionTarget.cfg`: bounded seam safety configuration
- `MobMeerkatCompositionTargetStress.cfg`: widened bounded seam safety
  configuration
- `MobMeerkatCompositionTargetLifecycleLiveness.cfg`: focused seam lifecycle
  liveness harness
- `MobMeerkatCompositionTargetFlowWorkLiveness.cfg`: focused seam flow/work
  liveness harness

For both machines, bounded TLC runs use `StateConstraint == step_count <= MaxSteps`.
Configs that explore bounded safety only may disable TLC deadlock checking so the
artificial step cap does not get reported as a semantic deadlock.

That bounded scaffold is honest for safety, but not yet the final liveness
proof harness. Named temporal properties can live in the model now, but a
bounded `StateConstraint` run is not treated as the canonical gate for proving
them.

Current status:

- `MeerkatMachineTarget` passes bounded base and widened stress TLC
- `MobMachineTarget` passes bounded base and widened stress TLC
- `MobMachineTargetProvisionLiveness` passes focused provisioning/kickoff
  temporal checks
- `MobMachineTargetLifecycleLiveness` passes focused lifecycle/provisioning
  temporal checks
- `MobMachineTargetRecoveryLiveness` passes focused recovery temporal checks
- `MobMachineTargetTaskLiveness` passes focused task-lifecycle temporal checks
- `MobMachineTargetWorkLiveness` passes focused work-ledger temporal checks
- `MobMachineTargetFlowLiveness` passes focused flow-step temporal checks
- `MobMachineTargetAudit` has explored significantly beyond the canonical
  stress envelope with no invariant violations observed before deliberate
  termination due to continuing state explosion
- `MobMachineTargetLiveness` is retained as a wider exploratory `FairSpec`
  liveness pass, not the canonical freeze gate
- `MobMeerkatCompositionTarget` passes bounded base and widened stress TLC
- `MobMeerkatCompositionTargetLifecycleLiveness` passes focused seam lifecycle
  temporal checks
- `MobMeerkatCompositionTargetFlowWorkLiveness` passes focused seam flow/work
  temporal checks
- the target Mob alphabet coverage audit is clean
- the target Mob derived-predicate coverage audit is clean
- the strengthened safety sets have already forced honest corrections into the
  frozen target machines, including:
  - explicit destroyed terminality on the Meerkat side
  - explicit peer terminalized/admitted lineage on the Meerkat side
  - applied-surface constraints for reload and removal lineage on the
    Meerkat side
  - explicit dispatch-capable-member gating for work submission and step
    dispatch on the Mob side
  - explicit `FailRunNoDispatchCapacity` terminalization on the Mob side
  - dependency-ready dispatch on the Mob side, with `Any` joins entering
    explicit ready/pending state before member dispatch
  - explicit work/step coupling on the Mob side, so `Pending` steps carry no
    bound work, `Dispatched` steps always retain bound work, stored dispatch
    width equals bound-work multiplicity, and terminal step/run transitions
    drain residual live work
  - explicit quorum contribution state on the Mob side, so quorum collection
    completion depends on observed contribution count instead of a free
    resolution step
  - explicit per-step dispatch-mode state on the Mob side, seeded at
    `StartFlowRun`, plus execution-time `step_dispatch_width` chosen by
    `DispatchStep` and supporting a total aggregate-shape partition
  - explicit terminal-run step cleanup on the Mob side, so terminal runs cannot
    strand `None`, `Pending`, or `Dispatched` step state
  - explicit loop-flow-only structural transitions on the Mob side, so frame /
    loop churn cannot occur on non-structured flows or while live bound work is
    still in flight
  - explicit split flow fairness on the Mob side, separating dispatch,
    terminal, and structural progress, with the focused flow harness including
    the work-ledger progress it depends on
  - terminal run transitions clearing live task bindings on the Mob side
  - per-identity history sequence/count alignment on the Mob side
  - removal of the false Mob safety theorem that treated dispatch capacity as
    a state invariant instead of a terminalization obligation

The goal of this scaffold is not to prove the implementation directly. It is
to prove the frozen target machine honestly enough that later refinement
against the implementation baseline can be reviewed as an explicit delta,
rather than smuggling target choices into the generated formal trees.
