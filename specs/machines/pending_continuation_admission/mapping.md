# PendingContinuationAdmissionMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `PendingContinuationAdmissionMachine`

### Code Anchors
- `pending_continuation_authority`: `meerkat-core/src/generated/pending_continuation_admission.rs` — generated PendingContinuationAdmissionMachine owner for ResolveWithBoundary, ResolveWithoutBoundary, ResolveLastNoPendingTerminal, PendingContinuationResolved, and PendingContinuationPublicTerminalResolved

### Scenarios
- `pending-continuation-admission-terminal` — ResolveWithBoundary admits RunPending, ResolveWithoutBoundary emits NoPendingBoundary through PendingContinuationResolved and PendingContinuationPublicTerminalResolved, and ResolveLastNoPendingTerminal replays the generated terminal witness

### Transitions
- `ResolveWithBoundary`
  - anchors: `pending_continuation_authority`
  - scenarios: `pending-continuation-admission-terminal`
- `ResolveWithoutBoundary`
  - anchors: `pending_continuation_authority`
  - scenarios: `pending-continuation-admission-terminal`
- `ResolveLastNoPendingTerminal`
  - anchors: `pending_continuation_authority`
  - scenarios: `pending-continuation-admission-terminal`

### Effects
- `PendingContinuationResolved`
  - anchors: `pending_continuation_authority`
  - scenarios: `pending-continuation-admission-terminal`
- `PendingContinuationPublicTerminalResolved`
  - anchors: `pending_continuation_authority`
  - scenarios: `pending-continuation-admission-terminal`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
