# TemporaryCouncilLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::temporary_council_lifecycle`

## State
- Phase enum: `Empty | Preparing | Running | Merging | Concluded | CleanupDebt | Settled`
- `revision`: `u64`
- `request_fingerprint`: `String`
- `exit_class`: `TemporaryCouncilExitClass`
- `cleanup_attempts`: `u64`
- `claim_id`: `String`
- `claim_epoch`: `u64`

## Inputs
- `Open`(request_fingerprint: String)
- `Claim`(claim_id: String, lease_expired: Bool)
- `StartDiscussion`(claim_id: String, claim_epoch: u64)
- `StartMerge`(claim_id: String, claim_epoch: u64)
- `SealResult`(claim_id: String, claim_epoch: u64)
- `SealInterruptedResult`(claim_id: String, claim_epoch: u64)
- `RecordCleanupSettled`(claim_id: String, claim_epoch: u64)
- `RecordCleanupDebt`(claim_id: String, claim_epoch: u64)
- `ClassifyRecovery`

## Signals

## Effects
- `CouncilOpened`(request_fingerprint: String)
- `CouncilOpenReplayed`(request_fingerprint: String)
- `CouncilOpenRejected`(reason: TemporaryCouncilOpenRejection)
- `DiscussionStarted`(revision: u64)
- `DiscussionStartReplayed`(revision: u64)
- `MergeStarted`(revision: u64)
- `MergeStartReplayed`(revision: u64)
- `AdvanceRejected`(reason: TemporaryCouncilAdvanceRejection)
- `ResultSealed`(revision: u64, exit_class: TemporaryCouncilExitClass)
- `ResultSealReplayed`(exit_class: TemporaryCouncilExitClass)
- `ResultSealRejected`(reason: TemporaryCouncilSealRejection)
- `CleanupSettled`(revision: u64, attempts: u64)
- `CleanupDebtRecorded`(revision: u64, attempts: u64)
- `CleanupSettlementReplayed`(attempts: u64)
- `CleanupRejected`(reason: TemporaryCouncilCleanupRejection)
- `RecoveryClassified`(unfinished: Bool, result_sealed: Bool, needs_cleanup: Bool)
- `ClaimGranted`(claim_id: String, claim_epoch: u64, took_over: Bool)
- `ClaimRenewed`(claim_id: String, claim_epoch: u64)
- `ClaimDenied`(reason: TemporaryCouncilClaimDenial, current_claim_epoch: u64)
- `CommandFenced`(current_claim_epoch: u64)

## Invariants
- `empty_record_has_no_council_facts`
- `opened_record_is_fingerprint_bound`
- `sealed_phase_has_exit_class`
- `unsealed_phase_has_no_exit_class`
- `cleanup_attempts_require_a_sealed_result`
- `debt_and_settlement_have_attempts`
- `claim_identity_and_epoch_agree`
- `only_a_bound_record_is_claimable`

## Transitions
### `OpenEmpty`
- From: `Empty`
- On: `Open`(request_fingerprint)
- Guards:
  - `well_formed_request`
- Emits: `CouncilOpened`
- To: `Preparing`

### `OpenEmptyMalformed`
- From: `Empty`
- On: `Open`(request_fingerprint)
- Guards:
  - `malformed_request`
- Emits: `CouncilOpenRejected`
- To: `Empty`

### `OpenReplayPreparing`
- From: `Preparing`
- On: `Open`(request_fingerprint)
- Guards:
  - `exact_request_replay`
- Emits: `CouncilOpenReplayed`
- To: `Preparing`

### `OpenReplayRunning`
- From: `Running`
- On: `Open`(request_fingerprint)
- Guards:
  - `exact_request_replay`
- Emits: `CouncilOpenReplayed`
- To: `Running`

### `OpenReplayMerging`
- From: `Merging`
- On: `Open`(request_fingerprint)
- Guards:
  - `exact_request_replay`
- Emits: `CouncilOpenReplayed`
- To: `Merging`

### `OpenReplayConcluded`
- From: `Concluded`
- On: `Open`(request_fingerprint)
- Guards:
  - `exact_request_replay`
- Emits: `CouncilOpenReplayed`
- To: `Concluded`

### `OpenReplayCleanupDebt`
- From: `CleanupDebt`
- On: `Open`(request_fingerprint)
- Guards:
  - `exact_request_replay`
- Emits: `CouncilOpenReplayed`
- To: `CleanupDebt`

### `OpenReplaySettled`
- From: `Settled`
- On: `Open`(request_fingerprint)
- Guards:
  - `exact_request_replay`
- Emits: `CouncilOpenReplayed`
- To: `Settled`

### `OpenConflictPreparing`
- From: `Preparing`
- On: `Open`(request_fingerprint)
- Guards:
  - `conflicting_request`
- Emits: `CouncilOpenRejected`
- To: `Preparing`

### `OpenConflictRunning`
- From: `Running`
- On: `Open`(request_fingerprint)
- Guards:
  - `conflicting_request`
- Emits: `CouncilOpenRejected`
- To: `Running`

### `OpenConflictMerging`
- From: `Merging`
- On: `Open`(request_fingerprint)
- Guards:
  - `conflicting_request`
- Emits: `CouncilOpenRejected`
- To: `Merging`

### `OpenConflictConcluded`
- From: `Concluded`
- On: `Open`(request_fingerprint)
- Guards:
  - `conflicting_request`
- Emits: `CouncilOpenRejected`
- To: `Concluded`

### `OpenConflictCleanupDebt`
- From: `CleanupDebt`
- On: `Open`(request_fingerprint)
- Guards:
  - `conflicting_request`
- Emits: `CouncilOpenRejected`
- To: `CleanupDebt`

### `OpenConflictSettled`
- From: `Settled`
- On: `Open`(request_fingerprint)
- Guards:
  - `conflicting_request`
- Emits: `CouncilOpenRejected`
- To: `Settled`

### `ClaimNotOpened`
- From: `Empty`
- On: `Claim`(claim_id, lease_expired)
- Emits: `ClaimDenied`
- To: `Empty`

### `ClaimSettled`
- From: `Settled`
- On: `Claim`(claim_id, lease_expired)
- Emits: `ClaimDenied`
- To: `Settled`

### `ClaimMalformedPreparing`
- From: `Preparing`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `malformed_claim`
- Emits: `ClaimDenied`
- To: `Preparing`

### `ClaimMalformedRunning`
- From: `Running`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `malformed_claim`
- Emits: `ClaimDenied`
- To: `Running`

### `ClaimMalformedMerging`
- From: `Merging`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `malformed_claim`
- Emits: `ClaimDenied`
- To: `Merging`

### `ClaimMalformedConcluded`
- From: `Concluded`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `malformed_claim`
- Emits: `ClaimDenied`
- To: `Concluded`

### `ClaimMalformedCleanupDebt`
- From: `CleanupDebt`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `malformed_claim`
- Emits: `ClaimDenied`
- To: `CleanupDebt`

### `ClaimGrantPreparing`
- From: `Preparing`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `unheld`
- Emits: `ClaimGranted`
- To: `Preparing`

### `ClaimGrantRunning`
- From: `Running`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `unheld`
- Emits: `ClaimGranted`
- To: `Running`

### `ClaimGrantMerging`
- From: `Merging`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `unheld`
- Emits: `ClaimGranted`
- To: `Merging`

### `ClaimGrantConcluded`
- From: `Concluded`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `unheld`
- Emits: `ClaimGranted`
- To: `Concluded`

### `ClaimGrantCleanupDebt`
- From: `CleanupDebt`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `unheld`
- Emits: `ClaimGranted`
- To: `CleanupDebt`

### `ClaimRenewPreparing`
- From: `Preparing`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `same_holder`
- Emits: `ClaimRenewed`
- To: `Preparing`

### `ClaimRenewRunning`
- From: `Running`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `same_holder`
- Emits: `ClaimRenewed`
- To: `Running`

### `ClaimRenewMerging`
- From: `Merging`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `same_holder`
- Emits: `ClaimRenewed`
- To: `Merging`

### `ClaimRenewConcluded`
- From: `Concluded`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `same_holder`
- Emits: `ClaimRenewed`
- To: `Concluded`

### `ClaimRenewCleanupDebt`
- From: `CleanupDebt`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `same_holder`
- Emits: `ClaimRenewed`
- To: `CleanupDebt`

### `ClaimTakeoverPreparing`
- From: `Preparing`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `other_holder`
  - `lease_observed_expired`
- Emits: `ClaimGranted`
- To: `Preparing`

### `ClaimTakeoverRunning`
- From: `Running`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `other_holder`
  - `lease_observed_expired`
- Emits: `ClaimGranted`
- To: `Running`

### `ClaimTakeoverMerging`
- From: `Merging`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `other_holder`
  - `lease_observed_expired`
- Emits: `ClaimGranted`
- To: `Merging`

### `ClaimTakeoverConcluded`
- From: `Concluded`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `other_holder`
  - `lease_observed_expired`
- Emits: `ClaimGranted`
- To: `Concluded`

### `ClaimTakeoverCleanupDebt`
- From: `CleanupDebt`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `other_holder`
  - `lease_observed_expired`
- Emits: `ClaimGranted`
- To: `CleanupDebt`

### `ClaimBusyPreparing`
- From: `Preparing`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `other_holder`
  - `lease_not_expired`
- Emits: `ClaimDenied`
- To: `Preparing`

### `ClaimBusyRunning`
- From: `Running`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `other_holder`
  - `lease_not_expired`
- Emits: `ClaimDenied`
- To: `Running`

### `ClaimBusyMerging`
- From: `Merging`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `other_holder`
  - `lease_not_expired`
- Emits: `ClaimDenied`
- To: `Merging`

### `ClaimBusyConcluded`
- From: `Concluded`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `other_holder`
  - `lease_not_expired`
- Emits: `ClaimDenied`
- To: `Concluded`

### `ClaimBusyCleanupDebt`
- From: `CleanupDebt`
- On: `Claim`(claim_id, lease_expired)
- Guards:
  - `well_formed_claim`
  - `other_holder`
  - `lease_not_expired`
- Emits: `ClaimDenied`
- To: `CleanupDebt`

### `StartDiscussionPreparing`
- From: `Preparing`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `DiscussionStarted`
- To: `Running`

### `StartDiscussionReplay`
- From: `Running`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `DiscussionStartReplayed`
- To: `Running`

### `StartDiscussionNotOpened`
- From: `Empty`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Emits: `AdvanceRejected`
- To: `Empty`

### `StartDiscussionAlreadyAdvancedMerging`
- From: `Merging`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `AdvanceRejected`
- To: `Merging`

### `StartDiscussionAlreadyAdvancedConcluded`
- From: `Concluded`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `AdvanceRejected`
- To: `Concluded`

### `StartDiscussionAlreadyAdvancedCleanupDebt`
- From: `CleanupDebt`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `AdvanceRejected`
- To: `CleanupDebt`

### `StartDiscussionAlreadyAdvancedSettled`
- From: `Settled`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `AdvanceRejected`
- To: `Settled`

### `StartMergePreparing`
- From: `Preparing`
- On: `StartMerge`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `MergeStarted`
- To: `Merging`

### `StartMergeRunning`
- From: `Running`
- On: `StartMerge`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `MergeStarted`
- To: `Merging`

### `StartMergeReplay`
- From: `Merging`
- On: `StartMerge`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `MergeStartReplayed`
- To: `Merging`

### `StartMergeNotOpened`
- From: `Empty`
- On: `StartMerge`(claim_id, claim_epoch)
- Emits: `AdvanceRejected`
- To: `Empty`

### `StartMergeAlreadyAdvancedConcluded`
- From: `Concluded`
- On: `StartMerge`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `AdvanceRejected`
- To: `Concluded`

### `StartMergeAlreadyAdvancedCleanupDebt`
- From: `CleanupDebt`
- On: `StartMerge`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `AdvanceRejected`
- To: `CleanupDebt`

### `StartMergeAlreadyAdvancedSettled`
- From: `Settled`
- On: `StartMerge`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `AdvanceRejected`
- To: `Settled`

### `SealResultMerging`
- From: `Merging`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `ResultSealed`
- To: `Concluded`

### `SealResultReplayConcluded`
- From: `Concluded`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `already_executed`
  - `claim_matches`
- Emits: `ResultSealReplayed`
- To: `Concluded`

### `SealResultReplayCleanupDebt`
- From: `CleanupDebt`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `already_executed`
  - `claim_matches`
- Emits: `ResultSealReplayed`
- To: `CleanupDebt`

### `SealResultReplaySettled`
- From: `Settled`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `already_executed`
  - `claim_matches`
- Emits: `ResultSealReplayed`
- To: `Settled`

### `SealResultConflictConcluded`
- From: `Concluded`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `sealed_under_another_class`
  - `claim_matches`
- Emits: `ResultSealRejected`
- To: `Concluded`

### `SealResultConflictCleanupDebt`
- From: `CleanupDebt`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `sealed_under_another_class`
  - `claim_matches`
- Emits: `ResultSealRejected`
- To: `CleanupDebt`

### `SealResultConflictSettled`
- From: `Settled`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `sealed_under_another_class`
  - `claim_matches`
- Emits: `ResultSealRejected`
- To: `Settled`

### `SealResultNotOpened`
- From: `Empty`
- On: `SealResult`(claim_id, claim_epoch)
- Emits: `ResultSealRejected`
- To: `Empty`

### `SealResultNotMergingPreparing`
- From: `Preparing`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `ResultSealRejected`
- To: `Preparing`

### `SealResultNotMergingRunning`
- From: `Running`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `ResultSealRejected`
- To: `Running`

### `SealInterruptedPreparing`
- From: `Preparing`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `ResultSealed`
- To: `Concluded`

### `SealInterruptedRunning`
- From: `Running`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `ResultSealed`
- To: `Concluded`

### `SealInterruptedMerging`
- From: `Merging`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `ResultSealed`
- To: `Concluded`

### `SealInterruptedReplayConcluded`
- From: `Concluded`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `already_interrupted`
  - `claim_matches`
- Emits: `ResultSealReplayed`
- To: `Concluded`

### `SealInterruptedReplayCleanupDebt`
- From: `CleanupDebt`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `already_interrupted`
  - `claim_matches`
- Emits: `ResultSealReplayed`
- To: `CleanupDebt`

### `SealInterruptedReplaySettled`
- From: `Settled`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `already_interrupted`
  - `claim_matches`
- Emits: `ResultSealReplayed`
- To: `Settled`

### `SealInterruptedConflictConcluded`
- From: `Concluded`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `sealed_under_another_class`
  - `claim_matches`
- Emits: `ResultSealRejected`
- To: `Concluded`

### `SealInterruptedConflictCleanupDebt`
- From: `CleanupDebt`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `sealed_under_another_class`
  - `claim_matches`
- Emits: `ResultSealRejected`
- To: `CleanupDebt`

### `SealInterruptedConflictSettled`
- From: `Settled`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `sealed_under_another_class`
  - `claim_matches`
- Emits: `ResultSealRejected`
- To: `Settled`

### `SealInterruptedNotOpened`
- From: `Empty`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Emits: `ResultSealRejected`
- To: `Empty`

### `RecordCleanupSettledConcluded`
- From: `Concluded`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `CleanupSettled`
- To: `Settled`

### `RecordCleanupSettledAfterDebt`
- From: `CleanupDebt`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `CleanupSettled`
- To: `Settled`

### `RecordCleanupSettledReplay`
- From: `Settled`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `CleanupSettlementReplayed`
- To: `Settled`

### `RecordCleanupSettledNotSealedEmpty`
- From: `Empty`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Emits: `CleanupRejected`
- To: `Empty`

### `RecordCleanupSettledNotSealedPreparing`
- From: `Preparing`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `CleanupRejected`
- To: `Preparing`

### `RecordCleanupSettledNotSealedRunning`
- From: `Running`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `CleanupRejected`
- To: `Running`

### `RecordCleanupSettledNotSealedMerging`
- From: `Merging`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `CleanupRejected`
- To: `Merging`

### `RecordCleanupDebtConcluded`
- From: `Concluded`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `CleanupDebtRecorded`
- To: `CleanupDebt`

### `RecordCleanupDebtRetry`
- From: `CleanupDebt`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `CleanupDebtRecorded`
- To: `CleanupDebt`

### `RecordCleanupDebtAlreadySettled`
- From: `Settled`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `CleanupRejected`
- To: `Settled`

### `RecordCleanupDebtNotSealedEmpty`
- From: `Empty`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Emits: `CleanupRejected`
- To: `Empty`

### `RecordCleanupDebtNotSealedPreparing`
- From: `Preparing`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `CleanupRejected`
- To: `Preparing`

### `RecordCleanupDebtNotSealedRunning`
- From: `Running`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `CleanupRejected`
- To: `Running`

### `RecordCleanupDebtNotSealedMerging`
- From: `Merging`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Guards:
  - `claim_matches`
- Emits: `CleanupRejected`
- To: `Merging`

### `StartDiscussionFencedPreparing`
- From: `Preparing`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Preparing`

### `StartDiscussionFencedRunning`
- From: `Running`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Running`

### `StartDiscussionFencedMerging`
- From: `Merging`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Merging`

### `StartDiscussionFencedConcluded`
- From: `Concluded`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Concluded`

### `StartDiscussionFencedCleanupDebt`
- From: `CleanupDebt`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `CleanupDebt`

### `StartDiscussionFencedSettled`
- From: `Settled`
- On: `StartDiscussion`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Settled`

### `StartMergeFencedPreparing`
- From: `Preparing`
- On: `StartMerge`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Preparing`

### `StartMergeFencedRunning`
- From: `Running`
- On: `StartMerge`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Running`

### `StartMergeFencedMerging`
- From: `Merging`
- On: `StartMerge`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Merging`

### `StartMergeFencedConcluded`
- From: `Concluded`
- On: `StartMerge`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Concluded`

### `StartMergeFencedCleanupDebt`
- From: `CleanupDebt`
- On: `StartMerge`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `CleanupDebt`

### `StartMergeFencedSettled`
- From: `Settled`
- On: `StartMerge`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Settled`

### `SealResultFencedPreparing`
- From: `Preparing`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Preparing`

### `SealResultFencedRunning`
- From: `Running`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Running`

### `SealResultFencedMerging`
- From: `Merging`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Merging`

### `SealResultFencedConcluded`
- From: `Concluded`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Concluded`

### `SealResultFencedCleanupDebt`
- From: `CleanupDebt`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `CleanupDebt`

### `SealResultFencedSettled`
- From: `Settled`
- On: `SealResult`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Settled`

### `SealInterruptedResultFencedPreparing`
- From: `Preparing`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Preparing`

### `SealInterruptedResultFencedRunning`
- From: `Running`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Running`

### `SealInterruptedResultFencedMerging`
- From: `Merging`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Merging`

### `SealInterruptedResultFencedConcluded`
- From: `Concluded`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Concluded`

### `SealInterruptedResultFencedCleanupDebt`
- From: `CleanupDebt`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `CleanupDebt`

### `SealInterruptedResultFencedSettled`
- From: `Settled`
- On: `SealInterruptedResult`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Settled`

### `RecordCleanupSettledFencedPreparing`
- From: `Preparing`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Preparing`

### `RecordCleanupSettledFencedRunning`
- From: `Running`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Running`

### `RecordCleanupSettledFencedMerging`
- From: `Merging`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Merging`

### `RecordCleanupSettledFencedConcluded`
- From: `Concluded`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Concluded`

### `RecordCleanupSettledFencedCleanupDebt`
- From: `CleanupDebt`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `CleanupDebt`

### `RecordCleanupSettledFencedSettled`
- From: `Settled`
- On: `RecordCleanupSettled`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Settled`

### `RecordCleanupDebtFencedPreparing`
- From: `Preparing`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Preparing`

### `RecordCleanupDebtFencedRunning`
- From: `Running`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Running`

### `RecordCleanupDebtFencedMerging`
- From: `Merging`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Merging`

### `RecordCleanupDebtFencedConcluded`
- From: `Concluded`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Concluded`

### `RecordCleanupDebtFencedCleanupDebt`
- From: `CleanupDebt`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `CleanupDebt`

### `RecordCleanupDebtFencedSettled`
- From: `Settled`
- On: `RecordCleanupDebt`(claim_id, claim_epoch)
- Guards:
  - `stale_claim`
- Emits: `CommandFenced`
- To: `Settled`

### `ClassifyRecoveryEmpty`
- From: `Empty`
- On: `ClassifyRecovery`()
- Emits: `RecoveryClassified`
- To: `Empty`

### `ClassifyRecoveryPreparing`
- From: `Preparing`
- On: `ClassifyRecovery`()
- Emits: `RecoveryClassified`
- To: `Preparing`

### `ClassifyRecoveryRunning`
- From: `Running`
- On: `ClassifyRecovery`()
- Emits: `RecoveryClassified`
- To: `Running`

### `ClassifyRecoveryMerging`
- From: `Merging`
- On: `ClassifyRecovery`()
- Emits: `RecoveryClassified`
- To: `Merging`

### `ClassifyRecoveryConcluded`
- From: `Concluded`
- On: `ClassifyRecovery`()
- Emits: `RecoveryClassified`
- To: `Concluded`

### `ClassifyRecoveryCleanupDebt`
- From: `CleanupDebt`
- On: `ClassifyRecovery`()
- Emits: `RecoveryClassified`
- To: `CleanupDebt`

### `ClassifyRecoverySettled`
- From: `Settled`
- On: `ClassifyRecovery`()
- Emits: `RecoveryClassified`
- To: `Settled`

## Coverage
### Code Anchors
- `temporary_council_lifecycle` (machine `TemporaryCouncilLifecycleMachine`): `meerkat-mob/src/machines/temporary_council_lifecycle.rs` — TemporaryCouncilLifecycleMachine owns one temporary-council record: request-identity binding, discussion/merge advance, immutable result sealing (executed or coordinator-interrupted), cleanup settlement versus retained debt, and the recovery-sweep verdict

### Scenarios
- `temporary_council_request_identity_binding` — One council id binds exactly one canonical request fingerprint: the exact request replays, a materially different request is a typed conflict that never rebinds the identity, and an empty fingerprint is refused outright
- `temporary_council_advance_and_result_seal` — A council advances Preparing -> Running -> Merging with idempotent replays, seals exactly one immutable result, and refuses a second seal under a different terminal class; a council that never reached a runnable discussion still enters merge so the explicit merge-back policy can produce its typed not-attempted outcome
- `temporary_council_single_executor_claim_and_fencing` — Exactly one coordinator executes a council at a time: an unheld record grants, the same holder renews, a second coordinator is refused busy while the lease is not observed expired, an observed-expired lease admits a takeover that advances the epoch, and every command carrying the superseded epoch is fenced without mutating anything
- `temporary_council_interrupted_recovery_and_cleanup_convergence` — A coordinator that dies in any unsealed phase is sealed exactly once as CoordinatorInterrupted and never re-executed, cleanup debt is retained across attempts with a monotonic attempt count, a later attempt converges to Settled, and the recovery sweep verdict comes from the machine rather than a shell phase predicate
