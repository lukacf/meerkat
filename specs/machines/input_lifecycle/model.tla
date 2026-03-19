---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for InputLifecycleMachine.

CONSTANTS BoundarySequenceValues, RunIdValues

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

VARIABLES phase, model_step_count, terminal_outcome, last_run_id, last_boundary_sequence

vars == << phase, model_step_count, terminal_outcome, last_run_id, last_boundary_sequence >>

Init ==
    /\ phase = "Accepted"
    /\ model_step_count = 0
    /\ terminal_outcome = None
    /\ last_run_id = None
    /\ last_boundary_sequence = None

TerminalStutter ==
    /\ phase = "Consumed" \/ phase = "Superseded" \/ phase = "Coalesced" \/ phase = "Abandoned"
    /\ UNCHANGED vars

QueueAccepted ==
    /\ phase = "Accepted"
    /\ phase' = "Queued"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << terminal_outcome, last_run_id, last_boundary_sequence >>


StageForRun(run_id) ==
    /\ phase = "Queued"
    /\ phase' = "Staged"
    /\ model_step_count' = model_step_count + 1
    /\ last_run_id' = Some(run_id)
    /\ UNCHANGED << terminal_outcome, last_boundary_sequence >>


RollbackStaged ==
    /\ phase = "Staged"
    /\ phase' = "Queued"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << terminal_outcome, last_run_id, last_boundary_sequence >>


MarkApplied(run_id) ==
    /\ phase = "Staged"
    /\ (last_run_id = Some(run_id))
    /\ phase' = "Applied"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << terminal_outcome, last_run_id, last_boundary_sequence >>


MarkAppliedPendingConsumption(boundary_sequence) ==
    /\ phase = "Applied"
    /\ phase' = "AppliedPendingConsumption"
    /\ model_step_count' = model_step_count + 1
    /\ last_boundary_sequence' = Some(boundary_sequence)
    /\ UNCHANGED << terminal_outcome, last_run_id >>


Consume ==
    /\ phase = "AppliedPendingConsumption"
    /\ phase' = "Consumed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = Some("Consumed")
    /\ UNCHANGED << last_run_id, last_boundary_sequence >>


ConsumeOnAccept ==
    /\ phase = "Accepted"
    /\ phase' = "Consumed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = Some("Consumed")
    /\ UNCHANGED << last_run_id, last_boundary_sequence >>


Supersede ==
    /\ phase = "Queued"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = Some("Superseded")
    /\ UNCHANGED << last_run_id, last_boundary_sequence >>


Coalesce ==
    /\ phase = "Queued"
    /\ phase' = "Coalesced"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = Some("Coalesced")
    /\ UNCHANGED << last_run_id, last_boundary_sequence >>


Abandon ==
    /\ phase = "Accepted" \/ phase = "Queued" \/ phase = "Staged" \/ phase = "Applied" \/ phase = "AppliedPendingConsumption"
    /\ phase' = "Abandoned"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = Some("Abandoned")
    /\ UNCHANGED << last_run_id, last_boundary_sequence >>


Next ==
    \/ QueueAccepted
    \/ \E run_id \in RunIdValues : StageForRun(run_id)
    \/ RollbackStaged
    \/ \E run_id \in RunIdValues : MarkApplied(run_id)
    \/ \E boundary_sequence \in BoundarySequenceValues : MarkAppliedPendingConsumption(boundary_sequence)
    \/ Consume
    \/ ConsumeOnAccept
    \/ Supersede
    \/ Coalesce
    \/ Abandon
    \/ TerminalStutter

accepted_has_no_run_or_boundary_metadata == ((phase # "Accepted") \/ ((last_run_id = None) /\ (last_boundary_sequence = None)))
boundary_metadata_requires_application == ((last_boundary_sequence = None) \/ ((phase = "AppliedPendingConsumption") \/ (phase = "Consumed") \/ (phase = "Abandoned")))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []accepted_has_no_run_or_boundary_metadata
THEOREM Spec => []boundary_metadata_requires_application

=============================================================================
