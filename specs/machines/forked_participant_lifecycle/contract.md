# ForkedParticipantLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::forked_participant_lifecycle`

## State
- Phase enum: `Empty | Reserved | ActivationFailed | Active | Attached | RevocationPendingAttached | ExpiryPendingAttached | Revoked | Expired | Exhausted`
- `request_fingerprint`: `String`
- `max_uses`: `u64`
- `use_count`: `u64`
- `fork_activation_id`: `String`
- `active_attachment_id`: `Option<String>`
- `granted_attachment_ids`: `Set<String>`
- `cleanup_state`: `ForkedParticipantCleanupState`

## Inputs
- `Reserve`(request_fingerprint: String, max_uses: u64)
- `RecordForkActivation`(request_fingerprint: String, fork_activation_id: String)
- `RecordForkActivationFailure`(request_fingerprint: String)
- `Attach`(attachment_id: String, authentication_valid: Bool, expired: Bool)
- `Release`(attachment_id: String)
- `Revoke`(authentication_valid: Bool)
- `ObserveExpiry`(expired: Bool)
- `CompleteCleanup`

## Signals

## Effects
- `CapabilityReserved`(request_fingerprint: String, max_uses: u64)
- `ReservationReplayed`(request_fingerprint: String)
- `ReservationRejected`(reason: ForkedParticipantReservationRejection)
- `ForkActivated`(fork_activation_id: String)
- `ForkActivationReplayed`(fork_activation_id: String)
- `ForkActivationFailed`(request_fingerprint: String)
- `ForkActivationFailureReplayed`(request_fingerprint: String)
- `ActivationRejected`(reason: ForkedParticipantActivationRejection)
- `AttachmentGranted`(attachment_id: String, use_index: u64, remaining_uses: u64)
- `AttachmentGrantReplayed`(attachment_id: String, use_index: u64)
- `AttachDenied`(attachment_id: String, reason: ForkedParticipantAttachDenial)
- `AttachmentReleased`(attachment_id: String, use_count: u64)
- `ReleaseReplayed`(attachment_id: String)
- `ReleaseRejected`(attachment_id: String, reason: ForkedParticipantReleaseRejection)
- `CapabilityExhausted`(use_count: u64)
- `CapabilityExpired`(cleanup_pending: Bool)
- `ExpiryPendingRecorded`
- `ExpiryObservationIgnored`(reason: ForkedParticipantExpiryIgnore)
- `CapabilityRevoked`(cleanup_pending: Bool)
- `RevocationPendingRecorded`
- `RevocationConverged`
- `RevocationDenied`(reason: ForkedParticipantRevocationDenial)
- `CleanupCompleted`
- `CleanupCompletionReplayed`
- `CleanupCompletionRejected`(reason: ForkedParticipantCleanupRejection)

## Invariants
- `reserved_capability_has_positive_max_uses`
- `use_count_within_max_uses`
- `granted_attachments_match_use_count`
- `active_holder_is_a_granted_attachment`
- `attachment_only_while_attached`
- `attached_phase_holds_one_attachment`
- `terminal_capability_is_detached`
- `cleanup_complete_requires_detached_terminal`
- `deferred_cleanup_requires_attachment`
- `empty_record_has_no_capability_facts`
- `pre_activation_record_has_no_grants`
- `usable_capability_has_fork_activation`

## Transitions
### `ReserveEmpty`
- From: `Empty`
- On: `Reserve`(request_fingerprint, max_uses)
- Guards:
  - `well_formed_request`
- Emits: `CapabilityReserved`
- To: `Reserved`

### `ReserveEmptyMalformed`
- From: `Empty`
- On: `Reserve`(request_fingerprint, max_uses)
- Guards:
  - `malformed_request`
- Emits: `ReservationRejected`
- To: `Empty`

### `ReserveReservedReplay`
- From: `Reserved`
- On: `Reserve`(request_fingerprint, max_uses)
- Guards:
  - `exact_request_replay`
- Emits: `ReservationReplayed`
- To: `Reserved`

### `ReserveReservedConflict`
- From: `Reserved`
- On: `Reserve`(request_fingerprint, max_uses)
- Guards:
  - `conflicting_request`
- Emits: `ReservationRejected`
- To: `Reserved`

### `ReserveActivationFailedRetry`
- From: `ActivationFailed`
- On: `Reserve`(request_fingerprint, max_uses)
- Guards:
  - `exact_request_retry`
- Emits: `CapabilityReserved`
- To: `Reserved`

### `ReserveActivationFailedConflict`
- From: `ActivationFailed`
- On: `Reserve`(request_fingerprint, max_uses)
- Guards:
  - `conflicting_request`
- Emits: `ReservationRejected`
- To: `ActivationFailed`

### `ReserveAlreadyProvisionedActive`
- From: `Active`
- On: `Reserve`(request_fingerprint, max_uses)
- Emits: `ReservationRejected`
- To: `Active`

### `ReserveAlreadyProvisionedAttached`
- From: `Attached`
- On: `Reserve`(request_fingerprint, max_uses)
- Emits: `ReservationRejected`
- To: `Attached`

### `ReserveAlreadyProvisionedRevocationPendingAttached`
- From: `RevocationPendingAttached`
- On: `Reserve`(request_fingerprint, max_uses)
- Emits: `ReservationRejected`
- To: `RevocationPendingAttached`

### `ReserveAlreadyProvisionedExpiryPendingAttached`
- From: `ExpiryPendingAttached`
- On: `Reserve`(request_fingerprint, max_uses)
- Emits: `ReservationRejected`
- To: `ExpiryPendingAttached`

### `ReserveAlreadyProvisionedRevoked`
- From: `Revoked`
- On: `Reserve`(request_fingerprint, max_uses)
- Emits: `ReservationRejected`
- To: `Revoked`

### `ReserveAlreadyProvisionedExpired`
- From: `Expired`
- On: `Reserve`(request_fingerprint, max_uses)
- Emits: `ReservationRejected`
- To: `Expired`

### `ReserveAlreadyProvisionedExhausted`
- From: `Exhausted`
- On: `Reserve`(request_fingerprint, max_uses)
- Emits: `ReservationRejected`
- To: `Exhausted`

### `ActivateEmpty`
- From: `Empty`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Emits: `ActivationRejected`
- To: `Empty`

### `ActivateReserved`
- From: `Reserved`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `exact_request`
  - `well_formed_activation`
- Emits: `ForkActivated`
- To: `Active`

### `ActivateReservedMismatch`
- From: `Reserved`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `request_mismatch`
- Emits: `ActivationRejected`
- To: `Reserved`

### `ActivateReservedMalformed`
- From: `Reserved`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `exact_request`
  - `malformed_activation`
- Emits: `ActivationRejected`
- To: `Reserved`

### `ActivateActivationFailedRecovery`
- From: `ActivationFailed`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `exact_request`
  - `well_formed_activation`
- Emits: `ForkActivated`
- To: `Active`

### `ActivateActivationFailedMismatch`
- From: `ActivationFailed`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `request_mismatch`
- Emits: `ActivationRejected`
- To: `ActivationFailed`

### `ActivateActivationFailedMalformed`
- From: `ActivationFailed`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `exact_request`
  - `malformed_activation`
- Emits: `ActivationRejected`
- To: `ActivationFailed`

### `ActivateActiveReplay`
- From: `Active`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `exact_activation_replay`
- Emits: `ForkActivationReplayed`
- To: `Active`

### `ActivateActiveConflict`
- From: `Active`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `activation_conflict`
- Emits: `ActivationRejected`
- To: `Active`

### `ActivateReplayAfterActivationAttached`
- From: `Attached`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `exact_activation_replay`
- Emits: `ForkActivationReplayed`
- To: `Attached`

### `ActivateReplayAfterActivationRevocationPendingAttached`
- From: `RevocationPendingAttached`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `exact_activation_replay`
- Emits: `ForkActivationReplayed`
- To: `RevocationPendingAttached`

### `ActivateReplayAfterActivationExpiryPendingAttached`
- From: `ExpiryPendingAttached`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `exact_activation_replay`
- Emits: `ForkActivationReplayed`
- To: `ExpiryPendingAttached`

### `ActivateReplayAfterActivationRevoked`
- From: `Revoked`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `exact_activation_replay`
- Emits: `ForkActivationReplayed`
- To: `Revoked`

### `ActivateReplayAfterActivationExpired`
- From: `Expired`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `exact_activation_replay`
- Emits: `ForkActivationReplayed`
- To: `Expired`

### `ActivateReplayAfterActivationExhausted`
- From: `Exhausted`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `exact_activation_replay`
- Emits: `ForkActivationReplayed`
- To: `Exhausted`

### `ActivateAttachedConflictAttached`
- From: `Attached`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `activation_conflict`
- Emits: `ActivationRejected`
- To: `Attached`

### `ActivateAttachedConflictRevocationPendingAttached`
- From: `RevocationPendingAttached`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `activation_conflict`
- Emits: `ActivationRejected`
- To: `RevocationPendingAttached`

### `ActivateAttachedConflictExpiryPendingAttached`
- From: `ExpiryPendingAttached`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `activation_conflict`
- Emits: `ActivationRejected`
- To: `ExpiryPendingAttached`

### `ActivateTerminalConflictRevoked`
- From: `Revoked`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `activation_conflict`
- Emits: `ActivationRejected`
- To: `Revoked`

### `ActivateTerminalConflictExpired`
- From: `Expired`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `activation_conflict`
- Emits: `ActivationRejected`
- To: `Expired`

### `ActivateTerminalConflictExhausted`
- From: `Exhausted`
- On: `RecordForkActivation`(request_fingerprint, fork_activation_id)
- Guards:
  - `activation_conflict`
- Emits: `ActivationRejected`
- To: `Exhausted`

### `FailActivationReserved`
- From: `Reserved`
- On: `RecordForkActivationFailure`(request_fingerprint)
- Guards:
  - `exact_request`
- Emits: `ForkActivationFailed`
- To: `ActivationFailed`

### `FailActivationReservedMismatch`
- From: `Reserved`
- On: `RecordForkActivationFailure`(request_fingerprint)
- Guards:
  - `request_mismatch`
- Emits: `ActivationRejected`
- To: `Reserved`

### `FailActivationReplay`
- From: `ActivationFailed`
- On: `RecordForkActivationFailure`(request_fingerprint)
- Guards:
  - `exact_request`
- Emits: `ForkActivationFailureReplayed`
- To: `ActivationFailed`

### `FailActivationFailedMismatch`
- From: `ActivationFailed`
- On: `RecordForkActivationFailure`(request_fingerprint)
- Guards:
  - `request_mismatch`
- Emits: `ActivationRejected`
- To: `ActivationFailed`

### `FailActivationAfterActivationActive`
- From: `Active`
- On: `RecordForkActivationFailure`(request_fingerprint)
- Emits: `ActivationRejected`
- To: `Active`

### `FailActivationAfterActivationAttached`
- From: `Attached`
- On: `RecordForkActivationFailure`(request_fingerprint)
- Emits: `ActivationRejected`
- To: `Attached`

### `FailActivationAfterActivationRevocationPendingAttached`
- From: `RevocationPendingAttached`
- On: `RecordForkActivationFailure`(request_fingerprint)
- Emits: `ActivationRejected`
- To: `RevocationPendingAttached`

### `FailActivationAfterActivationExpiryPendingAttached`
- From: `ExpiryPendingAttached`
- On: `RecordForkActivationFailure`(request_fingerprint)
- Emits: `ActivationRejected`
- To: `ExpiryPendingAttached`

### `FailActivationAfterActivationExhausted`
- From: `Exhausted`
- On: `RecordForkActivationFailure`(request_fingerprint)
- Emits: `ActivationRejected`
- To: `Exhausted`

### `FailActivationNotReservedEmpty`
- From: `Empty`
- On: `RecordForkActivationFailure`(request_fingerprint)
- Emits: `ActivationRejected`
- To: `Empty`

### `FailActivationNotReservedRevoked`
- From: `Revoked`
- On: `RecordForkActivationFailure`(request_fingerprint)
- Emits: `ActivationRejected`
- To: `Revoked`

### `FailActivationNotReservedExpired`
- From: `Expired`
- On: `RecordForkActivationFailure`(request_fingerprint)
- Emits: `ActivationRejected`
- To: `Expired`

### `AttachAuthenticationInvalidEmpty`
- From: `Empty`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_invalid`
- Emits: `AttachDenied`
- To: `Empty`

### `AttachAuthenticationInvalidReserved`
- From: `Reserved`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_invalid`
- Emits: `AttachDenied`
- To: `Reserved`

### `AttachAuthenticationInvalidActivationFailed`
- From: `ActivationFailed`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_invalid`
- Emits: `AttachDenied`
- To: `ActivationFailed`

### `AttachAuthenticationInvalidActive`
- From: `Active`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_invalid`
- Emits: `AttachDenied`
- To: `Active`

### `AttachAuthenticationInvalidAttached`
- From: `Attached`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_invalid`
- Emits: `AttachDenied`
- To: `Attached`

### `AttachAuthenticationInvalidRevocationPendingAttached`
- From: `RevocationPendingAttached`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_invalid`
- Emits: `AttachDenied`
- To: `RevocationPendingAttached`

### `AttachAuthenticationInvalidExpiryPendingAttached`
- From: `ExpiryPendingAttached`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_invalid`
- Emits: `AttachDenied`
- To: `ExpiryPendingAttached`

### `AttachAuthenticationInvalidRevoked`
- From: `Revoked`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_invalid`
- Emits: `AttachDenied`
- To: `Revoked`

### `AttachAuthenticationInvalidExpired`
- From: `Expired`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_invalid`
- Emits: `AttachDenied`
- To: `Expired`

### `AttachAuthenticationInvalidExhausted`
- From: `Exhausted`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_invalid`
- Emits: `AttachDenied`
- To: `Exhausted`

### `AttachNotActiveEmpty`
- From: `Empty`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
- Emits: `AttachDenied`
- To: `Empty`

### `AttachNotActiveReserved`
- From: `Reserved`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
- Emits: `AttachDenied`
- To: `Reserved`

### `AttachNotActiveActivationFailed`
- From: `ActivationFailed`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
- Emits: `AttachDenied`
- To: `ActivationFailed`

### `AttachActiveMalformed`
- From: `Active`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
  - `malformed_attachment`
- Emits: `AttachDenied`
- To: `Active`

### `AttachActiveExpired`
- From: `Active`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
  - `well_formed_attachment`
  - `expiry_observed`
- Emits: `CapabilityExpired`, `AttachDenied`
- To: `Expired`

### `AttachActiveAlreadyReleased`
- From: `Active`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
  - `well_formed_attachment`
  - `not_expired`
  - `already_granted_attachment`
- Emits: `AttachDenied`
- To: `Active`

### `AttachActiveGrant`
- From: `Active`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
  - `well_formed_attachment`
  - `not_expired`
  - `fresh_attachment`
  - `reuse_budget_available`
- Emits: `AttachmentGranted`
- To: `Attached`

### `AttachActiveBudgetSpent`
- From: `Active`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
  - `well_formed_attachment`
  - `not_expired`
  - `fresh_attachment`
  - `reuse_budget_spent`
- Emits: `AttachDenied`
- To: `Active`

### `AttachAttachedReplay`
- From: `Attached`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
  - `exact_attachment_replay`
- Emits: `AttachmentGrantReplayed`
- To: `Attached`

### `AttachAttachedAlreadyReleased`
- From: `Attached`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
  - `different_attachment`
  - `already_granted_attachment`
- Emits: `AttachDenied`
- To: `Attached`

### `AttachAttachedBusy`
- From: `Attached`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
  - `different_attachment`
  - `fresh_attachment`
- Emits: `AttachDenied`
- To: `Attached`

### `AttachRevokedCapabilityRevocationPendingAttached`
- From: `RevocationPendingAttached`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
- Emits: `AttachDenied`
- To: `RevocationPendingAttached`

### `AttachRevokedCapabilityRevoked`
- From: `Revoked`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
- Emits: `AttachDenied`
- To: `Revoked`

### `AttachExpiredCapabilityExpiryPendingAttached`
- From: `ExpiryPendingAttached`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
- Emits: `AttachDenied`
- To: `ExpiryPendingAttached`

### `AttachExpiredCapabilityExpired`
- From: `Expired`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
- Emits: `AttachDenied`
- To: `Expired`

### `AttachExhaustedCapabilityExhausted`
- From: `Exhausted`
- On: `Attach`(attachment_id, authentication_valid, expired)
- Guards:
  - `authentication_valid`
- Emits: `AttachDenied`
- To: `Exhausted`

### `ReleaseAttachedWithBudgetLeft`
- From: `Attached`
- On: `Release`(attachment_id)
- Guards:
  - `exact_attachment`
  - `reuse_budget_left`
- Emits: `AttachmentReleased`
- To: `Active`

### `ReleaseAttachedExhausts`
- From: `Attached`
- On: `Release`(attachment_id)
- Guards:
  - `exact_attachment`
  - `reuse_budget_spent`
- Emits: `AttachmentReleased`, `CapabilityExhausted`
- To: `Exhausted`

### `ReleaseRevocationPending`
- From: `RevocationPendingAttached`
- On: `Release`(attachment_id)
- Guards:
  - `exact_attachment`
- Emits: `AttachmentReleased`, `CapabilityRevoked`
- To: `Revoked`

### `ReleaseExpiryPending`
- From: `ExpiryPendingAttached`
- On: `Release`(attachment_id)
- Guards:
  - `exact_attachment`
- Emits: `AttachmentReleased`, `CapabilityExpired`
- To: `Expired`

### `ReleaseDuplicateWhileAttachedAttached`
- From: `Attached`
- On: `Release`(attachment_id)
- Guards:
  - `different_attachment`
  - `already_granted_attachment`
- Emits: `ReleaseReplayed`
- To: `Attached`

### `ReleaseAttachmentMismatchAttached`
- From: `Attached`
- On: `Release`(attachment_id)
- Guards:
  - `different_attachment`
  - `unknown_attachment`
- Emits: `ReleaseRejected`
- To: `Attached`

### `ReleaseDuplicateWhileAttachedRevocationPendingAttached`
- From: `RevocationPendingAttached`
- On: `Release`(attachment_id)
- Guards:
  - `different_attachment`
  - `already_granted_attachment`
- Emits: `ReleaseReplayed`
- To: `RevocationPendingAttached`

### `ReleaseAttachmentMismatchRevocationPendingAttached`
- From: `RevocationPendingAttached`
- On: `Release`(attachment_id)
- Guards:
  - `different_attachment`
  - `unknown_attachment`
- Emits: `ReleaseRejected`
- To: `RevocationPendingAttached`

### `ReleaseDuplicateWhileAttachedExpiryPendingAttached`
- From: `ExpiryPendingAttached`
- On: `Release`(attachment_id)
- Guards:
  - `different_attachment`
  - `already_granted_attachment`
- Emits: `ReleaseReplayed`
- To: `ExpiryPendingAttached`

### `ReleaseAttachmentMismatchExpiryPendingAttached`
- From: `ExpiryPendingAttached`
- On: `Release`(attachment_id)
- Guards:
  - `different_attachment`
  - `unknown_attachment`
- Emits: `ReleaseRejected`
- To: `ExpiryPendingAttached`

### `ReleaseDuplicateConvergesActive`
- From: `Active`
- On: `Release`(attachment_id)
- Guards:
  - `already_granted_attachment`
- Emits: `ReleaseReplayed`
- To: `Active`

### `ReleaseUnknownAttachmentActive`
- From: `Active`
- On: `Release`(attachment_id)
- Guards:
  - `unknown_attachment`
- Emits: `ReleaseRejected`
- To: `Active`

### `ReleaseDuplicateConvergesRevoked`
- From: `Revoked`
- On: `Release`(attachment_id)
- Guards:
  - `already_granted_attachment`
- Emits: `ReleaseReplayed`
- To: `Revoked`

### `ReleaseUnknownAttachmentRevoked`
- From: `Revoked`
- On: `Release`(attachment_id)
- Guards:
  - `unknown_attachment`
- Emits: `ReleaseRejected`
- To: `Revoked`

### `ReleaseDuplicateConvergesExpired`
- From: `Expired`
- On: `Release`(attachment_id)
- Guards:
  - `already_granted_attachment`
- Emits: `ReleaseReplayed`
- To: `Expired`

### `ReleaseUnknownAttachmentExpired`
- From: `Expired`
- On: `Release`(attachment_id)
- Guards:
  - `unknown_attachment`
- Emits: `ReleaseRejected`
- To: `Expired`

### `ReleaseDuplicateConvergesExhausted`
- From: `Exhausted`
- On: `Release`(attachment_id)
- Guards:
  - `already_granted_attachment`
- Emits: `ReleaseReplayed`
- To: `Exhausted`

### `ReleaseUnknownAttachmentExhausted`
- From: `Exhausted`
- On: `Release`(attachment_id)
- Guards:
  - `unknown_attachment`
- Emits: `ReleaseRejected`
- To: `Exhausted`

### `ReleaseUnknownAttachmentEmpty`
- From: `Empty`
- On: `Release`(attachment_id)
- Emits: `ReleaseRejected`
- To: `Empty`

### `ReleaseUnknownAttachmentReserved`
- From: `Reserved`
- On: `Release`(attachment_id)
- Emits: `ReleaseRejected`
- To: `Reserved`

### `ReleaseUnknownAttachmentActivationFailed`
- From: `ActivationFailed`
- On: `Release`(attachment_id)
- Emits: `ReleaseRejected`
- To: `ActivationFailed`

### `RevokeAuthenticationInvalidEmpty`
- From: `Empty`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_invalid`
- Emits: `RevocationDenied`
- To: `Empty`

### `RevokeAuthenticationInvalidReserved`
- From: `Reserved`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_invalid`
- Emits: `RevocationDenied`
- To: `Reserved`

### `RevokeAuthenticationInvalidActivationFailed`
- From: `ActivationFailed`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_invalid`
- Emits: `RevocationDenied`
- To: `ActivationFailed`

### `RevokeAuthenticationInvalidActive`
- From: `Active`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_invalid`
- Emits: `RevocationDenied`
- To: `Active`

### `RevokeAuthenticationInvalidAttached`
- From: `Attached`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_invalid`
- Emits: `RevocationDenied`
- To: `Attached`

### `RevokeAuthenticationInvalidRevocationPendingAttached`
- From: `RevocationPendingAttached`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_invalid`
- Emits: `RevocationDenied`
- To: `RevocationPendingAttached`

### `RevokeAuthenticationInvalidExpiryPendingAttached`
- From: `ExpiryPendingAttached`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_invalid`
- Emits: `RevocationDenied`
- To: `ExpiryPendingAttached`

### `RevokeAuthenticationInvalidRevoked`
- From: `Revoked`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_invalid`
- Emits: `RevocationDenied`
- To: `Revoked`

### `RevokeAuthenticationInvalidExpired`
- From: `Expired`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_invalid`
- Emits: `RevocationDenied`
- To: `Expired`

### `RevokeAuthenticationInvalidExhausted`
- From: `Exhausted`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_invalid`
- Emits: `RevocationDenied`
- To: `Exhausted`

### `RevokeEmpty`
- From: `Empty`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_valid`
- Emits: `RevocationDenied`
- To: `Empty`

### `RevokeReserved`
- From: `Reserved`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_valid`
- Emits: `CapabilityRevoked`
- To: `Revoked`

### `RevokeActivationFailed`
- From: `ActivationFailed`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_valid`
- Emits: `CapabilityRevoked`
- To: `Revoked`

### `RevokeActive`
- From: `Active`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_valid`
- Emits: `CapabilityRevoked`
- To: `Revoked`

### `RevokeAttached`
- From: `Attached`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_valid`
- Emits: `RevocationPendingRecorded`
- To: `RevocationPendingAttached`

### `RevokeExpiryPendingAttached`
- From: `ExpiryPendingAttached`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_valid`
- Emits: `RevocationPendingRecorded`
- To: `RevocationPendingAttached`

### `RevokeRevocationPendingReplay`
- From: `RevocationPendingAttached`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_valid`
- Emits: `RevocationConverged`
- To: `RevocationPendingAttached`

### `RevokeRevokedReplay`
- From: `Revoked`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_valid`
- Emits: `RevocationConverged`
- To: `Revoked`

### `RevokeAlreadyTerminalExpired`
- From: `Expired`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_valid`
- Emits: `RevocationDenied`
- To: `Expired`

### `RevokeAlreadyTerminalExhausted`
- From: `Exhausted`
- On: `Revoke`(authentication_valid)
- Guards:
  - `authentication_valid`
- Emits: `RevocationDenied`
- To: `Exhausted`

### `ExpiryNotObservedEmpty`
- From: `Empty`
- On: `ObserveExpiry`(expired)
- Guards:
  - `not_expired`
- Emits: `ExpiryObservationIgnored`
- To: `Empty`

### `ExpiryNotObservedReserved`
- From: `Reserved`
- On: `ObserveExpiry`(expired)
- Guards:
  - `not_expired`
- Emits: `ExpiryObservationIgnored`
- To: `Reserved`

### `ExpiryNotObservedActivationFailed`
- From: `ActivationFailed`
- On: `ObserveExpiry`(expired)
- Guards:
  - `not_expired`
- Emits: `ExpiryObservationIgnored`
- To: `ActivationFailed`

### `ExpiryNotObservedActive`
- From: `Active`
- On: `ObserveExpiry`(expired)
- Guards:
  - `not_expired`
- Emits: `ExpiryObservationIgnored`
- To: `Active`

### `ExpiryNotObservedAttached`
- From: `Attached`
- On: `ObserveExpiry`(expired)
- Guards:
  - `not_expired`
- Emits: `ExpiryObservationIgnored`
- To: `Attached`

### `ExpiryNotObservedRevocationPendingAttached`
- From: `RevocationPendingAttached`
- On: `ObserveExpiry`(expired)
- Guards:
  - `not_expired`
- Emits: `ExpiryObservationIgnored`
- To: `RevocationPendingAttached`

### `ExpiryNotObservedExpiryPendingAttached`
- From: `ExpiryPendingAttached`
- On: `ObserveExpiry`(expired)
- Guards:
  - `not_expired`
- Emits: `ExpiryObservationIgnored`
- To: `ExpiryPendingAttached`

### `ExpiryNotObservedRevoked`
- From: `Revoked`
- On: `ObserveExpiry`(expired)
- Guards:
  - `not_expired`
- Emits: `ExpiryObservationIgnored`
- To: `Revoked`

### `ExpiryNotObservedExpired`
- From: `Expired`
- On: `ObserveExpiry`(expired)
- Guards:
  - `not_expired`
- Emits: `ExpiryObservationIgnored`
- To: `Expired`

### `ExpiryNotObservedExhausted`
- From: `Exhausted`
- On: `ObserveExpiry`(expired)
- Guards:
  - `not_expired`
- Emits: `ExpiryObservationIgnored`
- To: `Exhausted`

### `ExpiryObservedEmpty`
- From: `Empty`
- On: `ObserveExpiry`(expired)
- Guards:
  - `expired`
- Emits: `ExpiryObservationIgnored`
- To: `Empty`

### `ExpireReserved`
- From: `Reserved`
- On: `ObserveExpiry`(expired)
- Guards:
  - `expired`
- Emits: `CapabilityExpired`
- To: `Expired`

### `ExpireActivationFailed`
- From: `ActivationFailed`
- On: `ObserveExpiry`(expired)
- Guards:
  - `expired`
- Emits: `CapabilityExpired`
- To: `Expired`

### `ExpireActive`
- From: `Active`
- On: `ObserveExpiry`(expired)
- Guards:
  - `expired`
- Emits: `CapabilityExpired`
- To: `Expired`

### `ExpireAttached`
- From: `Attached`
- On: `ObserveExpiry`(expired)
- Guards:
  - `expired`
- Emits: `ExpiryPendingRecorded`
- To: `ExpiryPendingAttached`

### `ExpiryPendingReplay`
- From: `ExpiryPendingAttached`
- On: `ObserveExpiry`(expired)
- Guards:
  - `expired`
- Emits: `ExpiryObservationIgnored`
- To: `ExpiryPendingAttached`

### `ExpiryUnderRevocationPending`
- From: `RevocationPendingAttached`
- On: `ObserveExpiry`(expired)
- Guards:
  - `expired`
- Emits: `ExpiryObservationIgnored`
- To: `RevocationPendingAttached`

### `ExpiryAfterTerminalRevoked`
- From: `Revoked`
- On: `ObserveExpiry`(expired)
- Guards:
  - `expired`
- Emits: `ExpiryObservationIgnored`
- To: `Revoked`

### `ExpiryAfterTerminalExpired`
- From: `Expired`
- On: `ObserveExpiry`(expired)
- Guards:
  - `expired`
- Emits: `ExpiryObservationIgnored`
- To: `Expired`

### `ExpiryAfterTerminalExhausted`
- From: `Exhausted`
- On: `ObserveExpiry`(expired)
- Guards:
  - `expired`
- Emits: `ExpiryObservationIgnored`
- To: `Exhausted`

### `CompleteCleanupPendingDebtRevoked`
- From: `Revoked`
- On: `CompleteCleanup`()
- Guards:
  - `cleanup_debt_pending`
- Emits: `CleanupCompleted`
- To: `Revoked`

### `CompleteCleanupReplayRevoked`
- From: `Revoked`
- On: `CompleteCleanup`()
- Guards:
  - `cleanup_already_complete`
- Emits: `CleanupCompletionReplayed`
- To: `Revoked`

### `CompleteCleanupWithoutDebtRevoked`
- From: `Revoked`
- On: `CompleteCleanup`()
- Guards:
  - `no_cleanup_debt`
- Emits: `CleanupCompletionRejected`
- To: `Revoked`

### `CompleteCleanupPendingDebtExpired`
- From: `Expired`
- On: `CompleteCleanup`()
- Guards:
  - `cleanup_debt_pending`
- Emits: `CleanupCompleted`
- To: `Expired`

### `CompleteCleanupReplayExpired`
- From: `Expired`
- On: `CompleteCleanup`()
- Guards:
  - `cleanup_already_complete`
- Emits: `CleanupCompletionReplayed`
- To: `Expired`

### `CompleteCleanupWithoutDebtExpired`
- From: `Expired`
- On: `CompleteCleanup`()
- Guards:
  - `no_cleanup_debt`
- Emits: `CleanupCompletionRejected`
- To: `Expired`

### `CompleteCleanupPendingDebtExhausted`
- From: `Exhausted`
- On: `CompleteCleanup`()
- Guards:
  - `cleanup_debt_pending`
- Emits: `CleanupCompleted`
- To: `Exhausted`

### `CompleteCleanupReplayExhausted`
- From: `Exhausted`
- On: `CompleteCleanup`()
- Guards:
  - `cleanup_already_complete`
- Emits: `CleanupCompletionReplayed`
- To: `Exhausted`

### `CompleteCleanupWithoutDebtExhausted`
- From: `Exhausted`
- On: `CompleteCleanup`()
- Guards:
  - `no_cleanup_debt`
- Emits: `CleanupCompletionRejected`
- To: `Exhausted`

### `CompleteCleanupWhileAttachedAttached`
- From: `Attached`
- On: `CompleteCleanup`()
- Emits: `CleanupCompletionRejected`
- To: `Attached`

### `CompleteCleanupWhileAttachedRevocationPendingAttached`
- From: `RevocationPendingAttached`
- On: `CompleteCleanup`()
- Emits: `CleanupCompletionRejected`
- To: `RevocationPendingAttached`

### `CompleteCleanupWhileAttachedExpiryPendingAttached`
- From: `ExpiryPendingAttached`
- On: `CompleteCleanup`()
- Emits: `CleanupCompletionRejected`
- To: `ExpiryPendingAttached`

### `CompleteCleanupNotTerminalEmpty`
- From: `Empty`
- On: `CompleteCleanup`()
- Emits: `CleanupCompletionRejected`
- To: `Empty`

### `CompleteCleanupNotTerminalReserved`
- From: `Reserved`
- On: `CompleteCleanup`()
- Emits: `CleanupCompletionRejected`
- To: `Reserved`

### `CompleteCleanupNotTerminalActivationFailed`
- From: `ActivationFailed`
- On: `CompleteCleanup`()
- Emits: `CleanupCompletionRejected`
- To: `ActivationFailed`

### `CompleteCleanupNotTerminalActive`
- From: `Active`
- On: `CompleteCleanup`()
- Emits: `CleanupCompletionRejected`
- To: `Active`

## Coverage
### Code Anchors
- `forked_participant_lifecycle` (machine `ForkedParticipantLifecycleMachine`): `meerkat-mob/src/machines/forked_participant_lifecycle.rs` — ForkedParticipantLifecycleMachine owns one source-owned capability record: reservation identity, durable fork activation identity, bounded single-holder attachment admission, revocation, expiry, and cleanup debt

### Scenarios
- `forked_participant_reservation_and_activation_identity` — A reservation binds one request fingerprint and a positive reuse budget, exact reserve replay converges, a conflicting fingerprint is a typed reject, and a create-failure keeps the SAME request retryable without letting a different request steal the identity
- `forked_participant_bounded_single_holder_attachment` — An active capability admits one attachment at a time and increments the use count exactly once, exact attach replay returns the original grant without a second increment, a concurrent attachment is typed busy, an invalid authentication observation changes nothing, and the bounded reuse budget terminalizes to Exhausted on the release that spends it
- `forked_participant_revocation_expiry_and_cleanup_debt` — Revocation and expiry terminalize a detached capability with cleanup debt, an attached capability instead records a typed pending-attached state whose debt becomes actionable only after the exact release, both observations converge on replay, and CompleteCleanup is admitted only for a terminal detached record that carries debt
