---- MODULE model ----
EXTENDS Naturals, Sequences, FiniteSets, TLC

\* Abstract, architecture-level model of RuntimeIngressMachine.
\* This is intentionally semantic rather than payload-detailed, and models
\* individually admitted inputs plus runtime-authoritative multi-contributor
\* staged runs.

CONSTANTS
    Inputs,
    RunIds,
    NoRun

ASSUME Inputs # {}
ASSUME NoRun \notin RunIds

LifecycleStates ==
    {"not_admitted", "accepted", "queued", "staged", "applied",
     "applied_pending_consumption", "consumed", "superseded",
     "coalesced", "abandoned"}

TerminalStates == {"consumed", "superseded", "coalesced", "abandoned"}

VARIABLES
    lifecycle,
    queue,
    admissionOrder,
    currentRun,
    currentContributors,
    lastRun,
    lastBoundary,
    wakeRequested,
    processRequested

vars ==
    << lifecycle, queue, admissionOrder, currentRun, currentContributors,
       lastRun, lastBoundary, wakeRequested, processRequested >>

IsTerminal(s) == s \in TerminalStates

InQueue(i) == \E n \in DOMAIN queue : queue[n] = i

OccursIn(seq, x) == \E n \in DOMAIN seq : seq[n] = x

AppendUnique(seq, x) ==
    IF \E n \in DOMAIN seq : seq[n] = x THEN seq ELSE Append(seq, x)

Init ==
    /\ lifecycle = [i \in Inputs |-> "not_admitted"]
    /\ queue = <<>>
    /\ admissionOrder = <<>>
    /\ currentRun = NoRun
    /\ currentContributors = <<>>
    /\ lastRun = [i \in Inputs |-> NoRun]
    /\ lastBoundary = [i \in Inputs |-> 0]
    /\ wakeRequested = FALSE
    /\ processRequested = FALSE

AdmitQueued(i, wake, process) ==
    /\ i \in Inputs
    /\ lifecycle[i] = "not_admitted"
    /\ lifecycle' = [lifecycle EXCEPT ![i] = "queued"]
    /\ queue' = Append(queue, i)
    /\ admissionOrder' = AppendUnique(admissionOrder, i)
    /\ currentRun' = currentRun
    /\ currentContributors' = currentContributors
    /\ lastRun' = lastRun
    /\ lastBoundary' = lastBoundary
    /\ wakeRequested' = (wakeRequested \/ wake)
    /\ processRequested' = (processRequested \/ process)

AdmitConsumedOnAccept(i) ==
    /\ i \in Inputs
    /\ lifecycle[i] = "not_admitted"
    /\ lifecycle' = [lifecycle EXCEPT ![i] = "consumed"]
    /\ queue' = queue
    /\ admissionOrder' = AppendUnique(admissionOrder, i)
    /\ currentRun' = currentRun
    /\ currentContributors' = currentContributors
    /\ lastRun' = lastRun
    /\ lastBoundary' = lastBoundary
    /\ wakeRequested' = wakeRequested
    /\ processRequested' = processRequested

StageDrainPrefix(r, n) ==
    /\ r \in RunIds
    /\ currentRun = NoRun
    /\ queue # <<>>
    /\ n \in 1..Len(queue)
    /\ LET batch == SubSeq(queue, 1, n) IN
       /\ \A k \in DOMAIN batch : lifecycle[batch[k]] = "queued"
       /\ lifecycle' =
            [i \in Inputs |->
                IF OccursIn(batch, i)
                THEN "staged"
                ELSE lifecycle[i]]
       /\ queue' =
            IF n = Len(queue)
            THEN <<>>
            ELSE SubSeq(queue, n + 1, Len(queue))
       /\ admissionOrder' = admissionOrder
       /\ currentRun' = r
       /\ currentContributors' = batch
       /\ lastRun' =
            [i \in Inputs |->
                IF OccursIn(batch, i)
                THEN r
                ELSE lastRun[i]]
       /\ lastBoundary' = lastBoundary
       /\ wakeRequested' = FALSE
       /\ processRequested' = FALSE

BoundaryApplied(r) ==
    /\ r \in RunIds
    /\ currentRun = r
    /\ currentContributors # <<>>
    /\ \A n \in DOMAIN currentContributors :
        lifecycle[currentContributors[n]] = "staged"
    /\ lifecycle' =
        [i \in Inputs |->
            IF OccursIn(currentContributors, i)
            THEN "applied_pending_consumption"
            ELSE lifecycle[i]]
    /\ queue' = queue
    /\ admissionOrder' = admissionOrder
    /\ currentRun' = currentRun
    /\ currentContributors' = currentContributors
    /\ lastRun' = lastRun
    /\ lastBoundary' =
        [i \in Inputs |->
            IF OccursIn(currentContributors, i)
            THEN lastBoundary[i] + 1
            ELSE lastBoundary[i]]
    /\ wakeRequested' = wakeRequested
    /\ processRequested' = processRequested

RunCompleted(r) ==
    /\ r \in RunIds
    /\ currentRun = r
    /\ currentContributors # <<>>
    /\ \A n \in DOMAIN currentContributors :
        lifecycle[currentContributors[n]] = "applied_pending_consumption"
    /\ lifecycle' =
        [i \in Inputs |->
            IF OccursIn(currentContributors, i)
            THEN "consumed"
            ELSE lifecycle[i]]
    /\ queue' = queue
    /\ admissionOrder' = admissionOrder
    /\ currentRun' = NoRun
    /\ currentContributors' = <<>>
    /\ lastRun' = lastRun
    /\ lastBoundary' = lastBoundary
    /\ wakeRequested' = wakeRequested
    /\ processRequested' = processRequested

RunFailedOrCancelled(r) ==
    /\ r \in RunIds
    /\ currentRun = r
    /\ currentContributors # <<>>
    /\ \A n \in DOMAIN currentContributors :
        lifecycle[currentContributors[n]] = "staged"
    /\ lifecycle' =
        [i \in Inputs |->
            IF OccursIn(currentContributors, i)
            THEN "queued"
            ELSE lifecycle[i]]
    /\ queue' = currentContributors \o queue
    /\ admissionOrder' = admissionOrder
    /\ currentRun' = NoRun
    /\ currentContributors' = <<>>
    /\ lastRun' = lastRun
    /\ lastBoundary' = lastBoundary
    /\ wakeRequested' = (queue' # <<>>)
    /\ processRequested' = processRequested

ResetOrDestroy ==
    /\ lifecycle' =
        [i \in Inputs |->
            IF ~IsTerminal(lifecycle[i]) /\ lifecycle[i] # "not_admitted"
            THEN "abandoned"
            ELSE lifecycle[i]]
    /\ queue' = <<>>
    /\ admissionOrder' = admissionOrder
    /\ currentRun' = NoRun
    /\ currentContributors' = <<>>
    /\ lastRun' = lastRun
    /\ lastBoundary' = lastBoundary
    /\ wakeRequested' = FALSE
    /\ processRequested' = FALSE

Next ==
    \/ \E i \in Inputs, wake \in BOOLEAN, process \in BOOLEAN :
        AdmitQueued(i, wake, process)
    \/ \E i \in Inputs : AdmitConsumedOnAccept(i)
    \/ \E r \in RunIds, n \in 1..Len(queue) : StageDrainPrefix(r, n)
    \/ \E r \in RunIds : BoundaryApplied(r)
    \/ \E r \in RunIds : RunCompleted(r)
    \/ \E r \in RunIds : RunFailedOrCancelled(r)
    \/ ResetOrDestroy

QueueContainsOnlyQueued ==
    \A i \in Inputs : InQueue(i) => lifecycle[i] = "queued"

TerminalNotQueued ==
    \A i \in Inputs : IsTerminal(lifecycle[i]) => ~InQueue(i)

AdmissionOrderUnique ==
    \A i \in Inputs :
        Cardinality({ n \in DOMAIN admissionOrder : admissionOrder[n] = i }) <= 1

NoIllegalAPCRollback ==
    \A i \in Inputs :
        lifecycle[i] = "applied_pending_consumption" => ~InQueue(i)

CurrentRunMatchesContributors ==
    (currentRun = NoRun) <=> (currentContributors = <<>>)

ContributorsNotQueued ==
    \A i \in Inputs :
        OccursIn(currentContributors, i) => ~InQueue(i)

Spec == Init /\ [][Next]_vars

THEOREM Spec => []QueueContainsOnlyQueued
THEOREM Spec => []TerminalNotQueued
THEOREM Spec => []AdmissionOrderUnique
THEOREM Spec => []NoIllegalAPCRollback
THEOREM Spec => []CurrentRunMatchesContributors
THEOREM Spec => []ContributorsNotQueued

=============================================================================
