# RuntimeIngressMachine Mapping Note

## Rust implementation anchors

Primary anchors in today's code:

- `meerkat-runtime/src/input.rs`
- `meerkat-runtime/src/input_state.rs`
- `meerkat-runtime/src/input_machine.rs`
- `meerkat-runtime/src/queue.rs`
- `meerkat-runtime/src/driver/ephemeral.rs`
- `meerkat-runtime/src/driver/persistent.rs`
- `meerkat-runtime/src/runtime_loop.rs`
- `meerkat-runtime/src/store/*`

## What the formal model abstracts away

- concrete input payload content
- persistence I/O and atomic store commits
- event-envelope emission details
- completion waiter transport mechanics
- exact wake-channel mechanics
- structured receipt payloads beyond contributor order and boundary evidence

The model keeps only the semantics that matter to the machine contract:

- admitted identity
- lifecycle state
- queued order
- runtime-authored staged contributor batches
- terminalization
- wake/process intent

## Important semantic choices now reflected in the model

- admitted units are individual typed inputs
- executed units may be multi-contributor staged runs
- the machine stages a non-empty queue prefix, not an opaque producer batch
- rollback restores staged contributors to the front of the queue in preserved
  contributor order
- `Projected` and top-level `SystemGenerated` are not normative `0.5` ingress
  kinds

## Where current code is only a precursor

1. admission and ingress are still folded together today

`EphemeralRuntimeDriver::accept_input()` both resolves policy and mutates the
ledger/queue. In the target `0.5` architecture, admission remains
runtime-owned but `RuntimeIngressMachine` is the named owner of the admitted
ledger, contributor batching, and queue semantics.

2. queue discipline is richer in policy than in current execution

The current runtime policy model includes `QueueMode::{Coalesce, Supersede,
Priority}`, and helper utilities exist in `coalescing.rs`, but the runtime
still behaves mostly as FIFO queueing plus ignore-on-accept. The normative
contract therefore describes the target `0.5` semantics rather than pretending
the current runtime already realizes all of them.

3. recovery is split across driver and store logic

The formal model treats recovery as a machine transition. Current Rust
implementation still splits it across runtime drivers, receipt persistence, and
store-level replay helpers.

## Proof vs test split

Best candidates for model-checked properties:

- queue/lifecycle alignment invariants
- contributor-batch legality
- terminal-state closure
- impossibility of `AppliedPendingConsumption -> Queued`
- no duplicate queue entries

Best candidates for Rust-side tests:

- persistence failure rollback behavior
- receipt/store integration
- concrete recovery/replay behavior
- event envelope contents
- completion-registry resolution behavior
