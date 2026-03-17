---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobLifecycleMachine.

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

VARIABLES phase, model_step_count, active_run_count, cleanup_pending

vars == << phase, model_step_count, active_run_count, cleanup_pending >>

Init ==
    /\ phase = "Creating"
    /\ model_step_count = 0
    /\ active_run_count = 0
    /\ cleanup_pending = FALSE

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

Start ==
    /\ phase = "Creating" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run_count, cleanup_pending >>


Stop ==
    /\ phase = "Running"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run_count, cleanup_pending >>


Resume ==
    /\ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run_count, cleanup_pending >>


MarkCompleted ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count = 0)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run_count, cleanup_pending >>


Destroy ==
    /\ phase = "Creating" \/ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ cleanup_pending' = FALSE


StartRun ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << cleanup_pending >>


FinishRun ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) - 1
    /\ UNCHANGED << cleanup_pending >>


BeginCleanup ==
    /\ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_pending' = TRUE
    /\ UNCHANGED << active_run_count >>


FinishCleanup ==
    /\ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ cleanup_pending' = FALSE
    /\ UNCHANGED << active_run_count >>


Next ==
    \/ Start
    \/ Stop
    \/ Resume
    \/ MarkCompleted
    \/ Destroy
    \/ StartRun
    \/ FinishRun
    \/ BeginCleanup
    \/ FinishCleanup
    \/ TerminalStutter

destroyed_has_no_active_runs == ((phase # "Destroyed") \/ (active_run_count = 0))
completed_has_no_active_runs == ((phase # "Completed") \/ (active_run_count = 0))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []destroyed_has_no_active_runs
THEOREM Spec => []completed_has_no_active_runs

=============================================================================
