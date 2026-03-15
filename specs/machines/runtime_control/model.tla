---- MODULE model ----
EXTENDS Naturals, TLC

\* Abstract, architecture-level model of RuntimeControlMachine.
\* This models lifecycle/control/scheduling, not queue internals.

CONSTANTS RunIds, NoRun, MaxBacklog

ASSUME NoRun \notin RunIds
ASSUME MaxBacklog \in Nat

RuntimeStates ==
    {"initializing", "idle", "running", "recovering",
     "retired", "stopped", "destroyed"}

VARIABLES
    rtState,
    currentRun,
    preRunState,
    wakePending,
    processPending,
    backlog

vars == << rtState, currentRun, preRunState, wakePending, processPending, backlog >>

Init ==
    /\ rtState = "initializing"
    /\ currentRun = NoRun
    /\ preRunState = "none"
    /\ wakePending = FALSE
    /\ processPending = FALSE
    /\ backlog = 0

Initialize ==
    /\ rtState = "initializing"
    /\ rtState' = "idle"
    /\ UNCHANGED << currentRun, preRunState, wakePending, processPending, backlog >>

AdmitWork(wake, process) ==
    /\ rtState \in {"idle", "running"}
    /\ backlog < MaxBacklog
    /\ rtState' = rtState
    /\ currentRun' = currentRun
    /\ preRunState' = preRunState
    /\ wakePending' = (wakePending \/ wake)
    /\ processPending' = (processPending \/ process)
    /\ backlog' = backlog + 1

StartRun(r) ==
    /\ r \in RunIds
    /\ rtState \in {"idle", "retired"}
    /\ currentRun = NoRun
    /\ backlog > 0
    /\ rtState' = "running"
    /\ currentRun' = r
    /\ preRunState' = rtState
    /\ wakePending' = FALSE
    /\ processPending' = FALSE
    /\ backlog' = backlog - 1

FinishRun ==
    /\ rtState \in {"running", "retired"}
    /\ currentRun # NoRun
    /\ rtState' =
        IF rtState = "retired" \/ preRunState = "retired" THEN "retired" ELSE "idle"
    /\ currentRun' = NoRun
    /\ preRunState' = "none"
    /\ wakePending' = wakePending
    /\ processPending' = processPending
    /\ backlog' = backlog

RecoverRequested ==
    /\ rtState \in {"idle", "running"}
    /\ rtState' = "recovering"
    /\ currentRun' = NoRun
    /\ preRunState' = "none"
    /\ wakePending' = wakePending
    /\ processPending' = processPending
    /\ backlog' = backlog

ResumeRequested ==
    /\ rtState = "recovering"
    /\ rtState' = "idle"
    /\ currentRun' = NoRun
    /\ preRunState' = "none"
    /\ wakePending' = wakePending
    /\ processPending' = processPending
    /\ backlog' = backlog

RetireRequested ==
    /\ rtState \in {"idle", "running"}
    /\ rtState' = "retired"
    /\ currentRun' = IF rtState = "running" THEN currentRun ELSE NoRun
    /\ preRunState' = IF rtState = "running" THEN "retired" ELSE preRunState
    /\ wakePending' = wakePending
    /\ processPending' = processPending
    /\ backlog' = backlog

ResetRequested ==
    /\ rtState # "running"
    /\ rtState \notin {"stopped", "destroyed"}
    /\ rtState' = "idle"
    /\ currentRun' = NoRun
    /\ preRunState' = "none"
    /\ wakePending' = FALSE
    /\ processPending' = FALSE
    /\ backlog' = 0

StopRequested ==
    /\ rtState \notin {"stopped", "destroyed"}
    /\ rtState' = "stopped"
    /\ currentRun' = NoRun
    /\ preRunState' = "none"
    /\ wakePending' = FALSE
    /\ processPending' = FALSE
    /\ backlog' = 0

DestroyRequested ==
    /\ rtState # "destroyed"
    /\ rtState' = "destroyed"
    /\ currentRun' = NoRun
    /\ preRunState' = "none"
    /\ wakePending' = FALSE
    /\ processPending' = FALSE
    /\ backlog' = 0

TerminalStutter ==
    /\ rtState \in {"stopped", "destroyed"}
    /\ UNCHANGED vars

Next ==
    \/ Initialize
    \/ \E wake \in BOOLEAN, process \in BOOLEAN : AdmitWork(wake, process)
    \/ \E r \in RunIds : StartRun(r)
    \/ FinishRun
    \/ RecoverRequested
    \/ ResumeRequested
    \/ RetireRequested
    \/ ResetRequested
    \/ StopRequested
    \/ DestroyRequested
    \/ TerminalStutter

RunningImpliesRunId ==
    (rtState = "running") => (currentRun # NoRun)

ActiveRunOnlyInRunningOrRetired ==
    (currentRun # NoRun) => (rtState \in {"running", "retired"})

TerminalStatesSticky ==
    rtState \in {"stopped", "destroyed"} =>
        /\ wakePending = FALSE
        /\ processPending = FALSE
        /\ backlog = 0

RetiredRejectsAdmission ==
    rtState = "retired" => currentRun = NoRun \/ preRunState = "retired"

Spec == Init /\ [][Next]_vars

THEOREM Spec => []RunningImpliesRunId
THEOREM Spec => []ActiveRunOnlyInRunningOrRetired
THEOREM Spec => []TerminalStatesSticky

=============================================================================
