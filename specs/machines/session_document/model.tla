---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for SessionDocumentMachine.

CONSTANTS BooleanValues, DurableHeadRelationValues, DurableTailRecoveryClassValues, DurableTailStopReasonValues, LiveAssistantPlaybackTerminalDispositionValues, LiveAssistantPlaybackTerminalObservationValues, LiveContextCommittedRowDispositionValues, LiveContextCommittedRowKindValues, LiveContextCommittedTextProvenanceValues, LiveSessionAuthorityKindValues, LiveSessionAuthorityReasonValues, LiveTranscriptReconciliationValues, NatValues, ObservedSessionTailKindValues, PendingContinuationDispositionValues, PendingContinuationPublicTerminalValues, RealtimeTranscriptLaneKindValues, RealtimeTranscriptMaterializeDecisionValues, RealtimeTranscriptRoleKindValues, RealtimeTranscriptStopReasonKindValues, RealtimeUserContentBlobFinalizeDispositionValues, RealtimeUserContentBlobRecoveryDispositionValues, RealtimeUserContentBlobStageDispositionValues, RealtimeUserContentIdentityDispositionValues, RecoveryCandidateIdValues, ResumeOverrideRejectionValues, ResumeProviderSelectionValues, ResumeSelfHostedSelectionValues, RunIdCardinalityValues, RuntimeCheckpointProjectionDispositionValues, SessionArchiveDispositionValues, SessionArchiveRuntimeObservationValues, SessionDocumentLifecycleMergeValues, SessionDocumentLifecycleValues, SessionFirstTurnPhaseValues, SessionIdValues, SessionInitialPromptStageDecisionValues, StringValues, TranscriptEditKindValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapSessionIdBoolValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in BOOLEAN }
MapSessionIdLiveTranscriptReconciliationValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in LiveTranscriptReconciliationValues }
MapSessionIdSessionDocumentLifecycleValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in SessionDocumentLifecycleValues }
MapSessionIdSessionFirstTurnPhaseValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in SessionFirstTurnPhaseValues }
MapSessionIdStringValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in StringValues }
MapSessionIdU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in NatValues }
OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
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

VARIABLES phase, model_step_count, session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index

vars == << phase, model_step_count, session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index >>

archive_should_retire_runtime(runtime_backed, durable_document_present, runtime_observation) == (runtime_backed /\ (runtime_observation # "QuiescentTerminal") /\ (IF durable_document_present THEN TRUE ELSE (runtime_observation = "RetirementRequired")))
store_projection_can_recover_authority(has_metadata, has_build_state, runtime_projection_quarantined) == (IF has_metadata THEN TRUE ELSE (IF has_build_state THEN TRUE ELSE runtime_projection_quarantined))
resume_provider_recompute_from_model(model_override_present, provider_override_present) == (model_override_present /\ (provider_override_present = FALSE))
resume_reject_provider_requires_model(provider_override_present, model_override_present) == (provider_override_present /\ (model_override_present = FALSE))
tail_has_pending_boundary(session_tail) == (IF (session_tail = "User") THEN TRUE ELSE (session_tail = "ToolResults"))
realtime_stop_reason_records_completion(stop_reason) == (stop_reason = "Other")
realtime_stop_reason_removes_completion(stop_reason) == (stop_reason = "ToolUse")
realtime_stop_reason_discards(stop_reason) == (stop_reason = "Cancelled")
realtime_should_mark_ready_after_write(response_completed, text_after_write_present) == (response_completed /\ text_after_write_present)
realtime_lane_accepts(item_has_text, current_lane, requested_lane) == (IF (current_lane = requested_lane) THEN TRUE ELSE (item_has_text = FALSE))
realtime_delta_is_duplicate(delta_id_present, delta_id_seen) == (delta_id_present /\ delta_id_seen)
should_store_initial_prompt(arg_phase, prompt_has_content) == ((arg_phase = "Pending") /\ prompt_has_content)
phase_allows_initial_turn_overrides(arg_phase) == (arg_phase = "Pending")
resume_reject_build_only_after_first_turn(has_build_only_overrides, first_turn_phase) == (has_build_only_overrides /\ (phase_allows_initial_turn_overrides(first_turn_phase) = FALSE))
has_effective_pending_boundary(session_tail, staged_tool_result_count) == (IF tail_has_pending_boundary(session_tail) THEN TRUE ELSE (staged_tool_result_count > 0))
resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) == ((resume_reject_provider_requires_model(provider_override_present, model_override_present) = FALSE) /\ (resume_reject_build_only_after_first_turn(has_build_only_overrides, first_turn_phase) = FALSE))

Init ==
    /\ phase = "Ready"
    /\ model_step_count = 0
    /\ session_first_turn_phase = [x \in {} |-> None]
    /\ session_pending_initial_prompt_present = [x \in {} |-> None]
    /\ session_pending_tool_results_count = [x \in {} |-> None]
    /\ session_lifecycle_terminal = [x \in {} |-> None]
    /\ session_live_channel_id = [x \in {} |-> None]
    /\ session_live_interaction_id = [x \in {} |-> None]
    /\ session_live_transcript_reconciliation = [x \in {} |-> None]
    /\ session_live_provisional_transcript_present = [x \in {} |-> None]
    /\ session_live_assistant_playback_response_id = [x \in {} |-> None]
    /\ session_live_assistant_playback_item_id = [x \in {} |-> None]
    /\ session_live_assistant_playback_content_index = [x \in {} |-> None]

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_1f5251ef33787a22 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index >>
UnchangedFrame_2807f6cd7f1c9b0f == UNCHANGED << session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index >>
UnchangedFrame_53eaf7d3d2b1af45 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present >>
UnchangedFrame_69253ab4c627acc7 == UNCHANGED << session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index >>
UnchangedFrame_71b61a788173b96a == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index >>
UnchangedFrame_9c9fa0dde5fc7140 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index >>
UnchangedFrame_a13d89f5f70b64b4 == UNCHANGED << session_first_turn_phase, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index >>
UnchangedFrame_a2f4711ff975b9d6 == UNCHANGED << session_first_turn_phase, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index >>
UnchangedFrame_b4248dd3143f6dbd == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>
UnchangedFrame_ddf7dba6ec14e2c9 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index >>
UnchangedFrame_e42f554887323f9c == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index >>
UnchangedFrame_f9cb9fecd7ccb9c4 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index >>

MarkSessionInitialTurnPendingInactiveOrPending(session_id) ==
    /\ phase = "Ready"
    /\ (IF ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive") THEN TRUE ELSE ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Pending")
    /\ UnchangedFrame_2807f6cd7f1c9b0f


MarkSessionInitialTurnPendingConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


StartSessionInitialTurnPending(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Consumed")
    /\ UnchangedFrame_2807f6cd7f1c9b0f


StartSessionInitialTurnInactive(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


StartSessionInitialTurnConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveSessionFirstTurnOverridesAllowed(session_id) ==
    /\ phase = "Ready"
    /\ phase_allows_initial_turn_overrides((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveSessionFirstTurnOverridesDenied(session_id) ==
    /\ phase = "Ready"
    /\ (phase_allows_initial_turn_overrides((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None)) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


StageSessionInitialPromptStore(session_id, prompt_has_content) ==
    /\ phase = "Ready"
    /\ should_store_initial_prompt((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None), prompt_has_content)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, TRUE)
    /\ UnchangedFrame_a2f4711ff975b9d6


StageSessionInitialPromptClear(session_id, prompt_has_content) ==
    /\ phase = "Ready"
    /\ (should_store_initial_prompt((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None), prompt_has_content) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ UnchangedFrame_a2f4711ff975b9d6


StageSessionToolResults(session_id, result_count) ==
    /\ phase = "Ready"
    /\ (IF ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive") THEN TRUE ELSE (IF ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending") THEN TRUE ELSE ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, result_count)
    /\ UnchangedFrame_71b61a788173b96a


ConsumeSessionDeferredInputsPending(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Consumed")
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UnchangedFrame_69253ab4c627acc7


ConsumeSessionDeferredInputsInactive(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UnchangedFrame_a13d89f5f70b64b4


ConsumeSessionDeferredInputsConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UnchangedFrame_a13d89f5f70b64b4


RestoreSessionConsumedInputs(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ restore_first_turn_pending
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Pending")
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)
    /\ UnchangedFrame_69253ab4c627acc7


RestoreSessionConsumedInputsNoPhaseRollback(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ (restore_first_turn_pending = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)
    /\ UnchangedFrame_a13d89f5f70b64b4


RecoverSessionFirstTurnPhase(session_id, arg_phase, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ (IF (arg_phase = "Inactive") THEN TRUE ELSE (IF (arg_phase = "Pending") THEN TRUE ELSE (arg_phase = "Consumed")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, arg_phase)
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)
    /\ UnchangedFrame_69253ab4c627acc7


ResolveRealtimeItemObservedDiscardedAssistant(role, response_discarded) ==
    /\ phase = "Ready"
    /\ ((role = "Assistant") /\ response_discarded)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeItemObservedPresent(role, response_discarded) ==
    /\ phase = "Ready"
    /\ (IF (role # "Assistant") THEN TRUE ELSE (response_discarded = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeItemSkipped ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserTranscriptFinalEmpty(text_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (text_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserTranscriptFinalStore(text_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (text_present /\ segment_empty)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserTranscriptFinalReplayOrConflict(text_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (text_present /\ (segment_empty = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentIdentityInvalid(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (IF (identity_fields_valid = FALSE) THEN TRUE ELSE ((key_tombstoned = FALSE) /\ predecessor_materialized /\ (existing_identity_present = FALSE) /\ target_item_id_available /\ reducer_commit_proof_required /\ (reducer_commit_proof_present = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentIdentityUnmaterializedPredecessor(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (identity_fields_valid /\ (key_tombstoned = FALSE) /\ (predecessor_materialized = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentIdentityConflict(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (identity_fields_valid /\ (IF key_tombstoned THEN TRUE ELSE (predecessor_materialized /\ (IF (existing_identity_present /\ (existing_payload_matches = FALSE)) THEN TRUE ELSE ((existing_identity_present = FALSE) /\ (target_item_id_available = FALSE))))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentIdentityReplay(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (identity_fields_valid /\ (key_tombstoned = FALSE) /\ predecessor_materialized /\ existing_identity_present /\ existing_payload_matches)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentIdentityCommitNew(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (identity_fields_valid /\ (key_tombstoned = FALSE) /\ predecessor_materialized /\ (existing_identity_present = FALSE) /\ target_item_id_available /\ (IF (reducer_commit_proof_required = FALSE) THEN TRUE ELSE reducer_commit_proof_present))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentBlobStageNew(pending_present, pending_matches_request) ==
    /\ phase = "Ready"
    /\ (pending_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentBlobStageReuseExact(pending_present, pending_matches_request) ==
    /\ phase = "Ready"
    /\ (pending_present /\ pending_matches_request)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentBlobStageRejectOccupied(pending_present, pending_matches_request) ==
    /\ phase = "Ready"
    /\ (pending_present /\ (pending_matches_request = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentBlobRecoveryNone(pending_present, request_matches_pending, pending_blob_valid) ==
    /\ phase = "Ready"
    /\ (pending_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentBlobRecoveryExact(pending_present, request_matches_pending, pending_blob_valid) ==
    /\ phase = "Ready"
    /\ (pending_present /\ request_matches_pending)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentBlobRecoveryCommitVerified(pending_present, request_matches_pending, pending_blob_valid) ==
    /\ phase = "Ready"
    /\ (pending_present /\ (request_matches_pending = FALSE) /\ pending_blob_valid)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentBlobRecoveryClearInvalid(pending_present, request_matches_pending, pending_blob_valid) ==
    /\ phase = "Ready"
    /\ (pending_present /\ (request_matches_pending = FALSE) /\ (pending_blob_valid = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentBlobFinalizeNone(pending_present, pending_matches_committed) ==
    /\ phase = "Ready"
    /\ (pending_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentBlobFinalizeClearCommitted(pending_present, pending_matches_committed) ==
    /\ phase = "Ready"
    /\ (pending_present /\ pending_matches_committed)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentBlobFinalizeRejectMismatch(pending_present, pending_matches_committed) ==
    /\ phase = "Ready"
    /\ (pending_present /\ (pending_matches_committed = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentFinalEmpty(content_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (content_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentFinalStore(content_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (content_present /\ segment_empty)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeUserContentFinalReplayOrConflict(content_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (content_present /\ (segment_empty = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantDeltaInvalidOrDuplicate(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (IF (response_id_valid = FALSE) THEN TRUE ELSE realtime_delta_is_duplicate(delta_id_present, delta_id_seen))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantDeltaDiscarded(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ response_discarded)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantDeltaLaneConflict(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (realtime_delta_is_duplicate(delta_id_present, delta_id_seen) = FALSE) /\ (realtime_lane_accepts(item_has_text, current_lane, requested_lane) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantDeltaAccepted(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (realtime_delta_is_duplicate(delta_id_present, delta_id_seen) = FALSE) /\ realtime_lane_accepts(item_has_text, current_lane, requested_lane))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantReplacementInvalid(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantReplacementDiscarded(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ response_discarded)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantReplacementLocked(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ item_materialized)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantReplacementLaneConflict(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (item_materialized = FALSE) /\ (realtime_lane_accepts(item_has_text, current_lane, requested_lane) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantReplacementAccepted(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (item_materialized = FALSE) /\ realtime_lane_accepts(item_has_text, current_lane, requested_lane))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantTurnCompletedInvalid(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantTurnCompletedDiscard(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (IF response_discarded THEN TRUE ELSE realtime_stop_reason_discards(stop_reason)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantTurnCompletedToolUse(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ realtime_stop_reason_removes_completion(stop_reason))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantTurnCompletedRecord(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ realtime_stop_reason_records_completion(stop_reason))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantTurnInterruptedInvalid(response_id_valid) ==
    /\ phase = "Ready"
    /\ (response_id_valid = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeAssistantTurnInterruptedValid(response_id_valid) ==
    /\ phase = "Ready"
    /\ response_id_valid
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeMaterializeAlreadyDone(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ item_materialized
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeMaterializeWaitForPredecessor(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ (predecessor_materialized = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeMaterializeSkipped(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ item_skipped)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeMaterializeWaitForReadyText(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ (IF (item_ready = FALSE) THEN TRUE ELSE (item_text_present = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeMaterializeUser(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "User"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeMaterializeAssistant(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "Assistant") /\ response_id_present /\ completion_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRealtimeMaterializeAssistantMissingCompletion(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "Assistant") /\ (IF (response_id_present = FALSE) THEN TRUE ELSE (completion_present = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


AuthorizeRestoreRealtimeTranscriptState(item_count, first_seen_count, first_seen_unique_count, every_item_has_order_entry, every_order_entry_has_item, all_materialized_predecessor_references_exist, no_self_predecessor_references, causal_graph_acyclic, all_materialized_items_have_materialized_ancestry, all_identity_fields_valid, all_user_content_identity_keys_match, all_user_content_identity_fields_valid, all_user_content_identity_item_ids_unique, all_user_content_identities_reference_materialized_user_items, all_user_content_tombstones_valid, user_content_identities_and_tombstones_disjoint, pending_user_content_blob_fields_valid, pending_user_content_blob_uncommitted, all_delta_ids_valid, all_completion_response_ids_valid, all_discarded_response_ids_valid, all_materialized_items_were_ready_or_skipped, all_assistant_items_have_response_unless_skipped, all_ready_assistant_items_have_completion_or_are_skipped, all_materialized_assistant_completions_consumed, all_completed_assistant_text_items_are_ready_or_materialized_or_skipped, all_discarded_assistant_items_are_skipped_or_materialized) ==
    /\ phase = "Ready"
    /\ ((item_count = first_seen_count) /\ (first_seen_count = first_seen_unique_count) /\ every_item_has_order_entry /\ every_order_entry_has_item /\ all_materialized_predecessor_references_exist /\ no_self_predecessor_references /\ causal_graph_acyclic /\ all_materialized_items_have_materialized_ancestry /\ all_identity_fields_valid /\ all_user_content_identity_keys_match /\ all_user_content_identity_fields_valid /\ all_user_content_identity_item_ids_unique /\ all_user_content_identities_reference_materialized_user_items /\ all_user_content_tombstones_valid /\ user_content_identities_and_tombstones_disjoint /\ pending_user_content_blob_fields_valid /\ pending_user_content_blob_uncommitted /\ all_delta_ids_valid /\ all_completion_response_ids_valid /\ all_discarded_response_ids_valid /\ all_materialized_items_were_ready_or_skipped /\ all_assistant_items_have_response_unless_skipped /\ all_ready_assistant_items_have_completion_or_are_skipped /\ all_materialized_assistant_completions_consumed /\ all_completed_assistant_text_items_are_ready_or_materialized_or_skipped /\ all_discarded_assistant_items_are_skipped_or_materialized)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


AdmitLiveInteractionTranscript(session_id, channel_id, interaction_id) ==
    /\ phase = "Ready"
    /\ ((channel_id # "") /\ (interaction_id # "") /\ ~((session_id \in DOMAIN session_live_interaction_id)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_channel_id' = MapSet(session_live_channel_id, session_id, channel_id)
    /\ session_live_interaction_id' = MapSet(session_live_interaction_id, session_id, interaction_id)
    /\ session_live_transcript_reconciliation' = MapSet(session_live_transcript_reconciliation, session_id, "Provisional")
    /\ session_live_provisional_transcript_present' = MapSet(session_live_provisional_transcript_present, session_id, FALSE)
    /\ UnchangedFrame_f9cb9fecd7ccb9c4


StageLiveProvisionalUserTranscript(session_id, channel_id, interaction_id, provisional_has_content) ==
    /\ phase = "Ready"
    /\ ((provisional_has_content = TRUE) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_transcript_reconciliation) THEN Some((IF session_id \in DOMAIN session_live_transcript_reconciliation THEN session_live_transcript_reconciliation[session_id] ELSE "None")) ELSE None) = Some("Provisional")) /\ ((IF (session_id \in DOMAIN session_live_provisional_transcript_present) THEN Some((IF session_id \in DOMAIN session_live_provisional_transcript_present THEN session_live_provisional_transcript_present[session_id] ELSE FALSE)) ELSE None) = Some(FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_provisional_transcript_present' = MapSet(session_live_provisional_transcript_present, session_id, TRUE)
    /\ UnchangedFrame_ddf7dba6ec14e2c9


ReconcileLiveFinalUserTranscript(session_id, channel_id, interaction_id, reconciliation) ==
    /\ phase = "Ready"
    /\ ((IF (reconciliation = "Committed") THEN TRUE ELSE (reconciliation = "Missing")) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_transcript_reconciliation) THEN Some((IF session_id \in DOMAIN session_live_transcript_reconciliation THEN session_live_transcript_reconciliation[session_id] ELSE "None")) ELSE None) = Some("Provisional")) /\ (IF ((reconciliation = "Committed") /\ ((IF (session_id \in DOMAIN session_live_provisional_transcript_present) THEN Some((IF session_id \in DOMAIN session_live_provisional_transcript_present THEN session_live_provisional_transcript_present[session_id] ELSE FALSE)) ELSE None) = Some(TRUE))) THEN TRUE ELSE (reconciliation # "Committed")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_transcript_reconciliation' = MapSet(session_live_transcript_reconciliation, session_id, reconciliation)
    /\ UnchangedFrame_1f5251ef33787a22


CompleteLiveInteractionTranscript(session_id, channel_id, interaction_id) ==
    /\ phase = "Ready"
    /\ (((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ (IF ((IF (session_id \in DOMAIN session_live_transcript_reconciliation) THEN Some((IF session_id \in DOMAIN session_live_transcript_reconciliation THEN session_live_transcript_reconciliation[session_id] ELSE "None")) ELSE None) # Some("Provisional")) THEN TRUE ELSE ((IF (session_id \in DOMAIN session_live_provisional_transcript_present) THEN Some((IF session_id \in DOMAIN session_live_provisional_transcript_present THEN session_live_provisional_transcript_present[session_id] ELSE FALSE)) ELSE None) = Some(FALSE))) /\ ~((session_id \in DOMAIN session_live_assistant_playback_response_id)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_channel_id' = MapRemove(session_live_channel_id, session_id)
    /\ session_live_interaction_id' = MapRemove(session_live_interaction_id, session_id)
    /\ session_live_transcript_reconciliation' = MapRemove(session_live_transcript_reconciliation, session_id)
    /\ session_live_provisional_transcript_present' = MapRemove(session_live_provisional_transcript_present, session_id)
    /\ UnchangedFrame_f9cb9fecd7ccb9c4


AdmitLiveAssistantPlaybackTarget(session_id, channel_id, interaction_id, response_id, item_id, content_index) ==
    /\ phase = "Ready"
    /\ ((response_id # "") /\ (item_id # "") /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ~((session_id \in DOMAIN session_live_assistant_playback_response_id)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_playback_response_id' = MapSet(session_live_assistant_playback_response_id, session_id, response_id)
    /\ session_live_assistant_playback_item_id' = MapSet(session_live_assistant_playback_item_id, session_id, item_id)
    /\ session_live_assistant_playback_content_index' = MapSet(session_live_assistant_playback_content_index, session_id, content_index)
    /\ UnchangedFrame_53eaf7d3d2b1af45


RecoverLiveAssistantPlaybackTarget(session_id, channel_id, interaction_id, response_id, item_id, content_index) ==
    /\ phase = "Ready"
    /\ ((channel_id # "") /\ (interaction_id # "") /\ (response_id # "") /\ (item_id # "") /\ ~((session_id \in DOMAIN session_live_interaction_id)) /\ ~((session_id \in DOMAIN session_live_assistant_playback_response_id)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_channel_id' = MapSet(session_live_channel_id, session_id, channel_id)
    /\ session_live_interaction_id' = MapSet(session_live_interaction_id, session_id, interaction_id)
    /\ session_live_transcript_reconciliation' = MapSet(session_live_transcript_reconciliation, session_id, "Missing")
    /\ session_live_provisional_transcript_present' = MapSet(session_live_provisional_transcript_present, session_id, FALSE)
    /\ session_live_assistant_playback_response_id' = MapSet(session_live_assistant_playback_response_id, session_id, response_id)
    /\ session_live_assistant_playback_item_id' = MapSet(session_live_assistant_playback_item_id, session_id, item_id)
    /\ session_live_assistant_playback_content_index' = MapSet(session_live_assistant_playback_content_index, session_id, content_index)
    /\ UnchangedFrame_b4248dd3143f6dbd


ResolveLiveAssistantPlaybackOnChannelClose(session_id, channel_id, interaction_id, response_id, item_id, content_index) ==
    /\ phase = "Ready"
    /\ (((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_channel_id' = MapRemove(session_live_channel_id, session_id)
    /\ session_live_interaction_id' = MapRemove(session_live_interaction_id, session_id)
    /\ session_live_transcript_reconciliation' = MapRemove(session_live_transcript_reconciliation, session_id)
    /\ session_live_provisional_transcript_present' = MapRemove(session_live_provisional_transcript_present, session_id)
    /\ session_live_assistant_playback_response_id' = MapRemove(session_live_assistant_playback_response_id, session_id)
    /\ session_live_assistant_playback_item_id' = MapRemove(session_live_assistant_playback_item_id, session_id)
    /\ session_live_assistant_playback_content_index' = MapRemove(session_live_assistant_playback_content_index, session_id)
    /\ UnchangedFrame_b4248dd3143f6dbd


ResolveLiveAssistantPlaybackComplete(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest, authoritative_assistant_final, observation, reported_prefix_chars, reported_prefix_digest, reported_prefix_matches_authoritative) ==
    /\ phase = "Ready"
    /\ ((observation = "PlaybackComplete") /\ (authoritative_assistant_final = TRUE) /\ (authoritative_assistant_chars > 0) /\ (authoritative_text_digest # "") /\ (reported_prefix_chars = 0) /\ (reported_prefix_digest = "") /\ (reported_prefix_matches_authoritative = FALSE) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_playback_response_id' = MapRemove(session_live_assistant_playback_response_id, session_id)
    /\ session_live_assistant_playback_item_id' = MapRemove(session_live_assistant_playback_item_id, session_id)
    /\ session_live_assistant_playback_content_index' = MapRemove(session_live_assistant_playback_content_index, session_id)
    /\ UnchangedFrame_53eaf7d3d2b1af45


ResolveLiveAssistantPlaybackReportedPrefix(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest, authoritative_assistant_final, observation, reported_prefix_chars, reported_prefix_digest, reported_prefix_matches_authoritative) ==
    /\ phase = "Ready"
    /\ ((observation = "ReportedPrefix") /\ (authoritative_text_digest # "") /\ (reported_prefix_digest # "") /\ (reported_prefix_matches_authoritative = TRUE) /\ (reported_prefix_chars <= authoritative_assistant_chars) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_playback_response_id' = MapRemove(session_live_assistant_playback_response_id, session_id)
    /\ session_live_assistant_playback_item_id' = MapRemove(session_live_assistant_playback_item_id, session_id)
    /\ session_live_assistant_playback_content_index' = MapRemove(session_live_assistant_playback_content_index, session_id)
    /\ UnchangedFrame_53eaf7d3d2b1af45


ResolveLiveAssistantPlaybackUnmeasured(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest, authoritative_assistant_final, observation, reported_prefix_chars, reported_prefix_digest, reported_prefix_matches_authoritative) ==
    /\ phase = "Ready"
    /\ ((observation = "Unmeasured") /\ (reported_prefix_chars = 0) /\ (reported_prefix_digest = "") /\ (reported_prefix_matches_authoritative = FALSE) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_playback_response_id' = MapRemove(session_live_assistant_playback_response_id, session_id)
    /\ session_live_assistant_playback_item_id' = MapRemove(session_live_assistant_playback_item_id, session_id)
    /\ session_live_assistant_playback_content_index' = MapRemove(session_live_assistant_playback_content_index, session_id)
    /\ UnchangedFrame_53eaf7d3d2b1af45


ClassifyLiveContextCommittedRow(session_id, canonical_row_sequence, row_kind, provenance, content_digest, store_commit_authority) ==
    /\ phase = "Ready"
    /\ ((canonical_row_sequence > 0) /\ (content_digest # "") /\ (store_commit_authority # ""))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


AuthorizeSessionMetadataPersist(schema_version, model_present) ==
    /\ phase = "Ready"
    /\ ((schema_version > 0) /\ (model_present = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


AuthorizeSessionBuildStatePersist(mob_tool_authority_context_present, mob_tool_authority_context_generated) ==
    /\ phase = "Ready"
    /\ (IF (mob_tool_authority_context_present = FALSE) THEN TRUE ELSE (mob_tool_authority_context_generated = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


RestoreSessionBuildState ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolvePendingContinuationWithBoundary(session_tail, staged_tool_result_count) ==
    /\ phase = "Ready"
    /\ has_effective_pending_boundary(session_tail, staged_tool_result_count)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolvePendingContinuationWithoutBoundary(session_tail, staged_tool_result_count) ==
    /\ phase = "Ready"
    /\ (has_effective_pending_boundary(session_tail, staged_tool_result_count) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


AuthorizeSessionResumeOverridesRejectProviderRequiresModel(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ resume_reject_provider_requires_model(provider_override_present, model_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ ((resume_reject_provider_requires_model(provider_override_present, model_override_present) = FALSE) /\ resume_reject_build_only_after_first_turn(has_build_only_overrides, first_turn_phase))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


AuthorizeSessionResumeOverridesAcceptRecomputeProvider(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ resume_provider_recompute_from_model(model_override_present, provider_override_present) /\ (self_hosted_server_override_present = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


AuthorizeSessionResumeOverridesAcceptRecomputeProviderWithSelfHostedOverride(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ resume_provider_recompute_from_model(model_override_present, provider_override_present) /\ self_hosted_server_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


AuthorizeSessionResumeOverridesAcceptUseOverride(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ provider_override_present /\ (self_hosted_server_override_present = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


AuthorizeSessionResumeOverridesAcceptUseOverrideWithSelfHostedOverride(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ provider_override_present /\ self_hosted_server_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


AuthorizeSessionResumeOverridesAcceptRetainStored(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ (provider_override_present = FALSE) /\ (self_hosted_server_override_present = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


AuthorizeSessionResumeOverridesAcceptRetainStoredWithSelfHostedOverride(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ (provider_override_present = FALSE) /\ self_hosted_server_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ClassifyLiveSessionAuthorityLive(stored_transcript_diverged, live_has_uncommitted_transcript, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_transcript_diverged = FALSE) /\ (live_has_uncommitted_transcript = FALSE) /\ (stored_is_archived = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ClassifyLiveSessionAuthorityDurableArchived(stored_transcript_diverged, live_has_uncommitted_transcript, stored_is_archived) ==
    /\ phase = "Ready"
    /\ (stored_is_archived = TRUE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ClassifyLiveSessionAuthorityDurableUncommitted(stored_transcript_diverged, live_has_uncommitted_transcript, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_is_archived = FALSE) /\ (live_has_uncommitted_transcript = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ClassifyLiveSessionAuthorityDurableRevision(stored_transcript_diverged, live_has_uncommitted_transcript, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_is_archived = FALSE) /\ (live_has_uncommitted_transcript = FALSE) /\ (stored_transcript_diverged = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


RecoverSessionFromStoreAuthorized(session_id, has_metadata, has_build_state, runtime_projection_quarantined) ==
    /\ phase = "Ready"
    /\ store_projection_can_recover_authority(has_metadata, has_build_state, runtime_projection_quarantined)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


RecoverSessionFromStoreUnrecoverable(session_id, has_metadata, has_build_state, runtime_projection_quarantined) ==
    /\ phase = "Ready"
    /\ (store_projection_can_recover_authority(has_metadata, has_build_state, runtime_projection_quarantined) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ClassifyDurableTailCompleted(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, messages_after_terminal) ==
    /\ phase = "Ready"
    /\ ((relation = "VerifiedStrictDescendant") /\ (run_id_cardinality = "SingleRunId") /\ (terminal_stop_reason = "EndTurn") /\ (dangling_tool_use_count = 0) /\ (orphan_tool_result_count = 0) /\ (messages_after_terminal = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ClassifyDurableTailRepairable(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, messages_after_terminal) ==
    /\ phase = "Ready"
    /\ ((relation = "VerifiedStrictDescendant") /\ (run_id_cardinality = "SingleRunId") /\ (dangling_tool_use_count = 0) /\ (orphan_tool_result_count = 0) /\ (messages_after_terminal = FALSE) /\ (IF (terminal_stop_reason = "ToolUse") THEN TRUE ELSE (terminal_stop_reason = "Absent")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ClassifyDurableTailAmbiguous(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, messages_after_terminal) ==
    /\ phase = "Ready"
    /\ (IF (relation # "VerifiedStrictDescendant") THEN TRUE ELSE (IF (run_id_cardinality # "SingleRunId") THEN TRUE ELSE (IF (orphan_tool_result_count # 0) THEN TRUE ELSE (IF (messages_after_terminal = TRUE) THEN TRUE ELSE (IF (terminal_stop_reason = "Other") THEN TRUE ELSE (dangling_tool_use_count # 0))))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRuntimeCheckpointProjectionActive(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Active")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveRuntimeCheckpointProjectionArchived(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Archived")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveSessionDocumentLifecycleMergeArchivedAbsorbing(session_id, authority_archived, candidate_archived) ==
    /\ phase = "Ready"
    /\ (IF (authority_archived = TRUE) THEN TRUE ELSE (candidate_archived = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ResolveSessionDocumentLifecycleMergeAuthority(session_id, authority_archived, candidate_archived) ==
    /\ phase = "Ready"
    /\ ((authority_archived = FALSE) /\ (candidate_archived = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ApplyPendingToolResults(session_id, result_count) ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


TranscriptEditFork(session_id, fork_or_rewrite_directive) ==
    /\ phase = "Ready"
    /\ (fork_or_rewrite_directive = "Fork")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


TranscriptEditRewrite(session_id, fork_or_rewrite_directive) ==
    /\ phase = "Ready"
    /\ (fork_or_rewrite_directive = "Rewrite")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


RecoverSessionLifecycleTerminal(session_id, terminal) ==
    /\ phase = "Ready"
    /\ (IF (terminal = "Active") THEN TRUE ELSE (terminal = "Archived"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_lifecycle_terminal' = MapSet(session_lifecycle_terminal, session_id, terminal)
    /\ UnchangedFrame_9c9fa0dde5fc7140


ReviveArchivedSessionDocument(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Archived")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_lifecycle_terminal' = MapSet(session_lifecycle_terminal, session_id, "Active")
    /\ UnchangedFrame_9c9fa0dde5fc7140


ArchiveSessionDocumentActive(session_id, runtime_backed, durable_document_present, runtime_observation) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Active")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_lifecycle_terminal' = MapSet(session_lifecycle_terminal, session_id, "Archived")
    /\ UnchangedFrame_9c9fa0dde5fc7140


ArchiveSessionDocumentAlreadyArchived(session_id, runtime_backed, durable_document_present, runtime_observation) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Archived")
    /\ (runtime_observation # "RetirementRequired")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


ArchiveSessionDocumentCompleteRetire(session_id, runtime_backed, durable_document_present, runtime_observation) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Archived")
    /\ (runtime_observation = "RetirementRequired")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_e42f554887323f9c


Next ==
    \/ \E session_id \in SessionIdValues : MarkSessionInitialTurnPendingInactiveOrPending(session_id)
    \/ \E session_id \in SessionIdValues : MarkSessionInitialTurnPendingConsumed(session_id)
    \/ \E session_id \in SessionIdValues : StartSessionInitialTurnPending(session_id)
    \/ \E session_id \in SessionIdValues : StartSessionInitialTurnInactive(session_id)
    \/ \E session_id \in SessionIdValues : StartSessionInitialTurnConsumed(session_id)
    \/ \E session_id \in SessionIdValues : ResolveSessionFirstTurnOverridesAllowed(session_id)
    \/ \E session_id \in SessionIdValues : ResolveSessionFirstTurnOverridesDenied(session_id)
    \/ \E session_id \in SessionIdValues : \E prompt_has_content \in BOOLEAN : StageSessionInitialPromptStore(session_id, prompt_has_content)
    \/ \E session_id \in SessionIdValues : \E prompt_has_content \in BOOLEAN : StageSessionInitialPromptClear(session_id, prompt_has_content)
    \/ \E session_id \in SessionIdValues : \E result_count \in 0..2 : StageSessionToolResults(session_id, result_count)
    \/ \E session_id \in SessionIdValues : ConsumeSessionDeferredInputsPending(session_id)
    \/ \E session_id \in SessionIdValues : ConsumeSessionDeferredInputsInactive(session_id)
    \/ \E session_id \in SessionIdValues : ConsumeSessionDeferredInputsConsumed(session_id)
    \/ \E session_id \in SessionIdValues : \E pending_initial_prompt_present \in BOOLEAN : \E pending_tool_result_message_count \in 0..2 : RestoreSessionConsumedInputs(session_id, TRUE, pending_initial_prompt_present, pending_tool_result_message_count)
    \/ \E session_id \in SessionIdValues : \E pending_initial_prompt_present \in BOOLEAN : \E pending_tool_result_message_count \in 0..2 : RestoreSessionConsumedInputsNoPhaseRollback(session_id, FALSE, pending_initial_prompt_present, pending_tool_result_message_count)
    \/ \E session_id \in SessionIdValues : \E arg_phase \in SessionFirstTurnPhaseValues : \E pending_initial_prompt_present \in BOOLEAN : \E pending_tool_result_message_count \in 0..2 : RecoverSessionFirstTurnPhase(session_id, arg_phase, pending_initial_prompt_present, pending_tool_result_message_count)
    \/ \E role \in RealtimeTranscriptRoleKindValues : ResolveRealtimeItemObservedDiscardedAssistant(role, TRUE)
    \/ \E role \in RealtimeTranscriptRoleKindValues : \E response_discarded \in BOOLEAN : ResolveRealtimeItemObservedPresent(role, response_discarded)
    \/ ResolveRealtimeItemSkipped
    \/ \E segment_empty \in BOOLEAN : \E segment_matches \in BOOLEAN : ResolveRealtimeUserTranscriptFinalEmpty(FALSE, segment_empty, segment_matches)
    \/ \E segment_matches \in BOOLEAN : ResolveRealtimeUserTranscriptFinalStore(TRUE, TRUE, segment_matches)
    \/ \E segment_matches \in BOOLEAN : ResolveRealtimeUserTranscriptFinalReplayOrConflict(TRUE, FALSE, segment_matches)
    \/ \E identity_fields_valid \in BOOLEAN : \E key_tombstoned \in BOOLEAN : \E predecessor_materialized \in BOOLEAN : \E existing_identity_present \in BOOLEAN : \E existing_payload_matches \in BOOLEAN : \E target_item_id_available \in BOOLEAN : \E reducer_commit_proof_required \in BOOLEAN : \E reducer_commit_proof_present \in BOOLEAN : ResolveRealtimeUserContentIdentityInvalid(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
    \/ \E existing_identity_present \in BOOLEAN : \E existing_payload_matches \in BOOLEAN : \E target_item_id_available \in BOOLEAN : \E reducer_commit_proof_required \in BOOLEAN : \E reducer_commit_proof_present \in BOOLEAN : ResolveRealtimeUserContentIdentityUnmaterializedPredecessor(TRUE, FALSE, FALSE, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
    \/ \E key_tombstoned \in BOOLEAN : \E predecessor_materialized \in BOOLEAN : \E existing_identity_present \in BOOLEAN : \E existing_payload_matches \in BOOLEAN : \E target_item_id_available \in BOOLEAN : \E reducer_commit_proof_required \in BOOLEAN : \E reducer_commit_proof_present \in BOOLEAN : ResolveRealtimeUserContentIdentityConflict(TRUE, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
    \/ \E target_item_id_available \in BOOLEAN : \E reducer_commit_proof_required \in BOOLEAN : \E reducer_commit_proof_present \in BOOLEAN : ResolveRealtimeUserContentIdentityReplay(TRUE, FALSE, TRUE, TRUE, TRUE, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
    \/ \E existing_payload_matches \in BOOLEAN : \E reducer_commit_proof_required \in BOOLEAN : \E reducer_commit_proof_present \in BOOLEAN : ResolveRealtimeUserContentIdentityCommitNew(TRUE, FALSE, TRUE, FALSE, existing_payload_matches, TRUE, reducer_commit_proof_required, reducer_commit_proof_present)
    \/ \E pending_matches_request \in BOOLEAN : ResolveRealtimeUserContentBlobStageNew(FALSE, pending_matches_request)
    \/ ResolveRealtimeUserContentBlobStageReuseExact(TRUE, TRUE)
    \/ ResolveRealtimeUserContentBlobStageRejectOccupied(TRUE, FALSE)
    \/ \E request_matches_pending \in BOOLEAN : \E pending_blob_valid \in BOOLEAN : ResolveRealtimeUserContentBlobRecoveryNone(FALSE, request_matches_pending, pending_blob_valid)
    \/ \E pending_blob_valid \in BOOLEAN : ResolveRealtimeUserContentBlobRecoveryExact(TRUE, TRUE, pending_blob_valid)
    \/ ResolveRealtimeUserContentBlobRecoveryCommitVerified(TRUE, FALSE, TRUE)
    \/ ResolveRealtimeUserContentBlobRecoveryClearInvalid(TRUE, FALSE, FALSE)
    \/ \E pending_matches_committed \in BOOLEAN : ResolveRealtimeUserContentBlobFinalizeNone(FALSE, pending_matches_committed)
    \/ ResolveRealtimeUserContentBlobFinalizeClearCommitted(TRUE, TRUE)
    \/ ResolveRealtimeUserContentBlobFinalizeRejectMismatch(TRUE, FALSE)
    \/ \E segment_empty \in BOOLEAN : \E segment_matches \in BOOLEAN : ResolveRealtimeUserContentFinalEmpty(FALSE, segment_empty, segment_matches)
    \/ \E segment_matches \in BOOLEAN : ResolveRealtimeUserContentFinalStore(TRUE, TRUE, segment_matches)
    \/ \E segment_matches \in BOOLEAN : ResolveRealtimeUserContentFinalReplayOrConflict(TRUE, FALSE, segment_matches)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E delta_id_present \in BOOLEAN : \E delta_id_seen \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_write_present \in BOOLEAN : ResolveRealtimeAssistantDeltaInvalidOrDuplicate(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
    \/ \E delta_id_present \in BOOLEAN : \E delta_id_seen \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_write_present \in BOOLEAN : ResolveRealtimeAssistantDeltaDiscarded(TRUE, TRUE, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
    \/ \E delta_id_present \in BOOLEAN : \E delta_id_seen \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_write_present \in BOOLEAN : ResolveRealtimeAssistantDeltaLaneConflict(TRUE, FALSE, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
    \/ \E delta_id_present \in BOOLEAN : \E delta_id_seen \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_write_present \in BOOLEAN : ResolveRealtimeAssistantDeltaAccepted(TRUE, FALSE, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
    \/ \E response_discarded \in BOOLEAN : \E item_materialized \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementInvalid(FALSE, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ \E item_materialized \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementDiscarded(TRUE, TRUE, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementLocked(TRUE, FALSE, TRUE, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementLaneConflict(TRUE, FALSE, FALSE, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementAccepted(TRUE, FALSE, FALSE, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ \E response_discarded \in BOOLEAN : \E stop_reason \in RealtimeTranscriptStopReasonKindValues : ResolveRealtimeAssistantTurnCompletedInvalid(FALSE, response_discarded, stop_reason)
    \/ \E response_discarded \in BOOLEAN : \E stop_reason \in RealtimeTranscriptStopReasonKindValues : ResolveRealtimeAssistantTurnCompletedDiscard(TRUE, response_discarded, stop_reason)
    \/ \E stop_reason \in RealtimeTranscriptStopReasonKindValues : ResolveRealtimeAssistantTurnCompletedToolUse(TRUE, FALSE, stop_reason)
    \/ \E stop_reason \in RealtimeTranscriptStopReasonKindValues : ResolveRealtimeAssistantTurnCompletedRecord(TRUE, FALSE, stop_reason)
    \/ ResolveRealtimeAssistantTurnInterruptedInvalid(FALSE)
    \/ ResolveRealtimeAssistantTurnInterruptedValid(TRUE)
    \/ \E predecessor_materialized \in BOOLEAN : \E item_skipped \in BOOLEAN : \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeAlreadyDone(TRUE, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E item_skipped \in BOOLEAN : \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeWaitForPredecessor(FALSE, FALSE, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeSkipped(FALSE, TRUE, TRUE, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeWaitForReadyText(FALSE, TRUE, FALSE, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeUser(FALSE, TRUE, FALSE, TRUE, TRUE, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E role \in RealtimeTranscriptRoleKindValues : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeAssistant(FALSE, TRUE, FALSE, TRUE, TRUE, role, TRUE, TRUE, completion_usage_consumed)
    \/ \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeAssistantMissingCompletion(FALSE, TRUE, FALSE, TRUE, TRUE, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E item_count \in 0..2 : \E first_seen_count \in 0..2 : \E first_seen_unique_count \in 0..2 : AuthorizeRestoreRealtimeTranscriptState(item_count, first_seen_count, first_seen_unique_count, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE)
    \/ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : AdmitLiveInteractionTranscript(session_id, channel_id, interaction_id)
    \/ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : StageLiveProvisionalUserTranscript(session_id, channel_id, interaction_id, TRUE)
    \/ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E reconciliation \in LiveTranscriptReconciliationValues : ReconcileLiveFinalUserTranscript(session_id, channel_id, interaction_id, reconciliation)
    \/ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : CompleteLiveInteractionTranscript(session_id, channel_id, interaction_id)
    \/ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : AdmitLiveAssistantPlaybackTarget(session_id, channel_id, interaction_id, response_id, item_id, content_index)
    \/ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : RecoverLiveAssistantPlaybackTarget(session_id, channel_id, interaction_id, response_id, item_id, content_index)
    \/ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : ResolveLiveAssistantPlaybackOnChannelClose(session_id, channel_id, interaction_id, response_id, item_id, content_index)
    \/ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E authoritative_assistant_chars \in 0..2 : \E authoritative_text_digest \in StringValues : \E observation \in LiveAssistantPlaybackTerminalObservationValues : \E reported_prefix_chars \in 0..2 : \E reported_prefix_digest \in StringValues : ResolveLiveAssistantPlaybackComplete(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest, TRUE, observation, reported_prefix_chars, reported_prefix_digest, FALSE)
    \/ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E authoritative_assistant_chars \in 0..2 : \E authoritative_text_digest \in StringValues : \E authoritative_assistant_final \in BOOLEAN : \E observation \in LiveAssistantPlaybackTerminalObservationValues : \E reported_prefix_chars \in 0..2 : \E reported_prefix_digest \in StringValues : ResolveLiveAssistantPlaybackReportedPrefix(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest, authoritative_assistant_final, observation, reported_prefix_chars, reported_prefix_digest, TRUE)
    \/ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E authoritative_assistant_chars \in 0..2 : \E authoritative_text_digest \in StringValues : \E authoritative_assistant_final \in BOOLEAN : \E observation \in LiveAssistantPlaybackTerminalObservationValues : \E reported_prefix_chars \in 0..2 : \E reported_prefix_digest \in StringValues : ResolveLiveAssistantPlaybackUnmeasured(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest, authoritative_assistant_final, observation, reported_prefix_chars, reported_prefix_digest, FALSE)
    \/ \E session_id \in SessionIdValues : \E canonical_row_sequence \in 0..2 : \E row_kind \in LiveContextCommittedRowKindValues : \E provenance \in LiveContextCommittedTextProvenanceValues : \E content_digest \in StringValues : \E store_commit_authority \in StringValues : ClassifyLiveContextCommittedRow(session_id, canonical_row_sequence, row_kind, provenance, content_digest, store_commit_authority)
    \/ \E schema_version \in 0..2 : AuthorizeSessionMetadataPersist(schema_version, TRUE)
    \/ \E mob_tool_authority_context_present \in BOOLEAN : \E mob_tool_authority_context_generated \in BOOLEAN : AuthorizeSessionBuildStatePersist(mob_tool_authority_context_present, mob_tool_authority_context_generated)
    \/ RestoreSessionBuildState
    \/ \E session_tail \in ObservedSessionTailKindValues : \E staged_tool_result_count \in 0..2 : ResolvePendingContinuationWithBoundary(session_tail, staged_tool_result_count)
    \/ \E session_tail \in ObservedSessionTailKindValues : \E staged_tool_result_count \in 0..2 : ResolvePendingContinuationWithoutBoundary(session_tail, staged_tool_result_count)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E self_hosted_server_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesRejectProviderRequiresModel(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E self_hosted_server_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptRecomputeProvider(provider_override_present, model_override_present, FALSE, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptRecomputeProviderWithSelfHostedOverride(provider_override_present, model_override_present, TRUE, has_build_only_overrides, first_turn_phase)
    \/ \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptUseOverride(TRUE, model_override_present, FALSE, has_build_only_overrides, first_turn_phase)
    \/ \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptUseOverrideWithSelfHostedOverride(TRUE, model_override_present, TRUE, has_build_only_overrides, first_turn_phase)
    \/ \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptRetainStored(FALSE, model_override_present, FALSE, has_build_only_overrides, first_turn_phase)
    \/ \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptRetainStoredWithSelfHostedOverride(FALSE, model_override_present, TRUE, has_build_only_overrides, first_turn_phase)
    \/ ClassifyLiveSessionAuthorityLive(FALSE, FALSE, FALSE)
    \/ \E stored_transcript_diverged \in BOOLEAN : \E live_has_uncommitted_transcript \in BOOLEAN : ClassifyLiveSessionAuthorityDurableArchived(stored_transcript_diverged, live_has_uncommitted_transcript, TRUE)
    \/ \E stored_transcript_diverged \in BOOLEAN : ClassifyLiveSessionAuthorityDurableUncommitted(stored_transcript_diverged, TRUE, FALSE)
    \/ ClassifyLiveSessionAuthorityDurableRevision(TRUE, FALSE, FALSE)
    \/ \E session_id \in SessionIdValues : \E has_metadata \in BOOLEAN : \E has_build_state \in BOOLEAN : \E runtime_projection_quarantined \in BOOLEAN : RecoverSessionFromStoreAuthorized(session_id, has_metadata, has_build_state, runtime_projection_quarantined)
    \/ \E session_id \in SessionIdValues : \E has_metadata \in BOOLEAN : \E has_build_state \in BOOLEAN : \E runtime_projection_quarantined \in BOOLEAN : RecoverSessionFromStoreUnrecoverable(session_id, has_metadata, has_build_state, runtime_projection_quarantined)
    \/ \E session_id \in SessionIdValues : \E candidate_id \in RecoveryCandidateIdValues : \E relation \in DurableHeadRelationValues : \E run_id_cardinality \in RunIdCardinalityValues : \E terminal_stop_reason \in DurableTailStopReasonValues : \E dangling_tool_use_count \in 0..2 : \E orphan_tool_result_count \in 0..2 : ClassifyDurableTailCompleted(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, FALSE)
    \/ \E session_id \in SessionIdValues : \E candidate_id \in RecoveryCandidateIdValues : \E relation \in DurableHeadRelationValues : \E run_id_cardinality \in RunIdCardinalityValues : \E terminal_stop_reason \in DurableTailStopReasonValues : \E dangling_tool_use_count \in 0..2 : \E orphan_tool_result_count \in 0..2 : ClassifyDurableTailRepairable(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, FALSE)
    \/ \E session_id \in SessionIdValues : \E candidate_id \in RecoveryCandidateIdValues : \E relation \in DurableHeadRelationValues : \E run_id_cardinality \in RunIdCardinalityValues : \E terminal_stop_reason \in DurableTailStopReasonValues : \E dangling_tool_use_count \in 0..2 : \E orphan_tool_result_count \in 0..2 : \E messages_after_terminal \in BOOLEAN : ClassifyDurableTailAmbiguous(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, messages_after_terminal)
    \/ \E session_id \in SessionIdValues : ResolveRuntimeCheckpointProjectionActive(session_id)
    \/ \E session_id \in SessionIdValues : ResolveRuntimeCheckpointProjectionArchived(session_id)
    \/ \E session_id \in SessionIdValues : \E authority_archived \in BOOLEAN : \E candidate_archived \in BOOLEAN : ResolveSessionDocumentLifecycleMergeArchivedAbsorbing(session_id, authority_archived, candidate_archived)
    \/ \E session_id \in SessionIdValues : ResolveSessionDocumentLifecycleMergeAuthority(session_id, FALSE, FALSE)
    \/ \E session_id \in SessionIdValues : \E result_count \in 0..2 : ApplyPendingToolResults(session_id, result_count)
    \/ \E session_id \in SessionIdValues : \E fork_or_rewrite_directive \in TranscriptEditKindValues : TranscriptEditFork(session_id, fork_or_rewrite_directive)
    \/ \E session_id \in SessionIdValues : \E fork_or_rewrite_directive \in TranscriptEditKindValues : TranscriptEditRewrite(session_id, fork_or_rewrite_directive)
    \/ \E session_id \in SessionIdValues : \E terminal \in SessionDocumentLifecycleValues : RecoverSessionLifecycleTerminal(session_id, terminal)
    \/ \E session_id \in SessionIdValues : ReviveArchivedSessionDocument(session_id)
    \/ \E session_id \in SessionIdValues : \E runtime_backed \in BOOLEAN : \E durable_document_present \in BOOLEAN : \E runtime_observation \in SessionArchiveRuntimeObservationValues : ArchiveSessionDocumentActive(session_id, runtime_backed, durable_document_present, runtime_observation)
    \/ \E session_id \in SessionIdValues : \E runtime_backed \in BOOLEAN : \E durable_document_present \in BOOLEAN : \E runtime_observation \in SessionArchiveRuntimeObservationValues : ArchiveSessionDocumentAlreadyArchived(session_id, runtime_backed, durable_document_present, runtime_observation)
    \/ \E session_id \in SessionIdValues : \E runtime_backed \in BOOLEAN : \E durable_document_present \in BOOLEAN : \E runtime_observation \in SessionArchiveRuntimeObservationValues : ArchiveSessionDocumentCompleteRetire(session_id, runtime_backed, durable_document_present, runtime_observation)

live_interaction_transcript_binding_is_complete == ((DOMAIN session_live_channel_id = DOMAIN session_live_interaction_id) /\ (DOMAIN session_live_channel_id = DOMAIN session_live_transcript_reconciliation) /\ (DOMAIN session_live_channel_id = DOMAIN session_live_provisional_transcript_present))
committed_live_transcript_had_provisional_admission == (\A session_id \in DOMAIN session_live_transcript_reconciliation : (IF ((IF (session_id \in DOMAIN session_live_transcript_reconciliation) THEN Some((IF session_id \in DOMAIN session_live_transcript_reconciliation THEN session_live_transcript_reconciliation[session_id] ELSE "None")) ELSE None) # Some("Committed")) THEN TRUE ELSE ((IF (session_id \in DOMAIN session_live_provisional_transcript_present) THEN Some((IF session_id \in DOMAIN session_live_provisional_transcript_present THEN session_live_provisional_transcript_present[session_id] ELSE FALSE)) ELSE None) = Some(TRUE))))
live_assistant_playback_target_is_complete_and_interaction_bound == ((DOMAIN session_live_assistant_playback_response_id = DOMAIN session_live_assistant_playback_item_id) /\ (DOMAIN session_live_assistant_playback_response_id = DOMAIN session_live_assistant_playback_content_index) /\ (\A session_id \in DOMAIN session_live_assistant_playback_response_id : ((session_id \in DOMAIN session_live_channel_id) /\ (session_id \in DOMAIN session_live_interaction_id))))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(DOMAIN session_first_turn_phase) <= 1 /\ Cardinality(DOMAIN session_pending_initial_prompt_present) <= 1 /\ Cardinality(DOMAIN session_pending_tool_results_count) <= 1 /\ Cardinality(DOMAIN session_lifecycle_terminal) <= 1 /\ Cardinality(DOMAIN session_live_channel_id) <= 1 /\ Cardinality(DOMAIN session_live_interaction_id) <= 1 /\ Cardinality(DOMAIN session_live_transcript_reconciliation) <= 1 /\ Cardinality(DOMAIN session_live_provisional_transcript_present) <= 1 /\ Cardinality(DOMAIN session_live_assistant_playback_response_id) <= 1 /\ Cardinality(DOMAIN session_live_assistant_playback_item_id) <= 1 /\ Cardinality(DOMAIN session_live_assistant_playback_content_index) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(DOMAIN session_first_turn_phase) <= 2 /\ Cardinality(DOMAIN session_pending_initial_prompt_present) <= 2 /\ Cardinality(DOMAIN session_pending_tool_results_count) <= 2 /\ Cardinality(DOMAIN session_lifecycle_terminal) <= 2 /\ Cardinality(DOMAIN session_live_channel_id) <= 2 /\ Cardinality(DOMAIN session_live_interaction_id) <= 2 /\ Cardinality(DOMAIN session_live_transcript_reconciliation) <= 2 /\ Cardinality(DOMAIN session_live_provisional_transcript_present) <= 2 /\ Cardinality(DOMAIN session_live_assistant_playback_response_id) <= 2 /\ Cardinality(DOMAIN session_live_assistant_playback_item_id) <= 2 /\ Cardinality(DOMAIN session_live_assistant_playback_content_index) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []live_interaction_transcript_binding_is_complete
THEOREM Spec => []committed_live_transcript_had_provisional_admission
THEOREM Spec => []live_assistant_playback_target_is_complete_and_interaction_bound

=============================================================================
