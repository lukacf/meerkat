---- MODULE model ----
EXTENDS Sequences, TLC

\* Abstract, architecture-level model of PeerCommsMachine.

CONSTANTS
    Items,
    Peers,
    NoPeer

ASSUME Items # {}
ASSUME Peers # {}
ASSUME NoPeer \notin Peers

IngressStates == {"absent", "received", "dropped", "submitted", "delivered"}
Kinds ==
    {"none", "message", "request", "response", "silent_request",
     "peer_added", "peer_retired", "plain_event", "ack"}
Classes ==
    {"none", "actionable_message", "actionable_request", "response",
     "silent_request", "peer_lifecycle_added", "peer_lifecycle_retired",
     "plain_event"}

ClassFor(k) ==
    CASE k = "message" -> "actionable_message"
      [] k = "request" -> "actionable_request"
      [] k = "response" -> "response"
      [] k = "silent_request" -> "silent_request"
      [] k = "peer_added" -> "peer_lifecycle_added"
      [] k = "peer_retired" -> "peer_lifecycle_retired"
      [] k = "plain_event" -> "plain_event"
      [] OTHER -> "none"

VARIABLES
    trusted,
    itemState,
    itemKind,
    itemPeer,
    itemClass,
    trustedSnapshot,
    submissionQueue

vars ==
    << trusted, itemState, itemKind, itemPeer,
       itemClass, trustedSnapshot, submissionQueue >>

Init ==
    /\ trusted = [p \in Peers |-> FALSE]
    /\ itemState = [i \in Items |-> "absent"]
    /\ itemKind = [i \in Items |-> "none"]
    /\ itemPeer = [i \in Items |-> NoPeer]
    /\ itemClass = [i \in Items |-> "none"]
    /\ trustedSnapshot = [i \in Items |-> FALSE]
    /\ submissionQueue = <<>>

TrustPeer(p) ==
    /\ p \in Peers
    /\ trusted' = [trusted EXCEPT ![p] = TRUE]
    /\ UNCHANGED << itemState, itemKind, itemPeer,
                   itemClass, trustedSnapshot, submissionQueue >>

UntrustPeer(p) ==
    /\ p \in Peers
    /\ trusted' = [trusted EXCEPT ![p] = FALSE]
    /\ UNCHANGED << itemState, itemKind, itemPeer,
                   itemClass, trustedSnapshot, submissionQueue >>

ReceivePeerEnvelope(i, p, k) ==
    /\ i \in Items
    /\ p \in Peers
    /\ k \in {"message", "request", "response", "silent_request",
              "peer_added", "peer_retired", "ack"}
    /\ itemState[i] = "absent"
    /\ itemState' = [itemState EXCEPT ![i] = "received"]
    /\ itemKind' = [itemKind EXCEPT ![i] = k]
    /\ itemPeer' = [itemPeer EXCEPT ![i] = p]
    /\ UNCHANGED << trusted, itemClass, trustedSnapshot, submissionQueue >>

ReceivePeerlessPlainEvent(i) ==
    /\ i \in Items
    /\ itemState[i] = "absent"
    /\ itemState' = [itemState EXCEPT ![i] = "received"]
    /\ itemKind' = [itemKind EXCEPT ![i] = "plain_event"]
    /\ itemPeer' = [itemPeer EXCEPT ![i] = NoPeer]
    /\ UNCHANGED << trusted, itemClass, trustedSnapshot, submissionQueue >>

DropInvalid(i) ==
    /\ i \in Items
    /\ itemState[i] = "received"
    /\ itemState' = [itemState EXCEPT ![i] = "dropped"]
    /\ UNCHANGED << trusted, itemKind, itemPeer,
                   itemClass, trustedSnapshot, submissionQueue >>

SubmitTypedPeerInput(i) ==
    /\ i \in Items
    /\ itemState[i] = "received"
    /\ itemKind[i] # "ack"
    /\ IF itemPeer[i] = NoPeer THEN TRUE ELSE trusted[itemPeer[i]]
    /\ ClassFor(itemKind[i]) # "none"
    /\ itemState' = [itemState EXCEPT ![i] = "submitted"]
    /\ itemClass' = [itemClass EXCEPT ![i] = ClassFor(itemKind[i])]
    /\ trustedSnapshot' =
        [trustedSnapshot EXCEPT
            ![i] = IF itemPeer[i] = NoPeer THEN TRUE ELSE trusted[itemPeer[i]]]
    /\ submissionQueue' = Append(submissionQueue, i)
    /\ UNCHANGED << trusted, itemKind, itemPeer >>

DeliverSubmitted ==
    /\ submissionQueue # <<>>
    /\ LET i == Head(submissionQueue) IN
       /\ itemState[i] = "submitted"
       /\ itemState' = [itemState EXCEPT ![i] = "delivered"]
       /\ submissionQueue' = SubSeq(submissionQueue, 2, Len(submissionQueue))
       /\ UNCHANGED << trusted, itemKind, itemPeer, itemClass, trustedSnapshot >>

Next ==
    \/ \E p \in Peers : TrustPeer(p)
    \/ \E p \in Peers : UntrustPeer(p)
    \/ \E i \in Items, p \in Peers,
         k \in {"message", "request", "response", "silent_request",
                "peer_added", "peer_retired", "ack"} :
            ReceivePeerEnvelope(i, p, k)
    \/ \E i \in Items : ReceivePeerlessPlainEvent(i)
    \/ \E i \in Items : DropInvalid(i)
    \/ \E i \in Items : SubmitTypedPeerInput(i)
    \/ DeliverSubmitted

QueueContainsOnlySubmitted ==
    \A i \in Items :
        (\E n \in DOMAIN submissionQueue : submissionQueue[n] = i) =>
            itemState[i] = "submitted"

SubmittedItemsHaveClass ==
    \A i \in Items :
        itemState[i] \in {"submitted", "delivered"} =>
            /\ itemClass[i] # "none"
            /\ itemClass[i] = ClassFor(itemKind[i])

AckNeverSubmitted ==
    \A i \in Items :
        itemKind[i] = "ack" => itemState[i] \notin {"submitted", "delivered"}

SubmittedPeerItemsTrustedAtAdmission ==
    \A i \in Items :
        itemState[i] \in {"submitted", "delivered"} /\ itemPeer[i] # NoPeer =>
            trustedSnapshot[i]

PeerlessItemsArePlainEvents ==
    \A i \in Items :
        itemState[i] # "absent" /\ itemPeer[i] = NoPeer =>
            itemKind[i] = "plain_event"

Spec == Init /\ [][Next]_vars

THEOREM Spec => []QueueContainsOnlySubmitted
THEOREM Spec => []SubmittedItemsHaveClass
THEOREM Spec => []AckNeverSubmitted
THEOREM Spec => []SubmittedPeerItemsTrustedAtAdmission
THEOREM Spec => []PeerlessItemsArePlainEvents

=============================================================================
