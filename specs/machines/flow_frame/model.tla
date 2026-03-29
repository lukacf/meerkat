---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for FlowFrameMachine.

CONSTANTS BranchIdValues, DependencyModeValues, FlowNodeIdValues, FlowNodeKindValues, FrameIdValues, SetOfFlowNodeIdValues

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

VARIABLES phase, model_step_count, frame_id, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_status, ready_queue, output_recorded, node_condition_results

vars == << phase, model_step_count, frame_id, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_status, ready_queue, output_recorded, node_condition_results >>

AllNodesTerminal == (\A t_node \in tracked_nodes : (((IF t_node \in DOMAIN node_status THEN node_status[t_node] ELSE "None") = "Completed") \/ ((IF t_node \in DOMAIN node_status THEN node_status[t_node] ELSE "None") = "Failed") \/ ((IF t_node \in DOMAIN node_status THEN node_status[t_node] ELSE "None") = "Skipped") \/ ((IF t_node \in DOMAIN node_status THEN node_status[t_node] ELSE "None") = "Canceled")))
AnyDepCompleted(node_id) == (\E dep_id \in (IF node_id \in DOMAIN node_dependencies THEN node_dependencies[node_id] ELSE <<>>) : ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Completed"))
AllDepsCompleted(node_id) == (\A dep_id \in (IF node_id \in DOMAIN node_dependencies THEN node_dependencies[node_id] ELSE <<>>) : ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Completed"))
NodeAdmissionEligible(node_id) == (IF (Len((IF node_id \in DOMAIN node_dependencies THEN node_dependencies[node_id] ELSE <<>>)) = 0) THEN TRUE ELSE (IF ((IF node_id \in DOMAIN node_dependency_modes THEN node_dependency_modes[node_id] ELSE "None") = "All") THEN (\A dep_id \in (IF node_id \in DOMAIN node_dependencies THEN node_dependencies[node_id] ELSE <<>>) : (((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Completed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Failed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Skipped") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Canceled"))) ELSE ((\E dep_id \in (IF node_id \in DOMAIN node_dependencies THEN node_dependencies[node_id] ELSE <<>>) : ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Completed")) \/ (\A dep_id \in (IF node_id \in DOMAIN node_dependencies THEN node_dependencies[node_id] ELSE <<>>) : (((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Completed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Failed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Skipped") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Canceled"))))))

Init ==
    /\ phase = "Absent"
    /\ model_step_count = 0
    /\ frame_id = ""
    /\ last_admitted_node = ""
    /\ tracked_nodes = {}
    /\ ordered_nodes = <<>>
    /\ node_kind = [x \in {} |-> None]
    /\ node_dependencies = [x \in {} |-> None]
    /\ node_dependency_modes = [x \in {} |-> None]
    /\ node_branches = [x \in {} |-> None]
    /\ node_status = [x \in {} |-> None]
    /\ ready_queue = <<>>
    /\ output_recorded = [x \in {} |-> None]
    /\ node_condition_results = [x \in {} |-> None]

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Failed" \/ phase = "Canceled"
    /\ UNCHANGED vars

RECURSIVE StartFrame_ForEach0_node_status(_, _)
StartFrame_ForEach0_node_status(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET init_node == item IN LET next_acc == MapSet(acc, init_node, "Pending") IN StartFrame_ForEach0_node_status(next_acc, remaining \ {item})

RECURSIVE StartFrame_ForEach1_node_status(_, _)
StartFrame_ForEach1_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN StartFrame_ForEach1_node_status(next_acc, Tail(items))

RECURSIVE StartFrame_ForEach2_ready_queue(_, _, _)
StartFrame_ForEach2_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN StartFrame_ForEach2_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE AdmitNextReadyNode_Skip_ForEach3_node_status(_, _)
AdmitNextReadyNode_Skip_ForEach3_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN AdmitNextReadyNode_Skip_ForEach3_node_status(next_acc, Tail(items))

RECURSIVE AdmitNextReadyNode_Skip_ForEach4_ready_queue(_, _, _)
AdmitNextReadyNode_Skip_ForEach4_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN AdmitNextReadyNode_Skip_ForEach4_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE AdmitNextReadyNode_Fail_ForEach5_node_status(_, _)
AdmitNextReadyNode_Fail_ForEach5_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN AdmitNextReadyNode_Fail_ForEach5_node_status(next_acc, Tail(items))

RECURSIVE AdmitNextReadyNode_Fail_ForEach6_ready_queue(_, _, _)
AdmitNextReadyNode_Fail_ForEach6_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN AdmitNextReadyNode_Fail_ForEach6_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE CompleteNode_ForEach7_node_status(_, _)
CompleteNode_ForEach7_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN CompleteNode_ForEach7_node_status(next_acc, Tail(items))

RECURSIVE CompleteNode_ForEach8_ready_queue(_, _, _)
CompleteNode_ForEach8_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN CompleteNode_ForEach8_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE FailNode_ForEach9_node_status(_, _)
FailNode_ForEach9_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN FailNode_ForEach9_node_status(next_acc, Tail(items))

RECURSIVE FailNode_ForEach10_ready_queue(_, _, _)
FailNode_ForEach10_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN FailNode_ForEach10_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE SkipNode_ForEach11_node_status(_, _)
SkipNode_ForEach11_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN SkipNode_ForEach11_node_status(next_acc, Tail(items))

RECURSIVE SkipNode_ForEach12_ready_queue(_, _, _)
SkipNode_ForEach12_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN SkipNode_ForEach12_ready_queue(next_acc, Tail(items), captured_node_status)

RECURSIVE CancelNode_ForEach13_node_status(_, _)
CancelNode_ForEach13_node_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN acc THEN acc[rf_node] ELSE "None") = "Pending") /\ NodeAdmissionEligible(rf_node)) THEN MapSet(acc, rf_node, "Ready") ELSE acc IN CancelNode_ForEach13_node_status(next_acc, Tail(items))

RECURSIVE CancelNode_ForEach14_ready_queue(_, _, _)
CancelNode_ForEach14_ready_queue(acc, items, captured_node_status) == IF Len(items) = 0 THEN acc ELSE LET rf_node == Head(items) IN LET next_acc == IF (((IF rf_node \in DOMAIN captured_node_status THEN captured_node_status[rf_node] ELSE "None") = "Ready") /\ ~((rf_node \in SeqElements(acc)))) THEN Append(acc, rf_node) ELSE acc IN CancelNode_ForEach14_ready_queue(next_acc, Tail(items), captured_node_status)

StartFrame(arg_frame_id, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches) ==
    /\ phase = "Absent"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ frame_id' = arg_frame_id
    /\ tracked_nodes' = arg_tracked_nodes
    /\ ordered_nodes' = arg_ordered_nodes
    /\ node_kind' = arg_node_kind
    /\ node_dependencies' = arg_node_dependencies
    /\ node_dependency_modes' = arg_node_dependency_modes
    /\ node_branches' = arg_node_branches
    /\ node_status' = StartFrame_ForEach1_node_status(StartFrame_ForEach0_node_status(node_status, arg_tracked_nodes), arg_ordered_nodes)
    /\ ready_queue' = StartFrame_ForEach2_ready_queue(ready_queue, arg_ordered_nodes, StartFrame_ForEach1_node_status(StartFrame_ForEach0_node_status(node_status, arg_tracked_nodes), arg_ordered_nodes))
    /\ UNCHANGED << last_admitted_node, output_recorded, node_condition_results >>


AdmitNextReadyNode_StepRun ==
    /\ phase = "Running"
    /\ (Len(ready_queue) > 0)
    /\ ((IF Head(ready_queue) \in DOMAIN node_kind THEN node_kind[Head(ready_queue)] ELSE "None") = "Step")
    /\ ((Len((IF Head(ready_queue) \in DOMAIN node_dependencies THEN node_dependencies[Head(ready_queue)] ELSE <<>>)) = 0) \/ (((IF Head(ready_queue) \in DOMAIN node_dependency_modes THEN node_dependency_modes[Head(ready_queue)] ELSE "None") = "All") /\ AllDepsCompleted(Head(ready_queue))) \/ (((IF Head(ready_queue) \in DOMAIN node_dependency_modes THEN node_dependency_modes[Head(ready_queue)] ELSE "None") = "Any") /\ AnyDepCompleted(Head(ready_queue))))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ last_admitted_node' = Head(ready_queue)
    /\ node_status' = MapSet(node_status, Head(ready_queue), "Running")
    /\ ready_queue' = Tail(ready_queue)
    /\ UNCHANGED << frame_id, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, output_recorded, node_condition_results >>


AdmitNextReadyNode_LoopRun ==
    /\ phase = "Running"
    /\ (Len(ready_queue) > 0)
    /\ ((IF Head(ready_queue) \in DOMAIN node_kind THEN node_kind[Head(ready_queue)] ELSE "None") = "Loop")
    /\ ((Len((IF Head(ready_queue) \in DOMAIN node_dependencies THEN node_dependencies[Head(ready_queue)] ELSE <<>>)) = 0) \/ (((IF Head(ready_queue) \in DOMAIN node_dependency_modes THEN node_dependency_modes[Head(ready_queue)] ELSE "None") = "All") /\ AllDepsCompleted(Head(ready_queue))) \/ (((IF Head(ready_queue) \in DOMAIN node_dependency_modes THEN node_dependency_modes[Head(ready_queue)] ELSE "None") = "Any") /\ AnyDepCompleted(Head(ready_queue))))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ last_admitted_node' = Head(ready_queue)
    /\ node_status' = MapSet(node_status, Head(ready_queue), "Running")
    /\ ready_queue' = Tail(ready_queue)
    /\ UNCHANGED << frame_id, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, output_recorded, node_condition_results >>


AdmitNextReadyNode_Skip ==
    /\ phase = "Running"
    /\ (Len(ready_queue) > 0)
    /\ (((IF Head(ready_queue) \in DOMAIN node_dependency_modes THEN node_dependency_modes[Head(ready_queue)] ELSE "None") = "All") /\ (\E dep_id \in (IF Head(ready_queue) \in DOMAIN node_dependencies THEN node_dependencies[Head(ready_queue)] ELSE <<>>) : (((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Failed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Skipped") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Canceled"))))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ last_admitted_node' = Head(ready_queue)
    /\ node_status' = AdmitNextReadyNode_Skip_ForEach3_node_status(MapSet(node_status, Head(ready_queue), "Skipped"), ordered_nodes)
    /\ ready_queue' = AdmitNextReadyNode_Skip_ForEach4_ready_queue(Tail(ready_queue), ordered_nodes, AdmitNextReadyNode_Skip_ForEach3_node_status(MapSet(node_status, Head(ready_queue), "Skipped"), ordered_nodes))
    /\ UNCHANGED << frame_id, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, output_recorded, node_condition_results >>


AdmitNextReadyNode_Fail ==
    /\ phase = "Running"
    /\ (Len(ready_queue) > 0)
    /\ (((IF Head(ready_queue) \in DOMAIN node_dependency_modes THEN node_dependency_modes[Head(ready_queue)] ELSE "None") = "Any") /\ (\A dep_id \in (IF Head(ready_queue) \in DOMAIN node_dependencies THEN node_dependencies[Head(ready_queue)] ELSE <<>>) : (((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Completed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Failed") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Skipped") \/ ((IF dep_id \in DOMAIN node_status THEN node_status[dep_id] ELSE "None") = "Canceled"))) /\ ~(AnyDepCompleted(Head(ready_queue))))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ last_admitted_node' = Head(ready_queue)
    /\ node_status' = AdmitNextReadyNode_Fail_ForEach5_node_status(MapSet(node_status, Head(ready_queue), "Failed"), ordered_nodes)
    /\ ready_queue' = AdmitNextReadyNode_Fail_ForEach6_ready_queue(Tail(ready_queue), ordered_nodes, AdmitNextReadyNode_Fail_ForEach5_node_status(MapSet(node_status, Head(ready_queue), "Failed"), ordered_nodes))
    /\ UNCHANGED << frame_id, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, output_recorded, node_condition_results >>


CompleteNode(node_id) ==
    /\ phase = "Running"
    /\ ((IF node_id \in DOMAIN node_status THEN node_status[node_id] ELSE "None") = "Running")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ node_status' = CompleteNode_ForEach7_node_status(MapSet(node_status, node_id, "Completed"), ordered_nodes)
    /\ ready_queue' = CompleteNode_ForEach8_ready_queue(ready_queue, ordered_nodes, CompleteNode_ForEach7_node_status(MapSet(node_status, node_id, "Completed"), ordered_nodes))
    /\ UNCHANGED << frame_id, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, output_recorded, node_condition_results >>


RecordNodeOutput(node_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ output_recorded' = MapSet(output_recorded, node_id, TRUE)
    /\ UNCHANGED << frame_id, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_status, ready_queue, node_condition_results >>


FailNode(node_id) ==
    /\ phase = "Running"
    /\ ((IF node_id \in DOMAIN node_status THEN node_status[node_id] ELSE "None") = "Running")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ node_status' = FailNode_ForEach9_node_status(MapSet(node_status, node_id, "Failed"), ordered_nodes)
    /\ ready_queue' = FailNode_ForEach10_ready_queue(ready_queue, ordered_nodes, FailNode_ForEach9_node_status(MapSet(node_status, node_id, "Failed"), ordered_nodes))
    /\ UNCHANGED << frame_id, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, output_recorded, node_condition_results >>


SkipNode(node_id) ==
    /\ phase = "Running"
    /\ ((IF node_id \in DOMAIN node_status THEN node_status[node_id] ELSE "None") = "Running")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ node_status' = SkipNode_ForEach11_node_status(MapSet(node_status, node_id, "Skipped"), ordered_nodes)
    /\ ready_queue' = SkipNode_ForEach12_ready_queue(ready_queue, ordered_nodes, SkipNode_ForEach11_node_status(MapSet(node_status, node_id, "Skipped"), ordered_nodes))
    /\ UNCHANGED << frame_id, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, output_recorded, node_condition_results >>


CancelNode(node_id) ==
    /\ phase = "Running"
    /\ ((IF node_id \in DOMAIN node_status THEN node_status[node_id] ELSE "None") = "Running")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ node_status' = CancelNode_ForEach13_node_status(MapSet(node_status, node_id, "Canceled"), ordered_nodes)
    /\ ready_queue' = CancelNode_ForEach14_ready_queue(ready_queue, ordered_nodes, CancelNode_ForEach13_node_status(MapSet(node_status, node_id, "Canceled"), ordered_nodes))
    /\ UNCHANGED << frame_id, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, output_recorded, node_condition_results >>


TerminalizeCompleted ==
    /\ phase = "Running"
    /\ AllNodesTerminal
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << frame_id, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_status, ready_queue, output_recorded, node_condition_results >>


TerminalizeFailed ==
    /\ phase = "Running" \/ phase = "Absent"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << frame_id, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_status, ready_queue, output_recorded, node_condition_results >>


TerminalizeCanceled ==
    /\ phase = "Running" \/ phase = "Absent"
    /\ phase' = "Canceled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << frame_id, last_admitted_node, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_status, ready_queue, output_recorded, node_condition_results >>


Next ==
    \/ \E arg_frame_id \in FrameIdValues : \E arg_tracked_nodes \in SetOfFlowNodeIdValues : \E arg_ordered_nodes \in SeqOfFlowNodeIdValues : \E arg_node_kind \in MapFlowNodeIdFlowNodeKindValues : \E arg_node_dependencies \in MapFlowNodeIdSeqFlowNodeIdValues : \E arg_node_dependency_modes \in MapFlowNodeIdDependencyModeValues : \E arg_node_branches \in MapFlowNodeIdOptionBranchIdValues : StartFrame(arg_frame_id, arg_tracked_nodes, arg_ordered_nodes, arg_node_kind, arg_node_dependencies, arg_node_dependency_modes, arg_node_branches)
    \/ AdmitNextReadyNode_StepRun
    \/ AdmitNextReadyNode_LoopRun
    \/ AdmitNextReadyNode_Skip
    \/ AdmitNextReadyNode_Fail
    \/ \E node_id \in FlowNodeIdValues : CompleteNode(node_id)
    \/ \E node_id \in FlowNodeIdValues : RecordNodeOutput(node_id)
    \/ \E node_id \in FlowNodeIdValues : FailNode(node_id)
    \/ \E node_id \in FlowNodeIdValues : SkipNode(node_id)
    \/ \E node_id \in FlowNodeIdValues : CancelNode(node_id)
    \/ TerminalizeCompleted
    \/ TerminalizeFailed
    \/ TerminalizeCanceled
    \/ TerminalStutter

ready_queue_membership_matches_ready_status == ((\A q_node \in SeqElements(ready_queue) : ((IF q_node \in DOMAIN node_status THEN node_status[q_node] ELSE "None") = "Ready")) /\ (\A t_node \in tracked_nodes : (((IF t_node \in DOMAIN node_status THEN node_status[t_node] ELSE "None") # "Ready") \/ (t_node \in SeqElements(ready_queue)))))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(tracked_nodes) <= 1 /\ Len(ordered_nodes) <= 1 /\ Cardinality(DOMAIN node_kind) <= 1 /\ Cardinality(DOMAIN node_dependencies) <= 1 /\ Cardinality(DOMAIN node_dependency_modes) <= 1 /\ Cardinality(DOMAIN node_branches) <= 1 /\ Cardinality(DOMAIN node_status) <= 1 /\ Len(ready_queue) <= 1 /\ Cardinality(DOMAIN output_recorded) <= 1 /\ Cardinality(DOMAIN node_condition_results) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(tracked_nodes) <= 2 /\ Len(ordered_nodes) <= 2 /\ Cardinality(DOMAIN node_kind) <= 2 /\ Cardinality(DOMAIN node_dependencies) <= 2 /\ Cardinality(DOMAIN node_dependency_modes) <= 2 /\ Cardinality(DOMAIN node_branches) <= 2 /\ Cardinality(DOMAIN node_status) <= 2 /\ Len(ready_queue) <= 2 /\ Cardinality(DOMAIN output_recorded) <= 2 /\ Cardinality(DOMAIN node_condition_results) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []ready_queue_membership_matches_ready_status

=============================================================================
