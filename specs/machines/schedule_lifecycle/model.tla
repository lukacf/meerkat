---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for ScheduleLifecycleMachine.

CONSTANTS MisfirePolicyValues, MissingTargetPolicyValues, NatValues, OccurrenceIdValues, OverlapPolicyValues, ScheduleIdValues, SetOfOccurrenceIdValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionU64Values == {None} \cup {Some(x) : x \in NatValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapIncrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) + amount ELSE map[x]]
MapDecrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) - amount ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
Count(seq, value) == Cardinality({i \in DOMAIN seq : seq[i] = value})
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, schedule_id, revision, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, planning_cursor_utc_ms, next_occurrence_ordinal, superseded_ack_ids

vars == << phase, model_step_count, schedule_id, revision, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, planning_cursor_utc_ms, next_occurrence_ordinal, superseded_ack_ids >>

Init ==
    /\ phase = "Active"
    /\ model_step_count = 0
    /\ schedule_id = "schedule-0"
    /\ revision = 1
    /\ trigger_key = "trigger-0"
    /\ target_binding_key = "target-0"
    /\ misfire_policy = "Skip"
    /\ misfire_policy_key = "misfire:skip"
    /\ overlap_policy = "SkipIfRunning"
    /\ overlap_policy_key = "overlap:skip_if_running"
    /\ missing_target_policy = "MarkMisfired"
    /\ missing_target_policy_key = "missing_target:mark_misfired"
    /\ planning_horizon_days = 30
    /\ planning_horizon_occurrences = 64
    /\ planning_cursor_utc_ms = None
    /\ next_occurrence_ordinal = 0
    /\ superseded_ack_ids = {}

TerminalStutter ==
    /\ phase = "Deleted"
    /\ UNCHANGED vars

CreateSchedule(arg_schedule_id, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ schedule_id' = arg_schedule_id
    /\ trigger_key' = arg_trigger_key
    /\ target_binding_key' = arg_target_binding_key
    /\ misfire_policy' = arg_misfire_policy
    /\ misfire_policy_key' = arg_misfire_policy_key
    /\ overlap_policy' = arg_overlap_policy
    /\ overlap_policy_key' = arg_overlap_policy_key
    /\ missing_target_policy' = arg_missing_target_policy
    /\ missing_target_policy_key' = arg_missing_target_policy_key
    /\ planning_horizon_days' = arg_planning_horizon_days
    /\ planning_horizon_occurrences' = arg_planning_horizon_occurrences
    /\ UNCHANGED << revision, planning_cursor_utc_ms, next_occurrence_ordinal, superseded_ack_ids >>


ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ trigger_key' = arg_trigger_key
    /\ target_binding_key' = arg_target_binding_key
    /\ misfire_policy' = arg_misfire_policy
    /\ misfire_policy_key' = arg_misfire_policy_key
    /\ overlap_policy' = arg_overlap_policy
    /\ overlap_policy_key' = arg_overlap_policy_key
    /\ missing_target_policy' = arg_missing_target_policy
    /\ missing_target_policy_key' = arg_missing_target_policy_key
    /\ planning_horizon_days' = arg_planning_horizon_days
    /\ planning_horizon_occurrences' = arg_planning_horizon_occurrences
    /\ planning_cursor_utc_ms' = None
    /\ UNCHANGED << schedule_id, next_occurrence_ordinal, superseded_ack_ids >>


RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences) ==
    /\ phase = "Paused"
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ trigger_key' = arg_trigger_key
    /\ target_binding_key' = arg_target_binding_key
    /\ misfire_policy' = arg_misfire_policy
    /\ misfire_policy_key' = arg_misfire_policy_key
    /\ overlap_policy' = arg_overlap_policy
    /\ overlap_policy_key' = arg_overlap_policy_key
    /\ missing_target_policy' = arg_missing_target_policy
    /\ missing_target_policy_key' = arg_missing_target_policy_key
    /\ planning_horizon_days' = arg_planning_horizon_days
    /\ planning_horizon_occurrences' = arg_planning_horizon_occurrences
    /\ planning_cursor_utc_ms' = None
    /\ UNCHANGED << schedule_id, next_occurrence_ordinal, superseded_ack_ids >>


UpdatePlanningConfigActive(arg_planning_horizon_days, arg_planning_horizon_occurrences) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ planning_horizon_days' = arg_planning_horizon_days
    /\ planning_horizon_occurrences' = arg_planning_horizon_occurrences
    /\ UNCHANGED << schedule_id, revision, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_cursor_utc_ms, next_occurrence_ordinal, superseded_ack_ids >>


UpdatePlanningConfigPaused(arg_planning_horizon_days, arg_planning_horizon_occurrences) ==
    /\ phase = "Paused"
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ planning_horizon_days' = arg_planning_horizon_days
    /\ planning_horizon_occurrences' = arg_planning_horizon_occurrences
    /\ UNCHANGED << schedule_id, revision, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_cursor_utc_ms, next_occurrence_ordinal, superseded_ack_ids >>


RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal) ==
    /\ phase = "Active"
    /\ (arg_next_occurrence_ordinal > 0)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ planning_cursor_utc_ms' = Some(arg_planning_cursor_utc_ms)
    /\ next_occurrence_ordinal' = arg_next_occurrence_ordinal
    /\ UNCHANGED << schedule_id, revision, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, superseded_ack_ids >>


SyncTargetSnapshotActive(arg_target_binding_key) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ target_binding_key' = arg_target_binding_key
    /\ UNCHANGED << schedule_id, revision, trigger_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, planning_cursor_utc_ms, next_occurrence_ordinal, superseded_ack_ids >>


SyncTargetSnapshotPaused(arg_target_binding_key) ==
    /\ phase = "Paused"
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ target_binding_key' = arg_target_binding_key
    /\ UNCHANGED << schedule_id, revision, trigger_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, planning_cursor_utc_ms, next_occurrence_ordinal, superseded_ack_ids >>


PauseActiveOrPaused(at_utc_ms) ==
    /\ phase = "Active" \/ phase = "Paused"
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << schedule_id, revision, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, planning_cursor_utc_ms, next_occurrence_ordinal, superseded_ack_ids >>


ResumeActiveOrPaused(at_utc_ms) ==
    /\ phase = "Active" \/ phase = "Paused"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << schedule_id, revision, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, planning_cursor_utc_ms, next_occurrence_ordinal, superseded_ack_ids >>


DeleteActive(at_utc_ms) ==
    /\ phase = "Active"
    /\ phase' = "Deleted"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ planning_cursor_utc_ms' = None
    /\ UNCHANGED << schedule_id, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, next_occurrence_ordinal, superseded_ack_ids >>


DeletePaused(at_utc_ms) ==
    /\ phase = "Paused"
    /\ phase' = "Deleted"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ planning_cursor_utc_ms' = None
    /\ UNCHANGED << schedule_id, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, next_occurrence_ordinal, superseded_ack_ids >>


DeleteDeleted(at_utc_ms) ==
    /\ phase = "Deleted"
    /\ phase' = "Deleted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << schedule_id, revision, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, planning_cursor_utc_ms, next_occurrence_ordinal, superseded_ack_ids >>


ConfirmOccurrencesSupersededActive(occurrence_id, superseding_revision) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ superseded_ack_ids' = (superseded_ack_ids \cup {occurrence_id})
    /\ UNCHANGED << schedule_id, revision, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, planning_cursor_utc_ms, next_occurrence_ordinal >>


ConfirmOccurrencesSupersededPaused(occurrence_id, superseding_revision) ==
    /\ phase = "Paused"
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ superseded_ack_ids' = (superseded_ack_ids \cup {occurrence_id})
    /\ UNCHANGED << schedule_id, revision, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, planning_cursor_utc_ms, next_occurrence_ordinal >>


ConfirmOccurrencesSupersededDeleted(occurrence_id, superseding_revision) ==
    /\ phase = "Deleted"
    /\ phase' = "Deleted"
    /\ model_step_count' = model_step_count + 1
    /\ superseded_ack_ids' = (superseded_ack_ids \cup {occurrence_id})
    /\ UNCHANGED << schedule_id, revision, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, planning_cursor_utc_ms, next_occurrence_ordinal >>


Next ==
    \/ \E arg_schedule_id \in ScheduleIdValues : \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : CreateSchedule(arg_schedule_id, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences)
    \/ \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences)
    \/ \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences)
    \/ \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : UpdatePlanningConfigActive(arg_planning_horizon_days, arg_planning_horizon_occurrences)
    \/ \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : UpdatePlanningConfigPaused(arg_planning_horizon_days, arg_planning_horizon_occurrences)
    \/ \E arg_planning_cursor_utc_ms \in 0..2 : \E arg_next_occurrence_ordinal \in 0..2 : RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal)
    \/ \E arg_target_binding_key \in StringValues : SyncTargetSnapshotActive(arg_target_binding_key)
    \/ \E arg_target_binding_key \in StringValues : SyncTargetSnapshotPaused(arg_target_binding_key)
    \/ \E at_utc_ms \in 0..2 : PauseActiveOrPaused(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : ResumeActiveOrPaused(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : DeleteActive(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : DeletePaused(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : DeleteDeleted(at_utc_ms)
    \/ \E occurrence_id \in OccurrenceIdValues : \E superseding_revision \in 0..2 : ConfirmOccurrencesSupersededActive(occurrence_id, superseding_revision)
    \/ \E occurrence_id \in OccurrenceIdValues : \E superseding_revision \in 0..2 : ConfirmOccurrencesSupersededPaused(occurrence_id, superseding_revision)
    \/ \E occurrence_id \in OccurrenceIdValues : \E superseding_revision \in 0..2 : ConfirmOccurrencesSupersededDeleted(occurrence_id, superseding_revision)
    \/ TerminalStutter

revision_is_positive == (revision > 0)
deleted_has_no_planning_cursor == ((phase # "Deleted") \/ (planning_cursor_utc_ms = None))
planning_cursor_requires_occurrence_progress == ((planning_cursor_utc_ms = None) \/ (next_occurrence_ordinal > 0))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(superseded_ack_ids) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(superseded_ack_ids) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []revision_is_positive
THEOREM Spec => []deleted_has_no_planning_cursor
THEOREM Spec => []planning_cursor_requires_occurrence_progress

=============================================================================
