---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for WorkGraphLifecycleMachine.

CONSTANTS BooleanValues, NatValues, SetOfWorkDependencyPathKeyValues, SetOfWorkEdgeKeyValues, SetOfWorkItemKeyValues, SetOfWorkOwnerKeyValues, WorkCloseStatusAdmissionKindValues, WorkCompletionPolicyMutationAdmissionKindValues, WorkCompletionPolicyValues, WorkConfirmationAdmissionKindValues, WorkConfirmationEvidenceObservationValues, WorkCreateCompletionPolicyAdmissionKindValues, WorkCreateStatusAdmissionKindValues, WorkDependencyPathKeyValues, WorkEdgeKeyValues, WorkEdgeKindValues, WorkEvidenceKindValues, WorkGraphErrorKindValues, WorkGraphPublicErrorClassValues, WorkItemKeyValues, WorkLifecycleStateValues, WorkOwnerKeyValues, WorkOwnerKindValues, WorkPublicConfirmationAdmissionKindValues

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

VARIABLES phase, model_step_count, revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys

vars == << phase, model_step_count, revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>

claim_time_window_eligible(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, now_utc_ms) == ((IF (arg_due_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_due_at_utc_ms THEN arg_due_at_utc_ms["value"] ELSE None) <= now_utc_ms)) /\ (IF (arg_not_before_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_not_before_utc_ms THEN arg_not_before_utc_ms["value"] ELSE None) <= now_utc_ms)) /\ (IF (arg_snoozed_until_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_snoozed_until_utc_ms THEN arg_snoozed_until_utc_ms["value"] ELSE None) <= now_utc_ms)))
confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind) == ((arg_completion_policy = "SelfAttest") /\ (supplied_evidence_kind = "Empty"))
confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key) == ((arg_completion_policy = "Supervisor") /\ (requested_principal_owner_key # None) /\ ((arg_completion_supervisor_owner_key = None) \/ ((IF "value" \in DOMAIN requested_principal_owner_key THEN requested_principal_owner_key["value"] ELSE None) # (IF "value" \in DOMAIN arg_completion_supervisor_owner_key THEN arg_completion_supervisor_owner_key["value"] ELSE None))))
confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind) == ((arg_completion_policy = "PrincipalConfirmed") /\ (requested_principal_owner_key # None) /\ ((requested_principal_kind = None) \/ ((IF "value" \in DOMAIN requested_principal_kind THEN requested_principal_kind["value"] ELSE None) # "Principal")))
confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key) == (((arg_completion_policy = "PrincipalConfirmed") \/ (arg_completion_policy = "Supervisor") \/ (arg_completion_policy = "ReviewerQuorum")) /\ (requested_principal_owner_key = None))
evidence_kind_owner_key_present(evidence_kind, confirming_owner_key) == (IF (evidence_kind = "SupervisorConfirmation") THEN (confirming_owner_key # None) ELSE (IF (evidence_kind = "ReviewerConfirmation") THEN (confirming_owner_key # None) ELSE TRUE))
completion_policy_is_satisfied(policy, supervisor_owner_key, reviewer_quorum_threshold, arg_host_confirmation_count, arg_principal_confirmation_count, arg_supervisor_confirmation_owner_keys, arg_reviewer_confirmation_owner_keys) == (IF (policy = "SelfAttest") THEN TRUE ELSE (IF (policy = "HostConfirmed") THEN (arg_host_confirmation_count > 0) ELSE (IF (policy = "PrincipalConfirmed") THEN (arg_principal_confirmation_count > 0) ELSE (IF (policy = "Supervisor") THEN ((supervisor_owner_key # None) /\ ((IF "value" \in DOMAIN supervisor_owner_key THEN supervisor_owner_key["value"] ELSE None) \in arg_supervisor_confirmation_owner_keys)) ELSE ((reviewer_quorum_threshold # None) /\ (Cardinality(arg_reviewer_confirmation_owner_keys) >= (IF "value" \in DOMAIN reviewer_quorum_threshold THEN reviewer_quorum_threshold["value"] ELSE None)))))))
completion_policy_payload_valid(policy, supervisor_owner_key, reviewer_quorum_threshold) == (IF (policy = "Supervisor") THEN ((supervisor_owner_key # None) /\ (reviewer_quorum_threshold = None)) ELSE (IF (policy = "ReviewerQuorum") THEN ((supervisor_owner_key = None) /\ (reviewer_quorum_threshold # None) /\ ((IF "value" \in DOMAIN reviewer_quorum_threshold THEN reviewer_quorum_threshold["value"] ELSE None) > 0)) ELSE ((supervisor_owner_key = None) /\ (reviewer_quorum_threshold = None))))
confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) == (IF (arg_completion_policy = "HostConfirmed") THEN (supplied_evidence_kind # "HostConfirmation") ELSE (IF (arg_completion_policy = "PrincipalConfirmed") THEN ((confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key) = FALSE) /\ (confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind) = FALSE) /\ (supplied_evidence_kind # "PrincipalConfirmation")) ELSE (IF (arg_completion_policy = "Supervisor") THEN ((confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key) = FALSE) /\ (confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key) = FALSE) /\ (supplied_evidence_kind # "SupervisorConfirmation")) ELSE (IF (arg_completion_policy = "ReviewerQuorum") THEN ((confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key) = FALSE) /\ (supplied_evidence_kind # "ReviewerConfirmation")) ELSE FALSE))))
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

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Cancelled" \/ phase = "Failed"
    /\ UNCHANGED vars

CreateOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
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
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


CreateBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
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
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


UpdateOpen(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ completion_policy_payload_valid(arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ completion_policy' = arg_completion_policy
    /\ completion_supervisor_owner_key' = arg_completion_supervisor_owner_key
    /\ completion_reviewer_quorum_threshold' = arg_completion_reviewer_quorum_threshold
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


UpdateInProgress(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ completion_policy_payload_valid(arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ completion_policy' = arg_completion_policy
    /\ completion_supervisor_owner_key' = arg_completion_supervisor_owner_key
    /\ completion_reviewer_quorum_threshold' = arg_completion_reviewer_quorum_threshold
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


UpdateBlocked(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ completion_policy_payload_valid(arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ completion_policy' = arg_completion_policy
    /\ completion_supervisor_owner_key' = arg_completion_supervisor_owner_key
    /\ completion_reviewer_quorum_threshold' = arg_completion_reviewer_quorum_threshold
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClaimOpen(expected_revision, owner_key, now_utc_ms, arg_lease_expires_at_utc_ms) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ (unresolved_blocker_count = 0)
    /\ (IF (due_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN due_at_utc_ms THEN due_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ (IF (not_before_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN not_before_utc_ms THEN not_before_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ (IF (snoozed_until_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN snoozed_until_utc_ms THEN snoozed_until_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = Some(owner_key)
    /\ claimed_at_utc_ms' = Some(now_utc_ms)
    /\ lease_expires_at_utc_ms' = arg_lease_expires_at_utc_ms
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClaimExpiredInProgress(expected_revision, owner_key, now_utc_ms, arg_lease_expires_at_utc_ms) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ (claim_owner_key # None)
    /\ (lease_expires_at_utc_ms # None)
    /\ (IF (lease_expires_at_utc_ms = None) THEN FALSE ELSE ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ (unresolved_blocker_count = 0)
    /\ (IF (due_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN due_at_utc_ms THEN due_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ (IF (not_before_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN not_before_utc_ms THEN not_before_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ (IF (snoozed_until_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN snoozed_until_utc_ms THEN snoozed_until_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = Some(owner_key)
    /\ claimed_at_utc_ms' = Some(now_utc_ms)
    /\ lease_expires_at_utc_ms' = arg_lease_expires_at_utc_ms
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ReleaseInProgress(expected_revision) ==
    /\ phase = "InProgress"
    /\ ((revision = expected_revision) /\ (claim_owner_key # None))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


BlockOpen(expected_revision) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


BlockInProgress(expected_revision) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


BlockBlocked(expected_revision) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


RefreshEligibilityOpen(arg_unresolved_blocker_count) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


RefreshEligibilityInProgress(arg_unresolved_blocker_count) ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


RefreshEligibilityBlocked(arg_unresolved_blocker_count) ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ValidateLink(kind, from_item_key, to_item_key, edge_key, reverse_path_key) ==
    /\ phase = "Absent"
    /\ (from_item_key \in topology_item_keys)
    /\ (to_item_key \in topology_item_keys)
    /\ (from_item_key # to_item_key)
    /\ ((edge_key \in topology_edge_keys) = FALSE)
    /\ ((kind # "Blocks") \/ ((reverse_path_key \in blocks_reachability) = FALSE))
    /\ ((kind # "Parent") \/ ((reverse_path_key \in parent_reachability) = FALSE))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms >>


ClassifyPublicErrorNotFoundAbsent(kind) ==
    /\ phase = "Absent"
    /\ ((kind = "NotFound") \/ (kind = "AttentionNotFound"))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorNotFoundOpen(kind) ==
    /\ phase = "Open"
    /\ ((kind = "NotFound") \/ (kind = "AttentionNotFound"))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorNotFoundInProgress(kind) ==
    /\ phase = "InProgress"
    /\ ((kind = "NotFound") \/ (kind = "AttentionNotFound"))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorNotFoundBlocked(kind) ==
    /\ phase = "Blocked"
    /\ ((kind = "NotFound") \/ (kind = "AttentionNotFound"))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorNotFoundCompleted(kind) ==
    /\ phase = "Completed"
    /\ ((kind = "NotFound") \/ (kind = "AttentionNotFound"))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorNotFoundCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ ((kind = "NotFound") \/ (kind = "AttentionNotFound"))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorNotFoundFailed(kind) ==
    /\ phase = "Failed"
    /\ ((kind = "NotFound") \/ (kind = "AttentionNotFound"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorConflictAbsent(kind) ==
    /\ phase = "Absent"
    /\ ((kind = "StaleRevision") \/ (kind = "Conflict"))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorConflictOpen(kind) ==
    /\ phase = "Open"
    /\ ((kind = "StaleRevision") \/ (kind = "Conflict"))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorConflictInProgress(kind) ==
    /\ phase = "InProgress"
    /\ ((kind = "StaleRevision") \/ (kind = "Conflict"))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorConflictBlocked(kind) ==
    /\ phase = "Blocked"
    /\ ((kind = "StaleRevision") \/ (kind = "Conflict"))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorConflictCompleted(kind) ==
    /\ phase = "Completed"
    /\ ((kind = "StaleRevision") \/ (kind = "Conflict"))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorConflictCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ ((kind = "StaleRevision") \/ (kind = "Conflict"))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorConflictFailed(kind) ==
    /\ phase = "Failed"
    /\ ((kind = "StaleRevision") \/ (kind = "Conflict"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidTransitionAbsent(kind) ==
    /\ phase = "Absent"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidTransitionOpen(kind) ==
    /\ phase = "Open"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidTransitionInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (kind = "InvalidTransition")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidTransitionBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidTransitionCompleted(kind) ==
    /\ phase = "Completed"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidTransitionCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidTransitionFailed(kind) ==
    /\ phase = "Failed"
    /\ (kind = "InvalidTransition")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidArgumentsAbsent(kind) ==
    /\ phase = "Absent"
    /\ ((kind = "InvalidInput") \/ (kind = "InvalidTimestampMillis"))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidArgumentsOpen(kind) ==
    /\ phase = "Open"
    /\ ((kind = "InvalidInput") \/ (kind = "InvalidTimestampMillis"))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidArgumentsInProgress(kind) ==
    /\ phase = "InProgress"
    /\ ((kind = "InvalidInput") \/ (kind = "InvalidTimestampMillis"))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidArgumentsBlocked(kind) ==
    /\ phase = "Blocked"
    /\ ((kind = "InvalidInput") \/ (kind = "InvalidTimestampMillis"))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidArgumentsCompleted(kind) ==
    /\ phase = "Completed"
    /\ ((kind = "InvalidInput") \/ (kind = "InvalidTimestampMillis"))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidArgumentsCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ ((kind = "InvalidInput") \/ (kind = "InvalidTimestampMillis"))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorInvalidArgumentsFailed(kind) ==
    /\ phase = "Failed"
    /\ ((kind = "InvalidInput") \/ (kind = "InvalidTimestampMillis"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorCapabilityUnavailableAbsent(kind) ==
    /\ phase = "Absent"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorCapabilityUnavailableOpen(kind) ==
    /\ phase = "Open"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorCapabilityUnavailableInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorCapabilityUnavailableBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorCapabilityUnavailableCompleted(kind) ==
    /\ phase = "Completed"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorCapabilityUnavailableCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorCapabilityUnavailableFailed(kind) ==
    /\ phase = "Failed"
    /\ (kind = "UnsupportedBackend")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorStoreErrorAbsent(kind) ==
    /\ phase = "Absent"
    /\ (kind = "Store")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorStoreErrorOpen(kind) ==
    /\ phase = "Open"
    /\ (kind = "Store")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorStoreErrorInProgress(kind) ==
    /\ phase = "InProgress"
    /\ (kind = "Store")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorStoreErrorBlocked(kind) ==
    /\ phase = "Blocked"
    /\ (kind = "Store")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorStoreErrorCompleted(kind) ==
    /\ phase = "Completed"
    /\ (kind = "Store")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorStoreErrorCancelled(kind) ==
    /\ phase = "Cancelled"
    /\ (kind = "Store")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicErrorStoreErrorFailed(kind) ==
    /\ phase = "Failed"
    /\ (kind = "Store")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyTerminalityTerminalCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyTerminalityTerminalCancelled ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyTerminalityTerminalFailed ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyTerminalityLiveAbsent ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyTerminalityLiveOpen ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyTerminalityLiveInProgress ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyTerminalityLiveBlocked ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyReadinessOpenOpen(now_utc_ms) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyReadinessInProgressInProgress(now_utc_ms) ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyReadinessNotClaimableAbsent(now_utc_ms) ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyReadinessNotClaimableBlocked(now_utc_ms) ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyReadinessNotClaimableCompleted(now_utc_ms) ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyReadinessNotClaimableCancelled(now_utc_ms) ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyReadinessNotClaimableFailed(now_utc_ms) ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyBlockerSatisfactionAbsent(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyBlockerSatisfactionOpen(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyBlockerSatisfactionInProgress(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyBlockerSatisfactionBlocked(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyBlockerSatisfactionCompleted(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyBlockerSatisfactionCancelled(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyBlockerSatisfactionFailed(blocker_present, blocker_lifecycle_phase) ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionOpenAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Open")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionOpenOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Open")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionOpenInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Open")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionOpenBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Open")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionOpenCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Open")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionOpenCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Open")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionOpenFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Open")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionBlockedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Blocked")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionBlockedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Blocked")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionBlockedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Blocked")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionBlockedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Blocked")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionBlockedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Blocked")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionBlockedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Blocked")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionBlockedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Blocked")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedAbsentAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Absent")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedAbsentOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Absent")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedAbsentInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Absent")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedAbsentBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Absent")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedAbsentCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Absent")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedAbsentCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Absent")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedAbsentFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Absent")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedInProgressAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "InProgress")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedInProgressOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "InProgress")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedInProgressInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "InProgress")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedInProgressBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "InProgress")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedInProgressCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "InProgress")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedInProgressCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "InProgress")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedInProgressFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "InProgress")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCompletedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Completed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCompletedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Completed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCompletedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Completed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCompletedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Completed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCompletedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Completed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCompletedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Completed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCompletedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Completed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCancelledAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCancelledOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCancelledInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Cancelled")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCancelledBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCancelledCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCancelledCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedCancelledFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedFailedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Failed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedFailedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Failed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedFailedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Failed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedFailedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Failed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedFailedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Failed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedFailedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Failed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateStatusAdmissionDeniedFailedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Failed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSelfAttestAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSelfAttestOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSelfAttestInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSelfAttestBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSelfAttestCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSelfAttestCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSelfAttestFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionHostConfirmedAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionHostConfirmedOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionHostConfirmedInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionHostConfirmedBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionHostConfirmedCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionHostConfirmedCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionHostConfirmedFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSupervisorAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSupervisorOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSupervisorInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSupervisorBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSupervisorCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSupervisorCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionSupervisorFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCreateCompletionPolicyAdmissionReviewerQuorumFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCompletedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Completed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCompletedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Completed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCompletedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Completed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCompletedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Completed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCompletedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Completed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCompletedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Completed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCompletedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Completed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCancelledAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCancelledOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCancelledInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Cancelled")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCancelledBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCancelledCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCancelledCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionCancelledFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Cancelled")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionFailedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Failed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionFailedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Failed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionFailedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Failed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionFailedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Failed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionFailedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Failed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionFailedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Failed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionFailedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Failed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedAbsentAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Absent")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedAbsentOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Absent")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedAbsentInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Absent")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedAbsentBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Absent")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedAbsentCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Absent")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedAbsentCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Absent")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedAbsentFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Absent")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedOpenAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Open")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedOpenOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Open")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedOpenInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Open")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedOpenBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Open")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedOpenCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Open")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedOpenCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Open")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedOpenFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Open")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedInProgressAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "InProgress")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedInProgressOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "InProgress")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedInProgressInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "InProgress")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedInProgressBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "InProgress")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedInProgressCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "InProgress")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedInProgressCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "InProgress")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedInProgressFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "InProgress")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedBlockedAbsent(requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = "Blocked")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedBlockedOpen(requested_status) ==
    /\ phase = "Open"
    /\ (requested_status = "Blocked")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedBlockedInProgress(requested_status) ==
    /\ phase = "InProgress"
    /\ (requested_status = "Blocked")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedBlockedBlocked(requested_status) ==
    /\ phase = "Blocked"
    /\ (requested_status = "Blocked")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedBlockedCompleted(requested_status) ==
    /\ phase = "Completed"
    /\ (requested_status = "Blocked")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedBlockedCancelled(requested_status) ==
    /\ phase = "Cancelled"
    /\ (requested_status = "Blocked")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCloseStatusAdmissionDeniedBlockedFailed(requested_status) ==
    /\ phase = "Failed"
    /\ (requested_status = "Blocked")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSelfAttestAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSelfAttestOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSelfAttestInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSelfAttestBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSelfAttestCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSelfAttestCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSelfAttestFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "SelfAttest")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionHostConfirmedAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionHostConfirmedOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionHostConfirmedInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionHostConfirmedBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionHostConfirmedCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionHostConfirmedCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionHostConfirmedFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "HostConfirmed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionPrincipalConfirmedAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionPrincipalConfirmedOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionPrincipalConfirmedInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionPrincipalConfirmedBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionPrincipalConfirmedCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionPrincipalConfirmedCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionPrincipalConfirmedFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "PrincipalConfirmed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSupervisorAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSupervisorOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSupervisorInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSupervisorBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSupervisorCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSupervisorCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionSupervisorFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "Supervisor")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionReviewerQuorumAbsent(arg_completion_policy) ==
    /\ phase = "Absent"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionReviewerQuorumOpen(arg_completion_policy) ==
    /\ phase = "Open"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionReviewerQuorumInProgress(arg_completion_policy) ==
    /\ phase = "InProgress"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionReviewerQuorumBlocked(arg_completion_policy) ==
    /\ phase = "Blocked"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionReviewerQuorumCompleted(arg_completion_policy) ==
    /\ phase = "Completed"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionReviewerQuorumCancelled(arg_completion_policy) ==
    /\ phase = "Cancelled"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyPublicConfirmationAdmissionReviewerQuorumFailed(arg_completion_policy) ==
    /\ phase = "Failed"
    /\ (arg_completion_policy = "ReviewerQuorum")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionUnchangedAbsent(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Absent"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionUnchangedOpen(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Open"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionUnchangedInProgress(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "InProgress"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionUnchangedBlocked(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Blocked"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionUnchangedCompleted(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Completed"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionUnchangedCancelled(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Cancelled"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionUnchangedFailed(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Failed"
    /\ ((requested_completion_policy = completion_policy) /\ (requested_completion_supervisor_owner_key = completion_supervisor_owner_key) /\ (requested_completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionChangedAbsent(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Absent"
    /\ ((requested_completion_policy # completion_policy) \/ (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) \/ (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionChangedOpen(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Open"
    /\ ((requested_completion_policy # completion_policy) \/ (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) \/ (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionChangedInProgress(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "InProgress"
    /\ ((requested_completion_policy # completion_policy) \/ (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) \/ (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold))
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionChangedBlocked(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Blocked"
    /\ ((requested_completion_policy # completion_policy) \/ (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) \/ (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionChangedCompleted(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Completed"
    /\ ((requested_completion_policy # completion_policy) \/ (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) \/ (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionChangedCancelled(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Cancelled"
    /\ ((requested_completion_policy # completion_policy) \/ (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) \/ (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyCompletionPolicyMutationAdmissionChangedFailed(requested_completion_policy, requested_completion_supervisor_owner_key, requested_completion_reviewer_quorum_threshold) ==
    /\ phase = "Failed"
    /\ ((requested_completion_policy # completion_policy) \/ (requested_completion_supervisor_owner_key # completion_supervisor_owner_key) \/ (requested_completion_reviewer_quorum_threshold # completion_reviewer_quorum_threshold))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalRequiredAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalRequiredOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalRequiredInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalRequiredBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalRequiredCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalRequiredCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalRequiredFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_principal_required(arg_completion_policy, requested_principal_owner_key)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalKindMismatchAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalKindMismatchOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalKindMismatchInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalKindMismatchBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalKindMismatchCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalKindMismatchCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionPrincipalKindMismatchFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_principal_kind_mismatch(arg_completion_policy, requested_principal_owner_key, requested_principal_kind)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSupervisorMismatchAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSupervisorMismatchOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSupervisorMismatchInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSupervisorMismatchBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSupervisorMismatchCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSupervisorMismatchCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSupervisorMismatchFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_supervisor_mismatch(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSelfAttestEmptyAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSelfAttestEmptyOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSelfAttestEmptyInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSelfAttestEmptyBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSelfAttestEmptyCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSelfAttestEmptyCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionSelfAttestEmptyFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_self_attest_empty(arg_completion_policy, supplied_evidence_kind)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionEvidenceKindAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionEvidenceKindOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionEvidenceKindInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionEvidenceKindBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionEvidenceKindCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionEvidenceKindCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionEvidenceKindFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_denies_evidence_kind(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionAdmittedAbsent(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Absent"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionAdmittedOpen(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Open"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionAdmittedInProgress(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "InProgress"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionAdmittedBlocked(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Blocked"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionAdmittedCompleted(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Completed"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionAdmittedCancelled(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Cancelled"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


ClassifyConfirmationAdmissionAdmittedFailed(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) ==
    /\ phase = "Failed"
    /\ confirmation_admits(arg_completion_policy, arg_completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, terminal_at_utc_ms, evidence_count, host_confirmation_count, principal_confirmation_count, supervisor_confirmation_owner_keys, reviewer_confirmation_owner_keys >>


Next ==
    \/ \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : CreateOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : CreateBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E expected_revision \in {revision} : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : UpdateOpen(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E expected_revision \in {revision} : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : UpdateInProgress(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E expected_revision \in {revision} : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : UpdateBlocked(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E expected_revision \in {revision} : \E owner_key \in WorkOwnerKeyValues : \E now_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in OptionU64Values : ClaimOpen(expected_revision, owner_key, now_utc_ms, arg_lease_expires_at_utc_ms)
    \/ \E expected_revision \in {revision} : \E owner_key \in WorkOwnerKeyValues : \E now_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in OptionU64Values : ClaimExpiredInProgress(expected_revision, owner_key, now_utc_ms, arg_lease_expires_at_utc_ms)
    \/ \E expected_revision \in {revision} : ReleaseInProgress(expected_revision)
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
    \/ \E now_utc_ms \in 0..2 : ClassifyReadinessOpenOpen(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyReadinessInProgressInProgress(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyReadinessNotClaimableAbsent(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyReadinessNotClaimableBlocked(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyReadinessNotClaimableCompleted(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyReadinessNotClaimableCancelled(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyReadinessNotClaimableFailed(now_utc_ms)
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

absent_has_zero_revision == ((phase # "Absent") \/ (revision = 0))
live_has_positive_revision == ((phase = "Absent") \/ (revision > 0))
topology_snapshot_is_stateless == ((topology_item_keys = {}) \/ (topology_edge_keys = {}) \/ (phase = "Absent"))
terminal_has_terminal_time == (((phase # "Completed") /\ (phase # "Cancelled") /\ (phase # "Failed")) \/ (terminal_at_utc_ms # None))
claim_only_in_progress == ((claim_owner_key = None) \/ (phase = "InProgress"))
blocked_has_no_claim == ((phase # "Blocked") \/ (claim_owner_key = None))
terminal_has_no_claim == (((phase # "Completed") /\ (phase # "Cancelled") /\ (phase # "Failed")) \/ (claim_owner_key = None))
supervisor_policy_has_owner == ((completion_policy # "Supervisor") \/ (completion_supervisor_owner_key # None))
non_supervisor_policy_has_no_owner == ((completion_policy = "Supervisor") \/ (completion_supervisor_owner_key = None))
reviewer_quorum_policy_has_positive_threshold == ((completion_policy # "ReviewerQuorum") \/ ((completion_reviewer_quorum_threshold # None) /\ ((IF "value" \in DOMAIN completion_reviewer_quorum_threshold THEN completion_reviewer_quorum_threshold["value"] ELSE None) > 0)))
non_reviewer_quorum_policy_has_no_threshold == ((completion_policy = "ReviewerQuorum") \/ (completion_reviewer_quorum_threshold = None))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(topology_item_keys) <= 1 /\ Cardinality(topology_edge_keys) <= 1 /\ Cardinality(blocks_reachability) <= 1 /\ Cardinality(parent_reachability) <= 1 /\ Cardinality(supervisor_confirmation_owner_keys) <= 1 /\ Cardinality(reviewer_confirmation_owner_keys) <= 1
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
