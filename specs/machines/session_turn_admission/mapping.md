# SessionTurnAdmissionMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `SessionTurnAdmissionMachine`

### Code Anchors
- `turn_admission`: `meerkat-core/src/turn_execution_authority.rs` — turn admission and gating authority

### Scenarios
- `turn_admission_accept_reject` — session turn admission accepts or rejects inputs according to machine-owned policy

### Transitions
- `RequestStartTurn`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`
- `AbortAdmittedTurn`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`
- `BeginRun`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`
- `ShutdownFromAdmitted`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`
- `ResolveRun`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`
- `RequestInterrupt`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`
- `RequestShutdownFromRunning`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`
- `RequestShutdownFromCompleting`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`
- `FinalizeTurnToIdle`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`
- `FinalizeTurnToShuttingDown`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`
- `RequestShutdownFromIdle`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`

### Effects
- `WakeInterrupt`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`

### Invariants
- `interrupt_pending_only_while_active`
  - anchors: `turn_admission`
  - scenarios: `turn_admission_accept_reject`


<!-- GENERATED_COVERAGE_END -->
