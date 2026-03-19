---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for RuntimeIngressMachine.

CONSTANTS ContentShapeValues, HandlingModeValues, NatValues, PolicyDecisionValues, RequestIdValues, ReservationKeyValues, RunIdValues, SetOfStringValues, StringValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

SeqOfWorkIdValues == {<<>>} \cup {<<x>> : x \in WorkIdValues} \cup {<<x, y>> : x \in WorkIdValues, y \in WorkIdValues}
OptionRequestIdValues == {None} \cup {Some(x) : x \in RequestIdValues}
OptionReservationKeyValues == {None} \cup {Some(x) : x \in ReservationKeyValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, lifecycle, terminal_outcome, queue, steer_queue, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested, silent_intent_overrides

vars == << phase, model_step_count, admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, lifecycle, terminal_outcome, queue, steer_queue, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested, silent_intent_overrides >>

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
    /\ silent_intent_overrides = {}

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

RECURSIVE StageDrainSnapshotFromActive_ForEach0_last_run(_, _, _)
StageDrainSnapshotFromActive_ForEach0_last_run(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some(outer_run_id)) IN StageDrainSnapshotFromActive_ForEach0_last_run(next_acc, Tail(items), outer_run_id)

RECURSIVE StageDrainSnapshotFromActive_ForEach0_lifecycle(_, _, _)
StageDrainSnapshotFromActive_ForEach0_lifecycle(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Staged") IN StageDrainSnapshotFromActive_ForEach0_lifecycle(next_acc, Tail(items), outer_run_id)

RECURSIVE StageDrainSnapshotFromRetired_ForEach1_last_run(_, _, _)
StageDrainSnapshotFromRetired_ForEach1_last_run(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some(outer_run_id)) IN StageDrainSnapshotFromRetired_ForEach1_last_run(next_acc, Tail(items), outer_run_id)

RECURSIVE StageDrainSnapshotFromRetired_ForEach1_lifecycle(_, _, _)
StageDrainSnapshotFromRetired_ForEach1_lifecycle(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Staged") IN StageDrainSnapshotFromRetired_ForEach1_lifecycle(next_acc, Tail(items), outer_run_id)

RECURSIVE BoundaryAppliedFromActive_ForEach2_last_boundary_sequence(_, _, _)
BoundaryAppliedFromActive_ForEach2_last_boundary_sequence(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some(outer_boundary_sequence)) IN BoundaryAppliedFromActive_ForEach2_last_boundary_sequence(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE BoundaryAppliedFromActive_ForEach2_lifecycle(_, _, _)
BoundaryAppliedFromActive_ForEach2_lifecycle(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "AppliedPendingConsumption") IN BoundaryAppliedFromActive_ForEach2_lifecycle(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE BoundaryAppliedFromRetired_ForEach3_last_boundary_sequence(_, _, _)
BoundaryAppliedFromRetired_ForEach3_last_boundary_sequence(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some(outer_boundary_sequence)) IN BoundaryAppliedFromRetired_ForEach3_last_boundary_sequence(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE BoundaryAppliedFromRetired_ForEach3_lifecycle(_, _, _)
BoundaryAppliedFromRetired_ForEach3_lifecycle(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "AppliedPendingConsumption") IN BoundaryAppliedFromRetired_ForEach3_lifecycle(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE RunCompletedFromActive_ForEach4_lifecycle(_, _)
RunCompletedFromActive_ForEach4_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Consumed") IN RunCompletedFromActive_ForEach4_lifecycle(next_acc, Tail(items))

RECURSIVE RunCompletedFromActive_ForEach4_terminal_outcome(_, _)
RunCompletedFromActive_ForEach4_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("Consumed")) IN RunCompletedFromActive_ForEach4_terminal_outcome(next_acc, Tail(items))

RECURSIVE RunCompletedFromRetired_ForEach5_lifecycle(_, _)
RunCompletedFromRetired_ForEach5_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Consumed") IN RunCompletedFromRetired_ForEach5_lifecycle(next_acc, Tail(items))

RECURSIVE RunCompletedFromRetired_ForEach5_terminal_outcome(_, _)
RunCompletedFromRetired_ForEach5_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("Consumed")) IN RunCompletedFromRetired_ForEach5_terminal_outcome(next_acc, Tail(items))

RECURSIVE RunFailedFromActive_ForEach6_lifecycle(_, _)
RunFailedFromActive_ForEach6_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") = "Staged") THEN MapSet(acc, work_id, "Queued") ELSE acc IN RunFailedFromActive_ForEach6_lifecycle(next_acc, Tail(items))

RECURSIVE RunFailedFromActive_ForEach6_queue(_, _, _)
RunFailedFromActive_ForEach6_queue(acc, items, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN handling_mode THEN handling_mode[work_id] ELSE "None") = "Steer") THEN acc ELSE (<<work_id>> \o acc) ELSE acc IN RunFailedFromActive_ForEach6_queue(next_acc, Tail(items), captured_lifecycle)

RECURSIVE RunFailedFromActive_ForEach6_steer_queue(_, _, _)
RunFailedFromActive_ForEach6_steer_queue(acc, items, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN handling_mode THEN handling_mode[work_id] ELSE "None") = "Steer") THEN (<<work_id>> \o acc) ELSE acc ELSE acc IN RunFailedFromActive_ForEach6_steer_queue(next_acc, Tail(items), captured_lifecycle)

RECURSIVE RunFailedFromRetired_ForEach7_lifecycle(_, _)
RunFailedFromRetired_ForEach7_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") = "Staged") THEN MapSet(acc, work_id, "Queued") ELSE acc IN RunFailedFromRetired_ForEach7_lifecycle(next_acc, Tail(items))

RECURSIVE RunFailedFromRetired_ForEach7_queue(_, _, _)
RunFailedFromRetired_ForEach7_queue(acc, items, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN handling_mode THEN handling_mode[work_id] ELSE "None") = "Steer") THEN acc ELSE (<<work_id>> \o acc) ELSE acc IN RunFailedFromRetired_ForEach7_queue(next_acc, Tail(items), captured_lifecycle)

RECURSIVE RunFailedFromRetired_ForEach7_steer_queue(_, _, _)
RunFailedFromRetired_ForEach7_steer_queue(acc, items, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN handling_mode THEN handling_mode[work_id] ELSE "None") = "Steer") THEN (<<work_id>> \o acc) ELSE acc ELSE acc IN RunFailedFromRetired_ForEach7_steer_queue(next_acc, Tail(items), captured_lifecycle)

RECURSIVE RunCancelledFromActive_ForEach8_lifecycle(_, _)
RunCancelledFromActive_ForEach8_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") = "Staged") THEN MapSet(acc, work_id, "Queued") ELSE acc IN RunCancelledFromActive_ForEach8_lifecycle(next_acc, Tail(items))

RECURSIVE RunCancelledFromActive_ForEach8_queue(_, _, _)
RunCancelledFromActive_ForEach8_queue(acc, items, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN handling_mode THEN handling_mode[work_id] ELSE "None") = "Steer") THEN acc ELSE (<<work_id>> \o acc) ELSE acc IN RunCancelledFromActive_ForEach8_queue(next_acc, Tail(items), captured_lifecycle)

RECURSIVE RunCancelledFromActive_ForEach8_steer_queue(_, _, _)
RunCancelledFromActive_ForEach8_steer_queue(acc, items, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN handling_mode THEN handling_mode[work_id] ELSE "None") = "Steer") THEN (<<work_id>> \o acc) ELSE acc ELSE acc IN RunCancelledFromActive_ForEach8_steer_queue(next_acc, Tail(items), captured_lifecycle)

RECURSIVE RunCancelledFromRetired_ForEach9_lifecycle(_, _)
RunCancelledFromRetired_ForEach9_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") = "Staged") THEN MapSet(acc, work_id, "Queued") ELSE acc IN RunCancelledFromRetired_ForEach9_lifecycle(next_acc, Tail(items))

RECURSIVE RunCancelledFromRetired_ForEach9_queue(_, _, _)
RunCancelledFromRetired_ForEach9_queue(acc, items, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN handling_mode THEN handling_mode[work_id] ELSE "None") = "Steer") THEN acc ELSE (<<work_id>> \o acc) ELSE acc IN RunCancelledFromRetired_ForEach9_queue(next_acc, Tail(items), captured_lifecycle)

RECURSIVE RunCancelledFromRetired_ForEach9_steer_queue(_, _, _)
RunCancelledFromRetired_ForEach9_steer_queue(acc, items, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN handling_mode THEN handling_mode[work_id] ELSE "None") = "Steer") THEN (<<work_id>> \o acc) ELSE acc ELSE acc IN RunCancelledFromRetired_ForEach9_steer_queue(next_acc, Tail(items), captured_lifecycle)

RECURSIVE CoalesceQueuedInputsFromActive_ForEach10_lifecycle(_, _)
CoalesceQueuedInputsFromActive_ForEach10_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Coalesced") IN CoalesceQueuedInputsFromActive_ForEach10_lifecycle(next_acc, Tail(items))

RECURSIVE CoalesceQueuedInputsFromActive_ForEach10_terminal_outcome(_, _)
CoalesceQueuedInputsFromActive_ForEach10_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("Coalesced")) IN CoalesceQueuedInputsFromActive_ForEach10_terminal_outcome(next_acc, Tail(items))

RECURSIVE CoalesceQueuedInputsFromRetired_ForEach11_lifecycle(_, _)
CoalesceQueuedInputsFromRetired_ForEach11_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Coalesced") IN CoalesceQueuedInputsFromRetired_ForEach11_lifecycle(next_acc, Tail(items))

RECURSIVE CoalesceQueuedInputsFromRetired_ForEach11_terminal_outcome(_, _)
CoalesceQueuedInputsFromRetired_ForEach11_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("Coalesced")) IN CoalesceQueuedInputsFromRetired_ForEach11_terminal_outcome(next_acc, Tail(items))

RECURSIVE ResetFromActive_ForEach12_lifecycle(_, _)
ResetFromActive_ForEach12_lifecycle(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET work_id == item IN LET next_acc == IF (((IF work_id \in DOMAIN terminal_outcome THEN terminal_outcome[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Abandoned")) THEN MapSet(acc, work_id, "Abandoned") ELSE acc IN ResetFromActive_ForEach12_lifecycle(next_acc, remaining \ {item})

RECURSIVE ResetFromActive_ForEach12_terminal_outcome(_, _, _)
ResetFromActive_ForEach12_terminal_outcome(acc, remaining, captured_lifecycle) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET work_id == item IN LET next_acc == IF (((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Abandoned")) THEN MapSet(acc, work_id, Some("AbandonedReset")) ELSE acc IN ResetFromActive_ForEach12_terminal_outcome(next_acc, remaining \ {item}, captured_lifecycle)

RECURSIVE ResetFromRetired_ForEach13_lifecycle(_, _)
ResetFromRetired_ForEach13_lifecycle(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET work_id == item IN LET next_acc == IF (((IF work_id \in DOMAIN terminal_outcome THEN terminal_outcome[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Abandoned")) THEN MapSet(acc, work_id, "Abandoned") ELSE acc IN ResetFromRetired_ForEach13_lifecycle(next_acc, remaining \ {item})

RECURSIVE ResetFromRetired_ForEach13_terminal_outcome(_, _, _)
ResetFromRetired_ForEach13_terminal_outcome(acc, remaining, captured_lifecycle) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET work_id == item IN LET next_acc == IF (((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Abandoned")) THEN MapSet(acc, work_id, Some("AbandonedReset")) ELSE acc IN ResetFromRetired_ForEach13_terminal_outcome(next_acc, remaining \ {item}, captured_lifecycle)

RECURSIVE Destroy_ForEach14_lifecycle(_, _)
Destroy_ForEach14_lifecycle(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET work_id == item IN LET next_acc == IF (((IF work_id \in DOMAIN terminal_outcome THEN terminal_outcome[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Abandoned")) THEN MapSet(acc, work_id, "Abandoned") ELSE acc IN Destroy_ForEach14_lifecycle(next_acc, remaining \ {item})

RECURSIVE Destroy_ForEach14_terminal_outcome(_, _, _)
Destroy_ForEach14_terminal_outcome(acc, remaining, captured_lifecycle) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET work_id == item IN LET next_acc == IF (((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Abandoned")) THEN MapSet(acc, work_id, Some("AbandonedDestroyed")) ELSE acc IN Destroy_ForEach14_terminal_outcome(next_acc, remaining \ {item}, captured_lifecycle)

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
    /\ wake_requested' = TRUE
    /\ process_requested' = (process_requested \/ FALSE)
    /\ UNCHANGED << steer_queue, current_run, current_run_contributors, silent_intent_overrides >>


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
    /\ wake_requested' = TRUE
    /\ process_requested' = (process_requested \/ TRUE)
    /\ UNCHANGED << queue, current_run, current_run_contributors, silent_intent_overrides >>


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
    /\ UNCHANGED << handling_mode, queue, steer_queue, current_run, current_run_contributors, wake_requested, process_requested, silent_intent_overrides >>


StageDrainSnapshotFromActive(run_id, contributing_work_ids) ==
    /\ phase = "Active"
    /\ (current_run = None)
    /\ (Len(contributing_work_ids) > 0)
    /\ (((Len(steer_queue) > 0) /\ StartsWith(steer_queue, contributing_work_ids)) \/ ((Len(steer_queue) = 0) /\ StartsWith(queue, contributing_work_ids)))
    /\ (\A work_id \in SeqElements(contributing_work_ids) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "Queued"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = StageDrainSnapshotFromActive_ForEach0_lifecycle(lifecycle, contributing_work_ids, run_id)
    /\ queue' = IF (Len(steer_queue) > 0) THEN queue ELSE SeqRemoveAll(queue, contributing_work_ids)
    /\ steer_queue' = IF (Len(steer_queue) > 0) THEN SeqRemoveAll(steer_queue, contributing_work_ids) ELSE steer_queue
    /\ current_run' = Some(run_id)
    /\ current_run_contributors' = contributing_work_ids
    /\ last_run' = StageDrainSnapshotFromActive_ForEach0_last_run(last_run, contributing_work_ids, run_id)
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, last_boundary_sequence, silent_intent_overrides >>


StageDrainSnapshotFromRetired(run_id, contributing_work_ids) ==
    /\ phase = "Retired"
    /\ (current_run = None)
    /\ (Len(contributing_work_ids) > 0)
    /\ (((Len(steer_queue) > 0) /\ StartsWith(steer_queue, contributing_work_ids)) \/ ((Len(steer_queue) = 0) /\ StartsWith(queue, contributing_work_ids)))
    /\ (\A work_id \in SeqElements(contributing_work_ids) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "Queued"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = StageDrainSnapshotFromRetired_ForEach1_lifecycle(lifecycle, contributing_work_ids, run_id)
    /\ queue' = IF (Len(steer_queue) > 0) THEN queue ELSE SeqRemoveAll(queue, contributing_work_ids)
    /\ steer_queue' = IF (Len(steer_queue) > 0) THEN SeqRemoveAll(steer_queue, contributing_work_ids) ELSE steer_queue
    /\ current_run' = Some(run_id)
    /\ current_run_contributors' = contributing_work_ids
    /\ last_run' = StageDrainSnapshotFromRetired_ForEach1_last_run(last_run, contributing_work_ids, run_id)
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, last_boundary_sequence, silent_intent_overrides >>


BoundaryAppliedFromActive(run_id, boundary_sequence) ==
    /\ phase = "Active"
    /\ (current_run = Some(run_id))
    /\ (\A work_id \in SeqElements(current_run_contributors) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "Staged"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = BoundaryAppliedFromActive_ForEach2_lifecycle(lifecycle, current_run_contributors, boundary_sequence)
    /\ last_boundary_sequence' = BoundaryAppliedFromActive_ForEach2_last_boundary_sequence(last_boundary_sequence, current_run_contributors, boundary_sequence)
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, queue, steer_queue, current_run, current_run_contributors, last_run, wake_requested, process_requested, silent_intent_overrides >>


BoundaryAppliedFromRetired(run_id, boundary_sequence) ==
    /\ phase = "Retired"
    /\ (current_run = Some(run_id))
    /\ (\A work_id \in SeqElements(current_run_contributors) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "Staged"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = BoundaryAppliedFromRetired_ForEach3_lifecycle(lifecycle, current_run_contributors, boundary_sequence)
    /\ last_boundary_sequence' = BoundaryAppliedFromRetired_ForEach3_last_boundary_sequence(last_boundary_sequence, current_run_contributors, boundary_sequence)
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, queue, steer_queue, current_run, current_run_contributors, last_run, wake_requested, process_requested, silent_intent_overrides >>


RunCompletedFromActive(run_id) ==
    /\ phase = "Active"
    /\ (current_run = Some(run_id))
    /\ (\A work_id \in SeqElements(current_run_contributors) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "AppliedPendingConsumption"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RunCompletedFromActive_ForEach4_lifecycle(lifecycle, current_run_contributors)
    /\ terminal_outcome' = RunCompletedFromActive_ForEach4_terminal_outcome(terminal_outcome, current_run_contributors)
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, queue, steer_queue, last_run, last_boundary_sequence, wake_requested, process_requested, silent_intent_overrides >>


RunCompletedFromRetired(run_id) ==
    /\ phase = "Retired"
    /\ (current_run = Some(run_id))
    /\ (\A work_id \in SeqElements(current_run_contributors) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "AppliedPendingConsumption"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RunCompletedFromRetired_ForEach5_lifecycle(lifecycle, current_run_contributors)
    /\ terminal_outcome' = RunCompletedFromRetired_ForEach5_terminal_outcome(terminal_outcome, current_run_contributors)
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, queue, steer_queue, last_run, last_boundary_sequence, wake_requested, process_requested, silent_intent_overrides >>


RunFailedFromActive(run_id) ==
    /\ phase = "Active"
    /\ (current_run = Some(run_id))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RunFailedFromActive_ForEach6_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = RunFailedFromActive_ForEach6_queue(queue, current_run_contributors, RunFailedFromActive_ForEach6_lifecycle(lifecycle, current_run_contributors))
    /\ steer_queue' = RunFailedFromActive_ForEach6_steer_queue(steer_queue, current_run_contributors, RunFailedFromActive_ForEach6_lifecycle(lifecycle, current_run_contributors))
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF ((Len(RunFailedFromActive_ForEach6_queue(queue, current_run_contributors, RunFailedFromActive_ForEach6_lifecycle(lifecycle, current_run_contributors))) > 0) \/ (Len(RunFailedFromActive_ForEach6_steer_queue(steer_queue, current_run_contributors, RunFailedFromActive_ForEach6_lifecycle(lifecycle, current_run_contributors))) > 0)) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, last_run, last_boundary_sequence, process_requested, silent_intent_overrides >>


RunFailedFromRetired(run_id) ==
    /\ phase = "Retired"
    /\ (current_run = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RunFailedFromRetired_ForEach7_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = RunFailedFromRetired_ForEach7_queue(queue, current_run_contributors, RunFailedFromRetired_ForEach7_lifecycle(lifecycle, current_run_contributors))
    /\ steer_queue' = RunFailedFromRetired_ForEach7_steer_queue(steer_queue, current_run_contributors, RunFailedFromRetired_ForEach7_lifecycle(lifecycle, current_run_contributors))
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF ((Len(RunFailedFromRetired_ForEach7_queue(queue, current_run_contributors, RunFailedFromRetired_ForEach7_lifecycle(lifecycle, current_run_contributors))) > 0) \/ (Len(RunFailedFromRetired_ForEach7_steer_queue(steer_queue, current_run_contributors, RunFailedFromRetired_ForEach7_lifecycle(lifecycle, current_run_contributors))) > 0)) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, last_run, last_boundary_sequence, process_requested, silent_intent_overrides >>


RunCancelledFromActive(run_id) ==
    /\ phase = "Active"
    /\ (current_run = Some(run_id))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RunCancelledFromActive_ForEach8_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = RunCancelledFromActive_ForEach8_queue(queue, current_run_contributors, RunCancelledFromActive_ForEach8_lifecycle(lifecycle, current_run_contributors))
    /\ steer_queue' = RunCancelledFromActive_ForEach8_steer_queue(steer_queue, current_run_contributors, RunCancelledFromActive_ForEach8_lifecycle(lifecycle, current_run_contributors))
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF ((Len(RunCancelledFromActive_ForEach8_queue(queue, current_run_contributors, RunCancelledFromActive_ForEach8_lifecycle(lifecycle, current_run_contributors))) > 0) \/ (Len(RunCancelledFromActive_ForEach8_steer_queue(steer_queue, current_run_contributors, RunCancelledFromActive_ForEach8_lifecycle(lifecycle, current_run_contributors))) > 0)) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, last_run, last_boundary_sequence, process_requested, silent_intent_overrides >>


RunCancelledFromRetired(run_id) ==
    /\ phase = "Retired"
    /\ (current_run = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RunCancelledFromRetired_ForEach9_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = RunCancelledFromRetired_ForEach9_queue(queue, current_run_contributors, RunCancelledFromRetired_ForEach9_lifecycle(lifecycle, current_run_contributors))
    /\ steer_queue' = RunCancelledFromRetired_ForEach9_steer_queue(steer_queue, current_run_contributors, RunCancelledFromRetired_ForEach9_lifecycle(lifecycle, current_run_contributors))
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF ((Len(RunCancelledFromRetired_ForEach9_queue(queue, current_run_contributors, RunCancelledFromRetired_ForEach9_lifecycle(lifecycle, current_run_contributors))) > 0) \/ (Len(RunCancelledFromRetired_ForEach9_steer_queue(steer_queue, current_run_contributors, RunCancelledFromRetired_ForEach9_lifecycle(lifecycle, current_run_contributors))) > 0)) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, last_run, last_boundary_sequence, process_requested, silent_intent_overrides >>


SupersedeQueuedInputFromActive(new_work_id, old_work_id) ==
    /\ phase = "Active"
    /\ (new_work_id \in admitted_inputs)
    /\ ((IF old_work_id \in DOMAIN lifecycle THEN lifecycle[old_work_id] ELSE "None") = "Queued")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = MapSet(lifecycle, old_work_id, "Superseded")
    /\ terminal_outcome' = MapSet(terminal_outcome, old_work_id, Some("Superseded"))
    /\ queue' = SeqRemove(queue, old_work_id)
    /\ steer_queue' = SeqRemove(steer_queue, old_work_id)
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested, silent_intent_overrides >>


SupersedeQueuedInputFromRetired(new_work_id, old_work_id) ==
    /\ phase = "Retired"
    /\ (new_work_id \in admitted_inputs)
    /\ ((IF old_work_id \in DOMAIN lifecycle THEN lifecycle[old_work_id] ELSE "None") = "Queued")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = MapSet(lifecycle, old_work_id, "Superseded")
    /\ terminal_outcome' = MapSet(terminal_outcome, old_work_id, Some("Superseded"))
    /\ queue' = SeqRemove(queue, old_work_id)
    /\ steer_queue' = SeqRemove(steer_queue, old_work_id)
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested, silent_intent_overrides >>


CoalesceQueuedInputsFromActive(aggregate_work_id, source_work_ids) ==
    /\ phase = "Active"
    /\ (aggregate_work_id \in admitted_inputs)
    /\ (Len(source_work_ids) > 0)
    /\ (\A work_id \in SeqElements(source_work_ids) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "Queued"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = CoalesceQueuedInputsFromActive_ForEach10_lifecycle(lifecycle, source_work_ids)
    /\ terminal_outcome' = CoalesceQueuedInputsFromActive_ForEach10_terminal_outcome(terminal_outcome, source_work_ids)
    /\ queue' = SeqRemoveAll(queue, source_work_ids)
    /\ steer_queue' = SeqRemoveAll(steer_queue, source_work_ids)
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested, silent_intent_overrides >>


CoalesceQueuedInputsFromRetired(aggregate_work_id, source_work_ids) ==
    /\ phase = "Retired"
    /\ (aggregate_work_id \in admitted_inputs)
    /\ (Len(source_work_ids) > 0)
    /\ (\A work_id \in SeqElements(source_work_ids) : ((IF work_id \in DOMAIN lifecycle THEN lifecycle[work_id] ELSE "None") = "Queued"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = CoalesceQueuedInputsFromRetired_ForEach11_lifecycle(lifecycle, source_work_ids)
    /\ terminal_outcome' = CoalesceQueuedInputsFromRetired_ForEach11_terminal_outcome(terminal_outcome, source_work_ids)
    /\ queue' = SeqRemoveAll(queue, source_work_ids)
    /\ steer_queue' = SeqRemoveAll(steer_queue, source_work_ids)
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested, silent_intent_overrides >>


Retire ==
    /\ phase = "Active"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, lifecycle, terminal_outcome, queue, steer_queue, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested, silent_intent_overrides >>


ResetFromActive ==
    /\ phase = "Active"
    /\ (current_run = None)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = ResetFromActive_ForEach12_lifecycle(lifecycle, DOMAIN lifecycle)
    /\ terminal_outcome' = ResetFromActive_ForEach12_terminal_outcome(terminal_outcome, DOMAIN lifecycle, ResetFromActive_ForEach12_lifecycle(lifecycle, DOMAIN lifecycle))
    /\ queue' = <<>>
    /\ steer_queue' = <<>>
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, last_run, last_boundary_sequence, silent_intent_overrides >>


ResetFromRetired ==
    /\ phase = "Retired"
    /\ (current_run = None)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = ResetFromRetired_ForEach13_lifecycle(lifecycle, DOMAIN lifecycle)
    /\ terminal_outcome' = ResetFromRetired_ForEach13_terminal_outcome(terminal_outcome, DOMAIN lifecycle, ResetFromRetired_ForEach13_lifecycle(lifecycle, DOMAIN lifecycle))
    /\ queue' = <<>>
    /\ steer_queue' = <<>>
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, last_run, last_boundary_sequence, silent_intent_overrides >>


Destroy ==
    /\ phase = "Active" \/ phase = "Retired"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = Destroy_ForEach14_lifecycle(lifecycle, DOMAIN lifecycle)
    /\ terminal_outcome' = Destroy_ForEach14_terminal_outcome(terminal_outcome, DOMAIN lifecycle, Destroy_ForEach14_lifecycle(lifecycle, DOMAIN lifecycle))
    /\ queue' = <<>>
    /\ steer_queue' = <<>>
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = FALSE
    /\ process_requested' = FALSE
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, last_run, last_boundary_sequence, silent_intent_overrides >>


RecoverFromActive ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RecoverFromActive_ForEach15_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = IF (Len(current_run_contributors) > 0) THEN (current_run_contributors \o queue) ELSE queue
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF (Len(current_run_contributors) > 0) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, steer_queue, last_run, last_boundary_sequence, process_requested, silent_intent_overrides >>


RecoverFromRetired ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ lifecycle' = RecoverFromRetired_ForEach16_lifecycle(lifecycle, current_run_contributors)
    /\ queue' = IF (Len(current_run_contributors) > 0) THEN (current_run_contributors \o queue) ELSE queue
    /\ current_run' = None
    /\ current_run_contributors' = <<>>
    /\ wake_requested' = IF (Len(current_run_contributors) > 0) THEN TRUE ELSE wake_requested
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, terminal_outcome, steer_queue, last_run, last_boundary_sequence, process_requested, silent_intent_overrides >>


SetSilentIntentOverridesFromActive(intents) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, lifecycle, terminal_outcome, queue, steer_queue, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested >>


SetSilentIntentOverridesFromRetired(intents) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << admitted_inputs, admission_order, content_shape, request_id, reservation_key, policy_snapshot, handling_mode, lifecycle, terminal_outcome, queue, steer_queue, current_run, current_run_contributors, last_run, last_boundary_sequence, wake_requested, process_requested >>


Next ==
    \/ \E work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E policy \in PolicyDecisionValues : AdmitQueuedQueue(work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, policy)
    \/ \E work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E policy \in PolicyDecisionValues : AdmitQueuedSteer(work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, policy)
    \/ \E work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E policy \in PolicyDecisionValues : AdmitConsumedOnAccept(work_id, arg_content_shape, arg_request_id, arg_reservation_key, policy)
    \/ \E run_id \in RunIdValues : \E contributing_work_ids \in SeqOfWorkIdValues : StageDrainSnapshotFromActive(run_id, contributing_work_ids)
    \/ \E run_id \in RunIdValues : \E contributing_work_ids \in SeqOfWorkIdValues : StageDrainSnapshotFromRetired(run_id, contributing_work_ids)
    \/ \E run_id \in RunIdValues : \E boundary_sequence \in 0..2 : BoundaryAppliedFromActive(run_id, boundary_sequence)
    \/ \E run_id \in RunIdValues : \E boundary_sequence \in 0..2 : BoundaryAppliedFromRetired(run_id, boundary_sequence)
    \/ \E run_id \in RunIdValues : RunCompletedFromActive(run_id)
    \/ \E run_id \in RunIdValues : RunCompletedFromRetired(run_id)
    \/ \E run_id \in RunIdValues : RunFailedFromActive(run_id)
    \/ \E run_id \in RunIdValues : RunFailedFromRetired(run_id)
    \/ \E run_id \in RunIdValues : RunCancelledFromActive(run_id)
    \/ \E run_id \in RunIdValues : RunCancelledFromRetired(run_id)
    \/ \E new_work_id \in WorkIdValues : \E old_work_id \in WorkIdValues : SupersedeQueuedInputFromActive(new_work_id, old_work_id)
    \/ \E new_work_id \in WorkIdValues : \E old_work_id \in WorkIdValues : SupersedeQueuedInputFromRetired(new_work_id, old_work_id)
    \/ \E aggregate_work_id \in WorkIdValues : \E source_work_ids \in SeqOfWorkIdValues : CoalesceQueuedInputsFromActive(aggregate_work_id, source_work_ids)
    \/ \E aggregate_work_id \in WorkIdValues : \E source_work_ids \in SeqOfWorkIdValues : CoalesceQueuedInputsFromRetired(aggregate_work_id, source_work_ids)
    \/ Retire
    \/ ResetFromActive
    \/ ResetFromRetired
    \/ Destroy
    \/ RecoverFromActive
    \/ RecoverFromRetired
    \/ \E intents \in SetOfStringValues : SetSilentIntentOverridesFromActive(intents)
    \/ \E intents \in SetOfStringValues : SetSilentIntentOverridesFromRetired(intents)
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

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(admitted_inputs) <= 1 /\ Len(admission_order) <= 1 /\ Cardinality(DOMAIN content_shape) <= 1 /\ Cardinality(DOMAIN request_id) <= 1 /\ Cardinality(DOMAIN reservation_key) <= 1 /\ Cardinality(DOMAIN policy_snapshot) <= 1 /\ Cardinality(DOMAIN handling_mode) <= 1 /\ Cardinality(DOMAIN lifecycle) <= 1 /\ Cardinality(DOMAIN terminal_outcome) <= 1 /\ Len(queue) <= 1 /\ Len(steer_queue) <= 1 /\ Len(current_run_contributors) <= 1 /\ Cardinality(DOMAIN last_run) <= 1 /\ Cardinality(DOMAIN last_boundary_sequence) <= 1 /\ Cardinality(silent_intent_overrides) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(admitted_inputs) <= 2 /\ Len(admission_order) <= 2 /\ Cardinality(DOMAIN content_shape) <= 2 /\ Cardinality(DOMAIN request_id) <= 2 /\ Cardinality(DOMAIN reservation_key) <= 2 /\ Cardinality(DOMAIN policy_snapshot) <= 2 /\ Cardinality(DOMAIN handling_mode) <= 2 /\ Cardinality(DOMAIN lifecycle) <= 2 /\ Cardinality(DOMAIN terminal_outcome) <= 2 /\ Len(queue) <= 2 /\ Len(steer_queue) <= 2 /\ Len(current_run_contributors) <= 2 /\ Cardinality(DOMAIN last_run) <= 2 /\ Cardinality(DOMAIN last_boundary_sequence) <= 2 /\ Cardinality(silent_intent_overrides) <= 2

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
