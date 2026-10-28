# ApprovalLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `6`
- Rust owner: `self` / `catalog::dsl::approval_lifecycle`

## State
- Phase enum: `Ready`
- `approval_ids`: `Set<String>`
- `approval_statuses`: `Map<String, ApprovalLifecycleStatus>`
- `approval_approve_allowed`: `Map<String, Bool>`
- `approval_deny_allowed`: `Map<String, Bool>`
- `approval_has_expiry`: `Map<String, Bool>`
- `approval_consent`: `Map<String, Bool>`
- `approval_consent_authenticated`: `Map<String, Bool>`
- `approval_consent_consumed`: `Map<String, Bool>`
- `approval_consent_review`: `Map<String, String>`
- `review_consent_approval`: `Map<String, String>`
- `review_ids`: `Set<String>`
- `review_statuses`: `Map<String, ReviewAttemptStatus>`
- `review_retirements`: `Map<String, ReviewRetirementReason>`
- `review_entered_phase`: `Map<String, EnteredEffectPhase>`
- `review_entered_steps`: `Map<String, u64>`

## Inputs
- `CreateApproval`(approval_id: String, approve_allowed: Bool, deny_allowed: Bool, has_expiry: Bool)
- `CreateConsentApproval`(approval_id: String, review_id: String, approve_allowed: Bool, deny_allowed: Bool, has_expiry: Bool)
- `RestoreApproval`(approval_id: String, status: ApprovalLifecycleStatus, approve_allowed: Bool, deny_allowed: Bool, has_expiry: Bool, decision: Option<ApprovalLifecycleDecision>, consent: Bool, authenticated: Bool, consumed: Bool)
- `ReconcileApproval`(approval_id: String, status: ApprovalLifecycleStatus, approve_allowed: Bool, deny_allowed: Bool, has_expiry: Bool, decision: Option<ApprovalLifecycleDecision>, consent: Bool, authenticated: Bool, consumed: Bool)
- `ObserveApprovalExpiry`(approval_id: String, expired: Bool)
- `DecideApproval`(approval_id: String, decision: ApprovalLifecycleDecision)
- `DecideConsent`(approval_id: String, review_id: String, decision: ApprovalLifecycleDecision, authenticated: Bool)
- `ConsumeConsentForEntry`(approval_id: String, review_id: String)
- `WithdrawConsent`(approval_id: String, review_id: String, reason: ReviewRetirementReason)
- `SettleUnlinkedConsent`(approval_id: String)
- `BeginReview`(review_id: String)
- `RecordReviewVerdict`(review_id: String, verdict: ReviewVerdict)
- `RecordReviewUnavailable`(review_id: String)
- `RetireReview`(review_id: String, reason: ReviewRetirementReason)
- `ConsumeReviewForEntry`(review_id: String)
- `ReleaseReview`(review_id: String)
- `AdmitEnteredStep`(review_id: String)
- `FinishEnteredStep`(review_id: String)

## Signals

## Effects
- `ApprovalStatusResolved`(approval_id: String, status: ApprovalLifecycleStatus)
- `ApprovalLifecycleRejected`(approval_id: String, reason: ApprovalLifecycleRejectionReason)
- `ReviewStatusResolved`(review_id: String, status: ReviewAttemptStatus)
- `ReviewLifecycleRejected`(review_id: String, reason: ApprovalLifecycleRejectionReason)

## Helpers
- `allowed_non_empty`(approve_allowed: Bool, deny_allowed: Bool) -> `Bool`
- `is_terminal_status`(status: ApprovalLifecycleStatus) -> `Bool`
- `restored_record_valid`(status: ApprovalLifecycleStatus, approve_allowed: Bool, deny_allowed: Bool, decision: Option<ApprovalLifecycleDecision>, consent: Bool, authenticated: Bool, consumed: Bool) -> `Bool`

## Invariants
- `approval_maps_cover_exactly_the_registered_ids`
- `review_statuses_cover_exactly_the_review_ids`
- `approval_consent_covers_exactly_the_approval_ids`
- `review_retirement_only_for_retired_attempts`
- `consent_link_is_a_bijection_over_live_attempts`
- `linked_attempt_allowed_iff_approved`
- `consumed_consent_is_approved_authenticated_and_unlinked`
- `entered_state_covers_exactly_the_used_attempts`
- `closing_entered_effect_has_a_step_in_flight`

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

### `CreateConsentRejectedEmptyAllowedDecisions`
- From: `Ready`
- On: `CreateConsentApproval`(approval_id, review_id, approve_allowed, deny_allowed, has_expiry)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `CreateConsentRejectedAlreadyExists`
- From: `Ready`
- On: `CreateConsentApproval`(approval_id, review_id, approve_allowed, deny_allowed, has_expiry)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `CreateConsentPending`
- From: `Ready`
- On: `CreateConsentApproval`(approval_id, review_id, approve_allowed, deny_allowed, has_expiry)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `RestoreRejectedDuplicate`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `RestoreRejectedEmptyAllowedDecisions`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `RestorePending`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `RestoreExpired`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `RestoreCancelled`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `RestoreApproved`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `RestoreDenied`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `RestoreRejectedInvalidRecord`
- From: `Ready`
- On: `RestoreApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `ReconcileRejectedMissing`
- From: `Ready`
- On: `ReconcileApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `ReconcileRejectedLinked`
- From: `Ready`
- On: `ReconcileApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `ReconcileRejectedInvalidRecord`
- From: `Ready`
- On: `ReconcileApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `ReconcileAppliedPending`
- From: `Ready`
- On: `ReconcileApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `ReconcileAppliedExpired`
- From: `Ready`
- On: `ReconcileApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `ReconcileAppliedCancelled`
- From: `Ready`
- On: `ReconcileApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `ReconcileAppliedApproved`
- From: `Ready`
- On: `ReconcileApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `ReconcileAppliedDenied`
- From: `Ready`
- On: `ReconcileApproval`(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
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

### `DecideRejectedAuthenticationRequired`
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

### `DecideConsentRejectedMissing`
- From: `Ready`
- On: `DecideConsent`(approval_id, review_id, decision, authenticated)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `DecideConsentRejectedMismatch`
- From: `Ready`
- On: `DecideConsent`(approval_id, review_id, decision, authenticated)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `DecideConsentRejectedExpired`
- From: `Ready`
- On: `DecideConsent`(approval_id, review_id, decision, authenticated)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `DecideConsentRejectedAlreadyDecided`
- From: `Ready`
- On: `DecideConsent`(approval_id, review_id, decision, authenticated)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `DecideConsentRejectedAuthenticationRequired`
- From: `Ready`
- On: `DecideConsent`(approval_id, review_id, decision, authenticated)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `DecideConsentRejectedNotAllowed`
- From: `Ready`
- On: `DecideConsent`(approval_id, review_id, decision, authenticated)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `DecideConsentApprove`
- From: `Ready`
- On: `DecideConsent`(approval_id, review_id, decision, authenticated)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `DecideConsentDeny`
- From: `Ready`
- On: `DecideConsent`(approval_id, review_id, decision, authenticated)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `ConsumeConsentRejectedMissing`
- From: `Ready`
- On: `ConsumeConsentForEntry`(approval_id, review_id)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `ConsumeConsentRejectedAlreadyConsumed`
- From: `Ready`
- On: `ConsumeConsentForEntry`(approval_id, review_id)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `ConsumeConsentRejectedMismatch`
- From: `Ready`
- On: `ConsumeConsentForEntry`(approval_id, review_id)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `ConsumeConsentRejectedNotSatisfied`
- From: `Ready`
- On: `ConsumeConsentForEntry`(approval_id, review_id)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `ConsumeConsentForEntry`
- From: `Ready`
- On: `ConsumeConsentForEntry`(approval_id, review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `WithdrawConsentRejectedMissing`
- From: `Ready`
- On: `WithdrawConsent`(approval_id, review_id, reason)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `WithdrawConsentRejectedMismatch`
- From: `Ready`
- On: `WithdrawConsent`(approval_id, review_id, reason)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `WithdrawConsentPending`
- From: `Ready`
- On: `WithdrawConsent`(approval_id, review_id, reason)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `WithdrawConsentSettled`
- From: `Ready`
- On: `WithdrawConsent`(approval_id, review_id, reason)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `SettleUnlinkedRejectedMissing`
- From: `Ready`
- On: `SettleUnlinkedConsent`(approval_id)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `SettleUnlinkedRejectedNotConsent`
- From: `Ready`
- On: `SettleUnlinkedConsent`(approval_id)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `SettleUnlinkedRejectedLinked`
- From: `Ready`
- On: `SettleUnlinkedConsent`(approval_id)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `SettleUnlinkedRejectedConsumed`
- From: `Ready`
- On: `SettleUnlinkedConsent`(approval_id)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `SettleUnlinkedRejectedExpired`
- From: `Ready`
- On: `SettleUnlinkedConsent`(approval_id)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `SettleUnlinkedRejectedSettled`
- From: `Ready`
- On: `SettleUnlinkedConsent`(approval_id)
- Guards:
  - ``
- Emits: `ApprovalLifecycleRejected`
- To: `Ready`

### `SettleUnlinkedPending`
- From: `Ready`
- On: `SettleUnlinkedConsent`(approval_id)
- Guards:
  - ``
- Emits: `ApprovalStatusResolved`
- To: `Ready`

### `SettleUnlinkedApproved`
- From: `Ready`
- On: `SettleUnlinkedConsent`(approval_id)
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

### `RecordReviewVerdictRejectedConsentLinked`
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

### `RecordReviewUnavailableRejectedConsentLinked`
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

### `RetireReviewRejectedConsentLinked`
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

### `ConsumeReviewRejectedConsentLinked`
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

### `ReleaseReviewRejectedConsentLinked`
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

### `AdmitEnteredStepRejectedMissing`
- From: `Ready`
- On: `AdmitEnteredStep`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `AdmitEnteredStepRejectedConsentLinked`
- From: `Ready`
- On: `AdmitEnteredStep`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `AdmitEnteredStepRejectedRetired`
- From: `Ready`
- On: `AdmitEnteredStep`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `AdmitEnteredStepRejectedNotEntered`
- From: `Ready`
- On: `AdmitEnteredStep`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `AdmitEnteredStep`
- From: `Ready`
- On: `AdmitEnteredStep`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `AdmitEnteredStepRejectedClosing`
- From: `Ready`
- On: `AdmitEnteredStep`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `ReleaseReviewUsedClosing`
- From: `Ready`
- On: `ReleaseReview`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `ReleaseReviewRejectedClosing`
- From: `Ready`
- On: `ReleaseReview`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `FinishEnteredStepRejectedMissing`
- From: `Ready`
- On: `FinishEnteredStep`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `FinishEnteredStepRejectedNotInFlight`
- From: `Ready`
- On: `FinishEnteredStep`(review_id)
- Guards:
  - ``
- Emits: `ReviewLifecycleRejected`
- To: `Ready`

### `FinishEnteredStepOpen`
- From: `Ready`
- On: `FinishEnteredStep`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `FinishEnteredStepClosing`
- From: `Ready`
- On: `FinishEnteredStep`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

### `FinishEnteredStepReleases`
- From: `Ready`
- On: `FinishEnteredStep`(review_id)
- Guards:
  - ``
- Emits: `ReviewStatusResolved`
- To: `Ready`

## Coverage
### Code Anchors
- `approval_lifecycle_authority` (machine `ApprovalLifecycleMachine`): `crates/meerkat-core/src/generated/approval_lifecycle.rs` — generated ApprovalLifecycleMachine owner for CreateRejectedEmptyAllowedDecisions, CreateRejectedAlreadyExists, CreatePending, CreateConsentRejectedEmptyAllowedDecisions, CreateConsentRejectedAlreadyExists, CreateConsentPending, RestoreRejectedDuplicate, RestoreRejectedEmptyAllowedDecisions, RestorePending, RestoreExpired, RestoreCancelled, RestoreApproved, RestoreDenied, RestoreRejectedInvalidRecord, ReconcileRejectedMissing, ReconcileRejectedLinked, ReconcileRejectedInvalidRecord, ReconcileAppliedPending, ReconcileAppliedExpired, ReconcileAppliedCancelled, ReconcileAppliedApproved, ReconcileAppliedDenied, ObserveExpiryRejectedMissing, ObserveExpiryExpiresPending, ObserveExpiryPendingNoop, ObserveExpiryApprovedNoop, ObserveExpiryDeniedNoop, ObserveExpiryExpiredNoop, ObserveExpiryCancelledNoop, DecideRejectedMissing, DecideRejectedExpired, DecideRejectedAlreadyDecided, DecideRejectedAuthenticationRequired, DecideRejectedApproveNotAllowed, DecideRejectedDenyNotAllowed, DecideApprove, DecideDeny, DecideConsentRejectedMissing, DecideConsentRejectedMismatch, DecideConsentRejectedExpired, DecideConsentRejectedAlreadyDecided, DecideConsentRejectedAuthenticationRequired, DecideConsentRejectedNotAllowed, DecideConsentApprove, DecideConsentDeny, ConsumeConsentRejectedMissing, ConsumeConsentRejectedAlreadyConsumed, ConsumeConsentRejectedMismatch, ConsumeConsentRejectedNotSatisfied, ConsumeConsentForEntry, WithdrawConsentRejectedMissing, WithdrawConsentRejectedMismatch, WithdrawConsentPending, WithdrawConsentSettled, SettleUnlinkedRejectedMissing, SettleUnlinkedRejectedNotConsent, SettleUnlinkedRejectedLinked, SettleUnlinkedRejectedConsumed, SettleUnlinkedRejectedExpired, SettleUnlinkedRejectedSettled, SettleUnlinkedPending, SettleUnlinkedApproved, BeginReviewRejectedDuplicate, BeginReviewPending, RecordReviewVerdictRejectedMissing, RecordReviewVerdictRejectedConsentLinked, RecordReviewVerdictRejectedRetired, RecordReviewVerdictRejectedSettled, RecordReviewVerdictAllowed, RecordReviewVerdictDenied, RecordReviewVerdictEscalated, RecordReviewUnavailableRejectedMissing, RecordReviewUnavailableRejectedConsentLinked, RecordReviewUnavailableRejectedRetired, RecordReviewUnavailableRejectedSettled, RecordReviewUnavailable, RetireReviewRejectedMissing, RetireReviewRejectedConsentLinked, RetireReviewRejectedRetired, RetireReviewRejectedSettled, RetireReview, ConsumeReviewRejectedMissing, ConsumeReviewRejectedConsentLinked, ConsumeReviewRejectedRetired, ConsumeReviewRejectedUsed, ConsumeReviewRejectedNotSatisfied, ConsumeReviewForEntry, ReleaseReviewRejectedMissing, ReleaseReviewRejectedConsentLinked, ReleaseReviewRejectedPending, ReleaseReviewAllowed, ReleaseReviewDenied, ReleaseReviewEscalated, ReleaseReviewUnavailable, ReleaseReviewRetired, ReleaseReviewUsed, AdmitEnteredStepRejectedMissing, AdmitEnteredStepRejectedConsentLinked, AdmitEnteredStepRejectedRetired, AdmitEnteredStepRejectedNotEntered, AdmitEnteredStep, AdmitEnteredStepRejectedClosing, ReleaseReviewUsedClosing, ReleaseReviewRejectedClosing, FinishEnteredStepRejectedMissing, FinishEnteredStepRejectedNotInFlight, FinishEnteredStepOpen, FinishEnteredStepClosing, FinishEnteredStepReleases, ApprovalStatusResolved, ApprovalLifecycleRejected, ReviewStatusResolved, and ReviewLifecycleRejected

### Scenarios
- `approval_request_pending` — CreateRejectedEmptyAllowedDecisions, CreateRejectedAlreadyExists, CreatePending, CreateConsentRejectedEmptyAllowedDecisions, CreateConsentRejectedAlreadyExists, CreateConsentPending keep request creation, and a consent approval created atomically with its linked human attempt, under ApprovalStatusResolved or ApprovalLifecycleRejected
- `approval_decide_terminal` — DecideRejectedMissing, DecideRejectedExpired, DecideRejectedAlreadyDecided, DecideRejectedAuthenticationRequired, DecideRejectedApproveNotAllowed, DecideRejectedDenyNotAllowed, DecideApprove, DecideDeny move Pending approvals to Approved or Denied only when generated allowed-decision state admits the terminal decision; a consent approval is refused here and decided only through DecideConsent
- `approval_consent_link_and_spend` — DecideConsentRejectedMissing, DecideConsentRejectedMismatch, DecideConsentRejectedExpired, DecideConsentRejectedAlreadyDecided, DecideConsentRejectedAuthenticationRequired, DecideConsentRejectedNotAllowed, DecideConsentApprove, DecideConsentDeny, ConsumeConsentRejectedMissing, ConsumeConsentRejectedAlreadyConsumed, ConsumeConsentRejectedMismatch, ConsumeConsentRejectedNotSatisfied, ConsumeConsentForEntry, WithdrawConsentRejectedMissing, WithdrawConsentRejectedMismatch, WithdrawConsentPending, WithdrawConsentSettled decide, spend once and withdraw a consent approval only together with its linked live attempt: an authenticated decision allows the attempt, the spend requires an approved authenticated unconsumed approval and an allowed attempt and records the one-use fact, and a withdrawal cancels a pending approval or keeps approved history while retiring the attempt
- `approval_consent_unlinked_settlement` — SettleUnlinkedRejectedMissing, SettleUnlinkedRejectedNotConsent, SettleUnlinkedRejectedLinked, SettleUnlinkedRejectedConsumed, SettleUnlinkedRejectedExpired, SettleUnlinkedRejectedSettled, SettleUnlinkedPending, SettleUnlinkedApproved settle a consent row no attempt is linked to, without any candidate, after its owner's retained withdrawal duty or a proven owner loss: a pending row is cancelled whatever its deadline, approved unspent history is kept, and a missing, non-consent, linked (held here), spent, expired or already settled row is refused
- `approval_expiry_feedback` — ObserveExpiryRejectedMissing, ObserveExpiryExpiresPending, ObserveExpiryPendingNoop, ObserveExpiryApprovedNoop, ObserveExpiryDeniedNoop, ObserveExpiryExpiredNoop, ObserveExpiryCancelledNoop consume typed time observation and emit Expired or unchanged status without handwritten status mutation
- `approval_restore_consistency` — RestoreRejectedDuplicate, RestoreRejectedEmptyAllowedDecisions, RestorePending, RestoreExpired, RestoreCancelled, RestoreApproved, RestoreDenied, RestoreRejectedInvalidRecord validate persisted status, decision audit, allowed-decision compatibility and consent history (authenticated and consumed) before rehydrating approval lifecycle truth, never as a live link
- `approval_reconcile_from_store` — ReconcileRejectedMissing, ReconcileRejectedLinked, ReconcileRejectedInvalidRecord, ReconcileAppliedPending, ReconcileAppliedExpired, ReconcileAppliedCancelled, ReconcileAppliedApproved, ReconcileAppliedDenied replace one existing unlinked row's state from its stored row after restore validation, never undoing a spend and never creating a link
- `approval_review_attempt_retirement` — BeginReviewRejectedDuplicate, BeginReviewPending, RecordReviewVerdictRejectedMissing, RecordReviewVerdictRejectedConsentLinked, RecordReviewVerdictRejectedRetired, RecordReviewVerdictRejectedSettled, RecordReviewVerdictAllowed, RecordReviewVerdictDenied, RecordReviewVerdictEscalated, RecordReviewUnavailableRejectedMissing, RecordReviewUnavailableRejectedConsentLinked, RecordReviewUnavailableRejectedRetired, RecordReviewUnavailableRejectedSettled, RecordReviewUnavailable, RetireReviewRejectedMissing, RetireReviewRejectedConsentLinked, RetireReviewRejectedRetired, RetireReviewRejectedSettled, RetireReview, ConsumeReviewRejectedMissing, ConsumeReviewRejectedConsentLinked, ConsumeReviewRejectedRetired, ConsumeReviewRejectedUsed, ConsumeReviewRejectedNotSatisfied, ConsumeReviewForEntry, ReleaseReviewRejectedMissing, ReleaseReviewRejectedConsentLinked, ReleaseReviewRejectedPending, ReleaseReviewAllowed, ReleaseReviewDenied, ReleaseReviewEscalated, ReleaseReviewUnavailable, ReleaseReviewRetired, ReleaseReviewUsed keep one process-local retained review attempt under generated status: a retired attempt rejects late verdicts and entry, an allow is consumed at most once, release disposes only settled attempts, and a linked human attempt is refused by every generic review input
- `approval_entered_effect_steps` — AdmitEnteredStepRejectedMissing, AdmitEnteredStepRejectedConsentLinked, AdmitEnteredStepRejectedRetired, AdmitEnteredStepRejectedNotEntered, AdmitEnteredStep, AdmitEnteredStepRejectedClosing, ReleaseReviewUsedClosing, ReleaseReviewRejectedClosing, FinishEnteredStepRejectedMissing, FinishEnteredStepRejectedNotInFlight, FinishEnteredStepOpen, FinishEnteredStepClosing, FinishEnteredStepReleases keep the entered state of a spent attempt's effect: a later step begins only against the attempt its entry spent once while that effect is open, and ends with its finish; neither spends; the admitting dispatch's end releases the attempt with no step in flight and otherwise closes the effect, which admits no new step and is released by its last step's finish; an unknown or released, consent-linked, retired or never spent attempt admits none
