---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for OpsLifecycleMachine.

CONSTANTS OperationIdValues, OperationKindValues, WaitRequestIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

SeqOfOperationIdValues == {<<>>} \cup {<<x>> : x \in OperationIdValues} \cup {<<x, y>> : x \in OperationIdValues, y \in OperationIdValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, known_operations, operation_status, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, terminal_buffered, completed_order, max_completed, max_concurrent, active_count, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids

vars == << phase, model_step_count, known_operations, operation_status, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, terminal_buffered, completed_order, max_completed, max_concurrent, active_count, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>

terminal_outcome_matches_status(status, arg_terminal_outcome) == (((status = "Completed") /\ (arg_terminal_outcome = "Completed")) \/ ((status = "Failed") /\ (arg_terminal_outcome = "Failed")) \/ ((status = "Cancelled") /\ (arg_terminal_outcome = "Cancelled")) \/ ((status = "Retired") /\ (arg_terminal_outcome = "Retired")) \/ ((status = "Terminated") /\ (arg_terminal_outcome = "Terminated")))
is_owner_terminatable_status(status) == ((status = "Provisioning") \/ (status = "Running") \/ (status = "Retiring"))
is_terminal_status(status) == ((status = "Completed") \/ (status = "Failed") \/ (status = "Cancelled") \/ (status = "Retired") \/ (status = "Terminated"))
terminal_buffered_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN terminal_buffered THEN terminal_buffered[operation_id] ELSE FALSE) ELSE FALSE)
terminal_outcome_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN terminal_outcome THEN terminal_outcome[operation_id] ELSE "None") ELSE "None")
progress_count_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN progress_count THEN progress_count[operation_id] ELSE 0) ELSE 0)
watcher_count_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN watcher_count THEN watcher_count[operation_id] ELSE 0) ELSE 0)
peer_ready_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN peer_ready THEN peer_ready[operation_id] ELSE FALSE) ELSE FALSE)
wait_tracks_operation(operation_id) == (operation_id \in SeqElements(wait_operation_ids))
all_operations_known(operation_ids) == (\A operation_id \in SeqElements(operation_ids) : (operation_id \in known_operations))
wait_is_active == wait_active
kind_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN operation_kind THEN operation_kind[operation_id] ELSE "None") ELSE "None")
status_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN operation_status THEN operation_status[operation_id] ELSE "None") ELSE "Absent")
wait_completes_on_terminal(operation_id) == (wait_is_active /\ wait_tracks_operation(operation_id) /\ (\A tracked_operation_id \in SeqElements(wait_operation_ids) : ((tracked_operation_id = operation_id) \/ is_terminal_status(status_of(tracked_operation_id)))))
all_operations_terminal(operation_ids) == (\A operation_id \in SeqElements(operation_ids) : is_terminal_status(status_of(operation_id)))

Init ==
    /\ phase = "Active"
    /\ model_step_count = 0
    /\ known_operations = {}
    /\ operation_status = [x \in {} |-> None]
    /\ operation_kind = [x \in {} |-> None]
    /\ peer_ready = [x \in {} |-> None]
    /\ progress_count = [x \in {} |-> None]
    /\ watcher_count = [x \in {} |-> None]
    /\ terminal_outcome = [x \in {} |-> None]
    /\ terminal_buffered = [x \in {} |-> None]
    /\ completed_order = <<>>
    /\ max_completed = 256
    /\ max_concurrent = 0
    /\ active_count = 0
    /\ created_at_ms = [x \in {} |-> None]
    /\ completed_at_ms = [x \in {} |-> None]
    /\ wait_active = FALSE
    /\ wait_request_id = "wait_request_none"
    /\ wait_operation_ids = <<>>

RECURSIVE OwnerTerminatedCompletesWait_ForEach0_operation_status(_, _)
OwnerTerminatedCompletesWait_ForEach0_operation_status(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF is_owner_terminatable_status(status_of(operation_id)) THEN MapSet(acc, operation_id, "Terminated") ELSE acc IN OwnerTerminatedCompletesWait_ForEach0_operation_status(next_acc, remaining \ {item})

RECURSIVE OwnerTerminatedCompletesWait_ForEach0_terminal_buffered(_, _)
OwnerTerminatedCompletesWait_ForEach0_terminal_buffered(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF is_owner_terminatable_status(status_of(operation_id)) THEN MapSet(acc, operation_id, TRUE) ELSE acc IN OwnerTerminatedCompletesWait_ForEach0_terminal_buffered(next_acc, remaining \ {item})

RECURSIVE OwnerTerminatedCompletesWait_ForEach0_terminal_outcome(_, _)
OwnerTerminatedCompletesWait_ForEach0_terminal_outcome(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF is_owner_terminatable_status(status_of(operation_id)) THEN MapSet(acc, operation_id, "Terminated") ELSE acc IN OwnerTerminatedCompletesWait_ForEach0_terminal_outcome(next_acc, remaining \ {item})

RECURSIVE OwnerTerminated_ForEach1_operation_status(_, _)
OwnerTerminated_ForEach1_operation_status(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF is_owner_terminatable_status(status_of(operation_id)) THEN MapSet(acc, operation_id, "Terminated") ELSE acc IN OwnerTerminated_ForEach1_operation_status(next_acc, remaining \ {item})

RECURSIVE OwnerTerminated_ForEach1_terminal_buffered(_, _)
OwnerTerminated_ForEach1_terminal_buffered(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF is_owner_terminatable_status(status_of(operation_id)) THEN MapSet(acc, operation_id, TRUE) ELSE acc IN OwnerTerminated_ForEach1_terminal_buffered(next_acc, remaining \ {item})

RECURSIVE OwnerTerminated_ForEach1_terminal_outcome(_, _)
OwnerTerminated_ForEach1_terminal_outcome(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF is_owner_terminatable_status(status_of(operation_id)) THEN MapSet(acc, operation_id, "Terminated") ELSE acc IN OwnerTerminated_ForEach1_terminal_outcome(next_acc, remaining \ {item})

RegisterOperation(operation_id, arg_operation_kind) ==
    /\ phase = "Active"
    /\ (status_of(operation_id) = "Absent")
    /\ (arg_operation_kind # "None")
    /\ ((max_concurrent = 0) \/ (active_count < max_concurrent))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ known_operations' = (known_operations \cup {operation_id})
    /\ operation_status' = MapSet(operation_status, operation_id, "Provisioning")
    /\ operation_kind' = MapSet(operation_kind, operation_id, arg_operation_kind)
    /\ peer_ready' = MapSet(peer_ready, operation_id, FALSE)
    /\ progress_count' = MapSet(progress_count, operation_id, 0)
    /\ watcher_count' = MapSet(watcher_count, operation_id, 0)
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "None")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, FALSE)
    /\ active_count' = (active_count) + 1
    /\ created_at_ms' = MapSet(created_at_ms, operation_id, 0)
    /\ completed_at_ms' = MapSet(completed_at_ms, operation_id, 0)
    /\ UNCHANGED << completed_order, max_completed, max_concurrent, wait_active, wait_request_id, wait_operation_ids >>


ProvisioningSucceeded(operation_id) ==
    /\ phase = "Active"
    /\ (status_of(operation_id) = "Provisioning")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Running")
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, terminal_buffered, completed_order, max_completed, max_concurrent, active_count, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


ProvisioningFailedCompletesWait(operation_id) ==
    /\ phase = "Active"
    /\ (status_of(operation_id) = "Provisioning")
    /\ wait_completes_on_terminal(operation_id)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Failed")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Failed")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ completed_order' = Append(completed_order, operation_id)
    /\ active_count' = (active_count) - 1
    /\ wait_active' = FALSE
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, max_completed, max_concurrent, created_at_ms, completed_at_ms, wait_request_id, wait_operation_ids >>


ProvisioningFailed(operation_id) ==
    /\ phase = "Active"
    /\ (status_of(operation_id) = "Provisioning")
    /\ ~(wait_completes_on_terminal(operation_id))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Failed")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Failed")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ completed_order' = Append(completed_order, operation_id)
    /\ active_count' = (active_count) - 1
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, max_completed, max_concurrent, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


PeerReady(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ (kind_of(operation_id) = "MobMemberChild")
    /\ (peer_ready_of(operation_id) = FALSE)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ready' = MapSet(peer_ready, operation_id, TRUE)
    /\ UNCHANGED << known_operations, operation_status, operation_kind, progress_count, watcher_count, terminal_outcome, terminal_buffered, completed_order, max_completed, max_concurrent, active_count, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


RegisterWatcher(operation_id) ==
    /\ phase = "Active"
    /\ (status_of(operation_id) # "Absent")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ watcher_count' = MapSet(watcher_count, operation_id, (watcher_count_of(operation_id) + 1))
    /\ UNCHANGED << known_operations, operation_status, operation_kind, peer_ready, progress_count, terminal_outcome, terminal_buffered, completed_order, max_completed, max_concurrent, active_count, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


ProgressReported(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ progress_count' = MapSet(progress_count, operation_id, (progress_count_of(operation_id) + 1))
    /\ UNCHANGED << known_operations, operation_status, operation_kind, peer_ready, watcher_count, terminal_outcome, terminal_buffered, completed_order, max_completed, max_concurrent, active_count, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


CompleteOperationCompletesWait(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ wait_completes_on_terminal(operation_id)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Completed")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Completed")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ completed_order' = Append(completed_order, operation_id)
    /\ active_count' = (active_count) - 1
    /\ wait_active' = FALSE
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, max_completed, max_concurrent, created_at_ms, completed_at_ms, wait_request_id, wait_operation_ids >>


CompleteOperation(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ ~(wait_completes_on_terminal(operation_id))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Completed")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Completed")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ completed_order' = Append(completed_order, operation_id)
    /\ active_count' = (active_count) - 1
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, max_completed, max_concurrent, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


FailOperationCompletesWait(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Provisioning") \/ (status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ wait_completes_on_terminal(operation_id)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Failed")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Failed")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ completed_order' = Append(completed_order, operation_id)
    /\ active_count' = (active_count) - 1
    /\ wait_active' = FALSE
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, max_completed, max_concurrent, created_at_ms, completed_at_ms, wait_request_id, wait_operation_ids >>


FailOperation(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Provisioning") \/ (status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ ~(wait_completes_on_terminal(operation_id))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Failed")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Failed")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ completed_order' = Append(completed_order, operation_id)
    /\ active_count' = (active_count) - 1
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, max_completed, max_concurrent, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


CancelOperationCompletesWait(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Provisioning") \/ (status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ wait_completes_on_terminal(operation_id)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Cancelled")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Cancelled")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ completed_order' = Append(completed_order, operation_id)
    /\ active_count' = (active_count) - 1
    /\ wait_active' = FALSE
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, max_completed, max_concurrent, created_at_ms, completed_at_ms, wait_request_id, wait_operation_ids >>


CancelOperation(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Provisioning") \/ (status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ ~(wait_completes_on_terminal(operation_id))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Cancelled")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Cancelled")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ completed_order' = Append(completed_order, operation_id)
    /\ active_count' = (active_count) - 1
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, max_completed, max_concurrent, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


RetireRequested(operation_id) ==
    /\ phase = "Active"
    /\ (status_of(operation_id) = "Running")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Retiring")
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, terminal_buffered, completed_order, max_completed, max_concurrent, active_count, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


RetireCompletedCompletesWait(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ wait_completes_on_terminal(operation_id)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Retired")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Retired")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ completed_order' = Append(completed_order, operation_id)
    /\ active_count' = (active_count) - 1
    /\ wait_active' = FALSE
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, max_completed, max_concurrent, created_at_ms, completed_at_ms, wait_request_id, wait_operation_ids >>


RetireCompleted(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ ~(wait_completes_on_terminal(operation_id))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Retired")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Retired")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ completed_order' = Append(completed_order, operation_id)
    /\ active_count' = (active_count) - 1
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, max_completed, max_concurrent, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


CollectTerminal(operation_id) ==
    /\ phase = "Active"
    /\ is_terminal_status(status_of(operation_id))
    /\ terminal_buffered_of(operation_id)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, FALSE)
    /\ UNCHANGED << known_operations, operation_status, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, completed_order, max_completed, max_concurrent, active_count, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


OwnerTerminatedCompletesWait ==
    /\ phase = "Active"
    /\ wait_is_active
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = OwnerTerminatedCompletesWait_ForEach0_operation_status(operation_status, known_operations)
    /\ terminal_outcome' = OwnerTerminatedCompletesWait_ForEach0_terminal_outcome(terminal_outcome, known_operations)
    /\ terminal_buffered' = OwnerTerminatedCompletesWait_ForEach0_terminal_buffered(terminal_buffered, known_operations)
    /\ active_count' = 0
    /\ wait_active' = FALSE
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, completed_order, max_completed, max_concurrent, created_at_ms, completed_at_ms, wait_request_id, wait_operation_ids >>


OwnerTerminated ==
    /\ phase = "Active"
    /\ ~(wait_is_active)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = OwnerTerminated_ForEach1_operation_status(operation_status, known_operations)
    /\ terminal_outcome' = OwnerTerminated_ForEach1_terminal_outcome(terminal_outcome, known_operations)
    /\ terminal_buffered' = OwnerTerminated_ForEach1_terminal_buffered(terminal_buffered, known_operations)
    /\ active_count' = 0
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, completed_order, max_completed, max_concurrent, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


BeginWaitAllImmediate(arg_wait_request_id, operation_ids) ==
    /\ phase = "Active"
    /\ ~(wait_is_active)
    /\ all_operations_known(operation_ids)
    /\ all_operations_terminal(operation_ids)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << known_operations, operation_status, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, terminal_buffered, completed_order, max_completed, max_concurrent, active_count, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


BeginWaitAllPending(arg_wait_request_id, operation_ids) ==
    /\ phase = "Active"
    /\ ~(wait_is_active)
    /\ all_operations_known(operation_ids)
    /\ ~(all_operations_terminal(operation_ids))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ wait_active' = TRUE
    /\ wait_request_id' = arg_wait_request_id
    /\ wait_operation_ids' = operation_ids
    /\ UNCHANGED << known_operations, operation_status, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, terminal_buffered, completed_order, max_completed, max_concurrent, active_count, created_at_ms, completed_at_ms >>


CancelWaitAll(arg_wait_request_id) ==
    /\ phase = "Active"
    /\ wait_is_active
    /\ (wait_request_id = arg_wait_request_id)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ wait_active' = FALSE
    /\ wait_request_id' = "wait_request_none"
    /\ wait_operation_ids' = <<>>
    /\ UNCHANGED << known_operations, operation_status, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, terminal_buffered, completed_order, max_completed, max_concurrent, active_count, created_at_ms, completed_at_ms >>


CollectCompleted ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << known_operations, operation_status, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, terminal_buffered, completed_order, max_completed, max_concurrent, active_count, created_at_ms, completed_at_ms, wait_active, wait_request_id, wait_operation_ids >>


Next ==
    \/ \E operation_id \in OperationIdValues : \E arg_operation_kind \in OperationKindValues : RegisterOperation(operation_id, arg_operation_kind)
    \/ \E operation_id \in OperationIdValues : ProvisioningSucceeded(operation_id)
    \/ \E operation_id \in OperationIdValues : ProvisioningFailedCompletesWait(operation_id)
    \/ \E operation_id \in OperationIdValues : ProvisioningFailed(operation_id)
    \/ \E operation_id \in OperationIdValues : PeerReady(operation_id)
    \/ \E operation_id \in OperationIdValues : RegisterWatcher(operation_id)
    \/ \E operation_id \in OperationIdValues : ProgressReported(operation_id)
    \/ \E operation_id \in OperationIdValues : CompleteOperationCompletesWait(operation_id)
    \/ \E operation_id \in OperationIdValues : CompleteOperation(operation_id)
    \/ \E operation_id \in OperationIdValues : FailOperationCompletesWait(operation_id)
    \/ \E operation_id \in OperationIdValues : FailOperation(operation_id)
    \/ \E operation_id \in OperationIdValues : CancelOperationCompletesWait(operation_id)
    \/ \E operation_id \in OperationIdValues : CancelOperation(operation_id)
    \/ \E operation_id \in OperationIdValues : RetireRequested(operation_id)
    \/ \E operation_id \in OperationIdValues : RetireCompletedCompletesWait(operation_id)
    \/ \E operation_id \in OperationIdValues : RetireCompleted(operation_id)
    \/ \E operation_id \in OperationIdValues : CollectTerminal(operation_id)
    \/ OwnerTerminatedCompletesWait
    \/ OwnerTerminated
    \/ \E arg_wait_request_id \in WaitRequestIdValues : \E operation_ids \in SeqOfOperationIdValues : BeginWaitAllImmediate(arg_wait_request_id, operation_ids)
    \/ \E arg_wait_request_id \in WaitRequestIdValues : \E operation_ids \in SeqOfOperationIdValues : BeginWaitAllPending(arg_wait_request_id, operation_ids)
    \/ \E arg_wait_request_id \in WaitRequestIdValues : CancelWaitAll(arg_wait_request_id)
    \/ CollectCompleted

terminal_buffered_only_for_terminal_states == (\A operation_id \in known_operations : (~(terminal_buffered_of(operation_id)) \/ is_terminal_status(status_of(operation_id))))
peer_ready_implies_mob_member_child == (\A operation_id \in known_operations : (~(peer_ready_of(operation_id)) \/ (kind_of(operation_id) = "MobMemberChild")))
peer_ready_implies_present == (\A operation_id \in known_operations : (~(peer_ready_of(operation_id)) \/ (status_of(operation_id) # "Absent")))
present_operations_keep_kind_identity == (\A operation_id \in known_operations : ((status_of(operation_id) # "Absent") /\ (kind_of(operation_id) # "None")))
terminal_statuses_have_matching_terminal_outcome == (\A operation_id \in known_operations : (~(is_terminal_status(status_of(operation_id))) \/ terminal_outcome_matches_status(status_of(operation_id), terminal_outcome_of(operation_id))))
nonterminal_statuses_have_no_terminal_outcome == (\A operation_id \in known_operations : (is_terminal_status(status_of(operation_id)) \/ (terminal_outcome_of(operation_id) = "None")))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(known_operations) <= 1 /\ Cardinality(DOMAIN operation_status) <= 1 /\ Cardinality(DOMAIN operation_kind) <= 1 /\ Cardinality(DOMAIN peer_ready) <= 1 /\ Cardinality(DOMAIN progress_count) <= 1 /\ Cardinality(DOMAIN watcher_count) <= 1 /\ Cardinality(DOMAIN terminal_outcome) <= 1 /\ Cardinality(DOMAIN terminal_buffered) <= 1 /\ Len(completed_order) <= 1 /\ Cardinality(DOMAIN created_at_ms) <= 1 /\ Cardinality(DOMAIN completed_at_ms) <= 1 /\ Len(wait_operation_ids) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(known_operations) <= 2 /\ Cardinality(DOMAIN operation_status) <= 2 /\ Cardinality(DOMAIN operation_kind) <= 2 /\ Cardinality(DOMAIN peer_ready) <= 2 /\ Cardinality(DOMAIN progress_count) <= 2 /\ Cardinality(DOMAIN watcher_count) <= 2 /\ Cardinality(DOMAIN terminal_outcome) <= 2 /\ Cardinality(DOMAIN terminal_buffered) <= 2 /\ Len(completed_order) <= 2 /\ Cardinality(DOMAIN created_at_ms) <= 2 /\ Cardinality(DOMAIN completed_at_ms) <= 2 /\ Len(wait_operation_ids) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []terminal_buffered_only_for_terminal_states
THEOREM Spec => []peer_ready_implies_mob_member_child
THEOREM Spec => []peer_ready_implies_present
THEOREM Spec => []present_operations_keep_kind_identity
THEOREM Spec => []terminal_statuses_have_matching_terminal_outcome
THEOREM Spec => []nonterminal_statuses_have_no_terminal_outcome

=============================================================================
