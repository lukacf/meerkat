---- MODULE model ----
EXTENDS TLC

\* Abstract, per-input lifecycle model for InputLifecycleMachine.

CONSTANTS
    RunIds,
    NoRun,
    BoundaryIds,
    NoBoundary

ASSUME NoRun \notin RunIds
ASSUME NoBoundary \notin BoundaryIds

States ==
    {"accepted", "queued", "staged", "applied",
     "applied_pending_consumption", "consumed",
     "superseded", "coalesced", "abandoned"}

TerminalStates == {"consumed", "superseded", "coalesced", "abandoned"}
Outcomes == {"none", "consumed", "superseded", "coalesced", "abandoned"}

VARIABLES
    state,
    terminalOutcome,
    lastRun,
    lastBoundary

vars == << state, terminalOutcome, lastRun, lastBoundary >>

Init ==
    /\ state = "accepted"
    /\ terminalOutcome = "none"
    /\ lastRun = NoRun
    /\ lastBoundary = NoBoundary

QueueAccepted ==
    /\ state = "accepted"
    /\ state' = "queued"
    /\ UNCHANGED << terminalOutcome, lastRun, lastBoundary >>

StageForRun(r) ==
    /\ r \in RunIds
    /\ state = "queued"
    /\ state' = "staged"
    /\ lastRun' = r
    /\ UNCHANGED << terminalOutcome, lastBoundary >>

RollbackStaged ==
    /\ state = "staged"
    /\ state' = "queued"
    /\ UNCHANGED << terminalOutcome, lastRun, lastBoundary >>

MarkApplied ==
    /\ state = "staged"
    /\ lastRun # NoRun
    /\ state' = "applied"
    /\ UNCHANGED << terminalOutcome, lastRun, lastBoundary >>

MarkAppliedPendingConsumption(b) ==
    /\ b \in BoundaryIds
    /\ state = "applied"
    /\ state' = "applied_pending_consumption"
    /\ lastBoundary' = b
    /\ UNCHANGED << terminalOutcome, lastRun >>

Consume ==
    /\ state = "applied_pending_consumption"
    /\ state' = "consumed"
    /\ terminalOutcome' = "consumed"
    /\ UNCHANGED << lastRun, lastBoundary >>

Supersede ==
    /\ state \in {"accepted", "queued", "staged"}
    /\ state' = "superseded"
    /\ terminalOutcome' = "superseded"
    /\ UNCHANGED << lastRun, lastBoundary >>

Coalesce ==
    /\ state \in {"accepted", "queued"}
    /\ state' = "coalesced"
    /\ terminalOutcome' = "coalesced"
    /\ UNCHANGED << lastRun, lastBoundary >>

Abandon ==
    /\ state \in {"accepted", "queued", "staged", "applied", "applied_pending_consumption"}
    /\ state' = "abandoned"
    /\ terminalOutcome' = "abandoned"
    /\ UNCHANGED << lastRun, lastBoundary >>

TerminalStutter ==
    /\ state \in TerminalStates
    /\ UNCHANGED vars

Next ==
    \/ QueueAccepted
    \/ \E r \in RunIds : StageForRun(r)
    \/ RollbackStaged
    \/ MarkApplied
    \/ \E b \in BoundaryIds : MarkAppliedPendingConsumption(b)
    \/ Consume
    \/ Supersede
    \/ Coalesce
    \/ Abandon
    \/ TerminalStutter

TerminalOutcomeMatchesState ==
    /\ (state = "consumed") => terminalOutcome = "consumed"
    /\ (state = "superseded") => terminalOutcome = "superseded"
    /\ (state = "coalesced") => terminalOutcome = "coalesced"
    /\ (state = "abandoned") => terminalOutcome = "abandoned"
    /\ (state \notin TerminalStates) => terminalOutcome = "none"

BoundaryOnlyAfterApply ==
    lastBoundary # NoBoundary =>
        state \in {"applied_pending_consumption", "consumed", "abandoned"}

AcceptedHasNoExecutionMetadata ==
    state = "accepted" => /\ lastRun = NoRun /\ lastBoundary = NoBoundary

CoalescedNeverHasBoundaryMetadata ==
    state = "coalesced" => lastBoundary = NoBoundary

Spec == Init /\ [][Next]_vars

THEOREM Spec => []TerminalOutcomeMatchesState
THEOREM Spec => []BoundaryOnlyAfterApply
THEOREM Spec => []AcceptedHasNoExecutionMetadata
THEOREM Spec => []CoalescedNeverHasBoundaryMetadata

=============================================================================
