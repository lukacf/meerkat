---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for PeerCommsMachine.

CONSTANTS PeerIdValues, RawItemIdValues, RawPeerKindValues

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

VARIABLES phase, model_step_count, trusted_peers, raw_item_peer, raw_item_kind, classified_as, trusted_snapshot, submission_queue

vars == << phase, model_step_count, trusted_peers, raw_item_peer, raw_item_kind, classified_as, trusted_snapshot, submission_queue >>

ClassFor(raw_kind) == (IF (raw_kind = "request") THEN "ActionableRequest" ELSE "ActionableMessage")

Init ==
    /\ phase = "Absent"
    /\ model_step_count = 0
    /\ trusted_peers = {}
    /\ raw_item_peer = [x \in {} |-> None]
    /\ raw_item_kind = [x \in {} |-> None]
    /\ classified_as = [x \in {} |-> None]
    /\ trusted_snapshot = [x \in {} |-> None]
    /\ submission_queue = <<>>

TerminalStutter ==
    /\ phase = "Dropped" \/ phase = "Delivered"
    /\ UNCHANGED vars

TrustPeer(peer_id) ==
    /\ phase = "Absent" \/ phase = "Received"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ trusted_peers' = (trusted_peers \cup {peer_id})
    /\ UNCHANGED << raw_item_peer, raw_item_kind, classified_as, trusted_snapshot, submission_queue >>


ReceiveTrustedPeerEnvelope(raw_item_id, peer_id, raw_kind) ==
    /\ phase = "Absent" \/ phase = "Received"
    /\ (peer_id \in trusted_peers)
    /\ phase' = "Received"
    /\ model_step_count' = model_step_count + 1
    /\ raw_item_peer' = MapSet(raw_item_peer, raw_item_id, peer_id)
    /\ raw_item_kind' = MapSet(raw_item_kind, raw_item_id, raw_kind)
    /\ classified_as' = MapSet(classified_as, raw_item_id, ClassFor(raw_kind))
    /\ trusted_snapshot' = MapSet(trusted_snapshot, raw_item_id, TRUE)
    /\ submission_queue' = Append(submission_queue, raw_item_id)
    /\ UNCHANGED << trusted_peers >>


DropUntrustedPeerEnvelope(raw_item_id, peer_id, raw_kind) ==
    /\ phase = "Absent" \/ phase = "Received"
    /\ ~((peer_id \in trusted_peers))
    /\ phase' = "Dropped"
    /\ model_step_count' = model_step_count + 1
    /\ raw_item_peer' = MapSet(raw_item_peer, raw_item_id, peer_id)
    /\ raw_item_kind' = MapSet(raw_item_kind, raw_item_id, raw_kind)
    /\ trusted_snapshot' = MapSet(trusted_snapshot, raw_item_id, FALSE)
    /\ UNCHANGED << trusted_peers, classified_as, submission_queue >>


SubmitTypedPeerInputDelivered(raw_item_id) ==
    /\ phase = "Received"
    /\ (raw_item_id \in SeqElements(submission_queue))
    /\ (raw_item_id \in DOMAIN classified_as)
    /\ (Len(submission_queue) = 1)
    /\ phase' = "Delivered"
    /\ model_step_count' = model_step_count + 1
    /\ submission_queue' = SeqRemove(submission_queue, raw_item_id)
    /\ UNCHANGED << trusted_peers, raw_item_peer, raw_item_kind, classified_as, trusted_snapshot >>


SubmitTypedPeerInputContinue(raw_item_id) ==
    /\ phase = "Received"
    /\ (raw_item_id \in SeqElements(submission_queue))
    /\ (raw_item_id \in DOMAIN classified_as)
    /\ (Len(submission_queue) > 1)
    /\ phase' = "Received"
    /\ model_step_count' = model_step_count + 1
    /\ submission_queue' = SeqRemove(submission_queue, raw_item_id)
    /\ UNCHANGED << trusted_peers, raw_item_peer, raw_item_kind, classified_as, trusted_snapshot >>


Next ==
    \/ \E peer_id \in PeerIdValues : TrustPeer(peer_id)
    \/ \E raw_item_id \in RawItemIdValues : \E peer_id \in PeerIdValues : \E raw_kind \in RawPeerKindValues : ReceiveTrustedPeerEnvelope(raw_item_id, peer_id, raw_kind)
    \/ \E raw_item_id \in RawItemIdValues : \E peer_id \in PeerIdValues : \E raw_kind \in RawPeerKindValues : DropUntrustedPeerEnvelope(raw_item_id, peer_id, raw_kind)
    \/ \E raw_item_id \in RawItemIdValues : SubmitTypedPeerInputDelivered(raw_item_id)
    \/ \E raw_item_id \in RawItemIdValues : SubmitTypedPeerInputContinue(raw_item_id)
    \/ TerminalStutter

queued_items_are_classified == \A raw_item_id \in SeqElements(submission_queue) : (raw_item_id \in DOMAIN classified_as)

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(trusted_peers) <= 1 /\ Cardinality(DOMAIN raw_item_peer) <= 1 /\ Cardinality(DOMAIN raw_item_kind) <= 1 /\ Cardinality(DOMAIN classified_as) <= 1 /\ Cardinality(DOMAIN trusted_snapshot) <= 1 /\ Len(submission_queue) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(trusted_peers) <= 2 /\ Cardinality(DOMAIN raw_item_peer) <= 2 /\ Cardinality(DOMAIN raw_item_kind) <= 2 /\ Cardinality(DOMAIN classified_as) <= 2 /\ Cardinality(DOMAIN trusted_snapshot) <= 2 /\ Len(submission_queue) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []queued_items_are_classified

=============================================================================
