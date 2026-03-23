---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for PeerDirectoryReachabilityMachine.

CONSTANTS PeerReachabilityReasonValues, PeerReachabilityValues, ReachabilityKeyValues, SetOfReachabilityKeyValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionPeerReachabilityReasonValues == {None} \cup {Some(x) : x \in PeerReachabilityReasonValues}
MapReachabilityKeyOptionPeerReachabilityReasonValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in ReachabilityKeyValues, v \in OptionPeerReachabilityReasonValues }
MapReachabilityKeyPeerReachabilityValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in ReachabilityKeyValues, v \in PeerReachabilityValues }

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, resolved_keys, reachability, last_reason

vars == << phase, model_step_count, resolved_keys, reachability, last_reason >>

Init ==
    /\ phase = "Tracking"
    /\ model_step_count = 0
    /\ resolved_keys = {}
    /\ reachability = [x \in {} |-> None]
    /\ last_reason = [x \in {} |-> None]

ReconcileResolvedDirectory(keys, arg_reachability, arg_last_reason) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ resolved_keys' = keys
    /\ reachability' = arg_reachability
    /\ last_reason' = arg_last_reason


RecordSendSucceeded(key) ==
    /\ phase = "Tracking"
    /\ (key \in resolved_keys)
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ reachability' = MapSet(reachability, key, "Reachable")
    /\ last_reason' = MapSet(last_reason, key, None)
    /\ UNCHANGED << resolved_keys >>


RecordSendFailed(key, reason) ==
    /\ phase = "Tracking"
    /\ (key \in resolved_keys)
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ reachability' = MapSet(reachability, key, "Unreachable")
    /\ last_reason' = MapSet(last_reason, key, Some(reason))
    /\ UNCHANGED << resolved_keys >>


Next ==
    \/ \E keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : ReconcileResolvedDirectory(keys, arg_reachability, arg_last_reason)
    \/ \E key \in ReachabilityKeyValues : RecordSendSucceeded(key)
    \/ \E key \in ReachabilityKeyValues : \E reason \in PeerReachabilityReasonValues : RecordSendFailed(key, reason)

reachability_keys_are_resolved == (\A key \in DOMAIN reachability : (key \in resolved_keys))
last_reason_keys_are_resolved == (\A key \in DOMAIN last_reason : (key \in resolved_keys))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(resolved_keys) <= 1 /\ Cardinality(DOMAIN reachability) <= 1 /\ Cardinality(DOMAIN last_reason) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(resolved_keys) <= 2 /\ Cardinality(DOMAIN reachability) <= 2 /\ Cardinality(DOMAIN last_reason) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []reachability_keys_are_resolved
THEOREM Spec => []last_reason_keys_are_resolved

=============================================================================
