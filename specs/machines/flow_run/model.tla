---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for FlowRunMachine.

CONSTANTS StepIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

SeqOfStepIdValues == {<<>>} \cup {<<x>> : x \in StepIdValues} \cup {<<x, y>> : x \in StepIdValues, y \in StepIdValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, tracked_steps, step_status, output_recorded, failure_count

vars == << phase, model_step_count, tracked_steps, step_status, output_recorded, failure_count >>

RunIsTerminal == ((phase = "Completed") \/ (phase = "Failed") \/ (phase = "Canceled"))
StepIsTracked(step_id) == (step_id \in tracked_steps)
StepStatusIs(step_id, expected_status) == ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE "None") = expected_status)
StepOutputRecordedIs(step_id, expected) == ((IF step_id \in DOMAIN output_recorded THEN output_recorded[step_id] ELSE FALSE) = expected)
AllTrackedStepsInAllowedStatuses(allowed_statuses) == (\A step_id \in tracked_steps : ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE "None") \in SeqElements(allowed_statuses)))
NoTrackedStepInStatus(status) == (\A step_id \in tracked_steps : ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE "None") # status))
AnyTrackedStepInStatus(status) == (\E step_id \in tracked_steps : ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE "None") = status))

Init ==
    /\ phase = "Absent"
    /\ model_step_count = 0
    /\ tracked_steps = {}
    /\ step_status = [x \in {} |-> None]
    /\ output_recorded = [x \in {} |-> None]
    /\ failure_count = 0

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Failed" \/ phase = "Canceled"
    /\ UNCHANGED vars

RECURSIVE CreateRun_ForEach0_output_recorded(_, _)
CreateRun_ForEach0_output_recorded(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, FALSE) IN CreateRun_ForEach0_output_recorded(next_acc, Tail(items))

RECURSIVE CreateRun_ForEach0_step_status(_, _)
CreateRun_ForEach0_step_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, "None") IN CreateRun_ForEach0_step_status(next_acc, Tail(items))

RECURSIVE CreateRun_ForEach0_tracked_steps(_, _)
CreateRun_ForEach0_tracked_steps(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == (acc \cup {step_id}) IN CreateRun_ForEach0_tracked_steps(next_acc, Tail(items))

CreateRun(step_ids) ==
    /\ phase = "Absent"
    /\ (Len(step_ids) > 0)
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ tracked_steps' = CreateRun_ForEach0_tracked_steps({}, step_ids)
    /\ step_status' = CreateRun_ForEach0_step_status([x \in {} |-> None], step_ids)
    /\ output_recorded' = CreateRun_ForEach0_output_recorded([x \in {} |-> None], step_ids)
    /\ failure_count' = 0


StartRun ==
    /\ phase = "Pending"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << tracked_steps, step_status, output_recorded, failure_count >>


DispatchStep(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "None")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_status' = MapSet(step_status, step_id, "Dispatched")
    /\ UNCHANGED << tracked_steps, output_recorded, failure_count >>


CompleteStep(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "Dispatched")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_status' = MapSet(step_status, step_id, "Completed")
    /\ UNCHANGED << tracked_steps, output_recorded, failure_count >>


RecordStepOutput(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "Completed")
    /\ StepOutputRecordedIs(step_id, FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ output_recorded' = MapSet(output_recorded, step_id, TRUE)
    /\ UNCHANGED << tracked_steps, step_status, failure_count >>


FailStep(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "Dispatched")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_status' = MapSet(step_status, step_id, "Failed")
    /\ failure_count' = (failure_count) + 1
    /\ UNCHANGED << tracked_steps, output_recorded >>


SkipStep(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "None")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_status' = MapSet(step_status, step_id, "Skipped")
    /\ UNCHANGED << tracked_steps, output_recorded, failure_count >>


CancelStep(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ (StepStatusIs(step_id, "None") \/ StepStatusIs(step_id, "Dispatched"))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_status' = MapSet(step_status, step_id, "Canceled")
    /\ UNCHANGED << tracked_steps, output_recorded, failure_count >>


TerminalizeCompleted ==
    /\ phase = "Running"
    /\ AllTrackedStepsInAllowedStatuses(<<"Completed", "Skipped">>)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << tracked_steps, step_status, output_recorded, failure_count >>


TerminalizeFailed ==
    /\ phase = "Running"
    /\ NoTrackedStepInStatus("Dispatched")
    /\ AnyTrackedStepInStatus("Failed")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << tracked_steps, step_status, output_recorded, failure_count >>


TerminalizeCanceled ==
    /\ phase = "Running"
    /\ NoTrackedStepInStatus("Dispatched")
    /\ AnyTrackedStepInStatus("Canceled")
    /\ phase' = "Canceled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << tracked_steps, step_status, output_recorded, failure_count >>


Next ==
    \/ \E step_ids \in SeqOfStepIdValues : CreateRun(step_ids)
    \/ StartRun
    \/ \E step_id \in StepIdValues : DispatchStep(step_id)
    \/ \E step_id \in StepIdValues : CompleteStep(step_id)
    \/ \E step_id \in StepIdValues : RecordStepOutput(step_id)
    \/ \E step_id \in StepIdValues : FailStep(step_id)
    \/ \E step_id \in StepIdValues : SkipStep(step_id)
    \/ \E step_id \in StepIdValues : CancelStep(step_id)
    \/ TerminalizeCompleted
    \/ TerminalizeFailed
    \/ TerminalizeCanceled
    \/ TerminalStutter

output_only_follows_completed_steps == (\A step_id \in tracked_steps : (~(StepOutputRecordedIs(step_id, TRUE)) \/ StepStatusIs(step_id, "Completed")))
terminal_runs_have_no_dispatched_steps == (~(RunIsTerminal) \/ NoTrackedStepInStatus("Dispatched"))
completed_runs_contain_only_completed_or_skipped_steps == ((phase # "Completed") \/ AllTrackedStepsInAllowedStatuses(<<"Completed", "Skipped">>))
failed_step_presence_requires_failure_count == (~(AnyTrackedStepInStatus("Failed")) \/ (failure_count >= 1))
failed_run_has_failed_step_or_recorded_failure == ((phase # "Failed") \/ (AnyTrackedStepInStatus("Failed") \/ (failure_count > 0)))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(tracked_steps) <= 1 /\ Cardinality(DOMAIN step_status) <= 1 /\ Cardinality(DOMAIN output_recorded) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(tracked_steps) <= 2 /\ Cardinality(DOMAIN step_status) <= 2 /\ Cardinality(DOMAIN output_recorded) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []output_only_follows_completed_steps
THEOREM Spec => []terminal_runs_have_no_dispatched_steps
THEOREM Spec => []completed_runs_contain_only_completed_or_skipped_steps
THEOREM Spec => []failed_step_presence_requires_failure_count
THEOREM Spec => []failed_run_has_failed_step_or_recorded_failure

=============================================================================
