# ToolGrantLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `ToolGrantLifecycleMachine`

### Code Anchors
- `tool_grant_lifecycle` (machine `ToolGrantLifecycleMachine`): `crates/meerkat-runtime/src/tool_grant/dsl.rs` — ToolGrantLifecycleMachine owns one scoped tool grant: issuance against a maximum lifetime, expiry at a supplied clock, revocation before the deadline, and the admission of each use; expired and revoked grants are terminal
- `tool_grant_service` (machine `ToolGrantLifecycleMachine`): `crates/meerkat-runtime/src/tool_grant/service.rs` — ToolGrantService rebuilds each grant's machine from its stored row, applies one input per adjudication at one clock reading and decides from the effects alone: issuance, revocation, settlement on list, audit read and timer fire, a boundary's open set, and each gated dispatch's admission

### Scenarios
- `tool_grant_issue_window` — tool_grant_lifecycle_machine.rs::issue_refuses_a_past_empty_or_overlong_window issues exactly the windows that are non-empty, in the future and within the maximum lifetime, and refuses a second issue of an issued record in every issued phase
- `tool_grant_use_admission_at_the_deadline` — tool_grant_lifecycle_machine.rs::use_one_millisecond_before_the_deadline_is_admitted_and_at_the_deadline_expires_and_is_refused admits a use one millisecond before the deadline, expires the grant at the first use at the deadline and refuses it, and refuses every later use
- `tool_grant_deadline_driver` — tool_grant_lifecycle_machine.rs::expire_before_the_deadline_is_a_byte_identical_noop leaves an early timer fire byte-identical and expires the grant at its deadline, and settled expired grants ignore later expiry observations; every_command_class_in_every_phase_is_total_and_only_mutates_where_intended drives Expire into a revoked grant as a byte-identical self-loop
- `tool_grant_revocation` — tool_grant_lifecycle_machine.rs::revoke_after_the_deadline_records_expiry_not_revocation revokes a live grant before its deadline, records expiry for a revoke at or after the deadline, and leaves settled grants unchanged; every_command_class_in_every_phase_is_total_and_only_mutates_where_intended refuses a use of the revoked grant
- `tool_grant_terminal_latch` — tool_grant_lifecycle_machine.rs::terminal_grants_refuse_use_after_a_clock_rollback and recover_from_state_rejects_torn_rows keep expired and revoked grants terminal under a rolled-back clock and refuse a recovered row that breaks any invariant

### Transitions
- `Issue`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_issue_window`
- `IssueRefusedWindow`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_issue_window`
- `IssueRefusedIssuedActive`
  - anchors: `tool_grant_lifecycle`
  - scenarios: `tool_grant_issue_window`
- `IssueRefusedIssuedExpired`
  - anchors: `tool_grant_lifecycle`
  - scenarios: `tool_grant_issue_window`
- `IssueRefusedIssuedRevoked`
  - anchors: `tool_grant_lifecycle`
  - scenarios: `tool_grant_issue_window`
- `ExpireDue`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_deadline_driver`
- `ExpireNotDue`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_deadline_driver`
- `ExpireSettledExpired`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_deadline_driver`
- `ExpireSettledRevoked`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_deadline_driver`
- `RevokeActive`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_revocation`
- `RevokeAfterDeadline`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_revocation`
- `RevokeSettledExpired`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_revocation`
- `RevokeSettledRevoked`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_revocation`
- `AdmitLive`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`
- `AdmitDue`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`
- `AdmitEndedExpired`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`, `tool_grant_terminal_latch`
- `AdmitEndedRevoked`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_revocation`, `tool_grant_terminal_latch`

### Effects
- `Issued`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_issue_window`
- `Expired`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`, `tool_grant_deadline_driver`, `tool_grant_revocation`
- `Revoked`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_revocation`
- `UseAdmitted`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`
- `UseRefused`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`, `tool_grant_revocation`, `tool_grant_terminal_latch`
- `IssueRefused`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_issue_window`

### Invariants
- `unissued_is_empty`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_issue_window`, `tool_grant_terminal_latch`
- `issued_window_is_nonempty`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_issue_window`, `tool_grant_terminal_latch`
- `active_has_no_end`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`, `tool_grant_terminal_latch`
- `ended_grants_record_their_end`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_deadline_driver`, `tool_grant_terminal_latch`
- `expiry_never_precedes_its_deadline`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`, `tool_grant_deadline_driver`, `tool_grant_terminal_latch`
- `revocation_precedes_the_deadline`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_revocation`, `tool_grant_terminal_latch`


<!-- GENERATED_COVERAGE_END -->
