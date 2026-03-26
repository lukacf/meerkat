---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobMemberLifecycleAnchorMachine.

CONSTANTS OperationIdValues, OperationTerminalOutcomeValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, observed_peer_exposed_operations, observed_terminalized_operations, peer_exposure_count, terminalization_count

vars == << phase, model_step_count, observed_peer_exposed_operations, observed_terminalized_operations, peer_exposure_count, terminalization_count >>

Init ==
    /\ phase = "Tracking"
    /\ model_step_count = 0
    /\ observed_peer_exposed_operations = {}
    /\ observed_terminalized_operations = {}
    /\ peer_exposure_count = 0
    /\ terminalization_count = 0

MemberPeerExposed(operation_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ observed_peer_exposed_operations' = (observed_peer_exposed_operations \cup {operation_id})
    /\ peer_exposure_count' = (peer_exposure_count) + 1
    /\ UNCHANGED << observed_terminalized_operations, terminalization_count >>


MemberTerminalized(operation_id, terminal_outcome) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ observed_terminalized_operations' = (observed_terminalized_operations \cup {operation_id})
    /\ terminalization_count' = (terminalization_count) + 1
    /\ UNCHANGED << observed_peer_exposed_operations, peer_exposure_count >>


Next ==
    \/ \E operation_id \in OperationIdValues : MemberPeerExposed(operation_id)
    \/ \E operation_id \in OperationIdValues : \E terminal_outcome \in OperationTerminalOutcomeValues : MemberTerminalized(operation_id, terminal_outcome)


CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(observed_peer_exposed_operations) <= 1 /\ Cardinality(observed_terminalized_operations) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(observed_peer_exposed_operations) <= 2 /\ Cardinality(observed_terminalized_operations) <= 2

Spec == Init /\ [][Next]_vars


=============================================================================
