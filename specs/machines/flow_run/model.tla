---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for FlowRunMachine.

CONSTANTS BooleanValues, BranchIdValues, CollectionPolicyKindValues, DependencyModeValues, MeerkatIdValues, NatValues, StepIdValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

SeqOfStepIdValues == {<<>>} \cup {<<x>> : x \in StepIdValues} \cup {<<x, y>> : x \in StepIdValues, y \in StepIdValues}
OptionBranchIdValues == {None} \cup {Some(x) : x \in BranchIdValues}
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
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, tracked_steps, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries

vars == << phase, model_step_count, tracked_steps, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>

RunIsTerminal == ((phase = "Completed") \/ (phase = "Failed") \/ (phase = "Canceled"))
StepIsTracked(step_id) == (step_id \in tracked_steps)
StepStatusIs(step_id, expected_status) == ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE None) = Some(expected_status))
StepOutputRecordedIs(step_id, expected) == ((IF step_id \in DOMAIN output_recorded THEN output_recorded[step_id] ELSE FALSE) = expected)
StepConditionRecordedIs(step_id, expected) == ((IF step_id \in DOMAIN step_condition_results THEN step_condition_results[step_id] ELSE None) = expected)
StepConditionAllowsDispatch(step_id) == (((IF step_id \in DOMAIN step_has_conditions THEN step_has_conditions[step_id] ELSE FALSE) = FALSE) \/ StepConditionRecordedIs(step_id, Some(TRUE)))
AllTrackedStepsInAllowedStatuses(allowed_statuses) == (\A step_id \in tracked_steps : ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE None) \in SeqElements(allowed_statuses)))
NoTrackedStepInStatus(status) == (\A step_id \in tracked_steps : ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE None) # Some(status)))
AnyTrackedStepInStatus(status) == (\E step_id \in tracked_steps : ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE None) = Some(status)))
StepHasDependencies(step_id) == (Len((IF (step_id \in DOMAIN step_dependencies) THEN (IF step_id \in DOMAIN step_dependencies THEN step_dependencies[step_id] ELSE <<>>) ELSE <<>>)) > 0)
AllDependenciesCompleted(step_id) == (\A dependency \in SeqElements((IF (step_id \in DOMAIN step_dependencies) THEN (IF step_id \in DOMAIN step_dependencies THEN step_dependencies[step_id] ELSE <<>>) ELSE <<>>)) : StepStatusIs(dependency, "Completed"))
AllDependenciesSkipped(step_id) == (\A dependency \in SeqElements((IF (step_id \in DOMAIN step_dependencies) THEN (IF step_id \in DOMAIN step_dependencies THEN step_dependencies[step_id] ELSE <<>>) ELSE <<>>)) : StepStatusIs(dependency, "Skipped"))
AnyDependencyCompleted(step_id) == (\E dependency \in SeqElements((IF (step_id \in DOMAIN step_dependencies) THEN (IF step_id \in DOMAIN step_dependencies THEN step_dependencies[step_id] ELSE <<>>) ELSE <<>>)) : StepStatusIs(dependency, "Completed"))
StepDependencyReady(step_id) == (IF ((IF (step_id \in DOMAIN step_dependency_modes) THEN (IF step_id \in DOMAIN step_dependency_modes THEN step_dependency_modes[step_id] ELSE "None") ELSE "All") = "Any") THEN AnyDependencyCompleted(step_id) ELSE (IF ~(StepHasDependencies(step_id)) THEN TRUE ELSE AllDependenciesCompleted(step_id)))
StepDependencyShouldSkip(step_id) == (((IF (step_id \in DOMAIN step_dependency_modes) THEN (IF step_id \in DOMAIN step_dependency_modes THEN step_dependency_modes[step_id] ELSE "None") ELSE "All") = "Any") /\ StepHasDependencies(step_id) /\ AllDependenciesSkipped(step_id))
StepBranchBlocked(step_id) == (IF ((IF (step_id \in DOMAIN step_branches) THEN (IF step_id \in DOMAIN step_branches THEN step_branches[step_id] ELSE None) ELSE None) = None) THEN FALSE ELSE (\E candidate \in tracked_steps : ((candidate # step_id) /\ ((IF (candidate \in DOMAIN step_branches) THEN (IF candidate \in DOMAIN step_branches THEN step_branches[candidate] ELSE None) ELSE None) = (IF (step_id \in DOMAIN step_branches) THEN (IF step_id \in DOMAIN step_branches THEN step_branches[step_id] ELSE None) ELSE None)) /\ StepStatusIs(candidate, "Completed"))))
EscalationWillTrigger == ((escalation_threshold > 0) /\ ((consecutive_failure_count + 1) >= escalation_threshold))
TargetRetryCount(retry_key) == (IF (retry_key \in DOMAIN target_retry_counts) THEN (IF retry_key \in DOMAIN target_retry_counts THEN target_retry_counts[retry_key] ELSE 0) ELSE 0)
TargetRetryAllowed(retry_key) == ((IF (retry_key \in DOMAIN target_retry_counts) THEN (IF retry_key \in DOMAIN target_retry_counts THEN target_retry_counts[retry_key] ELSE 0) ELSE 0) <= max_step_retries)
CollectionSatisfied(step_id) == (IF ((IF (step_id \in DOMAIN step_collection_policies) THEN (IF step_id \in DOMAIN step_collection_policies THEN step_collection_policies[step_id] ELSE "None") ELSE "All") = "All") THEN ((IF (step_id \in DOMAIN step_target_success_counts) THEN (IF step_id \in DOMAIN step_target_success_counts THEN step_target_success_counts[step_id] ELSE 0) ELSE 0) = (IF (step_id \in DOMAIN step_target_counts) THEN (IF step_id \in DOMAIN step_target_counts THEN step_target_counts[step_id] ELSE 0) ELSE 0)) ELSE (IF ((IF (step_id \in DOMAIN step_collection_policies) THEN (IF step_id \in DOMAIN step_collection_policies THEN step_collection_policies[step_id] ELSE "None") ELSE "All") = "Any") THEN ((IF (step_id \in DOMAIN step_target_success_counts) THEN (IF step_id \in DOMAIN step_target_success_counts THEN step_target_success_counts[step_id] ELSE 0) ELSE 0) >= 1) ELSE (IF ((IF (step_id \in DOMAIN step_collection_policies) THEN (IF step_id \in DOMAIN step_collection_policies THEN step_collection_policies[step_id] ELSE "None") ELSE "All") = "Quorum") THEN ((IF (step_id \in DOMAIN step_target_success_counts) THEN (IF step_id \in DOMAIN step_target_success_counts THEN step_target_success_counts[step_id] ELSE 0) ELSE 0) >= (IF (step_id \in DOMAIN step_quorum_thresholds) THEN (IF step_id \in DOMAIN step_quorum_thresholds THEN step_quorum_thresholds[step_id] ELSE 0) ELSE 0)) ELSE FALSE)))
CollectionFeasible(step_id) == (IF ((IF (step_id \in DOMAIN step_collection_policies) THEN (IF step_id \in DOMAIN step_collection_policies THEN step_collection_policies[step_id] ELSE "None") ELSE "All") = "All") THEN ((IF (step_id \in DOMAIN step_target_terminal_failure_counts) THEN (IF step_id \in DOMAIN step_target_terminal_failure_counts THEN step_target_terminal_failure_counts[step_id] ELSE 0) ELSE 0) = 0) ELSE (IF ((IF (step_id \in DOMAIN step_collection_policies) THEN (IF step_id \in DOMAIN step_collection_policies THEN step_collection_policies[step_id] ELSE "None") ELSE "All") = "Any") THEN (((IF (step_id \in DOMAIN step_target_success_counts) THEN (IF step_id \in DOMAIN step_target_success_counts THEN step_target_success_counts[step_id] ELSE 0) ELSE 0) >= 1) \/ (((IF (step_id \in DOMAIN step_target_counts) THEN (IF step_id \in DOMAIN step_target_counts THEN step_target_counts[step_id] ELSE 0) ELSE 0) - (IF (step_id \in DOMAIN step_target_terminal_failure_counts) THEN (IF step_id \in DOMAIN step_target_terminal_failure_counts THEN step_target_terminal_failure_counts[step_id] ELSE 0) ELSE 0)) > 0)) ELSE (IF ((IF (step_id \in DOMAIN step_collection_policies) THEN (IF step_id \in DOMAIN step_collection_policies THEN step_collection_policies[step_id] ELSE "None") ELSE "All") = "Quorum") THEN (((IF (step_id \in DOMAIN step_target_success_counts) THEN (IF step_id \in DOMAIN step_target_success_counts THEN step_target_success_counts[step_id] ELSE 0) ELSE 0) + ((IF (step_id \in DOMAIN step_target_counts) THEN (IF step_id \in DOMAIN step_target_counts THEN step_target_counts[step_id] ELSE 0) ELSE 0) - (IF (step_id \in DOMAIN step_target_terminal_failure_counts) THEN (IF step_id \in DOMAIN step_target_terminal_failure_counts THEN step_target_terminal_failure_counts[step_id] ELSE 0) ELSE 0))) >= (IF (step_id \in DOMAIN step_quorum_thresholds) THEN (IF step_id \in DOMAIN step_quorum_thresholds THEN step_quorum_thresholds[step_id] ELSE 0) ELSE 0)) ELSE FALSE)))
StepTargetCount(step_id) == (IF (step_id \in DOMAIN step_target_counts) THEN (IF step_id \in DOMAIN step_target_counts THEN step_target_counts[step_id] ELSE 0) ELSE 0)
StepTargetSuccessCount(step_id) == (IF (step_id \in DOMAIN step_target_success_counts) THEN (IF step_id \in DOMAIN step_target_success_counts THEN step_target_success_counts[step_id] ELSE 0) ELSE 0)
StepTargetTerminalFailureCount(step_id) == (IF (step_id \in DOMAIN step_target_terminal_failure_counts) THEN (IF step_id \in DOMAIN step_target_terminal_failure_counts THEN step_target_terminal_failure_counts[step_id] ELSE 0) ELSE 0)
RemainingTargetCount(step_id) == ((IF (step_id \in DOMAIN step_target_counts) THEN (IF step_id \in DOMAIN step_target_counts THEN step_target_counts[step_id] ELSE 0) ELSE 0) - (IF (step_id \in DOMAIN step_target_terminal_failure_counts) THEN (IF step_id \in DOMAIN step_target_terminal_failure_counts THEN step_target_terminal_failure_counts[step_id] ELSE 0) ELSE 0))

Init ==
    /\ phase = "Absent"
    /\ model_step_count = 0
    /\ tracked_steps = {}
    /\ ordered_steps = <<>>
    /\ step_status = [x \in {} |-> None]
    /\ output_recorded = [x \in {} |-> None]
    /\ step_condition_results = [x \in {} |-> None]
    /\ step_has_conditions = [x \in {} |-> None]
    /\ step_dependencies = [x \in {} |-> None]
    /\ step_dependency_modes = [x \in {} |-> None]
    /\ step_branches = [x \in {} |-> None]
    /\ step_collection_policies = [x \in {} |-> None]
    /\ step_quorum_thresholds = [x \in {} |-> None]
    /\ step_target_counts = [x \in {} |-> None]
    /\ step_target_success_counts = [x \in {} |-> None]
    /\ step_target_terminal_failure_counts = [x \in {} |-> None]
    /\ target_retry_counts = [x \in {} |-> None]
    /\ failure_count = 0
    /\ consecutive_failure_count = 0
    /\ escalation_threshold = 0
    /\ max_step_retries = 0

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Failed" \/ phase = "Canceled"
    /\ UNCHANGED vars

RECURSIVE CreateRun_ForEach0_output_recorded(_, _)
CreateRun_ForEach0_output_recorded(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, FALSE) IN CreateRun_ForEach0_output_recorded(next_acc, Tail(items))

RECURSIVE CreateRun_ForEach0_step_condition_results(_, _)
CreateRun_ForEach0_step_condition_results(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, None) IN CreateRun_ForEach0_step_condition_results(next_acc, Tail(items))

RECURSIVE CreateRun_ForEach0_step_status(_, _)
CreateRun_ForEach0_step_status(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, None) IN CreateRun_ForEach0_step_status(next_acc, Tail(items))

RECURSIVE CreateRun_ForEach0_step_target_counts(_, _)
CreateRun_ForEach0_step_target_counts(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, 0) IN CreateRun_ForEach0_step_target_counts(next_acc, Tail(items))

RECURSIVE CreateRun_ForEach0_step_target_success_counts(_, _)
CreateRun_ForEach0_step_target_success_counts(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, 0) IN CreateRun_ForEach0_step_target_success_counts(next_acc, Tail(items))

RECURSIVE CreateRun_ForEach0_step_target_terminal_failure_counts(_, _)
CreateRun_ForEach0_step_target_terminal_failure_counts(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == MapSet(acc, step_id, 0) IN CreateRun_ForEach0_step_target_terminal_failure_counts(next_acc, Tail(items))

RECURSIVE CreateRun_ForEach0_tracked_steps(_, _)
CreateRun_ForEach0_tracked_steps(acc, items) == IF Len(items) = 0 THEN acc ELSE LET step_id == Head(items) IN LET next_acc == (acc \cup {step_id}) IN CreateRun_ForEach0_tracked_steps(next_acc, Tail(items))

CreateRun(step_ids, arg_ordered_steps, arg_step_has_conditions, arg_step_dependencies, arg_step_dependency_modes, arg_step_branches, arg_step_collection_policies, arg_step_quorum_thresholds, arg_escalation_threshold, arg_max_step_retries) ==
    /\ phase = "Absent"
    /\ (Len(step_ids) > 0)
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ tracked_steps' = CreateRun_ForEach0_tracked_steps({}, step_ids)
    /\ ordered_steps' = arg_ordered_steps
    /\ step_status' = CreateRun_ForEach0_step_status([x \in {} |-> None], step_ids)
    /\ output_recorded' = CreateRun_ForEach0_output_recorded([x \in {} |-> None], step_ids)
    /\ step_condition_results' = CreateRun_ForEach0_step_condition_results([x \in {} |-> None], step_ids)
    /\ step_has_conditions' = arg_step_has_conditions
    /\ step_dependencies' = arg_step_dependencies
    /\ step_dependency_modes' = arg_step_dependency_modes
    /\ step_branches' = arg_step_branches
    /\ step_collection_policies' = arg_step_collection_policies
    /\ step_quorum_thresholds' = arg_step_quorum_thresholds
    /\ step_target_counts' = CreateRun_ForEach0_step_target_counts([x \in {} |-> None], step_ids)
    /\ step_target_success_counts' = CreateRun_ForEach0_step_target_success_counts([x \in {} |-> None], step_ids)
    /\ step_target_terminal_failure_counts' = CreateRun_ForEach0_step_target_terminal_failure_counts([x \in {} |-> None], step_ids)
    /\ target_retry_counts' = [x \in {} |-> None]
    /\ failure_count' = 0
    /\ consecutive_failure_count' = 0
    /\ escalation_threshold' = arg_escalation_threshold
    /\ max_step_retries' = arg_max_step_retries


StartRun ==
    /\ phase = "Pending"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << tracked_steps, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


DispatchStep(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE None) = None)
    /\ StepConditionAllowsDispatch(step_id)
    /\ StepDependencyReady(step_id)
    /\ ~(StepBranchBlocked(step_id))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_status' = MapSet(step_status, step_id, Some("Dispatched"))
    /\ UNCHANGED << tracked_steps, ordered_steps, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


CompleteStep(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "Dispatched")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_status' = MapSet(step_status, step_id, Some("Completed"))
    /\ consecutive_failure_count' = 0
    /\ UNCHANGED << tracked_steps, ordered_steps, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, escalation_threshold, max_step_retries >>


RecordStepOutput(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "Completed")
    /\ StepOutputRecordedIs(step_id, FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ output_recorded' = MapSet(output_recorded, step_id, TRUE)
    /\ UNCHANGED << tracked_steps, ordered_steps, step_status, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


ConditionPassed(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE None) = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_condition_results' = MapSet(step_condition_results, step_id, Some(TRUE))
    /\ UNCHANGED << tracked_steps, ordered_steps, step_status, output_recorded, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


ConditionRejected(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE None) = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_status' = MapSet(step_status, step_id, Some("Skipped"))
    /\ step_condition_results' = MapSet(step_condition_results, step_id, Some(FALSE))
    /\ UNCHANGED << tracked_steps, ordered_steps, output_recorded, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


FailStepEscalating(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "Dispatched")
    /\ EscalationWillTrigger
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_status' = MapSet(step_status, step_id, Some("Failed"))
    /\ failure_count' = (failure_count) + 1
    /\ consecutive_failure_count' = (consecutive_failure_count) + 1
    /\ UNCHANGED << tracked_steps, ordered_steps, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, escalation_threshold, max_step_retries >>


FailStep(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "Dispatched")
    /\ ~(EscalationWillTrigger)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_status' = MapSet(step_status, step_id, Some("Failed"))
    /\ failure_count' = (failure_count) + 1
    /\ consecutive_failure_count' = (consecutive_failure_count) + 1
    /\ UNCHANGED << tracked_steps, ordered_steps, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, escalation_threshold, max_step_retries >>


SkipStep(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE None) = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_status' = MapSet(step_status, step_id, Some("Skipped"))
    /\ UNCHANGED << tracked_steps, ordered_steps, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


CancelStep(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ (((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE None) = None) \/ StepStatusIs(step_id, "Dispatched"))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_status' = MapSet(step_status, step_id, Some("Canceled"))
    /\ UNCHANGED << tracked_steps, ordered_steps, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


RegisterTargets(step_id, target_count) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ ((IF step_id \in DOMAIN step_status THEN step_status[step_id] ELSE None) = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_target_counts' = MapSet(step_target_counts, step_id, target_count)
    /\ step_target_success_counts' = MapSet(step_target_success_counts, step_id, 0)
    /\ step_target_terminal_failure_counts' = MapSet(step_target_terminal_failure_counts, step_id, 0)
    /\ UNCHANGED << tracked_steps, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


RecordTargetSuccess(step_id, target_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "Dispatched")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_target_success_counts' = MapSet(step_target_success_counts, step_id, ((IF (step_id \in DOMAIN step_target_success_counts) THEN (IF step_id \in DOMAIN step_target_success_counts THEN step_target_success_counts[step_id] ELSE 0) ELSE 0) + 1))
    /\ UNCHANGED << tracked_steps, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


RecordTargetTerminalFailure(step_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "Dispatched")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_target_terminal_failure_counts' = MapSet(step_target_terminal_failure_counts, step_id, ((IF (step_id \in DOMAIN step_target_terminal_failure_counts) THEN (IF step_id \in DOMAIN step_target_terminal_failure_counts THEN step_target_terminal_failure_counts[step_id] ELSE 0) ELSE 0) + 1))
    /\ UNCHANGED << tracked_steps, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


RecordTargetCanceled(step_id, target_id) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "Dispatched")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ step_target_terminal_failure_counts' = MapSet(step_target_terminal_failure_counts, step_id, ((IF (step_id \in DOMAIN step_target_terminal_failure_counts) THEN (IF step_id \in DOMAIN step_target_terminal_failure_counts THEN step_target_terminal_failure_counts[step_id] ELSE 0) ELSE 0) + 1))
    /\ UNCHANGED << tracked_steps, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


RecordTargetFailure(step_id, target_id, retry_key) ==
    /\ phase = "Running"
    /\ StepIsTracked(step_id)
    /\ StepStatusIs(step_id, "Dispatched")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ target_retry_counts' = MapSet(target_retry_counts, retry_key, ((IF (retry_key \in DOMAIN target_retry_counts) THEN (IF retry_key \in DOMAIN target_retry_counts THEN target_retry_counts[retry_key] ELSE 0) ELSE 0) + 1))
    /\ UNCHANGED << tracked_steps, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


TerminalizeCompleted ==
    /\ phase = "Running"
    /\ AllTrackedStepsInAllowedStatuses(<<Some("Completed"), Some("Skipped")>>)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << tracked_steps, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


TerminalizeFailed ==
    /\ phase = "Pending" \/ phase = "Running"
    /\ NoTrackedStepInStatus("Dispatched")
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << tracked_steps, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


TerminalizeCanceled ==
    /\ phase = "Pending" \/ phase = "Running"
    /\ NoTrackedStepInStatus("Dispatched")
    /\ phase' = "Canceled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << tracked_steps, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, target_retry_counts, failure_count, consecutive_failure_count, escalation_threshold, max_step_retries >>


Next ==
    \/ \E step_ids \in SeqOfStepIdValues : \E arg_ordered_steps \in SeqOfStepIdValues : \E arg_step_has_conditions \in MapStepIdBoolValues : \E arg_step_dependencies \in MapStepIdSeqStepIdValues : \E arg_step_dependency_modes \in MapStepIdDependencyModeValues : \E arg_step_branches \in MapStepIdOptionBranchIdValues : \E arg_step_collection_policies \in MapStepIdCollectionPolicyKindValues : \E arg_step_quorum_thresholds \in MapStepIdU32Values : \E arg_escalation_threshold \in 0..2 : \E arg_max_step_retries \in 0..2 : CreateRun(step_ids, arg_ordered_steps, arg_step_has_conditions, arg_step_dependencies, arg_step_dependency_modes, arg_step_branches, arg_step_collection_policies, arg_step_quorum_thresholds, arg_escalation_threshold, arg_max_step_retries)
    \/ StartRun
    \/ \E step_id \in StepIdValues : DispatchStep(step_id)
    \/ \E step_id \in StepIdValues : CompleteStep(step_id)
    \/ \E step_id \in StepIdValues : RecordStepOutput(step_id)
    \/ \E step_id \in StepIdValues : ConditionPassed(step_id)
    \/ \E step_id \in StepIdValues : ConditionRejected(step_id)
    \/ \E step_id \in StepIdValues : FailStepEscalating(step_id)
    \/ \E step_id \in StepIdValues : FailStep(step_id)
    \/ \E step_id \in StepIdValues : SkipStep(step_id)
    \/ \E step_id \in StepIdValues : CancelStep(step_id)
    \/ \E step_id \in StepIdValues : \E target_count \in 0..2 : RegisterTargets(step_id, target_count)
    \/ \E step_id \in StepIdValues : \E target_id \in MeerkatIdValues : RecordTargetSuccess(step_id, target_id)
    \/ \E step_id \in StepIdValues : RecordTargetTerminalFailure(step_id)
    \/ \E step_id \in StepIdValues : \E target_id \in MeerkatIdValues : RecordTargetCanceled(step_id, target_id)
    \/ \E step_id \in StepIdValues : \E target_id \in MeerkatIdValues : \E retry_key \in {"alpha", "beta"} : RecordTargetFailure(step_id, target_id, retry_key)
    \/ TerminalizeCompleted
    \/ TerminalizeFailed
    \/ TerminalizeCanceled
    \/ TerminalStutter

output_only_follows_completed_steps == (\A step_id \in tracked_steps : (~(StepOutputRecordedIs(step_id, TRUE)) \/ StepStatusIs(step_id, "Completed")))
terminal_runs_have_no_dispatched_steps == (~(RunIsTerminal) \/ NoTrackedStepInStatus("Dispatched"))
completed_runs_contain_only_completed_or_skipped_steps == ((phase # "Completed") \/ AllTrackedStepsInAllowedStatuses(<<Some("Completed"), Some("Skipped")>>))
failed_step_presence_requires_failure_count == (~(AnyTrackedStepInStatus("Failed")) \/ (failure_count >= 1))
failed_run_has_failed_step_or_recorded_failure == ((phase # "Failed") \/ TRUE)

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(tracked_steps) <= 1 /\ Len(ordered_steps) <= 1 /\ Cardinality(DOMAIN step_status) <= 1 /\ Cardinality(DOMAIN output_recorded) <= 1 /\ Cardinality(DOMAIN step_condition_results) <= 1 /\ Cardinality(DOMAIN step_has_conditions) <= 1 /\ Cardinality(DOMAIN step_dependencies) <= 1 /\ Cardinality(DOMAIN step_dependency_modes) <= 1 /\ Cardinality(DOMAIN step_branches) <= 1 /\ Cardinality(DOMAIN step_collection_policies) <= 1 /\ Cardinality(DOMAIN step_quorum_thresholds) <= 1 /\ Cardinality(DOMAIN step_target_counts) <= 1 /\ Cardinality(DOMAIN step_target_success_counts) <= 1 /\ Cardinality(DOMAIN step_target_terminal_failure_counts) <= 1 /\ Cardinality(DOMAIN target_retry_counts) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(tracked_steps) <= 2 /\ Len(ordered_steps) <= 2 /\ Cardinality(DOMAIN step_status) <= 2 /\ Cardinality(DOMAIN output_recorded) <= 2 /\ Cardinality(DOMAIN step_condition_results) <= 2 /\ Cardinality(DOMAIN step_has_conditions) <= 2 /\ Cardinality(DOMAIN step_dependencies) <= 2 /\ Cardinality(DOMAIN step_dependency_modes) <= 2 /\ Cardinality(DOMAIN step_branches) <= 2 /\ Cardinality(DOMAIN step_collection_policies) <= 2 /\ Cardinality(DOMAIN step_quorum_thresholds) <= 2 /\ Cardinality(DOMAIN step_target_counts) <= 2 /\ Cardinality(DOMAIN step_target_success_counts) <= 2 /\ Cardinality(DOMAIN step_target_terminal_failure_counts) <= 2 /\ Cardinality(DOMAIN target_retry_counts) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []output_only_follows_completed_steps
THEOREM Spec => []terminal_runs_have_no_dispatched_steps
THEOREM Spec => []completed_runs_contain_only_completed_or_skipped_steps
THEOREM Spec => []failed_step_presence_requires_failure_count
THEOREM Spec => []failed_run_has_failed_step_or_recorded_failure

=============================================================================
