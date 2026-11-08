# ToolGrantLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `ToolGrantLifecycleMachine`

### Code Anchors
- `tool_grant_lifecycle` (machine `ToolGrantLifecycleMachine`): `crates/meerkat-runtime/src/tool_grant/dsl.rs` — ToolGrantLifecycleMachine owns one scoped tool grant: issuance against a maximum lifetime, expiry at a supplied clock, revocation before the deadline, the end of the run a run-scoped grant was issued for, its end with its subject (revoked, end reason SubjectEnded, its uses refused as ended with it), and the admission of each use; expired, revoked and run-ended grants are terminal
- `tool_grant_service` (machine `ToolGrantLifecycleMachine`): `crates/meerkat-runtime/src/tool_grant/service.rs` — ToolGrantService rebuilds each grant's machine from its stored row, applies one input per adjudication at one clock reading and decides from the effects alone: issuance, revocation, the end of a subject's grants with it (a grant ended with its subject explains no refusal), settlement on list, audit read and timer fire, a boundary's open set in its run, each gated dispatch's admission in its run, and the end of a run

### Scenarios
- `tool_grant_issue_window` — tool_grant_lifecycle_machine.rs::issue_refuses_a_past_empty_or_overlong_window issues exactly the windows that are non-empty, in the future and within the maximum lifetime, and refuses a second issue of an issued record in every issued phase
- `tool_grant_use_admission_at_the_deadline` — tool_grant_lifecycle_machine.rs::use_one_millisecond_before_the_deadline_is_admitted_and_at_the_deadline_expires_and_is_refused admits a use one millisecond before the deadline, expires the grant at the first use at the deadline and refuses it, and refuses every later use
- `tool_grant_deadline_driver` — tool_grant_lifecycle_machine.rs::expire_before_the_deadline_is_a_byte_identical_noop leaves an early timer fire byte-identical and expires the grant at its deadline, and settled expired grants ignore later expiry observations; every_command_class_in_every_phase_is_total_and_only_mutates_where_intended drives Expire into a revoked grant as a byte-identical self-loop
- `tool_grant_revocation` — tool_grant_lifecycle_machine.rs::revoke_after_the_deadline_records_expiry_not_revocation revokes a live grant before its deadline, records expiry for a revoke at or after the deadline, and leaves settled grants unchanged; every_command_class_in_every_phase_is_total_and_only_mutates_where_intended refuses a use of the revoked grant
- `tool_grant_subject_end` — tool_grant_lifecycle_machine.rs::a_grant_ended_with_its_subject_refuses_its_use_as_ended_with_it ends a live grant with its subject before its deadline as revoked with the end reason SubjectEnded, refuses every later use as ended with its subject, records the expiry for an end at or after the deadline, and leaves ended grants unchanged; every_command_class_in_every_phase_is_total_and_only_mutates_where_intended drives the end into every phase
- `tool_grant_terminal_latch` — tool_grant_lifecycle_machine.rs::terminal_grants_refuse_use_after_a_clock_rollback and recover_from_state_rejects_torn_rows keep expired and revoked grants terminal under a rolled-back clock and refuse a recovered row that breaks any invariant
- `tool_grant_run_scope_admission` — tool_grant_run_scope.rs::machine_admits_a_run_scoped_use_only_in_its_run admits a run-scoped grant's use of its own run only, refuses a use outside any run without ending the grant, ends the grant at a use of another run, and admits any use of a window grant
- `tool_grant_run_end` — tool_grant_run_scope.rs::machine_ends_a_grant_only_at_the_end_of_its_own_run ends a run-scoped grant at the end of its own run before the deadline, records the expiry for a run end at or after the deadline, and leaves a window grant and a grant of another run unchanged
- `tool_grant_run_ended_latch` — tool_grant_run_scope.rs::a_run_ended_grant_is_terminal_and_latched and recover_from_state_rejects_torn_run_scope_rows keep a run-ended grant terminal under every input and a rolled-back clock, settle a run end on every ended grant, and refuse a recovered row with a torn run scope
- `tool_grant_run_ended_subject_end` — tool_grant_lifecycle_machine.rs::every_command_class_in_every_phase_is_total_and_only_mutates_where_intended ends the subject of a grant whose run already ended and leaves it unchanged, as an ended grant settles every other input

### Transitions
- `Issue`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_issue_window`, `tool_grant_run_scope_admission`
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
- `EndWithSubjectActive`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_subject_end`
- `EndWithSubjectAfterDeadline`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_subject_end`
- `EndWithSubjectSettledExpired`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_subject_end`
- `EndWithSubjectSettledRevoked`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_subject_end`
- `AdmitLive`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`, `tool_grant_run_scope_admission`
- `AdmitDue`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`, `tool_grant_run_end`
- `AdmitEndedExpired`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`, `tool_grant_terminal_latch`
- `AdmitEndedRevoked`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_revocation`, `tool_grant_terminal_latch`
- `AdmitEndedRunEnded`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_ended_latch`
- `AdmitOtherRun`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_scope_admission`
- `AdmitOutsideRun`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_scope_admission`
- `EndRunMatching`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_end`
- `EndRunAfterDeadline`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_end`
- `EndRunOther`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_end`
- `EndRunSettledExpired`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_ended_latch`
- `EndRunSettledRevoked`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_ended_latch`
- `EndRunSettledRunEnded`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_ended_latch`
- `IssueRefusedIssuedRunEnded`
  - anchors: `tool_grant_lifecycle`
  - scenarios: `tool_grant_run_ended_latch`
- `ExpireSettledRunEnded`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_ended_latch`
- `RevokeSettledRunEnded`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_ended_latch`
- `AdmitEndedWithSubject`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_subject_end`
- `EndWithSubjectSettledRunEnded`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_ended_subject_end`

### Effects
- `Issued`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_issue_window`, `tool_grant_run_scope_admission`
- `Expired`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`, `tool_grant_deadline_driver`, `tool_grant_revocation`, `tool_grant_subject_end`, `tool_grant_run_end`
- `Revoked`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_revocation`, `tool_grant_subject_end`
- `UseAdmitted`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`, `tool_grant_run_scope_admission`
- `UseRefused`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`, `tool_grant_revocation`, `tool_grant_subject_end`, `tool_grant_terminal_latch`, `tool_grant_run_scope_admission`, `tool_grant_run_end`, `tool_grant_run_ended_latch`
- `IssueRefused`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_issue_window`, `tool_grant_run_ended_latch`
- `RunEnded`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_scope_admission`, `tool_grant_run_end`

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
  - scenarios: `tool_grant_deadline_driver`, `tool_grant_terminal_latch`, `tool_grant_run_ended_latch`
- `expiry_never_precedes_its_deadline`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_use_admission_at_the_deadline`, `tool_grant_deadline_driver`, `tool_grant_terminal_latch`, `tool_grant_run_end`
- `revocation_precedes_the_deadline`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_revocation`, `tool_grant_subject_end`, `tool_grant_terminal_latch`
- `run_ended_records_its_end`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_scope_admission`, `tool_grant_run_end`, `tool_grant_run_ended_latch`, `tool_grant_run_ended_subject_end`
- `run_scope_set_only_by_issue`
  - anchors: `tool_grant_lifecycle`, `tool_grant_service`
  - scenarios: `tool_grant_run_ended_latch`


<!-- GENERATED_COVERAGE_END -->
