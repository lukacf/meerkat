# PendingContinuationAdmissionMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::pending_continuation_admission`

## State
- Phase enum: `Ready`
- `last_public_terminal`: `Option<PendingContinuationPublicTerminal>`

## Inputs
- `ResolvePendingContinuation`(session_tail: ObservedSessionTailKind, staged_tool_result_count: u64)
- `ResolveLastPendingContinuationPublicTerminal`

## Signals

## Effects
- `PendingContinuationResolved`(disposition: PendingContinuationDisposition)
- `PendingContinuationPublicTerminalResolved`(terminal: PendingContinuationPublicTerminal)

## Helpers
- `tail_has_pending_boundary`(session_tail: ObservedSessionTailKind) -> `Bool`
- `has_effective_pending_boundary`(session_tail: ObservedSessionTailKind, staged_tool_result_count: u64) -> `Bool`

## Invariants

## Transitions
### `ResolveWithBoundary`
- From: `Ready`
- On: `ResolvePendingContinuation`(session_tail, staged_tool_result_count)
- Guards:
  - ``
- Emits: `PendingContinuationResolved`
- To: `Ready`

### `ResolveWithoutBoundary`
- From: `Ready`
- On: `ResolvePendingContinuation`(session_tail, staged_tool_result_count)
- Guards:
  - ``
- Emits: `PendingContinuationResolved`, `PendingContinuationPublicTerminalResolved`
- To: `Ready`

### `ResolveLastNoPendingTerminal`
- From: `Ready`
- On: `ResolveLastPendingContinuationPublicTerminal`()
- Guards:
  - ``
- Emits: `PendingContinuationPublicTerminalResolved`
- To: `Ready`

## Coverage
### Code Anchors
- `meerkat-core/src/generated/pending_continuation_admission.rs` — generated PendingContinuationAdmissionMachine owner for ResolveWithBoundary, ResolveWithoutBoundary, ResolveLastNoPendingTerminal, PendingContinuationResolved, and PendingContinuationPublicTerminalResolved

### Scenarios
- `pending-continuation-admission-terminal` — ResolveWithBoundary admits RunPending, ResolveWithoutBoundary emits NoPendingBoundary through PendingContinuationResolved and PendingContinuationPublicTerminalResolved, and ResolveLastNoPendingTerminal replays the generated terminal witness
