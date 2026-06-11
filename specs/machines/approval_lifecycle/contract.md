# ApprovalLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::approval_lifecycle`

## State
- Phase enum: `Ready`
- `approval_ids`: `Set<String>`
- `approval_statuses`: `Map<String, ApprovalLifecycleStatus>`
- `approval_approve_allowed`: `Map<String, Bool>`
- `approval_deny_allowed`: `Map<String, Bool>`
- `approval_has_expiry`: `Map<String, Bool>`

## Inputs
- `CreateApproval`(approval_id: String, approve_allowed: Bool, deny_allowed: Bool, has_expiry: Bool)
- `RestoreApproval`(approval_id: String, status: ApprovalLifecycleStatus, approve_allowed: Bool, deny_allowed: Bool, has_expiry: Bool, decision: Option<ApprovalLifecycleDecision>)
- `ObserveApprovalExpiry`(approval_id: String, expired: Bool)
- `DecideApproval`(approval_id: String, decision: ApprovalLifecycleDecision)

## Signals

## Effects
- `ApprovalStatusResolved`(approval_id: String, status: ApprovalLifecycleStatus)
- `ApprovalLifecycleRejected`(approval_id: String, reason: ApprovalLifecycleRejectionReason)

## Helpers
- `allowed_non_empty`(approve_allowed: Bool, deny_allowed: Bool) -> `Bool`
- `is_terminal_status`(status: ApprovalLifecycleStatus) -> `Bool`

## Invariants

## Transitions
### `CreateRejectedEmptyAllowedDecisions`
- From: `Ready`
- On: `CreateApproval`(approval_id, approve_allowed, deny_allowed, has_expiry)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `CreateRejectedAlreadyExists`
- From: `Ready`
- On: `CreateApproval`(approval_id, approve_allowed, deny_allowed, has_expiry)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `CreatePending`
- From: `Ready`
- On: `CreateApproval`(approval_id, approve_allowed, deny_allowed, has_expiry)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `RestoreRejectedDuplicate`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `RestoreRejectedEmptyAllowedDecisions`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `RestorePending`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `RestoreExpired`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `RestoreCancelled`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `RestoreApproved`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `RestoreDenied`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `RestoreRejectedInvalidRecord`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `ObserveExpiryRejectedMissing`
- From: `Ready`
- On: `ObserveApprovalExpiry`(approval_id, expired)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `ObserveExpiryExpiresPending`
- From: `Ready`
- On: `ObserveApprovalExpiry`(approval_id, expired)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `ObserveExpiryPendingNoop`
- From: `Ready`
- On: `ObserveApprovalExpiry`(approval_id, expired)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `ObserveExpiryApprovedNoop`
- From: `Ready`
- On: `ObserveApprovalExpiry`(approval_id, expired)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `ObserveExpiryDeniedNoop`
- From: `Ready`
- On: `ObserveApprovalExpiry`(approval_id, expired)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `ObserveExpiryExpiredNoop`
- From: `Ready`
- On: `ObserveApprovalExpiry`(approval_id, expired)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `ObserveExpiryCancelledNoop`
- From: `Ready`
- On: `ObserveApprovalExpiry`(approval_id, expired)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `DecideRejectedMissing`
- From: `Ready`
- On: `DecideApproval`(approval_id, decision)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `DecideRejectedExpired`
- From: `Ready`
- On: `DecideApproval`(approval_id, decision)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `DecideRejectedAlreadyDecided`
- From: `Ready`
- On: `DecideApproval`(approval_id, decision)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `DecideRejectedApproveNotAllowed`
- From: `Ready`
- On: `DecideApproval`(approval_id, decision)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `DecideRejectedDenyNotAllowed`
- From: `Ready`
- On: `DecideApproval`(approval_id, decision)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `DecideApprove`
- From: `Ready`
- On: `DecideApproval`(approval_id, decision)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `DecideDeny`
- From: `Ready`
- On: `DecideApproval`(approval_id, decision)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

## Coverage
### Code Anchors
- `approval_lifecycle_authority` (machine `ApprovalLifecycleMachine`): `meerkat-core/src/generated/approval_lifecycle.rs` — generated ApprovalLifecycleMachine owner for CreateRejectedEmptyAllowedDecisions, CreateRejectedAlreadyExists, CreatePending, RestoreRejectedDuplicate, RestoreRejectedEmptyAllowedDecisions, RestorePending, RestoreExpired, RestoreCancelled, RestoreApproved, RestoreDenied, RestoreRejectedInvalidRecord, ObserveExpiryRejectedMissing, ObserveExpiryExpiresPending, ObserveExpiryPendingNoop, ObserveExpiryApprovedNoop, ObserveExpiryDeniedNoop, ObserveExpiryExpiredNoop, ObserveExpiryCancelledNoop, DecideRejectedMissing, DecideRejectedExpired, DecideRejectedAlreadyDecided, DecideRejectedApproveNotAllowed, DecideRejectedDenyNotAllowed, DecideApprove, DecideDeny, ApprovalStatusResolved, and ApprovalLifecycleRejected

### Scenarios
- `approval_request_pending` — CreateRejectedEmptyAllowedDecisions, CreateRejectedAlreadyExists, and CreatePending keep request creation and Pending status projection under ApprovalStatusResolved or ApprovalLifecycleRejected
- `approval_decide_terminal` — DecideRejectedMissing, DecideRejectedExpired, DecideRejectedAlreadyDecided, DecideRejectedApproveNotAllowed, DecideRejectedDenyNotAllowed, DecideApprove, and DecideDeny move Pending approvals to Approved or Denied only when generated allowed-decision state admits the terminal decision
- `approval_expiry_feedback` — ObserveExpiryRejectedMissing, ObserveExpiryExpiresPending, ObserveExpiryPendingNoop, ObserveExpiryApprovedNoop, ObserveExpiryDeniedNoop, ObserveExpiryExpiredNoop, and ObserveExpiryCancelledNoop consume typed time observation and emit Expired or unchanged status without handwritten status mutation
- `approval_restore_consistency` — RestoreRejectedDuplicate, RestoreRejectedEmptyAllowedDecisions, RestorePending, RestoreExpired, RestoreCancelled, RestoreApproved, RestoreDenied, and RestoreRejectedInvalidRecord validate persisted status, decision audit consistency, and allowed-decision compatibility before rehydrating approval lifecycle truth
