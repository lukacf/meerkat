---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for CommsDrainLifecycleMachine.

CONSTANTS CommsDrainModeValues, DrainExitReasonValues

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

VARIABLES phase, model_step_count, mode, suppresses_turn_boundary_drain

vars == << phase, model_step_count, mode, suppresses_turn_boundary_drain >>

Init ==
    /\ phase = "Inactive"
    /\ model_step_count = 0
    /\ mode = None
    /\ suppresses_turn_boundary_drain = FALSE

TerminalStutter ==
    /\ phase = "Stopped"
    /\ UNCHANGED vars

EnsureRunningFromInactive(arg_mode) ==
    /\ phase = "Inactive"
    /\ phase' = "Starting"
    /\ model_step_count' = model_step_count + 1
    /\ mode' = Some(arg_mode)
    /\ suppresses_turn_boundary_drain' = TRUE


TaskSpawnedFromStarting ==
    /\ phase = "Starting"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << mode, suppresses_turn_boundary_drain >>


TaskExitedFromStartingRespawnable(reason) ==
    /\ phase = "Starting"
    /\ (reason = "Failed")
    /\ (mode = Some("PersistentHost"))
    /\ phase' = "ExitedRespawnable"
    /\ model_step_count' = model_step_count + 1
    /\ suppresses_turn_boundary_drain' = FALSE
    /\ UNCHANGED << mode >>


TaskExitedFromStartingStopped(reason) ==
    /\ phase = "Starting"
    /\ ((reason # "Failed") \/ (mode # Some("PersistentHost")))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ suppresses_turn_boundary_drain' = FALSE
    /\ UNCHANGED << mode >>


TaskExitedFromRunningRespawnable(reason) ==
    /\ phase = "Running"
    /\ (reason = "Failed")
    /\ (mode = Some("PersistentHost"))
    /\ phase' = "ExitedRespawnable"
    /\ model_step_count' = model_step_count + 1
    /\ suppresses_turn_boundary_drain' = FALSE
    /\ UNCHANGED << mode >>


TaskExitedFromRunningStopped(reason) ==
    /\ phase = "Running"
    /\ ((reason # "Failed") \/ (mode # Some("PersistentHost")))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ suppresses_turn_boundary_drain' = FALSE
    /\ UNCHANGED << mode >>


StopRequestedFromRunning ==
    /\ phase = "Running"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ suppresses_turn_boundary_drain' = FALSE
    /\ UNCHANGED << mode >>


StopRequestedFromStarting ==
    /\ phase = "Starting"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ suppresses_turn_boundary_drain' = FALSE
    /\ UNCHANGED << mode >>


EnsureRunningFromExitedRespawnable(arg_mode) ==
    /\ phase = "ExitedRespawnable"
    /\ phase' = "Starting"
    /\ model_step_count' = model_step_count + 1
    /\ mode' = Some(arg_mode)
    /\ suppresses_turn_boundary_drain' = TRUE


StopRequestedFromExitedRespawnable ==
    /\ phase = "ExitedRespawnable"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << mode, suppresses_turn_boundary_drain >>


EnsureRunningFromStopped(arg_mode) ==
    /\ phase = "Stopped"
    /\ phase' = "Starting"
    /\ model_step_count' = model_step_count + 1
    /\ mode' = Some(arg_mode)
    /\ suppresses_turn_boundary_drain' = TRUE


AbortObservedFromActive ==
    /\ phase = "Running" \/ phase = "Starting"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ suppresses_turn_boundary_drain' = FALSE
    /\ UNCHANGED << mode >>


Next ==
    \/ \E arg_mode \in CommsDrainModeValues : EnsureRunningFromInactive(arg_mode)
    \/ TaskSpawnedFromStarting
    \/ \E reason \in DrainExitReasonValues : TaskExitedFromStartingRespawnable(reason)
    \/ \E reason \in DrainExitReasonValues : TaskExitedFromStartingStopped(reason)
    \/ \E reason \in DrainExitReasonValues : TaskExitedFromRunningRespawnable(reason)
    \/ \E reason \in DrainExitReasonValues : TaskExitedFromRunningStopped(reason)
    \/ StopRequestedFromRunning
    \/ StopRequestedFromStarting
    \/ \E arg_mode \in CommsDrainModeValues : EnsureRunningFromExitedRespawnable(arg_mode)
    \/ StopRequestedFromExitedRespawnable
    \/ \E arg_mode \in CommsDrainModeValues : EnsureRunningFromStopped(arg_mode)
    \/ AbortObservedFromActive
    \/ TerminalStutter

active_implies_mode_set == (((phase # "Starting") /\ (phase # "Running")) \/ (mode # None))
active_implies_suppression == (((phase # "Starting") /\ (phase # "Running")) \/ (suppresses_turn_boundary_drain = TRUE))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []active_implies_mode_set
THEOREM Spec => []active_implies_suppression

=============================================================================
