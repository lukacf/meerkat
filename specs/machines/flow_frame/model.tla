---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for FlowFrameMachine.

CONSTANTS BranchIdValues, DependencyModeValues, FlowNodeIdValues, FlowNodeKindValues, FrameIdValues, LoopInstanceIdValues, NatValues, SetOfFlowNodeIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

SeqOfFlowNodeIdValues == {<<>>} \cup {<<x>> : x \in FlowNodeIdValues} \cup {<<x, y>> : x \in FlowNodeIdValues, y \in FlowNodeIdValues}
OptionBranchIdValues == {None} \cup {Some(x) : x \in BranchIdValues}
MapFlowNodeIdDependencyModeValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in FlowNodeIdValues, v \in DependencyModeValues }
MapFlowNodeIdFlowNodeKindValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in FlowNodeIdValues, v \in FlowNodeKindValues }
MapFlowNodeIdOptionBranchIdValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in FlowNodeIdValues, v \in OptionBranchIdValues }
MapFlowNodeIdSeqFlowNodeIdValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in FlowNodeIdValues, v \in SeqOfFlowNodeIdValues }

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, node_status, ready_queue, output_recorded, node_condition_results

vars == << phase, model_step_count, frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, node_status, ready_queue, output_recorded, node_condition_results >>

AllNodesTerminal == (\A t_node \in tracked_nodes : (((IF t_node \in DOMAIN node_status THEN node_status[t_node] ELSE "None") = "Completed") \/ ((IF t_node \in DOMAIN node_status THEN node_status[t_node] ELSE "None") = "Failed") \/ ((IF t_node \in DOMAIN node_status THEN node_status[t_node] ELSE "None") = "Skipped") \/ ((IF t_node \in DOMAIN node_status THEN node_status[t_node] ELSE "None") = "Canceled")))
AnyDepCompleted(node_id) == (\E dep_id \in (IF node_id \in DOMAIN node_dependencies THEN node_dependencies[node_id] ELSE <<>>) : ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Completed"))
AllDepsCompleted(node_id) == (\A dep_id \in (IF node_id \in DOMAIN node_dependencies THEN node_dependencies[node_id] ELSE <<>>) : ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Completed"))
NodeAdmissionEligible(node_id) == (IF (Len((IF node_id \in DOMAIN node_dependencies THEN node_dependencies[node_id] ELSE <<>>)) = 0) THEN TRUE ELSE (IF ((IF node_id \in DOMAIN node_dependency_modes THEN node_dependency_modes[node_id] ELSE "None") = "All") THEN (\A dep_id \in (IF node_id \in DOMAIN node_dependencies THEN node_dependencies[node_id] ELSE <<>>) : (((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Completed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Failed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Skipped") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Canceled"))) ELSE ((\E dep_id \in (IF node_id \in DOMAIN node_dependencies THEN node_dependencies[node_id] ELSE <<>>) : ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Completed")) \/ (\A dep_id \in (IF node_id \in DOMAIN node_dependencies THEN node_dependencies[node_id] ELSE <<>>) : (((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Completed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Failed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Skipped") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Canceled"))))))

Init ==
    /\ phase = "Absent"
    /\ model_step_count = 0
    /\ frame_id = ""
    /\ frame_scope = "Root"
    /\ loop_instance_id = ""
    /\ iteration = 0
    /\ last_admitted_node = ""
    /\ tracked_nodes = {}
    /\ ordered_nodes = <<>>
    /\ node_kind = [x \in {} |-> None]
    /\ node_dependencies = [x \in {} |-> None]
    /\ node_dependency_modes = [x \in {} |-> None]
    /\ node_branches = [x \in {} |-> None]
    /\ branch_winners = {}
    /\ node_status = [x \in {} |-> None]
    /\ ready_queue = <<>>
    /\ output_recorded = [x \in {} |-> None]
    /\ node_condition_results = [x \in {} |-> None]

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Failed" \/ phase = "Canceled"
    /\ UNCHANGED vars

RECURSIVE StartRootFrame_ForEach0_node_status(_, _)
StartRootFrame_ForEach0_node_status(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET init_node == item IN LET next_acc == MapSet(acc, init_node, "Pending") IN StartRootFrame_ForEach0_node_status(next_acc, remaining \ {item})

RECURSIVE StartRootFrame_ForEach1_node_status(_, _)
StartRootFrame_ForEach1_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN StartRootFrame_ForEach1_node_status(next_acc, Tail(items))

RECURSIVE StartRootFrame_ForEach2_ready_queue(_, _, _)
StartRootFrame_ForEach2_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN StartRootFrame_ForEach2_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE StartBodyFrame_ForEach3_node_status(_, _)
StartBodyFrame_ForEach3_node_status(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET init_node == item IN LET next_acc == MapSet(acc, init_node, "Pending") IN StartBodyFrame_ForEach3_node_status(next_acc, remaining \ {item})

RECURSIVE StartBodyFrame_ForEach4_node_status(_, _)
StartBodyFrame_ForEach4_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN StartBodyFrame_ForEach4_node_status(next_acc, Tail(items))

RECURSIVE StartBodyFrame_ForEach5_ready_queue(_, _, _)
StartBodyFrame_ForEach5_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN StartBodyFrame_ForEach5_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE AdmitNextReadyNode_Skip_ForEach6_node_status(_, _)
AdmitNextReadyNode_Skip_ForEach6_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN AdmitNextReadyNode_Skip_ForEach6_node_status(next_acc, Tail(items))

RECURSIVE AdmitNextReadyNode_Skip_ForEach7_ready_queue(_, _, _)
AdmitNextReadyNode_Skip_ForEach7_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN AdmitNextReadyNode_Skip_ForEach7_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE AdmitNextReadyNode_Fail_ForEach8_node_status(_, _)
AdmitNextReadyNode_Fail_ForEach8_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN AdmitNextReadyNode_Fail_ForEach8_node_status(next_acc, Tail(items))

RECURSIVE AdmitNextReadyNode_Fail_ForEach9_ready_queue(_, _, _)
AdmitNextReadyNode_Fail_ForEach9_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN AdmitNextReadyNode_Fail_ForEach9_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE CompleteNode_Step_ForEach10_node_status(_, _)
CompleteNode_Step_ForEach10_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN CompleteNode_Step_ForEach10_node_status(next_acc, Tail(items))

RECURSIVE CompleteNode_Step_ForEach11_ready_queue(_, _, _)
CompleteNode_Step_ForEach11_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN CompleteNode_Step_ForEach11_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE CompleteNode_Loop_ForEach12_node_status(_, _)
CompleteNode_Loop_ForEach12_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN CompleteNode_Loop_ForEach12_node_status(next_acc, Tail(items))

RECURSIVE CompleteNode_Loop_ForEach13_ready_queue(_, _, _)
CompleteNode_Loop_ForEach13_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN CompleteNode_Loop_ForEach13_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE FailNode_Step_ForEach14_node_status(_, _)
FailNode_Step_ForEach14_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN FailNode_Step_ForEach14_node_status(next_acc, Tail(items))

RECURSIVE FailNode_Step_ForEach15_ready_queue(_, _, _)
FailNode_Step_ForEach15_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN FailNode_Step_ForEach15_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE FailNode_Loop_ForEach16_node_status(_, _)
FailNode_Loop_ForEach16_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN FailNode_Loop_ForEach16_node_status(next_acc, Tail(items))

RECURSIVE FailNode_Loop_ForEach17_ready_queue(_, _, _)
FailNode_Loop_ForEach17_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN FailNode_Loop_ForEach17_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE SkipNode_Step_ForEach18_node_status(_, _)
SkipNode_Step_ForEach18_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN SkipNode_Step_ForEach18_node_status(next_acc, Tail(items))

RECURSIVE SkipNode_Step_ForEach19_ready_queue(_, _, _)
SkipNode_Step_ForEach19_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN SkipNode_Step_ForEach19_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE SkipNode_Loop_ForEach20_node_status(_, _)
SkipNode_Loop_ForEach20_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN SkipNode_Loop_ForEach20_node_status(next_acc, Tail(items))

RECURSIVE SkipNode_Loop_ForEach21_ready_queue(_, _, _)
SkipNode_Loop_ForEach21_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN SkipNode_Loop_ForEach21_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE CancelNode_Step_ForEach22_node_status(_, _)
CancelNode_Step_ForEach22_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN CancelNode_Step_ForEach22_node_status(next_acc, Tail(items))

RECURSIVE CancelNode_Step_ForEach23_ready_queue(_, _, _)
CancelNode_Step_ForEach23_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN CancelNode_Step_ForEach23_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE CancelNode_Loop_ForEach24_node_status(_, _)
CancelNode_Loop_ForEach24_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN CancelNode_Loop_ForEach24_node_status(next_acc, Tail(items))

RECURSIVE CancelNode_Loop_ForEach25_ready_queue(_, _, _)
CancelNode_Loop_ForEach25_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN CancelNode_Loop_ForEach25_ready_queue(next_acc, Tail(items), captured_node_status)

StartRootFrame(arg_frame_id, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches) ==
    /\ phase = "Absent"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ frame_id' = arg_frame_id
    /\ frame_scope' = "Root"
    /\ loop_instance_id' = ""
    /\ iteration' = 0
    /\ tracked_nodes' = arg_tracked_nodes
    /\ ordered_nodes' = arg_ordered_nodes
    /\ node_kind' = arg_node_kind
    /\ node_dependencies' = arg_node_dependencies
    /\ node_dependency_modes' = arg_node_dependency_modes
    /\ node_branches' = arg_node_branches
    /\ node_status' = StartRootFrame_ForEach1_node_status(StartRootFrame_ForEach0_node_status(node_status, arg_tracked_nodes), arg_ordered_nodes)
    /\ ready_queue' = StartRootFrame_ForEach2_ready_queue(ready_queue, arg_ordered_nodes, StartRootFrame_ForEach1_node_status(StartRootFrame_ForEach0_node_status(node_status, arg_tracked_nodes), arg_ordered_nodes))
    /\ UNCHANGED << last_admitted_node, branch_winners, output_recorded, node_condition_results >>


StartBodyFrame(arg_frame_id, arg_loop_instance_id, arg_iteration, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches) ==
    /\ phase = "Absent"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ frame_id' = arg_frame_id
    /\ frame_scope' = "Body"
    /\ loop_instance_id' = arg_loop_instance_id
    /\ iteration' = arg_iteration
    /\ tracked_nodes' = arg_tracked_nodes
    /\ ordered_nodes' = arg_ordered_nodes
    /\ node_kind' = arg_node_kind
    /\ node_dependencies' = arg_node_dependencies
    /\ node_dependency_modes' = arg_node_dependency_modes
    /\ node_branches' = arg_node_branches
    /\ node_status' = StartBodyFrame_ForEach4_node_status(StartBodyFrame_ForEach3_node_status(node_status, arg_tracked_nodes), arg_ordered_nodes)
    /\ ready_queue' = StartBodyFrame_ForEach5_ready_queue(ready_queue, arg_ordered_nodes, StartBodyFrame_ForEach4_node_status(StartBodyFrame_ForEach3_node_status(node_status, arg_tracked_nodes), arg_ordered_nodes))
    /\ UNCHANGED << last_admitted_node, branch_winners, output_recorded, node_condition_results >>


AdmitNextReadyNode_StepRun ==
    /\ phase = "Running"
    /\ (Len(ready_queue) > 0)
    /\ ((IF Head(ready_queue) \in DOMAIN node_kind THEN node_kind[Head(ready_queue)] ELSE "None") = "Step")
    /\ (~(((IF Head(ready_queue) \in DOMAIN node_branches THEN node_branches[Head(ready_queue)] ELSE None) \in branch_winners)) /\ ((Len((IF Head(ready_queue) \in DOMAIN node_dependencies THEN node_dependencies[Head(ready_queue)] ELSE <<>>)) = 0) \/ (((IF Head(ready_queue) \in DOMAIN node_dependency_modes THEN node_dependency_modes[Head(ready_queue)] ELSE "None") = "All") /\ AllDepsCompleted(Head(ready_queue))) \/ (((IF Head(ready_queue) \in DOMAIN node_dependency_modes THEN node_dependency_modes[Head(ready_queue)] ELSE "None") = "Any") /\ AnyDepCompleted(Head(ready_queue)))))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ last_admitted_node' = Head(ready_queue)
    /\ node_status' = MapSet(node_status, Head(ready_queue), "Running")
    /\ ready_queue' = Tail(ready_queue)
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, output_recorded, node_condition_results >>


AdmitNextReadyNode_LoopRun ==
    /\ phase = "Running"
    /\ (Len(ready_queue) > 0)
    /\ ((IF Head(ready_queue) \in DOMAIN node_kind THEN node_kind[Head(ready_queue)] ELSE "None") = "Loop")
    /\ (~(((IF Head(ready_queue) \in DOMAIN node_branches THEN node_branches[Head(ready_queue)] ELSE None) \in branch_winners)) /\ ((Len((IF Head(ready_queue) \in DOMAIN node_dependencies THEN node_dependencies[Head(ready_queue)] ELSE <<>>)) = 0) \/ (((IF Head(ready_queue) \in DOMAIN node_dependency_modes THEN node_dependency_modes[Head(ready_queue)] ELSE "None") = "All") /\ AllDepsCompleted(Head(ready_queue))) \/ (((IF Head(ready_queue) \in DOMAIN node_dependency_modes THEN node_dependency_modes[Head(ready_queue)] ELSE "None") = "Any") /\ AnyDepCompleted(Head(ready_queue)))))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ last_admitted_node' = Head(ready_queue)
    /\ node_status' = MapSet(node_status, Head(ready_queue), "Running")
    /\ ready_queue' = Tail(ready_queue)
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, output_recorded, node_condition_results >>


AdmitNextReadyNode_Skip ==
    /\ phase = "Running"
    /\ (Len(ready_queue) > 0)
    /\ ((((IF Head(ready_queue) \in DOMAIN node_dependency_modes THEN node_dependency_modes[Head(ready_queue)] ELSE "None") = "All") /\ (\E dep_id \in (IF Head(ready_queue) \in DOMAIN node_dependencies THEN node_dependencies[Head(ready_queue)] ELSE <<>>) : (((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Failed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Skipped") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Canceled")))) \/ ((IF Head(ready_queue) \in DOMAIN node_branches THEN node_branches[Head(ready_queue)] ELSE None) \in branch_winners))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ last_admitted_node' = Head(ready_queue)
    /\ node_status' = AdmitNextReadyNode_Skip_ForEach6_node_status(MapSet(node_status, Head(ready_queue), "Skipped"), ordered_nodes)
    /\ ready_queue' = AdmitNextReadyNode_Skip_ForEach7_ready_queue(Tail(ready_queue), ordered_nodes, AdmitNextReadyNode_Skip_ForEach6_node_status(MapSet(node_status, Head(ready_queue), "Skipped"), ordered_nodes))
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, output_recorded, node_condition_results >>


AdmitNextReadyNode_Fail ==
    /\ phase = "Running"
    /\ (Len(ready_queue) > 0)
    /\ (((IF Head(ready_queue) \in DOMAIN node_dependency_modes THEN node_dependency_modes[Head(ready_queue)] ELSE "None") = "Any") /\ (\A dep_id \in (IF Head(ready_queue) \in DOMAIN node_dependencies THEN node_dependencies[Head(ready_queue)] ELSE <<>>) : (((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Completed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Failed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Skipped") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Canceled"))) /\ ~(AnyDepCompleted(Head(ready_queue))))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ last_admitted_node' = Head(ready_queue)
    /\ node_status' = AdmitNextReadyNode_Fail_ForEach8_node_status(MapSet(node_status, Head(ready_queue), "Failed"), ordered_nodes)
    /\ ready_queue' = AdmitNextReadyNode_Fail_ForEach9_ready_queue(Tail(ready_queue), ordered_nodes, AdmitNextReadyNode_Fail_ForEach8_node_status(MapSet(node_status, Head(ready_queue), "Failed"), ordered_nodes))
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, output_recorded, node_condition_results >>


CompleteNode_Step(node_id) ==
    /\ phase = "Running"
    /\ ((IF node_id \in DOMAIN node_status THEN node_status[node_id] ELSE "None") = "Running")
    /\ ((IF node_id \in DOMAIN node_kind THEN node_kind[node_id] ELSE "None") = "Step")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ branch_winners' = IF ((IF node_id \in DOMAIN node_branches THEN node_branches[node_id] ELSE None) # None) THEN (branch_winners \cup {(IF node_id \in DOMAIN node_branches THEN node_branches[node_id] ELSE None)}) ELSE branch_winners
    /\ node_status' = CompleteNode_Step_ForEach10_node_status(MapSet(node_status, node_id, "Completed"), ordered_nodes)
    /\ ready_queue' = CompleteNode_Step_ForEach11_ready_queue(ready_queue, ordered_nodes, CompleteNode_Step_ForEach10_node_status(MapSet(node_status, node_id, "Completed"), ordered_nodes))
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, output_recorded, node_condition_results >>


CompleteNode_Loop(node_id) ==
    /\ phase = "Running"
    /\ ((IF node_id \in DOMAIN node_status THEN node_status[node_id] ELSE "None") = "Running")
    /\ ((IF node_id \in DOMAIN node_kind THEN node_kind[node_id] ELSE "None") = "Loop")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ branch_winners' = IF ((IF node_id \in DOMAIN node_branches THEN node_branches[node_id] ELSE None) # None) THEN (branch_winners \cup {(IF node_id \in DOMAIN node_branches THEN node_branches[node_id] ELSE None)}) ELSE branch_winners
    /\ node_status' = CompleteNode_Loop_ForEach12_node_status(MapSet(node_status, node_id, "Completed"), ordered_nodes)
    /\ ready_queue' = CompleteNode_Loop_ForEach13_ready_queue(ready_queue, ordered_nodes, CompleteNode_Loop_ForEach12_node_status(MapSet(node_status, node_id, "Completed"), ordered_nodes))
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, output_recorded, node_condition_results >>


RecordNodeOutput(node_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ output_recorded' = MapSet(output_recorded, node_id, TRUE)
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, node_status, ready_queue, node_condition_results >>


FailNode_Step(node_id) ==
    /\ phase = "Running"
    /\ ((IF node_id \in DOMAIN node_status THEN node_status[node_id] ELSE "None") = "Running")
    /\ ((IF node_id \in DOMAIN node_kind THEN node_kind[node_id] ELSE "None") = "Step")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ node_status' = FailNode_Step_ForEach14_node_status(MapSet(node_status, node_id, "Failed"), ordered_nodes)
    /\ ready_queue' = FailNode_Step_ForEach15_ready_queue(ready_queue, ordered_nodes, FailNode_Step_ForEach14_node_status(MapSet(node_status, node_id, "Failed"), ordered_nodes))
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, output_recorded, node_condition_results >>


FailNode_Loop(node_id) ==
    /\ phase = "Running"
    /\ ((IF node_id \in DOMAIN node_status THEN node_status[node_id] ELSE "None") = "Running")
    /\ ((IF node_id \in DOMAIN node_kind THEN node_kind[node_id] ELSE "None") = "Loop")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ node_status' = FailNode_Loop_ForEach16_node_status(MapSet(node_status, node_id, "Failed"), ordered_nodes)
    /\ ready_queue' = FailNode_Loop_ForEach17_ready_queue(ready_queue, ordered_nodes, FailNode_Loop_ForEach16_node_status(MapSet(node_status, node_id, "Failed"), ordered_nodes))
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, output_recorded, node_condition_results >>


SkipNode_Step(node_id) ==
    /\ phase = "Running"
    /\ ((IF node_id \in DOMAIN node_status THEN node_status[node_id] ELSE "None") = "Running")
    /\ ((IF node_id \in DOMAIN node_kind THEN node_kind[node_id] ELSE "None") = "Step")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ node_status' = SkipNode_Step_ForEach18_node_status(MapSet(node_status, node_id, "Skipped"), ordered_nodes)
    /\ ready_queue' = SkipNode_Step_ForEach19_ready_queue(ready_queue, ordered_nodes, SkipNode_Step_ForEach18_node_status(MapSet(node_status, node_id, "Skipped"), ordered_nodes))
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, output_recorded, node_condition_results >>


SkipNode_Loop(node_id) ==
    /\ phase = "Running"
    /\ ((IF node_id \in DOMAIN node_status THEN node_status[node_id] ELSE "None") = "Running")
    /\ ((IF node_id \in DOMAIN node_kind THEN node_kind[node_id] ELSE "None") = "Loop")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ node_status' = SkipNode_Loop_ForEach20_node_status(MapSet(node_status, node_id, "Skipped"), ordered_nodes)
    /\ ready_queue' = SkipNode_Loop_ForEach21_ready_queue(ready_queue, ordered_nodes, SkipNode_Loop_ForEach20_node_status(MapSet(node_status, node_id, "Skipped"), ordered_nodes))
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, output_recorded, node_condition_results >>


CancelNode_Step(node_id) ==
    /\ phase = "Running"
    /\ ((IF node_id \in DOMAIN node_status THEN node_status[node_id] ELSE "None") = "Running")
    /\ ((IF node_id \in DOMAIN node_kind THEN node_kind[node_id] ELSE "None") = "Step")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ node_status' = CancelNode_Step_ForEach22_node_status(MapSet(node_status, node_id, "Canceled"), ordered_nodes)
    /\ ready_queue' = CancelNode_Step_ForEach23_ready_queue(ready_queue, ordered_nodes, CancelNode_Step_ForEach22_node_status(MapSet(node_status, node_id, "Canceled"), ordered_nodes))
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, output_recorded, node_condition_results >>


CancelNode_Loop(node_id) ==
    /\ phase = "Running"
    /\ ((IF node_id \in DOMAIN node_status THEN node_status[node_id] ELSE "None") = "Running")
    /\ ((IF node_id \in DOMAIN node_kind THEN node_kind[node_id] ELSE "None") = "Loop")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ node_status' = CancelNode_Loop_ForEach24_node_status(MapSet(node_status, node_id, "Canceled"), ordered_nodes)
    /\ ready_queue' = CancelNode_Loop_ForEach25_ready_queue(ready_queue, ordered_nodes, CancelNode_Loop_ForEach24_node_status(MapSet(node_status, node_id, "Canceled"), ordered_nodes))
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, output_recorded, node_condition_results >>


SealRootFrameCanceled ==
    /\ phase = "Running"
    /\ AllNodesTerminal
    /\ (frame_scope = "Root")
    /\ (\E status_node \in tracked_nodes : ((IF status_node \in DOMAIN node_status THEN node_status[status_node] ELSE "None") = "Canceled"))
    /\ phase' = "Canceled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, node_status, ready_queue, output_recorded, node_condition_results >>


SealRootFrameFailed ==
    /\ phase = "Running"
    /\ AllNodesTerminal
    /\ (frame_scope = "Root")
    /\ ~((\E status_node \in tracked_nodes : ((IF status_node \in DOMAIN node_status THEN node_status[status_node] ELSE "None") = "Canceled")))
    /\ (\E status_node \in tracked_nodes : ((IF status_node \in DOMAIN node_status THEN node_status[status_node] ELSE "None") = "Failed"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, node_status, ready_queue, output_recorded, node_condition_results >>


SealRootFrameCompleted ==
    /\ phase = "Running"
    /\ AllNodesTerminal
    /\ (frame_scope = "Root")
    /\ ~((\E status_node \in tracked_nodes : ((IF status_node \in DOMAIN node_status THEN node_status[status_node] ELSE "None") = "Canceled")))
    /\ ~((\E status_node \in tracked_nodes : ((IF status_node \in DOMAIN node_status THEN node_status[status_node] ELSE "None") = "Failed")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, node_status, ready_queue, output_recorded, node_condition_results >>


SealBodyFrameCanceled ==
    /\ phase = "Running"
    /\ AllNodesTerminal
    /\ (frame_scope = "Body")
    /\ (\E status_node \in tracked_nodes : ((IF status_node \in DOMAIN node_status THEN node_status[status_node] ELSE "None") = "Canceled"))
    /\ phase' = "Canceled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, node_status, ready_queue, output_recorded, node_condition_results >>


SealBodyFrameFailed ==
    /\ phase = "Running"
    /\ AllNodesTerminal
    /\ (frame_scope = "Body")
    /\ ~((\E status_node \in tracked_nodes : ((IF status_node \in DOMAIN node_status THEN node_status[status_node] ELSE "None") = "Canceled")))
    /\ (\E status_node \in tracked_nodes : ((IF status_node \in DOMAIN node_status THEN node_status[status_node] ELSE "None") = "Failed"))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, node_status, ready_queue, output_recorded, node_condition_results >>


SealBodyFrameCompleted ==
    /\ phase = "Running"
    /\ AllNodesTerminal
    /\ (frame_scope = "Body")
    /\ ~((\E status_node \in tracked_nodes : ((IF status_node \in DOMAIN node_status THEN node_status[status_node] ELSE "None") = "Canceled")))
    /\ ~((\E status_node \in tracked_nodes : ((IF status_node \in DOMAIN node_status THEN node_status[status_node] ELSE "None") = "Failed")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << frame_id, frame_scope, loop_instance_id, iteration, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, branch_winners, node_status, ready_queue, output_recorded, node_condition_results >>


Next ==
    \/ \E arg_frame_id \in FrameIdValues : \E arg_tracked_nodes \in SetOfFlowNodeIdValues : \E arg_ordered_nodes \in SeqOfFlowNodeIdValues : \E arg_node_kind \in MapFlowNodeIdFlowNodeKindValues : \E arg_node_dependencies \in MapFlowNodeIdSeqFlowNodeIdValues : \E arg_node_dependency_modes \in MapFlowNodeIdDependencyModeValues : \E arg_node_branches \in MapFlowNodeIdOptionBranchIdValues : StartRootFrame(arg_frame_id, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches)
    \/ \E arg_frame_id \in FrameIdValues : \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_iteration \in 0..2 : \E arg_tracked_nodes \in SetOfFlowNodeIdValues : \E arg_ordered_nodes \in SeqOfFlowNodeIdValues : \E arg_node_kind \in MapFlowNodeIdFlowNodeKindValues : \E arg_node_dependencies \in MapFlowNodeIdSeqFlowNodeIdValues : \E arg_node_dependency_modes \in MapFlowNodeIdDependencyModeValues : \E arg_node_branches \in MapFlowNodeIdOptionBranchIdValues : StartBodyFrame(arg_frame_id, arg_loop_instance_id, arg_iteration, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches)
    \/ AdmitNextReadyNode_StepRun
    \/ AdmitNextReadyNode_LoopRun
    \/ AdmitNextReadyNode_Skip
    \/ AdmitNextReadyNode_Fail
    \/ \E node_id \in FlowNodeIdValues : CompleteNode_Step(node_id)
    \/ \E node_id \in FlowNodeIdValues : CompleteNode_Loop(node_id)
    \/ \E node_id \in FlowNodeIdValues : RecordNodeOutput(node_id)
    \/ \E node_id \in FlowNodeIdValues : FailNode_Step(node_id)
    \/ \E node_id \in FlowNodeIdValues : FailNode_Loop(node_id)
    \/ \E node_id \in FlowNodeIdValues : SkipNode_Step(node_id)
    \/ \E node_id \in FlowNodeIdValues : SkipNode_Loop(node_id)
    \/ \E node_id \in FlowNodeIdValues : CancelNode_Step(node_id)
    \/ \E node_id \in FlowNodeIdValues : CancelNode_Loop(node_id)
    \/ SealRootFrameCanceled
    \/ SealRootFrameFailed
    \/ SealRootFrameCompleted
    \/ SealBodyFrameCanceled
    \/ SealBodyFrameFailed
    \/ SealBodyFrameCompleted
    \/ TerminalStutter

ready_queue_membership_matches_ready_status == ((\A q_node \in SeqElements(ready_queue) : ((IF q_node \in DOMAIN node_status THEN node_status[q_node] ELSE "None") = "Ready")) /\ (\A t_node \in tracked_nodes : (((IF t_node \in DOMAIN node_status THEN node_status[t_node] ELSE "None") # "Ready") \/ (t_node \in SeqElements(ready_queue)))))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(tracked_nodes) <= 1 /\ Len(ordered_nodes) <= 1 /\ Cardinality(DOMAIN node_kind) <= 1 /\ Cardinality(DOMAIN node_dependencies) <= 1 /\ Cardinality(DOMAIN node_dependency_modes) <= 1 /\ Cardinality(DOMAIN node_branches) <= 1 /\ Cardinality(branch_winners) <= 1 /\ Cardinality(DOMAIN node_status) <= 1 /\ Len(ready_queue) <= 1 /\ Cardinality(DOMAIN output_recorded) <= 1 /\ Cardinality(DOMAIN node_condition_results) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(tracked_nodes) <= 2 /\ Len(ordered_nodes) <= 2 /\ Cardinality(DOMAIN node_kind) <= 2 /\ Cardinality(DOMAIN node_dependencies) <= 2 /\ Cardinality(DOMAIN node_dependency_modes) <= 2 /\ Cardinality(DOMAIN node_branches) <= 2 /\ Cardinality(branch_winners) <= 2 /\ Cardinality(DOMAIN node_status) <= 2 /\ Len(ready_queue) <= 2 /\ Cardinality(DOMAIN output_recorded) <= 2 /\ Cardinality(DOMAIN node_condition_results) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []ready_queue_membership_matches_ready_status

=============================================================================
