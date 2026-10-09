# ApprovalLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `2`
- Rust owner: `self` / `catalog::dsl::approval_lifecycle`

## State
- Phase enum: `Ready`
- `approval_ids`: `Set<String>`
- `approval_statuses`: `Map<String, ApprovalLifecycleStatus>`
- `approval_approve_allowed`: `Map<String, Bool>`
- `approval_deny_allowed`: `Map<String, Bool>`
- `approval_has_expiry`: `Map<String, Bool>`
- `review_ids`: `Set<String>`
- `review_statuses`: `Map<String, ReviewAttemptStatus>`
- `review_retirements`: `Map<String, ReviewRetirementReason>`

## Inputs
- `CreateApproval`(approval_id: String, approve_allowed: Bool, deny_allowed: Bool, has_expiry: Bool)
- `RestoreApproval`(approval_id: String, status: ApprovalLifecycleStatus, approve_allowed: Bool, deny_allowed: Bool, has_expiry: Bool, decision: Option<ApprovalLifecycleDecision>)
- `ObserveApprovalExpiry`(approval_id: String, expired: Bool)
- `DecideApproval`(approval_id: String, decision: ApprovalLifecycleDecision)
- `BeginReview`(review_id: String)
- `RecordReviewVerdict`(review_id: String, verdict: ReviewVerdict)
- `RecordReviewUnavailable`(review_id: String)
- `RetireReview`(review_id: String, reason: ReviewRetirementReason)
- `ConsumeReviewForEntry`(review_id: String)
- `ReleaseReview`(review_id: String)

## Signals

## Effects
- `ApprovalStatusResolved`(approval_id: String, status: ApprovalLifecycleStatus)
- `ApprovalLifecycleRejected`(approval_id: String, reason: ApprovalLifecycleRejectionReason)
- `ReviewStatusResolved`(review_id: String, status: ReviewAttemptStatus)
- `ReviewLifecycleRejected`(review_id: String, reason: ApprovalLifecycleRejectionReason)

## Helpers
- `allowed_non_empty`(approve_allowed: Bool, deny_allowed: Bool) -> `Bool`
- `is_terminal_status`(status: ApprovalLifecycleStatus) -> `Bool`

## Invariants
- `approval_maps_cover_exactly_the_registered_ids`
- `review_statuses_cover_exactly_the_review_ids`
- `review_retirement_only_for_retired_attempts`

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

### `BeginReviewRejectedDuplicate`
- From: `Ready`
- On: `BeginReview`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `BeginReviewPending`
- From: `Ready`
- On: `BeginReview`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `RecordReviewVerdictRejectedMissing`
- From: `Ready`
- On: `RecordReviewVerdict`(review_id, verdict)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `RecordReviewVerdictRejectedRetired`
- From: `Ready`
- On: `RecordReviewVerdict`(review_id, verdict)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `RecordReviewVerdictRejectedSettled`
- From: `Ready`
- On: `RecordReviewVerdict`(review_id, verdict)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `RecordReviewVerdictAllowed`
- From: `Ready`
- On: `RecordReviewVerdict`(review_id, verdict)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `RecordReviewVerdictDenied`
- From: `Ready`
- On: `RecordReviewVerdict`(review_id, verdict)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `RecordReviewVerdictEscalated`
- From: `Ready`
- On: `RecordReviewVerdict`(review_id, verdict)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `RecordReviewUnavailableRejectedMissing`
- From: `Ready`
- On: `RecordReviewUnavailable`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `RecordReviewUnavailableRejectedRetired`
- From: `Ready`
- On: `RecordReviewUnavailable`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `RecordReviewUnavailableRejectedSettled`
- From: `Ready`
- On: `RecordReviewUnavailable`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `RecordReviewUnavailable`
- From: `Ready`
- On: `RecordReviewUnavailable`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `RetireReviewRejectedMissing`
- From: `Ready`
- On: `RetireReview`(review_id, reason)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `RetireReviewRejectedRetired`
- From: `Ready`
- On: `RetireReview`(review_id, reason)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `RetireReviewRejectedSettled`
- From: `Ready`
- On: `RetireReview`(review_id, reason)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `RetireReview`
- From: `Ready`
- On: `RetireReview`(review_id, reason)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `ConsumeReviewRejectedMissing`
- From: `Ready`
- On: `ConsumeReviewForEntry`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `ConsumeReviewRejectedRetired`
- From: `Ready`
- On: `ConsumeReviewForEntry`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `ConsumeReviewRejectedUsed`
- From: `Ready`
- On: `ConsumeReviewForEntry`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `ConsumeReviewRejectedNotSatisfied`
- From: `Ready`
- On: `ConsumeReviewForEntry`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `ConsumeReviewForEntry`
- From: `Ready`
- On: `ConsumeReviewForEntry`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `ReleaseReviewRejectedMissing`
- From: `Ready`
- On: `ReleaseReview`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `ReleaseReviewRejectedPending`
- From: `Ready`
- On: `ReleaseReview`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `ReleaseReviewAllowed`
- From: `Ready`
- On: `ReleaseReview`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `ReleaseReviewDenied`
- From: `Ready`
- On: `ReleaseReview`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `ReleaseReviewEscalated`
- From: `Ready`
- On: `ReleaseReview`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `ReleaseReviewUnavailable`
- From: `Ready`
- On: `ReleaseReview`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `ReleaseReviewRetired`
- From: `Ready`
- On: `ReleaseReview`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `ReleaseReviewUsed`
- From: `Ready`
- On: `ReleaseReview`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

## Coverage
### Code Anchors
- `approval_lifecycle_authority` (machine `ApprovalLifecycleMachine`): `crates/meerkat-core/src/generated/approval_lifecycle.rs` — generated ApprovalLifecycleMachine owner for CreateRejectedEmptyAllowedDecisions, CreateRejectedAlreadyExists, CreatePending, RestoreRejectedDuplicate, RestoreRejectedEmptyAllowedDecisions, RestorePending, RestoreExpired, RestoreCancelled, RestoreApproved, RestoreDenied, RestoreRejectedInvalidRecord, ObserveExpiryRejectedMissing, ObserveExpiryExpiresPending, ObserveExpiryPendingNoop, ObserveExpiryApprovedNoop, ObserveExpiryDeniedNoop, ObserveExpiryExpiredNoop, ObserveExpiryCancelledNoop, DecideRejectedMissing, DecideRejectedExpired, DecideRejectedAlreadyDecided, DecideRejectedApproveNotAllowed, DecideRejectedDenyNotAllowed, DecideApprove, DecideDeny, BeginReviewRejectedDuplicate, BeginReviewPending, RecordReviewVerdictRejectedMissing, RecordReviewVerdictRejectedRetired, RecordReviewVerdictRejectedSettled, RecordReviewVerdictAllowed, RecordReviewVerdictDenied, RecordReviewVerdictEscalated, RecordReviewUnavailableRejectedMissing, RecordReviewUnavailableRejectedRetired, RecordReviewUnavailableRejectedSettled, RecordReviewUnavailable, RetireReviewRejectedMissing, RetireReviewRejectedRetired, RetireReviewRejectedSettled, RetireReview, ConsumeReviewRejectedMissing, ConsumeReviewRejectedRetired, ConsumeReviewRejectedUsed, ConsumeReviewRejectedNotSatisfied, ConsumeReviewForEntry, ReleaseReviewRejectedMissing, ReleaseReviewRejectedPending, ReleaseReviewAllowed, ReleaseReviewDenied, ReleaseReviewEscalated, ReleaseReviewUnavailable, ReleaseReviewRetired, ReleaseReviewUsed, ApprovalStatusResolved, ApprovalLifecycleRejected, ReviewStatusResolved, and ReviewLifecycleRejected

### Scenarios
- `approval_request_pending` — CreateRejectedEmptyAllowedDecisions, CreateRejectedAlreadyExists, and CreatePending keep request creation and Pending status projection under ApprovalStatusResolved or ApprovalLifecycleRejected
- `approval_decide_terminal` — DecideRejectedMissing, DecideRejectedExpired, DecideRejectedAlreadyDecided, DecideRejectedApproveNotAllowed, DecideRejectedDenyNotAllowed, DecideApprove, and DecideDeny move Pending approvals to Approved or Denied only when generated allowed-decision state admits the terminal decision
- `approval_expiry_feedback` — ObserveExpiryRejectedMissing, ObserveExpiryExpiresPending, ObserveExpiryPendingNoop, ObserveExpiryApprovedNoop, ObserveExpiryDeniedNoop, ObserveExpiryExpiredNoop, and ObserveExpiryCancelledNoop consume typed time observation and emit Expired or unchanged status without handwritten status mutation
- `approval_restore_consistency` — RestoreRejectedDuplicate, RestoreRejectedEmptyAllowedDecisions, RestorePending, RestoreExpired, RestoreCancelled, RestoreApproved, RestoreDenied, and RestoreRejectedInvalidRecord validate persisted status, decision audit consistency, and allowed-decision compatibility before rehydrating approval lifecycle truth
- `approval_review_attempt_retirement` — BeginReview, RecordReviewVerdict, RecordReviewUnavailable, RetireReview, ConsumeReviewForEntry, and ReleaseReview keep one process-local retained review attempt under generated status: a retired attempt rejects late verdicts and entry, an allow is consumed at most once, and release disposes only settled attempts
