---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for ApprovalLifecycleMachine.

CONSTANTS ApprovalLifecycleDecisionValues, ApprovalLifecycleRejectionReasonValues, ApprovalLifecycleStatusValues, BooleanValues, ReviewAttemptStatusValues, ReviewRetirementReasonValues, ReviewVerdictValues, SetOfStringValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapStringApprovalLifecycleStatusValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in ApprovalLifecycleStatusValues }
MapStringBoolValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in BOOLEAN }
MapStringReviewAttemptStatusValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in ReviewAttemptStatusValues }
MapStringReviewRetirementReasonValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in ReviewRetirementReasonValues }
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

VARIABLES phase, model_step_count, approval_ids, approval_statuses, approval_approve_allowed, approval_deny_allowed, approval_has_expiry, review_ids, review_statuses, review_retirements

vars == << phase, model_step_count, approval_ids, approval_statuses, approval_approve_allowed, approval_deny_allowed, approval_has_expiry, review_ids, review_statuses, review_retirements >>

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
    /\ review_ids = {}
    /\ review_statuses = [x \in {} |-> None]
    /\ review_retirements = [x \in {} |-> None]

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_0f3c7fe2720475ae == UNCHANGED << approval_ids, approval_approve_allowed, approval_deny_allowed, approval_has_expiry, review_ids, review_statuses, review_retirements >>
UnchangedFrame_1ae73e15532ef7ee == UNCHANGED << approval_ids, approval_statuses, approval_approve_allowed, approval_deny_allowed, approval_has_expiry, review_retirements >>
UnchangedFrame_296391d88cf1f750 == UNCHANGED << approval_ids, approval_statuses, approval_approve_allowed, approval_deny_allowed, approval_has_expiry, review_ids >>
UnchangedFrame_9efa023076def9c7 == UNCHANGED << review_ids, review_statuses, review_retirements >>
UnchangedFrame_a7f2b58ed53339b4 == UNCHANGED << approval_ids, approval_statuses, approval_approve_allowed, approval_deny_allowed, approval_has_expiry, review_ids, review_statuses, review_retirements >>
UnchangedFrame_b7fc92856f337c5b == UNCHANGED << approval_ids, approval_statuses, approval_approve_allowed, approval_deny_allowed, approval_has_expiry >>
UnchangedFrame_bbad2729dd558069 == UNCHANGED << approval_ids, approval_statuses, approval_approve_allowed, approval_deny_allowed, approval_has_expiry, review_ids, review_retirements >>

CreateRejectedEmptyAllowedDecisions(approval_id, approve_allowed, deny_allowed, has_expiry) ==
    /\ phase = "Ready"
    /\ (allowed_non_empty(approve_allowed, deny_allowed) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


CreateRejectedAlreadyExists(approval_id, approve_allowed, deny_allowed, has_expiry) ==
    /\ phase = "Ready"
    /\ (allowed_non_empty(approve_allowed, deny_allowed) /\ (approval_id \in approval_ids))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


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
    /\ UnchangedFrame_9efa023076def9c7


RestoreRejectedDuplicate(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision) ==
    /\ phase = "Ready"
    /\ (approval_id \in approval_ids)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


RestoreRejectedEmptyAllowedDecisions(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision) ==
    /\ phase = "Ready"
    /\ (((approval_id \in approval_ids) = FALSE) /\ (allowed_non_empty(approve_allowed, deny_allowed) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


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
    /\ UnchangedFrame_9efa023076def9c7


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
    /\ UnchangedFrame_9efa023076def9c7


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
    /\ UnchangedFrame_9efa023076def9c7


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
    /\ UnchangedFrame_9efa023076def9c7


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
    /\ UnchangedFrame_9efa023076def9c7


RestoreRejectedInvalidRecord(approval_id, status, approve_allowed, deny_allowed, has_expiry, decision) ==
    /\ phase = "Ready"
    /\ (((approval_id \in approval_ids) = FALSE) /\ allowed_non_empty(approve_allowed, deny_allowed) /\ (IF ((status = "Pending") /\ (decision # None)) THEN TRUE ELSE (IF ((status = "Expired") /\ (decision # None)) THEN TRUE ELSE (IF ((status = "Cancelled") /\ (decision # None)) THEN TRUE ELSE (IF ((status = "Approved") /\ (IF (approve_allowed = FALSE) THEN TRUE ELSE (decision # Some("Approve")))) THEN TRUE ELSE ((status = "Denied") /\ (IF (deny_allowed = FALSE) THEN TRUE ELSE (decision # Some("Deny")))))))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


ObserveExpiryRejectedMissing(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


ObserveExpiryExpiresPending(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((IF ~((approval_id \in approval_ids)) THEN TRUE ELSE ((approval_id \in DOMAIN approval_statuses) /\ (IF ~(((approval_statuses)[approval_id] = "Pending")) THEN TRUE ELSE (approval_id \in DOMAIN approval_has_expiry)))) /\ ((approval_id \in approval_ids) /\ ((approval_statuses)[approval_id] = "Pending") /\ (approval_has_expiry)[approval_id] /\ expired))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ approval_statuses' = MapSet(approval_statuses, approval_id, "Expired")
    /\ UnchangedFrame_0f3c7fe2720475ae


ObserveExpiryPendingNoop(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((IF ~((approval_id \in approval_ids)) THEN TRUE ELSE ((approval_id \in DOMAIN approval_statuses) /\ (IF ~(((approval_statuses)[approval_id] = "Pending")) THEN TRUE ELSE (approval_id \in DOMAIN approval_has_expiry)))) /\ ((approval_id \in approval_ids) /\ ((approval_statuses)[approval_id] = "Pending") /\ (IF ((approval_has_expiry)[approval_id] = FALSE) THEN TRUE ELSE (expired = FALSE))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


ObserveExpiryApprovedNoop(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((IF ~((approval_id \in approval_ids)) THEN TRUE ELSE (approval_id \in DOMAIN approval_statuses)) /\ ((approval_id \in approval_ids) /\ ((approval_statuses)[approval_id] = "Approved")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


ObserveExpiryDeniedNoop(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((IF ~((approval_id \in approval_ids)) THEN TRUE ELSE (approval_id \in DOMAIN approval_statuses)) /\ ((approval_id \in approval_ids) /\ ((approval_statuses)[approval_id] = "Denied")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


ObserveExpiryExpiredNoop(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((IF ~((approval_id \in approval_ids)) THEN TRUE ELSE (approval_id \in DOMAIN approval_statuses)) /\ ((approval_id \in approval_ids) /\ ((approval_statuses)[approval_id] = "Expired")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


ObserveExpiryCancelledNoop(approval_id, expired) ==
    /\ phase = "Ready"
    /\ ((IF ~((approval_id \in approval_ids)) THEN TRUE ELSE (approval_id \in DOMAIN approval_statuses)) /\ ((approval_id \in approval_ids) /\ ((approval_statuses)[approval_id] = "Cancelled")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


DecideRejectedMissing(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((approval_id \in approval_ids) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


DecideRejectedExpired(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((IF ~((approval_id \in approval_ids)) THEN TRUE ELSE (approval_id \in DOMAIN approval_statuses)) /\ ((approval_id \in approval_ids) /\ ((approval_statuses)[approval_id] = "Expired")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


DecideRejectedAlreadyDecided(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((IF ~((approval_id \in approval_ids)) THEN TRUE ELSE (approval_id \in DOMAIN approval_statuses)) /\ ((approval_id \in approval_ids) /\ is_terminal_status((approval_statuses)[approval_id])))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


DecideRejectedApproveNotAllowed(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((IF ~((approval_id \in approval_ids)) THEN TRUE ELSE ((approval_id \in DOMAIN approval_statuses) /\ (IF ~(((approval_statuses)[approval_id] = "Pending")) THEN TRUE ELSE (IF ~((decision = "Approve")) THEN TRUE ELSE (approval_id \in DOMAIN approval_approve_allowed))))) /\ ((approval_id \in approval_ids) /\ ((approval_statuses)[approval_id] = "Pending") /\ (decision = "Approve") /\ ((approval_approve_allowed)[approval_id] = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


DecideRejectedDenyNotAllowed(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((IF ~((approval_id \in approval_ids)) THEN TRUE ELSE ((approval_id \in DOMAIN approval_statuses) /\ (IF ~(((approval_statuses)[approval_id] = "Pending")) THEN TRUE ELSE (IF ~((decision = "Deny")) THEN TRUE ELSE (approval_id \in DOMAIN approval_deny_allowed))))) /\ ((approval_id \in approval_ids) /\ ((approval_statuses)[approval_id] = "Pending") /\ (decision = "Deny") /\ ((approval_deny_allowed)[approval_id] = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


DecideApprove(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((IF ~((approval_id \in approval_ids)) THEN TRUE ELSE ((approval_id \in DOMAIN approval_statuses) /\ (IF ~(((approval_statuses)[approval_id] = "Pending")) THEN TRUE ELSE (IF ~((decision = "Approve")) THEN TRUE ELSE (approval_id \in DOMAIN approval_approve_allowed))))) /\ ((approval_id \in approval_ids) /\ ((approval_statuses)[approval_id] = "Pending") /\ (decision = "Approve") /\ (approval_approve_allowed)[approval_id]))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ approval_statuses' = MapSet(approval_statuses, approval_id, "Approved")
    /\ UnchangedFrame_0f3c7fe2720475ae


DecideDeny(approval_id, decision) ==
    /\ phase = "Ready"
    /\ ((IF ~((approval_id \in approval_ids)) THEN TRUE ELSE ((approval_id \in DOMAIN approval_statuses) /\ (IF ~(((approval_statuses)[approval_id] = "Pending")) THEN TRUE ELSE (IF ~((decision = "Deny")) THEN TRUE ELSE (approval_id \in DOMAIN approval_deny_allowed))))) /\ ((approval_id \in approval_ids) /\ ((approval_statuses)[approval_id] = "Pending") /\ (decision = "Deny") /\ (approval_deny_allowed)[approval_id]))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ approval_statuses' = MapSet(approval_statuses, approval_id, "Denied")
    /\ UnchangedFrame_0f3c7fe2720475ae


BeginReviewRejectedDuplicate(review_id) ==
    /\ phase = "Ready"
    /\ (review_id \in review_ids)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


BeginReviewPending(review_id) ==
    /\ phase = "Ready"
    /\ ((review_id \in review_ids) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_ids' = (review_ids \cup {review_id})
    /\ review_statuses' = MapSet(review_statuses, review_id, "Pending")
    /\ UnchangedFrame_1ae73e15532ef7ee


RecordReviewVerdictRejectedMissing(review_id, verdict) ==
    /\ phase = "Ready"
    /\ ((review_id \in review_ids) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


RecordReviewVerdictRejectedRetired(review_id, verdict) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Retired")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


RecordReviewVerdictRejectedSettled(review_id, verdict) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE ((review_id \in DOMAIN review_statuses) /\ (IF ~(((review_statuses)[review_id] # "Pending")) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)))) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] # "Pending") /\ ((review_statuses)[review_id] # "Retired")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


RecordReviewVerdictAllowed(review_id, verdict) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Pending") /\ (verdict = "Allow")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_statuses' = MapSet(review_statuses, review_id, "Allowed")
    /\ UnchangedFrame_bbad2729dd558069


RecordReviewVerdictDenied(review_id, verdict) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Pending") /\ (verdict = "Deny")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_statuses' = MapSet(review_statuses, review_id, "Denied")
    /\ UnchangedFrame_bbad2729dd558069


RecordReviewVerdictEscalated(review_id, verdict) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Pending") /\ (verdict = "Escalate")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_statuses' = MapSet(review_statuses, review_id, "Escalated")
    /\ UnchangedFrame_bbad2729dd558069


RecordReviewUnavailableRejectedMissing(review_id) ==
    /\ phase = "Ready"
    /\ ((review_id \in review_ids) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


RecordReviewUnavailableRejectedRetired(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Retired")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


RecordReviewUnavailableRejectedSettled(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE ((review_id \in DOMAIN review_statuses) /\ (IF ~(((review_statuses)[review_id] # "Pending")) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)))) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] # "Pending") /\ ((review_statuses)[review_id] # "Retired")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


RecordReviewUnavailable(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Pending")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_statuses' = MapSet(review_statuses, review_id, "Unavailable")
    /\ UnchangedFrame_bbad2729dd558069


RetireReviewRejectedMissing(review_id, reason) ==
    /\ phase = "Ready"
    /\ ((review_id \in review_ids) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


RetireReviewRejectedRetired(review_id, reason) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Retired")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


RetireReviewRejectedSettled(review_id, reason) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE ((review_id \in DOMAIN review_statuses) /\ (IF ~(((review_statuses)[review_id] # "Pending")) THEN TRUE ELSE ((review_id \in DOMAIN review_statuses) /\ (IF ~(((review_statuses)[review_id] # "Allowed")) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)))))) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] # "Pending") /\ ((review_statuses)[review_id] # "Allowed") /\ ((review_statuses)[review_id] # "Retired")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


RetireReview(review_id, reason) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE ((review_id \in DOMAIN review_statuses) /\ (IF ((review_statuses)[review_id] = "Pending") THEN TRUE ELSE (review_id \in DOMAIN review_statuses)))) /\ ((review_id \in review_ids) /\ (IF ((review_statuses)[review_id] = "Pending") THEN TRUE ELSE ((review_statuses)[review_id] = "Allowed"))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_statuses' = MapSet(review_statuses, review_id, "Retired")
    /\ review_retirements' = MapSet(review_retirements, review_id, reason)
    /\ UnchangedFrame_296391d88cf1f750


ConsumeReviewRejectedMissing(review_id) ==
    /\ phase = "Ready"
    /\ ((review_id \in review_ids) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


ConsumeReviewRejectedRetired(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Retired")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


ConsumeReviewRejectedUsed(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Used")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


ConsumeReviewRejectedNotSatisfied(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE ((review_id \in DOMAIN review_statuses) /\ (IF ((review_statuses)[review_id] = "Pending") THEN TRUE ELSE ((review_id \in DOMAIN review_statuses) /\ (IF ((review_statuses)[review_id] = "Denied") THEN TRUE ELSE ((review_id \in DOMAIN review_statuses) /\ (IF ((review_statuses)[review_id] = "Escalated") THEN TRUE ELSE (review_id \in DOMAIN review_statuses)))))))) /\ ((review_id \in review_ids) /\ (IF ((review_statuses)[review_id] = "Pending") THEN TRUE ELSE (IF ((review_statuses)[review_id] = "Denied") THEN TRUE ELSE (IF ((review_statuses)[review_id] = "Escalated") THEN TRUE ELSE ((review_statuses)[review_id] = "Unavailable"))))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


ConsumeReviewForEntry(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Allowed")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_statuses' = MapSet(review_statuses, review_id, "Used")
    /\ UnchangedFrame_bbad2729dd558069


ReleaseReviewRejectedMissing(review_id) ==
    /\ phase = "Ready"
    /\ ((review_id \in review_ids) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


ReleaseReviewRejectedPending(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Pending")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_a7f2b58ed53339b4


ReleaseReviewAllowed(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Allowed")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_ids' = (review_ids \ {review_id})
    /\ review_statuses' = MapRemove(review_statuses, review_id)
    /\ review_retirements' = MapRemove(review_retirements, review_id)
    /\ UnchangedFrame_b7fc92856f337c5b


ReleaseReviewDenied(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Denied")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_ids' = (review_ids \ {review_id})
    /\ review_statuses' = MapRemove(review_statuses, review_id)
    /\ review_retirements' = MapRemove(review_retirements, review_id)
    /\ UnchangedFrame_b7fc92856f337c5b


ReleaseReviewEscalated(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Escalated")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_ids' = (review_ids \ {review_id})
    /\ review_statuses' = MapRemove(review_statuses, review_id)
    /\ review_retirements' = MapRemove(review_retirements, review_id)
    /\ UnchangedFrame_b7fc92856f337c5b


ReleaseReviewUnavailable(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Unavailable")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_ids' = (review_ids \ {review_id})
    /\ review_statuses' = MapRemove(review_statuses, review_id)
    /\ review_retirements' = MapRemove(review_retirements, review_id)
    /\ UnchangedFrame_b7fc92856f337c5b


ReleaseReviewRetired(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Retired")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_ids' = (review_ids \ {review_id})
    /\ review_statuses' = MapRemove(review_statuses, review_id)
    /\ review_retirements' = MapRemove(review_retirements, review_id)
    /\ UnchangedFrame_b7fc92856f337c5b


ReleaseReviewUsed(review_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((review_id \in review_ids)) THEN TRUE ELSE (review_id \in DOMAIN review_statuses)) /\ ((review_id \in review_ids) /\ ((review_statuses)[review_id] = "Used")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ review_ids' = (review_ids \ {review_id})
    /\ review_statuses' = MapRemove(review_statuses, review_id)
    /\ review_retirements' = MapRemove(review_retirements, review_id)
    /\ UnchangedFrame_b7fc92856f337c5b


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
    \/ (phase = "Ready") /\ \E review_id \in StringValues : BeginReviewRejectedDuplicate(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : BeginReviewPending(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : \E verdict \in ReviewVerdictValues : RecordReviewVerdictRejectedMissing(review_id, verdict)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : \E verdict \in ReviewVerdictValues : RecordReviewVerdictRejectedRetired(review_id, verdict)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : \E verdict \in ReviewVerdictValues : RecordReviewVerdictRejectedSettled(review_id, verdict)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : \E verdict \in ReviewVerdictValues : RecordReviewVerdictAllowed(review_id, verdict)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : \E verdict \in ReviewVerdictValues : RecordReviewVerdictDenied(review_id, verdict)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : \E verdict \in ReviewVerdictValues : RecordReviewVerdictEscalated(review_id, verdict)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : RecordReviewUnavailableRejectedMissing(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : RecordReviewUnavailableRejectedRetired(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : RecordReviewUnavailableRejectedSettled(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : RecordReviewUnavailable(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : \E reason \in ReviewRetirementReasonValues : RetireReviewRejectedMissing(review_id, reason)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : \E reason \in ReviewRetirementReasonValues : RetireReviewRejectedRetired(review_id, reason)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : \E reason \in ReviewRetirementReasonValues : RetireReviewRejectedSettled(review_id, reason)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : \E reason \in ReviewRetirementReasonValues : RetireReview(review_id, reason)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ConsumeReviewRejectedMissing(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ConsumeReviewRejectedRetired(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ConsumeReviewRejectedUsed(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ConsumeReviewRejectedNotSatisfied(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ConsumeReviewForEntry(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ReleaseReviewRejectedMissing(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ReleaseReviewRejectedPending(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ReleaseReviewAllowed(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ReleaseReviewDenied(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ReleaseReviewEscalated(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ReleaseReviewUnavailable(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ReleaseReviewRetired(review_id)
    \/ (phase = "Ready") /\ \E review_id \in StringValues : ReleaseReviewUsed(review_id)

approval_maps_cover_exactly_the_registered_ids == ((DOMAIN approval_statuses = approval_ids) /\ (DOMAIN approval_approve_allowed = approval_ids) /\ (DOMAIN approval_deny_allowed = approval_ids) /\ (DOMAIN approval_has_expiry = approval_ids))
review_statuses_cover_exactly_the_review_ids == (DOMAIN review_statuses = review_ids)
review_retirement_only_for_retired_attempts == (\A id \in DOMAIN review_retirements : ((IF (id \in DOMAIN review_statuses) THEN Some((IF id \in DOMAIN review_statuses THEN review_statuses[id] ELSE "None")) ELSE None) = Some("Retired")))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(approval_ids) <= 1 /\ Cardinality(DOMAIN approval_statuses) <= 1 /\ Cardinality(DOMAIN approval_approve_allowed) <= 1 /\ Cardinality(DOMAIN approval_deny_allowed) <= 1 /\ Cardinality(DOMAIN approval_has_expiry) <= 1 /\ Cardinality(review_ids) <= 1 /\ Cardinality(DOMAIN review_statuses) <= 1 /\ Cardinality(DOMAIN review_retirements) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(approval_ids) <= 2 /\ Cardinality(DOMAIN approval_statuses) <= 2 /\ Cardinality(DOMAIN approval_approve_allowed) <= 2 /\ Cardinality(DOMAIN approval_deny_allowed) <= 2 /\ Cardinality(DOMAIN approval_has_expiry) <= 2 /\ Cardinality(review_ids) <= 2 /\ Cardinality(DOMAIN review_statuses) <= 2 /\ Cardinality(DOMAIN review_retirements) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []approval_maps_cover_exactly_the_registered_ids
THEOREM Spec => []review_statuses_cover_exactly_the_review_ids
THEOREM Spec => []review_retirement_only_for_retired_attempts

=============================================================================
