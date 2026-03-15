# RuntimeControlMachine

Status: normative `0.5` machine contract, first formal-spec draft

## Purpose

`RuntimeControlMachine` is the top-level per-runtime operational owner.

It owns:

- runtime lifecycle state
- ordinary admission surface for work candidates
- wake/process scheduling decisions
- starting and finishing runs
- explicit control-plane handling
- coordination with `RuntimeIngressMachine`

It does **not** own:

- peer transport and trust/auth
- admitted-work queue internals
- LLM/tool execution internals
- external tool surface lifecycle

## Authoritative State Model

For one runtime instance, the machine state is:

- `runtime_state: RuntimeState`
- `current_run_id: Option<RunId>`
- `pre_run_state: Option<RuntimeState>`
- `wake_pending: Bool`
- `process_pending: Bool`
- `control_plane_status: ControlPlaneStatus`

Closed `RuntimeState` set:

- `Initializing`
- `Idle`
- `Running`
- `Recovering`
- `Retired`
- `Stopped`
- `Destroyed`

Terminal states:

- `Stopped`
- `Destroyed`

Derived predicates:

- `CanAcceptOrdinaryInput := runtime_state ∈ {Idle, Running}`
- `CanProcessQueue := runtime_state ∈ {Idle, Retired}`
- `IsTerminal := runtime_state ∈ {Stopped, Destroyed}`

## Input Alphabet

The closed external input/command alphabet is:

- `Initialize`
- `SubmitCandidate(candidate_id, candidate_kind)`
- `AdmissionAccepted(candidate_id, admission_effect, wake, process)`
- `AdmissionRejected(candidate_id, reason)`
- `AdmissionDeduplicated(candidate_id, existing_input_id)`
- `BeginRun(run_id)`
- `RunBoundaryApplied(run_id)`
- `RunCompleted(run_id)`
- `RunFailed(run_id)`
- `RunCancelled(run_id)`
- `RecoverRequested`
- `RecoverySucceeded`
- `RetireRequested`
- `ResetRequested`
- `StopRequested`
- `DestroyRequested`
- `ResumeRequested`
- `ExternalToolDeltaReceived`

Notes:

- `SubmitCandidate` is the external admission surface input
- `AdmissionAccepted` is the normalized result of runtime policy resolution
- `candidate_kind` ranges over the closed runtime ingress family:
  `WorkInput`, `PeerInput`, `ExternalEventInput`, `OperationInput`, and
  `ContinuationInput`
- ordinary work candidates are not admitted in `Retired`, `Stopped`, or
  `Destroyed`
- out-of-band control-plane commands are not FIFO-ordered relative to ordinary
  ingress

## Effect Family

The closed machine-boundary effect family is:

- `ResolveAdmission(candidate_id)`
- `SubmitAdmittedIngressEffect(candidate_id, admission_effect)`
- `SignalWake`
- `SignalImmediateProcess`
- `SubmitRunPrimitive(run_id)`
- `EmitRuntimeNotice(kind, detail)`
- `ResolveCompletionAsTerminated(reason)`
- `ApplyControlPlaneCommand(command)`

## Admission contract

`RuntimeControlMachine` owns the admission surface.

For each submitted candidate, it is responsible for:

- accepting or rejecting admission in the current runtime state
- resolving policy against current runtime state
- converting that policy into a normalized ingress effect
- submitting accepted work into `RuntimeIngressMachine`
- issuing wake/process signals as needed

This means the canonical sequencing is:

`SubmitCandidate -> ResolveAdmission -> AdmissionAccepted/Rejected/Deduplicated -> Ingress effect`

## Transition Relation

### Lifecycle transitions

1. `Initialize`

- `Initializing -> Idle`

2. `BeginRun`

Preconditions:

- `runtime_state ∈ {Idle, Retired}`
- no current active run

State updates:

- `current_run_id := run_id`
- `pre_run_state := runtime_state`
- `runtime_state := Running`
- `wake_pending := false`
- `process_pending := false`

3. `RunCompleted`, `RunFailed`, `RunCancelled`

Preconditions:

- `runtime_state = Running`
- `current_run_id = run_id`

State updates:

- if `pre_run_state = Retired`, return to `Retired`
- otherwise return to `Idle`
- clear `current_run_id`
- clear `pre_run_state`

4. `RecoverRequested`

- `Idle | Running -> Recovering`

5. `RecoverySucceeded`

- `Recovering -> Idle`

6. `RetireRequested`

- `Idle | Running -> Retired`

Normative rule:

- retiring stops new ordinary admissions
- retiring does not discard already admitted work
- retired runtimes may re-enter `Running` only for queue drain
- if retire is requested during an already-active run, the runtime may enter a
  `Retired`-with-active-run condition until that run finishes; this is still a
  runtime-control state, not a second execution owner

7. `ResetRequested`

Preconditions:

- `runtime_state ≠ Running`

State updates:

- abandon pending work through ingress
- `runtime_state := Idle`
- clear run state
- clear wake/process flags

8. `StopRequested`

- `Initializing | Idle | Running | Recovering | Retired -> Stopped`

State updates:

- clear run state
- resolve pending completions as terminated

9. `DestroyRequested`

- any non-terminal state -> `Destroyed`

State updates:

- clear run state
- resolve pending completions as terminated

10. `ResumeRequested`

- `Recovering -> Idle`

### Admission and scheduling transitions

11. `SubmitCandidate`

Preconditions:

- `CanAcceptOrdinaryInput`

Effect:

- emit `ResolveAdmission(candidate_id)`

12. `AdmissionAccepted`

State updates:

- emit `SubmitAdmittedIngressEffect`
- if `wake = true`, set `wake_pending := true` and emit `SignalWake`
- if `process = true`, set `process_pending := true` and emit
  `SignalImmediateProcess`

13. `AdmissionRejected`

State updates:

- emit runtime notice if operator/runtime policy requires visibility

14. `AdmissionDeduplicated`

State updates:

- may emit completion waiter linkage or informational notice
- does not change runtime lifecycle state

15. `ExternalToolDeltaReceived`

State updates:

- no required lifecycle change
- may emit `EmitRuntimeNotice`

## Invariants

1. `runtime_state = Running` implies `current_run_id ≠ None`
2. `current_run_id ≠ None` implies `runtime_state ∈ {Running, Retired}`
3. `pre_run_state ≠ None` only while there is an active run or immediately
   around run-transition bookkeeping
4. `Retired` rejects new ordinary work admission
5. `Stopped` and `Destroyed` reject all ordinary work admission
6. only `Idle` and `Retired` may transition to `Running`
7. only `Recovering` may transition via `ResumeRequested`
8. control-plane commands may preempt ordinary scheduling
9. ordinary work does not bypass admission and mutate runtime state directly
10. external tool lifecycle reaches runtime as typed deltas rather than split
    transport-local notice shapes

## Liveness / Fairness Assumptions

Under fair scheduling:

- if `runtime_state = Idle`, admitted runnable work exists, and no terminal
  control command intervenes, the runtime eventually begins a run
- if `runtime_state = Running`, the run eventually produces
  `RunCompleted`, `RunFailed`, or `RunCancelled`
- if `StopRequested` or `DestroyRequested` is accepted, the machine eventually
  reaches the corresponding terminal state

## Rust Refinement Anchors

Current implementation anchors:

- `meerkat-runtime/src/runtime_state.rs`
- `meerkat-runtime/src/state_machine.rs`
- `meerkat-runtime/src/session_adapter.rs`
- `meerkat-runtime/src/runtime_loop.rs`
- `meerkat-runtime/src/traits.rs`
- `meerkat-runtime/src/policy_table.rs`

## Known `0.4` / precursor divergences

- today, runtime control and runtime ingress are partly folded together inside
  `RuntimeDriver` implementations and `RuntimeSessionAdapter`
- host-idle/wake/drain behavior in `meerkat-core` still exists as a legacy path
  outside the canonical runtime loop
- `TurnExecutionMachine` is not yet fully factored as a narrower execution
  owner driven by runtime control
- admission normalization and queue mutation are not yet fully separated into
  distinct machine owners
