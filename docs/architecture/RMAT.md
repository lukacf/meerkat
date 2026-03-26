# RMAT: Rust Machine Authority Theory

This document defines the authority model Meerkat uses for canonical state
machines.

The short version:

- canonical semantic truth must come from machine definitions
- generated machine authority code must be the only place allowed to mutate
  canonical state
- handwritten code may capture evidence, route, persist, schedule, and execute
  effects
- handwritten code may not invent parallel semantic state or handwritten
  reducers
- semantically meaningful async seams must be modeled and closed through the
  formal composition layer, not left to shell folklore

## Core Rule

For any canonical domain, there must be exactly one answer to:

> What state is this in, what transitions are legal, and what state change
> happens next?

That answer must be:

- the machine definition in `meerkat-machine-schema`
- executed through generated authority code in `meerkat-machine-kernels`

If the answer is partly “the machine” and partly “this handwritten runtime
loop / actor / router file”, then we still have two sources of truth.

## What Counts As Canonical Truth

Canonical truth includes:

- lifecycle phase/state
- legality of transitions
- terminal outcomes
- rollback/requeue/consume semantics
- orchestration decisions that change semantic state
- any state mutation that changes what the system *is*, not merely how effects
  are delivered

## What Handwritten Code May Still Do

Handwritten shell code is allowed to:

- classify raw inputs into typed machine inputs when that classification is
  already constrained by machine or protocol truth
- route inputs to the right machine owner
- schedule and execute effects
- manage channels, tasks, notifiers, and completion waiters
- persist and load state snapshots
- perform transport, IO, and adapter work

Handwritten shell code is not allowed to:

- decide transition legality
- mutate canonical semantic state directly
- implement alternate reducers in parallel
- invent duplicate semantic enums/status models
- hide transition-relevant truth in side maps/booleans that the machine does
  not own
- choose ad hoc semantic feedback for a machine-owned async seam
- invent surfaced terminal classification branches that diverge from machine
  terminal truth

## Seams And Handoffs

Single-machine sealed mutators are necessary, but they are not the whole story.

Meerkat also has semantic seams between:

- a machine and owner-realized async work
- a machine phase and the associated wait-set or obligation that gives that
  phase meaning
- a machine terminal outcome and the surfaced result/error classification

RMAT treats those as first-class proof surfaces by extending the existing
composition ontology with:

- composition actors that may be `Machine` or `Owner`
- effect handoff protocols attached to existing effect-disposition rules
- outstanding-obligation validation in closed-world compositions
- generated seam helper APIs for owner feedback
- generated terminal classification derived from machine truth

This is not a second actor system or a second effect transport system. It is an
extension of the current composition/effect model.

## Enforcement Model

The enforcement mechanism must be compile-time, not convention.

For each canonical machine, generated code should own:

- canonical machine state
- canonical machine input/event types
- canonical machine effect types
- a sealed mutator trait
- the only implementation of that trait

Conceptually:

```rust
pub trait FlowRunMutator: sealed::Sealed {
    fn apply_flow_run(
        &mut self,
        input: FlowRunInput,
    ) -> Result<FlowRunTransition, FlowRunError>;
}
```

Only generated kernel code implements the trait.

That means handwritten code can:

- hold machine-owned state
- call the generated mutator
- execute returned effects

But handwritten code cannot:

- create a second semantic mutation path
- directly set canonical phases
- implement a shadow reducer

## Transient vs Persistent Canonical State

Not all canonical states have to be persisted.

Example: `Attached` is canonical runtime state while a runtime is alive and
listening, but it may be intentionally persisted as `Idle` and re-derived on
recovery.

That is valid if:

1. the machine defines `Attached` as a real state
2. the shell observes lifecycle facts such as executor/task/channel existence
3. the shell re-enters the transient state by calling a generated machine input
   such as `AttachExecutor`

That is not the shell inventing state. It is the shell triggering a legal
machine transition based on runtime lifecycle facts.

The machine owns:

- what states exist
- what transitions are legal

The shell may observe facts and trigger transitions, but only through generated
mutators.

## The “When To Call” Boundary

The shell does decide *when* to call the machine.

Example:

- raw inbox item arrives
- shell classifies it as peer message / silent intent / lifecycle notice
- shell calls the corresponding machine input

This classification step is allowed.

The rule is:

- classification is shell work
- classified outputs must become typed machine inputs

If the shell finds itself classifying inputs into meaningful categories that do
not map to machine inputs, then the machine is missing inputs and the model is
incomplete.

## Composition Authority

Single-machine sealed mutators solve local authority, not full composed
authority.

Some canonical truth is emergent across multiple machines, for example:

- runtime control
- runtime ingress
- input lifecycle
- turn execution

The truth of “this input was accepted, staged, executed, and consumed” is a
composition-level fact.

That means:

- single-machine authority is enforced by generated mutators
- cross-machine invariants are still enforced by:
  - composition definitions
  - composition witnesses
  - TLC / model checking
  - integration and owner tests

This is a known gap in the type-level enforcement model. Rust types can prevent
parallel reducers for one machine; they do not by themselves prove composition
correctness.

The seam layer narrows that gap:

- transport disposition still stays inside `Local`, `External`, or `Routed`
- an effect may additionally declare a handoff protocol when owner realization
  and later feedback are semantically meaningful
- compositions then prove structural coverage and safety for those obligations,
  plus scoped liveness only where explicitly modeled

## Proof Classes

Every important RMAT claim should be labeled explicitly:

- **Structural coverage**: the seam is named, protocolized, and wired
- **Safety**: no invalid seam state or transition is reachable
- **Liveness**: under explicit fairness assumptions, required feedback
  eventually arrives

Seam work currently proves:

- structural coverage for all owner handoffs (protocol exists, realization
  sites declared, feedback constrained to generated helpers)
- safety for comms drain seam closure (obligation invariants, no shell bypass)
- safety for turn-execution barrier (barrier satisfaction gated by authority,
  `ToolCallsResolved` requires `barrier_satisfied == true`)
- safety for terminal outcome alignment (exhaustive generated classifier, no
  default arm)
- one explicit liveness claim for comms drain feedback closure (spawn protocol
  under task-scheduling fairness)
- cross-machine barrier composition (OpsLifecycle `WaitAllSatisfied` routed to
  TurnExecution `OpsBarrierSatisfied` through declared handoff protocol)

## Shell vs Authority Boundary

Generated authority owns:

- semantic state
- legal transitions
- semantic outcomes
- transition-produced effects

Handwritten shell owns:

- task lifetime
- channels
- async waiting
- persistence adapters
- effect execution
- transport plumbing

The shell may observe state and capture raw evidence. It may not decide truth.

Concretely:

- shell may notice that a task exited or a wait completed
- shell may not choose an arbitrary semantic exit reason or closure class
- shell may assemble result payload data
- shell may not invent terminal classification outside generated machine-derived
  helpers

Hot-path observable projections are still allowed when they are derived from
machine truth. For example, a lock-free suppression cache is acceptable if it
is updated only from the machine/protocol path and never becomes a second source
of truth.

## Forbidden Patterns

For canonical domains, these are bugs:

- handwritten reducer/helper methods that decide transitions
- extra semantic booleans/maps/status fields outside machine-owned state
- duplicate lifecycle enums/status structs outside the generated authority
- replay or recovery that depends on hidden shell-local semantic truth
- docs/contracts that claim generated ownership while handwritten code still
  owns semantics

## Practical Test

For any canonical domain, ask:

> If I removed the handwritten shell code, would I lose semantic truth?

If the answer is yes, then machine authority is not complete.

## Proving Ground

The best proving ground for this model is the ugliest handwritten owner first,
not the easiest.

`FlowRunMachine` is the best stress test because it forces the authority model
to handle:

- dependency resolution
- quorum and branch winner selection
- retry/failure policy
- in-flight bookkeeping
- durable replay implications

If generated machine authority survives `flow.rs`, it will likely survive the
rest of the platform. If it fails there, we learn whether:

- the schema language needs richer guards/effects
- the kernel API shape needs to grow
- or the approach has real limits

## Adding a New Handoff Protocol

When adding a new async boundary that crosses an ownership seam:

1. **Annotate the effect** — set `handoff_protocol: Some("protocol_name")` on the
   `EffectDispositionRule` in the machine catalog file.

2. **Declare the protocol** — add an `EffectHandoffProtocol` to every composition
   that includes the producing machine. Specify the realizing actor, allowed
   feedback inputs, correlation fields, and closure policy.

3. **Run schema validation** — `./scripts/repo-cargo nextest run -p meerkat-machine-schema`
   catches missing protocols in closed-world compositions.

4. **Generate protocol helpers** — `./scripts/repo-cargo run -p xtask -- protocol-codegen`
   creates typed feedback submitters and (for `TerminalClosure` protocols) terminal
   classification helpers.

5. **Wire the shell** — replace handwritten feedback submission with calls to the
   generated protocol helpers. Generated code carries `// @generated` markers.

6. **Run the RMAT audit** — `./scripts/repo-cargo run -p xtask -- rmat-audit --strict`
   catches:
   - effects with `handoff_protocol` but no matching composition protocol
   - protocols without generated helper files
   - feedback submitted outside generated helpers
   - terminal classification without generated classifiers

   If a finding is expected in-progress work, update the baseline with
   `--update-baseline`. The baseline lives at `xtask/rmat-baseline.toml`.

7. **Add composition witnesses** — witnesses exercise the handoff path and verify
   that the protocol's feedback inputs are reachable.

## CI Enforcement

`make ci` runs `rmat-audit --strict` as a hard gate. New handoff-annotated effects
without protocols, protocol feedback outside generated helpers, and terminal
classification outside generated classifiers all fail the build unless baselined.

The baseline (`xtask/rmat-baseline.toml`) must be kept current. Stale entries
(findings in the baseline that no longer reproduce) also fail strict mode.

## End State

The intended end state is:

- one canonical machine authority per domain
- generated code as the only semantic mutator
- handwritten code reduced to shell/effect plumbing
- no parallel semantic owner path
- no second source of truth

That is the standard required for machine definitions to be meaningful rather
than aspirational.
