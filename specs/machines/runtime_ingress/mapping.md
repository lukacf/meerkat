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
- handling-mode semantics and the runtime-owned steer-drain trigger state they derive

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

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `RuntimeIngressMachine`

### Code Anchors
- `runtime_input_taxonomy`: `meerkat-runtime/src/input.rs` — runtime ingress input taxonomy precursor
- `runtime_queue`: `meerkat-runtime/src/queue.rs` — ordered queue discipline precursor
- `runtime_ephemeral_driver`: `meerkat-runtime/src/driver/ephemeral.rs` — ephemeral ingress mutation precursor
- `runtime_persistent_driver`: `meerkat-runtime/src/driver/persistent.rs` — persistent ingress/recovery precursor

### Scenarios
- `admit-and-stage-prefix` — individually admitted inputs form a runtime-authored staged prefix
- `prompt-queue` — queued user prompt enters ingress without immediate processing when already running
- `prompt-steer` — steering user prompt enters ingress with immediate-processing intent
- `rollback-on-failure` — failed or cancelled run restores staged contributors to the queue front
- `recover-retire-reset-destroy` — recovery and lifecycle terminalization preserve contributor legality

### Transitions
- `AdmitQueuedQueue`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `AdmitQueuedSteer`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `AdmitConsumedOnAccept`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `StageDrainSnapshot`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `BoundaryApplied`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `RunCompleted`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `RunFailed`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `rollback-on-failure`
- `RunCancelled`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `rollback-on-failure`
- `SupersedeQueuedInput`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `CoalesceQueuedInputs`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `Retire`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `recover-retire-reset-destroy`
- `ResetFromActive`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `recover-retire-reset-destroy`
- `ResetFromRetired`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `recover-retire-reset-destroy`
- `Destroy`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `recover-retire-reset-destroy`
- `RecoverFromActive`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `recover-retire-reset-destroy`
- `RecoverFromRetired`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `recover-retire-reset-destroy`

### Effects
- `IngressAccepted`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `ReadyForRun`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `InputLifecycleNotice`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `WakeRuntime`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `RequestImmediateProcessing`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `CompletionResolved`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `IngressNotice`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`

### Invariants
- `queue_entries_are_queued`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `queued_inputs_preserve_content_shape`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `admitted_inputs_preserve_correlation_slots`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `terminal_inputs_do_not_appear_in_queue`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `current_run_matches_contributor_presence`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `staged_contributors_are_not_queued`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`
- `applied_pending_consumption_has_last_run`
  - anchors: `runtime_input_taxonomy`, `runtime_queue`, `runtime_ephemeral_driver`, `runtime_persistent_driver`
  - scenarios: `admit-and-stage-prefix`


<!-- GENERATED_COVERAGE_END -->
