# RMAT: Rust Machine Authority Theory

This document defines the authority model Meerkat uses for canonical state
machines.

The short version:

- canonical semantic truth must come from machine definitions
- generated machine authority code must be the only place allowed to mutate
  canonical state
- handwritten code may classify, route, persist, schedule, and execute effects
- handwritten code may not invent parallel semantic state or handwritten
  reducers

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

- classify raw inputs into typed machine inputs
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

The shell may observe state. It may not decide truth.

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

## End State

The intended end state is:

- one canonical machine authority per domain
- generated code as the only semantic mutator
- handwritten code reduced to shell/effect plumbing
- no parallel semantic owner path
- no second source of truth

That is the standard required for machine definitions to be meaningful rather
than aspirational.
