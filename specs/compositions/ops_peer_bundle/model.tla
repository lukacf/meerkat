---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for ops_peer_bundle.

CONSTANTS ContentShapeValues, OperationIdValues, OperationKindValues, PeerIdValues, RawItemIdValues, RawPeerKindValues, RequestIdValues, ReservationKeyValues, StringValues, WaitRequestIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

SeqOfOperationIdValues == {<<>>} \cup {<<x>> : x \in OperationIdValues} \cup {<<x, y>> : x \in OperationIdValues, y \in OperationIdValues}
OptionRequestIdValues == {None} \cup {Some(x) : x \in RequestIdValues}
OptionReservationKeyValues == {None} \cup {Some(x) : x \in ReservationKeyValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN Tail(seq) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))
AppendIfMissing(seq, value) == IF value \in SeqElements(seq) THEN seq ELSE Append(seq, value)
Machines == {
    <<"ops_lifecycle", "OpsLifecycleMachine", "ops_plane">>,
    <<"peer_comms", "PeerCommsMachine", "peer_plane">>
}

RouteNames == {
    "ops_peer_ready_trusts_peer_comms"
}

Actors == {
    "ops_plane",
    "peer_plane"
}

ActorPriorities == {
}

SchedulerRules == {
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "ops_lifecycle" -> "ops_plane"
      [] machine_id = "peer_comms" -> "peer_plane"
      [] OTHER -> "unknown_actor"

RouteSource(route_name) ==
    CASE route_name = "ops_peer_ready_trusts_peer_comms" -> "ops_lifecycle"
      [] OTHER -> "unknown_machine"

RouteEffect(route_name) ==
    CASE route_name = "ops_peer_ready_trusts_peer_comms" -> "ExposeOperationPeer"
      [] OTHER -> "unknown_effect"

RouteTargetMachine(route_name) ==
    CASE route_name = "ops_peer_ready_trusts_peer_comms" -> "peer_comms"
      [] OTHER -> "unknown_machine"

RouteTargetInput(route_name) ==
    CASE route_name = "ops_peer_ready_trusts_peer_comms" -> "TrustPeer"
      [] OTHER -> "unknown_input"

RouteDeliveryKind(route_name) ==
    CASE route_name = "ops_peer_ready_trusts_peer_comms" -> "Immediate"
      [] OTHER -> "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, obligation_ops_barrier_satisfaction, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, obligation_ops_barrier_satisfaction, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ ops_lifecycle_phase = "Active"
    /\ ops_lifecycle_known_operations = {}
    /\ ops_lifecycle_operation_status = [x \in {} |-> None]
    /\ ops_lifecycle_operation_kind = [x \in {} |-> None]
    /\ ops_lifecycle_peer_ready = [x \in {} |-> None]
    /\ ops_lifecycle_progress_count = [x \in {} |-> None]
    /\ ops_lifecycle_watcher_count = [x \in {} |-> None]
    /\ ops_lifecycle_terminal_outcome = [x \in {} |-> None]
    /\ ops_lifecycle_terminal_buffered = [x \in {} |-> None]
    /\ ops_lifecycle_completed_order = <<>>
    /\ ops_lifecycle_max_completed = 256
    /\ ops_lifecycle_max_concurrent = 0
    /\ ops_lifecycle_active_count = 0
    /\ ops_lifecycle_created_at_ms = [x \in {} |-> None]
    /\ ops_lifecycle_completed_at_ms = [x \in {} |-> None]
    /\ ops_lifecycle_wait_active = FALSE
    /\ ops_lifecycle_wait_request_id = "wait_request_none"
    /\ ops_lifecycle_wait_operation_ids = <<>>
    /\ peer_comms_phase = "Absent"
    /\ peer_comms_trusted_peers = {}
    /\ peer_comms_raw_item_peer = [x \in {} |-> None]
    /\ peer_comms_raw_item_kind = [x \in {} |-> None]
    /\ peer_comms_classified_as = [x \in {} |-> None]
    /\ peer_comms_text_projection = [x \in {} |-> None]
    /\ peer_comms_content_shape = [x \in {} |-> None]
    /\ peer_comms_request_id = [x \in {} |-> None]
    /\ peer_comms_reservation_key = [x \in {} |-> None]
    /\ peer_comms_trusted_snapshot = [x \in {} |-> None]
    /\ peer_comms_submission_queue = <<>>
    /\ obligation_ops_barrier_satisfaction = {}
    /\ model_step_count = 0
    /\ pending_routes = <<>>
    /\ delivered_routes = {}
    /\ emitted_effects = {}
    /\ observed_transitions = {}

Init ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

WitnessInit_ops_peer_ready_trusts_peer_comms_path ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "ops_lifecycle", variant |-> "RegisterOperation", payload |-> [operation_id |-> "op_1", operation_kind |-> "MobMemberChild"], source_kind |-> "entry", source_route |-> "witness:ops_peer_ready_trusts_peer_comms_path:1", source_machine |-> "external_entry", source_effect |-> "RegisterOperation", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "ops_lifecycle", variant |-> "RegisterOperation", payload |-> [operation_id |-> "op_1", operation_kind |-> "MobMemberChild"], source_kind |-> "entry", source_route |-> "witness:ops_peer_ready_trusts_peer_comms_path:1", source_machine |-> "external_entry", source_effect |-> "RegisterOperation", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "ops_lifecycle", variant |-> "RegisterOperation", payload |-> [operation_id |-> "op_1", operation_kind |-> "MobMemberChild"], source_kind |-> "entry", source_route |-> "witness:ops_peer_ready_trusts_peer_comms_path:1", source_machine |-> "external_entry", source_effect |-> "RegisterOperation", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "ops_lifecycle", variant |-> "ProvisioningSucceeded", payload |-> [operation_id |-> "op_1"], source_kind |-> "entry", source_route |-> "witness:ops_peer_ready_trusts_peer_comms_path:2", source_machine |-> "external_entry", source_effect |-> "ProvisioningSucceeded", effect_id |-> 0], [machine |-> "ops_lifecycle", variant |-> "PeerReady", payload |-> [operation_id |-> "op_1"], source_kind |-> "entry", source_route |-> "witness:ops_peer_ready_trusts_peer_comms_path:3", source_machine |-> "external_entry", source_effect |-> "PeerReady", effect_id |-> 0]>>

ops_lifecycle__terminal_outcome_matches_status(status, arg_terminal_outcome) == (((status = "Completed") /\ (arg_terminal_outcome = "Completed")) \/ ((status = "Failed") /\ (arg_terminal_outcome = "Failed")) \/ ((status = "Cancelled") /\ (arg_terminal_outcome = "Cancelled")) \/ ((status = "Retired") /\ (arg_terminal_outcome = "Retired")) \/ ((status = "Terminated") /\ (arg_terminal_outcome = "Terminated")))

ops_lifecycle__is_owner_terminatable_status(status) == ((status = "Provisioning") \/ (status = "Running") \/ (status = "Retiring"))

ops_lifecycle__is_terminal_status(status) == ((status = "Completed") \/ (status = "Failed") \/ (status = "Cancelled") \/ (status = "Retired") \/ (status = "Terminated"))

ops_lifecycle__terminal_buffered_of(operation_id) == (IF (operation_id \in ops_lifecycle_known_operations) THEN (IF operation_id \in DOMAIN ops_lifecycle_terminal_buffered THEN ops_lifecycle_terminal_buffered[operation_id] ELSE FALSE) ELSE FALSE)

ops_lifecycle__terminal_outcome_of(operation_id) == (IF (operation_id \in ops_lifecycle_known_operations) THEN (IF operation_id \in DOMAIN ops_lifecycle_terminal_outcome THEN ops_lifecycle_terminal_outcome[operation_id] ELSE "None") ELSE "None")

ops_lifecycle__progress_count_of(operation_id) == (IF (operation_id \in ops_lifecycle_known_operations) THEN (IF operation_id \in DOMAIN ops_lifecycle_progress_count THEN ops_lifecycle_progress_count[operation_id] ELSE 0) ELSE 0)

ops_lifecycle__watcher_count_of(operation_id) == (IF (operation_id \in ops_lifecycle_known_operations) THEN (IF operation_id \in DOMAIN ops_lifecycle_watcher_count THEN ops_lifecycle_watcher_count[operation_id] ELSE 0) ELSE 0)

ops_lifecycle__peer_ready_of(operation_id) == (IF (operation_id \in ops_lifecycle_known_operations) THEN (IF operation_id \in DOMAIN ops_lifecycle_peer_ready THEN ops_lifecycle_peer_ready[operation_id] ELSE FALSE) ELSE FALSE)

ops_lifecycle__wait_tracks_operation(operation_id) == (operation_id \in SeqElements(ops_lifecycle_wait_operation_ids))

ops_lifecycle__all_operations_known(operation_ids) == (\A operation_id \in SeqElements(operation_ids) : (operation_id \in ops_lifecycle_known_operations))

ops_lifecycle__wait_is_active == ops_lifecycle_wait_active

ops_lifecycle__kind_of(operation_id) == (IF (operation_id \in ops_lifecycle_known_operations) THEN (IF operation_id \in DOMAIN ops_lifecycle_operation_kind THEN ops_lifecycle_operation_kind[operation_id] ELSE "None") ELSE "None")

ops_lifecycle__status_of(operation_id) == (IF (operation_id \in ops_lifecycle_known_operations) THEN (IF operation_id \in DOMAIN ops_lifecycle_operation_status THEN ops_lifecycle_operation_status[operation_id] ELSE "None") ELSE "Absent")

ops_lifecycle__wait_completes_on_terminal(operation_id) == (ops_lifecycle__wait_is_active /\ ops_lifecycle__wait_tracks_operation(operation_id) /\ (\A tracked_operation_id \in SeqElements(ops_lifecycle_wait_operation_ids) : ((tracked_operation_id = operation_id) \/ ops_lifecycle__is_terminal_status(ops_lifecycle__status_of(tracked_operation_id)))))

ops_lifecycle__all_operations_terminal(operation_ids) == (\A operation_id \in SeqElements(operation_ids) : ops_lifecycle__is_terminal_status(ops_lifecycle__status_of(operation_id)))

RECURSIVE ops_lifecycle_OwnerTerminatedCompletesWait_ForEach0_operation_status(_, _)
ops_lifecycle_OwnerTerminatedCompletesWait_ForEach0_operation_status(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF ops_lifecycle__is_owner_terminatable_status(ops_lifecycle__status_of(operation_id)) THEN MapSet(acc, operation_id, "Terminated") ELSE acc IN ops_lifecycle_OwnerTerminatedCompletesWait_ForEach0_operation_status(next_acc, remaining \ {item})

RECURSIVE ops_lifecycle_OwnerTerminatedCompletesWait_ForEach0_terminal_buffered(_, _)
ops_lifecycle_OwnerTerminatedCompletesWait_ForEach0_terminal_buffered(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF ops_lifecycle__is_owner_terminatable_status(ops_lifecycle__status_of(operation_id)) THEN MapSet(acc, operation_id, TRUE) ELSE acc IN ops_lifecycle_OwnerTerminatedCompletesWait_ForEach0_terminal_buffered(next_acc, remaining \ {item})

RECURSIVE ops_lifecycle_OwnerTerminatedCompletesWait_ForEach0_terminal_outcome(_, _)
ops_lifecycle_OwnerTerminatedCompletesWait_ForEach0_terminal_outcome(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF ops_lifecycle__is_owner_terminatable_status(ops_lifecycle__status_of(operation_id)) THEN MapSet(acc, operation_id, "Terminated") ELSE acc IN ops_lifecycle_OwnerTerminatedCompletesWait_ForEach0_terminal_outcome(next_acc, remaining \ {item})

RECURSIVE ops_lifecycle_OwnerTerminated_ForEach1_operation_status(_, _)
ops_lifecycle_OwnerTerminated_ForEach1_operation_status(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF ops_lifecycle__is_owner_terminatable_status(ops_lifecycle__status_of(operation_id)) THEN MapSet(acc, operation_id, "Terminated") ELSE acc IN ops_lifecycle_OwnerTerminated_ForEach1_operation_status(next_acc, remaining \ {item})

RECURSIVE ops_lifecycle_OwnerTerminated_ForEach1_terminal_buffered(_, _)
ops_lifecycle_OwnerTerminated_ForEach1_terminal_buffered(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF ops_lifecycle__is_owner_terminatable_status(ops_lifecycle__status_of(operation_id)) THEN MapSet(acc, operation_id, TRUE) ELSE acc IN ops_lifecycle_OwnerTerminated_ForEach1_terminal_buffered(next_acc, remaining \ {item})

RECURSIVE ops_lifecycle_OwnerTerminated_ForEach1_terminal_outcome(_, _)
ops_lifecycle_OwnerTerminated_ForEach1_terminal_outcome(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET operation_id == item IN LET next_acc == IF ops_lifecycle__is_owner_terminatable_status(ops_lifecycle__status_of(operation_id)) THEN MapSet(acc, operation_id, "Terminated") ELSE acc IN ops_lifecycle_OwnerTerminated_ForEach1_terminal_outcome(next_acc, remaining \ {item})

ops_lifecycle_RegisterOperation(arg_operation_id, arg_operation_kind) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "RegisterOperation"
       /\ packet.payload.operation_id = arg_operation_id
       /\ packet.payload.operation_kind = arg_operation_kind
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ (ops_lifecycle__status_of(packet.payload.operation_id) = "Absent")
       /\ (packet.payload.operation_kind # "None")
       /\ ((ops_lifecycle_max_concurrent = 0) \/ (ops_lifecycle_active_count < ops_lifecycle_max_concurrent))
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_known_operations' = (ops_lifecycle_known_operations \cup {packet.payload.operation_id})
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Provisioning")
       /\ ops_lifecycle_operation_kind' = MapSet(ops_lifecycle_operation_kind, packet.payload.operation_id, packet.payload.operation_kind)
       /\ ops_lifecycle_peer_ready' = MapSet(ops_lifecycle_peer_ready, packet.payload.operation_id, FALSE)
       /\ ops_lifecycle_progress_count' = MapSet(ops_lifecycle_progress_count, packet.payload.operation_id, 0)
       /\ ops_lifecycle_watcher_count' = MapSet(ops_lifecycle_watcher_count, packet.payload.operation_id, 0)
       /\ ops_lifecycle_terminal_outcome' = MapSet(ops_lifecycle_terminal_outcome, packet.payload.operation_id, "None")
       /\ ops_lifecycle_terminal_buffered' = MapSet(ops_lifecycle_terminal_buffered, packet.payload.operation_id, FALSE)
       /\ ops_lifecycle_active_count' = (ops_lifecycle_active_count) + 1
       /\ ops_lifecycle_created_at_ms' = MapSet(ops_lifecycle_created_at_ms, packet.payload.operation_id, 0)
       /\ ops_lifecycle_completed_at_ms' = MapSet(ops_lifecycle_completed_at_ms, packet.payload.operation_id, 0)
       /\ UNCHANGED << ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "RegisterOperation", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_ProvisioningSucceeded(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "ProvisioningSucceeded"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ (ops_lifecycle__status_of(packet.payload.operation_id) = "Provisioning")
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Running")
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "SubmitOpEvent", payload |-> [event_kind |-> "Started", operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "ProvisioningSucceeded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "ProvisioningSucceeded", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_ProvisioningFailedCompletesWait(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "ProvisioningFailed"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ (ops_lifecycle__status_of(packet.payload.operation_id) = "Provisioning")
       /\ ops_lifecycle__wait_completes_on_terminal(packet.payload.operation_id)
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Failed")
       /\ ops_lifecycle_terminal_outcome' = MapSet(ops_lifecycle_terminal_outcome, packet.payload.operation_id, "Failed")
       /\ ops_lifecycle_terminal_buffered' = MapSet(ops_lifecycle_terminal_buffered, packet.payload.operation_id, TRUE)
       /\ ops_lifecycle_completed_order' = Append(ops_lifecycle_completed_order, packet.payload.operation_id)
       /\ ops_lifecycle_active_count' = (ops_lifecycle_active_count) - 1
       /\ ops_lifecycle_wait_active' = FALSE
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "SubmitOpEvent", payload |-> [event_kind |-> "Failed", operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "ProvisioningFailedCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "NotifyOpWatcher", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "ProvisioningFailedCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "RetainTerminalRecord", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "ProvisioningFailedCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "WaitAllSatisfied", payload |-> [operation_ids |-> ops_lifecycle_wait_operation_ids, wait_request_id |-> ops_lifecycle_wait_request_id], effect_id |-> (model_step_count + 1), source_transition |-> "ProvisioningFailedCompletesWait"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "ProvisioningFailedCompletesWait", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ obligation_ops_barrier_satisfaction' = obligation_ops_barrier_satisfaction \cup {[wait_request_id |-> ops_lifecycle_wait_request_id, operation_ids |-> ops_lifecycle_wait_operation_ids]}
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_ProvisioningFailed(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "ProvisioningFailed"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ (ops_lifecycle__status_of(packet.payload.operation_id) = "Provisioning")
       /\ ~(ops_lifecycle__wait_completes_on_terminal(packet.payload.operation_id))
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Failed")
       /\ ops_lifecycle_terminal_outcome' = MapSet(ops_lifecycle_terminal_outcome, packet.payload.operation_id, "Failed")
       /\ ops_lifecycle_terminal_buffered' = MapSet(ops_lifecycle_terminal_buffered, packet.payload.operation_id, TRUE)
       /\ ops_lifecycle_completed_order' = Append(ops_lifecycle_completed_order, packet.payload.operation_id)
       /\ ops_lifecycle_active_count' = (ops_lifecycle_active_count) - 1
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "SubmitOpEvent", payload |-> [event_kind |-> "Failed", operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "ProvisioningFailed"], [machine |-> "ops_lifecycle", variant |-> "NotifyOpWatcher", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "ProvisioningFailed"], [machine |-> "ops_lifecycle", variant |-> "RetainTerminalRecord", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "ProvisioningFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "ProvisioningFailed", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_PeerReady(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "PeerReady"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ((ops_lifecycle__status_of(packet.payload.operation_id) = "Running") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Retiring"))
       /\ (ops_lifecycle__kind_of(packet.payload.operation_id) = "MobMemberChild")
       /\ (ops_lifecycle__peer_ready_of(packet.payload.operation_id) = FALSE)
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_peer_ready' = MapSet(ops_lifecycle_peer_ready, packet.payload.operation_id, TRUE)
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> packet.payload.operation_id], source_kind |-> "route", source_route |-> "ops_peer_ready_trusts_peer_comms", source_machine |-> "ops_lifecycle", source_effect |-> "ExposeOperationPeer", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> packet.payload.operation_id], source_kind |-> "route", source_route |-> "ops_peer_ready_trusts_peer_comms", source_machine |-> "ops_lifecycle", source_effect |-> "ExposeOperationPeer", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "ops_peer_ready_trusts_peer_comms", source_machine |-> "ops_lifecycle", effect |-> "ExposeOperationPeer", target_machine |-> "peer_comms", target_input |-> "TrustPeer", payload |-> [peer_id |-> packet.payload.operation_id], actor |-> "peer_plane", effect_id |-> (model_step_count + 1), source_transition |-> "PeerReady"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "ExposeOperationPeer", payload |-> [operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "PeerReady"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "PeerReady", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_RegisterWatcher(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "RegisterWatcher"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ (ops_lifecycle__status_of(packet.payload.operation_id) # "Absent")
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_watcher_count' = MapSet(ops_lifecycle_watcher_count, packet.payload.operation_id, (ops_lifecycle__watcher_count_of(packet.payload.operation_id) + 1))
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "RegisterWatcher", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_ProgressReported(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "ProgressReported"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ((ops_lifecycle__status_of(packet.payload.operation_id) = "Running") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Retiring"))
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_progress_count' = MapSet(ops_lifecycle_progress_count, packet.payload.operation_id, (ops_lifecycle__progress_count_of(packet.payload.operation_id) + 1))
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "SubmitOpEvent", payload |-> [event_kind |-> "Progress", operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "ProgressReported"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "ProgressReported", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_CompleteOperationCompletesWait(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "CompleteOperation"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ((ops_lifecycle__status_of(packet.payload.operation_id) = "Running") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Retiring"))
       /\ ops_lifecycle__wait_completes_on_terminal(packet.payload.operation_id)
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Completed")
       /\ ops_lifecycle_terminal_outcome' = MapSet(ops_lifecycle_terminal_outcome, packet.payload.operation_id, "Completed")
       /\ ops_lifecycle_terminal_buffered' = MapSet(ops_lifecycle_terminal_buffered, packet.payload.operation_id, TRUE)
       /\ ops_lifecycle_completed_order' = Append(ops_lifecycle_completed_order, packet.payload.operation_id)
       /\ ops_lifecycle_active_count' = (ops_lifecycle_active_count) - 1
       /\ ops_lifecycle_wait_active' = FALSE
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "SubmitOpEvent", payload |-> [event_kind |-> "Completed", operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteOperationCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "NotifyOpWatcher", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Completed"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteOperationCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "RetainTerminalRecord", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Completed"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteOperationCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "WaitAllSatisfied", payload |-> [operation_ids |-> ops_lifecycle_wait_operation_ids, wait_request_id |-> ops_lifecycle_wait_request_id], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteOperationCompletesWait"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "CompleteOperationCompletesWait", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ obligation_ops_barrier_satisfaction' = obligation_ops_barrier_satisfaction \cup {[wait_request_id |-> ops_lifecycle_wait_request_id, operation_ids |-> ops_lifecycle_wait_operation_ids]}
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_CompleteOperation(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "CompleteOperation"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ((ops_lifecycle__status_of(packet.payload.operation_id) = "Running") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Retiring"))
       /\ ~(ops_lifecycle__wait_completes_on_terminal(packet.payload.operation_id))
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Completed")
       /\ ops_lifecycle_terminal_outcome' = MapSet(ops_lifecycle_terminal_outcome, packet.payload.operation_id, "Completed")
       /\ ops_lifecycle_terminal_buffered' = MapSet(ops_lifecycle_terminal_buffered, packet.payload.operation_id, TRUE)
       /\ ops_lifecycle_completed_order' = Append(ops_lifecycle_completed_order, packet.payload.operation_id)
       /\ ops_lifecycle_active_count' = (ops_lifecycle_active_count) - 1
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "SubmitOpEvent", payload |-> [event_kind |-> "Completed", operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteOperation"], [machine |-> "ops_lifecycle", variant |-> "NotifyOpWatcher", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Completed"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteOperation"], [machine |-> "ops_lifecycle", variant |-> "RetainTerminalRecord", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Completed"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteOperation"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "CompleteOperation", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_FailOperationCompletesWait(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "FailOperation"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ((ops_lifecycle__status_of(packet.payload.operation_id) = "Provisioning") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Running") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Retiring"))
       /\ ops_lifecycle__wait_completes_on_terminal(packet.payload.operation_id)
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Failed")
       /\ ops_lifecycle_terminal_outcome' = MapSet(ops_lifecycle_terminal_outcome, packet.payload.operation_id, "Failed")
       /\ ops_lifecycle_terminal_buffered' = MapSet(ops_lifecycle_terminal_buffered, packet.payload.operation_id, TRUE)
       /\ ops_lifecycle_completed_order' = Append(ops_lifecycle_completed_order, packet.payload.operation_id)
       /\ ops_lifecycle_active_count' = (ops_lifecycle_active_count) - 1
       /\ ops_lifecycle_wait_active' = FALSE
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "SubmitOpEvent", payload |-> [event_kind |-> "Failed", operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "FailOperationCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "NotifyOpWatcher", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "FailOperationCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "RetainTerminalRecord", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "FailOperationCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "WaitAllSatisfied", payload |-> [operation_ids |-> ops_lifecycle_wait_operation_ids, wait_request_id |-> ops_lifecycle_wait_request_id], effect_id |-> (model_step_count + 1), source_transition |-> "FailOperationCompletesWait"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "FailOperationCompletesWait", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ obligation_ops_barrier_satisfaction' = obligation_ops_barrier_satisfaction \cup {[wait_request_id |-> ops_lifecycle_wait_request_id, operation_ids |-> ops_lifecycle_wait_operation_ids]}
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_FailOperation(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "FailOperation"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ((ops_lifecycle__status_of(packet.payload.operation_id) = "Provisioning") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Running") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Retiring"))
       /\ ~(ops_lifecycle__wait_completes_on_terminal(packet.payload.operation_id))
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Failed")
       /\ ops_lifecycle_terminal_outcome' = MapSet(ops_lifecycle_terminal_outcome, packet.payload.operation_id, "Failed")
       /\ ops_lifecycle_terminal_buffered' = MapSet(ops_lifecycle_terminal_buffered, packet.payload.operation_id, TRUE)
       /\ ops_lifecycle_completed_order' = Append(ops_lifecycle_completed_order, packet.payload.operation_id)
       /\ ops_lifecycle_active_count' = (ops_lifecycle_active_count) - 1
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "SubmitOpEvent", payload |-> [event_kind |-> "Failed", operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "FailOperation"], [machine |-> "ops_lifecycle", variant |-> "NotifyOpWatcher", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "FailOperation"], [machine |-> "ops_lifecycle", variant |-> "RetainTerminalRecord", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "FailOperation"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "FailOperation", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_CancelOperationCompletesWait(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "CancelOperation"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ((ops_lifecycle__status_of(packet.payload.operation_id) = "Provisioning") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Running") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Retiring"))
       /\ ops_lifecycle__wait_completes_on_terminal(packet.payload.operation_id)
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Cancelled")
       /\ ops_lifecycle_terminal_outcome' = MapSet(ops_lifecycle_terminal_outcome, packet.payload.operation_id, "Cancelled")
       /\ ops_lifecycle_terminal_buffered' = MapSet(ops_lifecycle_terminal_buffered, packet.payload.operation_id, TRUE)
       /\ ops_lifecycle_completed_order' = Append(ops_lifecycle_completed_order, packet.payload.operation_id)
       /\ ops_lifecycle_active_count' = (ops_lifecycle_active_count) - 1
       /\ ops_lifecycle_wait_active' = FALSE
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "SubmitOpEvent", payload |-> [event_kind |-> "Cancelled", operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "CancelOperationCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "NotifyOpWatcher", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Cancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelOperationCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "RetainTerminalRecord", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Cancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelOperationCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "WaitAllSatisfied", payload |-> [operation_ids |-> ops_lifecycle_wait_operation_ids, wait_request_id |-> ops_lifecycle_wait_request_id], effect_id |-> (model_step_count + 1), source_transition |-> "CancelOperationCompletesWait"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "CancelOperationCompletesWait", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ obligation_ops_barrier_satisfaction' = obligation_ops_barrier_satisfaction \cup {[wait_request_id |-> ops_lifecycle_wait_request_id, operation_ids |-> ops_lifecycle_wait_operation_ids]}
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_CancelOperation(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "CancelOperation"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ((ops_lifecycle__status_of(packet.payload.operation_id) = "Provisioning") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Running") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Retiring"))
       /\ ~(ops_lifecycle__wait_completes_on_terminal(packet.payload.operation_id))
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Cancelled")
       /\ ops_lifecycle_terminal_outcome' = MapSet(ops_lifecycle_terminal_outcome, packet.payload.operation_id, "Cancelled")
       /\ ops_lifecycle_terminal_buffered' = MapSet(ops_lifecycle_terminal_buffered, packet.payload.operation_id, TRUE)
       /\ ops_lifecycle_completed_order' = Append(ops_lifecycle_completed_order, packet.payload.operation_id)
       /\ ops_lifecycle_active_count' = (ops_lifecycle_active_count) - 1
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "SubmitOpEvent", payload |-> [event_kind |-> "Cancelled", operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "CancelOperation"], [machine |-> "ops_lifecycle", variant |-> "NotifyOpWatcher", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Cancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelOperation"], [machine |-> "ops_lifecycle", variant |-> "RetainTerminalRecord", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Cancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelOperation"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "CancelOperation", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_RetireRequested(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "RetireRequested"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ (ops_lifecycle__status_of(packet.payload.operation_id) = "Running")
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Retiring")
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "RetireRequested", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_RetireCompletedCompletesWait(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "RetireCompleted"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ((ops_lifecycle__status_of(packet.payload.operation_id) = "Running") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Retiring"))
       /\ ops_lifecycle__wait_completes_on_terminal(packet.payload.operation_id)
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Retired")
       /\ ops_lifecycle_terminal_outcome' = MapSet(ops_lifecycle_terminal_outcome, packet.payload.operation_id, "Retired")
       /\ ops_lifecycle_terminal_buffered' = MapSet(ops_lifecycle_terminal_buffered, packet.payload.operation_id, TRUE)
       /\ ops_lifecycle_completed_order' = Append(ops_lifecycle_completed_order, packet.payload.operation_id)
       /\ ops_lifecycle_active_count' = (ops_lifecycle_active_count) - 1
       /\ ops_lifecycle_wait_active' = FALSE
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "SubmitOpEvent", payload |-> [event_kind |-> "Retired", operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "RetireCompletedCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "NotifyOpWatcher", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Retired"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireCompletedCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "RetainTerminalRecord", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Retired"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireCompletedCompletesWait"], [machine |-> "ops_lifecycle", variant |-> "WaitAllSatisfied", payload |-> [operation_ids |-> ops_lifecycle_wait_operation_ids, wait_request_id |-> ops_lifecycle_wait_request_id], effect_id |-> (model_step_count + 1), source_transition |-> "RetireCompletedCompletesWait"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "RetireCompletedCompletesWait", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ obligation_ops_barrier_satisfaction' = obligation_ops_barrier_satisfaction \cup {[wait_request_id |-> ops_lifecycle_wait_request_id, operation_ids |-> ops_lifecycle_wait_operation_ids]}
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_RetireCompleted(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "RetireCompleted"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ((ops_lifecycle__status_of(packet.payload.operation_id) = "Running") \/ (ops_lifecycle__status_of(packet.payload.operation_id) = "Retiring"))
       /\ ~(ops_lifecycle__wait_completes_on_terminal(packet.payload.operation_id))
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = MapSet(ops_lifecycle_operation_status, packet.payload.operation_id, "Retired")
       /\ ops_lifecycle_terminal_outcome' = MapSet(ops_lifecycle_terminal_outcome, packet.payload.operation_id, "Retired")
       /\ ops_lifecycle_terminal_buffered' = MapSet(ops_lifecycle_terminal_buffered, packet.payload.operation_id, TRUE)
       /\ ops_lifecycle_completed_order' = Append(ops_lifecycle_completed_order, packet.payload.operation_id)
       /\ ops_lifecycle_active_count' = (ops_lifecycle_active_count) - 1
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "SubmitOpEvent", payload |-> [event_kind |-> "Retired", operation_id |-> packet.payload.operation_id], effect_id |-> (model_step_count + 1), source_transition |-> "RetireCompleted"], [machine |-> "ops_lifecycle", variant |-> "NotifyOpWatcher", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Retired"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireCompleted"], [machine |-> "ops_lifecycle", variant |-> "RetainTerminalRecord", payload |-> [operation_id |-> packet.payload.operation_id, terminal_outcome |-> "Retired"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "RetireCompleted", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_CollectTerminal(arg_operation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "CollectTerminal"
       /\ packet.payload.operation_id = arg_operation_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ops_lifecycle__is_terminal_status(ops_lifecycle__status_of(packet.payload.operation_id))
       /\ ops_lifecycle__terminal_buffered_of(packet.payload.operation_id)
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_terminal_buffered' = MapSet(ops_lifecycle_terminal_buffered, packet.payload.operation_id, FALSE)
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "CollectTerminal", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_OwnerTerminatedCompletesWait ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "OwnerTerminated"
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ops_lifecycle__wait_is_active
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = ops_lifecycle_OwnerTerminatedCompletesWait_ForEach0_operation_status(ops_lifecycle_operation_status, ops_lifecycle_known_operations)
       /\ ops_lifecycle_terminal_outcome' = ops_lifecycle_OwnerTerminatedCompletesWait_ForEach0_terminal_outcome(ops_lifecycle_terminal_outcome, ops_lifecycle_known_operations)
       /\ ops_lifecycle_terminal_buffered' = ops_lifecycle_OwnerTerminatedCompletesWait_ForEach0_terminal_buffered(ops_lifecycle_terminal_buffered, ops_lifecycle_known_operations)
       /\ ops_lifecycle_active_count' = 0
       /\ ops_lifecycle_wait_active' = FALSE
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "WaitAllSatisfied", payload |-> [operation_ids |-> ops_lifecycle_wait_operation_ids, wait_request_id |-> ops_lifecycle_wait_request_id], effect_id |-> (model_step_count + 1), source_transition |-> "OwnerTerminatedCompletesWait"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "OwnerTerminatedCompletesWait", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ obligation_ops_barrier_satisfaction' = obligation_ops_barrier_satisfaction \cup {[wait_request_id |-> ops_lifecycle_wait_request_id, operation_ids |-> ops_lifecycle_wait_operation_ids]}
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_OwnerTerminated ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "OwnerTerminated"
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ~(ops_lifecycle__wait_is_active)
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_operation_status' = ops_lifecycle_OwnerTerminated_ForEach1_operation_status(ops_lifecycle_operation_status, ops_lifecycle_known_operations)
       /\ ops_lifecycle_terminal_outcome' = ops_lifecycle_OwnerTerminated_ForEach1_terminal_outcome(ops_lifecycle_terminal_outcome, ops_lifecycle_known_operations)
       /\ ops_lifecycle_terminal_buffered' = ops_lifecycle_OwnerTerminated_ForEach1_terminal_buffered(ops_lifecycle_terminal_buffered, ops_lifecycle_known_operations)
       /\ ops_lifecycle_active_count' = 0
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "OwnerTerminated", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_BeginWaitAllImmediate(arg_wait_request_id, arg_operation_ids) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "BeginWaitAll"
       /\ packet.payload.wait_request_id = arg_wait_request_id
       /\ packet.payload.operation_ids = arg_operation_ids
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ~(ops_lifecycle__wait_is_active)
       /\ ops_lifecycle__all_operations_known(packet.payload.operation_ids)
       /\ ops_lifecycle__all_operations_terminal(packet.payload.operation_ids)
       /\ ops_lifecycle_phase' = "Active"
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "WaitAllSatisfied", payload |-> [operation_ids |-> packet.payload.operation_ids, wait_request_id |-> packet.payload.wait_request_id], effect_id |-> (model_step_count + 1), source_transition |-> "BeginWaitAllImmediate"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "BeginWaitAllImmediate", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ obligation_ops_barrier_satisfaction' = obligation_ops_barrier_satisfaction \cup {[wait_request_id |-> packet.payload.wait_request_id, operation_ids |-> packet.payload.operation_ids]}
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_BeginWaitAllPending(arg_wait_request_id, arg_operation_ids) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "BeginWaitAll"
       /\ packet.payload.wait_request_id = arg_wait_request_id
       /\ packet.payload.operation_ids = arg_operation_ids
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ~(ops_lifecycle__wait_is_active)
       /\ ops_lifecycle__all_operations_known(packet.payload.operation_ids)
       /\ ~(ops_lifecycle__all_operations_terminal(packet.payload.operation_ids))
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_wait_active' = TRUE
       /\ ops_lifecycle_wait_request_id' = packet.payload.wait_request_id
       /\ ops_lifecycle_wait_operation_ids' = packet.payload.operation_ids
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "BeginWaitAllPending", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_CancelWaitAll(arg_wait_request_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "CancelWaitAll"
       /\ packet.payload.wait_request_id = arg_wait_request_id
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ops_lifecycle__wait_is_active
       /\ (ops_lifecycle_wait_request_id = packet.payload.wait_request_id)
       /\ ops_lifecycle_phase' = "Active"
       /\ ops_lifecycle_wait_active' = FALSE
       /\ ops_lifecycle_wait_request_id' = "wait_request_none"
       /\ ops_lifecycle_wait_operation_ids' = <<>>
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "CancelWaitAll", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_CollectCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "ops_lifecycle"
       /\ packet.variant = "CollectCompleted"
       /\ ~HigherPriorityReady("ops_plane")
       /\ ops_lifecycle_phase = "Active"
       /\ ops_lifecycle_phase' = "Active"
       /\ UNCHANGED << ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "ops_lifecycle", variant |-> "CollectCompletedResult", payload |-> [operation_ids |-> ops_lifecycle_completed_order], effect_id |-> (model_step_count + 1), source_transition |-> "CollectCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "ops_lifecycle", transition |-> "CollectCompleted", actor |-> "ops_plane", step |-> (model_step_count + 1), from_phase |-> ops_lifecycle_phase, to_phase |-> "Active"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


ops_lifecycle_terminal_buffered_only_for_terminal_states == (\A operation_id \in ops_lifecycle_known_operations : (~(ops_lifecycle__terminal_buffered_of(operation_id)) \/ ops_lifecycle__is_terminal_status(ops_lifecycle__status_of(operation_id))))
ops_lifecycle_peer_ready_implies_mob_member_child == (\A operation_id \in ops_lifecycle_known_operations : (~(ops_lifecycle__peer_ready_of(operation_id)) \/ (ops_lifecycle__kind_of(operation_id) = "MobMemberChild")))
ops_lifecycle_peer_ready_implies_present == (\A operation_id \in ops_lifecycle_known_operations : (~(ops_lifecycle__peer_ready_of(operation_id)) \/ (ops_lifecycle__status_of(operation_id) # "Absent")))
ops_lifecycle_present_operations_keep_kind_identity == (\A operation_id \in ops_lifecycle_known_operations : ((ops_lifecycle__status_of(operation_id) # "Absent") /\ (ops_lifecycle__kind_of(operation_id) # "None")))
ops_lifecycle_terminal_statuses_have_matching_terminal_outcome == (\A operation_id \in ops_lifecycle_known_operations : (~(ops_lifecycle__is_terminal_status(ops_lifecycle__status_of(operation_id))) \/ ops_lifecycle__terminal_outcome_matches_status(ops_lifecycle__status_of(operation_id), ops_lifecycle__terminal_outcome_of(operation_id))))
ops_lifecycle_nonterminal_statuses_have_no_terminal_outcome == (\A operation_id \in ops_lifecycle_known_operations : (ops_lifecycle__is_terminal_status(ops_lifecycle__status_of(operation_id)) \/ (ops_lifecycle__terminal_outcome_of(operation_id) = "None")))

peer_comms__ClassFor(raw_kind) == (IF (raw_kind = "request") THEN "ActionableRequest" ELSE (IF (raw_kind = "response_terminal") THEN "Response" ELSE (IF (raw_kind = "response_progress") THEN "Response" ELSE (IF (raw_kind = "plain_event") THEN "PlainEvent" ELSE (IF (raw_kind = "silent_request") THEN "SilentRequest" ELSE "ActionableMessage")))))

peer_comms_TrustPeer(arg_peer_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "peer_comms"
       /\ packet.variant = "TrustPeer"
       /\ packet.payload.peer_id = arg_peer_id
       /\ ~HigherPriorityReady("peer_plane")
       /\ peer_comms_phase = "Absent" \/ peer_comms_phase = "Received"
       /\ peer_comms_phase' = "Absent"
       /\ peer_comms_trusted_peers' = (peer_comms_trusted_peers \cup {packet.payload.peer_id})
       /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "TrustPeer", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Absent"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


peer_comms_ReceiveTrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "peer_comms"
       /\ packet.variant = "ReceivePeerEnvelope"
       /\ packet.payload.raw_item_id = arg_raw_item_id
       /\ packet.payload.peer_id = arg_peer_id
       /\ packet.payload.raw_kind = arg_raw_kind
       /\ packet.payload.text_projection = arg_text_projection
       /\ packet.payload.content_shape = arg_content_shape
       /\ packet.payload.request_id = arg_request_id
       /\ packet.payload.reservation_key = arg_reservation_key
       /\ ~HigherPriorityReady("peer_plane")
       /\ peer_comms_phase = "Absent" \/ peer_comms_phase = "Received"
       /\ (packet.payload.peer_id \in peer_comms_trusted_peers)
       /\ peer_comms_phase' = "Received"
       /\ peer_comms_raw_item_peer' = MapSet(peer_comms_raw_item_peer, packet.payload.raw_item_id, packet.payload.peer_id)
       /\ peer_comms_raw_item_kind' = MapSet(peer_comms_raw_item_kind, packet.payload.raw_item_id, packet.payload.raw_kind)
       /\ peer_comms_classified_as' = MapSet(peer_comms_classified_as, packet.payload.raw_item_id, peer_comms__ClassFor(packet.payload.raw_kind))
       /\ peer_comms_text_projection' = MapSet(peer_comms_text_projection, packet.payload.raw_item_id, packet.payload.text_projection)
       /\ peer_comms_content_shape' = MapSet(peer_comms_content_shape, packet.payload.raw_item_id, packet.payload.content_shape)
       /\ peer_comms_request_id' = MapSet(peer_comms_request_id, packet.payload.raw_item_id, packet.payload.request_id)
       /\ peer_comms_reservation_key' = MapSet(peer_comms_reservation_key, packet.payload.raw_item_id, packet.payload.reservation_key)
       /\ peer_comms_trusted_snapshot' = MapSet(peer_comms_trusted_snapshot, packet.payload.raw_item_id, TRUE)
       /\ peer_comms_submission_queue' = Append(peer_comms_submission_queue, packet.payload.raw_item_id)
       /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_trusted_peers, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "ReceiveTrustedPeerEnvelope", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Received"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


peer_comms_DropUntrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "peer_comms"
       /\ packet.variant = "ReceivePeerEnvelope"
       /\ packet.payload.raw_item_id = arg_raw_item_id
       /\ packet.payload.peer_id = arg_peer_id
       /\ packet.payload.raw_kind = arg_raw_kind
       /\ packet.payload.text_projection = arg_text_projection
       /\ packet.payload.content_shape = arg_content_shape
       /\ packet.payload.request_id = arg_request_id
       /\ packet.payload.reservation_key = arg_reservation_key
       /\ ~HigherPriorityReady("peer_plane")
       /\ peer_comms_phase = "Absent" \/ peer_comms_phase = "Received"
       /\ ~((packet.payload.peer_id \in peer_comms_trusted_peers))
       /\ peer_comms_phase' = "Dropped"
       /\ peer_comms_raw_item_peer' = MapSet(peer_comms_raw_item_peer, packet.payload.raw_item_id, packet.payload.peer_id)
       /\ peer_comms_raw_item_kind' = MapSet(peer_comms_raw_item_kind, packet.payload.raw_item_id, packet.payload.raw_kind)
       /\ peer_comms_text_projection' = MapSet(peer_comms_text_projection, packet.payload.raw_item_id, packet.payload.text_projection)
       /\ peer_comms_content_shape' = MapSet(peer_comms_content_shape, packet.payload.raw_item_id, packet.payload.content_shape)
       /\ peer_comms_request_id' = MapSet(peer_comms_request_id, packet.payload.raw_item_id, packet.payload.request_id)
       /\ peer_comms_reservation_key' = MapSet(peer_comms_reservation_key, packet.payload.raw_item_id, packet.payload.reservation_key)
       /\ peer_comms_trusted_snapshot' = MapSet(peer_comms_trusted_snapshot, packet.payload.raw_item_id, FALSE)
       /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_trusted_peers, peer_comms_classified_as, peer_comms_submission_queue, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "DropUntrustedPeerEnvelope", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Dropped"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


peer_comms_SubmitTypedPeerInputDelivered(arg_raw_item_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "peer_comms"
       /\ packet.variant = "SubmitTypedPeerInput"
       /\ packet.payload.raw_item_id = arg_raw_item_id
       /\ ~HigherPriorityReady("peer_plane")
       /\ peer_comms_phase = "Received"
       /\ (packet.payload.raw_item_id \in SeqElements(peer_comms_submission_queue))
       /\ (packet.payload.raw_item_id \in DOMAIN peer_comms_classified_as)
       /\ (Len(peer_comms_submission_queue) = 1)
       /\ peer_comms_phase' = "Delivered"
       /\ peer_comms_submission_queue' = SeqRemove(peer_comms_submission_queue, packet.payload.raw_item_id)
       /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "peer_comms", variant |-> "SubmitPeerInputCandidate", payload |-> [content_shape |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_content_shape THEN peer_comms_content_shape[packet.payload.raw_item_id] ELSE "None"), peer_input_class |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_classified_as THEN peer_comms_classified_as[packet.payload.raw_item_id] ELSE "None"), raw_item_id |-> packet.payload.raw_item_id, request_id |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_request_id THEN peer_comms_request_id[packet.payload.raw_item_id] ELSE None), reservation_key |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_reservation_key THEN peer_comms_reservation_key[packet.payload.raw_item_id] ELSE None), text_projection |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_text_projection THEN peer_comms_text_projection[packet.payload.raw_item_id] ELSE "None")], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitTypedPeerInputDelivered"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "SubmitTypedPeerInputDelivered", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Delivered"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


peer_comms_SubmitTypedPeerInputContinue(arg_raw_item_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "peer_comms"
       /\ packet.variant = "SubmitTypedPeerInput"
       /\ packet.payload.raw_item_id = arg_raw_item_id
       /\ ~HigherPriorityReady("peer_plane")
       /\ peer_comms_phase = "Received"
       /\ (packet.payload.raw_item_id \in SeqElements(peer_comms_submission_queue))
       /\ (packet.payload.raw_item_id \in DOMAIN peer_comms_classified_as)
       /\ (Len(peer_comms_submission_queue) > 1)
       /\ peer_comms_phase' = "Received"
       /\ peer_comms_submission_queue' = SeqRemove(peer_comms_submission_queue, packet.payload.raw_item_id)
       /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "peer_comms", variant |-> "SubmitPeerInputCandidate", payload |-> [content_shape |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_content_shape THEN peer_comms_content_shape[packet.payload.raw_item_id] ELSE "None"), peer_input_class |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_classified_as THEN peer_comms_classified_as[packet.payload.raw_item_id] ELSE "None"), raw_item_id |-> packet.payload.raw_item_id, request_id |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_request_id THEN peer_comms_request_id[packet.payload.raw_item_id] ELSE None), reservation_key |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_reservation_key THEN peer_comms_reservation_key[packet.payload.raw_item_id] ELSE None), text_projection |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_text_projection THEN peer_comms_text_projection[packet.payload.raw_item_id] ELSE "None")], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitTypedPeerInputContinue"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "SubmitTypedPeerInputContinue", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Received"]}
       /\ UNCHANGED << obligation_ops_barrier_satisfaction >>
       /\ model_step_count' = model_step_count + 1


peer_comms_queued_items_are_classified == (\A raw_item_id \in SeqElements(peer_comms_submission_queue) : (raw_item_id \in DOMAIN peer_comms_classified_as))
peer_comms_queued_items_preserve_content_shape == (\A raw_item_id \in SeqElements(peer_comms_submission_queue) : (raw_item_id \in DOMAIN peer_comms_content_shape))
peer_comms_queued_items_preserve_text_projection == (\A raw_item_id \in SeqElements(peer_comms_submission_queue) : (raw_item_id \in DOMAIN peer_comms_text_projection))
peer_comms_queued_items_preserve_correlation_slots == (\A raw_item_id \in SeqElements(peer_comms_submission_queue) : ((raw_item_id \in DOMAIN peer_comms_request_id) /\ (raw_item_id \in DOMAIN peer_comms_reservation_key)))

Inject_register_operation(arg_operation_id, arg_operation_kind) ==
    /\ ~([machine |-> "ops_lifecycle", variant |-> "RegisterOperation", payload |-> [operation_id |-> arg_operation_id, operation_kind |-> arg_operation_kind], source_kind |-> "entry", source_route |-> "register_operation", source_machine |-> "external_entry", source_effect |-> "RegisterOperation", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "ops_lifecycle", variant |-> "RegisterOperation", payload |-> [operation_id |-> arg_operation_id, operation_kind |-> arg_operation_kind], source_kind |-> "entry", source_route |-> "register_operation", source_machine |-> "external_entry", source_effect |-> "RegisterOperation", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "ops_lifecycle", variant |-> "RegisterOperation", payload |-> [operation_id |-> arg_operation_id, operation_kind |-> arg_operation_kind], source_kind |-> "entry", source_route |-> "register_operation", source_machine |-> "external_entry", source_effect |-> "RegisterOperation", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, obligation_ops_barrier_satisfaction, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_provisioning_succeeded(arg_operation_id) ==
    /\ ~([machine |-> "ops_lifecycle", variant |-> "ProvisioningSucceeded", payload |-> [operation_id |-> arg_operation_id], source_kind |-> "entry", source_route |-> "provisioning_succeeded", source_machine |-> "external_entry", source_effect |-> "ProvisioningSucceeded", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "ops_lifecycle", variant |-> "ProvisioningSucceeded", payload |-> [operation_id |-> arg_operation_id], source_kind |-> "entry", source_route |-> "provisioning_succeeded", source_machine |-> "external_entry", source_effect |-> "ProvisioningSucceeded", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "ops_lifecycle", variant |-> "ProvisioningSucceeded", payload |-> [operation_id |-> arg_operation_id], source_kind |-> "entry", source_route |-> "provisioning_succeeded", source_machine |-> "external_entry", source_effect |-> "ProvisioningSucceeded", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, obligation_ops_barrier_satisfaction, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_peer_ready(arg_operation_id) ==
    /\ ~([machine |-> "ops_lifecycle", variant |-> "PeerReady", payload |-> [operation_id |-> arg_operation_id], source_kind |-> "entry", source_route |-> "peer_ready", source_machine |-> "external_entry", source_effect |-> "PeerReady", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "ops_lifecycle", variant |-> "PeerReady", payload |-> [operation_id |-> arg_operation_id], source_kind |-> "entry", source_route |-> "peer_ready", source_machine |-> "external_entry", source_effect |-> "PeerReady", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "ops_lifecycle", variant |-> "PeerReady", payload |-> [operation_id |-> arg_operation_id], source_kind |-> "entry", source_route |-> "peer_ready", source_machine |-> "external_entry", source_effect |-> "PeerReady", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, obligation_ops_barrier_satisfaction, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_complete_operation(arg_operation_id) ==
    /\ ~([machine |-> "ops_lifecycle", variant |-> "CompleteOperation", payload |-> [operation_id |-> arg_operation_id], source_kind |-> "entry", source_route |-> "complete_operation", source_machine |-> "external_entry", source_effect |-> "CompleteOperation", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "ops_lifecycle", variant |-> "CompleteOperation", payload |-> [operation_id |-> arg_operation_id], source_kind |-> "entry", source_route |-> "complete_operation", source_machine |-> "external_entry", source_effect |-> "CompleteOperation", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "ops_lifecycle", variant |-> "CompleteOperation", payload |-> [operation_id |-> arg_operation_id], source_kind |-> "entry", source_route |-> "complete_operation", source_machine |-> "external_entry", source_effect |-> "CompleteOperation", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, obligation_ops_barrier_satisfaction, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_peer_trust(arg_peer_id) ==
    /\ ~([machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> arg_peer_id], source_kind |-> "entry", source_route |-> "peer_trust", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> arg_peer_id], source_kind |-> "entry", source_route |-> "peer_trust", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> arg_peer_id], source_kind |-> "entry", source_route |-> "peer_trust", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, obligation_ops_barrier_satisfaction, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_peer_receive(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key) ==
    /\ ~([machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [content_shape |-> arg_content_shape, peer_id |-> arg_peer_id, raw_item_id |-> arg_raw_item_id, raw_kind |-> arg_raw_kind, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, text_projection |-> arg_text_projection], source_kind |-> "entry", source_route |-> "peer_receive", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [content_shape |-> arg_content_shape, peer_id |-> arg_peer_id, raw_item_id |-> arg_raw_item_id, raw_kind |-> arg_raw_kind, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, text_projection |-> arg_text_projection], source_kind |-> "entry", source_route |-> "peer_receive", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [content_shape |-> arg_content_shape, peer_id |-> arg_peer_id, raw_item_id |-> arg_raw_item_id, raw_kind |-> arg_raw_kind, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, text_projection |-> arg_text_projection], source_kind |-> "entry", source_route |-> "peer_receive", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, obligation_ops_barrier_satisfaction, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, obligation_ops_barrier_satisfaction, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_ops_peer_ready_trusts_peer_comms_path ==
    /\ witness_current_script_input # None
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_inputs) = 0
    /\ Len(pending_routes) = 0
    /\ Len(witness_remaining_script_inputs) > 0
    /\ pending_inputs' = Append(pending_inputs, Head(witness_remaining_script_inputs))
    /\ observed_inputs' = observed_inputs \cup {Head(witness_remaining_script_inputs)}
    /\ witness_current_script_input' = Head(witness_remaining_script_inputs)
    /\ witness_remaining_script_inputs' = Tail(witness_remaining_script_inputs)
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << ops_lifecycle_phase, ops_lifecycle_known_operations, ops_lifecycle_operation_status, ops_lifecycle_operation_kind, ops_lifecycle_peer_ready, ops_lifecycle_progress_count, ops_lifecycle_watcher_count, ops_lifecycle_terminal_outcome, ops_lifecycle_terminal_buffered, ops_lifecycle_completed_order, ops_lifecycle_max_completed, ops_lifecycle_max_concurrent, ops_lifecycle_active_count, ops_lifecycle_created_at_ms, ops_lifecycle_completed_at_ms, ops_lifecycle_wait_active, ops_lifecycle_wait_request_id, ops_lifecycle_wait_operation_ids, peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, obligation_ops_barrier_satisfaction, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

CoreNext ==
    \/ DeliverQueuedRoute
    \/ \E arg_operation_id \in OperationIdValues : \E arg_operation_kind \in OperationKindValues : ops_lifecycle_RegisterOperation(arg_operation_id, arg_operation_kind)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_ProvisioningSucceeded(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_ProvisioningFailedCompletesWait(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_ProvisioningFailed(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_PeerReady(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_RegisterWatcher(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_ProgressReported(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_CompleteOperationCompletesWait(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_CompleteOperation(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_FailOperationCompletesWait(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_FailOperation(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_CancelOperationCompletesWait(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_CancelOperation(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_RetireRequested(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_RetireCompletedCompletesWait(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_RetireCompleted(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : ops_lifecycle_CollectTerminal(arg_operation_id)
    \/ ops_lifecycle_OwnerTerminatedCompletesWait
    \/ ops_lifecycle_OwnerTerminated
    \/ \E arg_wait_request_id \in WaitRequestIdValues : \E arg_operation_ids \in SeqOfOperationIdValues : ops_lifecycle_BeginWaitAllImmediate(arg_wait_request_id, arg_operation_ids)
    \/ \E arg_wait_request_id \in WaitRequestIdValues : \E arg_operation_ids \in SeqOfOperationIdValues : ops_lifecycle_BeginWaitAllPending(arg_wait_request_id, arg_operation_ids)
    \/ \E arg_wait_request_id \in WaitRequestIdValues : ops_lifecycle_CancelWaitAll(arg_wait_request_id)
    \/ ops_lifecycle_CollectCompleted
    \/ \E arg_peer_id \in PeerIdValues : peer_comms_TrustPeer(arg_peer_id)
    \/ \E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : peer_comms_ReceiveTrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)
    \/ \E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : peer_comms_DropUntrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)
    \/ \E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputDelivered(arg_raw_item_id)
    \/ \E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputContinue(arg_raw_item_id)
    \/ QuiescentStutter

InjectNext ==
    \/ \E arg_operation_id \in OperationIdValues : \E arg_operation_kind \in OperationKindValues : Inject_register_operation(arg_operation_id, arg_operation_kind)
    \/ \E arg_operation_id \in OperationIdValues : Inject_provisioning_succeeded(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : Inject_peer_ready(arg_operation_id)
    \/ \E arg_operation_id \in OperationIdValues : Inject_complete_operation(arg_operation_id)
    \/ \E arg_peer_id \in PeerIdValues : Inject_peer_trust(arg_peer_id)
    \/ \E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : Inject_peer_receive(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)

Next ==
    \/ CoreNext
    \/ InjectNext

WitnessNext_ops_peer_ready_trusts_peer_comms_path ==
    \/ CoreNext
    \/ WitnessInjectNext_ops_peer_ready_trusts_peer_comms_path


ops_peer_ready_trusts_peer_comms == \A input_packet \in observed_inputs : ((input_packet.machine = "peer_comms" /\ input_packet.variant = "TrustPeer" /\ input_packet.source_route = "ops_peer_ready_trusts_peer_comms") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "ops_lifecycle" /\ input_packet.source_effect = "ExposeOperationPeer" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "ops_lifecycle" /\ effect_packet.variant = "ExposeOperationPeer" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = "ops_peer_ready_trusts_peer_comms" /\ route_packet.source_machine = "ops_lifecycle" /\ route_packet.effect = "ExposeOperationPeer" /\ route_packet.target_machine = "peer_comms" /\ route_packet.target_input = "TrustPeer" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))


RouteObserved_ops_peer_ready_trusts_peer_comms == \E packet \in RoutePackets : packet.route = "ops_peer_ready_trusts_peer_comms"
RouteCoverage_ops_peer_ready_trusts_peer_comms == (RouteObserved_ops_peer_ready_trusts_peer_comms \/ ~RouteObserved_ops_peer_ready_trusts_peer_comms)
CoverageInstrumentation == RouteCoverage_ops_peer_ready_trusts_peer_comms

CiStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 1 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(ops_lifecycle_known_operations) <= 1 /\ Cardinality(DOMAIN ops_lifecycle_operation_status) <= 1 /\ Cardinality(DOMAIN ops_lifecycle_operation_kind) <= 1 /\ Cardinality(DOMAIN ops_lifecycle_peer_ready) <= 1 /\ Cardinality(DOMAIN ops_lifecycle_progress_count) <= 1 /\ Cardinality(DOMAIN ops_lifecycle_watcher_count) <= 1 /\ Cardinality(DOMAIN ops_lifecycle_terminal_outcome) <= 1 /\ Cardinality(DOMAIN ops_lifecycle_terminal_buffered) <= 1 /\ Len(ops_lifecycle_completed_order) <= 1 /\ Cardinality(DOMAIN ops_lifecycle_created_at_ms) <= 1 /\ Cardinality(DOMAIN ops_lifecycle_completed_at_ms) <= 1 /\ Len(ops_lifecycle_wait_operation_ids) <= 1 /\ Cardinality(peer_comms_trusted_peers) <= 1 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 1 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 1 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 1 /\ Cardinality(DOMAIN peer_comms_text_projection) <= 1 /\ Cardinality(DOMAIN peer_comms_content_shape) <= 1 /\ Cardinality(DOMAIN peer_comms_request_id) <= 1 /\ Cardinality(DOMAIN peer_comms_reservation_key) <= 1 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 1 /\ Len(peer_comms_submission_queue) <= 1
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(ops_lifecycle_known_operations) <= 2 /\ Cardinality(DOMAIN ops_lifecycle_operation_status) <= 2 /\ Cardinality(DOMAIN ops_lifecycle_operation_kind) <= 2 /\ Cardinality(DOMAIN ops_lifecycle_peer_ready) <= 2 /\ Cardinality(DOMAIN ops_lifecycle_progress_count) <= 2 /\ Cardinality(DOMAIN ops_lifecycle_watcher_count) <= 2 /\ Cardinality(DOMAIN ops_lifecycle_terminal_outcome) <= 2 /\ Cardinality(DOMAIN ops_lifecycle_terminal_buffered) <= 2 /\ Len(ops_lifecycle_completed_order) <= 2 /\ Cardinality(DOMAIN ops_lifecycle_created_at_ms) <= 2 /\ Cardinality(DOMAIN ops_lifecycle_completed_at_ms) <= 2 /\ Len(ops_lifecycle_wait_operation_ids) <= 2 /\ Cardinality(peer_comms_trusted_peers) <= 2 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 2 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 2 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 2 /\ Cardinality(DOMAIN peer_comms_text_projection) <= 2 /\ Cardinality(DOMAIN peer_comms_content_shape) <= 2 /\ Cardinality(DOMAIN peer_comms_request_id) <= 2 /\ Cardinality(DOMAIN peer_comms_reservation_key) <= 2 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 2 /\ Len(peer_comms_submission_queue) <= 2
WitnessStateConstraint_ops_peer_ready_trusts_peer_comms_path == /\ model_step_count <= 7 /\ Len(pending_inputs) <= 4 /\ Cardinality(observed_inputs) <= 8 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 4 /\ Cardinality(observed_transitions) <= 7 /\ Cardinality(ops_lifecycle_known_operations) <= 4 /\ Cardinality(DOMAIN ops_lifecycle_operation_status) <= 4 /\ Cardinality(DOMAIN ops_lifecycle_operation_kind) <= 4 /\ Cardinality(DOMAIN ops_lifecycle_peer_ready) <= 4 /\ Cardinality(DOMAIN ops_lifecycle_progress_count) <= 4 /\ Cardinality(DOMAIN ops_lifecycle_watcher_count) <= 4 /\ Cardinality(DOMAIN ops_lifecycle_terminal_outcome) <= 4 /\ Cardinality(DOMAIN ops_lifecycle_terminal_buffered) <= 4 /\ Len(ops_lifecycle_completed_order) <= 4 /\ Cardinality(DOMAIN ops_lifecycle_created_at_ms) <= 4 /\ Cardinality(DOMAIN ops_lifecycle_completed_at_ms) <= 4 /\ Len(ops_lifecycle_wait_operation_ids) <= 4 /\ Cardinality(peer_comms_trusted_peers) <= 4 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 4 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 4 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 4 /\ Cardinality(DOMAIN peer_comms_text_projection) <= 4 /\ Cardinality(DOMAIN peer_comms_content_shape) <= 4 /\ Cardinality(DOMAIN peer_comms_request_id) <= 4 /\ Cardinality(DOMAIN peer_comms_reservation_key) <= 4 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 4 /\ Len(peer_comms_submission_queue) <= 4

Spec == Init /\ [][Next]_vars
WitnessSpec_ops_peer_ready_trusts_peer_comms_path == WitnessInit_ops_peer_ready_trusts_peer_comms_path /\ [] [WitnessNext_ops_peer_ready_trusts_peer_comms_path]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_operation_id \in OperationIdValues : \E arg_operation_kind \in OperationKindValues : ops_lifecycle_RegisterOperation(arg_operation_id, arg_operation_kind)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_ProvisioningSucceeded(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_ProvisioningFailedCompletesWait(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_ProvisioningFailed(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_PeerReady(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_RegisterWatcher(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_ProgressReported(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_CompleteOperationCompletesWait(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_CompleteOperation(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_FailOperationCompletesWait(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_FailOperation(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_CancelOperationCompletesWait(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_CancelOperation(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_RetireRequested(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_RetireCompletedCompletesWait(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_RetireCompleted(arg_operation_id)) /\ WF_vars(\E arg_operation_id \in OperationIdValues : ops_lifecycle_CollectTerminal(arg_operation_id)) /\ WF_vars(ops_lifecycle_OwnerTerminatedCompletesWait) /\ WF_vars(ops_lifecycle_OwnerTerminated) /\ WF_vars(\E arg_wait_request_id \in WaitRequestIdValues : \E arg_operation_ids \in SeqOfOperationIdValues : ops_lifecycle_BeginWaitAllImmediate(arg_wait_request_id, arg_operation_ids)) /\ WF_vars(\E arg_wait_request_id \in WaitRequestIdValues : \E arg_operation_ids \in SeqOfOperationIdValues : ops_lifecycle_BeginWaitAllPending(arg_wait_request_id, arg_operation_ids)) /\ WF_vars(\E arg_wait_request_id \in WaitRequestIdValues : ops_lifecycle_CancelWaitAll(arg_wait_request_id)) /\ WF_vars(ops_lifecycle_CollectCompleted) /\ WF_vars(\E arg_peer_id \in PeerIdValues : peer_comms_TrustPeer(arg_peer_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : peer_comms_ReceiveTrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : peer_comms_DropUntrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputDelivered(arg_raw_item_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputContinue(arg_raw_item_id)) /\ WF_vars(WitnessInjectNext_ops_peer_ready_trusts_peer_comms_path)

WitnessRouteObserved_ops_peer_ready_trusts_peer_comms_path_ops_peer_ready_trusts_peer_comms == <> RouteObserved_ops_peer_ready_trusts_peer_comms
WitnessStateObserved_ops_peer_ready_trusts_peer_comms_path_1 == <> (ops_lifecycle_phase = "Active")
WitnessStateObserved_ops_peer_ready_trusts_peer_comms_path_2 == <> (peer_comms_phase = "Absent")
WitnessTransitionObserved_ops_peer_ready_trusts_peer_comms_path_ops_lifecycle_RegisterOperation == <> (\E packet \in observed_transitions : /\ packet.machine = "ops_lifecycle" /\ packet.transition = "RegisterOperation")
WitnessTransitionObserved_ops_peer_ready_trusts_peer_comms_path_ops_lifecycle_ProvisioningSucceeded == <> (\E packet \in observed_transitions : /\ packet.machine = "ops_lifecycle" /\ packet.transition = "ProvisioningSucceeded")
WitnessTransitionObserved_ops_peer_ready_trusts_peer_comms_path_ops_lifecycle_PeerReady == <> (\E packet \in observed_transitions : /\ packet.machine = "ops_lifecycle" /\ packet.transition = "PeerReady")
WitnessTransitionObserved_ops_peer_ready_trusts_peer_comms_path_peer_comms_TrustPeer == <> (\E packet \in observed_transitions : /\ packet.machine = "peer_comms" /\ packet.transition = "TrustPeer")
WitnessTransitionOrder_ops_peer_ready_trusts_peer_comms_path_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "ops_lifecycle" /\ earlier.transition = "RegisterOperation" /\ later.machine = "ops_lifecycle" /\ later.transition = "ProvisioningSucceeded" /\ earlier.step < later.step)
WitnessTransitionOrder_ops_peer_ready_trusts_peer_comms_path_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "ops_lifecycle" /\ earlier.transition = "ProvisioningSucceeded" /\ later.machine = "ops_lifecycle" /\ later.transition = "PeerReady" /\ earlier.step < later.step)
WitnessTransitionOrder_ops_peer_ready_trusts_peer_comms_path_3 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "ops_lifecycle" /\ earlier.transition = "PeerReady" /\ later.machine = "peer_comms" /\ later.transition = "TrustPeer" /\ earlier.step < later.step)

THEOREM Spec => []ops_peer_ready_trusts_peer_comms
THEOREM Spec => []ops_lifecycle_terminal_buffered_only_for_terminal_states
THEOREM Spec => []ops_lifecycle_peer_ready_implies_mob_member_child
THEOREM Spec => []ops_lifecycle_peer_ready_implies_present
THEOREM Spec => []ops_lifecycle_present_operations_keep_kind_identity
THEOREM Spec => []ops_lifecycle_terminal_statuses_have_matching_terminal_outcome
THEOREM Spec => []ops_lifecycle_nonterminal_statuses_have_no_terminal_outcome
THEOREM Spec => []peer_comms_queued_items_are_classified
THEOREM Spec => []peer_comms_queued_items_preserve_content_shape
THEOREM Spec => []peer_comms_queued_items_preserve_text_projection
THEOREM Spec => []peer_comms_queued_items_preserve_correlation_slots

=============================================================================
