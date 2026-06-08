# SessionTurnAdmissionMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `SessionTurnAdmissionMachine`

### Code Anchors
- `session_turn_admission_authority` (machine `SessionTurnAdmissionMachine`): `meerkat-session/src/generated/session_turn_admission.rs` — generated SessionTurnAdmissionMachine owner for the ephemeral turn-admission lifecycle: ProjectTurnAdmission, ClaimTurn, AbortClaim, BeginTurn, ResolveTurn, FinalizeTurnToShutdown, FinalizeTurnToIdle, RequestInterruptAdmittedFirst, RequestInterruptAdmittedDuplicate, RequestInterruptRunningFirst, RequestInterruptRunningDuplicate, RequestShutdownImmediateIdle, RequestShutdownImmediateAdmitted, RequestShutdownDeferredRunning, RequestShutdownDeferredCompleting, RequestShutdownAlreadyShuttingDown, AuthorizeCancelAfterBoundaryAdmitted, AuthorizeCancelAfterBoundaryRunning, AuthorizeStartTurnDispatchAdmitted, AuthorizeStartTurnDispatchShuttingDown, ResolveDispositionContentTurn, ResolveDispositionResumePendingWithBoundary, ResolveDispositionResumePendingWithoutBoundary, ResolveDispositionDirectPrompt, ResolveDispositionDirectPending, ResolveDispositionDirectNoPending, ResolveRuntimeKeepAliveEnable, ResolveRuntimeKeepAlivePreserve, and ResolveLastStartTurnPublicTerminalNoPending; effects TurnAdmissionProjected, TurnInterruptRequested, StartTurnDispatchResolved, CancelAfterBoundaryAuthorized, StartTurnDispositionResolved, StartTurnPublicTerminalResolved, RuntimeKeepAliveResolved; invariant shutdown_phase_is_not_active

### Scenarios
- `turn_admission_claim_run_finalize` — ClaimTurn, BeginTurn, ResolveTurn, FinalizeTurnToIdle, FinalizeTurnToShutdown, AbortClaim, and ProjectTurnAdmission own the Idle/Admitted/Running/Completing/ShuttingDown turn-admission phase and emit TurnAdmissionProjected without handwritten phase mutation
- `turn_admission_interrupt_and_shutdown` — RequestInterruptAdmittedFirst, RequestInterruptAdmittedDuplicate, RequestInterruptRunningFirst, RequestInterruptRunningDuplicate, RequestShutdownImmediateIdle, RequestShutdownImmediateAdmitted, RequestShutdownDeferredRunning, RequestShutdownDeferredCompleting, and RequestShutdownAlreadyShuttingDown resolve interrupt wake feedback and immediate-or-deferred shutdown under TurnInterruptRequested and TurnAdmissionProjected while preserving the shutdown-not-active invariant
- `turn_admission_dispatch_and_boundary_cancel` — AuthorizeStartTurnDispatchAdmitted, AuthorizeStartTurnDispatchShuttingDown, AuthorizeCancelAfterBoundaryAdmitted, and AuthorizeCancelAfterBoundaryRunning resolve start-turn dispatch authorization and boundary-cancel legality from the admission phase under StartTurnDispatchResolved and CancelAfterBoundaryAuthorized
- `turn_admission_start_turn_disposition` — ResolveDispositionContentTurn, ResolveDispositionResumePendingWithBoundary, ResolveDispositionResumePendingWithoutBoundary, ResolveDispositionDirectPrompt, ResolveDispositionDirectPending, ResolveDispositionDirectNoPending, and ResolveLastStartTurnPublicTerminalNoPending resolve the start-turn disposition from the execution kind, prompt content observation, and the SessionDocumentMachine-emitted PendingContinuationDisposition under StartTurnDispositionResolved and StartTurnPublicTerminalResolved; the shell mirrors the disposition and never decides
- `turn_admission_runtime_keep_alive` — ResolveRuntimeKeepAliveEnable and ResolveRuntimeKeepAlivePreserve resolve runtime keep-alive persistence from the typed keep-alive-policy-present observation under RuntimeKeepAliveResolved

### Transitions
- `ProjectTurnAdmissionIdle`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_claim_run_finalize`, `turn_admission_interrupt_and_shutdown`
- `ProjectTurnAdmissionAdmitted`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_claim_run_finalize`, `turn_admission_interrupt_and_shutdown`
- `ProjectTurnAdmissionRunning`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_claim_run_finalize`, `turn_admission_interrupt_and_shutdown`
- `ProjectTurnAdmissionCompleting`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_claim_run_finalize`, `turn_admission_interrupt_and_shutdown`
- `ProjectTurnAdmissionShuttingDown`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_claim_run_finalize`, `turn_admission_interrupt_and_shutdown`
- `ClaimTurn`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_claim_run_finalize`
- `AbortClaim`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_claim_run_finalize`
- `BeginTurn`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_claim_run_finalize`
- `ResolveTurn`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_claim_run_finalize`, `turn_admission_interrupt_and_shutdown`, `turn_admission_dispatch_and_boundary_cancel`, `turn_admission_start_turn_disposition`, `turn_admission_runtime_keep_alive`
- `FinalizeTurnToShutdown`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_claim_run_finalize`
- `FinalizeTurnToIdle`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_claim_run_finalize`
- `RequestInterruptAdmittedFirst`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`
- `RequestInterruptAdmittedDuplicate`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`
- `RequestInterruptRunningFirst`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`
- `RequestInterruptRunningDuplicate`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`
- `RequestShutdownImmediateIdle`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`
- `RequestShutdownImmediateAdmitted`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`
- `RequestShutdownDeferredRunning`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`
- `RequestShutdownDeferredCompleting`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`
- `RequestShutdownAlreadyShuttingDown`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`
- `AuthorizeCancelAfterBoundaryAdmitted`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_dispatch_and_boundary_cancel`
- `AuthorizeStartTurnDispatchAdmitted`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_dispatch_and_boundary_cancel`
- `AuthorizeStartTurnDispatchShuttingDown`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_dispatch_and_boundary_cancel`
- `AuthorizeCancelAfterBoundaryRunning`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_dispatch_and_boundary_cancel`
- `ResolveDispositionContentTurn`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`
- `ResolveDispositionResumePendingWithBoundary`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`
- `ResolveDispositionResumePendingWithoutBoundary`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`
- `ResolveDispositionDirectPrompt`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`
- `ResolveDispositionDirectPending`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`
- `ResolveDispositionDirectNoPending`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`
- `ResolveRuntimeKeepAliveEnable`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_runtime_keep_alive`
- `ResolveRuntimeKeepAliveDisable`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_runtime_keep_alive`
- `ResolveRuntimeKeepAlivePreserve`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_runtime_keep_alive`
- `ResolveLiveInterruptRequiredSteer`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`, `turn_admission_runtime_keep_alive`
- `ResolveLiveInterruptRequiredQueue`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`, `turn_admission_runtime_keep_alive`
- `ResolveLastStartTurnPublicTerminalNoPendingIdle`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`
- `ResolveLastStartTurnPublicTerminalNoPendingAdmitted`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`
- `ResolveLastStartTurnPublicTerminalNoPendingRunning`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`
- `ResolveLastStartTurnPublicTerminalNoPendingCompleting`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`
- `ResolveLastStartTurnPublicTerminalNoPendingShuttingDown`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`

### Effects
- `TurnAdmissionProjected`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_claim_run_finalize`, `turn_admission_interrupt_and_shutdown`
- `TurnInterruptRequested`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`
- `StartTurnDispatchResolved`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_dispatch_and_boundary_cancel`
- `CancelAfterBoundaryAuthorized`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_dispatch_and_boundary_cancel`
- `StartTurnDispositionResolved`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`
- `StartTurnPublicTerminalResolved`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_start_turn_disposition`
- `RuntimeKeepAliveResolved`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_runtime_keep_alive`
- `LiveInterruptRequired`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`, `turn_admission_runtime_keep_alive`

### Invariants
- `shutdown_phase_is_not_active`
  - anchors: `session_turn_admission_authority`
  - scenarios: `turn_admission_interrupt_and_shutdown`


<!-- GENERATED_COVERAGE_END -->
