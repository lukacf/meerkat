---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for WorkGraphLifecycleMachine.

CONSTANTS NatValues, SetOfWorkDependencyPathKeyValues, SetOfWorkEdgeKeyValues, SetOfWorkItemKeyValues, WorkDependencyPathKeyValues, WorkEdgeKeyValues, WorkEdgeKindValues, WorkGraphErrorKindValues, WorkItemKeyValues, WorkLifecycleStateValues, WorkOwnerKeyValues

WorkOwnerKeyValuesCi == {}

WorkOwnerKeyValuesDeep == {[kind |-> "Principal", id |-> "alpha"], [kind |-> "Agent", id |-> "beta"]}

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionU64Values == {None} \cup {Some(x) : x \in NatValues}
OptionWorkLifecycleStateValues == {None} \cup {Some(x) : x \in WorkLifecycleStateValues}
OptionWorkOwnerKeyValues == {None} \cup {Some(x) : x \in WorkOwnerKeyValues}

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

VARIABLES phase, model_step_count, revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count

vars == << phase, model_step_count, revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>

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
    /\ terminal_at_utc_ms = None
    /\ evidence_count = 0

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Cancelled" \/ phase = "Failed"
    /\ UNCHANGED vars

CreateDefaultOrOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count, requested_status) ==
    /\ phase = "Absent"
    /\ ((requested_status = None) \/ (requested_status = Some("Open")))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count >>


CreateRequestedBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count, requested_status) ==
    /\ phase = "Absent"
    /\ (requested_status = Some("Blocked"))
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count >>


CreateOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count) ==
    /\ phase = "Absent"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count >>


CreateBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count) ==
    /\ phase = "Absent"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count >>


UpdateOpen(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count >>


UpdateInProgress(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count >>


UpdateBlocked(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ not_before_utc_ms' = arg_not_before_utc_ms
    /\ snoozed_until_utc_ms' = arg_snoozed_until_utc_ms
    /\ UNCHANGED << topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, terminal_at_utc_ms, evidence_count >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ReleaseInProgress(expected_revision) ==
    /\ phase = "InProgress"
    /\ ((revision = expected_revision) /\ (claim_owner_key # None))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


BlockOpen(expected_revision) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


BlockInProgress(expected_revision) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


BlockBlocked(expected_revision) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


RefreshEligibilityOpen(arg_unresolved_blocker_count) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ UNCHANGED << revision, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


RefreshEligibilityInProgress(arg_unresolved_blocker_count) ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ UNCHANGED << revision, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


RefreshEligibilityBlocked(arg_unresolved_blocker_count) ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ unresolved_blocker_count' = arg_unresolved_blocker_count
    /\ UNCHANGED << revision, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyBlockerSatisfiedCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyBlockerUnsatisfiedAbsent ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyBlockerUnsatisfiedOpen ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyBlockerUnsatisfiedInProgress ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyBlockerUnsatisfiedBlocked ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyBlockerUnsatisfiedCancelled ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyBlockerUnsatisfiedFailed ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyTerminalityAbsent ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyTerminalityOpen ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyTerminalityInProgress ==
    /\ phase = "InProgress"
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyTerminalityBlocked ==
    /\ phase = "Blocked"
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyTerminalityCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyTerminalityCancelled ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyTerminalityFailed ==
    /\ phase = "Failed"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


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
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


CloseOpenDefaultOrCompleted(expected_revision, at_utc_ms, requested_status) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ ((requested_status = None) \/ (requested_status = Some("Completed")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


CloseInProgressDefaultOrCompleted(expected_revision, at_utc_ms, requested_status) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ ((requested_status = None) \/ (requested_status = Some("Completed")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


CloseBlockedDefaultOrCompleted(expected_revision, at_utc_ms, requested_status) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ ((requested_status = None) \/ (requested_status = Some("Completed")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


CloseOpenRequestedCancelled(expected_revision, at_utc_ms, requested_status) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ (requested_status = Some("Cancelled"))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


CloseInProgressRequestedCancelled(expected_revision, at_utc_ms, requested_status) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ (requested_status = Some("Cancelled"))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


CloseBlockedRequestedCancelled(expected_revision, at_utc_ms, requested_status) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ (requested_status = Some("Cancelled"))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


CloseOpenRequestedFailed(expected_revision, at_utc_ms, requested_status) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ (requested_status = Some("Failed"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


CloseInProgressRequestedFailed(expected_revision, at_utc_ms, requested_status) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ (requested_status = Some("Failed"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


CloseBlockedRequestedFailed(expected_revision, at_utc_ms, requested_status) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ (requested_status = Some("Failed"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


CloseOpenCompleted(expected_revision, at_utc_ms) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


CloseInProgressCompleted(expected_revision, at_utc_ms) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


CloseBlockedCompleted(expected_revision, at_utc_ms) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ claim_owner_key' = None
    /\ claimed_at_utc_ms' = None
    /\ lease_expires_at_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


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
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, evidence_count >>


AddEvidenceOpen(expected_revision) ==
    /\ phase = "Open"
    /\ (revision = expected_revision)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ evidence_count' = (evidence_count) + 1
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms >>


AddEvidenceInProgress(expected_revision) ==
    /\ phase = "InProgress"
    /\ (revision = expected_revision)
    /\ phase' = "InProgress"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ evidence_count' = (evidence_count) + 1
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms >>


AddEvidenceBlocked(expected_revision) ==
    /\ phase = "Blocked"
    /\ (revision = expected_revision)
    /\ phase' = "Blocked"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ evidence_count' = (evidence_count) + 1
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms >>


AddEvidenceCompleted(expected_revision) ==
    /\ phase = "Completed"
    /\ (revision = expected_revision)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ evidence_count' = (evidence_count) + 1
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms >>


AddEvidenceCancelled(expected_revision) ==
    /\ phase = "Cancelled"
    /\ (revision = expected_revision)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ evidence_count' = (evidence_count) + 1
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms >>


AddEvidenceFailed(expected_revision) ==
    /\ phase = "Failed"
    /\ (revision = expected_revision)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ evidence_count' = (evidence_count) + 1
    /\ UNCHANGED << unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms >>


ClassifyPublicErrorNotFound(error_kind) ==
    /\ phase = "Absent"
    /\ (error_kind = "NotFound")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyPublicErrorStaleRevision(error_kind) ==
    /\ phase = "Absent"
    /\ (error_kind = "StaleRevision")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyPublicErrorConflict(error_kind) ==
    /\ phase = "Absent"
    /\ (error_kind = "Conflict")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyPublicErrorInvalidTransition(error_kind) ==
    /\ phase = "Absent"
    /\ (error_kind = "InvalidTransition")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyPublicErrorInvalidInput(error_kind) ==
    /\ phase = "Absent"
    /\ (error_kind = "InvalidInput")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyPublicErrorInvalidTimestampMillis(error_kind) ==
    /\ phase = "Absent"
    /\ (error_kind = "InvalidTimestampMillis")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyPublicErrorUnsupportedBackend(error_kind) ==
    /\ phase = "Absent"
    /\ (error_kind = "UnsupportedBackend")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


ClassifyPublicErrorStore(error_kind) ==
    /\ phase = "Absent"
    /\ (error_kind = "Store")
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, unresolved_blocker_count, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability, claim_owner_key, claimed_at_utc_ms, lease_expires_at_utc_ms, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, terminal_at_utc_ms, evidence_count >>


Next ==
    \/ \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : \E requested_status \in OptionWorkLifecycleStateValues : CreateDefaultOrOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count, requested_status)
    \/ \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : \E requested_status \in OptionWorkLifecycleStateValues : CreateRequestedBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count, requested_status)
    \/ \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : CreateOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count)
    \/ \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : CreateBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count)
    \/ \E expected_revision \in 0..2 : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : UpdateOpen(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count)
    \/ \E expected_revision \in 0..2 : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : UpdateInProgress(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count)
    \/ \E expected_revision \in 0..2 : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : UpdateBlocked(expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_unresolved_blocker_count)
    \/ \E expected_revision \in 0..2 : \E owner_key \in WorkOwnerKeyValues : \E now_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in OptionU64Values : ClaimOpen(expected_revision, owner_key, now_utc_ms, arg_lease_expires_at_utc_ms)
    \/ \E expected_revision \in 0..2 : \E owner_key \in WorkOwnerKeyValues : \E now_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in OptionU64Values : ClaimExpiredInProgress(expected_revision, owner_key, now_utc_ms, arg_lease_expires_at_utc_ms)
    \/ \E expected_revision \in 0..2 : ReleaseInProgress(expected_revision)
    \/ \E expected_revision \in 0..2 : BlockOpen(expected_revision)
    \/ \E expected_revision \in 0..2 : BlockInProgress(expected_revision)
    \/ \E expected_revision \in 0..2 : BlockBlocked(expected_revision)
    \/ \E arg_unresolved_blocker_count \in 0..2 : RefreshEligibilityOpen(arg_unresolved_blocker_count)
    \/ \E arg_unresolved_blocker_count \in 0..2 : RefreshEligibilityInProgress(arg_unresolved_blocker_count)
    \/ \E arg_unresolved_blocker_count \in 0..2 : RefreshEligibilityBlocked(arg_unresolved_blocker_count)
    \/ ClassifyBlockerSatisfiedCompleted
    \/ ClassifyBlockerUnsatisfiedAbsent
    \/ ClassifyBlockerUnsatisfiedOpen
    \/ ClassifyBlockerUnsatisfiedInProgress
    \/ ClassifyBlockerUnsatisfiedBlocked
    \/ ClassifyBlockerUnsatisfiedCancelled
    \/ ClassifyBlockerUnsatisfiedFailed
    \/ ClassifyTerminalityAbsent
    \/ ClassifyTerminalityOpen
    \/ ClassifyTerminalityInProgress
    \/ ClassifyTerminalityBlocked
    \/ ClassifyTerminalityCompleted
    \/ ClassifyTerminalityCancelled
    \/ ClassifyTerminalityFailed
    \/ \E kind \in WorkEdgeKindValues : \E from_item_key \in WorkItemKeyValues : \E to_item_key \in WorkItemKeyValues : \E edge_key \in WorkEdgeKeyValues : \E reverse_path_key \in WorkDependencyPathKeyValues : ValidateLink(kind, from_item_key, to_item_key, edge_key, reverse_path_key)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : \E requested_status \in OptionWorkLifecycleStateValues : CloseOpenDefaultOrCompleted(expected_revision, at_utc_ms, requested_status)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : \E requested_status \in OptionWorkLifecycleStateValues : CloseInProgressDefaultOrCompleted(expected_revision, at_utc_ms, requested_status)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : \E requested_status \in OptionWorkLifecycleStateValues : CloseBlockedDefaultOrCompleted(expected_revision, at_utc_ms, requested_status)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : \E requested_status \in OptionWorkLifecycleStateValues : CloseOpenRequestedCancelled(expected_revision, at_utc_ms, requested_status)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : \E requested_status \in OptionWorkLifecycleStateValues : CloseInProgressRequestedCancelled(expected_revision, at_utc_ms, requested_status)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : \E requested_status \in OptionWorkLifecycleStateValues : CloseBlockedRequestedCancelled(expected_revision, at_utc_ms, requested_status)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : \E requested_status \in OptionWorkLifecycleStateValues : CloseOpenRequestedFailed(expected_revision, at_utc_ms, requested_status)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : \E requested_status \in OptionWorkLifecycleStateValues : CloseInProgressRequestedFailed(expected_revision, at_utc_ms, requested_status)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : \E requested_status \in OptionWorkLifecycleStateValues : CloseBlockedRequestedFailed(expected_revision, at_utc_ms, requested_status)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : CloseOpenCompleted(expected_revision, at_utc_ms)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : CloseInProgressCompleted(expected_revision, at_utc_ms)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : CloseBlockedCompleted(expected_revision, at_utc_ms)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : CloseOpenCancelled(expected_revision, at_utc_ms)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : CloseInProgressCancelled(expected_revision, at_utc_ms)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : CloseBlockedCancelled(expected_revision, at_utc_ms)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : CloseOpenFailed(expected_revision, at_utc_ms)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : CloseInProgressFailed(expected_revision, at_utc_ms)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : CloseBlockedFailed(expected_revision, at_utc_ms)
    \/ \E expected_revision \in 0..2 : AddEvidenceOpen(expected_revision)
    \/ \E expected_revision \in 0..2 : AddEvidenceInProgress(expected_revision)
    \/ \E expected_revision \in 0..2 : AddEvidenceBlocked(expected_revision)
    \/ \E expected_revision \in 0..2 : AddEvidenceCompleted(expected_revision)
    \/ \E expected_revision \in 0..2 : AddEvidenceCancelled(expected_revision)
    \/ \E expected_revision \in 0..2 : AddEvidenceFailed(expected_revision)
    \/ \E error_kind \in WorkGraphErrorKindValues : ClassifyPublicErrorNotFound(error_kind)
    \/ \E error_kind \in WorkGraphErrorKindValues : ClassifyPublicErrorStaleRevision(error_kind)
    \/ \E error_kind \in WorkGraphErrorKindValues : ClassifyPublicErrorConflict(error_kind)
    \/ \E error_kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidTransition(error_kind)
    \/ \E error_kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidInput(error_kind)
    \/ \E error_kind \in WorkGraphErrorKindValues : ClassifyPublicErrorInvalidTimestampMillis(error_kind)
    \/ \E error_kind \in WorkGraphErrorKindValues : ClassifyPublicErrorUnsupportedBackend(error_kind)
    \/ \E error_kind \in WorkGraphErrorKindValues : ClassifyPublicErrorStore(error_kind)
    \/ TerminalStutter

absent_has_zero_revision == ((phase # "Absent") \/ (revision = 0))
live_has_positive_revision == ((phase = "Absent") \/ (revision > 0))
topology_snapshot_is_stateless == ((topology_item_keys = {}) \/ (topology_edge_keys = {}) \/ (phase = "Absent"))
terminal_has_terminal_time == (((phase # "Completed") /\ (phase # "Cancelled") /\ (phase # "Failed")) \/ (terminal_at_utc_ms # None))
claim_only_in_progress == ((claim_owner_key = None) \/ (phase = "InProgress"))
blocked_has_no_claim == ((phase # "Blocked") \/ (claim_owner_key = None))
terminal_has_no_claim == (((phase # "Completed") /\ (phase # "Cancelled") /\ (phase # "Failed")) \/ (claim_owner_key = None))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(topology_item_keys) <= 1 /\ Cardinality(topology_edge_keys) <= 1 /\ Cardinality(blocks_reachability) <= 1 /\ Cardinality(parent_reachability) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(topology_item_keys) <= 2 /\ Cardinality(topology_edge_keys) <= 2 /\ Cardinality(blocks_reachability) <= 2 /\ Cardinality(parent_reachability) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []absent_has_zero_revision
THEOREM Spec => []live_has_positive_revision
THEOREM Spec => []topology_snapshot_is_stateless
THEOREM Spec => []terminal_has_terminal_time
THEOREM Spec => []claim_only_in_progress
THEOREM Spec => []blocked_has_no_claim
THEOREM Spec => []terminal_has_no_claim

=============================================================================
