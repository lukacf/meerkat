---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for ScheduleLifecycleMachine.

CONSTANTS MisfirePolicyValues, MissingTargetPolicyValues, NatValues, OverlapPolicyValues, StringValues

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

VARIABLES phase, model_step_count, revision, trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy, planning_cursor_utc_ms, next_occurrence_ordinal

vars == << phase, model_step_count, revision, trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy, planning_cursor_utc_ms, next_occurrence_ordinal >>

Init ==
    /\ phase = "Active"
    /\ model_step_count = 0
    /\ revision = 1
    /\ trigger_key = "trigger-0"
    /\ target_binding_key = "target-0"
    /\ misfire_policy = "Skip"
    /\ overlap_policy = "SkipIfRunning"
    /\ missing_target_policy = "MarkMisfired"
    /\ planning_cursor_utc_ms = None
    /\ next_occurrence_ordinal = 0

TerminalStutter ==
    /\ phase = "Deleted"
    /\ UNCHANGED vars

ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ trigger_key' = arg_trigger_key
    /\ target_binding_key' = arg_target_binding_key
    /\ misfire_policy' = arg_misfire_policy
    /\ overlap_policy' = arg_overlap_policy
    /\ missing_target_policy' = arg_missing_target_policy
    /\ planning_cursor_utc_ms' = None
    /\ UNCHANGED << next_occurrence_ordinal >>


RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy) ==
    /\ phase = "Paused"
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ trigger_key' = arg_trigger_key
    /\ target_binding_key' = arg_target_binding_key
    /\ misfire_policy' = arg_misfire_policy
    /\ overlap_policy' = arg_overlap_policy
    /\ missing_target_policy' = arg_missing_target_policy
    /\ planning_cursor_utc_ms' = None
    /\ UNCHANGED << next_occurrence_ordinal >>


RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal) ==
    /\ phase = "Active"
    /\ (arg_next_occurrence_ordinal > 0)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ planning_cursor_utc_ms' = Some(arg_planning_cursor_utc_ms)
    /\ next_occurrence_ordinal' = arg_next_occurrence_ordinal
    /\ UNCHANGED << revision, trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy >>


RecordPlanningWindowPaused(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal) ==
    /\ phase = "Paused"
    /\ (arg_next_occurrence_ordinal > 0)
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ planning_cursor_utc_ms' = Some(arg_planning_cursor_utc_ms)
    /\ next_occurrence_ordinal' = arg_next_occurrence_ordinal
    /\ UNCHANGED << revision, trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy >>


PauseActive ==
    /\ phase = "Active"
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy, planning_cursor_utc_ms, next_occurrence_ordinal >>


ResumePaused ==
    /\ phase = "Paused"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy, planning_cursor_utc_ms, next_occurrence_ordinal >>


DeleteActive ==
    /\ phase = "Active"
    /\ phase' = "Deleted"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ planning_cursor_utc_ms' = None
    /\ UNCHANGED << trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy, next_occurrence_ordinal >>


DeletePaused ==
    /\ phase = "Paused"
    /\ phase' = "Deleted"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ planning_cursor_utc_ms' = None
    /\ UNCHANGED << trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy, next_occurrence_ordinal >>


Next ==
    \/ \E arg_trigger_key \in {"alpha", "beta"} : \E arg_target_binding_key \in {"alpha", "beta"} : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)
    \/ \E arg_trigger_key \in {"alpha", "beta"} : \E arg_target_binding_key \in {"alpha", "beta"} : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)
    \/ \E arg_planning_cursor_utc_ms \in 0..2 : \E arg_next_occurrence_ordinal \in 0..2 : RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal)
    \/ \E arg_planning_cursor_utc_ms \in 0..2 : \E arg_next_occurrence_ordinal \in 0..2 : RecordPlanningWindowPaused(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal)
    \/ PauseActive
    \/ ResumePaused
    \/ DeleteActive
    \/ DeletePaused
    \/ TerminalStutter

revision_is_positive == (revision > 0)
deleted_has_no_planning_cursor == ((phase # "Deleted") \/ (planning_cursor_utc_ms = None))
planning_cursor_requires_occurrence_progress == ((planning_cursor_utc_ms = None) \/ (next_occurrence_ordinal > 0))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []revision_is_positive
THEOREM Spec => []deleted_has_no_planning_cursor
THEOREM Spec => []planning_cursor_requires_occurrence_progress

=============================================================================
