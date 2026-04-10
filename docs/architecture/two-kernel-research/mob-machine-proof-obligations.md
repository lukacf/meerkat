# MobMachine Proof Obligations

Status: frozen target-state proof handoff

This note records the proof obligations for the target-state `MobMachine`.

Coverage of these obligations against executable invariants, named
transitions, and fairness bundles is recorded in `mob-machine-proof-coverage.md`.

## Safety obligations

### Identity / authority

- at most one active `FenceToken` per identity
- respawn preserves generation but advances authority
- reset advances generation and authority together
- destroyed members have no live binding

### Roster / topology

- roster presence implies known identity
- coordinator is always rostered, active, and runtime-bound
- topology edges reference only rostered members
- external peer specs reference only active, runtime-bound roster members
- topology revision is monotonic

### Provisioning / kickoff

- kickoff pending implies roster presence
- kickoff pending implies active authority, active member status, and a live
  runtime binding
- kickoff pending implies autonomous runtime mode
- kickoff pending never overlaps pending spawn or restore failure
- pending spawn identities are always in `Provisioning` authority state
- pending spawn identities always have a pending runtime binding
- pending runtime-id presence iff pending-spawn presence
- if a pending spawn identity is already rostered, it must be `Pending` or
  `Broken`, and it must not expose a live runtime binding yet
- restore-failure records reference known identities
- restore-failure reason presence iff restore-failure presence
- identity and recovery checkpoint versions stay aligned

### Top-level mob lifecycle

- stopped mob phase requires `active_run_count == 0`
- completed mob phase requires `active_run_count == 0`
- stop/completion require kickoff pending to be empty
- destroyed mob phase requires empty roster
- destroyed mob phase requires empty pending spawn
- destroyed mob phase requires no live work
- destroyed mob phase is terminal for the machine instance
- stopped mob phase admits only completion, destroy, or stutter
- completed mob phase admits only destroy or stutter

### Work ledger

- work targets only known identities
- new work only targets dispatch-capable member/runtime bindings
- direct `SubmitWork` never implicitly materializes a flow-step dispatch
- work references at most one `(run, step)` pair
- live run-bound work only targets steps that are already machine-visible
- terminal step states never retain live bound work
- terminal run states never retain live bound work
- terminal run states never retain non-terminal step status
- terminal work has terminal status
- rejected work is never later accepted or running

### Flows

- tracked runs exist in the durable run set
- terminal run iff completion presence
- no duplicate ordered steps
- every ordered step has:
  - dependency map entry
  - dispatch mode entry
  - dependency mode entry
  - condition flag entry
  - branch entry
  - collection-policy entry
  - quorum-threshold entry
- dependency targets are known ordered steps
- no self-dependencies
- no duplicate dependencies
- dispatch mode is explicit machine truth for every ordered step
- start-flow initialization seeds a dispatch-mode entry for every ordered step
- dispatch width is explicit machine truth for every dispatched step
- dispatch width is chosen at `DispatchStep`, not guessed at run start
- one-to-one dispatched steps have width `1`
- quorum-dispatched steps have width at least their stored threshold
- bound work count equals stored dispatch width once a step has dispatched
- `DependencyMode::Any` requires at least one branch-backed dependency
- `DependencyMode::Any` join resolution enters explicit machine-owned
  ready/pending state before member dispatch
- `Pending` join-ready steps carry no bound work yet
- dispatched steps only arise from dependency-satisfied states
- dispatched steps always retain at least one bound work record
- quorum-completed steps require observed contribution count to satisfy the
  stored threshold
- no-dispatch-capacity failure applies only when a run has dispatch-ready
  undispatched work and no dispatch-capable member exists
- branch-labelled steps must also have condition presence
- branch groups larger than one must agree on dependency sets
- aggregate shape class is determined by dispatch mode plus collection policy
- aggregate shape class is total and mutually exclusive for every ordered step
- frame / loop structure is only live on loop-capable flow archetypes
- frame / loop transitions do not churn while a run still has live bound work
- loop iteration count implies loop and frame structure
- loop structure implies frame structure
- structured runs require modern schema
- `consecutive_failure_count <= failure_count`
- if a running run loses dispatch capacity while undispatched steps remain,
  the explicit no-dispatch-capacity failure transition is enabled

### Tasks / history

- task bindings reference known identities or runs
- live task bindings reference only non-terminal runs
- terminal run transitions clear any live task binding to that run
- terminal tasks have no live run binding
- per-identity history sequence never exceeds global sequence
- per-identity history last-sequence equals per-identity event count

## Liveness obligations

- pending spawn eventually resolves to ready or failed
- kickoff pending eventually resolves or is explicitly cleared by lifecycle
  teardown or failure
- accepted work eventually reaches running or terminal
- running work eventually terminalizes or is fenced
- running flows eventually dispatch, resolve, structurally advance, or
  terminalize under the frozen split fairness bundles
- running flows without any dispatch-capable member eventually terminalize as
  failed
- retiring members eventually destroy or return to a stable terminal posture
- stopped mobs with empty roster eventually destroy or remain quiescent

## Refinement obligations

The target Mob proof must later support refinement against:

- `mob-machine-exact-current-freeze.md`
- `mob-machine-refinement-map.md`
- `mob-machine-refinement-delta.md`
- the joined diagnostic snapshot in `meerkat-mob/src/mob_machine.rs`
- the current flow-bearing runtime tests in `meerkat-mob/src/runtime/tests.rs`

The target proof is allowed to differ from exact-current behavior, but every
difference must be reviewable as an explicit delta.
