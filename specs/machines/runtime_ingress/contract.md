# RuntimeIngressMachine

Status: normative `0.5` machine contract

## Purpose

`RuntimeIngressMachine` owns the canonical admitted-work ledger and queue for a
single runtime instance.

It is the authoritative owner of:

- admitted input identity and total admission order
- closed runtime ingress taxonomy after admission normalization
- per-input lifecycle state
- queued order for runnable work
- runtime-authoritative multi-contributor staged runs
- per-run staged/applied/consumed bookkeeping
- terminalization of admitted inputs
- wake/process request flags derived from admitted work

It is **not** the owner of:

- raw peer transport envelopes
- trust/auth decisions
- control-plane authority commands
- runtime lifecycle state (`Idle`, `Running`, `Retired`, etc.)
- LLM/tool execution itself

In the target `0.5` architecture, items reach `RuntimeIngressMachine` only
through the runtime-owned admission surface after policy resolution.

## Scope Boundary

This machine begins **after** an input candidate has been:

- normalized into a runtime ingress kind
- assigned a canonical `InputId`
- given a policy snapshot
- accepted for runtime participation

That means rejection and deduplication happen at the admission surface, not as
steady-state transitions of `RuntimeIngressMachine`.

## Authoritative State Model

For one runtime instance, the machine state is the tuple:

- `admitted_inputs: Set<InputId>`
- `admission_order: Seq<InputId>`
- `input_kind: Map<InputId, InputKind>`
- `policy_snapshot: Map<InputId, PolicyDecision>`
- `lifecycle: Map<InputId, InputLifecycleState>`
- `terminal_outcome: Map<InputId, Option<InputTerminalOutcome>>`
- `queue: Seq<InputId>`
- `current_run: Option<RunId>`
- `current_run_contributors: Seq<InputId>`
- `last_run: Map<InputId, Option<RunId>>`
- `last_boundary_sequence: Map<InputId, Option<u64>>`
- `wake_requested: Bool`
- `process_requested: Bool`

`InputKind` is the closed `0.5` runtime ingress family:

- `WorkInput`
- `PeerInput`
- `ExternalEventInput`
- `OperationInput`
- `ContinuationInput`

`InputLifecycleState` is the closed lifecycle set:

- `Accepted`
- `Queued`
- `Staged`
- `Applied`
- `AppliedPendingConsumption`
- `Consumed`
- `Superseded`
- `Coalesced`
- `Abandoned`

Terminal states:

- `Consumed`
- `Superseded`
- `Coalesced`
- `Abandoned`

## Input Alphabet

The closed external input/command alphabet for this machine is:

- `AdmitQueued(input_id, input_kind, policy, wake, process)`
- `AdmitConsumedOnAccept(input_id, input_kind, policy)`
- `StageDrainSnapshot(run_id, contributing_input_ids)`
- `BoundaryApplied(run_id, boundary_sequence)`
- `RunCompleted(run_id)`
- `RunFailed(run_id)`
- `RunCancelled(run_id)`
- `SupersedeQueuedInput(new_input_id, old_input_id)`
- `CoalesceQueuedInputs(aggregate_input_id, source_input_ids)`
- `Retire`
- `Reset`
- `Destroy`
- `Recover`

Notes:

- `AdmitQueued` and `AdmitConsumedOnAccept` are **normalized admission**
  actions. The runtime-owned admission surface decides which one applies.
- admitted units are always individual typed inputs with their own `InputId`.
- executed units may be runtime-authoritative multi-contributor staged runs.
- `StageDrainSnapshot` stages one non-empty admission-ordered queue prefix for a
  run. Surfaces and producers do not get to define an opaque pre-batched run.
- `SupersedeQueuedInput` and `CoalesceQueuedInputs` are explicit because
  `QueueMode::{Supersede, Coalesce}` exist in the policy domain even though the
  current runtime only partially realizes them.

## Effect Family

The closed machine-boundary effect family is:

- `IngressAccepted(input_id)`
- `InputLifecycleNotice(input_id, new_state)`
- `WakeRuntime`
- `RequestImmediateProcessing`
- `ReadyForRun(run_id, contributing_input_ids)`
- `CompletionResolved(input_id, outcome)`
- `IngressNotice(kind, detail)`

These are architecture-level effect names. The current Rust implementation may
realize them as runtime events, wake flags, queue operations, receipt updates,
or completion-registry resolution.

## Transition Relation

### Admission

1. `AdmitQueued`

Preconditions:

- `input_id ∉ admitted_inputs`
- runtime admission has already accepted this item

State updates:

- add `input_id` to `admitted_inputs`
- append `input_id` to `admission_order`
- set `input_kind[input_id]`
- store `policy_snapshot[input_id]`
- set `lifecycle[input_id] = Queued`
- append `input_id` to `queue`
- if `wake = true`, set `wake_requested = true`
- if `process = true`, set `process_requested = true`

2. `AdmitConsumedOnAccept`

Preconditions:

- `input_id ∉ admitted_inputs`

State updates:

- add `input_id` to `admitted_inputs`
- append `input_id` to `admission_order`
- set `input_kind[input_id]`
- store `policy_snapshot[input_id]`
- set `lifecycle[input_id] = Consumed`
- set `terminal_outcome[input_id] = Consumed`
- do not enqueue

### Run staging and application

3. `StageDrainSnapshot`

Preconditions:

- `current_run = None`
- `contributing_input_ids` is a non-empty prefix of `queue`
- every contributor is currently `Queued`

State updates:

- remove the drained prefix from `queue`
- set `current_run = run_id`
- set `current_run_contributors = contributing_input_ids`
- for each contributor:
  - set `last_run[input_id] = run_id`
  - set `lifecycle[input_id] = Staged`
- clear `wake_requested` and `process_requested` once the owner consumes them

Normative rule:

- staged contributor order is preserved exactly as the order assigned by
  `admission_order` / `queue`

4. `BoundaryApplied`

Preconditions:

- `current_run = run_id`
- every `current_run_contributors` item is `Staged`

State updates for each contributor:

- `Staged -> AppliedPendingConsumption`
- set `last_boundary_sequence[input_id] = boundary_sequence`

5. `RunCompleted`

Preconditions:

- `current_run = run_id`
- every `current_run_contributors` item is `AppliedPendingConsumption`

State updates:

- for each contributor:
  - `AppliedPendingConsumption -> Consumed`
  - terminal outcome becomes `Consumed`
- `current_run = None`
- `current_run_contributors = []`

6. `RunFailed` / `RunCancelled`

Preconditions:

- `current_run = run_id`
- every `current_run_contributors` item is still `Staged`

State updates:

- every staged contributor for `run_id` returns to `Queued`
- reinsert rolled-back contributors at the **front** of `queue` in the exact
  recorded contributor order
- `current_run = None`
- `current_run_contributors = []`
- if queue remains non-empty after rollback, set `wake_requested = true`

Hard rule:

- `AppliedPendingConsumption -> Queued` is forbidden

### Queue-discipline normalization

7. `SupersedeQueuedInput`

Preconditions:

- `lifecycle[old_input_id] = Queued`
- `new_input_id ∈ admitted_inputs`
- supersession relation was validated by the admission layer

State updates:

- remove `old_input_id` from `queue`
- `Queued -> Superseded`
- `terminal_outcome[old_input_id] = Superseded { superseded_by = new_input_id }`

8. `CoalesceQueuedInputs`

Preconditions:

- every source input is currently `Queued`
- `aggregate_input_id ∈ admitted_inputs`
- coalescing relation was validated by the admission layer

State updates:

- remove all source input ids from `queue`
- each source input transitions `Queued -> Coalesced`
- terminal outcome becomes `Coalesced { aggregate_id = aggregate_input_id }`
- `aggregate_input_id` remains the representative queued work item

### Lifecycle control

9. `Retire`

State updates:

- does not abandon admitted work
- preserves `queue`
- does not change terminal inputs

10. `Reset`

State updates:

- all non-terminal inputs become `Abandoned { Reset }`
- `queue := []`
- `current_run := None`
- `current_run_contributors := []`
- `wake_requested := false`
- `process_requested := false`

11. `Destroy`

State updates:

- all non-terminal inputs become `Abandoned { Destroyed }`
- `queue := []`
- `current_run := None`
- `current_run_contributors := []`
- `wake_requested := false`
- `process_requested := false`

12. `Recover`

Normative `0.5` rule:

- `Accepted` may become either `Consumed` or `Queued`, depending on stored
  policy outcome
- `Staged` rolls back to `Queued` if boundary evidence is missing
- `AppliedPendingConsumption` becomes `Consumed` if boundary receipt exists,
  otherwise `Queued` is forbidden and recovery must re-drive completion from
  persisted boundary evidence
- `Queued` remains `Queued`

## Invariants

The following invariants are mandatory:

1. `queue` contains no duplicate `InputId`
2. every `InputId` in `queue` is in lifecycle state `Queued`
3. no terminal input appears in `queue`
4. `current_run = None` iff `current_run_contributors = []`
5. staged contributors are never still present in `queue`
6. an input in `AppliedPendingConsumption` must have `last_run[input_id] ≠ None`
7. `admission_order` is append-only and contains each admitted input exactly
   once
8. terminal inputs reject further lifecycle transitions
9. `AppliedPendingConsumption -> Queued` is unreachable
10. `wake_requested` and `process_requested` are derived from admitted work and
    may only be cleared by the owner that consumes those signals

## Liveness / Fairness Assumptions

Under fair scheduling:

- if `queue ≠ []` and the owning runtime is allowed to process work, some
  non-empty queue prefix is eventually staged for a run
- if `BoundaryApplied` occurs for a run, that run eventually reaches
  `RunCompleted`
- if a run fails or is cancelled before `BoundaryApplied`, its staged
  contributors eventually become queued again or terminal via an explicit
  control action

## Rust Refinement Anchors

Current implementation anchors:

- `meerkat-runtime/src/input.rs`
- `meerkat-runtime/src/input_state.rs`
- `meerkat-runtime/src/input_machine.rs`
- `meerkat-runtime/src/queue.rs`
- `meerkat-runtime/src/runtime_loop.rs`
- `meerkat-runtime/src/driver/ephemeral.rs`
- `meerkat-runtime/src/driver/persistent.rs`
- `meerkat-runtime/src/store/*`

## Known `0.4` / precursor divergences

- current runtime drivers still fold admission, ingress, and parts of runtime
  control into one owner
- current runtime execution still behaves mostly like single-item staging even
  though receipts/store state already carry contributor vectors
- `QueueMode::{Coalesce, Supersede, Priority}` exist in policy, but only a
  subset of that behavior is fully realized today
- current runtime still has `SystemGenerated` and `Projected` compatibility
  categories, but neither survives as a top-level normative `0.5` ingress
  family
