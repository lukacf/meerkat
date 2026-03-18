---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for RuntimeIngressMachine.

CONSTANTS ContentShapeValues, HandlingModeValues, NatValues, PolicyDecisionValues, RequestIdValues, ReservationKeyValues, RunIdValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionRequestIdValues == {None} \cup {Some(x) : x \in RequestIdValues}
OptionReservationKeyValues == {None} \cup {Some(x) : x \in ReservationKeyValues}
SeqOfWorkIdValues == {<<>>} \cup {<<x>> : x \in WorkIdValues} \cup {<<x, y>> : x \in WorkIdValues, y \in WorkIdValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, lifecycle, terminal_outcome, queue, steer_queue, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested

vars == << phase, model_step_count, admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, lifecycle, terminal_outcome, queue, steer_queue, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested >>

Init ==
    /\ phase = "Active"
    /\ model_step_count = 0
    /\ admitted_inputs = {}
    /\ admission_order = <<>>
    /\ content_shape = [x \in {} |-> None]
    /\ request_id = [x \in {} |-> None]
    /\ reservation_key = [x \in {} |-> None]
    /\ policy_snapshot = [x \in {} |-> None]
    /\ handling_mode = [x \in {} |-> None]
    /\ lifecycle = [x \in {} |-> None]
    /\ terminal_outcome = [x \in {} |-> None]
    /\ queue = <<>>
    /\ steer_queue = <<>>
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
StageDrainSnapshot_ForEach0_last_run(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some(outer_run_id)) IN StageDrainSnapshot_ForEach0_last_run(next_acc, Tail(items), outer_run_id)

RECURSIVE StageDrainSnapshot_ForEach0_lifecycle(_, _, _)
StageDrainSnapshot_ForEach0_lifecycle(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Staged") IN StageDrainSnapshot_ForEach0_lifecycle(next_acc, Tail(items), outer_run_id)

RECURSIVE BoundaryApplied_ForEach1_last_boundary_sequence(_, _, _)
BoundaryApplied_ForEach1_last_boundary_sequence(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some(outer_boundary_sequence)) IN BoundaryApplied_ForEach1_last_boundary_sequence(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE BoundaryApplied_ForEach1_lifecycle(_, _, _)
BoundaryApplied_ForEach1_lifecycle(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "AppliedPendingConsumption") IN BoundaryApplied_ForEach1_lifecycle(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE RunCompleted_ForEach2_lifecycle(_, _)
RunCompleted_ForEach2_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Consumed") IN RunCompleted_ForEach2_lifecycle(next_acc, Tail(items))

RECURSIVE RunCompleted_ForEach2_terminal_outcome(_, _)
RunCompleted_ForEach2_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("Consumed")) IN RunCompleted_ForEach2_terminal_outcome(next_acc, Tail(items))

RECURSIVE RunFailed_ForEach3_lifecycle(_, _)
RunFailed_ForEach3_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Queued") IN RunFailed_ForEach3_lifecycle(next_acc, Tail(items))

RECURSIVE RunCancelled_ForEach4_lifecycle(_, _)
RunCancelled_ForEach4_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Queued") IN RunCancelled_ForEach4_lifecycle(next_acc, Tail(items))

RECURSIVE CoalesceQueuedInputs_ForEach5_lifecycle(_, _)
CoalesceQueuedInputs_ForEach5_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Coalesced") IN CoalesceQueuedInputs_ForEach5_lifecycle(next_acc, Tail(items))

RECURSIVE CoalesceQueuedInputs_ForEach5_terminal_outcome(_, _)
CoalesceQueuedInputs_ForEach5_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("Coalesced")) IN CoalesceQueuedInputs_ForEach5_terminal_outcome(next_acc, Tail(items))

RECURSIVE ResetFromActive_ForEach6_lifecycle(_, _)
ResetFromActive_ForEach6_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN ResetFromActive_ForEach6_lifecycle(next_acc, Tail(items))

RECURSIVE ResetFromActive_ForEach6_terminal_outcome(_, _)
ResetFromActive_ForEach6_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedReset")) IN ResetFromActive_ForEach6_terminal_outcome(next_acc, Tail(items))

RECURSIVE ResetFromActive_ForEach7_lifecycle(_, _)
ResetFromActive_ForEach7_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN ResetFromActive_ForEach7_lifecycle(next_acc, Tail(items))

RECURSIVE ResetFromActive_ForEach7_terminal_outcome(_, _)
ResetFromActive_ForEach7_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedReset")) IN ResetFromActive_ForEach7_terminal_outcome(next_acc, Tail(items))

RECURSIVE ResetFromActive_ForEach8_lifecycle(_, _)
ResetFromActive_ForEach8_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN ResetFromActive_ForEach8_lifecycle(next_acc, Tail(items))

RECURSIVE ResetFromActive_ForEach8_terminal_outcome(_, _)
ResetFromActive_ForEach8_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedReset")) IN ResetFromActive_ForEach8_terminal_outcome(next_acc, Tail(items))

RECURSIVE ResetFromRetired_ForEach9_lifecycle(_, _)
ResetFromRetired_ForEach9_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN ResetFromRetired_ForEach9_lifecycle(next_acc, Tail(items))

RECURSIVE ResetFromRetired_ForEach9_terminal_outcome(_, _)
ResetFromRetired_ForEach9_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedReset")) IN ResetFromRetired_ForEach9_terminal_outcome(next_acc, Tail(items))

RECURSIVE ResetFromRetired_ForEach10_lifecycle(_, _)
ResetFromRetired_ForEach10_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN ResetFromRetired_ForEach10_lifecycle(next_acc, Tail(items))

RECURSIVE ResetFromRetired_ForEach10_terminal_outcome(_, _)
ResetFromRetired_ForEach10_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedReset")) IN ResetFromRetired_ForEach10_terminal_outcome(next_acc, Tail(items))

RECURSIVE ResetFromRetired_ForEach11_lifecycle(_, _)
ResetFromRetired_ForEach11_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN ResetFromRetired_ForEach11_lifecycle(next_acc, Tail(items))

RECURSIVE ResetFromRetired_ForEach11_terminal_outcome(_, _)
ResetFromRetired_ForEach11_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedReset")) IN ResetFromRetired_ForEach11_terminal_outcome(next_acc, Tail(items))

RECURSIVE Destroy_ForEach12_lifecycle(_, _)
Destroy_ForEach12_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN Destroy_ForEach12_lifecycle(next_acc, Tail(items))

RECURSIVE Destroy_ForEach12_terminal_outcome(_, _)
Destroy_ForEach12_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedDestroyed")) IN Destroy_ForEach12_terminal_outcome(next_acc, Tail(items))

RECURSIVE Destroy_ForEach13_lifecycle(_, _)
Destroy_ForEach13_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN Destroy_ForEach13_lifecycle(next_acc, Tail(items))

RECURSIVE Destroy_ForEach13_terminal_outcome(_, _)
Destroy_ForEach13_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedDestroyed")) IN Destroy_ForEach13_terminal_outcome(next_acc, Tail(items))

RECURSIVE Destroy_ForEach14_lifecycle(_, _)
Destroy_ForEach14_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN Destroy_ForEach14_lifecycle(next_acc, Tail(items))

RECURSIVE Destroy_ForEach14_terminal_outcome(_, _)
Destroy_ForEach14_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedDestroyed")) IN Destroy_ForEach14_terminal_outcome(next_acc, Tail(items))

RECURSIVE RecoverFromActive_ForEach15_lifecycle(_, _)
RecoverFromActive_ForEach15_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Queued") IN RecoverFromActive_ForEach15_lifecycle(next_acc, Tail(items))

RECURSIVE RecoverFromRetired_ForEach16_lifecycle(_, _)
RecoverFromRetired_ForEach16_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Queued") IN RecoverFromRetired_ForEach16_lifecycle(next_acc, Tail(items))

AdmitQueuedQueue(work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, policy) ==
    /\ phase = "Active"
    /\ ~((work_id \in admitted_inputs))
    /\ (arg_handling_mode = "Queue")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_inputs' = (admitted_inputs \cup {work_id})
    /\ admission_order' = Append(admission_order, work_id)
    /\ content_shape' = MapSet(content_shape, work_id, arg_content_shape)
    /\ request_id' = MapSet(request_id, work_id, arg_request_id)
    /\ reservation_key' = MapSet(reservation_key, work_id, arg_reservation_key)
    /\ policy_snapshot' = MapSet(policy_snapshot, work_id, policy)
    /\ handling_mode' = MapSet(handling_mode, work_id, arg_handling_mode)
    /\ lifecycle' = MapSet(lifecycle, work_id, "Queued")
    /\ terminal_outcome' = MapSet(terminal_outcome, work_id, None)
    /\ queue' = Append(queue, work_id)
    /\ last_run' = MapSet(last_run, work_id, None)
    /\ last_boundary_sequence' = MapSet(last_boundary_sequence, work_id, None)
    /\ wake_requested' = (wake_requested \/ FALSE)
    /\ process_requested' = (process_requested \/ FALSE)
    /\ UNCHANGED << steer_queue, current_run, current_run_contributors >>


AdmitQueuedSteer(work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, policy) ==
    /\ phase = "Active"
    /\ ~((work_id \in admitted_inputs))
    /\ (arg_handling_mode = "Steer")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_inputs' = (admitted_inputs \cup {work_id})
    /\ admission_order' = Append(admission_order, work_id)
    /\ content_shape' = MapSet(content_shape, work_id, arg_content_shape)
    /\ request_id' = MapSet(request_id, work_id, arg_request_id)
    /\ reservation_key' = MapSet(reservation_key, work_id, arg_reservation_key)
    /\ policy_snapshot' = MapSet(policy_snapshot, work_id, policy)
    /\ handling_mode' = MapSet(handling_mode, work_id, arg_handling_mode)
    /\ lifecycle' = MapSet(lifecycle, work_id, "Queued")
    /\ terminal_outcome' = MapSet(terminal_outcome, work_id, None)
    /\ steer_queue' = Append(steer_queue, work_id)
    /\ last_run' = MapSet(last_run, work_id, None)
    /\ last_boundary_sequence' = MapSet(last_boundary_sequence, work_id, None)
    /\ wake_requested' = (wake_requested \/ TRUE)
    /\ process_requested' = (process_requested \/ TRUE)
    /\ UNCHANGED << queue, current_run, current_run_contributors >>


AdmitConsumedOnAccept(work_id, arg_content_shape, arg_request_id, arg_reservation_key, policy) ==
    /\ phase = "Active"
    /\ ~((work_id \in admitted_inputs))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_inputs' = (admitted_inputs \cup {work_id})
    /\ admission_order' = Append(admission_order, work_id)
    /\ content_shape' = MapSet(content_shape, work_id, arg_content_shape)
    /\ request_id' = MapSet(request_id, work_id, arg_request_id)
    /\ reservation_key' = MapSet(reservation_key, work_id, arg_reservation_key)
    /\ policy_snapshot' = MapSet(policy_snapshot, work_id, policy)
    /\ lifecycle' = MapSet(lifecycle, work_id, "Consumed")
    /\ terminal_outcome' = MapSet(terminal_outcome, work_id, Some("Consumed"))
    /\ last_run' = MapSet(last_run, work_id, None)
    /\ last_boundary_sequence' = MapSet(last_boundary_sequence, work_id, None)
    /\ UNCHANGED << handling_mode, queue, steer_queue, current_run, current_run_contributors, wake_requested, process_requested >>


StageDrainSnapshot(run_id, contributing_work_ids) ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ (current_run = None)
    /\ (Len(contributing_work_ids) > 0)
    /\ (((Len(steer_queue) > 0) /\ StartsWith(steer_queue, contributing_work_ids)) \/ ((Len(steer_queue) = 0) /\ StartsWith(queue, contributing_work_ids)))
    /\ (\A work_id \in SeqElements(contributing_work_ids) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "Queued"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = StageDrainSnapshot_ForEach0_lifecycle(lifecycle, contributing_work_ids, run_id)
    /\ queue' = IF (Len(steer_queue) > 0) THEN queue ELSE SeqRemoveAll(queue, contributing_work_ids)
    /\ steer_queue' = IF (Len(steer_queue) > 0) THEN SeqRemoveAll(steer_queue, contributing_work_ids) ELSE steer_queue
    /\ current_run' = Some(run_id)
    /\ current_run_contributors' = contributing_work_ids
    /\ last_run' = StageDrainSnapshot_ForEach0_last_run(last_run, contributing_work_ids, run_id)
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, last_boundary_sequence >>


BoundaryApplied(run_id, boundary_sequence) ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ (current_run = Some(run_id))
    /\ (\A work_id \in SeqElements(current_run_contributors) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "Staged"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = BoundaryApplied_ForEach1_lifecycle(lifecycle, current_run_contributors, boundary_sequence)
    /\ last_boundary_sequence' = BoundaryApplied_ForEach1_last_boundary_sequence(last_boundary_sequence, current_run_contributors, boundary_sequence)
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, queue, steer_queue, current_run, current_run_contributors, last_run, wake_requested, process_requested >>


RunCompleted(run_id) ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ (current_run = Some(run_id))
    /\ (\A work_id \in SeqElements(current_run_contributors) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "AppliedPendingConsumption"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RunCompleted_ForEach2_lifecycle(lifecycle, current_run_contributors)
    /\ terminal_outcome' = RunCompleted_ForEach2_terminal_outcome(terminal_outcome, current_run_contributors)
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, queue, steer_queue, last_run, last_boundary_sequence, wake_requested, process_requested >>


RunFailed(run_id) ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ (current_run = Some(run_id))
    /\ (\A work_id \in SeqElements(current_run_contributors) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "Staged"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RunFailed_ForEach3_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = IF (Len(current_run_contributors) > 0) THEN (current_run_contributors \o queue) ELSE queue
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF (Len(current_run_contributors) > 0) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, steer_queue, last_run, last_boundary_sequence, process_requested >>


RunCancelled(run_id) ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ (current_run = Some(run_id))
    /\ (\A work_id \in SeqElements(current_run_contributors) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "Staged"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RunCancelled_ForEach4_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = IF (Len(current_run_contributors) > 0) THEN (current_run_contributors \o queue) ELSE queue
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF (Len(current_run_contributors) > 0) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, steer_queue, last_run, last_boundary_sequence, process_requested >>


SupersedeQueuedInput(new_work_id, old_work_id) ==
    /\ phase = "Active"
    /\ (new_work_id \in admitted_inputs)
    /\ ((IF old_work_id \in DOMAIN lifecycle THEN lifecycle[old_work_id] ELSE "None") = "Queued")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = MapSet(lifecycle, old_work_id, "Superseded")
    /\ terminal_outcome' = MapSet(terminal_outcome, old_work_id, Some("Superseded"))
    /\ queue' = SeqRemove(queue, old_work_id)
    /\ steer_queue' = SeqRemove(steer_queue, old_work_id)
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested >>


CoalesceQueuedInputs(aggregate_work_id, source_work_ids) ==
    /\ phase = "Active"
    /\ (aggregate_work_id \in admitted_inputs)
    /\ (Len(source_work_ids) > 0)
    /\ (\A work_id \in SeqElements(source_work_ids) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "Queued"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = CoalesceQueuedInputs_ForEach5_lifecycle(lifecycle, source_work_ids)
    /\ terminal_outcome' = CoalesceQueuedInputs_ForEach5_terminal_outcome(terminal_outcome, source_work_ids)
    /\ queue' = SeqRemoveAll(queue, source_work_ids)
    /\ steer_queue' = SeqRemoveAll(steer_queue, source_work_ids)
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested >>


Retire ==
    /\ phase = "Active"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, lifecycle, terminal_outcome, queue, steer_queue, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested >>


ResetFromActive ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = ResetFromActive_ForEach8_lifecycle(ResetFromActive_ForEach7_lifecycle(ResetFromActive_ForEach6_lifecycle(lifecycle, queue), steer_queue), current_run_contributors)
    /\ terminal_outcome' = ResetFromActive_ForEach8_terminal_outcome(ResetFromActive_ForEach7_terminal_outcome(ResetFromActive_ForEach6_terminal_outcome(terminal_outcome, queue), steer_queue), current_run_contributors)
    /\ queue' = <<>>
    /\ steer_queue' = <<>>
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, last_run, last_boundary_sequence >>


ResetFromRetired ==
    /\ phase = "Retired"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = ResetFromRetired_ForEach11_lifecycle(ResetFromRetired_ForEach10_lifecycle(ResetFromRetired_ForEach9_lifecycle(lifecycle, queue), steer_queue), current_run_contributors)
    /\ terminal_outcome' = ResetFromRetired_ForEach11_terminal_outcome(ResetFromRetired_ForEach10_terminal_outcome(ResetFromRetired_ForEach9_terminal_outcome(terminal_outcome, queue), steer_queue), current_run_contributors)
    /\ queue' = <<>>
    /\ steer_queue' = <<>>
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, last_run, last_boundary_sequence >>


Destroy ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = Destroy_ForEach14_lifecycle(Destroy_ForEach13_lifecycle(Destroy_ForEach12_lifecycle(lifecycle, queue), steer_queue), current_run_contributors)
    /\ terminal_outcome' = Destroy_ForEach14_terminal_outcome(Destroy_ForEach13_terminal_outcome(Destroy_ForEach12_terminal_outcome(terminal_outcome, queue), steer_queue), current_run_contributors)
    /\ queue' = <<>>
    /\ steer_queue' = <<>>
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, last_run, last_boundary_sequence >>


RecoverFromActive ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RecoverFromActive_ForEach15_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = IF (Len(current_run_contributors) > 0) THEN (current_run_contributors \o queue) ELSE queue
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF (Len(current_run_contributors) > 0) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, steer_queue, last_run, last_boundary_sequence, process_requested >>


RecoverFromRetired ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RecoverFromRetired_ForEach16_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = IF (Len(current_run_contributors) > 0) THEN (current_run_contributors \o queue) ELSE queue
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF (Len(current_run_contributors) > 0) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, steer_queue, last_run, last_boundary_sequence, process_requested >>


Next ==
    \/ \E work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E policy \in PolicyDecisionValues : AdmitQueuedQueue(work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, policy)
    \/ \E work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E policy \in PolicyDecisionValues : AdmitQueuedSteer(work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, policy)
    \/ \E work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E policy \in PolicyDecisionValues : AdmitConsumedOnAccept(work_id, arg_content_shape, arg_request_id, arg_reservation_key, policy)
    \/ \E run_id \in RunIdValues : \E contributing_work_ids \in SeqOfWorkIdValues : StageDrainSnapshot(run_id, contributing_work_ids)
    \/ \E run_id \in RunIdValues : \E boundary_sequence \in 0..2 : BoundaryApplied(run_id, boundary_sequence)
    \/ \E run_id \in RunIdValues : RunCompleted(run_id)
    \/ \E run_id \in RunIdValues : RunFailed(run_id)
    \/ \E run_id \in RunIdValues : RunCancelled(run_id)
    \/ \E new_work_id \in WorkIdValues : \E old_work_id \in WorkIdValues : SupersedeQueuedInput(new_work_id, old_work_id)
    \/ \E aggregate_work_id \in WorkIdValues : \E source_work_ids \in SeqOfWorkIdValues : CoalesceQueuedInputs(aggregate_work_id, source_work_ids)
    \/ Retire
    \/ ResetFromActive
    \/ ResetFromRetired
    \/ Destroy
    \/ RecoverFromActive
    \/ RecoverFromRetired
    \/ TerminalStutter

queue_entries_are_queued == (\A work_id \in SeqElements(queue) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "Queued"))
steer_entries_are_queued == (\A work_id \in SeqElements(steer_queue) : (work_id \in DOMAIN lifecycle))
pending_inputs_preserve_content_shape == ((\A work_id \in SeqElements(queue) : (work_id \in DOMAIN content_shape)) /\ (\A work_id \in SeqElements(steer_queue) : (work_id \in DOMAIN content_shape)))
admitted_inputs_preserve_correlation_slots == (\A work_id \in admitted_inputs : ((work_id \in DOMAIN request_id) /\ (work_id \in DOMAIN reservation_key)))
queue_entries_preserve_handling_mode == (\A work_id \in SeqElements(queue) : ((IF work_id \in DOMAIN handling_mode THEN handling_mode[work_id] ELSE "None") = "Queue"))
steer_entries_preserve_handling_mode == (\A work_id \in SeqElements(steer_queue) : ((IF work_id \in DOMAIN handling_mode THEN handling_mode[work_id] ELSE "None") = "Steer"))
pending_queues_do_not_overlap == (\A work_id \in SeqElements(steer_queue) : ~((work_id \in SeqElements(queue))))
terminal_inputs_do_not_appear_in_queue == ((\A work_id \in SeqElements(queue) : (((IF work_id \in DOMAIN terminal_outcome THEN terminal_outcome[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") # "Abandoned"))) /\ (\A work_id \in SeqElements(steer_queue) : (((IF work_id \in DOMAIN terminal_outcome THEN terminal_outcome[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") # "Abandoned"))))
current_run_matches_contributor_presence == ((current_run = None) = (Len(current_run_contributors) = 0))
staged_contributors_are_not_queued == (\A work_id \in SeqElements(current_run_contributors) : (~((work_id \in SeqElements(queue))) /\ ~((work_id \in SeqElements(steer_queue)))))
applied_pending_consumption_has_last_run == (\A work_id \in admitted_inputs : (((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") # "AppliedPendingConsumption") \/ ((IF work_id \in DOMAIN last_run THEN last_run[work_id] ELSE None) # None)))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(admitted_inputs) <= 1 /\ Len(admission_order) <= 1 /\ Cardinality(DOMAIN content_shape) <= 1 /\ Cardinality(DOMAIN request_id) <= 1 /\ Cardinality(DOMAIN reservation_key) <= 1 /\ Cardinality(DOMAIN policy_snapshot) <= 1 /\ Cardinality(DOMAIN handling_mode) <= 1 /\ Cardinality(DOMAIN lifecycle) <= 1 /\ Cardinality(DOMAIN terminal_outcome) <= 1 /\ Len(queue) <= 1 /\ Len(steer_queue) <= 1 /\ Len(current_run_contributors) <= 1 /\ Cardinality(DOMAIN last_run) <= 1 /\ Cardinality(DOMAIN last_boundary_sequence) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(admitted_inputs) <= 2 /\ Len(admission_order) <= 2 /\ Cardinality(DOMAIN content_shape) <= 2 /\ Cardinality(DOMAIN request_id) <= 2 /\ Cardinality(DOMAIN reservation_key) <= 2 /\ Cardinality(DOMAIN policy_snapshot) <= 2 /\ Cardinality(DOMAIN handling_mode) <= 2 /\ Cardinality(DOMAIN lifecycle) <= 2 /\ Cardinality(DOMAIN terminal_outcome) <= 2 /\ Len(queue) <= 2 /\ Len(steer_queue) <= 2 /\ Len(current_run_contributors) <= 2 /\ Cardinality(DOMAIN last_run) <= 2 /\ Cardinality(DOMAIN last_boundary_sequence) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []queue_entries_are_queued
THEOREM Spec => []steer_entries_are_queued
THEOREM Spec => []pending_inputs_preserve_content_shape
THEOREM Spec => []admitted_inputs_preserve_correlation_slots
THEOREM Spec => []queue_entries_preserve_handling_mode
THEOREM Spec => []steer_entries_preserve_handling_mode
THEOREM Spec => []pending_queues_do_not_overlap
THEOREM Spec => []terminal_inputs_do_not_appear_in_queue
THEOREM Spec => []current_run_matches_contributor_presence
THEOREM Spec => []staged_contributors_are_not_queued
THEOREM Spec => []applied_pending_consumption_has_last_run

=============================================================================
