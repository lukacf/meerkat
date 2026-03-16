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
- wake/process scheduling intent
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

### Scenarios
- `control-preempts-ingress` — control commands preempt ordinary ingress work
- `prompt-queue` — queued user prompt sets wake/process flags conservatively and waits for the next ordinary drain
- `prompt-steer` — steering user prompt requests ASAP processing at the earliest admissible boundary
- `begin-run-complete` — runtime transitions idle to running to idle for a completed run
- `retire-stop-destroy` — runtime transitions through retire/stop/destroy commands without reopening ordinary work

### Transitions
- `Initialize`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `BeginRunFromIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `BeginRunFromRetired`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`
- `RunCompleted`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `RunFailed`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `RunCancelled`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `RecoverRequestedFromIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`
- `RecoverRequestedFromRunning`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`
- `RecoverySucceeded`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`
- `RetireRequestedFromIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`
- `RetireRequestedFromRunning`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`
- `ResetRequested`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`
- `StopRequested`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`
- `DestroyRequested`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`
- `ResumeRequested`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`
- `SubmitCandidateFromIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `SubmitCandidateFromRunning`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `AdmissionAcceptedIdleNone`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `AdmissionAcceptedIdleWake`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `AdmissionAcceptedIdleProcess`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `AdmissionAcceptedIdleWakeAndProcess`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `AdmissionAcceptedRunningNone`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `AdmissionAcceptedRunningWake`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `AdmissionAcceptedRunningProcess`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `AdmissionAcceptedRunningWakeAndProcess`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `AdmissionRejectedIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `AdmissionRejectedRunning`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `AdmissionDeduplicatedIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `AdmissionDeduplicatedRunning`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `ExternalToolDeltaReceivedIdle`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `ExternalToolDeltaReceivedRunning`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `ExternalToolDeltaReceivedRecovering`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`
- `ExternalToolDeltaReceivedRetired`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`

### Effects
- `ResolveAdmission`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `SubmitAdmittedIngressEffect`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `SubmitRunPrimitive`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `SignalWake`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `SignalImmediateProcess`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `EmitRuntimeNotice`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `ResolveCompletionAsTerminated`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`
- `ApplyControlPlaneCommand`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `control-preempts-ingress`

### Invariants
- `running_implies_active_run`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `begin-run-complete`
- `active_run_only_while_running_or_retired`
  - anchors: `runtime_state`, `runtime_state_machine`, `runtime_loop`
  - scenarios: `retire-stop-destroy`


<!-- GENERATED_COVERAGE_END -->
