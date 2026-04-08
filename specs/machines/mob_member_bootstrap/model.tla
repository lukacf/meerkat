---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobMemberBootstrapMachine.

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

VARIABLES phase, model_step_count, started_runs, callback_pending_runs, failed_runs, cancelled_runs, force_cancel_count

vars == << phase, model_step_count, started_runs, callback_pending_runs, failed_runs, cancelled_runs, force_cancel_count >>

Init ==
    /\ phase = "Tracking"
    /\ model_step_count = 0
    /\ started_runs = {}
    /\ callback_pending_runs = {}
    /\ failed_runs = {}
    /\ cancelled_runs = {}
    /\ force_cancel_count = 0

KickoffStarted(run_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ started_runs' = (started_runs \cup {run_id})
    /\ callback_pending_runs' = (callback_pending_runs \ {run_id})
    /\ failed_runs' = (failed_runs \ {run_id})
    /\ cancelled_runs' = (cancelled_runs \ {run_id})
    /\ UNCHANGED << force_cancel_count >>


KickoffCallbackPending(run_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ started_runs' = (started_runs \ {run_id})
    /\ callback_pending_runs' = (callback_pending_runs \cup {run_id})
    /\ failed_runs' = (failed_runs \ {run_id})
    /\ cancelled_runs' = (cancelled_runs \ {run_id})
    /\ UNCHANGED << force_cancel_count >>


KickoffFailed(run_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ started_runs' = (started_runs \ {run_id})
    /\ callback_pending_runs' = (callback_pending_runs \ {run_id})
    /\ failed_runs' = (failed_runs \cup {run_id})
    /\ cancelled_runs' = (cancelled_runs \ {run_id})
    /\ UNCHANGED << force_cancel_count >>


KickoffCancelled(run_id) ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ started_runs' = (started_runs \ {run_id})
    /\ callback_pending_runs' = (callback_pending_runs \ {run_id})
    /\ failed_runs' = (failed_runs \ {run_id})
    /\ cancelled_runs' = (cancelled_runs \cup {run_id})
    /\ UNCHANGED << force_cancel_count >>


KickoffForceCancelled ==
    /\ phase = "Tracking"
    /\ phase' = "Tracking"
    /\ model_step_count' = model_step_count + 1
    /\ force_cancel_count' = (force_cancel_count) + 1
    /\ UNCHANGED << started_runs, callback_pending_runs, failed_runs, cancelled_runs >>


Next ==
    \/ \E run_id \in RunIdValues : KickoffStarted(run_id)
    \/ \E run_id \in RunIdValues : KickoffCallbackPending(run_id)
    \/ \E run_id \in RunIdValues : KickoffFailed(run_id)
    \/ \E run_id \in RunIdValues : KickoffCancelled(run_id)
    \/ KickoffForceCancelled

started_not_failed == (\A run_id \in started_runs : ~((run_id \in failed_runs)))
started_not_cancelled == (\A run_id \in started_runs : ~((run_id \in cancelled_runs)))
failed_not_cancelled == (\A run_id \in failed_runs : ~((run_id \in cancelled_runs)))
callback_pending_not_terminal == (\A run_id \in callback_pending_runs : (~((run_id \in started_runs)) /\ ~((run_id \in failed_runs)) /\ ~((run_id \in cancelled_runs))))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(started_runs) <= 1 /\ Cardinality(callback_pending_runs) <= 1 /\ Cardinality(failed_runs) <= 1 /\ Cardinality(cancelled_runs) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(started_runs) <= 2 /\ Cardinality(callback_pending_runs) <= 2 /\ Cardinality(failed_runs) <= 2 /\ Cardinality(cancelled_runs) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []started_not_failed
THEOREM Spec => []started_not_cancelled
THEOREM Spec => []failed_not_cancelled
THEOREM Spec => []callback_pending_not_terminal

=============================================================================
