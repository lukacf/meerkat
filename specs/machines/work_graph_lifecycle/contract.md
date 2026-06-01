# WorkGraphLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::workgraph_lifecycle`

## State
- Phase enum: `Absent | Open | InProgress | Blocked | Completed | Cancelled | Failed`
- `revision`: `u64`
- `unresolved_blocker_count`: `u64`
- `topology_item_keys`: `Set<WorkItemKey>`
- `topology_edge_keys`: `Set<WorkEdgeKey>`
- `blocks_reachability`: `Set<WorkDependencyPathKey>`
- `parent_reachability`: `Set<WorkDependencyPathKey>`
- `claim_owner_key`: `Option<WorkOwnerKey>`
- `claimed_at_utc_ms`: `Option<u64>`
- `lease_expires_at_utc_ms`: `Option<u64>`
- `due_at_utc_ms`: `Option<u64>`
- `not_before_utc_ms`: `Option<u64>`
- `snoozed_until_utc_ms`: `Option<u64>`
- `completion_policy`: `WorkCompletionPolicy`
- `completion_supervisor_owner_key`: `Option<WorkOwnerKey>`
- `completion_reviewer_quorum_threshold`: `Option<u64>`
- `terminal_at_utc_ms`: `Option<u64>`
- `evidence_count`: `u64`
- `host_confirmation_count`: `u64`
- `principal_confirmation_count`: `u64`
- `supervisor_confirmation_owner_keys`: `Set<WorkOwnerKey>`
- `reviewer_confirmation_owner_keys`: `Set<WorkOwnerKey>`

## Inputs
- `CreateOpen`(due_at_utc_ms: Option<u64>, not_before_utc_ms: Option<u64>, snoozed_until_utc_ms: Option<u64>, completion_policy: WorkCompletionPolicy, completion_supervisor_owner_key: Option<WorkOwnerKey>, completion_reviewer_quorum_threshold: Option<u64>, unresolved_blocker_count: u64)
- `CreateBlocked`(due_at_utc_ms: Option<u64>, not_before_utc_ms: Option<u64>, snoozed_until_utc_ms: Option<u64>, completion_policy: WorkCompletionPolicy, completion_supervisor_owner_key: Option<WorkOwnerKey>, completion_reviewer_quorum_threshold: Option<u64>, unresolved_blocker_count: u64)
- `Update`(expected_revision: u64, due_at_utc_ms: Option<u64>, not_before_utc_ms: Option<u64>, snoozed_until_utc_ms: Option<u64>, completion_policy: WorkCompletionPolicy, completion_supervisor_owner_key: Option<WorkOwnerKey>, completion_reviewer_quorum_threshold: Option<u64>, unresolved_blocker_count: u64)
- `Claim`(expected_revision: u64, owner_key: WorkOwnerKey, now_utc_ms: u64, lease_expires_at_utc_ms: Option<u64>)
- `Release`(expected_revision: u64)
- `Block`(expected_revision: u64)
- `RefreshEligibility`(unresolved_blocker_count: u64)
- `ValidateLink`(kind: WorkEdgeKind, from_item_key: WorkItemKey, to_item_key: WorkItemKey, edge_key: WorkEdgeKey, reverse_path_key: WorkDependencyPathKey)
- `CloseCompleted`(expected_revision: u64, at_utc_ms: u64)
- `CloseCancelled`(expected_revision: u64, at_utc_ms: u64)
- `CloseFailed`(expected_revision: u64, at_utc_ms: u64)
- `AddEvidence`(expected_revision: u64, evidence_kind: WorkEvidenceKind, confirming_owner_key: Option<WorkOwnerKey>)
- `ClassifyWorkGraphPublicError`(kind: WorkGraphErrorKind)
- `ClassifyTerminality`
- `ClassifyBlockerSatisfied`(blocker_present: Bool, blocker_lifecycle_phase: WorkLifecycleState)
- `ClassifyCreateStatusAdmission`(requested_status: WorkLifecycleState)
- `ClassifyPublicConfirmationAdmission`(completion_policy: WorkCompletionPolicy)
- `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy: WorkCompletionPolicy, requested_completion_supervisor_owner_key: Option<WorkOwnerKey>, requested_completion_reviewer_quorum_threshold: Option<u64>)
- `ClassifyReadiness`(now_utc_ms: u64)

## Signals

## Effects
- `Created`
- `Updated`
- `Claimed`(owner_key: WorkOwnerKey)
- `Released`
- `Blocked`
- `LinkValidated`
- `Closed`(terminal_state: WorkLifecycleState, at_utc_ms: u64)
- `EvidenceAdded`
- `WorkGraphPublicErrorClassified`(kind: WorkGraphErrorKind, public_class: WorkGraphPublicErrorClass)
- `WorkItemTerminalityClassified`(terminal: Bool)
- `BlockerSatisfactionClassified`(satisfied: Bool)
- `CreateStatusAdmissionClassified`(admission: WorkCreateStatusAdmissionKind)
- `PublicConfirmationAdmissionClassified`(admission: WorkPublicConfirmationAdmissionKind)
- `CompletionPolicyMutationAdmissionClassified`(admission: WorkCompletionPolicyMutationAdmissionKind)
- `WorkItemReadinessClassified`(ready: Bool)

## Helpers
- `completion_policy_payload_valid`(policy: WorkCompletionPolicy, supervisor_owner_key: Option<WorkOwnerKey>, reviewer_quorum_threshold: Option<u64>) -> `Bool`
- `completion_policy_is_satisfied`(policy: WorkCompletionPolicy, supervisor_owner_key: Option<WorkOwnerKey>, reviewer_quorum_threshold: Option<u64>, host_confirmation_count: u64, principal_confirmation_count: u64, supervisor_confirmation_owner_keys: Set<WorkOwnerKey>, reviewer_confirmation_owner_keys: Set<WorkOwnerKey>) -> `Bool`
- `evidence_kind_owner_key_present`(evidence_kind: WorkEvidenceKind, confirming_owner_key: Option<WorkOwnerKey>) -> `Bool`
- `claim_time_window_eligible`(due_at_utc_ms: Option<u64>, not_before_utc_ms: Option<u64>, snoozed_until_utc_ms: Option<u64>, now_utc_ms: u64) -> `Bool`

## Invariants
- `absent_has_zero_revision`
- `live_has_positive_revision`
- `topology_snapshot_is_stateless`
- `terminal_has_terminal_time`
- `claim_only_in_progress`
- `blocked_has_no_claim`
- `terminal_has_no_claim`
- `supervisor_policy_has_owner`
- `non_supervisor_policy_has_no_owner`
- `reviewer_quorum_policy_has_positive_threshold`
- `non_reviewer_quorum_policy_has_no_threshold`

## Transitions
### `CreateOpen`
- From: `Absent`
- On: `CreateOpen`(due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, unresolved_blocker_count)
- Guards:
  - `completion_policy_payload_valid`
- Emits: `Created`
- To: `Open`

### `CreateBlocked`
- From: `Absent`
- On: `CreateBlocked`(due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, unresolved_blocker_count)
- Guards:
  - `completion_policy_payload_valid`
- Emits: `Created`
- To: `Blocked`

### `UpdateOpen`
- From: `Open`
- On: `Update`(expected_revision, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, unresolved_blocker_count)
- Guards:
  - ``
  - `completion_policy_payload_valid`
- Emits: `Updated`
- To: `Open`

### `UpdateInProgress`
- From: `InProgress`
- On: `Update`(expected_revision, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, unresolved_blocker_count)
- Guards:
  - ``
  - `completion_policy_payload_valid`
- Emits: `Updated`
- To: `InProgress`

### `UpdateBlocked`
- From: `Blocked`
- On: `Update`(expected_revision, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, unresolved_blocker_count)
- Guards:
  - ``
  - `completion_policy_payload_valid`
- Emits: `Updated`
- To: `Blocked`

### `ClaimOpen`
- From: `Open`
- On: `Claim`(expected_revision, owner_key, now_utc_ms, lease_expires_at_utc_ms)
- Guards:
  - `revision_matches`
  - `dependencies_satisfied`
  - `due_eligible`
  - `not_before_eligible`
  - `snooze_eligible`
- Emits: `Claimed`
- To: `InProgress`

### `ClaimExpiredInProgress`
- From: `InProgress`
- On: `Claim`(expected_revision, owner_key, now_utc_ms, lease_expires_at_utc_ms)
- Guards:
  - `revision_matches`
  - `prior_claim_present`
  - `prior_claim_has_lease`
  - `prior_claim_expired`
  - `dependencies_satisfied`
  - `due_eligible`
  - `not_before_eligible`
  - `snooze_eligible`
- Emits: `Claimed`
- To: `InProgress`

### `ReleaseInProgress`
- From: `InProgress`
- On: `Release`(expected_revision)
- Guards:
  - ``
- Emits: `Released`
- To: `Open`

### `BlockOpen`
- From: `Open`
- On: `Block`(expected_revision)
- Guards:
  - ``
- Emits: `Blocked`
- To: `Blocked`

### `BlockInProgress`
- From: `InProgress`
- On: `Block`(expected_revision)
- Guards:
  - ``
- Emits: `Blocked`
- To: `Blocked`

### `BlockBlocked`
- From: `Blocked`
- On: `Block`(expected_revision)
- Guards:
  - ``
- Emits: `Blocked`
- To: `Blocked`

### `RefreshEligibilityOpen`
- From: `Open`
- On: `RefreshEligibility`(unresolved_blocker_count)
- To: `Open`

### `RefreshEligibilityInProgress`
- From: `InProgress`
- On: `RefreshEligibility`(unresolved_blocker_count)
- To: `InProgress`

### `RefreshEligibilityBlocked`
- From: `Blocked`
- On: `RefreshEligibility`(unresolved_blocker_count)
- To: `Blocked`

### `ValidateLink`
- From: `Absent`
- On: `ValidateLink`(kind, from_item_key, to_item_key, edge_key, reverse_path_key)
- Guards:
  - `from_endpoint_exists`
  - `to_endpoint_exists`
  - `not_self_edge`
  - `not_duplicate_edge`
  - `blocks_acyclic`
  - `parent_acyclic`
- Emits: `LinkValidated`
- To: `Absent`

### `CloseOpenCompleted`
- From: `Open`
- On: `CloseCompleted`(expected_revision, at_utc_ms)
- Guards:
  - ``
  - `completion_policy_satisfied`
- Emits: `Closed`
- To: `Completed`

### `CloseInProgressCompleted`
- From: `InProgress`
- On: `CloseCompleted`(expected_revision, at_utc_ms)
- Guards:
  - ``
  - `completion_policy_satisfied`
- Emits: `Closed`
- To: `Completed`

### `CloseBlockedCompleted`
- From: `Blocked`
- On: `CloseCompleted`(expected_revision, at_utc_ms)
- Guards:
  - ``
  - `completion_policy_satisfied`
- Emits: `Closed`
- To: `Completed`

### `CloseOpenCancelled`
- From: `Open`
- On: `CloseCancelled`(expected_revision, at_utc_ms)
- Guards:
  - ``
- Emits: `Closed`
- To: `Cancelled`

### `CloseInProgressCancelled`
- From: `InProgress`
- On: `CloseCancelled`(expected_revision, at_utc_ms)
- Guards:
  - ``
- Emits: `Closed`
- To: `Cancelled`

### `CloseBlockedCancelled`
- From: `Blocked`
- On: `CloseCancelled`(expected_revision, at_utc_ms)
- Guards:
  - ``
- Emits: `Closed`
- To: `Cancelled`

### `CloseOpenFailed`
- From: `Open`
- On: `CloseFailed`(expected_revision, at_utc_ms)
- Guards:
  - ``
- Emits: `Closed`
- To: `Failed`

### `CloseInProgressFailed`
- From: `InProgress`
- On: `CloseFailed`(expected_revision, at_utc_ms)
- Guards:
  - ``
- Emits: `Closed`
- To: `Failed`

### `CloseBlockedFailed`
- From: `Blocked`
- On: `CloseFailed`(expected_revision, at_utc_ms)
- Guards:
  - ``
- Emits: `Closed`
- To: `Failed`

### `AddEvidenceOpen`
- From: `Open`
- On: `AddEvidence`(expected_revision, evidence_kind, confirming_owner_key)
- Guards:
  - ``
  - `owner_key_present_for_kind`
- Emits: `EvidenceAdded`
- To: `Open`

### `AddEvidenceInProgress`
- From: `InProgress`
- On: `AddEvidence`(expected_revision, evidence_kind, confirming_owner_key)
- Guards:
  - ``
  - `owner_key_present_for_kind`
- Emits: `EvidenceAdded`
- To: `InProgress`

### `AddEvidenceBlocked`
- From: `Blocked`
- On: `AddEvidence`(expected_revision, evidence_kind, confirming_owner_key)
- Guards:
  - ``
  - `owner_key_present_for_kind`
- Emits: `EvidenceAdded`
- To: `Blocked`

### `AddEvidenceCompleted`
- From: `Completed`
- On: `AddEvidence`(expected_revision, evidence_kind, confirming_owner_key)
- Guards:
  - ``
  - `owner_key_present_for_kind`
- Emits: `EvidenceAdded`
- To: `Completed`

### `AddEvidenceCancelled`
- From: `Cancelled`
- On: `AddEvidence`(expected_revision, evidence_kind, confirming_owner_key)
- Guards:
  - ``
  - `owner_key_present_for_kind`
- Emits: `EvidenceAdded`
- To: `Cancelled`

### `AddEvidenceFailed`
- From: `Failed`
- On: `AddEvidence`(expected_revision, evidence_kind, confirming_owner_key)
- Guards:
  - ``
  - `owner_key_present_for_kind`
- Emits: `EvidenceAdded`
- To: `Failed`

### `ClassifyPublicErrorNotFoundAbsent`
- From: `Absent`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `not_found_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorNotFoundOpen`
- From: `Open`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `not_found_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Open`

### `ClassifyPublicErrorNotFoundInProgress`
- From: `InProgress`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `not_found_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `InProgress`

### `ClassifyPublicErrorNotFoundBlocked`
- From: `Blocked`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `not_found_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Blocked`

### `ClassifyPublicErrorNotFoundCompleted`
- From: `Completed`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `not_found_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Completed`

### `ClassifyPublicErrorNotFoundCancelled`
- From: `Cancelled`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `not_found_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Cancelled`

### `ClassifyPublicErrorNotFoundFailed`
- From: `Failed`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `not_found_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Failed`

### `ClassifyPublicErrorConflictAbsent`
- From: `Absent`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `conflict_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorConflictOpen`
- From: `Open`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `conflict_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Open`

### `ClassifyPublicErrorConflictInProgress`
- From: `InProgress`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `conflict_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `InProgress`

### `ClassifyPublicErrorConflictBlocked`
- From: `Blocked`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `conflict_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Blocked`

### `ClassifyPublicErrorConflictCompleted`
- From: `Completed`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `conflict_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Completed`

### `ClassifyPublicErrorConflictCancelled`
- From: `Cancelled`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `conflict_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Cancelled`

### `ClassifyPublicErrorConflictFailed`
- From: `Failed`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `conflict_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Failed`

### `ClassifyPublicErrorInvalidTransitionAbsent`
- From: `Absent`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_transition_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorInvalidTransitionOpen`
- From: `Open`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_transition_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Open`

### `ClassifyPublicErrorInvalidTransitionInProgress`
- From: `InProgress`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_transition_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `InProgress`

### `ClassifyPublicErrorInvalidTransitionBlocked`
- From: `Blocked`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_transition_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Blocked`

### `ClassifyPublicErrorInvalidTransitionCompleted`
- From: `Completed`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_transition_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Completed`

### `ClassifyPublicErrorInvalidTransitionCancelled`
- From: `Cancelled`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_transition_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Cancelled`

### `ClassifyPublicErrorInvalidTransitionFailed`
- From: `Failed`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_transition_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Failed`

### `ClassifyPublicErrorInvalidArgumentsAbsent`
- From: `Absent`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_arguments_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorInvalidArgumentsOpen`
- From: `Open`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_arguments_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Open`

### `ClassifyPublicErrorInvalidArgumentsInProgress`
- From: `InProgress`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_arguments_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `InProgress`

### `ClassifyPublicErrorInvalidArgumentsBlocked`
- From: `Blocked`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_arguments_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Blocked`

### `ClassifyPublicErrorInvalidArgumentsCompleted`
- From: `Completed`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_arguments_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Completed`

### `ClassifyPublicErrorInvalidArgumentsCancelled`
- From: `Cancelled`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_arguments_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Cancelled`

### `ClassifyPublicErrorInvalidArgumentsFailed`
- From: `Failed`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `invalid_arguments_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Failed`

### `ClassifyPublicErrorCapabilityUnavailableAbsent`
- From: `Absent`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `capability_unavailable_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorCapabilityUnavailableOpen`
- From: `Open`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `capability_unavailable_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Open`

### `ClassifyPublicErrorCapabilityUnavailableInProgress`
- From: `InProgress`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `capability_unavailable_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `InProgress`

### `ClassifyPublicErrorCapabilityUnavailableBlocked`
- From: `Blocked`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `capability_unavailable_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Blocked`

### `ClassifyPublicErrorCapabilityUnavailableCompleted`
- From: `Completed`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `capability_unavailable_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Completed`

### `ClassifyPublicErrorCapabilityUnavailableCancelled`
- From: `Cancelled`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `capability_unavailable_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Cancelled`

### `ClassifyPublicErrorCapabilityUnavailableFailed`
- From: `Failed`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `capability_unavailable_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Failed`

### `ClassifyPublicErrorStoreErrorAbsent`
- From: `Absent`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `store_error_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorStoreErrorOpen`
- From: `Open`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `store_error_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Open`

### `ClassifyPublicErrorStoreErrorInProgress`
- From: `InProgress`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `store_error_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `InProgress`

### `ClassifyPublicErrorStoreErrorBlocked`
- From: `Blocked`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `store_error_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Blocked`

### `ClassifyPublicErrorStoreErrorCompleted`
- From: `Completed`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `store_error_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Completed`

### `ClassifyPublicErrorStoreErrorCancelled`
- From: `Cancelled`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `store_error_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Cancelled`

### `ClassifyPublicErrorStoreErrorFailed`
- From: `Failed`
- On: `ClassifyWorkGraphPublicError`(kind)
- Guards:
  - `store_error_class`
- Emits: `WorkGraphPublicErrorClassified`
- To: `Failed`

### `ClassifyTerminalityTerminalCompleted`
- From: `Completed`
- On: `ClassifyTerminality`()
- Emits: `WorkItemTerminalityClassified`
- To: `Completed`

### `ClassifyTerminalityTerminalCancelled`
- From: `Cancelled`
- On: `ClassifyTerminality`()
- Emits: `WorkItemTerminalityClassified`
- To: `Cancelled`

### `ClassifyTerminalityTerminalFailed`
- From: `Failed`
- On: `ClassifyTerminality`()
- Emits: `WorkItemTerminalityClassified`
- To: `Failed`

### `ClassifyTerminalityLiveAbsent`
- From: `Absent`
- On: `ClassifyTerminality`()
- Emits: `WorkItemTerminalityClassified`
- To: `Absent`

### `ClassifyTerminalityLiveOpen`
- From: `Open`
- On: `ClassifyTerminality`()
- Emits: `WorkItemTerminalityClassified`
- To: `Open`

### `ClassifyTerminalityLiveInProgress`
- From: `InProgress`
- On: `ClassifyTerminality`()
- Emits: `WorkItemTerminalityClassified`
- To: `InProgress`

### `ClassifyTerminalityLiveBlocked`
- From: `Blocked`
- On: `ClassifyTerminality`()
- Emits: `WorkItemTerminalityClassified`
- To: `Blocked`

### `ClassifyReadinessOpenOpen`
- From: `Open`
- On: `ClassifyReadiness`(now_utc_ms)
- Emits: `WorkItemReadinessClassified`
- To: `Open`

### `ClassifyReadinessInProgressInProgress`
- From: `InProgress`
- On: `ClassifyReadiness`(now_utc_ms)
- Emits: `WorkItemReadinessClassified`
- To: `InProgress`

### `ClassifyReadinessNotClaimableAbsent`
- From: `Absent`
- On: `ClassifyReadiness`(now_utc_ms)
- Emits: `WorkItemReadinessClassified`
- To: `Absent`

### `ClassifyReadinessNotClaimableBlocked`
- From: `Blocked`
- On: `ClassifyReadiness`(now_utc_ms)
- Emits: `WorkItemReadinessClassified`
- To: `Blocked`

### `ClassifyReadinessNotClaimableCompleted`
- From: `Completed`
- On: `ClassifyReadiness`(now_utc_ms)
- Emits: `WorkItemReadinessClassified`
- To: `Completed`

### `ClassifyReadinessNotClaimableCancelled`
- From: `Cancelled`
- On: `ClassifyReadiness`(now_utc_ms)
- Emits: `WorkItemReadinessClassified`
- To: `Cancelled`

### `ClassifyReadinessNotClaimableFailed`
- From: `Failed`
- On: `ClassifyReadiness`(now_utc_ms)
- Emits: `WorkItemReadinessClassified`
- To: `Failed`

### `ClassifyBlockerSatisfactionAbsent`
- From: `Absent`
- On: `ClassifyBlockerSatisfied`(blocker_present, blocker_lifecycle_phase)
- Emits: `BlockerSatisfactionClassified`
- To: `Absent`

### `ClassifyBlockerSatisfactionOpen`
- From: `Open`
- On: `ClassifyBlockerSatisfied`(blocker_present, blocker_lifecycle_phase)
- Emits: `BlockerSatisfactionClassified`
- To: `Open`

### `ClassifyBlockerSatisfactionInProgress`
- From: `InProgress`
- On: `ClassifyBlockerSatisfied`(blocker_present, blocker_lifecycle_phase)
- Emits: `BlockerSatisfactionClassified`
- To: `InProgress`

### `ClassifyBlockerSatisfactionBlocked`
- From: `Blocked`
- On: `ClassifyBlockerSatisfied`(blocker_present, blocker_lifecycle_phase)
- Emits: `BlockerSatisfactionClassified`
- To: `Blocked`

### `ClassifyBlockerSatisfactionCompleted`
- From: `Completed`
- On: `ClassifyBlockerSatisfied`(blocker_present, blocker_lifecycle_phase)
- Emits: `BlockerSatisfactionClassified`
- To: `Completed`

### `ClassifyBlockerSatisfactionCancelled`
- From: `Cancelled`
- On: `ClassifyBlockerSatisfied`(blocker_present, blocker_lifecycle_phase)
- Emits: `BlockerSatisfactionClassified`
- To: `Cancelled`

### `ClassifyBlockerSatisfactionFailed`
- From: `Failed`
- On: `ClassifyBlockerSatisfied`(blocker_present, blocker_lifecycle_phase)
- Emits: `BlockerSatisfactionClassified`
- To: `Failed`

### `ClassifyCreateStatusAdmissionOpenAbsent`
- From: `Absent`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_open`
- Emits: `CreateStatusAdmissionClassified`
- To: `Absent`

### `ClassifyCreateStatusAdmissionOpenOpen`
- From: `Open`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_open`
- Emits: `CreateStatusAdmissionClassified`
- To: `Open`

### `ClassifyCreateStatusAdmissionOpenInProgress`
- From: `InProgress`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_open`
- Emits: `CreateStatusAdmissionClassified`
- To: `InProgress`

### `ClassifyCreateStatusAdmissionOpenBlocked`
- From: `Blocked`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_open`
- Emits: `CreateStatusAdmissionClassified`
- To: `Blocked`

### `ClassifyCreateStatusAdmissionOpenCompleted`
- From: `Completed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_open`
- Emits: `CreateStatusAdmissionClassified`
- To: `Completed`

### `ClassifyCreateStatusAdmissionOpenCancelled`
- From: `Cancelled`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_open`
- Emits: `CreateStatusAdmissionClassified`
- To: `Cancelled`

### `ClassifyCreateStatusAdmissionOpenFailed`
- From: `Failed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_open`
- Emits: `CreateStatusAdmissionClassified`
- To: `Failed`

### `ClassifyCreateStatusAdmissionBlockedAbsent`
- From: `Absent`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_blocked`
- Emits: `CreateStatusAdmissionClassified`
- To: `Absent`

### `ClassifyCreateStatusAdmissionBlockedOpen`
- From: `Open`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_blocked`
- Emits: `CreateStatusAdmissionClassified`
- To: `Open`

### `ClassifyCreateStatusAdmissionBlockedInProgress`
- From: `InProgress`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_blocked`
- Emits: `CreateStatusAdmissionClassified`
- To: `InProgress`

### `ClassifyCreateStatusAdmissionBlockedBlocked`
- From: `Blocked`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_blocked`
- Emits: `CreateStatusAdmissionClassified`
- To: `Blocked`

### `ClassifyCreateStatusAdmissionBlockedCompleted`
- From: `Completed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_blocked`
- Emits: `CreateStatusAdmissionClassified`
- To: `Completed`

### `ClassifyCreateStatusAdmissionBlockedCancelled`
- From: `Cancelled`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_blocked`
- Emits: `CreateStatusAdmissionClassified`
- To: `Cancelled`

### `ClassifyCreateStatusAdmissionBlockedFailed`
- From: `Failed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_blocked`
- Emits: `CreateStatusAdmissionClassified`
- To: `Failed`

### `ClassifyCreateStatusAdmissionDeniedAbsentAbsent`
- From: `Absent`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_absent`
- Emits: `CreateStatusAdmissionClassified`
- To: `Absent`

### `ClassifyCreateStatusAdmissionDeniedAbsentOpen`
- From: `Open`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_absent`
- Emits: `CreateStatusAdmissionClassified`
- To: `Open`

### `ClassifyCreateStatusAdmissionDeniedAbsentInProgress`
- From: `InProgress`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_absent`
- Emits: `CreateStatusAdmissionClassified`
- To: `InProgress`

### `ClassifyCreateStatusAdmissionDeniedAbsentBlocked`
- From: `Blocked`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_absent`
- Emits: `CreateStatusAdmissionClassified`
- To: `Blocked`

### `ClassifyCreateStatusAdmissionDeniedAbsentCompleted`
- From: `Completed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_absent`
- Emits: `CreateStatusAdmissionClassified`
- To: `Completed`

### `ClassifyCreateStatusAdmissionDeniedAbsentCancelled`
- From: `Cancelled`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_absent`
- Emits: `CreateStatusAdmissionClassified`
- To: `Cancelled`

### `ClassifyCreateStatusAdmissionDeniedAbsentFailed`
- From: `Failed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_absent`
- Emits: `CreateStatusAdmissionClassified`
- To: `Failed`

### `ClassifyCreateStatusAdmissionDeniedInProgressAbsent`
- From: `Absent`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_in_progress`
- Emits: `CreateStatusAdmissionClassified`
- To: `Absent`

### `ClassifyCreateStatusAdmissionDeniedInProgressOpen`
- From: `Open`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_in_progress`
- Emits: `CreateStatusAdmissionClassified`
- To: `Open`

### `ClassifyCreateStatusAdmissionDeniedInProgressInProgress`
- From: `InProgress`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_in_progress`
- Emits: `CreateStatusAdmissionClassified`
- To: `InProgress`

### `ClassifyCreateStatusAdmissionDeniedInProgressBlocked`
- From: `Blocked`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_in_progress`
- Emits: `CreateStatusAdmissionClassified`
- To: `Blocked`

### `ClassifyCreateStatusAdmissionDeniedInProgressCompleted`
- From: `Completed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_in_progress`
- Emits: `CreateStatusAdmissionClassified`
- To: `Completed`

### `ClassifyCreateStatusAdmissionDeniedInProgressCancelled`
- From: `Cancelled`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_in_progress`
- Emits: `CreateStatusAdmissionClassified`
- To: `Cancelled`

### `ClassifyCreateStatusAdmissionDeniedInProgressFailed`
- From: `Failed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_in_progress`
- Emits: `CreateStatusAdmissionClassified`
- To: `Failed`

### `ClassifyCreateStatusAdmissionDeniedCompletedAbsent`
- From: `Absent`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_completed`
- Emits: `CreateStatusAdmissionClassified`
- To: `Absent`

### `ClassifyCreateStatusAdmissionDeniedCompletedOpen`
- From: `Open`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_completed`
- Emits: `CreateStatusAdmissionClassified`
- To: `Open`

### `ClassifyCreateStatusAdmissionDeniedCompletedInProgress`
- From: `InProgress`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_completed`
- Emits: `CreateStatusAdmissionClassified`
- To: `InProgress`

### `ClassifyCreateStatusAdmissionDeniedCompletedBlocked`
- From: `Blocked`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_completed`
- Emits: `CreateStatusAdmissionClassified`
- To: `Blocked`

### `ClassifyCreateStatusAdmissionDeniedCompletedCompleted`
- From: `Completed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_completed`
- Emits: `CreateStatusAdmissionClassified`
- To: `Completed`

### `ClassifyCreateStatusAdmissionDeniedCompletedCancelled`
- From: `Cancelled`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_completed`
- Emits: `CreateStatusAdmissionClassified`
- To: `Cancelled`

### `ClassifyCreateStatusAdmissionDeniedCompletedFailed`
- From: `Failed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_completed`
- Emits: `CreateStatusAdmissionClassified`
- To: `Failed`

### `ClassifyCreateStatusAdmissionDeniedCancelledAbsent`
- From: `Absent`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_cancelled`
- Emits: `CreateStatusAdmissionClassified`
- To: `Absent`

### `ClassifyCreateStatusAdmissionDeniedCancelledOpen`
- From: `Open`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_cancelled`
- Emits: `CreateStatusAdmissionClassified`
- To: `Open`

### `ClassifyCreateStatusAdmissionDeniedCancelledInProgress`
- From: `InProgress`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_cancelled`
- Emits: `CreateStatusAdmissionClassified`
- To: `InProgress`

### `ClassifyCreateStatusAdmissionDeniedCancelledBlocked`
- From: `Blocked`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_cancelled`
- Emits: `CreateStatusAdmissionClassified`
- To: `Blocked`

### `ClassifyCreateStatusAdmissionDeniedCancelledCompleted`
- From: `Completed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_cancelled`
- Emits: `CreateStatusAdmissionClassified`
- To: `Completed`

### `ClassifyCreateStatusAdmissionDeniedCancelledCancelled`
- From: `Cancelled`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_cancelled`
- Emits: `CreateStatusAdmissionClassified`
- To: `Cancelled`

### `ClassifyCreateStatusAdmissionDeniedCancelledFailed`
- From: `Failed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_cancelled`
- Emits: `CreateStatusAdmissionClassified`
- To: `Failed`

### `ClassifyCreateStatusAdmissionDeniedFailedAbsent`
- From: `Absent`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_failed`
- Emits: `CreateStatusAdmissionClassified`
- To: `Absent`

### `ClassifyCreateStatusAdmissionDeniedFailedOpen`
- From: `Open`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_failed`
- Emits: `CreateStatusAdmissionClassified`
- To: `Open`

### `ClassifyCreateStatusAdmissionDeniedFailedInProgress`
- From: `InProgress`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_failed`
- Emits: `CreateStatusAdmissionClassified`
- To: `InProgress`

### `ClassifyCreateStatusAdmissionDeniedFailedBlocked`
- From: `Blocked`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_failed`
- Emits: `CreateStatusAdmissionClassified`
- To: `Blocked`

### `ClassifyCreateStatusAdmissionDeniedFailedCompleted`
- From: `Completed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_failed`
- Emits: `CreateStatusAdmissionClassified`
- To: `Completed`

### `ClassifyCreateStatusAdmissionDeniedFailedCancelled`
- From: `Cancelled`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_failed`
- Emits: `CreateStatusAdmissionClassified`
- To: `Cancelled`

### `ClassifyCreateStatusAdmissionDeniedFailedFailed`
- From: `Failed`
- On: `ClassifyCreateStatusAdmission`(requested_status)
- Guards:
  - `requested_failed`
- Emits: `CreateStatusAdmissionClassified`
- To: `Failed`

### `ClassifyPublicConfirmationAdmissionSelfAttestAbsent`
- From: `Absent`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `self_attest_public_confirmable`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Absent`

### `ClassifyPublicConfirmationAdmissionSelfAttestOpen`
- From: `Open`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `self_attest_public_confirmable`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Open`

### `ClassifyPublicConfirmationAdmissionSelfAttestInProgress`
- From: `InProgress`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `self_attest_public_confirmable`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `InProgress`

### `ClassifyPublicConfirmationAdmissionSelfAttestBlocked`
- From: `Blocked`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `self_attest_public_confirmable`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Blocked`

### `ClassifyPublicConfirmationAdmissionSelfAttestCompleted`
- From: `Completed`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `self_attest_public_confirmable`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Completed`

### `ClassifyPublicConfirmationAdmissionSelfAttestCancelled`
- From: `Cancelled`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `self_attest_public_confirmable`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Cancelled`

### `ClassifyPublicConfirmationAdmissionSelfAttestFailed`
- From: `Failed`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `self_attest_public_confirmable`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Failed`

### `ClassifyPublicConfirmationAdmissionHostConfirmedAbsent`
- From: `Absent`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `host_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Absent`

### `ClassifyPublicConfirmationAdmissionHostConfirmedOpen`
- From: `Open`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `host_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Open`

### `ClassifyPublicConfirmationAdmissionHostConfirmedInProgress`
- From: `InProgress`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `host_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `InProgress`

### `ClassifyPublicConfirmationAdmissionHostConfirmedBlocked`
- From: `Blocked`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `host_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Blocked`

### `ClassifyPublicConfirmationAdmissionHostConfirmedCompleted`
- From: `Completed`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `host_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Completed`

### `ClassifyPublicConfirmationAdmissionHostConfirmedCancelled`
- From: `Cancelled`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `host_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Cancelled`

### `ClassifyPublicConfirmationAdmissionHostConfirmedFailed`
- From: `Failed`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `host_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Failed`

### `ClassifyPublicConfirmationAdmissionPrincipalConfirmedAbsent`
- From: `Absent`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `principal_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Absent`

### `ClassifyPublicConfirmationAdmissionPrincipalConfirmedOpen`
- From: `Open`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `principal_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Open`

### `ClassifyPublicConfirmationAdmissionPrincipalConfirmedInProgress`
- From: `InProgress`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `principal_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `InProgress`

### `ClassifyPublicConfirmationAdmissionPrincipalConfirmedBlocked`
- From: `Blocked`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `principal_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Blocked`

### `ClassifyPublicConfirmationAdmissionPrincipalConfirmedCompleted`
- From: `Completed`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `principal_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Completed`

### `ClassifyPublicConfirmationAdmissionPrincipalConfirmedCancelled`
- From: `Cancelled`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `principal_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Cancelled`

### `ClassifyPublicConfirmationAdmissionPrincipalConfirmedFailed`
- From: `Failed`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `principal_confirmed_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Failed`

### `ClassifyPublicConfirmationAdmissionSupervisorAbsent`
- From: `Absent`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `supervisor_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Absent`

### `ClassifyPublicConfirmationAdmissionSupervisorOpen`
- From: `Open`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `supervisor_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Open`

### `ClassifyPublicConfirmationAdmissionSupervisorInProgress`
- From: `InProgress`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `supervisor_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `InProgress`

### `ClassifyPublicConfirmationAdmissionSupervisorBlocked`
- From: `Blocked`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `supervisor_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Blocked`

### `ClassifyPublicConfirmationAdmissionSupervisorCompleted`
- From: `Completed`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `supervisor_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Completed`

### `ClassifyPublicConfirmationAdmissionSupervisorCancelled`
- From: `Cancelled`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `supervisor_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Cancelled`

### `ClassifyPublicConfirmationAdmissionSupervisorFailed`
- From: `Failed`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `supervisor_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Failed`

### `ClassifyPublicConfirmationAdmissionReviewerQuorumAbsent`
- From: `Absent`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `reviewer_quorum_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Absent`

### `ClassifyPublicConfirmationAdmissionReviewerQuorumOpen`
- From: `Open`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `reviewer_quorum_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Open`

### `ClassifyPublicConfirmationAdmissionReviewerQuorumInProgress`
- From: `InProgress`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `reviewer_quorum_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `InProgress`

### `ClassifyPublicConfirmationAdmissionReviewerQuorumBlocked`
- From: `Blocked`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `reviewer_quorum_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Blocked`

### `ClassifyPublicConfirmationAdmissionReviewerQuorumCompleted`
- From: `Completed`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `reviewer_quorum_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Completed`

### `ClassifyPublicConfirmationAdmissionReviewerQuorumCancelled`
- From: `Cancelled`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `reviewer_quorum_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Cancelled`

### `ClassifyPublicConfirmationAdmissionReviewerQuorumFailed`
- From: `Failed`
- On: `ClassifyPublicConfirmationAdmission`(completion_policy)
- Guards:
  - `reviewer_quorum_requires_trusted_host`
- Emits: `PublicConfirmationAdmissionClassified`
- To: `Failed`

### `ClassifyCompletionPolicyMutationAdmissionUnchangedAbsent`
- From: `Absent`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_unchanged`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `Absent`

### `ClassifyCompletionPolicyMutationAdmissionUnchangedOpen`
- From: `Open`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_unchanged`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `Open`

### `ClassifyCompletionPolicyMutationAdmissionUnchangedInProgress`
- From: `InProgress`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_unchanged`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `InProgress`

### `ClassifyCompletionPolicyMutationAdmissionUnchangedBlocked`
- From: `Blocked`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_unchanged`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `Blocked`

### `ClassifyCompletionPolicyMutationAdmissionUnchangedCompleted`
- From: `Completed`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_unchanged`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `Completed`

### `ClassifyCompletionPolicyMutationAdmissionUnchangedCancelled`
- From: `Cancelled`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_unchanged`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `Cancelled`

### `ClassifyCompletionPolicyMutationAdmissionUnchangedFailed`
- From: `Failed`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_unchanged`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `Failed`

### `ClassifyCompletionPolicyMutationAdmissionChangedAbsent`
- From: `Absent`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_changed`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `Absent`

### `ClassifyCompletionPolicyMutationAdmissionChangedOpen`
- From: `Open`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_changed`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `Open`

### `ClassifyCompletionPolicyMutationAdmissionChangedInProgress`
- From: `InProgress`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_changed`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `InProgress`

### `ClassifyCompletionPolicyMutationAdmissionChangedBlocked`
- From: `Blocked`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_changed`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `Blocked`

### `ClassifyCompletionPolicyMutationAdmissionChangedCompleted`
- From: `Completed`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_changed`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `Completed`

### `ClassifyCompletionPolicyMutationAdmissionChangedCancelled`
- From: `Cancelled`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_changed`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `Cancelled`

### `ClassifyCompletionPolicyMutationAdmissionChangedFailed`
- From: `Failed`
- On: `ClassifyCompletionPolicyMutationAdmission`(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
- Guards:
  - `completion_policy_changed`
- Emits: `CompletionPolicyMutationAdmissionClassified`
- To: `Failed`

## Coverage
### Code Anchors
- `meerkat-workgraph/src/machine.rs` — WorkGraphMachine domain-facing lifecycle transition seam over CreateDefaultOrOpen, CreateRequestedBlocked, CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, ClaimOpen, ClaimExpiredInProgress, ReleaseInProgress, BlockOpen, BlockInProgress, BlockBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, ClassifyTerminalityAbsent, ClassifyTerminalityOpen, ClassifyTerminalityInProgress, ClassifyTerminalityBlocked, ClassifyTerminalityCompleted, ClassifyTerminalityCancelled, ClassifyTerminalityFailed, ValidateLink, CloseOpenDefaultOrCompleted, CloseInProgressDefaultOrCompleted, CloseBlockedDefaultOrCompleted, CloseOpenRequestedCancelled, CloseInProgressRequestedCancelled, CloseBlockedRequestedCancelled, CloseOpenRequestedFailed, CloseInProgressRequestedFailed, CloseBlockedRequestedFailed, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed, ClassifyCreateStatusAdmissionOpen, ClassifyCreateStatusAdmissionBlocked, ClassifyCreateStatusAdmissionDeniedAbsent, ClassifyCreateStatusAdmissionDeniedInProgress, ClassifyCreateStatusAdmissionDeniedCompleted, ClassifyCreateStatusAdmissionDeniedCancelled, ClassifyCreateStatusAdmissionDeniedFailed, ClassifyPublicConfirmationAdmissionSelfAttest, ClassifyPublicConfirmationAdmissionHostConfirmed, ClassifyPublicConfirmationAdmissionPrincipalConfirmed, ClassifyPublicConfirmationAdmissionSupervisor, ClassifyPublicConfirmationAdmissionReviewerQuorum, ClassifyCompletionPolicyMutationAdmissionUnchanged, ClassifyCompletionPolicyMutationAdmissionChanged; effects Created, Updated, Claimed, Released, Blocked, BlockerSatisfied, BlockerUnsatisfied, LifecycleTerminal, LifecycleNonTerminal, LinkValidated, Closed, EvidenceAdded, CreateStatusAdmissionClassified, PublicConfirmationAdmissionClassified, CompletionPolicyMutationAdmissionClassified; invariants absent_has_zero_revision, live_has_positive_revision, terminal_has_terminal_time, claim_only_in_progress, blocked_has_no_claim, terminal_has_no_claim; revision, leases, due eligibility, unresolved blockers, blocker satisfaction, public status defaults, terminality classification, create status admission, public confirmation admission, completion policy mutation admission, and topology legality

### Scenarios
- `workgraph_create_update_ready_claim` — CreateDefaultOrOpen, CreateRequestedBlocked, CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, Created, Updated, ClaimOpen, ClaimExpiredInProgress, Claimed, due eligibility, blocker satisfaction, public create status defaulting, create status admission classifies open and blocked as admissible creation states and denies the rest, and CAS revision
- `workgraph_claim_release_recovery` — only one active claim exists, ReleaseInProgress, Released, expired leases become recoverable through machine-approved claim, claim_only_in_progress, blocked_has_no_claim, and terminal_has_no_claim
- `workgraph_block_close_evidence` — BlockOpen, BlockInProgress, BlockBlocked, Blocked, CloseOpenDefaultOrCompleted, CloseInProgressDefaultOrCompleted, CloseBlockedDefaultOrCompleted, CloseOpenRequestedCancelled, CloseInProgressRequestedCancelled, CloseBlockedRequestedCancelled, CloseOpenRequestedFailed, CloseInProgressRequestedFailed, CloseBlockedRequestedFailed, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, Closed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed, EvidenceAdded, public close status defaulting, public confirmation admission admits only a self-attested completion policy and denies every other policy as requiring trusted host, absent_has_zero_revision, live_has_positive_revision, and terminal_has_terminal_time
- `workgraph_topology_legality` — ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, ClassifyTerminalityAbsent, ClassifyTerminalityOpen, ClassifyTerminalityInProgress, ClassifyTerminalityBlocked, ClassifyTerminalityCompleted, ClassifyTerminalityCancelled, ClassifyTerminalityFailed, BlockerSatisfied, BlockerUnsatisfied, LifecycleTerminal, LifecycleNonTerminal, ValidateLink, and LinkValidated reject missing endpoints, self edges, duplicate edges, dependency cycles, and unsatisfied blockers without adding a separate topology machine
