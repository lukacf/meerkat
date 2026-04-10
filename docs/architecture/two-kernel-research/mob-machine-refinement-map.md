# MobMachine Refinement Map

Status: frozen target-state refinement handoff

This note maps the current joined `MobMachineSnapshot` surface onto the frozen
target-state `MobMachine`.

It is narrower than `mob-lowering-map.md`:

- `mob-lowering-map.md` maps target inputs/effects to current code paths
- this note maps current observable state into target machine regions

Use together with:

- `mob-machine-exact-current-freeze.md`
- `mob-machine-freeze.md`
- `mob-machine-refinement-delta.md`
- `meerkat-mob/src/mob_machine.rs`

## Purpose

This map prevents three kinds of drift:

1. treating the joined diagnostic snapshot as if it were already the target
   machine
2. claiming a target region is “obviously represented” without naming its
   current carrier
3. hiding real target-state deltas inside informal field-by-field reasoning

## Current snapshot shape

The exact-current joined surface is:

```text
MobMachineSnapshot =
  phase
  + kernel
  + roster
  + members
  + restore_failures
  + tracked_runs
```

This map answers: where does each of those carriers land in the target machine?

## Region mapping

### `identity`

Target truth:

- `AgentIdentity`
- `Generation`
- `AgentRuntimeId`
- `FenceToken`
- authority state
- binding-present flag
- checkpoint version

Current carriers:

- `roster.entries[*].meerkat_id`
- `kernel.lifecycle`
- `kernel.pending_spawn`
- `kernel.restore_failures`
- projected member binding/session state

Refinement read:

- exact-current only carries identity through `MeerkatId`
- target identity/generation/runtime/fence truth is a real refinement delta,
  not a direct field rename

### `lifecycle`

Target truth:

- top-level mob phase
- active run count
- cleanup pending

Current carriers:

- `phase`
- `kernel.lifecycle.phase`
- `kernel.lifecycle.active_run_count`
- `kernel.lifecycle.cleanup_pending`

Refinement read:

- this is one of the strongest exact-current to target mappings

### `roster`

Target truth:

- present identities
- profile assignment
- runtime mode
- labels present
- member lifecycle status
- runtime-binding visibility

Current carriers:

- `roster`
- `members`

Refinement read:

- exact-current is still `MeerkatId` / session centered
- otherwise this region is already substantially represented

### `topology`

Target truth:

- coordinator identity
- topology revision
- collaboration edges
- external spec presence

Current carriers:

- `kernel.orchestrator`
- `kernel.topology`
- `roster.wired_to`

Refinement read:

- current topology is real, but still roster-centered rather than
  identity-native

### `provisioning`

Target truth:

- pending spawn lineage
- pending runtime-id assignments
- kickoff pending set
- restore-failure presence
- restore-failure reason presence

Current carriers:

- `kernel.pending_spawn`
- `kernel.kickoff_barrier`
- `restore_failures`

Refinement read:

- this region is already strongly represented in exact-current form
- runtime-id/fence semantics remain target-state cleanup over current spawn
  lineage

### `work`

Target truth:

- known work refs
- work owner identity/runtime
- optional run binding
- optional step binding
- work status
- terminal outcome class

Current carriers:

- partially implied through `tracked_runs`
- partially implied through flow engine / actor task maps

Refinement read:

- current code does not yet expose one canonical joined work ledger in the
  snapshot
- this is a target-state consolidation, not an exact-current field mapping

### `flows`

Target truth:

- durable runs
- tracked active run sets
- frame / loop structure
- ordered steps
- dependency maps
- dispatch modes
- dependency modes
- condition flags
- branch labels
- collection-policy kinds
- quorum thresholds
- observed quorum contribution count
- materialized step statuses
- failure / retry / escalation counters

Current carriers:

- `tracked_runs`
- `kernel.flow_trackers`

Refinement read:

- durable run shape is already strongly represented
- active status materialization remains partial exact-current truth
- dispatch family is exact-current truth carried by the runtime, but target
  `step_dispatch_mode` is an explicit promotion of that truth into the machine
  surface
- explicit observed quorum contribution count is a target-state strengthening

### `tasks`

Target truth:

- known tasks
- task status
- owning identity
- optional live run binding

Current carriers:

- `kernel.flow_trackers` task/stream/taskboard-adjacent state
- actor-owned task board and scoped event machinery outside the joined snapshot

Refinement read:

- current implementation has real task truth, but the joined snapshot does not
  yet expose it as a first-class region
- target machine promotes that to canonical machine state

### `recovery`

Target truth:

- restore-failure set
- checkpoint version
- continuity-bound presence

Current carriers:

- `restore_failures`
- actor/builder restore diagnostics

Refinement read:

- restore-failure truth is already strong
- checkpoint / continuity state remains partly implicit in exact-current Mob
  and explicit in the target machine

### `history`

Target truth:

- global sequence frontier
- per-identity event count
- per-identity last sequence

Current carriers:

- actor-owned event history and scoped event machinery
- not directly exposed in the joined exact-current snapshot

Refinement read:

- this is target-state promotion of real current logic into explicit machine
  state

## Current-to-target summary

### Strong exact-current mappings

- `lifecycle`
- `topology`
- `provisioning`
- durable parts of `flows`

### Partial exact-current mappings

- `roster`
- `recovery`

### Target-state promotions / consolidations

- `identity`
- `work`
- `tasks`
- `history`
- full explicit `flows.step_collection_observed`

## Review rule

If a target region cannot be explained as one of:

1. strong exact-current mapping,
2. partial exact-current mapping, or
3. explicit target-state promotion,

then the target `MobMachine` freeze is still hiding a semantic gap.
