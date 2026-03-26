---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobWiringAnchorMachine.

CONSTANTS HandlingModeValues, OperationIdValues, PeerInputClassValues, RawItemIdValues, WorkIdValues

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

VARIABLES phase, model_step_count, observed_trusted_operation_peers, observed_peer_input_candidates, observed_runtime_work_items

vars == << phase, model_step_count, observed_trusted_operation_peers, observed_peer_input_candidates, observed_runtime_work_items >>

Init ==
    /\ phase = "Tracking"
    /\ model_step_count = 0
    /\ observed_trusted_operation_peers = {}
    /\ observed_peer_input_candidates = {}
    /\ observed_runtime_work_items = {}

OperationPeerTrusted(operation_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ observed_trusted_operation_peers' = (observed_trusted_operation_peers \cup {operation_id})
    /\ UNCHANGED << observed_peer_input_candidates, observed_runtime_work_items >>


PeerInputAdmitted(raw_item_id, peer_input_class) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ observed_peer_input_candidates' = (observed_peer_input_candidates \cup {raw_item_id})
    /\ UNCHANGED << observed_trusted_operation_peers, observed_runtime_work_items >>


RuntimeWorkAdmitted(work_id, handling_mode) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ observed_runtime_work_items' = (observed_runtime_work_items \cup {work_id})
    /\ UNCHANGED << observed_trusted_operation_peers, observed_peer_input_candidates >>


Next ==
    \/ \E operation_id \in OperationIdValues : OperationPeerTrusted(operation_id)
    \/ \E raw_item_id \in RawItemIdValues : \E peer_input_class \in PeerInputClassValues : PeerInputAdmitted(raw_item_id, peer_input_class)
    \/ \E work_id \in WorkIdValues : \E handling_mode \in HandlingModeValues : RuntimeWorkAdmitted(work_id, handling_mode)


CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(observed_trusted_operation_peers) <= 1 /\ Cardinality(observed_peer_input_candidates) <= 1 /\ Cardinality(observed_runtime_work_items) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(observed_trusted_operation_peers) <= 2 /\ Cardinality(observed_peer_input_candidates) <= 2 /\ Cardinality(observed_runtime_work_items) <= 2

Spec == Init /\ [][Next]_vars


=============================================================================
