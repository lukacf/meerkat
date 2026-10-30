# ToolGrantLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::tool_grant_lifecycle`

## State
- Phase enum: `Unissued | Active | Expired | Revoked`
- `grant_id`: `String`
- `issued_at_ms`: `u64`
- `expires_at_ms`: `u64`
- `ended_at_ms`: `Option<u64>`
- `end_reason`: `Option<ToolGrantEndReason>`

## Inputs
- `Issue`(grant_id: String, now_ms: u64, expires_at_ms: u64, max_lifetime_ms: u64)
- `Expire`(now_ms: u64)
- `Revoke`(now_ms: u64)
- `AdmitUse`(now_ms: u64)

## Signals

## Effects
- `Issued`(grant_id: String, expires_at_ms: u64)
- `Expired`(grant_id: String, expires_at_ms: u64, observed_at_ms: u64)
- `Revoked`(grant_id: String, observed_at_ms: u64)
- `UseAdmitted`(grant_id: String)
- `UseRefused`(grant_id: String, reason: ToolGrantUseRefusal)
- `IssueRefused`(reason: ToolGrantIssueRefusal)

## Invariants
- `unissued_is_empty`
- `issued_window_is_nonempty`
- `active_has_no_end`
- `ended_grants_record_their_end`
- `expiry_never_precedes_its_deadline`
- `revocation_precedes_the_deadline`

## Transitions
### `Issue`
- From: `Unissued`
- On: `Issue`(grant_id, now_ms, expires_at_ms, max_lifetime_ms)
- Guards:
  - `named_grant`
  - `window_is_not_past_or_empty`
  - `window_within_maximum_lifetime`
- Emits: `Issued`
- To: `Active`

### `IssueRefusedWindow`
- From: `Unissued`
- On: `Issue`(grant_id, now_ms, expires_at_ms, max_lifetime_ms)
- Guards:
  - `window_is_past_empty_or_overlong`
- Emits: `IssueRefused`
- To: `Unissued`

### `IssueRefusedIssuedActive`
- From: `Active`
- On: `Issue`(grant_id, now_ms, expires_at_ms, max_lifetime_ms)
- Emits: `IssueRefused`
- To: `Active`

### `IssueRefusedIssuedExpired`
- From: `Expired`
- On: `Issue`(grant_id, now_ms, expires_at_ms, max_lifetime_ms)
- Emits: `IssueRefused`
- To: `Expired`

### `IssueRefusedIssuedRevoked`
- From: `Revoked`
- On: `Issue`(grant_id, now_ms, expires_at_ms, max_lifetime_ms)
- Emits: `IssueRefused`
- To: `Revoked`

### `ExpireDue`
- From: `Active`
- On: `Expire`(now_ms)
- Guards:
  - `deadline_reached`
- Emits: `Expired`
- To: `Expired`

### `ExpireNotDue`
- From: `Active`
- On: `Expire`(now_ms)
- Guards:
  - `deadline_not_reached`
- To: `Active`

### `ExpireSettledExpired`
- From: `Expired`
- On: `Expire`(now_ms)
- To: `Expired`

### `ExpireSettledRevoked`
- From: `Revoked`
- On: `Expire`(now_ms)
- To: `Revoked`

### `RevokeActive`
- From: `Active`
- On: `Revoke`(now_ms)
- Guards:
  - `deadline_not_reached`
- Emits: `Revoked`
- To: `Revoked`

### `RevokeAfterDeadline`
- From: `Active`
- On: `Revoke`(now_ms)
- Guards:
  - `deadline_reached`
- Emits: `Expired`
- To: `Expired`

### `RevokeSettledExpired`
- From: `Expired`
- On: `Revoke`(now_ms)
- To: `Expired`

### `RevokeSettledRevoked`
- From: `Revoked`
- On: `Revoke`(now_ms)
- To: `Revoked`

### `AdmitLive`
- From: `Active`
- On: `AdmitUse`(now_ms)
- Guards:
  - `deadline_not_reached`
- Emits: `UseAdmitted`
- To: `Active`

### `AdmitDue`
- From: `Active`
- On: `AdmitUse`(now_ms)
- Guards:
  - `deadline_reached`
- Emits: `Expired`, `UseRefused`
- To: `Expired`

### `AdmitEndedExpired`
- From: `Expired`
- On: `AdmitUse`(now_ms)
- Emits: `UseRefused`
- To: `Expired`

### `AdmitEndedRevoked`
- From: `Revoked`
- On: `AdmitUse`(now_ms)
- Emits: `UseRefused`
- To: `Revoked`

## Coverage
### Code Anchors
- `tool_grant_lifecycle` (machine `ToolGrantLifecycleMachine`): `crates/meerkat-runtime/src/tool_grant/dsl.rs` — ToolGrantLifecycleMachine owns one scoped tool grant: issuance against a maximum lifetime, expiry at a supplied clock, revocation before the deadline, and the admission of each use; expired and revoked grants are terminal
- `tool_grant_service` (machine `ToolGrantLifecycleMachine`): `crates/meerkat-runtime/src/tool_grant/service.rs` — ToolGrantService rebuilds each grant's machine from its stored row, applies one input per adjudication at one clock reading and decides from the effects alone: issuance, revocation, settlement on list, audit read and timer fire, a boundary's open set, and each gated dispatch's admission

### Scenarios
- `tool_grant_issue_window` — tool_grant_lifecycle_machine.rs::issue_refuses_a_past_empty_or_overlong_window issues exactly the windows that are non-empty, in the future and within the maximum lifetime, and refuses a second issue of an issued record in every issued phase
- `tool_grant_use_admission_at_the_deadline` — tool_grant_lifecycle_machine.rs::use_one_millisecond_before_the_deadline_is_admitted_and_at_the_deadline_expires_and_is_refused admits a use one millisecond before the deadline, expires the grant at the first use at the deadline and refuses it, and refuses every later use
- `tool_grant_deadline_driver` — tool_grant_lifecycle_machine.rs::expire_before_the_deadline_is_a_byte_identical_noop leaves an early timer fire byte-identical and expires the grant at its deadline, and settled expired grants ignore later expiry observations; every_command_class_in_every_phase_is_total_and_only_mutates_where_intended drives Expire into a revoked grant as a byte-identical self-loop
- `tool_grant_revocation` — tool_grant_lifecycle_machine.rs::revoke_after_the_deadline_records_expiry_not_revocation revokes a live grant before its deadline, records expiry for a revoke at or after the deadline, and leaves settled grants unchanged; every_command_class_in_every_phase_is_total_and_only_mutates_where_intended refuses a use of the revoked grant
- `tool_grant_terminal_latch` — tool_grant_lifecycle_machine.rs::terminal_grants_refuse_use_after_a_clock_rollback and recover_from_state_rejects_torn_rows keep expired and revoked grants terminal under a rolled-back clock and refuse a recovered row that breaks any invariant
