# Mob Shadow Validation Plan

Status: active pre-cutover validation plan

This note turns `mob-cutover-lowering-inventory.md` into the first explicit
shadow-validation plan for `MobMachine`.

The machine remains read-only during this phase; the implementation still owns
behavior.

## What to shadow

Construct `MobMachine` snapshots from the live runtime at the same observation
points the current branch already uses for diagnostic projection.

Primary comparison surfaces:

- handle diagnostic snapshot
- actor lifecycle/orchestrator/topology state
- tracked run store snapshot
- pending spawn lineage
- restore-failure snapshot
- task/history projections

## First shadow lanes

### S1. Provisioning / lifecycle lane

Compare:

- member lifecycle phase
- pending spawn lineage
- kickoff barrier posture
- restore-failure state
- roster membership after finalize/retire/destroy

Expected mismatch classes:

- spawn/finalize helper drift
- kickoff semantics split across actors/helpers
- restore failure truth still living only in diagnostics

### S2. Work ledger lane

Compare:

- work submission intent
- accepted/running/terminal work truth
- Meerkat bridge work ownership
- terminal cleanup of bound work

Expected mismatch classes:

- work truth implicit in flow engine instead of one ledger
- cancellation cleanup differing from target work contract

### S3. Flow / frame / loop lane

Compare:

- tracked run state
- step dispatch/collection state
- frame and loop structure
- retry/supervisor counters
- terminal run cleanup

Expected mismatch classes:

- flow engine helper drift
- partial live materialization where target expects canonical owned state
- loop/frame truth split between runtime and durable store

### S4. Topology / coordinator lane

Compare:

- coordinator binding
- topology edges
- external spec presence
- per-identity visibility after rewiring/reset/retire

Expected mismatch classes:

- roster/topology split truth
- external spec persistence drift

### S5. Task / history / recovery lane

Compare:

- task-board truth
- history alignment
- checkpoint/continuity posture
- restore failure clearing

Expected mismatch classes:

- diagnostic join still acting like semantic owner
- history/task persistence drift from target cleanup rules

## Mismatch triage

Use the same three classes as `MeerkatMachine`:

- `implementation_detail`
- `semantic_gap`
- `dogma_violation`

## Exit criteria

This plan is complete enough to start implementation once:

- one concrete shadow-read hook exists for each of the five lanes above
- mismatch output can name lane, machine region, and triage class
- representative provisioning, flow, and destroy paths can run through the
  shadow checks

## Read with

- `mob-cutover-lowering-inventory.md`
- `mob-machine-refinement-delta.md`
- `mob-meerkat-composition-refinement-delta.md`
- `two-kernel-refinement-program.md`
