# Mob Cutover Checklist

Status: frozen target-state checklist

This is the Mob-only execution checklist for freezing `M2 = MobMachine` as one
target-state orchestration kernel that is ready to take into TLA+ proof work.

It does not claim that the implementation already matches the target machine.
That comparison baseline remains separately frozen in
`mob-machine-exact-current-freeze.md`.

This checklist exists so the Mob freeze can be reviewed as one concrete asset
instead of as a pile of passing TLC runs and supporting notes.

It is derived from:

- `cutover-gate.md`
- `mob-machine-freeze.md`
- `mob-machine-proof-handoff.md`
- `mob-machine-package-audit.md`
- `mob-machine-self-containment-audit.md`
- `mob-machine-traceability-audit.md`
- `mob-machine-state-schema.md`
- `mob-machine-proof-obligations.md`
- `mob-machine-proof-coverage.md`
- `mob-machine-flow-family-coverage.md`
- `mob-machine-coverage-matrix.md`
- the live convergence log in `build-progress.md`

## Nomenclature

Top-level machine milestones use:

- `M1` = full `MeerkatMachine`
- `M2` = full `MobMachine`

This checklist is internal to `M2` and uses `C1`-`C10` to avoid collision with
the top-level milestone labels.

## Reading rule

- `Done` means the target MobMachine commitment is frozen, documented, and
  represented in the executable target-state TLC scaffold.
- `In Progress` means the target boundary looks right, but the target package
  is still missing a canonical artifact or a verified executable check.
- `Pending` means the area still blocks an honest target-state Mob freeze.

## Current Mob posture

What now exists as frozen target-state MobMachine assets:

- `mob-machine-freeze.md`
- `mob-input-effect-alphabet.md`
- `mob-lowering-map.md`
- `mob-ownership-decisions.md`
- `mob-machine-state-schema.md`
- `mob-machine-derived-predicates.md`
- `mob-machine-transition-catalog.md`
- `mob-machine-proof-obligations.md`
- `mob-machine-proof-coverage.md`
- `mob-machine-effect-coverage.md`
- `mob-machine-flow-family-coverage.md`
- `mob-machine-refinement-map.md`
- `mob-machine-coverage-matrix.md`
- `mob-machine-fairness-assumptions.md`
- `mob-machine-glossary.md`
- `mob-machine-refinement-delta.md`
- `mob-machine-freeze-closeout.md`
- `mob-machine-proof-handoff.md`
- `mob-machine-package-audit.md`
- `mob-machine-self-containment-audit.md`
- `mob-machine-traceability-audit.md`
- `tla/MobMachineTarget.tla`
- `tla/MobMachineTarget.cfg`
- `tla/MobMachineTargetStress.cfg`
- `tla/MobMachineTargetAudit.cfg`
- `tla/MobMachineTargetProvisionLiveness.cfg`
- `tla/MobMachineTargetWorkLiveness.cfg`
- `tla/MobMachineTargetFlowLiveness.cfg`

What the target machine now explicitly includes:

- identity, generation, runtime-incarnation, authority, and fencing truth
- roster membership, profile/runtime-mode assignment, labels, and binding
  visibility
- coordinator binding, topology intent, and external peer-spec presence
- pending spawn lineage, kickoff barrier state, and restore-failure truth
- one work ledger for direct work and flow-step work
- durable flow-run, frame, and loop semantics
- collection semantics for `All`, `Any`, and `Quorum`
- explicit dispatch-family semantics for `OneToOne`, `FanIn`, and `FanOut`
- task-board truth, history truth, and recovery/checkpoint truth

What is now executably checked:

- bounded TLC base run passes
- bounded TLC stress run passes
- focused lifecycle/provisioning liveness passes
- focused provisioning/kickoff liveness passes
- focused recovery liveness passes
- focused task-lifecycle liveness passes
- focused work-ledger liveness passes
- focused flow-step liveness passes
- the target alphabet coverage audit is clean
- the active Mob freeze docs no longer contain hedge phrases
- top-level lifecycle is authoritative: `Stopped` and `Completed` are quiescent
  postures, and `Destroyed` is stutter-only

## Checklist

| ID | Item | Status | Why it blocks freeze | Exit criteria |
| --- | --- | --- | --- | --- |
| C1 | Freeze the exact-current comparison baseline | Done | Without a frozen comparison baseline, target cleanup can be misread as already-true current behavior | `mob-machine-exact-current-freeze.md` exists and explicitly separates current joined diagnostics from the target machine |
| C2 | Freeze the target boundary and owned regions | Done | The target machine cannot be proved honestly if the semantic boundary is still fuzzy | `mob-machine-freeze.md` defines the full target boundary and the internal regions `identity`, `lifecycle`, `roster`, `topology`, `provisioning`, `work`, `flows`, `tasks`, `recovery`, and `history` |
| C3 | Freeze identity, authority, provisioning, and topology truth | Done | Identity-native mob semantics collapse without first-class authority, spawn lineage, kickoff, and topology truth | the target freeze and state schema explicitly model generation, runtime incarnation, fence state, pending spawn lineage, kickoff barrier state, coordinator binding, topology edges, and external spec presence |
| C4 | Freeze work-ledger semantics and work/step coupling | Done | A second work truth would reintroduce the same split-owner bug class the architecture is meant to remove | target docs and TLC model both encode one work ledger, dispatch-capable submission gating, pending/dispatched coupling, terminal step cleanup, and terminal run cleanup |
| C5 | Freeze full flow, frame, and loop semantics | Done | Mob is not honestly frozen if flows are reduced to only single-step happy paths | target docs and TLC model cover durable run shape, branch fallback, `Any` join semantics, `All`/`Any`/`Quorum` collection semantics, explicit `OneToOne`/`FanIn`/`FanOut` dispatch-family truth, frame structure, loop structure, loop iteration structure, retry budget, supervisor threshold, and no-dispatch-capacity failure |
| C6 | Freeze tasks, recovery, and history truth | Done | Task/run cleanup, restore failure truth, and per-identity history are semantic facts, not projections | target docs and TLC model cover task lifecycle, live/terminal run binding rules, restore-failure records, checkpoint versioning, continuity-bound state, and per-identity history sequence/count alignment |
| C7 | Freeze the canonical proof handoff package | Done | TLA+ work cannot proceed honestly without one canonical package for state, predicates, transitions, obligations, effects, refinement framing, and obligation coverage | input/effect alphabet, lowering map, ownership decisions, state schema, derived predicates, transition catalog, proof obligations, proof coverage, effect coverage, refinement map, coverage matrix, fairness assumptions, glossary, and refinement delta all exist and are cross-linked from the target freeze |
| C8 | Execute bounded TLC over the target machine | Done | A prose freeze without an executable scaffold can still hide structural contradictions | `MobMachineTarget.tla` passes bounded TLC in both base and stress configs |
| C9 | Verify target alphabet and artifact consistency | Done | The target machine is not review-ready if inputs/effects are missing from docs or if active docs still contain target-state hedges | the input/effect coverage audit is clean, the coverage matrix matches the model, and hedge sweeps over active Mob freeze docs are clean |
| C10 | Final Mob freeze review | Done | Without explicit close-out and reviewer-facing audit artifacts, “done” remains implicit and easy to reopen by accident | this checklist exists, all items are `Done`, `mob-machine-freeze-closeout.md` exists, `mob-machine-package-audit.md` exists, `mob-machine-self-containment-audit.md` exists, `mob-machine-traceability-audit.md` exists, and the next step is TLA+ proof work against the frozen target machine rather than more freeze repair |

## Review-ready read

`M2 = MobMachine` is now frozen as a target-state machine package that is
ready for TLA+ proof work.

That means:

- the target machine is explicit
- the exact-current baseline is explicit
- the proof handoff package is explicit
- the executable TLC scaffold is green
- the target alphabet and artifacts have no known silent gaps

It does not mean implementation refinement or cutover is complete. Those are
later steps.

## Next active step

The next active step is:

1. use `mob-machine-freeze.md` as the target-state machine asset
2. use `mob-machine-exact-current-freeze.md` as the implementation comparison
   baseline
3. use `mob-machine-proof-obligations.md` plus the TLC scaffold in `tla/` as
   the proof handoff package
4. continue deeper TLA+ proof work on `MobMachine` beyond the current focused
   lifecycle/provision/work/flow harnesses and wider exploratory `FairSpec`
   search
