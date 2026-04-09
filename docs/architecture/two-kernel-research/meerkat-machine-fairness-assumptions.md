# MeerkatMachine Fairness Assumptions

Status: frozen target-state proof assumption handoff

This note defines the fairness and progress assumptions allowed in the target
`MeerkatMachine` proof.

Use it with:

- `meerkat-machine-proof-obligations.md`
- `meerkat-machine-transition-catalog.md`
- `meerkat-machine-freeze.md`

## Purpose

Liveness claims are only honest if the proof package says what kind of fairness
or progress assumptions it is allowed to rely on.

This note prevents a common mistake:

- safety is modeled explicitly
- liveness is stated in prose
- fairness assumptions are smuggled in later without review

## Allowed Fairness Assumptions

The target proof may assume only the following classes of fairness.

### 1. Internal enabled-transition fairness

If a machine transition remains continuously enabled and is not precluded by a
stronger lifecycle command, it may be assumed to eventually occur.

Examples:

- a pending queue admission step
- a barrier satisfaction handoff once all barrier ops are terminal
- a continuation injection step once quiescence holds

This is the default fairness class for machine-internal progress.

### 2. External completion fairness

If an outstanding async operation, peer item, or tool-surface mutation is
modeled as eventually settling, that eventual settlement must be represented as
an explicit fairness assumption on the corresponding external source, not as an
implicit machine guarantee.

Examples:

- background operations eventually report terminal state
- peer items remain available to drain if the transport keeps supplying them
- staged tool-surface mutations eventually receive completion callbacks if the
  underlying mutation subsystem is assumed live

### 3. Recovery-source availability fairness

If `RecoverRuntime` or `RecycleRuntime` is modeled as reconstructing from
authoritative records, the proof may assume those authoritative records remain
available and consistent for the duration of the reconstruction step.

This is a bounded availability assumption, not a claim that storage is
infallible forever.

## Forbidden Fairness Assumptions

The proof should not rely on any of the following without explicit revision of
the freeze package.

### 1. Universal scheduler fairness

Do not assume “the implementation scheduler is fair” in a blanket sense.

Fairness must attach to named machine transitions or explicit external sources,
not to a generic runtime scheduler.

### 2. Hidden driver magic

Do not assume a driver, executor, router, or runtime loop “eventually does the
right thing” unless that behavior is already represented as:

- a machine effect
- a machine input
- or an explicit external completion assumption

### 3. Unbounded environment kindness

Do not assume:

- peers always retry forever
- external tool providers always succeed
- drain tasks never crash
- recovery stores never lag

Such claims must either be excluded or represented as explicit external
assumptions for the specific proof being attempted.

## Liveness Mapping

The liveness obligations in `meerkat-machine-proof-obligations.md` depend on
these assumption classes as follows:

- admitted input termination:
  - internal enabled-transition fairness
- `wait_all` resolution:
  - external completion fairness for referenced ops
  - internal fairness for the machine resolution step
- detached continuation injection:
  - external completion fairness for the terminal background event
  - internal fairness for the quiescent injection step
- `CancelAfterBoundary` observation:
  - internal fairness for boundary-drain progress
- peer replay/admission:
  - external availability fairness for peer backlog source
  - internal fairness for replay/drain transitions
- tool-surface convergence:
  - external completion fairness for staged mutations
  - internal fairness for boundary application / finalize transitions

## TLA+ Guidance

When encoding fairness:

- prefer weak/strong fairness on named actions, not on giant disjunctions
- keep fairness assumptions minimal and reviewable
- document every fairness annotation back against this note

If a proof requires a fairness assumption not listed here, that is a design
review event, not a local modeling convenience.

## Acceptance

The target `MeerkatMachine` proof package is considered fairness-complete when:

- every liveness claim in `meerkat-machine-proof-obligations.md` can be traced
  to an allowed fairness class here
- no proof step relies on hidden scheduler or driver folklore

That state is now reached.
