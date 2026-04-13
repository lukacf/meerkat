# SessionTurnAdmissionMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-session` / `generated::session_turn_admission`

## State
- Phase enum: `Idle | Admitted | Running | Completing | ShuttingDown`
- `interrupt_pending`: `Bool`
- `shutdown_pending`: `Bool`

## Inputs
- `RequestStartTurn`
- `AbortAdmittedTurn`
- `BeginRun`
- `ResolveRun`
- `FinalizeTurn`
- `RequestInterrupt`
- `RequestShutdown`

## Effects
- `WakeInterrupt`

## Invariants
- `interrupt_pending_only_while_active`

## Transitions
### `RequestStartTurn`
- From: `Idle`
- On: `RequestStartTurn`()
- To: `Admitted`

### `AbortAdmittedTurn`
- From: `Admitted`
- On: `AbortAdmittedTurn`()
- To: `Idle`

### `BeginRun`
- From: `Admitted`
- On: `BeginRun`()
- To: `Running`

### `ShutdownFromAdmitted`
- From: `Admitted`
- On: `RequestShutdown`()
- To: `ShuttingDown`

### `ResolveRun`
- From: `Running`
- On: `ResolveRun`()
- To: `Completing`

### `RequestInterrupt`
- From: `Running`
- On: `RequestInterrupt`()
- Emits: `WakeInterrupt`
- To: `Running`

### `RequestShutdownFromRunning`
- From: `Running`
- On: `RequestShutdown`()
- To: `Running`

### `RequestShutdownFromCompleting`
- From: `Completing`
- On: `RequestShutdown`()
- To: `Completing`

### `FinalizeTurnToIdle`
- From: `Completing`
- On: `FinalizeTurn`()
- Guards:
  - `shutdown_pending_false`
- To: `Idle`

### `FinalizeTurnToShuttingDown`
- From: `Completing`
- On: `FinalizeTurn`()
- Guards:
  - `shutdown_pending_true`
- To: `ShuttingDown`

### `RequestShutdownFromIdle`
- From: `Idle`
- On: `RequestShutdown`()
- To: `ShuttingDown`

## Coverage
### Code Anchors
- `meerkat-core/src/turn_execution_authority.rs` — turn admission and gating authority

### Scenarios
- `turn_admission_accept_reject` — session turn admission accepts or rejects inputs according to machine-owned policy
