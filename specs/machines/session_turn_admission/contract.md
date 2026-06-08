# SessionTurnAdmissionMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::session_turn_admission`

## State
- Phase enum: `Idle | Admitted | Running | Completing | ShuttingDown`
- `interrupt_pending`: `Bool`
- `shutdown_pending`: `Bool`
- `last_public_terminal`: `Option<StartTurnPublicTerminal>`

## Inputs
- `ProjectTurnAdmission`
- `ClaimTurn`
- `AbortClaim`
- `BeginTurn`
- `ResolveTurn`
- `FinalizeTurn`
- `RequestInterrupt`
- `RequestShutdown`
- `AuthorizeStartTurnDispatch`
- `AuthorizeCancelAfterBoundary`
- `ResolveLastStartTurnPublicTerminal`
- `ResolveRuntimeKeepAlive`(keep_alive_request: RuntimeKeepAliveRequest)
- `ResolveLiveInterruptRequired`(handling_mode: TurnHandlingMode)
- `ResolveStartTurnDisposition`(execution_kind_present: Bool, execution_kind: StartTurnExecutionKind, prompt_trimmed_text_byte_count: u64, prompt_non_text_block_count: u64, pending_continuation: PendingContinuationDisposition)

## Signals

## Effects
- `TurnAdmissionProjected`(phase: TurnAdmissionPhase, interrupt_pending: Bool, shutdown_pending: Bool, is_active: Bool)
- `TurnInterruptRequested`(wake: Bool)
- `StartTurnDispatchResolved`(authorization: StartTurnDispatchAuthorization)
- `CancelAfterBoundaryAuthorized`
- `StartTurnDispositionResolved`(disposition: StartTurnDisposition)
- `StartTurnPublicTerminalResolved`(terminal: StartTurnPublicTerminal)
- `RuntimeKeepAliveResolved`(persist_keep_alive: Bool)
- `LiveInterruptRequired`(required: Bool)

## Helpers
- `is_active_phase`(phase: TurnAdmissionPhase) -> `Bool`
- `prompt_has_content`(prompt_trimmed_text_byte_count: u64, prompt_non_text_block_count: u64) -> `Bool`

## Invariants
- `shutdown_phase_is_not_active`

## Transitions
### `ProjectTurnAdmissionIdle`
- From: `Idle`
- On: `ProjectTurnAdmission`()
- Emits: `TurnAdmissionProjected`
- To: `Idle`

### `ProjectTurnAdmissionAdmitted`
- From: `Admitted`
- On: `ProjectTurnAdmission`()
- Emits: `TurnAdmissionProjected`
- To: `Admitted`

### `ProjectTurnAdmissionRunning`
- From: `Running`
- On: `ProjectTurnAdmission`()
- Emits: `TurnAdmissionProjected`
- To: `Running`

### `ProjectTurnAdmissionCompleting`
- From: `Completing`
- On: `ProjectTurnAdmission`()
- Emits: `TurnAdmissionProjected`
- To: `Completing`

### `ProjectTurnAdmissionShuttingDown`
- From: `ShuttingDown`
- On: `ProjectTurnAdmission`()
- Emits: `TurnAdmissionProjected`
- To: `ShuttingDown`

### `ClaimTurn`
- From: `Idle`
- On: `ClaimTurn`()
- Emits: `TurnAdmissionProjected`
- To: `Admitted`

### `AbortClaim`
- From: `Admitted`
- On: `AbortClaim`()
- Emits: `TurnAdmissionProjected`
- To: `Idle`

### `BeginTurn`
- From: `Admitted`
- On: `BeginTurn`()
- Emits: `TurnAdmissionProjected`
- To: `Running`

### `ResolveTurn`
- From: `Running`
- On: `ResolveTurn`()
- Emits: `TurnAdmissionProjected`
- To: `Completing`

### `FinalizeTurnToShutdown`
- From: `Completing`
- On: `FinalizeTurn`()
- Guards:
  - ``
- Emits: `TurnAdmissionProjected`
- To: `ShuttingDown`

### `FinalizeTurnToIdle`
- From: `Completing`
- On: `FinalizeTurn`()
- Guards:
  - ``
- Emits: `TurnAdmissionProjected`
- To: `Idle`

### `RequestInterruptAdmittedFirst`
- From: `Admitted`
- On: `RequestInterrupt`()
- Guards:
  - ``
- Emits: `TurnInterruptRequested`, `TurnAdmissionProjected`
- To: `Admitted`

### `RequestInterruptAdmittedDuplicate`
- From: `Admitted`
- On: `RequestInterrupt`()
- Guards:
  - ``
- Emits: `TurnInterruptRequested`, `TurnAdmissionProjected`
- To: `Admitted`

### `RequestInterruptRunningFirst`
- From: `Running`
- On: `RequestInterrupt`()
- Guards:
  - ``
- Emits: `TurnInterruptRequested`, `TurnAdmissionProjected`
- To: `Running`

### `RequestInterruptRunningDuplicate`
- From: `Running`
- On: `RequestInterrupt`()
- Guards:
  - ``
- Emits: `TurnInterruptRequested`, `TurnAdmissionProjected`
- To: `Running`

### `RequestShutdownImmediateIdle`
- From: `Idle`
- On: `RequestShutdown`()
- Emits: `TurnAdmissionProjected`
- To: `ShuttingDown`

### `RequestShutdownImmediateAdmitted`
- From: `Admitted`
- On: `RequestShutdown`()
- Emits: `TurnAdmissionProjected`
- To: `ShuttingDown`

### `RequestShutdownDeferredRunning`
- From: `Running`
- On: `RequestShutdown`()
- Emits: `TurnAdmissionProjected`
- To: `Running`

### `RequestShutdownDeferredCompleting`
- From: `Completing`
- On: `RequestShutdown`()
- Emits: `TurnAdmissionProjected`
- To: `Completing`

### `RequestShutdownAlreadyShuttingDown`
- From: `ShuttingDown`
- On: `RequestShutdown`()
- Emits: `TurnAdmissionProjected`
- To: `ShuttingDown`

### `AuthorizeCancelAfterBoundaryAdmitted`
- From: `Admitted`
- On: `AuthorizeCancelAfterBoundary`()
- Emits: `CancelAfterBoundaryAuthorized`
- To: `Admitted`

### `AuthorizeStartTurnDispatchAdmitted`
- From: `Admitted`
- On: `AuthorizeStartTurnDispatch`()
- Emits: `StartTurnDispatchResolved`
- To: `Admitted`

### `AuthorizeStartTurnDispatchShuttingDown`
- From: `ShuttingDown`
- On: `AuthorizeStartTurnDispatch`()
- Emits: `StartTurnDispatchResolved`
- To: `ShuttingDown`

### `AuthorizeCancelAfterBoundaryRunning`
- From: `Running`
- On: `AuthorizeCancelAfterBoundary`()
- Emits: `CancelAfterBoundaryAuthorized`
- To: `Running`

### `ResolveDispositionContentTurn`
- From: `Admitted`
- On: `ResolveStartTurnDisposition`(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation)
- Guards:
  - ``
- Emits: `StartTurnDispositionResolved`
- To: `Admitted`

### `ResolveDispositionResumePendingWithBoundary`
- From: `Admitted`
- On: `ResolveStartTurnDisposition`(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation)
- Guards:
  - ``
- Emits: `StartTurnDispositionResolved`
- To: `Admitted`

### `ResolveDispositionResumePendingWithoutBoundary`
- From: `Admitted`
- On: `ResolveStartTurnDisposition`(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation)
- Guards:
  - ``
- Emits: `StartTurnDispositionResolved`, `StartTurnPublicTerminalResolved`
- To: `Admitted`

### `ResolveDispositionDirectPrompt`
- From: `Admitted`
- On: `ResolveStartTurnDisposition`(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation)
- Guards:
  - ``
- Emits: `StartTurnDispositionResolved`
- To: `Admitted`

### `ResolveDispositionDirectPending`
- From: `Admitted`
- On: `ResolveStartTurnDisposition`(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation)
- Guards:
  - ``
- Emits: `StartTurnDispositionResolved`
- To: `Admitted`

### `ResolveDispositionDirectNoPending`
- From: `Admitted`
- On: `ResolveStartTurnDisposition`(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation)
- Guards:
  - ``
- Emits: `StartTurnDispositionResolved`, `StartTurnPublicTerminalResolved`
- To: `Admitted`

### `ResolveRuntimeKeepAliveEnable`
- From: `Admitted`
- On: `ResolveRuntimeKeepAlive`(keep_alive_request)
- Guards:
  - ``
- Emits: `RuntimeKeepAliveResolved`
- To: `Admitted`

### `ResolveRuntimeKeepAliveDisable`
- From: `Admitted`
- On: `ResolveRuntimeKeepAlive`(keep_alive_request)
- Guards:
  - ``
- Emits: `RuntimeKeepAliveResolved`
- To: `Admitted`

### `ResolveRuntimeKeepAlivePreserve`
- From: `Admitted`
- On: `ResolveRuntimeKeepAlive`(keep_alive_request)
- Guards:
  - ``
- Emits: `RuntimeKeepAliveResolved`
- To: `Admitted`

### `ResolveLiveInterruptRequiredSteer`
- From: `Admitted`
- On: `ResolveLiveInterruptRequired`(handling_mode)
- Guards:
  - ``
- Emits: `LiveInterruptRequired`
- To: `Admitted`

### `ResolveLiveInterruptRequiredQueue`
- From: `Admitted`
- On: `ResolveLiveInterruptRequired`(handling_mode)
- Guards:
  - ``
- Emits: `LiveInterruptRequired`
- To: `Admitted`

### `ResolveLastStartTurnPublicTerminalNoPendingIdle`
- From: `Idle`
- On: `ResolveLastStartTurnPublicTerminal`()
- Guards:
  - ``
- Emits: `StartTurnPublicTerminalResolved`
- To: `Idle`

### `ResolveLastStartTurnPublicTerminalNoPendingAdmitted`
- From: `Admitted`
- On: `ResolveLastStartTurnPublicTerminal`()
- Guards:
  - ``
- Emits: `StartTurnPublicTerminalResolved`
- To: `Admitted`

### `ResolveLastStartTurnPublicTerminalNoPendingRunning`
- From: `Running`
- On: `ResolveLastStartTurnPublicTerminal`()
- Guards:
  - ``
- Emits: `StartTurnPublicTerminalResolved`
- To: `Running`

### `ResolveLastStartTurnPublicTerminalNoPendingCompleting`
- From: `Completing`
- On: `ResolveLastStartTurnPublicTerminal`()
- Guards:
  - ``
- Emits: `StartTurnPublicTerminalResolved`
- To: `Completing`

### `ResolveLastStartTurnPublicTerminalNoPendingShuttingDown`
- From: `ShuttingDown`
- On: `ResolveLastStartTurnPublicTerminal`()
- Guards:
  - ``
- Emits: `StartTurnPublicTerminalResolved`
- To: `ShuttingDown`

## Coverage
### Code Anchors
- `session_turn_admission_authority` (machine `SessionTurnAdmissionMachine`): `meerkat-session/src/generated/session_turn_admission.rs` — generated SessionTurnAdmissionMachine owner for the ephemeral turn-admission lifecycle: ProjectTurnAdmission, ClaimTurn, AbortClaim, BeginTurn, ResolveTurn, FinalizeTurnToShutdown, FinalizeTurnToIdle, RequestInterruptAdmittedFirst, RequestInterruptAdmittedDuplicate, RequestInterruptRunningFirst, RequestInterruptRunningDuplicate, RequestShutdownImmediateIdle, RequestShutdownImmediateAdmitted, RequestShutdownDeferredRunning, RequestShutdownDeferredCompleting, RequestShutdownAlreadyShuttingDown, AuthorizeCancelAfterBoundaryAdmitted, AuthorizeCancelAfterBoundaryRunning, AuthorizeStartTurnDispatchAdmitted, AuthorizeStartTurnDispatchShuttingDown, ResolveDispositionContentTurn, ResolveDispositionResumePendingWithBoundary, ResolveDispositionResumePendingWithoutBoundary, ResolveDispositionDirectPrompt, ResolveDispositionDirectPending, ResolveDispositionDirectNoPending, ResolveRuntimeKeepAliveEnable, ResolveRuntimeKeepAlivePreserve, and ResolveLastStartTurnPublicTerminalNoPending; effects TurnAdmissionProjected, TurnInterruptRequested, StartTurnDispatchResolved, CancelAfterBoundaryAuthorized, StartTurnDispositionResolved, StartTurnPublicTerminalResolved, RuntimeKeepAliveResolved; invariant shutdown_phase_is_not_active

### Scenarios
- `turn_admission_claim_run_finalize` — ClaimTurn, BeginTurn, ResolveTurn, FinalizeTurnToIdle, FinalizeTurnToShutdown, AbortClaim, and ProjectTurnAdmission own the Idle/Admitted/Running/Completing/ShuttingDown turn-admission phase and emit TurnAdmissionProjected without handwritten phase mutation
- `turn_admission_interrupt_and_shutdown` — RequestInterruptAdmittedFirst, RequestInterruptAdmittedDuplicate, RequestInterruptRunningFirst, RequestInterruptRunningDuplicate, RequestShutdownImmediateIdle, RequestShutdownImmediateAdmitted, RequestShutdownDeferredRunning, RequestShutdownDeferredCompleting, and RequestShutdownAlreadyShuttingDown resolve interrupt wake feedback and immediate-or-deferred shutdown under TurnInterruptRequested and TurnAdmissionProjected while preserving the shutdown-not-active invariant
- `turn_admission_dispatch_and_boundary_cancel` — AuthorizeStartTurnDispatchAdmitted, AuthorizeStartTurnDispatchShuttingDown, AuthorizeCancelAfterBoundaryAdmitted, and AuthorizeCancelAfterBoundaryRunning resolve start-turn dispatch authorization and boundary-cancel legality from the admission phase under StartTurnDispatchResolved and CancelAfterBoundaryAuthorized
- `turn_admission_start_turn_disposition` — ResolveDispositionContentTurn, ResolveDispositionResumePendingWithBoundary, ResolveDispositionResumePendingWithoutBoundary, ResolveDispositionDirectPrompt, ResolveDispositionDirectPending, ResolveDispositionDirectNoPending, and ResolveLastStartTurnPublicTerminalNoPending resolve the start-turn disposition from the execution kind, prompt content observation, and the SessionDocumentMachine-emitted PendingContinuationDisposition under StartTurnDispositionResolved and StartTurnPublicTerminalResolved; the shell mirrors the disposition and never decides
- `turn_admission_runtime_keep_alive` — ResolveRuntimeKeepAliveEnable and ResolveRuntimeKeepAlivePreserve resolve runtime keep-alive persistence from the typed keep-alive-policy-present observation under RuntimeKeepAliveResolved
