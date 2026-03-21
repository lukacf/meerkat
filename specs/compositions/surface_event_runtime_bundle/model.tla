---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for surface_event_runtime_bundle.

CONSTANTS AdmissionEffectValues, AsyncOpRefValues, BooleanValues, ContentShapeValues, HandlingModeValues, NatValues, OperationIdValues, PolicyDecisionValues, RequestIdValues, ReservationKeyValues, RunIdValues, SetOfStringValues, StringValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

SeqOfAsyncOpRefValues == {<<>>} \cup {<<x>> : x \in AsyncOpRefValues} \cup {<<x, y>> : x \in AsyncOpRefValues, y \in AsyncOpRefValues}
SeqOfOperationIdValues == {<<>>} \cup {<<x>> : x \in OperationIdValues} \cup {<<x, y>> : x \in OperationIdValues, y \in OperationIdValues}
SeqOfWorkIdValues == {<<>>} \cup {<<x>> : x \in WorkIdValues} \cup {<<x, y>> : x \in WorkIdValues, y \in WorkIdValues}
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
    <<"runtime_control", "RuntimeControlMachine", "control_plane">>,
    <<"runtime_ingress", "RuntimeIngressMachine", "ordinary_ingress">>,
    <<"turn_execution", "TurnExecutionMachine", "turn_executor">>
}

RouteNames == {
    "surface_event_enters_ingress",
    "surface_ingress_ready_starts_runtime_control",
    "surface_runtime_control_starts_execution",
    "surface_execution_boundary_updates_ingress",
    "surface_execution_completion_updates_ingress",
    "surface_execution_completion_notifies_control",
    "surface_execution_failure_updates_ingress",
    "surface_execution_failure_notifies_control",
    "surface_execution_cancel_updates_ingress",
    "surface_execution_cancel_notifies_control"
}

Actors == {
    "control_plane",
    "ordinary_ingress",
    "turn_executor"
}

ActorPriorities == {
    <<"control_plane", "ordinary_ingress">>
}

SchedulerRules == {
    <<"PreemptWhenReady", "control_plane", "ordinary_ingress">>
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "runtime_control" -> "control_plane"
      [] machine_id = "runtime_ingress" -> "ordinary_ingress"
      [] machine_id = "turn_execution" -> "turn_executor"
      [] OTHER -> "unknown_actor"

RouteSource(route_name) ==
    CASE route_name = "surface_event_enters_ingress" -> "runtime_control"
      [] route_name = "surface_ingress_ready_starts_runtime_control" -> "runtime_ingress"
      [] route_name = "surface_runtime_control_starts_execution" -> "runtime_control"
      [] route_name = "surface_execution_boundary_updates_ingress" -> "turn_execution"
      [] route_name = "surface_execution_completion_updates_ingress" -> "turn_execution"
      [] route_name = "surface_execution_completion_notifies_control" -> "turn_execution"
      [] route_name = "surface_execution_failure_updates_ingress" -> "turn_execution"
      [] route_name = "surface_execution_failure_notifies_control" -> "turn_execution"
      [] route_name = "surface_execution_cancel_updates_ingress" -> "turn_execution"
      [] route_name = "surface_execution_cancel_notifies_control" -> "turn_execution"
      [] OTHER -> "unknown_machine"

RouteEffect(route_name) ==
    CASE route_name = "surface_event_enters_ingress" -> "SubmitAdmittedIngressEffect"
      [] route_name = "surface_ingress_ready_starts_runtime_control" -> "ReadyForRun"
      [] route_name = "surface_runtime_control_starts_execution" -> "SubmitRunPrimitive"
      [] route_name = "surface_execution_boundary_updates_ingress" -> "BoundaryApplied"
      [] route_name = "surface_execution_completion_updates_ingress" -> "RunCompleted"
      [] route_name = "surface_execution_completion_notifies_control" -> "RunCompleted"
      [] route_name = "surface_execution_failure_updates_ingress" -> "RunFailed"
      [] route_name = "surface_execution_failure_notifies_control" -> "RunFailed"
      [] route_name = "surface_execution_cancel_updates_ingress" -> "RunCancelled"
      [] route_name = "surface_execution_cancel_notifies_control" -> "RunCancelled"
      [] OTHER -> "unknown_effect"

RouteTargetMachine(route_name) ==
    CASE route_name = "surface_event_enters_ingress" -> "runtime_ingress"
      [] route_name = "surface_ingress_ready_starts_runtime_control" -> "runtime_control"
      [] route_name = "surface_runtime_control_starts_execution" -> "turn_execution"
      [] route_name = "surface_execution_boundary_updates_ingress" -> "runtime_ingress"
      [] route_name = "surface_execution_completion_updates_ingress" -> "runtime_ingress"
      [] route_name = "surface_execution_completion_notifies_control" -> "runtime_control"
      [] route_name = "surface_execution_failure_updates_ingress" -> "runtime_ingress"
      [] route_name = "surface_execution_failure_notifies_control" -> "runtime_control"
      [] route_name = "surface_execution_cancel_updates_ingress" -> "runtime_ingress"
      [] route_name = "surface_execution_cancel_notifies_control" -> "runtime_control"
      [] OTHER -> "unknown_machine"

RouteTargetInput(route_name) ==
    CASE route_name = "surface_event_enters_ingress" -> "AdmitQueued"
      [] route_name = "surface_ingress_ready_starts_runtime_control" -> "BeginRun"
      [] route_name = "surface_runtime_control_starts_execution" -> "StartConversationRun"
      [] route_name = "surface_execution_boundary_updates_ingress" -> "BoundaryApplied"
      [] route_name = "surface_execution_completion_updates_ingress" -> "RunCompleted"
      [] route_name = "surface_execution_completion_notifies_control" -> "RunCompleted"
      [] route_name = "surface_execution_failure_updates_ingress" -> "RunFailed"
      [] route_name = "surface_execution_failure_notifies_control" -> "RunFailed"
      [] route_name = "surface_execution_cancel_updates_ingress" -> "RunCancelled"
      [] route_name = "surface_execution_cancel_notifies_control" -> "RunCancelled"
      [] OTHER -> "unknown_input"

RouteDeliveryKind(route_name) ==
    CASE route_name = "surface_event_enters_ingress" -> "Immediate"
      [] route_name = "surface_ingress_ready_starts_runtime_control" -> "Immediate"
      [] route_name = "surface_runtime_control_starts_execution" -> "Immediate"
      [] route_name = "surface_execution_boundary_updates_ingress" -> "Immediate"
      [] route_name = "surface_execution_completion_updates_ingress" -> "Immediate"
      [] route_name = "surface_execution_completion_notifies_control" -> "Immediate"
      [] route_name = "surface_execution_failure_updates_ingress" -> "Immediate"
      [] route_name = "surface_execution_failure_notifies_control" -> "Immediate"
      [] route_name = "surface_execution_cancel_updates_ingress" -> "Immediate"
      [] route_name = "surface_execution_cancel_notifies_control" -> "Immediate"
      [] OTHER -> "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
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
    /\ turn_execution_phase = "Ready"
    /\ turn_execution_active_run = None
    /\ turn_execution_primitive_kind = "None"
    /\ turn_execution_admitted_content_shape = None
    /\ turn_execution_vision_enabled = FALSE
    /\ turn_execution_image_tool_results_enabled = FALSE
    /\ turn_execution_tool_calls_pending = 0
    /\ turn_execution_pending_op_refs = None
    /\ turn_execution_barrier_operation_ids = <<>>
    /\ turn_execution_has_barrier_ops = FALSE
    /\ turn_execution_barrier_satisfied = TRUE
    /\ turn_execution_boundary_count = 0
    /\ turn_execution_cancel_after_boundary = FALSE
    /\ turn_execution_terminal_outcome = "None"
    /\ turn_execution_extraction_attempts = 0
    /\ turn_execution_max_extraction_retries = 0
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

WitnessInit_surface_event_success_path ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_success_path:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_success_path:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_success_path:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> "TextOnly", handling_mode |-> "Queue", request_id |-> None, reservation_key |-> None, work_id |-> "external_evt_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_success_path:2", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0], [machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> "SubmitAdmittedIngressEffect", content_shape |-> "TextOnly", handling_mode |-> "Queue", request_id |-> None, reservation_key |-> None, work_id |-> "external_evt_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_success_path:3", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0], [machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_work_ids |-> <<"external_evt_1">>, run_id |-> "runid_surface_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_success_path:4", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "PrimitiveApplied", payload |-> [admitted_content_shape |-> "TextOnly", image_tool_results_enabled |-> FALSE, run_id |-> "runid_surface_1", vision_enabled |-> FALSE], source_kind |-> "entry", source_route |-> "witness:surface_event_success_path:5", source_machine |-> "external_entry", source_effect |-> "PrimitiveApplied", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "LlmReturnedTerminal", payload |-> [run_id |-> "runid_surface_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_success_path:6", source_machine |-> "external_entry", source_effect |-> "LlmReturnedTerminal", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "BoundaryComplete", payload |-> [run_id |-> "runid_surface_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_success_path:7", source_machine |-> "external_entry", source_effect |-> "BoundaryComplete", effect_id |-> 0]>>

WitnessInit_surface_event_inline_image_success_path ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_inline_image_success_path:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_inline_image_success_path:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_inline_image_success_path:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> "InlineImage", handling_mode |-> "Queue", request_id |-> None, reservation_key |-> None, work_id |-> "external_evt_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_inline_image_success_path:2", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0], [machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> "SubmitAdmittedIngressEffect", content_shape |-> "InlineImage", handling_mode |-> "Queue", request_id |-> None, reservation_key |-> None, work_id |-> "external_evt_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_inline_image_success_path:3", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0], [machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_work_ids |-> <<"external_evt_1">>, run_id |-> "runid_surface_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_inline_image_success_path:4", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "PrimitiveApplied", payload |-> [admitted_content_shape |-> "InlineImage", image_tool_results_enabled |-> FALSE, run_id |-> "runid_surface_1", vision_enabled |-> FALSE], source_kind |-> "entry", source_route |-> "witness:surface_event_inline_image_success_path:5", source_machine |-> "external_entry", source_effect |-> "PrimitiveApplied", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "LlmReturnedTerminal", payload |-> [run_id |-> "runid_surface_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_inline_image_success_path:6", source_machine |-> "external_entry", source_effect |-> "LlmReturnedTerminal", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "BoundaryComplete", payload |-> [run_id |-> "runid_surface_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_inline_image_success_path:7", source_machine |-> "external_entry", source_effect |-> "BoundaryComplete", effect_id |-> 0]>>

WitnessInit_surface_event_failure_path ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_failure_path:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_failure_path:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_failure_path:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> "TextOnly", handling_mode |-> "Queue", request_id |-> None, reservation_key |-> None, work_id |-> "external_evt_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_failure_path:2", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0], [machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> "SubmitAdmittedIngressEffect", content_shape |-> "TextOnly", handling_mode |-> "Queue", request_id |-> None, reservation_key |-> None, work_id |-> "external_evt_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_failure_path:3", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0], [machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_work_ids |-> <<"external_evt_1">>, run_id |-> "runid_surface_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_failure_path:4", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "FatalFailure", payload |-> [run_id |-> "runid_surface_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_failure_path:5", source_machine |-> "external_entry", source_effect |-> "FatalFailure", effect_id |-> 0]>>

WitnessInit_surface_event_cancel_path ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_cancel_path:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_cancel_path:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_cancel_path:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> "TextOnly", handling_mode |-> "Queue", request_id |-> None, reservation_key |-> None, work_id |-> "external_evt_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_cancel_path:2", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0], [machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> "SubmitAdmittedIngressEffect", content_shape |-> "TextOnly", handling_mode |-> "Queue", request_id |-> None, reservation_key |-> None, work_id |-> "external_evt_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_cancel_path:3", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0], [machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_work_ids |-> <<"external_evt_1">>, run_id |-> "runid_surface_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_cancel_path:4", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "CancelNow", payload |-> [run_id |-> "runid_surface_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_cancel_path:5", source_machine |-> "external_entry", source_effect |-> "CancelNow", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "CancellationObserved", payload |-> [run_id |-> "runid_surface_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_cancel_path:6", source_machine |-> "external_entry", source_effect |-> "CancellationObserved", effect_id |-> 0]>>

WitnessInit_surface_event_control_preemption ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_control_preemption:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0], [machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> "TextOnly", handling_mode |-> "Queue", request_id |-> None, reservation_key |-> None, work_id |-> "external_evt_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_control_preemption:2", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_event_control_preemption:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0], [machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> "TextOnly", handling_mode |-> "Queue", request_id |-> None, reservation_key |-> None, work_id |-> "external_evt_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_control_preemption:2", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> "TextOnly", handling_mode |-> "Queue", request_id |-> None, reservation_key |-> None, work_id |-> "external_evt_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_control_preemption:2", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> "SubmitAdmittedIngressEffect", content_shape |-> "TextOnly", handling_mode |-> "Steer", request_id |-> None, reservation_key |-> None, work_id |-> "external_evt_1"], source_kind |-> "entry", source_route |-> "witness:surface_event_control_preemption:3", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0]>>

runtime_control_Initialize ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "Initialize"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Initializing"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "Initialize", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AttachFromIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AttachExecutor"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ runtime_control_phase' = "Attached"
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AttachFromIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_DetachToIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "DetachExecutor"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Attached"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "DetachToIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
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
       /\ UNCHANGED << runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_runtime_control_starts_execution", source_machine |-> "runtime_control", source_effect |-> "SubmitRunPrimitive", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_runtime_control_starts_execution", source_machine |-> "runtime_control", source_effect |-> "SubmitRunPrimitive", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_runtime_control_starts_execution", source_machine |-> "runtime_control", effect |-> "SubmitRunPrimitive", target_machine |-> "turn_execution", target_input |-> "StartConversationRun", payload |-> [run_id |-> packet.payload.run_id], actor |-> "turn_executor", effect_id |-> (model_step_count + 1), source_transition |-> "BeginRunFromIdle"] }
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
       /\ UNCHANGED << runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_runtime_control_starts_execution", source_machine |-> "runtime_control", source_effect |-> "SubmitRunPrimitive", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_runtime_control_starts_execution", source_machine |-> "runtime_control", source_effect |-> "SubmitRunPrimitive", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_runtime_control_starts_execution", source_machine |-> "runtime_control", effect |-> "SubmitRunPrimitive", target_machine |-> "turn_execution", target_input |-> "StartConversationRun", payload |-> [run_id |-> packet.payload.run_id], actor |-> "turn_executor", effect_id |-> (model_step_count + 1), source_transition |-> "BeginRunFromRetired"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitRunPrimitive", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRunFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "BeginRunFromRetired", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_BeginRunFromAttached(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "BeginRun"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Attached"
       /\ (runtime_control_current_run_id = None)
       /\ runtime_control_phase' = "Running"
       /\ runtime_control_current_run_id' = Some(packet.payload.run_id)
       /\ runtime_control_pre_run_state' = Some("Attached")
       /\ runtime_control_wake_pending' = FALSE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_runtime_control_starts_execution", source_machine |-> "runtime_control", source_effect |-> "SubmitRunPrimitive", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_runtime_control_starts_execution", source_machine |-> "runtime_control", source_effect |-> "SubmitRunPrimitive", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_runtime_control_starts_execution", source_machine |-> "runtime_control", effect |-> "SubmitRunPrimitive", target_machine |-> "turn_execution", target_input |-> "StartConversationRun", payload |-> [run_id |-> packet.payload.run_id], actor |-> "turn_executor", effect_id |-> (model_step_count + 1), source_transition |-> "BeginRunFromAttached"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitRunPrimitive", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRunFromAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "BeginRunFromAttached", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_BeginRunFromRecovering(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "BeginRun"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Recovering"
       /\ (runtime_control_current_run_id = None)
       /\ runtime_control_phase' = "Running"
       /\ runtime_control_current_run_id' = Some(packet.payload.run_id)
       /\ runtime_control_pre_run_state' = Some("Recovering")
       /\ runtime_control_wake_pending' = FALSE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_runtime_control_starts_execution", source_machine |-> "runtime_control", source_effect |-> "SubmitRunPrimitive", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_runtime_control_starts_execution", source_machine |-> "runtime_control", source_effect |-> "SubmitRunPrimitive", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_runtime_control_starts_execution", source_machine |-> "runtime_control", effect |-> "SubmitRunPrimitive", target_machine |-> "turn_execution", target_input |-> "StartConversationRun", payload |-> [run_id |-> packet.payload.run_id], actor |-> "turn_executor", effect_id |-> (model_step_count + 1), source_transition |-> "BeginRunFromRecovering"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitRunPrimitive", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRunFromRecovering"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "BeginRunFromRecovering", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunCompletedToIdle(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunCompleted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ ((runtime_control_pre_run_state = None) \/ (runtime_control_pre_run_state = Some("Idle")) \/ (runtime_control_pre_run_state = Some("Recovering")))
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunCompletedToIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunCompletedToAttached(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunCompleted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ (runtime_control_pre_run_state = Some("Attached"))
       /\ runtime_control_phase' = "Attached"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunCompletedToAttached", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunCompletedToRetired(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunCompleted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ (runtime_control_pre_run_state = Some("Retired"))
       /\ runtime_control_phase' = "Retired"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunCompletedToRetired", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunFailedToIdle(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunFailed"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ ((runtime_control_pre_run_state = None) \/ (runtime_control_pre_run_state = Some("Idle")) \/ (runtime_control_pre_run_state = Some("Recovering")))
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunFailedToIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunFailedToAttached(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunFailed"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ (runtime_control_pre_run_state = Some("Attached"))
       /\ runtime_control_phase' = "Attached"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunFailedToAttached", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunFailedToRetired(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunFailed"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ (runtime_control_pre_run_state = Some("Retired"))
       /\ runtime_control_phase' = "Retired"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunFailedToRetired", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunCancelledToIdle(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunCancelled"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ ((runtime_control_pre_run_state = None) \/ (runtime_control_pre_run_state = Some("Idle")) \/ (runtime_control_pre_run_state = Some("Recovering")))
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunCancelledToIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunCancelledToAttached(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunCancelled"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ (runtime_control_pre_run_state = Some("Attached"))
       /\ runtime_control_phase' = "Attached"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunCancelledToAttached", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunCancelledToRetired(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunCancelled"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Running"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ (runtime_control_pre_run_state = Some("Retired"))
       /\ runtime_control_phase' = "Retired"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunCancelledToRetired", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunCompletedFromRetiredInFlight(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunCompleted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Retired"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ runtime_control_phase' = "Retired"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunCompletedFromRetiredInFlight", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunFailedFromRetiredInFlight(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunFailed"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Retired"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ runtime_control_phase' = "Retired"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunFailedFromRetiredInFlight", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RunCancelledFromRetiredInFlight(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RunCancelled"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Retired"
       /\ (runtime_control_current_run_id = Some(packet.payload.run_id))
       /\ runtime_control_phase' = "Retired"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RunCancelledFromRetiredInFlight", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RecoverRequestedFromIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RecoverRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ runtime_control_phase' = "Recovering"
       /\ runtime_control_pre_run_state' = Some("Idle")
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RecoverRequestedFromRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Recovering"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RecoverRequestedFromAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RecoverRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Attached"
       /\ runtime_control_phase' = "Recovering"
       /\ runtime_control_pre_run_state' = Some("Attached")
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RecoverRequestedFromAttached", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Recovering"]}
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
       /\ UNCHANGED << runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ApplyControlPlaneCommand", payload |-> [command |-> "Retire"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireRequestedFromRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RetireRequestedFromRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RetireRequestedFromAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RetireRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Attached"
       /\ runtime_control_phase' = "Retired"
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ApplyControlPlaneCommand", payload |-> [command |-> "Retire"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireRequestedFromAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RetireRequestedFromAttached", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_ResetRequested ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "ResetRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Initializing" \/ runtime_control_phase = "Idle" \/ runtime_control_phase = "Attached" \/ runtime_control_phase = "Recovering" \/ runtime_control_phase = "Retired"
       /\ runtime_control_phase' = "Idle"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ runtime_control_wake_pending' = FALSE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ runtime_control_phase = "Initializing" \/ runtime_control_phase = "Idle" \/ runtime_control_phase = "Attached" \/ runtime_control_phase = "Running" \/ runtime_control_phase = "Recovering" \/ runtime_control_phase = "Retired"
       /\ runtime_control_phase' = "Stopped"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ runtime_control_phase = "Initializing" \/ runtime_control_phase = "Idle" \/ runtime_control_phase = "Attached" \/ runtime_control_phase = "Running" \/ runtime_control_phase = "Recovering" \/ runtime_control_phase = "Retired" \/ runtime_control_phase = "Stopped"
       /\ runtime_control_phase' = "Destroyed"
       /\ runtime_control_current_run_id' = None
       /\ runtime_control_pre_run_state' = None
       /\ UNCHANGED << runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ResolveAdmission", payload |-> [work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitWorkFromRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "SubmitWorkFromRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_SubmitWorkFromAttached(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "SubmitWork"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.content_shape = arg_content_shape
       /\ packet.payload.handling_mode = arg_handling_mode
       /\ packet.payload.request_id = arg_request_id
       /\ packet.payload.reservation_key = arg_reservation_key
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Attached"
       /\ runtime_control_phase' = "Attached"
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "ResolveAdmission", payload |-> [work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitWorkFromAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "SubmitWorkFromAttached", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Attached"]}
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleQueue"] }
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedIdleSteer"] }
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningQueue"] }
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningSteer"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningSteer"], [machine |-> "runtime_control", variant |-> "SignalWake", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningSteer"], [machine |-> "runtime_control", variant |-> "SignalImmediateProcess", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedRunningSteer"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedRunningSteer", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedAttachedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect) ==
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
       /\ runtime_control_phase = "Attached"
       /\ (packet.payload.handling_mode = "Queue")
       /\ runtime_control_phase' = "Attached"
       /\ runtime_control_wake_pending' = TRUE
       /\ runtime_control_process_pending' = FALSE
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedAttachedQueue"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedAttachedQueue"], [machine |-> "runtime_control", variant |-> "SignalWake", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedAttachedQueue"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedAttachedQueue", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionAcceptedAttachedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect) ==
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
       /\ runtime_control_phase = "Attached"
       /\ (packet.payload.handling_mode = "Steer")
       /\ runtime_control_phase' = "Attached"
       /\ runtime_control_wake_pending' = TRUE
       /\ runtime_control_process_pending' = TRUE
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", source_effect |-> "SubmitAdmittedIngressEffect", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_event_enters_ingress", source_machine |-> "runtime_control", effect |-> "SubmitAdmittedIngressEffect", target_machine |-> "runtime_ingress", target_input |-> "AdmitQueued", payload |-> [content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, policy |-> "ExternalEventQueued", request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedAttachedSteer"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "SubmitAdmittedIngressEffect", payload |-> [admission_effect |-> packet.payload.admission_effect, content_shape |-> packet.payload.content_shape, handling_mode |-> packet.payload.handling_mode, request_id |-> packet.payload.request_id, reservation_key |-> packet.payload.reservation_key, work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedAttachedSteer"], [machine |-> "runtime_control", variant |-> "SignalWake", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedAttachedSteer"], [machine |-> "runtime_control", variant |-> "SignalImmediateProcess", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionAcceptedAttachedSteer"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionAcceptedAttachedSteer", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Attached"]}
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> packet.payload.reason, kind |-> "AdmissionRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionRejectedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionRejectedRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionRejectedAttached(arg_work_id, arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionRejected"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Attached"
       /\ runtime_control_phase' = "Attached"
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> packet.payload.reason, kind |-> "AdmissionRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionRejectedAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionRejectedAttached", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Attached"]}
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "ExistingInputLinked", kind |-> "AdmissionDeduplicated"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionDeduplicatedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionDeduplicatedRunning", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_AdmissionDeduplicatedAttached(arg_work_id, arg_existing_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "AdmissionDeduplicated"
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.existing_work_id = arg_existing_work_id
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Attached"
       /\ runtime_control_phase' = "Attached"
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "ExistingInputLinked", kind |-> "AdmissionDeduplicated"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmissionDeduplicatedAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "AdmissionDeduplicatedAttached", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_ExternalToolDeltaReceivedIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "ExternalToolDeltaReceived"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Idle"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "Received", kind |-> "ExternalToolDelta"], effect_id |-> (model_step_count + 1), source_transition |-> "ExternalToolDeltaReceivedRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "ExternalToolDeltaReceivedRetired", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_ExternalToolDeltaReceivedAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "ExternalToolDeltaReceived"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Attached"
       /\ runtime_control_phase' = "Attached"
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "Received", kind |-> "ExternalToolDelta"], effect_id |-> (model_step_count + 1), source_transition |-> "ExternalToolDeltaReceivedAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "ExternalToolDeltaReceivedAttached", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Attached"]}
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "InitiateRecycle", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecycleRequestedFromIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RecycleRequestedFromIdle", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Recovering"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_RecycleRequestedFromAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "RecycleRequested"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Attached"
       /\ (runtime_control_current_run_id = None)
       /\ runtime_control_phase' = "Recovering"
       /\ runtime_control_pre_run_state' = Some("Attached")
       /\ UNCHANGED << runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "InitiateRecycle", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecycleRequestedFromAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RecycleRequestedFromAttached", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Recovering"]}
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
       /\ UNCHANGED << runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "Succeeded", kind |-> "Recycle"], effect_id |-> (model_step_count + 1), source_transition |-> "RecycleSucceeded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RecycleSucceeded", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_running_implies_active_run == ((runtime_control_phase # "Running") \/ (runtime_control_current_run_id # None))
runtime_control_active_run_only_while_running_or_retired == ((runtime_control_current_run_id = None) \/ (runtime_control_phase = "Running") \/ (runtime_control_phase = "Retired"))

RECURSIVE runtime_ingress_StageDrainSnapshotFromActive_ForEach0_last_run(_, _, _)
runtime_ingress_StageDrainSnapshotFromActive_ForEach0_last_run(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some(outer_run_id)) IN runtime_ingress_StageDrainSnapshotFromActive_ForEach0_last_run(next_acc, Tail(items), outer_run_id)

RECURSIVE runtime_ingress_StageDrainSnapshotFromActive_ForEach0_lifecycle(_, _, _)
runtime_ingress_StageDrainSnapshotFromActive_ForEach0_lifecycle(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Staged") IN runtime_ingress_StageDrainSnapshotFromActive_ForEach0_lifecycle(next_acc, Tail(items), outer_run_id)

RECURSIVE runtime_ingress_StageDrainSnapshotFromRetired_ForEach1_last_run(_, _, _)
runtime_ingress_StageDrainSnapshotFromRetired_ForEach1_last_run(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some(outer_run_id)) IN runtime_ingress_StageDrainSnapshotFromRetired_ForEach1_last_run(next_acc, Tail(items), outer_run_id)

RECURSIVE runtime_ingress_StageDrainSnapshotFromRetired_ForEach1_lifecycle(_, _, _)
runtime_ingress_StageDrainSnapshotFromRetired_ForEach1_lifecycle(acc, items, outer_run_id) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Staged") IN runtime_ingress_StageDrainSnapshotFromRetired_ForEach1_lifecycle(next_acc, Tail(items), outer_run_id)

RECURSIVE runtime_ingress_BoundaryAppliedFromActive_ForEach2_last_boundary_sequence(_, _, _)
runtime_ingress_BoundaryAppliedFromActive_ForEach2_last_boundary_sequence(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some(outer_boundary_sequence)) IN runtime_ingress_BoundaryAppliedFromActive_ForEach2_last_boundary_sequence(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE runtime_ingress_BoundaryAppliedFromActive_ForEach2_lifecycle(_, _, _)
runtime_ingress_BoundaryAppliedFromActive_ForEach2_lifecycle(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "AppliedPendingConsumption") IN runtime_ingress_BoundaryAppliedFromActive_ForEach2_lifecycle(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE runtime_ingress_BoundaryAppliedFromRetired_ForEach3_last_boundary_sequence(_, _, _)
runtime_ingress_BoundaryAppliedFromRetired_ForEach3_last_boundary_sequence(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some(outer_boundary_sequence)) IN runtime_ingress_BoundaryAppliedFromRetired_ForEach3_last_boundary_sequence(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE runtime_ingress_BoundaryAppliedFromRetired_ForEach3_lifecycle(_, _, _)
runtime_ingress_BoundaryAppliedFromRetired_ForEach3_lifecycle(acc, items, outer_boundary_sequence) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "AppliedPendingConsumption") IN runtime_ingress_BoundaryAppliedFromRetired_ForEach3_lifecycle(next_acc, Tail(items), outer_boundary_sequence)

RECURSIVE runtime_ingress_RunCompletedFromActive_ForEach4_lifecycle(_, _)
runtime_ingress_RunCompletedFromActive_ForEach4_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Consumed") IN runtime_ingress_RunCompletedFromActive_ForEach4_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_RunCompletedFromActive_ForEach4_terminal_outcome(_, _)
runtime_ingress_RunCompletedFromActive_ForEach4_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("Consumed")) IN runtime_ingress_RunCompletedFromActive_ForEach4_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_RunCompletedFromRetired_ForEach5_lifecycle(_, _)
runtime_ingress_RunCompletedFromRetired_ForEach5_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Consumed") IN runtime_ingress_RunCompletedFromRetired_ForEach5_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_RunCompletedFromRetired_ForEach5_terminal_outcome(_, _)
runtime_ingress_RunCompletedFromRetired_ForEach5_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("Consumed")) IN runtime_ingress_RunCompletedFromRetired_ForEach5_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_RunFailedFromActive_ForEach6_lifecycle(_, _, _)
runtime_ingress_RunFailedFromActive_ForEach6_lifecycle(acc, items, captured_handling_mode) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") = "Staged") THEN MapSet(acc, work_id, "Queued") ELSE acc IN runtime_ingress_RunFailedFromActive_ForEach6_lifecycle(next_acc, Tail(items), captured_handling_mode)

RECURSIVE runtime_ingress_RunFailedFromActive_ForEach6_queue(_, _, _, _)
runtime_ingress_RunFailedFromActive_ForEach6_queue(acc, items, captured_handling_mode, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN captured_handling_mode THEN captured_handling_mode[work_id] ELSE "None") = "Steer") THEN acc ELSE (<<work_id>> \o acc) ELSE acc IN runtime_ingress_RunFailedFromActive_ForEach6_queue(next_acc, Tail(items), captured_handling_mode, captured_lifecycle)

RECURSIVE runtime_ingress_RunFailedFromActive_ForEach6_steer_queue(_, _, _, _)
runtime_ingress_RunFailedFromActive_ForEach6_steer_queue(acc, items, captured_handling_mode, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN captured_handling_mode THEN captured_handling_mode[work_id] ELSE "None") = "Steer") THEN (<<work_id>> \o acc) ELSE acc ELSE acc IN runtime_ingress_RunFailedFromActive_ForEach6_steer_queue(next_acc, Tail(items), captured_handling_mode, captured_lifecycle)

RECURSIVE runtime_ingress_RunFailedFromRetired_ForEach7_lifecycle(_, _, _)
runtime_ingress_RunFailedFromRetired_ForEach7_lifecycle(acc, items, captured_handling_mode) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") = "Staged") THEN MapSet(acc, work_id, "Queued") ELSE acc IN runtime_ingress_RunFailedFromRetired_ForEach7_lifecycle(next_acc, Tail(items), captured_handling_mode)

RECURSIVE runtime_ingress_RunFailedFromRetired_ForEach7_queue(_, _, _, _)
runtime_ingress_RunFailedFromRetired_ForEach7_queue(acc, items, captured_handling_mode, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN captured_handling_mode THEN captured_handling_mode[work_id] ELSE "None") = "Steer") THEN acc ELSE (<<work_id>> \o acc) ELSE acc IN runtime_ingress_RunFailedFromRetired_ForEach7_queue(next_acc, Tail(items), captured_handling_mode, captured_lifecycle)

RECURSIVE runtime_ingress_RunFailedFromRetired_ForEach7_steer_queue(_, _, _, _)
runtime_ingress_RunFailedFromRetired_ForEach7_steer_queue(acc, items, captured_handling_mode, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN captured_handling_mode THEN captured_handling_mode[work_id] ELSE "None") = "Steer") THEN (<<work_id>> \o acc) ELSE acc ELSE acc IN runtime_ingress_RunFailedFromRetired_ForEach7_steer_queue(next_acc, Tail(items), captured_handling_mode, captured_lifecycle)

RECURSIVE runtime_ingress_RunCancelledFromActive_ForEach8_lifecycle(_, _, _)
runtime_ingress_RunCancelledFromActive_ForEach8_lifecycle(acc, items, captured_handling_mode) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") = "Staged") THEN MapSet(acc, work_id, "Queued") ELSE acc IN runtime_ingress_RunCancelledFromActive_ForEach8_lifecycle(next_acc, Tail(items), captured_handling_mode)

RECURSIVE runtime_ingress_RunCancelledFromActive_ForEach8_queue(_, _, _, _)
runtime_ingress_RunCancelledFromActive_ForEach8_queue(acc, items, captured_handling_mode, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN captured_handling_mode THEN captured_handling_mode[work_id] ELSE "None") = "Steer") THEN acc ELSE (<<work_id>> \o acc) ELSE acc IN runtime_ingress_RunCancelledFromActive_ForEach8_queue(next_acc, Tail(items), captured_handling_mode, captured_lifecycle)

RECURSIVE runtime_ingress_RunCancelledFromActive_ForEach8_steer_queue(_, _, _, _)
runtime_ingress_RunCancelledFromActive_ForEach8_steer_queue(acc, items, captured_handling_mode, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN captured_handling_mode THEN captured_handling_mode[work_id] ELSE "None") = "Steer") THEN (<<work_id>> \o acc) ELSE acc ELSE acc IN runtime_ingress_RunCancelledFromActive_ForEach8_steer_queue(next_acc, Tail(items), captured_handling_mode, captured_lifecycle)

RECURSIVE runtime_ingress_RunCancelledFromRetired_ForEach9_lifecycle(_, _, _)
runtime_ingress_RunCancelledFromRetired_ForEach9_lifecycle(acc, items, captured_handling_mode) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") = "Staged") THEN MapSet(acc, work_id, "Queued") ELSE acc IN runtime_ingress_RunCancelledFromRetired_ForEach9_lifecycle(next_acc, Tail(items), captured_handling_mode)

RECURSIVE runtime_ingress_RunCancelledFromRetired_ForEach9_queue(_, _, _, _)
runtime_ingress_RunCancelledFromRetired_ForEach9_queue(acc, items, captured_handling_mode, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN captured_handling_mode THEN captured_handling_mode[work_id] ELSE "None") = "Steer") THEN acc ELSE (<<work_id>> \o acc) ELSE acc IN runtime_ingress_RunCancelledFromRetired_ForEach9_queue(next_acc, Tail(items), captured_handling_mode, captured_lifecycle)

RECURSIVE runtime_ingress_RunCancelledFromRetired_ForEach9_steer_queue(_, _, _, _)
runtime_ingress_RunCancelledFromRetired_ForEach9_steer_queue(acc, items, captured_handling_mode, captured_lifecycle) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == IF ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") = "Staged") THEN IF ((IF work_id \in DOMAIN captured_handling_mode THEN captured_handling_mode[work_id] ELSE "None") = "Steer") THEN (<<work_id>> \o acc) ELSE acc ELSE acc IN runtime_ingress_RunCancelledFromRetired_ForEach9_steer_queue(next_acc, Tail(items), captured_handling_mode, captured_lifecycle)

RECURSIVE runtime_ingress_CoalesceQueuedInputsFromActive_ForEach10_lifecycle(_, _)
runtime_ingress_CoalesceQueuedInputsFromActive_ForEach10_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Coalesced") IN runtime_ingress_CoalesceQueuedInputsFromActive_ForEach10_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_CoalesceQueuedInputsFromActive_ForEach10_terminal_outcome(_, _)
runtime_ingress_CoalesceQueuedInputsFromActive_ForEach10_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("Coalesced")) IN runtime_ingress_CoalesceQueuedInputsFromActive_ForEach10_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_CoalesceQueuedInputsFromRetired_ForEach11_lifecycle(_, _)
runtime_ingress_CoalesceQueuedInputsFromRetired_ForEach11_lifecycle(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, "Coalesced") IN runtime_ingress_CoalesceQueuedInputsFromRetired_ForEach11_lifecycle(next_acc, Tail(items))

RECURSIVE runtime_ingress_CoalesceQueuedInputsFromRetired_ForEach11_terminal_outcome(_, _)
runtime_ingress_CoalesceQueuedInputsFromRetired_ForEach11_terminal_outcome(acc, items) == IF Len(items) = 0 THEN acc ELSE LET work_id == Head(items) IN LET next_acc == MapSet(acc, work_id, Some("Coalesced")) IN runtime_ingress_CoalesceQueuedInputsFromRetired_ForEach11_terminal_outcome(next_acc, Tail(items))

RECURSIVE runtime_ingress_ResetFromActive_ForEach12_lifecycle(_, _, _)
runtime_ingress_ResetFromActive_ForEach12_lifecycle(acc, remaining, captured_terminal_outcome) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET work_id == item IN LET next_acc == IF (((IF work_id \in DOMAIN captured_terminal_outcome THEN captured_terminal_outcome[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Abandoned")) THEN MapSet(acc, work_id, "Abandoned") ELSE acc IN runtime_ingress_ResetFromActive_ForEach12_lifecycle(next_acc, remaining \ {item}, captured_terminal_outcome)

RECURSIVE runtime_ingress_ResetFromActive_ForEach12_terminal_outcome(_, _, _)
runtime_ingress_ResetFromActive_ForEach12_terminal_outcome(acc, remaining, captured_lifecycle) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET work_id == item IN LET next_acc == IF (((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Abandoned")) THEN MapSet(acc, work_id, Some("AbandonedReset")) ELSE acc IN runtime_ingress_ResetFromActive_ForEach12_terminal_outcome(next_acc, remaining \ {item}, captured_lifecycle)

RECURSIVE runtime_ingress_ResetFromRetired_ForEach13_lifecycle(_, _, _)
runtime_ingress_ResetFromRetired_ForEach13_lifecycle(acc, remaining, captured_terminal_outcome) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET work_id == item IN LET next_acc == IF (((IF work_id \in DOMAIN captured_terminal_outcome THEN captured_terminal_outcome[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Abandoned")) THEN MapSet(acc, work_id, "Abandoned") ELSE acc IN runtime_ingress_ResetFromRetired_ForEach13_lifecycle(next_acc, remaining \ {item}, captured_terminal_outcome)

RECURSIVE runtime_ingress_ResetFromRetired_ForEach13_terminal_outcome(_, _, _)
runtime_ingress_ResetFromRetired_ForEach13_terminal_outcome(acc, remaining, captured_lifecycle) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET work_id == item IN LET next_acc == IF (((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Abandoned")) THEN MapSet(acc, work_id, Some("AbandonedReset")) ELSE acc IN runtime_ingress_ResetFromRetired_ForEach13_terminal_outcome(next_acc, remaining \ {item}, captured_lifecycle)

RECURSIVE runtime_ingress_Destroy_ForEach14_lifecycle(_, _, _)
runtime_ingress_Destroy_ForEach14_lifecycle(acc, remaining, captured_terminal_outcome) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET work_id == item IN LET next_acc == IF (((IF work_id \in DOMAIN captured_terminal_outcome THEN captured_terminal_outcome[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE "None") # "Abandoned")) THEN MapSet(acc, work_id, "Abandoned") ELSE acc IN runtime_ingress_Destroy_ForEach14_lifecycle(next_acc, remaining \ {item}, captured_terminal_outcome)

RECURSIVE runtime_ingress_Destroy_ForEach14_terminal_outcome(_, _, _)
runtime_ingress_Destroy_ForEach14_terminal_outcome(acc, remaining, captured_lifecycle) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET work_id == item IN LET next_acc == IF (((IF work_id \in DOMAIN acc THEN acc[work_id] ELSE None) = None) /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Consumed") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Superseded") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Coalesced") /\ ((IF work_id \in DOMAIN captured_lifecycle THEN captured_lifecycle[work_id] ELSE "None") # "Abandoned")) THEN MapSet(acc, work_id, Some("AbandonedDestroyed")) ELSE acc IN runtime_ingress_Destroy_ForEach14_terminal_outcome(next_acc, remaining \ {item}, captured_lifecycle)

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
       /\ runtime_ingress_wake_requested' = TRUE
       /\ runtime_ingress_process_requested' = (runtime_ingress_process_requested \/ FALSE)
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressAccepted", payload |-> [work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedQueue"], [machine |-> "runtime_ingress", variant |-> "InputLifecycleNotice", payload |-> [new_state |-> "Queued", work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedQueue"], [machine |-> "runtime_ingress", variant |-> "WakeRuntime", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedQueue"] }
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
       /\ runtime_ingress_wake_requested' = TRUE
       /\ runtime_ingress_process_requested' = (runtime_ingress_process_requested \/ TRUE)
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_handling_mode, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressAccepted", payload |-> [work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitConsumedOnAccept"], [machine |-> "runtime_ingress", variant |-> "InputLifecycleNotice", payload |-> [new_state |-> "Consumed", work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitConsumedOnAccept"], [machine |-> "runtime_ingress", variant |-> "CompletionResolved", payload |-> [outcome |-> "Consumed", work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitConsumedOnAccept"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "AdmitConsumedOnAccept", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_StageDrainSnapshotFromActive(arg_run_id, arg_contributing_work_ids) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "StageDrainSnapshot"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.contributing_work_ids = arg_contributing_work_ids
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ (runtime_ingress_current_run = None)
       /\ (Len(packet.payload.contributing_work_ids) > 0)
       /\ (((Len(runtime_ingress_steer_queue) > 0) /\ StartsWith(runtime_ingress_steer_queue, packet.payload.contributing_work_ids)) \/ ((Len(runtime_ingress_steer_queue) = 0) /\ StartsWith(runtime_ingress_queue, packet.payload.contributing_work_ids)))
       /\ (\A work_id \in SeqElements(packet.payload.contributing_work_ids) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "Queued"))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_StageDrainSnapshotFromActive_ForEach0_lifecycle(runtime_ingress_lifecycle, packet.payload.contributing_work_ids, packet.payload.run_id)
       /\ runtime_ingress_queue' = IF (Len(runtime_ingress_steer_queue) > 0) THEN runtime_ingress_queue ELSE SeqRemoveAll(runtime_ingress_queue, packet.payload.contributing_work_ids)
       /\ runtime_ingress_steer_queue' = IF (Len(runtime_ingress_steer_queue) > 0) THEN SeqRemoveAll(runtime_ingress_steer_queue, packet.payload.contributing_work_ids) ELSE runtime_ingress_steer_queue
       /\ runtime_ingress_current_run' = Some(packet.payload.run_id)
       /\ runtime_ingress_current_run_contributors' = packet.payload.contributing_work_ids
       /\ runtime_ingress_last_run' = runtime_ingress_StageDrainSnapshotFromActive_ForEach0_last_run(runtime_ingress_last_run, packet.payload.contributing_work_ids, packet.payload.run_id)
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_last_boundary_sequence, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "BeginRun", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_ingress_ready_starts_runtime_control", source_machine |-> "runtime_ingress", source_effect |-> "ReadyForRun", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "BeginRun", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_ingress_ready_starts_runtime_control", source_machine |-> "runtime_ingress", source_effect |-> "ReadyForRun", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_ingress_ready_starts_runtime_control", source_machine |-> "runtime_ingress", effect |-> "ReadyForRun", target_machine |-> "runtime_control", target_input |-> "BeginRun", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "StageDrainSnapshotFromActive"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "ReadyForRun", payload |-> [contributing_work_ids |-> packet.payload.contributing_work_ids, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "StageDrainSnapshotFromActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "StageDrainSnapshotFromActive", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_StageDrainSnapshotFromRetired(arg_run_id, arg_contributing_work_ids) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "StageDrainSnapshot"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.contributing_work_ids = arg_contributing_work_ids
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Retired"
       /\ (runtime_ingress_current_run = None)
       /\ (Len(packet.payload.contributing_work_ids) > 0)
       /\ (((Len(runtime_ingress_steer_queue) > 0) /\ StartsWith(runtime_ingress_steer_queue, packet.payload.contributing_work_ids)) \/ ((Len(runtime_ingress_steer_queue) = 0) /\ StartsWith(runtime_ingress_queue, packet.payload.contributing_work_ids)))
       /\ (\A work_id \in SeqElements(packet.payload.contributing_work_ids) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "Queued"))
       /\ runtime_ingress_phase' = "Retired"
       /\ runtime_ingress_lifecycle' = runtime_ingress_StageDrainSnapshotFromRetired_ForEach1_lifecycle(runtime_ingress_lifecycle, packet.payload.contributing_work_ids, packet.payload.run_id)
       /\ runtime_ingress_queue' = IF (Len(runtime_ingress_steer_queue) > 0) THEN runtime_ingress_queue ELSE SeqRemoveAll(runtime_ingress_queue, packet.payload.contributing_work_ids)
       /\ runtime_ingress_steer_queue' = IF (Len(runtime_ingress_steer_queue) > 0) THEN SeqRemoveAll(runtime_ingress_steer_queue, packet.payload.contributing_work_ids) ELSE runtime_ingress_steer_queue
       /\ runtime_ingress_current_run' = Some(packet.payload.run_id)
       /\ runtime_ingress_current_run_contributors' = packet.payload.contributing_work_ids
       /\ runtime_ingress_last_run' = runtime_ingress_StageDrainSnapshotFromRetired_ForEach1_last_run(runtime_ingress_last_run, packet.payload.contributing_work_ids, packet.payload.run_id)
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_last_boundary_sequence, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "BeginRun", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_ingress_ready_starts_runtime_control", source_machine |-> "runtime_ingress", source_effect |-> "ReadyForRun", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "BeginRun", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_ingress_ready_starts_runtime_control", source_machine |-> "runtime_ingress", source_effect |-> "ReadyForRun", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_ingress_ready_starts_runtime_control", source_machine |-> "runtime_ingress", effect |-> "ReadyForRun", target_machine |-> "runtime_control", target_input |-> "BeginRun", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "StageDrainSnapshotFromRetired"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "ReadyForRun", payload |-> [contributing_work_ids |-> packet.payload.contributing_work_ids, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "StageDrainSnapshotFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "StageDrainSnapshotFromRetired", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_BoundaryAppliedFromActive(arg_run_id, arg_boundary_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "BoundaryApplied"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.boundary_sequence = arg_boundary_sequence
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ (runtime_ingress_current_run = Some(packet.payload.run_id))
       /\ (\A work_id \in SeqElements(runtime_ingress_current_run_contributors) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "Staged"))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_BoundaryAppliedFromActive_ForEach2_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, packet.payload.boundary_sequence)
       /\ runtime_ingress_last_boundary_sequence' = runtime_ingress_BoundaryAppliedFromActive_ForEach2_last_boundary_sequence(runtime_ingress_last_boundary_sequence, runtime_ingress_current_run_contributors, packet.payload.boundary_sequence)
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "ContributorsPendingConsumption", kind |-> "BoundaryApplied"], effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryAppliedFromActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "BoundaryAppliedFromActive", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_BoundaryAppliedFromRetired(arg_run_id, arg_boundary_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "BoundaryApplied"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.boundary_sequence = arg_boundary_sequence
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Retired"
       /\ (runtime_ingress_current_run = Some(packet.payload.run_id))
       /\ (\A work_id \in SeqElements(runtime_ingress_current_run_contributors) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "Staged"))
       /\ runtime_ingress_phase' = "Retired"
       /\ runtime_ingress_lifecycle' = runtime_ingress_BoundaryAppliedFromRetired_ForEach3_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, packet.payload.boundary_sequence)
       /\ runtime_ingress_last_boundary_sequence' = runtime_ingress_BoundaryAppliedFromRetired_ForEach3_last_boundary_sequence(runtime_ingress_last_boundary_sequence, runtime_ingress_current_run_contributors, packet.payload.boundary_sequence)
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "ContributorsPendingConsumption", kind |-> "BoundaryApplied"], effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryAppliedFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "BoundaryAppliedFromRetired", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_RunCompletedFromActive(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "RunCompleted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ (runtime_ingress_current_run = Some(packet.payload.run_id))
       /\ (\A work_id \in SeqElements(runtime_ingress_current_run_contributors) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "AppliedPendingConsumption"))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RunCompletedFromActive_ForEach4_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_RunCompletedFromActive_ForEach4_terminal_outcome(runtime_ingress_terminal_outcome, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "ContributorsConsumed", kind |-> "RunCompleted"], effect_id |-> (model_step_count + 1), source_transition |-> "RunCompletedFromActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RunCompletedFromActive", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_RunCompletedFromRetired(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "RunCompleted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Retired"
       /\ (runtime_ingress_current_run = Some(packet.payload.run_id))
       /\ (\A work_id \in SeqElements(runtime_ingress_current_run_contributors) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "AppliedPendingConsumption"))
       /\ runtime_ingress_phase' = "Retired"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RunCompletedFromRetired_ForEach5_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_RunCompletedFromRetired_ForEach5_terminal_outcome(runtime_ingress_terminal_outcome, runtime_ingress_current_run_contributors)
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "ContributorsConsumed", kind |-> "RunCompleted"], effect_id |-> (model_step_count + 1), source_transition |-> "RunCompletedFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RunCompletedFromRetired", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_RunFailedFromActive(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "RunFailed"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ (runtime_ingress_current_run = Some(packet.payload.run_id))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RunFailedFromActive_ForEach6_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode)
       /\ runtime_ingress_queue' = runtime_ingress_RunFailedFromActive_ForEach6_queue(runtime_ingress_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunFailedFromActive_ForEach6_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))
       /\ runtime_ingress_steer_queue' = runtime_ingress_RunFailedFromActive_ForEach6_steer_queue(runtime_ingress_steer_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunFailedFromActive_ForEach6_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = IF ((Len(runtime_ingress_RunFailedFromActive_ForEach6_queue(runtime_ingress_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunFailedFromActive_ForEach6_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))) > 0) \/ (Len(runtime_ingress_RunFailedFromActive_ForEach6_steer_queue(runtime_ingress_steer_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunFailedFromActive_ForEach6_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))) > 0)) THEN TRUE ELSE runtime_ingress_wake_requested
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "StagedRolledBack", kind |-> "RunFailed"], effect_id |-> (model_step_count + 1), source_transition |-> "RunFailedFromActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RunFailedFromActive", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_RunFailedFromRetired(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "RunFailed"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Retired"
       /\ (runtime_ingress_current_run = Some(packet.payload.run_id))
       /\ runtime_ingress_phase' = "Retired"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RunFailedFromRetired_ForEach7_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode)
       /\ runtime_ingress_queue' = runtime_ingress_RunFailedFromRetired_ForEach7_queue(runtime_ingress_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunFailedFromRetired_ForEach7_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))
       /\ runtime_ingress_steer_queue' = runtime_ingress_RunFailedFromRetired_ForEach7_steer_queue(runtime_ingress_steer_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunFailedFromRetired_ForEach7_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = IF ((Len(runtime_ingress_RunFailedFromRetired_ForEach7_queue(runtime_ingress_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunFailedFromRetired_ForEach7_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))) > 0) \/ (Len(runtime_ingress_RunFailedFromRetired_ForEach7_steer_queue(runtime_ingress_steer_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunFailedFromRetired_ForEach7_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))) > 0)) THEN TRUE ELSE runtime_ingress_wake_requested
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "StagedRolledBack", kind |-> "RunFailed"], effect_id |-> (model_step_count + 1), source_transition |-> "RunFailedFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RunFailedFromRetired", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_RunCancelledFromActive(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "RunCancelled"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ (runtime_ingress_current_run = Some(packet.payload.run_id))
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RunCancelledFromActive_ForEach8_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode)
       /\ runtime_ingress_queue' = runtime_ingress_RunCancelledFromActive_ForEach8_queue(runtime_ingress_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunCancelledFromActive_ForEach8_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))
       /\ runtime_ingress_steer_queue' = runtime_ingress_RunCancelledFromActive_ForEach8_steer_queue(runtime_ingress_steer_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunCancelledFromActive_ForEach8_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = IF ((Len(runtime_ingress_RunCancelledFromActive_ForEach8_queue(runtime_ingress_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunCancelledFromActive_ForEach8_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))) > 0) \/ (Len(runtime_ingress_RunCancelledFromActive_ForEach8_steer_queue(runtime_ingress_steer_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunCancelledFromActive_ForEach8_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))) > 0)) THEN TRUE ELSE runtime_ingress_wake_requested
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "StagedRolledBack", kind |-> "RunCancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "RunCancelledFromActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RunCancelledFromActive", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_RunCancelledFromRetired(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "RunCancelled"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Retired"
       /\ (runtime_ingress_current_run = Some(packet.payload.run_id))
       /\ runtime_ingress_phase' = "Retired"
       /\ runtime_ingress_lifecycle' = runtime_ingress_RunCancelledFromRetired_ForEach9_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode)
       /\ runtime_ingress_queue' = runtime_ingress_RunCancelledFromRetired_ForEach9_queue(runtime_ingress_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunCancelledFromRetired_ForEach9_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))
       /\ runtime_ingress_steer_queue' = runtime_ingress_RunCancelledFromRetired_ForEach9_steer_queue(runtime_ingress_steer_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunCancelledFromRetired_ForEach9_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = IF ((Len(runtime_ingress_RunCancelledFromRetired_ForEach9_queue(runtime_ingress_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunCancelledFromRetired_ForEach9_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))) > 0) \/ (Len(runtime_ingress_RunCancelledFromRetired_ForEach9_steer_queue(runtime_ingress_steer_queue, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode, runtime_ingress_RunCancelledFromRetired_ForEach9_lifecycle(runtime_ingress_lifecycle, runtime_ingress_current_run_contributors, runtime_ingress_handling_mode))) > 0)) THEN TRUE ELSE runtime_ingress_wake_requested
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "StagedRolledBack", kind |-> "RunCancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "RunCancelledFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "RunCancelledFromRetired", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_SupersedeQueuedInputFromActive(arg_new_work_id, arg_old_work_id) ==
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
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "InputLifecycleNotice", payload |-> [new_state |-> "Superseded", work_id |-> packet.payload.old_work_id], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedeQueuedInputFromActive"], [machine |-> "runtime_ingress", variant |-> "CompletionResolved", payload |-> [outcome |-> "Superseded", work_id |-> packet.payload.old_work_id], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedeQueuedInputFromActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "SupersedeQueuedInputFromActive", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_SupersedeQueuedInputFromRetired(arg_new_work_id, arg_old_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "SupersedeQueuedInput"
       /\ packet.payload.new_work_id = arg_new_work_id
       /\ packet.payload.old_work_id = arg_old_work_id
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Retired"
       /\ (packet.payload.new_work_id \in runtime_ingress_admitted_inputs)
       /\ ((IF packet.payload.old_work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[packet.payload.old_work_id] ELSE "None") = "Queued")
       /\ runtime_ingress_phase' = "Retired"
       /\ runtime_ingress_lifecycle' = MapSet(runtime_ingress_lifecycle, packet.payload.old_work_id, "Superseded")
       /\ runtime_ingress_terminal_outcome' = MapSet(runtime_ingress_terminal_outcome, packet.payload.old_work_id, Some("Superseded"))
       /\ runtime_ingress_queue' = SeqRemove(runtime_ingress_queue, packet.payload.old_work_id)
       /\ runtime_ingress_steer_queue' = SeqRemove(runtime_ingress_steer_queue, packet.payload.old_work_id)
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "InputLifecycleNotice", payload |-> [new_state |-> "Superseded", work_id |-> packet.payload.old_work_id], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedeQueuedInputFromRetired"], [machine |-> "runtime_ingress", variant |-> "CompletionResolved", payload |-> [outcome |-> "Superseded", work_id |-> packet.payload.old_work_id], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedeQueuedInputFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "SupersedeQueuedInputFromRetired", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_CoalesceQueuedInputsFromActive(arg_aggregate_work_id, arg_source_work_ids) ==
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
       /\ runtime_ingress_lifecycle' = runtime_ingress_CoalesceQueuedInputsFromActive_ForEach10_lifecycle(runtime_ingress_lifecycle, packet.payload.source_work_ids)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_CoalesceQueuedInputsFromActive_ForEach10_terminal_outcome(runtime_ingress_terminal_outcome, packet.payload.source_work_ids)
       /\ runtime_ingress_queue' = SeqRemoveAll(runtime_ingress_queue, packet.payload.source_work_ids)
       /\ runtime_ingress_steer_queue' = SeqRemoveAll(runtime_ingress_steer_queue, packet.payload.source_work_ids)
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "SourcesCoalescedIntoAggregate", kind |-> "CoalesceQueuedInputs"], effect_id |-> (model_step_count + 1), source_transition |-> "CoalesceQueuedInputsFromActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "CoalesceQueuedInputsFromActive", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_CoalesceQueuedInputsFromRetired(arg_aggregate_work_id, arg_source_work_ids) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "CoalesceQueuedInputs"
       /\ packet.payload.aggregate_work_id = arg_aggregate_work_id
       /\ packet.payload.source_work_ids = arg_source_work_ids
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Retired"
       /\ (packet.payload.aggregate_work_id \in runtime_ingress_admitted_inputs)
       /\ (Len(packet.payload.source_work_ids) > 0)
       /\ (\A work_id \in SeqElements(packet.payload.source_work_ids) : ((IF work_id \in DOMAIN runtime_ingress_lifecycle THEN runtime_ingress_lifecycle[work_id] ELSE "None") = "Queued"))
       /\ runtime_ingress_phase' = "Retired"
       /\ runtime_ingress_lifecycle' = runtime_ingress_CoalesceQueuedInputsFromRetired_ForEach11_lifecycle(runtime_ingress_lifecycle, packet.payload.source_work_ids)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_CoalesceQueuedInputsFromRetired_ForEach11_terminal_outcome(runtime_ingress_terminal_outcome, packet.payload.source_work_ids)
       /\ runtime_ingress_queue' = SeqRemoveAll(runtime_ingress_queue, packet.payload.source_work_ids)
       /\ runtime_ingress_steer_queue' = SeqRemoveAll(runtime_ingress_steer_queue, packet.payload.source_work_ids)
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_ingress", variant |-> "IngressNotice", payload |-> [detail |-> "SourcesCoalescedIntoAggregate", kind |-> "CoalesceQueuedInputs"], effect_id |-> (model_step_count + 1), source_transition |-> "CoalesceQueuedInputsFromRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_ingress", transition |-> "CoalesceQueuedInputsFromRetired", actor |-> "ordinary_ingress", step |-> (model_step_count + 1), from_phase |-> runtime_ingress_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


runtime_ingress_Retire ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_ingress"
       /\ packet.variant = "Retire"
       /\ ~HigherPriorityReady("ordinary_ingress")
       /\ runtime_ingress_phase = "Active"
       /\ runtime_ingress_phase' = "Retired"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ (runtime_ingress_current_run = None)
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_ResetFromActive_ForEach12_lifecycle(runtime_ingress_lifecycle, DOMAIN runtime_ingress_lifecycle, runtime_ingress_terminal_outcome)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_ResetFromActive_ForEach12_terminal_outcome(runtime_ingress_terminal_outcome, DOMAIN runtime_ingress_lifecycle, runtime_ingress_ResetFromActive_ForEach12_lifecycle(runtime_ingress_lifecycle, DOMAIN runtime_ingress_lifecycle, runtime_ingress_terminal_outcome))
       /\ runtime_ingress_queue' = <<>>
       /\ runtime_ingress_steer_queue' = <<>>
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ (runtime_ingress_current_run = None)
       /\ runtime_ingress_phase' = "Active"
       /\ runtime_ingress_lifecycle' = runtime_ingress_ResetFromRetired_ForEach13_lifecycle(runtime_ingress_lifecycle, DOMAIN runtime_ingress_lifecycle, runtime_ingress_terminal_outcome)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_ResetFromRetired_ForEach13_terminal_outcome(runtime_ingress_terminal_outcome, DOMAIN runtime_ingress_lifecycle, runtime_ingress_ResetFromRetired_ForEach13_lifecycle(runtime_ingress_lifecycle, DOMAIN runtime_ingress_lifecycle, runtime_ingress_terminal_outcome))
       /\ runtime_ingress_queue' = <<>>
       /\ runtime_ingress_steer_queue' = <<>>
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ runtime_ingress_lifecycle' = runtime_ingress_Destroy_ForEach14_lifecycle(runtime_ingress_lifecycle, DOMAIN runtime_ingress_lifecycle, runtime_ingress_terminal_outcome)
       /\ runtime_ingress_terminal_outcome' = runtime_ingress_Destroy_ForEach14_terminal_outcome(runtime_ingress_terminal_outcome, DOMAIN runtime_ingress_lifecycle, runtime_ingress_Destroy_ForEach14_lifecycle(runtime_ingress_lifecycle, DOMAIN runtime_ingress_lifecycle, runtime_ingress_terminal_outcome))
       /\ runtime_ingress_queue' = <<>>
       /\ runtime_ingress_steer_queue' = <<>>
       /\ runtime_ingress_current_run' = None
       /\ runtime_ingress_current_run_contributors' = <<>>
       /\ runtime_ingress_wake_requested' = FALSE
       /\ runtime_ingress_process_requested' = FALSE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_steer_queue, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_terminal_outcome, runtime_ingress_steer_queue, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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

turn_execution_StartConversationRun(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "StartConversationRun"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Ready"
       /\ turn_execution_phase' = "ApplyingPrimitive"
       /\ turn_execution_active_run' = Some(packet.payload.run_id)
       /\ turn_execution_primitive_kind' = "ConversationTurn"
       /\ turn_execution_admitted_content_shape' = None
       /\ turn_execution_vision_enabled' = FALSE
       /\ turn_execution_image_tool_results_enabled' = FALSE
       /\ turn_execution_tool_calls_pending' = 0
       /\ turn_execution_boundary_count' = 0
       /\ turn_execution_cancel_after_boundary' = FALSE
       /\ turn_execution_terminal_outcome' = "None"
       /\ turn_execution_extraction_attempts' = 0
       /\ turn_execution_max_extraction_retries' = 0
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunStarted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "StartConversationRun"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "StartConversationRun", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "ApplyingPrimitive"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_StartImmediateAppend(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "StartImmediateAppend"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Ready"
       /\ turn_execution_phase' = "ApplyingPrimitive"
       /\ turn_execution_active_run' = Some(packet.payload.run_id)
       /\ turn_execution_primitive_kind' = "ImmediateAppend"
       /\ turn_execution_admitted_content_shape' = None
       /\ turn_execution_vision_enabled' = FALSE
       /\ turn_execution_image_tool_results_enabled' = FALSE
       /\ turn_execution_tool_calls_pending' = 0
       /\ turn_execution_boundary_count' = 0
       /\ turn_execution_cancel_after_boundary' = FALSE
       /\ turn_execution_terminal_outcome' = "None"
       /\ turn_execution_extraction_attempts' = 0
       /\ turn_execution_max_extraction_retries' = 0
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunStarted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "StartImmediateAppend"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "StartImmediateAppend", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "ApplyingPrimitive"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_StartImmediateContext(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "StartImmediateContext"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Ready"
       /\ turn_execution_phase' = "ApplyingPrimitive"
       /\ turn_execution_active_run' = Some(packet.payload.run_id)
       /\ turn_execution_primitive_kind' = "ImmediateContextAppend"
       /\ turn_execution_admitted_content_shape' = None
       /\ turn_execution_vision_enabled' = FALSE
       /\ turn_execution_image_tool_results_enabled' = FALSE
       /\ turn_execution_tool_calls_pending' = 0
       /\ turn_execution_boundary_count' = 0
       /\ turn_execution_cancel_after_boundary' = FALSE
       /\ turn_execution_terminal_outcome' = "None"
       /\ turn_execution_extraction_attempts' = 0
       /\ turn_execution_max_extraction_retries' = 0
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunStarted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "StartImmediateContext"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "StartImmediateContext", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "ApplyingPrimitive"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_PrimitiveAppliedConversationTurn(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "PrimitiveApplied"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.admitted_content_shape = arg_admitted_content_shape
       /\ packet.payload.vision_enabled = arg_vision_enabled
       /\ packet.payload.image_tool_results_enabled = arg_image_tool_results_enabled
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ApplyingPrimitive"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (turn_execution_primitive_kind = "ConversationTurn")
       /\ turn_execution_phase' = "CallingLlm"
       /\ turn_execution_admitted_content_shape' = Some(packet.payload.admitted_content_shape)
       /\ turn_execution_vision_enabled' = packet.payload.vision_enabled
       /\ turn_execution_image_tool_results_enabled' = packet.payload.image_tool_results_enabled
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "DrainCommsInbox", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedConversationTurn"], [machine |-> "turn_execution", variant |-> "CheckCompaction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedConversationTurn"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "PrimitiveAppliedConversationTurn", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "CallingLlm"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_PrimitiveAppliedImmediateAppend(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "PrimitiveApplied"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.admitted_content_shape = arg_admitted_content_shape
       /\ packet.payload.vision_enabled = arg_vision_enabled
       /\ packet.payload.image_tool_results_enabled = arg_image_tool_results_enabled
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ApplyingPrimitive"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (turn_execution_primitive_kind = "ImmediateAppend")
       /\ (turn_execution_cancel_after_boundary = FALSE)
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_admitted_content_shape' = Some(packet.payload.admitted_content_shape)
       /\ turn_execution_vision_enabled' = packet.payload.vision_enabled
       /\ turn_execution_image_tool_results_enabled' = packet.payload.image_tool_results_enabled
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "Completed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateAppend"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateAppend"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateAppend"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateAppend"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateAppend"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "PrimitiveAppliedImmediateAppend", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_PrimitiveAppliedImmediateAppendCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "PrimitiveApplied"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.admitted_content_shape = arg_admitted_content_shape
       /\ packet.payload.vision_enabled = arg_vision_enabled
       /\ packet.payload.image_tool_results_enabled = arg_image_tool_results_enabled
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ApplyingPrimitive"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (turn_execution_primitive_kind = "ImmediateAppend")
       /\ (turn_execution_cancel_after_boundary = TRUE)
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_admitted_content_shape' = Some(packet.payload.admitted_content_shape)
       /\ turn_execution_vision_enabled' = packet.payload.vision_enabled
       /\ turn_execution_image_tool_results_enabled' = packet.payload.image_tool_results_enabled
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_cancel_after_boundary' = FALSE
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateAppendCancelsAfterBoundary"], [route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCancelled", target_machine |-> "runtime_ingress", target_input |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateAppendCancelsAfterBoundary"], [route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCancelled", target_machine |-> "runtime_control", target_input |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateAppendCancelsAfterBoundary"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateAppendCancelsAfterBoundary"], [machine |-> "turn_execution", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateAppendCancelsAfterBoundary"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "PrimitiveAppliedImmediateAppendCancelsAfterBoundary", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_PrimitiveAppliedImmediateContext(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "PrimitiveApplied"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.admitted_content_shape = arg_admitted_content_shape
       /\ packet.payload.vision_enabled = arg_vision_enabled
       /\ packet.payload.image_tool_results_enabled = arg_image_tool_results_enabled
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ApplyingPrimitive"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (turn_execution_primitive_kind = "ImmediateContextAppend")
       /\ (turn_execution_cancel_after_boundary = FALSE)
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_admitted_content_shape' = Some(packet.payload.admitted_content_shape)
       /\ turn_execution_vision_enabled' = packet.payload.vision_enabled
       /\ turn_execution_image_tool_results_enabled' = packet.payload.image_tool_results_enabled
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "Completed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateContext"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateContext"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateContext"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateContext"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateContext"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "PrimitiveAppliedImmediateContext", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_PrimitiveAppliedImmediateContextCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "PrimitiveApplied"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.admitted_content_shape = arg_admitted_content_shape
       /\ packet.payload.vision_enabled = arg_vision_enabled
       /\ packet.payload.image_tool_results_enabled = arg_image_tool_results_enabled
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ApplyingPrimitive"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (turn_execution_primitive_kind = "ImmediateContextAppend")
       /\ (turn_execution_cancel_after_boundary = TRUE)
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_admitted_content_shape' = Some(packet.payload.admitted_content_shape)
       /\ turn_execution_vision_enabled' = packet.payload.vision_enabled
       /\ turn_execution_image_tool_results_enabled' = packet.payload.image_tool_results_enabled
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_cancel_after_boundary' = FALSE
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateContextCancelsAfterBoundary"], [route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCancelled", target_machine |-> "runtime_ingress", target_input |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateContextCancelsAfterBoundary"], [route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCancelled", target_machine |-> "runtime_control", target_input |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateContextCancelsAfterBoundary"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateContextCancelsAfterBoundary"], [machine |-> "turn_execution", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateContextCancelsAfterBoundary"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "PrimitiveAppliedImmediateContextCancelsAfterBoundary", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_LlmReturnedToolCalls(arg_run_id, arg_tool_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "LlmReturnedToolCalls"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.tool_count = arg_tool_count
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "CallingLlm"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (packet.payload.tool_count > 0)
       /\ turn_execution_phase' = "WaitingForOps"
       /\ turn_execution_tool_calls_pending' = packet.payload.tool_count
       /\ turn_execution_pending_op_refs' = None
       /\ turn_execution_barrier_operation_ids' = <<>>
       /\ turn_execution_has_barrier_ops' = FALSE
       /\ turn_execution_barrier_satisfied' = TRUE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "LlmReturnedToolCalls", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "WaitingForOps"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_RegisterPendingOps(arg_run_id, arg_op_refs, arg_barrier_operation_ids, arg_has_barrier_ops) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "RegisterPendingOps"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.op_refs = arg_op_refs
       /\ packet.payload.barrier_operation_ids = arg_barrier_operation_ids
       /\ packet.payload.has_barrier_ops = arg_has_barrier_ops
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "WaitingForOps"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (turn_execution_tool_calls_pending > 0)
       /\ turn_execution_phase' = "WaitingForOps"
       /\ turn_execution_pending_op_refs' = Some(packet.payload.op_refs)
       /\ turn_execution_barrier_operation_ids' = packet.payload.barrier_operation_ids
       /\ turn_execution_has_barrier_ops' = packet.payload.has_barrier_ops
       /\ turn_execution_barrier_satisfied' = (IF packet.payload.has_barrier_ops THEN FALSE ELSE TRUE)
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "RegisterPendingOps", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "WaitingForOps"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_OpsBarrierSatisfied(arg_run_id, arg_operation_ids) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "OpsBarrierSatisfied"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.operation_ids = arg_operation_ids
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "WaitingForOps"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (turn_execution_barrier_satisfied = FALSE)
       /\ ((SeqElements(turn_execution_barrier_operation_ids) = SeqElements(packet.payload.operation_ids)) /\ (Len(turn_execution_barrier_operation_ids) = Len(packet.payload.operation_ids)))
       /\ turn_execution_phase' = "WaitingForOps"
       /\ turn_execution_barrier_satisfied' = TRUE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "OpsBarrierSatisfied", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "WaitingForOps"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ToolCallsResolved(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "ToolCallsResolved"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "WaitingForOps"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (turn_execution_tool_calls_pending > 0)
       /\ (turn_execution_pending_op_refs # None)
       /\ (turn_execution_barrier_satisfied = TRUE)
       /\ turn_execution_phase' = "DrainingBoundary"
       /\ turn_execution_tool_calls_pending' = 0
       /\ turn_execution_pending_op_refs' = None
       /\ turn_execution_barrier_operation_ids' = <<>>
       /\ turn_execution_has_barrier_ops' = FALSE
       /\ turn_execution_barrier_satisfied' = TRUE
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "ToolCallsResolved"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "ToolCallsResolved"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "ToolCallsResolved", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "DrainingBoundary"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_LlmReturnedTerminal(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "LlmReturnedTerminal"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "CallingLlm"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "DrainingBoundary"
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "LlmReturnedTerminal"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "LlmReturnedTerminal"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "LlmReturnedTerminal", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "DrainingBoundary"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_BoundaryContinue(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "BoundaryContinue"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "DrainingBoundary"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (turn_execution_primitive_kind = "ConversationTurn")
       /\ (turn_execution_cancel_after_boundary = FALSE)
       /\ turn_execution_phase' = "CallingLlm"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "DrainCommsInbox", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryContinue"], [machine |-> "turn_execution", variant |-> "CheckCompaction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryContinue"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "BoundaryContinue", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "CallingLlm"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_BoundaryContinueCancelsAfterBoundary(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "BoundaryContinue"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "DrainingBoundary"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (turn_execution_primitive_kind = "ConversationTurn")
       /\ (turn_execution_cancel_after_boundary = TRUE)
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_cancel_after_boundary' = FALSE
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCancelled", target_machine |-> "runtime_ingress", target_input |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryContinueCancelsAfterBoundary"], [route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCancelled", target_machine |-> "runtime_control", target_input |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryContinueCancelsAfterBoundary"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryContinueCancelsAfterBoundary"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "BoundaryContinueCancelsAfterBoundary", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_BoundaryComplete(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "BoundaryComplete"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "DrainingBoundary"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (turn_execution_cancel_after_boundary = FALSE)
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_terminal_outcome' = "Completed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryComplete"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryComplete"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryComplete"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "BoundaryComplete", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_BoundaryCompleteCancelsAfterBoundary(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "BoundaryComplete"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "DrainingBoundary"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ (turn_execution_cancel_after_boundary = TRUE)
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_cancel_after_boundary' = FALSE
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCancelled", target_machine |-> "runtime_ingress", target_input |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryCompleteCancelsAfterBoundary"], [route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCancelled", target_machine |-> "runtime_control", target_input |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryCompleteCancelsAfterBoundary"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryCompleteCancelsAfterBoundary"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "BoundaryCompleteCancelsAfterBoundary", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_EnterExtraction(arg_run_id, arg_max_retries) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "EnterExtraction"
       /\ packet.payload.run_id = arg_run_id
       /\ packet.payload.max_retries = arg_max_retries
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "DrainingBoundary"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Extracting"
       /\ turn_execution_max_extraction_retries' = packet.payload.max_retries
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "EnterExtraction", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Extracting"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ExtractionValidationPassed(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "ExtractionValidationPassed"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Extracting"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_terminal_outcome' = "Completed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "ExtractionValidationPassed"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "ExtractionValidationPassed"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "ExtractionValidationPassed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "ExtractionValidationPassed", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ExtractionRetry(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "ExtractionRetry"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Extracting"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "CallingLlm"
       /\ turn_execution_extraction_attempts' = (turn_execution_extraction_attempts) + 1
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "DrainCommsInbox", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ExtractionRetry"], [machine |-> "turn_execution", variant |-> "CheckCompaction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ExtractionRetry"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "ExtractionRetry", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "CallingLlm"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ExtractionExhausted(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "ExtractionExhausted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Extracting"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_terminal_outcome' = "Completed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "ExtractionExhausted"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "ExtractionExhausted"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "ExtractionExhausted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "ExtractionExhausted", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_RecoverableFailureFromCallingLlm(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "RecoverableFailure"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "CallingLlm"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "ErrorRecovery"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "RecoverableFailureFromCallingLlm", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "ErrorRecovery"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_RecoverableFailureFromWaitingForOps(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "RecoverableFailure"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "WaitingForOps"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "ErrorRecovery"
       /\ turn_execution_pending_op_refs' = None
       /\ turn_execution_barrier_operation_ids' = <<>>
       /\ turn_execution_has_barrier_ops' = FALSE
       /\ turn_execution_barrier_satisfied' = TRUE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "RecoverableFailureFromWaitingForOps", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "ErrorRecovery"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_RecoverableFailureFromDrainingBoundary(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "RecoverableFailure"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "DrainingBoundary"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "ErrorRecovery"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "RecoverableFailureFromDrainingBoundary", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "ErrorRecovery"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_RetryRequested(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "RetryRequested"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ErrorRecovery"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "CallingLlm"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "DrainCommsInbox", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RetryRequested"], [machine |-> "turn_execution", variant |-> "CheckCompaction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RetryRequested"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "RetryRequested", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "CallingLlm"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_FatalFailureFromApplyingPrimitive(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "FatalFailure"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ApplyingPrimitive"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Failed"
       /\ turn_execution_terminal_outcome' = "Failed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunFailed", target_machine |-> "runtime_ingress", target_input |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromApplyingPrimitive"], [route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", effect |-> "RunFailed", target_machine |-> "runtime_control", target_input |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromApplyingPrimitive"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromApplyingPrimitive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "FatalFailureFromApplyingPrimitive", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_FatalFailureFromCallingLlm(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "FatalFailure"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "CallingLlm"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Failed"
       /\ turn_execution_terminal_outcome' = "Failed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunFailed", target_machine |-> "runtime_ingress", target_input |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromCallingLlm"], [route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", effect |-> "RunFailed", target_machine |-> "runtime_control", target_input |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromCallingLlm"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromCallingLlm"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "FatalFailureFromCallingLlm", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_FatalFailureFromWaitingForOps(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "FatalFailure"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "WaitingForOps"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Failed"
       /\ turn_execution_pending_op_refs' = None
       /\ turn_execution_barrier_operation_ids' = <<>>
       /\ turn_execution_has_barrier_ops' = FALSE
       /\ turn_execution_barrier_satisfied' = TRUE
       /\ turn_execution_terminal_outcome' = "Failed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunFailed", target_machine |-> "runtime_ingress", target_input |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromWaitingForOps"], [route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", effect |-> "RunFailed", target_machine |-> "runtime_control", target_input |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromWaitingForOps"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromWaitingForOps"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "FatalFailureFromWaitingForOps", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_FatalFailureFromDrainingBoundary(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "FatalFailure"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "DrainingBoundary"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Failed"
       /\ turn_execution_terminal_outcome' = "Failed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunFailed", target_machine |-> "runtime_ingress", target_input |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromDrainingBoundary"], [route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", effect |-> "RunFailed", target_machine |-> "runtime_control", target_input |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromDrainingBoundary"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromDrainingBoundary"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "FatalFailureFromDrainingBoundary", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_FatalFailureFromExtracting(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "FatalFailure"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Extracting"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Failed"
       /\ turn_execution_terminal_outcome' = "Failed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunFailed", target_machine |-> "runtime_ingress", target_input |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromExtracting"], [route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", effect |-> "RunFailed", target_machine |-> "runtime_control", target_input |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromExtracting"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromExtracting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "FatalFailureFromExtracting", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_FatalFailureFromErrorRecovery(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "FatalFailure"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ErrorRecovery"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Failed"
       /\ turn_execution_terminal_outcome' = "Failed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunFailed", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_failure_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunFailed", target_machine |-> "runtime_ingress", target_input |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromErrorRecovery"], [route |-> "surface_execution_failure_notifies_control", source_machine |-> "turn_execution", effect |-> "RunFailed", target_machine |-> "runtime_control", target_input |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromErrorRecovery"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunFailed", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureFromErrorRecovery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "FatalFailureFromErrorRecovery", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancelNowFromApplyingPrimitive(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancelNow"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ApplyingPrimitive"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Cancelling"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancelNowFromApplyingPrimitive", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelling"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancelNowFromCallingLlm(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancelNow"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "CallingLlm"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Cancelling"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancelNowFromCallingLlm", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelling"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancelNowFromWaitingForOps(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancelNow"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "WaitingForOps"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Cancelling"
       /\ turn_execution_pending_op_refs' = None
       /\ turn_execution_barrier_operation_ids' = <<>>
       /\ turn_execution_has_barrier_ops' = FALSE
       /\ turn_execution_barrier_satisfied' = TRUE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancelNowFromWaitingForOps", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelling"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancelNowFromDrainingBoundary(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancelNow"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "DrainingBoundary"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Cancelling"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancelNowFromDrainingBoundary", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelling"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancelNowFromExtracting(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancelNow"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Extracting"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Cancelling"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancelNowFromExtracting", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelling"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancelNowFromErrorRecovery(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancelNow"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ErrorRecovery"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Cancelling"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancelNowFromErrorRecovery", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelling"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancelAfterBoundaryFromApplyingPrimitive(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancelAfterBoundary"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ApplyingPrimitive"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "ApplyingPrimitive"
       /\ turn_execution_cancel_after_boundary' = TRUE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancelAfterBoundaryFromApplyingPrimitive", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "ApplyingPrimitive"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancelAfterBoundaryFromCallingLlm(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancelAfterBoundary"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "CallingLlm"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "CallingLlm"
       /\ turn_execution_cancel_after_boundary' = TRUE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancelAfterBoundaryFromCallingLlm", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "CallingLlm"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancelAfterBoundaryFromWaitingForOps(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancelAfterBoundary"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "WaitingForOps"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "WaitingForOps"
       /\ turn_execution_cancel_after_boundary' = TRUE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancelAfterBoundaryFromWaitingForOps", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "WaitingForOps"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancelAfterBoundaryFromDrainingBoundary(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancelAfterBoundary"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "DrainingBoundary"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "DrainingBoundary"
       /\ turn_execution_cancel_after_boundary' = TRUE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancelAfterBoundaryFromDrainingBoundary", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "DrainingBoundary"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancelAfterBoundaryFromExtracting(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancelAfterBoundary"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Extracting"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Extracting"
       /\ turn_execution_cancel_after_boundary' = TRUE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancelAfterBoundaryFromExtracting", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Extracting"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancelAfterBoundaryFromErrorRecovery(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancelAfterBoundary"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ErrorRecovery"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "ErrorRecovery"
       /\ turn_execution_cancel_after_boundary' = TRUE
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancelAfterBoundaryFromErrorRecovery", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "ErrorRecovery"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_CancellationObserved(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "CancellationObserved"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Cancelling"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_cancel_after_boundary' = FALSE
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCancelled", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_cancel_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCancelled", target_machine |-> "runtime_ingress", target_input |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "CancellationObserved"], [route |-> "surface_execution_cancel_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCancelled", target_machine |-> "runtime_control", target_input |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "CancellationObserved"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "RunCancelled", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "CancellationObserved"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "CancellationObserved", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_TurnLimitReachedFromApplyingPrimitive(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "TurnLimitReached"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ApplyingPrimitive"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "Completed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromApplyingPrimitive"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromApplyingPrimitive"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromApplyingPrimitive"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromApplyingPrimitive"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromApplyingPrimitive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "TurnLimitReachedFromApplyingPrimitive", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_TurnLimitReachedFromCallingLlm(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "TurnLimitReached"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "CallingLlm"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "Completed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromCallingLlm"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromCallingLlm"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromCallingLlm"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromCallingLlm"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromCallingLlm"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "TurnLimitReachedFromCallingLlm", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_TurnLimitReachedFromWaitingForOps(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "TurnLimitReached"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "WaitingForOps"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_pending_op_refs' = None
       /\ turn_execution_barrier_operation_ids' = <<>>
       /\ turn_execution_has_barrier_ops' = FALSE
       /\ turn_execution_barrier_satisfied' = TRUE
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "Completed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromWaitingForOps"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromWaitingForOps"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromWaitingForOps"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromWaitingForOps"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromWaitingForOps"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "TurnLimitReachedFromWaitingForOps", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_TurnLimitReachedFromDrainingBoundary(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "TurnLimitReached"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "DrainingBoundary"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "Completed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromDrainingBoundary"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromDrainingBoundary"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromDrainingBoundary"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromDrainingBoundary"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromDrainingBoundary"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "TurnLimitReachedFromDrainingBoundary", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_TurnLimitReachedFromExtracting(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "TurnLimitReached"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Extracting"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "Completed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromExtracting"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromExtracting"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromExtracting"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromExtracting"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromExtracting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "TurnLimitReachedFromExtracting", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_TurnLimitReachedFromErrorRecovery(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "TurnLimitReached"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ErrorRecovery"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "Completed"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromErrorRecovery"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromErrorRecovery"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromErrorRecovery"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromErrorRecovery"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromErrorRecovery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "TurnLimitReachedFromErrorRecovery", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_BudgetExhaustedFromApplyingPrimitive(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "BudgetExhausted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ApplyingPrimitive"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "BudgetExhausted"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromApplyingPrimitive"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromApplyingPrimitive"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromApplyingPrimitive"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromApplyingPrimitive"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromApplyingPrimitive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "BudgetExhaustedFromApplyingPrimitive", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_BudgetExhaustedFromCallingLlm(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "BudgetExhausted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "CallingLlm"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "BudgetExhausted"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromCallingLlm"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromCallingLlm"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromCallingLlm"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromCallingLlm"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromCallingLlm"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "BudgetExhaustedFromCallingLlm", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_BudgetExhaustedFromWaitingForOps(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "BudgetExhausted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "WaitingForOps"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_pending_op_refs' = None
       /\ turn_execution_barrier_operation_ids' = <<>>
       /\ turn_execution_has_barrier_ops' = FALSE
       /\ turn_execution_barrier_satisfied' = TRUE
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "BudgetExhausted"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromWaitingForOps"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromWaitingForOps"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromWaitingForOps"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromWaitingForOps"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromWaitingForOps"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "BudgetExhaustedFromWaitingForOps", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_BudgetExhaustedFromDrainingBoundary(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "BudgetExhausted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "DrainingBoundary"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "BudgetExhausted"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromDrainingBoundary"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromDrainingBoundary"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromDrainingBoundary"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromDrainingBoundary"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromDrainingBoundary"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "BudgetExhaustedFromDrainingBoundary", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_BudgetExhaustedFromExtracting(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "BudgetExhausted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Extracting"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "BudgetExhausted"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromExtracting"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromExtracting"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromExtracting"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromExtracting"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromExtracting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "BudgetExhaustedFromExtracting", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_BudgetExhaustedFromErrorRecovery(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "BudgetExhausted"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ErrorRecovery"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Completed"
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "BudgetExhausted"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]), [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_ingress", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)], [machine |-> "runtime_control", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], source_kind |-> "route", source_route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", source_effect |-> "RunCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_execution_boundary_updates_ingress", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "runtime_ingress", target_input |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromErrorRecovery"], [route |-> "surface_execution_completion_updates_ingress", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_ingress", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "ordinary_ingress", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromErrorRecovery"], [route |-> "surface_execution_completion_notifies_control", source_machine |-> "turn_execution", effect |-> "RunCompleted", target_machine |-> "runtime_control", target_input |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromErrorRecovery"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "turn_execution", variant |-> "BoundaryApplied", payload |-> [boundary_sequence |-> (turn_execution_boundary_count) + 1, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromErrorRecovery"], [machine |-> "turn_execution", variant |-> "RunCompleted", payload |-> [run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromErrorRecovery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "BudgetExhaustedFromErrorRecovery", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ForceCancelNoRunFromReady ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "ForceCancelNoRun"
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Ready"
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "ForceCancelNoRunFromReady", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ForceCancelNoRunFromApplyingPrimitive ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "ForceCancelNoRun"
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ApplyingPrimitive"
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "ForceCancelNoRunFromApplyingPrimitive", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ForceCancelNoRunFromCallingLlm ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "ForceCancelNoRun"
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "CallingLlm"
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "ForceCancelNoRunFromCallingLlm", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ForceCancelNoRunFromWaitingForOps ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "ForceCancelNoRun"
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "WaitingForOps"
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_pending_op_refs' = None
       /\ turn_execution_barrier_operation_ids' = <<>>
       /\ turn_execution_has_barrier_ops' = FALSE
       /\ turn_execution_barrier_satisfied' = TRUE
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "ForceCancelNoRunFromWaitingForOps", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ForceCancelNoRunFromDrainingBoundary ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "ForceCancelNoRun"
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "DrainingBoundary"
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "ForceCancelNoRunFromDrainingBoundary", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ForceCancelNoRunFromExtracting ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "ForceCancelNoRun"
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Extracting"
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "ForceCancelNoRunFromExtracting", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ForceCancelNoRunFromErrorRecovery ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "ForceCancelNoRun"
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "ErrorRecovery"
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "ForceCancelNoRunFromErrorRecovery", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ForceCancelNoRunFromCancelling ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "ForceCancelNoRun"
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Cancelling"
       /\ turn_execution_phase' = "Cancelled"
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "ForceCancelNoRunFromCancelling", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_AcknowledgeTerminalFromCompleted(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "AcknowledgeTerminal"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Completed"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Ready"
       /\ turn_execution_active_run' = None
       /\ turn_execution_primitive_kind' = "None"
       /\ turn_execution_admitted_content_shape' = None
       /\ turn_execution_vision_enabled' = FALSE
       /\ turn_execution_image_tool_results_enabled' = FALSE
       /\ turn_execution_tool_calls_pending' = 0
       /\ turn_execution_boundary_count' = 0
       /\ turn_execution_cancel_after_boundary' = FALSE
       /\ turn_execution_terminal_outcome' = "None"
       /\ turn_execution_extraction_attempts' = 0
       /\ turn_execution_max_extraction_retries' = 0
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "AcknowledgeTerminalFromCompleted", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Ready"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_AcknowledgeTerminalFromFailed(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "AcknowledgeTerminal"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Failed"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Ready"
       /\ turn_execution_active_run' = None
       /\ turn_execution_primitive_kind' = "None"
       /\ turn_execution_admitted_content_shape' = None
       /\ turn_execution_vision_enabled' = FALSE
       /\ turn_execution_image_tool_results_enabled' = FALSE
       /\ turn_execution_tool_calls_pending' = 0
       /\ turn_execution_boundary_count' = 0
       /\ turn_execution_cancel_after_boundary' = FALSE
       /\ turn_execution_terminal_outcome' = "None"
       /\ turn_execution_extraction_attempts' = 0
       /\ turn_execution_max_extraction_retries' = 0
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "AcknowledgeTerminalFromFailed", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Ready"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_AcknowledgeTerminalFromCancelled(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "turn_execution"
       /\ packet.variant = "AcknowledgeTerminal"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("turn_executor")
       /\ turn_execution_phase = "Cancelled"
       /\ (turn_execution_active_run = Some(packet.payload.run_id))
       /\ turn_execution_phase' = "Ready"
       /\ turn_execution_active_run' = None
       /\ turn_execution_primitive_kind' = "None"
       /\ turn_execution_admitted_content_shape' = None
       /\ turn_execution_vision_enabled' = FALSE
       /\ turn_execution_image_tool_results_enabled' = FALSE
       /\ turn_execution_tool_calls_pending' = 0
       /\ turn_execution_boundary_count' = 0
       /\ turn_execution_cancel_after_boundary' = FALSE
       /\ turn_execution_terminal_outcome' = "None"
       /\ turn_execution_extraction_attempts' = 0
       /\ turn_execution_max_extraction_retries' = 0
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "AcknowledgeTerminalFromCancelled", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "Ready"]}
       /\ model_step_count' = model_step_count + 1


turn_execution_ready_has_no_active_run == ((turn_execution_phase # "Ready") \/ (turn_execution_active_run = None))
turn_execution_ready_has_no_admitted_content == ((turn_execution_phase # "Ready") \/ (turn_execution_admitted_content_shape = None))
turn_execution_non_ready_has_active_run == ((turn_execution_phase = "Ready") \/ (turn_execution_phase = "Completed") \/ (turn_execution_phase = "Failed") \/ (turn_execution_phase = "Cancelled") \/ (turn_execution_active_run # None))
turn_execution_waiting_for_ops_implies_pending_tools == ((turn_execution_phase # "WaitingForOps") \/ (turn_execution_tool_calls_pending > 0))
turn_execution_pending_op_refs_only_used_while_waiting == ((turn_execution_phase = "WaitingForOps") \/ (turn_execution_pending_op_refs = None))
turn_execution_ready_has_no_boundary_cancel_request == ((turn_execution_phase # "Ready") \/ (turn_execution_cancel_after_boundary = FALSE))
turn_execution_immediate_primitives_skip_llm_and_recovery == ((turn_execution_primitive_kind = "ConversationTurn") \/ ((turn_execution_phase # "CallingLlm") /\ (turn_execution_phase # "WaitingForOps") /\ (turn_execution_phase # "ErrorRecovery")))
turn_execution_terminal_states_match_terminal_outcome == (((turn_execution_phase # "Completed") \/ (turn_execution_terminal_outcome = "Completed") \/ (turn_execution_terminal_outcome = "BudgetExhausted")) /\ ((turn_execution_phase # "Failed") \/ (turn_execution_terminal_outcome = "Failed")) /\ ((turn_execution_phase # "Cancelled") \/ (turn_execution_terminal_outcome = "Cancelled")))
turn_execution_completed_runs_have_seen_a_boundary == ((turn_execution_phase # "Completed") \/ (turn_execution_boundary_count > 0))

Inject_control_initialize ==
    /\ ~([machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "control_initialize", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "control_initialize", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "control_initialize", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_surface_submit_candidate(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key) ==
    /\ ~([machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> arg_content_shape, handling_mode |-> arg_handling_mode, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "surface_submit_candidate", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> arg_content_shape, handling_mode |-> arg_handling_mode, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "surface_submit_candidate", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "SubmitWork", payload |-> [content_shape |-> arg_content_shape, handling_mode |-> arg_handling_mode, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "surface_submit_candidate", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_runtime_admission_accepted(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect) ==
    /\ ~([machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> arg_admission_effect, content_shape |-> arg_content_shape, handling_mode |-> arg_handling_mode, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "runtime_admission_accepted", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> arg_admission_effect, content_shape |-> arg_content_shape, handling_mode |-> arg_handling_mode, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "runtime_admission_accepted", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "AdmissionAccepted", payload |-> [admission_effect |-> arg_admission_effect, content_shape |-> arg_content_shape, handling_mode |-> arg_handling_mode, request_id |-> arg_request_id, reservation_key |-> arg_reservation_key, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "runtime_admission_accepted", source_machine |-> "external_entry", source_effect |-> "AdmissionAccepted", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_ingress_stage_drain_snapshot(arg_run_id, arg_contributing_work_ids) ==
    /\ ~([machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_work_ids |-> arg_contributing_work_ids, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "ingress_stage_drain_snapshot", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_work_ids |-> arg_contributing_work_ids, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "ingress_stage_drain_snapshot", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_ingress", variant |-> "StageDrainSnapshot", payload |-> [contributing_work_ids |-> arg_contributing_work_ids, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "ingress_stage_drain_snapshot", source_machine |-> "external_entry", source_effect |-> "StageDrainSnapshot", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_turn_primitive_applied(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled) ==
    /\ ~([machine |-> "turn_execution", variant |-> "PrimitiveApplied", payload |-> [admitted_content_shape |-> arg_admitted_content_shape, image_tool_results_enabled |-> arg_image_tool_results_enabled, run_id |-> arg_run_id, vision_enabled |-> arg_vision_enabled], source_kind |-> "entry", source_route |-> "turn_primitive_applied", source_machine |-> "external_entry", source_effect |-> "PrimitiveApplied", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "turn_execution", variant |-> "PrimitiveApplied", payload |-> [admitted_content_shape |-> arg_admitted_content_shape, image_tool_results_enabled |-> arg_image_tool_results_enabled, run_id |-> arg_run_id, vision_enabled |-> arg_vision_enabled], source_kind |-> "entry", source_route |-> "turn_primitive_applied", source_machine |-> "external_entry", source_effect |-> "PrimitiveApplied", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "turn_execution", variant |-> "PrimitiveApplied", payload |-> [admitted_content_shape |-> arg_admitted_content_shape, image_tool_results_enabled |-> arg_image_tool_results_enabled, run_id |-> arg_run_id, vision_enabled |-> arg_vision_enabled], source_kind |-> "entry", source_route |-> "turn_primitive_applied", source_machine |-> "external_entry", source_effect |-> "PrimitiveApplied", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_turn_llm_returned_terminal(arg_run_id) ==
    /\ ~([machine |-> "turn_execution", variant |-> "LlmReturnedTerminal", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_llm_returned_terminal", source_machine |-> "external_entry", source_effect |-> "LlmReturnedTerminal", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "turn_execution", variant |-> "LlmReturnedTerminal", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_llm_returned_terminal", source_machine |-> "external_entry", source_effect |-> "LlmReturnedTerminal", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "turn_execution", variant |-> "LlmReturnedTerminal", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_llm_returned_terminal", source_machine |-> "external_entry", source_effect |-> "LlmReturnedTerminal", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_turn_boundary_complete(arg_run_id) ==
    /\ ~([machine |-> "turn_execution", variant |-> "BoundaryComplete", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_boundary_complete", source_machine |-> "external_entry", source_effect |-> "BoundaryComplete", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "turn_execution", variant |-> "BoundaryComplete", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_boundary_complete", source_machine |-> "external_entry", source_effect |-> "BoundaryComplete", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "turn_execution", variant |-> "BoundaryComplete", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_boundary_complete", source_machine |-> "external_entry", source_effect |-> "BoundaryComplete", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_turn_fatal_failure(arg_run_id) ==
    /\ ~([machine |-> "turn_execution", variant |-> "FatalFailure", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_fatal_failure", source_machine |-> "external_entry", source_effect |-> "FatalFailure", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "turn_execution", variant |-> "FatalFailure", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_fatal_failure", source_machine |-> "external_entry", source_effect |-> "FatalFailure", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "turn_execution", variant |-> "FatalFailure", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_fatal_failure", source_machine |-> "external_entry", source_effect |-> "FatalFailure", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_surface_event_success_path ==
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
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_surface_event_inline_image_success_path ==
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
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_surface_event_failure_path ==
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
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_surface_event_cancel_path ==
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
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_surface_event_control_preemption ==
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
    /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, runtime_ingress_phase, runtime_ingress_admitted_inputs, runtime_ingress_admission_order, runtime_ingress_content_shape, runtime_ingress_request_id, runtime_ingress_reservation_key, runtime_ingress_policy_snapshot, runtime_ingress_handling_mode, runtime_ingress_lifecycle, runtime_ingress_terminal_outcome, runtime_ingress_queue, runtime_ingress_steer_queue, runtime_ingress_current_run, runtime_ingress_current_run_contributors, runtime_ingress_last_run, runtime_ingress_last_boundary_sequence, runtime_ingress_wake_requested, runtime_ingress_process_requested, runtime_ingress_silent_intent_overrides, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_pending_op_refs, turn_execution_barrier_operation_ids, turn_execution_has_barrier_ops, turn_execution_barrier_satisfied, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

CoreNext ==
    \/ DeliverQueuedRoute
    \/ runtime_control_Initialize
    \/ runtime_control_AttachFromIdle
    \/ runtime_control_DetachToIdle
    \/ \E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_BeginRunFromAttached(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRecovering(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunCompletedToIdle(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunCompletedToAttached(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunCompletedToRetired(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunFailedToIdle(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunFailedToAttached(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunFailedToRetired(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunCancelledToIdle(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunCancelledToAttached(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunCancelledToRetired(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunCompletedFromRetiredInFlight(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunFailedFromRetiredInFlight(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_control_RunCancelledFromRetiredInFlight(arg_run_id)
    \/ runtime_control_RecoverRequestedFromIdle
    \/ runtime_control_RecoverRequestedFromRunning
    \/ runtime_control_RecoverRequestedFromAttached
    \/ runtime_control_RecoverySucceeded
    \/ runtime_control_RetireRequestedFromIdle
    \/ runtime_control_RetireRequestedFromRunning
    \/ runtime_control_RetireRequestedFromAttached
    \/ runtime_control_ResetRequested
    \/ runtime_control_StopRequested
    \/ runtime_control_DestroyRequested
    \/ runtime_control_ResumeRequested
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromAttached(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)
    \/ \E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)
    \/ \E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)
    \/ \E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedAttached(arg_work_id, arg_reason)
    \/ \E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)
    \/ \E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)
    \/ \E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedAttached(arg_work_id, arg_existing_work_id)
    \/ runtime_control_ExternalToolDeltaReceivedIdle
    \/ runtime_control_ExternalToolDeltaReceivedRunning
    \/ runtime_control_ExternalToolDeltaReceivedRecovering
    \/ runtime_control_ExternalToolDeltaReceivedRetired
    \/ runtime_control_ExternalToolDeltaReceivedAttached
    \/ runtime_control_RecycleRequestedFromRetired
    \/ runtime_control_RecycleRequestedFromIdle
    \/ runtime_control_RecycleRequestedFromAttached
    \/ runtime_control_RecycleSucceeded
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_work_id, arg_content_shape, arg_request_id, arg_reservation_key, arg_policy)
    \/ \E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshotFromActive(arg_run_id, arg_contributing_work_ids)
    \/ \E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshotFromRetired(arg_run_id, arg_contributing_work_ids)
    \/ \E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryAppliedFromActive(arg_run_id, arg_boundary_sequence)
    \/ \E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryAppliedFromRetired(arg_run_id, arg_boundary_sequence)
    \/ \E arg_run_id \in RunIdValues : runtime_ingress_RunCompletedFromActive(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_ingress_RunCompletedFromRetired(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_ingress_RunFailedFromActive(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_ingress_RunFailedFromRetired(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_ingress_RunCancelledFromActive(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : runtime_ingress_RunCancelledFromRetired(arg_run_id)
    \/ \E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInputFromActive(arg_new_work_id, arg_old_work_id)
    \/ \E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInputFromRetired(arg_new_work_id, arg_old_work_id)
    \/ \E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputsFromActive(arg_aggregate_work_id, arg_source_work_ids)
    \/ \E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputsFromRetired(arg_aggregate_work_id, arg_source_work_ids)
    \/ runtime_ingress_Retire
    \/ runtime_ingress_ResetFromActive
    \/ runtime_ingress_ResetFromRetired
    \/ runtime_ingress_Destroy
    \/ runtime_ingress_RecoverFromActive
    \/ runtime_ingress_RecoverFromRetired
    \/ \E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromActive(arg_intents)
    \/ \E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromRetired(arg_intents)
    \/ \E arg_run_id \in RunIdValues : turn_execution_StartConversationRun(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_StartImmediateAppend(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_StartImmediateContext(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedConversationTurn(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppend(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppendCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContext(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContextCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E arg_run_id \in RunIdValues : \E arg_tool_count \in 0..2 : turn_execution_LlmReturnedToolCalls(arg_run_id, arg_tool_count)
    \/ \E arg_run_id \in RunIdValues : \E arg_op_refs \in SeqOfAsyncOpRefValues : \E arg_barrier_operation_ids \in SeqOfOperationIdValues : \E arg_has_barrier_ops \in BOOLEAN : turn_execution_RegisterPendingOps(arg_run_id, arg_op_refs, arg_barrier_operation_ids, arg_has_barrier_ops)
    \/ \E arg_run_id \in RunIdValues : \E arg_operation_ids \in SeqOfOperationIdValues : turn_execution_OpsBarrierSatisfied(arg_run_id, arg_operation_ids)
    \/ \E arg_run_id \in RunIdValues : turn_execution_ToolCallsResolved(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_LlmReturnedTerminal(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_BoundaryContinue(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_BoundaryContinueCancelsAfterBoundary(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_BoundaryComplete(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_BoundaryCompleteCancelsAfterBoundary(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : \E arg_max_retries \in 0..2 : turn_execution_EnterExtraction(arg_run_id, arg_max_retries)
    \/ \E arg_run_id \in RunIdValues : turn_execution_ExtractionValidationPassed(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_ExtractionRetry(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_ExtractionExhausted(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromCallingLlm(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromWaitingForOps(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromDrainingBoundary(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_RetryRequested(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromApplyingPrimitive(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromCallingLlm(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromWaitingForOps(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromDrainingBoundary(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromExtracting(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromErrorRecovery(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancelNowFromApplyingPrimitive(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancelNowFromCallingLlm(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancelNowFromWaitingForOps(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancelNowFromDrainingBoundary(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancelNowFromExtracting(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancelNowFromErrorRecovery(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromApplyingPrimitive(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromCallingLlm(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromWaitingForOps(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromDrainingBoundary(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromExtracting(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromErrorRecovery(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_CancellationObserved(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromApplyingPrimitive(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromCallingLlm(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromWaitingForOps(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromDrainingBoundary(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromExtracting(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromErrorRecovery(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromApplyingPrimitive(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromCallingLlm(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromWaitingForOps(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromDrainingBoundary(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromExtracting(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromErrorRecovery(arg_run_id)
    \/ turn_execution_ForceCancelNoRunFromReady
    \/ turn_execution_ForceCancelNoRunFromApplyingPrimitive
    \/ turn_execution_ForceCancelNoRunFromCallingLlm
    \/ turn_execution_ForceCancelNoRunFromWaitingForOps
    \/ turn_execution_ForceCancelNoRunFromDrainingBoundary
    \/ turn_execution_ForceCancelNoRunFromExtracting
    \/ turn_execution_ForceCancelNoRunFromErrorRecovery
    \/ turn_execution_ForceCancelNoRunFromCancelling
    \/ \E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCompleted(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromFailed(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCancelled(arg_run_id)
    \/ QuiescentStutter

InjectNext ==
    \/ Inject_control_initialize
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : Inject_surface_submit_candidate(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)
    \/ \E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : Inject_runtime_admission_accepted(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)
    \/ \E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : Inject_ingress_stage_drain_snapshot(arg_run_id, arg_contributing_work_ids)
    \/ \E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : Inject_turn_primitive_applied(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E arg_run_id \in RunIdValues : Inject_turn_llm_returned_terminal(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : Inject_turn_boundary_complete(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : Inject_turn_fatal_failure(arg_run_id)

Next ==
    \/ CoreNext
    \/ InjectNext

WitnessNext_surface_event_success_path ==
    \/ CoreNext
    \/ WitnessInjectNext_surface_event_success_path

WitnessNext_surface_event_inline_image_success_path ==
    \/ CoreNext
    \/ WitnessInjectNext_surface_event_inline_image_success_path

WitnessNext_surface_event_failure_path ==
    \/ CoreNext
    \/ WitnessInjectNext_surface_event_failure_path

WitnessNext_surface_event_cancel_path ==
    \/ CoreNext
    \/ WitnessInjectNext_surface_event_cancel_path

WitnessNext_surface_event_control_preemption ==
    \/ CoreNext
    \/ WitnessInjectNext_surface_event_control_preemption


surface_event_uses_canonical_runtime_admission == \A input_packet \in observed_inputs : ((input_packet.machine = "runtime_ingress" /\ input_packet.variant = "AdmitQueued") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "runtime_control" /\ input_packet.source_effect = "SubmitAdmittedIngressEffect" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "runtime_control" /\ effect_packet.variant = "SubmitAdmittedIngressEffect" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = input_packet.source_route /\ route_packet.source_machine = "runtime_control" /\ route_packet.effect = "SubmitAdmittedIngressEffect" /\ route_packet.target_machine = "runtime_ingress" /\ route_packet.target_input = "AdmitQueued" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))
surface_event_begin_run_requires_staged_drain == \A input_packet \in observed_inputs : ((input_packet.machine = "runtime_control" /\ input_packet.variant = "BeginRun") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "runtime_ingress" /\ input_packet.source_effect = "ReadyForRun" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "runtime_ingress" /\ effect_packet.variant = "ReadyForRun" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = input_packet.source_route /\ route_packet.source_machine = "runtime_ingress" /\ route_packet.effect = "ReadyForRun" /\ route_packet.target_machine = "runtime_control" /\ route_packet.target_input = "BeginRun" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))
surface_event_execution_completion_is_handled == \A effect_packet \in emitted_effects : ((effect_packet.machine = "turn_execution" /\ effect_packet.variant = "RunCompleted") => (\E route_packet_0 \in RoutePackets : /\ route_packet_0.source_machine = "turn_execution" /\ route_packet_0.effect = "RunCompleted" /\ route_packet_0.target_machine = "runtime_ingress" /\ route_packet_0.target_input = "RunCompleted" /\ route_packet_0.effect_id = effect_packet.effect_id /\ IF RouteDeliveryKind(route_packet_0.route) = "Immediate" THEN \E input_packet_0 \in observed_inputs : /\ input_packet_0.machine = "runtime_ingress" /\ input_packet_0.variant = "RunCompleted" /\ input_packet_0.source_kind = "route" /\ input_packet_0.source_route = route_packet_0.route /\ input_packet_0.source_machine = "turn_execution" /\ input_packet_0.source_effect = "RunCompleted" /\ input_packet_0.effect_id = effect_packet.effect_id /\ input_packet_0.payload = route_packet_0.payload ELSE TRUE /\ \E route_packet_1 \in RoutePackets : /\ route_packet_1.source_machine = "turn_execution" /\ route_packet_1.effect = "RunCompleted" /\ route_packet_1.target_machine = "runtime_control" /\ route_packet_1.target_input = "RunCompleted" /\ route_packet_1.effect_id = effect_packet.effect_id /\ IF RouteDeliveryKind(route_packet_1.route) = "Immediate" THEN \E input_packet_1 \in observed_inputs : /\ input_packet_1.machine = "runtime_control" /\ input_packet_1.variant = "RunCompleted" /\ input_packet_1.source_kind = "route" /\ input_packet_1.source_route = route_packet_1.route /\ input_packet_1.source_machine = "turn_execution" /\ input_packet_1.source_effect = "RunCompleted" /\ input_packet_1.effect_id = effect_packet.effect_id /\ input_packet_1.payload = route_packet_1.payload ELSE TRUE))
surface_event_execution_failure_is_handled == \A effect_packet \in emitted_effects : ((effect_packet.machine = "turn_execution" /\ effect_packet.variant = "RunFailed") => (\E route_packet_0 \in RoutePackets : /\ route_packet_0.source_machine = "turn_execution" /\ route_packet_0.effect = "RunFailed" /\ route_packet_0.target_machine = "runtime_ingress" /\ route_packet_0.target_input = "RunFailed" /\ route_packet_0.effect_id = effect_packet.effect_id /\ IF RouteDeliveryKind(route_packet_0.route) = "Immediate" THEN \E input_packet_0 \in observed_inputs : /\ input_packet_0.machine = "runtime_ingress" /\ input_packet_0.variant = "RunFailed" /\ input_packet_0.source_kind = "route" /\ input_packet_0.source_route = route_packet_0.route /\ input_packet_0.source_machine = "turn_execution" /\ input_packet_0.source_effect = "RunFailed" /\ input_packet_0.effect_id = effect_packet.effect_id /\ input_packet_0.payload = route_packet_0.payload ELSE TRUE /\ \E route_packet_1 \in RoutePackets : /\ route_packet_1.source_machine = "turn_execution" /\ route_packet_1.effect = "RunFailed" /\ route_packet_1.target_machine = "runtime_control" /\ route_packet_1.target_input = "RunFailed" /\ route_packet_1.effect_id = effect_packet.effect_id /\ IF RouteDeliveryKind(route_packet_1.route) = "Immediate" THEN \E input_packet_1 \in observed_inputs : /\ input_packet_1.machine = "runtime_control" /\ input_packet_1.variant = "RunFailed" /\ input_packet_1.source_kind = "route" /\ input_packet_1.source_route = route_packet_1.route /\ input_packet_1.source_machine = "turn_execution" /\ input_packet_1.source_effect = "RunFailed" /\ input_packet_1.effect_id = effect_packet.effect_id /\ input_packet_1.payload = route_packet_1.payload ELSE TRUE))
surface_event_execution_cancel_is_handled == \A effect_packet \in emitted_effects : ((effect_packet.machine = "turn_execution" /\ effect_packet.variant = "RunCancelled") => (\E route_packet_0 \in RoutePackets : /\ route_packet_0.source_machine = "turn_execution" /\ route_packet_0.effect = "RunCancelled" /\ route_packet_0.target_machine = "runtime_ingress" /\ route_packet_0.target_input = "RunCancelled" /\ route_packet_0.effect_id = effect_packet.effect_id /\ IF RouteDeliveryKind(route_packet_0.route) = "Immediate" THEN \E input_packet_0 \in observed_inputs : /\ input_packet_0.machine = "runtime_ingress" /\ input_packet_0.variant = "RunCancelled" /\ input_packet_0.source_kind = "route" /\ input_packet_0.source_route = route_packet_0.route /\ input_packet_0.source_machine = "turn_execution" /\ input_packet_0.source_effect = "RunCancelled" /\ input_packet_0.effect_id = effect_packet.effect_id /\ input_packet_0.payload = route_packet_0.payload ELSE TRUE /\ \E route_packet_1 \in RoutePackets : /\ route_packet_1.source_machine = "turn_execution" /\ route_packet_1.effect = "RunCancelled" /\ route_packet_1.target_machine = "runtime_control" /\ route_packet_1.target_input = "RunCancelled" /\ route_packet_1.effect_id = effect_packet.effect_id /\ IF RouteDeliveryKind(route_packet_1.route) = "Immediate" THEN \E input_packet_1 \in observed_inputs : /\ input_packet_1.machine = "runtime_control" /\ input_packet_1.variant = "RunCancelled" /\ input_packet_1.source_kind = "route" /\ input_packet_1.source_route = route_packet_1.route /\ input_packet_1.source_machine = "turn_execution" /\ input_packet_1.source_effect = "RunCancelled" /\ input_packet_1.effect_id = effect_packet.effect_id /\ input_packet_1.payload = route_packet_1.payload ELSE TRUE))
control_preempts_surface_event_ingress == <<"PreemptWhenReady", "control_plane", "ordinary_ingress">> \in SchedulerRules

RouteObserved_surface_event_enters_ingress == \E packet \in RoutePackets : packet.route = "surface_event_enters_ingress"
RouteCoverage_surface_event_enters_ingress == (RouteObserved_surface_event_enters_ingress \/ ~RouteObserved_surface_event_enters_ingress)
RouteObserved_surface_ingress_ready_starts_runtime_control == \E packet \in RoutePackets : packet.route = "surface_ingress_ready_starts_runtime_control"
RouteCoverage_surface_ingress_ready_starts_runtime_control == (RouteObserved_surface_ingress_ready_starts_runtime_control \/ ~RouteObserved_surface_ingress_ready_starts_runtime_control)
RouteObserved_surface_runtime_control_starts_execution == \E packet \in RoutePackets : packet.route = "surface_runtime_control_starts_execution"
RouteCoverage_surface_runtime_control_starts_execution == (RouteObserved_surface_runtime_control_starts_execution \/ ~RouteObserved_surface_runtime_control_starts_execution)
RouteObserved_surface_execution_boundary_updates_ingress == \E packet \in RoutePackets : packet.route = "surface_execution_boundary_updates_ingress"
RouteCoverage_surface_execution_boundary_updates_ingress == (RouteObserved_surface_execution_boundary_updates_ingress \/ ~RouteObserved_surface_execution_boundary_updates_ingress)
RouteObserved_surface_execution_completion_updates_ingress == \E packet \in RoutePackets : packet.route = "surface_execution_completion_updates_ingress"
RouteCoverage_surface_execution_completion_updates_ingress == (RouteObserved_surface_execution_completion_updates_ingress \/ ~RouteObserved_surface_execution_completion_updates_ingress)
RouteObserved_surface_execution_completion_notifies_control == \E packet \in RoutePackets : packet.route = "surface_execution_completion_notifies_control"
RouteCoverage_surface_execution_completion_notifies_control == (RouteObserved_surface_execution_completion_notifies_control \/ ~RouteObserved_surface_execution_completion_notifies_control)
RouteObserved_surface_execution_failure_updates_ingress == \E packet \in RoutePackets : packet.route = "surface_execution_failure_updates_ingress"
RouteCoverage_surface_execution_failure_updates_ingress == (RouteObserved_surface_execution_failure_updates_ingress \/ ~RouteObserved_surface_execution_failure_updates_ingress)
RouteObserved_surface_execution_failure_notifies_control == \E packet \in RoutePackets : packet.route = "surface_execution_failure_notifies_control"
RouteCoverage_surface_execution_failure_notifies_control == (RouteObserved_surface_execution_failure_notifies_control \/ ~RouteObserved_surface_execution_failure_notifies_control)
RouteObserved_surface_execution_cancel_updates_ingress == \E packet \in RoutePackets : packet.route = "surface_execution_cancel_updates_ingress"
RouteCoverage_surface_execution_cancel_updates_ingress == (RouteObserved_surface_execution_cancel_updates_ingress \/ ~RouteObserved_surface_execution_cancel_updates_ingress)
RouteObserved_surface_execution_cancel_notifies_control == \E packet \in RoutePackets : packet.route = "surface_execution_cancel_notifies_control"
RouteCoverage_surface_execution_cancel_notifies_control == (RouteObserved_surface_execution_cancel_notifies_control \/ ~RouteObserved_surface_execution_cancel_notifies_control)
SchedulerTriggered_PreemptWhenReady_control_plane_ordinary_ingress == /\ "control_plane" \in PendingActors /\ "ordinary_ingress" \in PendingActors
SchedulerCoverage_PreemptWhenReady_control_plane_ordinary_ingress == (SchedulerTriggered_PreemptWhenReady_control_plane_ordinary_ingress \/ ~SchedulerTriggered_PreemptWhenReady_control_plane_ordinary_ingress)
CoverageInstrumentation == RouteCoverage_surface_event_enters_ingress /\ RouteCoverage_surface_ingress_ready_starts_runtime_control /\ RouteCoverage_surface_runtime_control_starts_execution /\ RouteCoverage_surface_execution_boundary_updates_ingress /\ RouteCoverage_surface_execution_completion_updates_ingress /\ RouteCoverage_surface_execution_completion_notifies_control /\ RouteCoverage_surface_execution_failure_updates_ingress /\ RouteCoverage_surface_execution_failure_notifies_control /\ RouteCoverage_surface_execution_cancel_updates_ingress /\ RouteCoverage_surface_execution_cancel_notifies_control /\ SchedulerCoverage_PreemptWhenReady_control_plane_ordinary_ingress

CiStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 1 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(runtime_ingress_admitted_inputs) <= 1 /\ Len(runtime_ingress_admission_order) <= 1 /\ Cardinality(DOMAIN runtime_ingress_content_shape) <= 1 /\ Cardinality(DOMAIN runtime_ingress_request_id) <= 1 /\ Cardinality(DOMAIN runtime_ingress_reservation_key) <= 1 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 1 /\ Cardinality(DOMAIN runtime_ingress_handling_mode) <= 1 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 1 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 1 /\ Len(runtime_ingress_queue) <= 1 /\ Len(runtime_ingress_steer_queue) <= 1 /\ Len(runtime_ingress_current_run_contributors) <= 1 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 1 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 1 /\ Cardinality(runtime_ingress_silent_intent_overrides) <= 1 /\ Len(turn_execution_barrier_operation_ids) <= 1
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(runtime_ingress_admitted_inputs) <= 2 /\ Len(runtime_ingress_admission_order) <= 2 /\ Cardinality(DOMAIN runtime_ingress_content_shape) <= 2 /\ Cardinality(DOMAIN runtime_ingress_request_id) <= 2 /\ Cardinality(DOMAIN runtime_ingress_reservation_key) <= 2 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 2 /\ Cardinality(DOMAIN runtime_ingress_handling_mode) <= 2 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 2 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 2 /\ Len(runtime_ingress_queue) <= 2 /\ Len(runtime_ingress_steer_queue) <= 2 /\ Len(runtime_ingress_current_run_contributors) <= 2 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 2 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 2 /\ Cardinality(runtime_ingress_silent_intent_overrides) <= 2 /\ Len(turn_execution_barrier_operation_ids) <= 2
WitnessStateConstraint_surface_event_success_path == /\ model_step_count <= 16 /\ Len(pending_inputs) <= 7 /\ Cardinality(observed_inputs) <= 16 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 7 /\ Cardinality(emitted_effects) <= 9 /\ Cardinality(observed_transitions) <= 16 /\ Cardinality(runtime_ingress_admitted_inputs) <= 7 /\ Len(runtime_ingress_admission_order) <= 7 /\ Cardinality(DOMAIN runtime_ingress_content_shape) <= 4 /\ Cardinality(DOMAIN runtime_ingress_request_id) <= 4 /\ Cardinality(DOMAIN runtime_ingress_reservation_key) <= 4 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 4 /\ Cardinality(DOMAIN runtime_ingress_handling_mode) <= 4 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 4 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 4 /\ Len(runtime_ingress_queue) <= 7 /\ Len(runtime_ingress_steer_queue) <= 7 /\ Len(runtime_ingress_current_run_contributors) <= 7 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 4 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 4 /\ Cardinality(runtime_ingress_silent_intent_overrides) <= 7 /\ Len(turn_execution_barrier_operation_ids) <= 7
WitnessStateConstraint_surface_event_inline_image_success_path == /\ model_step_count <= 16 /\ Len(pending_inputs) <= 7 /\ Cardinality(observed_inputs) <= 16 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 7 /\ Cardinality(emitted_effects) <= 9 /\ Cardinality(observed_transitions) <= 16 /\ Cardinality(runtime_ingress_admitted_inputs) <= 7 /\ Len(runtime_ingress_admission_order) <= 7 /\ Cardinality(DOMAIN runtime_ingress_content_shape) <= 4 /\ Cardinality(DOMAIN runtime_ingress_request_id) <= 4 /\ Cardinality(DOMAIN runtime_ingress_reservation_key) <= 4 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 4 /\ Cardinality(DOMAIN runtime_ingress_handling_mode) <= 4 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 4 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 4 /\ Len(runtime_ingress_queue) <= 7 /\ Len(runtime_ingress_steer_queue) <= 7 /\ Len(runtime_ingress_current_run_contributors) <= 7 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 4 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 4 /\ Cardinality(runtime_ingress_silent_intent_overrides) <= 7 /\ Len(turn_execution_barrier_operation_ids) <= 7
WitnessStateConstraint_surface_event_failure_path == /\ model_step_count <= 13 /\ Len(pending_inputs) <= 6 /\ Cardinality(observed_inputs) <= 14 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 6 /\ Cardinality(emitted_effects) <= 8 /\ Cardinality(observed_transitions) <= 13 /\ Cardinality(runtime_ingress_admitted_inputs) <= 6 /\ Len(runtime_ingress_admission_order) <= 6 /\ Cardinality(DOMAIN runtime_ingress_content_shape) <= 4 /\ Cardinality(DOMAIN runtime_ingress_request_id) <= 4 /\ Cardinality(DOMAIN runtime_ingress_reservation_key) <= 4 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 4 /\ Cardinality(DOMAIN runtime_ingress_handling_mode) <= 4 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 4 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 4 /\ Len(runtime_ingress_queue) <= 6 /\ Len(runtime_ingress_steer_queue) <= 6 /\ Len(runtime_ingress_current_run_contributors) <= 6 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 4 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 4 /\ Cardinality(runtime_ingress_silent_intent_overrides) <= 6 /\ Len(turn_execution_barrier_operation_ids) <= 6
WitnessStateConstraint_surface_event_cancel_path == /\ model_step_count <= 15 /\ Len(pending_inputs) <= 6 /\ Cardinality(observed_inputs) <= 14 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 6 /\ Cardinality(emitted_effects) <= 9 /\ Cardinality(observed_transitions) <= 15 /\ Cardinality(runtime_ingress_admitted_inputs) <= 6 /\ Len(runtime_ingress_admission_order) <= 6 /\ Cardinality(DOMAIN runtime_ingress_content_shape) <= 4 /\ Cardinality(DOMAIN runtime_ingress_request_id) <= 4 /\ Cardinality(DOMAIN runtime_ingress_reservation_key) <= 4 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 4 /\ Cardinality(DOMAIN runtime_ingress_handling_mode) <= 4 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 4 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 4 /\ Len(runtime_ingress_queue) <= 6 /\ Len(runtime_ingress_steer_queue) <= 6 /\ Len(runtime_ingress_current_run_contributors) <= 6 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 4 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 4 /\ Cardinality(runtime_ingress_silent_intent_overrides) <= 6 /\ Len(turn_execution_barrier_operation_ids) <= 6
WitnessStateConstraint_surface_event_control_preemption == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 4 /\ Cardinality(observed_inputs) <= 9 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 3 /\ Cardinality(emitted_effects) <= 3 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(runtime_ingress_admitted_inputs) <= 4 /\ Len(runtime_ingress_admission_order) <= 4 /\ Cardinality(DOMAIN runtime_ingress_content_shape) <= 3 /\ Cardinality(DOMAIN runtime_ingress_request_id) <= 3 /\ Cardinality(DOMAIN runtime_ingress_reservation_key) <= 3 /\ Cardinality(DOMAIN runtime_ingress_policy_snapshot) <= 3 /\ Cardinality(DOMAIN runtime_ingress_handling_mode) <= 3 /\ Cardinality(DOMAIN runtime_ingress_lifecycle) <= 3 /\ Cardinality(DOMAIN runtime_ingress_terminal_outcome) <= 3 /\ Len(runtime_ingress_queue) <= 4 /\ Len(runtime_ingress_steer_queue) <= 4 /\ Len(runtime_ingress_current_run_contributors) <= 4 /\ Cardinality(DOMAIN runtime_ingress_last_run) <= 3 /\ Cardinality(DOMAIN runtime_ingress_last_boundary_sequence) <= 3 /\ Cardinality(runtime_ingress_silent_intent_overrides) <= 4 /\ Len(turn_execution_barrier_operation_ids) <= 4

Spec == Init /\ [][Next]_vars
WitnessSpec_surface_event_success_path == WitnessInit_surface_event_success_path /\ [] [WitnessNext_surface_event_success_path]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(runtime_control_AttachFromIdle) /\ WF_vars(runtime_control_DetachToIdle) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRecovering(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledFromRetiredInFlight(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverRequestedFromAttached) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_RetireRequestedFromAttached) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromAttached(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedAttached(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedAttached(arg_work_id, arg_existing_work_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedAttached) /\ WF_vars(runtime_control_RecycleRequestedFromRetired) /\ WF_vars(runtime_control_RecycleRequestedFromIdle) /\ WF_vars(runtime_control_RecycleRequestedFromAttached) /\ WF_vars(runtime_control_RecycleSucceeded) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_work_id, arg_content_shape, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshotFromActive(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshotFromRetired(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryAppliedFromActive(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryAppliedFromRetired(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompletedFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompletedFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailedFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailedFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelledFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelledFromRetired(arg_run_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInputFromActive(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInputFromRetired(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputsFromActive(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputsFromRetired(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(runtime_ingress_Retire) /\ WF_vars(runtime_ingress_ResetFromActive) /\ WF_vars(runtime_ingress_ResetFromRetired) /\ WF_vars(runtime_ingress_Destroy) /\ WF_vars(runtime_ingress_RecoverFromActive) /\ WF_vars(runtime_ingress_RecoverFromRetired) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromActive(arg_intents)) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromRetired(arg_intents)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartConversationRun(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateAppend(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateContext(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedConversationTurn(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppend(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppendCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContext(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContextCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_tool_count \in 0..2 : turn_execution_LlmReturnedToolCalls(arg_run_id, arg_tool_count)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_op_refs \in SeqOfAsyncOpRefValues : \E arg_barrier_operation_ids \in SeqOfOperationIdValues : \E arg_has_barrier_ops \in BOOLEAN : turn_execution_RegisterPendingOps(arg_run_id, arg_op_refs, arg_barrier_operation_ids, arg_has_barrier_ops)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_operation_ids \in SeqOfOperationIdValues : turn_execution_OpsBarrierSatisfied(arg_run_id, arg_operation_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ToolCallsResolved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_LlmReturnedTerminal(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinue(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinueCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryComplete(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryCompleteCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_max_retries \in 0..2 : turn_execution_EnterExtraction(arg_run_id, arg_max_retries)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionValidationPassed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionRetry(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionExhausted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RetryRequested(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancellationObserved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromErrorRecovery(arg_run_id)) /\ WF_vars(turn_execution_ForceCancelNoRunFromReady) /\ WF_vars(turn_execution_ForceCancelNoRunFromApplyingPrimitive) /\ WF_vars(turn_execution_ForceCancelNoRunFromCallingLlm) /\ WF_vars(turn_execution_ForceCancelNoRunFromWaitingForOps) /\ WF_vars(turn_execution_ForceCancelNoRunFromDrainingBoundary) /\ WF_vars(turn_execution_ForceCancelNoRunFromExtracting) /\ WF_vars(turn_execution_ForceCancelNoRunFromErrorRecovery) /\ WF_vars(turn_execution_ForceCancelNoRunFromCancelling) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCancelled(arg_run_id)) /\ WF_vars(WitnessInjectNext_surface_event_success_path)
WitnessSpec_surface_event_inline_image_success_path == WitnessInit_surface_event_inline_image_success_path /\ [] [WitnessNext_surface_event_inline_image_success_path]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(runtime_control_AttachFromIdle) /\ WF_vars(runtime_control_DetachToIdle) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRecovering(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledFromRetiredInFlight(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverRequestedFromAttached) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_RetireRequestedFromAttached) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromAttached(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedAttached(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedAttached(arg_work_id, arg_existing_work_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedAttached) /\ WF_vars(runtime_control_RecycleRequestedFromRetired) /\ WF_vars(runtime_control_RecycleRequestedFromIdle) /\ WF_vars(runtime_control_RecycleRequestedFromAttached) /\ WF_vars(runtime_control_RecycleSucceeded) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_work_id, arg_content_shape, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshotFromActive(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshotFromRetired(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryAppliedFromActive(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryAppliedFromRetired(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompletedFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompletedFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailedFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailedFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelledFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelledFromRetired(arg_run_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInputFromActive(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInputFromRetired(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputsFromActive(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputsFromRetired(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(runtime_ingress_Retire) /\ WF_vars(runtime_ingress_ResetFromActive) /\ WF_vars(runtime_ingress_ResetFromRetired) /\ WF_vars(runtime_ingress_Destroy) /\ WF_vars(runtime_ingress_RecoverFromActive) /\ WF_vars(runtime_ingress_RecoverFromRetired) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromActive(arg_intents)) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromRetired(arg_intents)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartConversationRun(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateAppend(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateContext(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedConversationTurn(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppend(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppendCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContext(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContextCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_tool_count \in 0..2 : turn_execution_LlmReturnedToolCalls(arg_run_id, arg_tool_count)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_op_refs \in SeqOfAsyncOpRefValues : \E arg_barrier_operation_ids \in SeqOfOperationIdValues : \E arg_has_barrier_ops \in BOOLEAN : turn_execution_RegisterPendingOps(arg_run_id, arg_op_refs, arg_barrier_operation_ids, arg_has_barrier_ops)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_operation_ids \in SeqOfOperationIdValues : turn_execution_OpsBarrierSatisfied(arg_run_id, arg_operation_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ToolCallsResolved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_LlmReturnedTerminal(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinue(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinueCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryComplete(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryCompleteCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_max_retries \in 0..2 : turn_execution_EnterExtraction(arg_run_id, arg_max_retries)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionValidationPassed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionRetry(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionExhausted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RetryRequested(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancellationObserved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromErrorRecovery(arg_run_id)) /\ WF_vars(turn_execution_ForceCancelNoRunFromReady) /\ WF_vars(turn_execution_ForceCancelNoRunFromApplyingPrimitive) /\ WF_vars(turn_execution_ForceCancelNoRunFromCallingLlm) /\ WF_vars(turn_execution_ForceCancelNoRunFromWaitingForOps) /\ WF_vars(turn_execution_ForceCancelNoRunFromDrainingBoundary) /\ WF_vars(turn_execution_ForceCancelNoRunFromExtracting) /\ WF_vars(turn_execution_ForceCancelNoRunFromErrorRecovery) /\ WF_vars(turn_execution_ForceCancelNoRunFromCancelling) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCancelled(arg_run_id)) /\ WF_vars(WitnessInjectNext_surface_event_inline_image_success_path)
WitnessSpec_surface_event_failure_path == WitnessInit_surface_event_failure_path /\ [] [WitnessNext_surface_event_failure_path]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(runtime_control_AttachFromIdle) /\ WF_vars(runtime_control_DetachToIdle) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRecovering(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledFromRetiredInFlight(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverRequestedFromAttached) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_RetireRequestedFromAttached) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromAttached(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedAttached(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedAttached(arg_work_id, arg_existing_work_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedAttached) /\ WF_vars(runtime_control_RecycleRequestedFromRetired) /\ WF_vars(runtime_control_RecycleRequestedFromIdle) /\ WF_vars(runtime_control_RecycleRequestedFromAttached) /\ WF_vars(runtime_control_RecycleSucceeded) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_work_id, arg_content_shape, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshotFromActive(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshotFromRetired(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryAppliedFromActive(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryAppliedFromRetired(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompletedFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompletedFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailedFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailedFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelledFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelledFromRetired(arg_run_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInputFromActive(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInputFromRetired(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputsFromActive(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputsFromRetired(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(runtime_ingress_Retire) /\ WF_vars(runtime_ingress_ResetFromActive) /\ WF_vars(runtime_ingress_ResetFromRetired) /\ WF_vars(runtime_ingress_Destroy) /\ WF_vars(runtime_ingress_RecoverFromActive) /\ WF_vars(runtime_ingress_RecoverFromRetired) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromActive(arg_intents)) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromRetired(arg_intents)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartConversationRun(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateAppend(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateContext(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedConversationTurn(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppend(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppendCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContext(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContextCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_tool_count \in 0..2 : turn_execution_LlmReturnedToolCalls(arg_run_id, arg_tool_count)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_op_refs \in SeqOfAsyncOpRefValues : \E arg_barrier_operation_ids \in SeqOfOperationIdValues : \E arg_has_barrier_ops \in BOOLEAN : turn_execution_RegisterPendingOps(arg_run_id, arg_op_refs, arg_barrier_operation_ids, arg_has_barrier_ops)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_operation_ids \in SeqOfOperationIdValues : turn_execution_OpsBarrierSatisfied(arg_run_id, arg_operation_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ToolCallsResolved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_LlmReturnedTerminal(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinue(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinueCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryComplete(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryCompleteCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_max_retries \in 0..2 : turn_execution_EnterExtraction(arg_run_id, arg_max_retries)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionValidationPassed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionRetry(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionExhausted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RetryRequested(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancellationObserved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromErrorRecovery(arg_run_id)) /\ WF_vars(turn_execution_ForceCancelNoRunFromReady) /\ WF_vars(turn_execution_ForceCancelNoRunFromApplyingPrimitive) /\ WF_vars(turn_execution_ForceCancelNoRunFromCallingLlm) /\ WF_vars(turn_execution_ForceCancelNoRunFromWaitingForOps) /\ WF_vars(turn_execution_ForceCancelNoRunFromDrainingBoundary) /\ WF_vars(turn_execution_ForceCancelNoRunFromExtracting) /\ WF_vars(turn_execution_ForceCancelNoRunFromErrorRecovery) /\ WF_vars(turn_execution_ForceCancelNoRunFromCancelling) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCancelled(arg_run_id)) /\ WF_vars(WitnessInjectNext_surface_event_failure_path)
WitnessSpec_surface_event_cancel_path == WitnessInit_surface_event_cancel_path /\ [] [WitnessNext_surface_event_cancel_path]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(runtime_control_AttachFromIdle) /\ WF_vars(runtime_control_DetachToIdle) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRecovering(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledFromRetiredInFlight(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverRequestedFromAttached) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_RetireRequestedFromAttached) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromAttached(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedAttached(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedAttached(arg_work_id, arg_existing_work_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedAttached) /\ WF_vars(runtime_control_RecycleRequestedFromRetired) /\ WF_vars(runtime_control_RecycleRequestedFromIdle) /\ WF_vars(runtime_control_RecycleRequestedFromAttached) /\ WF_vars(runtime_control_RecycleSucceeded) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_work_id, arg_content_shape, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshotFromActive(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshotFromRetired(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryAppliedFromActive(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryAppliedFromRetired(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompletedFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompletedFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailedFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailedFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelledFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelledFromRetired(arg_run_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInputFromActive(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInputFromRetired(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputsFromActive(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputsFromRetired(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(runtime_ingress_Retire) /\ WF_vars(runtime_ingress_ResetFromActive) /\ WF_vars(runtime_ingress_ResetFromRetired) /\ WF_vars(runtime_ingress_Destroy) /\ WF_vars(runtime_ingress_RecoverFromActive) /\ WF_vars(runtime_ingress_RecoverFromRetired) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromActive(arg_intents)) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromRetired(arg_intents)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartConversationRun(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateAppend(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateContext(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedConversationTurn(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppend(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppendCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContext(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContextCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_tool_count \in 0..2 : turn_execution_LlmReturnedToolCalls(arg_run_id, arg_tool_count)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_op_refs \in SeqOfAsyncOpRefValues : \E arg_barrier_operation_ids \in SeqOfOperationIdValues : \E arg_has_barrier_ops \in BOOLEAN : turn_execution_RegisterPendingOps(arg_run_id, arg_op_refs, arg_barrier_operation_ids, arg_has_barrier_ops)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_operation_ids \in SeqOfOperationIdValues : turn_execution_OpsBarrierSatisfied(arg_run_id, arg_operation_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ToolCallsResolved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_LlmReturnedTerminal(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinue(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinueCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryComplete(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryCompleteCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_max_retries \in 0..2 : turn_execution_EnterExtraction(arg_run_id, arg_max_retries)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionValidationPassed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionRetry(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionExhausted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RetryRequested(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancellationObserved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromErrorRecovery(arg_run_id)) /\ WF_vars(turn_execution_ForceCancelNoRunFromReady) /\ WF_vars(turn_execution_ForceCancelNoRunFromApplyingPrimitive) /\ WF_vars(turn_execution_ForceCancelNoRunFromCallingLlm) /\ WF_vars(turn_execution_ForceCancelNoRunFromWaitingForOps) /\ WF_vars(turn_execution_ForceCancelNoRunFromDrainingBoundary) /\ WF_vars(turn_execution_ForceCancelNoRunFromExtracting) /\ WF_vars(turn_execution_ForceCancelNoRunFromErrorRecovery) /\ WF_vars(turn_execution_ForceCancelNoRunFromCancelling) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCancelled(arg_run_id)) /\ WF_vars(WitnessInjectNext_surface_event_cancel_path)
WitnessSpec_surface_event_control_preemption == WitnessInit_surface_event_control_preemption /\ [] [WitnessNext_surface_event_control_preemption]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(runtime_control_AttachFromIdle) /\ WF_vars(runtime_control_DetachToIdle) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRecovering(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledFromRetiredInFlight(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverRequestedFromAttached) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_RetireRequestedFromAttached) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromAttached(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedAttached(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedAttached(arg_work_id, arg_existing_work_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedAttached) /\ WF_vars(runtime_control_RecycleRequestedFromRetired) /\ WF_vars(runtime_control_RecycleRequestedFromIdle) /\ WF_vars(runtime_control_RecycleRequestedFromAttached) /\ WF_vars(runtime_control_RecycleSucceeded) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitQueuedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_policy \in PolicyDecisionValues : runtime_ingress_AdmitConsumedOnAccept(arg_work_id, arg_content_shape, arg_request_id, arg_reservation_key, arg_policy)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshotFromActive(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_contributing_work_ids \in SeqOfWorkIdValues : runtime_ingress_StageDrainSnapshotFromRetired(arg_run_id, arg_contributing_work_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryAppliedFromActive(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_boundary_sequence \in 0..2 : runtime_ingress_BoundaryAppliedFromRetired(arg_run_id, arg_boundary_sequence)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompletedFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCompletedFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailedFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunFailedFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelledFromActive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_ingress_RunCancelledFromRetired(arg_run_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInputFromActive(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_new_work_id \in WorkIdValues : \E arg_old_work_id \in WorkIdValues : runtime_ingress_SupersedeQueuedInputFromRetired(arg_new_work_id, arg_old_work_id)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputsFromActive(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(\E arg_aggregate_work_id \in WorkIdValues : \E arg_source_work_ids \in SeqOfWorkIdValues : runtime_ingress_CoalesceQueuedInputsFromRetired(arg_aggregate_work_id, arg_source_work_ids)) /\ WF_vars(runtime_ingress_Retire) /\ WF_vars(runtime_ingress_ResetFromActive) /\ WF_vars(runtime_ingress_ResetFromRetired) /\ WF_vars(runtime_ingress_Destroy) /\ WF_vars(runtime_ingress_RecoverFromActive) /\ WF_vars(runtime_ingress_RecoverFromRetired) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromActive(arg_intents)) /\ WF_vars(\E arg_intents \in SetOfStringValues : runtime_ingress_SetSilentIntentOverridesFromRetired(arg_intents)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartConversationRun(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateAppend(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateContext(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedConversationTurn(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppend(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppendCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContext(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContextCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_tool_count \in 0..2 : turn_execution_LlmReturnedToolCalls(arg_run_id, arg_tool_count)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_op_refs \in SeqOfAsyncOpRefValues : \E arg_barrier_operation_ids \in SeqOfOperationIdValues : \E arg_has_barrier_ops \in BOOLEAN : turn_execution_RegisterPendingOps(arg_run_id, arg_op_refs, arg_barrier_operation_ids, arg_has_barrier_ops)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_operation_ids \in SeqOfOperationIdValues : turn_execution_OpsBarrierSatisfied(arg_run_id, arg_operation_ids)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ToolCallsResolved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_LlmReturnedTerminal(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinue(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinueCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryComplete(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryCompleteCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_max_retries \in 0..2 : turn_execution_EnterExtraction(arg_run_id, arg_max_retries)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionValidationPassed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionRetry(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionExhausted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RetryRequested(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancellationObserved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromErrorRecovery(arg_run_id)) /\ WF_vars(turn_execution_ForceCancelNoRunFromReady) /\ WF_vars(turn_execution_ForceCancelNoRunFromApplyingPrimitive) /\ WF_vars(turn_execution_ForceCancelNoRunFromCallingLlm) /\ WF_vars(turn_execution_ForceCancelNoRunFromWaitingForOps) /\ WF_vars(turn_execution_ForceCancelNoRunFromDrainingBoundary) /\ WF_vars(turn_execution_ForceCancelNoRunFromExtracting) /\ WF_vars(turn_execution_ForceCancelNoRunFromErrorRecovery) /\ WF_vars(turn_execution_ForceCancelNoRunFromCancelling) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCancelled(arg_run_id)) /\ WF_vars(WitnessInjectNext_surface_event_control_preemption)

WitnessRouteObserved_surface_event_success_path_surface_event_enters_ingress == <> RouteObserved_surface_event_enters_ingress
WitnessRouteObserved_surface_event_success_path_surface_ingress_ready_starts_runtime_control == <> RouteObserved_surface_ingress_ready_starts_runtime_control
WitnessRouteObserved_surface_event_success_path_surface_runtime_control_starts_execution == <> RouteObserved_surface_runtime_control_starts_execution
WitnessRouteObserved_surface_event_success_path_surface_execution_boundary_updates_ingress == <> RouteObserved_surface_execution_boundary_updates_ingress
WitnessRouteObserved_surface_event_success_path_surface_execution_completion_updates_ingress == <> RouteObserved_surface_execution_completion_updates_ingress
WitnessRouteObserved_surface_event_success_path_surface_execution_completion_notifies_control == <> RouteObserved_surface_execution_completion_notifies_control
WitnessStateObserved_surface_event_success_path_1 == <> (runtime_control_phase = "Idle" /\ runtime_control_current_run_id = None)
WitnessStateObserved_surface_event_success_path_2 == <> (runtime_ingress_phase = "Active" /\ runtime_ingress_current_run = None)
WitnessStateObserved_surface_event_success_path_3 == <> (turn_execution_phase = "Completed")
WitnessTransitionObserved_surface_event_success_path_runtime_control_Initialize == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "Initialize")
WitnessTransitionObserved_surface_event_success_path_runtime_control_AdmissionAcceptedIdleQueue == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "AdmissionAcceptedIdleQueue")
WitnessTransitionObserved_surface_event_success_path_runtime_ingress_AdmitQueuedQueue == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "AdmitQueuedQueue")
WitnessTransitionObserved_surface_event_success_path_runtime_ingress_StageDrainSnapshotFromActive == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "StageDrainSnapshotFromActive")
WitnessTransitionObserved_surface_event_success_path_runtime_control_BeginRunFromIdle == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "BeginRunFromIdle")
WitnessTransitionObserved_surface_event_success_path_turn_execution_StartConversationRun == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "StartConversationRun")
WitnessTransitionObserved_surface_event_success_path_turn_execution_BoundaryComplete == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "BoundaryComplete")
WitnessTransitionObserved_surface_event_success_path_runtime_ingress_RunCompletedFromActive == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "RunCompletedFromActive")
WitnessTransitionObserved_surface_event_success_path_runtime_control_RunCompletedToIdle == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "RunCompletedToIdle")
WitnessTransitionOrder_surface_event_success_path_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "AdmissionAcceptedIdleQueue" /\ later.machine = "runtime_ingress" /\ later.transition = "AdmitQueuedQueue" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_success_path_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_ingress" /\ earlier.transition = "StageDrainSnapshotFromActive" /\ later.machine = "runtime_control" /\ later.transition = "BeginRunFromIdle" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_success_path_3 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "BeginRunFromIdle" /\ later.machine = "turn_execution" /\ later.transition = "StartConversationRun" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_success_path_4 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "turn_execution" /\ earlier.transition = "BoundaryComplete" /\ later.machine = "runtime_ingress" /\ later.transition = "RunCompletedFromActive" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_success_path_5 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "turn_execution" /\ earlier.transition = "BoundaryComplete" /\ later.machine = "runtime_control" /\ later.transition = "RunCompletedToIdle" /\ earlier.step < later.step)
WitnessRouteObserved_surface_event_inline_image_success_path_surface_event_enters_ingress == <> RouteObserved_surface_event_enters_ingress
WitnessRouteObserved_surface_event_inline_image_success_path_surface_ingress_ready_starts_runtime_control == <> RouteObserved_surface_ingress_ready_starts_runtime_control
WitnessRouteObserved_surface_event_inline_image_success_path_surface_runtime_control_starts_execution == <> RouteObserved_surface_runtime_control_starts_execution
WitnessRouteObserved_surface_event_inline_image_success_path_surface_execution_boundary_updates_ingress == <> RouteObserved_surface_execution_boundary_updates_ingress
WitnessRouteObserved_surface_event_inline_image_success_path_surface_execution_completion_updates_ingress == <> RouteObserved_surface_execution_completion_updates_ingress
WitnessRouteObserved_surface_event_inline_image_success_path_surface_execution_completion_notifies_control == <> RouteObserved_surface_execution_completion_notifies_control
WitnessStateObserved_surface_event_inline_image_success_path_1 == <> (runtime_control_phase = "Idle" /\ runtime_control_current_run_id = None)
WitnessStateObserved_surface_event_inline_image_success_path_2 == <> (runtime_ingress_phase = "Active" /\ runtime_ingress_current_run = None)
WitnessStateObserved_surface_event_inline_image_success_path_3 == <> (turn_execution_phase = "Completed")
WitnessTransitionObserved_surface_event_inline_image_success_path_runtime_control_Initialize == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "Initialize")
WitnessTransitionObserved_surface_event_inline_image_success_path_runtime_control_AdmissionAcceptedIdleQueue == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "AdmissionAcceptedIdleQueue")
WitnessTransitionObserved_surface_event_inline_image_success_path_runtime_ingress_AdmitQueuedQueue == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "AdmitQueuedQueue")
WitnessTransitionObserved_surface_event_inline_image_success_path_runtime_ingress_StageDrainSnapshotFromActive == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "StageDrainSnapshotFromActive")
WitnessTransitionObserved_surface_event_inline_image_success_path_runtime_control_BeginRunFromIdle == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "BeginRunFromIdle")
WitnessTransitionObserved_surface_event_inline_image_success_path_turn_execution_StartConversationRun == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "StartConversationRun")
WitnessTransitionObserved_surface_event_inline_image_success_path_turn_execution_BoundaryComplete == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "BoundaryComplete")
WitnessTransitionObserved_surface_event_inline_image_success_path_runtime_ingress_RunCompletedFromActive == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "RunCompletedFromActive")
WitnessTransitionObserved_surface_event_inline_image_success_path_runtime_control_RunCompletedToIdle == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "RunCompletedToIdle")
WitnessTransitionOrder_surface_event_inline_image_success_path_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "AdmissionAcceptedIdleQueue" /\ later.machine = "runtime_ingress" /\ later.transition = "AdmitQueuedQueue" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_inline_image_success_path_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_ingress" /\ earlier.transition = "StageDrainSnapshotFromActive" /\ later.machine = "runtime_control" /\ later.transition = "BeginRunFromIdle" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_inline_image_success_path_3 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "BeginRunFromIdle" /\ later.machine = "turn_execution" /\ later.transition = "StartConversationRun" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_inline_image_success_path_4 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "turn_execution" /\ earlier.transition = "BoundaryComplete" /\ later.machine = "runtime_ingress" /\ later.transition = "RunCompletedFromActive" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_inline_image_success_path_5 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "turn_execution" /\ earlier.transition = "BoundaryComplete" /\ later.machine = "runtime_control" /\ later.transition = "RunCompletedToIdle" /\ earlier.step < later.step)
WitnessRouteObserved_surface_event_failure_path_surface_event_enters_ingress == <> RouteObserved_surface_event_enters_ingress
WitnessRouteObserved_surface_event_failure_path_surface_ingress_ready_starts_runtime_control == <> RouteObserved_surface_ingress_ready_starts_runtime_control
WitnessRouteObserved_surface_event_failure_path_surface_runtime_control_starts_execution == <> RouteObserved_surface_runtime_control_starts_execution
WitnessRouteObserved_surface_event_failure_path_surface_execution_failure_updates_ingress == <> RouteObserved_surface_execution_failure_updates_ingress
WitnessRouteObserved_surface_event_failure_path_surface_execution_failure_notifies_control == <> RouteObserved_surface_execution_failure_notifies_control
WitnessStateObserved_surface_event_failure_path_1 == <> (runtime_control_phase = "Idle")
WitnessStateObserved_surface_event_failure_path_2 == <> (runtime_ingress_phase = "Active" /\ runtime_ingress_current_run = None)
WitnessStateObserved_surface_event_failure_path_3 == <> (turn_execution_phase = "Failed")
WitnessTransitionObserved_surface_event_failure_path_runtime_control_AdmissionAcceptedIdleQueue == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "AdmissionAcceptedIdleQueue")
WitnessTransitionObserved_surface_event_failure_path_runtime_ingress_AdmitQueuedQueue == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "AdmitQueuedQueue")
WitnessTransitionObserved_surface_event_failure_path_runtime_ingress_StageDrainSnapshotFromActive == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "StageDrainSnapshotFromActive")
WitnessTransitionObserved_surface_event_failure_path_runtime_control_BeginRunFromIdle == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "BeginRunFromIdle")
WitnessTransitionObserved_surface_event_failure_path_turn_execution_StartConversationRun == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "StartConversationRun")
WitnessTransitionObserved_surface_event_failure_path_turn_execution_FatalFailureFromApplyingPrimitive == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "FatalFailureFromApplyingPrimitive")
WitnessTransitionObserved_surface_event_failure_path_runtime_ingress_RunFailedFromActive == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "RunFailedFromActive")
WitnessTransitionObserved_surface_event_failure_path_runtime_control_RunFailedToIdle == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "RunFailedToIdle")
WitnessTransitionOrder_surface_event_failure_path_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "AdmissionAcceptedIdleQueue" /\ later.machine = "runtime_ingress" /\ later.transition = "AdmitQueuedQueue" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_failure_path_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_ingress" /\ earlier.transition = "StageDrainSnapshotFromActive" /\ later.machine = "runtime_control" /\ later.transition = "BeginRunFromIdle" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_failure_path_3 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "BeginRunFromIdle" /\ later.machine = "turn_execution" /\ later.transition = "StartConversationRun" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_failure_path_4 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "turn_execution" /\ earlier.transition = "FatalFailureFromApplyingPrimitive" /\ later.machine = "runtime_ingress" /\ later.transition = "RunFailedFromActive" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_failure_path_5 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "turn_execution" /\ earlier.transition = "FatalFailureFromApplyingPrimitive" /\ later.machine = "runtime_control" /\ later.transition = "RunFailedToIdle" /\ earlier.step < later.step)
WitnessRouteObserved_surface_event_cancel_path_surface_event_enters_ingress == <> RouteObserved_surface_event_enters_ingress
WitnessRouteObserved_surface_event_cancel_path_surface_ingress_ready_starts_runtime_control == <> RouteObserved_surface_ingress_ready_starts_runtime_control
WitnessRouteObserved_surface_event_cancel_path_surface_runtime_control_starts_execution == <> RouteObserved_surface_runtime_control_starts_execution
WitnessRouteObserved_surface_event_cancel_path_surface_execution_cancel_updates_ingress == <> RouteObserved_surface_execution_cancel_updates_ingress
WitnessRouteObserved_surface_event_cancel_path_surface_execution_cancel_notifies_control == <> RouteObserved_surface_execution_cancel_notifies_control
WitnessStateObserved_surface_event_cancel_path_1 == <> (runtime_control_phase = "Idle" /\ runtime_control_current_run_id = None)
WitnessStateObserved_surface_event_cancel_path_2 == <> (runtime_ingress_phase = "Active" /\ runtime_ingress_current_run = None)
WitnessStateObserved_surface_event_cancel_path_3 == <> (turn_execution_phase = "Cancelled")
WitnessTransitionObserved_surface_event_cancel_path_runtime_control_AdmissionAcceptedIdleQueue == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "AdmissionAcceptedIdleQueue")
WitnessTransitionObserved_surface_event_cancel_path_runtime_ingress_AdmitQueuedQueue == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "AdmitQueuedQueue")
WitnessTransitionObserved_surface_event_cancel_path_runtime_ingress_StageDrainSnapshotFromActive == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "StageDrainSnapshotFromActive")
WitnessTransitionObserved_surface_event_cancel_path_runtime_control_BeginRunFromIdle == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "BeginRunFromIdle")
WitnessTransitionObserved_surface_event_cancel_path_turn_execution_StartConversationRun == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "StartConversationRun")
WitnessTransitionObserved_surface_event_cancel_path_turn_execution_CancelNowFromApplyingPrimitive == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "CancelNowFromApplyingPrimitive")
WitnessTransitionObserved_surface_event_cancel_path_turn_execution_CancellationObserved == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "CancellationObserved")
WitnessTransitionObserved_surface_event_cancel_path_runtime_ingress_RunCancelledFromActive == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "RunCancelledFromActive")
WitnessTransitionObserved_surface_event_cancel_path_runtime_control_RunCancelledToIdle == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "RunCancelledToIdle")
WitnessTransitionOrder_surface_event_cancel_path_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "AdmissionAcceptedIdleQueue" /\ later.machine = "runtime_ingress" /\ later.transition = "AdmitQueuedQueue" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_cancel_path_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_ingress" /\ earlier.transition = "StageDrainSnapshotFromActive" /\ later.machine = "runtime_control" /\ later.transition = "BeginRunFromIdle" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_cancel_path_3 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "BeginRunFromIdle" /\ later.machine = "turn_execution" /\ later.transition = "StartConversationRun" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_cancel_path_4 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "turn_execution" /\ earlier.transition = "CancellationObserved" /\ later.machine = "runtime_ingress" /\ later.transition = "RunCancelledFromActive" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_cancel_path_5 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "turn_execution" /\ earlier.transition = "CancellationObserved" /\ later.machine = "runtime_control" /\ later.transition = "RunCancelledToIdle" /\ earlier.step < later.step)
WitnessRouteObserved_surface_event_control_preemption_surface_event_enters_ingress == <> RouteObserved_surface_event_enters_ingress
WitnessSchedulerTriggered_surface_event_control_preemption_PreemptWhenReady_control_plane_ordinary_ingress == <> SchedulerTriggered_PreemptWhenReady_control_plane_ordinary_ingress
WitnessStateObserved_surface_event_control_preemption_1 == <> (runtime_control_phase = "Idle")
WitnessTransitionObserved_surface_event_control_preemption_runtime_control_Initialize == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "Initialize")
WitnessTransitionObserved_surface_event_control_preemption_runtime_control_AdmissionAcceptedIdleSteer == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "AdmissionAcceptedIdleSteer")
WitnessTransitionObserved_surface_event_control_preemption_runtime_ingress_AdmitQueuedSteer == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_ingress" /\ packet.transition = "AdmitQueuedSteer")
WitnessTransitionOrder_surface_event_control_preemption_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "Initialize" /\ later.machine = "runtime_control" /\ later.transition = "AdmissionAcceptedIdleSteer" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_event_control_preemption_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "AdmissionAcceptedIdleSteer" /\ later.machine = "runtime_ingress" /\ later.transition = "AdmitQueuedSteer" /\ earlier.step < later.step)

THEOREM Spec => []surface_event_uses_canonical_runtime_admission
THEOREM Spec => []surface_event_begin_run_requires_staged_drain
THEOREM Spec => []surface_event_execution_completion_is_handled
THEOREM Spec => []surface_event_execution_failure_is_handled
THEOREM Spec => []surface_event_execution_cancel_is_handled
THEOREM Spec => []control_preempts_surface_event_ingress
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
THEOREM Spec => []turn_execution_ready_has_no_active_run
THEOREM Spec => []turn_execution_ready_has_no_admitted_content
THEOREM Spec => []turn_execution_non_ready_has_active_run
THEOREM Spec => []turn_execution_waiting_for_ops_implies_pending_tools
THEOREM Spec => []turn_execution_pending_op_refs_only_used_while_waiting
THEOREM Spec => []turn_execution_ready_has_no_boundary_cancel_request
THEOREM Spec => []turn_execution_immediate_primitives_skip_llm_and_recovery
THEOREM Spec => []turn_execution_terminal_states_match_terminal_outcome
THEOREM Spec => []turn_execution_completed_runs_have_seen_a_boundary

=============================================================================
