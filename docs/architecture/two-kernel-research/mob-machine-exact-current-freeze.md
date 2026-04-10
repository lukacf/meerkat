# MobMachine Exact-Current Freeze

Status: frozen implementation comparison baseline

This note freezes the current joined `MobMachine` surface exactly as it exists
today in the rebased codebase.

It is not the intended target machine. It is the comparison baseline for:

- `mob-machine-freeze.md`
- `mob-machine-state-schema.md`
- `mob-machine-transition-catalog.md`
- `mob-machine-proof-obligations.md`

## Purpose

This note exists to keep two things from getting mixed together:

- the durable truths that the current code already exposes through the joined
  diagnostic kernel
- the identity-native target machine we actually want to cut over to

Without this baseline, target cleanup can get smuggled in as if the current
runtime already owned it.

## Current Joined Surface

The current exact-current joined snapshot is the diagnostic view built in
`meerkat-mob/src/mob_machine.rs`.

It is:

- public mob lifecycle phase
- actor-owned kernel diagnostics:
  - lifecycle snapshot
  - orchestrator snapshot
  - topology snapshot
  - pending spawn lineage snapshot
  - kickoff barrier snapshot
  - flow-tracker snapshot
- roster authority state
- projected member list state
- restore-failure diagnostics
- tracked-run projections from durable `MobRun` state

The current snapshot shape is:

```text
MobMachineSnapshot =
  phase
  + kernel
  + roster
  + members
  + restore_failures
  + tracked_runs
```

## Current Exact-Current Commitments

### 1. Current Mob identity is still `MeerkatId`

The current machine surface is still keyed by `MeerkatId`, not by:

- `AgentIdentity`
- `Generation`
- `AgentRuntimeId`
- `FenceToken`

Those are target-state Mob concepts, not current exact-current truths.

### 2. Session-scoped member binding still leaks

The current surface still exposes session-centric details through:

- `MemberRef`
- `current_session_id`
- roster/session binding projections

The exact-current freeze does not pretend those are already hidden.

### 3. Current flow truth is durable but partial

The current machine already owns durable tracked-run truth for:

- run identity and status
- schema version
- frame / loop / loop-iteration presence
- ordered step set
- dependency map
- dependency mode map
- branch labels
- collection-policy kind
- quorum thresholds
- failure / retry / escalation counters

But active status materialization remains partial:

- step statuses are visible only when the live kernel has already materialized
  them
- root outputs are not part of the frozen machine surface

### 4. Pending spawn and kickoff are real current-state seams

The current code already sustains:

- pending spawn lineage
- kickoff barriers
- restore-failure diagnostics

Those are exact-current machine state, not just future target aspirations.

### 5. Current topology is still roster-centered

The current topology surface is:

- roster wiring
- external peer specs
- coordinator-bound status
- monotonic topology revision

It is not yet the fully identity-native target topology ledger.

## Current Non-Commitments

This exact-current freeze does not claim that today's Mob runtime already owns:

- durable `AgentIdentity`
- generation advancement
- `AgentRuntimeId`
- `FenceToken`
- continuity or checkpoint lineage as canonical Mob machine truth
- hidden `SessionId`-free member binding
- explicit `WorkRef` / `WorkSpec` bridge semantics

Those belong to the target freeze, not this baseline.

## Current Flow Surface Families Already Sustained

The current diagnostic + durable run surface already sustains the following
families strongly enough to use as implementation comparison points:

- single-step flows
- two-step flows
- shared-path flows
- branch fallback flows
- dispatch families:
  - one-to-one
  - fan-in
  - fan-out
- collection families:
  - all
  - any
  - quorum
- loop-aware flows with frame + loop + iteration structure
- retry-budget and supervisor-threshold flows

## Exact-Current Review Rule

If a target-state Mob rule contradicts this baseline, it must be treated as a
real target-state delta, not hand-waved as “already true in the code.”
