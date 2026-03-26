---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobRuntimeBridgeAnchorMachine.

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

VARIABLES phase, model_step_count, observed_submitted_runs, observed_completed_runs, observed_failed_runs, observed_cancelled_runs, stop_request_count

vars == << phase, model_step_count, observed_submitted_runs, observed_completed_runs, observed_failed_runs, observed_cancelled_runs, stop_request_count >>

Init ==
    /\ phase = "Tracking"
    /\ model_step_count = 0
    /\ observed_submitted_runs = {}
    /\ observed_completed_runs = {}
    /\ observed_failed_runs = {}
    /\ observed_cancelled_runs = {}
    /\ stop_request_count = 0

RuntimeRunSubmitted(run_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ observed_submitted_runs' = (observed_submitted_runs \cup {run_id})
    /\ UNCHANGED << observed_completed_runs, observed_failed_runs, observed_cancelled_runs, stop_request_count >>


RuntimeRunCompleted(run_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ observed_completed_runs' = (observed_completed_runs \cup {run_id})
    /\ UNCHANGED << observed_submitted_runs, observed_failed_runs, observed_cancelled_runs, stop_request_count >>


RuntimeRunFailed(run_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ observed_failed_runs' = (observed_failed_runs \cup {run_id})
    /\ UNCHANGED << observed_submitted_runs, observed_completed_runs, observed_cancelled_runs, stop_request_count >>


RuntimeRunCancelled(run_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ observed_cancelled_runs' = (observed_cancelled_runs \cup {run_id})
    /\ UNCHANGED << observed_submitted_runs, observed_completed_runs, observed_failed_runs, stop_request_count >>


RuntimeStopRequested ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ stop_request_count' = (stop_request_count) + 1
    /\ UNCHANGED << observed_submitted_runs, observed_completed_runs, observed_failed_runs, observed_cancelled_runs >>


Next ==
    \/ \E run_id \in RunIdValues : RuntimeRunSubmitted(run_id)
    \/ \E run_id \in RunIdValues : RuntimeRunCompleted(run_id)
    \/ \E run_id \in RunIdValues : RuntimeRunFailed(run_id)
    \/ \E run_id \in RunIdValues : RuntimeRunCancelled(run_id)
    \/ RuntimeStopRequested


CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(observed_submitted_runs) <= 1 /\ Cardinality(observed_completed_runs) <= 1 /\ Cardinality(observed_failed_runs) <= 1 /\ Cardinality(observed_cancelled_runs) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(observed_submitted_runs) <= 2 /\ Cardinality(observed_completed_runs) <= 2 /\ Cardinality(observed_failed_runs) <= 2 /\ Cardinality(observed_cancelled_runs) <= 2

Spec == Init /\ [][Next]_vars


=============================================================================
