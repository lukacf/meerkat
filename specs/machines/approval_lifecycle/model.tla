---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for ApprovalLifecycleMachine.

CONSTANTS ApprovalLifecycleDecisionValues, ApprovalLifecycleRejectionReasonValues, ApprovalLifecycleStatusValues, BooleanValues, SetOfStringValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapStringApprovalLifecycleStatusValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in ApprovalLifecycleStatusValues }
MapStringBoolValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in BOOLEAN }
OptionApprovalLifecycleDecisionValues == {None} \cup {Some(x) : x \in ApprovalLifecycleDecisionValues}

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

VARIABLES phase, model_step_count, approval_ids, approval_statuses, approval_approve_allowed, approval_deny_allowed, approval_has_expiry

vars == << phase, model_step_count, approval_ids, approval_statuses, approval_approve_allowed, approval_deny_allowed, approval_has_expiry >>

is_terminal_status(status) == (IF (status = "Approved") THEN TRUE ELSE (IF (status = "Denied") THEN TRUE ELSE (status = "Cancelled")))
allowed_non_empty(approve_allowed, deny_allowed) == (IF approve_allowed THEN TRUE ELSE deny_allowed)

Init ==
    /\ phase = "Ready"
    /\ model_step_count = 0
    /\ approval_ids = {}
    /\ approval_statuses = [x \in {} |-> None]
    /\ approval_approve_allowed = [x \in {} |-> None]
    /\ approval_deny_allowed = [x \in {} |-> None]
    /\ approval_has_expiry = [x \in {} |-> None]

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_b7fc92856f337c5b == UNCHANGED << approval_ids, approval_statuses, approval_approve_allowed, approval_deny_allowed, approval_has_expiry >>
UnchangedFrame_cf95e632324f10b9 == UNCHANGED << approval_ids, approval_approve_allowed, approval_deny_allowed, approval_has_expiry >>

CreateRejectedEmptyAllowedDecisions(approval_id, approve_allowed, deny_allowed, has_expiry) ==
    /\ phase = "Ready"
    /\ (allowed_non_empty(approve_allowed, deny_allowed) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


CreateRejectedAlreadyExists(approval_id, approve_allowed, deny_allowed, has_expiry) ==
    /\ phase = "Ready"
    /\ (allowed_non_empty(approve_allowed, deny_allowed) /\ (approval_id \in approval_ids))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


CreatePending(approval_id, approve_allowed, deny_allowed, has_expiry) ==
    /\ phase = "Ready"
    /\ (allowed_non_empty(approve_allowed, deny_allowed) /\ ((approval_id \in approval_ids) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ approval_ids' = (approval_ids \cup {approval_id})
    /\ approval_statuses' = MapSet(approval_statuses, approval_id, "Pending")
    /\ approval_approve_allowed' = MapSet(approval_approve_allowed, approval_id, approve_allowed)
    /\ approval_deny_allowed' = MapSet(approval_deny_allowed, approval_id, deny_allowed)
    /\ approval_has_expiry' = MapSet(approval_has_expiry, approval_id, has_expiry)


RestoreRejectedDuplicate(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision) ==
    /\ phase = "Ready"
    /\ (approval_id \in approval_ids)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


RestoreRejectedEmptyAllowedDecisions(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision) ==
    /\ phase = "Ready"
    /\ (((approval_id \in approval_ids) = FALSE) /\ (allowed_non_empty(approve_allowed, deny_allowed) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


RestorePending(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision) ==
    /\ phase = "Ready"
    /\ (((approval_id \in approval_ids) = FALSE) /\ allowed_non_empty(approve_allowed, deny_allowed) /\ (status = "Pending") /\ (decision = None))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ approval_ids' = (approval_ids \cup {approval_id})
    /\ approval_statuses' = MapSet(approval_statuses, approval_id, "Pending")
    /\ approval_approve_allowed' = MapSet(approval_approve_allowed, approval_id, approve_allowed)
    /\ approval_deny_allowed' = MapSet(approval_deny_allowed, approval_id, deny_allowed)
    /\ approval_has_expiry' = MapSet(approval_has_expiry, approval_id, has_expiry)


RestoreExpired(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision) ==
    /\ phase = "Ready"
    /\ (((approval_id \in approval_ids) = FALSE) /\ allowed_non_empty(approve_allowed, deny_allowed) /\ (status = "Expired") /\ (decision = None))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ approval_ids' = (approval_ids \cup {approval_id})
    /\ approval_statuses' = MapSet(approval_statuses, approval_id, "Expired")
    /\ approval_approve_allowed' = MapSet(approval_approve_allowed, approval_id, approve_allowed)
    /\ approval_deny_allowed' = MapSet(approval_deny_allowed, approval_id, deny_allowed)
    /\ approval_has_expiry' = MapSet(approval_has_expiry, approval_id, has_expiry)


RestoreCancelled(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision) ==
    /\ phase = "Ready"
    /\ (((approval_id \in approval_ids) = FALSE) /\ allowed_non_empty(approve_allowed, deny_allowed) /\ (status = "Cancelled") /\ (decision = None))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ approval_ids' = (approval_ids \cup {approval_id})
    /\ approval_statuses' = MapSet(approval_statuses, approval_id, "Cancelled")
    /\ approval_approve_allowed' = MapSet(approval_approve_allowed, approval_id, approve_allowed)
    /\ approval_deny_allowed' = MapSet(approval_deny_allowed, approval_id, deny_allowed)
    /\ approval_has_expiry' = MapSet(approval_has_expiry, approval_id, has_expiry)


RestoreApproved(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision) ==
    /\ phase = "Ready"
    /\ (((approval_id \in approval_ids) = FALSE) /\ allowed_non_empty(approve_allowed, deny_allowed) /\ approve_allowed /\ (status = "Approved") /\ (decision = Some("Approve")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ approval_ids' = (approval_ids \cup {approval_id})
    /\ approval_statuses' = MapSet(approval_statuses, approval_id, "Approved")
    /\ approval_approve_allowed' = MapSet(approval_approve_allowed, approval_id, approve_allowed)
    /\ approval_deny_allowed' = MapSet(approval_deny_allowed, approval_id, deny_allowed)
    /\ approval_has_expiry' = MapSet(approval_has_expiry, approval_id, has_expiry)


RestoreDenied(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision) ==
    /\ phase = "Ready"
    /\ (((approval_id \in approval_ids) = FALSE) /\ allowed_non_empty(approve_allowed, deny_allowed) /\ deny_allowed /\ (status = "Denied") /\ (decision = Some("Deny")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ approval_ids' = (approval_ids \cup {approval_id})
    /\ approval_statuses' = MapSet(approval_statuses, approval_id, "Denied")
    /\ approval_approve_allowed' = MapSet(approval_approve_allowed, approval_id, approve_allowed)
    /\ approval_deny_allowed' = MapSet(approval_deny_allowed, approval_id, deny_allowed)
    /\ approval_has_expiry' = MapSet(approval_has_expiry, approval_id, has_expiry)


RestoreRejectedInvalidRecord(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision) ==
    /\ phase = "Ready"
    /\ (((approval_id \in approval_ids) = FALSE) /\ allowed_non_empty(approve_allowed, deny_allowed) /\ (IF ((status = "Pending") /\ (decision # None)) THEN TRUE ELSE (IF ((status = "Expired") /\ (decision # None)) THEN TRUE ELSE (IF ((status = "Cancelled") /\ (decision # None)) THEN TRUE ELSE (IF ((status = "Approved") /\ (IF (approve_allowed = FALSE) THEN TRUE ELSE (decision # Some("Approve")))) THEN TRUE ELSE ((status = "Denied") /\ (IF (deny_allowed = FALSE) THEN TRUE ELSE (decision # Some("Deny")))))))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


ObserveExpiryRejectedMissing(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


ObserveExpiryExpiresPending(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None) THEN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending") /\ (IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_has_expiry) THEN Some((IF approval_id \in DOMAIN approval_has_expiry THEN approval_has_expiry[approval_id] ELSE FALSE)) ELSE None) THEN (IF (approval_id \in DOMAIN approval_has_expiry) THEN Some((IF approval_id \in DOMAIN approval_has_expiry THEN approval_has_expiry[approval_id] ELSE FALSE)) ELSE None)["value"] ELSE None) /\ expired)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ approval_statuses' = MapSet(approval_statuses, approval_id, "Expired")
    /\ UnchangedFrame_cf95e632324f10b9


ObserveExpiryPendingNoop(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None) THEN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending") /\ (IF ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_has_expiry) THEN Some((IF approval_id \in DOMAIN approval_has_expiry THEN approval_has_expiry[approval_id] ELSE FALSE)) ELSE None) THEN (IF (approval_id \in DOMAIN approval_has_expiry) THEN Some((IF approval_id \in DOMAIN approval_has_expiry THEN approval_has_expiry[approval_id] ELSE FALSE)) ELSE None)["value"] ELSE None) = FALSE) THEN TRUE ELSE (expired = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


ObserveExpiryApprovedNoop(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None) THEN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Approved"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


ObserveExpiryDeniedNoop(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None) THEN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Denied"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


ObserveExpiryExpiredNoop(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None) THEN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Expired"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


ObserveExpiryCancelledNoop(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None) THEN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Cancelled"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


DecideRejectedMissing(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


DecideRejectedExpired(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None) THEN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Expired"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


DecideRejectedAlreadyDecided(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) /\ is_terminal_status((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None) THEN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None)["value"] ELSE None)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


DecideRejectedApproveNotAllowed(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None) THEN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending") /\ (decision = "Approve") /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_approve_allowed) THEN Some((IF approval_id \in DOMAIN approval_approve_allowed THEN approval_approve_allowed[approval_id] ELSE FALSE)) ELSE None) THEN (IF (approval_id \in DOMAIN approval_approve_allowed) THEN Some((IF approval_id \in DOMAIN approval_approve_allowed THEN approval_approve_allowed[approval_id] ELSE FALSE)) ELSE None)["value"] ELSE None) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


DecideRejectedDenyNotAllowed(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None) THEN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending") /\ (decision = "Deny") /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_deny_allowed) THEN Some((IF approval_id \in DOMAIN approval_deny_allowed THEN approval_deny_allowed[approval_id] ELSE FALSE)) ELSE None) THEN (IF (approval_id \in DOMAIN approval_deny_allowed) THEN Some((IF approval_id \in DOMAIN approval_deny_allowed THEN approval_deny_allowed[approval_id] ELSE FALSE)) ELSE None)["value"] ELSE None) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b7fc92856f337c5b


DecideApprove(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None) THEN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending") /\ (decision = "Approve") /\ (IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_approve_allowed) THEN Some((IF approval_id \in DOMAIN approval_approve_allowed THEN approval_approve_allowed[approval_id] ELSE FALSE)) ELSE None) THEN (IF (approval_id \in DOMAIN approval_approve_allowed) THEN Some((IF approval_id \in DOMAIN approval_approve_allowed THEN approval_approve_allowed[approval_id] ELSE FALSE)) ELSE None)["value"] ELSE None))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ approval_statuses' = MapSet(approval_statuses, approval_id, "Approved")
    /\ UnchangedFrame_cf95e632324f10b9


DecideDeny(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) /\ ((IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None) THEN (IF (approval_id \in DOMAIN approval_statuses) THEN Some((IF approval_id \in DOMAIN approval_statuses THEN approval_statuses[approval_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending") /\ (decision = "Deny") /\ (IF "value" \in DOMAIN (IF (approval_id \in DOMAIN approval_deny_allowed) THEN Some((IF approval_id \in DOMAIN approval_deny_allowed THEN approval_deny_allowed[approval_id] ELSE FALSE)) ELSE None) THEN (IF (approval_id \in DOMAIN approval_deny_allowed) THEN Some((IF approval_id \in DOMAIN approval_deny_allowed THEN approval_deny_allowed[approval_id] ELSE FALSE)) ELSE None)["value"] ELSE None))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ approval_statuses' = MapSet(approval_statuses, approval_id, "Denied")
    /\ UnchangedFrame_cf95e632324f10b9


Next ==
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E approve_allowed \in BOOLEAN : \E deny_allowed \in BOOLEAN : \E has_expiry \in BOOLEAN : CreateRejectedEmptyAllowedDecisions(approval_id, approve_allowed, deny_allowed, has_expiry)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E approve_allowed \in BOOLEAN : \E deny_allowed \in BOOLEAN : \E has_expiry \in BOOLEAN : CreateRejectedAlreadyExists(approval_id, approve_allowed, deny_allowed, has_expiry)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E approve_allowed \in BOOLEAN : \E deny_allowed \in BOOLEAN : \E has_expiry \in BOOLEAN : CreatePending(approval_id, approve_allowed, deny_allowed, has_expiry)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E status \in ApprovalLifecycleStatusValues : \E approve_allowed \in BOOLEAN : \E deny_allowed \in BOOLEAN : \E has_expiry \in BOOLEAN : \E decision \in OptionApprovalLifecycleDecisionValues : RestoreRejectedDuplicate(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E status \in ApprovalLifecycleStatusValues : \E approve_allowed \in BOOLEAN : \E deny_allowed \in BOOLEAN : \E has_expiry \in BOOLEAN : \E decision \in OptionApprovalLifecycleDecisionValues : RestoreRejectedEmptyAllowedDecisions(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E status \in ApprovalLifecycleStatusValues : \E approve_allowed \in BOOLEAN : \E deny_allowed \in BOOLEAN : \E has_expiry \in BOOLEAN : \E decision \in OptionApprovalLifecycleDecisionValues : RestorePending(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E status \in ApprovalLifecycleStatusValues : \E approve_allowed \in BOOLEAN : \E deny_allowed \in BOOLEAN : \E has_expiry \in BOOLEAN : \E decision \in OptionApprovalLifecycleDecisionValues : RestoreExpired(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E status \in ApprovalLifecycleStatusValues : \E approve_allowed \in BOOLEAN : \E deny_allowed \in BOOLEAN : \E has_expiry \in BOOLEAN : \E decision \in OptionApprovalLifecycleDecisionValues : RestoreCancelled(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E status \in ApprovalLifecycleStatusValues : \E deny_allowed \in BOOLEAN : \E has_expiry \in BOOLEAN : \E decision \in OptionApprovalLifecycleDecisionValues : RestoreApproved(approval_id, status, TRUE, deny_allowed, has_expiry, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E status \in ApprovalLifecycleStatusValues : \E approve_allowed \in BOOLEAN : \E has_expiry \in BOOLEAN : \E decision \in OptionApprovalLifecycleDecisionValues : RestoreDenied(approval_id, status, approve_allowed, TRUE, has_expiry, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E status \in ApprovalLifecycleStatusValues : \E approve_allowed \in BOOLEAN : \E deny_allowed \in BOOLEAN : \E has_expiry \in BOOLEAN : \E decision \in OptionApprovalLifecycleDecisionValues : RestoreRejectedInvalidRecord(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E expired \in BOOLEAN : ObserveExpiryRejectedMissing(approval_id, expired)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : ObserveExpiryExpiresPending(approval_id, TRUE)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E expired \in BOOLEAN : ObserveExpiryPendingNoop(approval_id, expired)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E expired \in BOOLEAN : ObserveExpiryApprovedNoop(approval_id, expired)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E expired \in BOOLEAN : ObserveExpiryDeniedNoop(approval_id, expired)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E expired \in BOOLEAN : ObserveExpiryExpiredNoop(approval_id, expired)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E expired \in BOOLEAN : ObserveExpiryCancelledNoop(approval_id, expired)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E decision \in ApprovalLifecycleDecisionValues : DecideRejectedMissing(approval_id, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E decision \in ApprovalLifecycleDecisionValues : DecideRejectedExpired(approval_id, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E decision \in ApprovalLifecycleDecisionValues : DecideRejectedAlreadyDecided(approval_id, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E decision \in ApprovalLifecycleDecisionValues : DecideRejectedApproveNotAllowed(approval_id, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E decision \in ApprovalLifecycleDecisionValues : DecideRejectedDenyNotAllowed(approval_id, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E decision \in ApprovalLifecycleDecisionValues : DecideApprove(approval_id, decision)
    \/ (phase = "Ready") /\ \E approval_id \in StringValues : \E decision \in ApprovalLifecycleDecisionValues : DecideDeny(approval_id, decision)


CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(approval_ids) <= 1 /\ Cardinality(DOMAIN approval_statuses) <= 1 /\ Cardinality(DOMAIN approval_approve_allowed) <= 1 /\ Cardinality(DOMAIN approval_deny_allowed) <= 1 /\ Cardinality(DOMAIN approval_has_expiry) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(approval_ids) <= 2 /\ Cardinality(DOMAIN approval_statuses) <= 2 /\ Cardinality(DOMAIN approval_approve_allowed) <= 2 /\ Cardinality(DOMAIN approval_deny_allowed) <= 2 /\ Cardinality(DOMAIN approval_has_expiry) <= 2

Spec == Init /\ [][Next]_vars


=============================================================================
