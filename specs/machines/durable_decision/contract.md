# DurableDecisionMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::durable_decision`

## State
- Phase enum: `Unheld | Open | Approved | Rejected | Expired | Withdrawn | Executed | ExecutionRefused`
- `decision_id`: `String`
- `deadline_at_ms`: `u64`
- `subject_digest`: `DecisionDigest`
- `operation_id`: `String`
- `request_digest`: `DecisionDigest`
- `requester`: `DecisionParty`
- `approver_id`: `DecisionParty`
- `reviewer`: `Option<DecisionParty>`
- `stage`: `DurableDecisionStage`
- `first_presented_to_approver_at_ms`: `Option<u64>`
- `presented_to_reviewer`: `Bool`
- `evidence_digest`: `Option<String>`
- `decided_at_ms`: `Option<u64>`
- `ended_at_ms`: `Option<u64>`
- `permit_issued`: `Bool`
- `executed_request_digest`: `Option<DecisionDigest>`
- `notice`: `DurableDecisionNotice`

## Inputs
- `Hold`(decision_id: String, now_ms: u64, deadline_at_ms: u64, subject_digest: DecisionDigest, operation_id: String, request_digest: DecisionDigest, requester: DecisionParty, approver_id: DecisionParty, reviewer: Option<DecisionParty>, notice_routable: Bool)
- `Present`(audience: DecisionParty, digest: DecisionDigest, complete: Bool, presented_at_ms: u64, now_ms: u64)
- `Decide`(approve: Bool, approver_id: DecisionParty, decider: DecisionParty, digest: DecisionDigest, evidence_digest: Option<String>, eligible: Bool, carried_presentation_at_ms: Option<u64>, observed_at_ms: u64, now_ms: u64)
- `Review`(verdict: DurableDecisionReviewVerdict, reviewer: DecisionParty, digest: DecisionDigest, evidence_digest: Option<String>, now_ms: u64)
- `Withdraw`(withdrawer: DecisionParty, by_host: Bool, now_ms: u64)
- `Expire`(now_ms: u64)
- `AuthorizeExecution`(operation_id: String, request_digest: DecisionDigest, now_ms: u64)
- `ReleaseExecution`(now_ms: u64)
- `RecordExecuted`(operation_id: String, request_digest: DecisionDigest, now_ms: u64)
- `CloseRefused`(now_ms: u64)
- `SettleNotice`(delivered: Bool)

## Signals

## Effects
- `Held`(decision_id: String, deadline_at_ms: u64)
- `HoldReplayed`(decision_id: String)
- `Presented`(decision_id: String, audience: DecisionParty)
- `Escalated`(decision_id: String)
- `Approved`(decision_id: String, decided_at_ms: u64)
- `Rejected`(decision_id: String, decided_at_ms: u64)
- `Expired`(decision_id: String, deadline_at_ms: u64, observed_at_ms: u64)
- `Withdrawn`(decision_id: String, observed_at_ms: u64)
- `ExecutionAuthorized`(decision_id: String, operation_id: String, request_digest: DecisionDigest, deadline_at_ms: u64)
- `ExecutionReleased`(decision_id: String)
- `Executed`(decision_id: String, operation_id: String, request_digest: DecisionDigest, executed_at_ms: u64)
- `ExecutionReplayed`(decision_id: String)
- `ExecutionRefused`(decision_id: String, closed_at_ms: u64)
- `NoticeSettled`(decision_id: String, delivered: Bool)
- `Refused`(decision_id: String, reason: DurableDecisionRefusal)

## Helpers
- `owed_if_due`(notice: DurableDecisionNotice) -> `DurableDecisionNotice`
- `earliest_presentation`(first: Option<u64>, presented: Option<u64>) -> `Option<u64>`

## Invariants
- `unheld_is_empty`
- `held_records_are_named`
- `review_stage_names_another_reviewer`
- `reviewer_presentation_needs_a_reviewer`
- `first_presentation_is_a_reading`
- `open_has_no_outcome`
- `approved_has_not_ended`
- `evidence_and_decision_time_agree`
- `decided_decisions_are_attested`
- `permit_is_outstanding_only_while_approved`
- `executed_names_its_held_request`
- `only_executed_records_an_execution`
- `ended_decisions_record_their_end`
- `expiry_never_precedes_its_deadline`
- `withdrawal_precedes_the_deadline`
- `ended_decisions_owe_their_notice`

## Transitions
### `HoldHuman`
- From: `Unheld`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `named_decision`
  - `named_operation`
  - `window_is_open`
  - `no_reviewer`
- Emits: `Held`
- To: `Open`

### `HoldReview`
- From: `Unheld`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `named_decision`
  - `named_operation`
  - `window_is_open`
  - `names_a_reviewer`
  - `reviewer_is_not_requester`
- Emits: `Held`
- To: `Open`

### `HoldRefusedWindow`
- From: `Unheld`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `window_is_closed`
- Emits: `Refused`
- To: `Unheld`

### `HoldRefusedSelfReview`
- From: `Unheld`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `window_is_open`
  - `requester_reviews`
- Emits: `Refused`
- To: `Unheld`

### `HoldReplayedOpen`
- From: `Open`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `same_decision`
  - `same_deadline`
  - `same_subject`
  - `same_operation`
  - `same_request`
  - `same_requester`
  - `same_approver`
  - `same_reviewer`
  - `same_notice_route`
- Emits: `HoldReplayed`
- To: `Open`

### `HoldReplayedApproved`
- From: `Approved`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `same_decision`
  - `same_deadline`
  - `same_subject`
  - `same_operation`
  - `same_request`
  - `same_requester`
  - `same_approver`
  - `same_reviewer`
  - `same_notice_route`
- Emits: `HoldReplayed`
- To: `Approved`

### `HoldReplayedRejected`
- From: `Rejected`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `same_decision`
  - `same_deadline`
  - `same_subject`
  - `same_operation`
  - `same_request`
  - `same_requester`
  - `same_approver`
  - `same_reviewer`
  - `same_notice_route`
- Emits: `HoldReplayed`
- To: `Rejected`

### `HoldReplayedExpired`
- From: `Expired`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `same_decision`
  - `same_deadline`
  - `same_subject`
  - `same_operation`
  - `same_request`
  - `same_requester`
  - `same_approver`
  - `same_reviewer`
  - `same_notice_route`
- Emits: `HoldReplayed`
- To: `Expired`

### `HoldReplayedWithdrawn`
- From: `Withdrawn`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `same_decision`
  - `same_deadline`
  - `same_subject`
  - `same_operation`
  - `same_request`
  - `same_requester`
  - `same_approver`
  - `same_reviewer`
  - `same_notice_route`
- Emits: `HoldReplayed`
- To: `Withdrawn`

### `HoldReplayedExecuted`
- From: `Executed`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `same_decision`
  - `same_deadline`
  - `same_subject`
  - `same_operation`
  - `same_request`
  - `same_requester`
  - `same_approver`
  - `same_reviewer`
  - `same_notice_route`
- Emits: `HoldReplayed`
- To: `Executed`

### `HoldReplayedExecutionRefused`
- From: `ExecutionRefused`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `same_decision`
  - `same_deadline`
  - `same_subject`
  - `same_operation`
  - `same_request`
  - `same_requester`
  - `same_approver`
  - `same_reviewer`
  - `same_notice_route`
- Emits: `HoldReplayed`
- To: `ExecutionRefused`

### `HoldConflictOpen`
- From: `Open`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `a_sealed_field_differs`
- Emits: `Refused`
- To: `Open`

### `HoldConflictApproved`
- From: `Approved`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `a_sealed_field_differs`
- Emits: `Refused`
- To: `Approved`

### `HoldConflictRejected`
- From: `Rejected`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `a_sealed_field_differs`
- Emits: `Refused`
- To: `Rejected`

### `HoldConflictExpired`
- From: `Expired`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `a_sealed_field_differs`
- Emits: `Refused`
- To: `Expired`

### `HoldConflictWithdrawn`
- From: `Withdrawn`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `a_sealed_field_differs`
- Emits: `Refused`
- To: `Withdrawn`

### `HoldConflictExecuted`
- From: `Executed`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `a_sealed_field_differs`
- Emits: `Refused`
- To: `Executed`

### `HoldConflictExecutionRefused`
- From: `ExecutionRefused`
- On: `Hold`(decision_id, now_ms, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, notice_routable)
- Guards:
  - `a_sealed_field_differs`
- Emits: `Refused`
- To: `ExecutionRefused`

### `PresentAtDeadline`
- From: `Open`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Guards:
  - `deadline_reached`
- Emits: `Expired`, `Refused`
- To: `Expired`

### `PresentRefusedSubjectChanged`
- From: `Open`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `subject_differs`
- Emits: `Refused`
- To: `Open`

### `PresentRefusedIncomplete`
- From: `Open`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `subject_matches`
  - `incomplete`
- Emits: `Refused`
- To: `Open`

### `PresentToApprover`
- From: `Open`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `subject_matches`
  - `complete`
  - `human_stage`
  - `approver_audience`
  - `timed`
- Emits: `Presented`
- To: `Open`

### `PresentRefusedUntimed`
- From: `Open`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `subject_matches`
  - `complete`
  - `human_stage`
  - `approver_audience`
  - `untimed`
- Emits: `Refused`
- To: `Open`

### `PresentRefusedWrongApproverAudience`
- From: `Open`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `subject_matches`
  - `complete`
  - `human_stage`
  - `other_audience`
- Emits: `Refused`
- To: `Open`

### `PresentToReviewer`
- From: `Open`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `subject_matches`
  - `complete`
  - `review_stage`
  - `reviewer_audience`
- Emits: `Presented`
- To: `Open`

### `PresentRefusedWrongReviewerAudience`
- From: `Open`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `subject_matches`
  - `complete`
  - `review_stage`
  - `other_audience`
- Emits: `Refused`
- To: `Open`

### `PresentRefusedNotOpenApproved`
- From: `Approved`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Emits: `Refused`
- To: `Approved`

### `PresentRefusedNotOpenRejected`
- From: `Rejected`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Emits: `Refused`
- To: `Rejected`

### `PresentRefusedNotOpenExpired`
- From: `Expired`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Emits: `Refused`
- To: `Expired`

### `PresentRefusedNotOpenWithdrawn`
- From: `Withdrawn`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Emits: `Refused`
- To: `Withdrawn`

### `PresentRefusedNotOpenExecuted`
- From: `Executed`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Emits: `Refused`
- To: `Executed`

### `PresentRefusedNotOpenExecutionRefused`
- From: `ExecutionRefused`
- On: `Present`(audience, digest, complete, presented_at_ms, now_ms)
- Emits: `Refused`
- To: `ExecutionRefused`

### `DecideAtDeadline`
- From: `Open`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Guards:
  - `deadline_reached`
- Emits: `Expired`, `Refused`
- To: `Expired`

### `DecideRefusedReviewStage`
- From: `Open`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `review_stage`
- Emits: `Refused`
- To: `Open`

### `DecideRefusedEvidenceRequired`
- From: `Open`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `human_stage`
  - `no_evidence`
- Emits: `Refused`
- To: `Open`

### `DecideRefusedSubjectChanged`
- From: `Open`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `human_stage`
  - `evidence_present`
  - `subject_differs`
- Emits: `Refused`
- To: `Open`

### `DecideRefusedNotEligible`
- From: `Open`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `human_stage`
  - `evidence_present`
  - `subject_matches`
  - `approver_or_eligibility_fails`
- Emits: `Refused`
- To: `Open`

### `DecideRefusedSelfApproval`
- From: `Open`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `human_stage`
  - `evidence_present`
  - `subject_matches`
  - `sealed_approver`
  - `eligible`
  - `decider_is_requester`
- Emits: `Refused`
- To: `Open`

### `DecideRefusedPresentationIncomplete`
- From: `Open`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `human_stage`
  - `evidence_present`
  - `subject_matches`
  - `sealed_approver`
  - `eligible`
  - `decider_is_not_requester`
  - `no_timed_presentation`
- Emits: `Refused`
- To: `Open`

### `DecideRefusedDecidedBeforePresentation`
- From: `Open`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `human_stage`
  - `evidence_present`
  - `subject_matches`
  - `sealed_approver`
  - `eligible`
  - `decider_is_not_requester`
  - `carried_presentation_timed`
  - `presented_or_carried`
  - `observed_before_the_recorded_presentation`
  - `observed_before_the_carried_presentation`
- Emits: `Refused`
- To: `Open`

### `DecideApproved`
- From: `Open`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `human_stage`
  - `evidence_present`
  - `subject_matches`
  - `sealed_approver`
  - `eligible`
  - `decider_is_not_requester`
  - `carried_presentation_timed`
  - `observed_at_or_after_a_presentation`
  - `approves`
- Emits: `Approved`
- To: `Approved`

### `DecideRejected`
- From: `Open`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Guards:
  - `before_deadline`
  - `human_stage`
  - `evidence_present`
  - `subject_matches`
  - `sealed_approver`
  - `eligible`
  - `decider_is_not_requester`
  - `carried_presentation_timed`
  - `observed_at_or_after_a_presentation`
  - `rejects`
- Emits: `Rejected`
- To: `Rejected`

### `DecideRefusedNotOpenApproved`
- From: `Approved`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Emits: `Refused`
- To: `Approved`

### `DecideRefusedNotOpenRejected`
- From: `Rejected`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Emits: `Refused`
- To: `Rejected`

### `DecideRefusedNotOpenExpired`
- From: `Expired`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Emits: `Refused`
- To: `Expired`

### `DecideRefusedNotOpenWithdrawn`
- From: `Withdrawn`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Emits: `Refused`
- To: `Withdrawn`

### `DecideRefusedNotOpenExecuted`
- From: `Executed`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Emits: `Refused`
- To: `Executed`

### `DecideRefusedNotOpenExecutionRefused`
- From: `ExecutionRefused`
- On: `Decide`(approve, approver_id, decider, digest, evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
- Emits: `Refused`
- To: `ExecutionRefused`

### `ReviewAtDeadline`
- From: `Open`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Guards:
  - `deadline_reached`
- Emits: `Expired`, `Refused`
- To: `Expired`

### `ReviewRefusedHumanStage`
- From: `Open`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Guards:
  - `before_deadline`
  - `human_stage`
- Emits: `Refused`
- To: `Open`

### `ReviewRefusedWrongReviewer`
- From: `Open`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Guards:
  - `before_deadline`
  - `review_stage`
  - `other_reviewer`
- Emits: `Refused`
- To: `Open`

### `ReviewRefusedEvidenceRequired`
- From: `Open`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Guards:
  - `before_deadline`
  - `review_stage`
  - `sealed_reviewer`
  - `no_evidence`
- Emits: `Refused`
- To: `Open`

### `ReviewRefusedSubjectChanged`
- From: `Open`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Guards:
  - `before_deadline`
  - `review_stage`
  - `sealed_reviewer`
  - `evidence_present`
  - `subject_differs`
- Emits: `Refused`
- To: `Open`

### `ReviewRefusedPresentationIncomplete`
- From: `Open`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Guards:
  - `before_deadline`
  - `review_stage`
  - `sealed_reviewer`
  - `evidence_present`
  - `subject_matches`
  - `not_presented_to_reviewer`
- Emits: `Refused`
- To: `Open`

### `ReviewAllowed`
- From: `Open`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Guards:
  - `before_deadline`
  - `review_stage`
  - `sealed_reviewer`
  - `evidence_present`
  - `subject_matches`
  - `presented_to_reviewer`
  - `allows`
- Emits: `Approved`
- To: `Approved`

### `ReviewDenied`
- From: `Open`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Guards:
  - `before_deadline`
  - `review_stage`
  - `sealed_reviewer`
  - `evidence_present`
  - `subject_matches`
  - `presented_to_reviewer`
  - `denies`
- Emits: `Rejected`
- To: `Rejected`

### `ReviewEscalated`
- From: `Open`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Guards:
  - `before_deadline`
  - `review_stage`
  - `sealed_reviewer`
  - `evidence_present`
  - `subject_matches`
  - `presented_to_reviewer`
  - `escalates`
- Emits: `Escalated`
- To: `Open`

### `ReviewRefusedNotOpenApproved`
- From: `Approved`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Emits: `Refused`
- To: `Approved`

### `ReviewRefusedNotOpenRejected`
- From: `Rejected`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Emits: `Refused`
- To: `Rejected`

### `ReviewRefusedNotOpenExpired`
- From: `Expired`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Emits: `Refused`
- To: `Expired`

### `ReviewRefusedNotOpenWithdrawn`
- From: `Withdrawn`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Emits: `Refused`
- To: `Withdrawn`

### `ReviewRefusedNotOpenExecuted`
- From: `Executed`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Emits: `Refused`
- To: `Executed`

### `ReviewRefusedNotOpenExecutionRefused`
- From: `ExecutionRefused`
- On: `Review`(verdict, reviewer, digest, evidence_digest, now_ms)
- Emits: `Refused`
- To: `ExecutionRefused`

### `WithdrawAtDeadline`
- From: `Open`
- On: `Withdraw`(withdrawer, by_host, now_ms)
- Guards:
  - `deadline_reached`
- Emits: `Expired`, `Refused`
- To: `Expired`

### `WithdrawByHost`
- From: `Open`
- On: `Withdraw`(withdrawer, by_host, now_ms)
- Guards:
  - `before_deadline`
  - `host_withdraws`
- Emits: `Withdrawn`
- To: `Withdrawn`

### `WithdrawByRequester`
- From: `Open`
- On: `Withdraw`(withdrawer, by_host, now_ms)
- Guards:
  - `before_deadline`
  - `not_by_host`
  - `withdrawer_is_requester`
- Emits: `Withdrawn`
- To: `Withdrawn`

### `WithdrawRefusedNotRequester`
- From: `Open`
- On: `Withdraw`(withdrawer, by_host, now_ms)
- Guards:
  - `before_deadline`
  - `not_by_host`
  - `withdrawer_is_not_requester`
- Emits: `Refused`
- To: `Open`

### `WithdrawRefusedNotOpenApproved`
- From: `Approved`
- On: `Withdraw`(withdrawer, by_host, now_ms)
- Emits: `Refused`
- To: `Approved`

### `WithdrawRefusedNotOpenRejected`
- From: `Rejected`
- On: `Withdraw`(withdrawer, by_host, now_ms)
- Emits: `Refused`
- To: `Rejected`

### `WithdrawRefusedNotOpenExpired`
- From: `Expired`
- On: `Withdraw`(withdrawer, by_host, now_ms)
- Emits: `Refused`
- To: `Expired`

### `WithdrawRefusedNotOpenWithdrawn`
- From: `Withdrawn`
- On: `Withdraw`(withdrawer, by_host, now_ms)
- Emits: `Refused`
- To: `Withdrawn`

### `WithdrawRefusedNotOpenExecuted`
- From: `Executed`
- On: `Withdraw`(withdrawer, by_host, now_ms)
- Emits: `Refused`
- To: `Executed`

### `WithdrawRefusedNotOpenExecutionRefused`
- From: `ExecutionRefused`
- On: `Withdraw`(withdrawer, by_host, now_ms)
- Emits: `Refused`
- To: `ExecutionRefused`

### `ExpireOpenDue`
- From: `Open`
- On: `Expire`(now_ms)
- Guards:
  - `deadline_reached`
- Emits: `Expired`
- To: `Expired`

### `ExpireOpenNotDue`
- From: `Open`
- On: `Expire`(now_ms)
- Guards:
  - `before_deadline`
- To: `Open`

### `ExpireApprovedDue`
- From: `Approved`
- On: `Expire`(now_ms)
- Guards:
  - `deadline_reached`
  - `no_permit`
- Emits: `Expired`
- To: `Expired`

### `ExpireApprovedSettlementOwed`
- From: `Approved`
- On: `Expire`(now_ms)
- Guards:
  - `deadline_reached`
  - `permit_outstanding`
- To: `Approved`

### `ExpireApprovedNotDue`
- From: `Approved`
- On: `Expire`(now_ms)
- Guards:
  - `before_deadline`
- To: `Approved`

### `ExpireSettledRejected`
- From: `Rejected`
- On: `Expire`(now_ms)
- To: `Rejected`

### `ExpireSettledExpired`
- From: `Expired`
- On: `Expire`(now_ms)
- To: `Expired`

### `ExpireSettledWithdrawn`
- From: `Withdrawn`
- On: `Expire`(now_ms)
- To: `Withdrawn`

### `ExpireSettledExecuted`
- From: `Executed`
- On: `Expire`(now_ms)
- To: `Executed`

### `ExpireSettledExecutionRefused`
- From: `ExecutionRefused`
- On: `Expire`(now_ms)
- To: `ExecutionRefused`

### `AuthorizeExecution`
- From: `Approved`
- On: `AuthorizeExecution`(operation_id, request_digest, now_ms)
- Guards:
  - `same_operation`
  - `same_request`
  - `before_deadline`
- Emits: `ExecutionAuthorized`
- To: `Approved`

### `AuthorizeExecutionAtDeadline`
- From: `Approved`
- On: `AuthorizeExecution`(operation_id, request_digest, now_ms)
- Guards:
  - `same_operation`
  - `same_request`
  - `deadline_reached`
  - `no_permit`
- Emits: `Expired`, `Refused`
- To: `Expired`

### `AuthorizeExecutionRefusedSettlementOwed`
- From: `Approved`
- On: `AuthorizeExecution`(operation_id, request_digest, now_ms)
- Guards:
  - `same_operation`
  - `same_request`
  - `deadline_reached`
  - `permit_outstanding`
- Emits: `Refused`
- To: `Approved`

### `AuthorizeExecutionRefusedBinding`
- From: `Approved`
- On: `AuthorizeExecution`(operation_id, request_digest, now_ms)
- Guards:
  - `binding_differs`
- Emits: `Refused`
- To: `Approved`

### `AuthorizeExecutionRefusedNotApprovedOpen`
- From: `Open`
- On: `AuthorizeExecution`(operation_id, request_digest, now_ms)
- Emits: `Refused`
- To: `Open`

### `AuthorizeExecutionRefusedNotApprovedRejected`
- From: `Rejected`
- On: `AuthorizeExecution`(operation_id, request_digest, now_ms)
- Emits: `Refused`
- To: `Rejected`

### `AuthorizeExecutionRefusedNotApprovedExpired`
- From: `Expired`
- On: `AuthorizeExecution`(operation_id, request_digest, now_ms)
- Emits: `Refused`
- To: `Expired`

### `AuthorizeExecutionRefusedNotApprovedWithdrawn`
- From: `Withdrawn`
- On: `AuthorizeExecution`(operation_id, request_digest, now_ms)
- Emits: `Refused`
- To: `Withdrawn`

### `AuthorizeExecutionRefusedNotApprovedExecuted`
- From: `Executed`
- On: `AuthorizeExecution`(operation_id, request_digest, now_ms)
- Emits: `Refused`
- To: `Executed`

### `AuthorizeExecutionRefusedNotApprovedExecutionRefused`
- From: `ExecutionRefused`
- On: `AuthorizeExecution`(operation_id, request_digest, now_ms)
- Emits: `Refused`
- To: `ExecutionRefused`

### `ReleaseExecution`
- From: `Approved`
- On: `ReleaseExecution`(now_ms)
- Guards:
  - `permit_outstanding`
  - `before_deadline`
- Emits: `ExecutionReleased`
- To: `Approved`

### `ReleaseExecutionAtDeadline`
- From: `Approved`
- On: `ReleaseExecution`(now_ms)
- Guards:
  - `permit_outstanding`
  - `deadline_reached`
- Emits: `ExecutionReleased`, `Expired`
- To: `Expired`

### `ReleaseExecutionRefusedNotAuthorized`
- From: `Approved`
- On: `ReleaseExecution`(now_ms)
- Guards:
  - `no_permit`
- Emits: `Refused`
- To: `Approved`

### `ReleaseExecutionRefusedNotApprovedOpen`
- From: `Open`
- On: `ReleaseExecution`(now_ms)
- Emits: `Refused`
- To: `Open`

### `ReleaseExecutionRefusedNotApprovedRejected`
- From: `Rejected`
- On: `ReleaseExecution`(now_ms)
- Emits: `Refused`
- To: `Rejected`

### `ReleaseExecutionRefusedNotApprovedExpired`
- From: `Expired`
- On: `ReleaseExecution`(now_ms)
- Emits: `Refused`
- To: `Expired`

### `ReleaseExecutionRefusedNotApprovedWithdrawn`
- From: `Withdrawn`
- On: `ReleaseExecution`(now_ms)
- Emits: `Refused`
- To: `Withdrawn`

### `ReleaseExecutionRefusedNotApprovedExecuted`
- From: `Executed`
- On: `ReleaseExecution`(now_ms)
- Emits: `Refused`
- To: `Executed`

### `ReleaseExecutionRefusedNotApprovedExecutionRefused`
- From: `ExecutionRefused`
- On: `ReleaseExecution`(now_ms)
- Emits: `Refused`
- To: `ExecutionRefused`

### `RecordExecuted`
- From: `Approved`
- On: `RecordExecuted`(operation_id, request_digest, now_ms)
- Guards:
  - `same_operation`
  - `same_request`
  - `permit_outstanding`
- Emits: `Executed`
- To: `Executed`

### `RecordExecutedRefusedNotAuthorized`
- From: `Approved`
- On: `RecordExecuted`(operation_id, request_digest, now_ms)
- Guards:
  - `same_operation`
  - `same_request`
  - `no_permit`
- Emits: `Refused`
- To: `Approved`

### `RecordExecutedRefusedBinding`
- From: `Approved`
- On: `RecordExecuted`(operation_id, request_digest, now_ms)
- Guards:
  - `binding_differs`
- Emits: `Refused`
- To: `Approved`

### `RecordExecutedReplayed`
- From: `Executed`
- On: `RecordExecuted`(operation_id, request_digest, now_ms)
- Guards:
  - `same_operation`
  - `same_request`
- Emits: `ExecutionReplayed`
- To: `Executed`

### `RecordExecutedReplayRefusedBinding`
- From: `Executed`
- On: `RecordExecuted`(operation_id, request_digest, now_ms)
- Guards:
  - `binding_differs`
- Emits: `Refused`
- To: `Executed`

### `RecordExecutedRefusedNotApprovedOpen`
- From: `Open`
- On: `RecordExecuted`(operation_id, request_digest, now_ms)
- Emits: `Refused`
- To: `Open`

### `RecordExecutedRefusedNotApprovedRejected`
- From: `Rejected`
- On: `RecordExecuted`(operation_id, request_digest, now_ms)
- Emits: `Refused`
- To: `Rejected`

### `RecordExecutedRefusedNotApprovedExpired`
- From: `Expired`
- On: `RecordExecuted`(operation_id, request_digest, now_ms)
- Emits: `Refused`
- To: `Expired`

### `RecordExecutedRefusedNotApprovedWithdrawn`
- From: `Withdrawn`
- On: `RecordExecuted`(operation_id, request_digest, now_ms)
- Emits: `Refused`
- To: `Withdrawn`

### `RecordExecutedRefusedNotApprovedExecutionRefused`
- From: `ExecutionRefused`
- On: `RecordExecuted`(operation_id, request_digest, now_ms)
- Emits: `Refused`
- To: `ExecutionRefused`

### `CloseRefused`
- From: `Approved`
- On: `CloseRefused`(now_ms)
- Guards:
  - `before_deadline`
- Emits: `ExecutionRefused`
- To: `ExecutionRefused`

### `CloseRefusedWithPermit`
- From: `Approved`
- On: `CloseRefused`(now_ms)
- Guards:
  - `deadline_reached`
  - `permit_outstanding`
- Emits: `ExecutionRefused`
- To: `ExecutionRefused`

### `CloseRefusedAtDeadline`
- From: `Approved`
- On: `CloseRefused`(now_ms)
- Guards:
  - `deadline_reached`
  - `no_permit`
- Emits: `Expired`, `Refused`
- To: `Expired`

### `CloseRefusedNotApprovedOpen`
- From: `Open`
- On: `CloseRefused`(now_ms)
- Emits: `Refused`
- To: `Open`

### `CloseRefusedNotApprovedRejected`
- From: `Rejected`
- On: `CloseRefused`(now_ms)
- Emits: `Refused`
- To: `Rejected`

### `CloseRefusedNotApprovedExpired`
- From: `Expired`
- On: `CloseRefused`(now_ms)
- Emits: `Refused`
- To: `Expired`

### `CloseRefusedNotApprovedWithdrawn`
- From: `Withdrawn`
- On: `CloseRefused`(now_ms)
- Emits: `Refused`
- To: `Withdrawn`

### `CloseRefusedNotApprovedExecuted`
- From: `Executed`
- On: `CloseRefused`(now_ms)
- Emits: `Refused`
- To: `Executed`

### `CloseRefusedNotApprovedExecutionRefused`
- From: `ExecutionRefused`
- On: `CloseRefused`(now_ms)
- Emits: `Refused`
- To: `ExecutionRefused`

### `SettleNoticeDeliveredRejected`
- From: `Rejected`
- On: `SettleNotice`(delivered)
- Guards:
  - `notice_owed`
  - `delivered`
- Emits: `NoticeSettled`
- To: `Rejected`

### `SettleNoticeDeliveredExpired`
- From: `Expired`
- On: `SettleNotice`(delivered)
- Guards:
  - `notice_owed`
  - `delivered`
- Emits: `NoticeSettled`
- To: `Expired`

### `SettleNoticeDeliveredWithdrawn`
- From: `Withdrawn`
- On: `SettleNotice`(delivered)
- Guards:
  - `notice_owed`
  - `delivered`
- Emits: `NoticeSettled`
- To: `Withdrawn`

### `SettleNoticeDeliveredExecuted`
- From: `Executed`
- On: `SettleNotice`(delivered)
- Guards:
  - `notice_owed`
  - `delivered`
- Emits: `NoticeSettled`
- To: `Executed`

### `SettleNoticeDeliveredExecutionRefused`
- From: `ExecutionRefused`
- On: `SettleNotice`(delivered)
- Guards:
  - `notice_owed`
  - `delivered`
- Emits: `NoticeSettled`
- To: `ExecutionRefused`

### `SettleNoticeFailedRejected`
- From: `Rejected`
- On: `SettleNotice`(delivered)
- Guards:
  - `notice_owed`
  - `failed`
- Emits: `NoticeSettled`
- To: `Rejected`

### `SettleNoticeFailedExpired`
- From: `Expired`
- On: `SettleNotice`(delivered)
- Guards:
  - `notice_owed`
  - `failed`
- Emits: `NoticeSettled`
- To: `Expired`

### `SettleNoticeFailedWithdrawn`
- From: `Withdrawn`
- On: `SettleNotice`(delivered)
- Guards:
  - `notice_owed`
  - `failed`
- Emits: `NoticeSettled`
- To: `Withdrawn`

### `SettleNoticeFailedExecuted`
- From: `Executed`
- On: `SettleNotice`(delivered)
- Guards:
  - `notice_owed`
  - `failed`
- Emits: `NoticeSettled`
- To: `Executed`

### `SettleNoticeFailedExecutionRefused`
- From: `ExecutionRefused`
- On: `SettleNotice`(delivered)
- Guards:
  - `notice_owed`
  - `failed`
- Emits: `NoticeSettled`
- To: `ExecutionRefused`

### `SettleNoticeReplayedRejected`
- From: `Rejected`
- On: `SettleNotice`(delivered)
- Guards:
  - `same_receipt`
- Emits: `NoticeSettled`
- To: `Rejected`

### `SettleNoticeReplayedExpired`
- From: `Expired`
- On: `SettleNotice`(delivered)
- Guards:
  - `same_receipt`
- Emits: `NoticeSettled`
- To: `Expired`

### `SettleNoticeReplayedWithdrawn`
- From: `Withdrawn`
- On: `SettleNotice`(delivered)
- Guards:
  - `same_receipt`
- Emits: `NoticeSettled`
- To: `Withdrawn`

### `SettleNoticeReplayedExecuted`
- From: `Executed`
- On: `SettleNotice`(delivered)
- Guards:
  - `same_receipt`
- Emits: `NoticeSettled`
- To: `Executed`

### `SettleNoticeReplayedExecutionRefused`
- From: `ExecutionRefused`
- On: `SettleNotice`(delivered)
- Guards:
  - `same_receipt`
- Emits: `NoticeSettled`
- To: `ExecutionRefused`

### `SettleNoticeRefusedRejected`
- From: `Rejected`
- On: `SettleNotice`(delivered)
- Guards:
  - `not_owed`
- Emits: `Refused`
- To: `Rejected`

### `SettleNoticeRefusedExpired`
- From: `Expired`
- On: `SettleNotice`(delivered)
- Guards:
  - `not_owed`
- Emits: `Refused`
- To: `Expired`

### `SettleNoticeRefusedWithdrawn`
- From: `Withdrawn`
- On: `SettleNotice`(delivered)
- Guards:
  - `not_owed`
- Emits: `Refused`
- To: `Withdrawn`

### `SettleNoticeRefusedExecuted`
- From: `Executed`
- On: `SettleNotice`(delivered)
- Guards:
  - `not_owed`
- Emits: `Refused`
- To: `Executed`

### `SettleNoticeRefusedExecutionRefused`
- From: `ExecutionRefused`
- On: `SettleNotice`(delivered)
- Guards:
  - `not_owed`
- Emits: `Refused`
- To: `ExecutionRefused`

### `SettleNoticeRefusedNotEndedOpen`
- From: `Open`
- On: `SettleNotice`(delivered)
- Emits: `Refused`
- To: `Open`

### `SettleNoticeRefusedNotEndedApproved`
- From: `Approved`
- On: `SettleNotice`(delivered)
- Emits: `Refused`
- To: `Approved`

## Coverage
### Code Anchors
- `durable_decision` (machine `DurableDecisionMachine`): `crates/meerkat-runtime/src/decision/dsl.rs` — DurableDecisionMachine owns one durable decision: the hold and its sealed binding, the presentation of the held subject, the attested human or reviewer decision bound to the subject digest, the execution permit and the executed fact, expiry at the sealed deadline, withdrawal, and the requester notice an ended decision owes; ended decisions are terminal

### Scenarios
- `durable_decision_hold` — durable_decision_machine.rs::hold_seals_the_binding_and_opens_by_stage and hold_replays_the_same_binding_and_refuses_another hold a decision by stage with its sealed binding, refuse a past or empty window and a self review, replay the same hold in every held phase and refuse a hold that changes any sealed field
- `durable_decision_human_decision` — durable_decision_machine.rs::each_decide_conjunct_alone_refuses, the_first_valid_decision_wins_and_end_phases_latch, a_human_decision_is_ordered_after_its_first_presentation and a_presentation_time_of_zero_is_no_presentation refuse a human decision for each missing conjunct alone (evidence, digest, approver, eligibility, the requester as decider, presentation, presentation order, stage), record a complete presentation with the earliest time it reached the approver, refuse a presentation at 0 recorded or carried, refuse a decision observed before that first presentation and accept one in the same millisecond, and let the first valid decision win
- `durable_decision_review` — durable_decision_machine.rs::review_allows_denies_escalates_and_refuses_another_reviewer lets the sealed reviewer allow, deny or escalate a presented decision, hands an escalation to the human approver, and refuses another reviewer, missing evidence, a changed subject, a missing presentation and a verdict in the human stage
- `durable_decision_deadline` — durable_decision_machine.rs::a_touch_at_the_deadline_records_expiry and an_approved_decision_expires_at_its_deadline_unless_a_permit_is_outstanding record the expiry of an open decision at any touch at or after its deadline, let an approved decision with no outstanding permit lapse at its deadline, and hold the expiry off while a permit is outstanding
- `durable_decision_withdrawal` — durable_decision_machine.rs::withdraw_is_open_only_by_requester_or_host withdraws an open decision by its requester or the host before its deadline and refuses another withdrawer and a decided decision
- `durable_decision_execution` — durable_decision_machine.rs::execution_is_authorized_only_while_approved_with_the_held_binding, executed_replays_and_a_mismatched_binding_is_refused, a_permit_outstanding_across_the_deadline_settles_executed_or_expired and close_refused_never_follows_executed authorize a permit only for the held binding of an approved decision, record the execution only under an outstanding permit, replay it, release a permit with no commit, and close a permanent refusal
- `durable_decision_notice` — durable_decision_machine.rs::notice_is_owed_once_at_the_end_for_routable_decisions owes one requester notice at every end of a routable decision, settles it delivered or failed, replays the same receipt and refuses a contradicting one, an unroutable requester's and a notice before the end
- `durable_decision_terminal_latch` — durable_decision_machine.rs::every_command_class_in_every_phase_is_total_and_only_mutates_where_intended and recover_from_state_refuses_each_torn_shape refuse every input an ended or approved decision does not accept with its typed refusal and a byte-identical state, and refuse a recovered row that breaks any invariant
