---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for peer_runtime_bundle.

CONSTANTS AdmissionEffectValues, BooleanValues, CandidateIdValues, InputIdValues, InputKindValues, NatValues, PeerIdValues, PolicyDecisionValues, RawItemIdValues, RawPeerKindValues, RunIdValues, StringValues

SeqOfInputIdValues == {<<>>} \cup {<<x>> : x \in InputIdValues} \cup {<<x, y>> : x \in InputIdValues, y \in InputIdValues}

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]
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
    CASE route_name = "peer_candidate_enters_runtime_admission" -> "SubmitCandidate"
      [] route_name = "admitted_peer_work_enters_ingress" -> "AdmitQueued"
      [] OTHER -> "unknown_input"

RouteDeliveryKind(route_name) ==
    CASE route_name = "peer_candidate_enters_runtime_admission" -> "Immediate"
      [] route_name = "admitted_peer_work_enters_ingress" -> "Immediate"
      [] OTHER -> "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ peer_comms_phase = "Absent"
    /\ peer_comms_trusted_peers = {}
    /\ peer_comms_raw_item_peer = [x \in {} |-> None]
    /\ peer_comms_raw_item_kind = [x \in {} |-> None]
    /\ peer_comms_classified_as = [x \in {} |-> None]
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
    /\ runtime_ingress_input_kind = [x \in {} |-> None]
    /\ runtime_ingress_policy_snapshot = [x \in {} |-> None]
    /\ runtime_ingress_lifecycle = [x \in {} |-> None]
    /\ runtime_ingress_terminal_outcome = [x \in {} |-> None]
    /\ runtime_ingress_queue = <<>>
    /\ runtime_ingress_current_run = None
    /\ runtime_ingress_current_run_contributors = <<>>
    /\ runtime_ingress_last_run = [x \in {} |-> None]
    /\ runtime_ingress_last_boundary_sequence = [x \in {} |-> None]
    /\ runtime_ingress_wake_requested = FALSE
    /\ runtime_ingress_process_requested = FALSE
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
    /\ witness_remaining_script_inputs = <<[machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> "peer_1"], source_kind |-> "entry", source_route |-> "witness:trusted_peer_enters_runtime:2", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0], [machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [peer_id |-> "peer_1", raw_item_id |-> "raw_1", raw_kind |-> "Message"], source_kind |-> "entry", source_route |-> "witness:trusted_peer_enters_runtime:3", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0], [machine |-> "peer_comms", variant |-> "SubmitTypedPeerInput", payload |-> [raw_item_id |-> "raw_1"], source_kind |-> "entry", source_route |-> "witness:trusted_peer_enters_runtime:4", source_machine |-> "external_entry", source_effect |-> "SubmitTypedPeerInput", effect_id |-> 0]>>

WitnessInit_admitted_peer_work_enters_ingress ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> "peer_1"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:2", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0], [machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [peer_id |-> "peer_1", raw_item_id |-> "raw_1", raw_kind |-> "Message"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:3", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0], [machine |-> "peer_comms", variant |-> "SubmitTypedPeerInput", payload |-> [raw_item_id |-> "raw_1"], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:4", source_machine |-> "external_entry", source_effect |-> "SubmitTypedPeerInput", effect_id |-> 0], [machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> "SubmitAdmittedIngressEffect", candidate_id |-> "raw_1", candidate_kind |-> "PeerInput", process |-> TRUE, wake |-> FALSE], source_kind |-> "entry", source_route |-> "witness:admitted_peer_work_enters_ingress:5", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0]>>

WitnessInit_control_preempts_peer_delivery ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:control_preempts_peer_delivery:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0], [machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> "peer_1"], source_kind |-> "entry", source_route |-> "witness:control_preempts_peer_delivery:2", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:control_preempts_peer_delivery:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0], [machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> "peer_1"], source_kind |-> "entry", source_route |-> "witness:control_preempts_peer_delivery:2", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> "peer_1"], source_kind |-> "entry", source_route |-> "witness:control_preempts_peer_delivery:2", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [peer_id |-> "peer_1", raw_item_id |-> "raw_1", raw_kind |-> "Message"], source_kind |-> "entry", source_route |-> "witness:control_preempts_peer_delivery:3", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0]>>

peer_comms__ClassFor(raw_kind) == (IF (raw_kind = "request") THEN "ActionableRequest" ELSE "ActionableMessage")

peer_comms_TrustPeer(arg_peer_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "peer_comms"
       /\ packet.variant = "TrustPeer"
       /\ packet.payload.peer_id = arg_peer_id
       /\ ~HigherPriorityReady("peer_plane")
       /\ peer_comms_phase = "Absent" \/ peer_comms_phase = "Received"
       /\ peer_comms_phase' = "Absent"
       /\ peer_comms_trusted_peers' = (peer_comms_trusted_peers \cup {packet.payload.peer_id})
       /\ UNCHANGED << peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "TrustPeer", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Absent"]}
       /\ model_step_count' = model_step_count + 1


peer_comms_ReceiveTrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "peer_comms"
       /\ packet.variant = "ReceivePeerEnvelope"
       /\ packet.payload.raw_item_id = arg_raw_item_id
       /\ packet.payload.peer_id = arg_peer_id
       /\ packet.payload.raw_kind = arg_raw_kind
       /\ ~HigherPriorityReady("peer_plane")
       /\ peer_comms_phase = "Absent" \/ peer_comms_phase = "Received"
       /\ (packet.payload.peer_id \in peer_comms_trusted_peers)
       /\ peer_comms_phase' = "Received"
       /\ peer_comms_raw_item_peer' = MapSet(peer_comms_raw_item_peer, packet.payload.raw_item_id, packet.payload.peer_id)
       /\ peer_comms_raw_item_kind' = MapSet(peer_comms_raw_item_kind, packet.payload.raw_item_id, packet.payload.raw_kind)
       /\ peer_comms_classified_as' = MapSet(peer_comms_classified_as, packet.payload.raw_item_id, peer_comms__ClassFor(packet.payload.raw_kind))
       /\ peer_comms_trusted_snapshot' = MapSet(peer_comms_trusted_snapshot, packet.payload.raw_item_id, TRUE)
       /\ peer_comms_submission_queue' = Append(peer_comms_submission_queue, packet.payload.raw_item_id)
       /\ UNCHANGED << peer_comms_trusted_peers, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "ReceiveTrustedPeerEnvelope", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Received"]}
       /\ model_step_count' = model_step_count + 1


peer_comms_DropUntrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "peer_comms"
       /\ packet.variant = "ReceivePeerEnvelope"
       /\ packet.payload.raw_item_id = arg_raw_item_id
       /\ packet.payload.peer_id = arg_peer_id
       /\ packet.payload.raw_kind = arg_raw_kind
       /\ ~HigherPriorityReady("peer_plane")
       /\ peer_comms_phase = "Absent" \/ peer_comms_phase = "Received"
       /\ ~((packet.payload.peer_id \in peer_comms_trusted_peers))
       /\ peer_comms_phase' = "Dropped"
       /\ peer_comms_raw_item_peer' = MapSet(peer_comms_raw_item_peer, packet.payload.raw_item_id, packet.payload.peer_id)
       /\ peer_comms_raw_item_kind' = MapSet(peer_comms_raw_item_kind, packet.payload.raw_item_id, packet.payload.raw_kind)
       /\ peer_comms_trusted_snapshot' = MapSet(peer_comms_trusted_snapshot, packet.payload.raw_item_id, FALSE)
       /\ UNCHANGED << peer_comms_trusted_peers, peer_comms_classified_as, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "SubmitCandidate", payload |-> [candidate_id |-> packet.payload.raw_item_id, candidate_kind |-> "PeerInput"], source_kind |-> "route", source_route |-> "peer_candidate_enters_runtime_admission", source_machine |-> "peer_comms", source_effect |-> "SubmitPeerInputCandidate", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "SubmitCandidate", payload |-> [candidate_id |-> packet.payload.raw_item_id, candidate_kind |-> "PeerInput"], source_kind |-> "route", source_route |-> "peer_candidate_enters_runtime_admission", source_machine |-> "peer_comms", source_effect |-> "SubmitPeerInputCandidate", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "peer_candidate_enters_runtime_admission", source_machine |-> "peer_comms", effect |-> "SubmitPeerInputCandidate", target_machine |-> "runtime_control", target_input |-> "SubmitCandidate", payload |-> [candidate_id |-> packet.payload.raw_item_id, candidate_kind |-> "PeerInput"], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "SubmitTypedPeerInputDelivered"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "peer_comms", variant |-> "SubmitPeerInputCandidate", payload |-> [peer_input_class |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_classified_as THEN peer_comms_classified_as[packet.payload.raw_item_id] ELSE "None"), raw_item_id |-> packet.payload.raw_item_id], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitTypedPeerInputDelivered"] }
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
       /\ UNCHANGED << peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "SubmitCandidate", payload |-> [candidate_id |-> packet.payload.raw_item_id, candidate_kind |-> "PeerInput"], source_kind |-> "route", source_route |-> "peer_candidate_enters_runtime_admission", source_machine |-> "peer_comms", source_effect |-> "SubmitPeerInputCandidate", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "SubmitCandidate", payload |-> [candidate_id |-> packet.payload.raw_item_id, candidate_kind |-> "PeerInput"], source_kind |-> "route", source_route |-> "peer_candidate_enters_runtime_admission", source_machine |-> "peer_comms", source_effect |-> "SubmitPeerInputCandidate", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "peer_candidate_enters_runtime_admission", source_machine |-> "peer_comms", effect |-> "SubmitPeerInputCandidate", target_machine |-> "runtime_control", target_input |-> "SubmitCandidate", payload |-> [candidate_id |-> packet.payload.raw_item_id, candidate_kind |-> "PeerInput"], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "SubmitTypedPeerInputContinue"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "peer_comms", variant |-> "SubmitPeerInputCandidate", payload |-> [peer_input_class |-> (IF packet.payload.raw_item_id \in DOMAIN peer_comms_classified_as THEN peer_comms_classified_as[packet.payload.raw_item_id] ELSE "None"), raw_item_id |-> packet.payload.raw_item_id], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitTypedPeerInputContinue"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "peer_comms", transition |-> "SubmitTypedPeerInputContinue", actor |-> "peer_plane", step |-> (model_step_count + 1), from_phase |-> peer_comms_phase, to_phase |-> "Received"]}
       /\ model_step_count' = model_step_count + 1


peer_comms_queued_items_are_classified == \A raw_item_id \in SeqElements(peer_comms_submission_queue) : (raw_item_id \in DOMAIN peer_comms_classified_as)

runtime_control_Initialize ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "Initialize"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Initializing"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ApplyControlPlaneCommand", payload |-> [command |-> "Reset"], effect_id |-> (model_step_count + 1), source_transition |-> "ResetRequested"] }
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ResolveCompletionAsTerminated", payload |-> [reason |-> "Stopped"], effect_id |-> (model_step_count + 1), source_transition |-> "StopRequested"] }
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ResolveCompletionAsTerminated", payload |-> [reason |-> "Destroyed"], effect_id |-> (model_step_count + 1), source_transition |-> "DestroyRequested"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "DestroyRequested", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_ResumeRequested ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "ResumeRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Recovering"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "ResumeRequested", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_SubmitCandidateFromIdle(arg_candidate_id, arg_candidate_kind) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "SubmitCandidate"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.candidate_kind = arg_candidate_kind
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ResolveAdmission", payload |-> [candidate_id |-> packet.payload.candidate_id], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitCandidateFromIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "SubmitCandidateFromIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_SubmitCandidateFromRunning(arg_candidate_id, arg_candidate_kind) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "SubmitCandidate"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.candidate_kind = arg_candidate_kind
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ runtime_control_phase' = "Running"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ResolveAdmission", payload |-> [candidate_id |-> packet.payload.candidate_id], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitCandidateFromRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "SubmitCandidateFromRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedIdleNone(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionAccepted"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.candidate_kind = arg_candidate_kind
       /\ packet.payload.admission_effect = arg_admission_effect
       /\ packet.payload.wake = arg_wake
       /\ packet.payload.process = arg_process
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ (packet.payload.wake = FALSE)
       /\ (packet.payload.process = FALSE)
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_wake_pending' = packet.payload.wake
       /\ runtime_control_process_pending' = packet.payload.process
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleNone"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, candidate_id |-> packet.payload.candidate_id, candidate_kind |-> packet.payload.candidate_kind, process |-> packet.payload.process, wake |-> packet.payload.wake], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleNone"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedIdleNone", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedIdleWake(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionAccepted"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.candidate_kind = arg_candidate_kind
       /\ packet.payload.admission_effect = arg_admission_effect
       /\ packet.payload.wake = arg_wake
       /\ packet.payload.process = arg_process
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ (packet.payload.wake = TRUE)
       /\ (packet.payload.process = FALSE)
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_wake_pending' = TRUE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleWake"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, candidate_id |-> packet.payload.candidate_id, candidate_kind |-> packet.payload.candidate_kind, process |-> packet.payload.process, wake |-> packet.payload.wake], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleWake"], [machine |-> "runtime_control", variant |-> "SignalWake", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleWake"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedIdleWake", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedIdleProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionAccepted"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.candidate_kind = arg_candidate_kind
       /\ packet.payload.admission_effect = arg_admission_effect
       /\ packet.payload.wake = arg_wake
       /\ packet.payload.process = arg_process
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ (packet.payload.wake = FALSE)
       /\ (packet.payload.process = TRUE)
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_wake_pending' = FALSE
       /\ runtime_control_process_pending' = TRUE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleProcess"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, candidate_id |-> packet.payload.candidate_id, candidate_kind |-> packet.payload.candidate_kind, process |-> packet.payload.process, wake |-> packet.payload.wake], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleProcess"], [machine |-> "runtime_control", variant |-> "SignalImmediateProcess", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleProcess"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedIdleProcess", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedIdleWakeAndProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionAccepted"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.candidate_kind = arg_candidate_kind
       /\ packet.payload.admission_effect = arg_admission_effect
       /\ packet.payload.wake = arg_wake
       /\ packet.payload.process = arg_process
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ (packet.payload.wake = TRUE)
       /\ (packet.payload.process = TRUE)
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_wake_pending' = TRUE
       /\ runtime_control_process_pending' = TRUE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleWakeAndProcess"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, candidate_id |-> packet.payload.candidate_id, candidate_kind |-> packet.payload.candidate_kind, process |-> packet.payload.process, wake |-> packet.payload.wake], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleWakeAndProcess"], [machine |-> "runtime_control", variant |-> "SignalWake", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleWakeAndProcess"], [machine |-> "runtime_control", variant |-> "SignalImmediateProcess", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleWakeAndProcess"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedIdleWakeAndProcess", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedRunningNone(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionAccepted"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.candidate_kind = arg_candidate_kind
       /\ packet.payload.admission_effect = arg_admission_effect
       /\ packet.payload.wake = arg_wake
       /\ packet.payload.process = arg_process
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (packet.payload.wake = FALSE)
       /\ (packet.payload.process = FALSE)
       /\ runtime_control_phase' = "Running"
       /\ runtime_control_wake_pending' = FALSE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningNone"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, candidate_id |-> packet.payload.candidate_id, candidate_kind |-> packet.payload.candidate_kind, process |-> packet.payload.process, wake |-> packet.payload.wake], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningNone"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedRunningNone", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedRunningWake(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionAccepted"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.candidate_kind = arg_candidate_kind
       /\ packet.payload.admission_effect = arg_admission_effect
       /\ packet.payload.wake = arg_wake
       /\ packet.payload.process = arg_process
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (packet.payload.wake = TRUE)
       /\ (packet.payload.process = FALSE)
       /\ runtime_control_phase' = "Running"
       /\ runtime_control_wake_pending' = TRUE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningWake"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, candidate_id |-> packet.payload.candidate_id, candidate_kind |-> packet.payload.candidate_kind, process |-> packet.payload.process, wake |-> packet.payload.wake], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningWake"], [machine |-> "runtime_control", variant |-> "SignalWake", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningWake"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedRunningWake", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedRunningProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionAccepted"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.candidate_kind = arg_candidate_kind
       /\ packet.payload.admission_effect = arg_admission_effect
       /\ packet.payload.wake = arg_wake
       /\ packet.payload.process = arg_process
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (packet.payload.wake = FALSE)
       /\ (packet.payload.process = TRUE)
       /\ runtime_control_phase' = "Running"
       /\ runtime_control_wake_pending' = FALSE
       /\ runtime_control_process_pending' = TRUE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningProcess"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, candidate_id |-> packet.payload.candidate_id, candidate_kind |-> packet.payload.candidate_kind, process |-> packet.payload.process, wake |-> packet.payload.wake], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningProcess"], [machine |-> "runtime_control", variant |-> "SignalImmediateProcess", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningProcess"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedRunningProcess", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedRunningWakeAndProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionAccepted"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.candidate_kind = arg_candidate_kind
       /\ packet.payload.admission_effect = arg_admission_effect
       /\ packet.payload.wake = arg_wake
       /\ packet.payload.process = arg_process
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (packet.payload.wake = TRUE)
       /\ (packet.payload.process = TRUE)
       /\ runtime_control_phase' = "Running"
       /\ runtime_control_wake_pending' = TRUE
       /\ runtime_control_process_pending' = TRUE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], source_kind |-> "route", source_route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "admitted_peer_work_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [input_id |-> packet.payload.candidate_id, input_kind |-> packet.payload.candidate_kind, policy |-> "PeerQueued", process |-> TRUE, wake |-> TRUE], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningWakeAndProcess"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, candidate_id |-> packet.payload.candidate_id, candidate_kind |-> packet.payload.candidate_kind, process |-> packet.payload.process, wake |-> packet.payload.wake], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningWakeAndProcess"], [machine |-> "runtime_control", variant |-> "SignalWake", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningWakeAndProcess"], [machine |-> "runtime_control", variant |-> "SignalImmediateProcess", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningWakeAndProcess"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedRunningWakeAndProcess", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionRejectedIdle(arg_candidate_id, arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionRejected"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> packet.payload.reason, kind |-> "AdmissionRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionRejectedIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionRejectedIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionRejectedRunning(arg_candidate_id, arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionRejected"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ runtime_control_phase' = "Running"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> packet.payload.reason, kind |-> "AdmissionRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionRejectedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionRejectedRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionDeduplicatedIdle(arg_candidate_id, arg_existing_input_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionDeduplicated"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.existing_input_id = arg_existing_input_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "ExistingInputLinked", kind |-> "AdmissionDeduplicated"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionDeduplicatedIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionDeduplicatedIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionDeduplicatedRunning(arg_candidate_id, arg_existing_input_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionDeduplicated"
       /\ packet.payload.candidate_id = arg_candidate_id
       /\ packet.payload.existing_input_id = arg_existing_input_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ runtime_control_phase' = "Running"
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "Received", kind |-> "ExternalToolDelta"], effect_id |-> (model_step_count + 1), source_transition |-> "ExternalToolDeltaReceivedRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "ExternalToolDeltaReceivedRetired", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_running_implies_active_run == ((runtime_control_phase # "Running") \/ (runtime_control_current_run_id # None))
runtime_control_active_run_only_while_running_or_retired == ((runtime_control_current_run_id = None) \/ (runtime_control_phase = "Running") \/ (runtime_control_phase = "Retired"))

RECURSIVE runtime_ingress_StageDrainSnapshot_ForEach0_last_run(_, _, _)
runtime_ingress_StageDrainSnapshot_ForEach0_last_run(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some(outer_run_id)) IN runtime_ingress_StageDrainSnapshot_ForEach0_last_run(next_acc, Tail(items), outer_run_id)

RECURSIVE runtime_ingress_StageDrainSnapshot_ForEach0_lifecycle(_, _, _)
runtime_ingress_StageDrainSnapshot_ForEach0_lifecycle(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Staged") IN runtime_ingress_StageDrainSnapshot_ForEach0_lifecycle(next_acc, Tail(items), outer_run_id)

RECURSIVE runtime_ingress_BoundaryApplied_ForEach1_last_boundary_sequence(_, _, _)
runtime_ingress_BoundaryApplied_ForEach1_last_boundary_sequence(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some(outer_boundary_sequence)) IN runtime_ingress_BoundaryApplied_ForEach1_last_boundary_sequence(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE runtime_ingress_BoundaryApplied_ForEach1_lifecycle(_, _, _)
runtime_ingress_BoundaryApplied_ForEach1_lifecycle(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "AppliedPendingConsumption") IN runtime_ingress_BoundaryApplied_ForEach1_lifecycle(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE runtime_ingress_RunCompleted_ForEach2_lifecycle(_, _)
runtime_ingress_RunCompleted_ForEach2_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Consumed") IN runtime_ingress_RunCompleted_ForEach2_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_RunCompleted_ForEach2_terminal_outcome(_, _)
runtime_ingress_RunCompleted_ForEach2_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("Consumed")) IN runtime_ingress_RunCompleted_ForEach2_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_RunFailed_ForEach3_lifecycle(_, _)
runtime_ingress_RunFailed_ForEach3_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Queued") IN runtime_ingress_RunFailed_ForEach3_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_RunCancelled_ForEach4_lifecycle(_, _)
runtime_ingress_RunCancelled_ForEach4_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Queued") IN runtime_ingress_RunCancelled_ForEach4_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_CoalesceQueuedInputs_ForEach5_lifecycle(_, _)
runtime_ingress_CoalesceQueuedInputs_ForEach5_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Coalesced") IN runtime_ingress_CoalesceQueuedInputs_ForEach5_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_CoalesceQueuedInputs_ForEach5_terminal_outcome(_, _)
runtime_ingress_CoalesceQueuedInputs_ForEach5_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("Coalesced")) IN runtime_ingress_CoalesceQueuedInputs_ForEach5_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromActive_ForEach6_lifecycle(_, _)
runtime_ingress_ResetFromActive_ForEach6_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Abandoned") IN runtime_ingress_ResetFromActive_ForEach6_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromActive_ForEach6_terminal_outcome(_, _)
runtime_ingress_ResetFromActive_ForEach6_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("AbandonedReset")) IN runtime_ingress_ResetFromActive_ForEach6_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromActive_ForEach7_lifecycle(_, _)
runtime_ingress_ResetFromActive_ForEach7_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Abandoned") IN runtime_ingress_ResetFromActive_ForEach7_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromActive_ForEach7_terminal_outcome(_, _)
runtime_ingress_ResetFromActive_ForEach7_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("AbandonedReset")) IN runtime_ingress_ResetFromActive_ForEach7_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromRetired_ForEach8_lifecycle(_, _)
runtime_ingress_ResetFromRetired_ForEach8_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Abandoned") IN runtime_ingress_ResetFromRetired_ForEach8_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromRetired_ForEach8_terminal_outcome(_, _)
runtime_ingress_ResetFromRetired_ForEach8_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("AbandonedReset")) IN runtime_ingress_ResetFromRetired_ForEach8_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromRetired_ForEach9_lifecycle(_, _)
runtime_ingress_ResetFromRetired_ForEach9_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Abandoned") IN runtime_ingress_ResetFromRetired_ForEach9_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromRetired_ForEach9_terminal_outcome(_, _)
runtime_ingress_ResetFromRetired_ForEach9_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("AbandonedReset")) IN runtime_ingress_ResetFromRetired_ForEach9_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_Destroy_ForEach10_lifecycle(_, _)
runtime_ingress_Destroy_ForEach10_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Abandoned") IN runtime_ingress_Destroy_ForEach10_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_Destroy_ForEach10_terminal_outcome(_, _)
runtime_ingress_Destroy_ForEach10_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("AbandonedDestroyed")) IN runtime_ingress_Destroy_ForEach10_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_Destroy_ForEach11_lifecycle(_, _)
runtime_ingress_Destroy_ForEach11_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Abandoned") IN runtime_ingress_Destroy_ForEach11_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_Destroy_ForEach11_terminal_outcome(_, _)
runtime_ingress_Destroy_ForEach11_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, Some("AbandonedDestroyed")) IN runtime_ingress_Destroy_ForEach11_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_RecoverFromActive_ForEach12_lifecycle(_, _)
runtime_ingress_RecoverFromActive_ForEach12_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Queued") IN runtime_ingress_RecoverFromActive_ForEach12_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_RecoverFromRetired_ForEach13_lifecycle(_, _)
runtime_ingress_RecoverFromRetired_ForEach13_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET input_id == Head(items) IN LET next_acc == MapSet(acc, input_id, "Queued") IN runtime_ingress_RecoverFromRetired_ForEach13_lifecycle(next_acc, Tail(items))

runtime_ingress_AdmitQueuedNone(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "AdmitQueued"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.input_kind = arg_input_kind
       /\ packet.payload.policy = arg_policy
       /\ packet.payload.wake = arg_wake
       /\ packet.payload.process = arg_process
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ ~((packet.payload.input_id \in runtime_ingress_admitted_inputs))
       /\ (packet.payload.wake = FALSE)
       /\ (packet.payload.process = FALSE)
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_admitted_inputs' = (runtime_ingress_admitted_inputs \cup {packet.payload.input_id})
       /\ runtime_ingress_admission_order' = Append(runtime_ingress_admission_order, packet.payload.input_id)
       /\ runtime_ingress_input_kind' = MapSet(runtime_ingress_input_kind, packet.payload.input_id, packet.payload.input_kind)
       /\ runtime_ingress_policy_snapshot' = MapSet(runtime_ingress_policy_snapshot, packet.payload.input_id, packet.payload.policy)
       /\ runtime_ingress_lifecycle' = MapSet(runtime_ingress_lifecycle, packet.payload.input_id, "Queued")
       /\ runtime_ingress_terminal_outcome' = MapSet(runtime_ingress_terminal_outcome, packet.payload.input_id, None)
       /\ runtime_ingress_queue' = Append(runtime_ingress_queue, packet.payload.input_id)
       /\ runtime_ingress_last_run' = MapSet(runtime_ingress_last_run, packet.payload.input_id, None)
       /\ runtime_ingress_last_boundary_sequence' = MapSet(runtime_ingress_last_boundary_sequence, packet.payload.input_id, None)
       /\ runtime_ingress_wake_requested' = (runtime_ingress_wake_requested \/ packet.payload.wake)
       /\ runtime_ingress_process_requested' = (runtime_ingress_process_requested \/ packet.payload.process)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_current_run, runtime_ingress_current_run_contributors, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressAccepted", payload |-> [input_id |-> packet.payload.input_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedNone"], [machine |-> "runtime_ingress", variant |-> "InputLifecycleNotice", payload |-> [input_id |-> packet.payload.input_id, new_state |-> "Queued"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedNone"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "AdmitQueuedNone", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_AdmitQueuedWake(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "AdmitQueued"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.input_kind = arg_input_kind
       /\ packet.payload.policy = arg_policy
       /\ packet.payload.wake = arg_wake
       /\ packet.payload.process = arg_process
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ ~((packet.payload.input_id \in runtime_ingress_admitted_inputs))
       /\ (packet.payload.wake = TRUE)
       /\ (packet.payload.process = FALSE)
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_admitted_inputs' = (runtime_ingress_admitted_inputs \cup {packet.payload.input_id})
       /\ runtime_ingress_admission_order' = Append(runtime_ingress_admission_order, packet.payload.input_id)
       /\ runtime_ingress_input_kind' = MapSet(runtime_ingress_input_kind, packet.payload.input_id, packet.payload.input_kind)
       /\ runtime_ingress_policy_snapshot' = MapSet(runtime_ingress_policy_snapshot, packet.payload.input_id, packet.payload.policy)
       /\ runtime_ingress_lifecycle' = MapSet(runtime_ingress_lifecycle, packet.payload.input_id, "Queued")
       /\ runtime_ingress_terminal_outcome' = MapSet(runtime_ingress_terminal_outcome, packet.payload.input_id, None)
       /\ runtime_ingress_queue' = Append(runtime_ingress_queue, packet.payload.input_id)
       /\ runtime_ingress_last_run' = MapSet(runtime_ingress_last_run, packet.payload.input_id, None)
       /\ runtime_ingress_last_boundary_sequence' = MapSet(runtime_ingress_last_boundary_sequence, packet.payload.input_id, None)
       /\ runtime_ingress_wake_requested' = (runtime_ingress_wake_requested \/ packet.payload.wake)
       /\ runtime_ingress_process_requested' = (runtime_ingress_process_requested \/ packet.payload.process)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_current_run, runtime_ingress_current_run_contributors, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressAccepted", payload |-> [input_id |-> packet.payload.input_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedWake"], [machine |-> "runtime_ingress", variant |-> "InputLifecycleNotice", payload |-> [input_id |-> packet.payload.input_id, new_state |-> "Queued"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedWake"], [machine |-> "runtime_ingress", variant |-> "WakeRuntime", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedWake"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "AdmitQueuedWake", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_AdmitQueuedProcess(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "AdmitQueued"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.input_kind = arg_input_kind
       /\ packet.payload.policy = arg_policy
       /\ packet.payload.wake = arg_wake
       /\ packet.payload.process = arg_process
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ ~((packet.payload.input_id \in runtime_ingress_admitted_inputs))
       /\ (packet.payload.wake = FALSE)
       /\ (packet.payload.process = TRUE)
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_admitted_inputs' = (runtime_ingress_admitted_inputs \cup {packet.payload.input_id})
       /\ runtime_ingress_admission_order' = Append(runtime_ingress_admission_order, packet.payload.input_id)
       /\ runtime_ingress_input_kind' = MapSet(runtime_ingress_input_kind, packet.payload.input_id, packet.payload.input_kind)
       /\ runtime_ingress_policy_snapshot' = MapSet(runtime_ingress_policy_snapshot, packet.payload.input_id, packet.payload.policy)
       /\ runtime_ingress_lifecycle' = MapSet(runtime_ingress_lifecycle, packet.payload.input_id, "Queued")
       /\ runtime_ingress_terminal_outcome' = MapSet(runtime_ingress_terminal_outcome, packet.payload.input_id, None)
       /\ runtime_ingress_queue' = Append(runtime_ingress_queue, packet.payload.input_id)
       /\ runtime_ingress_last_run' = MapSet(runtime_ingress_last_run, packet.payload.input_id, None)
       /\ runtime_ingress_last_boundary_sequence' = MapSet(runtime_ingress_last_boundary_sequence, packet.payload.input_id, None)
       /\ runtime_ingress_wake_requested' = (runtime_ingress_wake_requested \/ packet.payload.wake)
       /\ runtime_ingress_process_requested' = (runtime_ingress_process_requested \/ packet.payload.process)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_current_run, runtime_ingress_current_run_contributors, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressAccepted", payload |-> [input_id |-> packet.payload.input_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedProcess"], [machine |-> "runtime_ingress", variant |-> "InputLifecycleNotice", payload |-> [input_id |-> packet.payload.input_id, new_state |-> "Queued"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedProcess"], [machine |-> "runtime_ingress", variant |-> "RequestImmediateProcessing", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedProcess"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "AdmitQueuedProcess", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_AdmitQueuedWakeAndProcess(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "AdmitQueued"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.input_kind = arg_input_kind
       /\ packet.payload.policy = arg_policy
       /\ packet.payload.wake = arg_wake
       /\ packet.payload.process = arg_process
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ ~((packet.payload.input_id \in runtime_ingress_admitted_inputs))
       /\ (packet.payload.wake = TRUE)
       /\ (packet.payload.process = TRUE)
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_admitted_inputs' = (runtime_ingress_admitted_inputs \cup {packet.payload.input_id})
       /\ runtime_ingress_admission_order' = Append(runtime_ingress_admission_order, packet.payload.input_id)
       /\ runtime_ingress_input_kind' = MapSet(runtime_ingress_input_kind, packet.payload.input_id, packet.payload.input_kind)
       /\ runtime_ingress_policy_snapshot' = MapSet(runtime_ingress_policy_snapshot, packet.payload.input_id, packet.payload.policy)
       /\ runtime_ingress_lifecycle' = MapSet(runtime_ingress_lifecycle, packet.payload.input_id, "Queued")
       /\ runtime_ingress_terminal_outcome' = MapSet(runtime_ingress_terminal_outcome, packet.payload.input_id, None)
       /\ runtime_ingress_queue' = Append(runtime_ingress_queue, packet.payload.input_id)
       /\ runtime_ingress_last_run' = MapSet(runtime_ingress_last_run, packet.payload.input_id, None)
       /\ runtime_ingress_last_boundary_sequence' = MapSet(runtime_ingress_last_boundary_sequence, packet.payload.input_id, None)
       /\ runtime_ingress_wake_requested' = (runtime_ingress_wake_requested \/ packet.payload.wake)
       /\ runtime_ingress_process_requested' = (runtime_ingress_process_requested \/ packet.payload.process)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_current_run, runtime_ingress_current_run_contributors, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressAccepted", payload |-> [input_id |-> packet.payload.input_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedWakeAndProcess"], [machine |-> "runtime_ingress", variant |-> "InputLifecycleNotice", payload |-> [input_id |-> packet.payload.input_id, new_state |-> "Queued"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedWakeAndProcess"], [machine |-> "runtime_ingress", variant |-> "WakeRuntime", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedWakeAndProcess"], [machine |-> "runtime_ingress", variant |-> "RequestImmediateProcessing", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedWakeAndProcess"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "AdmitQueuedWakeAndProcess", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_AdmitConsumedOnAccept(arg_input_id, arg_input_kind, arg_policy) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "AdmitConsumedOnAccept"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.input_kind = arg_input_kind
       /\ packet.payload.policy = arg_policy
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ ~((packet.payload.input_id \in runtime_ingress_admitted_inputs))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_admitted_inputs' = (runtime_ingress_admitted_inputs \cup {packet.payload.input_id})
       /\ runtime_ingress_admission_order' = Append(runtime_ingress_admission_order, packet.payload.input_id)
       /\ runtime_ingress_input_kind' = MapSet(runtime_ingress_input_kind, packet.payload.input_id, packet.payload.input_kind)
       /\ runtime_ingress_policy_snapshot' = MapSet(runtime_ingress_policy_snapshot, packet.payload.input_id, packet.payload.policy)
       /\ runtime_ingress_lifecycle' = MapSet(runtime_ingress_lifecycle, packet.payload.input_id, "Consumed")
       /\ runtime_ingress_terminal_outcome' = MapSet(runtime_ingress_terminal_outcome, packet.payload.input_id, Some("Consumed"))
       /\ runtime_ingress_last_run' = MapSet(runtime_ingress_last_run, packet.payload.input_id, None)
       /\ runtime_ingress_last_boundary_sequence' = MapSet(runtime_ingress_last_boundary_sequence, packet.payload.input_id, None)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressAccepted", payload |-> [input_id |-> packet.payload.input_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitConsumedOnAccept"], [machine |-> "runtime_ingress", variant |-> "InputLifecycleNotice", payload |-> [input_id |-> packet.payload.input_id, new_state |-> "Consumed"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitConsumedOnAccept"], [machine |-> "runtime_ingress", variant |-> "CompletionResolved", payload |-> [input_id |-> packet.payload.input_id, outcome |-> "Consumed"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitConsumedOnAccept"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "AdmitConsumedOnAccept", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_StageDrainSnapshot(arg_run_id, arg_contributing_input_ids) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "StageDrainSnapshot"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.contributing_input_ids = arg_contributing_input_ids
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active" \/ runtime_ingress_phase = "Retired"
       /\ (runtime_ingress_current_run = None)
       /\ (Len(packet.payload.contributing_input_ids) > 0)
       /\ StartsWith(runtime_ingress_queue, packet.payload.contributing_input_ids)
       /\ \A input_id \in SeqElements(packet.payload.contributing_input_ids) : ((IF input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[input_id] ELSE "None") = "Queued")
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_StageDrainSnapshot_ForEach0_lifecycle(runtime_ingress_lifecycle, packet.payload.contributing_input_ids, packet.payload.run_id)
       /\ runtime_ingress_queue' = SeqRemoveAll(runtime_ingress_queue, packet.payload.contributing_input_ids)
       /\ runtime_ingress_current_run' = Some(packet.payload.run_id)
       /\ runtime_ingress_current_run_contributors' = packet.payload.contributing_input_ids
       /\ runtime_ingress_last_run' = runtime_ingress_StageDrainSnapshot_ForEach0_last_run(runtime_ingress_last_run, packet.payload.contributing_input_ids, packet.payload.run_id)
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_terminal_outcome, runtime_ingress_last_boundary_sequence, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "ReadyForRun", payload |-> [contributing_input_ids |-> packet.payload.contributing_input_ids, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "StageDrainSnapshot"] }
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
       /\ \A input_id \in SeqElements(runtime_ingress_current_run_contributors) : ((IF input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[input_id] ELSE "None") = "Staged")
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_BoundaryApplied_ForEach1_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, packet.payload.boundary_sequence)
       /\ runtime_ingress_last_boundary_sequence' = runtime_ingress_BoundaryApplied_ForEach1_last_boundary_sequence(runtime_ingress_last_boundary_sequence, runtime_ingress_current_run_contributors, packet.payload.boundary_sequence)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ \A input_id \in SeqElements(runtime_ingress_current_run_contributors) : ((IF input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[input_id] ELSE "None") = "AppliedPendingConsumption")
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RunCompleted_ForEach2_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_RunCompleted_ForEach2_terminal_outcome(runtime_ingress_terminal_outcome, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_queue, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "ContributorsConsumed", kind |-> "RunCompleted"], effect_id |-> (model_step_count + 1), source_transition |-> "RunCompleted"], [machine |-> "runtime_ingress", variant |-> "CompletionResolved", payload |-> [input_id |-> Head(<<>>), outcome |-> "Consumed"], effect_id |-> (model_step_count + 1), source_transition |-> "RunCompleted"] }
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
       /\ \A input_id \in SeqElements(runtime_ingress_current_run_contributors) : ((IF input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[input_id] ELSE "None") = "Staged")
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RunFailed_ForEach3_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN (runtime_ingress_current_run_contributors \o runtime_ingress_queue) ELSE runtime_ingress_queue
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN TRUE ELSE runtime_ingress_wake_requested
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_terminal_outcome, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ \A input_id \in SeqElements(runtime_ingress_current_run_contributors) : ((IF input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[input_id] ELSE "None") = "Staged")
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RunCancelled_ForEach4_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN (runtime_ingress_current_run_contributors \o runtime_ingress_queue) ELSE runtime_ingress_queue
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN TRUE ELSE runtime_ingress_wake_requested
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_terminal_outcome, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "ContributorsRequeued", kind |-> "RunCancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "RunCancelled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RunCancelled", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_SupersedeQueuedInput(arg_new_input_id, arg_old_input_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "SupersedeQueuedInput"
       /\ packet.payload.new_input_id = arg_new_input_id
       /\ packet.payload.old_input_id = arg_old_input_id
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ (packet.payload.new_input_id \in runtime_ingress_admitted_inputs)
       /\ ((IF packet.payload.old_input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[packet.payload.old_input_id] ELSE "None") = "Queued")
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = MapSet(runtime_ingress_lifecycle, packet.payload.old_input_id, "Superseded")
       /\ runtime_ingress_terminal_outcome' = MapSet(runtime_ingress_terminal_outcome, packet.payload.old_input_id, Some("Superseded"))
       /\ runtime_ingress_queue' = SeqRemove(runtime_ingress_queue, packet.payload.old_input_id)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "QueuedInputSuperseded", kind |-> "SupersedeQueuedInput"], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedeQueuedInput"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "SupersedeQueuedInput", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_CoalesceQueuedInputs(arg_aggregate_input_id, arg_source_input_ids) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "CoalesceQueuedInputs"
       /\ packet.payload.aggregate_input_id = arg_aggregate_input_id
       /\ packet.payload.source_input_ids = arg_source_input_ids
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ (packet.payload.aggregate_input_id \in runtime_ingress_admitted_inputs)
       /\ (Len(packet.payload.source_input_ids) > 0)
       /\ \A input_id \in SeqElements(packet.payload.source_input_ids) : ((IF input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[input_id] ELSE "None") = "Queued")
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_CoalesceQueuedInputs_ForEach5_lifecycle(runtime_ingress_lifecycle, packet.payload.source_input_ids)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_CoalesceQueuedInputs_ForEach5_terminal_outcome(runtime_ingress_terminal_outcome, packet.payload.source_input_ids)
       /\ runtime_ingress_queue' = SeqRemoveAll(runtime_ingress_queue, packet.payload.source_input_ids)
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ runtime_ingress_lifecycle' = runtime_ingress_ResetFromActive_ForEach7_lifecycle(runtime_ingress_ResetFromActive_ForEach6_lifecycle(runtime_ingress_lifecycle, runtime_ingress_queue), runtime_ingress_current_run_contributors)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_ResetFromActive_ForEach7_terminal_outcome(runtime_ingress_ResetFromActive_ForEach6_terminal_outcome(runtime_ingress_terminal_outcome, runtime_ingress_queue), runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = <<>>
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ runtime_ingress_lifecycle' = runtime_ingress_ResetFromRetired_ForEach9_lifecycle(runtime_ingress_ResetFromRetired_ForEach8_lifecycle(runtime_ingress_lifecycle, runtime_ingress_queue), runtime_ingress_current_run_contributors)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_ResetFromRetired_ForEach9_terminal_outcome(runtime_ingress_ResetFromRetired_ForEach8_terminal_outcome(runtime_ingress_terminal_outcome, runtime_ingress_queue), runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = <<>>
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ runtime_ingress_lifecycle' = runtime_ingress_Destroy_ForEach11_lifecycle(runtime_ingress_Destroy_ForEach10_lifecycle(runtime_ingress_lifecycle, runtime_ingress_queue), runtime_ingress_current_run_contributors)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_Destroy_ForEach11_terminal_outcome(runtime_ingress_Destroy_ForEach10_terminal_outcome(runtime_ingress_terminal_outcome, runtime_ingress_queue), runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = <<>>
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ runtime_ingress_lifecycle' = runtime_ingress_RecoverFromActive_ForEach12_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN (runtime_ingress_current_run_contributors \o runtime_ingress_queue) ELSE runtime_ingress_queue
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN TRUE ELSE runtime_ingress_wake_requested
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_terminal_outcome, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ runtime_ingress_lifecycle' = runtime_ingress_RecoverFromRetired_ForEach13_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_queue' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN (runtime_ingress_current_run_contributors \o runtime_ingress_queue) ELSE runtime_ingress_queue
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = IF (Len(runtime_ingress_current_run_contributors) > 0) THEN TRUE ELSE runtime_ingress_wake_requested
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_terminal_outcome, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "RecoveryAppliedToCurrentRun", kind |-> "Recover"], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RecoverFromRetired", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_queue_entries_are_queued == \A input_id \in SeqElements(runtime_ingress_queue) : ((IF input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[input_id] ELSE "None") = "Queued")
runtime_ingress_terminal_inputs_do_not_appear_in_queue == \A input_id \in SeqElements(runtime_ingress_queue) : (((IF input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[input_id] ELSE "None") # "Consumed") /\ ((IF input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[input_id] ELSE "None") # "Superseded") /\ ((IF input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[input_id] ELSE "None") # "Coalesced") /\ ((IF input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[input_id] ELSE "None") # "Abandoned"))
runtime_ingress_current_run_matches_contributor_presence == ((runtime_ingress_current_run = None) = (Len(runtime_ingress_current_run_contributors) = 0))
runtime_ingress_staged_contributors_are_not_queued == \A input_id \in SeqElements(runtime_ingress_current_run_contributors) : ~((input_id \in SeqElements(runtime_ingress_queue)))
runtime_ingress_applied_pending_consumption_has_last_run == \A input_id \in runtime_ingress_admitted_inputs : (((IF input_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[input_id] ELSE "None") # "AppliedPendingConsumption") \/ ((IF input_id \in DOMAIN runtime_ingress_last_run THEN runtime_ingress_last_run[input_id] ELSE None) # None))

Inject_control_initialize ==
    /\ ~([machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "control_initialize", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "control_initialize", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "control_initialize", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_trust_peer(arg_peer_id) ==
    /\ ~([machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> arg_peer_id], source_kind |-> "entry", source_route |-> "trust_peer", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> arg_peer_id], source_kind |-> "entry", source_route |-> "trust_peer", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "peer_comms", variant |-> "TrustPeer", payload |-> [peer_id |-> arg_peer_id], source_kind |-> "entry", source_route |-> "trust_peer", source_machine |-> "external_entry", source_effect |-> "TrustPeer", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_receive_peer_envelope(arg_raw_item_id, arg_peer_id, arg_raw_kind) ==
    /\ ~([machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [peer_id |-> arg_peer_id, raw_item_id |-> arg_raw_item_id, raw_kind |-> arg_raw_kind], source_kind |-> "entry", source_route |-> "receive_peer_envelope", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [peer_id |-> arg_peer_id, raw_item_id |-> arg_raw_item_id, raw_kind |-> arg_raw_kind], source_kind |-> "entry", source_route |-> "receive_peer_envelope", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "peer_comms", variant |-> "ReceivePeerEnvelope", payload |-> [peer_id |-> arg_peer_id, raw_item_id |-> arg_raw_item_id, raw_kind |-> arg_raw_kind], source_kind |-> "entry", source_route |-> "receive_peer_envelope", source_machine |-> "external_entry", source_effect |-> "ReceivePeerEnvelope", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_submit_typed_peer_input(arg_raw_item_id) ==
    /\ ~([machine |-> "peer_comms", variant |-> "SubmitTypedPeerInput", payload |-> [raw_item_id |-> arg_raw_item_id], source_kind |-> "entry", source_route |-> "submit_typed_peer_input", source_machine |-> "external_entry", source_effect |-> "SubmitTypedPeerInput", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "peer_comms", variant |-> "SubmitTypedPeerInput", payload |-> [raw_item_id |-> arg_raw_item_id], source_kind |-> "entry", source_route |-> "submit_typed_peer_input", source_machine |-> "external_entry", source_effect |-> "SubmitTypedPeerInput", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "peer_comms", variant |-> "SubmitTypedPeerInput", payload |-> [raw_item_id |-> arg_raw_item_id], source_kind |-> "entry", source_route |-> "submit_typed_peer_input", source_machine |-> "external_entry", source_effect |-> "SubmitTypedPeerInput", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_runtime_admission_accepted(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process) ==
    /\ ~([machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> arg_admission_effect, candidate_id |-> arg_candidate_id, candidate_kind |-> arg_candidate_kind, process |-> arg_process, wake |-> arg_wake], source_kind |-> "entry", source_route |-> "runtime_admission_accepted", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> arg_admission_effect, candidate_id |-> arg_candidate_id, candidate_kind |-> arg_candidate_kind, process |-> arg_process, wake |-> arg_wake], source_kind |-> "entry", source_route |-> "runtime_admission_accepted", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> arg_admission_effect, candidate_id |-> arg_candidate_id, candidate_kind |-> arg_candidate_kind, process |-> arg_process, wake |-> arg_wake], source_kind |-> "entry", source_route |-> "runtime_admission_accepted", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_ingress_stage_drain_snapshot(arg_run_id, arg_contributing_input_ids) ==
    /\ ~([machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_input_ids |-> arg_contributing_input_ids, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "ingress_stage_drain_snapshot", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_input_ids |-> arg_contributing_input_ids, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "ingress_stage_drain_snapshot", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_input_ids |-> arg_contributing_input_ids, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "ingress_stage_drain_snapshot", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

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
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

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
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

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
    /\ UNCHANGED << peer_comms_phase, peer_comms_trusted_peers, peer_comms_raw_item_peer, peer_comms_raw_item_kind, peer_comms_classified_as, peer_comms_trusted_snapshot, peer_comms_submission_queue, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_input_kind, runtime_ingress_policy_snapshot, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

CoreNext ==
    \/ DeliverQueuedRoute
    \/ \E arg_peer_id \in PeerIdValues : peer_comms_TrustPeer(arg_peer_id)
    \/ \E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : peer_comms_ReceiveTrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind)
    \/ \E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : peer_comms_DropUntrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind)
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
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : runtime_control_SubmitCandidateFromIdle(arg_candidate_id, arg_candidate_kind)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : runtime_control_SubmitCandidateFromRunning(arg_candidate_id, arg_candidate_kind)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleNone(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleWake(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleWakeAndProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningNone(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningWake(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningWakeAndProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_candidate_id, arg_reason)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_candidate_id, arg_reason)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_existing_input_id \in InputIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_candidate_id, arg_existing_input_id)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_existing_input_id \in InputIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_candidate_id, arg_existing_input_id)
    \/ runtime_control_ExternalToolDeltaReceivedIdle
    \/ runtime_control_ExternalToolDeltaReceivedRunning
    \/ runtime_control_ExternalToolDeltaReceivedRecovering
    \/ runtime_control_ExternalToolDeltaReceivedRetired
    \/ \E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedNone(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)
    \/ \E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedWake(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)
    \/ \E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedProcess(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)
    \/ \E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedWakeAndProcess(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)
    \/ \E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_input_id, arg_input_kind, arg_policy)
    \/ \E arg_run_id \in RunIdValues : \E arg_contributing_input_ids \in SeqOfInputIdValues : runtime_ingress_StageDrainSnapshot(arg_run_id, arg_contributing_input_ids)
    \/ \E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryApplied(arg_run_id, arg_boundary_sequence)
    \/ \E arg_run_id \in RunIdValues : runtime_ingress_RunCompleted(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_ingress_RunFailed(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_ingress_RunCancelled(arg_run_id)
    \/ \E arg_new_input_id \in InputIdValues : \E arg_old_input_id \in InputIdValues : runtime_ingress_SupersedeQueuedInput(arg_new_input_id, arg_old_input_id)
    \/ \E arg_aggregate_input_id \in InputIdValues : \E arg_source_input_ids \in SeqOfInputIdValues : runtime_ingress_CoalesceQueuedInputs(arg_aggregate_input_id, arg_source_input_ids)
    \/ runtime_ingress_Retire
    \/ runtime_ingress_ResetFromActive
    \/ runtime_ingress_ResetFromRetired
    \/ runtime_ingress_Destroy
    \/ runtime_ingress_RecoverFromActive
    \/ runtime_ingress_RecoverFromRetired
    \/ QuiescentStutter

InjectNext ==
    \/ Inject_control_initialize
    \/ \E arg_peer_id \in PeerIdValues : Inject_trust_peer(arg_peer_id)
    \/ \E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : Inject_receive_peer_envelope(arg_raw_item_id, arg_peer_id, arg_raw_kind)
    \/ \E arg_raw_item_id \in RawItemIdValues : Inject_submit_typed_peer_input(arg_raw_item_id)
    \/ \E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : Inject_runtime_admission_accepted(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)
    \/ \E arg_run_id \in RunIdValues : \E arg_contributing_input_ids \in SeqOfInputIdValues : Inject_ingress_stage_drain_snapshot(arg_run_id, arg_contributing_input_ids)

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


peer_work_enters_runtime_via_canonical_admission == \A input_packet \in observed_inputs : ((input_packet.machine = "runtime_control" /\ input_packet.variant = "SubmitCandidate") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "peer_comms" /\ input_packet.source_effect = "SubmitPeerInputCandidate" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "peer_comms" /\ effect_packet.variant = "SubmitPeerInputCandidate" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = input_packet.source_route /\ route_packet.source_machine = "peer_comms" /\ route_packet.effect = "SubmitPeerInputCandidate" /\ route_packet.target_machine = "runtime_control" /\ route_packet.target_input = "SubmitCandidate" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))
peer_admission_flows_into_ingress == \A input_packet \in observed_inputs : ((input_packet.machine = "runtime_ingress" /\ input_packet.variant = "AdmitQueued") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "runtime_control" /\ input_packet.source_effect = "SubmitAdmittedIngressEffect" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "runtime_control" /\ effect_packet.variant = "SubmitAdmittedIngressEffect" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = input_packet.source_route /\ route_packet.source_machine = "runtime_control" /\ route_packet.effect = "SubmitAdmittedIngressEffect" /\ route_packet.target_machine = "runtime_ingress" /\ route_packet.target_input = "AdmitQueued" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))
control_preempts_peer_delivery == <<"PreemptWhenReady", "control_plane", "peer_plane">> \in SchedulerRules

RouteObserved_peer_candidate_enters_runtime_admission == \E packet \in RoutePackets : packet.route = "peer_candidate_enters_runtime_admission"
RouteCoverage_peer_candidate_enters_runtime_admission == (RouteObserved_peer_candidate_enters_runtime_admission \/ ~RouteObserved_peer_candidate_enters_runtime_admission)
RouteObserved_admitted_peer_work_enters_ingress == \E packet \in RoutePackets : packet.route = "admitted_peer_work_enters_ingress"
RouteCoverage_admitted_peer_work_enters_ingress == (RouteObserved_admitted_peer_work_enters_ingress \/ ~RouteObserved_admitted_peer_work_enters_ingress)
SchedulerTriggered_PreemptWhenReady_control_plane_peer_plane == /\ "control_plane" \in PendingActors /\ "peer_plane" \in PendingActors
SchedulerCoverage_PreemptWhenReady_control_plane_peer_plane == (SchedulerTriggered_PreemptWhenReady_control_plane_peer_plane \/ ~SchedulerTriggered_PreemptWhenReady_control_plane_peer_plane)
CoverageInstrumentation == RouteCoverage_peer_candidate_enters_runtime_admission /\ RouteCoverage_admitted_peer_work_enters_ingress /\ SchedulerCoverage_PreemptWhenReady_control_plane_peer_plane

CiStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 1 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(peer_comms_trusted_peers) <= 1 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 1 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 1 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 1 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 1 /\ Len(peer_comms_submission_queue) <= 1 /\ Cardinality(runtime_ingress_admitted_inputs) <= 1 /\ Len(runtime_ingress_admission_order) <= 1 /\ Cardinality(DOMAIN runtime_ingress_input_kind) <= 1 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 1 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 1 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 1 /\ Len(runtime_ingress_queue) <= 1 /\ Len(runtime_ingress_current_run_contributors) <= 1 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 1 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 1
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(peer_comms_trusted_peers) <= 2 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 2 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 2 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 2 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 2 /\ Len(peer_comms_submission_queue) <= 2 /\ Cardinality(runtime_ingress_admitted_inputs) <= 2 /\ Len(runtime_ingress_admission_order) <= 2 /\ Cardinality(DOMAIN runtime_ingress_input_kind) <= 2 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 2 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 2 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 2 /\ Len(runtime_ingress_queue) <= 2 /\ Len(runtime_ingress_current_run_contributors) <= 2 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 2 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 2
WitnessStateConstraint_trusted_peer_enters_runtime == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 4 /\ Cardinality(observed_inputs) <= 8 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 4 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(peer_comms_trusted_peers) <= 4 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 4 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 4 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 4 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 4 /\ Len(peer_comms_submission_queue) <= 4 /\ Cardinality(runtime_ingress_admitted_inputs) <= 4 /\ Len(runtime_ingress_admission_order) <= 4 /\ Cardinality(DOMAIN runtime_ingress_input_kind) <= 4 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 4 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 4 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 4 /\ Len(runtime_ingress_queue) <= 4 /\ Len(runtime_ingress_current_run_contributors) <= 4 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 4 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 4
WitnessStateConstraint_admitted_peer_work_enters_ingress == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 5 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 3 /\ Cardinality(emitted_effects) <= 3 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(peer_comms_trusted_peers) <= 5 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 5 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 5 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 5 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 5 /\ Len(peer_comms_submission_queue) <= 5 /\ Cardinality(runtime_ingress_admitted_inputs) <= 5 /\ Len(runtime_ingress_admission_order) <= 5 /\ Cardinality(DOMAIN runtime_ingress_input_kind) <= 5 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 5 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 5 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 5 /\ Len(runtime_ingress_queue) <= 5 /\ Len(runtime_ingress_current_run_contributors) <= 5 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 5 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 5
WitnessStateConstraint_control_preempts_peer_delivery == /\ model_step_count <= 5 /\ Len(pending_inputs) <= 3 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 5 /\ Cardinality(peer_comms_trusted_peers) <= 3 /\ Cardinality(DOMAIN peer_comms_raw_item_peer) <= 3 /\ Cardinality(DOMAIN peer_comms_raw_item_kind) <= 3 /\ Cardinality(DOMAIN peer_comms_classified_as) <= 3 /\ Cardinality(DOMAIN peer_comms_trusted_snapshot) <= 3 /\ Len(peer_comms_submission_queue) <= 3 /\ Cardinality(runtime_ingress_admitted_inputs) <= 3 /\ Len(runtime_ingress_admission_order) <= 3 /\ Cardinality(DOMAIN runtime_ingress_input_kind) <= 3 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 3 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 3 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 3 /\ Len(runtime_ingress_queue) <= 3 /\ Len(runtime_ingress_current_run_contributors) <= 3 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 3 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 3

Spec == Init /\ [][Next]_vars
WitnessSpec_trusted_peer_enters_runtime == WitnessInit_trusted_peer_enters_runtime /\ [] [WitnessNext_trusted_peer_enters_runtime]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_peer_id \in PeerIdValues : peer_comms_TrustPeer(arg_peer_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : peer_comms_ReceiveTrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : peer_comms_DropUntrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputDelivered(arg_raw_item_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputContinue(arg_raw_item_id)) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelled(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : runtime_control_SubmitCandidateFromIdle(arg_candidate_id, arg_candidate_kind)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : runtime_control_SubmitCandidateFromRunning(arg_candidate_id, arg_candidate_kind)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleNone(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleWake(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleWakeAndProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningNone(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningWake(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningWakeAndProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_candidate_id, arg_reason)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_candidate_id, arg_reason)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_existing_input_id \in InputIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_candidate_id, arg_existing_input_id)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_existing_input_id \in InputIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_candidate_id, arg_existing_input_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedNone(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedWake(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedProcess(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedWakeAndProcess(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_input_id, arg_input_kind, arg_policy)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_input_ids \in SeqOfInputIdValues : runtime_ingress_StageDrainSnapshot(arg_run_id, arg_contributing_input_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryApplied(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelled(arg_run_id)) /\ WF_vars(\E arg_new_input_id \in InputIdValues : \E arg_old_input_id \in InputIdValues : runtime_ingress_SupersedeQueuedInput(arg_new_input_id, arg_old_input_id)) /\ WF_vars(\E arg_aggregate_input_id \in InputIdValues : \E arg_source_input_ids \in SeqOfInputIdValues : runtime_ingress_CoalesceQueuedInputs(arg_aggregate_input_id, arg_source_input_ids)) /\ WF_vars(runtime_ingress_Retire) /\ WF_vars(runtime_ingress_ResetFromActive) /\ WF_vars(runtime_ingress_ResetFromRetired) /\ WF_vars(runtime_ingress_Destroy) /\ WF_vars(runtime_ingress_RecoverFromActive) /\ WF_vars(runtime_ingress_RecoverFromRetired) /\ WF_vars(WitnessInjectNext_trusted_peer_enters_runtime)
WitnessSpec_admitted_peer_work_enters_ingress == WitnessInit_admitted_peer_work_enters_ingress /\ [] [WitnessNext_admitted_peer_work_enters_ingress]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_peer_id \in PeerIdValues : peer_comms_TrustPeer(arg_peer_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : peer_comms_ReceiveTrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : peer_comms_DropUntrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputDelivered(arg_raw_item_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputContinue(arg_raw_item_id)) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelled(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : runtime_control_SubmitCandidateFromIdle(arg_candidate_id, arg_candidate_kind)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : runtime_control_SubmitCandidateFromRunning(arg_candidate_id, arg_candidate_kind)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleNone(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleWake(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleWakeAndProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningNone(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningWake(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningWakeAndProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_candidate_id, arg_reason)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_candidate_id, arg_reason)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_existing_input_id \in InputIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_candidate_id, arg_existing_input_id)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_existing_input_id \in InputIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_candidate_id, arg_existing_input_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedNone(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedWake(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedProcess(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedWakeAndProcess(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_input_id, arg_input_kind, arg_policy)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_input_ids \in SeqOfInputIdValues : runtime_ingress_StageDrainSnapshot(arg_run_id, arg_contributing_input_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryApplied(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelled(arg_run_id)) /\ WF_vars(\E arg_new_input_id \in InputIdValues : \E arg_old_input_id \in InputIdValues : runtime_ingress_SupersedeQueuedInput(arg_new_input_id, arg_old_input_id)) /\ WF_vars(\E arg_aggregate_input_id \in InputIdValues : \E arg_source_input_ids \in SeqOfInputIdValues : runtime_ingress_CoalesceQueuedInputs(arg_aggregate_input_id, arg_source_input_ids)) /\ WF_vars(runtime_ingress_Retire) /\ WF_vars(runtime_ingress_ResetFromActive) /\ WF_vars(runtime_ingress_ResetFromRetired) /\ WF_vars(runtime_ingress_Destroy) /\ WF_vars(runtime_ingress_RecoverFromActive) /\ WF_vars(runtime_ingress_RecoverFromRetired) /\ WF_vars(WitnessInjectNext_admitted_peer_work_enters_ingress)
WitnessSpec_control_preempts_peer_delivery == WitnessInit_control_preempts_peer_delivery /\ [] [WitnessNext_control_preempts_peer_delivery]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_peer_id \in PeerIdValues : peer_comms_TrustPeer(arg_peer_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : peer_comms_ReceiveTrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : \E arg_peer_id \in PeerIdValues : \E arg_raw_kind \in RawPeerKindValues : peer_comms_DropUntrustedPeerEnvelope(arg_raw_item_id, arg_peer_id, arg_raw_kind)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputDelivered(arg_raw_item_id)) /\ WF_vars(\E arg_raw_item_id \in RawItemIdValues : peer_comms_SubmitTypedPeerInputContinue(arg_raw_item_id)) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelled(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : runtime_control_SubmitCandidateFromIdle(arg_candidate_id, arg_candidate_kind)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : runtime_control_SubmitCandidateFromRunning(arg_candidate_id, arg_candidate_kind)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleNone(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleWake(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedIdleWakeAndProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningNone(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningWake(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_candidate_kind \in InputKindValues : \E arg_admission_effect \in AdmissionEffectValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_control_AdmissionAcceptedRunningWakeAndProcess(arg_candidate_id, arg_candidate_kind, arg_admission_effect, arg_wake, arg_process)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_candidate_id, arg_reason)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_candidate_id, arg_reason)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_existing_input_id \in InputIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_candidate_id, arg_existing_input_id)) /\ WF_vars(\E arg_candidate_id \in CandidateIdValues : \E arg_existing_input_id \in InputIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_candidate_id, arg_existing_input_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedNone(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedWake(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedProcess(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : \E arg_wake \in BOOLEAN : \E arg_process \in BOOLEAN : runtime_ingress_AdmitQueuedWakeAndProcess(arg_input_id, arg_input_kind, arg_policy, arg_wake, arg_process)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_input_kind \in InputKindValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_input_id, arg_input_kind, arg_policy)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_input_ids \in SeqOfInputIdValues : runtime_ingress_StageDrainSnapshot(arg_run_id, arg_contributing_input_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryApplied(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelled(arg_run_id)) /\ WF_vars(\E arg_new_input_id \in InputIdValues : \E arg_old_input_id \in InputIdValues : runtime_ingress_SupersedeQueuedInput(arg_new_input_id, arg_old_input_id)) /\ WF_vars(\E arg_aggregate_input_id \in InputIdValues : \E arg_source_input_ids \in SeqOfInputIdValues : runtime_ingress_CoalesceQueuedInputs(arg_aggregate_input_id, arg_source_input_ids)) /\ WF_vars(runtime_ingress_Retire) /\ WF_vars(runtime_ingress_ResetFromActive) /\ WF_vars(runtime_ingress_ResetFromRetired) /\ WF_vars(runtime_ingress_Destroy) /\ WF_vars(runtime_ingress_RecoverFromActive) /\ WF_vars(runtime_ingress_RecoverFromRetired) /\ WF_vars(WitnessInjectNext_control_preempts_peer_delivery)

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
WitnessTransitionObserved_admitted_peer_work_enters_ingress_runtime_control_AdmissionAcceptedIdleWakeAndProcess == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "AdmissionAcceptedIdleWakeAndProcess")
WitnessTransitionObserved_admitted_peer_work_enters_ingress_runtime_ingress_AdmitQueuedWakeAndProcess == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "AdmitQueuedWakeAndProcess")
WitnessTransitionOrder_admitted_peer_work_enters_ingress_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "peer_comms" /\ earlier.transition = "SubmitTypedPeerInputDelivered" /\ later.machine = "runtime_control" /\ later.transition = "AdmissionAcceptedIdleWakeAndProcess" /\ earlier.step < later.step)
WitnessTransitionOrder_admitted_peer_work_enters_ingress_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "AdmissionAcceptedIdleWakeAndProcess" /\ later.machine = "runtime_ingress" /\ later.transition = "AdmitQueuedWakeAndProcess" /\ earlier.step < later.step)
WitnessSchedulerTriggered_control_preempts_peer_delivery_PreemptWhenReady_control_plane_peer_plane == <> SchedulerTriggered_PreemptWhenReady_control_plane_peer_plane
WitnessStateObserved_control_preempts_peer_delivery_1 == <> (runtime_control_phase = "Idle")
WitnessTransitionObserved_control_preempts_peer_delivery_runtime_control_Initialize == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "Initialize")
WitnessTransitionObserved_control_preempts_peer_delivery_peer_comms_TrustPeer == <> (\E packet \in observed_transitions : /\ packet.machine = "peer_comms" /\ packet.transition = "TrustPeer")
WitnessTransitionOrder_control_preempts_peer_delivery_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "Initialize" /\ later.machine = "peer_comms" /\ later.transition = "TrustPeer" /\ earlier.step < later.step)

THEOREM Spec => []peer_work_enters_runtime_via_canonical_admission
THEOREM Spec => []peer_admission_flows_into_ingress
THEOREM Spec => []control_preempts_peer_delivery
THEOREM Spec => []peer_comms_queued_items_are_classified
THEOREM Spec => []runtime_control_running_implies_active_run
THEOREM Spec => []runtime_control_active_run_only_while_running_or_retired
THEOREM Spec => []runtime_ingress_queue_entries_are_queued
THEOREM Spec => []runtime_ingress_terminal_inputs_do_not_appear_in_queue
THEOREM Spec => []runtime_ingress_current_run_matches_contributor_presence
THEOREM Spec => []runtime_ingress_staged_contributors_are_not_queued
THEOREM Spec => []runtime_ingress_applied_pending_consumption_has_last_run

=============================================================================
