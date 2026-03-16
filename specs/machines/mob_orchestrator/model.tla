---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobOrchestratorMachine.

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

VARIABLES phase, model_step_count, coordinator_bound, pending_spawn_count, active_flow_count, topology_revision, supervisor_active

vars == << phase, model_step_count, coordinator_bound, pending_spawn_count, active_flow_count, topology_revision, supervisor_active >>

Init ==
    /\ phase = "Creating"
    /\ model_step_count = 0
    /\ coordinator_bound = FALSE
    /\ pending_spawn_count = 0
    /\ active_flow_count = 0
    /\ topology_revision = 0
    /\ supervisor_active = FALSE

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

InitializeOrchestrator ==
    /\ phase = "Creating"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_active' = TRUE
    /\ UNCHANGED << coordinator_bound, pending_spawn_count, active_flow_count, topology_revision >>


BindCoordinator ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ (coordinator_bound = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ topology_revision' = (topology_revision) + 1
    /\ UNCHANGED << pending_spawn_count, active_flow_count, supervisor_active >>


UnbindCoordinator ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ (coordinator_bound = TRUE)
    /\ (pending_spawn_count = 0)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ topology_revision' = (topology_revision) + 1
    /\ UNCHANGED << pending_spawn_count, active_flow_count, supervisor_active >>


StageSpawn ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) + 1
    /\ topology_revision' = (topology_revision) + 1
    /\ UNCHANGED << coordinator_bound, active_flow_count, supervisor_active >>


CompleteSpawn ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (pending_spawn_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) - 1
    /\ topology_revision' = (topology_revision) + 1
    /\ UNCHANGED << coordinator_bound, active_flow_count, supervisor_active >>


StartFlow ==
    /\ phase = "Running" \/ phase = "Completed"
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_flow_count' = (active_flow_count) + 1
    /\ UNCHANGED << coordinator_bound, pending_spawn_count, topology_revision, supervisor_active >>


CompleteFlow ==
    /\ phase = "Running" \/ phase = "Completed"
    /\ (active_flow_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_flow_count' = (active_flow_count) - 1
    /\ UNCHANGED << coordinator_bound, pending_spawn_count, topology_revision, supervisor_active >>


StopOrchestrator ==
    /\ phase = "Running" \/ phase = "Completed"
    /\ (active_flow_count = 0)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_active' = FALSE
    /\ UNCHANGED << coordinator_bound, pending_spawn_count, active_flow_count, topology_revision >>


ResumeOrchestrator ==
    /\ phase = "Stopped"
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_active' = TRUE
    /\ UNCHANGED << coordinator_bound, pending_spawn_count, active_flow_count, topology_revision >>


MarkCompleted ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_flow_count = 0)
    /\ (pending_spawn_count = 0)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << coordinator_bound, pending_spawn_count, active_flow_count, topology_revision, supervisor_active >>


DestroyOrchestrator ==
    /\ phase = "Stopped" \/ phase = "Completed"
    /\ (pending_spawn_count = 0)
    /\ (active_flow_count = 0)
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ supervisor_active' = FALSE
    /\ UNCHANGED << pending_spawn_count, active_flow_count, topology_revision >>


Next ==
    \/ InitializeOrchestrator
    \/ BindCoordinator
    \/ UnbindCoordinator
    \/ StageSpawn
    \/ CompleteSpawn
    \/ StartFlow
    \/ CompleteFlow
    \/ StopOrchestrator
    \/ ResumeOrchestrator
    \/ MarkCompleted
    \/ DestroyOrchestrator
    \/ TerminalStutter

destroyed_is_terminal == ((phase # "Destroyed") \/ (supervisor_active # TRUE))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []destroyed_is_terminal

=============================================================================
