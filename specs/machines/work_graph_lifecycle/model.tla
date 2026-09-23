---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for WorkGraphLifecycleMachine.

CONSTANTS BooleanValues, CancelledChildJoinPolicyValues, ChildJoinDispositionValues, FailedChildJoinPolicyValues, NatValues, SetOfWorkDependencyPathKeyValues, SetOfWorkEdgeKeyValues, SetOfWorkItemKeyValues, SetOfWorkOwnerKeyValues, WorkCloseStatusAdmissionKindValues, WorkCompletionPolicyMutationAdmissionKindValues, WorkCompletionPolicyValues, WorkConfirmationAdmissionKindValues, WorkConfirmationEvidenceObservationValues, WorkCreateCompletionPolicyAdmissionKindValues, WorkCreateStatusAdmissionKindValues, WorkDependencyPathKeyValues, WorkEdgeKeyValues, WorkEdgeKindValues, WorkEvidenceKindValues, WorkGraphErrorKindValues, WorkGraphPublicErrorClassValues, WorkItemKeyValues, WorkLifecycleStateValues, WorkOwnerKeyValues, WorkOwnerKindValues, WorkPolicyEscalationAdmissionKindValues, WorkPublicConfirmationAdmissionKindValues

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

VARIABLES phase, model_step_count, revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy

vars == << phase, model_step_count, revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy >>

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

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Cancelled" \/ phase = "Failed"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_11dfc16157be893f == UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy >>
UnchangedFrame_20118334a59dd68e == UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy >>
UnchangedFrame_2c72a9c9f706d66e == UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy >>
UnchangedFrame_624154d7d0ffe604 == UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy >>
UnchangedFrame_96e0469c31b81d90 == UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy >>
UnchangedFrame_cb7c9ec2829fe2f8 == UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy >>
UnchangedFrame_cdb06b1cc475a560 == UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, failed_child_join_policy, cancelled_child_join_policy >>
UnchangedFrame_d98d5f8c941e6bc0 == UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys, failed_child_join_policy, cancelled_child_join_policy >>
UnchangedFrame_ea30709c66621d98 == UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>

CreateOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count, arg_failed_child_join_policy, arg_cancelled_child_join_policy) ==
    /\ phase = "Absent"
    /\ completion_policy_payload_valid(arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold)
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
    /\ UnchangedFrame_ea30709c66621d98


CreateBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count, arg_failed_child_join_policy, arg_cancelled_child_join_policy) ==
    /\ phase = "Absent"
    /\ completion_policy_payload_valid(arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold)
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
    /\ UnchangedFrame_d98d5f8c941e6bc0


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
    /\ UnchangedFrame_d98d5f8c941e6bc0


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
    /\ UnchangedFrame_d98d5f8c941e6bc0


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
    /\ UnchangedFrame_96e0469c31b81d90


PolicyEscalateOpenDenied(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ (completion_policy_escalation_admissible(completion_policy, completion_reviewer_quorum_threshold, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) = FALSE)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


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
    /\ UnchangedFrame_96e0469c31b81d90


PolicyEscalateInProgressDenied(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ (completion_policy_escalation_admissible(completion_policy, completion_reviewer_quorum_threshold, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) = FALSE)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


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
    /\ UnchangedFrame_96e0469c31b81d90


PolicyEscalateBlockedDenied(expected_revision, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ (completion_policy_escalation_admissible(completion_policy, completion_reviewer_quorum_threshold, requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) = FALSE)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


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
    /\ UnchangedFrame_cb7c9ec2829fe2f8


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
    /\ UnchangedFrame_cb7c9ec2829fe2f8


ReleaseInProgress(expected_revision) ==
    /\ phase = "InProgress"
    /\ ((revision = expected_revision) /\ (claim_owner_key # None))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UnchangedFrame_cb7c9ec2829fe2f8


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
    /\ UnchangedFrame_cb7c9ec2829fe2f8


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
    /\ UnchangedFrame_2c72a9c9f706d66e


BlockOpen(expected_revision) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UnchangedFrame_cb7c9ec2829fe2f8


BlockInProgress(expected_revision) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UnchangedFrame_cb7c9ec2829fe2f8


BlockBlocked(expected_revision) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UnchangedFrame_cb7c9ec2829fe2f8


RefreshEligibilityOpen(arg_unresolved_blocker_count) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ UnchangedFrame_624154d7d0ffe604


RefreshEligibilityInProgress(arg_unresolved_blocker_count) ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ UnchangedFrame_624154d7d0ffe604


RefreshEligibilityBlocked(arg_unresolved_blocker_count) ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ UnchangedFrame_624154d7d0ffe604


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
    /\ UnchangedFrame_11dfc16157be893f


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
    /\ UnchangedFrame_20118334a59dd68e


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
    /\ UnchangedFrame_20118334a59dd68e


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
    /\ UnchangedFrame_20118334a59dd68e


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
    /\ UnchangedFrame_20118334a59dd68e


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
    /\ UnchangedFrame_20118334a59dd68e


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
    /\ UnchangedFrame_20118334a59dd68e


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
    /\ UnchangedFrame_20118334a59dd68e


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
    /\ UnchangedFrame_20118334a59dd68e


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
    /\ UnchangedFrame_20118334a59dd68e


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
    /\ UnchangedFrame_cdb06b1cc475a560


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
    /\ UnchangedFrame_cdb06b1cc475a560


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
    /\ UnchangedFrame_cdb06b1cc475a560


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
    /\ UnchangedFrame_cdb06b1cc475a560


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
    /\ UnchangedFrame_cdb06b1cc475a560


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
    /\ UnchangedFrame_cdb06b1cc475a560


ClassifyPublicErrorNotFoundAbsent(kind) ==
    /\ phase = "Absent"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorNotFoundOpen(kind) ==
    /\ phase = "Open"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorNotFoundInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorNotFoundBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorNotFoundCompleted(kind) ==
    /\ phase = "Completed"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorNotFoundCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorNotFoundFailed(kind) ==
    /\ phase = "Failed"
    /\ (IF (kind = "NotFound") THEN TRUE ELSE (kind = "AttentionNotFound"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorConflictAbsent(kind) ==
    /\ phase = "Absent"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorConflictOpen(kind) ==
    /\ phase = "Open"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorConflictInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorConflictBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorConflictCompleted(kind) ==
    /\ phase = "Completed"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorConflictCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorConflictFailed(kind) ==
    /\ phase = "Failed"
    /\ (IF (kind = "StaleRevision") THEN TRUE ELSE (kind = "Conflict"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidTransitionAbsent(kind) ==
    /\ phase = "Absent"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidTransitionOpen(kind) ==
    /\ phase = "Open"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidTransitionInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (kind = "InvalidTransition")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidTransitionBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidTransitionCompleted(kind) ==
    /\ phase = "Completed"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidTransitionCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidTransitionFailed(kind) ==
    /\ phase = "Failed"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidArgumentsAbsent(kind) ==
    /\ phase = "Absent"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidArgumentsOpen(kind) ==
    /\ phase = "Open"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidArgumentsInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidArgumentsBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidArgumentsCompleted(kind) ==
    /\ phase = "Completed"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidArgumentsCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorInvalidArgumentsFailed(kind) ==
    /\ phase = "Failed"
    /\ (IF (kind = "InvalidInput") THEN TRUE ELSE (IF (kind = "InvalidTimestampMillis") THEN TRUE ELSE (kind = "AttentionTargetRealmMismatch")))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorCapabilityUnavailableAbsent(kind) ==
    /\ phase = "Absent"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorCapabilityUnavailableOpen(kind) ==
    /\ phase = "Open"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorCapabilityUnavailableInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorCapabilityUnavailableBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorCapabilityUnavailableCompleted(kind) ==
    /\ phase = "Completed"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorCapabilityUnavailableCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorCapabilityUnavailableFailed(kind) ==
    /\ phase = "Failed"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorStoreErrorAbsent(kind) ==
    /\ phase = "Absent"
    /\ (IF (kind = "Store") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired"))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorStoreErrorOpen(kind) ==
    /\ phase = "Open"
    /\ (IF (kind = "Store") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired"))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorStoreErrorInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (IF (kind = "Store") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired"))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorStoreErrorBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (IF (kind = "Store") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired"))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorStoreErrorCompleted(kind) ==
    /\ phase = "Completed"
    /\ (IF (kind = "Store") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired"))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorStoreErrorCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (IF (kind = "Store") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired"))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicErrorStoreErrorFailed(kind) ==
    /\ phase = "Failed"
    /\ (IF (kind = "Store") THEN TRUE ELSE (kind = "NamespaceAssignmentRequired"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyTerminalityTerminalCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyTerminalityTerminalCancelled ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyTerminalityTerminalFailed ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyTerminalityLiveAbsent ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyTerminalityLiveOpen ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyTerminalityLiveInProgress ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyTerminalityLiveBlocked ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyReadinessOpenOpen(now_utc_ms, child_join_satisfied) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyReadinessInProgressInProgress(now_utc_ms, child_join_satisfied) ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyReadinessNotClaimableAbsent(now_utc_ms, child_join_satisfied) ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyReadinessNotClaimableBlocked(now_utc_ms, child_join_satisfied) ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyReadinessNotClaimableCompleted(now_utc_ms, child_join_satisfied) ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyReadinessNotClaimableCancelled(now_utc_ms, child_join_satisfied) ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyReadinessNotClaimableFailed(now_utc_ms, child_join_satisfied) ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyChildJoinAbsent(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyChildJoinOpen(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyChildJoinInProgress(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyChildJoinBlocked(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyChildJoinCompleted(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyChildJoinCancelled(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyChildJoinFailed(active_child_count, failed_child_count, cancelled_child_count) ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyBlockerSatisfactionAbsent(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyBlockerSatisfactionOpen(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyBlockerSatisfactionInProgress(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyBlockerSatisfactionBlocked(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyBlockerSatisfactionCompleted(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyBlockerSatisfactionCancelled(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyBlockerSatisfactionFailed(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionOpenAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Open")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionOpenOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Open")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionOpenInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Open")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionOpenBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Open")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionOpenCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Open")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionOpenCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Open")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionOpenFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Open")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionBlockedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Blocked")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionBlockedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Blocked")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionBlockedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Blocked")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionBlockedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Blocked")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionBlockedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Blocked")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionBlockedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Blocked")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionBlockedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Blocked")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedAbsentAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Absent")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedAbsentOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Absent")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedAbsentInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Absent")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedAbsentBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Absent")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedAbsentCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Absent")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedAbsentCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Absent")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedAbsentFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Absent")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedInProgressAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "InProgress")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedInProgressOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "InProgress")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedInProgressInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "InProgress")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedInProgressBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "InProgress")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedInProgressCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "InProgress")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedInProgressCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "InProgress")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedInProgressFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "InProgress")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCompletedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Completed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCompletedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Completed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCompletedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Completed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCompletedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Completed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCompletedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Completed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCompletedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Completed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCompletedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Completed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCancelledAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCancelledOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCancelledInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Cancelled")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCancelledBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCancelledCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCancelledCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedCancelledFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedFailedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Failed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedFailedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Failed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedFailedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Failed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedFailedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Failed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedFailedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Failed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedFailedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Failed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateStatusAdmissionDeniedFailedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Failed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSelfAttestAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSelfAttestOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSelfAttestInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSelfAttestBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSelfAttestCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSelfAttestCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSelfAttestFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionHostConfirmedAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionHostConfirmedOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionHostConfirmedInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionHostConfirmedBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionHostConfirmedCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionHostConfirmedCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionHostConfirmedFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSupervisorAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSupervisorOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSupervisorInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSupervisorBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSupervisorCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSupervisorCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionSupervisorFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCompletedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Completed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCompletedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Completed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCompletedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Completed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCompletedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Completed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCompletedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Completed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCompletedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Completed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCompletedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Completed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCancelledAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCancelledOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCancelledInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Cancelled")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCancelledBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCancelledCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCancelledCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionCancelledFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionFailedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Failed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionFailedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Failed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionFailedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Failed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionFailedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Failed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionFailedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Failed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionFailedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Failed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionFailedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Failed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedAbsentAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Absent")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedAbsentOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Absent")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedAbsentInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Absent")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedAbsentBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Absent")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedAbsentCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Absent")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedAbsentCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Absent")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedAbsentFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Absent")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedOpenAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Open")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedOpenOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Open")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedOpenInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Open")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedOpenBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Open")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedOpenCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Open")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedOpenCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Open")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedOpenFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Open")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedInProgressAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "InProgress")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedInProgressOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "InProgress")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedInProgressInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "InProgress")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedInProgressBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "InProgress")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedInProgressCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "InProgress")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedInProgressCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "InProgress")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedInProgressFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "InProgress")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedBlockedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Blocked")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedBlockedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Blocked")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedBlockedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Blocked")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedBlockedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Blocked")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedBlockedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Blocked")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedBlockedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Blocked")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCloseStatusAdmissionDeniedBlockedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Blocked")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSelfAttestAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSelfAttestOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSelfAttestInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSelfAttestBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSelfAttestCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSelfAttestCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSelfAttestFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionHostConfirmedAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionHostConfirmedOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionHostConfirmedInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionHostConfirmedBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionHostConfirmedCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionHostConfirmedCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionHostConfirmedFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionPrincipalConfirmedAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionPrincipalConfirmedOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionPrincipalConfirmedInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionPrincipalConfirmedBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionPrincipalConfirmedCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionPrincipalConfirmedCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionPrincipalConfirmedFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSupervisorAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSupervisorOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSupervisorInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSupervisorBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSupervisorCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSupervisorCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionSupervisorFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionReviewerQuorumAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionReviewerQuorumOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionReviewerQuorumInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionReviewerQuorumBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionReviewerQuorumCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionReviewerQuorumCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyPublicConfirmationAdmissionReviewerQuorumFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionUnchangedAbsent(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Absent"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionUnchangedOpen(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Open"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionUnchangedInProgress(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "InProgress"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionUnchangedBlocked(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Blocked"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionUnchangedCompleted(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Completed"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionUnchangedCancelled(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Cancelled"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionUnchangedFailed(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Failed"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionChangedAbsent(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Absent"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionChangedOpen(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Open"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionChangedInProgress(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "InProgress"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionChangedBlocked(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Blocked"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionChangedCompleted(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Completed"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionChangedCancelled(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Cancelled"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyCompletionPolicyMutationAdmissionChangedFailed(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Failed"
    /\ (IF (requested_completion_policy # completion_policy) THEN TRUE ELSE (IF (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) THEN TRUE ELSE (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold)))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalRequiredAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalRequiredOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalRequiredInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalRequiredBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalRequiredCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalRequiredCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalRequiredFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalKindMismatchAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalKindMismatchOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalKindMismatchInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalKindMismatchBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalKindMismatchCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalKindMismatchCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionPrincipalKindMismatchFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSupervisorMismatchAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSupervisorMismatchOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSupervisorMismatchInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSupervisorMismatchBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSupervisorMismatchCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSupervisorMismatchCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSupervisorMismatchFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSelfAttestEmptyAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSelfAttestEmptyOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSelfAttestEmptyInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSelfAttestEmptyBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSelfAttestEmptyCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSelfAttestEmptyCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionSelfAttestEmptyFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionEvidenceKindAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionEvidenceKindOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionEvidenceKindInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionEvidenceKindBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionEvidenceKindCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionEvidenceKindCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionEvidenceKindFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionAdmittedAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionAdmittedOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionAdmittedInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionAdmittedBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionAdmittedCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionAdmittedCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


ClassifyConfirmationAdmissionAdmittedFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_11dfc16157be893f


Next ==
    \/ \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : \E arg_failed_child_join_policy \in FailedChildJoinPolicyValues : \E arg_cancelled_child_join_policy \in CancelledChildJoinPolicyValues : CreateOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count, arg_failed_child_join_policy, arg_cancelled_child_join_policy)
    \/ \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : \E arg_failed_child_join_policy \in FailedChildJoinPolicyValues : \E arg_cancelled_child_join_policy \in CancelledChildJoinPolicyValues : CreateBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count, arg_failed_child_join_policy, arg_cancelled_child_join_policy)
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
THEOREM Spec => []terminal_has_no_claim
THEOREM Spec => []supervisor_policy_has_owner
THEOREM Spec => []non_supervisor_policy_has_no_owner
THEOREM Spec => []reviewer_quorum_policy_has_positive_threshold
THEOREM Spec => []non_reviewer_quorum_policy_has_no_threshold

=============================================================================
