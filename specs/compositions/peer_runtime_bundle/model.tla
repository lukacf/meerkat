---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for peer_runtime_bundle.

CONSTANTS AdmissionEffectValues, ContentShapeValues, HandlingModeValues, NatValues, PeerIdValues, PolicyDecisionValues, RawItemIdValues, RawPeerKindValues, RequestIdValues, ReservationKeyValues, RunIdValues, SetOfStringValues, StringValues, WorkIdValues

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
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN Tail(seq) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))
AppendIfMissing(seq, value) == IF value \in SeqElements(seq) THEN seq ELSE Append(seq, value)
Machines == {
    <<"peer_comms", "PeerCommsMachine", "peer_plane">>,
    <<"runtime_control", "RuntimeControlMachine", "control_plane">>,
    <<"runtime_ingress", "RuntimeIngressMachine", "ordinary_ingress">>
}

RouteNames == {
    "peer_candidate_enters_runtime_admission",
    "admitted_peer_work_enters_ingress"
}

Actors == {
    "peer_plane",
    "control_plane",
    "ordinary_ingress"
}

ActorPriorities == {
    <<"control_plane", "peer_plane">>
}

SchedulerRules == {
    <<"PreemptWhenReady", "control_plane", "peer_plane">>
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "peer_comms" -> "peer_plane"
      [] machine_id = "runtime_control" -> "control_plane"
      [] machine_id = "runtime_ingress" -> "ordinary_ingress"
      [] OTHER -> "unknown_actor"

RouteSource(route_name) ==
    CASE route_name = "peer_candidate_enters_runtime_admission" -> "peer_comms"
      [] route_name = "admitted_peer_work_enters_ingress" -> "runtime_control"
      [] OTHER -> "unknown_machine"

RouteEffect(route_name) ==
    CASE route_name = "peer_candidate_enters_runtime_admission" -> "SubmitPeerInputCandidate"
      [] route_name = "admitted_peer_work_enters_ingress" -> "SubmitAdmittedIngressEffect"
      [] OTHER -> "unknown_effect"

RouteTargetMachine(route_name) ==
    CASE route_name = "peer_candidate_enters_runtime_admission" -> "runtime_control"
      [] route_name = "admitted_peer_work_enters_ingress" -> "runtime_ingress"
      [] OTHER -> "unknown_machine"

RouteTargetInput(route_name) ==
    CASE route_name = "peer_candidate_enters_runtime_admission" -> "SubmitWork"
      [] route_name = "admitted_peer_work_enters_ingress" -> "AdmitQueued"
      [] OTHER -> "unknown_input"

RouteDeliveryKind(route_name) ==
    CASE route_name = "peer_candidate_enters_runtime_admission" -> "Immediate"
      [] route_name = "admitted_peer_work_enters_ingress" -> "Immediate"
      [] OTHER -> "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
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
    /\ runtime_control_phase = "Initializing"
    /\ runtime_control_current_run_id = None
    /\ runtime_control_pre_run_state = None
    /\ runtime_control_wake_pending = FALSE
    /\ runtime_control_process_pending = FALSE
    /\ runtime_ingress_phase = "Active"
    /\ runtime_ingress_admitted_inputs = {}
    /\ runtime_ingress_admission_order = <<>>
    /\ runtime_ingress_content_shape = [x \in {} |-> None]
    /\ runtime_ingress_request_id = [x \in {} |-> None]
    /\ runtime_ingress_reservation_key = [x \in {} |-> None]
    /\ runtime_ingress_policy_snapshot = [x \in {} |-> None]
    /\ runtime_ingress_handling_mode = [x \in {} |-> None]
    /\ runtime_ingress_lifecycle = [x \in {} |-> None]
    /\ runtime_ingress_terminal_outcome = [x \in {} |-> None]
    /\ runtime_ingress_queue = <<>>
    /\ runtime_ingress_steer_queue = <<>>
    /\ runtime_ingress_current_run = None
    /\ runtime_ingress_current_run_contributors = <<>>
    /\ runtime_ingress_last_run = [x \in {} |-> None]
    /\ runtime_ingress_last_boundary_sequence = [x \in {} |-> None]
    /\ runtime_ingress_wake_requested = FALSE
    /\ runtime_ingress_process_requested = FALSE
    /\ runtime_ingress_silent_intent_overrides = {}
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

WitnessInit_trusted_peer_enters_runtime ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:trusted_peer_enters_runtime:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:trusted_peer_enters_runtime:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:trusted_peer_enters_runtime:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> "peer_1"], source_kind |-> "entry", source_route |-> "witness:trusted_peer_enters_runtime:2", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0], [machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [content_shape |-> "InlineImage", peer_id |-> "peer_1", raw_item_id |-> "raw_1", raw_kind |-> "Message", request_id |-> None, reservation_key |-> None, text_projection |-> "see attached diagram"], source_kind |-> "entry", source_route |-> "witness:trusted_peer_enters_runtime:3", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0], [machine |-> "peer_comms", variant |-> "SubmitTypedPeerInput", payload |-> [raw_item_id |-> "raw_1"], source_kind |-> "entry", source_route |-> "witness:trusted_peer_enters_runtime:4", source_machine |-> "external_entry", source_effect |-> "SubmitTypedPeerInput", effect_id |-> 0]>>

WitnessInit_admitted_peer_work_enters_ingress ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> "peer_1"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:2", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0], [machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [content_shape |-> "InlineImage", peer_id |-> "peer_1", raw_item_id |-> "raw_1", raw_kind |-> "request", request_id |-> Some("req_1"), reservation_key |-> Some("resv_1"), text_projection |-> "inspect this inline image"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:3", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0], [machine |-> "peer_comms", variant |-> "SubmitTypedPeerInput", payload |-> [raw_item_id |-> "raw_1"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:4", source_machine |-> "external_entry", source_effect |-> "SubmitTypedPeerInput", effect_id |-> 0], [machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> "SubmitAdmittedIngressEffect", content_shape |-> "InlineImage", handling_mode |-> "Steer", request_id |-> None, reservation_key |-> None, work_id |-> "raw_1"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:5", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0]>>

WitnessInit_control_preempts_peer_delivery ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:control_preempts_peer_delivery:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0], [machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> "peer_1"], source_kind |-> "entry", source_route |-> "witness:control_preempts_peer_delivery:2", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:control_preempts_peer_delivery:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0], [machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> "peer_1"], source_kind |-> "entry", source_route |-> "witness:control_preempts_peer_delivery:2", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> "peer_1"], source_kind |-> "entry", source_route |-> "witness:control_preempts_peer_delivery:2", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [content_shape |-> "TextOnly", peer_id |-> "peer_1", raw_item_id |-> "raw_1", raw_kind |-> "Message", request_id |-> None, reservation_key |-> None, text_projection |-> "priority handoff"], source_kind |-> "entry", source_route |-> "witness:control_preempts_peer_delivery:3", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0]>>

peer_comms__ClassFor(raw_kind) == (IF (raw_kind = "request") THEN "ActionableRequest" ELSE (IF (raw_kind = "response_terminal") THEN "InlineResponseTerminal" ELSE (IF (raw_kind = "response_progress") THEN "InlineResponseProgress" ELSE (IF (raw_kind = "plain_event") THEN "ActionableEvent" ELSE (IF (raw_kind = "silent_request") THEN "InlineSilentRequest" ELSE "ActionableMessage")))))

peer_comms_TrustPeer(arg_peer_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "peer_comms"
       /\ packet.variant = "TrustPeer"
       /\ packet.payload.peer_id = arg_peer_id
       /\ ~HigherPriorityReady("peer_plane")
       /\ peer_comms_phase = "Absent" \/ peer_comms_phase = "Received"
       /\ peer_comms_phase' = "Absent"
       /\ peer_comms_trusted_peers' = (peer_comms_trusted_peers \cup {packet.payload.peer_id})
       /\ UNCHANGED << peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "TrustPeer", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Absent"]}
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
       /\ UNCHANGED << peer_comms_trusted_peers, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "ReceiveTrustedPeerEnvelope", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Received"]}
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
       /\ UNCHANGED << peer_comms_trusted_peers, peer_comms_classified_as, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "DropUntrustedPeerEnvelope", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Dropped"]}
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
       /\ UNCHANGED << peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_content_shape THEN peer_comms_content_shape[packet.payload.raw_item_id] ELSE "None"), handling_mode |-> "Steer", request_id |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_request_id THEN peer_comms_request_id[packet.payload.raw_item_id] ELSE None), reservation_key |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_reservation_key THEN peer_comms_reservation_key[packet.payload.raw_item_id] ELSE None), work_id |-> packet.payload.raw_item_id], source_kind |-> "route", source_route |-> "peer_candidate_enters_runtime_admission", source_machine |-> "peer_comms", source_effect |-> "SubmitPeerInputCandidate", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_content_shape THEN peer_comms_content_shape[packet.payload.raw_item_id] ELSE "None"), handling_mode |-> "Steer", request_id |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_request_id THEN peer_comms_request_id[packet.payload.raw_item_id] ELSE None), reservation_key |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_reservation_key THEN peer_comms_reservation_key[packet.payload.raw_item_id] ELSE None), work_id |-> packet.payload.raw_item_id], source_kind |-> "route", source_route |-> "peer_candidate_enters_runtime_admission", source_machine |-> "peer_comms", source_effect |-> "SubmitPeerInputCandidate", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "peer_candidate_enters_runtime_admission", source_machine |-> "peer_comms", effect |-> "SubmitPeerInputCandidate", target_machine |-> "runtime_control", target_input |-> "SubmitWork", payload |-> [content_shape |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_content_shape THEN peer_comms_content_shape[packet.payload.raw_item_id] ELSE "None"), handling_mode |-> "Steer", request_id |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_request_id THEN peer_comms_request_id[packet.payload.raw_item_id] ELSE None), reservation_key |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_reservation_key THEN peer_comms_reservation_key[packet.payload.raw_item_id] ELSE None), work_id |-> packet.payload.raw_item_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "SubmitTypedPeerInputDelivered"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "peer_comms", variant |-> "SubmitPeerInputCandidate", payload |-> [content_shape |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_content_shape THEN peer_comms_content_shape[packet.payload.raw_item_id] ELSE "None"), peer_input_class |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_classified_as THEN peer_comms_classified_as[packet.payload.raw_item_id] ELSE "None"), raw_item_id |-> packet.payload.raw_item_id, request_id |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_request_id THEN peer_comms_request_id[packet.payload.raw_item_id] ELSE None), reservation_key |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_reservation_key THEN peer_comms_reservation_key[packet.payload.raw_item_id] ELSE None), text_projection |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_text_projection THEN peer_comms_text_projection[packet.payload.raw_item_id] ELSE "None")], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitTypedPeerInputDelivered"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "SubmitTypedPeerInputDelivered", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Delivered"]}
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
       /\ UNCHANGED << peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_content_shape THEN peer_comms_content_shape[packet.payload.raw_item_id] ELSE "None"), handling_mode |-> "Steer", request_id |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_request_id THEN peer_comms_request_id[packet.payload.raw_item_id] ELSE None), reservation_key |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_reservation_key THEN peer_comms_reservation_key[packet.payload.raw_item_id] ELSE None), work_id |-> packet.payload.raw_item_id], source_kind |-> "route", source_route |-> "peer_candidate_enters_runtime_admission", source_machine |-> "peer_comms", source_effect |-> "SubmitPeerInputCandidate", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_content_shape THEN peer_comms_content_shape[packet.payload.raw_item_id] ELSE "None"), handling_mode |-> "Steer", request_id |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_request_id THEN peer_comms_request_id[packet.payload.raw_item_id] ELSE None), reservation_key |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_reservation_key THEN peer_comms_reservation_key[packet.payload.raw_item_id] ELSE None), work_id |-> packet.payload.raw_item_id], source_kind |-> "route", source_route |-> "peer_candidate_enters_runtime_admission", source_machine |-> "peer_comms", source_effect |-> "SubmitPeerInputCandidate", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "peer_candidate_enters_runtime_admission", source_machine |-> "peer_comms", effect |-> "SubmitPeerInputCandidate", target_machine |-> "runtime_control", target_input |-> "SubmitWork", payload |-> [content_shape |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_content_shape THEN peer_comms_content_shape[packet.payload.raw_item_id] ELSE "None"), handling_mode |-> "Steer", request_id |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_request_id THEN peer_comms_request_id[packet.payload.raw_item_id] ELSE None), reservation_key |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_reservation_key THEN peer_comms_reservation_key[packet.payload.raw_item_id] ELSE None), work_id |-> packet.payload.raw_item_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "SubmitTypedPeerInputContinue"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "peer_comms", variant |-> "SubmitPeerInputCandidate", payload |-> [content_shape |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_content_shape THEN peer_comms_content_shape[packet.payload.raw_item_id] ELSE "None"), peer_input_class |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_classified_as THEN peer_comms_classified_as[packet.payload.raw_item_id] ELSE "None"), raw_item_id |-> packet.payload.raw_item_id, request_id |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_request_id THEN peer_comms_request_id[packet.payload.raw_item_id] ELSE None), reservation_key |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_reservation_key THEN peer_comms_reservation_key[packet.payload.raw_item_id] ELSE None), text_projection |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_text_projection THEN peer_comms_text_projection[packet.payload.raw_item_id] ELSE "None")], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitTypedPeerInputContinue"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "SubmitTypedPeerInputContinue", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Received"]}
       /\ model_step_count' = model_step_count + 1


peer_comms_queued_items_are_classified == (\A raw_item_id \in SeqElements(peer_comms_submission_queue) : (raw_item_id \in DOMAIN peer_comms_classified_as))
peer_comms_queued_items_preserve_content_shape == (\A raw_item_id \in SeqElements(peer_comms_submission_queue) : (raw_item_id \in DOMAIN peer_comms_content_shape))
peer_comms_queued_items_preserve_text_projection == (\A raw_item_id \in SeqElements(peer_comms_submission_queue) : (raw_item_id \in DOMAIN peer_comms_text_projection))
peer_comms_queued_items_preserve_correlation_slots == (\A raw_item_id \in SeqElements(peer_comms_submission_queue) : ((raw_item_id \in DOMAIN peer_comms_request_id) /\ (raw_item_id \in DOMAIN peer_comms_reservation_key)))

runtime_control_Initialize ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "Initialize"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Initializing"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "Initialize", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_BeginRunFromIdle(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "BeginRun"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ (runtime_control_current_run_id = None)
       /\ runtime_control_phase' = "Running"
       /\ runtime_control_current_run_id' = Some(packet.payload.run_id)
       /\ runtime_control_pre_run_state' = Some("Idle")
       /\ runtime_control_wake_pending' = FALSE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitRunPrimitive", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRunFromIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "BeginRunFromIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_BeginRunFromRetired(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "BeginRun"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Retired"
       /\ (runtime_control_current_run_id = None)
       /\ runtime_control_phase' = "Running"
       /\ runtime_control_current_run_id' = Some(packet.payload.run_id)
       /\ runtime_control_pre_run_state' = Some("Retired")
       /\ runtime_control_wake_pending' = FALSE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitRunPrimitive", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRunFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "BeginRunFromRetired", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunCompleted(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunCompleted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunCompleted", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunFailed(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunFailed"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunFailed", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunCancelled(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunCancelled"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunCancelled", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RecoverRequestedFromIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RecoverRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ runtime_control_phase' = "Recovering"
       /\ runtime_control_pre_run_state' = Some("Idle")
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RecoverRequestedFromIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Recovering"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RecoverRequestedFromRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RecoverRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ runtime_control_phase' = "Recovering"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = Some("Running")
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RecoverRequestedFromRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Recovering"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RecoverySucceeded ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RecoverySucceeded"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Recovering"
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ runtime_control_wake_pending' = FALSE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RecoverySucceeded", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RetireRequestedFromIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RetireRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ runtime_control_phase' = "Retired"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ApplyControlPlaneCommand", payload |-> [command |-> "Retire"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireRequestedFromIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RetireRequestedFromIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RetireRequestedFromRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RetireRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ runtime_control_phase' = "Retired"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ApplyControlPlaneCommand", payload |-> [command |-> "Retire"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireRequestedFromRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RetireRequestedFromRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_ResetRequested ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "ResetRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Initializing" \/ runtime_control_phase = "Idle" \/ runtime_control_phase = "Recovering" \/ runtime_control_phase = "Retired"
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ runtime_control_wake_pending' = FALSE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ApplyControlPlaneCommand", payload |-> [command |-> "Reset"], effect_id |-> (model_step_count + 1), source_transition |-> "ResetRequested"], [machine |-> "runtime_control", variant |-> "ResolveCompletionAsTerminated", payload |-> [reason |-> "Reset"], effect_id |-> (model_step_count + 1), source_transition |-> "ResetRequested"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "ResetRequested", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_StopRequested ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "StopRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Initializing" \/ runtime_control_phase = "Idle" \/ runtime_control_phase = "Running" \/ runtime_control_phase = "Recovering" \/ runtime_control_phase = "Retired"
       /\ runtime_control_phase' = "Stopped"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ApplyControlPlaneCommand", payload |-> [command |-> "Stop"], effect_id |-> (model_step_count + 1), source_transition |-> "StopRequested"], [machine |-> "runtime_control", variant |-> "ResolveCompletionAsTerminated", payload |-> [reason |-> "Stopped"], effect_id |-> (model_step_count + 1), source_transition |-> "StopRequested"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "StopRequested", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_DestroyRequested ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "DestroyRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Initializing" \/ runtime_control_phase = "Idle" \/ runtime_control_phase = "Running" \/ runtime_control_phase = "Recovering" \/ runtime_control_phase = "Retired" \/ runtime_control_phase = "Stopped"
       /\ runtime_control_phase' = "Destroyed"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ApplyControlPlaneCommand", payload |-> [command |-> "Destroy"], effect_id |-> (model_step_count + 1), source_transition |-> "DestroyRequested"], [machine |-> "runtime_control", variant |-> "ResolveCompletionAsTerminated", payload |-> [reason |-> "Destroyed"], effect_id |-> (model_step_count + 1), source_transition |-> "DestroyRequested"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "DestroyRequested", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_ResumeRequested ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "ResumeRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Recovering"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "ResumeRequested", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "SubmitWork"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.content_shape = arg_content_shape
       /\ packet.payload.handling_mode = arg_handling_mode
       /\ packet.payload.request_id = arg_request_id
       /\ packet.payload.reservation_key = arg_reservation_key
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ResolveAdmission", payload |-> [work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitWorkFromIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "SubmitWorkFromIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "SubmitWork"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.content_shape = arg_content_shape
       /\ packet.payload.handling_mode = arg_handling_mode
       /\ packet.payload.request_id = arg_request_id
       /\ packet.payload.reservation_key = arg_reservation_key
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ runtime_control_phase' = "Running"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ResolveAdmission", payload |-> [work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitWorkFromRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "SubmitWorkFromRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionAccepted"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.content_shape = arg_content_shape
       /\ packet.payload.handling_mode = arg_handling_mode
       /\ packet.payload.request_id = arg_request_id
       /\ packet.payload.reservation_key = arg_reservation_key
       /\ packet.payload.admission_effect = arg_admission_effect
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ (packet.payload.handling_mode = "Queue")
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_wake_pending' = TRUE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "PeerQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "PeerQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "PeerQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleQueue"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleQueue"], [machine |-> "runtime_control", variant |-> "SignalWake", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleQueue"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedIdleQueue", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionAccepted"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.content_shape = arg_content_shape
       /\ packet.payload.handling_mode = arg_handling_mode
       /\ packet.payload.request_id = arg_request_id
       /\ packet.payload.reservation_key = arg_reservation_key
       /\ packet.payload.admission_effect = arg_admission_effect
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ (packet.payload.handling_mode = "Steer")
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_wake_pending' = TRUE
       /\ runtime_control_process_pending' = TRUE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "PeerQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "PeerQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "PeerQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleSteer"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleSteer"], [machine |-> "runtime_control", variant |-> "SignalWake", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleSteer"], [machine |-> "runtime_control", variant |-> "SignalImmediateProcess", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleSteer"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedIdleSteer", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionAccepted"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.content_shape = arg_content_shape
       /\ packet.payload.handling_mode = arg_handling_mode
       /\ packet.payload.request_id = arg_request_id
       /\ packet.payload.reservation_key = arg_reservation_key
       /\ packet.payload.admission_effect = arg_admission_effect
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (packet.payload.handling_mode = "Queue")
       /\ runtime_control_phase' = "Running"
       /\ runtime_control_wake_pending' = FALSE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "PeerQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "PeerQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "PeerQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningQueue"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningQueue"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedRunningQueue", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionAccepted"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.content_shape = arg_content_shape
       /\ packet.payload.handling_mode = arg_handling_mode
       /\ packet.payload.request_id = arg_request_id
       /\ packet.payload.reservation_key = arg_reservation_key
       /\ packet.payload.admission_effect = arg_admission_effect
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (packet.payload.handling_mode = "Steer")
       /\ runtime_control_phase' = "Running"
       /\ runtime_control_wake_pending' = TRUE
       /\ runtime_control_process_pending' = TRUE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "PeerQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "PeerQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "PeerQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningSteer"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningSteer"], [machine |-> "runtime_control", variant |-> "SignalWake", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningSteer"], [machine |-> "runtime_control", variant |-> "SignalImmediateProcess", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningSteer"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedRunningSteer", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionRejected"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> packet.payload.reason, kind |-> "AdmissionRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionRejectedIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionRejectedIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionRejected"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ runtime_control_phase' = "Running"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> packet.payload.reason, kind |-> "AdmissionRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionRejectedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionRejectedRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionDeduplicated"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.existing_work_id = arg_existing_work_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "ExistingInputLinked", kind |-> "AdmissionDeduplicated"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionDeduplicatedIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionDeduplicatedIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionDeduplicated"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.existing_work_id = arg_existing_work_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ runtime_control_phase' = "Running"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "ExistingInputLinked", kind |-> "AdmissionDeduplicated"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionDeduplicatedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionDeduplicatedRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_ExternalToolDeltaReceivedIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "ExternalToolDeltaReceived"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "Received", kind |-> "ExternalToolDelta"], effect_id |-> (model_step_count + 1), source_transition |-> "ExternalToolDeltaReceivedIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "ExternalToolDeltaReceivedIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_ExternalToolDeltaReceivedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "ExternalToolDeltaReceived"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ runtime_control_phase' = "Running"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "Received", kind |-> "ExternalToolDelta"], effect_id |-> (model_step_count + 1), source_transition |-> "ExternalToolDeltaReceivedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "ExternalToolDeltaReceivedRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_ExternalToolDeltaReceivedRecovering ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "ExternalToolDeltaReceived"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Recovering"
       /\ runtime_control_phase' = "Recovering"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "Received", kind |-> "ExternalToolDelta"], effect_id |-> (model_step_count + 1), source_transition |-> "ExternalToolDeltaReceivedRecovering"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "ExternalToolDeltaReceivedRecovering", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Recovering"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_ExternalToolDeltaReceivedRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "ExternalToolDeltaReceived"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Retired"
       /\ runtime_control_phase' = "Retired"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "Received", kind |-> "ExternalToolDelta"], effect_id |-> (model_step_count + 1), source_transition |-> "ExternalToolDeltaReceivedRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "ExternalToolDeltaReceivedRetired", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RecycleRequestedFromRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RecycleRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Retired"
       /\ (runtime_control_current_run_id = None)
       /\ runtime_control_phase' = "Recovering"
       /\ runtime_control_pre_run_state' = Some("Retired")
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "InitiateRecycle", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecycleRequestedFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RecycleRequestedFromRetired", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Recovering"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RecycleRequestedFromIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RecycleRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ (runtime_control_current_run_id = None)
       /\ runtime_control_phase' = "Recovering"
       /\ runtime_control_pre_run_state' = Some("Idle")
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "InitiateRecycle", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecycleRequestedFromIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RecycleRequestedFromIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Recovering"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RecycleSucceeded ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RecycleSucceeded"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Recovering"
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ runtime_control_wake_pending' = FALSE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "Succeeded", kind |-> "Recycle"], effect_id |-> (model_step_count + 1), source_transition |-> "RecycleSucceeded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RecycleSucceeded", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_running_implies_active_run == ((runtime_control_phase # "Running") \/ (runtime_control_current_run_id # None))
runtime_control_active_run_only_while_running_or_retired == ((runtime_control_current_run_id = None) \/ (runtime_control_phase = "Running") \/ (runtime_control_phase = "Retired"))

RECURSIVE runtime_ingress_StageDrainSnapshot_ForEach0_last_run(_, _, _)
runtime_ingress_StageDrainSnapshot_ForEach0_last_run(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some(outer_run_id)) IN runtime_ingress_StageDrainSnapshot_ForEach0_last_run(next_acc, Tail(items), outer_run_id)

RECURSIVE runtime_ingress_StageDrainSnapshot_ForEach0_lifecycle(_, _, _)
runtime_ingress_StageDrainSnapshot_ForEach0_lifecycle(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Staged") IN runtime_ingress_StageDrainSnapshot_ForEach0_lifecycle(next_acc, Tail(items), outer_run_id)

RECURSIVE runtime_ingress_BoundaryApplied_ForEach1_last_boundary_sequence(_, _, _)
runtime_ingress_BoundaryApplied_ForEach1_last_boundary_sequence(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some(outer_boundary_sequence)) IN runtime_ingress_BoundaryApplied_ForEach1_last_boundary_sequence(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE runtime_ingress_BoundaryApplied_ForEach1_lifecycle(_, _, _)
runtime_ingress_BoundaryApplied_ForEach1_lifecycle(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "AppliedPendingConsumption") IN runtime_ingress_BoundaryApplied_ForEach1_lifecycle(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE runtime_ingress_RunCompleted_ForEach2_lifecycle(_, _)
runtime_ingress_RunCompleted_ForEach2_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Consumed") IN runtime_ingress_RunCompleted_ForEach2_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_RunCompleted_ForEach2_terminal_outcome(_, _)
runtime_ingress_RunCompleted_ForEach2_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("Consumed")) IN runtime_ingress_RunCompleted_ForEach2_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_RunFailed_ForEach3_lifecycle(_, _)
runtime_ingress_RunFailed_ForEach3_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Queued") IN runtime_ingress_RunFailed_ForEach3_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_RunCancelled_ForEach4_lifecycle(_, _)
runtime_ingress_RunCancelled_ForEach4_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Queued") IN runtime_ingress_RunCancelled_ForEach4_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_CoalesceQueuedInputs_ForEach5_lifecycle(_, _)
runtime_ingress_CoalesceQueuedInputs_ForEach5_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Coalesced") IN runtime_ingress_CoalesceQueuedInputs_ForEach5_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_CoalesceQueuedInputs_ForEach5_terminal_outcome(_, _)
runtime_ingress_CoalesceQueuedInputs_ForEach5_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("Coalesced")) IN runtime_ingress_CoalesceQueuedInputs_ForEach5_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromActive_ForEach6_lifecycle(_, _)
runtime_ingress_ResetFromActive_ForEach6_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN runtime_ingress_ResetFromActive_ForEach6_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromActive_ForEach6_terminal_outcome(_, _)
runtime_ingress_ResetFromActive_ForEach6_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedReset")) IN runtime_ingress_ResetFromActive_ForEach6_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromActive_ForEach7_lifecycle(_, _)
runtime_ingress_ResetFromActive_ForEach7_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN runtime_ingress_ResetFromActive_ForEach7_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromActive_ForEach7_terminal_outcome(_, _)
runtime_ingress_ResetFromActive_ForEach7_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedReset")) IN runtime_ingress_ResetFromActive_ForEach7_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromActive_ForEach8_lifecycle(_, _)
runtime_ingress_ResetFromActive_ForEach8_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN runtime_ingress_ResetFromActive_ForEach8_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromActive_ForEach8_terminal_outcome(_, _)
runtime_ingress_ResetFromActive_ForEach8_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedReset")) IN runtime_ingress_ResetFromActive_ForEach8_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromRetired_ForEach9_lifecycle(_, _)
runtime_ingress_ResetFromRetired_ForEach9_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN runtime_ingress_ResetFromRetired_ForEach9_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromRetired_ForEach9_terminal_outcome(_, _)
runtime_ingress_ResetFromRetired_ForEach9_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedReset")) IN runtime_ingress_ResetFromRetired_ForEach9_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromRetired_ForEach10_lifecycle(_, _)
runtime_ingress_ResetFromRetired_ForEach10_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN runtime_ingress_ResetFromRetired_ForEach10_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromRetired_ForEach10_terminal_outcome(_, _)
runtime_ingress_ResetFromRetired_ForEach10_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedReset")) IN runtime_ingress_ResetFromRetired_ForEach10_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromRetired_ForEach11_lifecycle(_, _)
runtime_ingress_ResetFromRetired_ForEach11_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN runtime_ingress_ResetFromRetired_ForEach11_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromRetired_ForEach11_terminal_outcome(_, _)
runtime_ingress_ResetFromRetired_ForEach11_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedReset")) IN runtime_ingress_ResetFromRetired_ForEach11_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_Destroy_ForEach12_lifecycle(_, _)
runtime_ingress_Destroy_ForEach12_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN runtime_ingress_Destroy_ForEach12_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_Destroy_ForEach12_terminal_outcome(_, _)
runtime_ingress_Destroy_ForEach12_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedDestroyed")) IN runtime_ingress_Destroy_ForEach12_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_Destroy_ForEach13_lifecycle(_, _)
runtime_ingress_Destroy_ForEach13_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN runtime_ingress_Destroy_ForEach13_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_Destroy_ForEach13_terminal_outcome(_, _)
runtime_ingress_Destroy_ForEach13_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedDestroyed")) IN runtime_ingress_Destroy_ForEach13_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_Destroy_ForEach14_lifecycle(_, _)
runtime_ingress_Destroy_ForEach14_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Abandoned") IN runtime_ingress_Destroy_ForEach14_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_Destroy_ForEach14_terminal_outcome(_, _)
runtime_ingress_Destroy_ForEach14_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("AbandonedDestroyed")) IN runtime_ingress_Destroy_ForEach14_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_RecoverFromActive_ForEach15_lifecycle(_, _)
runtime_ingress_RecoverFromActive_ForEach15_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Queued") IN runtime_ingress_RecoverFromActive_ForEach15_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_RecoverFromRetired_ForEach16_lifecycle(_, _)
runtime_ingress_RecoverFromRetired_ForEach16_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Queued") IN runtime_ingress_RecoverFromRetired_ForEach16_lifecycle(next_acc, Tail(items))

runtime_ingress_AdmitQueuedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "AdmitQueued"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.content_shape = arg_content_shape
       /\ packet.payload.handling_mode = arg_handling_mode
       /\ packet.payload.request_id = arg_request_id
       /\ packet.payload.reservation_key = arg_reservation_key
       /\ packet.payload.policy = arg_policy
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ ~((packet.payload.work_id \in runtime_ingress_admitted_inputs))
       /\ (packet.payload.handling_mode = "Queue")
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_admitted_inputs' = (runtime_ingress_admitted_inputs \cup {packet.payload.work_id})
       /\ runtime_ingress_admission_order' = Append(runtime_ingress_admission_order, packet.payload.work_id)
       /\ runtime_ingress_content_shape' = MapSet(runtime_ingress_content_shape, packet.payload.work_id, packet.payload.content_shape)
       /\ runtime_ingress_request_id' = MapSet(runtime_ingress_request_id, packet.payload.work_id, packet.payload.request_id)
       /\ runtime_ingress_reservation_key' = MapSet(runtime_ingress_reservation_key, packet.payload.work_id, packet.payload.reservation_key)
       /\ runtime_ingress_policy_snapshot' = MapSet(runtime_ingress_policy_snapshot, packet.payload.work_id, packet.payload.policy)
       /\ runtime_ingress_handling_mode' = MapSet(runtime_ingress_handling_mode, packet.payload.work_id, packet.payload.handling_mode)
       /\ runtime_ingress_lifecycle' = MapSet(runtime_ingress_lifecycle, packet.payload.work_id, "Queued")
       /\ runtime_ingress_terminal_outcome' = MapSet(runtime_ingress_terminal_outcome, packet.payload.work_id, None)
       /\ runtime_ingress_queue' = Append(runtime_ingress_queue, packet.payload.work_id)
       /\ runtime_ingress_last_run' = MapSet(runtime_ingress_last_run, packet.payload.work_id, None)
       /\ runtime_ingress_last_boundary_sequence' = MapSet(runtime_ingress_last_boundary_sequence, packet.payload.work_id, None)
       /\ runtime_ingress_wake_requested' = (runtime_ingress_wake_requested \/ FALSE)
       /\ runtime_ingress_process_requested' = (runtime_ingress_process_requested \/ FALSE)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressAccepted", payload |-> [work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedQueue"], [machine |-> "runtime_ingress", variant |-> "InputLifecycleNotice", payload |-> [new_state |-> "Queued", work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedQueue"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "AdmitQueuedQueue", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_AdmitQueuedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "AdmitQueued"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.content_shape = arg_content_shape
       /\ packet.payload.handling_mode = arg_handling_mode
       /\ packet.payload.request_id = arg_request_id
       /\ packet.payload.reservation_key = arg_reservation_key
       /\ packet.payload.policy = arg_policy
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ ~((packet.payload.work_id \in runtime_ingress_admitted_inputs))
       /\ (packet.payload.handling_mode = "Steer")
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_admitted_inputs' = (runtime_ingress_admitted_inputs \cup {packet.payload.work_id})
       /\ runtime_ingress_admission_order' = Append(runtime_ingress_admission_order, packet.payload.work_id)
       /\ runtime_ingress_content_shape' = MapSet(runtime_ingress_content_shape, packet.payload.work_id, packet.payload.content_shape)
       /\ runtime_ingress_request_id' = MapSet(runtime_ingress_request_id, packet.payload.work_id, packet.payload.request_id)
       /\ runtime_ingress_reservation_key' = MapSet(runtime_ingress_reservation_key, packet.payload.work_id, packet.payload.reservation_key)
       /\ runtime_ingress_policy_snapshot' = MapSet(runtime_ingress_policy_snapshot, packet.payload.work_id, packet.payload.policy)
       /\ runtime_ingress_handling_mode' = MapSet(runtime_ingress_handling_mode, packet.payload.work_id, packet.payload.handling_mode)
       /\ runtime_ingress_lifecycle' = MapSet(runtime_ingress_lifecycle, packet.payload.work_id, "Queued")
       /\ runtime_ingress_terminal_outcome' = MapSet(runtime_ingress_terminal_outcome, packet.payload.work_id, None)
       /\ runtime_ingress_steer_queue' = Append(runtime_ingress_steer_queue, packet.payload.work_id)
       /\ runtime_ingress_last_run' = MapSet(runtime_ingress_last_run, packet.payload.work_id, None)
       /\ runtime_ingress_last_boundary_sequence' = MapSet(runtime_ingress_last_boundary_sequence, packet.payload.work_id, None)
       /\ runtime_ingress_wake_requested' = (runtime_ingress_wake_requested \/ TRUE)
       /\ runtime_ingress_process_requested' = (runtime_ingress_process_requested \/ TRUE)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressAccepted", payload |-> [work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedSteer"], [machine |-> "runtime_ingress", variant |-> "InputLifecycleNotice", payload |-> [new_state |-> "Queued", work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedSteer"], [machine |-> "runtime_ingress", variant |-> "WakeRuntime", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedSteer"], [machine |-> "runtime_ingress", variant |-> "RequestImmediateProcessing", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedSteer"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "AdmitQueuedSteer", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_AdmitConsumedOnAccept(arg_work_id, arg_content_shape, arg_request_id, arg_reservation_key, arg_policy) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "AdmitConsumedOnAccept"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.content_shape = arg_content_shape
       /\ packet.payload.request_id = arg_request_id
       /\ packet.payload.reservation_key = arg_reservation_key
       /\ packet.payload.policy = arg_policy
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ ~((packet.payload.work_id \in runtime_ingress_admitted_inputs))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_admitted_inputs' = (runtime_ingress_admitted_inputs \cup {packet.payload.work_id})
       /\ runtime_ingress_admission_order' = Append(runtime_ingress_admission_order, packet.payload.work_id)
       /\ runtime_ingress_content_shape' = MapSet(runtime_ingress_content_shape, packet.payload.work_id, packet.payload.content_shape)
       /\ runtime_ingress_request_id' = MapSet(runtime_ingress_request_id, packet.payload.work_id, packet.payload.request_id)
       /\ runtime_ingress_reservation_key' = MapSet(runtime_ingress_reservation_key, packet.payload.work_id, packet.payload.reservation_key)
       /\ runtime_ingress_policy_snapshot' = MapSet(runtime_ingress_policy_snapshot, packet.payload.work_id, packet.payload.policy)
       /\ runtime_ingress_lifecycle' = MapSet(runtime_ingress_lifecycle, packet.payload.work_id, "Consumed")
       /\ runtime_ingress_terminal_outcome' = MapSet(runtime_ingress_terminal_outcome, packet.payload.work_id, Some("Consumed"))
       /\ runtime_ingress_last_run' = MapSet(runtime_ingress_last_run, packet.payload.work_id, None)
       /\ runtime_ingress_last_boundary_sequence' = MapSet(runtime_ingress_last_boundary_sequence, packet.payload.work_id, None)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_handling_mode, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressAccepted", payload |-> [work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitConsumedOnAccept"], [machine |-> "runtime_ingress", variant |-> "InputLifecycleNotice", payload |-> [new_state |-> "Consumed", work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitConsumedOnAccept"], [machine |-> "runtime_ingress", variant |-> "CompletionResolved", payload |-> [outcome |-> "Consumed", work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitConsumedOnAccept"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "AdmitConsumedOnAccept", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_StageDrainSnapshot(arg_run_id, arg_contributing_work_ids) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "StageDrainSnapshot"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.contributing_work_ids = arg_contributing_work_ids
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active" \/ runtime_ingress_phase = "Retired"
       /\ (runtime_ingress_current_run = None)
       /\ (Len(packet.payload.contributing_work_ids) > 0)
       /\ (((Len(runtime_ingress_steer_queue) > 0) /\ StartsWith(runtime_ingress_steer_queue, packet.payload.contributing_work_ids)) \/ ((Len(runtime_ingress_steer_queue) = 0) /\ StartsWith(runtime_ingress_queue, packet.payload.contributing_work_ids)))
       /\ (\A work_id \in SeqElements(packet.payload.contributing_work_ids) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "Queued"))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_StageDrainSnapshot_ForEach0_lifecycle(runtime_ingress_lifecycle, packet.payload.contributing_work_ids, packet.payload.run_id)
       /\ runtime_ingress_queue' = IF (Len(runtime_ingress_steer_queue) > 0) THEN runtime_ingress_queue ELSE SeqRemoveAll(runtime_ingress_queue, packet.payload.contributing_work_ids)
       /\ runtime_ingress_steer_queue' = IF (Len(runtime_ingress_steer_queue) > 0) THEN SeqRemoveAll(runtime_ingress_steer_queue, packet.payload.contributing_work_ids) ELSE runtime_ingress_steer_queue
       /\ runtime_ingress_current_run' = Some(packet.payload.run_id)
       /\ runtime_ingress_current_run_contributors' = packet.payload.contributing_work_ids
       /\ runtime_ingress_last_run' = runtime_ingress_StageDrainSnapshot_ForEach0_last_run(runtime_ingress_last_run, packet.payload.contributing_work_ids, packet.payload.run_id)
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_last_boundary_sequence, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "ReadyForRun", payload |-> [contributing_work_ids |-> packet.payload.contributing_work_ids, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "StageDrainSnapshot"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "StageDrainSnapshot", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_BoundaryApplied(arg_run_id, arg_boundary_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "BoundaryApplied"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.boundary_sequence = arg_boundary_sequence
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active" \/ runtime_ingress_phase = "Retired"
       /\ (runtime_ingress_current_run = Some(packet.payload.run_id))
       /\ (\A work_id \in SeqElements(runtime_ingress_current_run_contributors) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "Staged"))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_BoundaryApplied_ForEach1_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, packet.payload.boundary_sequence)
       /\ runtime_ingress_last_boundary_sequence' = runtime_ingress_BoundaryApplied_ForEach1_last_boundary_sequence(runtime_ingress_last_boundary_sequence, runtime_ingress_current_run_contributors, packet.payload.boundary_sequence)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "ContributorsPendingConsumption", kind |-> "BoundaryApplied"], effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "BoundaryApplied", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_RunCompleted(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "RunCompleted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active" \/ runtime_ingress_phase = "Retired"
       /\ (runtime_ingress_current_run = Some(packet.payload.run_id))
       /\ (\A work_id \in SeqElements(runtime_ingress_current_run_contributors) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "AppliedPendingConsumption"))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RunCompleted_ForEach2_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_RunCompleted_ForEach2_terminal_outcome(runtime_ingress_terminal_outcome, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "ContributorsConsumed", kind |-> "RunCompleted"], effect_id |-> (model_step_count + 1), source_transition |-> "RunCompleted"], [machine |-> "runtime_ingress", variant |-> "CompletionResolved", payload |-> [outcome |-> "Consumed", work_id |-> Head(<<>>)], effect_id |-> (model_step_count + 1), source_transition |-> "RunCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RunCompleted", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_RunFailed(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "RunFailed"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active" \/ runtime_ingress_phase = "Retired"
       /\ (runtime_ingress_current_run = Some(packet.payload.run_id))
       /\ (\A work_id \in SeqElements(runtime_ingress_current_run_contributors) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "Staged"))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RunFailed_ForEach3_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN (runtime_ingress_current_run_contributors \o runtime_ingress_queue) ELSE runtime_ingress_queue
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN TRUE ELSE runtime_ingress_wake_requested
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_steer_queue, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "ContributorsRequeued", kind |-> "RunFailed"], effect_id |-> (model_step_count + 1), source_transition |-> "RunFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RunFailed", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_RunCancelled(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "RunCancelled"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active" \/ runtime_ingress_phase = "Retired"
       /\ (runtime_ingress_current_run = Some(packet.payload.run_id))
       /\ (\A work_id \in SeqElements(runtime_ingress_current_run_contributors) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "Staged"))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RunCancelled_ForEach4_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN (runtime_ingress_current_run_contributors \o runtime_ingress_queue) ELSE runtime_ingress_queue
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN TRUE ELSE runtime_ingress_wake_requested
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_steer_queue, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "ContributorsRequeued", kind |-> "RunCancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "RunCancelled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RunCancelled", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_SupersedeQueuedInput(arg_new_work_id, arg_old_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "SupersedeQueuedInput"
       /\ packet.payload.new_work_id = arg_new_work_id
       /\ packet.payload.old_work_id = arg_old_work_id
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ (packet.payload.new_work_id \in runtime_ingress_admitted_inputs)
       /\ ((IF packet.payload.old_work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[packet.payload.old_work_id] ELSE "None") = "Queued")
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = MapSet(runtime_ingress_lifecycle, packet.payload.old_work_id, "Superseded")
       /\ runtime_ingress_terminal_outcome' = MapSet(runtime_ingress_terminal_outcome, packet.payload.old_work_id, Some("Superseded"))
       /\ runtime_ingress_queue' = SeqRemove(runtime_ingress_queue, packet.payload.old_work_id)
       /\ runtime_ingress_steer_queue' = SeqRemove(runtime_ingress_steer_queue, packet.payload.old_work_id)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "QueuedInputSuperseded", kind |-> "SupersedeQueuedInput"], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedeQueuedInput"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "SupersedeQueuedInput", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_CoalesceQueuedInputs(arg_aggregate_work_id, arg_source_work_ids) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "CoalesceQueuedInputs"
       /\ packet.payload.aggregate_work_id = arg_aggregate_work_id
       /\ packet.payload.source_work_ids = arg_source_work_ids
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ (packet.payload.aggregate_work_id \in runtime_ingress_admitted_inputs)
       /\ (Len(packet.payload.source_work_ids) > 0)
       /\ (\A work_id \in SeqElements(packet.payload.source_work_ids) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "Queued"))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_CoalesceQueuedInputs_ForEach5_lifecycle(runtime_ingress_lifecycle, packet.payload.source_work_ids)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_CoalesceQueuedInputs_ForEach5_terminal_outcome(runtime_ingress_terminal_outcome, packet.payload.source_work_ids)
       /\ runtime_ingress_queue' = SeqRemoveAll(runtime_ingress_queue, packet.payload.source_work_ids)
       /\ runtime_ingress_steer_queue' = SeqRemoveAll(runtime_ingress_steer_queue, packet.payload.source_work_ids)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "SourcesCoalescedIntoAggregate", kind |-> "CoalesceQueuedInputs"], effect_id |-> (model_step_count + 1), source_transition |-> "CoalesceQueuedInputs"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "CoalesceQueuedInputs", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_Retire ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "Retire"
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ runtime_ingress_phase' = "Retired"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "QueuePreserved", kind |-> "Retire"], effect_id |-> (model_step_count + 1), source_transition |-> "Retire"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "Retire", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_ResetFromActive ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "Reset"
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_ResetFromActive_ForEach8_lifecycle(runtime_ingress_ResetFromActive_ForEach7_lifecycle(runtime_ingress_ResetFromActive_ForEach6_lifecycle(runtime_ingress_lifecycle, runtime_ingress_queue), runtime_ingress_steer_queue), runtime_ingress_current_run_contributors)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_ResetFromActive_ForEach8_terminal_outcome(runtime_ingress_ResetFromActive_ForEach7_terminal_outcome(runtime_ingress_ResetFromActive_ForEach6_terminal_outcome(runtime_ingress_terminal_outcome, runtime_ingress_queue), runtime_ingress_steer_queue), runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = <<>>
       /\ runtime_ingress_steer_queue' = <<>>
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "NonTerminalInputsAbandoned", kind |-> "Reset"], effect_id |-> (model_step_count + 1), source_transition |-> "ResetFromActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "ResetFromActive", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_ResetFromRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "Reset"
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Retired"
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_ResetFromRetired_ForEach11_lifecycle(runtime_ingress_ResetFromRetired_ForEach10_lifecycle(runtime_ingress_ResetFromRetired_ForEach9_lifecycle(runtime_ingress_lifecycle, runtime_ingress_queue), runtime_ingress_steer_queue), runtime_ingress_current_run_contributors)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_ResetFromRetired_ForEach11_terminal_outcome(runtime_ingress_ResetFromRetired_ForEach10_terminal_outcome(runtime_ingress_ResetFromRetired_ForEach9_terminal_outcome(runtime_ingress_terminal_outcome, runtime_ingress_queue), runtime_ingress_steer_queue), runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = <<>>
       /\ runtime_ingress_steer_queue' = <<>>
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "NonTerminalInputsAbandoned", kind |-> "Reset"], effect_id |-> (model_step_count + 1), source_transition |-> "ResetFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "ResetFromRetired", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_Destroy ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "Destroy"
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active" \/ runtime_ingress_phase = "Retired"
       /\ runtime_ingress_phase' = "Destroyed"
       /\ runtime_ingress_lifecycle' = runtime_ingress_Destroy_ForEach14_lifecycle(runtime_ingress_Destroy_ForEach13_lifecycle(runtime_ingress_Destroy_ForEach12_lifecycle(runtime_ingress_lifecycle, runtime_ingress_queue), runtime_ingress_steer_queue), runtime_ingress_current_run_contributors)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_Destroy_ForEach14_terminal_outcome(runtime_ingress_Destroy_ForEach13_terminal_outcome(runtime_ingress_Destroy_ForEach12_terminal_outcome(runtime_ingress_terminal_outcome, runtime_ingress_queue), runtime_ingress_steer_queue), runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = <<>>
       /\ runtime_ingress_steer_queue' = <<>>
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "IngressDestroyed", kind |-> "Destroy"], effect_id |-> (model_step_count + 1), source_transition |-> "Destroy"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "Destroy", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_RecoverFromActive ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RecoverFromActive_ForEach15_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN (runtime_ingress_current_run_contributors \o runtime_ingress_queue) ELSE runtime_ingress_queue
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN TRUE ELSE runtime_ingress_wake_requested
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_steer_queue, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "RecoveryAppliedToCurrentRun", kind |-> "Recover"], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverFromActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RecoverFromActive", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_RecoverFromRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Retired"
       /\ runtime_ingress_phase' = "Retired"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RecoverFromRetired_ForEach16_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN (runtime_ingress_current_run_contributors \o runtime_ingress_queue) ELSE runtime_ingress_queue
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN TRUE ELSE runtime_ingress_wake_requested
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_steer_queue, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "RecoveryAppliedToCurrentRun", kind |-> "Recover"], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RecoverFromRetired", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_SetSilentIntentOverridesFromActive(arg_intents) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "SetSilentIntentOverrides"
       /\ packet.payload.intents = arg_intents
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_silent_intent_overrides' = packet.payload.intents
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "Updated", kind |-> "SilentIntentOverrides"], effect_id |-> (model_step_count + 1), source_transition |-> "SetSilentIntentOverridesFromActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "SetSilentIntentOverridesFromActive", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_SetSilentIntentOverridesFromRetired(arg_intents) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "SetSilentIntentOverrides"
       /\ packet.payload.intents = arg_intents
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Retired"
       /\ runtime_ingress_phase' = "Retired"
       /\ runtime_ingress_silent_intent_overrides' = packet.payload.intents
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "Updated", kind |-> "SilentIntentOverrides"], effect_id |-> (model_step_count + 1), source_transition |-> "SetSilentIntentOverridesFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "SetSilentIntentOverridesFromRetired", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_queue_entries_are_queued == (\A work_id \in SeqElements(runtime_ingress_queue) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "Queued"))
runtime_ingress_steer_entries_are_queued == (\A work_id \in SeqElements(runtime_ingress_steer_queue) : (work_id \in DOMAIN runtime_ingress_lifecycle))
runtime_ingress_pending_inputs_preserve_content_shape == ((\A work_id \in SeqElements(runtime_ingress_queue) : (work_id \in DOMAIN runtime_ingress_content_shape)) /\ (\A work_id \in SeqElements(runtime_ingress_steer_queue) : (work_id \in DOMAIN runtime_ingress_content_shape)))
runtime_ingress_admitted_inputs_preserve_correlation_slots == (\A work_id \in runtime_ingress_admitted_inputs : ((work_id \in DOMAIN runtime_ingress_request_id) /\ (work_id \in DOMAIN runtime_ingress_reservation_key)))
runtime_ingress_queue_entries_preserve_handling_mode == (\A work_id \in SeqElements(runtime_ingress_queue) : ((IF work_id \in DOMAIN runtime_ingress_handling_mode THEN runtime_ingress_handling_mode[work_id] ELSE "None") = "Queue"))
runtime_ingress_steer_entries_preserve_handling_mode == (\A work_id \in SeqElements(runtime_ingress_steer_queue) : ((IF work_id \in DOMAIN runtime_ingress_handling_mode THEN runtime_ingress_handling_mode[work_id] ELSE "None") = "Steer"))
runtime_ingress_pending_queues_do_not_overlap == (\A work_id \in SeqElements(runtime_ingress_steer_queue) : ~((work_id \in SeqElements(runtime_ingress_queue))))
runtime_ingress_terminal_inputs_do_not_appear_in_queue == ((\A work_id \in SeqElements(runtime_ingress_queue) : (((IF work_id \in DOMAIN runtime_ingress_terminal_outcome THEN runtime_ingress_terminal_outcome[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") # "Abandoned"))) /\ (\A work_id \in SeqElements(runtime_ingress_steer_queue) : (((IF work_id \in DOMAIN runtime_ingress_terminal_outcome THEN runtime_ingress_terminal_outcome[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") # "Abandoned"))))
runtime_ingress_current_run_matches_contributor_presence == ((runtime_ingress_current_run = None) = (Len(runtime_ingress_current_run_contributors) = 0))
runtime_ingress_staged_contributors_are_not_queued == (\A work_id \in SeqElements(runtime_ingress_current_run_contributors) : (~((work_id \in SeqElements(runtime_ingress_queue))) /\ ~((work_id \in SeqElements(runtime_ingress_steer_queue)))))
runtime_ingress_applied_pending_consumption_has_last_run == (\A work_id \in runtime_ingress_admitted_inputs : (((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") # "AppliedPendingConsumption") \/ ((IF work_id \in DOMAIN runtime_ingress_last_run THEN runtime_ingress_last_run[work_id] ELSE None) # None)))

Inject_control_initialize ==
    /\ ~([machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "control_initialize", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "control_initialize", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "control_initialize", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_trust_peer(arg_peer_id) ==
    /\ ~([machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> arg_peer_id], source_kind |-> "entry", source_route |-> "trust_peer", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> arg_peer_id], source_kind |-> "entry", source_route |-> "trust_peer", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> arg_peer_id], source_kind |-> "entry", source_route |-> "trust_peer", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_receive_peer_envelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key) ==
    /\ ~([machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [content_shape |-> arg_content_shape, peer_id |-> arg_peer_id, raw_item_id |-> arg_raw_item_id, raw_kind |-> arg_raw_kind, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, text_projection |-> arg_text_projection], source_kind |-> "entry", source_route |-> "receive_peer_envelope", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [content_shape |-> arg_content_shape, peer_id |-> arg_peer_id, raw_item_id |-> arg_raw_item_id, raw_kind |-> arg_raw_kind, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, text_projection |-> arg_text_projection], source_kind |-> "entry", source_route |-> "receive_peer_envelope", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [content_shape |-> arg_content_shape, peer_id |-> arg_peer_id, raw_item_id |-> arg_raw_item_id, raw_kind |-> arg_raw_kind, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, text_projection |-> arg_text_projection], source_kind |-> "entry", source_route |-> "receive_peer_envelope", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_submit_typed_peer_input(arg_raw_item_id) ==
    /\ ~([machine |-> "peer_comms", variant |-> "SubmitTypedPeerInput", payload |-> [raw_item_id |-> arg_raw_item_id], source_kind |-> "entry", source_route |-> "submit_typed_peer_input", source_machine |-> "external_entry", source_effect |-> "SubmitTypedPeerInput", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "peer_comms", variant |-> "SubmitTypedPeerInput", payload |-> [raw_item_id |-> arg_raw_item_id], source_kind |-> "entry", source_route |-> "submit_typed_peer_input", source_machine |-> "external_entry", source_effect |-> "SubmitTypedPeerInput", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "peer_comms", variant |-> "SubmitTypedPeerInput", payload |-> [raw_item_id |-> arg_raw_item_id], source_kind |-> "entry", source_route |-> "submit_typed_peer_input", source_machine |-> "external_entry", source_effect |-> "SubmitTypedPeerInput", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_runtime_admission_accepted(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect) ==
    /\ ~([machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> arg_admission_effect, content_shape |-> arg_content_shape, handling_mode |-> arg_handling_mode, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "runtime_admission_accepted", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> arg_admission_effect, content_shape |-> arg_content_shape, handling_mode |-> arg_handling_mode, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "runtime_admission_accepted", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> arg_admission_effect, content_shape |-> arg_content_shape, handling_mode |-> arg_handling_mode, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "runtime_admission_accepted", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_ingress_stage_drain_snapshot(arg_run_id, arg_contributing_work_ids) ==
    /\ ~([machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_work_ids |-> arg_contributing_work_ids, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "ingress_stage_drain_snapshot", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_work_ids |-> arg_contributing_work_ids, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "ingress_stage_drain_snapshot", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_work_ids |-> arg_contributing_work_ids, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "ingress_stage_drain_snapshot", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_trusted_peer_enters_runtime ==
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
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_admitted_peer_work_enters_ingress ==
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
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_control_preempts_peer_delivery ==
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
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_text_projection, peer_comms_content_shape, peer_comms_request_id, peer_comms_reservation_key, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

CoreNext ==
    \/ DeliverQueuedRoute
    \/ \E arg_peer_id \in PeerIdValues : peer_comms_TrustPeer(arg_peer_id)
    \/ \E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : peer_comms_ReceiveTrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)
    \/ \E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : peer_comms_DropUntrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)
    \/ \E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputDelivered(arg_raw_item_id)
    \/ \E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputContinue(arg_raw_item_id)
    \/ runtime_control_Initialize
    \/ \E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunCompleted(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunFailed(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunCancelled(arg_run_id)
    \/ runtime_control_RecoverRequestedFromIdle
    \/ runtime_control_RecoverRequestedFromRunning
    \/ runtime_control_RecoverySucceeded
    \/ runtime_control_RetireRequestedFromIdle
    \/ runtime_control_RetireRequestedFromRunning
    \/ runtime_control_ResetRequested
    \/ runtime_control_StopRequested
    \/ runtime_control_DestroyRequested
    \/ runtime_control_ResumeRequested
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)
    \/ \E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)
    \/ \E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)
    \/ \E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)
    \/ \E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)
    \/ runtime_control_ExternalToolDeltaReceivedIdle
    \/ runtime_control_ExternalToolDeltaReceivedRunning
    \/ runtime_control_ExternalToolDeltaReceivedRecovering
    \/ runtime_control_ExternalToolDeltaReceivedRetired
    \/ runtime_control_RecycleRequestedFromRetired
    \/ runtime_control_RecycleRequestedFromIdle
    \/ runtime_control_RecycleSucceeded
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_work_id, arg_content_shape, arg_request_id, arg_reservation_key, arg_policy)
    \/ \E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshot(arg_run_id, arg_contributing_work_ids)
    \/ \E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryApplied(arg_run_id, arg_boundary_sequence)
    \/ \E arg_run_id \in RunIdValues : runtime_ingress_RunCompleted(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_ingress_RunFailed(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_ingress_RunCancelled(arg_run_id)
    \/ \E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInput(arg_new_work_id, arg_old_work_id)
    \/ \E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputs(arg_aggregate_work_id, arg_source_work_ids)
    \/ runtime_ingress_Retire
    \/ runtime_ingress_ResetFromActive
    \/ runtime_ingress_ResetFromRetired
    \/ runtime_ingress_Destroy
    \/ runtime_ingress_RecoverFromActive
    \/ runtime_ingress_RecoverFromRetired
    \/ \E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromActive(arg_intents)
    \/ \E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromRetired(arg_intents)
    \/ QuiescentStutter

InjectNext ==
    \/ Inject_control_initialize
    \/ \E arg_peer_id \in PeerIdValues : Inject_trust_peer(arg_peer_id)
    \/ \E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : Inject_receive_peer_envelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)
    \/ \E arg_raw_item_id \in RawItemIdValues : Inject_submit_typed_peer_input(arg_raw_item_id)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : Inject_runtime_admission_accepted(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)
    \/ \E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : Inject_ingress_stage_drain_snapshot(arg_run_id, arg_contributing_work_ids)

Next ==
    \/ CoreNext
    \/ InjectNext

WitnessNext_trusted_peer_enters_runtime ==
    \/ CoreNext
    \/ WitnessInjectNext_trusted_peer_enters_runtime

WitnessNext_admitted_peer_work_enters_ingress ==
    \/ CoreNext
    \/ WitnessInjectNext_admitted_peer_work_enters_ingress

WitnessNext_control_preempts_peer_delivery ==
    \/ CoreNext
    \/ WitnessInjectNext_control_preempts_peer_delivery


peer_work_enters_runtime_via_canonical_admission == \A input_packet \in observed_inputs : ((input_packet.machine = "runtime_control" /\ input_packet.variant = "SubmitWork") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "peer_comms" /\ input_packet.source_effect = "SubmitPeerInputCandidate" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "peer_comms" /\ effect_packet.variant = "SubmitPeerInputCandidate" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = input_packet.source_route /\ route_packet.source_machine = "peer_comms" /\ route_packet.effect = "SubmitPeerInputCandidate" /\ route_packet.target_machine = "runtime_control" /\ route_packet.target_input = "SubmitWork" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))
peer_admission_flows_into_ingress == \A input_packet \in observed_inputs : ((input_packet.machine = "runtime_ingress" /\ input_packet.variant = "AdmitQueued") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "runtime_control" /\ input_packet.source_effect = "SubmitAdmittedIngressEffect" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "runtime_control" /\ effect_packet.variant = "SubmitAdmittedIngressEffect" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = input_packet.source_route /\ route_packet.source_machine = "runtime_control" /\ route_packet.effect = "SubmitAdmittedIngressEffect" /\ route_packet.target_machine = "runtime_ingress" /\ route_packet.target_input = "AdmitQueued" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))
control_preempts_peer_delivery == <<"PreemptWhenReady", "control_plane", "peer_plane">> \in SchedulerRules

RouteObserved_peer_candidate_enters_runtime_admission == \E packet \in RoutePackets : packet.route = "peer_candidate_enters_runtime_admission"
RouteCoverage_peer_candidate_enters_runtime_admission == (RouteObserved_peer_candidate_enters_runtime_admission \/ ~RouteObserved_peer_candidate_enters_runtime_admission)
RouteObserved_admitted_peer_work_enters_ingress == \E packet \in RoutePackets : packet.route = "admitted_peer_work_enters_ingress"
RouteCoverage_admitted_peer_work_enters_ingress == (RouteObserved_admitted_peer_work_enters_ingress \/ ~RouteObserved_admitted_peer_work_enters_ingress)
SchedulerTriggered_PreemptWhenReady_control_plane_peer_plane == /\ "control_plane" \in PendingActors /\ "peer_plane" \in PendingActors
SchedulerCoverage_PreemptWhenReady_control_plane_peer_plane == (SchedulerTriggered_PreemptWhenReady_control_plane_peer_plane \/ ~SchedulerTriggered_PreemptWhenReady_control_plane_peer_plane)
CoverageInstrumentation == RouteCoverage_peer_candidate_enters_runtime_admission /\ RouteCoverage_admitted_peer_work_enters_ingress /\ SchedulerCoverage_PreemptWhenReady_control_plane_peer_plane

CiStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 1 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(peer_comms_trusted_peers) <= 1 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 1 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 1 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 1 /\ Cardinality(DOMAIN peer_comms_text_projection) <= 1 /\ Cardinality(DOMAIN peer_comms_content_shape) <= 1 /\ Cardinality(DOMAIN peer_comms_request_id) <= 1 /\ Cardinality(DOMAIN peer_comms_reservation_key) <= 1 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 1 /\ Len(peer_comms_submission_queue) <= 1 /\ Cardinality(runtime_ingress_admitted_inputs) <= 1 /\ Len(runtime_ingress_admission_order) <= 1 /\ Cardinality(DOMAIN runtime_ingress_content_shape) <= 1 /\ Cardinality(DOMAIN runtime_ingress_request_id) <= 1 /\ Cardinality(DOMAIN runtime_ingress_reservation_key) <= 1 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 1 /\ Cardinality(DOMAIN runtime_ingress_handling_mode) <= 1 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 1 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 1 /\ Len(runtime_ingress_queue) <= 1 /\ Len(runtime_ingress_steer_queue) <= 1 /\ Len(runtime_ingress_current_run_contributors) <= 1 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 1 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 1 /\ Cardinality(runtime_ingress_silent_intent_overrides) <= 1
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(peer_comms_trusted_peers) <= 2 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 2 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 2 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 2 /\ Cardinality(DOMAIN peer_comms_text_projection) <= 2 /\ Cardinality(DOMAIN peer_comms_content_shape) <= 2 /\ Cardinality(DOMAIN peer_comms_request_id) <= 2 /\ Cardinality(DOMAIN peer_comms_reservation_key) <= 2 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 2 /\ Len(peer_comms_submission_queue) <= 2 /\ Cardinality(runtime_ingress_admitted_inputs) <= 2 /\ Len(runtime_ingress_admission_order) <= 2 /\ Cardinality(DOMAIN runtime_ingress_content_shape) <= 2 /\ Cardinality(DOMAIN runtime_ingress_request_id) <= 2 /\ Cardinality(DOMAIN runtime_ingress_reservation_key) <= 2 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 2 /\ Cardinality(DOMAIN runtime_ingress_handling_mode) <= 2 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 2 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 2 /\ Len(runtime_ingress_queue) <= 2 /\ Len(runtime_ingress_steer_queue) <= 2 /\ Len(runtime_ingress_current_run_contributors) <= 2 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 2 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 2 /\ Cardinality(runtime_ingress_silent_intent_overrides) <= 2
WitnessStateConstraint_trusted_peer_enters_runtime == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 4 /\ Cardinality(observed_inputs) <= 8 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 4 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(peer_comms_trusted_peers) <= 4 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 4 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 4 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 4 /\ Cardinality(DOMAIN peer_comms_text_projection) <= 4 /\ Cardinality(DOMAIN peer_comms_content_shape) <= 4 /\ Cardinality(DOMAIN peer_comms_request_id) <= 4 /\ Cardinality(DOMAIN peer_comms_reservation_key) <= 4 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 4 /\ Len(peer_comms_submission_queue) <= 4 /\ Cardinality(runtime_ingress_admitted_inputs) <= 4 /\ Len(runtime_ingress_admission_order) <= 4 /\ Cardinality(DOMAIN runtime_ingress_content_shape) <= 4 /\ Cardinality(DOMAIN runtime_ingress_request_id) <= 4 /\ Cardinality(DOMAIN runtime_ingress_reservation_key) <= 4 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 4 /\ Cardinality(DOMAIN runtime_ingress_handling_mode) <= 4 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 4 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 4 /\ Len(runtime_ingress_queue) <= 4 /\ Len(runtime_ingress_steer_queue) <= 4 /\ Len(runtime_ingress_current_run_contributors) <= 4 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 4 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 4 /\ Cardinality(runtime_ingress_silent_intent_overrides) <= 4
WitnessStateConstraint_admitted_peer_work_enters_ingress == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 5 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 3 /\ Cardinality(emitted_effects) <= 3 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(peer_comms_trusted_peers) <= 5 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 5 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 5 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 5 /\ Cardinality(DOMAIN peer_comms_text_projection) <= 5 /\ Cardinality(DOMAIN peer_comms_content_shape) <= 5 /\ Cardinality(DOMAIN peer_comms_request_id) <= 5 /\ Cardinality(DOMAIN peer_comms_reservation_key) <= 5 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 5 /\ Len(peer_comms_submission_queue) <= 5 /\ Cardinality(runtime_ingress_admitted_inputs) <= 5 /\ Len(runtime_ingress_admission_order) <= 5 /\ Cardinality(DOMAIN runtime_ingress_content_shape) <= 5 /\ Cardinality(DOMAIN runtime_ingress_request_id) <= 5 /\ Cardinality(DOMAIN runtime_ingress_reservation_key) <= 5 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 5 /\ Cardinality(DOMAIN runtime_ingress_handling_mode) <= 5 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 5 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 5 /\ Len(runtime_ingress_queue) <= 5 /\ Len(runtime_ingress_steer_queue) <= 5 /\ Len(runtime_ingress_current_run_contributors) <= 5 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 5 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 5 /\ Cardinality(runtime_ingress_silent_intent_overrides) <= 5
WitnessStateConstraint_control_preempts_peer_delivery == /\ model_step_count <= 5 /\ Len(pending_inputs) <= 3 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 5 /\ Cardinality(peer_comms_trusted_peers) <= 3 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 3 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 3 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 3 /\ Cardinality(DOMAIN peer_comms_text_projection) <= 3 /\ Cardinality(DOMAIN peer_comms_content_shape) <= 3 /\ Cardinality(DOMAIN peer_comms_request_id) <= 3 /\ Cardinality(DOMAIN peer_comms_reservation_key) <= 3 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 3 /\ Len(peer_comms_submission_queue) <= 3 /\ Cardinality(runtime_ingress_admitted_inputs) <= 3 /\ Len(runtime_ingress_admission_order) <= 3 /\ Cardinality(DOMAIN runtime_ingress_content_shape) <= 3 /\ Cardinality(DOMAIN runtime_ingress_request_id) <= 3 /\ Cardinality(DOMAIN runtime_ingress_reservation_key) <= 3 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 3 /\ Cardinality(DOMAIN runtime_ingress_handling_mode) <= 3 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 3 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 3 /\ Len(runtime_ingress_queue) <= 3 /\ Len(runtime_ingress_steer_queue) <= 3 /\ Len(runtime_ingress_current_run_contributors) <= 3 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 3 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 3 /\ Cardinality(runtime_ingress_silent_intent_overrides) <= 3

Spec == Init /\ [][Next]_vars
WitnessSpec_trusted_peer_enters_runtime == WitnessInit_trusted_peer_enters_runtime /\ [] [WitnessNext_trusted_peer_enters_runtime]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_peer_id \in PeerIdValues : peer_comms_TrustPeer(arg_peer_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : peer_comms_ReceiveTrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : peer_comms_DropUntrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputDelivered(arg_raw_item_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputContinue(arg_raw_item_id)) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelled(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(runtime_control_RecycleRequestedFromRetired) /\ WF_vars(runtime_control_RecycleRequestedFromIdle) /\ WF_vars(runtime_control_RecycleSucceeded) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_work_id, arg_content_shape, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshot(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryApplied(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelled(arg_run_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInput(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputs(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(runtime_ingress_Retire) /\ WF_vars(runtime_ingress_ResetFromActive) /\ WF_vars(runtime_ingress_ResetFromRetired) /\ WF_vars(runtime_ingress_Destroy) /\ WF_vars(runtime_ingress_RecoverFromActive) /\ WF_vars(runtime_ingress_RecoverFromRetired) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromActive(arg_intents)) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromRetired(arg_intents)) /\ WF_vars(WitnessInjectNext_trusted_peer_enters_runtime)
WitnessSpec_admitted_peer_work_enters_ingress == WitnessInit_admitted_peer_work_enters_ingress /\ [] [WitnessNext_admitted_peer_work_enters_ingress]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_peer_id \in PeerIdValues : peer_comms_TrustPeer(arg_peer_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : peer_comms_ReceiveTrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : peer_comms_DropUntrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputDelivered(arg_raw_item_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputContinue(arg_raw_item_id)) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelled(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(runtime_control_RecycleRequestedFromRetired) /\ WF_vars(runtime_control_RecycleRequestedFromIdle) /\ WF_vars(runtime_control_RecycleSucceeded) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_work_id, arg_content_shape, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshot(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryApplied(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelled(arg_run_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInput(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputs(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(runtime_ingress_Retire) /\ WF_vars(runtime_ingress_ResetFromActive) /\ WF_vars(runtime_ingress_ResetFromRetired) /\ WF_vars(runtime_ingress_Destroy) /\ WF_vars(runtime_ingress_RecoverFromActive) /\ WF_vars(runtime_ingress_RecoverFromRetired) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromActive(arg_intents)) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromRetired(arg_intents)) /\ WF_vars(WitnessInjectNext_admitted_peer_work_enters_ingress)
WitnessSpec_control_preempts_peer_delivery == WitnessInit_control_preempts_peer_delivery /\ [] [WitnessNext_control_preempts_peer_delivery]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_peer_id \in PeerIdValues : peer_comms_TrustPeer(arg_peer_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : peer_comms_ReceiveTrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : \E arg_text_projection \in {"alpha", "beta"} : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : peer_comms_DropUntrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind, arg_text_projection, arg_content_shape, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputDelivered(arg_raw_item_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputContinue(arg_raw_item_id)) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelled(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(runtime_control_RecycleRequestedFromRetired) /\ WF_vars(runtime_control_RecycleRequestedFromIdle) /\ WF_vars(runtime_control_RecycleSucceeded) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_work_id, arg_content_shape, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshot(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryApplied(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelled(arg_run_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInput(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputs(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(runtime_ingress_Retire) /\ WF_vars(runtime_ingress_ResetFromActive) /\ WF_vars(runtime_ingress_ResetFromRetired) /\ WF_vars(runtime_ingress_Destroy) /\ WF_vars(runtime_ingress_RecoverFromActive) /\ WF_vars(runtime_ingress_RecoverFromRetired) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromActive(arg_intents)) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromRetired(arg_intents)) /\ WF_vars(WitnessInjectNext_control_preempts_peer_delivery)

WitnessRouteObserved_trusted_peer_enters_runtime_peer_candidate_enters_runtime_admission == <> RouteObserved_peer_candidate_enters_runtime_admission
WitnessStateObserved_trusted_peer_enters_runtime_1 == <> (peer_comms_phase = "Delivered")
WitnessStateObserved_trusted_peer_enters_runtime_2 == <> (runtime_control_phase = "Idle")
WitnessTransitionObserved_trusted_peer_enters_runtime_runtime_control_Initialize == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "Initialize")
WitnessTransitionObserved_trusted_peer_enters_runtime_peer_comms_TrustPeer == <> (\E packet \in observed_transitions : /\ packet.machine = "peer_comms" /\ packet.transition = "TrustPeer")
WitnessTransitionObserved_trusted_peer_enters_runtime_peer_comms_ReceiveTrustedPeerEnvelope == <> (\E packet \in observed_transitions : /\ packet.machine = "peer_comms" /\ packet.transition = "ReceiveTrustedPeerEnvelope")
WitnessTransitionObserved_trusted_peer_enters_runtime_peer_comms_SubmitTypedPeerInputDelivered == <> (\E packet \in observed_transitions : /\ packet.machine = "peer_comms" /\ packet.transition = "SubmitTypedPeerInputDelivered")
WitnessTransitionOrder_trusted_peer_enters_runtime_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "peer_comms" /\ earlier.transition = "ReceiveTrustedPeerEnvelope" /\ later.machine = "peer_comms" /\ later.transition = "SubmitTypedPeerInputDelivered" /\ earlier.step < later.step)
WitnessRouteObserved_admitted_peer_work_enters_ingress_peer_candidate_enters_runtime_admission == <> RouteObserved_peer_candidate_enters_runtime_admission
WitnessRouteObserved_admitted_peer_work_enters_ingress_admitted_peer_work_enters_ingress == <> RouteObserved_admitted_peer_work_enters_ingress
WitnessStateObserved_admitted_peer_work_enters_ingress_1 == <> (runtime_control_phase = "Idle")
WitnessStateObserved_admitted_peer_work_enters_ingress_2 == <> (runtime_ingress_phase = "Active")
WitnessTransitionObserved_admitted_peer_work_enters_ingress_peer_comms_SubmitTypedPeerInputDelivered == <> (\E packet \in observed_transitions : /\ packet.machine = "peer_comms" /\ packet.transition = "SubmitTypedPeerInputDelivered")
WitnessTransitionObserved_admitted_peer_work_enters_ingress_runtime_control_AdmissionAcceptedIdleSteer == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "AdmissionAcceptedIdleSteer")
WitnessTransitionObserved_admitted_peer_work_enters_ingress_runtime_ingress_AdmitQueuedSteer == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "AdmitQueuedSteer")
WitnessTransitionOrder_admitted_peer_work_enters_ingress_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "peer_comms" /\ earlier.transition = "SubmitTypedPeerInputDelivered" /\ later.machine = "runtime_control" /\ later.transition = "AdmissionAcceptedIdleSteer" /\ earlier.step < later.step)
WitnessTransitionOrder_admitted_peer_work_enters_ingress_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "AdmissionAcceptedIdleSteer" /\ later.machine = "runtime_ingress" /\ later.transition = "AdmitQueuedSteer" /\ earlier.step < later.step)
WitnessSchedulerTriggered_control_preempts_peer_delivery_PreemptWhenReady_control_plane_peer_plane == <> SchedulerTriggered_PreemptWhenReady_control_plane_peer_plane
WitnessStateObserved_control_preempts_peer_delivery_1 == <> (runtime_control_phase = "Idle")
WitnessTransitionObserved_control_preempts_peer_delivery_runtime_control_Initialize == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "Initialize")
WitnessTransitionObserved_control_preempts_peer_delivery_peer_comms_TrustPeer == <> (\E packet \in observed_transitions : /\ packet.machine = "peer_comms" /\ packet.transition = "TrustPeer")
WitnessTransitionOrder_control_preempts_peer_delivery_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "Initialize" /\ later.machine = "peer_comms" /\ later.transition = "TrustPeer" /\ earlier.step < later.step)

THEOREM Spec => []peer_work_enters_runtime_via_canonical_admission
THEOREM Spec => []peer_admission_flows_into_ingress
THEOREM Spec => []control_preempts_peer_delivery
THEOREM Spec => []peer_comms_queued_items_are_classified
THEOREM Spec => []peer_comms_queued_items_preserve_content_shape
THEOREM Spec => []peer_comms_queued_items_preserve_text_projection
THEOREM Spec => []peer_comms_queued_items_preserve_correlation_slots
THEOREM Spec => []runtime_control_running_implies_active_run
THEOREM Spec => []runtime_control_active_run_only_while_running_or_retired
THEOREM Spec => []runtime_ingress_queue_entries_are_queued
THEOREM Spec => []runtime_ingress_steer_entries_are_queued
THEOREM Spec => []runtime_ingress_pending_inputs_preserve_content_shape
THEOREM Spec => []runtime_ingress_admitted_inputs_preserve_correlation_slots
THEOREM Spec => []runtime_ingress_queue_entries_preserve_handling_mode
THEOREM Spec => []runtime_ingress_steer_entries_preserve_handling_mode
THEOREM Spec => []runtime_ingress_pending_queues_do_not_overlap
THEOREM Spec => []runtime_ingress_terminal_inputs_do_not_appear_in_queue
THEOREM Spec => []runtime_ingress_current_run_matches_contributor_presence
THEOREM Spec => []runtime_ingress_staged_contributors_are_not_queued
THEOREM Spec => []runtime_ingress_applied_pending_consumption_has_last_run

=============================================================================
