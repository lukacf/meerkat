---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for WorkGraphLifecycleMachine.

CONSTANTS BooleanValues, CancelledChildJoinPolicyValues, ChildJoinDispositionValues, FailedChildJoinPolicyValues, NatValues, SetOfWorkDependencyPathKeyValues, SetOfWorkEdgeKeyValues, SetOfWorkItemKeyValues, SetOfWorkOwnerKeyValues, WorkAdmissionDigestRefValues, WorkAdmissionKeyRefValues, WorkAdmissionReplayKindValues, WorkCloseStatusAdmissionKindValues, WorkCompletionPolicyMutationAdmissionKindValues, WorkCompletionPolicyValues, WorkConfirmationAdmissionKindValues, WorkConfirmationEvidenceObservationValues, WorkCreateCompletionPolicyAdmissionKindValues, WorkCreateStatusAdmissionKindValues, WorkDependencyPathKeyValues, WorkEdgeKeyValues, WorkEdgeKindValues, WorkEvidenceKindValues, WorkGraphErrorKindValues, WorkGraphPublicErrorClassValues, WorkItemKeyValues, WorkLifecycleStateValues, WorkOwnerKeyValues, WorkOwnerKindValues, WorkPolicyEscalationAdmissionKindValues, WorkPublicConfirmationAdmissionKindValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

SetOfWorkDependencyPathKeyValuesCi == {{}}
SetOfWorkEdgeKeyValuesCi == {{}}
SetOfWorkOwnerKeyValuesCi == {{}}
WorkDependencyPathKeyValuesCi == {}
WorkEdgeKeyValuesCi == {}
WorkOwnerKeyValuesCi == {}

SetOfWorkDependencyPathKeyValuesDeep == {{}, {[kind |-> "Blocks", from_item_key |-> "workitemkey_1", to_item_key |-> "workitemkey_1"]}, {[kind |-> "Blocks", from_item_key |-> "workitemkey_1", to_item_key |-> "workitemkey_1"], [kind |-> "Parent", from_item_key |-> "workitemkey_2", to_item_key |-> "workitemkey_2"]}}
SetOfWorkEdgeKeyValuesDeep == {{}, {[kind |-> "Blocks", from_item_key |-> "workitemkey_1", to_item_key |-> "workitemkey_1"]}, {[kind |-> "Blocks", from_item_key |-> "workitemkey_1", to_item_key |-> "workitemkey_1"], [kind |-> "Parent", from_item_key |-> "workitemkey_2", to_item_key |-> "workitemkey_2"]}}
SetOfWorkOwnerKeyValuesDeep == {{}, {[kind |-> "Principal", id |-> "alpha"]}, {[kind |-> "Principal", id |-> "alpha"], [kind |-> "Agent", id |-> "beta"]}}
WorkDependencyPathKeyValuesDeep == {[kind |-> "Blocks", from_item_key |-> "workitemkey_1", to_item_key |-> "workitemkey_1"], [kind |-> "Parent", from_item_key |-> "workitemkey_2", to_item_key |-> "workitemkey_2"]}
WorkEdgeKeyValuesDeep == {[kind |-> "Blocks", from_item_key |-> "workitemkey_1", to_item_key |-> "workitemkey_1"], [kind |-> "Parent", from_item_key |-> "workitemkey_2", to_item_key |-> "workitemkey_2"]}
WorkOwnerKeyValuesDeep == {[kind |-> "Principal", id |-> "alpha"], [kind |-> "Agent", id |-> "beta"]}

OptionU64Values == {None} \cup {Some(x) : x \in NatValues}
OptionWorkAdmissionDigestRefValues == {None} \cup {Some(x) : x \in WorkAdmissionDigestRefValues}
OptionWorkAdmissionKeyRefValues == {None} \cup {Some(x) : x \in WorkAdmissionKeyRefValues}
OptionWorkOwnerKeyValues == {None} \cup {Some(x) : x \in WorkOwnerKeyValues}
OptionWorkOwnerKindValues == {None} \cup {Some(x) : x \in WorkOwnerKindValues}

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

VARIABLES phase, model_step_count, revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy, admission_key, admission_request_digest

vars == << phase, model_step_count, revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy, admission_key, admission_request_digest >>

claim_time_window_eligible(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, now_utc_ms) == ((IF (arg_due_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_due_at_utc_ms THEN arg_due_at_utc_ms["value"] ELSE None) <= now_utc_ms)) /\ (IF (arg_not_before_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_not_before_utc_ms THEN arg_not_before_utc_ms["value"] ELSE None) <= now_utc_ms)) /\ (IF (arg_snoozed_until_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_snoozed_until_utc_ms THEN arg_snoozed_until_utc_ms["value"] ELSE None) <= now_utc_ms)))
confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind) == ((arg_completion_policy = "SelfAttest") /\ (supplied_evidence_kind = "Empty"))
confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key) == ((arg_completion_policy = "Supervisor") /\ (requested_principal_owner_key # None) /\ (IF (arg_completion_supervisor_owner_key = None) THEN TRUE ELSE ((IF "value" \in DOMAIN requested_principal_owner_key THEN requested_principal_owner_key["value"] ELSE None) # (IF "value" \in DOMAIN arg_completion_supervisor_owner_key THEN arg_completion_supervisor_owner_key["value"] ELSE None))))
confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind) == ((arg_completion_policy = "PrincipalConfirmed") /\ (requested_principal_owner_key # None) /\ (IF (requested_principal_kind = None) THEN TRUE ELSE ((IF "value" \in DOMAIN requested_principal_kind THEN requested_principal_kind["value"] ELSE None) # "Principal")))
confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key) == ((IF (arg_completion_policy = "PrincipalConfirmed") THEN TRUE ELSE (IF (arg_completion_policy = "Supervisor") THEN TRUE ELSE (arg_completion_policy = "ReviewerQuorum"))) /\ (requested_principal_owner_key = None))
evidence_kind_owner_key_present(evidence_kind, confirming_owner_key) == (IF (evidence_kind = "SupervisorConfirmation") THEN (confirming_owner_key # None) ELSE (IF (evidence_kind = "ReviewerConfirmation") THEN (confirming_owner_key # None) ELSE TRUE))
completion_policy_is_satisfied(policy, supervisor_owner_key, reviewer_quorum_threshold, arg_host_confirmation_count, arg_principal_confirmation_count, arg_supervisor_confirmation_owner_keys, arg_reviewer_confirmation_owner_keys) == (IF (policy = "SelfAttest") THEN TRUE ELSE (IF (policy = "HostConfirmed") THEN (arg_host_confirmation_count > 0) ELSE (IF (policy = "PrincipalConfirmed") THEN (arg_principal_confirmation_count > 0) ELSE (IF (policy = "Supervisor") THEN ((supervisor_owner_key # None) /\ ((IF "value" \in DOMAIN supervisor_owner_key THEN supervisor_owner_key["value"] ELSE None) \in arg_supervisor_confirmation_owner_keys)) ELSE ((reviewer_quorum_threshold # None) /\ (Cardinality(arg_reviewer_confirmation_owner_keys) >= (IF "value" \in DOMAIN reviewer_quorum_threshold THEN reviewer_quorum_threshold["value"] ELSE None)))))))
completion_policy_payload_valid(policy, supervisor_owner_key, reviewer_quorum_threshold) == (IF (policy = "Supervisor") THEN ((supervisor_owner_key # None) /\ (reviewer_quorum_threshold = None)) ELSE (IF (policy = "ReviewerQuorum") THEN ((supervisor_owner_key = None) /\ (reviewer_quorum_threshold # None) /\ ((IF "value" \in DOMAIN reviewer_quorum_threshold THEN reviewer_quorum_threshold["value"] ELSE None) > 0)) ELSE ((supervisor_owner_key = None) /\ (reviewer_quorum_threshold = None))))
confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) == (IF (arg_completion_policy = "HostConfirmed") THEN (supplied_evidence_kind # "HostConfirmation") ELSE (IF (arg_completion_policy = "PrincipalConfirmed") THEN ((confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key) = FALSE) /\ (confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind) = FALSE) /\ (supplied_evidence_kind # "PrincipalConfirmation")) ELSE (IF (arg_completion_policy = "Supervisor") THEN ((confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key) = FALSE) /\ (confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key) = FALSE) /\ (supplied_evidence_kind # "SupervisorConfirmation")) ELSE (IF (arg_completion_policy = "ReviewerQuorum") THEN ((confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key) = FALSE) /\ (supplied_evidence_kind # "ReviewerConfirmation")) ELSE FALSE))))
completion_policy_escalation_admissible(current_policy, current_reviewer_quorum_threshold, requested_policy, requested_supervisor_owner_key, requested_reviewer_quorum_threshold) == (completion_policy_payload_valid(requested_policy, requested_supervisor_owner_key, requested_reviewer_quorum_threshold) /\ (IF ((current_policy = "SelfAttest") /\ (requested_policy # "SelfAttest")) THEN TRUE ELSE ((current_policy = "ReviewerQuorum") /\ (requested_policy = "ReviewerQuorum") /\ (current_reviewer_quorum_threshold # None) /\ (requested_reviewer_quorum_threshold # None) /\ ((IF "value" \in DOMAIN requested_reviewer_quorum_threshold THEN requested_reviewer_quorum_threshold["value"] ELSE None) > (IF "value" \in DOMAIN current_reviewer_quorum_threshold THEN current_reviewer_quorum_threshold["value"] ELSE None)))))
confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) == ((confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key) = FALSE) /\ (confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind) = FALSE) /\ (confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key) = FALSE) /\ (confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind) = FALSE) /\ (confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) = FALSE))

Init ==
    /\ phase = "Absent"
    /\ model_step_count = 0
    /\ revision = 0
    /\ unresolved_blocker_count = 0
    /\ topology_item_keys = {}
    /\ topology_edge_keys = {}
    /\ blocks_reachability = {}
    /\ parent_reachability = {}
    /\ claim_owner_key = None
    /\ claimed_at_utc_ms = None
    /\ lease_expires_at_utc_ms = None
    /\ due_at_utc_ms = None
    /\ not_before_utc_ms = None
    /\ snoozed_until_utc_ms = None
    /\ completion_policy = "SelfAttest"
    /\ completion_supervisor_owner_key = None
    /\ completion_reviewer_quorum_threshold = None
    /\ terminal_at_utc_ms = None
    /\ evidence_count = 0
    /\ host_confirmation_count = 0
    /\ principal_confirmation_count = 0
    /\ supervisor_confirmation_owner_keys = {}
    /\ reviewer_confirmation_owner_keys = {}
    /\ failed_child_join_policy = "RequireSuccess"
    /\ cancelled_child_join_policy = "RequireSuccess"
    /\ admission_key = None
    /\ admission_request_digest = None

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Cancelled" \/ phase = "Failed"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_49f64e92f6cb9fc7 == UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy, admission_key, admission_request_digest >>
UnchangedFrame_4a47e10320b99e05 == UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy, admission_key, admission_request_digest >>
UnchangedFrame_6e245d8d68381c13 == UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy, admission_key, admission_request_digest >>
UnchangedFrame_8d339dd77aca8942 == UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy, admission_key, admission_request_digest >>
UnchangedFrame_a36a4d58e8925165 == UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy, admission_key, admission_request_digest >>
UnchangedFrame_b10385287604008b == UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, failed_child_join_policy, cancelled_child_join_policy, admission_key, admission_request_digest >>
UnchangedFrame_bd690218bdfad0fb == UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy, admission_key, admission_request_digest >>
UnchangedFrame_e03ecbdbe867fd2b == UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy, admission_key, admission_request_digest >>
UnchangedFrame_ea30709c66621d98 == UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>

CreateOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count, arg_failed_child_join_policy, arg_cancelled_child_join_policy, arg_admission_key, arg_admission_request_digest) ==
    /\ phase = "Absent"
    /\ completion_policy_payload_valid(arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold)
    /\ (IF ((arg_admission_key = None) /\ (arg_admission_request_digest = None)) THEN TRUE ELSE ((arg_admission_key # None) /\ (arg_admission_request_digest # None)))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ completion_policy' = arg_completion_policy
    /\ completion_supervisor_owner_key' = arg_completion_supervisor_owner_key
    /\ completion_reviewer_quorum_threshold' = arg_completion_reviewer_quorum_threshold
    /\ failed_child_join_policy' = arg_failed_child_join_policy
    /\ cancelled_child_join_policy' = arg_cancelled_child_join_policy
    /\ admission_key' = arg_admission_key
    /\ admission_request_digest' = arg_admission_request_digest
    /\ UnchangedFrame_ea30709c66621d98


CreateBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count, arg_failed_child_join_policy, arg_cancelled_child_join_policy, arg_admission_key, arg_admission_request_digest) ==
    /\ phase = "Absent"
    /\ completion_policy_payload_valid(arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold)
    /\ (IF ((arg_admission_key = None) /\ (arg_admission_request_digest = None)) THEN TRUE ELSE ((arg_admission_key # None) /\ (arg_admission_request_digest # None)))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ completion_policy' = arg_completion_policy
    /\ completion_supervisor_owner_key' = arg_completion_supervisor_owner_key
    /\ completion_reviewer_quorum_threshold' = arg_completion_reviewer_quorum_threshold
    /\ failed_child_join_policy' = arg_failed_child_join_policy
    /\ cancelled_child_join_policy' = arg_cancelled_child_join_policy
    /\ admission_key' = arg_admission_key
    /\ admission_request_digest' = arg_admission_request_digest
    /\ UnchangedFrame_ea30709c66621d98


UpdateOpen(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ completion_policy_payload_valid(arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold)
    /\ ((arg_completion_policy = completion_policy) /\ (arg_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (arg_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ UnchangedFrame_e03ecbdbe867fd2b


UpdateInProgress(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ completion_policy_payload_valid(arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold)
    /\ ((arg_completion_policy = completion_policy) /\ (arg_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (arg_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ UnchangedFrame_e03ecbdbe867fd2b


UpdateBlocked(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ completion_policy_payload_valid(arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold)
    /\ ((arg_completion_policy = completion_policy) /\ (arg_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (arg_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ UnchangedFrame_e03ecbdbe867fd2b


PolicyEscalateOpenAdmitted(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ completion_policy_escalation_admissible(completion_policy, completion_reviewer_quorum_threshold, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ completion_policy' = requested_completion_policy
    /\ completion_supervisor_owner_key' = requested_completion_supervisor_owner_key
    /\ completion_reviewer_quorum_threshold' = requested_completion_reviewer_quorum_threshold
    /\ UnchangedFrame_bd690218bdfad0fb


PolicyEscalateOpenDenied(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ (completion_policy_escalation_admissible(completion_policy, completion_reviewer_quorum_threshold, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) = FALSE)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


PolicyEscalateInProgressAdmitted(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ completion_policy_escalation_admissible(completion_policy, completion_reviewer_quorum_threshold, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ completion_policy' = requested_completion_policy
    /\ completion_supervisor_owner_key' = requested_completion_supervisor_owner_key
    /\ completion_reviewer_quorum_threshold' = requested_completion_reviewer_quorum_threshold
    /\ UnchangedFrame_bd690218bdfad0fb


PolicyEscalateInProgressDenied(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ (completion_policy_escalation_admissible(completion_policy, completion_reviewer_quorum_threshold, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) = FALSE)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


PolicyEscalateBlockedAdmitted(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ completion_policy_escalation_admissible(completion_policy, completion_reviewer_quorum_threshold, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ completion_policy' = requested_completion_policy
    /\ completion_supervisor_owner_key' = requested_completion_supervisor_owner_key
    /\ completion_reviewer_quorum_threshold' = requested_completion_reviewer_quorum_threshold
    /\ UnchangedFrame_bd690218bdfad0fb


PolicyEscalateBlockedDenied(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ (completion_policy_escalation_admissible(completion_policy, completion_reviewer_quorum_threshold, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) = FALSE)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClaimOpen(expected_revision, owner_key, now_utc_ms, arg_lease_expires_at_utc_ms, child_join_satisfied) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ (unresolved_blocker_count = 0)
    /\ child_join_satisfied
    /\ (IF (arg_lease_expires_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_lease_expires_at_utc_ms THEN arg_lease_expires_at_utc_ms["value"] ELSE None) > now_utc_ms))
    /\ (IF (due_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN due_at_utc_ms THEN due_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ (IF (not_before_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN not_before_utc_ms THEN not_before_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ (IF (snoozed_until_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN snoozed_until_utc_ms THEN snoozed_until_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = Some(owner_key)
    /\ claimed_at_utc_ms' = Some(now_utc_ms)
    /\ lease_expires_at_utc_ms' = arg_lease_expires_at_utc_ms
    /\ UnchangedFrame_6e245d8d68381c13


ClaimExpiredInProgress(expected_revision, owner_key, now_utc_ms, arg_lease_expires_at_utc_ms, child_join_satisfied) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ (claim_owner_key # None)
    /\ (lease_expires_at_utc_ms # None)
    /\ (IF (lease_expires_at_utc_ms = None) THEN FALSE ELSE ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ (unresolved_blocker_count = 0)
    /\ child_join_satisfied
    /\ (IF (arg_lease_expires_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_lease_expires_at_utc_ms THEN arg_lease_expires_at_utc_ms["value"] ELSE None) > now_utc_ms))
    /\ (IF (due_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN due_at_utc_ms THEN due_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ (IF (not_before_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN not_before_utc_ms THEN not_before_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ (IF (snoozed_until_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN snoozed_until_utc_ms THEN snoozed_until_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = Some(owner_key)
    /\ claimed_at_utc_ms' = Some(now_utc_ms)
    /\ lease_expires_at_utc_ms' = arg_lease_expires_at_utc_ms
    /\ UnchangedFrame_6e245d8d68381c13


ReleaseInProgress(expected_revision) ==
    /\ phase = "InProgress"
    /\ ((revision = expected_revision) /\ (claim_owner_key # None))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UnchangedFrame_6e245d8d68381c13


ObserveLeaseExpiryInProgress(expected_revision, observed_at_utc_ms) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ (claim_owner_key # None)
    /\ (lease_expires_at_utc_ms # None)
    /\ ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= observed_at_utc_ms)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UnchangedFrame_6e245d8d68381c13


ObserveReadinessOpen(expected_revision, observed_at_utc_ms, child_join_satisfied) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ (unresolved_blocker_count = 0)
    /\ child_join_satisfied
    /\ (IF (due_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN due_at_utc_ms THEN due_at_utc_ms["value"] ELSE None) <= observed_at_utc_ms))
    /\ (IF (not_before_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN not_before_utc_ms THEN not_before_utc_ms["value"] ELSE None) <= observed_at_utc_ms))
    /\ (IF (snoozed_until_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN snoozed_until_utc_ms THEN snoozed_until_utc_ms["value"] ELSE None) <= observed_at_utc_ms))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ UnchangedFrame_4a47e10320b99e05


BlockOpen(expected_revision) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UnchangedFrame_6e245d8d68381c13


BlockInProgress(expected_revision) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UnchangedFrame_6e245d8d68381c13


BlockBlocked(expected_revision) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UnchangedFrame_6e245d8d68381c13


RefreshEligibilityOpen(arg_unresolved_blocker_count) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ UnchangedFrame_49f64e92f6cb9fc7


RefreshEligibilityInProgress(arg_unresolved_blocker_count) ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ UnchangedFrame_49f64e92f6cb9fc7


RefreshEligibilityBlocked(arg_unresolved_blocker_count) ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ UnchangedFrame_49f64e92f6cb9fc7


ValidateLink(kind, from_item_key, to_item_key, edge_key, reverse_path_key) ==
    /\ phase = "Absent"
    /\ (from_item_key \in topology_item_keys)
    /\ (to_item_key \in topology_item_keys)
    /\ (from_item_key # to_item_key)
    /\ ((edge_key \in topology_edge_keys) = FALSE)
    /\ (IF (kind # "Blocks") THEN TRUE ELSE ((reverse_path_key \in blocks_reachability) = FALSE))
    /\ (IF (kind # "Parent") THEN TRUE ELSE ((reverse_path_key \in parent_reachability) = FALSE))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


CloseOpenCompleted(expected_revision, at_utc_ms) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ completion_policy_is_satisfied(completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UnchangedFrame_a36a4d58e8925165


CloseInProgressCompleted(expected_revision, at_utc_ms) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ completion_policy_is_satisfied(completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UnchangedFrame_a36a4d58e8925165


CloseBlockedCompleted(expected_revision, at_utc_ms) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ completion_policy_is_satisfied(completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UnchangedFrame_a36a4d58e8925165


CloseOpenCancelled(expected_revision, at_utc_ms) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UnchangedFrame_a36a4d58e8925165


CloseInProgressCancelled(expected_revision, at_utc_ms) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UnchangedFrame_a36a4d58e8925165


CloseBlockedCancelled(expected_revision, at_utc_ms) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UnchangedFrame_a36a4d58e8925165


CloseOpenFailed(expected_revision, at_utc_ms) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UnchangedFrame_a36a4d58e8925165


CloseInProgressFailed(expected_revision, at_utc_ms) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UnchangedFrame_a36a4d58e8925165


CloseBlockedFailed(expected_revision, at_utc_ms) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UnchangedFrame_a36a4d58e8925165


AddEvidenceOpen(expected_revision, evidence_kind, confirming_owner_key) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ evidence_kind_owner_key_present(evidence_kind, confirming_owner_key)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ evidence_count' = (evidence_count) + 1
    /\ host_confirmation_count' = IF (evidence_kind = "HostConfirmation") THEN (host_confirmation_count) + 1 ELSE host_confirmation_count
    /\ principal_confirmation_count' = IF (evidence_kind = "PrincipalConfirmation") THEN (principal_confirmation_count) + 1 ELSE principal_confirmation_count
    /\ supervisor_confirmation_owner_keys' = IF (evidence_kind = "SupervisorConfirmation") THEN (supervisor_confirmation_owner_keys \cup {(IF "value" \in DOMAIN confirming_owner_key THEN confirming_owner_key["value"] ELSE None)}) ELSE supervisor_confirmation_owner_keys
    /\ reviewer_confirmation_owner_keys' = IF (evidence_kind = "ReviewerConfirmation") THEN (reviewer_confirmation_owner_keys \cup {(IF "value" \in DOMAIN confirming_owner_key THEN confirming_owner_key["value"] ELSE None)}) ELSE reviewer_confirmation_owner_keys
    /\ UnchangedFrame_b10385287604008b


AddEvidenceInProgress(expected_revision, evidence_kind, confirming_owner_key) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ evidence_kind_owner_key_present(evidence_kind, confirming_owner_key)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ evidence_count' = (evidence_count) + 1
    /\ host_confirmation_count' = IF (evidence_kind = "HostConfirmation") THEN (host_confirmation_count) + 1 ELSE host_confirmation_count
    /\ principal_confirmation_count' = IF (evidence_kind = "PrincipalConfirmation") THEN (principal_confirmation_count) + 1 ELSE principal_confirmation_count
    /\ supervisor_confirmation_owner_keys' = IF (evidence_kind = "SupervisorConfirmation") THEN (supervisor_confirmation_owner_keys \cup {(IF "value" \in DOMAIN confirming_owner_key THEN confirming_owner_key["value"] ELSE None)}) ELSE supervisor_confirmation_owner_keys
    /\ reviewer_confirmation_owner_keys' = IF (evidence_kind = "ReviewerConfirmation") THEN (reviewer_confirmation_owner_keys \cup {(IF "value" \in DOMAIN confirming_owner_key THEN confirming_owner_key["value"] ELSE None)}) ELSE reviewer_confirmation_owner_keys
    /\ UnchangedFrame_b10385287604008b


AddEvidenceBlocked(expected_revision, evidence_kind, confirming_owner_key) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ evidence_kind_owner_key_present(evidence_kind, confirming_owner_key)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ evidence_count' = (evidence_count) + 1
    /\ host_confirmation_count' = IF (evidence_kind = "HostConfirmation") THEN (host_confirmation_count) + 1 ELSE host_confirmation_count
    /\ principal_confirmation_count' = IF (evidence_kind = "PrincipalConfirmation") THEN (principal_confirmation_count) + 1 ELSE principal_confirmation_count
    /\ supervisor_confirmation_owner_keys' = IF (evidence_kind = "SupervisorConfirmation") THEN (supervisor_confirmation_owner_keys \cup {(IF "value" \in DOMAIN confirming_owner_key THEN confirming_owner_key["value"] ELSE None)}) ELSE supervisor_confirmation_owner_keys
    /\ reviewer_confirmation_owner_keys' = IF (evidence_kind = "ReviewerConfirmation") THEN (reviewer_confirmation_owner_keys \cup {(IF "value" \in DOMAIN confirming_owner_key THEN confirming_owner_key["value"] ELSE None)}) ELSE reviewer_confirmation_owner_keys
    /\ UnchangedFrame_b10385287604008b


AddEvidenceCompleted(expected_revision, evidence_kind, confirming_owner_key) ==
    /\ phase = "Completed"
    /\ (revision = expected_revision)
    /\ evidence_kind_owner_key_present(evidence_kind, confirming_owner_key)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ evidence_count' = (evidence_count) + 1
    /\ host_confirmation_count' = IF (evidence_kind = "HostConfirmation") THEN (host_confirmation_count) + 1 ELSE host_confirmation_count
    /\ principal_confirmation_count' = IF (evidence_kind = "PrincipalConfirmation") THEN (principal_confirmation_count) + 1 ELSE principal_confirmation_count
    /\ supervisor_confirmation_owner_keys' = IF (evidence_kind = "SupervisorConfirmation") THEN (supervisor_confirmation_owner_keys \cup {(IF "value" \in DOMAIN confirming_owner_key THEN confirming_owner_key["value"] ELSE None)}) ELSE supervisor_confirmation_owner_keys
    /\ reviewer_confirmation_owner_keys' = IF (evidence_kind = "ReviewerConfirmation") THEN (reviewer_confirmation_owner_keys \cup {(IF "value" \in DOMAIN confirming_owner_key THEN confirming_owner_key["value"] ELSE None)}) ELSE reviewer_confirmation_owner_keys
    /\ UnchangedFrame_b10385287604008b


AddEvidenceCancelled(expected_revision, evidence_kind, confirming_owner_key) ==
    /\ phase = "Cancelled"
    /\ (revision = expected_revision)
    /\ evidence_kind_owner_key_present(evidence_kind, confirming_owner_key)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ evidence_count' = (evidence_count) + 1
    /\ host_confirmation_count' = IF (evidence_kind = "HostConfirmation") THEN (host_confirmation_count) + 1 ELSE host_confirmation_count
    /\ principal_confirmation_count' = IF (evidence_kind = "PrincipalConfirmation") THEN (principal_confirmation_count) + 1 ELSE principal_confirmation_count
    /\ supervisor_confirmation_owner_keys' = IF (evidence_kind = "SupervisorConfirmation") THEN (supervisor_confirmation_owner_keys \cup {(IF "value" \in DOMAIN confirming_owner_key THEN confirming_owner_key["value"] ELSE None)}) ELSE supervisor_confirmation_owner_keys
    /\ reviewer_confirmation_owner_keys' = IF (evidence_kind = "ReviewerConfirmation") THEN (reviewer_confirmation_owner_keys \cup {(IF "value" \in DOMAIN confirming_owner_key THEN confirming_owner_key["value"] ELSE None)}) ELSE reviewer_confirmation_owner_keys
    /\ UnchangedFrame_b10385287604008b


AddEvidenceFailed(expected_revision, evidence_kind, confirming_owner_key) ==
    /\ phase = "Failed"
    /\ (revision = expected_revision)
    /\ evidence_kind_owner_key_present(evidence_kind, confirming_owner_key)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ evidence_count' = (evidence_count) + 1
    /\ host_confirmation_count' = IF (evidence_kind = "HostConfirmation") THEN (host_confirmation_count) + 1 ELSE host_confirmation_count
    /\ principal_confirmation_count' = IF (evidence_kind = "PrincipalConfirmation") THEN (principal_confirmation_count) + 1 ELSE principal_confirmation_count
    /\ supervisor_confirmation_owner_keys' = IF (evidence_kind = "SupervisorConfirmation") THEN (supervisor_confirmation_owner_keys \cup {(IF "value" \in DOMAIN confirming_owner_key THEN confirming_owner_key["value"] ELSE None)}) ELSE supervisor_confirmation_owner_keys
    /\ reviewer_confirmation_owner_keys' = IF (evidence_kind = "ReviewerConfirmation") THEN (reviewer_confirmation_owner_keys \cup {(IF "value" \in DOMAIN confirming_owner_key THEN confirming_owner_key["value"] ELSE None)}) ELSE reviewer_confirmation_owner_keys
    /\ UnchangedFrame_b10385287604008b


ClassifyPublicErrorNotFoundAbsent(kind) ==
    /\ phase = "Absent"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorNotFoundOpen(kind) ==
    /\ phase = "Open"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorNotFoundInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorNotFoundBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorNotFoundCompleted(kind) ==
    /\ phase = "Completed"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorNotFoundCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorNotFoundFailed(kind) ==
    /\ phase = "Failed"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorConflictAbsent(kind) ==
    /\ phase = "Absent"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorConflictOpen(kind) ==
    /\ phase = "Open"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorConflictInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorConflictBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorConflictCompleted(kind) ==
    /\ phase = "Completed"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorConflictCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorConflictFailed(kind) ==
    /\ phase = "Failed"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidTransitionAbsent(kind) ==
    /\ phase = "Absent"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidTransitionOpen(kind) ==
    /\ phase = "Open"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidTransitionInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (kind = "InvalidTransition")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidTransitionBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidTransitionCompleted(kind) ==
    /\ phase = "Completed"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidTransitionCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidTransitionFailed(kind) ==
    /\ phase = "Failed"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidArgumentsAbsent(kind) ==
    /\ phase = "Absent"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidArgumentsOpen(kind) ==
    /\ phase = "Open"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidArgumentsInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidArgumentsBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidArgumentsCompleted(kind) ==
    /\ phase = "Completed"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidArgumentsCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorInvalidArgumentsFailed(kind) ==
    /\ phase = "Failed"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorCapabilityUnavailableAbsent(kind) ==
    /\ phase = "Absent"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorCapabilityUnavailableOpen(kind) ==
    /\ phase = "Open"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorCapabilityUnavailableInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorCapabilityUnavailableBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorCapabilityUnavailableCompleted(kind) ==
    /\ phase = "Completed"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorCapabilityUnavailableCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorCapabilityUnavailableFailed(kind) ==
    /\ phase = "Failed"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorStoreErrorAbsent(kind) ==
    /\ phase = "Absent"
    /\ (IF (kind = "Store") THEN TRUE ELSE (IF (kind = "BackingStoreUnavailable") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired")))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorStoreErrorOpen(kind) ==
    /\ phase = "Open"
    /\ (IF (kind = "Store") THEN TRUE ELSE (IF (kind = "BackingStoreUnavailable") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired")))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorStoreErrorInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (IF (kind = "Store") THEN TRUE ELSE (IF (kind = "BackingStoreUnavailable") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired")))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorStoreErrorBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (IF (kind = "Store") THEN TRUE ELSE (IF (kind = "BackingStoreUnavailable") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired")))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorStoreErrorCompleted(kind) ==
    /\ phase = "Completed"
    /\ (IF (kind = "Store") THEN TRUE ELSE (IF (kind = "BackingStoreUnavailable") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorStoreErrorCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (IF (kind = "Store") THEN TRUE ELSE (IF (kind = "BackingStoreUnavailable") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired")))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicErrorStoreErrorFailed(kind) ==
    /\ phase = "Failed"
    /\ (IF (kind = "Store") THEN TRUE ELSE (IF (kind = "BackingStoreUnavailable") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired")))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyTerminalityTerminalCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyTerminalityTerminalCancelled ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyTerminalityTerminalFailed ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyTerminalityLiveAbsent ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyTerminalityLiveOpen ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyTerminalityLiveInProgress ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyTerminalityLiveBlocked ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyReadinessOpenOpen(now_utc_ms, child_join_satisfied) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyReadinessInProgressInProgress(now_utc_ms, child_join_satisfied) ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyReadinessNotClaimableAbsent(now_utc_ms, child_join_satisfied) ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyReadinessNotClaimableBlocked(now_utc_ms, child_join_satisfied) ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyReadinessNotClaimableCompleted(now_utc_ms, child_join_satisfied) ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyReadinessNotClaimableCancelled(now_utc_ms, child_join_satisfied) ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyReadinessNotClaimableFailed(now_utc_ms, child_join_satisfied) ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyChildJoinAbsent(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyChildJoinOpen(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyChildJoinInProgress(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyChildJoinBlocked(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyChildJoinCompleted(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyChildJoinCancelled(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyChildJoinFailed(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyBlockerSatisfactionAbsent(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyBlockerSatisfactionOpen(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyBlockerSatisfactionInProgress(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyBlockerSatisfactionBlocked(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyBlockerSatisfactionCompleted(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyBlockerSatisfactionCancelled(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyBlockerSatisfactionFailed(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionOpenAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Open")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionOpenOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Open")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionOpenInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Open")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionOpenBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Open")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionOpenCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Open")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionOpenCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Open")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionOpenFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Open")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionBlockedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Blocked")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionBlockedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Blocked")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionBlockedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Blocked")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionBlockedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Blocked")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionBlockedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Blocked")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionBlockedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Blocked")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionBlockedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Blocked")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedAbsentAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Absent")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedAbsentOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Absent")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedAbsentInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Absent")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedAbsentBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Absent")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedAbsentCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Absent")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedAbsentCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Absent")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedAbsentFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Absent")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedInProgressAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "InProgress")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedInProgressOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "InProgress")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedInProgressInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "InProgress")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedInProgressBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "InProgress")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedInProgressCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "InProgress")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedInProgressCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "InProgress")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedInProgressFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "InProgress")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCompletedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Completed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCompletedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Completed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCompletedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Completed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCompletedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Completed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCompletedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Completed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCompletedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Completed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCompletedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Completed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCancelledAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCancelledOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCancelledInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Cancelled")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCancelledBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCancelledCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCancelledCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedCancelledFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedFailedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Failed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedFailedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Failed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedFailedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Failed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedFailedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Failed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedFailedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Failed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedFailedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Failed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateStatusAdmissionDeniedFailedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Failed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSelfAttestAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSelfAttestOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSelfAttestInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSelfAttestBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSelfAttestCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSelfAttestCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSelfAttestFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionHostConfirmedAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionHostConfirmedOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionHostConfirmedInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionHostConfirmedBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionHostConfirmedCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionHostConfirmedCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionHostConfirmedFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSupervisorAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSupervisorOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSupervisorInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSupervisorBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSupervisorCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSupervisorCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionSupervisorFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCompletedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Completed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCompletedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Completed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCompletedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Completed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCompletedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Completed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCompletedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Completed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCompletedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Completed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCompletedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Completed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCancelledAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCancelledOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCancelledInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Cancelled")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCancelledBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCancelledCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCancelledCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionCancelledFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionFailedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Failed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionFailedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Failed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionFailedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Failed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionFailedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Failed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionFailedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Failed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionFailedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Failed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionFailedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Failed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedAbsentAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Absent")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedAbsentOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Absent")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedAbsentInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Absent")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedAbsentBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Absent")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedAbsentCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Absent")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedAbsentCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Absent")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedAbsentFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Absent")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedOpenAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Open")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedOpenOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Open")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedOpenInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Open")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedOpenBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Open")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedOpenCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Open")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedOpenCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Open")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedOpenFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Open")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedInProgressAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "InProgress")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedInProgressOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "InProgress")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedInProgressInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "InProgress")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedInProgressBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "InProgress")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedInProgressCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "InProgress")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedInProgressCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "InProgress")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedInProgressFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "InProgress")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedBlockedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Blocked")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedBlockedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Blocked")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedBlockedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Blocked")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedBlockedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Blocked")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedBlockedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Blocked")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedBlockedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Blocked")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCloseStatusAdmissionDeniedBlockedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Blocked")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSelfAttestAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSelfAttestOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSelfAttestInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSelfAttestBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSelfAttestCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSelfAttestCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSelfAttestFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionHostConfirmedAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionHostConfirmedOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionHostConfirmedInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionHostConfirmedBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionHostConfirmedCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionHostConfirmedCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionHostConfirmedFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionPrincipalConfirmedAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionPrincipalConfirmedOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionPrincipalConfirmedInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionPrincipalConfirmedBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionPrincipalConfirmedCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionPrincipalConfirmedCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionPrincipalConfirmedFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSupervisorAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSupervisorOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSupervisorInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSupervisorBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSupervisorCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSupervisorCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionSupervisorFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionReviewerQuorumAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionReviewerQuorumOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionReviewerQuorumInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionReviewerQuorumBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionReviewerQuorumCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionReviewerQuorumCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyPublicConfirmationAdmissionReviewerQuorumFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionUnchangedAbsent(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Absent"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionUnchangedOpen(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Open"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionUnchangedInProgress(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "InProgress"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionUnchangedBlocked(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Blocked"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionUnchangedCompleted(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Completed"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionUnchangedCancelled(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Cancelled"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionUnchangedFailed(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Failed"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionChangedAbsent(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Absent"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionChangedOpen(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Open"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionChangedInProgress(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "InProgress"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionChangedBlocked(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Blocked"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionChangedCompleted(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Completed"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionChangedCancelled(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Cancelled"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyCompletionPolicyMutationAdmissionChangedFailed(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Failed"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayExactAbsent(requested_admission_key, requested_request_digest) ==
    /\ phase = "Absent"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest = admission_request_digest))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayExactOpen(requested_admission_key, requested_request_digest) ==
    /\ phase = "Open"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest = admission_request_digest))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayExactInProgress(requested_admission_key, requested_request_digest) ==
    /\ phase = "InProgress"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest = admission_request_digest))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayExactBlocked(requested_admission_key, requested_request_digest) ==
    /\ phase = "Blocked"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest = admission_request_digest))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayExactCompleted(requested_admission_key, requested_request_digest) ==
    /\ phase = "Completed"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest = admission_request_digest))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayExactCancelled(requested_admission_key, requested_request_digest) ==
    /\ phase = "Cancelled"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest = admission_request_digest))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayExactFailed(requested_admission_key, requested_request_digest) ==
    /\ phase = "Failed"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest = admission_request_digest))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayConflictAbsent(requested_admission_key, requested_request_digest) ==
    /\ phase = "Absent"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest # admission_request_digest))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayConflictOpen(requested_admission_key, requested_request_digest) ==
    /\ phase = "Open"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest # admission_request_digest))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayConflictInProgress(requested_admission_key, requested_request_digest) ==
    /\ phase = "InProgress"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest # admission_request_digest))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayConflictBlocked(requested_admission_key, requested_request_digest) ==
    /\ phase = "Blocked"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest # admission_request_digest))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayConflictCompleted(requested_admission_key, requested_request_digest) ==
    /\ phase = "Completed"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest # admission_request_digest))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayConflictCancelled(requested_admission_key, requested_request_digest) ==
    /\ phase = "Cancelled"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest # admission_request_digest))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayConflictFailed(requested_admission_key, requested_request_digest) ==
    /\ phase = "Failed"
    /\ ((requested_admission_key # None) /\ (requested_admission_key = admission_key) /\ (requested_request_digest # admission_request_digest))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayKeyMismatchAbsent(requested_admission_key, requested_request_digest) ==
    /\ phase = "Absent"
    /\ (IF (requested_admission_key = None) THEN TRUE ELSE (requested_admission_key # admission_key))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayKeyMismatchOpen(requested_admission_key, requested_request_digest) ==
    /\ phase = "Open"
    /\ (IF (requested_admission_key = None) THEN TRUE ELSE (requested_admission_key # admission_key))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayKeyMismatchInProgress(requested_admission_key, requested_request_digest) ==
    /\ phase = "InProgress"
    /\ (IF (requested_admission_key = None) THEN TRUE ELSE (requested_admission_key # admission_key))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayKeyMismatchBlocked(requested_admission_key, requested_request_digest) ==
    /\ phase = "Blocked"
    /\ (IF (requested_admission_key = None) THEN TRUE ELSE (requested_admission_key # admission_key))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayKeyMismatchCompleted(requested_admission_key, requested_request_digest) ==
    /\ phase = "Completed"
    /\ (IF (requested_admission_key = None) THEN TRUE ELSE (requested_admission_key # admission_key))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayKeyMismatchCancelled(requested_admission_key, requested_request_digest) ==
    /\ phase = "Cancelled"
    /\ (IF (requested_admission_key = None) THEN TRUE ELSE (requested_admission_key # admission_key))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyAdmissionReplayKeyMismatchFailed(requested_admission_key, requested_request_digest) ==
    /\ phase = "Failed"
    /\ (IF (requested_admission_key = None) THEN TRUE ELSE (requested_admission_key # admission_key))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalRequiredAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalRequiredOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalRequiredInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalRequiredBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalRequiredCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalRequiredCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalRequiredFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalKindMismatchAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalKindMismatchOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalKindMismatchInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalKindMismatchBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalKindMismatchCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalKindMismatchCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionPrincipalKindMismatchFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSupervisorMismatchAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSupervisorMismatchOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSupervisorMismatchInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSupervisorMismatchBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSupervisorMismatchCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSupervisorMismatchCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSupervisorMismatchFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSelfAttestEmptyAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSelfAttestEmptyOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSelfAttestEmptyInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSelfAttestEmptyBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSelfAttestEmptyCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSelfAttestEmptyCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionSelfAttestEmptyFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionEvidenceKindAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionEvidenceKindOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionEvidenceKindInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionEvidenceKindBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionEvidenceKindCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionEvidenceKindCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionEvidenceKindFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionAdmittedAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionAdmittedOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionAdmittedInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionAdmittedBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionAdmittedCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionAdmittedCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


ClassifyConfirmationAdmissionAdmittedFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_8d339dd77aca8942


Next ==
    \/ \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : \E arg_failed_child_join_policy \in FailedChildJoinPolicyValues : \E arg_cancelled_child_join_policy \in CancelledChildJoinPolicyValues : \E arg_admission_key \in OptionWorkAdmissionKeyRefValues : \E arg_admission_request_digest \in OptionWorkAdmissionDigestRefValues : CreateOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count, arg_failed_child_join_policy, arg_cancelled_child_join_policy, arg_admission_key, arg_admission_request_digest)
    \/ \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : \E arg_failed_child_join_policy \in FailedChildJoinPolicyValues : \E arg_cancelled_child_join_policy \in CancelledChildJoinPolicyValues : \E arg_admission_key \in OptionWorkAdmissionKeyRefValues : \E arg_admission_request_digest \in OptionWorkAdmissionDigestRefValues : CreateBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count, arg_failed_child_join_policy, arg_cancelled_child_join_policy, arg_admission_key, arg_admission_request_digest)
    \/ \E expected_revision \in {revision} : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : UpdateOpen(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E expected_revision \in {revision} : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : UpdateInProgress(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E expected_revision \in {revision} : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : UpdateBlocked(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E expected_revision \in {revision} : \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : PolicyEscalateOpenAdmitted(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E expected_revision \in {revision} : \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : PolicyEscalateOpenDenied(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E expected_revision \in {revision} : \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : PolicyEscalateInProgressAdmitted(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E expected_revision \in {revision} : \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : PolicyEscalateInProgressDenied(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E expected_revision \in {revision} : \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : PolicyEscalateBlockedAdmitted(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E expected_revision \in {revision} : \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : PolicyEscalateBlockedDenied(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E expected_revision \in {revision} : \E owner_key \in WorkOwnerKeyValues : \E now_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in OptionU64Values : ClaimOpen(expected_revision, owner_key, now_utc_ms, arg_lease_expires_at_utc_ms, TRUE)
    \/ \E expected_revision \in {revision} : \E owner_key \in WorkOwnerKeyValues : \E now_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in OptionU64Values : ClaimExpiredInProgress(expected_revision, owner_key, now_utc_ms, arg_lease_expires_at_utc_ms, TRUE)
    \/ \E expected_revision \in {revision} : ReleaseInProgress(expected_revision)
    \/ \E expected_revision \in {revision} : \E observed_at_utc_ms \in 0..2 : ObserveLeaseExpiryInProgress(expected_revision, observed_at_utc_ms)
    \/ \E expected_revision \in {revision} : \E observed_at_utc_ms \in 0..2 : ObserveReadinessOpen(expected_revision, observed_at_utc_ms, TRUE)
    \/ \E expected_revision \in {revision} : BlockOpen(expected_revision)
    \/ \E expected_revision \in {revision} : BlockInProgress(expected_revision)
    \/ \E expected_revision \in {revision} : BlockBlocked(expected_revision)
    \/ \E arg_unresolved_blocker_count \in 0..2 : RefreshEligibilityOpen(arg_unresolved_blocker_count)
    \/ \E arg_unresolved_blocker_count \in 0..2 : RefreshEligibilityInProgress(arg_unresolved_blocker_count)
    \/ \E arg_unresolved_blocker_count \in 0..2 : RefreshEligibilityBlocked(arg_unresolved_blocker_count)
    \/ \E kind \in WorkEdgeKindValues : \E from_item_key \in WorkItemKeyValues : \E to_item_key \in WorkItemKeyValues : \E edge_key \in WorkEdgeKeyValues : \E reverse_path_key \in WorkDependencyPathKeyValues : ValidateLink(kind, from_item_key, to_item_key, edge_key, reverse_path_key)
    \/ \E expected_revision \in {revision} : \E at_utc_ms \in 0..2 : CloseOpenCompleted(expected_revision, at_utc_ms)
    \/ \E expected_revision \in {revision} : \E at_utc_ms \in 0..2 : CloseInProgressCompleted(expected_revision, at_utc_ms)
    \/ \E expected_revision \in {revision} : \E at_utc_ms \in 0..2 : CloseBlockedCompleted(expected_revision, at_utc_ms)
    \/ \E expected_revision \in {revision} : \E at_utc_ms \in 0..2 : CloseOpenCancelled(expected_revision, at_utc_ms)
    \/ \E expected_revision \in {revision} : \E at_utc_ms \in 0..2 : CloseInProgressCancelled(expected_revision, at_utc_ms)
    \/ \E expected_revision \in {revision} : \E at_utc_ms \in 0..2 : CloseBlockedCancelled(expected_revision, at_utc_ms)
    \/ \E expected_revision \in {revision} : \E at_utc_ms \in 0..2 : CloseOpenFailed(expected_revision, at_utc_ms)
    \/ \E expected_revision \in {revision} : \E at_utc_ms \in 0..2 : CloseInProgressFailed(expected_revision, at_utc_ms)
    \/ \E expected_revision \in {revision} : \E at_utc_ms \in 0..2 : CloseBlockedFailed(expected_revision, at_utc_ms)
    \/ \E expected_revision \in {revision} : \E evidence_kind \in WorkEvidenceKindValues : \E confirming_owner_key \in OptionWorkOwnerKeyValues : AddEvidenceOpen(expected_revision, evidence_kind, confirming_owner_key)
    \/ \E expected_revision \in {revision} : \E evidence_kind \in WorkEvidenceKindValues : \E confirming_owner_key \in OptionWorkOwnerKeyValues : AddEvidenceInProgress(expected_revision, evidence_kind, confirming_owner_key)
    \/ \E expected_revision \in {revision} : \E evidence_kind \in WorkEvidenceKindValues : \E confirming_owner_key \in OptionWorkOwnerKeyValues : AddEvidenceBlocked(expected_revision, evidence_kind, confirming_owner_key)
    \/ \E expected_revision \in {revision} : \E evidence_kind \in WorkEvidenceKindValues : \E confirming_owner_key \in OptionWorkOwnerKeyValues : AddEvidenceCompleted(expected_revision, evidence_kind, confirming_owner_key)
    \/ \E expected_revision \in {revision} : \E evidence_kind \in WorkEvidenceKindValues : \E confirming_owner_key \in OptionWorkOwnerKeyValues : AddEvidenceCancelled(expected_revision, evidence_kind, confirming_owner_key)
    \/ \E expected_revision \in {revision} : \E evidence_kind \in WorkEvidenceKindValues : \E confirming_owner_key \in OptionWorkOwnerKeyValues : AddEvidenceFailed(expected_revision, evidence_kind, confirming_owner_key)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorNotFoundAbsent(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorNotFoundOpen(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorNotFoundInProgress(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorNotFoundBlocked(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorNotFoundCompleted(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorNotFoundCancelled(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorNotFoundFailed(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorConflictAbsent(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorConflictOpen(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorConflictInProgress(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorConflictBlocked(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorConflictCompleted(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorConflictCancelled(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorConflictFailed(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidTransitionAbsent(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidTransitionOpen(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidTransitionInProgress(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidTransitionBlocked(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidTransitionCompleted(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidTransitionCancelled(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidTransitionFailed(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidArgumentsAbsent(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidArgumentsOpen(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidArgumentsInProgress(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidArgumentsBlocked(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidArgumentsCompleted(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidArgumentsCancelled(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidArgumentsFailed(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorCapabilityUnavailableAbsent(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorCapabilityUnavailableOpen(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorCapabilityUnavailableInProgress(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorCapabilityUnavailableBlocked(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorCapabilityUnavailableCompleted(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorCapabilityUnavailableCancelled(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorCapabilityUnavailableFailed(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorStoreErrorAbsent(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorStoreErrorOpen(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorStoreErrorInProgress(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorStoreErrorBlocked(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorStoreErrorCompleted(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorStoreErrorCancelled(kind)
    \/ \E kind \in WorkGraphErrorKindValues : ClassifyPublicErrorStoreErrorFailed(kind)
    \/ ClassifyTerminalityTerminalCompleted
    \/ ClassifyTerminalityTerminalCancelled
    \/ ClassifyTerminalityTerminalFailed
    \/ ClassifyTerminalityLiveAbsent
    \/ ClassifyTerminalityLiveOpen
    \/ ClassifyTerminalityLiveInProgress
    \/ ClassifyTerminalityLiveBlocked
    \/ \E now_utc_ms \in 0..2 : \E child_join_satisfied \in BOOLEAN : ClassifyReadinessOpenOpen(now_utc_ms, child_join_satisfied)
    \/ \E now_utc_ms \in 0..2 : \E child_join_satisfied \in BOOLEAN : ClassifyReadinessInProgressInProgress(now_utc_ms, child_join_satisfied)
    \/ \E now_utc_ms \in 0..2 : \E child_join_satisfied \in BOOLEAN : ClassifyReadinessNotClaimableAbsent(now_utc_ms, child_join_satisfied)
    \/ \E now_utc_ms \in 0..2 : \E child_join_satisfied \in BOOLEAN : ClassifyReadinessNotClaimableBlocked(now_utc_ms, child_join_satisfied)
    \/ \E now_utc_ms \in 0..2 : \E child_join_satisfied \in BOOLEAN : ClassifyReadinessNotClaimableCompleted(now_utc_ms, child_join_satisfied)
    \/ \E now_utc_ms \in 0..2 : \E child_join_satisfied \in BOOLEAN : ClassifyReadinessNotClaimableCancelled(now_utc_ms, child_join_satisfied)
    \/ \E now_utc_ms \in 0..2 : \E child_join_satisfied \in BOOLEAN : ClassifyReadinessNotClaimableFailed(now_utc_ms, child_join_satisfied)
    \/ \E active_child_count \in 0..2 : \E failed_child_count \in 0..2 : \E cancelled_child_count \in 0..2 : ClassifyChildJoinAbsent(active_child_count, failed_child_count, cancelled_child_count)
    \/ \E active_child_count \in 0..2 : \E failed_child_count \in 0..2 : \E cancelled_child_count \in 0..2 : ClassifyChildJoinOpen(active_child_count, failed_child_count, cancelled_child_count)
    \/ \E active_child_count \in 0..2 : \E failed_child_count \in 0..2 : \E cancelled_child_count \in 0..2 : ClassifyChildJoinInProgress(active_child_count, failed_child_count, cancelled_child_count)
    \/ \E active_child_count \in 0..2 : \E failed_child_count \in 0..2 : \E cancelled_child_count \in 0..2 : ClassifyChildJoinBlocked(active_child_count, failed_child_count, cancelled_child_count)
    \/ \E active_child_count \in 0..2 : \E failed_child_count \in 0..2 : \E cancelled_child_count \in 0..2 : ClassifyChildJoinCompleted(active_child_count, failed_child_count, cancelled_child_count)
    \/ \E active_child_count \in 0..2 : \E failed_child_count \in 0..2 : \E cancelled_child_count \in 0..2 : ClassifyChildJoinCancelled(active_child_count, failed_child_count, cancelled_child_count)
    \/ \E active_child_count \in 0..2 : \E failed_child_count \in 0..2 : \E cancelled_child_count \in 0..2 : ClassifyChildJoinFailed(active_child_count, failed_child_count, cancelled_child_count)
    \/ \E blocker_present \in BOOLEAN : \E blocker_lifecycle_phase \in WorkLifecycleStateValues : ClassifyBlockerSatisfactionAbsent(blocker_present, blocker_lifecycle_phase)
    \/ \E blocker_present \in BOOLEAN : \E blocker_lifecycle_phase \in WorkLifecycleStateValues : ClassifyBlockerSatisfactionOpen(blocker_present, blocker_lifecycle_phase)
    \/ \E blocker_present \in BOOLEAN : \E blocker_lifecycle_phase \in WorkLifecycleStateValues : ClassifyBlockerSatisfactionInProgress(blocker_present, blocker_lifecycle_phase)
    \/ \E blocker_present \in BOOLEAN : \E blocker_lifecycle_phase \in WorkLifecycleStateValues : ClassifyBlockerSatisfactionBlocked(blocker_present, blocker_lifecycle_phase)
    \/ \E blocker_present \in BOOLEAN : \E blocker_lifecycle_phase \in WorkLifecycleStateValues : ClassifyBlockerSatisfactionCompleted(blocker_present, blocker_lifecycle_phase)
    \/ \E blocker_present \in BOOLEAN : \E blocker_lifecycle_phase \in WorkLifecycleStateValues : ClassifyBlockerSatisfactionCancelled(blocker_present, blocker_lifecycle_phase)
    \/ \E blocker_present \in BOOLEAN : \E blocker_lifecycle_phase \in WorkLifecycleStateValues : ClassifyBlockerSatisfactionFailed(blocker_present, blocker_lifecycle_phase)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionOpenAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionOpenOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionOpenInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionOpenBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionOpenCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionOpenCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionOpenFailed(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionBlockedAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionBlockedOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionBlockedInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionBlockedBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionBlockedCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionBlockedCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionBlockedFailed(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedAbsentAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedAbsentOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedAbsentInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedAbsentBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedAbsentCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedAbsentCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedAbsentFailed(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedInProgressAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedInProgressOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedInProgressInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedInProgressBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedInProgressCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedInProgressCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedInProgressFailed(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCompletedAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCompletedOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCompletedInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCompletedBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCompletedCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCompletedCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCompletedFailed(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCancelledAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCancelledOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCancelledInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCancelledBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCancelledCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCancelledCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedCancelledFailed(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedFailedAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedFailedOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedFailedInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedFailedBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedFailedCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedFailedCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCreateStatusAdmissionDeniedFailedFailed(requested_status)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSelfAttestAbsent(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSelfAttestOpen(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSelfAttestInProgress(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSelfAttestBlocked(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSelfAttestCompleted(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSelfAttestCancelled(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSelfAttestFailed(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionHostConfirmedAbsent(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionHostConfirmedOpen(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionHostConfirmedInProgress(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionHostConfirmedBlocked(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionHostConfirmedCompleted(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionHostConfirmedCancelled(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionHostConfirmedFailed(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedAbsent(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedOpen(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedInProgress(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedBlocked(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedCompleted(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedCancelled(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedFailed(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSupervisorAbsent(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSupervisorOpen(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSupervisorInProgress(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSupervisorBlocked(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSupervisorCompleted(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSupervisorCancelled(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionSupervisorFailed(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionReviewerQuorumAbsent(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionReviewerQuorumOpen(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionReviewerQuorumInProgress(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionReviewerQuorumBlocked(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionReviewerQuorumCompleted(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionReviewerQuorumCancelled(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyCreateCompletionPolicyAdmissionReviewerQuorumFailed(arg_completion_policy)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCompletedAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCompletedOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCompletedInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCompletedBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCompletedCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCompletedCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCompletedFailed(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCancelledAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCancelledOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCancelledInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCancelledBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCancelledCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCancelledCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionCancelledFailed(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionFailedAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionFailedOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionFailedInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionFailedBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionFailedCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionFailedCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionFailedFailed(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedAbsentAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedAbsentOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedAbsentInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedAbsentBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedAbsentCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedAbsentCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedAbsentFailed(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedOpenAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedOpenOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedOpenInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedOpenBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedOpenCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedOpenCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedOpenFailed(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedInProgressAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedInProgressOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedInProgressInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedInProgressBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedInProgressCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedInProgressCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedInProgressFailed(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedBlockedAbsent(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedBlockedOpen(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedBlockedInProgress(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedBlockedBlocked(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedBlockedCompleted(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedBlockedCancelled(requested_status)
    \/ \E requested_status \in WorkLifecycleStateValues : ClassifyCloseStatusAdmissionDeniedBlockedFailed(requested_status)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSelfAttestAbsent(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSelfAttestOpen(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSelfAttestInProgress(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSelfAttestBlocked(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSelfAttestCompleted(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSelfAttestCancelled(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSelfAttestFailed(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionHostConfirmedAbsent(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionHostConfirmedOpen(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionHostConfirmedInProgress(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionHostConfirmedBlocked(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionHostConfirmedCompleted(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionHostConfirmedCancelled(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionHostConfirmedFailed(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionPrincipalConfirmedAbsent(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionPrincipalConfirmedOpen(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionPrincipalConfirmedInProgress(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionPrincipalConfirmedBlocked(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionPrincipalConfirmedCompleted(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionPrincipalConfirmedCancelled(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionPrincipalConfirmedFailed(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSupervisorAbsent(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSupervisorOpen(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSupervisorInProgress(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSupervisorBlocked(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSupervisorCompleted(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSupervisorCancelled(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionSupervisorFailed(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionReviewerQuorumAbsent(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionReviewerQuorumOpen(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionReviewerQuorumInProgress(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionReviewerQuorumBlocked(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionReviewerQuorumCompleted(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionReviewerQuorumCancelled(arg_completion_policy)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : ClassifyPublicConfirmationAdmissionReviewerQuorumFailed(arg_completion_policy)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionUnchangedAbsent(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionUnchangedOpen(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionUnchangedInProgress(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionUnchangedBlocked(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionUnchangedCompleted(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionUnchangedCancelled(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionUnchangedFailed(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionChangedAbsent(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionChangedOpen(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionChangedInProgress(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionChangedBlocked(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionChangedCompleted(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionChangedCancelled(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_completion_policy \in WorkCompletionPolicyValues : \E requested_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_completion_reviewer_quorum_threshold \in OptionU64Values : ClassifyCompletionPolicyMutationAdmissionChangedFailed(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayExactAbsent(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayExactOpen(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayExactInProgress(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayExactBlocked(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayExactCompleted(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayExactCancelled(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayExactFailed(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayConflictAbsent(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayConflictOpen(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayConflictInProgress(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayConflictBlocked(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayConflictCompleted(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayConflictCancelled(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayConflictFailed(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayKeyMismatchAbsent(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayKeyMismatchOpen(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayKeyMismatchInProgress(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayKeyMismatchBlocked(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayKeyMismatchCompleted(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayKeyMismatchCancelled(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in OptionWorkAdmissionKeyRefValues : \E requested_request_digest \in OptionWorkAdmissionDigestRefValues : ClassifyAdmissionReplayKeyMismatchFailed(requested_admission_key, requested_request_digest)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalRequiredAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalRequiredOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalRequiredInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalRequiredBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalRequiredCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalRequiredCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalRequiredFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalKindMismatchAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalKindMismatchOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalKindMismatchInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalKindMismatchBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalKindMismatchCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalKindMismatchCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionPrincipalKindMismatchFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSupervisorMismatchAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSupervisorMismatchOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSupervisorMismatchInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSupervisorMismatchBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSupervisorMismatchCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSupervisorMismatchCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSupervisorMismatchFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSelfAttestEmptyAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSelfAttestEmptyOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSelfAttestEmptyInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSelfAttestEmptyBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSelfAttestEmptyCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSelfAttestEmptyCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionSelfAttestEmptyFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionEvidenceKindAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionEvidenceKindOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionEvidenceKindInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionEvidenceKindBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionEvidenceKindCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionEvidenceKindCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionEvidenceKindFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionAdmittedAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionAdmittedOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionAdmittedInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionAdmittedBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionAdmittedCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionAdmittedCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_owner_key \in OptionWorkOwnerKeyValues : \E requested_principal_kind \in OptionWorkOwnerKindValues : \E supplied_evidence_kind \in WorkConfirmationEvidenceObservationValues : ClassifyConfirmationAdmissionAdmittedFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    \/ TerminalStutter

absent_has_zero_revision == (IF (phase # "Absent") THEN TRUE ELSE (revision = 0))
live_has_positive_revision == (IF (phase = "Absent") THEN TRUE ELSE (revision > 0))
topology_snapshot_is_stateless == (IF (topology_item_keys = {}) THEN TRUE ELSE (IF (topology_edge_keys = {}) THEN TRUE ELSE (phase = "Absent")))
terminal_has_terminal_time == (IF ((phase # "Completed") /\ (phase # "Cancelled") /\ (phase # "Failed")) THEN TRUE ELSE (terminal_at_utc_ms # None))
claim_only_in_progress == (IF (claim_owner_key = None) THEN TRUE ELSE (phase = "InProgress"))
blocked_has_no_claim == (IF (phase # "Blocked") THEN TRUE ELSE (claim_owner_key = None))
admission_identity_paired == (IF ((admission_key = None) /\ (admission_request_digest = None)) THEN TRUE ELSE ((admission_key # None) /\ (admission_request_digest # None)))
absent_has_no_admission_identity == (IF (phase # "Absent") THEN TRUE ELSE (admission_key = None))
terminal_has_no_claim == (IF ((phase # "Completed") /\ (phase # "Cancelled") /\ (phase # "Failed")) THEN TRUE ELSE (claim_owner_key = None))
supervisor_policy_has_owner == (IF (completion_policy # "Supervisor") THEN TRUE ELSE (completion_supervisor_owner_key # None))
non_supervisor_policy_has_no_owner == (IF (completion_policy = "Supervisor") THEN TRUE ELSE (completion_supervisor_owner_key = None))
reviewer_quorum_policy_has_positive_threshold == (IF (completion_policy # "ReviewerQuorum") THEN TRUE ELSE ((completion_reviewer_quorum_threshold # None) /\ ((IF "value" \in DOMAIN completion_reviewer_quorum_threshold THEN completion_reviewer_quorum_threshold["value"] ELSE None) > 0)))
non_reviewer_quorum_policy_has_no_threshold == (IF (completion_policy = "ReviewerQuorum") THEN TRUE ELSE (completion_reviewer_quorum_threshold = None))

CiStateConstraint == /\ model_step_count <= 5 /\ Cardinality(topology_item_keys) <= 1 /\ Cardinality(topology_edge_keys) <= 1 /\ Cardinality(blocks_reachability) <= 1 /\ Cardinality(parent_reachability) <= 1 /\ Cardinality(supervisor_confirmation_owner_keys) <= 1 /\ Cardinality(reviewer_confirmation_owner_keys) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(topology_item_keys) <= 2 /\ Cardinality(topology_edge_keys) <= 2 /\ Cardinality(blocks_reachability) <= 2 /\ Cardinality(parent_reachability) <= 2 /\ Cardinality(supervisor_confirmation_owner_keys) <= 2 /\ Cardinality(reviewer_confirmation_owner_keys) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []absent_has_zero_revision
THEOREM Spec => []live_has_positive_revision
THEOREM Spec => []topology_snapshot_is_stateless
THEOREM Spec => []terminal_has_terminal_time
THEOREM Spec => []claim_only_in_progress
THEOREM Spec => []blocked_has_no_claim
THEOREM Spec => []admission_identity_paired
THEOREM Spec => []absent_has_no_admission_identity
THEOREM Spec => []terminal_has_no_claim
THEOREM Spec => []supervisor_policy_has_owner
THEOREM Spec => []non_supervisor_policy_has_no_owner
THEOREM Spec => []reviewer_quorum_policy_has_positive_threshold
THEOREM Spec => []non_reviewer_quorum_policy_has_no_threshold

=============================================================================
