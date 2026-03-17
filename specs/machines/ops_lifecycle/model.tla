---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for OpsLifecycleMachine.

CONSTANTS OperationIdValues, OperationKindValues

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

VARIABLES phase, model_step_count, known_operations, operation_status, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, terminal_buffered

vars == << phase, model_step_count, known_operations, operation_status, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, terminal_buffered >>

status_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN operation_status THEN operation_status[operation_id] ELSE "None") ELSE "Absent")
kind_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN operation_kind THEN operation_kind[operation_id] ELSE "None") ELSE "None")
peer_ready_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN peer_ready THEN peer_ready[operation_id] ELSE FALSE) ELSE FALSE)
watcher_count_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN watcher_count THEN watcher_count[operation_id] ELSE 0) ELSE 0)
progress_count_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN progress_count THEN progress_count[operation_id] ELSE 0) ELSE 0)
terminal_outcome_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN terminal_outcome THEN terminal_outcome[operation_id] ELSE "None") ELSE "None")
terminal_buffered_of(operation_id) == (IF (operation_id \in known_operations) THEN (IF operation_id \in DOMAIN terminal_buffered THEN terminal_buffered[operation_id] ELSE FALSE) ELSE FALSE)
is_terminal_status(status) == ((status = "Completed") \/ (status = "Failed") \/ (status = "Cancelled") \/ (status = "Retired") \/ (status = "Terminated"))
is_owner_terminatable_status(status) == ((status = "Provisioning") \/ (status = "Running") \/ (status = "Retiring"))
terminal_outcome_matches_status(status, arg_terminal_outcome) == (((status = "Completed") /\ (arg_terminal_outcome = "Completed")) \/ ((status = "Failed") /\ (arg_terminal_outcome = "Failed")) \/ ((status = "Cancelled") /\ (arg_terminal_outcome = "Cancelled")) \/ ((status = "Retired") /\ (arg_terminal_outcome = "Retired")) \/ ((status = "Terminated") /\ (arg_terminal_outcome = "Terminated")))

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

RECURSIVE OwnerTerminated_ForEach0_operation_status(_, _)
OwnerTerminated_ForEach0_operation_status(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF is_owner_terminatable_status(status_of(operation_id)) THEN MapSet(acc, operation_id, "Terminated") ELSE acc IN OwnerTerminated_ForEach0_operation_status(next_acc, remaining \ {item})

RECURSIVE OwnerTerminated_ForEach0_terminal_buffered(_, _)
OwnerTerminated_ForEach0_terminal_buffered(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF is_owner_terminatable_status(status_of(operation_id)) THEN MapSet(acc, operation_id, TRUE) ELSE acc IN OwnerTerminated_ForEach0_terminal_buffered(next_acc, remaining \ {item})

RECURSIVE OwnerTerminated_ForEach0_terminal_outcome(_, _)
OwnerTerminated_ForEach0_terminal_outcome(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF is_owner_terminatable_status(status_of(operation_id)) THEN MapSet(acc, operation_id, "Terminated") ELSE acc IN OwnerTerminated_ForEach0_terminal_outcome(next_acc, remaining \ {item})

RegisterOperation(operation_id, arg_operation_kind) ==
    /\ phase = "Active"
    /\ (status_of(operation_id) = "Absent")
    /\ (arg_operation_kind # "None")
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


ProvisioningSucceeded(operation_id) ==
    /\ phase = "Active"
    /\ (status_of(operation_id) = "Provisioning")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Running")
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, terminal_buffered >>


ProvisioningFailed(operation_id) ==
    /\ phase = "Active"
    /\ (status_of(operation_id) = "Provisioning")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Failed")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Failed")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count >>


PeerReady(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ (kind_of(operation_id) = "MobMemberChild")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ready' = MapSet(peer_ready, operation_id, TRUE)
    /\ UNCHANGED << known_operations, operation_status, operation_kind, progress_count, watcher_count, terminal_outcome, terminal_buffered >>


RegisterWatcher(operation_id) ==
    /\ phase = "Active"
    /\ (status_of(operation_id) # "Absent")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ watcher_count' = MapSet(watcher_count, operation_id, (watcher_count_of(operation_id) + 1))
    /\ UNCHANGED << known_operations, operation_status, operation_kind, peer_ready, progress_count, terminal_outcome, terminal_buffered >>


ProgressReported(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ progress_count' = MapSet(progress_count, operation_id, (progress_count_of(operation_id) + 1))
    /\ UNCHANGED << known_operations, operation_status, operation_kind, peer_ready, watcher_count, terminal_outcome, terminal_buffered >>


CompleteOperation(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Completed")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Completed")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count >>


FailOperation(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Provisioning") \/ (status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Failed")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Failed")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count >>


CancelOperation(operation_id) ==
    /\ phase = "Active"
    /\ ((status_of(operation_id) = "Provisioning") \/ (status_of(operation_id) = "Running") \/ (status_of(operation_id) = "Retiring"))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Cancelled")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Cancelled")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count >>


RetireRequested(operation_id) ==
    /\ phase = "Active"
    /\ (status_of(operation_id) = "Running")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Retiring")
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome, terminal_buffered >>


RetireCompleted(operation_id) ==
    /\ phase = "Active"
    /\ (status_of(operation_id) = "Retiring")
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = MapSet(operation_status, operation_id, "Retired")
    /\ terminal_outcome' = MapSet(terminal_outcome, operation_id, "Retired")
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, TRUE)
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count >>


CollectTerminal(operation_id) ==
    /\ phase = "Active"
    /\ is_terminal_status(status_of(operation_id))
    /\ terminal_buffered_of(operation_id)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_buffered' = MapSet(terminal_buffered, operation_id, FALSE)
    /\ UNCHANGED << known_operations, operation_status, operation_kind, peer_ready, progress_count, watcher_count, terminal_outcome >>


OwnerTerminated ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ operation_status' = OwnerTerminated_ForEach0_operation_status(operation_status, known_operations)
    /\ terminal_outcome' = OwnerTerminated_ForEach0_terminal_outcome(terminal_outcome, known_operations)
    /\ terminal_buffered' = OwnerTerminated_ForEach0_terminal_buffered(terminal_buffered, known_operations)
    /\ UNCHANGED << known_operations, operation_kind, peer_ready, progress_count, watcher_count >>


Next ==
    \/ \E operation_id \in OperationIdValues : \E arg_operation_kind \in OperationKindValues : RegisterOperation(operation_id, arg_operation_kind)
    \/ \E operation_id \in OperationIdValues : ProvisioningSucceeded(operation_id)
    \/ \E operation_id \in OperationIdValues : ProvisioningFailed(operation_id)
    \/ \E operation_id \in OperationIdValues : PeerReady(operation_id)
    \/ \E operation_id \in OperationIdValues : RegisterWatcher(operation_id)
    \/ \E operation_id \in OperationIdValues : ProgressReported(operation_id)
    \/ \E operation_id \in OperationIdValues : CompleteOperation(operation_id)
    \/ \E operation_id \in OperationIdValues : FailOperation(operation_id)
    \/ \E operation_id \in OperationIdValues : CancelOperation(operation_id)
    \/ \E operation_id \in OperationIdValues : RetireRequested(operation_id)
    \/ \E operation_id \in OperationIdValues : RetireCompleted(operation_id)
    \/ \E operation_id \in OperationIdValues : CollectTerminal(operation_id)
    \/ OwnerTerminated

terminal_buffered_only_for_terminal_states == (\A operation_id \in known_operations : (~(terminal_buffered_of(operation_id)) \/ is_terminal_status(status_of(operation_id))))
peer_ready_implies_mob_member_child == (\A operation_id \in known_operations : (~(peer_ready_of(operation_id)) \/ (kind_of(operation_id) = "MobMemberChild")))
peer_ready_implies_present == (\A operation_id \in known_operations : (~(peer_ready_of(operation_id)) \/ (status_of(operation_id) # "Absent")))
present_operations_keep_kind_identity == (\A operation_id \in known_operations : ((status_of(operation_id) # "Absent") /\ (kind_of(operation_id) # "None")))
terminal_statuses_have_matching_terminal_outcome == (\A operation_id \in known_operations : (~(is_terminal_status(status_of(operation_id))) \/ terminal_outcome_matches_status(status_of(operation_id), terminal_outcome_of(operation_id))))
nonterminal_statuses_have_no_terminal_outcome == (\A operation_id \in known_operations : (is_terminal_status(status_of(operation_id)) \/ (terminal_outcome_of(operation_id) = "None")))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(known_operations) <= 1 /\ Cardinality(DOMAIN operation_status) <= 1 /\ Cardinality(DOMAIN operation_kind) <= 1 /\ Cardinality(DOMAIN peer_ready) <= 1 /\ Cardinality(DOMAIN progress_count) <= 1 /\ Cardinality(DOMAIN watcher_count) <= 1 /\ Cardinality(DOMAIN terminal_outcome) <= 1 /\ Cardinality(DOMAIN terminal_buffered) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(known_operations) <= 2 /\ Cardinality(DOMAIN operation_status) <= 2 /\ Cardinality(DOMAIN operation_kind) <= 2 /\ Cardinality(DOMAIN peer_ready) <= 2 /\ Cardinality(DOMAIN progress_count) <= 2 /\ Cardinality(DOMAIN watcher_count) <= 2 /\ Cardinality(DOMAIN terminal_outcome) <= 2 /\ Cardinality(DOMAIN terminal_buffered) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []terminal_buffered_only_for_terminal_states
THEOREM Spec => []peer_ready_implies_mob_member_child
THEOREM Spec => []peer_ready_implies_present
THEOREM Spec => []present_operations_keep_kind_identity
THEOREM Spec => []terminal_statuses_have_matching_terminal_outcome
THEOREM Spec => []nonterminal_statuses_have_no_terminal_outcome

=============================================================================
