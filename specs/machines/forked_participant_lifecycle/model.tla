---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for ForkedParticipantLifecycleMachine.

CONSTANTS BooleanValues, ForkedParticipantActivationRejectionValues, ForkedParticipantAttachDenialValues, ForkedParticipantCleanupRejectionValues, ForkedParticipantCleanupStateValues, ForkedParticipantExpiryIgnoreValues, ForkedParticipantReleaseRejectionValues, ForkedParticipantReservationRejectionValues, ForkedParticipantRevocationDenialValues, NatValues, SetOfStringValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionStringValues == {None} \cup {Some(x) : x \in StringValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapIncrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) + amount ELSE map[x]]
MapDecrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) - amount ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
Count(seq, value) == Cardinality({i \in DOMAIN seq : seq[i] = value})
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, request_fingerprint, max_uses, use_count, fork_activation_id, active_attachment_id, granted_attachment_ids, cleanup_state

vars == << phase, model_step_count, request_fingerprint, max_uses, use_count, fork_activation_id, active_attachment_id, granted_attachment_ids, cleanup_state >>

Init ==
    /\ phase = "Empty"
    /\ model_step_count = 0
    /\ request_fingerprint = ""
    /\ max_uses = 0
    /\ use_count = 0
    /\ fork_activation_id = ""
    /\ active_attachment_id = None
    /\ granted_attachment_ids = {}
    /\ cleanup_state = "NotRequired"

TerminalStutter ==
    /\ phase = "Revoked" \/ phase = "Expired" \/ phase = "Exhausted"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_2c7acb748b58d762 == UNCHANGED << use_count, fork_activation_id, active_attachment_id, granted_attachment_ids, cleanup_state >>
UnchangedFrame_414bb6b102ecbb41 == UNCHANGED << request_fingerprint, max_uses, use_count, fork_activation_id, active_attachment_id, granted_attachment_ids, cleanup_state >>
UnchangedFrame_49704fc2390ccded == UNCHANGED << request_fingerprint, max_uses, use_count, fork_activation_id, active_attachment_id, granted_attachment_ids >>
UnchangedFrame_895c83351d72639f == UNCHANGED << request_fingerprint, max_uses, use_count, fork_activation_id, granted_attachment_ids >>
UnchangedFrame_ac1c100999575497 == UNCHANGED << request_fingerprint, max_uses, use_count, fork_activation_id, granted_attachment_ids, cleanup_state >>
UnchangedFrame_bd4ad4df278b4b06 == UNCHANGED << request_fingerprint, max_uses, use_count, active_attachment_id, granted_attachment_ids, cleanup_state >>
UnchangedFrame_e34c5fec435b523e == UNCHANGED << request_fingerprint, max_uses, fork_activation_id, cleanup_state >>

ReserveEmpty(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "Empty"
    /\ ((arg_request_fingerprint # "") /\ (arg_max_uses > 0))
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ request_fingerprint' = arg_request_fingerprint
    /\ max_uses' = arg_max_uses
    /\ UnchangedFrame_2c7acb748b58d762


ReserveEmptyMalformed(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "Empty"
    /\ (IF (arg_request_fingerprint = "") THEN TRUE ELSE (arg_max_uses = 0))
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReserveReservedReplay(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "Reserved"
    /\ ((arg_request_fingerprint = request_fingerprint) /\ (arg_max_uses = max_uses))
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReserveReservedConflict(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "Reserved"
    /\ (IF (arg_request_fingerprint # request_fingerprint) THEN TRUE ELSE (arg_max_uses # max_uses))
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReserveActivationFailedRetry(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "ActivationFailed"
    /\ ((arg_request_fingerprint = request_fingerprint) /\ (arg_max_uses = max_uses))
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReserveActivationFailedConflict(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "ActivationFailed"
    /\ (IF (arg_request_fingerprint # request_fingerprint) THEN TRUE ELSE (arg_max_uses # max_uses))
    /\ phase' = "ActivationFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReserveAlreadyProvisionedActive(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReserveAlreadyProvisionedAttached(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReserveAlreadyProvisionedRevocationPendingAttached(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "RevocationPendingAttached"
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReserveAlreadyProvisionedExpiryPendingAttached(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "ExpiryPendingAttached"
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReserveAlreadyProvisionedRevoked(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "Revoked"
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReserveAlreadyProvisionedExpired(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReserveAlreadyProvisionedExhausted(arg_request_fingerprint, arg_max_uses) ==
    /\ phase = "Exhausted"
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateEmpty(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Empty"
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateReserved(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Reserved"
    /\ (arg_request_fingerprint = request_fingerprint)
    /\ (arg_fork_activation_id # "")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ fork_activation_id' = arg_fork_activation_id
    /\ UnchangedFrame_bd4ad4df278b4b06


ActivateReservedMismatch(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Reserved"
    /\ (arg_request_fingerprint # request_fingerprint)
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateReservedMalformed(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Reserved"
    /\ (arg_request_fingerprint = request_fingerprint)
    /\ (arg_fork_activation_id = "")
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateActivationFailedRecovery(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "ActivationFailed"
    /\ (arg_request_fingerprint = request_fingerprint)
    /\ (arg_fork_activation_id # "")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ fork_activation_id' = arg_fork_activation_id
    /\ UnchangedFrame_bd4ad4df278b4b06


ActivateActivationFailedMismatch(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "ActivationFailed"
    /\ (arg_request_fingerprint # request_fingerprint)
    /\ phase' = "ActivationFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateActivationFailedMalformed(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "ActivationFailed"
    /\ (arg_request_fingerprint = request_fingerprint)
    /\ (arg_fork_activation_id = "")
    /\ phase' = "ActivationFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateActiveReplay(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Active"
    /\ ((arg_request_fingerprint = request_fingerprint) /\ (arg_fork_activation_id = fork_activation_id))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateActiveConflict(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Active"
    /\ (IF (arg_request_fingerprint # request_fingerprint) THEN TRUE ELSE (arg_fork_activation_id # fork_activation_id))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateReplayAfterActivationAttached(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Attached"
    /\ ((arg_request_fingerprint = request_fingerprint) /\ (arg_fork_activation_id = fork_activation_id) /\ (fork_activation_id # ""))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateReplayAfterActivationRevocationPendingAttached(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "RevocationPendingAttached"
    /\ ((arg_request_fingerprint = request_fingerprint) /\ (arg_fork_activation_id = fork_activation_id) /\ (fork_activation_id # ""))
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateReplayAfterActivationExpiryPendingAttached(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "ExpiryPendingAttached"
    /\ ((arg_request_fingerprint = request_fingerprint) /\ (arg_fork_activation_id = fork_activation_id) /\ (fork_activation_id # ""))
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateReplayAfterActivationRevoked(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Revoked"
    /\ ((arg_request_fingerprint = request_fingerprint) /\ (arg_fork_activation_id = fork_activation_id) /\ (fork_activation_id # ""))
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateReplayAfterActivationExpired(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Expired"
    /\ ((arg_request_fingerprint = request_fingerprint) /\ (arg_fork_activation_id = fork_activation_id) /\ (fork_activation_id # ""))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateReplayAfterActivationExhausted(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Exhausted"
    /\ ((arg_request_fingerprint = request_fingerprint) /\ (arg_fork_activation_id = fork_activation_id) /\ (fork_activation_id # ""))
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateAttachedConflictAttached(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Attached"
    /\ (IF (arg_request_fingerprint # request_fingerprint) THEN TRUE ELSE (IF (arg_fork_activation_id # fork_activation_id) THEN TRUE ELSE (fork_activation_id = "")))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateAttachedConflictRevocationPendingAttached(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "RevocationPendingAttached"
    /\ (IF (arg_request_fingerprint # request_fingerprint) THEN TRUE ELSE (IF (arg_fork_activation_id # fork_activation_id) THEN TRUE ELSE (fork_activation_id = "")))
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateAttachedConflictExpiryPendingAttached(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "ExpiryPendingAttached"
    /\ (IF (arg_request_fingerprint # request_fingerprint) THEN TRUE ELSE (IF (arg_fork_activation_id # fork_activation_id) THEN TRUE ELSE (fork_activation_id = "")))
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateTerminalConflictRevoked(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Revoked"
    /\ (IF (arg_request_fingerprint # request_fingerprint) THEN TRUE ELSE (IF (arg_fork_activation_id # fork_activation_id) THEN TRUE ELSE (fork_activation_id = "")))
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateTerminalConflictExpired(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Expired"
    /\ (IF (arg_request_fingerprint # request_fingerprint) THEN TRUE ELSE (IF (arg_fork_activation_id # fork_activation_id) THEN TRUE ELSE (fork_activation_id = "")))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ActivateTerminalConflictExhausted(arg_request_fingerprint, arg_fork_activation_id) ==
    /\ phase = "Exhausted"
    /\ (IF (arg_request_fingerprint # request_fingerprint) THEN TRUE ELSE (IF (arg_fork_activation_id # fork_activation_id) THEN TRUE ELSE (fork_activation_id = "")))
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


FailActivationReserved(arg_request_fingerprint) ==
    /\ phase = "Reserved"
    /\ (arg_request_fingerprint = request_fingerprint)
    /\ phase' = "ActivationFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


FailActivationReservedMismatch(arg_request_fingerprint) ==
    /\ phase = "Reserved"
    /\ (arg_request_fingerprint # request_fingerprint)
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


FailActivationReplay(arg_request_fingerprint) ==
    /\ phase = "ActivationFailed"
    /\ (arg_request_fingerprint = request_fingerprint)
    /\ phase' = "ActivationFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


FailActivationFailedMismatch(arg_request_fingerprint) ==
    /\ phase = "ActivationFailed"
    /\ (arg_request_fingerprint # request_fingerprint)
    /\ phase' = "ActivationFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


FailActivationAfterActivationActive(arg_request_fingerprint) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


FailActivationAfterActivationAttached(arg_request_fingerprint) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


FailActivationAfterActivationRevocationPendingAttached(arg_request_fingerprint) ==
    /\ phase = "RevocationPendingAttached"
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


FailActivationAfterActivationExpiryPendingAttached(arg_request_fingerprint) ==
    /\ phase = "ExpiryPendingAttached"
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


FailActivationAfterActivationExhausted(arg_request_fingerprint) ==
    /\ phase = "Exhausted"
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


FailActivationNotReservedEmpty(arg_request_fingerprint) ==
    /\ phase = "Empty"
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


FailActivationNotReservedRevoked(arg_request_fingerprint) ==
    /\ phase = "Revoked"
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


FailActivationNotReservedExpired(arg_request_fingerprint) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAuthenticationInvalidEmpty(attachment_id, authentication_valid, expired) ==
    /\ phase = "Empty"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAuthenticationInvalidReserved(attachment_id, authentication_valid, expired) ==
    /\ phase = "Reserved"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAuthenticationInvalidActivationFailed(attachment_id, authentication_valid, expired) ==
    /\ phase = "ActivationFailed"
    /\ (authentication_valid = FALSE)
    /\ phase' = "ActivationFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAuthenticationInvalidActive(attachment_id, authentication_valid, expired) ==
    /\ phase = "Active"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAuthenticationInvalidAttached(attachment_id, authentication_valid, expired) ==
    /\ phase = "Attached"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAuthenticationInvalidRevocationPendingAttached(attachment_id, authentication_valid, expired) ==
    /\ phase = "RevocationPendingAttached"
    /\ (authentication_valid = FALSE)
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAuthenticationInvalidExpiryPendingAttached(attachment_id, authentication_valid, expired) ==
    /\ phase = "ExpiryPendingAttached"
    /\ (authentication_valid = FALSE)
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAuthenticationInvalidRevoked(attachment_id, authentication_valid, expired) ==
    /\ phase = "Revoked"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAuthenticationInvalidExpired(attachment_id, authentication_valid, expired) ==
    /\ phase = "Expired"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAuthenticationInvalidExhausted(attachment_id, authentication_valid, expired) ==
    /\ phase = "Exhausted"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachNotActiveEmpty(attachment_id, authentication_valid, expired) ==
    /\ phase = "Empty"
    /\ (authentication_valid = TRUE)
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachNotActiveReserved(attachment_id, authentication_valid, expired) ==
    /\ phase = "Reserved"
    /\ (authentication_valid = TRUE)
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachNotActiveActivationFailed(attachment_id, authentication_valid, expired) ==
    /\ phase = "ActivationFailed"
    /\ (authentication_valid = TRUE)
    /\ phase' = "ActivationFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachActiveMalformed(attachment_id, authentication_valid, expired) ==
    /\ phase = "Active"
    /\ (authentication_valid = TRUE)
    /\ (attachment_id = "")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachActiveExpired(attachment_id, authentication_valid, expired) ==
    /\ phase = "Active"
    /\ (authentication_valid = TRUE)
    /\ (attachment_id # "")
    /\ (expired = TRUE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_state' = "Pending"
    /\ UnchangedFrame_49704fc2390ccded


AttachActiveAlreadyReleased(attachment_id, authentication_valid, expired) ==
    /\ phase = "Active"
    /\ (authentication_valid = TRUE)
    /\ (attachment_id # "")
    /\ (expired = FALSE)
    /\ (attachment_id \in granted_attachment_ids)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachActiveGrant(attachment_id, authentication_valid, expired) ==
    /\ phase = "Active"
    /\ (authentication_valid = TRUE)
    /\ (attachment_id # "")
    /\ (expired = FALSE)
    /\ ((attachment_id \in granted_attachment_ids) = FALSE)
    /\ (use_count < max_uses)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ use_count' = (use_count) + 1
    /\ active_attachment_id' = Some(attachment_id)
    /\ granted_attachment_ids' = (granted_attachment_ids \cup {attachment_id})
    /\ UnchangedFrame_e34c5fec435b523e


AttachActiveBudgetSpent(attachment_id, authentication_valid, expired) ==
    /\ phase = "Active"
    /\ (authentication_valid = TRUE)
    /\ (attachment_id # "")
    /\ (expired = FALSE)
    /\ ((attachment_id \in granted_attachment_ids) = FALSE)
    /\ (use_count >= max_uses)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAttachedReplay(attachment_id, authentication_valid, expired) ==
    /\ phase = "Attached"
    /\ (authentication_valid = TRUE)
    /\ (active_attachment_id = Some(attachment_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAttachedAlreadyReleased(attachment_id, authentication_valid, expired) ==
    /\ phase = "Attached"
    /\ (authentication_valid = TRUE)
    /\ (active_attachment_id # Some(attachment_id))
    /\ (attachment_id \in granted_attachment_ids)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachAttachedBusy(attachment_id, authentication_valid, expired) ==
    /\ phase = "Attached"
    /\ (authentication_valid = TRUE)
    /\ (active_attachment_id # Some(attachment_id))
    /\ ((attachment_id \in granted_attachment_ids) = FALSE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachRevokedCapabilityRevocationPendingAttached(attachment_id, authentication_valid, expired) ==
    /\ phase = "RevocationPendingAttached"
    /\ (authentication_valid = TRUE)
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachRevokedCapabilityRevoked(attachment_id, authentication_valid, expired) ==
    /\ phase = "Revoked"
    /\ (authentication_valid = TRUE)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachExpiredCapabilityExpiryPendingAttached(attachment_id, authentication_valid, expired) ==
    /\ phase = "ExpiryPendingAttached"
    /\ (authentication_valid = TRUE)
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachExpiredCapabilityExpired(attachment_id, authentication_valid, expired) ==
    /\ phase = "Expired"
    /\ (authentication_valid = TRUE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


AttachExhaustedCapabilityExhausted(attachment_id, authentication_valid, expired) ==
    /\ phase = "Exhausted"
    /\ (authentication_valid = TRUE)
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseAttachedWithBudgetLeft(attachment_id) ==
    /\ phase = "Attached"
    /\ (active_attachment_id = Some(attachment_id))
    /\ (use_count < max_uses)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ active_attachment_id' = None
    /\ UnchangedFrame_ac1c100999575497


ReleaseAttachedExhausts(attachment_id) ==
    /\ phase = "Attached"
    /\ (active_attachment_id = Some(attachment_id))
    /\ (use_count >= max_uses)
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ active_attachment_id' = None
    /\ cleanup_state' = "Pending"
    /\ UnchangedFrame_895c83351d72639f


ReleaseRevocationPending(attachment_id) ==
    /\ phase = "RevocationPendingAttached"
    /\ (active_attachment_id = Some(attachment_id))
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ active_attachment_id' = None
    /\ cleanup_state' = "Pending"
    /\ UnchangedFrame_895c83351d72639f


ReleaseExpiryPending(attachment_id) ==
    /\ phase = "ExpiryPendingAttached"
    /\ (active_attachment_id = Some(attachment_id))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ active_attachment_id' = None
    /\ cleanup_state' = "Pending"
    /\ UnchangedFrame_895c83351d72639f


ReleaseDuplicateWhileAttachedAttached(attachment_id) ==
    /\ phase = "Attached"
    /\ (active_attachment_id # Some(attachment_id))
    /\ (attachment_id \in granted_attachment_ids)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseAttachmentMismatchAttached(attachment_id) ==
    /\ phase = "Attached"
    /\ (active_attachment_id # Some(attachment_id))
    /\ ((attachment_id \in granted_attachment_ids) = FALSE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseDuplicateWhileAttachedRevocationPendingAttached(attachment_id) ==
    /\ phase = "RevocationPendingAttached"
    /\ (active_attachment_id # Some(attachment_id))
    /\ (attachment_id \in granted_attachment_ids)
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseAttachmentMismatchRevocationPendingAttached(attachment_id) ==
    /\ phase = "RevocationPendingAttached"
    /\ (active_attachment_id # Some(attachment_id))
    /\ ((attachment_id \in granted_attachment_ids) = FALSE)
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseDuplicateWhileAttachedExpiryPendingAttached(attachment_id) ==
    /\ phase = "ExpiryPendingAttached"
    /\ (active_attachment_id # Some(attachment_id))
    /\ (attachment_id \in granted_attachment_ids)
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseAttachmentMismatchExpiryPendingAttached(attachment_id) ==
    /\ phase = "ExpiryPendingAttached"
    /\ (active_attachment_id # Some(attachment_id))
    /\ ((attachment_id \in granted_attachment_ids) = FALSE)
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseDuplicateConvergesActive(attachment_id) ==
    /\ phase = "Active"
    /\ (attachment_id \in granted_attachment_ids)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseUnknownAttachmentActive(attachment_id) ==
    /\ phase = "Active"
    /\ ((attachment_id \in granted_attachment_ids) = FALSE)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseDuplicateConvergesRevoked(attachment_id) ==
    /\ phase = "Revoked"
    /\ (attachment_id \in granted_attachment_ids)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseUnknownAttachmentRevoked(attachment_id) ==
    /\ phase = "Revoked"
    /\ ((attachment_id \in granted_attachment_ids) = FALSE)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseDuplicateConvergesExpired(attachment_id) ==
    /\ phase = "Expired"
    /\ (attachment_id \in granted_attachment_ids)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseUnknownAttachmentExpired(attachment_id) ==
    /\ phase = "Expired"
    /\ ((attachment_id \in granted_attachment_ids) = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseDuplicateConvergesExhausted(attachment_id) ==
    /\ phase = "Exhausted"
    /\ (attachment_id \in granted_attachment_ids)
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseUnknownAttachmentExhausted(attachment_id) ==
    /\ phase = "Exhausted"
    /\ ((attachment_id \in granted_attachment_ids) = FALSE)
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseUnknownAttachmentEmpty(attachment_id) ==
    /\ phase = "Empty"
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseUnknownAttachmentReserved(attachment_id) ==
    /\ phase = "Reserved"
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ReleaseUnknownAttachmentActivationFailed(attachment_id) ==
    /\ phase = "ActivationFailed"
    /\ phase' = "ActivationFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeAuthenticationInvalidEmpty(authentication_valid) ==
    /\ phase = "Empty"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeAuthenticationInvalidReserved(authentication_valid) ==
    /\ phase = "Reserved"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeAuthenticationInvalidActivationFailed(authentication_valid) ==
    /\ phase = "ActivationFailed"
    /\ (authentication_valid = FALSE)
    /\ phase' = "ActivationFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeAuthenticationInvalidActive(authentication_valid) ==
    /\ phase = "Active"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeAuthenticationInvalidAttached(authentication_valid) ==
    /\ phase = "Attached"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeAuthenticationInvalidRevocationPendingAttached(authentication_valid) ==
    /\ phase = "RevocationPendingAttached"
    /\ (authentication_valid = FALSE)
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeAuthenticationInvalidExpiryPendingAttached(authentication_valid) ==
    /\ phase = "ExpiryPendingAttached"
    /\ (authentication_valid = FALSE)
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeAuthenticationInvalidRevoked(authentication_valid) ==
    /\ phase = "Revoked"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeAuthenticationInvalidExpired(authentication_valid) ==
    /\ phase = "Expired"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeAuthenticationInvalidExhausted(authentication_valid) ==
    /\ phase = "Exhausted"
    /\ (authentication_valid = FALSE)
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeEmpty(authentication_valid) ==
    /\ phase = "Empty"
    /\ (authentication_valid = TRUE)
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeReserved(authentication_valid) ==
    /\ phase = "Reserved"
    /\ (authentication_valid = TRUE)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeActivationFailed(authentication_valid) ==
    /\ phase = "ActivationFailed"
    /\ (authentication_valid = TRUE)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_state' = "Pending"
    /\ UnchangedFrame_49704fc2390ccded


RevokeActive(authentication_valid) ==
    /\ phase = "Active"
    /\ (authentication_valid = TRUE)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_state' = "Pending"
    /\ UnchangedFrame_49704fc2390ccded


RevokeAttached(authentication_valid) ==
    /\ phase = "Attached"
    /\ (authentication_valid = TRUE)
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_state' = "Deferred"
    /\ UnchangedFrame_49704fc2390ccded


RevokeExpiryPendingAttached(authentication_valid) ==
    /\ phase = "ExpiryPendingAttached"
    /\ (authentication_valid = TRUE)
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_state' = "Deferred"
    /\ UnchangedFrame_49704fc2390ccded


RevokeRevocationPendingReplay(authentication_valid) ==
    /\ phase = "RevocationPendingAttached"
    /\ (authentication_valid = TRUE)
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeRevokedReplay(authentication_valid) ==
    /\ phase = "Revoked"
    /\ (authentication_valid = TRUE)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeAlreadyTerminalExpired(authentication_valid) ==
    /\ phase = "Expired"
    /\ (authentication_valid = TRUE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


RevokeAlreadyTerminalExhausted(authentication_valid) ==
    /\ phase = "Exhausted"
    /\ (authentication_valid = TRUE)
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryNotObservedEmpty(expired) ==
    /\ phase = "Empty"
    /\ (expired = FALSE)
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryNotObservedReserved(expired) ==
    /\ phase = "Reserved"
    /\ (expired = FALSE)
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryNotObservedActivationFailed(expired) ==
    /\ phase = "ActivationFailed"
    /\ (expired = FALSE)
    /\ phase' = "ActivationFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryNotObservedActive(expired) ==
    /\ phase = "Active"
    /\ (expired = FALSE)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryNotObservedAttached(expired) ==
    /\ phase = "Attached"
    /\ (expired = FALSE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryNotObservedRevocationPendingAttached(expired) ==
    /\ phase = "RevocationPendingAttached"
    /\ (expired = FALSE)
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryNotObservedExpiryPendingAttached(expired) ==
    /\ phase = "ExpiryPendingAttached"
    /\ (expired = FALSE)
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryNotObservedRevoked(expired) ==
    /\ phase = "Revoked"
    /\ (expired = FALSE)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryNotObservedExpired(expired) ==
    /\ phase = "Expired"
    /\ (expired = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryNotObservedExhausted(expired) ==
    /\ phase = "Exhausted"
    /\ (expired = FALSE)
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryObservedEmpty(expired) ==
    /\ phase = "Empty"
    /\ (expired = TRUE)
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpireReserved(expired) ==
    /\ phase = "Reserved"
    /\ (expired = TRUE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpireActivationFailed(expired) ==
    /\ phase = "ActivationFailed"
    /\ (expired = TRUE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_state' = "Pending"
    /\ UnchangedFrame_49704fc2390ccded


ExpireActive(expired) ==
    /\ phase = "Active"
    /\ (expired = TRUE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_state' = "Pending"
    /\ UnchangedFrame_49704fc2390ccded


ExpireAttached(expired) ==
    /\ phase = "Attached"
    /\ (expired = TRUE)
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_state' = "Deferred"
    /\ UnchangedFrame_49704fc2390ccded


ExpiryPendingReplay(expired) ==
    /\ phase = "ExpiryPendingAttached"
    /\ (expired = TRUE)
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryUnderRevocationPending(expired) ==
    /\ phase = "RevocationPendingAttached"
    /\ (expired = TRUE)
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryAfterTerminalRevoked(expired) ==
    /\ phase = "Revoked"
    /\ (expired = TRUE)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryAfterTerminalExpired(expired) ==
    /\ phase = "Expired"
    /\ (expired = TRUE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


ExpiryAfterTerminalExhausted(expired) ==
    /\ phase = "Exhausted"
    /\ (expired = TRUE)
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupPendingDebtRevoked ==
    /\ phase = "Revoked"
    /\ (cleanup_state = "Pending")
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_state' = "Complete"
    /\ UnchangedFrame_49704fc2390ccded


CompleteCleanupReplayRevoked ==
    /\ phase = "Revoked"
    /\ (cleanup_state = "Complete")
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupWithoutDebtRevoked ==
    /\ phase = "Revoked"
    /\ ((cleanup_state # "Pending") /\ (cleanup_state # "Complete"))
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupPendingDebtExpired ==
    /\ phase = "Expired"
    /\ (cleanup_state = "Pending")
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_state' = "Complete"
    /\ UnchangedFrame_49704fc2390ccded


CompleteCleanupReplayExpired ==
    /\ phase = "Expired"
    /\ (cleanup_state = "Complete")
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupWithoutDebtExpired ==
    /\ phase = "Expired"
    /\ ((cleanup_state # "Pending") /\ (cleanup_state # "Complete"))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupPendingDebtExhausted ==
    /\ phase = "Exhausted"
    /\ (cleanup_state = "Pending")
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_state' = "Complete"
    /\ UnchangedFrame_49704fc2390ccded


CompleteCleanupReplayExhausted ==
    /\ phase = "Exhausted"
    /\ (cleanup_state = "Complete")
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupWithoutDebtExhausted ==
    /\ phase = "Exhausted"
    /\ ((cleanup_state # "Pending") /\ (cleanup_state # "Complete"))
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupWhileAttachedAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupWhileAttachedRevocationPendingAttached ==
    /\ phase = "RevocationPendingAttached"
    /\ phase' = "RevocationPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupWhileAttachedExpiryPendingAttached ==
    /\ phase = "ExpiryPendingAttached"
    /\ phase' = "ExpiryPendingAttached"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupNotTerminalEmpty ==
    /\ phase = "Empty"
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupNotTerminalReserved ==
    /\ phase = "Reserved"
    /\ phase' = "Reserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupNotTerminalActivationFailed ==
    /\ phase = "ActivationFailed"
    /\ phase' = "ActivationFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


CompleteCleanupNotTerminalActive ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_414bb6b102ecbb41


Next ==
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveEmpty(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveEmptyMalformed(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveReservedReplay(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveReservedConflict(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveActivationFailedRetry(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveActivationFailedConflict(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveAlreadyProvisionedActive(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveAlreadyProvisionedAttached(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveAlreadyProvisionedRevocationPendingAttached(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveAlreadyProvisionedExpiryPendingAttached(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveAlreadyProvisionedRevoked(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveAlreadyProvisionedExpired(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_max_uses \in 0..2 : ReserveAlreadyProvisionedExhausted(arg_request_fingerprint, arg_max_uses)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateEmpty(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateReserved(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateReservedMismatch(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateReservedMalformed(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateActivationFailedRecovery(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateActivationFailedMismatch(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateActivationFailedMalformed(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateActiveReplay(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateActiveConflict(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateReplayAfterActivationAttached(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateReplayAfterActivationRevocationPendingAttached(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateReplayAfterActivationExpiryPendingAttached(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateReplayAfterActivationRevoked(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateReplayAfterActivationExpired(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateReplayAfterActivationExhausted(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateAttachedConflictAttached(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateAttachedConflictRevocationPendingAttached(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateAttachedConflictExpiryPendingAttached(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateTerminalConflictRevoked(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateTerminalConflictExpired(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : \E arg_fork_activation_id \in StringValues : ActivateTerminalConflictExhausted(arg_request_fingerprint, arg_fork_activation_id)
    \/ \E arg_request_fingerprint \in StringValues : FailActivationReserved(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : FailActivationReservedMismatch(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : FailActivationReplay(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : FailActivationFailedMismatch(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : FailActivationAfterActivationActive(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : FailActivationAfterActivationAttached(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : FailActivationAfterActivationRevocationPendingAttached(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : FailActivationAfterActivationExpiryPendingAttached(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : FailActivationAfterActivationExhausted(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : FailActivationNotReservedEmpty(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : FailActivationNotReservedRevoked(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : FailActivationNotReservedExpired(arg_request_fingerprint)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAuthenticationInvalidEmpty(attachment_id, FALSE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAuthenticationInvalidReserved(attachment_id, FALSE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAuthenticationInvalidActivationFailed(attachment_id, FALSE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAuthenticationInvalidActive(attachment_id, FALSE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAuthenticationInvalidAttached(attachment_id, FALSE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAuthenticationInvalidRevocationPendingAttached(attachment_id, FALSE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAuthenticationInvalidExpiryPendingAttached(attachment_id, FALSE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAuthenticationInvalidRevoked(attachment_id, FALSE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAuthenticationInvalidExpired(attachment_id, FALSE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAuthenticationInvalidExhausted(attachment_id, FALSE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachNotActiveEmpty(attachment_id, TRUE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachNotActiveReserved(attachment_id, TRUE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachNotActiveActivationFailed(attachment_id, TRUE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachActiveMalformed(attachment_id, TRUE, expired)
    \/ \E attachment_id \in StringValues : AttachActiveExpired(attachment_id, TRUE, TRUE)
    \/ \E attachment_id \in StringValues : AttachActiveAlreadyReleased(attachment_id, TRUE, FALSE)
    \/ \E attachment_id \in StringValues : AttachActiveGrant(attachment_id, TRUE, FALSE)
    \/ \E attachment_id \in StringValues : AttachActiveBudgetSpent(attachment_id, TRUE, FALSE)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAttachedReplay(attachment_id, TRUE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAttachedAlreadyReleased(attachment_id, TRUE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachAttachedBusy(attachment_id, TRUE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachRevokedCapabilityRevocationPendingAttached(attachment_id, TRUE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachRevokedCapabilityRevoked(attachment_id, TRUE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachExpiredCapabilityExpiryPendingAttached(attachment_id, TRUE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachExpiredCapabilityExpired(attachment_id, TRUE, expired)
    \/ \E attachment_id \in StringValues : \E expired \in BOOLEAN : AttachExhaustedCapabilityExhausted(attachment_id, TRUE, expired)
    \/ \E attachment_id \in StringValues : ReleaseAttachedWithBudgetLeft(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseAttachedExhausts(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseRevocationPending(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseExpiryPending(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseDuplicateWhileAttachedAttached(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseAttachmentMismatchAttached(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseDuplicateWhileAttachedRevocationPendingAttached(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseAttachmentMismatchRevocationPendingAttached(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseDuplicateWhileAttachedExpiryPendingAttached(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseAttachmentMismatchExpiryPendingAttached(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseDuplicateConvergesActive(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseUnknownAttachmentActive(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseDuplicateConvergesRevoked(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseUnknownAttachmentRevoked(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseDuplicateConvergesExpired(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseUnknownAttachmentExpired(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseDuplicateConvergesExhausted(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseUnknownAttachmentExhausted(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseUnknownAttachmentEmpty(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseUnknownAttachmentReserved(attachment_id)
    \/ \E attachment_id \in StringValues : ReleaseUnknownAttachmentActivationFailed(attachment_id)
    \/ RevokeAuthenticationInvalidEmpty(FALSE)
    \/ RevokeAuthenticationInvalidReserved(FALSE)
    \/ RevokeAuthenticationInvalidActivationFailed(FALSE)
    \/ RevokeAuthenticationInvalidActive(FALSE)
    \/ RevokeAuthenticationInvalidAttached(FALSE)
    \/ RevokeAuthenticationInvalidRevocationPendingAttached(FALSE)
    \/ RevokeAuthenticationInvalidExpiryPendingAttached(FALSE)
    \/ RevokeAuthenticationInvalidRevoked(FALSE)
    \/ RevokeAuthenticationInvalidExpired(FALSE)
    \/ RevokeAuthenticationInvalidExhausted(FALSE)
    \/ RevokeEmpty(TRUE)
    \/ RevokeReserved(TRUE)
    \/ RevokeActivationFailed(TRUE)
    \/ RevokeActive(TRUE)
    \/ RevokeAttached(TRUE)
    \/ RevokeExpiryPendingAttached(TRUE)
    \/ RevokeRevocationPendingReplay(TRUE)
    \/ RevokeRevokedReplay(TRUE)
    \/ RevokeAlreadyTerminalExpired(TRUE)
    \/ RevokeAlreadyTerminalExhausted(TRUE)
    \/ ExpiryNotObservedEmpty(FALSE)
    \/ ExpiryNotObservedReserved(FALSE)
    \/ ExpiryNotObservedActivationFailed(FALSE)
    \/ ExpiryNotObservedActive(FALSE)
    \/ ExpiryNotObservedAttached(FALSE)
    \/ ExpiryNotObservedRevocationPendingAttached(FALSE)
    \/ ExpiryNotObservedExpiryPendingAttached(FALSE)
    \/ ExpiryNotObservedRevoked(FALSE)
    \/ ExpiryNotObservedExpired(FALSE)
    \/ ExpiryNotObservedExhausted(FALSE)
    \/ ExpiryObservedEmpty(TRUE)
    \/ ExpireReserved(TRUE)
    \/ ExpireActivationFailed(TRUE)
    \/ ExpireActive(TRUE)
    \/ ExpireAttached(TRUE)
    \/ ExpiryPendingReplay(TRUE)
    \/ ExpiryUnderRevocationPending(TRUE)
    \/ ExpiryAfterTerminalRevoked(TRUE)
    \/ ExpiryAfterTerminalExpired(TRUE)
    \/ ExpiryAfterTerminalExhausted(TRUE)
    \/ CompleteCleanupPendingDebtRevoked
    \/ CompleteCleanupReplayRevoked
    \/ CompleteCleanupWithoutDebtRevoked
    \/ CompleteCleanupPendingDebtExpired
    \/ CompleteCleanupReplayExpired
    \/ CompleteCleanupWithoutDebtExpired
    \/ CompleteCleanupPendingDebtExhausted
    \/ CompleteCleanupReplayExhausted
    \/ CompleteCleanupWithoutDebtExhausted
    \/ CompleteCleanupWhileAttachedAttached
    \/ CompleteCleanupWhileAttachedRevocationPendingAttached
    \/ CompleteCleanupWhileAttachedExpiryPendingAttached
    \/ CompleteCleanupNotTerminalEmpty
    \/ CompleteCleanupNotTerminalReserved
    \/ CompleteCleanupNotTerminalActivationFailed
    \/ CompleteCleanupNotTerminalActive
    \/ TerminalStutter

reserved_capability_has_positive_max_uses == (IF (phase = "Empty") THEN TRUE ELSE (max_uses > 0))
use_count_within_max_uses == (use_count <= max_uses)
granted_attachments_match_use_count == (Cardinality(granted_attachment_ids) = use_count)
active_holder_is_a_granted_attachment == (IF (active_attachment_id = None) THEN TRUE ELSE ((IF "value" \in DOMAIN active_attachment_id THEN active_attachment_id["value"] ELSE None) \in granted_attachment_ids))
attachment_only_while_attached == (IF (active_attachment_id = None) THEN TRUE ELSE (IF (phase = "Attached") THEN TRUE ELSE (IF (phase = "RevocationPendingAttached") THEN TRUE ELSE (phase = "ExpiryPendingAttached"))))
attached_phase_holds_one_attachment == (IF ((phase # "Attached") /\ (phase # "RevocationPendingAttached") /\ (phase # "ExpiryPendingAttached")) THEN TRUE ELSE (active_attachment_id # None))
terminal_capability_is_detached == (IF ((phase # "Revoked") /\ (phase # "Expired") /\ (phase # "Exhausted")) THEN TRUE ELSE (active_attachment_id = None))
cleanup_complete_requires_detached_terminal == (IF (cleanup_state # "Complete") THEN TRUE ELSE ((IF (phase = "Revoked") THEN TRUE ELSE (IF (phase = "Expired") THEN TRUE ELSE (phase = "Exhausted"))) /\ (active_attachment_id = None)))
deferred_cleanup_requires_attachment == (IF (cleanup_state # "Deferred") THEN TRUE ELSE (IF (phase = "RevocationPendingAttached") THEN TRUE ELSE (phase = "ExpiryPendingAttached")))
empty_record_has_no_capability_facts == (IF (phase # "Empty") THEN TRUE ELSE ((request_fingerprint = "") /\ (max_uses = 0) /\ (use_count = 0) /\ (fork_activation_id = "") /\ (active_attachment_id = None) /\ (Cardinality(granted_attachment_ids) = 0) /\ (cleanup_state = "NotRequired")))
pre_activation_record_has_no_grants == (IF ((phase # "Empty") /\ (phase # "Reserved") /\ (phase # "ActivationFailed")) THEN TRUE ELSE ((use_count = 0) /\ (Cardinality(granted_attachment_ids) = 0) /\ (active_attachment_id = None)))
usable_capability_has_fork_activation == (IF ((phase # "Active") /\ (phase # "Attached") /\ (phase # "RevocationPendingAttached") /\ (phase # "ExpiryPendingAttached") /\ (phase # "Exhausted")) THEN TRUE ELSE (fork_activation_id # ""))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(granted_attachment_ids) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(granted_attachment_ids) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []reserved_capability_has_positive_max_uses
THEOREM Spec => []use_count_within_max_uses
THEOREM Spec => []granted_attachments_match_use_count
THEOREM Spec => []active_holder_is_a_granted_attachment
THEOREM Spec => []attachment_only_while_attached
THEOREM Spec => []attached_phase_holds_one_attachment
THEOREM Spec => []terminal_capability_is_detached
THEOREM Spec => []cleanup_complete_requires_detached_terminal
THEOREM Spec => []deferred_cleanup_requires_attachment
THEOREM Spec => []empty_record_has_no_capability_facts
THEOREM Spec => []pre_activation_record_has_no_grants
THEOREM Spec => []usable_capability_has_fork_activation

=============================================================================
