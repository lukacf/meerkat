# MobMachine Freeze

Status: frozen target-state asset

This note closes top-level `M2 = MobMachine`.

It is the full target-state freeze of `MobMachine` as a single semantic
kernel. This is the asset to take into TLA+ if the goal is to prove the mob
orchestration machine we intend to cut over to, not merely the implementation
baseline.

The exact-current baseline remains separately frozen in
`mob-machine-exact-current-freeze.md`.

The frozen target vocabulary is defined in `mob-machine-glossary.md`.

The reviewer-facing proof handoff is in `mob-machine-proof-handoff.md`.

The final freeze closeout is in `mob-machine-freeze-closeout.md`.

## Purpose

This freeze exists to prevent the same failure mode we just closed on the
Meerkat side:

- exact-current roster/session behavior gets mistaken for the intended machine
- flow orchestration stays split across “real owners” and convenience sidecars
- the TLA+ model quietly proves a smaller machine than the one we mean to cut
  over to

This note fixes the target machine explicitly.

## Target Boundary

`MobMachine` owns the full semantics of one mob orchestration kernel:

- durable member identity
- generation and runtime-incarnation truth
- authority and fencing
- hidden binding from runtime incarnation to the current Meerkat-side binding
- top-level mob lifecycle
- roster membership, profile assignment, runtime mode, and mutable labels
- coordinator binding and topology intent
- pending spawn and kickoff barrier truth
- work assignment intent and work terminalization at Mob level
- durable flow-run semantics
- frame scheduler semantics
- loop iteration semantics
- task-board and operator coordination truth
- restore failure and continuity-facing recovery truth
- durable identity-scoped event history

It does not own:

- Meerkat session execution, tools, ops, comms, or drain behavior
- scheduler / occurrence semantics
- raw wire / unwire / message delivery mechanics
- operator projections as semantic truth

## Target Design Commitments

### 1. One semantic owner per orchestration fact

Every semantic orchestration fact has one canonical owner inside
`MobMachine`.

Rosters, projections, stores, event routers, helper APIs, and bridge helpers
can exist, but none of them become parallel authorities.

### 2. Identity, generation, and fencing are first-class machine truth

The target machine explicitly owns:

- `AgentIdentity`
- `Generation`
- `AgentRuntimeId`
- `FenceToken`
- authority state

Respawn and reset semantics are machine transitions, not adapter folklore.

### 3. One work ledger for all member-directed work

All member-directed work crosses the machine through one Mob-owned work ledger:

- flow step work
- direct mob-directed member work
- cancel / bulk-cancel work intent

There is no second side path through raw turn delivery that is treated as
canonical machine behavior.

Work submission and step dispatch target only dispatch-capable members:
rostered, active, runtime-bound members with a live runtime incarnation.

Direct `SubmitWork` is external member work. It does not secretly materialize
flow-step dispatch. Flow-owned step dispatch is carried only by `DispatchStep`.

Step/work coupling is machine-owned truth:

- `Pending` join-ready steps carry no bound work yet
- `Dispatched` steps always retain at least one bound work record
- dispatched steps carry an explicit `step_dispatch_width` equal to their bound
  work multiplicity
- terminal step transitions drain any live work bound to that step
- terminal run transitions drain any residual live work bound to that run
- terminal run transitions also rewrite any remaining incomplete step state to
  terminal step state; terminal runs do not retain `None`, `Pending`, or
  `Dispatched` step status

### 4. Flow runs, frames, and loops are internal regions, not sibling machines

The target machine absorbs the current flow kernel surface:

- durable flow-run state
- frame-local execution frontier
- loop-instance and iteration state

Those remain internal regions of one machine, not separate proof boundaries.

Frame and loop structure are not generic “running flow” churn. They are
machine-owned structure for loop-capable flow archetypes, and the target
machine does not mutate frame / loop structure while a run still has live bound
work in flight.

### 5. Topology intent is Mob truth, not comms truth

The machine owns:

- coordinator identity
- topology revision
- collaboration topology intent
- external peer-spec presence

It does not own raw peer identifiers or delivery mechanics.

### 5a. Running flows must retain dispatch capacity or terminalize honestly

If a running flow still has undispatched steps, the machine must either retain
at least one dispatch-capable member or make the explicit no-dispatch-capacity
failure transition available. The target machine does not allow running flow
state to pretend work can still be dispatched through a merely rostered,
retiring, or binding-less member.

### 6. Pending spawn and kickoff are explicit machine state

Spawn lineage, partial provision progress, kickoff barriers, and restore
failures are explicit machine state, not helper-only scaffolding.

### 7. Recovery reconstructs canonical truth

Recovery rebuilds from authoritative Mob-owned records:

- identity state
- authority / fence state
- runtime binding state
- roster membership
- topology intent
- work ledger
- flow runs, frames, and loops
- restore-failure records
- history and task-board state

### 8. Event history and task-board truth are machine-owned

Identity-scoped durable event history and coordination/task-board state are
explicit machine regions in the target freeze. They are not left as a
projection-only afterthought.

Terminal run transitions clear any live task binding to that run, and
per-identity history keeps its own local sequence/count truth in addition to
the global sequence frontier.

## Target Internal Regions

The target `MobMachine` state is:

```text
MobMachine =
  identity
  + lifecycle
  + roster
  + topology
  + provisioning
  + work
  + flows
  + tasks
  + recovery
  + history
```

### `identity`

- known identities
- current generation
- current `AgentRuntimeId`
- current `FenceToken`
- authority state
- hidden binding-present flag
- continuity/checkpoint version

### `lifecycle`

- top-level mob phase
- active run count
- cleanup pending

### `roster`

- present identities
- profile assignment
- runtime mode
- labels
- member lifecycle status
- hidden runtime-binding presence

### `topology`

- coordinator identity
- topology revision
- collaboration edges
- external peer-spec presence

### `provisioning`

- pending spawn set
- pending runtime-id assignments
- kickoff pending set
- restore-failure presence
- restore-failure reason presence

### `work`

- known work refs
- target identity and runtime
- flow/run/step binding
- work status
- terminal outcome class

Bound work is not a side ledger relative to flows. The target machine commits
that step dispatch and work allocation are coupled, and step/run terminal
transitions drain any residual live bound work.

### `flows`

- known runs
- flow id
- run status
- schema version
- completed-at presence
- frame count
- loop count
- loop iteration count
- ordered steps
- dependency maps
- dispatch modes
- dispatch widths
- dependency modes
- condition flags
- branch labels
- collection-policy kinds
- quorum thresholds
- materialized step statuses
- failure counters
- retry budget
- supervisor escalation threshold
- tracked active run / cancel / stream sets

### `tasks`

- known tasks
- task status
- owning identity
- optional run binding

Active task-board entries attach only to a live flow run. Terminal task entries
do not retain a live run binding.

### `recovery`

- restore-failure records
- checkpoint version
- continuity-bound presence

### `history`

- monotonically increasing event sequence
- per-identity durable event count
- last durable event sequence per identity

## Target Lifecycle Semantics

### `SpawnMember`

- creates an absent identity
- generation starts at `0`
- authority is issued
- a fresh `AgentRuntimeId` is bound
- enters pending-spawn lineage until provision is finalized or failed

### `RespawnMember`

- preserves `AgentIdentity`
- preserves `Generation`
- preserves `AgentRuntimeId`
- issues a strictly newer `FenceToken`
- rebinds the hidden runtime binding

### `ResetMember`

- preserves `AgentIdentity`
- advances `Generation`
- mints a fresh `AgentRuntimeId`
- issues a fresh `FenceToken`
- retires or fences the old incarnation
- allows the identity to remain rostered while it re-enters pending-spawn
  lineage
- must not expose a live runtime binding while that pending spawn is unresolved
- surfaces as `Broken` instead of `Pending` if restore or provisioning fails
  before the new incarnation finalizes

### `RetireMember`

- forbids new work admission
- permits already accepted work to drain according to policy
- clears any outstanding kickoff barrier membership
- does not imply destruction by itself

### `DestroyMember`

- removes the active binding and roster presence
- clears pending spawn or kickoff membership for that identity
- leaves durable identity history intact

### Top-level mob lifecycle

### `StopMob`

- applies only while the mob is `Running`
- requires `active_run_count == 0`
- requires kickoff pending to be empty
- moves the top-level phase to `Stopped`
- clears `cleanup_pending`
- after it commits, only `CompleteMob`, `DestroyMob`, or stutter remain enabled

### `CompleteMob`

- applies only while the mob is `Running` or `Stopped`
- requires `active_run_count == 0`
- requires kickoff pending to be empty
- moves the top-level phase to `Completed`
- clears `cleanup_pending`
- after it commits, only `DestroyMob` or stutter remain enabled

### `DestroyMob`

- applies only while the mob is `Stopped` or `Completed`
- requires `active_run_count == 0`
- requires the roster to be empty
- requires pending spawn to be empty
- requires kickoff pending to be empty
- requires live work to be drained
- moves the top-level phase to `Destroyed`
- clears `cleanup_pending`
- makes the machine instance terminal

### Autonomous kickoff barrier

Autonomous members enter kickoff pending state only after spawn finalization:

- they are already rostered
- they already have a live runtime binding
- they are no longer part of pending-spawn lineage

Kickoff pending is cleared by:

- successful kickoff resolution
- lifecycle teardown that retires, resets, or destroys the member
- restore or provisioning failure that breaks the member

## Target Flow Semantics

### One run ledger

Every flow run is represented in one Mob-owned run ledger with:

- durable run identity
- durable kernel maps
- frame / loop structure
- failure and retry counters

### One collection-policy story

`All`, `Any`, and `Quorum` are machine-owned collection semantics.

For `Quorum`, observed contribution count is explicit machine state, and
completion is not allowed until that count reaches the stored threshold.

### One dispatch-mode story

`OneToOne`, `FanIn`, and `FanOut` are machine-owned flow semantics.

The target machine does not leave dispatch mode buried inside helper dispatch
code or aggregate-output shaping:

- `OneToOne` keeps one selected target as the contributing source
- `FanIn` keeps explicit multi-target aggregation semantics
- `FanOut` keeps explicit broadcast-style target semantics

Payload values are outside this machine, but aggregate shape class is still
determined by machine truth:

- `FanIn` => array-style aggregate
- `OneToOne` + `Any` => scalar-like aggregate
- otherwise => keyed-object aggregate

Per-step dispatch mode is seeded explicitly at run start. The target machine
does not infer dispatch family later from helper code or flow labels.

Per-step dispatch width is different: it is execution-time machine state. It
starts at `0` and becomes explicit only when `DispatchStep` commits actual work
multiplicity for a step.

The frozen flow-family coverage story is recorded separately in
`mob-machine-flow-family-coverage.md` so the target machine is explicit about
how flow archetypes and dispatch families compose.

### One branch story

Branch membership, branch-conditioned steps, and join semantics are
machine-owned. Conflicting dependency maps inside one branch group are illegal.
`DependencyMode::Any` joins resolve into explicit machine-owned ready/pending
state before member work is dispatched.

### One dependency-satisfaction story

Step dispatch is machine-owned dependency truth, not adapter folklore:

- `All` dependencies must be satisfied before direct dispatch
- `Any` joins must first resolve into explicit ready/pending state
- `Pending` steps carry no bound work yet
- `Dispatched` steps always retain at least one bound work record
- no-dispatch-capacity failure only applies when dispatch-ready undispatched
  work exists and no dispatch-capable member remains
- terminal step and run transitions drain residual live bound work instead of
  leaving cleanup to a sidecar ledger

### One loop story

Loop presence, loop iteration count, and loop/frame coupling are
machine-owned. Loop iteration ledger rows cannot exist without loop and frame
structure.

## Target Recovery Semantics

Recovery is machine-owned and rebuilds:

- identity authority
- current runtime binding
- roster membership
- topology revision and edges
- pending spawn lineage
- kickoff barrier state
- tracked flow-run structure
- restore failures
- history and task-board state

## Target Non-Commitments

The target machine does not claim ownership of:

- raw `SessionId`
- `MemberRef`
- raw peer identifiers
- raw peer delivery or transport reachability
- Meerkat-side queue depth, barrier membership, or drain internals

Those remain inside `MeerkatMachine` or perimeter services.

## Proof Handoff Package

This target freeze is meant to be proved together with:

- `mob-input-effect-alphabet.md`
- `mob-lowering-map.md`
- `mob-ownership-decisions.md`
- `mob-machine-state-schema.md`
- `mob-machine-derived-predicates.md`
- `mob-machine-transition-catalog.md`
- `mob-machine-proof-obligations.md`
- `mob-machine-proof-coverage.md`
- `mob-machine-effect-coverage.md`
- `mob-machine-refinement-map.md`
- `mob-machine-coverage-matrix.md`
- `mob-machine-fairness-assumptions.md`
- `mob-machine-glossary.md`
- `mob-machine-refinement-delta.md`
- `mob-cutover-checklist.md`
- `tla/MobMachineTarget.tla`
- `tla/MobMachineTarget.cfg`
- `tla/MobMachineTargetStress.cfg`
