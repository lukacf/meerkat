---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for external_tool_bundle.

CONSTANTS AdmissionEffectValues, BooleanValues, ContentShapeValues, HandlingModeValues, NatValues, RequestIdValues, ReservationKeyValues, RunIdValues, StringValues, SurfaceIdValues, TurnNumberValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

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
    <<"external_tool_surface", "ExternalToolSurfaceMachine", "surface_boundary">>,
    <<"runtime_control", "RuntimeControlMachine", "control_plane">>,
    <<"turn_execution", "TurnExecutionMachine", "turn_executor">>
}

RouteNames == {
    "surface_delta_notifies_runtime_control",
    "turn_boundary_applies_surface_changes"
}

Actors == {
    "surface_boundary",
    "control_plane",
    "turn_executor"
}

ActorPriorities == {
    <<"control_plane", "surface_boundary">>
}

SchedulerRules == {
    <<"PreemptWhenReady", "control_plane", "surface_boundary">>
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "external_tool_surface" -> "surface_boundary"
      [] machine_id = "runtime_control" -> "control_plane"
      [] machine_id = "turn_execution" -> "turn_executor"
      [] OTHER -> "unknown_actor"

RouteSource(route_name) ==
    CASE route_name = "surface_delta_notifies_runtime_control" -> "external_tool_surface"
      [] route_name = "turn_boundary_applies_surface_changes" -> "turn_execution"
      [] OTHER -> "unknown_machine"

RouteEffect(route_name) ==
    CASE route_name = "surface_delta_notifies_runtime_control" -> "EmitExternalToolDelta"
      [] route_name = "turn_boundary_applies_surface_changes" -> "BoundaryApplied"
      [] OTHER -> "unknown_effect"

RouteTargetMachine(route_name) ==
    CASE route_name = "surface_delta_notifies_runtime_control" -> "runtime_control"
      [] route_name = "turn_boundary_applies_surface_changes" -> "external_tool_surface"
      [] OTHER -> "unknown_machine"

RouteTargetInput(route_name) ==
    CASE route_name = "surface_delta_notifies_runtime_control" -> "ExternalToolDeltaReceived"
      [] route_name = "turn_boundary_applies_surface_changes" -> "ApplyBoundary"
      [] OTHER -> "unknown_input"

RouteDeliveryKind(route_name) ==
    CASE route_name = "surface_delta_notifies_runtime_control" -> "Immediate"
      [] route_name = "turn_boundary_applies_surface_changes" -> "Immediate"
      [] OTHER -> "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ external_tool_surface_phase = "Operating"
    /\ external_tool_surface_known_surfaces = {}
    /\ external_tool_surface_visible_surfaces = {}
    /\ external_tool_surface_base_state = [x \in {} |-> None]
    /\ external_tool_surface_pending_op = [x \in {} |-> None]
    /\ external_tool_surface_staged_op = [x \in {} |-> None]
    /\ external_tool_surface_inflight_calls = [x \in {} |-> None]
    /\ external_tool_surface_last_delta_operation = [x \in {} |-> None]
    /\ external_tool_surface_last_delta_phase = [x \in {} |-> None]
    /\ runtime_control_phase = "Initializing"
    /\ runtime_control_current_run_id = None
    /\ runtime_control_pre_run_state = None
    /\ runtime_control_wake_pending = FALSE
    /\ runtime_control_process_pending = FALSE
    /\ turn_execution_phase = "Ready"
    /\ turn_execution_active_run = None
    /\ turn_execution_primitive_kind = "None"
    /\ turn_execution_admitted_content_shape = None
    /\ turn_execution_vision_enabled = FALSE
    /\ turn_execution_image_tool_results_enabled = FALSE
    /\ turn_execution_tool_calls_pending = 0
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

WitnessInit_surface_add_notifies_control ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_add_notifies_control:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_add_notifies_control:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:surface_add_notifies_control:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "external_tool_surface", variant |-> "StageAdd", payload |-> [surface_id |-> "default_surface"], source_kind |-> "entry", source_route |-> "witness:surface_add_notifies_control:2", source_machine |-> "external_entry", source_effect |-> "StageAdd", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> "runid_1"], source_kind |-> "entry", source_route |-> "witness:surface_add_notifies_control:3", source_machine |-> "external_entry", source_effect |-> "StartConversationRun", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "PrimitiveApplied", payload |-> [admitted_content_shape |-> "TextOnly", image_tool_results_enabled |-> FALSE, run_id |-> "runid_1", vision_enabled |-> FALSE], source_kind |-> "entry", source_route |-> "witness:surface_add_notifies_control:4", source_machine |-> "external_entry", source_effect |-> "PrimitiveApplied", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "LlmReturnedTerminal", payload |-> [run_id |-> "runid_1"], source_kind |-> "entry", source_route |-> "witness:surface_add_notifies_control:5", source_machine |-> "external_entry", source_effect |-> "LlmReturnedTerminal", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "BoundaryComplete", payload |-> [run_id |-> "runid_1"], source_kind |-> "entry", source_route |-> "witness:surface_add_notifies_control:6", source_machine |-> "external_entry", source_effect |-> "BoundaryComplete", effect_id |-> 0]>>

WitnessInit_turn_boundary_reaches_surface ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:turn_boundary_reaches_surface:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:turn_boundary_reaches_surface:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:turn_boundary_reaches_surface:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "external_tool_surface", variant |-> "StageAdd", payload |-> [surface_id |-> "default_surface"], source_kind |-> "entry", source_route |-> "witness:turn_boundary_reaches_surface:2", source_machine |-> "external_entry", source_effect |-> "StageAdd", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> "runid_1"], source_kind |-> "entry", source_route |-> "witness:turn_boundary_reaches_surface:3", source_machine |-> "external_entry", source_effect |-> "StartConversationRun", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "PrimitiveApplied", payload |-> [admitted_content_shape |-> "TextOnly", image_tool_results_enabled |-> FALSE, run_id |-> "runid_1", vision_enabled |-> FALSE], source_kind |-> "entry", source_route |-> "witness:turn_boundary_reaches_surface:4", source_machine |-> "external_entry", source_effect |-> "PrimitiveApplied", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "LlmReturnedTerminal", payload |-> [run_id |-> "runid_1"], source_kind |-> "entry", source_route |-> "witness:turn_boundary_reaches_surface:5", source_machine |-> "external_entry", source_effect |-> "LlmReturnedTerminal", effect_id |-> 0], [machine |-> "turn_execution", variant |-> "BoundaryComplete", payload |-> [run_id |-> "runid_1"], source_kind |-> "entry", source_route |-> "witness:turn_boundary_reaches_surface:6", source_machine |-> "external_entry", source_effect |-> "BoundaryComplete", effect_id |-> 0]>>

WitnessInit_control_preempts_surface ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:control_preempts_surface:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0], [machine |-> "external_tool_surface", variant |-> "StageAdd", payload |-> [surface_id |-> "surface_1"], source_kind |-> "entry", source_route |-> "witness:control_preempts_surface:2", source_machine |-> "external_entry", source_effect |-> "StageAdd", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:control_preempts_surface:1", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0], [machine |-> "external_tool_surface", variant |-> "StageAdd", payload |-> [surface_id |-> "surface_1"], source_kind |-> "entry", source_route |-> "witness:control_preempts_surface:2", source_machine |-> "external_entry", source_effect |-> "StageAdd", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "external_tool_surface", variant |-> "StageAdd", payload |-> [surface_id |-> "surface_1"], source_kind |-> "entry", source_route |-> "witness:control_preempts_surface:2", source_machine |-> "external_entry", source_effect |-> "StageAdd", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<>>

external_tool_surface__SurfaceBase(surface_id) == (IF ~((surface_id \in DOMAIN external_tool_surface_base_state)) THEN "Absent" ELSE (IF surface_id \in DOMAIN external_tool_surface_base_state THEN external_tool_surface_base_state[surface_id] ELSE "None"))

external_tool_surface__PendingOp(surface_id) == (IF ~((surface_id \in DOMAIN external_tool_surface_pending_op)) THEN "None" ELSE (IF surface_id \in DOMAIN external_tool_surface_pending_op THEN external_tool_surface_pending_op[surface_id] ELSE "None"))

external_tool_surface__StagedOp(surface_id) == (IF ~((surface_id \in DOMAIN external_tool_surface_staged_op)) THEN "None" ELSE (IF surface_id \in DOMAIN external_tool_surface_staged_op THEN external_tool_surface_staged_op[surface_id] ELSE "None"))

external_tool_surface__InflightCallCount(surface_id) == (IF ~((surface_id \in DOMAIN external_tool_surface_inflight_calls)) THEN 0 ELSE (IF surface_id \in DOMAIN external_tool_surface_inflight_calls THEN external_tool_surface_inflight_calls[surface_id] ELSE 0))

external_tool_surface__LastDeltaOperation(surface_id) == (IF ~((surface_id \in DOMAIN external_tool_surface_last_delta_operation)) THEN "None" ELSE (IF surface_id \in DOMAIN external_tool_surface_last_delta_operation THEN external_tool_surface_last_delta_operation[surface_id] ELSE "None"))

external_tool_surface__LastDeltaPhase(surface_id) == (IF ~((surface_id \in DOMAIN external_tool_surface_last_delta_phase)) THEN "None" ELSE (IF surface_id \in DOMAIN external_tool_surface_last_delta_phase THEN external_tool_surface_last_delta_phase[surface_id] ELSE "None"))

external_tool_surface__IsVisible(surface_id) == (surface_id \in external_tool_surface_visible_surfaces)

external_tool_surface_StageAdd(arg_surface_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "StageAdd"
       /\ packet.payload.surface_id = arg_surface_id
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_staged_op' = MapSet(external_tool_surface_staged_op, packet.payload.surface_id, "Add")
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "StageAdd", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_StageRemove(arg_surface_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "StageRemove"
       /\ packet.payload.surface_id = arg_surface_id
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_staged_op' = MapSet(external_tool_surface_staged_op, packet.payload.surface_id, "Remove")
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "StageRemove", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_StageReload(arg_surface_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "StageReload"
       /\ packet.payload.surface_id = arg_surface_id
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__SurfaceBase(packet.payload.surface_id) = "Active")
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_staged_op' = MapSet(external_tool_surface_staged_op, packet.payload.surface_id, "Reload")
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "StageReload", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_ApplyBoundaryAdd(arg_surface_id, arg_applied_at_turn) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "ApplyBoundary"
       /\ packet.payload.surface_id = arg_surface_id
       /\ packet.payload.applied_at_turn = arg_applied_at_turn
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__StagedOp(packet.payload.surface_id) = "Add")
       /\ ((external_tool_surface__SurfaceBase(packet.payload.surface_id) = "Absent") \/ (external_tool_surface__SurfaceBase(packet.payload.surface_id) = "Active") \/ (external_tool_surface__SurfaceBase(packet.payload.surface_id) = "Removed"))
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_pending_op' = MapSet(external_tool_surface_pending_op, packet.payload.surface_id, "Add")
       /\ external_tool_surface_staged_op' = MapSet(external_tool_surface_staged_op, packet.payload.surface_id, "None")
       /\ external_tool_surface_last_delta_operation' = MapSet(external_tool_surface_last_delta_operation, packet.payload.surface_id, "Add")
       /\ external_tool_surface_last_delta_phase' = MapSet(external_tool_surface_last_delta_phase, packet.payload.surface_id, "Pending")
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_inflight_calls, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", effect |-> "EmitExternalToolDelta", target_machine |-> "runtime_control", target_input |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "ApplyBoundaryAdd"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "external_tool_surface", variant |-> "ScheduleSurfaceCompletion", payload |-> [operation |-> "Add", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyBoundaryAdd"], [machine |-> "external_tool_surface", variant |-> "EmitExternalToolDelta", payload |-> [applied_at_turn |-> packet.payload.applied_at_turn, operation |-> "Add", persisted |-> FALSE, phase |-> "Pending", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyBoundaryAdd"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "ApplyBoundaryAdd", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_ApplyBoundaryReload(arg_surface_id, arg_applied_at_turn) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "ApplyBoundary"
       /\ packet.payload.surface_id = arg_surface_id
       /\ packet.payload.applied_at_turn = arg_applied_at_turn
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__StagedOp(packet.payload.surface_id) = "Reload")
       /\ (external_tool_surface__SurfaceBase(packet.payload.surface_id) = "Active")
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_pending_op' = MapSet(external_tool_surface_pending_op, packet.payload.surface_id, "Reload")
       /\ external_tool_surface_staged_op' = MapSet(external_tool_surface_staged_op, packet.payload.surface_id, "None")
       /\ external_tool_surface_last_delta_operation' = MapSet(external_tool_surface_last_delta_operation, packet.payload.surface_id, "Reload")
       /\ external_tool_surface_last_delta_phase' = MapSet(external_tool_surface_last_delta_phase, packet.payload.surface_id, "Pending")
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_inflight_calls, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", effect |-> "EmitExternalToolDelta", target_machine |-> "runtime_control", target_input |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "ApplyBoundaryReload"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "external_tool_surface", variant |-> "ScheduleSurfaceCompletion", payload |-> [operation |-> "Reload", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyBoundaryReload"], [machine |-> "external_tool_surface", variant |-> "EmitExternalToolDelta", payload |-> [applied_at_turn |-> packet.payload.applied_at_turn, operation |-> "Reload", persisted |-> FALSE, phase |-> "Pending", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyBoundaryReload"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "ApplyBoundaryReload", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_ApplyBoundaryRemoveDraining(arg_surface_id, arg_applied_at_turn) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "ApplyBoundary"
       /\ packet.payload.surface_id = arg_surface_id
       /\ packet.payload.applied_at_turn = arg_applied_at_turn
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__StagedOp(packet.payload.surface_id) = "Remove")
       /\ (external_tool_surface__SurfaceBase(packet.payload.surface_id) = "Active")
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_visible_surfaces' = (external_tool_surface_visible_surfaces \ {packet.payload.surface_id})
       /\ external_tool_surface_base_state' = MapSet(external_tool_surface_base_state, packet.payload.surface_id, "Removing")
       /\ external_tool_surface_pending_op' = MapSet(external_tool_surface_pending_op, packet.payload.surface_id, "None")
       /\ external_tool_surface_staged_op' = MapSet(external_tool_surface_staged_op, packet.payload.surface_id, "None")
       /\ external_tool_surface_last_delta_operation' = MapSet(external_tool_surface_last_delta_operation, packet.payload.surface_id, "Remove")
       /\ external_tool_surface_last_delta_phase' = MapSet(external_tool_surface_last_delta_phase, packet.payload.surface_id, "Draining")
       /\ UNCHANGED << external_tool_surface_inflight_calls, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", effect |-> "EmitExternalToolDelta", target_machine |-> "runtime_control", target_input |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "ApplyBoundaryRemoveDraining"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "external_tool_surface", variant |-> "RefreshVisibleSurfaceSet", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyBoundaryRemoveDraining"], [machine |-> "external_tool_surface", variant |-> "EmitExternalToolDelta", payload |-> [applied_at_turn |-> packet.payload.applied_at_turn, operation |-> "Remove", persisted |-> FALSE, phase |-> "Draining", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyBoundaryRemoveDraining"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "ApplyBoundaryRemoveDraining", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_ApplyBoundaryRemoveNoop(arg_surface_id, arg_applied_at_turn) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "ApplyBoundary"
       /\ packet.payload.surface_id = arg_surface_id
       /\ packet.payload.applied_at_turn = arg_applied_at_turn
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__StagedOp(packet.payload.surface_id) = "Remove")
       /\ (external_tool_surface__SurfaceBase(packet.payload.surface_id) # "Active")
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_pending_op' = MapSet(external_tool_surface_pending_op, packet.payload.surface_id, "None")
       /\ external_tool_surface_staged_op' = MapSet(external_tool_surface_staged_op, packet.payload.surface_id, "None")
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "ApplyBoundaryRemoveNoop", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_PendingSucceededAdd(arg_surface_id, arg_applied_at_turn) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "PendingSucceeded"
       /\ packet.payload.surface_id = arg_surface_id
       /\ packet.payload.applied_at_turn = arg_applied_at_turn
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__PendingOp(packet.payload.surface_id) = "Add")
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_visible_surfaces' = (external_tool_surface_visible_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_base_state' = MapSet(external_tool_surface_base_state, packet.payload.surface_id, "Active")
       /\ external_tool_surface_pending_op' = MapSet(external_tool_surface_pending_op, packet.payload.surface_id, "None")
       /\ external_tool_surface_last_delta_operation' = MapSet(external_tool_surface_last_delta_operation, packet.payload.surface_id, "Add")
       /\ external_tool_surface_last_delta_phase' = MapSet(external_tool_surface_last_delta_phase, packet.payload.surface_id, "Applied")
       /\ UNCHANGED << external_tool_surface_staged_op, external_tool_surface_inflight_calls, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", effect |-> "EmitExternalToolDelta", target_machine |-> "runtime_control", target_input |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "PendingSucceededAdd"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "external_tool_surface", variant |-> "RefreshVisibleSurfaceSet", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PendingSucceededAdd"], [machine |-> "external_tool_surface", variant |-> "EmitExternalToolDelta", payload |-> [applied_at_turn |-> packet.payload.applied_at_turn, operation |-> "Add", persisted |-> TRUE, phase |-> "Applied", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "PendingSucceededAdd"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "PendingSucceededAdd", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_PendingSucceededReload(arg_surface_id, arg_applied_at_turn) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "PendingSucceeded"
       /\ packet.payload.surface_id = arg_surface_id
       /\ packet.payload.applied_at_turn = arg_applied_at_turn
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__PendingOp(packet.payload.surface_id) = "Reload")
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_visible_surfaces' = (external_tool_surface_visible_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_base_state' = MapSet(external_tool_surface_base_state, packet.payload.surface_id, "Active")
       /\ external_tool_surface_pending_op' = MapSet(external_tool_surface_pending_op, packet.payload.surface_id, "None")
       /\ external_tool_surface_last_delta_operation' = MapSet(external_tool_surface_last_delta_operation, packet.payload.surface_id, "Reload")
       /\ external_tool_surface_last_delta_phase' = MapSet(external_tool_surface_last_delta_phase, packet.payload.surface_id, "Applied")
       /\ UNCHANGED << external_tool_surface_staged_op, external_tool_surface_inflight_calls, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", effect |-> "EmitExternalToolDelta", target_machine |-> "runtime_control", target_input |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "PendingSucceededReload"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "external_tool_surface", variant |-> "RefreshVisibleSurfaceSet", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PendingSucceededReload"], [machine |-> "external_tool_surface", variant |-> "EmitExternalToolDelta", payload |-> [applied_at_turn |-> packet.payload.applied_at_turn, operation |-> "Reload", persisted |-> TRUE, phase |-> "Applied", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "PendingSucceededReload"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "PendingSucceededReload", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_PendingFailedAdd(arg_surface_id, arg_applied_at_turn) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "PendingFailed"
       /\ packet.payload.surface_id = arg_surface_id
       /\ packet.payload.applied_at_turn = arg_applied_at_turn
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__PendingOp(packet.payload.surface_id) = "Add")
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_pending_op' = MapSet(external_tool_surface_pending_op, packet.payload.surface_id, "None")
       /\ external_tool_surface_last_delta_operation' = MapSet(external_tool_surface_last_delta_operation, packet.payload.surface_id, "Add")
       /\ external_tool_surface_last_delta_phase' = MapSet(external_tool_surface_last_delta_phase, packet.payload.surface_id, "Failed")
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_staged_op, external_tool_surface_inflight_calls, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", effect |-> "EmitExternalToolDelta", target_machine |-> "runtime_control", target_input |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "PendingFailedAdd"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "external_tool_surface", variant |-> "EmitExternalToolDelta", payload |-> [applied_at_turn |-> packet.payload.applied_at_turn, operation |-> "Add", persisted |-> TRUE, phase |-> "Failed", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "PendingFailedAdd"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "PendingFailedAdd", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_PendingFailedReload(arg_surface_id, arg_applied_at_turn) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "PendingFailed"
       /\ packet.payload.surface_id = arg_surface_id
       /\ packet.payload.applied_at_turn = arg_applied_at_turn
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__PendingOp(packet.payload.surface_id) = "Reload")
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_pending_op' = MapSet(external_tool_surface_pending_op, packet.payload.surface_id, "None")
       /\ external_tool_surface_last_delta_operation' = MapSet(external_tool_surface_last_delta_operation, packet.payload.surface_id, "Reload")
       /\ external_tool_surface_last_delta_phase' = MapSet(external_tool_surface_last_delta_phase, packet.payload.surface_id, "Failed")
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_staged_op, external_tool_surface_inflight_calls, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", effect |-> "EmitExternalToolDelta", target_machine |-> "runtime_control", target_input |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "PendingFailedReload"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "external_tool_surface", variant |-> "EmitExternalToolDelta", payload |-> [applied_at_turn |-> packet.payload.applied_at_turn, operation |-> "Reload", persisted |-> TRUE, phase |-> "Failed", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "PendingFailedReload"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "PendingFailedReload", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_CallStartedActive(arg_surface_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "CallStarted"
       /\ packet.payload.surface_id = arg_surface_id
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__SurfaceBase(packet.payload.surface_id) = "Active")
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_inflight_calls' = MapSet(external_tool_surface_inflight_calls, packet.payload.surface_id, (external_tool_surface__InflightCallCount(packet.payload.surface_id) + 1))
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "CallStartedActive", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_CallStartedRejectWhileRemoving(arg_surface_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "CallStarted"
       /\ packet.payload.surface_id = arg_surface_id
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__SurfaceBase(packet.payload.surface_id) = "Removing")
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "external_tool_surface", variant |-> "RejectSurfaceCall", payload |-> [reason |-> "surface_draining", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "CallStartedRejectWhileRemoving"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "CallStartedRejectWhileRemoving", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_CallStartedRejectWhileUnavailable(arg_surface_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "CallStarted"
       /\ packet.payload.surface_id = arg_surface_id
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ ((external_tool_surface__SurfaceBase(packet.payload.surface_id) # "Active") /\ (external_tool_surface__SurfaceBase(packet.payload.surface_id) # "Removing"))
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "external_tool_surface", variant |-> "RejectSurfaceCall", payload |-> [reason |-> "surface_unavailable", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "CallStartedRejectWhileUnavailable"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "CallStartedRejectWhileUnavailable", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_CallFinishedActive(arg_surface_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "CallFinished"
       /\ packet.payload.surface_id = arg_surface_id
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__SurfaceBase(packet.payload.surface_id) = "Active")
       /\ (external_tool_surface__InflightCallCount(packet.payload.surface_id) > 0)
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_inflight_calls' = MapSet(external_tool_surface_inflight_calls, packet.payload.surface_id, (external_tool_surface__InflightCallCount(packet.payload.surface_id) - 1))
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "CallFinishedActive", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_CallFinishedRemoving(arg_surface_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "CallFinished"
       /\ packet.payload.surface_id = arg_surface_id
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__SurfaceBase(packet.payload.surface_id) = "Removing")
       /\ (external_tool_surface__InflightCallCount(packet.payload.surface_id) > 0)
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_inflight_calls' = MapSet(external_tool_surface_inflight_calls, packet.payload.surface_id, (external_tool_surface__InflightCallCount(packet.payload.surface_id) - 1))
       /\ UNCHANGED << external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "CallFinishedRemoving", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_FinalizeRemovalClean(arg_surface_id, arg_applied_at_turn) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "FinalizeRemovalClean"
       /\ packet.payload.surface_id = arg_surface_id
       /\ packet.payload.applied_at_turn = arg_applied_at_turn
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__SurfaceBase(packet.payload.surface_id) = "Removing")
       /\ (external_tool_surface__InflightCallCount(packet.payload.surface_id) = 0)
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_visible_surfaces' = (external_tool_surface_visible_surfaces \ {packet.payload.surface_id})
       /\ external_tool_surface_base_state' = MapSet(external_tool_surface_base_state, packet.payload.surface_id, "Removed")
       /\ external_tool_surface_pending_op' = MapSet(external_tool_surface_pending_op, packet.payload.surface_id, "None")
       /\ external_tool_surface_last_delta_operation' = MapSet(external_tool_surface_last_delta_operation, packet.payload.surface_id, "Remove")
       /\ external_tool_surface_last_delta_phase' = MapSet(external_tool_surface_last_delta_phase, packet.payload.surface_id, "Applied")
       /\ UNCHANGED << external_tool_surface_staged_op, external_tool_surface_inflight_calls, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", effect |-> "EmitExternalToolDelta", target_machine |-> "runtime_control", target_input |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "FinalizeRemovalClean"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "external_tool_surface", variant |-> "CloseSurfaceConnection", payload |-> [surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "FinalizeRemovalClean"], [machine |-> "external_tool_surface", variant |-> "RefreshVisibleSurfaceSet", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FinalizeRemovalClean"], [machine |-> "external_tool_surface", variant |-> "EmitExternalToolDelta", payload |-> [applied_at_turn |-> packet.payload.applied_at_turn, operation |-> "Remove", persisted |-> TRUE, phase |-> "Applied", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "FinalizeRemovalClean"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "FinalizeRemovalClean", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_FinalizeRemovalForced(arg_surface_id, arg_applied_at_turn) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "FinalizeRemovalForced"
       /\ packet.payload.surface_id = arg_surface_id
       /\ packet.payload.applied_at_turn = arg_applied_at_turn
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating"
       /\ (external_tool_surface__SurfaceBase(packet.payload.surface_id) = "Removing")
       /\ external_tool_surface_phase' = "Operating"
       /\ external_tool_surface_known_surfaces' = (external_tool_surface_known_surfaces \cup {packet.payload.surface_id})
       /\ external_tool_surface_visible_surfaces' = (external_tool_surface_visible_surfaces \ {packet.payload.surface_id})
       /\ external_tool_surface_base_state' = MapSet(external_tool_surface_base_state, packet.payload.surface_id, "Removed")
       /\ external_tool_surface_pending_op' = MapSet(external_tool_surface_pending_op, packet.payload.surface_id, "None")
       /\ external_tool_surface_inflight_calls' = MapSet(external_tool_surface_inflight_calls, packet.payload.surface_id, 0)
       /\ external_tool_surface_last_delta_operation' = MapSet(external_tool_surface_last_delta_operation, packet.payload.surface_id, "Remove")
       /\ external_tool_surface_last_delta_phase' = MapSet(external_tool_surface_last_delta_phase, packet.payload.surface_id, "Forced")
       /\ UNCHANGED << external_tool_surface_staged_op, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", source_effect |-> "EmitExternalToolDelta", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "surface_delta_notifies_runtime_control", source_machine |-> "external_tool_surface", effect |-> "EmitExternalToolDelta", target_machine |-> "runtime_control", target_input |-> "ExternalToolDeltaReceived", payload |-> [tag |-> "unit"], actor |-> "control_plane", effect_id |-> (model_step_count + 1), source_transition |-> "FinalizeRemovalForced"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "external_tool_surface", variant |-> "CloseSurfaceConnection", payload |-> [surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "FinalizeRemovalForced"], [machine |-> "external_tool_surface", variant |-> "RefreshVisibleSurfaceSet", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FinalizeRemovalForced"], [machine |-> "external_tool_surface", variant |-> "EmitExternalToolDelta", payload |-> [applied_at_turn |-> packet.payload.applied_at_turn, operation |-> "Remove", persisted |-> TRUE, phase |-> "Forced", surface_id |-> packet.payload.surface_id], effect_id |-> (model_step_count + 1), source_transition |-> "FinalizeRemovalForced"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "FinalizeRemovalForced", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Operating"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_Shutdown ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "external_tool_surface"
       /\ packet.variant = "Shutdown"
       /\ ~HigherPriorityReady("surface_boundary")
       /\ external_tool_surface_phase = "Operating" \/ external_tool_surface_phase = "Shutdown"
       /\ external_tool_surface_phase' = "Shutdown"
       /\ external_tool_surface_known_surfaces' = {}
       /\ external_tool_surface_visible_surfaces' = {}
       /\ external_tool_surface_base_state' = [x \in {} |-> None]
       /\ external_tool_surface_pending_op' = [x \in {} |-> None]
       /\ external_tool_surface_staged_op' = [x \in {} |-> None]
       /\ external_tool_surface_inflight_calls' = [x \in {} |-> None]
       /\ external_tool_surface_last_delta_operation' = [x \in {} |-> None]
       /\ external_tool_surface_last_delta_phase' = [x \in {} |-> None]
       /\ UNCHANGED << runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "external_tool_surface", transition |-> "Shutdown", actor |-> "surface_boundary", step |-> (model_step_count + 1), from_phase |-> external_tool_surface_phase, to_phase |-> "Shutdown"]}
       /\ model_step_count' = model_step_count + 1


external_tool_surface_removing_or_removed_surfaces_are_not_visible == (\A surface_id \in external_tool_surface_known_surfaces : (((external_tool_surface__SurfaceBase(surface_id) = "Removing") /\ ~(external_tool_surface__IsVisible(surface_id))) \/ ((external_tool_surface__SurfaceBase(surface_id) = "Removed") /\ ~(external_tool_surface__IsVisible(surface_id))) \/ ((external_tool_surface__SurfaceBase(surface_id) # "Removing") /\ (external_tool_surface__SurfaceBase(surface_id) # "Removed"))))
external_tool_surface_visible_membership_matches_active_base_state == (\A surface_id \in external_tool_surface_known_surfaces : (external_tool_surface__IsVisible(surface_id) = (external_tool_surface__SurfaceBase(surface_id) = "Active")))
external_tool_surface_removing_surfaces_have_no_pending_add_or_reload == (\A surface_id \in external_tool_surface_known_surfaces : ((external_tool_surface__SurfaceBase(surface_id) # "Removing") \/ (external_tool_surface__PendingOp(surface_id) = "None")))
external_tool_surface_removed_surfaces_only_allow_pending_none_or_add == (\A surface_id \in external_tool_surface_known_surfaces : ((external_tool_surface__SurfaceBase(surface_id) # "Removed") \/ ((external_tool_surface__PendingOp(surface_id) = "None") \/ (external_tool_surface__PendingOp(surface_id) = "Add"))))
external_tool_surface_inflight_calls_only_exist_for_active_or_removing_surfaces == (\A surface_id \in external_tool_surface_known_surfaces : ((external_tool_surface__InflightCallCount(surface_id) = 0) \/ ((external_tool_surface__SurfaceBase(surface_id) = "Active") \/ (external_tool_surface__SurfaceBase(surface_id) = "Removing"))))
external_tool_surface_reload_pending_requires_active_base_state == (\A surface_id \in external_tool_surface_known_surfaces : ((external_tool_surface__PendingOp(surface_id) # "Reload") \/ (external_tool_surface__SurfaceBase(surface_id) = "Active")))
external_tool_surface_removed_surfaces_have_zero_inflight_calls == (\A surface_id \in external_tool_surface_known_surfaces : ((external_tool_surface__SurfaceBase(surface_id) # "Removed") \/ (external_tool_surface__InflightCallCount(surface_id) = 0)))
external_tool_surface_forced_delta_phase_is_always_a_remove_delta == (\A surface_id \in external_tool_surface_known_surfaces : ((external_tool_surface__LastDeltaPhase(surface_id) # "Forced") \/ (external_tool_surface__LastDeltaOperation(surface_id) = "Remove")))

runtime_control_Initialize ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_control"
       /\ packet.variant = "Initialize"
       /\ ~HigherPriorityReady("control_plane")
       /\ runtime_control_phase = "Initializing"
       /\ runtime_control_phase' = "Idle"
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_current_run_id, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_control", variant |-> "EmitRuntimeNotice", payload |-> [detail |-> "Succeeded", kind |-> "Recycle"], effect_id |-> (model_step_count + 1), source_transition |-> "RecycleSucceeded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_control", transition |-> "RecycleSucceeded", actor |-> "control_plane", step |-> (model_step_count + 1), from_phase |-> runtime_control_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


runtime_control_running_implies_active_run == ((runtime_control_phase # "Running") \/ (runtime_control_current_run_id # None))
runtime_control_active_run_only_while_running_or_retired == ((runtime_control_current_run_id = None) \/ (runtime_control_phase = "Running") \/ (runtime_control_phase = "Retired"))

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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateAppend"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_tool_calls_pending, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateAppendCancelsAfterBoundary"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateContext"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_tool_calls_pending, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedImmediateContextCancelsAfterBoundary"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "turn_execution", transition |-> "LlmReturnedToolCalls", actor |-> "turn_executor", step |-> (model_step_count + 1), from_phase |-> turn_execution_phase, to_phase |-> "WaitingForOps"]}
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
       /\ turn_execution_phase' = "DrainingBoundary"
       /\ turn_execution_tool_calls_pending' = 0
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "ToolCallsResolved"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "LlmReturnedTerminal"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ turn_execution_terminal_outcome' = "Failed"
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromApplyingPrimitive"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromCallingLlm"] }
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
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "Completed"
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromWaitingForOps"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromDrainingBoundary"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromExtracting"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedFromErrorRecovery"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromApplyingPrimitive"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromCallingLlm"] }
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
       /\ turn_execution_boundary_count' = (turn_execution_boundary_count) + 1
       /\ turn_execution_terminal_outcome' = "BudgetExhausted"
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromWaitingForOps"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromDrainingBoundary"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromExtracting"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], source_kind |-> "route", source_route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", source_effect |-> "BoundaryApplied", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "turn_boundary_applies_surface_changes", source_machine |-> "turn_execution", effect |-> "BoundaryApplied", target_machine |-> "external_tool_surface", target_input |-> "ApplyBoundary", payload |-> [applied_at_turn |-> "turn_1", surface_id |-> "default_surface"], actor |-> "surface_boundary", effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedFromErrorRecovery"] }
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ turn_execution_terminal_outcome' = "Cancelled"
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
turn_execution_ready_has_no_boundary_cancel_request == ((turn_execution_phase # "Ready") \/ (turn_execution_cancel_after_boundary = FALSE))
turn_execution_immediate_primitives_skip_llm_and_recovery == ((turn_execution_primitive_kind = "ConversationTurn") \/ ((turn_execution_phase # "CallingLlm") /\ (turn_execution_phase # "WaitingForOps") /\ (turn_execution_phase # "ErrorRecovery")))
turn_execution_terminal_states_match_terminal_outcome == (((turn_execution_phase # "Completed") \/ (turn_execution_terminal_outcome = "Completed") \/ (turn_execution_terminal_outcome = "BudgetExhausted")) /\ ((turn_execution_phase # "Failed") \/ (turn_execution_terminal_outcome = "Failed")) /\ ((turn_execution_phase # "Cancelled") \/ (turn_execution_terminal_outcome = "Cancelled")))
turn_execution_completed_runs_have_seen_a_boundary == ((turn_execution_phase # "Completed") \/ (turn_execution_boundary_count > 0))

Inject_control_initialize ==
    /\ ~([machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "control_initialize", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "control_initialize", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "runtime_control", variant |-> "Initialize", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "control_initialize", source_machine |-> "external_entry", source_effect |-> "Initialize", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_stage_add(arg_surface_id) ==
    /\ ~([machine |-> "external_tool_surface", variant |-> "StageAdd", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "stage_add", source_machine |-> "external_entry", source_effect |-> "StageAdd", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "external_tool_surface", variant |-> "StageAdd", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "stage_add", source_machine |-> "external_entry", source_effect |-> "StageAdd", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "StageAdd", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "stage_add", source_machine |-> "external_entry", source_effect |-> "StageAdd", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_stage_remove(arg_surface_id) ==
    /\ ~([machine |-> "external_tool_surface", variant |-> "StageRemove", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "stage_remove", source_machine |-> "external_entry", source_effect |-> "StageRemove", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "external_tool_surface", variant |-> "StageRemove", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "stage_remove", source_machine |-> "external_entry", source_effect |-> "StageRemove", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "StageRemove", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "stage_remove", source_machine |-> "external_entry", source_effect |-> "StageRemove", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_stage_reload(arg_surface_id) ==
    /\ ~([machine |-> "external_tool_surface", variant |-> "StageReload", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "stage_reload", source_machine |-> "external_entry", source_effect |-> "StageReload", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "external_tool_surface", variant |-> "StageReload", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "stage_reload", source_machine |-> "external_entry", source_effect |-> "StageReload", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "StageReload", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "stage_reload", source_machine |-> "external_entry", source_effect |-> "StageReload", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_pending_succeeded(arg_surface_id, arg_applied_at_turn) ==
    /\ ~([machine |-> "external_tool_surface", variant |-> "PendingSucceeded", payload |-> [applied_at_turn |-> arg_applied_at_turn, surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "pending_succeeded", source_machine |-> "external_entry", source_effect |-> "PendingSucceeded", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "external_tool_surface", variant |-> "PendingSucceeded", payload |-> [applied_at_turn |-> arg_applied_at_turn, surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "pending_succeeded", source_machine |-> "external_entry", source_effect |-> "PendingSucceeded", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "PendingSucceeded", payload |-> [applied_at_turn |-> arg_applied_at_turn, surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "pending_succeeded", source_machine |-> "external_entry", source_effect |-> "PendingSucceeded", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_pending_failed(arg_surface_id, arg_applied_at_turn) ==
    /\ ~([machine |-> "external_tool_surface", variant |-> "PendingFailed", payload |-> [applied_at_turn |-> arg_applied_at_turn, surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "pending_failed", source_machine |-> "external_entry", source_effect |-> "PendingFailed", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "external_tool_surface", variant |-> "PendingFailed", payload |-> [applied_at_turn |-> arg_applied_at_turn, surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "pending_failed", source_machine |-> "external_entry", source_effect |-> "PendingFailed", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "PendingFailed", payload |-> [applied_at_turn |-> arg_applied_at_turn, surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "pending_failed", source_machine |-> "external_entry", source_effect |-> "PendingFailed", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_call_started(arg_surface_id) ==
    /\ ~([machine |-> "external_tool_surface", variant |-> "CallStarted", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "call_started", source_machine |-> "external_entry", source_effect |-> "CallStarted", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "external_tool_surface", variant |-> "CallStarted", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "call_started", source_machine |-> "external_entry", source_effect |-> "CallStarted", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "CallStarted", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "call_started", source_machine |-> "external_entry", source_effect |-> "CallStarted", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_call_finished(arg_surface_id) ==
    /\ ~([machine |-> "external_tool_surface", variant |-> "CallFinished", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "call_finished", source_machine |-> "external_entry", source_effect |-> "CallFinished", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "external_tool_surface", variant |-> "CallFinished", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "call_finished", source_machine |-> "external_entry", source_effect |-> "CallFinished", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "CallFinished", payload |-> [surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "call_finished", source_machine |-> "external_entry", source_effect |-> "CallFinished", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_finalize_removal_clean(arg_surface_id, arg_applied_at_turn) ==
    /\ ~([machine |-> "external_tool_surface", variant |-> "FinalizeRemovalClean", payload |-> [applied_at_turn |-> arg_applied_at_turn, surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "finalize_removal_clean", source_machine |-> "external_entry", source_effect |-> "FinalizeRemovalClean", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "external_tool_surface", variant |-> "FinalizeRemovalClean", payload |-> [applied_at_turn |-> arg_applied_at_turn, surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "finalize_removal_clean", source_machine |-> "external_entry", source_effect |-> "FinalizeRemovalClean", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "FinalizeRemovalClean", payload |-> [applied_at_turn |-> arg_applied_at_turn, surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "finalize_removal_clean", source_machine |-> "external_entry", source_effect |-> "FinalizeRemovalClean", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_finalize_removal_forced(arg_surface_id, arg_applied_at_turn) ==
    /\ ~([machine |-> "external_tool_surface", variant |-> "FinalizeRemovalForced", payload |-> [applied_at_turn |-> arg_applied_at_turn, surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "finalize_removal_forced", source_machine |-> "external_entry", source_effect |-> "FinalizeRemovalForced", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "external_tool_surface", variant |-> "FinalizeRemovalForced", payload |-> [applied_at_turn |-> arg_applied_at_turn, surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "finalize_removal_forced", source_machine |-> "external_entry", source_effect |-> "FinalizeRemovalForced", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "external_tool_surface", variant |-> "FinalizeRemovalForced", payload |-> [applied_at_turn |-> arg_applied_at_turn, surface_id |-> arg_surface_id], source_kind |-> "entry", source_route |-> "finalize_removal_forced", source_machine |-> "external_entry", source_effect |-> "FinalizeRemovalForced", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_turn_start_conversation(arg_run_id) ==
    /\ ~([machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_start_conversation", source_machine |-> "external_entry", source_effect |-> "StartConversationRun", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_start_conversation", source_machine |-> "external_entry", source_effect |-> "StartConversationRun", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "turn_execution", variant |-> "StartConversationRun", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_start_conversation", source_machine |-> "external_entry", source_effect |-> "StartConversationRun", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_turn_primitive_applied(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled) ==
    /\ ~([machine |-> "turn_execution", variant |-> "PrimitiveApplied", payload |-> [admitted_content_shape |-> arg_admitted_content_shape, image_tool_results_enabled |-> arg_image_tool_results_enabled, run_id |-> arg_run_id, vision_enabled |-> arg_vision_enabled], source_kind |-> "entry", source_route |-> "turn_primitive_applied", source_machine |-> "external_entry", source_effect |-> "PrimitiveApplied", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "turn_execution", variant |-> "PrimitiveApplied", payload |-> [admitted_content_shape |-> arg_admitted_content_shape, image_tool_results_enabled |-> arg_image_tool_results_enabled, run_id |-> arg_run_id, vision_enabled |-> arg_vision_enabled], source_kind |-> "entry", source_route |-> "turn_primitive_applied", source_machine |-> "external_entry", source_effect |-> "PrimitiveApplied", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "turn_execution", variant |-> "PrimitiveApplied", payload |-> [admitted_content_shape |-> arg_admitted_content_shape, image_tool_results_enabled |-> arg_image_tool_results_enabled, run_id |-> arg_run_id, vision_enabled |-> arg_vision_enabled], source_kind |-> "entry", source_route |-> "turn_primitive_applied", source_machine |-> "external_entry", source_effect |-> "PrimitiveApplied", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_turn_llm_returned_terminal(arg_run_id) ==
    /\ ~([machine |-> "turn_execution", variant |-> "LlmReturnedTerminal", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_llm_returned_terminal", source_machine |-> "external_entry", source_effect |-> "LlmReturnedTerminal", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "turn_execution", variant |-> "LlmReturnedTerminal", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_llm_returned_terminal", source_machine |-> "external_entry", source_effect |-> "LlmReturnedTerminal", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "turn_execution", variant |-> "LlmReturnedTerminal", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_llm_returned_terminal", source_machine |-> "external_entry", source_effect |-> "LlmReturnedTerminal", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_turn_boundary_complete(arg_run_id) ==
    /\ ~([machine |-> "turn_execution", variant |-> "BoundaryComplete", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_boundary_complete", source_machine |-> "external_entry", source_effect |-> "BoundaryComplete", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "turn_execution", variant |-> "BoundaryComplete", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_boundary_complete", source_machine |-> "external_entry", source_effect |-> "BoundaryComplete", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "turn_execution", variant |-> "BoundaryComplete", payload |-> [run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "turn_boundary_complete", source_machine |-> "external_entry", source_effect |-> "BoundaryComplete", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_surface_add_notifies_control ==
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
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_turn_boundary_reaches_surface ==
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
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_control_preempts_surface ==
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
    /\ UNCHANGED << external_tool_surface_phase, external_tool_surface_known_surfaces, external_tool_surface_visible_surfaces, external_tool_surface_base_state, external_tool_surface_pending_op, external_tool_surface_staged_op, external_tool_surface_inflight_calls, external_tool_surface_last_delta_operation, external_tool_surface_last_delta_phase, runtime_control_phase, runtime_control_current_run_id, runtime_control_pre_run_state, runtime_control_wake_pending, runtime_control_process_pending, turn_execution_phase, turn_execution_active_run, turn_execution_primitive_kind, turn_execution_admitted_content_shape, turn_execution_vision_enabled, turn_execution_image_tool_results_enabled, turn_execution_tool_calls_pending, turn_execution_boundary_count, turn_execution_cancel_after_boundary, turn_execution_terminal_outcome, turn_execution_extraction_attempts, turn_execution_max_extraction_retries, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

CoreNext ==
    \/ DeliverQueuedRoute
    \/ \E arg_surface_id \in SurfaceIdValues : external_tool_surface_StageAdd(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : external_tool_surface_StageRemove(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : external_tool_surface_StageReload(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryAdd(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryReload(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryRemoveDraining(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryRemoveNoop(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingSucceededAdd(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingSucceededReload(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingFailedAdd(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingFailedReload(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallStartedActive(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallStartedRejectWhileRemoving(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallStartedRejectWhileUnavailable(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallFinishedActive(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallFinishedRemoving(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_FinalizeRemovalClean(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_FinalizeRemovalForced(arg_surface_id, arg_applied_at_turn)
    \/ external_tool_surface_Shutdown
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
    \/ \E arg_run_id \in RunIdValues : turn_execution_StartConversationRun(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_StartImmediateAppend(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : turn_execution_StartImmediateContext(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedConversationTurn(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppend(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppendCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContext(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContextCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E arg_run_id \in RunIdValues : \E arg_tool_count \in 0..2 : turn_execution_LlmReturnedToolCalls(arg_run_id, arg_tool_count)
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
    \/ \E arg_surface_id \in SurfaceIdValues : Inject_stage_add(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : Inject_stage_remove(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : Inject_stage_reload(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : Inject_pending_succeeded(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : Inject_pending_failed(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_surface_id \in SurfaceIdValues : Inject_call_started(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : Inject_call_finished(arg_surface_id)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : Inject_finalize_removal_clean(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : Inject_finalize_removal_forced(arg_surface_id, arg_applied_at_turn)
    \/ \E arg_run_id \in RunIdValues : Inject_turn_start_conversation(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : Inject_turn_primitive_applied(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E arg_run_id \in RunIdValues : Inject_turn_llm_returned_terminal(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : Inject_turn_boundary_complete(arg_run_id)

Next ==
    \/ CoreNext
    \/ InjectNext

WitnessNext_surface_add_notifies_control ==
    \/ CoreNext
    \/ WitnessInjectNext_surface_add_notifies_control

WitnessNext_turn_boundary_reaches_surface ==
    \/ CoreNext
    \/ WitnessInjectNext_turn_boundary_reaches_surface

WitnessNext_control_preempts_surface ==
    \/ CoreNext
    \/ WitnessInjectNext_control_preempts_surface


external_tool_delta_enters_runtime_control == \A input_packet \in observed_inputs : ((input_packet.machine = "runtime_control" /\ input_packet.variant = "ExternalToolDeltaReceived") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "external_tool_surface" /\ input_packet.source_effect = "EmitExternalToolDelta" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "external_tool_surface" /\ effect_packet.variant = "EmitExternalToolDelta" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = input_packet.source_route /\ route_packet.source_machine = "external_tool_surface" /\ route_packet.effect = "EmitExternalToolDelta" /\ route_packet.target_machine = "runtime_control" /\ route_packet.target_input = "ExternalToolDeltaReceived" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))
boundary_application_reaches_surface_authority == \A input_packet \in observed_inputs : ((input_packet.machine = "external_tool_surface" /\ input_packet.variant = "ApplyBoundary") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "turn_execution" /\ input_packet.source_effect = "BoundaryApplied" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "turn_execution" /\ effect_packet.variant = "BoundaryApplied" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = input_packet.source_route /\ route_packet.source_machine = "turn_execution" /\ route_packet.effect = "BoundaryApplied" /\ route_packet.target_machine = "external_tool_surface" /\ route_packet.target_input = "ApplyBoundary" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))
control_preempts_surface_boundary == <<"PreemptWhenReady", "control_plane", "surface_boundary">> \in SchedulerRules

RouteObserved_surface_delta_notifies_runtime_control == \E packet \in RoutePackets : packet.route = "surface_delta_notifies_runtime_control"
RouteCoverage_surface_delta_notifies_runtime_control == (RouteObserved_surface_delta_notifies_runtime_control \/ ~RouteObserved_surface_delta_notifies_runtime_control)
RouteObserved_turn_boundary_applies_surface_changes == \E packet \in RoutePackets : packet.route = "turn_boundary_applies_surface_changes"
RouteCoverage_turn_boundary_applies_surface_changes == (RouteObserved_turn_boundary_applies_surface_changes \/ ~RouteObserved_turn_boundary_applies_surface_changes)
SchedulerTriggered_PreemptWhenReady_control_plane_surface_boundary == /\ "control_plane" \in PendingActors /\ "surface_boundary" \in PendingActors
SchedulerCoverage_PreemptWhenReady_control_plane_surface_boundary == (SchedulerTriggered_PreemptWhenReady_control_plane_surface_boundary \/ ~SchedulerTriggered_PreemptWhenReady_control_plane_surface_boundary)
CoverageInstrumentation == RouteCoverage_surface_delta_notifies_runtime_control /\ RouteCoverage_turn_boundary_applies_surface_changes /\ SchedulerCoverage_PreemptWhenReady_control_plane_surface_boundary

CiStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 1 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(external_tool_surface_known_surfaces) <= 1 /\ Cardinality(external_tool_surface_visible_surfaces) <= 1 /\ Cardinality(DOMAIN external_tool_surface_base_state) <= 1 /\ Cardinality(DOMAIN external_tool_surface_pending_op) <= 1 /\ Cardinality(DOMAIN external_tool_surface_staged_op) <= 1 /\ Cardinality(DOMAIN external_tool_surface_inflight_calls) <= 1 /\ Cardinality(DOMAIN external_tool_surface_last_delta_operation) <= 1 /\ Cardinality(DOMAIN external_tool_surface_last_delta_phase) <= 1
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(external_tool_surface_known_surfaces) <= 2 /\ Cardinality(external_tool_surface_visible_surfaces) <= 2 /\ Cardinality(DOMAIN external_tool_surface_base_state) <= 2 /\ Cardinality(DOMAIN external_tool_surface_pending_op) <= 2 /\ Cardinality(DOMAIN external_tool_surface_staged_op) <= 2 /\ Cardinality(DOMAIN external_tool_surface_inflight_calls) <= 2 /\ Cardinality(DOMAIN external_tool_surface_last_delta_operation) <= 2 /\ Cardinality(DOMAIN external_tool_surface_last_delta_phase) <= 2
WitnessStateConstraint_surface_add_notifies_control == /\ model_step_count <= 14 /\ Len(pending_inputs) <= 6 /\ Cardinality(observed_inputs) <= 12 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 4 /\ Cardinality(emitted_effects) <= 8 /\ Cardinality(observed_transitions) <= 14 /\ Cardinality(external_tool_surface_known_surfaces) <= 6 /\ Cardinality(external_tool_surface_visible_surfaces) <= 6 /\ Cardinality(DOMAIN external_tool_surface_base_state) <= 3 /\ Cardinality(DOMAIN external_tool_surface_pending_op) <= 3 /\ Cardinality(DOMAIN external_tool_surface_staged_op) <= 3 /\ Cardinality(DOMAIN external_tool_surface_inflight_calls) <= 3 /\ Cardinality(DOMAIN external_tool_surface_last_delta_operation) <= 3 /\ Cardinality(DOMAIN external_tool_surface_last_delta_phase) <= 3
WitnessStateConstraint_turn_boundary_reaches_surface == /\ model_step_count <= 14 /\ Len(pending_inputs) <= 6 /\ Cardinality(observed_inputs) <= 12 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 4 /\ Cardinality(emitted_effects) <= 8 /\ Cardinality(observed_transitions) <= 14 /\ Cardinality(external_tool_surface_known_surfaces) <= 6 /\ Cardinality(external_tool_surface_visible_surfaces) <= 6 /\ Cardinality(DOMAIN external_tool_surface_base_state) <= 3 /\ Cardinality(DOMAIN external_tool_surface_pending_op) <= 3 /\ Cardinality(DOMAIN external_tool_surface_staged_op) <= 3 /\ Cardinality(DOMAIN external_tool_surface_inflight_calls) <= 3 /\ Cardinality(DOMAIN external_tool_surface_last_delta_operation) <= 3 /\ Cardinality(DOMAIN external_tool_surface_last_delta_phase) <= 3
WitnessStateConstraint_control_preempts_surface == /\ model_step_count <= 4 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 5 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 4 /\ Cardinality(external_tool_surface_known_surfaces) <= 2 /\ Cardinality(external_tool_surface_visible_surfaces) <= 2 /\ Cardinality(DOMAIN external_tool_surface_base_state) <= 2 /\ Cardinality(DOMAIN external_tool_surface_pending_op) <= 2 /\ Cardinality(DOMAIN external_tool_surface_staged_op) <= 2 /\ Cardinality(DOMAIN external_tool_surface_inflight_calls) <= 2 /\ Cardinality(DOMAIN external_tool_surface_last_delta_operation) <= 2 /\ Cardinality(DOMAIN external_tool_surface_last_delta_phase) <= 2

Spec == Init /\ [][Next]_vars
WitnessSpec_surface_add_notifies_control == WitnessInit_surface_add_notifies_control /\ [] [WitnessNext_surface_add_notifies_control]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_StageAdd(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_StageRemove(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_StageReload(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryAdd(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryReload(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryRemoveDraining(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryRemoveNoop(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingSucceededAdd(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingSucceededReload(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingFailedAdd(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingFailedReload(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallStartedActive(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallStartedRejectWhileRemoving(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallStartedRejectWhileUnavailable(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallFinishedActive(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallFinishedRemoving(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_FinalizeRemovalClean(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_FinalizeRemovalForced(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(external_tool_surface_Shutdown) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(runtime_control_AttachFromIdle) /\ WF_vars(runtime_control_DetachToIdle) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRecovering(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledFromRetiredInFlight(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverRequestedFromAttached) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_RetireRequestedFromAttached) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromAttached(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedAttached(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedAttached(arg_work_id, arg_existing_work_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedAttached) /\ WF_vars(runtime_control_RecycleRequestedFromRetired) /\ WF_vars(runtime_control_RecycleRequestedFromIdle) /\ WF_vars(runtime_control_RecycleRequestedFromAttached) /\ WF_vars(runtime_control_RecycleSucceeded) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartConversationRun(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateAppend(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateContext(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedConversationTurn(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppend(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppendCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContext(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContextCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_tool_count \in 0..2 : turn_execution_LlmReturnedToolCalls(arg_run_id, arg_tool_count)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ToolCallsResolved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_LlmReturnedTerminal(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinue(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinueCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryComplete(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryCompleteCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_max_retries \in 0..2 : turn_execution_EnterExtraction(arg_run_id, arg_max_retries)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionValidationPassed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionRetry(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionExhausted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RetryRequested(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancellationObserved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromErrorRecovery(arg_run_id)) /\ WF_vars(turn_execution_ForceCancelNoRunFromReady) /\ WF_vars(turn_execution_ForceCancelNoRunFromApplyingPrimitive) /\ WF_vars(turn_execution_ForceCancelNoRunFromCallingLlm) /\ WF_vars(turn_execution_ForceCancelNoRunFromWaitingForOps) /\ WF_vars(turn_execution_ForceCancelNoRunFromDrainingBoundary) /\ WF_vars(turn_execution_ForceCancelNoRunFromExtracting) /\ WF_vars(turn_execution_ForceCancelNoRunFromErrorRecovery) /\ WF_vars(turn_execution_ForceCancelNoRunFromCancelling) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCancelled(arg_run_id)) /\ WF_vars(WitnessInjectNext_surface_add_notifies_control)
WitnessSpec_turn_boundary_reaches_surface == WitnessInit_turn_boundary_reaches_surface /\ [] [WitnessNext_turn_boundary_reaches_surface]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_StageAdd(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_StageRemove(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_StageReload(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryAdd(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryReload(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryRemoveDraining(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryRemoveNoop(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingSucceededAdd(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingSucceededReload(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingFailedAdd(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingFailedReload(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallStartedActive(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallStartedRejectWhileRemoving(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallStartedRejectWhileUnavailable(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallFinishedActive(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallFinishedRemoving(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_FinalizeRemovalClean(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_FinalizeRemovalForced(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(external_tool_surface_Shutdown) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(runtime_control_AttachFromIdle) /\ WF_vars(runtime_control_DetachToIdle) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRecovering(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledFromRetiredInFlight(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverRequestedFromAttached) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_RetireRequestedFromAttached) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromAttached(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedAttached(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedAttached(arg_work_id, arg_existing_work_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedAttached) /\ WF_vars(runtime_control_RecycleRequestedFromRetired) /\ WF_vars(runtime_control_RecycleRequestedFromIdle) /\ WF_vars(runtime_control_RecycleRequestedFromAttached) /\ WF_vars(runtime_control_RecycleSucceeded) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartConversationRun(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateAppend(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateContext(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedConversationTurn(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppend(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppendCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContext(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContextCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_tool_count \in 0..2 : turn_execution_LlmReturnedToolCalls(arg_run_id, arg_tool_count)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ToolCallsResolved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_LlmReturnedTerminal(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinue(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinueCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryComplete(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryCompleteCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_max_retries \in 0..2 : turn_execution_EnterExtraction(arg_run_id, arg_max_retries)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionValidationPassed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionRetry(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionExhausted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RetryRequested(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancellationObserved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromErrorRecovery(arg_run_id)) /\ WF_vars(turn_execution_ForceCancelNoRunFromReady) /\ WF_vars(turn_execution_ForceCancelNoRunFromApplyingPrimitive) /\ WF_vars(turn_execution_ForceCancelNoRunFromCallingLlm) /\ WF_vars(turn_execution_ForceCancelNoRunFromWaitingForOps) /\ WF_vars(turn_execution_ForceCancelNoRunFromDrainingBoundary) /\ WF_vars(turn_execution_ForceCancelNoRunFromExtracting) /\ WF_vars(turn_execution_ForceCancelNoRunFromErrorRecovery) /\ WF_vars(turn_execution_ForceCancelNoRunFromCancelling) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCancelled(arg_run_id)) /\ WF_vars(WitnessInjectNext_turn_boundary_reaches_surface)
WitnessSpec_control_preempts_surface == WitnessInit_control_preempts_surface /\ [] [WitnessNext_control_preempts_surface]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_StageAdd(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_StageRemove(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_StageReload(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryAdd(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryReload(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryRemoveDraining(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_ApplyBoundaryRemoveNoop(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingSucceededAdd(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingSucceededReload(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingFailedAdd(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_PendingFailedReload(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallStartedActive(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallStartedRejectWhileRemoving(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallStartedRejectWhileUnavailable(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallFinishedActive(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : external_tool_surface_CallFinishedRemoving(arg_surface_id)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_FinalizeRemovalClean(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(\E arg_surface_id \in SurfaceIdValues : \E arg_applied_at_turn \in TurnNumberValues : external_tool_surface_FinalizeRemovalForced(arg_surface_id, arg_applied_at_turn)) /\ WF_vars(external_tool_surface_Shutdown) /\ WF_vars(runtime_control_Initialize) /\ WF_vars(runtime_control_AttachFromIdle) /\ WF_vars(runtime_control_DetachToIdle) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_BeginRunFromRecovering(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledToRetired(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCompletedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunFailedFromRetiredInFlight(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : runtime_control_RunCancelledFromRetiredInFlight(arg_run_id)) /\ WF_vars(runtime_control_RecoverRequestedFromIdle) /\ WF_vars(runtime_control_RecoverRequestedFromRunning) /\ WF_vars(runtime_control_RecoverRequestedFromAttached) /\ WF_vars(runtime_control_RecoverySucceeded) /\ WF_vars(runtime_control_RetireRequestedFromIdle) /\ WF_vars(runtime_control_RetireRequestedFromRunning) /\ WF_vars(runtime_control_RetireRequestedFromAttached) /\ WF_vars(runtime_control_ResetRequested) /\ WF_vars(runtime_control_StopRequested) /\ WF_vars(runtime_control_DestroyRequested) /\ WF_vars(runtime_control_ResumeRequested) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromIdle(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromRunning(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : runtime_control_SubmitWorkFromAttached(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedIdleSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedRunningSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedQueue(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_content_shape \in ContentShapeValues : \E arg_handling_mode \in HandlingModeValues : \E arg_request_id \in OptionRequestIdValues : \E arg_reservation_key \in OptionReservationKeyValues : \E arg_admission_effect \in AdmissionEffectValues : runtime_control_AdmissionAcceptedAttachedSteer(arg_work_id, arg_content_shape, arg_handling_mode, arg_request_id, arg_reservation_key, arg_admission_effect)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedIdle(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedRunning(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_reason \in {"alpha", "beta"} : runtime_control_AdmissionRejectedAttached(arg_work_id, arg_reason)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedIdle(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedRunning(arg_work_id, arg_existing_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : \E arg_existing_work_id \in WorkIdValues : runtime_control_AdmissionDeduplicatedAttached(arg_work_id, arg_existing_work_id)) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedIdle) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRunning) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRecovering) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedRetired) /\ WF_vars(runtime_control_ExternalToolDeltaReceivedAttached) /\ WF_vars(runtime_control_RecycleRequestedFromRetired) /\ WF_vars(runtime_control_RecycleRequestedFromIdle) /\ WF_vars(runtime_control_RecycleRequestedFromAttached) /\ WF_vars(runtime_control_RecycleSucceeded) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartConversationRun(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateAppend(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_StartImmediateContext(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedConversationTurn(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppend(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateAppendCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContext(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : turn_execution_PrimitiveAppliedImmediateContextCancelsAfterBoundary(arg_run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_tool_count \in 0..2 : turn_execution_LlmReturnedToolCalls(arg_run_id, arg_tool_count)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ToolCallsResolved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_LlmReturnedTerminal(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinue(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryContinueCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryComplete(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BoundaryCompleteCancelsAfterBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : \E arg_max_retries \in 0..2 : turn_execution_EnterExtraction(arg_run_id, arg_max_retries)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionValidationPassed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionRetry(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_ExtractionExhausted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RecoverableFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_RetryRequested(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_FatalFailureFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelNowFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancelAfterBoundaryFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_CancellationObserved(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_TurnLimitReachedFromErrorRecovery(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromApplyingPrimitive(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromCallingLlm(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromWaitingForOps(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromDrainingBoundary(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromExtracting(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_BudgetExhaustedFromErrorRecovery(arg_run_id)) /\ WF_vars(turn_execution_ForceCancelNoRunFromReady) /\ WF_vars(turn_execution_ForceCancelNoRunFromApplyingPrimitive) /\ WF_vars(turn_execution_ForceCancelNoRunFromCallingLlm) /\ WF_vars(turn_execution_ForceCancelNoRunFromWaitingForOps) /\ WF_vars(turn_execution_ForceCancelNoRunFromDrainingBoundary) /\ WF_vars(turn_execution_ForceCancelNoRunFromExtracting) /\ WF_vars(turn_execution_ForceCancelNoRunFromErrorRecovery) /\ WF_vars(turn_execution_ForceCancelNoRunFromCancelling) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCompleted(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromFailed(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : turn_execution_AcknowledgeTerminalFromCancelled(arg_run_id)) /\ WF_vars(WitnessInjectNext_control_preempts_surface)

WitnessRouteObserved_surface_add_notifies_control_turn_boundary_applies_surface_changes == <> RouteObserved_turn_boundary_applies_surface_changes
WitnessRouteObserved_surface_add_notifies_control_surface_delta_notifies_runtime_control == <> RouteObserved_surface_delta_notifies_runtime_control
WitnessStateObserved_surface_add_notifies_control_1 == <> (external_tool_surface_phase = "Operating")
WitnessStateObserved_surface_add_notifies_control_2 == <> (runtime_control_phase = "Idle")
WitnessStateObserved_surface_add_notifies_control_3 == <> (turn_execution_phase = "Completed")
WitnessTransitionObserved_surface_add_notifies_control_runtime_control_Initialize == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "Initialize")
WitnessTransitionObserved_surface_add_notifies_control_external_tool_surface_StageAdd == <> (\E packet \in observed_transitions : /\ packet.machine = "external_tool_surface" /\ packet.transition = "StageAdd")
WitnessTransitionObserved_surface_add_notifies_control_turn_execution_StartConversationRun == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "StartConversationRun")
WitnessTransitionObserved_surface_add_notifies_control_turn_execution_PrimitiveAppliedConversationTurn == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "PrimitiveAppliedConversationTurn")
WitnessTransitionObserved_surface_add_notifies_control_turn_execution_LlmReturnedTerminal == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "LlmReturnedTerminal")
WitnessTransitionObserved_surface_add_notifies_control_turn_execution_BoundaryComplete == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "BoundaryComplete")
WitnessTransitionObserved_surface_add_notifies_control_external_tool_surface_ApplyBoundaryAdd == <> (\E packet \in observed_transitions : /\ packet.machine = "external_tool_surface" /\ packet.transition = "ApplyBoundaryAdd")
WitnessTransitionObserved_surface_add_notifies_control_runtime_control_ExternalToolDeltaReceivedIdle == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "ExternalToolDeltaReceivedIdle")
WitnessTransitionOrder_surface_add_notifies_control_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "turn_execution" /\ earlier.transition = "LlmReturnedTerminal" /\ later.machine = "external_tool_surface" /\ later.transition = "ApplyBoundaryAdd" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_add_notifies_control_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "external_tool_surface" /\ earlier.transition = "ApplyBoundaryAdd" /\ later.machine = "turn_execution" /\ later.transition = "BoundaryComplete" /\ earlier.step < later.step)
WitnessTransitionOrder_surface_add_notifies_control_3 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "external_tool_surface" /\ earlier.transition = "ApplyBoundaryAdd" /\ later.machine = "runtime_control" /\ later.transition = "ExternalToolDeltaReceivedIdle" /\ earlier.step < later.step)
WitnessRouteObserved_turn_boundary_reaches_surface_turn_boundary_applies_surface_changes == <> RouteObserved_turn_boundary_applies_surface_changes
WitnessStateObserved_turn_boundary_reaches_surface_1 == <> (runtime_control_phase = "Idle")
WitnessStateObserved_turn_boundary_reaches_surface_2 == <> (turn_execution_phase = "Completed")
WitnessStateObserved_turn_boundary_reaches_surface_3 == <> (external_tool_surface_phase = "Operating")
WitnessTransitionObserved_turn_boundary_reaches_surface_runtime_control_Initialize == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "Initialize")
WitnessTransitionObserved_turn_boundary_reaches_surface_external_tool_surface_StageAdd == <> (\E packet \in observed_transitions : /\ packet.machine = "external_tool_surface" /\ packet.transition = "StageAdd")
WitnessTransitionObserved_turn_boundary_reaches_surface_turn_execution_StartConversationRun == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "StartConversationRun")
WitnessTransitionObserved_turn_boundary_reaches_surface_turn_execution_PrimitiveAppliedConversationTurn == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "PrimitiveAppliedConversationTurn")
WitnessTransitionObserved_turn_boundary_reaches_surface_turn_execution_LlmReturnedTerminal == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "LlmReturnedTerminal")
WitnessTransitionObserved_turn_boundary_reaches_surface_runtime_control_ExternalToolDeltaReceivedIdle == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "ExternalToolDeltaReceivedIdle")
WitnessTransitionObserved_turn_boundary_reaches_surface_turn_execution_BoundaryComplete == <> (\E packet \in observed_transitions : /\ packet.machine = "turn_execution" /\ packet.transition = "BoundaryComplete")
WitnessTransitionObserved_turn_boundary_reaches_surface_external_tool_surface_ApplyBoundaryAdd == <> (\E packet \in observed_transitions : /\ packet.machine = "external_tool_surface" /\ packet.transition = "ApplyBoundaryAdd")
WitnessTransitionOrder_turn_boundary_reaches_surface_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "turn_execution" /\ earlier.transition = "LlmReturnedTerminal" /\ later.machine = "external_tool_surface" /\ later.transition = "ApplyBoundaryAdd" /\ earlier.step < later.step)
WitnessTransitionOrder_turn_boundary_reaches_surface_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "external_tool_surface" /\ earlier.transition = "ApplyBoundaryAdd" /\ later.machine = "runtime_control" /\ later.transition = "ExternalToolDeltaReceivedIdle" /\ earlier.step < later.step)
WitnessTransitionOrder_turn_boundary_reaches_surface_3 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "ExternalToolDeltaReceivedIdle" /\ later.machine = "turn_execution" /\ later.transition = "BoundaryComplete" /\ earlier.step < later.step)
WitnessSchedulerTriggered_control_preempts_surface_PreemptWhenReady_control_plane_surface_boundary == <> SchedulerTriggered_PreemptWhenReady_control_plane_surface_boundary
WitnessStateObserved_control_preempts_surface_1 == <> (runtime_control_phase = "Idle")
WitnessTransitionObserved_control_preempts_surface_runtime_control_Initialize == <> (\E packet \in observed_transitions : /\ packet.machine = "runtime_control" /\ packet.transition = "Initialize")
WitnessTransitionObserved_control_preempts_surface_external_tool_surface_StageAdd == <> (\E packet \in observed_transitions : /\ packet.machine = "external_tool_surface" /\ packet.transition = "StageAdd")
WitnessTransitionOrder_control_preempts_surface_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "runtime_control" /\ earlier.transition = "Initialize" /\ later.machine = "external_tool_surface" /\ later.transition = "StageAdd" /\ earlier.step < later.step)

THEOREM Spec => []external_tool_delta_enters_runtime_control
THEOREM Spec => []boundary_application_reaches_surface_authority
THEOREM Spec => []control_preempts_surface_boundary
THEOREM Spec => []external_tool_surface_removing_or_removed_surfaces_are_not_visible
THEOREM Spec => []external_tool_surface_visible_membership_matches_active_base_state
THEOREM Spec => []external_tool_surface_removing_surfaces_have_no_pending_add_or_reload
THEOREM Spec => []external_tool_surface_removed_surfaces_only_allow_pending_none_or_add
THEOREM Spec => []external_tool_surface_inflight_calls_only_exist_for_active_or_removing_surfaces
THEOREM Spec => []external_tool_surface_reload_pending_requires_active_base_state
THEOREM Spec => []external_tool_surface_removed_surfaces_have_zero_inflight_calls
THEOREM Spec => []external_tool_surface_forced_delta_phase_is_always_a_remove_delta
THEOREM Spec => []runtime_control_running_implies_active_run
THEOREM Spec => []runtime_control_active_run_only_while_running_or_retired
THEOREM Spec => []turn_execution_ready_has_no_active_run
THEOREM Spec => []turn_execution_ready_has_no_admitted_content
THEOREM Spec => []turn_execution_non_ready_has_active_run
THEOREM Spec => []turn_execution_waiting_for_ops_implies_pending_tools
THEOREM Spec => []turn_execution_ready_has_no_boundary_cancel_request
THEOREM Spec => []turn_execution_immediate_primitives_skip_llm_and_recovery
THEOREM Spec => []turn_execution_terminal_states_match_terminal_outcome
THEOREM Spec => []turn_execution_completed_runs_have_seen_a_boundary

=============================================================================
