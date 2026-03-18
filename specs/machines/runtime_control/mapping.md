# RuntimeControlMachine Mapping Note

## Rust implementation anchors

Primary anchors in today's code:

- `meerkat-runtime/src/runtime_state.rs`
- `meerkat-runtime/src/state_machine.rs`
- `meerkat-runtime/src/runtime_loop.rs`
- `meerkat-runtime/src/session_adapter.rs`
- `meerkat-runtime/src/traits.rs`
- `meerkat-runtime/src/policy_table.rs`

## What the formal model abstracts away

- concrete input payload content
- concrete queue contents and per-input ledger internals
- store I/O
- transport/channel implementation details
- executor internals below `RunEvent`

The model keeps:

- runtime lifecycle state
- run ownership
- handling-mode scheduling intent and the runtime admission behavior derived from it
- admission/control sequencing
- preemption rules

## Where current code is only a precursor

1. runtime control and ingress still share an implementation owner

Today, `RuntimeDriver` plus `RuntimeSessionAdapter` embody both control and
ingress concerns. The formal split is architectural: `RuntimeControlMachine`
owns lifecycle/control/admission coordination, while `RuntimeIngressMachine`
owns the admitted queue/ledger.

2. the canonical runtime path already exists, but legacy bypasses still do too

`RuntimeLoop` is the intended owner of idle/wake/dequeue/apply coordination, but
`meerkat-core` still has a legacy host-mode loop outside this path. The formal
machine contract describes the converged `0.5` path.

3. control-plane precedence is clearer in the contract than in current code

Current runtime loop handles control on a separate channel via `tokio::select!`.
The formal contract makes that precedence explicit: control is out-of-band and
may preempt ordinary work scheduling.

## Proof vs test split

Best candidates for model-checked properties:

- runtime-state transition legality
- run-id ownership invariants
- retired/stopped/destroyed admission rules
- control-plane precedence over ordinary work

Best candidates for Rust-side tests:

- channel closure behavior
- executor error propagation
- completion registry resolution on shutdown
- exact `RuntimeSessionAdapter` surface semantics

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `RuntimeControlMachine`

### Code Anchors
- `runtime_state`: `meerkat-runtime/src/runtime_state.rs` — runtime lifecycle state precursor
- `runtime_state_machine`: `meerkat-runtime/src/state_machine.rs` — runtime control reducer precursor
- `runtime_loop`: `meerkat-runtime/src/runtime_loop.rs` — control-plane select loop and run coordination precursor
- `runtime_control_plane`: `meerkat-runtime/src/control_plane.rs` — stop/preemption seam and completion-resolution precursor
- `runtime_session_adapter`: `meerkat-runtime/src/session_adapter.rs` — surface-facing lifecycle and completion owner precursor

### Scenarios
- `control-preempts-ingress` — control commands preempt ordinary ingress work
- `prompt-queue` — queued ordinary work waits for the next outer-loop turn without modifying the current run
- `prompt-steer` — steered ordinary work drains into the active run at the earliest admissible boundary
- `begin-run-complete` — runtime transitions idle to running to idle for a completed run
- `retire-stop-destroy` — runtime transitions through retire/stop/destroy commands without reopening ordinary work
- `reset-terminates-waiters` — reset abandons pending work and resolves completion waiters exactly once

### Transitions
- `Initialize`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `BeginRunFromIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `BeginRunFromRetired`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `RunCompleted`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `RunFailed`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `RunCancelled`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `RecoverRequestedFromIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `RecoverRequestedFromRunning`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `RecoverySucceeded`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `RetireRequestedFromIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `RetireRequestedFromRunning`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `ResetRequested`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `StopRequested`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `DestroyRequested`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `ResumeRequested`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `SubmitWorkFromIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `SubmitWorkFromRunning`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `AdmissionAcceptedIdleQueue`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `AdmissionAcceptedIdleSteer`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `AdmissionAcceptedRunningQueue`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `AdmissionAcceptedRunningSteer`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `AdmissionRejectedIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `AdmissionRejectedRunning`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `AdmissionDeduplicatedIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `AdmissionDeduplicatedRunning`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `ExternalToolDeltaReceivedIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `ExternalToolDeltaReceivedRunning`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `ExternalToolDeltaReceivedRecovering`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `ExternalToolDeltaReceivedRetired`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `RecycleRequestedFromRetired`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`
- `RecycleRequestedFromIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `RecycleSucceeded`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`

### Effects
- `ResolveAdmission`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `SubmitAdmittedIngressEffect`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `SubmitRunPrimitive`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `SignalWake`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `SignalImmediateProcess`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `EmitRuntimeNotice`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `ResolveCompletionAsTerminated`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `ApplyControlPlaneCommand`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`
- `InitiateRecycle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `control-preempts-ingress`

### Invariants
- `running_implies_active_run`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `begin-run-complete`
- `active_run_only_while_running_or_retired`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`, `runtime_control_plane`, `runtime_session_adapter`
  - scenarios: `retire-stop-destroy`, `reset-terminates-waiters`


<!-- GENERATED_COVERAGE_END -->
