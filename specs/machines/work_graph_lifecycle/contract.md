# WorkGraphLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `3`
- Rust owner: `self` / `catalog::dsl::workgraph_lifecycle`

## State
- Phase enum: `Absent | Open | InProgress | Blocked | Completed | Cancelled | Failed`
- `revision`: `u64`
- `item_key`: `Option<WorkItemKey>`
- `external_ref_tokens`: `Seq<String>`
- `evidence_ref_tokens`: `Seq<String>`
- `unresolved_blocker_count`: `u64`
- `claim_owner_key`: `Option<WorkOwnerKey>`
- `claimed_at_utc_ms`: `Option<u64>`
- `lease_expires_at_utc_ms`: `Option<u64>`
- `due_at_utc_ms`: `Option<u64>`
- `not_before_utc_ms`: `Option<u64>`
- `snoozed_until_utc_ms`: `Option<u64>`
- `terminal_at_utc_ms`: `Option<u64>`
- `evidence_count`: `u64`

## Inputs
- `Create`(item_key: WorkItemKey, external_ref_tokens: Seq<String>, evidence_ref_tokens: Seq<String>, evidence_ref_count: u64, due_at_utc_ms: Option<u64>, not_before_utc_ms: Option<u64>, snoozed_until_utc_ms: Option<u64>, requested_status: Option<WorkLifecycleState>)
- `CreateOpen`(item_key: WorkItemKey, external_ref_tokens: Seq<String>, evidence_ref_tokens: Seq<String>, evidence_ref_count: u64, due_at_utc_ms: Option<u64>, not_before_utc_ms: Option<u64>, snoozed_until_utc_ms: Option<u64>)
- `CreateBlocked`(item_key: WorkItemKey, external_ref_tokens: Seq<String>, evidence_ref_tokens: Seq<String>, evidence_ref_count: u64, due_at_utc_ms: Option<u64>, not_before_utc_ms: Option<u64>, snoozed_until_utc_ms: Option<u64>)
- `Update`(expected_revision: u64, external_ref_tokens: Seq<String>, due_at_utc_ms: Option<u64>, not_before_utc_ms: Option<u64>, snoozed_until_utc_ms: Option<u64>)
- `Claim`(expected_revision: u64, owner_key: WorkOwnerKey, now_utc_ms: u64, lease_expires_at_utc_ms: Option<u64>)
- `Release`(expected_revision: u64)
- `Block`(expected_revision: u64)
- `RefreshEligibility`(target_item_key: WorkItemKey, blocking_from_item_keys: Set<WorkItemKey>, satisfied_blocker_item_keys: Set<WorkItemKey>, unsatisfied_blocker_item_keys: Set<WorkItemKey>)
- `ClassifyReadiness`(now_utc_ms: u64)
- `ClassifyBlockerSatisfaction`
- `ClassifyTerminality`
- `ValidateLink`(kind: WorkEdgeKind, from_item_key: WorkItemKey, to_item_key: WorkItemKey, edge_key: WorkEdgeKey, reverse_path_key: WorkDependencyPathKey, topology_item_keys: Set<WorkItemKey>, topology_edge_keys: Set<WorkEdgeKey>, blocks_reachability: Set<WorkDependencyPathKey>, parent_reachability: Set<WorkDependencyPathKey>)
- `Close`(expected_revision: u64, at_utc_ms: u64, requested_status: Option<WorkLifecycleState>)
- `CloseCompleted`(expected_revision: u64, at_utc_ms: u64)
- `CloseCancelled`(expected_revision: u64, at_utc_ms: u64)
- `CloseFailed`(expected_revision: u64, at_utc_ms: u64)
- `AddEvidence`(expected_revision: u64, evidence_ref_tokens: Seq<String>, evidence_ref_count: u64)
- `ClassifyPublicError`(error_kind: WorkGraphErrorKind)

## Signals

## Effects
- `Created`
- `Updated`
- `Claimed`(owner_key: WorkOwnerKey)
- `Released`
- `Blocked`
- `BlockerSatisfied`
- `BlockerUnsatisfied`
- `LifecycleTerminal`
- `LifecycleNonTerminal`
- `WorkReady`
- `WorkNotReady`
- `LinkValidated`
- `Closed`(terminal_state: WorkLifecycleState)
- `EvidenceAdded`
- `PublicErrorClassified`(public_class: WorkGraphPublicErrorClass)

## Helpers
- `workgraph_item_key_present`(item_key: WorkItemKey) -> `Bool`

## Invariants
- `absent_has_zero_revision`
- `live_has_positive_revision`
- `absent_has_no_item_projection`
- `live_has_item_key`
- `terminal_has_terminal_time`
- `claim_only_in_progress`
- `blocked_has_no_claim`
- `terminal_has_no_claim`

## Transitions
### `CreateDefaultOrOpen`
- From: `Absent`
- On: `Create`(item_key, external_ref_tokens, evidence_ref_tokens, evidence_ref_count, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, requested_status)
- Guards:
  - `default_or_open`
  - `item_key_present`
- Emits: `Created`
- To: `Open`

### `CreateRequestedBlocked`
- From: `Absent`
- On: `Create`(item_key, external_ref_tokens, evidence_ref_tokens, evidence_ref_count, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, requested_status)
- Guards:
  - `requested_blocked`
  - `item_key_present`
- Emits: `Created`
- To: `Blocked`

### `CreateOpen`
- From: `Absent`
- On: `CreateOpen`(item_key, external_ref_tokens, evidence_ref_tokens, evidence_ref_count, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms)
- Guards:
  - `item_key_present`
- Emits: `Created`
- To: `Open`

### `CreateBlocked`
- From: `Absent`
- On: `CreateBlocked`(item_key, external_ref_tokens, evidence_ref_tokens, evidence_ref_count, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms)
- Guards:
  - `item_key_present`
- Emits: `Created`
- To: `Blocked`

### `UpdateOpen`
- From: `Open`
- On: `Update`(expected_revision, external_ref_tokens, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms)
- Guards:
  - ``
- Emits: `Updated`
- To: `Open`

### `UpdateInProgress`
- From: `InProgress`
- On: `Update`(expected_revision, external_ref_tokens, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms)
- Guards:
  - ``
- Emits: `Updated`
- To: `InProgress`

### `UpdateBlocked`
- From: `Blocked`
- On: `Update`(expected_revision, external_ref_tokens, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms)
- Guards:
  - ``
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
- On: `RefreshEligibility`(target_item_key, blocking_from_item_keys, satisfied_blocker_item_keys, unsatisfied_blocker_item_keys)
- Guards:
  - `target_matches`
  - `partition_covers_blockers`
  - `satisfied_subset`
  - `unsatisfied_subset`
- Emits: `Updated`
- To: `Open`

### `RefreshEligibilityInProgress`
- From: `InProgress`
- On: `RefreshEligibility`(target_item_key, blocking_from_item_keys, satisfied_blocker_item_keys, unsatisfied_blocker_item_keys)
- Guards:
  - `target_matches`
  - `partition_covers_blockers`
  - `satisfied_subset`
  - `unsatisfied_subset`
- Emits: `Updated`
- To: `InProgress`

### `RefreshEligibilityBlocked`
- From: `Blocked`
- On: `RefreshEligibility`(target_item_key, blocking_from_item_keys, satisfied_blocker_item_keys, unsatisfied_blocker_item_keys)
- Guards:
  - `target_matches`
  - `partition_covers_blockers`
  - `satisfied_subset`
  - `unsatisfied_subset`
- Emits: `Updated`
- To: `Blocked`

### `ClassifyReadinessOpenReady`
- From: `Open`
- On: `ClassifyReadiness`(now_utc_ms)
- Guards:
  - `dependencies_satisfied`
  - `due_eligible`
  - `not_before_eligible`
  - `snooze_eligible`
- Emits: `WorkReady`
- To: `Open`

### `ClassifyReadinessExpiredInProgressReady`
- From: `InProgress`
- On: `ClassifyReadiness`(now_utc_ms)
- Guards:
  - `prior_claim_present`
  - `prior_claim_has_lease`
  - `prior_claim_expired`
  - `dependencies_satisfied`
  - `due_eligible`
  - `not_before_eligible`
  - `snooze_eligible`
- Emits: `WorkReady`
- To: `InProgress`

### `ClassifyReadinessAbsentNotReady`
- From: `Absent`
- On: `ClassifyReadiness`(now_utc_ms)
- Emits: `WorkNotReady`
- To: `Absent`

### `ClassifyReadinessOpenNotReady`
- From: `Open`
- On: `ClassifyReadiness`(now_utc_ms)
- Guards:
  - `not_ready`
- Emits: `WorkNotReady`
- To: `Open`

### `ClassifyReadinessInProgressNotReady`
- From: `InProgress`
- On: `ClassifyReadiness`(now_utc_ms)
- Guards:
  - `not_ready`
- Emits: `WorkNotReady`
- To: `InProgress`

### `ClassifyReadinessBlockedNotReady`
- From: `Blocked`
- On: `ClassifyReadiness`(now_utc_ms)
- Emits: `WorkNotReady`
- To: `Blocked`

### `ClassifyReadinessCompletedNotReady`
- From: `Completed`
- On: `ClassifyReadiness`(now_utc_ms)
- Emits: `WorkNotReady`
- To: `Completed`

### `ClassifyReadinessCancelledNotReady`
- From: `Cancelled`
- On: `ClassifyReadiness`(now_utc_ms)
- Emits: `WorkNotReady`
- To: `Cancelled`

### `ClassifyReadinessFailedNotReady`
- From: `Failed`
- On: `ClassifyReadiness`(now_utc_ms)
- Emits: `WorkNotReady`
- To: `Failed`

### `ClassifyBlockerSatisfiedCompleted`
- From: `Completed`
- On: `ClassifyBlockerSatisfaction`()
- Emits: `BlockerSatisfied`
- To: `Completed`

### `ClassifyBlockerUnsatisfiedAbsent`
- From: `Absent`
- On: `ClassifyBlockerSatisfaction`()
- Emits: `BlockerUnsatisfied`
- To: `Absent`

### `ClassifyBlockerUnsatisfiedOpen`
- From: `Open`
- On: `ClassifyBlockerSatisfaction`()
- Emits: `BlockerUnsatisfied`
- To: `Open`

### `ClassifyBlockerUnsatisfiedInProgress`
- From: `InProgress`
- On: `ClassifyBlockerSatisfaction`()
- Emits: `BlockerUnsatisfied`
- To: `InProgress`

### `ClassifyBlockerUnsatisfiedBlocked`
- From: `Blocked`
- On: `ClassifyBlockerSatisfaction`()
- Emits: `BlockerUnsatisfied`
- To: `Blocked`

### `ClassifyBlockerUnsatisfiedCancelled`
- From: `Cancelled`
- On: `ClassifyBlockerSatisfaction`()
- Emits: `BlockerUnsatisfied`
- To: `Cancelled`

### `ClassifyBlockerUnsatisfiedFailed`
- From: `Failed`
- On: `ClassifyBlockerSatisfaction`()
- Emits: `BlockerUnsatisfied`
- To: `Failed`

### `ClassifyTerminalityAbsent`
- From: `Absent`
- On: `ClassifyTerminality`()
- Emits: `LifecycleNonTerminal`
- To: `Absent`

### `ClassifyTerminalityOpen`
- From: `Open`
- On: `ClassifyTerminality`()
- Emits: `LifecycleNonTerminal`
- To: `Open`

### `ClassifyTerminalityInProgress`
- From: `InProgress`
- On: `ClassifyTerminality`()
- Emits: `LifecycleNonTerminal`
- To: `InProgress`

### `ClassifyTerminalityBlocked`
- From: `Blocked`
- On: `ClassifyTerminality`()
- Emits: `LifecycleNonTerminal`
- To: `Blocked`

### `ClassifyTerminalityCompleted`
- From: `Completed`
- On: `ClassifyTerminality`()
- Emits: `LifecycleTerminal`
- To: `Completed`

### `ClassifyTerminalityCancelled`
- From: `Cancelled`
- On: `ClassifyTerminality`()
- Emits: `LifecycleTerminal`
- To: `Cancelled`

### `ClassifyTerminalityFailed`
- From: `Failed`
- On: `ClassifyTerminality`()
- Emits: `LifecycleTerminal`
- To: `Failed`

### `ValidateLink`
- From: `Absent`
- On: `ValidateLink`(kind, from_item_key, to_item_key, edge_key, reverse_path_key, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability)
- Guards:
  - `from_endpoint_exists`
  - `to_endpoint_exists`
  - `not_self_edge`
  - `not_duplicate_edge`
  - `blocks_acyclic`
  - `parent_acyclic`
- Emits: `LinkValidated`
- To: `Absent`

### `CloseOpenDefaultOrCompleted`
- From: `Open`
- On: `Close`(expected_revision, at_utc_ms, requested_status)
- Guards:
  - `revision_matches`
  - `default_or_completed`
- Emits: `Closed`
- To: `Completed`

### `CloseInProgressDefaultOrCompleted`
- From: `InProgress`
- On: `Close`(expected_revision, at_utc_ms, requested_status)
- Guards:
  - `revision_matches`
  - `default_or_completed`
- Emits: `Closed`
- To: `Completed`

### `CloseBlockedDefaultOrCompleted`
- From: `Blocked`
- On: `Close`(expected_revision, at_utc_ms, requested_status)
- Guards:
  - `revision_matches`
  - `default_or_completed`
- Emits: `Closed`
- To: `Completed`

### `CloseOpenRequestedCancelled`
- From: `Open`
- On: `Close`(expected_revision, at_utc_ms, requested_status)
- Guards:
  - `revision_matches`
  - `requested_cancelled`
- Emits: `Closed`
- To: `Cancelled`

### `CloseInProgressRequestedCancelled`
- From: `InProgress`
- On: `Close`(expected_revision, at_utc_ms, requested_status)
- Guards:
  - `revision_matches`
  - `requested_cancelled`
- Emits: `Closed`
- To: `Cancelled`

### `CloseBlockedRequestedCancelled`
- From: `Blocked`
- On: `Close`(expected_revision, at_utc_ms, requested_status)
- Guards:
  - `revision_matches`
  - `requested_cancelled`
- Emits: `Closed`
- To: `Cancelled`

### `CloseOpenRequestedFailed`
- From: `Open`
- On: `Close`(expected_revision, at_utc_ms, requested_status)
- Guards:
  - `revision_matches`
  - `requested_failed`
- Emits: `Closed`
- To: `Failed`

### `CloseInProgressRequestedFailed`
- From: `InProgress`
- On: `Close`(expected_revision, at_utc_ms, requested_status)
- Guards:
  - `revision_matches`
  - `requested_failed`
- Emits: `Closed`
- To: `Failed`

### `CloseBlockedRequestedFailed`
- From: `Blocked`
- On: `Close`(expected_revision, at_utc_ms, requested_status)
- Guards:
  - `revision_matches`
  - `requested_failed`
- Emits: `Closed`
- To: `Failed`

### `CloseOpenCompleted`
- From: `Open`
- On: `CloseCompleted`(expected_revision, at_utc_ms)
- Guards:
  - ``
- Emits: `Closed`
- To: `Completed`

### `CloseInProgressCompleted`
- From: `InProgress`
- On: `CloseCompleted`(expected_revision, at_utc_ms)
- Guards:
  - ``
- Emits: `Closed`
- To: `Completed`

### `CloseBlockedCompleted`
- From: `Blocked`
- On: `CloseCompleted`(expected_revision, at_utc_ms)
- Guards:
  - ``
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
- On: `AddEvidence`(expected_revision, evidence_ref_tokens, evidence_ref_count)
- Guards:
  - ``
  - `evidence_refs_preserve_existing`
  - `evidence_count_advances_one`
- Emits: `EvidenceAdded`
- To: `Open`

### `AddEvidenceInProgress`
- From: `InProgress`
- On: `AddEvidence`(expected_revision, evidence_ref_tokens, evidence_ref_count)
- Guards:
  - ``
  - `evidence_refs_preserve_existing`
  - `evidence_count_advances_one`
- Emits: `EvidenceAdded`
- To: `InProgress`

### `AddEvidenceBlocked`
- From: `Blocked`
- On: `AddEvidence`(expected_revision, evidence_ref_tokens, evidence_ref_count)
- Guards:
  - ``
  - `evidence_refs_preserve_existing`
  - `evidence_count_advances_one`
- Emits: `EvidenceAdded`
- To: `Blocked`

### `AddEvidenceCompleted`
- From: `Completed`
- On: `AddEvidence`(expected_revision, evidence_ref_tokens, evidence_ref_count)
- Guards:
  - ``
  - `evidence_refs_preserve_existing`
  - `evidence_count_advances_one`
- Emits: `EvidenceAdded`
- To: `Completed`

### `AddEvidenceCancelled`
- From: `Cancelled`
- On: `AddEvidence`(expected_revision, evidence_ref_tokens, evidence_ref_count)
- Guards:
  - ``
  - `evidence_refs_preserve_existing`
  - `evidence_count_advances_one`
- Emits: `EvidenceAdded`
- To: `Cancelled`

### `AddEvidenceFailed`
- From: `Failed`
- On: `AddEvidence`(expected_revision, evidence_ref_tokens, evidence_ref_count)
- Guards:
  - ``
  - `evidence_refs_preserve_existing`
  - `evidence_count_advances_one`
- Emits: `EvidenceAdded`
- To: `Failed`

### `ClassifyPublicErrorNotFound`
- From: `Absent`
- On: `ClassifyPublicError`(error_kind)
- Guards:
  - ``
- Emits: `PublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorStaleRevision`
- From: `Absent`
- On: `ClassifyPublicError`(error_kind)
- Guards:
  - ``
- Emits: `PublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorConflict`
- From: `Absent`
- On: `ClassifyPublicError`(error_kind)
- Guards:
  - ``
- Emits: `PublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorInvalidTransition`
- From: `Absent`
- On: `ClassifyPublicError`(error_kind)
- Guards:
  - ``
- Emits: `PublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorInvalidInput`
- From: `Absent`
- On: `ClassifyPublicError`(error_kind)
- Guards:
  - ``
- Emits: `PublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorInvalidTimestampMillis`
- From: `Absent`
- On: `ClassifyPublicError`(error_kind)
- Guards:
  - ``
- Emits: `PublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorUnsupportedBackend`
- From: `Absent`
- On: `ClassifyPublicError`(error_kind)
- Guards:
  - ``
- Emits: `PublicErrorClassified`
- To: `Absent`

### `ClassifyPublicErrorStore`
- From: `Absent`
- On: `ClassifyPublicError`(error_kind)
- Guards:
  - ``
- Emits: `PublicErrorClassified`
- To: `Absent`

## Coverage
### Code Anchors
- `meerkat-workgraph/src/machine.rs` — WorkGraphMachine domain-facing lifecycle transition seam over CreateDefaultOrOpen, CreateRequestedBlocked, CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, ClaimOpen, ClaimExpiredInProgress, ReleaseInProgress, BlockOpen, BlockInProgress, BlockBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, ClassifyTerminalityAbsent, ClassifyTerminalityOpen, ClassifyTerminalityInProgress, ClassifyTerminalityBlocked, ClassifyTerminalityCompleted, ClassifyTerminalityCancelled, ClassifyTerminalityFailed, ValidateLink, CloseOpenDefaultOrCompleted, CloseInProgressDefaultOrCompleted, CloseBlockedDefaultOrCompleted, CloseOpenRequestedCancelled, CloseInProgressRequestedCancelled, CloseBlockedRequestedCancelled, CloseOpenRequestedFailed, CloseInProgressRequestedFailed, CloseBlockedRequestedFailed, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed; effects Created, Updated, Claimed, Released, Blocked, BlockerSatisfied, BlockerUnsatisfied, LifecycleTerminal, LifecycleNonTerminal, LinkValidated, Closed, EvidenceAdded; invariants absent_has_zero_revision, live_has_positive_revision, terminal_has_terminal_time, claim_only_in_progress, blocked_has_no_claim, terminal_has_no_claim; revision, leases, due eligibility, unresolved blockers, blocker satisfaction, public status defaults, terminality classification, and topology legality

### Scenarios
- `workgraph_create_update_ready_claim` — CreateDefaultOrOpen, CreateRequestedBlocked, CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, Created, Updated, ClaimOpen, ClaimExpiredInProgress, Claimed, due eligibility, blocker satisfaction, public create status defaulting, and CAS revision
- `workgraph_claim_release_recovery` — only one active claim exists, ReleaseInProgress, Released, expired leases become recoverable through machine-approved claim, claim_only_in_progress, blocked_has_no_claim, and terminal_has_no_claim
- `workgraph_block_close_evidence` — BlockOpen, BlockInProgress, BlockBlocked, Blocked, CloseOpenDefaultOrCompleted, CloseInProgressDefaultOrCompleted, CloseBlockedDefaultOrCompleted, CloseOpenRequestedCancelled, CloseInProgressRequestedCancelled, CloseBlockedRequestedCancelled, CloseOpenRequestedFailed, CloseInProgressRequestedFailed, CloseBlockedRequestedFailed, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, Closed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed, EvidenceAdded, public close status defaulting, absent_has_zero_revision, live_has_positive_revision, and terminal_has_terminal_time
- `workgraph_topology_legality` — ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, ClassifyTerminalityAbsent, ClassifyTerminalityOpen, ClassifyTerminalityInProgress, ClassifyTerminalityBlocked, ClassifyTerminalityCompleted, ClassifyTerminalityCancelled, ClassifyTerminalityFailed, BlockerSatisfied, BlockerUnsatisfied, LifecycleTerminal, LifecycleNonTerminal, ValidateLink, and LinkValidated reject missing endpoints, self edges, duplicate edges, dependency cycles, and unsatisfied blockers without adding a separate topology machine
