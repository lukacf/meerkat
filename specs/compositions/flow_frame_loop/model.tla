---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for flow_frame_loop.

CONSTANTS BooleanValues, BranchIdValues, CollectionPolicyKindValues, DependencyModeValues, FlowNodeIdValues, FlowNodeKindValues, FrameIdValues, LoopIdValues, LoopInstanceIdValues, MeerkatIdValues, NatValues, SetOfFlowNodeIdValues, StepIdValues, StepRunStatusValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

SeqOfFlowNodeIdValues == {<<>>} \cup {<<x>> : x \in FlowNodeIdValues} \cup {<<x, y>> : x \in FlowNodeIdValues, y \in FlowNodeIdValues}
SeqOfStepIdValues == {<<>>} \cup {<<x>> : x \in StepIdValues} \cup {<<x, y>> : x \in StepIdValues, y \in StepIdValues}
OptionBranchIdValues == {None} \cup {Some(x) : x \in BranchIdValues}
MapFlowNodeIdDependencyModeValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in FlowNodeIdValues, v \in DependencyModeValues }
MapFlowNodeIdFlowNodeKindValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in FlowNodeIdValues, v \in FlowNodeKindValues }
MapFlowNodeIdOptionBranchIdValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in FlowNodeIdValues, v \in OptionBranchIdValues }
MapFlowNodeIdSeqFlowNodeIdValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in FlowNodeIdValues, v \in SeqOfFlowNodeIdValues }
MapStepIdBoolValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StepIdValues, v \in BOOLEAN }
MapStepIdCollectionPolicyKindValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StepIdValues, v \in CollectionPolicyKindValues }
MapStepIdDependencyModeValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StepIdValues, v \in DependencyModeValues }
MapStepIdOptionBranchIdValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StepIdValues, v \in OptionBranchIdValues }
MapStepIdSeqStepIdValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StepIdValues, v \in SeqOfStepIdValues }
MapStepIdU32Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StepIdValues, v \in NatValues }

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
    <<"flow_run", "FlowRunMachine", "run_engine">>,
    <<"flow_frame", "FlowFrameMachine", "frame_engine">>,
    <<"loop_iteration", "LoopIterationMachine", "loop_engine">>
}

RouteNames == {
    "flow_frame_ready_frontier_updates_run_ready_frames",
    "flow_frame_node_release_updates_run_slots",
    "loop_request_body_frame_updates_run_pending_queue",
    "body_frame_completed_advances_loop_iteration",
    "body_frame_failed_fails_loop_iteration",
    "body_frame_canceled_cancels_loop_iteration",
    "loop_completed_completes_parent_loop_node",
    "loop_exhausted_fails_parent_loop_node",
    "loop_failed_fails_parent_loop_node",
    "loop_canceled_cancels_parent_loop_node"
}

Actors == {
    "run_engine",
    "frame_engine",
    "loop_engine"
}

ActorPriorities == {
}

SchedulerRules == {
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "flow_run" -> "run_engine"
      [] machine_id = "flow_frame" -> "frame_engine"
      [] machine_id = "loop_iteration" -> "loop_engine"
      [] OTHER -> "unknown_actor"

RouteSource(route_name) ==
    CASE route_name = "flow_frame_ready_frontier_updates_run_ready_frames" -> "flow_frame"
      [] route_name = "flow_frame_node_release_updates_run_slots" -> "flow_frame"
      [] route_name = "loop_request_body_frame_updates_run_pending_queue" -> "loop_iteration"
      [] route_name = "body_frame_completed_advances_loop_iteration" -> "flow_frame"
      [] route_name = "body_frame_failed_fails_loop_iteration" -> "flow_frame"
      [] route_name = "body_frame_canceled_cancels_loop_iteration" -> "flow_frame"
      [] route_name = "loop_completed_completes_parent_loop_node" -> "loop_iteration"
      [] route_name = "loop_exhausted_fails_parent_loop_node" -> "loop_iteration"
      [] route_name = "loop_failed_fails_parent_loop_node" -> "loop_iteration"
      [] route_name = "loop_canceled_cancels_parent_loop_node" -> "loop_iteration"
      [] OTHER -> "unknown_machine"

RouteEffect(route_name) ==
    CASE route_name = "flow_frame_ready_frontier_updates_run_ready_frames" -> "ReadyFrontierChanged"
      [] route_name = "flow_frame_node_release_updates_run_slots" -> "NodeExecutionReleased"
      [] route_name = "loop_request_body_frame_updates_run_pending_queue" -> "RequestBodyFrameStart"
      [] route_name = "body_frame_completed_advances_loop_iteration" -> "BodyFrameCompleted"
      [] route_name = "body_frame_failed_fails_loop_iteration" -> "BodyFrameFailed"
      [] route_name = "body_frame_canceled_cancels_loop_iteration" -> "BodyFrameCanceled"
      [] route_name = "loop_completed_completes_parent_loop_node" -> "LoopCompleted"
      [] route_name = "loop_exhausted_fails_parent_loop_node" -> "LoopExhausted"
      [] route_name = "loop_failed_fails_parent_loop_node" -> "LoopFailed"
      [] route_name = "loop_canceled_cancels_parent_loop_node" -> "LoopCanceled"
      [] OTHER -> "unknown_effect"

RouteTargetMachine(route_name) ==
    CASE route_name = "flow_frame_ready_frontier_updates_run_ready_frames" -> "flow_run"
      [] route_name = "flow_frame_node_release_updates_run_slots" -> "flow_run"
      [] route_name = "loop_request_body_frame_updates_run_pending_queue" -> "flow_run"
      [] route_name = "body_frame_completed_advances_loop_iteration" -> "loop_iteration"
      [] route_name = "body_frame_failed_fails_loop_iteration" -> "loop_iteration"
      [] route_name = "body_frame_canceled_cancels_loop_iteration" -> "loop_iteration"
      [] route_name = "loop_completed_completes_parent_loop_node" -> "flow_frame"
      [] route_name = "loop_exhausted_fails_parent_loop_node" -> "flow_frame"
      [] route_name = "loop_failed_fails_parent_loop_node" -> "flow_frame"
      [] route_name = "loop_canceled_cancels_parent_loop_node" -> "flow_frame"
      [] OTHER -> "unknown_machine"

RouteTargetInput(route_name) ==
    CASE route_name = "flow_frame_ready_frontier_updates_run_ready_frames" -> "RegisterReadyFrame"
      [] route_name = "flow_frame_node_release_updates_run_slots" -> "NodeExecutionReleased"
      [] route_name = "loop_request_body_frame_updates_run_pending_queue" -> "RegisterPendingBodyFrame"
      [] route_name = "body_frame_completed_advances_loop_iteration" -> "BodyFrameCompleted"
      [] route_name = "body_frame_failed_fails_loop_iteration" -> "BodyFrameFailed"
      [] route_name = "body_frame_canceled_cancels_loop_iteration" -> "BodyFrameCanceled"
      [] route_name = "loop_completed_completes_parent_loop_node" -> "CompleteNode"
      [] route_name = "loop_exhausted_fails_parent_loop_node" -> "FailNode"
      [] route_name = "loop_failed_fails_parent_loop_node" -> "FailNode"
      [] route_name = "loop_canceled_cancels_parent_loop_node" -> "CancelNode"
      [] OTHER -> "unknown_input"

RouteDeliveryKind(route_name) ==
    CASE route_name = "flow_frame_ready_frontier_updates_run_ready_frames" -> "Immediate"
      [] route_name = "flow_frame_node_release_updates_run_slots" -> "Immediate"
      [] route_name = "loop_request_body_frame_updates_run_pending_queue" -> "Immediate"
      [] route_name = "body_frame_completed_advances_loop_iteration" -> "Immediate"
      [] route_name = "body_frame_failed_fails_loop_iteration" -> "Immediate"
      [] route_name = "body_frame_canceled_cancels_loop_iteration" -> "Immediate"
      [] route_name = "loop_completed_completes_parent_loop_node" -> "Immediate"
      [] route_name = "loop_exhausted_fails_parent_loop_node" -> "Immediate"
      [] route_name = "loop_failed_fails_parent_loop_node" -> "Immediate"
      [] route_name = "loop_canceled_cancels_parent_loop_node" -> "Immediate"
      [] OTHER -> "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ flow_run_phase = "Absent"
    /\ flow_run_tracked_steps = {}
    /\ flow_run_ordered_steps = <<>>
    /\ flow_run_step_status = [x \in {} |-> None]
    /\ flow_run_output_recorded = [x \in {} |-> None]
    /\ flow_run_step_condition_results = [x \in {} |-> None]
    /\ flow_run_step_has_conditions = [x \in {} |-> None]
    /\ flow_run_step_dependencies = [x \in {} |-> None]
    /\ flow_run_step_dependency_modes = [x \in {} |-> None]
    /\ flow_run_step_branches = [x \in {} |-> None]
    /\ flow_run_step_collection_policies = [x \in {} |-> None]
    /\ flow_run_step_quorum_thresholds = [x \in {} |-> None]
    /\ flow_run_step_target_counts = [x \in {} |-> None]
    /\ flow_run_step_target_success_counts = [x \in {} |-> None]
    /\ flow_run_step_target_terminal_failure_counts = [x \in {} |-> None]
    /\ flow_run_target_retry_counts = [x \in {} |-> None]
    /\ flow_run_failure_count = 0
    /\ flow_run_consecutive_failure_count = 0
    /\ flow_run_escalation_threshold = 0
    /\ flow_run_max_step_retries = 0
    /\ flow_run_ready_frames = <<>>
    /\ flow_run_ready_frame_membership = {}
    /\ flow_run_pending_body_frame_loops = <<>>
    /\ flow_run_pending_body_frame_loop_membership = {}
    /\ flow_run_active_node_count = 0
    /\ flow_run_active_frame_count = 0
    /\ flow_run_max_active_nodes = 0
    /\ flow_run_max_active_frames = 0
    /\ flow_run_max_frame_depth = 0
    /\ flow_run_last_granted_frame = ""
    /\ flow_run_last_granted_loop = ""
    /\ flow_frame_phase = "Absent"
    /\ flow_frame_frame_id = ""
    /\ flow_frame_frame_scope = "Root"
    /\ flow_frame_loop_instance_id = ""
    /\ flow_frame_iteration = 0
    /\ flow_frame_last_admitted_node = ""
    /\ flow_frame_tracked_nodes = {}
    /\ flow_frame_ordered_nodes = <<>>
    /\ flow_frame_node_kind = [x \in {} |-> None]
    /\ flow_frame_node_dependencies = [x \in {} |-> None]
    /\ flow_frame_node_dependency_modes = [x \in {} |-> None]
    /\ flow_frame_node_branches = [x \in {} |-> None]
    /\ flow_frame_branch_winners = {}
    /\ flow_frame_node_status = [x \in {} |-> None]
    /\ flow_frame_ready_queue = <<>>
    /\ flow_frame_output_recorded = [x \in {} |-> None]
    /\ flow_frame_node_condition_results = [x \in {} |-> None]
    /\ loop_iteration_phase = "Absent"
    /\ loop_iteration_loop_instance_id = ""
    /\ loop_iteration_parent_frame_id = ""
    /\ loop_iteration_parent_node_id = ""
    /\ loop_iteration_loop_id = ""
    /\ loop_iteration_depth = 0
    /\ loop_iteration_stage = "AwaitingBodyFrame"
    /\ loop_iteration_current_iteration = 0
    /\ loop_iteration_last_completed_iteration = 0
    /\ loop_iteration_max_iterations = 0
    /\ loop_iteration_active_body_frame_id = None
    /\ obligation_flow_loop_until_evaluation = {}
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

WitnessInit_flow_frame_loop_route_coverage ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

flow_run__RemainingTargetCount(step_id) == ((IF (step_id \in DOMAIN flow_run_step_target_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_counts THEN flow_run_step_target_counts[step_id] ELSE 0) ELSE 0) - (IF (step_id \in DOMAIN flow_run_step_target_terminal_failure_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_terminal_failure_counts THEN flow_run_step_target_terminal_failure_counts[step_id] ELSE 0) ELSE 0))

flow_run__StepTargetTerminalFailureCount(step_id) == (IF (step_id \in DOMAIN flow_run_step_target_terminal_failure_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_terminal_failure_counts THEN flow_run_step_target_terminal_failure_counts[step_id] ELSE 0) ELSE 0)

flow_run__StepTargetSuccessCount(step_id) == (IF (step_id \in DOMAIN flow_run_step_target_success_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_success_counts THEN flow_run_step_target_success_counts[step_id] ELSE 0) ELSE 0)

flow_run__StepTargetCount(step_id) == (IF (step_id \in DOMAIN flow_run_step_target_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_counts THEN flow_run_step_target_counts[step_id] ELSE 0) ELSE 0)

flow_run__CollectionFeasible(step_id) == (IF ((IF (step_id \in DOMAIN flow_run_step_collection_policies) THEN (IF step_id \in DOMAIN flow_run_step_collection_policies THEN flow_run_step_collection_policies[step_id] ELSE "None") ELSE "All") = "All") THEN ((IF (step_id \in DOMAIN flow_run_step_target_terminal_failure_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_terminal_failure_counts THEN flow_run_step_target_terminal_failure_counts[step_id] ELSE 0) ELSE 0) = 0) ELSE (IF ((IF (step_id \in DOMAIN flow_run_step_collection_policies) THEN (IF step_id \in DOMAIN flow_run_step_collection_policies THEN flow_run_step_collection_policies[step_id] ELSE "None") ELSE "All") = "Any") THEN (((IF (step_id \in DOMAIN flow_run_step_target_success_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_success_counts THEN flow_run_step_target_success_counts[step_id] ELSE 0) ELSE 0) >= 1) \/ (((IF (step_id \in DOMAIN flow_run_step_target_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_counts THEN flow_run_step_target_counts[step_id] ELSE 0) ELSE 0) - (IF (step_id \in DOMAIN flow_run_step_target_terminal_failure_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_terminal_failure_counts THEN flow_run_step_target_terminal_failure_counts[step_id] ELSE 0) ELSE 0)) > 0)) ELSE (IF ((IF (step_id \in DOMAIN flow_run_step_collection_policies) THEN (IF step_id \in DOMAIN flow_run_step_collection_policies THEN flow_run_step_collection_policies[step_id] ELSE "None") ELSE "All") = "Quorum") THEN (((IF (step_id \in DOMAIN flow_run_step_target_success_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_success_counts THEN flow_run_step_target_success_counts[step_id] ELSE 0) ELSE 0) + ((IF (step_id \in DOMAIN flow_run_step_target_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_counts THEN flow_run_step_target_counts[step_id] ELSE 0) ELSE 0) - (IF (step_id \in DOMAIN flow_run_step_target_terminal_failure_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_terminal_failure_counts THEN flow_run_step_target_terminal_failure_counts[step_id] ELSE 0) ELSE 0))) >= (IF (step_id \in DOMAIN flow_run_step_quorum_thresholds) THEN (IF step_id \in DOMAIN flow_run_step_quorum_thresholds THEN flow_run_step_quorum_thresholds[step_id] ELSE 0) ELSE 0)) ELSE FALSE)))

flow_run__CollectionSatisfied(step_id) == (IF ((IF (step_id \in DOMAIN flow_run_step_collection_policies) THEN (IF step_id \in DOMAIN flow_run_step_collection_policies THEN flow_run_step_collection_policies[step_id] ELSE "None") ELSE "All") = "All") THEN ((IF (step_id \in DOMAIN flow_run_step_target_success_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_success_counts THEN flow_run_step_target_success_counts[step_id] ELSE 0) ELSE 0) = (IF (step_id \in DOMAIN flow_run_step_target_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_counts THEN flow_run_step_target_counts[step_id] ELSE 0) ELSE 0)) ELSE (IF ((IF (step_id \in DOMAIN flow_run_step_collection_policies) THEN (IF step_id \in DOMAIN flow_run_step_collection_policies THEN flow_run_step_collection_policies[step_id] ELSE "None") ELSE "All") = "Any") THEN ((IF (step_id \in DOMAIN flow_run_step_target_success_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_success_counts THEN flow_run_step_target_success_counts[step_id] ELSE 0) ELSE 0) >= 1) ELSE (IF ((IF (step_id \in DOMAIN flow_run_step_collection_policies) THEN (IF step_id \in DOMAIN flow_run_step_collection_policies THEN flow_run_step_collection_policies[step_id] ELSE "None") ELSE "All") = "Quorum") THEN ((IF (step_id \in DOMAIN flow_run_step_target_success_counts) THEN (IF step_id \in DOMAIN flow_run_step_target_success_counts THEN flow_run_step_target_success_counts[step_id] ELSE 0) ELSE 0) >= (IF (step_id \in DOMAIN flow_run_step_quorum_thresholds) THEN (IF step_id \in DOMAIN flow_run_step_quorum_thresholds THEN flow_run_step_quorum_thresholds[step_id] ELSE 0) ELSE 0)) ELSE FALSE)))

flow_run__TargetRetryAllowed(retry_key) == ((IF (retry_key \in DOMAIN flow_run_target_retry_counts) THEN (IF retry_key \in DOMAIN flow_run_target_retry_counts THEN flow_run_target_retry_counts[retry_key] ELSE 0) ELSE 0) <= flow_run_max_step_retries)

flow_run__TargetRetryCount(retry_key) == (IF (retry_key \in DOMAIN flow_run_target_retry_counts) THEN (IF retry_key \in DOMAIN flow_run_target_retry_counts THEN flow_run_target_retry_counts[retry_key] ELSE 0) ELSE 0)

flow_run__EscalationWillTrigger == ((flow_run_escalation_threshold > 0) /\ ((flow_run_consecutive_failure_count + 1) >= flow_run_escalation_threshold))

flow_run__StepHasDependencies(step_id) == (Len((IF (step_id \in DOMAIN flow_run_step_dependencies) THEN (IF step_id \in DOMAIN flow_run_step_dependencies THEN flow_run_step_dependencies[step_id] ELSE <<>>) ELSE <<>>)) > 0)

flow_run__AnyTrackedStepInStatus(status) == (\E step_id \in flow_run_tracked_steps : ((IF step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[step_id] ELSE None) = Some(status)))

flow_run__NoTrackedStepInStatus(status) == (\A step_id \in flow_run_tracked_steps : ((IF step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[step_id] ELSE None) # Some(status)))

flow_run__AllTrackedStepsInAllowedStatuses(allowed_statuses) == (\A step_id \in flow_run_tracked_steps : ((IF step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[step_id] ELSE None) \in SeqElements(allowed_statuses)))

flow_run__StepConditionRecordedIs(step_id, expected) == ((IF step_id \in DOMAIN flow_run_step_condition_results THEN flow_run_step_condition_results[step_id] ELSE None) = expected)

flow_run__StepOutputRecordedIs(step_id, expected) == ((IF step_id \in DOMAIN flow_run_output_recorded THEN flow_run_output_recorded[step_id] ELSE FALSE) = expected)

flow_run__StepStatusIs(step_id, expected_status) == ((IF step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[step_id] ELSE None) = Some(expected_status))

flow_run__StepIsTracked(step_id) == (step_id \in flow_run_tracked_steps)

flow_run__RunIsTerminal == ((flow_run_phase = "Completed") \/ (flow_run_phase = "Failed") \/ (flow_run_phase = "Canceled"))

flow_run__StepBranchBlocked(step_id) == (IF ((IF (step_id \in DOMAIN flow_run_step_branches) THEN (IF step_id \in DOMAIN flow_run_step_branches THEN flow_run_step_branches[step_id] ELSE None) ELSE None) = None) THEN FALSE ELSE (\E candidate \in flow_run_tracked_steps : ((candidate # step_id) /\ ((IF (candidate \in DOMAIN flow_run_step_branches) THEN (IF candidate \in DOMAIN flow_run_step_branches THEN flow_run_step_branches[candidate] ELSE None) ELSE None) = (IF (step_id \in DOMAIN flow_run_step_branches) THEN (IF step_id \in DOMAIN flow_run_step_branches THEN flow_run_step_branches[step_id] ELSE None) ELSE None)) /\ flow_run__StepStatusIs(candidate, "Completed"))))

flow_run__AnyDependencyCompleted(step_id) == (\E dependency \in SeqElements((IF (step_id \in DOMAIN flow_run_step_dependencies) THEN (IF step_id \in DOMAIN flow_run_step_dependencies THEN flow_run_step_dependencies[step_id] ELSE <<>>) ELSE <<>>)) : flow_run__StepStatusIs(dependency, "Completed"))

flow_run__AllDependenciesSkipped(step_id) == (\A dependency \in SeqElements((IF (step_id \in DOMAIN flow_run_step_dependencies) THEN (IF step_id \in DOMAIN flow_run_step_dependencies THEN flow_run_step_dependencies[step_id] ELSE <<>>) ELSE <<>>)) : flow_run__StepStatusIs(dependency, "Skipped"))

flow_run__AllDependenciesCompleted(step_id) == (\A dependency \in SeqElements((IF (step_id \in DOMAIN flow_run_step_dependencies) THEN (IF step_id \in DOMAIN flow_run_step_dependencies THEN flow_run_step_dependencies[step_id] ELSE <<>>) ELSE <<>>)) : flow_run__StepStatusIs(dependency, "Completed"))

flow_run__StepConditionAllowsDispatch(step_id) == (((IF step_id \in DOMAIN flow_run_step_has_conditions THEN flow_run_step_has_conditions[step_id] ELSE FALSE) = FALSE) \/ flow_run__StepConditionRecordedIs(step_id, Some(TRUE)))

flow_run__StepDependencyShouldSkip(step_id) == (((IF (step_id \in DOMAIN flow_run_step_dependency_modes) THEN (IF step_id \in DOMAIN flow_run_step_dependency_modes THEN flow_run_step_dependency_modes[step_id] ELSE "None") ELSE "All") = "Any") /\ flow_run__StepHasDependencies(step_id) /\ flow_run__AllDependenciesSkipped(step_id))

flow_run__StepDependencyReady(step_id) == (IF ((IF (step_id \in DOMAIN flow_run_step_dependency_modes) THEN (IF step_id \in DOMAIN flow_run_step_dependency_modes THEN flow_run_step_dependency_modes[step_id] ELSE "None") ELSE "All") = "Any") THEN flow_run__AnyDependencyCompleted(step_id) ELSE (IF ~(flow_run__StepHasDependencies(step_id)) THEN TRUE ELSE flow_run__AllDependenciesCompleted(step_id)))

RECURSIVE flow_run_CreateRun_ForEach0_output_recorded(_, _)
flow_run_CreateRun_ForEach0_output_recorded(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, FALSE) IN flow_run_CreateRun_ForEach0_output_recorded(next_acc, Tail(items))

RECURSIVE flow_run_CreateRun_ForEach0_step_condition_results(_, _)
flow_run_CreateRun_ForEach0_step_condition_results(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, None) IN flow_run_CreateRun_ForEach0_step_condition_results(next_acc, Tail(items))

RECURSIVE flow_run_CreateRun_ForEach0_step_status(_, _)
flow_run_CreateRun_ForEach0_step_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, None) IN flow_run_CreateRun_ForEach0_step_status(next_acc, Tail(items))

RECURSIVE flow_run_CreateRun_ForEach0_step_target_counts(_, _)
flow_run_CreateRun_ForEach0_step_target_counts(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, 0) IN flow_run_CreateRun_ForEach0_step_target_counts(next_acc, Tail(items))

RECURSIVE flow_run_CreateRun_ForEach0_step_target_success_counts(_, _)
flow_run_CreateRun_ForEach0_step_target_success_counts(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, 0) IN flow_run_CreateRun_ForEach0_step_target_success_counts(next_acc, Tail(items))

RECURSIVE flow_run_CreateRun_ForEach0_step_target_terminal_failure_counts(_, _)
flow_run_CreateRun_ForEach0_step_target_terminal_failure_counts(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, 0) IN flow_run_CreateRun_ForEach0_step_target_terminal_failure_counts(next_acc, Tail(items))

RECURSIVE flow_run_CreateRun_ForEach0_tracked_steps(_, _)
flow_run_CreateRun_ForEach0_tracked_steps(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == (acc \cup {step_id}) IN flow_run_CreateRun_ForEach0_tracked_steps(next_acc, Tail(items))

flow_run_CreateRun(arg_step_ids, arg_ordered_steps, arg_step_has_conditions, arg_step_dependencies, arg_step_dependency_modes, arg_step_branches, arg_step_collection_policies, arg_step_quorum_thresholds, arg_escalation_threshold, arg_max_step_retries, arg_max_active_nodes, arg_max_active_frames, arg_max_frame_depth) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "CreateRun"
       /\ packet.payload.step_ids = arg_step_ids
       /\ packet.payload.ordered_steps = arg_ordered_steps
       /\ packet.payload.step_has_conditions = arg_step_has_conditions
       /\ packet.payload.step_dependencies = arg_step_dependencies
       /\ packet.payload.step_dependency_modes = arg_step_dependency_modes
       /\ packet.payload.step_branches = arg_step_branches
       /\ packet.payload.step_collection_policies = arg_step_collection_policies
       /\ packet.payload.step_quorum_thresholds = arg_step_quorum_thresholds
       /\ packet.payload.escalation_threshold = arg_escalation_threshold
       /\ packet.payload.max_step_retries = arg_max_step_retries
       /\ packet.payload.max_active_nodes = arg_max_active_nodes
       /\ packet.payload.max_active_frames = arg_max_active_frames
       /\ packet.payload.max_frame_depth = arg_max_frame_depth
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Absent"
       /\ (Len(packet.payload.step_ids) > 0)
       /\ (\A value \in SeqElements(packet.payload.ordered_steps) : (value \in SeqElements(packet.payload.step_ids)))
       /\ (\A value \in SeqElements(packet.payload.step_ids) : (value \in SeqElements(packet.payload.ordered_steps)))
       /\ ((\A step_key \in DOMAIN packet.payload.step_has_conditions : (step_key \in SeqElements(packet.payload.step_ids))) /\ (\A step_id \in SeqElements(packet.payload.step_ids) : (step_id \in DOMAIN packet.payload.step_has_conditions)))
       /\ ((\A step_key \in DOMAIN packet.payload.step_dependencies : (step_key \in SeqElements(packet.payload.step_ids))) /\ (\A step_id \in SeqElements(packet.payload.step_ids) : (step_id \in DOMAIN packet.payload.step_dependencies)))
       /\ ((\A step_key \in DOMAIN packet.payload.step_dependency_modes : (step_key \in SeqElements(packet.payload.step_ids))) /\ (\A step_id \in SeqElements(packet.payload.step_ids) : (step_id \in DOMAIN packet.payload.step_dependency_modes)))
       /\ ((\A step_key \in DOMAIN packet.payload.step_branches : (step_key \in SeqElements(packet.payload.step_ids))) /\ (\A step_id \in SeqElements(packet.payload.step_ids) : (step_id \in DOMAIN packet.payload.step_branches)))
       /\ ((\A step_key \in DOMAIN packet.payload.step_collection_policies : (step_key \in SeqElements(packet.payload.step_ids))) /\ (\A step_id \in SeqElements(packet.payload.step_ids) : (step_id \in DOMAIN packet.payload.step_collection_policies)))
       /\ ((\A step_key \in DOMAIN packet.payload.step_quorum_thresholds : (step_key \in SeqElements(packet.payload.step_ids))) /\ (\A step_id \in SeqElements(packet.payload.step_ids) : (step_id \in DOMAIN packet.payload.step_quorum_thresholds)))
       /\ (\A step_id \in DOMAIN packet.payload.step_dependencies : (\A dependency \in SeqElements((IF step_id \in DOMAIN packet.payload.step_dependencies THEN packet.payload.step_dependencies[step_id] ELSE <<>>)) : (dependency \in SeqElements(packet.payload.step_ids))))
       /\ flow_run_phase' = "Pending"
       /\ flow_run_tracked_steps' = flow_run_CreateRun_ForEach0_tracked_steps({}, packet.payload.step_ids)
       /\ flow_run_ordered_steps' = packet.payload.ordered_steps
       /\ flow_run_step_status' = flow_run_CreateRun_ForEach0_step_status([x \in {} |-> None], packet.payload.step_ids)
       /\ flow_run_output_recorded' = flow_run_CreateRun_ForEach0_output_recorded([x \in {} |-> None], packet.payload.step_ids)
       /\ flow_run_step_condition_results' = flow_run_CreateRun_ForEach0_step_condition_results([x \in {} |-> None], packet.payload.step_ids)
       /\ flow_run_step_has_conditions' = packet.payload.step_has_conditions
       /\ flow_run_step_dependencies' = packet.payload.step_dependencies
       /\ flow_run_step_dependency_modes' = packet.payload.step_dependency_modes
       /\ flow_run_step_branches' = packet.payload.step_branches
       /\ flow_run_step_collection_policies' = packet.payload.step_collection_policies
       /\ flow_run_step_quorum_thresholds' = packet.payload.step_quorum_thresholds
       /\ flow_run_step_target_counts' = flow_run_CreateRun_ForEach0_step_target_counts([x \in {} |-> None], packet.payload.step_ids)
       /\ flow_run_step_target_success_counts' = flow_run_CreateRun_ForEach0_step_target_success_counts([x \in {} |-> None], packet.payload.step_ids)
       /\ flow_run_step_target_terminal_failure_counts' = flow_run_CreateRun_ForEach0_step_target_terminal_failure_counts([x \in {} |-> None], packet.payload.step_ids)
       /\ flow_run_target_retry_counts' = [x \in {} |-> None]
       /\ flow_run_failure_count' = 0
       /\ flow_run_consecutive_failure_count' = 0
       /\ flow_run_escalation_threshold' = packet.payload.escalation_threshold
       /\ flow_run_max_step_retries' = packet.payload.max_step_retries
       /\ flow_run_ready_frames' = <<>>
       /\ flow_run_ready_frame_membership' = {}
       /\ flow_run_pending_body_frame_loops' = <<>>
       /\ flow_run_pending_body_frame_loop_membership' = {}
       /\ flow_run_active_node_count' = 0
       /\ flow_run_active_frame_count' = 0
       /\ flow_run_max_active_nodes' = packet.payload.max_active_nodes
       /\ flow_run_max_active_frames' = packet.payload.max_active_frames
       /\ flow_run_max_frame_depth' = packet.payload.max_frame_depth
       /\ flow_run_last_granted_frame' = ""
       /\ flow_run_last_granted_loop' = ""
       /\ UNCHANGED << flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EmitFlowRunNotice", payload |-> [run_status |-> "Pending"], effect_id |-> (model_step_count + 1), source_transition |-> "CreateRun"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "CreateRun", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Pending"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_StartRun ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "StartRun"
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Pending"
       /\ flow_run_phase' = "Running"
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EmitFlowRunNotice", payload |-> [run_status |-> "Running"], effect_id |-> (model_step_count + 1), source_transition |-> "StartRun"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "StartRun", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_DispatchStep(arg_step_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "DispatchStep"
       /\ packet.payload.step_id = arg_step_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ ((IF packet.payload.step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[packet.payload.step_id] ELSE None) = None)
       /\ flow_run__StepConditionAllowsDispatch(packet.payload.step_id)
       /\ flow_run__StepDependencyReady(packet.payload.step_id)
       /\ ~(flow_run__StepBranchBlocked(packet.payload.step_id))
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Dispatched"))
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EmitStepNotice", payload |-> [step_id |-> packet.payload.step_id, step_status |-> "Dispatched"], effect_id |-> (model_step_count + 1), source_transition |-> "DispatchStep"], [machine |-> "flow_run", variant |-> "AdmitStepWork", payload |-> [step_id |-> packet.payload.step_id], effect_id |-> (model_step_count + 1), source_transition |-> "DispatchStep"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "DispatchStep", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_CompleteStep(arg_step_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "CompleteStep"
       /\ packet.payload.step_id = arg_step_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched")
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Completed"))
       /\ flow_run_consecutive_failure_count' = 0
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EmitStepNotice", payload |-> [step_id |-> packet.payload.step_id, step_status |-> "Completed"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteStep"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "CompleteStep", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_RecordStepOutput(arg_step_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "RecordStepOutput"
       /\ packet.payload.step_id = arg_step_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ flow_run__StepStatusIs(packet.payload.step_id, "Completed")
       /\ flow_run__StepOutputRecordedIs(packet.payload.step_id, FALSE)
       /\ flow_run_phase' = "Running"
       /\ flow_run_output_recorded' = MapSet(flow_run_output_recorded, packet.payload.step_id, TRUE)
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "PersistStepOutput", payload |-> [step_id |-> packet.payload.step_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecordStepOutput"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "RecordStepOutput", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_ConditionPassed(arg_step_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "ConditionPassed"
       /\ packet.payload.step_id = arg_step_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ ((IF packet.payload.step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[packet.payload.step_id] ELSE None) = None)
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_condition_results' = MapSet(flow_run_step_condition_results, packet.payload.step_id, Some(TRUE))
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "ConditionPassed", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_ConditionRejected(arg_step_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "ConditionRejected"
       /\ packet.payload.step_id = arg_step_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ ((IF packet.payload.step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[packet.payload.step_id] ELSE None) = None)
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Skipped"))
       /\ flow_run_step_condition_results' = MapSet(flow_run_step_condition_results, packet.payload.step_id, Some(FALSE))
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_output_recorded, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EmitStepNotice", payload |-> [step_id |-> packet.payload.step_id, step_status |-> "Skipped"], effect_id |-> (model_step_count + 1), source_transition |-> "ConditionRejected"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "ConditionRejected", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_FailStepEscalating(arg_step_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "FailStep"
       /\ packet.payload.step_id = arg_step_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched")
       /\ flow_run__EscalationWillTrigger
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Failed"))
       /\ flow_run_failure_count' = (flow_run_failure_count) + 1
       /\ flow_run_consecutive_failure_count' = (flow_run_consecutive_failure_count) + 1
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EmitStepNotice", payload |-> [step_id |-> packet.payload.step_id, step_status |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "FailStepEscalating"], [machine |-> "flow_run", variant |-> "AppendFailureLedger", payload |-> [step_id |-> packet.payload.step_id], effect_id |-> (model_step_count + 1), source_transition |-> "FailStepEscalating"], [machine |-> "flow_run", variant |-> "EscalateSupervisor", payload |-> [step_id |-> packet.payload.step_id], effect_id |-> (model_step_count + 1), source_transition |-> "FailStepEscalating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "FailStepEscalating", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_FailStep(arg_step_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "FailStep"
       /\ packet.payload.step_id = arg_step_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched")
       /\ ~(flow_run__EscalationWillTrigger)
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Failed"))
       /\ flow_run_failure_count' = (flow_run_failure_count) + 1
       /\ flow_run_consecutive_failure_count' = (flow_run_consecutive_failure_count) + 1
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EmitStepNotice", payload |-> [step_id |-> packet.payload.step_id, step_status |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "FailStep"], [machine |-> "flow_run", variant |-> "AppendFailureLedger", payload |-> [step_id |-> packet.payload.step_id], effect_id |-> (model_step_count + 1), source_transition |-> "FailStep"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "FailStep", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_SkipStep(arg_step_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "SkipStep"
       /\ packet.payload.step_id = arg_step_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ ((IF packet.payload.step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[packet.payload.step_id] ELSE None) = None)
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Skipped"))
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EmitStepNotice", payload |-> [step_id |-> packet.payload.step_id, step_status |-> "Skipped"], effect_id |-> (model_step_count + 1), source_transition |-> "SkipStep"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "SkipStep", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_ProjectFrameStepCompleted(arg_step_id, arg_step_status, arg_append_failure_ledger) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "ProjectFrameStepStatus"
       /\ packet.payload.step_id = arg_step_id
       /\ packet.payload.step_status = arg_step_status
       /\ packet.payload.append_failure_ledger = arg_append_failure_ledger
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ (((IF packet.payload.step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[packet.payload.step_id] ELSE None) = None) \/ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched"))
       /\ (packet.payload.step_status = "Completed")
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Completed"))
       /\ flow_run_output_recorded' = MapSet(flow_run_output_recorded, packet.payload.step_id, TRUE)
       /\ flow_run_consecutive_failure_count' = 0
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "ProjectFrameStepCompleted", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_ProjectFrameStepSkipped(arg_step_id, arg_step_status, arg_append_failure_ledger) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "ProjectFrameStepStatus"
       /\ packet.payload.step_id = arg_step_id
       /\ packet.payload.step_status = arg_step_status
       /\ packet.payload.append_failure_ledger = arg_append_failure_ledger
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ (((IF packet.payload.step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[packet.payload.step_id] ELSE None) = None) \/ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched"))
       /\ (packet.payload.step_status = "Skipped")
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Skipped"))
       /\ flow_run_output_recorded' = MapSet(flow_run_output_recorded, packet.payload.step_id, FALSE)
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "ProjectFrameStepSkipped", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_ProjectFrameStepFailedEscalatingWithLedger(arg_step_id, arg_step_status, arg_append_failure_ledger) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "ProjectFrameStepStatus"
       /\ packet.payload.step_id = arg_step_id
       /\ packet.payload.step_status = arg_step_status
       /\ packet.payload.append_failure_ledger = arg_append_failure_ledger
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ (((IF packet.payload.step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[packet.payload.step_id] ELSE None) = None) \/ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched"))
       /\ (packet.payload.step_status = "Failed")
       /\ (packet.payload.append_failure_ledger = TRUE)
       /\ flow_run__EscalationWillTrigger
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Failed"))
       /\ flow_run_output_recorded' = MapSet(flow_run_output_recorded, packet.payload.step_id, FALSE)
       /\ flow_run_failure_count' = (flow_run_failure_count) + 1
       /\ flow_run_consecutive_failure_count' = (flow_run_consecutive_failure_count) + 1
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "AppendFailureLedger", payload |-> [step_id |-> packet.payload.step_id], effect_id |-> (model_step_count + 1), source_transition |-> "ProjectFrameStepFailedEscalatingWithLedger"], [machine |-> "flow_run", variant |-> "EscalateSupervisor", payload |-> [step_id |-> packet.payload.step_id], effect_id |-> (model_step_count + 1), source_transition |-> "ProjectFrameStepFailedEscalatingWithLedger"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "ProjectFrameStepFailedEscalatingWithLedger", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_ProjectFrameStepFailedEscalatingWithoutLedger(arg_step_id, arg_step_status, arg_append_failure_ledger) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "ProjectFrameStepStatus"
       /\ packet.payload.step_id = arg_step_id
       /\ packet.payload.step_status = arg_step_status
       /\ packet.payload.append_failure_ledger = arg_append_failure_ledger
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ (((IF packet.payload.step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[packet.payload.step_id] ELSE None) = None) \/ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched"))
       /\ (packet.payload.step_status = "Failed")
       /\ (packet.payload.append_failure_ledger = FALSE)
       /\ flow_run__EscalationWillTrigger
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Failed"))
       /\ flow_run_output_recorded' = MapSet(flow_run_output_recorded, packet.payload.step_id, FALSE)
       /\ flow_run_failure_count' = (flow_run_failure_count) + 1
       /\ flow_run_consecutive_failure_count' = (flow_run_consecutive_failure_count) + 1
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EscalateSupervisor", payload |-> [step_id |-> packet.payload.step_id], effect_id |-> (model_step_count + 1), source_transition |-> "ProjectFrameStepFailedEscalatingWithoutLedger"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "ProjectFrameStepFailedEscalatingWithoutLedger", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_ProjectFrameStepFailedWithLedger(arg_step_id, arg_step_status, arg_append_failure_ledger) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "ProjectFrameStepStatus"
       /\ packet.payload.step_id = arg_step_id
       /\ packet.payload.step_status = arg_step_status
       /\ packet.payload.append_failure_ledger = arg_append_failure_ledger
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ (((IF packet.payload.step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[packet.payload.step_id] ELSE None) = None) \/ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched"))
       /\ (packet.payload.step_status = "Failed")
       /\ (packet.payload.append_failure_ledger = TRUE)
       /\ ~(flow_run__EscalationWillTrigger)
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Failed"))
       /\ flow_run_output_recorded' = MapSet(flow_run_output_recorded, packet.payload.step_id, FALSE)
       /\ flow_run_failure_count' = (flow_run_failure_count) + 1
       /\ flow_run_consecutive_failure_count' = (flow_run_consecutive_failure_count) + 1
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "AppendFailureLedger", payload |-> [step_id |-> packet.payload.step_id], effect_id |-> (model_step_count + 1), source_transition |-> "ProjectFrameStepFailedWithLedger"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "ProjectFrameStepFailedWithLedger", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_ProjectFrameStepFailedWithoutLedger(arg_step_id, arg_step_status, arg_append_failure_ledger) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "ProjectFrameStepStatus"
       /\ packet.payload.step_id = arg_step_id
       /\ packet.payload.step_status = arg_step_status
       /\ packet.payload.append_failure_ledger = arg_append_failure_ledger
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ (((IF packet.payload.step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[packet.payload.step_id] ELSE None) = None) \/ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched"))
       /\ (packet.payload.step_status = "Failed")
       /\ (packet.payload.append_failure_ledger = FALSE)
       /\ ~(flow_run__EscalationWillTrigger)
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Failed"))
       /\ flow_run_output_recorded' = MapSet(flow_run_output_recorded, packet.payload.step_id, FALSE)
       /\ flow_run_failure_count' = (flow_run_failure_count) + 1
       /\ flow_run_consecutive_failure_count' = (flow_run_consecutive_failure_count) + 1
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "ProjectFrameStepFailedWithoutLedger", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_CancelStep(arg_step_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "CancelStep"
       /\ packet.payload.step_id = arg_step_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ (((IF packet.payload.step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[packet.payload.step_id] ELSE None) = None) \/ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched"))
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_status' = MapSet(flow_run_step_status, packet.payload.step_id, Some("Canceled"))
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EmitStepNotice", payload |-> [step_id |-> packet.payload.step_id, step_status |-> "Canceled"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelStep"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "CancelStep", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_RegisterTargets(arg_step_id, arg_target_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "RegisterTargets"
       /\ packet.payload.step_id = arg_step_id
       /\ packet.payload.target_count = arg_target_count
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ ((IF packet.payload.step_id \in DOMAIN flow_run_step_status THEN flow_run_step_status[packet.payload.step_id] ELSE None) = None)
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_target_counts' = MapSet(flow_run_step_target_counts, packet.payload.step_id, packet.payload.target_count)
       /\ flow_run_step_target_success_counts' = MapSet(flow_run_step_target_success_counts, packet.payload.step_id, 0)
       /\ flow_run_step_target_terminal_failure_counts' = MapSet(flow_run_step_target_terminal_failure_counts, packet.payload.step_id, 0)
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "RegisterTargets", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_RecordTargetSuccess(arg_step_id, arg_target_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "RecordTargetSuccess"
       /\ packet.payload.step_id = arg_step_id
       /\ packet.payload.target_id = arg_target_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched")
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_target_success_counts' = MapSet(flow_run_step_target_success_counts, packet.payload.step_id, ((IF (packet.payload.step_id \in DOMAIN flow_run_step_target_success_counts) THEN (IF packet.payload.step_id \in DOMAIN flow_run_step_target_success_counts THEN flow_run_step_target_success_counts[packet.payload.step_id] ELSE 0) ELSE 0) + 1))
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "ProjectTargetSuccess", payload |-> [step_id |-> packet.payload.step_id, target_id |-> packet.payload.target_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecordTargetSuccess"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "RecordTargetSuccess", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_RecordTargetTerminalFailure(arg_step_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "RecordTargetTerminalFailure"
       /\ packet.payload.step_id = arg_step_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched")
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_target_terminal_failure_counts' = MapSet(flow_run_step_target_terminal_failure_counts, packet.payload.step_id, ((IF (packet.payload.step_id \in DOMAIN flow_run_step_target_terminal_failure_counts) THEN (IF packet.payload.step_id \in DOMAIN flow_run_step_target_terminal_failure_counts THEN flow_run_step_target_terminal_failure_counts[packet.payload.step_id] ELSE 0) ELSE 0) + 1))
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "RecordTargetTerminalFailure", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_RecordTargetCanceled(arg_step_id, arg_target_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "RecordTargetCanceled"
       /\ packet.payload.step_id = arg_step_id
       /\ packet.payload.target_id = arg_target_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched")
       /\ flow_run_phase' = "Running"
       /\ flow_run_step_target_terminal_failure_counts' = MapSet(flow_run_step_target_terminal_failure_counts, packet.payload.step_id, ((IF (packet.payload.step_id \in DOMAIN flow_run_step_target_terminal_failure_counts) THEN (IF packet.payload.step_id \in DOMAIN flow_run_step_target_terminal_failure_counts THEN flow_run_step_target_terminal_failure_counts[packet.payload.step_id] ELSE 0) ELSE 0) + 1))
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "ProjectTargetCanceled", payload |-> [step_id |-> packet.payload.step_id, target_id |-> packet.payload.target_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecordTargetCanceled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "RecordTargetCanceled", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_RecordTargetFailure(arg_step_id, arg_target_id, arg_retry_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "RecordTargetFailure"
       /\ packet.payload.step_id = arg_step_id
       /\ packet.payload.target_id = arg_target_id
       /\ packet.payload.retry_key = arg_retry_key
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__StepIsTracked(packet.payload.step_id)
       /\ flow_run__StepStatusIs(packet.payload.step_id, "Dispatched")
       /\ flow_run_phase' = "Running"
       /\ flow_run_target_retry_counts' = MapSet(flow_run_target_retry_counts, packet.payload.retry_key, ((IF (packet.payload.retry_key \in DOMAIN flow_run_target_retry_counts) THEN (IF packet.payload.retry_key \in DOMAIN flow_run_target_retry_counts THEN flow_run_target_retry_counts[packet.payload.retry_key] ELSE 0) ELSE 0) + 1))
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "ProjectTargetFailure", payload |-> [step_id |-> packet.payload.step_id, target_id |-> packet.payload.target_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecordTargetFailure"], [machine |-> "flow_run", variant |-> "AppendFailureLedger", payload |-> [step_id |-> packet.payload.step_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecordTargetFailure"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "RecordTargetFailure", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_RegisterReadyFrame(arg_frame_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "RegisterReadyFrame"
       /\ packet.payload.frame_id = arg_frame_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ ~((packet.payload.frame_id \in flow_run_ready_frame_membership))
       /\ flow_run_phase' = "Running"
       /\ flow_run_ready_frames' = Append(flow_run_ready_frames, packet.payload.frame_id)
       /\ flow_run_ready_frame_membership' = (flow_run_ready_frame_membership \cup {packet.payload.frame_id})
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "RegisterReadyFrame", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_PumpNodeScheduler ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "PumpNodeScheduler"
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ ((Len(flow_run_ready_frames) > 0) /\ ((flow_run_max_active_nodes = 0) \/ (flow_run_active_node_count < flow_run_max_active_nodes)))
       /\ flow_run_phase' = "Running"
       /\ flow_run_ready_frames' = Tail(flow_run_ready_frames)
       /\ flow_run_ready_frame_membership' = (flow_run_ready_frame_membership \ {Head(flow_run_ready_frames)})
       /\ flow_run_active_node_count' = (flow_run_active_node_count) + 1
       /\ flow_run_last_granted_frame' = Head(flow_run_ready_frames)
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "GrantNodeSlot", payload |-> [frame_id |-> Head(flow_run_ready_frames)], effect_id |-> (model_step_count + 1), source_transition |-> "PumpNodeScheduler"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "PumpNodeScheduler", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_RegisterPendingBodyFrame(arg_loop_instance_id, arg_depth) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "RegisterPendingBodyFrame"
       /\ packet.payload.loop_instance_id = arg_loop_instance_id
       /\ packet.payload.depth = arg_depth
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ ((flow_run_max_frame_depth = 0) \/ (packet.payload.depth <= flow_run_max_frame_depth))
       /\ ~((packet.payload.loop_instance_id \in flow_run_pending_body_frame_loop_membership))
       /\ flow_run_phase' = "Running"
       /\ flow_run_pending_body_frame_loops' = Append(flow_run_pending_body_frame_loops, packet.payload.loop_instance_id)
       /\ flow_run_pending_body_frame_loop_membership' = (flow_run_pending_body_frame_loop_membership \cup {packet.payload.loop_instance_id})
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "RegisterPendingBodyFrame", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_PumpFrameScheduler ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "PumpFrameScheduler"
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ ((Len(flow_run_pending_body_frame_loops) > 0) /\ ((flow_run_max_active_frames = 0) \/ (flow_run_active_frame_count < flow_run_max_active_frames)))
       /\ flow_run_phase' = "Running"
       /\ flow_run_pending_body_frame_loops' = Tail(flow_run_pending_body_frame_loops)
       /\ flow_run_pending_body_frame_loop_membership' = (flow_run_pending_body_frame_loop_membership \ {Head(flow_run_pending_body_frame_loops)})
       /\ flow_run_active_frame_count' = (flow_run_active_frame_count) + 1
       /\ flow_run_last_granted_loop' = Head(flow_run_pending_body_frame_loops)
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_active_node_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "GrantBodyFrameStart", payload |-> [loop_instance_id |-> Head(flow_run_pending_body_frame_loops)], effect_id |-> (model_step_count + 1), source_transition |-> "PumpFrameScheduler"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "PumpFrameScheduler", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_NodeExecutionReleased(arg_frame_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "NodeExecutionReleased"
       /\ packet.payload.frame_id = arg_frame_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ (flow_run_active_node_count > 0)
       /\ flow_run_phase' = "Running"
       /\ flow_run_active_node_count' = (flow_run_active_node_count) - 1
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "NodeExecutionReleased", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_FrameTerminated(arg_frame_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "FrameTerminated"
       /\ packet.payload.frame_id = arg_frame_id
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ (flow_run_active_frame_count > 0)
       /\ flow_run_phase' = "Running"
       /\ flow_run_active_frame_count' = (flow_run_active_frame_count) - 1
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "FrameTerminated", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_TerminalizeCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "TerminalizeCompleted"
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Running"
       /\ flow_run__AllTrackedStepsInAllowedStatuses(<<Some("Completed"), Some("Skipped")>>)
       /\ flow_run_phase' = "Completed"
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EmitFlowRunNotice", payload |-> [run_status |-> "Completed"], effect_id |-> (model_step_count + 1), source_transition |-> "TerminalizeCompleted"], [machine |-> "flow_run", variant |-> "FlowTerminalized", payload |-> [run_status |-> "Completed"], effect_id |-> (model_step_count + 1), source_transition |-> "TerminalizeCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "TerminalizeCompleted", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Completed"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_TerminalizeFailed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "TerminalizeFailed"
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Pending" \/ flow_run_phase = "Running"
       /\ flow_run__NoTrackedStepInStatus("Dispatched")
       /\ flow_run_phase' = "Failed"
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EmitFlowRunNotice", payload |-> [run_status |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "TerminalizeFailed"], [machine |-> "flow_run", variant |-> "FlowTerminalized", payload |-> [run_status |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "TerminalizeFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "TerminalizeFailed", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Failed"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_TerminalizeCanceled ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_run"
       /\ packet.variant = "TerminalizeCanceled"
       /\ ~HigherPriorityReady("run_engine")
       /\ flow_run_phase = "Pending" \/ flow_run_phase = "Running"
       /\ flow_run__NoTrackedStepInStatus("Dispatched")
       /\ flow_run_phase' = "Canceled"
       /\ UNCHANGED << flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_run", variant |-> "EmitFlowRunNotice", payload |-> [run_status |-> "Canceled"], effect_id |-> (model_step_count + 1), source_transition |-> "TerminalizeCanceled"], [machine |-> "flow_run", variant |-> "FlowTerminalized", payload |-> [run_status |-> "Canceled"], effect_id |-> (model_step_count + 1), source_transition |-> "TerminalizeCanceled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_run", transition |-> "TerminalizeCanceled", actor |-> "run_engine", step |-> (model_step_count + 1), from_phase |-> flow_run_phase, to_phase |-> "Canceled"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_run_output_only_follows_completed_steps == (\A step_id \in flow_run_tracked_steps : (~(flow_run__StepOutputRecordedIs(step_id, TRUE)) \/ flow_run__StepStatusIs(step_id, "Completed")))
flow_run_terminal_runs_have_no_dispatched_steps == (~(flow_run__RunIsTerminal) \/ flow_run__NoTrackedStepInStatus("Dispatched"))
flow_run_completed_runs_contain_only_completed_or_skipped_steps == ((flow_run_phase # "Completed") \/ flow_run__AllTrackedStepsInAllowedStatuses(<<Some("Completed"), Some("Skipped")>>))
flow_run_failed_step_presence_requires_failure_count == (~(flow_run__AnyTrackedStepInStatus("Failed")) \/ (flow_run_failure_count >= 1))
flow_run_failed_run_has_failed_step_or_recorded_failure == ((flow_run_phase # "Failed") \/ TRUE)

flow_frame__AllNodesTerminal == (\A t_node \in flow_frame_tracked_nodes : (((IF t_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[t_node] ELSE "None") = "Completed") \/ ((IF t_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[t_node] ELSE "None") = "Failed") \/ ((IF t_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[t_node] ELSE "None") = "Skipped") \/ ((IF t_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[t_node] ELSE "None") = "Canceled")))

flow_frame__AnyDepCompleted(node_id) == (\E dep_id \in (IF node_id \in DOMAIN flow_frame_node_dependencies THEN flow_frame_node_dependencies[node_id] ELSE <<>>) : ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Completed"))

flow_frame__AllDepsCompleted(node_id) == (\A dep_id \in (IF node_id \in DOMAIN flow_frame_node_dependencies THEN flow_frame_node_dependencies[node_id] ELSE <<>>) : ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Completed"))

flow_frame__NodeAdmissionEligible(node_id) == (IF (Len((IF node_id \in DOMAIN flow_frame_node_dependencies THEN flow_frame_node_dependencies[node_id] ELSE <<>>)) = 0) THEN TRUE ELSE (IF ((IF node_id \in DOMAIN flow_frame_node_dependency_modes THEN flow_frame_node_dependency_modes[node_id] ELSE "None") = "All") THEN (\A dep_id \in (IF node_id \in DOMAIN flow_frame_node_dependencies THEN flow_frame_node_dependencies[node_id] ELSE <<>>) : (((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Completed") \/ ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Failed") \/ ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Skipped") \/ ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Canceled"))) ELSE ((\E dep_id \in (IF node_id \in DOMAIN flow_frame_node_dependencies THEN flow_frame_node_dependencies[node_id] ELSE <<>>) : ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Completed")) \/ (\A dep_id \in (IF node_id \in DOMAIN flow_frame_node_dependencies THEN flow_frame_node_dependencies[node_id] ELSE <<>>) : (((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Completed") \/ ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Failed") \/ ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Skipped") \/ ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Canceled"))))))

RECURSIVE flow_frame_StartRootFrame_ForEach0_node_status(_, _)
flow_frame_StartRootFrame_ForEach0_node_status(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET init_node == item IN LET next_acc == MapSet(acc, init_node, "Pending") IN flow_frame_StartRootFrame_ForEach0_node_status(next_acc, remaining \ {item})

RECURSIVE flow_frame_StartRootFrame_ForEach1_node_status(_, _)
flow_frame_StartRootFrame_ForEach1_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ flow_frame__NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN flow_frame_StartRootFrame_ForEach1_node_status(next_acc, Tail(items))

RECURSIVE flow_frame_StartRootFrame_ForEach2_ready_queue(_, _, _)
flow_frame_StartRootFrame_ForEach2_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN flow_frame_StartRootFrame_ForEach2_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE flow_frame_StartBodyFrame_ForEach3_node_status(_, _)
flow_frame_StartBodyFrame_ForEach3_node_status(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET init_node == item IN LET next_acc == MapSet(acc, init_node, "Pending") IN flow_frame_StartBodyFrame_ForEach3_node_status(next_acc, remaining \ {item})

RECURSIVE flow_frame_StartBodyFrame_ForEach4_node_status(_, _)
flow_frame_StartBodyFrame_ForEach4_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ flow_frame__NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN flow_frame_StartBodyFrame_ForEach4_node_status(next_acc, Tail(items))

RECURSIVE flow_frame_StartBodyFrame_ForEach5_ready_queue(_, _, _)
flow_frame_StartBodyFrame_ForEach5_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN flow_frame_StartBodyFrame_ForEach5_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE flow_frame_AdmitNextReadyNode_Skip_ForEach6_node_status(_, _)
flow_frame_AdmitNextReadyNode_Skip_ForEach6_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ flow_frame__NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN flow_frame_AdmitNextReadyNode_Skip_ForEach6_node_status(next_acc, Tail(items))

RECURSIVE flow_frame_AdmitNextReadyNode_Skip_ForEach7_ready_queue(_, _, _)
flow_frame_AdmitNextReadyNode_Skip_ForEach7_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN flow_frame_AdmitNextReadyNode_Skip_ForEach7_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE flow_frame_AdmitNextReadyNode_Fail_ForEach8_node_status(_, _)
flow_frame_AdmitNextReadyNode_Fail_ForEach8_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ flow_frame__NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN flow_frame_AdmitNextReadyNode_Fail_ForEach8_node_status(next_acc, Tail(items))

RECURSIVE flow_frame_AdmitNextReadyNode_Fail_ForEach9_ready_queue(_, _, _)
flow_frame_AdmitNextReadyNode_Fail_ForEach9_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN flow_frame_AdmitNextReadyNode_Fail_ForEach9_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE flow_frame_CompleteNode_Step_ForEach10_node_status(_, _)
flow_frame_CompleteNode_Step_ForEach10_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ flow_frame__NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN flow_frame_CompleteNode_Step_ForEach10_node_status(next_acc, Tail(items))

RECURSIVE flow_frame_CompleteNode_Step_ForEach11_ready_queue(_, _, _)
flow_frame_CompleteNode_Step_ForEach11_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN flow_frame_CompleteNode_Step_ForEach11_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE flow_frame_CompleteNode_Loop_ForEach12_node_status(_, _)
flow_frame_CompleteNode_Loop_ForEach12_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ flow_frame__NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN flow_frame_CompleteNode_Loop_ForEach12_node_status(next_acc, Tail(items))

RECURSIVE flow_frame_CompleteNode_Loop_ForEach13_ready_queue(_, _, _)
flow_frame_CompleteNode_Loop_ForEach13_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN flow_frame_CompleteNode_Loop_ForEach13_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE flow_frame_FailNode_Step_ForEach14_node_status(_, _)
flow_frame_FailNode_Step_ForEach14_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ flow_frame__NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN flow_frame_FailNode_Step_ForEach14_node_status(next_acc, Tail(items))

RECURSIVE flow_frame_FailNode_Step_ForEach15_ready_queue(_, _, _)
flow_frame_FailNode_Step_ForEach15_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN flow_frame_FailNode_Step_ForEach15_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE flow_frame_FailNode_Loop_ForEach16_node_status(_, _)
flow_frame_FailNode_Loop_ForEach16_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ flow_frame__NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN flow_frame_FailNode_Loop_ForEach16_node_status(next_acc, Tail(items))

RECURSIVE flow_frame_FailNode_Loop_ForEach17_ready_queue(_, _, _)
flow_frame_FailNode_Loop_ForEach17_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN flow_frame_FailNode_Loop_ForEach17_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE flow_frame_SkipNode_Step_ForEach18_node_status(_, _)
flow_frame_SkipNode_Step_ForEach18_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ flow_frame__NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN flow_frame_SkipNode_Step_ForEach18_node_status(next_acc, Tail(items))

RECURSIVE flow_frame_SkipNode_Step_ForEach19_ready_queue(_, _, _)
flow_frame_SkipNode_Step_ForEach19_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN flow_frame_SkipNode_Step_ForEach19_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE flow_frame_SkipNode_Loop_ForEach20_node_status(_, _)
flow_frame_SkipNode_Loop_ForEach20_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ flow_frame__NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN flow_frame_SkipNode_Loop_ForEach20_node_status(next_acc, Tail(items))

RECURSIVE flow_frame_SkipNode_Loop_ForEach21_ready_queue(_, _, _)
flow_frame_SkipNode_Loop_ForEach21_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN flow_frame_SkipNode_Loop_ForEach21_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE flow_frame_CancelNode_Step_ForEach22_node_status(_, _)
flow_frame_CancelNode_Step_ForEach22_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ flow_frame__NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN flow_frame_CancelNode_Step_ForEach22_node_status(next_acc, Tail(items))

RECURSIVE flow_frame_CancelNode_Step_ForEach23_ready_queue(_, _, _)
flow_frame_CancelNode_Step_ForEach23_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN flow_frame_CancelNode_Step_ForEach23_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE flow_frame_CancelNode_Loop_ForEach24_node_status(_, _)
flow_frame_CancelNode_Loop_ForEach24_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ flow_frame__NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN flow_frame_CancelNode_Loop_ForEach24_node_status(next_acc, Tail(items))

RECURSIVE flow_frame_CancelNode_Loop_ForEach25_ready_queue(_, _, _)
flow_frame_CancelNode_Loop_ForEach25_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN flow_frame_CancelNode_Loop_ForEach25_ready_queue(next_acc, Tail(items), captured_node_status)

flow_frame_StartRootFrame(arg_frame_id, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "StartRootFrame"
       /\ packet.payload.frame_id = arg_frame_id
       /\ packet.payload.tracked_nodes = arg_tracked_nodes
       /\ packet.payload.ordered_nodes = arg_ordered_nodes
       /\ packet.payload.node_kind = arg_node_kind
       /\ packet.payload.node_dependencies = arg_node_dependencies
       /\ packet.payload.node_dependency_modes = arg_node_dependency_modes
       /\ packet.payload.node_branches = arg_node_branches
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Absent"
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_frame_id' = packet.payload.frame_id
       /\ flow_frame_frame_scope' = "Root"
       /\ flow_frame_loop_instance_id' = ""
       /\ flow_frame_iteration' = 0
       /\ flow_frame_tracked_nodes' = packet.payload.tracked_nodes
       /\ flow_frame_ordered_nodes' = packet.payload.ordered_nodes
       /\ flow_frame_node_kind' = packet.payload.node_kind
       /\ flow_frame_node_dependencies' = packet.payload.node_dependencies
       /\ flow_frame_node_dependency_modes' = packet.payload.node_dependency_modes
       /\ flow_frame_node_branches' = packet.payload.node_branches
       /\ flow_frame_node_status' = flow_frame_StartRootFrame_ForEach1_node_status(flow_frame_StartRootFrame_ForEach0_node_status(flow_frame_node_status, packet.payload.tracked_nodes), packet.payload.ordered_nodes)
       /\ flow_frame_ready_queue' = flow_frame_StartRootFrame_ForEach2_ready_queue(flow_frame_ready_queue, packet.payload.ordered_nodes, flow_frame_StartRootFrame_ForEach1_node_status(flow_frame_StartRootFrame_ForEach0_node_status(flow_frame_node_status, packet.payload.tracked_nodes), packet.payload.ordered_nodes))
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_last_admitted_node, flow_frame_branch_winners, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> packet.payload.frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> packet.payload.frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> packet.payload.frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "StartRootFrame"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> packet.payload.frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "StartRootFrame"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "StartRootFrame", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_StartBodyFrame(arg_frame_id, arg_loop_instance_id, arg_iteration, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "StartBodyFrame"
       /\ packet.payload.frame_id = arg_frame_id
       /\ packet.payload.loop_instance_id = arg_loop_instance_id
       /\ packet.payload.iteration = arg_iteration
       /\ packet.payload.tracked_nodes = arg_tracked_nodes
       /\ packet.payload.ordered_nodes = arg_ordered_nodes
       /\ packet.payload.node_kind = arg_node_kind
       /\ packet.payload.node_dependencies = arg_node_dependencies
       /\ packet.payload.node_dependency_modes = arg_node_dependency_modes
       /\ packet.payload.node_branches = arg_node_branches
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Absent"
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_frame_id' = packet.payload.frame_id
       /\ flow_frame_frame_scope' = "Body"
       /\ flow_frame_loop_instance_id' = packet.payload.loop_instance_id
       /\ flow_frame_iteration' = packet.payload.iteration
       /\ flow_frame_tracked_nodes' = packet.payload.tracked_nodes
       /\ flow_frame_ordered_nodes' = packet.payload.ordered_nodes
       /\ flow_frame_node_kind' = packet.payload.node_kind
       /\ flow_frame_node_dependencies' = packet.payload.node_dependencies
       /\ flow_frame_node_dependency_modes' = packet.payload.node_dependency_modes
       /\ flow_frame_node_branches' = packet.payload.node_branches
       /\ flow_frame_node_status' = flow_frame_StartBodyFrame_ForEach4_node_status(flow_frame_StartBodyFrame_ForEach3_node_status(flow_frame_node_status, packet.payload.tracked_nodes), packet.payload.ordered_nodes)
       /\ flow_frame_ready_queue' = flow_frame_StartBodyFrame_ForEach5_ready_queue(flow_frame_ready_queue, packet.payload.ordered_nodes, flow_frame_StartBodyFrame_ForEach4_node_status(flow_frame_StartBodyFrame_ForEach3_node_status(flow_frame_node_status, packet.payload.tracked_nodes), packet.payload.ordered_nodes))
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_last_admitted_node, flow_frame_branch_winners, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> packet.payload.frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> packet.payload.frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> packet.payload.frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "StartBodyFrame"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> packet.payload.frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "StartBodyFrame"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "StartBodyFrame", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_AdmitNextReadyNode_StepRun ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "AdmitNextReadyNode"
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ (Len(flow_frame_ready_queue) > 0)
       /\ ((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_kind THEN flow_frame_node_kind[Head(flow_frame_ready_queue)] ELSE "None") = "Step")
       /\ (~(((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_branches THEN flow_frame_node_branches[Head(flow_frame_ready_queue)] ELSE None) \in flow_frame_branch_winners)) /\ ((Len((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_dependencies THEN flow_frame_node_dependencies[Head(flow_frame_ready_queue)] ELSE <<>>)) = 0) \/ (((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_dependency_modes THEN flow_frame_node_dependency_modes[Head(flow_frame_ready_queue)] ELSE "None") = "All") /\ flow_frame__AllDepsCompleted(Head(flow_frame_ready_queue))) \/ (((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_dependency_modes THEN flow_frame_node_dependency_modes[Head(flow_frame_ready_queue)] ELSE "None") = "Any") /\ flow_frame__AnyDepCompleted(Head(flow_frame_ready_queue)))))
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_last_admitted_node' = Head(flow_frame_ready_queue)
       /\ flow_frame_node_status' = MapSet(flow_frame_node_status, Head(flow_frame_ready_queue), "Running")
       /\ flow_frame_ready_queue' = Tail(flow_frame_ready_queue)
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_StepRun"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "AdmitStepWork", payload |-> [frame_id |-> flow_frame_frame_id, node_id |-> Head(flow_frame_ready_queue)], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_StepRun"], [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_StepRun"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "AdmitNextReadyNode_StepRun", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_AdmitNextReadyNode_LoopRun ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "AdmitNextReadyNode"
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ (Len(flow_frame_ready_queue) > 0)
       /\ ((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_kind THEN flow_frame_node_kind[Head(flow_frame_ready_queue)] ELSE "None") = "Loop")
       /\ (~(((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_branches THEN flow_frame_node_branches[Head(flow_frame_ready_queue)] ELSE None) \in flow_frame_branch_winners)) /\ ((Len((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_dependencies THEN flow_frame_node_dependencies[Head(flow_frame_ready_queue)] ELSE <<>>)) = 0) \/ (((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_dependency_modes THEN flow_frame_node_dependency_modes[Head(flow_frame_ready_queue)] ELSE "None") = "All") /\ flow_frame__AllDepsCompleted(Head(flow_frame_ready_queue))) \/ (((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_dependency_modes THEN flow_frame_node_dependency_modes[Head(flow_frame_ready_queue)] ELSE "None") = "Any") /\ flow_frame__AnyDepCompleted(Head(flow_frame_ready_queue)))))
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_last_admitted_node' = Head(flow_frame_ready_queue)
       /\ flow_frame_node_status' = MapSet(flow_frame_node_status, Head(flow_frame_ready_queue), "Running")
       /\ flow_frame_ready_queue' = Tail(flow_frame_ready_queue)
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)]), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)], [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", effect |-> "NodeExecutionReleased", target_machine |-> "flow_run", target_input |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_LoopRun"], [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_LoopRun"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "StartLoopNode", payload |-> [frame_id |-> flow_frame_frame_id, node_id |-> Head(flow_frame_ready_queue)], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_LoopRun"], [machine |-> "flow_frame", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id, node_id |-> Head(flow_frame_ready_queue)], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_LoopRun"], [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_LoopRun"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "AdmitNextReadyNode_LoopRun", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_AdmitNextReadyNode_Skip ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "AdmitNextReadyNode"
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ (Len(flow_frame_ready_queue) > 0)
       /\ ((((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_dependency_modes THEN flow_frame_node_dependency_modes[Head(flow_frame_ready_queue)] ELSE "None") = "All") /\ (\E dep_id \in (IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_dependencies THEN flow_frame_node_dependencies[Head(flow_frame_ready_queue)] ELSE <<>>) : (((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Failed") \/ ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Skipped") \/ ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Canceled")))) \/ ((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_branches THEN flow_frame_node_branches[Head(flow_frame_ready_queue)] ELSE None) \in flow_frame_branch_winners))
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_last_admitted_node' = Head(flow_frame_ready_queue)
       /\ flow_frame_node_status' = flow_frame_AdmitNextReadyNode_Skip_ForEach6_node_status(MapSet(flow_frame_node_status, Head(flow_frame_ready_queue), "Skipped"), flow_frame_ordered_nodes)
       /\ flow_frame_ready_queue' = flow_frame_AdmitNextReadyNode_Skip_ForEach7_ready_queue(Tail(flow_frame_ready_queue), flow_frame_ordered_nodes, flow_frame_AdmitNextReadyNode_Skip_ForEach6_node_status(MapSet(flow_frame_node_status, Head(flow_frame_ready_queue), "Skipped"), flow_frame_ordered_nodes))
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)]), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)], [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", effect |-> "NodeExecutionReleased", target_machine |-> "flow_run", target_input |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_Skip"], [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_Skip"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id, node_id |-> Head(flow_frame_ready_queue)], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_Skip"], [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_Skip"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "AdmitNextReadyNode_Skip", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_AdmitNextReadyNode_Fail ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "AdmitNextReadyNode"
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ (Len(flow_frame_ready_queue) > 0)
       /\ (((IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_dependency_modes THEN flow_frame_node_dependency_modes[Head(flow_frame_ready_queue)] ELSE "None") = "Any") /\ (\A dep_id \in (IF Head(flow_frame_ready_queue) \in DOMAIN flow_frame_node_dependencies THEN flow_frame_node_dependencies[Head(flow_frame_ready_queue)] ELSE <<>>) : (((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Completed") \/ ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Failed") \/ ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Skipped") \/ ((IF dep_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[dep_id] ELSE "None") = "Canceled"))) /\ ~(flow_frame__AnyDepCompleted(Head(flow_frame_ready_queue))))
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_last_admitted_node' = Head(flow_frame_ready_queue)
       /\ flow_frame_node_status' = flow_frame_AdmitNextReadyNode_Fail_ForEach8_node_status(MapSet(flow_frame_node_status, Head(flow_frame_ready_queue), "Failed"), flow_frame_ordered_nodes)
       /\ flow_frame_ready_queue' = flow_frame_AdmitNextReadyNode_Fail_ForEach9_ready_queue(Tail(flow_frame_ready_queue), flow_frame_ordered_nodes, flow_frame_AdmitNextReadyNode_Fail_ForEach8_node_status(MapSet(flow_frame_node_status, Head(flow_frame_ready_queue), "Failed"), flow_frame_ordered_nodes))
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)]), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)], [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", effect |-> "NodeExecutionReleased", target_machine |-> "flow_run", target_input |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_Fail"], [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_Fail"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id, node_id |-> Head(flow_frame_ready_queue)], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_Fail"], [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitNextReadyNode_Fail"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "AdmitNextReadyNode_Fail", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_CompleteNode_Step(arg_node_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "CompleteNode"
       /\ packet.payload.node_id = arg_node_id
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[packet.payload.node_id] ELSE "None") = "Running")
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_kind THEN flow_frame_node_kind[packet.payload.node_id] ELSE "None") = "Step")
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_branch_winners' = IF ((IF packet.payload.node_id \in DOMAIN flow_frame_node_branches THEN flow_frame_node_branches[packet.payload.node_id] ELSE None) # None) THEN (flow_frame_branch_winners \cup {(IF packet.payload.node_id \in DOMAIN flow_frame_node_branches THEN flow_frame_node_branches[packet.payload.node_id] ELSE None)}) ELSE flow_frame_branch_winners
       /\ flow_frame_node_status' = flow_frame_CompleteNode_Step_ForEach10_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Completed"), flow_frame_ordered_nodes)
       /\ flow_frame_ready_queue' = flow_frame_CompleteNode_Step_ForEach11_ready_queue(flow_frame_ready_queue, flow_frame_ordered_nodes, flow_frame_CompleteNode_Step_ForEach10_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Completed"), flow_frame_ordered_nodes))
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)]), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)], [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", effect |-> "NodeExecutionReleased", target_machine |-> "flow_run", target_input |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "CompleteNode_Step"], [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "CompleteNode_Step"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id, node_id |-> packet.payload.node_id], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteNode_Step"], [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteNode_Step"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "CompleteNode_Step", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_CompleteNode_Loop(arg_node_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "CompleteNode"
       /\ packet.payload.node_id = arg_node_id
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[packet.payload.node_id] ELSE "None") = "Running")
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_kind THEN flow_frame_node_kind[packet.payload.node_id] ELSE "None") = "Loop")
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_branch_winners' = IF ((IF packet.payload.node_id \in DOMAIN flow_frame_node_branches THEN flow_frame_node_branches[packet.payload.node_id] ELSE None) # None) THEN (flow_frame_branch_winners \cup {(IF packet.payload.node_id \in DOMAIN flow_frame_node_branches THEN flow_frame_node_branches[packet.payload.node_id] ELSE None)}) ELSE flow_frame_branch_winners
       /\ flow_frame_node_status' = flow_frame_CompleteNode_Loop_ForEach12_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Completed"), flow_frame_ordered_nodes)
       /\ flow_frame_ready_queue' = flow_frame_CompleteNode_Loop_ForEach13_ready_queue(flow_frame_ready_queue, flow_frame_ordered_nodes, flow_frame_CompleteNode_Loop_ForEach12_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Completed"), flow_frame_ordered_nodes))
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "CompleteNode_Loop"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteNode_Loop"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "CompleteNode_Loop", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_RecordNodeOutput(arg_node_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "RecordNodeOutput"
       /\ packet.payload.node_id = arg_node_id
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_output_recorded' = MapSet(flow_frame_output_recorded, packet.payload.node_id, TRUE)
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "PersistStepOutput", payload |-> [frame_id |-> flow_frame_frame_id, node_id |-> packet.payload.node_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecordNodeOutput"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "RecordNodeOutput", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_FailNode_Step(arg_node_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "FailNode"
       /\ packet.payload.node_id = arg_node_id
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[packet.payload.node_id] ELSE "None") = "Running")
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_kind THEN flow_frame_node_kind[packet.payload.node_id] ELSE "None") = "Step")
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_node_status' = flow_frame_FailNode_Step_ForEach14_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Failed"), flow_frame_ordered_nodes)
       /\ flow_frame_ready_queue' = flow_frame_FailNode_Step_ForEach15_ready_queue(flow_frame_ready_queue, flow_frame_ordered_nodes, flow_frame_FailNode_Step_ForEach14_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Failed"), flow_frame_ordered_nodes))
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)]), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)], [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", effect |-> "NodeExecutionReleased", target_machine |-> "flow_run", target_input |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "FailNode_Step"], [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "FailNode_Step"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id, node_id |-> packet.payload.node_id], effect_id |-> (model_step_count + 1), source_transition |-> "FailNode_Step"], [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "FailNode_Step"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "FailNode_Step", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_FailNode_Loop(arg_node_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "FailNode"
       /\ packet.payload.node_id = arg_node_id
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[packet.payload.node_id] ELSE "None") = "Running")
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_kind THEN flow_frame_node_kind[packet.payload.node_id] ELSE "None") = "Loop")
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_node_status' = flow_frame_FailNode_Loop_ForEach16_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Failed"), flow_frame_ordered_nodes)
       /\ flow_frame_ready_queue' = flow_frame_FailNode_Loop_ForEach17_ready_queue(flow_frame_ready_queue, flow_frame_ordered_nodes, flow_frame_FailNode_Loop_ForEach16_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Failed"), flow_frame_ordered_nodes))
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "FailNode_Loop"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "FailNode_Loop"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "FailNode_Loop", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_SkipNode_Step(arg_node_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "SkipNode"
       /\ packet.payload.node_id = arg_node_id
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[packet.payload.node_id] ELSE "None") = "Running")
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_kind THEN flow_frame_node_kind[packet.payload.node_id] ELSE "None") = "Step")
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_node_status' = flow_frame_SkipNode_Step_ForEach18_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Skipped"), flow_frame_ordered_nodes)
       /\ flow_frame_ready_queue' = flow_frame_SkipNode_Step_ForEach19_ready_queue(flow_frame_ready_queue, flow_frame_ordered_nodes, flow_frame_SkipNode_Step_ForEach18_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Skipped"), flow_frame_ordered_nodes))
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)]), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)], [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", effect |-> "NodeExecutionReleased", target_machine |-> "flow_run", target_input |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "SkipNode_Step"], [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "SkipNode_Step"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id, node_id |-> packet.payload.node_id], effect_id |-> (model_step_count + 1), source_transition |-> "SkipNode_Step"], [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "SkipNode_Step"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "SkipNode_Step", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_SkipNode_Loop(arg_node_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "SkipNode"
       /\ packet.payload.node_id = arg_node_id
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[packet.payload.node_id] ELSE "None") = "Running")
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_kind THEN flow_frame_node_kind[packet.payload.node_id] ELSE "None") = "Loop")
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_node_status' = flow_frame_SkipNode_Loop_ForEach20_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Skipped"), flow_frame_ordered_nodes)
       /\ flow_frame_ready_queue' = flow_frame_SkipNode_Loop_ForEach21_ready_queue(flow_frame_ready_queue, flow_frame_ordered_nodes, flow_frame_SkipNode_Loop_ForEach20_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Skipped"), flow_frame_ordered_nodes))
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "SkipNode_Loop"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "SkipNode_Loop"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "SkipNode_Loop", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_CancelNode_Step(arg_node_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "CancelNode"
       /\ packet.payload.node_id = arg_node_id
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[packet.payload.node_id] ELSE "None") = "Running")
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_kind THEN flow_frame_node_kind[packet.payload.node_id] ELSE "None") = "Step")
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_node_status' = flow_frame_CancelNode_Step_ForEach22_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Canceled"), flow_frame_ordered_nodes)
       /\ flow_frame_ready_queue' = flow_frame_CancelNode_Step_ForEach23_ready_queue(flow_frame_ready_queue, flow_frame_ordered_nodes, flow_frame_CancelNode_Step_ForEach22_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Canceled"), flow_frame_ordered_nodes))
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)]), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", source_effect |-> "NodeExecutionReleased", effect_id |-> (model_step_count + 1)], [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_node_release_updates_run_slots", source_machine |-> "flow_frame", effect |-> "NodeExecutionReleased", target_machine |-> "flow_run", target_input |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "CancelNode_Step"], [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "CancelNode_Step"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> flow_frame_frame_id, node_id |-> packet.payload.node_id], effect_id |-> (model_step_count + 1), source_transition |-> "CancelNode_Step"], [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "CancelNode_Step"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "CancelNode_Step", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_CancelNode_Loop(arg_node_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "CancelNode"
       /\ packet.payload.node_id = arg_node_id
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[packet.payload.node_id] ELSE "None") = "Running")
       /\ ((IF packet.payload.node_id \in DOMAIN flow_frame_node_kind THEN flow_frame_node_kind[packet.payload.node_id] ELSE "None") = "Loop")
       /\ flow_frame_phase' = "Running"
       /\ flow_frame_node_status' = flow_frame_CancelNode_Loop_ForEach24_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Canceled"), flow_frame_ordered_nodes)
       /\ flow_frame_ready_queue' = flow_frame_CancelNode_Loop_ForEach25_ready_queue(flow_frame_ready_queue, flow_frame_ordered_nodes, flow_frame_CancelNode_Loop_ForEach24_node_status(MapSet(flow_frame_node_status, packet.payload.node_id, "Canceled"), flow_frame_ordered_nodes))
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], source_kind |-> "route", source_route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", source_effect |-> "ReadyFrontierChanged", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "flow_frame_ready_frontier_updates_run_ready_frames", source_machine |-> "flow_frame", effect |-> "ReadyFrontierChanged", target_machine |-> "flow_run", target_input |-> "RegisterReadyFrame", payload |-> [frame_id |-> flow_frame_frame_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "CancelNode_Loop"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "ReadyFrontierChanged", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "CancelNode_Loop"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "CancelNode_Loop", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_SealRootFrameCanceled ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "SealFrame"
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ flow_frame__AllNodesTerminal
       /\ (flow_frame_frame_scope = "Root")
       /\ (\E status_node \in flow_frame_tracked_nodes : ((IF status_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[status_node] ELSE "None") = "Canceled"))
       /\ flow_frame_phase' = "Canceled"
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "RootFrameCanceled", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "SealRootFrameCanceled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "SealRootFrameCanceled", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Canceled"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_SealRootFrameFailed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "SealFrame"
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ flow_frame__AllNodesTerminal
       /\ (flow_frame_frame_scope = "Root")
       /\ ~((\E status_node \in flow_frame_tracked_nodes : ((IF status_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[status_node] ELSE "None") = "Canceled")))
       /\ (\E status_node \in flow_frame_tracked_nodes : ((IF status_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[status_node] ELSE "None") = "Failed"))
       /\ flow_frame_phase' = "Failed"
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "RootFrameFailed", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "SealRootFrameFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "SealRootFrameFailed", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Failed"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_SealRootFrameCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "SealFrame"
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ flow_frame__AllNodesTerminal
       /\ (flow_frame_frame_scope = "Root")
       /\ ~((\E status_node \in flow_frame_tracked_nodes : ((IF status_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[status_node] ELSE "None") = "Canceled")))
       /\ ~((\E status_node \in flow_frame_tracked_nodes : ((IF status_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[status_node] ELSE "None") = "Failed")))
       /\ flow_frame_phase' = "Completed"
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "RootFrameCompleted", payload |-> [frame_id |-> flow_frame_frame_id], effect_id |-> (model_step_count + 1), source_transition |-> "SealRootFrameCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "SealRootFrameCompleted", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Completed"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_SealBodyFrameCanceled ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "SealFrame"
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ flow_frame__AllNodesTerminal
       /\ (flow_frame_frame_scope = "Body")
       /\ (\E status_node \in flow_frame_tracked_nodes : ((IF status_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[status_node] ELSE "None") = "Canceled"))
       /\ flow_frame_phase' = "Canceled"
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "loop_iteration", variant |-> "BodyFrameCanceled", payload |-> [iteration |-> flow_frame_iteration, loop_instance_id |-> flow_frame_loop_instance_id], source_kind |-> "route", source_route |-> "body_frame_canceled_cancels_loop_iteration", source_machine |-> "flow_frame", source_effect |-> "BodyFrameCanceled", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "loop_iteration", variant |-> "BodyFrameCanceled", payload |-> [iteration |-> flow_frame_iteration, loop_instance_id |-> flow_frame_loop_instance_id], source_kind |-> "route", source_route |-> "body_frame_canceled_cancels_loop_iteration", source_machine |-> "flow_frame", source_effect |-> "BodyFrameCanceled", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "body_frame_canceled_cancels_loop_iteration", source_machine |-> "flow_frame", effect |-> "BodyFrameCanceled", target_machine |-> "loop_iteration", target_input |-> "BodyFrameCanceled", payload |-> [iteration |-> flow_frame_iteration, loop_instance_id |-> flow_frame_loop_instance_id], actor |-> "loop_engine", effect_id |-> (model_step_count + 1), source_transition |-> "SealBodyFrameCanceled"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "BodyFrameCanceled", payload |-> [frame_id |-> flow_frame_frame_id, iteration |-> flow_frame_iteration, loop_instance_id |-> flow_frame_loop_instance_id], effect_id |-> (model_step_count + 1), source_transition |-> "SealBodyFrameCanceled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "SealBodyFrameCanceled", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Canceled"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_SealBodyFrameFailed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "SealFrame"
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ flow_frame__AllNodesTerminal
       /\ (flow_frame_frame_scope = "Body")
       /\ ~((\E status_node \in flow_frame_tracked_nodes : ((IF status_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[status_node] ELSE "None") = "Canceled")))
       /\ (\E status_node \in flow_frame_tracked_nodes : ((IF status_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[status_node] ELSE "None") = "Failed"))
       /\ flow_frame_phase' = "Failed"
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "loop_iteration", variant |-> "BodyFrameFailed", payload |-> [iteration |-> flow_frame_iteration, loop_instance_id |-> flow_frame_loop_instance_id], source_kind |-> "route", source_route |-> "body_frame_failed_fails_loop_iteration", source_machine |-> "flow_frame", source_effect |-> "BodyFrameFailed", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "loop_iteration", variant |-> "BodyFrameFailed", payload |-> [iteration |-> flow_frame_iteration, loop_instance_id |-> flow_frame_loop_instance_id], source_kind |-> "route", source_route |-> "body_frame_failed_fails_loop_iteration", source_machine |-> "flow_frame", source_effect |-> "BodyFrameFailed", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "body_frame_failed_fails_loop_iteration", source_machine |-> "flow_frame", effect |-> "BodyFrameFailed", target_machine |-> "loop_iteration", target_input |-> "BodyFrameFailed", payload |-> [iteration |-> flow_frame_iteration, loop_instance_id |-> flow_frame_loop_instance_id], actor |-> "loop_engine", effect_id |-> (model_step_count + 1), source_transition |-> "SealBodyFrameFailed"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "BodyFrameFailed", payload |-> [frame_id |-> flow_frame_frame_id, iteration |-> flow_frame_iteration, loop_instance_id |-> flow_frame_loop_instance_id], effect_id |-> (model_step_count + 1), source_transition |-> "SealBodyFrameFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "SealBodyFrameFailed", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Failed"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_SealBodyFrameCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "flow_frame"
       /\ packet.variant = "SealFrame"
       /\ ~HigherPriorityReady("frame_engine")
       /\ flow_frame_phase = "Running"
       /\ flow_frame__AllNodesTerminal
       /\ (flow_frame_frame_scope = "Body")
       /\ ~((\E status_node \in flow_frame_tracked_nodes : ((IF status_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[status_node] ELSE "None") = "Canceled")))
       /\ ~((\E status_node \in flow_frame_tracked_nodes : ((IF status_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[status_node] ELSE "None") = "Failed")))
       /\ flow_frame_phase' = "Completed"
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "loop_iteration", variant |-> "BodyFrameCompleted", payload |-> [iteration |-> flow_frame_iteration, loop_instance_id |-> flow_frame_loop_instance_id], source_kind |-> "route", source_route |-> "body_frame_completed_advances_loop_iteration", source_machine |-> "flow_frame", source_effect |-> "BodyFrameCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "loop_iteration", variant |-> "BodyFrameCompleted", payload |-> [iteration |-> flow_frame_iteration, loop_instance_id |-> flow_frame_loop_instance_id], source_kind |-> "route", source_route |-> "body_frame_completed_advances_loop_iteration", source_machine |-> "flow_frame", source_effect |-> "BodyFrameCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "body_frame_completed_advances_loop_iteration", source_machine |-> "flow_frame", effect |-> "BodyFrameCompleted", target_machine |-> "loop_iteration", target_input |-> "BodyFrameCompleted", payload |-> [iteration |-> flow_frame_iteration, loop_instance_id |-> flow_frame_loop_instance_id], actor |-> "loop_engine", effect_id |-> (model_step_count + 1), source_transition |-> "SealBodyFrameCompleted"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "flow_frame", variant |-> "BodyFrameCompleted", payload |-> [frame_id |-> flow_frame_frame_id, iteration |-> flow_frame_iteration, loop_instance_id |-> flow_frame_loop_instance_id], effect_id |-> (model_step_count + 1), source_transition |-> "SealBodyFrameCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "flow_frame", transition |-> "SealBodyFrameCompleted", actor |-> "frame_engine", step |-> (model_step_count + 1), from_phase |-> flow_frame_phase, to_phase |-> "Completed"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


flow_frame_ready_queue_membership_matches_ready_status == ((\A q_node \in SeqElements(flow_frame_ready_queue) : ((IF q_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[q_node] ELSE "None") = "Ready")) /\ (\A t_node \in flow_frame_tracked_nodes : (((IF t_node \in DOMAIN flow_frame_node_status THEN flow_frame_node_status[t_node] ELSE "None") # "Ready") \/ (t_node \in SeqElements(flow_frame_ready_queue)))))

loop_iteration_StartLoop(arg_loop_instance_id, arg_max_iterations, arg_parent_frame_id, arg_parent_node_id, arg_loop_id, arg_depth) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "loop_iteration"
       /\ packet.variant = "StartLoop"
       /\ packet.payload.loop_instance_id = arg_loop_instance_id
       /\ packet.payload.max_iterations = arg_max_iterations
       /\ packet.payload.parent_frame_id = arg_parent_frame_id
       /\ packet.payload.parent_node_id = arg_parent_node_id
       /\ packet.payload.loop_id = arg_loop_id
       /\ packet.payload.depth = arg_depth
       /\ ~HigherPriorityReady("loop_engine")
       /\ loop_iteration_phase = "Absent"
       /\ loop_iteration_phase' = "Running"
       /\ loop_iteration_loop_instance_id' = packet.payload.loop_instance_id
       /\ loop_iteration_parent_frame_id' = packet.payload.parent_frame_id
       /\ loop_iteration_parent_node_id' = packet.payload.parent_node_id
       /\ loop_iteration_loop_id' = packet.payload.loop_id
       /\ loop_iteration_depth' = packet.payload.depth
       /\ loop_iteration_stage' = "AwaitingBodyFrame"
       /\ loop_iteration_current_iteration' = 0
       /\ loop_iteration_last_completed_iteration' = 0
       /\ loop_iteration_max_iterations' = packet.payload.max_iterations
       /\ loop_iteration_active_body_frame_id' = None
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "RegisterPendingBodyFrame", payload |-> [depth |-> packet.payload.depth, loop_instance_id |-> packet.payload.loop_instance_id], source_kind |-> "route", source_route |-> "loop_request_body_frame_updates_run_pending_queue", source_machine |-> "loop_iteration", source_effect |-> "RequestBodyFrameStart", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "RegisterPendingBodyFrame", payload |-> [depth |-> packet.payload.depth, loop_instance_id |-> packet.payload.loop_instance_id], source_kind |-> "route", source_route |-> "loop_request_body_frame_updates_run_pending_queue", source_machine |-> "loop_iteration", source_effect |-> "RequestBodyFrameStart", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "loop_request_body_frame_updates_run_pending_queue", source_machine |-> "loop_iteration", effect |-> "RequestBodyFrameStart", target_machine |-> "flow_run", target_input |-> "RegisterPendingBodyFrame", payload |-> [depth |-> packet.payload.depth, loop_instance_id |-> packet.payload.loop_instance_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "StartLoop"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "loop_iteration", variant |-> "RequestBodyFrameStart", payload |-> [depth |-> packet.payload.depth, loop_instance_id |-> packet.payload.loop_instance_id], effect_id |-> (model_step_count + 1), source_transition |-> "StartLoop"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "loop_iteration", transition |-> "StartLoop", actor |-> "loop_engine", step |-> (model_step_count + 1), from_phase |-> loop_iteration_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


loop_iteration_BodyFrameStarted(arg_loop_instance_id, arg_frame_id, arg_iteration) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "loop_iteration"
       /\ packet.variant = "BodyFrameStarted"
       /\ packet.payload.loop_instance_id = arg_loop_instance_id
       /\ packet.payload.frame_id = arg_frame_id
       /\ packet.payload.iteration = arg_iteration
       /\ ~HigherPriorityReady("loop_engine")
       /\ loop_iteration_phase = "Running"
       /\ (loop_iteration_loop_instance_id = packet.payload.loop_instance_id)
       /\ (loop_iteration_stage = "AwaitingBodyFrame")
       /\ (loop_iteration_current_iteration = packet.payload.iteration)
       /\ loop_iteration_phase' = "Running"
       /\ loop_iteration_stage' = "BodyFrameActive"
       /\ loop_iteration_active_body_frame_id' = Some(packet.payload.frame_id)
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "loop_iteration", transition |-> "BodyFrameStarted", actor |-> "loop_engine", step |-> (model_step_count + 1), from_phase |-> loop_iteration_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


loop_iteration_BodyFrameCompleted(arg_loop_instance_id, arg_iteration) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "loop_iteration"
       /\ packet.variant = "BodyFrameCompleted"
       /\ packet.payload.loop_instance_id = arg_loop_instance_id
       /\ packet.payload.iteration = arg_iteration
       /\ ~HigherPriorityReady("loop_engine")
       /\ loop_iteration_phase = "Running"
       /\ (loop_iteration_loop_instance_id = packet.payload.loop_instance_id)
       /\ (loop_iteration_stage = "BodyFrameActive")
       /\ (loop_iteration_current_iteration = packet.payload.iteration)
       /\ loop_iteration_phase' = "Running"
       /\ loop_iteration_stage' = "AwaitingUntil"
       /\ loop_iteration_current_iteration' = (loop_iteration_current_iteration) + 1
       /\ loop_iteration_last_completed_iteration' = packet.payload.iteration
       /\ loop_iteration_active_body_frame_id' = None
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_max_iterations, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "loop_iteration", variant |-> "EvaluateUntilCondition", payload |-> [iteration |-> packet.payload.iteration, loop_id |-> loop_iteration_loop_id, loop_instance_id |-> loop_iteration_loop_instance_id, parent_frame_id |-> loop_iteration_parent_frame_id, parent_node_id |-> loop_iteration_parent_node_id], effect_id |-> (model_step_count + 1), source_transition |-> "BodyFrameCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "loop_iteration", transition |-> "BodyFrameCompleted", actor |-> "loop_engine", step |-> (model_step_count + 1), from_phase |-> loop_iteration_phase, to_phase |-> "Running"]}
       /\ obligation_flow_loop_until_evaluation' = obligation_flow_loop_until_evaluation \cup {[loop_instance_id |-> loop_iteration_loop_instance_id, iteration |-> packet.payload.iteration, parent_frame_id |-> loop_iteration_parent_frame_id, parent_node_id |-> loop_iteration_parent_node_id, loop_id |-> loop_iteration_loop_id]}
       /\ model_step_count' = model_step_count + 1


loop_iteration_UntilConditionMet(arg_loop_instance_id, arg_iteration) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "loop_iteration"
       /\ packet.variant = "UntilConditionMet"
       /\ packet.payload.loop_instance_id = arg_loop_instance_id
       /\ packet.payload.iteration = arg_iteration
       /\ ~HigherPriorityReady("loop_engine")
       /\ loop_iteration_phase = "Running"
       /\ (loop_iteration_loop_instance_id = packet.payload.loop_instance_id)
       /\ (loop_iteration_stage = "AwaitingUntil")
       /\ (loop_iteration_last_completed_iteration = packet.payload.iteration)
       /\ loop_iteration_phase' = "Completed"
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_frame", variant |-> "CompleteNode", payload |-> [node_id |-> loop_iteration_parent_node_id], source_kind |-> "route", source_route |-> "loop_completed_completes_parent_loop_node", source_machine |-> "loop_iteration", source_effect |-> "LoopCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_frame", variant |-> "CompleteNode", payload |-> [node_id |-> loop_iteration_parent_node_id], source_kind |-> "route", source_route |-> "loop_completed_completes_parent_loop_node", source_machine |-> "loop_iteration", source_effect |-> "LoopCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "loop_completed_completes_parent_loop_node", source_machine |-> "loop_iteration", effect |-> "LoopCompleted", target_machine |-> "flow_frame", target_input |-> "CompleteNode", payload |-> [node_id |-> loop_iteration_parent_node_id], actor |-> "frame_engine", effect_id |-> (model_step_count + 1), source_transition |-> "UntilConditionMet"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "loop_iteration", variant |-> "LoopCompleted", payload |-> [loop_instance_id |-> loop_iteration_loop_instance_id, parent_frame_id |-> loop_iteration_parent_frame_id, parent_node_id |-> loop_iteration_parent_node_id], effect_id |-> (model_step_count + 1), source_transition |-> "UntilConditionMet"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "loop_iteration", transition |-> "UntilConditionMet", actor |-> "loop_engine", step |-> (model_step_count + 1), from_phase |-> loop_iteration_phase, to_phase |-> "Completed"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


loop_iteration_UntilConditionFailed(arg_loop_instance_id, arg_iteration) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "loop_iteration"
       /\ packet.variant = "UntilConditionFailed"
       /\ packet.payload.loop_instance_id = arg_loop_instance_id
       /\ packet.payload.iteration = arg_iteration
       /\ ~HigherPriorityReady("loop_engine")
       /\ loop_iteration_phase = "Running"
       /\ (loop_iteration_loop_instance_id = packet.payload.loop_instance_id)
       /\ (loop_iteration_stage = "AwaitingUntil")
       /\ (loop_iteration_last_completed_iteration = packet.payload.iteration)
       /\ (loop_iteration_current_iteration < loop_iteration_max_iterations)
       /\ loop_iteration_phase' = "Running"
       /\ loop_iteration_stage' = "AwaitingBodyFrame"
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_run", variant |-> "RegisterPendingBodyFrame", payload |-> [depth |-> loop_iteration_depth, loop_instance_id |-> loop_iteration_loop_instance_id], source_kind |-> "route", source_route |-> "loop_request_body_frame_updates_run_pending_queue", source_machine |-> "loop_iteration", source_effect |-> "RequestBodyFrameStart", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "RegisterPendingBodyFrame", payload |-> [depth |-> loop_iteration_depth, loop_instance_id |-> loop_iteration_loop_instance_id], source_kind |-> "route", source_route |-> "loop_request_body_frame_updates_run_pending_queue", source_machine |-> "loop_iteration", source_effect |-> "RequestBodyFrameStart", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "loop_request_body_frame_updates_run_pending_queue", source_machine |-> "loop_iteration", effect |-> "RequestBodyFrameStart", target_machine |-> "flow_run", target_input |-> "RegisterPendingBodyFrame", payload |-> [depth |-> loop_iteration_depth, loop_instance_id |-> loop_iteration_loop_instance_id], actor |-> "run_engine", effect_id |-> (model_step_count + 1), source_transition |-> "UntilConditionFailed"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "loop_iteration", variant |-> "RequestBodyFrameStart", payload |-> [depth |-> loop_iteration_depth, loop_instance_id |-> loop_iteration_loop_instance_id], effect_id |-> (model_step_count + 1), source_transition |-> "UntilConditionFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "loop_iteration", transition |-> "UntilConditionFailed", actor |-> "loop_engine", step |-> (model_step_count + 1), from_phase |-> loop_iteration_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


loop_iteration_ExhaustedIterations(arg_loop_instance_id, arg_iteration) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "loop_iteration"
       /\ packet.variant = "UntilConditionFailed"
       /\ packet.payload.loop_instance_id = arg_loop_instance_id
       /\ packet.payload.iteration = arg_iteration
       /\ ~HigherPriorityReady("loop_engine")
       /\ loop_iteration_phase = "Running"
       /\ (loop_iteration_loop_instance_id = packet.payload.loop_instance_id)
       /\ (loop_iteration_stage = "AwaitingUntil")
       /\ (loop_iteration_last_completed_iteration = packet.payload.iteration)
       /\ (loop_iteration_current_iteration >= loop_iteration_max_iterations)
       /\ loop_iteration_phase' = "Exhausted"
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_frame", variant |-> "FailNode", payload |-> [node_id |-> loop_iteration_parent_node_id], source_kind |-> "route", source_route |-> "loop_exhausted_fails_parent_loop_node", source_machine |-> "loop_iteration", source_effect |-> "LoopExhausted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_frame", variant |-> "FailNode", payload |-> [node_id |-> loop_iteration_parent_node_id], source_kind |-> "route", source_route |-> "loop_exhausted_fails_parent_loop_node", source_machine |-> "loop_iteration", source_effect |-> "LoopExhausted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "loop_exhausted_fails_parent_loop_node", source_machine |-> "loop_iteration", effect |-> "LoopExhausted", target_machine |-> "flow_frame", target_input |-> "FailNode", payload |-> [node_id |-> loop_iteration_parent_node_id], actor |-> "frame_engine", effect_id |-> (model_step_count + 1), source_transition |-> "ExhaustedIterations"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "loop_iteration", variant |-> "LoopExhausted", payload |-> [loop_instance_id |-> loop_iteration_loop_instance_id, parent_frame_id |-> loop_iteration_parent_frame_id, parent_node_id |-> loop_iteration_parent_node_id], effect_id |-> (model_step_count + 1), source_transition |-> "ExhaustedIterations"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "loop_iteration", transition |-> "ExhaustedIterations", actor |-> "loop_engine", step |-> (model_step_count + 1), from_phase |-> loop_iteration_phase, to_phase |-> "Exhausted"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


loop_iteration_BodyFrameFailed(arg_loop_instance_id, arg_iteration) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "loop_iteration"
       /\ packet.variant = "BodyFrameFailed"
       /\ packet.payload.loop_instance_id = arg_loop_instance_id
       /\ packet.payload.iteration = arg_iteration
       /\ ~HigherPriorityReady("loop_engine")
       /\ loop_iteration_phase = "Running"
       /\ (loop_iteration_loop_instance_id = packet.payload.loop_instance_id)
       /\ (loop_iteration_stage = "BodyFrameActive")
       /\ (loop_iteration_current_iteration = packet.payload.iteration)
       /\ loop_iteration_phase' = "Failed"
       /\ loop_iteration_active_body_frame_id' = None
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_frame", variant |-> "FailNode", payload |-> [node_id |-> loop_iteration_parent_node_id], source_kind |-> "route", source_route |-> "loop_failed_fails_parent_loop_node", source_machine |-> "loop_iteration", source_effect |-> "LoopFailed", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_frame", variant |-> "FailNode", payload |-> [node_id |-> loop_iteration_parent_node_id], source_kind |-> "route", source_route |-> "loop_failed_fails_parent_loop_node", source_machine |-> "loop_iteration", source_effect |-> "LoopFailed", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "loop_failed_fails_parent_loop_node", source_machine |-> "loop_iteration", effect |-> "LoopFailed", target_machine |-> "flow_frame", target_input |-> "FailNode", payload |-> [node_id |-> loop_iteration_parent_node_id], actor |-> "frame_engine", effect_id |-> (model_step_count + 1), source_transition |-> "BodyFrameFailed"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "loop_iteration", variant |-> "LoopFailed", payload |-> [loop_instance_id |-> loop_iteration_loop_instance_id, parent_frame_id |-> loop_iteration_parent_frame_id, parent_node_id |-> loop_iteration_parent_node_id], effect_id |-> (model_step_count + 1), source_transition |-> "BodyFrameFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "loop_iteration", transition |-> "BodyFrameFailed", actor |-> "loop_engine", step |-> (model_step_count + 1), from_phase |-> loop_iteration_phase, to_phase |-> "Failed"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


loop_iteration_BodyFrameCanceled(arg_loop_instance_id, arg_iteration) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "loop_iteration"
       /\ packet.variant = "BodyFrameCanceled"
       /\ packet.payload.loop_instance_id = arg_loop_instance_id
       /\ packet.payload.iteration = arg_iteration
       /\ ~HigherPriorityReady("loop_engine")
       /\ loop_iteration_phase = "Running"
       /\ (loop_iteration_loop_instance_id = packet.payload.loop_instance_id)
       /\ (loop_iteration_stage = "BodyFrameActive")
       /\ (loop_iteration_current_iteration = packet.payload.iteration)
       /\ loop_iteration_phase' = "Canceled"
       /\ loop_iteration_active_body_frame_id' = None
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_frame", variant |-> "CancelNode", payload |-> [node_id |-> loop_iteration_parent_node_id], source_kind |-> "route", source_route |-> "loop_canceled_cancels_parent_loop_node", source_machine |-> "loop_iteration", source_effect |-> "LoopCanceled", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_frame", variant |-> "CancelNode", payload |-> [node_id |-> loop_iteration_parent_node_id], source_kind |-> "route", source_route |-> "loop_canceled_cancels_parent_loop_node", source_machine |-> "loop_iteration", source_effect |-> "LoopCanceled", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "loop_canceled_cancels_parent_loop_node", source_machine |-> "loop_iteration", effect |-> "LoopCanceled", target_machine |-> "flow_frame", target_input |-> "CancelNode", payload |-> [node_id |-> loop_iteration_parent_node_id], actor |-> "frame_engine", effect_id |-> (model_step_count + 1), source_transition |-> "BodyFrameCanceled"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "loop_iteration", variant |-> "LoopCanceled", payload |-> [loop_instance_id |-> loop_iteration_loop_instance_id, parent_frame_id |-> loop_iteration_parent_frame_id, parent_node_id |-> loop_iteration_parent_node_id], effect_id |-> (model_step_count + 1), source_transition |-> "BodyFrameCanceled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "loop_iteration", transition |-> "BodyFrameCanceled", actor |-> "loop_engine", step |-> (model_step_count + 1), from_phase |-> loop_iteration_phase, to_phase |-> "Canceled"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


loop_iteration_CancelLoop(arg_loop_instance_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "loop_iteration"
       /\ packet.variant = "CancelLoop"
       /\ packet.payload.loop_instance_id = arg_loop_instance_id
       /\ ~HigherPriorityReady("loop_engine")
       /\ loop_iteration_phase = "Running"
       /\ (loop_iteration_loop_instance_id = packet.payload.loop_instance_id)
       /\ loop_iteration_phase' = "Canceled"
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "flow_frame", variant |-> "CancelNode", payload |-> [node_id |-> loop_iteration_parent_node_id], source_kind |-> "route", source_route |-> "loop_canceled_cancels_parent_loop_node", source_machine |-> "loop_iteration", source_effect |-> "LoopCanceled", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_frame", variant |-> "CancelNode", payload |-> [node_id |-> loop_iteration_parent_node_id], source_kind |-> "route", source_route |-> "loop_canceled_cancels_parent_loop_node", source_machine |-> "loop_iteration", source_effect |-> "LoopCanceled", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "loop_canceled_cancels_parent_loop_node", source_machine |-> "loop_iteration", effect |-> "LoopCanceled", target_machine |-> "flow_frame", target_input |-> "CancelNode", payload |-> [node_id |-> loop_iteration_parent_node_id], actor |-> "frame_engine", effect_id |-> (model_step_count + 1), source_transition |-> "CancelLoop"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "loop_iteration", variant |-> "LoopCanceled", payload |-> [loop_instance_id |-> loop_iteration_loop_instance_id, parent_frame_id |-> loop_iteration_parent_frame_id, parent_node_id |-> loop_iteration_parent_node_id], effect_id |-> (model_step_count + 1), source_transition |-> "CancelLoop"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "loop_iteration", transition |-> "CancelLoop", actor |-> "loop_engine", step |-> (model_step_count + 1), from_phase |-> loop_iteration_phase, to_phase |-> "Canceled"]}
       /\ UNCHANGED << obligation_flow_loop_until_evaluation >>
       /\ model_step_count' = model_step_count + 1


Inject_flow_run_register_ready_frame(arg_frame_id) ==
    /\ ~([machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> arg_frame_id], source_kind |-> "entry", source_route |-> "flow_run_register_ready_frame", source_machine |-> "external_entry", source_effect |-> "RegisterReadyFrame", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> arg_frame_id], source_kind |-> "entry", source_route |-> "flow_run_register_ready_frame", source_machine |-> "external_entry", source_effect |-> "RegisterReadyFrame", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "RegisterReadyFrame", payload |-> [frame_id |-> arg_frame_id], source_kind |-> "entry", source_route |-> "flow_run_register_ready_frame", source_machine |-> "external_entry", source_effect |-> "RegisterReadyFrame", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_flow_run_register_pending_body_frame(arg_loop_instance_id, arg_depth) ==
    /\ ~([machine |-> "flow_run", variant |-> "RegisterPendingBodyFrame", payload |-> [depth |-> arg_depth, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "flow_run_register_pending_body_frame", source_machine |-> "external_entry", source_effect |-> "RegisterPendingBodyFrame", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "flow_run", variant |-> "RegisterPendingBodyFrame", payload |-> [depth |-> arg_depth, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "flow_run_register_pending_body_frame", source_machine |-> "external_entry", source_effect |-> "RegisterPendingBodyFrame", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "RegisterPendingBodyFrame", payload |-> [depth |-> arg_depth, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "flow_run_register_pending_body_frame", source_machine |-> "external_entry", source_effect |-> "RegisterPendingBodyFrame", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_flow_run_node_execution_released(arg_frame_id) ==
    /\ ~([machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> arg_frame_id], source_kind |-> "entry", source_route |-> "flow_run_node_execution_released", source_machine |-> "external_entry", source_effect |-> "NodeExecutionReleased", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> arg_frame_id], source_kind |-> "entry", source_route |-> "flow_run_node_execution_released", source_machine |-> "external_entry", source_effect |-> "NodeExecutionReleased", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "NodeExecutionReleased", payload |-> [frame_id |-> arg_frame_id], source_kind |-> "entry", source_route |-> "flow_run_node_execution_released", source_machine |-> "external_entry", source_effect |-> "NodeExecutionReleased", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_flow_run_frame_terminated(arg_frame_id) ==
    /\ ~([machine |-> "flow_run", variant |-> "FrameTerminated", payload |-> [frame_id |-> arg_frame_id], source_kind |-> "entry", source_route |-> "flow_run_frame_terminated", source_machine |-> "external_entry", source_effect |-> "FrameTerminated", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "flow_run", variant |-> "FrameTerminated", payload |-> [frame_id |-> arg_frame_id], source_kind |-> "entry", source_route |-> "flow_run_frame_terminated", source_machine |-> "external_entry", source_effect |-> "FrameTerminated", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_run", variant |-> "FrameTerminated", payload |-> [frame_id |-> arg_frame_id], source_kind |-> "entry", source_route |-> "flow_run_frame_terminated", source_machine |-> "external_entry", source_effect |-> "FrameTerminated", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_flow_frame_start_root_frame(arg_frame_id, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches) ==
    /\ ~([machine |-> "flow_frame", variant |-> "StartRootFrame", payload |-> [frame_id |-> arg_frame_id, node_branches |-> arg_node_branches, node_dependencies |-> arg_node_dependencies, node_dependency_modes |-> arg_node_dependency_modes, node_kind |-> arg_node_kind, ordered_nodes |-> arg_ordered_nodes, tracked_nodes |-> arg_tracked_nodes], source_kind |-> "entry", source_route |-> "flow_frame_start_root_frame", source_machine |-> "external_entry", source_effect |-> "StartRootFrame", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "flow_frame", variant |-> "StartRootFrame", payload |-> [frame_id |-> arg_frame_id, node_branches |-> arg_node_branches, node_dependencies |-> arg_node_dependencies, node_dependency_modes |-> arg_node_dependency_modes, node_kind |-> arg_node_kind, ordered_nodes |-> arg_ordered_nodes, tracked_nodes |-> arg_tracked_nodes], source_kind |-> "entry", source_route |-> "flow_frame_start_root_frame", source_machine |-> "external_entry", source_effect |-> "StartRootFrame", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_frame", variant |-> "StartRootFrame", payload |-> [frame_id |-> arg_frame_id, node_branches |-> arg_node_branches, node_dependencies |-> arg_node_dependencies, node_dependency_modes |-> arg_node_dependency_modes, node_kind |-> arg_node_kind, ordered_nodes |-> arg_ordered_nodes, tracked_nodes |-> arg_tracked_nodes], source_kind |-> "entry", source_route |-> "flow_frame_start_root_frame", source_machine |-> "external_entry", source_effect |-> "StartRootFrame", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_flow_frame_start_body_frame(arg_frame_id, arg_loop_instance_id, arg_iteration, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches) ==
    /\ ~([machine |-> "flow_frame", variant |-> "StartBodyFrame", payload |-> [frame_id |-> arg_frame_id, iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id, node_branches |-> arg_node_branches, node_dependencies |-> arg_node_dependencies, node_dependency_modes |-> arg_node_dependency_modes, node_kind |-> arg_node_kind, ordered_nodes |-> arg_ordered_nodes, tracked_nodes |-> arg_tracked_nodes], source_kind |-> "entry", source_route |-> "flow_frame_start_body_frame", source_machine |-> "external_entry", source_effect |-> "StartBodyFrame", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "flow_frame", variant |-> "StartBodyFrame", payload |-> [frame_id |-> arg_frame_id, iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id, node_branches |-> arg_node_branches, node_dependencies |-> arg_node_dependencies, node_dependency_modes |-> arg_node_dependency_modes, node_kind |-> arg_node_kind, ordered_nodes |-> arg_ordered_nodes, tracked_nodes |-> arg_tracked_nodes], source_kind |-> "entry", source_route |-> "flow_frame_start_body_frame", source_machine |-> "external_entry", source_effect |-> "StartBodyFrame", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_frame", variant |-> "StartBodyFrame", payload |-> [frame_id |-> arg_frame_id, iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id, node_branches |-> arg_node_branches, node_dependencies |-> arg_node_dependencies, node_dependency_modes |-> arg_node_dependency_modes, node_kind |-> arg_node_kind, ordered_nodes |-> arg_ordered_nodes, tracked_nodes |-> arg_tracked_nodes], source_kind |-> "entry", source_route |-> "flow_frame_start_body_frame", source_machine |-> "external_entry", source_effect |-> "StartBodyFrame", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_flow_frame_complete_node(arg_node_id) ==
    /\ ~([machine |-> "flow_frame", variant |-> "CompleteNode", payload |-> [node_id |-> arg_node_id], source_kind |-> "entry", source_route |-> "flow_frame_complete_node", source_machine |-> "external_entry", source_effect |-> "CompleteNode", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "flow_frame", variant |-> "CompleteNode", payload |-> [node_id |-> arg_node_id], source_kind |-> "entry", source_route |-> "flow_frame_complete_node", source_machine |-> "external_entry", source_effect |-> "CompleteNode", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_frame", variant |-> "CompleteNode", payload |-> [node_id |-> arg_node_id], source_kind |-> "entry", source_route |-> "flow_frame_complete_node", source_machine |-> "external_entry", source_effect |-> "CompleteNode", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_flow_frame_fail_node(arg_node_id) ==
    /\ ~([machine |-> "flow_frame", variant |-> "FailNode", payload |-> [node_id |-> arg_node_id], source_kind |-> "entry", source_route |-> "flow_frame_fail_node", source_machine |-> "external_entry", source_effect |-> "FailNode", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "flow_frame", variant |-> "FailNode", payload |-> [node_id |-> arg_node_id], source_kind |-> "entry", source_route |-> "flow_frame_fail_node", source_machine |-> "external_entry", source_effect |-> "FailNode", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_frame", variant |-> "FailNode", payload |-> [node_id |-> arg_node_id], source_kind |-> "entry", source_route |-> "flow_frame_fail_node", source_machine |-> "external_entry", source_effect |-> "FailNode", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_flow_frame_cancel_node(arg_node_id) ==
    /\ ~([machine |-> "flow_frame", variant |-> "CancelNode", payload |-> [node_id |-> arg_node_id], source_kind |-> "entry", source_route |-> "flow_frame_cancel_node", source_machine |-> "external_entry", source_effect |-> "CancelNode", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "flow_frame", variant |-> "CancelNode", payload |-> [node_id |-> arg_node_id], source_kind |-> "entry", source_route |-> "flow_frame_cancel_node", source_machine |-> "external_entry", source_effect |-> "CancelNode", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_frame", variant |-> "CancelNode", payload |-> [node_id |-> arg_node_id], source_kind |-> "entry", source_route |-> "flow_frame_cancel_node", source_machine |-> "external_entry", source_effect |-> "CancelNode", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_flow_frame_seal_frame ==
    /\ ~([machine |-> "flow_frame", variant |-> "SealFrame", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "flow_frame_seal_frame", source_machine |-> "external_entry", source_effect |-> "SealFrame", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "flow_frame", variant |-> "SealFrame", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "flow_frame_seal_frame", source_machine |-> "external_entry", source_effect |-> "SealFrame", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "flow_frame", variant |-> "SealFrame", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "flow_frame_seal_frame", source_machine |-> "external_entry", source_effect |-> "SealFrame", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_loop_start_loop(arg_loop_instance_id, arg_max_iterations, arg_parent_frame_id, arg_parent_node_id, arg_loop_id, arg_depth) ==
    /\ ~([machine |-> "loop_iteration", variant |-> "StartLoop", payload |-> [depth |-> arg_depth, loop_id |-> arg_loop_id, loop_instance_id |-> arg_loop_instance_id, max_iterations |-> arg_max_iterations, parent_frame_id |-> arg_parent_frame_id, parent_node_id |-> arg_parent_node_id], source_kind |-> "entry", source_route |-> "loop_start_loop", source_machine |-> "external_entry", source_effect |-> "StartLoop", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "loop_iteration", variant |-> "StartLoop", payload |-> [depth |-> arg_depth, loop_id |-> arg_loop_id, loop_instance_id |-> arg_loop_instance_id, max_iterations |-> arg_max_iterations, parent_frame_id |-> arg_parent_frame_id, parent_node_id |-> arg_parent_node_id], source_kind |-> "entry", source_route |-> "loop_start_loop", source_machine |-> "external_entry", source_effect |-> "StartLoop", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "loop_iteration", variant |-> "StartLoop", payload |-> [depth |-> arg_depth, loop_id |-> arg_loop_id, loop_instance_id |-> arg_loop_instance_id, max_iterations |-> arg_max_iterations, parent_frame_id |-> arg_parent_frame_id, parent_node_id |-> arg_parent_node_id], source_kind |-> "entry", source_route |-> "loop_start_loop", source_machine |-> "external_entry", source_effect |-> "StartLoop", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_loop_body_frame_started(arg_loop_instance_id, arg_frame_id, arg_iteration) ==
    /\ ~([machine |-> "loop_iteration", variant |-> "BodyFrameStarted", payload |-> [frame_id |-> arg_frame_id, iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_body_frame_started", source_machine |-> "external_entry", source_effect |-> "BodyFrameStarted", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "loop_iteration", variant |-> "BodyFrameStarted", payload |-> [frame_id |-> arg_frame_id, iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_body_frame_started", source_machine |-> "external_entry", source_effect |-> "BodyFrameStarted", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "loop_iteration", variant |-> "BodyFrameStarted", payload |-> [frame_id |-> arg_frame_id, iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_body_frame_started", source_machine |-> "external_entry", source_effect |-> "BodyFrameStarted", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_loop_body_frame_completed(arg_loop_instance_id, arg_iteration) ==
    /\ ~([machine |-> "loop_iteration", variant |-> "BodyFrameCompleted", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_body_frame_completed", source_machine |-> "external_entry", source_effect |-> "BodyFrameCompleted", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "loop_iteration", variant |-> "BodyFrameCompleted", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_body_frame_completed", source_machine |-> "external_entry", source_effect |-> "BodyFrameCompleted", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "loop_iteration", variant |-> "BodyFrameCompleted", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_body_frame_completed", source_machine |-> "external_entry", source_effect |-> "BodyFrameCompleted", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_loop_body_frame_failed(arg_loop_instance_id, arg_iteration) ==
    /\ ~([machine |-> "loop_iteration", variant |-> "BodyFrameFailed", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_body_frame_failed", source_machine |-> "external_entry", source_effect |-> "BodyFrameFailed", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "loop_iteration", variant |-> "BodyFrameFailed", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_body_frame_failed", source_machine |-> "external_entry", source_effect |-> "BodyFrameFailed", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "loop_iteration", variant |-> "BodyFrameFailed", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_body_frame_failed", source_machine |-> "external_entry", source_effect |-> "BodyFrameFailed", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_loop_body_frame_canceled(arg_loop_instance_id, arg_iteration) ==
    /\ ~([machine |-> "loop_iteration", variant |-> "BodyFrameCanceled", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_body_frame_canceled", source_machine |-> "external_entry", source_effect |-> "BodyFrameCanceled", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "loop_iteration", variant |-> "BodyFrameCanceled", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_body_frame_canceled", source_machine |-> "external_entry", source_effect |-> "BodyFrameCanceled", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "loop_iteration", variant |-> "BodyFrameCanceled", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_body_frame_canceled", source_machine |-> "external_entry", source_effect |-> "BodyFrameCanceled", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_loop_until_condition_met(arg_loop_instance_id, arg_iteration) ==
    /\ ~([machine |-> "loop_iteration", variant |-> "UntilConditionMet", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_until_condition_met", source_machine |-> "external_entry", source_effect |-> "UntilConditionMet", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "loop_iteration", variant |-> "UntilConditionMet", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_until_condition_met", source_machine |-> "external_entry", source_effect |-> "UntilConditionMet", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "loop_iteration", variant |-> "UntilConditionMet", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_until_condition_met", source_machine |-> "external_entry", source_effect |-> "UntilConditionMet", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_loop_until_condition_failed(arg_loop_instance_id, arg_iteration) ==
    /\ ~([machine |-> "loop_iteration", variant |-> "UntilConditionFailed", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_until_condition_failed", source_machine |-> "external_entry", source_effect |-> "UntilConditionFailed", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "loop_iteration", variant |-> "UntilConditionFailed", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_until_condition_failed", source_machine |-> "external_entry", source_effect |-> "UntilConditionFailed", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "loop_iteration", variant |-> "UntilConditionFailed", payload |-> [iteration |-> arg_iteration, loop_instance_id |-> arg_loop_instance_id], source_kind |-> "entry", source_route |-> "loop_until_condition_failed", source_machine |-> "external_entry", source_effect |-> "UntilConditionFailed", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, obligation_flow_loop_until_evaluation, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_flow_frame_loop_route_coverage ==
    FALSE

CoreNext ==
    \/ DeliverQueuedRoute
    \/ \E arg_step_ids \in SeqOfStepIdValues : \E arg_ordered_steps \in SeqOfStepIdValues : \E arg_step_has_conditions \in MapStepIdBoolValues : \E arg_step_dependencies \in MapStepIdSeqStepIdValues : \E arg_step_dependency_modes \in MapStepIdDependencyModeValues : \E arg_step_branches \in MapStepIdOptionBranchIdValues : \E arg_step_collection_policies \in MapStepIdCollectionPolicyKindValues : \E arg_step_quorum_thresholds \in MapStepIdU32Values : \E arg_escalation_threshold \in 0..2 : \E arg_max_step_retries \in 0..2 : \E arg_max_active_nodes \in 0..2 : \E arg_max_active_frames \in 0..2 : \E arg_max_frame_depth \in 0..2 : flow_run_CreateRun(arg_step_ids, arg_ordered_steps, arg_step_has_conditions, arg_step_dependencies, arg_step_dependency_modes, arg_step_branches, arg_step_collection_policies, arg_step_quorum_thresholds, arg_escalation_threshold, arg_max_step_retries, arg_max_active_nodes, arg_max_active_frames, arg_max_frame_depth)
    \/ flow_run_StartRun
    \/ \E arg_step_id \in StepIdValues : flow_run_DispatchStep(arg_step_id)
    \/ \E arg_step_id \in StepIdValues : flow_run_CompleteStep(arg_step_id)
    \/ \E arg_step_id \in StepIdValues : flow_run_RecordStepOutput(arg_step_id)
    \/ \E arg_step_id \in StepIdValues : flow_run_ConditionPassed(arg_step_id)
    \/ \E arg_step_id \in StepIdValues : flow_run_ConditionRejected(arg_step_id)
    \/ \E arg_step_id \in StepIdValues : flow_run_FailStepEscalating(arg_step_id)
    \/ \E arg_step_id \in StepIdValues : flow_run_FailStep(arg_step_id)
    \/ \E arg_step_id \in StepIdValues : flow_run_SkipStep(arg_step_id)
    \/ \E arg_step_id \in StepIdValues : \E arg_step_status \in StepRunStatusValues : \E arg_append_failure_ledger \in BOOLEAN : flow_run_ProjectFrameStepCompleted(arg_step_id, arg_step_status, arg_append_failure_ledger)
    \/ \E arg_step_id \in StepIdValues : \E arg_step_status \in StepRunStatusValues : \E arg_append_failure_ledger \in BOOLEAN : flow_run_ProjectFrameStepSkipped(arg_step_id, arg_step_status, arg_append_failure_ledger)
    \/ \E arg_step_id \in StepIdValues : \E arg_step_status \in StepRunStatusValues : \E arg_append_failure_ledger \in BOOLEAN : flow_run_ProjectFrameStepFailedEscalatingWithLedger(arg_step_id, arg_step_status, arg_append_failure_ledger)
    \/ \E arg_step_id \in StepIdValues : \E arg_step_status \in StepRunStatusValues : \E arg_append_failure_ledger \in BOOLEAN : flow_run_ProjectFrameStepFailedEscalatingWithoutLedger(arg_step_id, arg_step_status, arg_append_failure_ledger)
    \/ \E arg_step_id \in StepIdValues : \E arg_step_status \in StepRunStatusValues : \E arg_append_failure_ledger \in BOOLEAN : flow_run_ProjectFrameStepFailedWithLedger(arg_step_id, arg_step_status, arg_append_failure_ledger)
    \/ \E arg_step_id \in StepIdValues : \E arg_step_status \in StepRunStatusValues : \E arg_append_failure_ledger \in BOOLEAN : flow_run_ProjectFrameStepFailedWithoutLedger(arg_step_id, arg_step_status, arg_append_failure_ledger)
    \/ \E arg_step_id \in StepIdValues : flow_run_CancelStep(arg_step_id)
    \/ \E arg_step_id \in StepIdValues : \E arg_target_count \in 0..2 : flow_run_RegisterTargets(arg_step_id, arg_target_count)
    \/ \E arg_step_id \in StepIdValues : \E arg_target_id \in MeerkatIdValues : flow_run_RecordTargetSuccess(arg_step_id, arg_target_id)
    \/ \E arg_step_id \in StepIdValues : flow_run_RecordTargetTerminalFailure(arg_step_id)
    \/ \E arg_step_id \in StepIdValues : \E arg_target_id \in MeerkatIdValues : flow_run_RecordTargetCanceled(arg_step_id, arg_target_id)
    \/ \E arg_step_id \in StepIdValues : \E arg_target_id \in MeerkatIdValues : \E arg_retry_key \in {"alpha", "beta"} : flow_run_RecordTargetFailure(arg_step_id, arg_target_id, arg_retry_key)
    \/ \E arg_frame_id \in FrameIdValues : flow_run_RegisterReadyFrame(arg_frame_id)
    \/ flow_run_PumpNodeScheduler
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_depth \in 0..2 : flow_run_RegisterPendingBodyFrame(arg_loop_instance_id, arg_depth)
    \/ flow_run_PumpFrameScheduler
    \/ \E arg_frame_id \in FrameIdValues : flow_run_NodeExecutionReleased(arg_frame_id)
    \/ \E arg_frame_id \in FrameIdValues : flow_run_FrameTerminated(arg_frame_id)
    \/ flow_run_TerminalizeCompleted
    \/ flow_run_TerminalizeFailed
    \/ flow_run_TerminalizeCanceled
    \/ \E arg_frame_id \in FrameIdValues : \E arg_tracked_nodes \in SetOfFlowNodeIdValues : \E arg_ordered_nodes \in SeqOfFlowNodeIdValues : \E arg_node_kind \in MapFlowNodeIdFlowNodeKindValues : \E arg_node_dependencies \in MapFlowNodeIdSeqFlowNodeIdValues : \E arg_node_dependency_modes \in MapFlowNodeIdDependencyModeValues : \E arg_node_branches \in MapFlowNodeIdOptionBranchIdValues : flow_frame_StartRootFrame(arg_frame_id, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches)
    \/ \E arg_frame_id \in FrameIdValues : \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : \E arg_tracked_nodes \in SetOfFlowNodeIdValues : \E arg_ordered_nodes \in SeqOfFlowNodeIdValues : \E arg_node_kind \in MapFlowNodeIdFlowNodeKindValues : \E arg_node_dependencies \in MapFlowNodeIdSeqFlowNodeIdValues : \E arg_node_dependency_modes \in MapFlowNodeIdDependencyModeValues : \E arg_node_branches \in MapFlowNodeIdOptionBranchIdValues : flow_frame_StartBodyFrame(arg_frame_id, arg_loop_instance_id, arg_iteration, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches)
    \/ flow_frame_AdmitNextReadyNode_StepRun
    \/ flow_frame_AdmitNextReadyNode_LoopRun
    \/ flow_frame_AdmitNextReadyNode_Skip
    \/ flow_frame_AdmitNextReadyNode_Fail
    \/ \E arg_node_id \in FlowNodeIdValues : flow_frame_CompleteNode_Step(arg_node_id)
    \/ \E arg_node_id \in FlowNodeIdValues : flow_frame_CompleteNode_Loop(arg_node_id)
    \/ \E arg_node_id \in FlowNodeIdValues : flow_frame_RecordNodeOutput(arg_node_id)
    \/ \E arg_node_id \in FlowNodeIdValues : flow_frame_FailNode_Step(arg_node_id)
    \/ \E arg_node_id \in FlowNodeIdValues : flow_frame_FailNode_Loop(arg_node_id)
    \/ \E arg_node_id \in FlowNodeIdValues : flow_frame_SkipNode_Step(arg_node_id)
    \/ \E arg_node_id \in FlowNodeIdValues : flow_frame_SkipNode_Loop(arg_node_id)
    \/ \E arg_node_id \in FlowNodeIdValues : flow_frame_CancelNode_Step(arg_node_id)
    \/ \E arg_node_id \in FlowNodeIdValues : flow_frame_CancelNode_Loop(arg_node_id)
    \/ flow_frame_SealRootFrameCanceled
    \/ flow_frame_SealRootFrameFailed
    \/ flow_frame_SealRootFrameCompleted
    \/ flow_frame_SealBodyFrameCanceled
    \/ flow_frame_SealBodyFrameFailed
    \/ flow_frame_SealBodyFrameCompleted
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_max_iterations \in 0..2 : \E arg_parent_frame_id \in FrameIdValues : \E arg_parent_node_id \in FlowNodeIdValues : \E arg_loop_id \in LoopIdValues : \E arg_depth \in 0..2 : loop_iteration_StartLoop(arg_loop_instance_id, arg_max_iterations, arg_parent_frame_id, arg_parent_node_id, arg_loop_id, arg_depth)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_frame_id \in FrameIdValues : \E arg_iteration \in 0..2 : loop_iteration_BodyFrameStarted(arg_loop_instance_id, arg_frame_id, arg_iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : loop_iteration_BodyFrameCompleted(arg_loop_instance_id, arg_iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : loop_iteration_UntilConditionMet(arg_loop_instance_id, arg_iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : loop_iteration_UntilConditionFailed(arg_loop_instance_id, arg_iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : loop_iteration_ExhaustedIterations(arg_loop_instance_id, arg_iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : loop_iteration_BodyFrameFailed(arg_loop_instance_id, arg_iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : loop_iteration_BodyFrameCanceled(arg_loop_instance_id, arg_iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : loop_iteration_CancelLoop(arg_loop_instance_id)
    \/ QuiescentStutter

InjectNext ==
    \/ \E arg_frame_id \in FrameIdValues : Inject_flow_run_register_ready_frame(arg_frame_id)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_depth \in 0..2 : Inject_flow_run_register_pending_body_frame(arg_loop_instance_id, arg_depth)
    \/ \E arg_frame_id \in FrameIdValues : Inject_flow_run_node_execution_released(arg_frame_id)
    \/ \E arg_frame_id \in FrameIdValues : Inject_flow_run_frame_terminated(arg_frame_id)
    \/ \E arg_frame_id \in FrameIdValues : \E arg_tracked_nodes \in SetOfFlowNodeIdValues : \E arg_ordered_nodes \in SeqOfFlowNodeIdValues : \E arg_node_kind \in MapFlowNodeIdFlowNodeKindValues : \E arg_node_dependencies \in MapFlowNodeIdSeqFlowNodeIdValues : \E arg_node_dependency_modes \in MapFlowNodeIdDependencyModeValues : \E arg_node_branches \in MapFlowNodeIdOptionBranchIdValues : Inject_flow_frame_start_root_frame(arg_frame_id, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches)
    \/ \E arg_frame_id \in FrameIdValues : \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : \E arg_tracked_nodes \in SetOfFlowNodeIdValues : \E arg_ordered_nodes \in SeqOfFlowNodeIdValues : \E arg_node_kind \in MapFlowNodeIdFlowNodeKindValues : \E arg_node_dependencies \in MapFlowNodeIdSeqFlowNodeIdValues : \E arg_node_dependency_modes \in MapFlowNodeIdDependencyModeValues : \E arg_node_branches \in MapFlowNodeIdOptionBranchIdValues : Inject_flow_frame_start_body_frame(arg_frame_id, arg_loop_instance_id, arg_iteration, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches)
    \/ \E arg_node_id \in FlowNodeIdValues : Inject_flow_frame_complete_node(arg_node_id)
    \/ \E arg_node_id \in FlowNodeIdValues : Inject_flow_frame_fail_node(arg_node_id)
    \/ \E arg_node_id \in FlowNodeIdValues : Inject_flow_frame_cancel_node(arg_node_id)
    \/ Inject_flow_frame_seal_frame
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_max_iterations \in 0..2 : \E arg_parent_frame_id \in FrameIdValues : \E arg_parent_node_id \in FlowNodeIdValues : \E arg_loop_id \in LoopIdValues : \E arg_depth \in 0..2 : Inject_loop_start_loop(arg_loop_instance_id, arg_max_iterations, arg_parent_frame_id, arg_parent_node_id, arg_loop_id, arg_depth)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_frame_id \in FrameIdValues : \E arg_iteration \in 0..2 : Inject_loop_body_frame_started(arg_loop_instance_id, arg_frame_id, arg_iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : Inject_loop_body_frame_completed(arg_loop_instance_id, arg_iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : Inject_loop_body_frame_failed(arg_loop_instance_id, arg_iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : Inject_loop_body_frame_canceled(arg_loop_instance_id, arg_iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : Inject_loop_until_condition_met(arg_loop_instance_id, arg_iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : Inject_loop_until_condition_failed(arg_loop_instance_id, arg_iteration)

Next ==
    \/ CoreNext
    \/ InjectNext

WitnessNext_flow_frame_loop_route_coverage ==
    \/ CoreNext
    \/ WitnessInjectNext_flow_frame_loop_route_coverage


flow_loop_until_protocol_covered == TRUE

NoOpenObligationsOnTerminal_flow_loop_until_evaluation == (loop_iteration_phase = "Completed" \/ loop_iteration_phase = "Exhausted" \/ loop_iteration_phase = "Failed" \/ loop_iteration_phase = "Canceled") => obligation_flow_loop_until_evaluation = {}
NoFeedbackWithoutObligation_flow_loop_until_evaluation == \A input_packet \in observed_inputs : ((((input_packet.machine = "loop_iteration" /\ input_packet.variant = "UntilConditionMet")) => ((\E record \in obligation_flow_loop_until_evaluation : (record.loop_instance_id = input_packet.payload.loop_instance_id /\ record.iteration = input_packet.payload.iteration)))) /\ (((input_packet.machine = "loop_iteration" /\ input_packet.variant = "UntilConditionFailed")) => ((\E record \in obligation_flow_loop_until_evaluation : (record.loop_instance_id = input_packet.payload.loop_instance_id /\ record.iteration = input_packet.payload.iteration)))))

\* Liveness: eventual feedback under task-scheduling fairness
OwnerFeedback_flow_loop_until_evaluation ==
    /\ obligation_flow_loop_until_evaluation /= {}
    /\ \E token \in obligation_flow_loop_until_evaluation :
        /\ ((/\ pending_inputs' = Append(pending_inputs, [machine |-> "loop_iteration", variant |-> "UntilConditionMet", source_kind |-> "owner", source_machine |-> "loop_iteration", source_effect |-> "EvaluateUntilCondition", source_route |-> "none", effect_id |-> token, payload |-> [loop_instance_id |-> token.loop_instance_id, iteration |-> token.iteration]]) /\ obligation_flow_loop_until_evaluation' = obligation_flow_loop_until_evaluation \ {token}) \/ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "loop_iteration", variant |-> "UntilConditionFailed", source_kind |-> "owner", source_machine |-> "loop_iteration", source_effect |-> "EvaluateUntilCondition", source_route |-> "none", effect_id |-> token, payload |-> [loop_instance_id |-> token.loop_instance_id, iteration |-> token.iteration]]) /\ obligation_flow_loop_until_evaluation' = obligation_flow_loop_until_evaluation \ {token}))
    /\ UNCHANGED << flow_run_phase, flow_run_tracked_steps, flow_run_ordered_steps, flow_run_step_status, flow_run_output_recorded, flow_run_step_condition_results, flow_run_step_has_conditions, flow_run_step_dependencies, flow_run_step_dependency_modes, flow_run_step_branches, flow_run_step_collection_policies, flow_run_step_quorum_thresholds, flow_run_step_target_counts, flow_run_step_target_success_counts, flow_run_step_target_terminal_failure_counts, flow_run_target_retry_counts, flow_run_failure_count, flow_run_consecutive_failure_count, flow_run_escalation_threshold, flow_run_max_step_retries, flow_run_ready_frames, flow_run_ready_frame_membership, flow_run_pending_body_frame_loops, flow_run_pending_body_frame_loop_membership, flow_run_active_node_count, flow_run_active_frame_count, flow_run_max_active_nodes, flow_run_max_active_frames, flow_run_max_frame_depth, flow_run_last_granted_frame, flow_run_last_granted_loop, flow_frame_phase, flow_frame_frame_id, flow_frame_frame_scope, flow_frame_loop_instance_id, flow_frame_iteration, flow_frame_last_admitted_node, flow_frame_tracked_nodes, flow_frame_ordered_nodes, flow_frame_node_kind, flow_frame_node_dependencies, flow_frame_node_dependency_modes, flow_frame_node_branches, flow_frame_branch_winners, flow_frame_node_status, flow_frame_ready_queue, flow_frame_output_recorded, flow_frame_node_condition_results, loop_iteration_phase, loop_iteration_loop_instance_id, loop_iteration_parent_frame_id, loop_iteration_parent_node_id, loop_iteration_loop_id, loop_iteration_depth, loop_iteration_stage, loop_iteration_current_iteration, loop_iteration_last_completed_iteration, loop_iteration_max_iterations, loop_iteration_active_body_frame_id, model_step_count, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RouteObserved_flow_frame_ready_frontier_updates_run_ready_frames == \E packet \in RoutePackets : packet.route = "flow_frame_ready_frontier_updates_run_ready_frames"
RouteCoverage_flow_frame_ready_frontier_updates_run_ready_frames == (RouteObserved_flow_frame_ready_frontier_updates_run_ready_frames \/ ~RouteObserved_flow_frame_ready_frontier_updates_run_ready_frames)
RouteObserved_flow_frame_node_release_updates_run_slots == \E packet \in RoutePackets : packet.route = "flow_frame_node_release_updates_run_slots"
RouteCoverage_flow_frame_node_release_updates_run_slots == (RouteObserved_flow_frame_node_release_updates_run_slots \/ ~RouteObserved_flow_frame_node_release_updates_run_slots)
RouteObserved_loop_request_body_frame_updates_run_pending_queue == \E packet \in RoutePackets : packet.route = "loop_request_body_frame_updates_run_pending_queue"
RouteCoverage_loop_request_body_frame_updates_run_pending_queue == (RouteObserved_loop_request_body_frame_updates_run_pending_queue \/ ~RouteObserved_loop_request_body_frame_updates_run_pending_queue)
RouteObserved_body_frame_completed_advances_loop_iteration == \E packet \in RoutePackets : packet.route = "body_frame_completed_advances_loop_iteration"
RouteCoverage_body_frame_completed_advances_loop_iteration == (RouteObserved_body_frame_completed_advances_loop_iteration \/ ~RouteObserved_body_frame_completed_advances_loop_iteration)
RouteObserved_body_frame_failed_fails_loop_iteration == \E packet \in RoutePackets : packet.route = "body_frame_failed_fails_loop_iteration"
RouteCoverage_body_frame_failed_fails_loop_iteration == (RouteObserved_body_frame_failed_fails_loop_iteration \/ ~RouteObserved_body_frame_failed_fails_loop_iteration)
RouteObserved_body_frame_canceled_cancels_loop_iteration == \E packet \in RoutePackets : packet.route = "body_frame_canceled_cancels_loop_iteration"
RouteCoverage_body_frame_canceled_cancels_loop_iteration == (RouteObserved_body_frame_canceled_cancels_loop_iteration \/ ~RouteObserved_body_frame_canceled_cancels_loop_iteration)
RouteObserved_loop_completed_completes_parent_loop_node == \E packet \in RoutePackets : packet.route = "loop_completed_completes_parent_loop_node"
RouteCoverage_loop_completed_completes_parent_loop_node == (RouteObserved_loop_completed_completes_parent_loop_node \/ ~RouteObserved_loop_completed_completes_parent_loop_node)
RouteObserved_loop_exhausted_fails_parent_loop_node == \E packet \in RoutePackets : packet.route = "loop_exhausted_fails_parent_loop_node"
RouteCoverage_loop_exhausted_fails_parent_loop_node == (RouteObserved_loop_exhausted_fails_parent_loop_node \/ ~RouteObserved_loop_exhausted_fails_parent_loop_node)
RouteObserved_loop_failed_fails_parent_loop_node == \E packet \in RoutePackets : packet.route = "loop_failed_fails_parent_loop_node"
RouteCoverage_loop_failed_fails_parent_loop_node == (RouteObserved_loop_failed_fails_parent_loop_node \/ ~RouteObserved_loop_failed_fails_parent_loop_node)
RouteObserved_loop_canceled_cancels_parent_loop_node == \E packet \in RoutePackets : packet.route = "loop_canceled_cancels_parent_loop_node"
RouteCoverage_loop_canceled_cancels_parent_loop_node == (RouteObserved_loop_canceled_cancels_parent_loop_node \/ ~RouteObserved_loop_canceled_cancels_parent_loop_node)
CoverageInstrumentation == RouteCoverage_flow_frame_ready_frontier_updates_run_ready_frames /\ RouteCoverage_flow_frame_node_release_updates_run_slots /\ RouteCoverage_loop_request_body_frame_updates_run_pending_queue /\ RouteCoverage_body_frame_completed_advances_loop_iteration /\ RouteCoverage_body_frame_failed_fails_loop_iteration /\ RouteCoverage_body_frame_canceled_cancels_loop_iteration /\ RouteCoverage_loop_completed_completes_parent_loop_node /\ RouteCoverage_loop_exhausted_fails_parent_loop_node /\ RouteCoverage_loop_failed_fails_parent_loop_node /\ RouteCoverage_loop_canceled_cancels_parent_loop_node

CiStateConstraint == /\ model_step_count <= 0 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 1 /\ Cardinality(observed_transitions) <= 0 /\ Cardinality(flow_run_tracked_steps) <= 1 /\ Len(flow_run_ordered_steps) <= 1 /\ Cardinality(DOMAIN flow_run_step_status) <= 1 /\ Cardinality(DOMAIN flow_run_output_recorded) <= 1 /\ Cardinality(DOMAIN flow_run_step_condition_results) <= 1 /\ Cardinality(DOMAIN flow_run_step_has_conditions) <= 1 /\ Cardinality(DOMAIN flow_run_step_dependencies) <= 1 /\ Cardinality(DOMAIN flow_run_step_dependency_modes) <= 1 /\ Cardinality(DOMAIN flow_run_step_branches) <= 1 /\ Cardinality(DOMAIN flow_run_step_collection_policies) <= 1 /\ Cardinality(DOMAIN flow_run_step_quorum_thresholds) <= 1 /\ Cardinality(DOMAIN flow_run_step_target_counts) <= 1 /\ Cardinality(DOMAIN flow_run_step_target_success_counts) <= 1 /\ Cardinality(DOMAIN flow_run_step_target_terminal_failure_counts) <= 1 /\ Cardinality(DOMAIN flow_run_target_retry_counts) <= 1 /\ Len(flow_run_ready_frames) <= 1 /\ Cardinality(flow_run_ready_frame_membership) <= 1 /\ Len(flow_run_pending_body_frame_loops) <= 1 /\ Cardinality(flow_run_pending_body_frame_loop_membership) <= 1 /\ Cardinality(flow_frame_tracked_nodes) <= 1 /\ Len(flow_frame_ordered_nodes) <= 1 /\ Cardinality(DOMAIN flow_frame_node_kind) <= 1 /\ Cardinality(DOMAIN flow_frame_node_dependencies) <= 1 /\ Cardinality(DOMAIN flow_frame_node_dependency_modes) <= 1 /\ Cardinality(DOMAIN flow_frame_node_branches) <= 1 /\ Cardinality(flow_frame_branch_winners) <= 1 /\ Cardinality(DOMAIN flow_frame_node_status) <= 1 /\ Len(flow_frame_ready_queue) <= 1 /\ Cardinality(DOMAIN flow_frame_output_recorded) <= 1 /\ Cardinality(DOMAIN flow_frame_node_condition_results) <= 1
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(flow_run_tracked_steps) <= 2 /\ Len(flow_run_ordered_steps) <= 2 /\ Cardinality(DOMAIN flow_run_step_status) <= 2 /\ Cardinality(DOMAIN flow_run_output_recorded) <= 2 /\ Cardinality(DOMAIN flow_run_step_condition_results) <= 2 /\ Cardinality(DOMAIN flow_run_step_has_conditions) <= 2 /\ Cardinality(DOMAIN flow_run_step_dependencies) <= 2 /\ Cardinality(DOMAIN flow_run_step_dependency_modes) <= 2 /\ Cardinality(DOMAIN flow_run_step_branches) <= 2 /\ Cardinality(DOMAIN flow_run_step_collection_policies) <= 2 /\ Cardinality(DOMAIN flow_run_step_quorum_thresholds) <= 2 /\ Cardinality(DOMAIN flow_run_step_target_counts) <= 2 /\ Cardinality(DOMAIN flow_run_step_target_success_counts) <= 2 /\ Cardinality(DOMAIN flow_run_step_target_terminal_failure_counts) <= 2 /\ Cardinality(DOMAIN flow_run_target_retry_counts) <= 2 /\ Len(flow_run_ready_frames) <= 2 /\ Cardinality(flow_run_ready_frame_membership) <= 2 /\ Len(flow_run_pending_body_frame_loops) <= 2 /\ Cardinality(flow_run_pending_body_frame_loop_membership) <= 2 /\ Cardinality(flow_frame_tracked_nodes) <= 2 /\ Len(flow_frame_ordered_nodes) <= 2 /\ Cardinality(DOMAIN flow_frame_node_kind) <= 2 /\ Cardinality(DOMAIN flow_frame_node_dependencies) <= 2 /\ Cardinality(DOMAIN flow_frame_node_dependency_modes) <= 2 /\ Cardinality(DOMAIN flow_frame_node_branches) <= 2 /\ Cardinality(flow_frame_branch_winners) <= 2 /\ Cardinality(DOMAIN flow_frame_node_status) <= 2 /\ Len(flow_frame_ready_queue) <= 2 /\ Cardinality(DOMAIN flow_frame_output_recorded) <= 2 /\ Cardinality(DOMAIN flow_frame_node_condition_results) <= 2
WitnessStateConstraint_flow_frame_loop_route_coverage == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 13 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 10 /\ Cardinality(emitted_effects) <= 1 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(flow_run_tracked_steps) <= 1 /\ Len(flow_run_ordered_steps) <= 1 /\ Cardinality(DOMAIN flow_run_step_status) <= 1 /\ Cardinality(DOMAIN flow_run_output_recorded) <= 1 /\ Cardinality(DOMAIN flow_run_step_condition_results) <= 1 /\ Cardinality(DOMAIN flow_run_step_has_conditions) <= 1 /\ Cardinality(DOMAIN flow_run_step_dependencies) <= 1 /\ Cardinality(DOMAIN flow_run_step_dependency_modes) <= 1 /\ Cardinality(DOMAIN flow_run_step_branches) <= 1 /\ Cardinality(DOMAIN flow_run_step_collection_policies) <= 1 /\ Cardinality(DOMAIN flow_run_step_quorum_thresholds) <= 1 /\ Cardinality(DOMAIN flow_run_step_target_counts) <= 1 /\ Cardinality(DOMAIN flow_run_step_target_success_counts) <= 1 /\ Cardinality(DOMAIN flow_run_step_target_terminal_failure_counts) <= 1 /\ Cardinality(DOMAIN flow_run_target_retry_counts) <= 1 /\ Len(flow_run_ready_frames) <= 1 /\ Cardinality(flow_run_ready_frame_membership) <= 1 /\ Len(flow_run_pending_body_frame_loops) <= 1 /\ Cardinality(flow_run_pending_body_frame_loop_membership) <= 1 /\ Cardinality(flow_frame_tracked_nodes) <= 1 /\ Len(flow_frame_ordered_nodes) <= 1 /\ Cardinality(DOMAIN flow_frame_node_kind) <= 1 /\ Cardinality(DOMAIN flow_frame_node_dependencies) <= 1 /\ Cardinality(DOMAIN flow_frame_node_dependency_modes) <= 1 /\ Cardinality(DOMAIN flow_frame_node_branches) <= 1 /\ Cardinality(flow_frame_branch_winners) <= 1 /\ Cardinality(DOMAIN flow_frame_node_status) <= 1 /\ Len(flow_frame_ready_queue) <= 1 /\ Cardinality(DOMAIN flow_frame_output_recorded) <= 1 /\ Cardinality(DOMAIN flow_frame_node_condition_results) <= 1

Spec == Init /\ [][Next]_vars
WitnessSpec_flow_frame_loop_route_coverage == WitnessInit_flow_frame_loop_route_coverage /\ [] [WitnessNext_flow_frame_loop_route_coverage]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_step_ids \in SeqOfStepIdValues : \E arg_ordered_steps \in SeqOfStepIdValues : \E arg_step_has_conditions \in MapStepIdBoolValues : \E arg_step_dependencies \in MapStepIdSeqStepIdValues : \E arg_step_dependency_modes \in MapStepIdDependencyModeValues : \E arg_step_branches \in MapStepIdOptionBranchIdValues : \E arg_step_collection_policies \in MapStepIdCollectionPolicyKindValues : \E arg_step_quorum_thresholds \in MapStepIdU32Values : \E arg_escalation_threshold \in 0..2 : \E arg_max_step_retries \in 0..2 : \E arg_max_active_nodes \in 0..2 : \E arg_max_active_frames \in 0..2 : \E arg_max_frame_depth \in 0..2 : flow_run_CreateRun(arg_step_ids, arg_ordered_steps, arg_step_has_conditions, arg_step_dependencies, arg_step_dependency_modes, arg_step_branches, arg_step_collection_policies, arg_step_quorum_thresholds, arg_escalation_threshold, arg_max_step_retries, arg_max_active_nodes, arg_max_active_frames, arg_max_frame_depth)) /\ WF_vars(flow_run_StartRun) /\ WF_vars(\E arg_step_id \in StepIdValues : flow_run_DispatchStep(arg_step_id)) /\ WF_vars(\E arg_step_id \in StepIdValues : flow_run_CompleteStep(arg_step_id)) /\ WF_vars(\E arg_step_id \in StepIdValues : flow_run_RecordStepOutput(arg_step_id)) /\ WF_vars(\E arg_step_id \in StepIdValues : flow_run_ConditionPassed(arg_step_id)) /\ WF_vars(\E arg_step_id \in StepIdValues : flow_run_ConditionRejected(arg_step_id)) /\ WF_vars(\E arg_step_id \in StepIdValues : flow_run_FailStepEscalating(arg_step_id)) /\ WF_vars(\E arg_step_id \in StepIdValues : flow_run_FailStep(arg_step_id)) /\ WF_vars(\E arg_step_id \in StepIdValues : flow_run_SkipStep(arg_step_id)) /\ WF_vars(\E arg_step_id \in StepIdValues : \E arg_step_status \in StepRunStatusValues : \E arg_append_failure_ledger \in BOOLEAN : flow_run_ProjectFrameStepCompleted(arg_step_id, arg_step_status, arg_append_failure_ledger)) /\ WF_vars(\E arg_step_id \in StepIdValues : \E arg_step_status \in StepRunStatusValues : \E arg_append_failure_ledger \in BOOLEAN : flow_run_ProjectFrameStepSkipped(arg_step_id, arg_step_status, arg_append_failure_ledger)) /\ WF_vars(\E arg_step_id \in StepIdValues : \E arg_step_status \in StepRunStatusValues : \E arg_append_failure_ledger \in BOOLEAN : flow_run_ProjectFrameStepFailedEscalatingWithLedger(arg_step_id, arg_step_status, arg_append_failure_ledger)) /\ WF_vars(\E arg_step_id \in StepIdValues : \E arg_step_status \in StepRunStatusValues : \E arg_append_failure_ledger \in BOOLEAN : flow_run_ProjectFrameStepFailedEscalatingWithoutLedger(arg_step_id, arg_step_status, arg_append_failure_ledger)) /\ WF_vars(\E arg_step_id \in StepIdValues : \E arg_step_status \in StepRunStatusValues : \E arg_append_failure_ledger \in BOOLEAN : flow_run_ProjectFrameStepFailedWithLedger(arg_step_id, arg_step_status, arg_append_failure_ledger)) /\ WF_vars(\E arg_step_id \in StepIdValues : \E arg_step_status \in StepRunStatusValues : \E arg_append_failure_ledger \in BOOLEAN : flow_run_ProjectFrameStepFailedWithoutLedger(arg_step_id, arg_step_status, arg_append_failure_ledger)) /\ WF_vars(\E arg_step_id \in StepIdValues : flow_run_CancelStep(arg_step_id)) /\ WF_vars(\E arg_step_id \in StepIdValues : \E arg_target_count \in 0..2 : flow_run_RegisterTargets(arg_step_id, arg_target_count)) /\ WF_vars(\E arg_step_id \in StepIdValues : \E arg_target_id \in MeerkatIdValues : flow_run_RecordTargetSuccess(arg_step_id, arg_target_id)) /\ WF_vars(\E arg_step_id \in StepIdValues : flow_run_RecordTargetTerminalFailure(arg_step_id)) /\ WF_vars(\E arg_step_id \in StepIdValues : \E arg_target_id \in MeerkatIdValues : flow_run_RecordTargetCanceled(arg_step_id, arg_target_id)) /\ WF_vars(\E arg_step_id \in StepIdValues : \E arg_target_id \in MeerkatIdValues : \E arg_retry_key \in {"alpha", "beta"} : flow_run_RecordTargetFailure(arg_step_id, arg_target_id, arg_retry_key)) /\ WF_vars(\E arg_frame_id \in FrameIdValues : flow_run_RegisterReadyFrame(arg_frame_id)) /\ WF_vars(flow_run_PumpNodeScheduler) /\ WF_vars(\E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_depth \in 0..2 : flow_run_RegisterPendingBodyFrame(arg_loop_instance_id, arg_depth)) /\ WF_vars(flow_run_PumpFrameScheduler) /\ WF_vars(\E arg_frame_id \in FrameIdValues : flow_run_NodeExecutionReleased(arg_frame_id)) /\ WF_vars(\E arg_frame_id \in FrameIdValues : flow_run_FrameTerminated(arg_frame_id)) /\ WF_vars(flow_run_TerminalizeCompleted) /\ WF_vars(flow_run_TerminalizeFailed) /\ WF_vars(flow_run_TerminalizeCanceled) /\ WF_vars(\E arg_frame_id \in FrameIdValues : \E arg_tracked_nodes \in SetOfFlowNodeIdValues : \E arg_ordered_nodes \in SeqOfFlowNodeIdValues : \E arg_node_kind \in MapFlowNodeIdFlowNodeKindValues : \E arg_node_dependencies \in MapFlowNodeIdSeqFlowNodeIdValues : \E arg_node_dependency_modes \in MapFlowNodeIdDependencyModeValues : \E arg_node_branches \in MapFlowNodeIdOptionBranchIdValues : flow_frame_StartRootFrame(arg_frame_id, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches)) /\ WF_vars(\E arg_frame_id \in FrameIdValues : \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : \E arg_tracked_nodes \in SetOfFlowNodeIdValues : \E arg_ordered_nodes \in SeqOfFlowNodeIdValues : \E arg_node_kind \in MapFlowNodeIdFlowNodeKindValues : \E arg_node_dependencies \in MapFlowNodeIdSeqFlowNodeIdValues : \E arg_node_dependency_modes \in MapFlowNodeIdDependencyModeValues : \E arg_node_branches \in MapFlowNodeIdOptionBranchIdValues : flow_frame_StartBodyFrame(arg_frame_id, arg_loop_instance_id, arg_iteration, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches)) /\ WF_vars(flow_frame_AdmitNextReadyNode_StepRun) /\ WF_vars(flow_frame_AdmitNextReadyNode_LoopRun) /\ WF_vars(flow_frame_AdmitNextReadyNode_Skip) /\ WF_vars(flow_frame_AdmitNextReadyNode_Fail) /\ WF_vars(\E arg_node_id \in FlowNodeIdValues : flow_frame_CompleteNode_Step(arg_node_id)) /\ WF_vars(\E arg_node_id \in FlowNodeIdValues : flow_frame_CompleteNode_Loop(arg_node_id)) /\ WF_vars(\E arg_node_id \in FlowNodeIdValues : flow_frame_RecordNodeOutput(arg_node_id)) /\ WF_vars(\E arg_node_id \in FlowNodeIdValues : flow_frame_FailNode_Step(arg_node_id)) /\ WF_vars(\E arg_node_id \in FlowNodeIdValues : flow_frame_FailNode_Loop(arg_node_id)) /\ WF_vars(\E arg_node_id \in FlowNodeIdValues : flow_frame_SkipNode_Step(arg_node_id)) /\ WF_vars(\E arg_node_id \in FlowNodeIdValues : flow_frame_SkipNode_Loop(arg_node_id)) /\ WF_vars(\E arg_node_id \in FlowNodeIdValues : flow_frame_CancelNode_Step(arg_node_id)) /\ WF_vars(\E arg_node_id \in FlowNodeIdValues : flow_frame_CancelNode_Loop(arg_node_id)) /\ WF_vars(flow_frame_SealRootFrameCanceled) /\ WF_vars(flow_frame_SealRootFrameFailed) /\ WF_vars(flow_frame_SealRootFrameCompleted) /\ WF_vars(flow_frame_SealBodyFrameCanceled) /\ WF_vars(flow_frame_SealBodyFrameFailed) /\ WF_vars(flow_frame_SealBodyFrameCompleted) /\ WF_vars(\E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_max_iterations \in 0..2 : \E arg_parent_frame_id \in FrameIdValues : \E arg_parent_node_id \in FlowNodeIdValues : \E arg_loop_id \in LoopIdValues : \E arg_depth \in 0..2 : loop_iteration_StartLoop(arg_loop_instance_id, arg_max_iterations, arg_parent_frame_id, arg_parent_node_id, arg_loop_id, arg_depth)) /\ WF_vars(\E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_frame_id \in FrameIdValues : \E arg_iteration \in 0..2 : loop_iteration_BodyFrameStarted(arg_loop_instance_id, arg_frame_id, arg_iteration)) /\ WF_vars(\E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : loop_iteration_BodyFrameCompleted(arg_loop_instance_id, arg_iteration)) /\ WF_vars(\E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : loop_iteration_UntilConditionMet(arg_loop_instance_id, arg_iteration)) /\ WF_vars(\E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : loop_iteration_UntilConditionFailed(arg_loop_instance_id, arg_iteration)) /\ WF_vars(\E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : loop_iteration_ExhaustedIterations(arg_loop_instance_id, arg_iteration)) /\ WF_vars(\E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : loop_iteration_BodyFrameFailed(arg_loop_instance_id, arg_iteration)) /\ WF_vars(\E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : loop_iteration_BodyFrameCanceled(arg_loop_instance_id, arg_iteration)) /\ WF_vars(\E arg_loop_instance_id \in LoopInstanceIdValues : loop_iteration_CancelLoop(arg_loop_instance_id))

WitnessRouteObserved_flow_frame_loop_route_coverage_flow_frame_ready_frontier_updates_run_ready_frames == <> RouteObserved_flow_frame_ready_frontier_updates_run_ready_frames
WitnessRouteObserved_flow_frame_loop_route_coverage_flow_frame_node_release_updates_run_slots == <> RouteObserved_flow_frame_node_release_updates_run_slots
WitnessRouteObserved_flow_frame_loop_route_coverage_loop_request_body_frame_updates_run_pending_queue == <> RouteObserved_loop_request_body_frame_updates_run_pending_queue
WitnessRouteObserved_flow_frame_loop_route_coverage_body_frame_completed_advances_loop_iteration == <> RouteObserved_body_frame_completed_advances_loop_iteration
WitnessRouteObserved_flow_frame_loop_route_coverage_body_frame_failed_fails_loop_iteration == <> RouteObserved_body_frame_failed_fails_loop_iteration
WitnessRouteObserved_flow_frame_loop_route_coverage_body_frame_canceled_cancels_loop_iteration == <> RouteObserved_body_frame_canceled_cancels_loop_iteration
WitnessRouteObserved_flow_frame_loop_route_coverage_loop_completed_completes_parent_loop_node == <> RouteObserved_loop_completed_completes_parent_loop_node
WitnessRouteObserved_flow_frame_loop_route_coverage_loop_exhausted_fails_parent_loop_node == <> RouteObserved_loop_exhausted_fails_parent_loop_node
WitnessRouteObserved_flow_frame_loop_route_coverage_loop_failed_fails_parent_loop_node == <> RouteObserved_loop_failed_fails_parent_loop_node
WitnessRouteObserved_flow_frame_loop_route_coverage_loop_canceled_cancels_parent_loop_node == <> RouteObserved_loop_canceled_cancels_parent_loop_node

THEOREM Spec => []flow_loop_until_protocol_covered
THEOREM Spec => []flow_run_output_only_follows_completed_steps
THEOREM Spec => []flow_run_terminal_runs_have_no_dispatched_steps
THEOREM Spec => []flow_run_completed_runs_contain_only_completed_or_skipped_steps
THEOREM Spec => []flow_run_failed_step_presence_requires_failure_count
THEOREM Spec => []flow_run_failed_run_has_failed_step_or_recorded_failure
THEOREM Spec => []flow_frame_ready_queue_membership_matches_ready_status
THEOREM Spec => []NoOpenObligationsOnTerminal_flow_loop_until_evaluation
THEOREM Spec => []NoFeedbackWithoutObligation_flow_loop_until_evaluation

=============================================================================
