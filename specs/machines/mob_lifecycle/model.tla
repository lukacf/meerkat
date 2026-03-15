---- MODULE model ----
EXTENDS Naturals, TLC

\* Abstract, architecture-level model of MobLifecycleMachine.

CONSTANT MaxFlowTrackers

ASSUME MaxFlowTrackers \in Nat

States == {"creating", "running", "stopped", "completed", "destroyed"}

VARIABLES
    mobState,
    hostRuntimeActive,
    mcpSurfaceActive,
    trackedFlowCount

vars == << mobState, hostRuntimeActive, mcpSurfaceActive, trackedFlowCount >>

Init ==
    /\ mobState = "creating"
    /\ hostRuntimeActive = FALSE
    /\ mcpSurfaceActive = FALSE
    /\ trackedFlowCount = 0

EnterRunning ==
    /\ mobState = "creating"
    /\ mobState' = "running"
    /\ hostRuntimeActive' = TRUE
    /\ mcpSurfaceActive' = TRUE
    /\ UNCHANGED trackedFlowCount

TrackFlow ==
    /\ mobState = "running"
    /\ trackedFlowCount < MaxFlowTrackers
    /\ trackedFlowCount' = trackedFlowCount + 1
    /\ UNCHANGED << mobState, hostRuntimeActive, mcpSurfaceActive >>

ReleaseTrackedFlow ==
    /\ trackedFlowCount > 0
    /\ trackedFlowCount' = trackedFlowCount - 1
    /\ UNCHANGED << mobState, hostRuntimeActive, mcpSurfaceActive >>

StopMob ==
    /\ mobState = "running"
    /\ mobState' = "stopped"
    /\ hostRuntimeActive' = FALSE
    /\ mcpSurfaceActive' = FALSE
    /\ trackedFlowCount' = 0

ResumeMob ==
    /\ mobState = "stopped"
    /\ mobState' = "running"
    /\ hostRuntimeActive' = TRUE
    /\ mcpSurfaceActive' = TRUE
    /\ UNCHANGED trackedFlowCount

CompleteMob ==
    /\ mobState = "running"
    /\ trackedFlowCount = 0
    /\ mobState' = "completed"
    /\ hostRuntimeActive' = FALSE
    /\ mcpSurfaceActive' = FALSE
    /\ UNCHANGED trackedFlowCount

DestroyMob ==
    /\ mobState \in {"creating", "running", "stopped", "completed"}
    /\ mobState' = "destroyed"
    /\ hostRuntimeActive' = FALSE
    /\ mcpSurfaceActive' = FALSE
    /\ trackedFlowCount' = 0

ResetMob ==
    /\ mobState \in {"running", "stopped", "completed"}
    /\ mobState' = "running"
    /\ hostRuntimeActive' = TRUE
    /\ mcpSurfaceActive' = TRUE
    /\ trackedFlowCount' = 0

DestroyedStutter ==
    /\ mobState = "destroyed"
    /\ UNCHANGED vars

Next ==
    \/ EnterRunning
    \/ TrackFlow
    \/ ReleaseTrackedFlow
    \/ StopMob
    \/ ResumeMob
    \/ CompleteMob
    \/ DestroyMob
    \/ ResetMob
    \/ DestroyedStutter

DestroyedImpliesDrained ==
    mobState = "destroyed" =>
        /\ ~hostRuntimeActive
        /\ ~mcpSurfaceActive
        /\ trackedFlowCount = 0

RunningImpliesInfrastructure ==
    mobState = "running" => /\ hostRuntimeActive /\ mcpSurfaceActive

TrackedFlowsOnlyWhileRunning ==
    trackedFlowCount > 0 => mobState = "running"

CompletedHasNoTrackedFlows ==
    mobState = "completed" => trackedFlowCount = 0

Spec == Init /\ [][Next]_vars

THEOREM Spec => []DestroyedImpliesDrained
THEOREM Spec => []RunningImpliesInfrastructure
THEOREM Spec => []TrackedFlowsOnlyWhileRunning
THEOREM Spec => []CompletedHasNoTrackedFlows

=============================================================================
