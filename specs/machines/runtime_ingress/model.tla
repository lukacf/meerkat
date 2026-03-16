---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for RuntimeIngressMachine.

CONSTANTS BooleanValues, InputIdValues, InputKindValues, NatValues, PolicyDecisionValues, RunIdValues

SeqOfInputIdValues == {<<>>} \cup {<<x>> : x \in InputIdValues} \cup {<<x, y>> : x \in InputIdValues, y \in InputIdValues}

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

VARIABLES phase, model_step_count, admitted_inputs, admission_order, input_kind, policy_snapshot, lifecycle, terminal_outcome, queue, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested

vars == << phase, model_step_count, admitted_inputs, admission_order, input_kind, policy_snapshot, lifecycle, terminal_outcome, queue, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested >>

Init ==
    /\ phase = "Active"
    /\ model_step_count = 0
    /\ admitted_inputs = {}
    /\ admission_order = <<>>
    /\ input_kind = [x \in {} |-> None]
    /\ policy_snapshot = [x \in {} |-> None]
    /\ lifecycle = [x \in {} |-> None]
    /\ terminal_outcome = [x \in {} |-> None]
    /\ queue = <<>>
    /\ current_run = None
    /\ current_run_contributors = <<>>
    /\ last_run = [x \in {} |-> None]
    /\ last_boundary_sequence = [x \in {} |-> None]
    /\ wake_requested = FALSE
    /\ process_requested = FALSE

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

RECURSIVE StageDrainSnapshot_ForEach0_last_run(_, _, _)
StageDrainSnapshot_ForEach0_last_run(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some(outer_run_id)) IN StageDrainSnapshot_ForEach0_last_run(next_acc, Tail(items), outer_run_id)

RECURSIVE StageDrainSnapshot_ForEach0_lifecycle(_, _, _)
StageDrainSnapshot_ForEach0_lifecycle(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Staged") IN StageDrainSnapshot_ForEach0_lifecycle(next_acc, Tail(items), outer_run_id)

RECURSIVE BoundaryApplied_ForEach1_last_boundary_sequence(_, _, _)
BoundaryApplied_ForEach1_last_boundary_sequence(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some(outer_boundary_sequence)) IN BoundaryApplied_ForEach1_last_boundary_sequence(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE BoundaryApplied_ForEach1_lifecycle(_, _, _)
BoundaryApplied_ForEach1_lifecycle(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "AppliedPendingConsumption") IN BoundaryApplied_ForEach1_lifecycle(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE RunCompleted_ForEach2_lifecycle(_, _)
RunCompleted_ForEach2_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Consumed") IN RunCompleted_ForEach2_lifecycle(next_acc, Tail(items))

RECURSIVE RunCompleted_ForEach2_terminal_outcome(_, _)
RunCompleted_ForEach2_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("Consumed")) IN RunCompleted_ForEach2_terminal_outcome(next_acc, Tail(items))

RECURSIVE RunFailed_ForEach3_lifecycle(_, _)
RunFailed_ForEach3_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Queued") IN RunFailed_ForEach3_lifecycle(next_acc, Tail(items))

RECURSIVE RunCancelled_ForEach4_lifecycle(_, _)
RunCancelled_ForEach4_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Queued") IN RunCancelled_ForEach4_lifecycle(next_acc, Tail(items))

RECURSIVE CoalesceQueuedInputs_ForEach5_lifecycle(_, _)
CoalesceQueuedInputs_ForEach5_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Coalesced") IN CoalesceQueuedInputs_ForEach5_lifecycle(next_acc, Tail(items))

RECURSIVE CoalesceQueuedInputs_ForEach5_terminal_outcome(_, _)
CoalesceQueuedInputs_ForEach5_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("Coalesced")) IN CoalesceQueuedInputs_ForEach5_terminal_outcome(next_acc, Tail(items))

RECURSIVE ResetFromActive_ForEach6_lifecycle(_, _)
ResetFromActive_ForEach6_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Abandoned") IN ResetFromActive_ForEach6_lifecycle(next_acc, Tail(items))

RECURSIVE ResetFromActive_ForEach6_terminal_outcome(_, _)
ResetFromActive_ForEach6_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("AbandonedReset")) IN ResetFromActive_ForEach6_terminal_outcome(next_acc, Tail(items))

RECURSIVE ResetFromActive_ForEach7_lifecycle(_, _)
ResetFromActive_ForEach7_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Abandoned") IN ResetFromActive_ForEach7_lifecycle(next_acc, Tail(items))

RECURSIVE ResetFromActive_ForEach7_terminal_outcome(_, _)
ResetFromActive_ForEach7_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("AbandonedReset")) IN ResetFromActive_ForEach7_terminal_outcome(next_acc, Tail(items))

RECURSIVE ResetFromRetired_ForEach8_lifecycle(_, _)
ResetFromRetired_ForEach8_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Abandoned") IN ResetFromRetired_ForEach8_lifecycle(next_acc, Tail(items))

RECURSIVE ResetFromRetired_ForEach8_terminal_outcome(_, _)
ResetFromRetired_ForEach8_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("AbandonedReset")) IN ResetFromRetired_ForEach8_terminal_outcome(next_acc, Tail(items))

RECURSIVE ResetFromRetired_ForEach9_lifecycle(_, _)
ResetFromRetired_ForEach9_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Abandoned") IN ResetFromRetired_ForEach9_lifecycle(next_acc, Tail(items))

RECURSIVE ResetFromRetired_ForEach9_terminal_outcome(_, _)
ResetFromRetired_ForEach9_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("AbandonedReset")) IN ResetFromRetired_ForEach9_terminal_outcome(next_acc, Tail(items))

RECURSIVE Destroy_ForEach10_lifecycle(_, _)
Destroy_ForEach10_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Abandoned") IN Destroy_ForEach10_lifecycle(next_acc, Tail(items))

RECURSIVE Destroy_ForEach10_terminal_outcome(_, _)
Destroy_ForEach10_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("AbandonedDestroyed")) IN Destroy_ForEach10_terminal_outcome(next_acc, Tail(items))

RECURSIVE Destroy_ForEach11_lifecycle(_, _)
Destroy_ForEach11_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Abandoned") IN Destroy_ForEach11_lifecycle(next_acc, Tail(items))

RECURSIVE Destroy_ForEach11_terminal_outcome(_, _)
Destroy_ForEach11_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("AbandonedDestroyed")) IN Destroy_ForEach11_terminal_outcome(next_acc, Tail(items))

RECURSIVE RecoverFromActive_ForEach12_lifecycle(_, _)
RecoverFromActive_ForEach12_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Queued") IN RecoverFromActive_ForEach12_lifecycle(next_acc, Tail(items))

RECURSIVE RecoverFromRetired_ForEach13_lifecycle(_, _)
RecoverFromRetired_ForEach13_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Queued") IN RecoverFromRetired_ForEach13_lifecycle(next_acc, Tail(items))

AdmitQueuedNone(input_id, arg_input_kind, policy, wake, process) ==
    /\ phase = "Active"
    /\ ~((input_id \in admitted_inputs))
    /\ (wake = FALSE)
    /\ (process = FALSE)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_inputs' = (admitted_inputs \cup {input_id})
    /\ admission_order' = Append(admission_order, input_id)
    /\ input_kind' = MapSet(input_kind, input_id, arg_input_kind)
    /\ policy_snapshot' = MapSet(policy_snapshot, input_id, policy)
    /\ lifecycle' = MapSet(lifecycle, input_id, "Queued")
    /\ terminal_outcome' = MapSet(terminal_outcome, input_id, None)
    /\ queue' = Append(queue, input_id)
    /\ last_run' = MapSet(last_run, input_id, None)
    /\ last_boundary_sequence' = MapSet(last_boundary_sequence, input_id, None)
    /\ wake_requested' = (wake_requested \/ wake)
    /\ process_requested' = (process_requested \/ process)
    /\ UNCHANGED << current_run, current_run_contributors >>


AdmitQueuedWake(input_id, arg_input_kind, policy, wake, process) ==
    /\ phase = "Active"
    /\ ~((input_id \in admitted_inputs))
    /\ (wake = TRUE)
    /\ (process = FALSE)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_inputs' = (admitted_inputs \cup {input_id})
    /\ admission_order' = Append(admission_order, input_id)
    /\ input_kind' = MapSet(input_kind, input_id, arg_input_kind)
    /\ policy_snapshot' = MapSet(policy_snapshot, input_id, policy)
    /\ lifecycle' = MapSet(lifecycle, input_id, "Queued")
    /\ terminal_outcome' = MapSet(terminal_outcome, input_id, None)
    /\ queue' = Append(queue, input_id)
    /\ last_run' = MapSet(last_run, input_id, None)
    /\ last_boundary_sequence' = MapSet(last_boundary_sequence, input_id, None)
    /\ wake_requested' = (wake_requested \/ wake)
    /\ process_requested' = (process_requested \/ process)
    /\ UNCHANGED << current_run, current_run_contributors >>


AdmitQueuedProcess(input_id, arg_input_kind, policy, wake, process) ==
    /\ phase = "Active"
    /\ ~((input_id \in admitted_inputs))
    /\ (wake = FALSE)
    /\ (process = TRUE)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_inputs' = (admitted_inputs \cup {input_id})
    /\ admission_order' = Append(admission_order, input_id)
    /\ input_kind' = MapSet(input_kind, input_id, arg_input_kind)
    /\ policy_snapshot' = MapSet(policy_snapshot, input_id, policy)
    /\ lifecycle' = MapSet(lifecycle, input_id, "Queued")
    /\ terminal_outcome' = MapSet(terminal_outcome, input_id, None)
    /\ queue' = Append(queue, input_id)
    /\ last_run' = MapSet(last_run, input_id, None)
    /\ last_boundary_sequence' = MapSet(last_boundary_sequence, input_id, None)
    /\ wake_requested' = (wake_requested \/ wake)
    /\ process_requested' = (process_requested \/ process)
    /\ UNCHANGED << current_run, current_run_contributors >>


AdmitQueuedWakeAndProcess(input_id, arg_input_kind, policy, wake, process) ==
    /\ phase = "Active"
    /\ ~((input_id \in admitted_inputs))
    /\ (wake = TRUE)
    /\ (process = TRUE)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_inputs' = (admitted_inputs \cup {input_id})
    /\ admission_order' = Append(admission_order, input_id)
    /\ input_kind' = MapSet(input_kind, input_id, arg_input_kind)
    /\ policy_snapshot' = MapSet(policy_snapshot, input_id, policy)
    /\ lifecycle' = MapSet(lifecycle, input_id, "Queued")
    /\ terminal_outcome' = MapSet(terminal_outcome, input_id, None)
    /\ queue' = Append(queue, input_id)
    /\ last_run' = MapSet(last_run, input_id, None)
    /\ last_boundary_sequence' = MapSet(last_boundary_sequence, input_id, None)
    /\ wake_requested' = (wake_requested \/ wake)
    /\ process_requested' = (process_requested \/ process)
    /\ UNCHANGED << current_run, current_run_contributors >>


AdmitConsumedOnAccept(input_id, arg_input_kind, policy) ==
    /\ phase = "Active"
    /\ ~((input_id \in admitted_inputs))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_inputs' = (admitted_inputs \cup {input_id})
    /\ admission_order' = Append(admission_order, input_id)
    /\ input_kind' = MapSet(input_kind, input_id, arg_input_kind)
    /\ policy_snapshot' = MapSet(policy_snapshot, input_id, policy)
    /\ lifecycle' = MapSet(lifecycle, input_id, "Consumed")
    /\ terminal_outcome' = MapSet(terminal_outcome, input_id, Some("Consumed"))
    /\ last_run' = MapSet(last_run, input_id, None)
    /\ last_boundary_sequence' = MapSet(last_boundary_sequence, input_id, None)
    /\ UNCHANGED << queue, current_run, current_run_contributors, wake_requested, process_requested >>


StageDrainSnapshot(run_id, contributing_input_ids) ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ (current_run = None)
    /\ (Len(contributing_input_ids) > 0)
    /\ StartsWith(queue, contributing_input_ids)
    /\ \A input_id \in SeqElements(contributing_input_ids) : ((IF input_id \in DOMAIN lifecycle THEN lifecycle[input_id] ELSE "None") = "Queued")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = StageDrainSnapshot_ForEach0_lifecycle(lifecycle, contributing_input_ids, run_id)
    /\ queue' = SeqRemoveAll(queue, contributing_input_ids)
    /\ current_run' = Some(run_id)
    /\ current_run_contributors' = contributing_input_ids
    /\ last_run' = StageDrainSnapshot_ForEach0_last_run(last_run, contributing_input_ids, run_id)
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, terminal_outcome, last_boundary_sequence >>


BoundaryApplied(run_id, boundary_sequence) ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ (current_run = Some(run_id))
    /\ \A input_id \in SeqElements(current_run_contributors) : ((IF input_id \in DOMAIN lifecycle THEN lifecycle[input_id] ELSE "None") = "Staged")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = BoundaryApplied_ForEach1_lifecycle(lifecycle, current_run_contributors, boundary_sequence)
    /\ last_boundary_sequence' = BoundaryApplied_ForEach1_last_boundary_sequence(last_boundary_sequence, current_run_contributors, boundary_sequence)
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, terminal_outcome, queue, current_run, current_run_contributors, last_run, wake_requested, process_requested >>


RunCompleted(run_id) ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ (current_run = Some(run_id))
    /\ \A input_id \in SeqElements(current_run_contributors) : ((IF input_id \in DOMAIN lifecycle THEN lifecycle[input_id] ELSE "None") = "AppliedPendingConsumption")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RunCompleted_ForEach2_lifecycle(lifecycle, current_run_contributors)
    /\ terminal_outcome' = RunCompleted_ForEach2_terminal_outcome(terminal_outcome, current_run_contributors)
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, queue, last_run, last_boundary_sequence, wake_requested, process_requested >>


RunFailed(run_id) ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ (current_run = Some(run_id))
    /\ \A input_id \in SeqElements(current_run_contributors) : ((IF input_id \in DOMAIN lifecycle THEN lifecycle[input_id] ELSE "None") = "Staged")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RunFailed_ForEach3_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = IF (Len(current_run_contributors) > 0) THEN (current_run_contributors \o queue) ELSE queue
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF (Len(current_run_contributors) > 0) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, terminal_outcome, last_run, last_boundary_sequence, process_requested >>


RunCancelled(run_id) ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ (current_run = Some(run_id))
    /\ \A input_id \in SeqElements(current_run_contributors) : ((IF input_id \in DOMAIN lifecycle THEN lifecycle[input_id] ELSE "None") = "Staged")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RunCancelled_ForEach4_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = IF (Len(current_run_contributors) > 0) THEN (current_run_contributors \o queue) ELSE queue
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF (Len(current_run_contributors) > 0) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, terminal_outcome, last_run, last_boundary_sequence, process_requested >>


SupersedeQueuedInput(new_input_id, old_input_id) ==
    /\ phase = "Active"
    /\ (new_input_id \in admitted_inputs)
    /\ ((IF old_input_id \in DOMAIN lifecycle THEN lifecycle[old_input_id] ELSE "None") = "Queued")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = MapSet(lifecycle, old_input_id, "Superseded")
    /\ terminal_outcome' = MapSet(terminal_outcome, old_input_id, Some("Superseded"))
    /\ queue' = SeqRemove(queue, old_input_id)
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested >>


CoalesceQueuedInputs(aggregate_input_id, source_input_ids) ==
    /\ phase = "Active"
    /\ (aggregate_input_id \in admitted_inputs)
    /\ (Len(source_input_ids) > 0)
    /\ \A input_id \in SeqElements(source_input_ids) : ((IF input_id \in DOMAIN lifecycle THEN lifecycle[input_id] ELSE "None") = "Queued")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = CoalesceQueuedInputs_ForEach5_lifecycle(lifecycle, source_input_ids)
    /\ terminal_outcome' = CoalesceQueuedInputs_ForEach5_terminal_outcome(terminal_outcome, source_input_ids)
    /\ queue' = SeqRemoveAll(queue, source_input_ids)
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested >>


Retire ==
    /\ phase = "Active"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, lifecycle, terminal_outcome, queue, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested >>


ResetFromActive ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = ResetFromActive_ForEach7_lifecycle(ResetFromActive_ForEach6_lifecycle(lifecycle, queue), current_run_contributors)
    /\ terminal_outcome' = ResetFromActive_ForEach7_terminal_outcome(ResetFromActive_ForEach6_terminal_outcome(terminal_outcome, queue), current_run_contributors)
    /\ queue' = <<>>
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, last_run, last_boundary_sequence >>


ResetFromRetired ==
    /\ phase = "Retired"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = ResetFromRetired_ForEach9_lifecycle(ResetFromRetired_ForEach8_lifecycle(lifecycle, queue), current_run_contributors)
    /\ terminal_outcome' = ResetFromRetired_ForEach9_terminal_outcome(ResetFromRetired_ForEach8_terminal_outcome(terminal_outcome, queue), current_run_contributors)
    /\ queue' = <<>>
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, last_run, last_boundary_sequence >>


Destroy ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = Destroy_ForEach11_lifecycle(Destroy_ForEach10_lifecycle(lifecycle, queue), current_run_contributors)
    /\ terminal_outcome' = Destroy_ForEach11_terminal_outcome(Destroy_ForEach10_terminal_outcome(terminal_outcome, queue), current_run_contributors)
    /\ queue' = <<>>
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, last_run, last_boundary_sequence >>


RecoverFromActive ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RecoverFromActive_ForEach12_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = IF (Len(current_run_contributors) > 0) THEN (current_run_contributors \o queue) ELSE queue
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF (Len(current_run_contributors) > 0) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, terminal_outcome, last_run, last_boundary_sequence, process_requested >>


RecoverFromRetired ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RecoverFromRetired_ForEach13_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = IF (Len(current_run_contributors) > 0) THEN (current_run_contributors \o queue) ELSE queue
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF (Len(current_run_contributors) > 0) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, input_kind, policy_snapshot, terminal_outcome, last_run, last_boundary_sequence, process_requested >>


Next ==
    \/ \E input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E policy \in PolicyDecisionValues : \E wake \in BOOLEAN : \E process \in BOOLEAN : AdmitQueuedNone(input_id, arg_input_kind, policy, wake, process)
    \/ \E input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E policy \in PolicyDecisionValues : \E wake \in BOOLEAN : \E process \in BOOLEAN : AdmitQueuedWake(input_id, arg_input_kind, policy, wake, process)
    \/ \E input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E policy \in PolicyDecisionValues : \E wake \in BOOLEAN : \E process \in BOOLEAN : AdmitQueuedProcess(input_id, arg_input_kind, policy, wake, process)
    \/ \E input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E policy \in PolicyDecisionValues : \E wake \in BOOLEAN : \E process \in BOOLEAN : AdmitQueuedWakeAndProcess(input_id, arg_input_kind, policy, wake, process)
    \/ \E input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E policy \in PolicyDecisionValues : AdmitConsumedOnAccept(input_id, arg_input_kind, policy)
    \/ \E run_id \in RunIdValues : \E contributing_input_ids \in SeqOfInputIdValues : StageDrainSnapshot(run_id, contributing_input_ids)
    \/ \E run_id \in RunIdValues : \E boundary_sequence \in 0..2 : BoundaryApplied(run_id, boundary_sequence)
    \/ \E run_id \in RunIdValues : RunCompleted(run_id)
    \/ \E run_id \in RunIdValues : RunFailed(run_id)
    \/ \E run_id \in RunIdValues : RunCancelled(run_id)
    \/ \E new_input_id \in InputIdValues : \E old_input_id \in InputIdValues : SupersedeQueuedInput(new_input_id, old_input_id)
    \/ \E aggregate_input_id \in InputIdValues : \E source_input_ids \in SeqOfInputIdValues : CoalesceQueuedInputs(aggregate_input_id, source_input_ids)
    \/ Retire
    \/ ResetFromActive
    \/ ResetFromRetired
    \/ Destroy
    \/ RecoverFromActive
    \/ RecoverFromRetired
    \/ TerminalStutter

queue_entries_are_queued == \A input_id \in SeqElements(queue) : ((IF input_id \in DOMAIN lifecycle THEN lifecycle[input_id] ELSE "None") = "Queued")
terminal_inputs_do_not_appear_in_queue == \A input_id \in SeqElements(queue) : (((IF input_id \in DOMAIN lifecycle THEN lifecycle[input_id] ELSE "None") # "Consumed") /\ ((IF input_id \in DOMAIN lifecycle THEN lifecycle[input_id] ELSE "None") # "Superseded") /\ ((IF input_id \in DOMAIN lifecycle THEN lifecycle[input_id] ELSE "None") # "Coalesced") /\ ((IF input_id \in DOMAIN lifecycle THEN lifecycle[input_id] ELSE "None") # "Abandoned"))
current_run_matches_contributor_presence == ((current_run = None) = (Len(current_run_contributors) = 0))
staged_contributors_are_not_queued == \A input_id \in SeqElements(current_run_contributors) : ~((input_id \in SeqElements(queue)))
applied_pending_consumption_has_last_run == \A input_id \in admitted_inputs : (((IF input_id \in DOMAIN lifecycle THEN lifecycle[input_id] ELSE "None") # "AppliedPendingConsumption") \/ ((IF input_id \in DOMAIN last_run THEN last_run[input_id] ELSE None) # None))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(admitted_inputs) <= 1 /\ Len(admission_order) <= 1 /\ Cardinality(DOMAIN input_kind) <= 1 /\ Cardinality(DOMAIN policy_snapshot) <= 1 /\ Cardinality(DOMAIN lifecycle) <= 1 /\ Cardinality(DOMAIN terminal_outcome) <= 1 /\ Len(queue) <= 1 /\ Len(current_run_contributors) <= 1 /\ Cardinality(DOMAIN last_run) <= 1 /\ Cardinality(DOMAIN last_boundary_sequence) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(admitted_inputs) <= 2 /\ Len(admission_order) <= 2 /\ Cardinality(DOMAIN input_kind) <= 2 /\ Cardinality(DOMAIN policy_snapshot) <= 2 /\ Cardinality(DOMAIN lifecycle) <= 2 /\ Cardinality(DOMAIN terminal_outcome) <= 2 /\ Len(queue) <= 2 /\ Len(current_run_contributors) <= 2 /\ Cardinality(DOMAIN last_run) <= 2 /\ Cardinality(DOMAIN last_boundary_sequence) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []queue_entries_are_queued
THEOREM Spec => []terminal_inputs_do_not_appear_in_queue
THEOREM Spec => []current_run_matches_contributor_presence
THEOREM Spec => []staged_contributors_are_not_queued
THEOREM Spec => []applied_pending_consumption_has_last_run

=============================================================================
