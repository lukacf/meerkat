# WorkGraphLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `WorkGraphLifecycleMachine`

### Code Anchors
- `workgraph_lifecycle`: `meerkat-workgraph/src/machine.rs` — WorkGraphMachine domain-facing lifecycle transition seam over CreateDefaultOrOpen, CreateRequestedBlocked, CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, ClaimOpen, ClaimExpiredInProgress, ReleaseInProgress, BlockOpen, BlockInProgress, BlockBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, ClassifyTerminalityAbsent, ClassifyTerminalityOpen, ClassifyTerminalityInProgress, ClassifyTerminalityBlocked, ClassifyTerminalityCompleted, ClassifyTerminalityCancelled, ClassifyTerminalityFailed, ValidateLink, CloseOpenDefaultOrCompleted, CloseInProgressDefaultOrCompleted, CloseBlockedDefaultOrCompleted, CloseOpenRequestedCancelled, CloseInProgressRequestedCancelled, CloseBlockedRequestedCancelled, CloseOpenRequestedFailed, CloseInProgressRequestedFailed, CloseBlockedRequestedFailed, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed; effects Created, Updated, Claimed, Released, Blocked, BlockerSatisfied, BlockerUnsatisfied, LifecycleTerminal, LifecycleNonTerminal, LinkValidated, Closed, EvidenceAdded; invariants absent_has_zero_revision, live_has_positive_revision, terminal_has_terminal_time, claim_only_in_progress, blocked_has_no_claim, terminal_has_no_claim; revision, leases, due eligibility, unresolved blockers, blocker satisfaction, public status defaults, terminality classification, and topology legality

### Scenarios
- `workgraph_create_update_ready_claim` — CreateDefaultOrOpen, CreateRequestedBlocked, CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, Created, Updated, ClaimOpen, ClaimExpiredInProgress, Claimed, due eligibility, blocker satisfaction, public create status defaulting, and CAS revision
- `workgraph_claim_release_recovery` — only one active claim exists, ReleaseInProgress, Released, expired leases become recoverable through machine-approved claim, claim_only_in_progress, blocked_has_no_claim, and terminal_has_no_claim
- `workgraph_block_close_evidence` — BlockOpen, BlockInProgress, BlockBlocked, Blocked, CloseOpenDefaultOrCompleted, CloseInProgressDefaultOrCompleted, CloseBlockedDefaultOrCompleted, CloseOpenRequestedCancelled, CloseInProgressRequestedCancelled, CloseBlockedRequestedCancelled, CloseOpenRequestedFailed, CloseInProgressRequestedFailed, CloseBlockedRequestedFailed, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, Closed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed, EvidenceAdded, public close status defaulting, absent_has_zero_revision, live_has_positive_revision, and terminal_has_terminal_time
- `workgraph_topology_legality` — ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, ClassifyTerminalityAbsent, ClassifyTerminalityOpen, ClassifyTerminalityInProgress, ClassifyTerminalityBlocked, ClassifyTerminalityCompleted, ClassifyTerminalityCancelled, ClassifyTerminalityFailed, BlockerSatisfied, BlockerUnsatisfied, LifecycleTerminal, LifecycleNonTerminal, ValidateLink, and LinkValidated reject missing endpoints, self edges, duplicate edges, dependency cycles, and unsatisfied blockers without adding a separate topology machine

### Transitions
- `CreateOpen`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`
- `CreateBlocked`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`
- `UpdateOpen`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`
- `UpdateInProgress`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`
- `UpdateBlocked`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`
- `ClaimOpen`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`
- `ClaimExpiredInProgress`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`, `workgraph_claim_release_recovery`
- `ReleaseInProgress`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_claim_release_recovery`
- `BlockOpen`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`, `workgraph_block_close_evidence`, `workgraph_topology_legality`
- `BlockInProgress`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`, `workgraph_claim_release_recovery`, `workgraph_block_close_evidence`, `workgraph_topology_legality`
- `BlockBlocked`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`, `workgraph_claim_release_recovery`, `workgraph_block_close_evidence`, `workgraph_topology_legality`
- `RefreshEligibilityOpen`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`
- `RefreshEligibilityInProgress`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`
- `RefreshEligibilityBlocked`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`
- `ValidateLink`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_topology_legality`
- `CloseOpenCompleted`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `CloseInProgressCompleted`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `CloseBlockedCompleted`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `CloseOpenCancelled`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `CloseInProgressCancelled`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `CloseBlockedCancelled`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `CloseOpenFailed`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `CloseInProgressFailed`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `CloseBlockedFailed`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `AddEvidenceOpen`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `AddEvidenceInProgress`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `AddEvidenceBlocked`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `AddEvidenceCompleted`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `AddEvidenceCancelled`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `AddEvidenceFailed`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`

### Effects
- `Created`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`
- `Updated`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`
- `Claimed`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`
- `Released`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_claim_release_recovery`
- `Blocked`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_create_update_ready_claim`, `workgraph_claim_release_recovery`, `workgraph_block_close_evidence`, `workgraph_topology_legality`
- `LinkValidated`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_topology_legality`
- `Closed`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `EvidenceAdded`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`

### Invariants
- `absent_has_zero_revision`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `live_has_positive_revision`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `topology_snapshot_is_stateless`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_topology_legality`
- `terminal_has_terminal_time`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `claim_only_in_progress`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_claim_release_recovery`
- `blocked_has_no_claim`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_claim_release_recovery`
- `terminal_has_no_claim`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_claim_release_recovery`
- `supervisor_policy_has_owner`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_claim_release_recovery`, `workgraph_block_close_evidence`
- `non_supervisor_policy_has_no_owner`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_claim_release_recovery`, `workgraph_block_close_evidence`, `workgraph_topology_legality`
- `reviewer_quorum_policy_has_positive_threshold`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_block_close_evidence`
- `non_reviewer_quorum_policy_has_no_threshold`
  - anchors: `workgraph_lifecycle`
  - scenarios: `workgraph_claim_release_recovery`, `workgraph_block_close_evidence`, `workgraph_topology_legality`


<!-- GENERATED_COVERAGE_END -->
