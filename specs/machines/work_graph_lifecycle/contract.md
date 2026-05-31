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

## Helpers
- `completion_policy_payload_valid`(policy: WorkCompletionPolicy, supervisor_owner_key: Option<WorkOwnerKey>, reviewer_quorum_threshold: Option<u64>) -> `Bool`
- `completion_policy_is_satisfied`(policy: WorkCompletionPolicy, supervisor_owner_key: Option<WorkOwnerKey>, reviewer_quorum_threshold: Option<u64>, host_confirmation_count: u64, principal_confirmation_count: u64, supervisor_confirmation_owner_keys: Set<WorkOwnerKey>, reviewer_confirmation_owner_keys: Set<WorkOwnerKey>) -> `Bool`
- `evidence_kind_owner_key_present`(evidence_kind: WorkEvidenceKind, confirming_owner_key: Option<WorkOwnerKey>) -> `Bool`

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

## Coverage
### Code Anchors
- `meerkat-workgraph/src/machine.rs` — WorkGraphMachine domain-facing lifecycle transition seam over CreateDefaultOrOpen, CreateRequestedBlocked, CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, ClaimOpen, ClaimExpiredInProgress, ReleaseInProgress, BlockOpen, BlockInProgress, BlockBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, ClassifyTerminalityAbsent, ClassifyTerminalityOpen, ClassifyTerminalityInProgress, ClassifyTerminalityBlocked, ClassifyTerminalityCompleted, ClassifyTerminalityCancelled, ClassifyTerminalityFailed, ValidateLink, CloseOpenDefaultOrCompleted, CloseInProgressDefaultOrCompleted, CloseBlockedDefaultOrCompleted, CloseOpenRequestedCancelled, CloseInProgressRequestedCancelled, CloseBlockedRequestedCancelled, CloseOpenRequestedFailed, CloseInProgressRequestedFailed, CloseBlockedRequestedFailed, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed; effects Created, Updated, Claimed, Released, Blocked, BlockerSatisfied, BlockerUnsatisfied, LifecycleTerminal, LifecycleNonTerminal, LinkValidated, Closed, EvidenceAdded; invariants absent_has_zero_revision, live_has_positive_revision, terminal_has_terminal_time, claim_only_in_progress, blocked_has_no_claim, terminal_has_no_claim; revision, leases, due eligibility, unresolved blockers, blocker satisfaction, public status defaults, terminality classification, and topology legality

### Scenarios
- `workgraph_create_update_ready_claim` — CreateDefaultOrOpen, CreateRequestedBlocked, CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, Created, Updated, ClaimOpen, ClaimExpiredInProgress, Claimed, due eligibility, blocker satisfaction, public create status defaulting, and CAS revision
- `workgraph_claim_release_recovery` — only one active claim exists, ReleaseInProgress, Released, expired leases become recoverable through machine-approved claim, claim_only_in_progress, blocked_has_no_claim, and terminal_has_no_claim
- `workgraph_block_close_evidence` — BlockOpen, BlockInProgress, BlockBlocked, Blocked, CloseOpenDefaultOrCompleted, CloseInProgressDefaultOrCompleted, CloseBlockedDefaultOrCompleted, CloseOpenRequestedCancelled, CloseInProgressRequestedCancelled, CloseBlockedRequestedCancelled, CloseOpenRequestedFailed, CloseInProgressRequestedFailed, CloseBlockedRequestedFailed, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, Closed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed, EvidenceAdded, public close status defaulting, absent_has_zero_revision, live_has_positive_revision, and terminal_has_terminal_time
- `workgraph_topology_legality` — ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, ClassifyTerminalityAbsent, ClassifyTerminalityOpen, ClassifyTerminalityInProgress, ClassifyTerminalityBlocked, ClassifyTerminalityCompleted, ClassifyTerminalityCancelled, ClassifyTerminalityFailed, BlockerSatisfied, BlockerUnsatisfied, LifecycleTerminal, LifecycleNonTerminal, ValidateLink, and LinkValidated reject missing endpoints, self edges, duplicate edges, dependency cycles, and unsatisfied blockers without adding a separate topology machine
