---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobHelperResultAnchorMachine.

CONSTANTS RunIdValues

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

VARIABLES phase, model_step_count, observed_completed_runs, observed_failed_runs, observed_cancelled_runs, force_cancel_count

vars == << phase, model_step_count, observed_completed_runs, observed_failed_runs, observed_cancelled_runs, force_cancel_count >>

Init ==
    /\ phase = "Tracking"
    /\ model_step_count = 0
    /\ observed_completed_runs = {}
    /\ observed_failed_runs = {}
    /\ observed_cancelled_runs = {}
    /\ force_cancel_count = 0

AnchorCompleted(run_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ observed_completed_runs' = (observed_completed_runs \cup {run_id})
    /\ observed_failed_runs' = (observed_failed_runs \ {run_id})
    /\ observed_cancelled_runs' = (observed_cancelled_runs \ {run_id})
    /\ UNCHANGED << force_cancel_count >>


AnchorFailed(run_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ observed_completed_runs' = (observed_completed_runs \ {run_id})
    /\ observed_failed_runs' = (observed_failed_runs \cup {run_id})
    /\ observed_cancelled_runs' = (observed_cancelled_runs \ {run_id})
    /\ UNCHANGED << force_cancel_count >>


AnchorCancelled(run_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ observed_completed_runs' = (observed_completed_runs \ {run_id})
    /\ observed_failed_runs' = (observed_failed_runs \ {run_id})
    /\ observed_cancelled_runs' = (observed_cancelled_runs \cup {run_id})
    /\ UNCHANGED << force_cancel_count >>


AnchorForceCancelled ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ force_cancel_count' = (force_cancel_count) + 1
    /\ UNCHANGED << observed_completed_runs, observed_failed_runs, observed_cancelled_runs >>


Next ==
    \/ \E run_id \in RunIdValues : AnchorCompleted(run_id)
    \/ \E run_id \in RunIdValues : AnchorFailed(run_id)
    \/ \E run_id \in RunIdValues : AnchorCancelled(run_id)
    \/ AnchorForceCancelled

completed_not_failed == (\A run_id \in observed_completed_runs : ~((run_id \in observed_failed_runs)))
completed_not_cancelled == (\A run_id \in observed_completed_runs : ~((run_id \in observed_cancelled_runs)))
failed_not_cancelled == (\A run_id \in observed_failed_runs : ~((run_id \in observed_cancelled_runs)))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(observed_completed_runs) <= 1 /\ Cardinality(observed_failed_runs) <= 1 /\ Cardinality(observed_cancelled_runs) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(observed_completed_runs) <= 2 /\ Cardinality(observed_failed_runs) <= 2 /\ Cardinality(observed_cancelled_runs) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []completed_not_failed
THEOREM Spec => []completed_not_cancelled
THEOREM Spec => []failed_not_cancelled

=============================================================================
