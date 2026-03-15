---- MODULE model ----
EXTENDS Naturals, TLC

\* Abstract, architecture-level model of MobOrchestratorMachine.

CONSTANTS
    MaxPendingSpawns,
    MaxActiveFlows,
    MaxTopologyRevisions

ASSUME MaxPendingSpawns \in Nat
ASSUME MaxActiveFlows \in Nat
ASSUME MaxTopologyRevisions \in Nat

States == {"creating", "running", "stopped", "completed", "destroyed"}

VARIABLES
    orchestratorState,
    coordinatorBound,
    pendingSpawnCount,
    activeFlowCount,
    topologyRevision,
    supervisorActive

vars ==
    << orchestratorState, coordinatorBound, pendingSpawnCount,
       activeFlowCount, topologyRevision, supervisorActive >>

Init ==
    /\ orchestratorState = "creating"
    /\ coordinatorBound = FALSE
    /\ pendingSpawnCount = 0
    /\ activeFlowCount = 0
    /\ topologyRevision = 0
    /\ supervisorActive = FALSE

InitializeOrchestrator ==
    /\ orchestratorState = "creating"
    /\ orchestratorState' = "running"
    /\ coordinatorBound' = TRUE
    /\ supervisorActive' = TRUE
    /\ UNCHANGED << pendingSpawnCount, activeFlowCount, topologyRevision >>

BindCoordinator ==
    /\ orchestratorState \in {"creating", "running", "stopped", "completed"}
    /\ ~coordinatorBound
    /\ coordinatorBound' = TRUE
    /\ supervisorActive' =
        IF orchestratorState = "running" THEN TRUE ELSE supervisorActive
    /\ UNCHANGED << orchestratorState, pendingSpawnCount,
                   activeFlowCount, topologyRevision >>

UnbindCoordinator ==
    /\ orchestratorState \in {"running", "stopped", "completed"}
    /\ coordinatorBound
    /\ coordinatorBound' = FALSE
    /\ pendingSpawnCount' = 0
    /\ activeFlowCount' = 0
    /\ supervisorActive' = FALSE
    /\ UNCHANGED << orchestratorState, topologyRevision >>

StageSpawn ==
    /\ orchestratorState = "running"
    /\ coordinatorBound
    /\ pendingSpawnCount < MaxPendingSpawns
    /\ pendingSpawnCount' = pendingSpawnCount + 1
    /\ UNCHANGED << orchestratorState, coordinatorBound,
                   activeFlowCount, topologyRevision, supervisorActive >>

CompleteSpawn ==
    /\ pendingSpawnCount > 0
    /\ pendingSpawnCount' = pendingSpawnCount - 1
    /\ UNCHANGED << orchestratorState, coordinatorBound,
                   activeFlowCount, topologyRevision, supervisorActive >>

StartFlowOwnership ==
    /\ orchestratorState = "running"
    /\ coordinatorBound
    /\ activeFlowCount < MaxActiveFlows
    /\ activeFlowCount' = activeFlowCount + 1
    /\ UNCHANGED << orchestratorState, coordinatorBound,
                   pendingSpawnCount, topologyRevision, supervisorActive >>

FinishFlowOwnership ==
    /\ activeFlowCount > 0
    /\ activeFlowCount' = activeFlowCount - 1
    /\ UNCHANGED << orchestratorState, coordinatorBound,
                   pendingSpawnCount, topologyRevision, supervisorActive >>

AdvanceTopology ==
    /\ orchestratorState \in {"running", "stopped", "completed"}
    /\ topologyRevision < MaxTopologyRevisions
    /\ topologyRevision' = topologyRevision + 1
    /\ UNCHANGED << orchestratorState, coordinatorBound,
                   pendingSpawnCount, activeFlowCount, supervisorActive >>

StopOrchestration ==
    /\ orchestratorState = "running"
    /\ orchestratorState' = "stopped"
    /\ pendingSpawnCount' = 0
    /\ activeFlowCount' = 0
    /\ supervisorActive' = FALSE
    /\ UNCHANGED << coordinatorBound, topologyRevision >>

ResumeOrchestration ==
    /\ orchestratorState = "stopped"
    /\ coordinatorBound
    /\ orchestratorState' = "running"
    /\ supervisorActive' = TRUE
    /\ UNCHANGED << coordinatorBound, pendingSpawnCount,
                   activeFlowCount, topologyRevision >>

CompleteOrchestration ==
    /\ orchestratorState = "running"
    /\ pendingSpawnCount = 0
    /\ activeFlowCount = 0
    /\ orchestratorState' = "completed"
    /\ supervisorActive' = FALSE
    /\ UNCHANGED << coordinatorBound, pendingSpawnCount,
                   activeFlowCount, topologyRevision >>

DestroyOrchestration ==
    /\ orchestratorState \in {"creating", "running", "stopped", "completed"}
    /\ orchestratorState' = "destroyed"
    /\ coordinatorBound' = FALSE
    /\ pendingSpawnCount' = 0
    /\ activeFlowCount' = 0
    /\ supervisorActive' = FALSE
    /\ UNCHANGED topologyRevision

ResetOrchestration ==
    /\ orchestratorState \in {"running", "stopped", "completed"}
    /\ orchestratorState' = "running"
    /\ pendingSpawnCount' = 0
    /\ activeFlowCount' = 0
    /\ supervisorActive' = coordinatorBound
    /\ UNCHANGED << coordinatorBound, topologyRevision >>

DestroyedStutter ==
    /\ orchestratorState = "destroyed"
    /\ UNCHANGED vars

Next ==
    \/ InitializeOrchestrator
    \/ BindCoordinator
    \/ UnbindCoordinator
    \/ StageSpawn
    \/ CompleteSpawn
    \/ StartFlowOwnership
    \/ FinishFlowOwnership
    \/ AdvanceTopology
    \/ StopOrchestration
    \/ ResumeOrchestration
    \/ CompleteOrchestration
    \/ DestroyOrchestration
    \/ ResetOrchestration
    \/ DestroyedStutter

DestroyedImpliesDrained ==
    orchestratorState = "destroyed" =>
        /\ ~coordinatorBound
        /\ pendingSpawnCount = 0
        /\ activeFlowCount = 0
        /\ ~supervisorActive

ActiveFlowsRequireBoundRunningCoordinator ==
    activeFlowCount > 0 =>
        /\ orchestratorState = "running"
        /\ coordinatorBound

PendingSpawnsRequireBoundRunningCoordinator ==
    pendingSpawnCount > 0 =>
        /\ orchestratorState = "running"
        /\ coordinatorBound

SupervisorRequiresBoundRunningCoordinator ==
    supervisorActive =>
        /\ orchestratorState = "running"
        /\ coordinatorBound

CompletedIsQuiescent ==
    orchestratorState = "completed" =>
        /\ pendingSpawnCount = 0
        /\ activeFlowCount = 0
        /\ ~supervisorActive

Spec == Init /\ [][Next]_vars

THEOREM Spec => []DestroyedImpliesDrained
THEOREM Spec => []ActiveFlowsRequireBoundRunningCoordinator
THEOREM Spec => []PendingSpawnsRequireBoundRunningCoordinator
THEOREM Spec => []SupervisorRequiresBoundRunningCoordinator
THEOREM Spec => []CompletedIsQuiescent

=============================================================================
