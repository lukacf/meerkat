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
- `terminal_at_utc_ms`: `Option<u64>`
- `evidence_count`: `u64`

## Inputs
- `CreateOpen`(due_at_utc_ms: Option<u64>, not_before_utc_ms: Option<u64>, snoozed_until_utc_ms: Option<u64>, unresolved_blocker_count: u64)
- `CreateBlocked`(due_at_utc_ms: Option<u64>, not_before_utc_ms: Option<u64>, snoozed_until_utc_ms: Option<u64>, unresolved_blocker_count: u64)
- `Update`(expected_revision: u64, due_at_utc_ms: Option<u64>, not_before_utc_ms: Option<u64>, snoozed_until_utc_ms: Option<u64>, unresolved_blocker_count: u64)
- `Claim`(expected_revision: u64, owner_key: WorkOwnerKey, now_utc_ms: u64, lease_expires_at_utc_ms: Option<u64>)
- `Release`(expected_revision: u64)
- `Block`(expected_revision: u64)
- `RefreshEligibility`(unresolved_blocker_count: u64)
- `ClassifyBlockerSatisfaction`
- `ValidateLink`(kind: WorkEdgeKind, from_item_key: WorkItemKey, to_item_key: WorkItemKey, edge_key: WorkEdgeKey, reverse_path_key: WorkDependencyPathKey)
- `CloseCompleted`(expected_revision: u64, at_utc_ms: u64)
- `CloseCancelled`(expected_revision: u64, at_utc_ms: u64)
- `CloseFailed`(expected_revision: u64, at_utc_ms: u64)
- `AddEvidence`(expected_revision: u64)

## Signals

## Effects
- `Created`
- `Updated`
- `Claimed`(owner_key: WorkOwnerKey)
- `Released`
- `Blocked`
- `BlockerSatisfied`
- `BlockerUnsatisfied`
- `LinkValidated`
- `Closed`(terminal_state: WorkLifecycleState)
- `EvidenceAdded`

## Invariants
- `absent_has_zero_revision`
- `live_has_positive_revision`
- `topology_snapshot_is_stateless`
- `terminal_has_terminal_time`
- `claim_only_in_progress`
- `blocked_has_no_claim`
- `terminal_has_no_claim`

## Transitions
### `CreateOpen`
- From: `Absent`
- On: `CreateOpen`(due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count)
- Emits: `Created`
- To: `Open`

### `CreateBlocked`
- From: `Absent`
- On: `CreateBlocked`(due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count)
- Emits: `Created`
- To: `Blocked`

### `UpdateOpen`
- From: `Open`
- On: `Update`(expected_revision, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count)
- Guards:
  - ``
- Emits: `Updated`
- To: `Open`

### `UpdateInProgress`
- From: `InProgress`
- On: `Update`(expected_revision, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count)
- Guards:
  - ``
- Emits: `Updated`
- To: `InProgress`

### `UpdateBlocked`
- From: `Blocked`
- On: `Update`(expected_revision, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count)
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
- On: `RefreshEligibility`(unresolved_blocker_count)
- Emits: `Updated`
- To: `Open`

### `RefreshEligibilityInProgress`
- From: `InProgress`
- On: `RefreshEligibility`(unresolved_blocker_count)
- Emits: `Updated`
- To: `InProgress`

### `RefreshEligibilityBlocked`
- From: `Blocked`
- On: `RefreshEligibility`(unresolved_blocker_count)
- Emits: `Updated`
- To: `Blocked`

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
- On: `AddEvidence`(expected_revision)
- Guards:
  - ``
- Emits: `EvidenceAdded`
- To: `Open`

### `AddEvidenceInProgress`
- From: `InProgress`
- On: `AddEvidence`(expected_revision)
- Guards:
  - ``
- Emits: `EvidenceAdded`
- To: `InProgress`

### `AddEvidenceBlocked`
- From: `Blocked`
- On: `AddEvidence`(expected_revision)
- Guards:
  - ``
- Emits: `EvidenceAdded`
- To: `Blocked`

### `AddEvidenceCompleted`
- From: `Completed`
- On: `AddEvidence`(expected_revision)
- Guards:
  - ``
- Emits: `EvidenceAdded`
- To: `Completed`

### `AddEvidenceCancelled`
- From: `Cancelled`
- On: `AddEvidence`(expected_revision)
- Guards:
  - ``
- Emits: `EvidenceAdded`
- To: `Cancelled`

### `AddEvidenceFailed`
- From: `Failed`
- On: `AddEvidence`(expected_revision)
- Guards:
  - ``
- Emits: `EvidenceAdded`
- To: `Failed`

## Coverage
### Code Anchors
- `meerkat-workgraph/src/machine.rs` — WorkGraphMachine domain-facing lifecycle transition seam over CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, ClaimOpen, ClaimExpiredInProgress, ReleaseInProgress, BlockOpen, BlockInProgress, BlockBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, ValidateLink, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed; effects Created, Updated, Claimed, Released, Blocked, BlockerSatisfied, BlockerUnsatisfied, LinkValidated, Closed, EvidenceAdded; invariants absent_has_zero_revision, live_has_positive_revision, terminal_has_terminal_time, claim_only_in_progress, blocked_has_no_claim, terminal_has_no_claim; revision, leases, due eligibility, unresolved blockers, blocker satisfaction, and topology legality

### Scenarios
- `workgraph_create_update_ready_claim` — CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, Created, Updated, ClaimOpen, ClaimExpiredInProgress, Claimed, due eligibility, blocker satisfaction, and CAS revision
- `workgraph_claim_release_recovery` — only one active claim exists, ReleaseInProgress, Released, expired leases become recoverable through machine-approved claim, claim_only_in_progress, blocked_has_no_claim, and terminal_has_no_claim
- `workgraph_block_close_evidence` — BlockOpen, BlockInProgress, BlockBlocked, Blocked, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, Closed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed, EvidenceAdded, absent_has_zero_revision, live_has_positive_revision, and terminal_has_terminal_time
- `workgraph_topology_legality` — ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, BlockerSatisfied, BlockerUnsatisfied, ValidateLink, and LinkValidated reject missing endpoints, self edges, duplicate edges, dependency cycles, and unsatisfied blockers without adding a separate topology machine
