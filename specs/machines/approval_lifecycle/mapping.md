# ApprovalLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `ApprovalLifecycleMachine`

### Code Anchors
- `approval_lifecycle_authority` (machine `ApprovalLifecycleMachine`): `meerkat-core/src/generated/approval_lifecycle.rs` — generated ApprovalLifecycleMachine owner for CreateRejectedEmptyAllowedDecisions, CreateRejectedAlreadyExists, CreatePending, RestoreRejectedDuplicate, RestoreRejectedEmptyAllowedDecisions, RestorePending, RestoreExpired, RestoreCancelled, RestoreApproved, RestoreDenied, RestoreRejectedInvalidRecord, ObserveExpiryRejectedMissing, ObserveExpiryExpiresPending, ObserveExpiryPendingNoop, ObserveExpiryApprovedNoop, ObserveExpiryDeniedNoop, ObserveExpiryExpiredNoop, ObserveExpiryCancelledNoop, DecideRejectedMissing, DecideRejectedExpired, DecideRejectedAlreadyDecided, DecideRejectedApproveNotAllowed, DecideRejectedDenyNotAllowed, DecideApprove, DecideDeny, ApprovalStatusResolved, and ApprovalLifecycleRejected

### Scenarios
- `approval_request_pending` — CreateRejectedEmptyAllowedDecisions, CreateRejectedAlreadyExists, and CreatePending keep request creation and Pending status projection under ApprovalStatusResolved or ApprovalLifecycleRejected
- `approval_decide_terminal` — DecideRejectedMissing, DecideRejectedExpired, DecideRejectedAlreadyDecided, DecideRejectedApproveNotAllowed, DecideRejectedDenyNotAllowed, DecideApprove, and DecideDeny move Pending approvals to Approved or Denied only when generated allowed-decision state admits the terminal decision
- `approval_expiry_feedback` — ObserveExpiryRejectedMissing, ObserveExpiryExpiresPending, ObserveExpiryPendingNoop, ObserveExpiryApprovedNoop, ObserveExpiryDeniedNoop, ObserveExpiryExpiredNoop, and ObserveExpiryCancelledNoop consume typed time observation and emit Expired or unchanged status without handwritten status mutation
- `approval_restore_consistency` — RestoreRejectedDuplicate, RestoreRejectedEmptyAllowedDecisions, RestorePending, RestoreExpired, RestoreCancelled, RestoreApproved, RestoreDenied, and RestoreRejectedInvalidRecord validate persisted status, decision audit consistency, and allowed-decision compatibility before rehydrating approval lifecycle truth

### Transitions
- `CreateRejectedEmptyAllowedDecisions`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_request_pending`
- `CreateRejectedAlreadyExists`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_request_pending`
- `CreatePending`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_request_pending`
- `RestoreRejectedDuplicate`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_restore_consistency`
- `RestoreRejectedEmptyAllowedDecisions`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_restore_consistency`
- `RestorePending`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_restore_consistency`
- `RestoreExpired`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_restore_consistency`
- `RestoreCancelled`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_restore_consistency`
- `RestoreApproved`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_restore_consistency`
- `RestoreDenied`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_restore_consistency`
- `RestoreRejectedInvalidRecord`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_restore_consistency`
- `ObserveExpiryRejectedMissing`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_expiry_feedback`
- `ObserveExpiryExpiresPending`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_expiry_feedback`
- `ObserveExpiryPendingNoop`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_expiry_feedback`
- `ObserveExpiryApprovedNoop`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_expiry_feedback`
- `ObserveExpiryDeniedNoop`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_expiry_feedback`
- `ObserveExpiryExpiredNoop`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_expiry_feedback`
- `ObserveExpiryCancelledNoop`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_expiry_feedback`
- `DecideRejectedMissing`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_decide_terminal`
- `DecideRejectedExpired`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_decide_terminal`
- `DecideRejectedAlreadyDecided`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_decide_terminal`
- `DecideRejectedApproveNotAllowed`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_decide_terminal`
- `DecideRejectedDenyNotAllowed`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_decide_terminal`
- `DecideApprove`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_decide_terminal`
- `DecideDeny`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_decide_terminal`

### Effects
- `ApprovalStatusResolved`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_request_pending`
- `ApprovalLifecycleRejected`
  - anchors: `approval_lifecycle_authority`
  - scenarios: `approval_request_pending`, `approval_restore_consistency`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
